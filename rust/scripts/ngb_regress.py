#!/usr/bin/env python3
# NGBoost experiment, step 5: an instance-level predictive DISTRIBUTION of the search effort, which is what NGBoost adds over a point predictor.
#
# The target is log10(conflicts + 1) of an instance under a configuration (base, target phases, vivification); the features are the instance-level features of
# ngb_common.features and the configuration (one-hot). A Normal on the log scale is a LogNormal on the conflicts. Held out by instance (grouped k-fold: the three
# configurations of an instance are never split), and by family (trained on the other families).
# Models: ngb (NGBRegressor, Normal; the number of stages chosen by early stopping on an inner split by instance), gbm+const (sklearn gradient boosting for the mean,
# one constant sigma from inner out-of-fold residuals), gbm quantile (sklearn gradient boosting with the quantile loss, intervals only), const (the training mean and deviation).
# Then two uses of the distribution, computed from the recorded runs: choosing the configuration per instance (by the predicted mean, or risk-aware by the predicted 90% quantile),
# and setting a conflict budget from a predicted quantile.
#
# Usage: ngb_regress.py --work DIR [--folds K]
#   Reads DIR/instances.json; writes DIR/tables/4_regression.md and DIR/regress.json.
import argparse, json, math, os, sys, warnings

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from scipy.stats import norm
from sklearn.ensemble import GradientBoostingRegressor
from sklearn.model_selection import GroupKFold, GroupShuffleSplit
from sklearn.tree import DecisionTreeRegressor
from ngboost import NGBRegressor
from ngboost.distns import Normal
from ngb_common import PER_INST

CONFIGS = ["base", "target", "vivify"]
QS = [0.05, 0.25, 0.5, 0.75, 0.95]

def new_gbm(**kw):
	return GradientBoostingRegressor(n_estimators=200, learning_rate=0.05, max_depth=3, subsample=0.8, random_state=0, **kw)

def new_ngb(n):
	return NGBRegressor(Dist=Normal, n_estimators=n, learning_rate=0.03, minibatch_frac=1.0, Base=DecisionTreeRegressor(criterion="friedman_mse", max_depth=3), verbose=False, random_state=0)

# Returns {model: (mu, sigma)} for the test rows, and the quantile predictions of the quantile GBM
def fit_predict(Xtr, ytr, gtr, Xte):
	res = {}
	# ngb: the number of the boosting stages by early stopping on a fifth of the training instances, then a fit on all of them
	tr, va = next(GroupShuffleSplit(n_splits=1, test_size=0.2, random_state=0).split(Xtr, ytr, gtr))
	m = new_ngb(600)
	m.fit(Xtr[tr], ytr[tr], X_val=Xtr[va], Y_val=ytr[va], early_stopping_rounds=40)
	best = int(getattr(m, "best_val_loss_itr", None) or 600)
	best = max(best + 1, 20)
	d = new_ngb(best).fit(Xtr, ytr).pred_dist(Xte)
	res["ngb"] = (np.asarray(d.params["loc"]), np.asarray(d.params["scale"]))
	# gbm+const: sigma is the deviation of the out-of-fold residuals of inner folds (split by instance), so it is not the optimistic in-sample one
	oof = np.zeros(len(ytr))
	for a, b in GroupKFold(n_splits=3).split(Xtr, ytr, gtr):
		oof[b] = new_gbm().fit(Xtr[a], ytr[a]).predict(Xtr[b])
	res["gbm+const"] = (new_gbm().fit(Xtr, ytr).predict(Xte), np.full(len(Xte), float(np.sqrt(np.mean((oof - ytr) ** 2)))))
	res["const"] = (np.full(len(Xte), ytr.mean()), np.full(len(Xte), ytr.std()))
	quant = np.column_stack([new_gbm(loss="quantile", alpha=q).fit(Xtr, ytr).predict(Xte) for q in QS])
	return res, quant, best

def evaluate(mu, sigma, y):
	z = (y - mu) / sigma
	pit = norm.cdf(z)
	hist = np.histogram(pit, bins=10, range=(0, 1))[0] / len(y)
	return {"rmse": float(np.sqrt(np.mean((y - mu) ** 2))), "mae": float(np.mean(np.abs(y - mu))), "nll": float(np.mean(0.5 * np.log(2 * np.pi * sigma ** 2) + 0.5 * z ** 2)),
		"cov50": float(np.mean(np.abs(z) <= norm.ppf(0.75))), "cov90": float(np.mean(np.abs(z) <= norm.ppf(0.95))), "width90": float(np.mean(2 * norm.ppf(0.95) * sigma)),
		"pit": [float(h) for h in hist], "pit_dev": float(np.abs(hist - 0.1).max()), "sigma_mean": float(sigma.mean())}

def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("--work", required=True)
	ap.add_argument("--folds", type=int, default=5)
	a = ap.parse_args()
	warnings.filterwarnings("ignore")
	records = [r for r in json.load(open(os.path.join(a.work, "instances.json"))) if r["family"] != "planner"]
	use = [r for r in records if all(r[k] in ("SAT", "UNSAT") for k in ("status", "target_status", "vivify_status"))]
	confs = lambda r, c: r["confs"] if c == "base" else r[c + "_confs"]
	X, y, g, cfg, fam, sat = [], [], [], [], [], []
	for i, r in enumerate(use):
		for k, c in enumerate(CONFIGS):
			X.append(r["inst_features"] + [float(k == j) for j in range(len(CONFIGS))])
			y.append(math.log10(confs(r, c) + 1)); g.append(i); cfg.append(k); fam.append(r["family"]); sat.append(r["status"] == "SAT")
	X, y, g, cfg, fam, sat = np.array(X), np.array(y), np.array(g), np.array(cfg), np.array(fam), np.array(sat)
	names = ["ngb", "gbm+const", "const"]
	out = {}

	def run(splits):
		mu = {m: np.full(len(y), np.nan) for m in names}
		sg = {m: np.full(len(y), np.nan) for m in names}
		quant = np.full((len(y), len(QS)), np.nan)
		stages = []
		for tr, te in splits:
			assert not set(g[tr]) & set(g[te])
			res, q, best = fit_predict(X[tr], y[tr], g[tr], X[te])
			stages.append(best)
			for m in names:
				mu[m][te], sg[m][te] = res[m]
			quant[te] = q
		return mu, sg, quant, stages

	md = ["## 4. Instance-level regression: a predictive distribution of log10(conflicts)", "",
		f"Generated by `scripts/ngb_regress.py`. **The data set is small: {len(use)} generated instances** ({', '.join(f'{f}: {sum(r['family'] == f for r in use)}' for f in sorted(set(fam)))}; "
		f"{sum(r['status'] == 'SAT' for r in use)} satisfiable), each under {len(CONFIGS)} configurations ({len(y)} rows; {len(records) - len(use)} instances dropped for a run without an answer). "
		f"{len(PER_INST)} instance-level features and the configuration. The target is log10(conflicts + 1): mean {y.mean():.2f}, deviation {y.std():.2f}, range {y.min():.2f} to {y.max():.2f}.",
		"NLL is that of a Normal on the log10 scale (lower is better). `cov50`/`cov90`: the share of the held-out rows inside the central 50%/90% predictive interval (calibrated: 0.50/0.90). `width90`: the mean width of the 90% interval in log10 units (a factor of 10^width in conflicts).",
		"PIT: the predictive CDF at the observed value, in 10 bins (calibrated: 0.10 each).", ""]
	results = {}
	for protocol, title, splits in (("within", f"Held out by instance ({a.folds} grouped folds over all the families)", list(GroupKFold(n_splits=a.folds).split(X, y, g))),
			("family", "Held out by family (trained on the other families; the pigeonhole formulas are in the training sets only)", [(np.nonzero(fam != f)[0], np.nonzero(fam == f)[0]) for f in ("rand", "factor", "col")])):
		mu, sg, quant, stages = run(splits)
		results[protocol] = (mu, sg, quant)
		done = ~np.isnan(mu["ngb"])
		md += [f"### {title}", "", f"NGBoost stages chosen by early stopping per fold: {stages}.", "", "| rows | model | RMSE | MAE | NLL | cov50 | cov90 | width90 | mean sigma | PIT histogram | max PIT deviation |", "|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|"]
		subsets = [("all", done)] + [(f, done & (fam == f)) for f in ("rand", "factor", "col")]
		for sname, mask in subsets:
			for m in names:
				e = evaluate(mu[m][mask], sg[m][mask], y[mask])
				out[f"{protocol}/{sname}/{m}"] = e
				md.append(f"| {sname} ({int(mask.sum())}) | {m} | {e['rmse']:.3f} | {e['mae']:.3f} | {e['nll']:.3f} | {e['cov50']:.2f} | {e['cov90']:.2f} | {e['width90']:.2f} | {e['sigma_mean']:.2f} | " + " ".join(f"{h:.2f}" for h in e["pit"]) + f" | {e['pit_dev']:.2f} |")
			q = quant[mask]
			c50, c90 = np.mean((y[mask] >= q[:, 1]) & (y[mask] <= q[:, 3])), np.mean((y[mask] >= q[:, 0]) & (y[mask] <= q[:, 4]))
			out[f"{protocol}/{sname}/gbm quantile"] = {"cov50": float(c50), "cov90": float(c90), "width90": float(np.mean(q[:, 4] - q[:, 0]))}
			md.append(f"| {sname} ({int(mask.sum())}) | gbm quantile | {np.sqrt(np.mean((y[mask] - q[:, 2]) ** 2)):.3f} | {np.mean(np.abs(y[mask] - q[:, 2])):.3f} | - | {c50:.2f} | {c90:.2f} | {np.mean(q[:, 4] - q[:, 0]):.2f} | - | - | - |")
		md.append("")
		# Is the predicted sigma informative? The rows sorted by the predicted sigma, in thirds: the predicted sigma against the deviation actually observed
		md += ["Is NGBoost's sigma informative? The held-out rows in thirds by the predicted sigma: the mean predicted sigma against the root mean square of the actual error.", "", "| third | rows | mean predicted sigma | RMS error of ngb | RMS error of gbm+const on the same rows | cov90 of ngb |", "|---|---:|---:|---:|---:|---:|"]
		idx = np.nonzero(done)[0]
		srt = idx[np.argsort(sg["ngb"][idx])]
		for k, part in enumerate(np.array_split(srt, 3)):
			z = (y[part] - mu["ngb"][part]) / sg["ngb"][part]
			md.append(f"| {('lowest', 'middle', 'highest')[k]} | {len(part)} | {sg['ngb'][part].mean():.3f} | {np.sqrt(np.mean((y[part] - mu['ngb'][part]) ** 2)):.3f} | {np.sqrt(np.mean((y[part] - mu['gbm+const'][part]) ** 2)):.3f} | {np.mean(np.abs(z) <= norm.ppf(0.95)):.2f} |")
		md.append("")

	mu, sg, quant = results["within"]
	# The noise floor: the same instance under a perturbation without information
	md += ["### The noise floor: what an uninformative perturbation does to log10(conflicts) of the same instance", "", "`/decision/polarity/flip_factor 1000` against the baseline, per family: the root mean square of the difference of log10(conflicts + 1). No predictor from the formula alone can have a smaller error than about this over the square root of 2.", "",
		"| family | answer | instances | RMS difference | mean NGBoost sigma (base rows, held out by instance) |", "|---|---|---:|---:|---:|"]
	for f in ("rand", "factor", "col"):
		for s in ("SAT", "UNSAT"):
			rs = [(i, r) for i, r in enumerate(use) if r["family"] == f and r["status"] == s and r["flip1000_status"] == s]
			if rs:
				diff = np.array([math.log10(r["flip1000_confs"] + 1) - math.log10(r["confs"] + 1) for _, r in rs])
				rows = [np.nonzero((g == i) & (cfg == 0))[0][0] for i, _ in rs]
				md.append(f"| {f} | {s} | {len(rs)} | {np.sqrt(np.mean(diff ** 2)):.3f} | {sg['ngb'][rows].mean():.3f} |")
	md.append("")

	# Use 1: the choice of the configuration per instance
	C = np.array([[confs(r, c) for c in CONFIGS] for r in use], dtype=float)
	M = np.column_stack([mu["ngb"][cfg == k] for k in range(3)])
	S = np.column_stack([sg["ngb"][cfg == k] for k in range(3)])
	G = np.column_stack([mu["gbm+const"][cfg == k] for k in range(3)])
	assert all((g[cfg == k] == np.arange(len(use))).all() for k in range(3))
	rules = [("always base", np.zeros(len(use), dtype=int)), ("always target", np.ones(len(use), dtype=int)), ("always vivify", np.full(len(use), 2)),
		("ngb: lowest predicted mean", M.argmin(1)), ("ngb: lowest predicted 90% quantile (risk-aware)", (M + norm.ppf(0.9) * S).argmin(1)),
		("ngb: leave base only if P(other < base) > 0.75 (independent Normals)", None), ("gbm: lowest predicted mean", G.argmin(1)), ("oracle: the best of the three per instance", C.argmin(1))]
	# The cautious rule: switch to the alternative with the highest probability of beating the baseline, if that probability exceeds 0.75
	pBeat = norm.cdf((M[:, [0]] - M) / np.sqrt(S[:, [0]] ** 2 + S ** 2))
	pBeat[:, 0] = 0
	rules[5] = (rules[5][0], np.where(pBeat.max(1) > 0.75, pBeat.argmax(1), 0))
	nontrivial = C[:, 0] >= 200
	isSat = np.array([r["status"] == "SAT" for r in use])
	md += ["### Use 1: choosing the configuration per instance from the predictions (held out by instance)", "",
		"The conflicts of the chosen configuration against always running the baseline: the geometric mean of (x + 1) / (base + 1), over all the instances and over those with at least 200 baseline conflicts, split by the answer.", "",
		f"| rule | chosen: base / target / vivify | ratio, all ({len(use)}) | ratio, >= 200 conflicts ({int(nontrivial.sum())}) | of them satisfiable ({int((nontrivial & isSat).sum())}) | unsatisfiable ({int((nontrivial & ~isSat).sum())}) | total conflicts |", "|---|---|---:|---:|---:|---:|---:|"]
	for name, choice in rules:
		x = C[np.arange(len(use)), choice]
		ratio = lambda mask: float(np.exp(np.mean(np.log((x[mask] + 1) / (C[mask, 0] + 1)))))
		out["choose/" + name] = [ratio(np.ones(len(use), dtype=bool)), ratio(nontrivial)]
		md.append(f"| {name} | {' / '.join(str(int((choice == k).sum())) for k in range(3))} | {ratio(np.ones(len(use), dtype=bool)):.3f} | {ratio(nontrivial):.3f} | {ratio(nontrivial & isSat):.3f} | {ratio(nontrivial & ~isSat):.3f} | {x.sum():.0f} |")
	md += ["", f"How well are the differences between the configurations predicted? The correlation of the predicted difference of log10(conflicts) (target minus base, vivify minus base) with the actual one, on the instances with at least 200 baseline conflicts: "
		+ ", ".join(f"{CONFIGS[k]}: ngb {np.corrcoef((M[:, k] - M[:, 0])[nontrivial], np.log10((C[:, k] + 1) / (C[:, 0] + 1))[nontrivial])[0, 1]:.3f}, gbm {np.corrcoef((G[:, k] - G[:, 0])[nontrivial], np.log10((C[:, k] + 1) / (C[:, 0] + 1))[nontrivial])[0, 1]:.3f}" for k in (1, 2)) + ".", ""]

	# Use 2: a conflict budget from a predicted quantile (the baseline configuration)
	base = cfg == 0
	yb = y[base]
	md += ["### Use 2: a conflict budget from a predicted quantile (baseline configuration, held out by instance)", "",
		"The budget of an instance is the predicted q-quantile of its conflicts; `solved`: the share of the instances that finish within their budget (calibrated: q); `spent`: the conflicts spent in total, min(conflicts, budget) per instance,",
		"as a share of the conflicts of solving everything; `budget`: the geometric mean of the budgets. A sharper predictive distribution reaches the same solved share with less spent on the instances it gives up on.", "",
		"| q | model | solved | spent | geometric mean budget | wasted on unsolved (share of all conflicts) |", "|---:|---|---:|---:|---:|---:|"]
	conf = 10 ** yb - 1
	for q in (0.5, 0.75, 0.9, 0.95):
		for m in names:
			b = 10 ** (mu[m][base] + norm.ppf(q) * sg[m][base]) - 1
			solved = conf <= b
			out[f"budget/{q}/{m}"] = [float(solved.mean()), float(np.minimum(conf, b).sum() / conf.sum())]
			md.append(f"| {q} | {m} | {solved.mean():.3f} | {np.minimum(conf, b).sum() / conf.sum():.3f} | {10 ** np.mean(np.log10(b + 1)):.0f} | {b[~solved].sum() / conf.sum():.3f} |")
	# The same solved share with one global budget, and with the budget of each model scaled to solve exactly 90%
	md += ["", "At an equal solved share of exactly 90% (each model's budgets shifted by a constant on the log scale until 90% of the held-out instances finish; `const` is then one global budget):", "", "| model | spent | wasted on unsolved |", "|---|---:|---:|"]
	for m in names:
		z = (yb - mu[m][base]) / sg[m][base]
		b = 10 ** (mu[m][base] + np.quantile(z, 0.9) * sg[m][base]) - 1
		solved = conf <= b + 1e-9
		out[f"budget90/{m}"] = float(np.minimum(conf, b).sum() / conf.sum())
		md.append(f"| {m} | {np.minimum(conf, b).sum() / conf.sum():.3f} | {b[~solved].sum() / conf.sum():.3f} |")
	md.append("")
	with open(os.path.join(a.work, "tables", "4_regression.md"), "w") as f:
		f.write("\n".join(md) + "\n")
	with open(os.path.join(a.work, "regress.json"), "w") as f:
		json.dump(out, f, indent=1)
	print("\n".join(md))

if __name__ == "__main__":
	main()
