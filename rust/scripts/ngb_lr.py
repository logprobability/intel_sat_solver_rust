#!/usr/bin/env python3
# NGBoost experiment, step 7: natural-gradient boosting on top of a logistic-regression baseline (the per-variable phase predictors of ngb_phase.py).
#
# ngb_lr     the logit of every sample starts at the logit of the logistic regression (a per-sample offset) and the trees fit only the residual.
#            ngboost 0.6.1 has no per-sample initial parameter (NGBoost.init_params is one vector, the marginal fit; pred_param starts every row from it), so the
#            Bernoulli case is implemented here (OffsetBernoulliNGB): parameter theta, p = sigmoid(theta), the natural gradient of the log score is
#            (p - y) / (p (1 - p)) (the gradient p - y divided by the Fisher information p (1 - p); p clipped to [EPS, 1 - EPS] in it), a DecisionTreeRegressor
#            with the settings of the ngb run is fitted to it on a minibatch, and theta <- theta - RATE * scale * tree(x), the scale by NGBoost's own line search.
# ngb_stack  the stock NGBClassifier with the logit of the logistic regression appended as a feature.
# ngb_es     the stock NGBClassifier of ngb_phase.py with the same early stopping, so that the comparison isolates the baseline and not the stopping rule.
# The logistic regression is fitted on the training fold only. The logits which the boosters train on are cross-fitted over the instances of the training fold
# (each instance's logits come from a logistic regression which has not seen that instance), so that the booster does not train on optimistically fitted offsets;
# the logits of the held-out instances come from the logistic regression of the whole training fold (which is exactly the `logreg` of ngb_phase.py).
# Early stopping: a fifth of the INSTANCES of the training fold is the validation set; the number of rounds with the lowest validation log loss is chosen
# (0 rounds is a legal choice: the baseline alone), and the model is then fitted again on the whole training fold with that many rounds.
# The folds, the held-out instances and the protocols are those of ngb_phase.py.
#
# Usage: ngb_lr.py --work DIR [--stage fit|report|all] [--jobs N]
#   fit: reads DIR/vars.npz, instances.json, preds.npz; writes DIR/preds_lr.npz and DIR/lr_info.json.
#   report: reads those, backbone.npz and solve_runs.csv (after `ngb_solve.py --resume` has run the new configurations); writes DIR/tables/6_ngb_lr.md.
import argparse, csv, json, os, sys, time, warnings

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from scipy.special import expit
from scipy.stats import wilcoxon
from sklearn.linear_model import LogisticRegression
from sklearn.model_selection import GroupKFold, GroupShuffleSplit
from sklearn.pipeline import make_pipeline
from sklearn.preprocessing import StandardScaler
from sklearn.tree import DecisionTreeRegressor
from ngboost import NGBClassifier
from ngboost.distns import Bernoulli
from ngb_phase import metrics

NEW = ["ngb_es", "ngb_lr", "ngb_stack"]
SHOWN = ["logreg", "ngb", "ngb_es", "ngb_lr", "ngb_stack"]
ROUNDS, PATIENCE, RATE, FRAC, MAX_ROWS = 300, 50, 0.05, 0.5, 30000	# the rounds, learning rate, minibatch fraction and row limit of the ngb of ngb_phase.py
EPS = 1e-3	# p is kept inside [EPS, 1 - EPS] in the natural gradient
LOGIT_CLIP = float(np.log((1 - 1e-6) / 1e-6))	# the offsets are clipped where metrics() clips the probabilities, so 0 rounds is exactly logreg

def new_logreg():
	return make_pipeline(StandardScaler(), LogisticRegression(C=1.0, max_iter=3000))

def new_ngb(n):
	return NGBClassifier(Dist=Bernoulli, n_estimators=n, learning_rate=RATE, minibatch_frac=FRAC, Base=DecisionTreeRegressor(criterion="friedman_mse", max_depth=3), verbose=False, random_state=0)

def log_loss(theta, y):
	return float(np.mean(np.logaddexp(0, theta) - y * theta))

# Natural-gradient boosting of a Bernoulli logit from a per-sample offset
class OffsetBernoulliNGB:
	# rate, depth: the learning rate and the tree depth; patience: the rounds after the best validation loss after which the fit stops (None: never; ngb_nested_cv.py records whole curves)
	def __init__(self, rate=RATE, depth=3, patience=PATIENCE):
		self.rate, self.depth, self.patience = rate, depth, patience

	# NGBoost's line search: double the scale while the loss of the minibatch does not rise, then halve it until the loss is below the initial one
	@staticmethod
	def line_search(step, theta, y):
		init, scale = log_loss(theta, y), 1.0
		while True:
			loss = log_loss(theta - scale * step, y)
			if not np.isfinite(loss) or loss > init or scale > 256:
				break
			scale *= 2
		while True:
			loss = log_loss(theta - scale * step, y)
			if np.abs(scale * step).mean() < 1e-4 or (np.isfinite(loss) and loss < init):
				break
			scale *= 0.5
		return scale

	# val: (X, y, offset) of the validation rows; with it, val_loss[k] is the validation log loss after k rounds, and the fit stops PATIENCE rounds after the best one
	def fit(self, X, y, offset, rounds, val=None):
		rng = np.random.RandomState(0)
		self.trees, self.scales = [], []
		theta = np.array(offset, dtype=float)
		if val is not None:
			Xv, yv, thetaV = val[0], val[1], np.array(val[2], dtype=float)
			self.val_loss = [log_loss(thetaV, yv)]
		for _ in range(rounds):
			idx = rng.choice(len(y), int(FRAC * len(y)), replace=False)
			p = np.clip(expit(theta[idx]), EPS, 1 - EPS)
			grad = (p - y[idx]) / (p * (1 - p))
			tree = DecisionTreeRegressor(criterion="friedman_mse", max_depth=self.depth, random_state=rng.randint(2 ** 31 - 1)).fit(X[idx], grad)
			step = tree.predict(X[idx])
			scale = self.line_search(step, theta[idx], y[idx])
			self.trees.append(tree)
			self.scales.append(scale)
			theta -= self.rate * scale * tree.predict(X)
			if val is not None:
				thetaV -= self.rate * scale * tree.predict(Xv)
				self.val_loss.append(log_loss(thetaV, yv))
				if self.patience is not None and len(self.val_loss) - 1 > self.patience and min(self.val_loss) < min(self.val_loss[-self.patience:]):
					break
			if np.abs(step).mean() < 1e-4:
				break
		return self

	def logit(self, X, offset):
		theta = np.array(offset, dtype=float)
		for tree, scale in zip(self.trees, self.scales):
			theta -= self.rate * scale * tree.predict(X)
		return theta

# The stock NGBClassifier with the early stopping used here: returns (probabilities of Xte, rounds chosen, validation loss at 0 rounds, at the chosen number, seconds per predicted row)
def stock(Xs, ys, Xv, yv, Xall, yall, Xte):
	m = new_ngb(ROUNDS).fit(Xs, ys, X_val=Xv, Y_val=yv, early_stopping_rounds=PATIENCE)
	val = [float(v) for v in list(m.evals_result["val"].values())[0]]
	p0 = float(np.clip(ys.mean(), 1e-6, 1 - 1e-6))
	init = log_loss(np.full(len(yv), np.log(p0 / (1 - p0))), yv)
	n = int(np.argmin(val)) + 1 if min(val) < init else 0
	if n == 0:
		return np.full(len(Xte), yall.mean()), 0, init, init, 0.0
	m = new_ngb(n).fit(Xall, yall)
	t = time.perf_counter()
	p = m.predict_proba(Xte)[:, 1]
	return p, n, init, min(val), (time.perf_counter() - t) / max(len(Xte), 1)

# One training set and one held-out set: the probabilities of the held-out rows per model, and what was chosen
def task(key, Xtr, ytr, gtr, Xte):
	warnings.filterwarnings("ignore")
	# The baseline: cross-fitted logits for the training rows, the logits of the model of the whole training fold for the held-out rows
	oof = np.zeros(len(ytr))
	for a, b in GroupKFold(n_splits=min(5, len(set(gtr)))).split(Xtr, ytr, gtr):
		assert not set(gtr[a]) & set(gtr[b])
		oof[b] = new_logreg().fit(Xtr[a], ytr[a]).decision_function(Xtr[b])
	oof = np.clip(oof, -LOGIT_CLIP, LOGIT_CLIP)
	lr = new_logreg().fit(Xtr, ytr)
	t = time.perf_counter()
	offTe = np.clip(lr.decision_function(Xte), -LOGIT_CLIP, LOGIT_CLIP)
	tLr = (time.perf_counter() - t) / max(len(Xte), 1)
	# The validation instances of the early stopping, and the row limit of ngb_phase.py (the rows of the final fit are exactly those its ngb was fitted on)
	sub, va = next(GroupShuffleSplit(n_splits=1, test_size=0.2, random_state=0).split(Xtr, ytr, gtr))
	assert not set(gtr[sub]) & set(gtr[va])
	cap = lambda rows: rows if len(rows) <= MAX_ROWS else np.random.RandomState(0).choice(rows, MAX_ROWS, replace=False)
	sub, va, full = cap(sub), cap(va), cap(np.arange(len(ytr)))
	info = {"key": key, "train_rows": int(len(ytr)), "train_instances": len(set(gtr)), "val_instances": len(set(gtr[va])), "oof_logloss": log_loss(oof, ytr), "t_logreg": tLr}
	res = {"logreg": expit(offTe)}
	# ngb_lr
	m = OffsetBernoulliNGB().fit(Xtr[sub], ytr[sub], oof[sub], ROUNDS, val=(Xtr[va], ytr[va], oof[va]))
	n = int(np.argmin(m.val_loss))
	final = OffsetBernoulliNGB().fit(Xtr[full], ytr[full], oof[full], n)
	t = time.perf_counter()
	res["ngb_lr"] = expit(final.logit(Xte, offTe))
	info["ngb_lr"] = {"rounds": n, "val0": m.val_loss[0], "val_best": min(m.val_loss), "rounds_run": len(m.val_loss) - 1, "t": tLr + (time.perf_counter() - t) / max(len(Xte), 1), "mean_scale": float(np.mean(final.scales)) if n else 0.0}
	# ngb_es and ngb_stack
	S = lambda rows, logit: np.column_stack([Xtr[rows], logit[rows]])
	for name, args in (("ngb_es", (Xtr[sub], ytr[sub], Xtr[va], ytr[va], Xtr[full], ytr[full], Xte)),
			("ngb_stack", (S(sub, oof), ytr[sub], S(va, oof), ytr[va], S(full, oof), ytr[full], np.column_stack([Xte, offTe])))):
		p, n, v0, vb, t = stock(*args)
		res[name] = p
		info[name] = {"rounds": n, "val0": v0, "val_best": vb, "t": t + (tLr if name == "ngb_stack" else 0)}
	return key, res, info

def fit(a):
	from joblib import Parallel, delayed
	d = np.load(os.path.join(a.work, "vars.npz"))
	X, y, inst = d["X"], d["y"], d["inst"]
	feat = [str(f) for f in d["features"]]
	records = json.load(open(os.path.join(a.work, "instances.json")))
	old = np.load(os.path.join(a.work, "preds.npz"))
	noLf = [i for i, f in enumerate(feat) if f != "lf_pol"]
	allF = list(range(len(feat)))
	perVar = [i for i in noLf if not feat[i].startswith("i_")]
	family = np.array([records[i]["family"] for i in inst])
	phase = np.array([records[i]["role"] == "phase" for i in inst])
	lab = (y >= 0) & phase
	# The training and held-out sets of ngb_phase.py: (protocol, family, fold), the training rows, the held-out rows, the columns
	tasks = []
	for fam in ("rand", "factor", "col", "planner"):
		rows = np.nonzero(lab & (family == fam))[0]
		groups = inst[rows]
		for protocol, cols in [("within", noLf)] + ([("within+lf", allF), ("within-pv", perVar)] if fam == "planner" else []):
			for fold, (tr, te) in enumerate(GroupKFold(n_splits=min(a.folds, len(set(groups)))).split(rows, y[rows], groups)):
				assert not set(groups[tr]) & set(groups[te])
				tasks.append(((protocol, fam, fold), rows[tr], rows[te], cols))
		other = np.nonzero((family == fam) & ~lab)[0]
		if len(other):
			tasks.append((("within", fam, "other"), rows, other, noLf))
		tasks.append((("cross", fam, 0), np.nonzero(lab & (family != fam) & (family != "planner"))[0], rows, noLf))
	out = Parallel(n_jobs=a.jobs, verbose=5)(delayed(task)(key, X[tr][:, cols], y[tr], inst[tr], X[te][:, cols]) for key, tr, te, cols in tasks)
	preds, infos = {}, []
	for (key, res, info), (_, tr, te, _) in zip(out, tasks):
		for m, p in res.items():
			preds.setdefault(f"{key[0]}:{m}", np.full(len(y), np.nan))[te] = p
		infos.append(info)
	# The logistic regression fitted here must be the logreg of ngb_phase.py
	diff = max(float(np.nanmax(np.abs(preds[f"{p}:logreg"] - np.clip(old[f"{p}:logreg"], 1e-6, 1 - 1e-6)))) for p in ("within", "within+lf", "within-pv", "cross"))
	print("largest difference between the logistic regression fitted here and the logreg of preds.npz:", diff)
	assert diff < 1e-9, diff
	np.savez_compressed(os.path.join(a.work, "preds_lr.npz"), **{k: v for k, v in preds.items() if not k.endswith(":logreg")})
	with open(os.path.join(a.work, "lr_info.json"), "w") as f:
		json.dump({"logreg_diff": diff, "tasks": infos}, f, indent=1)

# The pooled difference of a metric between two models with a 95% bootstrap interval over the INSTANCES (the variables of an instance stay together)
def paired_metrics(pa, pb, y, groups, rng):
	pa, pb = np.clip(pa, 1e-6, 1 - 1e-6), np.clip(pb, 1e-6, 1 - 1e-6)
	ids = np.unique(groups, return_inverse=True)[1]
	k = ids.max() + 1
	per = lambda v: np.bincount(ids, weights=v, minlength=k)
	n = per(np.ones(len(y)))
	ll = lambda p: -(y * np.log(p) + (1 - y) * np.log(1 - p))
	dLL, dBr = per(ll(pa) - ll(pb)), per((pa - y) ** 2 - (pb - y) ** 2)
	# The ECE of a resample from the sums per instance and bin
	def bins(p):
		b = np.minimum((p * 10).astype(int), 9)
		return np.stack([np.bincount(ids * 10 + b, weights=w, minlength=k * 10).reshape(k, 10) for w in (p, y.astype(float))])
	Ba, Bb = bins(pa), bins(pb)
	ece = lambda B, w: np.abs((B[0] * w[:, None]).sum(0) - (B[1] * w[:, None]).sum(0)).sum() / (n * w).sum()
	one = np.ones(k)
	res = {"logloss": [dLL.sum() / n.sum()], "brier": [dBr.sum() / n.sum()], "ece": [ece(Ba, one) - ece(Bb, one)]}
	boot = {m: [] for m in res}
	for _ in range(2000):
		w = np.bincount(rng.randint(k, size=k), minlength=k).astype(float)
		boot["logloss"].append((dLL * w).sum() / (n * w).sum()); boot["brier"].append((dBr * w).sum() / (n * w).sum()); boot["ece"].append(ece(Ba, w) - ece(Bb, w))
	return {m: (float(res[m][0]), float(np.percentile(boot[m], 2.5)), float(np.percentile(boot[m], 97.5))) for m in res}

def report(a):
	d = np.load(os.path.join(a.work, "vars.npz"))
	y, inst = d["y"], d["inst"]
	records = json.load(open(os.path.join(a.work, "instances.json")))
	preds = dict(np.load(os.path.join(a.work, "preds.npz")))
	preds.update(np.load(os.path.join(a.work, "preds_lr.npz")))
	info = json.load(open(os.path.join(a.work, "lr_info.json")))
	family = np.array([records[i]["family"] for i in inst])
	lab = (y >= 0) & np.array([records[i]["role"] == "phase" for i in inst])
	fams = ["rand", "factor", "col", "planner"]
	rng = np.random.RandomState(0)
	out = {}
	md = ["## 6. NGBoost on a logistic-regression baseline", "",
		"Generated by `scripts/ngb_lr.py` (and, for the solver runs, `scripts/ngb_solve.py --resume`). `ngb_lr`: natural-gradient boosting of the Bernoulli logit from a per-sample offset, the logit of the logistic regression",
		f"(own implementation: ngboost 0.6.1 has no per-sample initial parameter); `ngb_stack`: the stock NGBClassifier with that logit as one more feature; `ngb_es`: the stock NGBClassifier of section 1 with the same early stopping; `logreg`, `ngb`: the rows of section 1.",
		f"All three: trees of depth 3, learning rate {RATE}, minibatch fraction {FRAC}, at most {MAX_ROWS} training rows, at most {ROUNDS} rounds, NGBoost's line search for the scale of a step; the number of rounds is the one with the lowest log loss on a validation set of a fifth of the training",
		f"instances (never the variables of one instance on both sides; the fit stops {PATIENCE} rounds after the best one; 0 rounds is allowed and means the baseline alone), and the model is then fitted on the whole training fold with that many rounds.",
		"The logits which the boosters train on are cross-fitted over the instances of the training fold (5 grouped folds; 3 for the planner), those of the held-out instances come from the logistic regression of the whole training fold,",
		f"which reproduces the `logreg` predictions of section 1 (largest difference {info['logreg_diff']:.1e}). Held-out instances, folds and protocols are those of section 1.", "",
		"### Rounds chosen by early stopping", "",
		"Per fold of the protocol; `val`: the validation log loss with 0 rounds (for `ngb_lr`: the cross-fitted logistic regression alone; for the other two: the base rate) and with the chosen number. `other`: the model of all the satisfiable instances of the family, which predicts its unsatisfiable ones.", "",
		"| protocol | family | fold | training instances (validation) | ngb_es: rounds | val 0 -> best | ngb_lr: rounds | val 0 -> best | ngb_stack: rounds | val 0 -> best |", "|---|---|---|---:|---:|---|---:|---|---:|---|"]
	for t in info["tasks"]:
		k = t["key"]
		md.append(f"| {k[0]} | {k[1]} | {k[2] if k[2] == 'other' else k[2] + 1} | {t['train_instances']} ({t['val_instances']}) | " + " | ".join(f"{t[m]['rounds']} | {t[m]['val0']:.4f} -> {t[m]['val_best']:.4f}" for m in NEW) + " |")
	out["rounds"] = {m: {f"{p}/{f}": [t[m]["rounds"] for t in info["tasks"] if t["key"][0] == p and t["key"][1] == f and t["key"][2] != "other"] for p in ("within", "within+lf", "within-pv", "cross") for f in fams} for m in NEW}
	md += ["", "### Calibration, out-of-sample by instance", "",
		"The columns of section 1. `d log loss`, `d Brier`, `d ECE`: the model minus `logreg` on the same variables, with a 95% bootstrap interval over the held-out instances (negative: better than the logistic regression).", ""]
	for protocol, title in (("within", "Within-family (grouped folds; the planner: leave-one-instance-out)"), ("within+lf", "The planner, with the planner's lf polarity as a feature (leave-one-instance-out)"),
			("within-pv", "The planner, with the per-variable features only (leave-one-instance-out)"), ("cross", "Cross-family (trained on the generated families other than the one tested)")):
		md += [f"#### {title}", "", "| family | variables | model | rounds | accuracy | Brier | log loss | ECE | acc@0.8 (cov) | acc@0.9 (cov) | d log loss vs logreg | d Brier vs logreg | d ECE vs logreg |", "|---|---:|---|---|---:|---:|---:|---:|---:|---:|---|---|---|"]
		for fam in fams:
			rows = np.nonzero(lab & (family == fam))[0]
			if np.isnan(preds[f"{protocol}:ngb_lr"][rows]).all():
				continue
			for model in SHOWN:
				p = preds[f"{protocol}:{model}"][rows]
				m = metrics(p, y[rows])
				pm = paired_metrics(p, preds[f"{protocol}:logreg"][rows], y[rows], inst[rows], rng) if model != "logreg" else None
				out[f"{protocol}/{fam}/{model}"] = {k: m[k] for k in ("n", "acc", "brier", "logloss", "ece", "acc0.8", "cov0.8", "acc0.9", "cov0.9")} | {"vs_logreg": pm}
				rounds = "-" if model == "logreg" else "300" if model == "ngb" else " ".join(map(str, out["rounds"][model][f"{protocol}/{fam}"]))
				md.append(f"| {fam} | {m['n']} | {model} | {rounds} | {m['acc']:.3f} | {m['brier']:.4f} | {m['logloss']:.4f} | {m['ece']:.4f} | " + " | ".join(f"{m[f'acc{t}']:.3f} ({m[f'cov{t}']:.2f})" if m[f"cov{t}"] > 0 else "- (0.00)" for t in (0.8, 0.9))
					+ " | " + (" | ".join(f"{pm[k][0]:+.4f} ({pm[k][1]:+.4f} to {pm[k][2]:+.4f})" for k in ("logloss", "brier", "ece")) if pm else "- | - | -") + " |")
		md.append("")
	md += ["#### Reliability (within-family): per bin of p, the number of variables, the mean predicted p and the observed frequency", ""]
	for fam in fams:
		rows = np.nonzero(lab & (family == fam))[0]
		rel = {m: {r[0]: r for r in metrics(preds[f"within:{m}"][rows], y[rows])["rel"]} for m in SHOWN}
		md += [f"**{fam}**", "", "| bin of p | " + " | ".join(f"{m}: n | mean p | observed" for m in SHOWN) + " |", "|---|" + "---:|---:|---:|" * len(SHOWN)]
		for b in range(10):
			md.append(f"| {b / 10:.1f}-{(b + 1) / 10:.1f} | " + " | ".join(f"{rel[m][b][1]} | {rel[m][b][2]:.3f} | {rel[m][b][3]:.3f}" if b in rel[m] else "0 | - | -" for m in SHOWN) + " |")
		md.append("")
	# How different are the predictions from those of the baseline?
	md += ["#### How far the boosted models move from the logistic regression (within-family, held-out variables)", "",
		"| family | model | mean absolute difference of p from logreg | share of the variables whose predicted polarity differs | Spearman correlation of the confidence with that of logreg |", "|---|---|---:|---:|---:|"]
	from scipy.stats import spearmanr
	for fam in fams:
		rows = np.nonzero(lab & (family == fam))[0]
		q = preds["within:logreg"][rows]
		for model in ("ngb", "ngb_es", "ngb_lr", "ngb_stack"):
			p = preds[f"within:{model}"][rows]
			rho = spearmanr(np.abs(p - 0.5), np.abs(q - 0.5))[0]
			out[f"move/{fam}/{model}"] = [float(np.abs(p - q).mean()), float(((p > 0.5) != (q > 0.5)).mean()), float(rho)]
			md.append(f"| {fam} | {model} | {np.abs(p - q).mean():.4f} | {((p > 0.5) != (q > 0.5)).mean():.3f} | {rho:.3f} |")
	md.append("")
	# Backbone and non-backbone variables (section 2)
	bbPath = os.path.join(a.work, "backbone.npz")
	if os.path.exists(bbPath):
		backbone = np.load(bbPath)["backbone"]
		md += ["### Backbone and non-backbone variables (the instances of section 2)", "", "| family | subset | variables | model | accuracy | mean confidence | Brier | ECE | acc@0.8 (cov) |", "|---|---|---:|---|---:|---:|---:|---:|---:|"]
		for fam in ("rand", "factor"):
			known = (family == fam) & (backbone >= 0) & (y >= 0)
			for name, mask in (("backbone", known & (backbone == 1)), ("non-backbone", known & (backbone == 0))):
				for model in SHOWN:
					p = preds[f"within:{model}"][mask]
					m = metrics(p, y[mask])
					out[f"backbone/{fam}/{name}/{model}"] = [m["acc"], float(np.maximum(p, 1 - p).mean())]
					md.append(f"| {fam} | {name} | {int(mask.sum())} | {model} | {m['acc']:.3f} | {np.maximum(p, 1 - p).mean():.3f} | {m['brier']:.4f} | {m['ece']:.4f} | {m['acc0.8']:.3f} ({m['cov0.8']:.2f}) |")
		md.append("")
	# The cost of inference
	tasks = [t for t in info["tasks"] if t["key"][0] == "within" and t["key"][1] != "planner" and t["key"][2] != "other"]
	old = json.load(open(os.path.join(a.work, "phase_metrics.json")))["times"]
	us = {"logreg": np.median([t["t_logreg"] for t in tasks]) * 1e6} | {m: np.median([t[m]["t"] for t in tasks if t[m]["rounds"] > 0] or [float("nan")]) * 1e6 for m in NEW}
	out["microseconds"] = {k: float(v) for k, v in us.items()}
	md += ["### Cost of inference", "", "Microseconds per variable, the median over the within-family folds of the generated families (the folds with at least one round), measured in this script on a shared machine; `ngb_lr` and `ngb_stack` include the logistic regression they start from.",
		f"For reference, section 1 measured ngb (300 rounds) at {old['ngb'][1] / max(old['ngb'][3], 1) * 1e6:.2f} and logreg at {old['logreg'][1] / max(old['logreg'][3], 1) * 1e6:.2f} in another run.", "",
		"| model | microseconds per variable | relative to logreg | median rounds (within-family, generated) |", "|---|---:|---:|---:|", f"| logreg | {us['logreg']:.2f} | 1.0 | - |"]
	for m in NEW:
		md.append(f"| {m} | {us[m]:.2f} | {us[m] / us['logreg']:.0f} | {np.median([t[m]['rounds'] for t in tasks]):.0f} |")
	md.append("")
	md += solving(a, records, out)
	os.makedirs(os.path.join(a.work, "tables"), exist_ok=True)
	with open(os.path.join(a.work, "tables", "6_ngb_lr.md"), "w") as f:
		f.write("\n".join(md) + "\n")
	with open(os.path.join(a.work, "lr_report.json"), "w") as f:
		json.dump(out, f, indent=1)
	print("\n".join(md))

# The solver runs of the new models (run by ngb_solve.py --resume), next to the cached ones, and the paired comparisons
def solving(a, records, out):
	path = os.path.join(a.work, "solve_runs.csv")
	rows = list(csv.DictReader(open(path))) if os.path.exists(path) else []
	by = {(os.path.splitext(r["instance"])[0], r["config"]): r for r in rows}
	if not any(c.startswith("ngb_lr:") for _, c in by):
		return ["### Solving", "", "The solver runs of the new models are missing: run `ngb_solve.py --resume` and this script again.", ""]
	byName = {r["name"]: r for r in records}
	bad = [(i, c) for (i, c), r in by.items() if any({x, z} == {"S", "U"} for x, z in zip(r["verdicts"].split(), by[(i, "base")]["verdicts"].split()))]
	rng = np.random.RandomState(0)
	def value(i, c):
		rs = [by[k] for k in ([(i, c)] if (i, c) in by else [(i, f"{c}_s{s}") for s in range(3)]) if k in by]
		rs = [r for r in rs if r["status"] == by[(i, "base")]["status"]]
		return float(np.median([int(r["confs"]) for r in rs])) if rs else None
	def stats(group, c, ref):
		pairs = [(value(i, c), value(i, ref)) for i in group]
		pairs = [(x, b) for x, b in pairs if x is not None and b is not None]
		if not pairs:
			return None
		logs = np.array([np.log((x + 1) / (b + 1)) for x, b in pairs])
		boot = np.exp(np.percentile([rng.choice(logs, len(logs)).mean() for _ in range(2000)], [2.5, 97.5]))
		nz = logs[logs != 0]
		return {"n": len(pairs), "ratio": float(np.exp(logs.mean())), "lo": float(boot[0]), "hi": float(boot[1]), "wins": int((logs < 0).sum()), "losses": int((logs > 0).sum()), "ties": int((logs == 0).sum()),
			"total": float(sum(x for x, _ in pairs)), "over10": int((logs > np.log(10)).sum()), "under01": int((logs < -np.log(10)).sum()), "median": float(np.exp(np.median(logs))),
			"wilcoxon": float(wilcoxon(nz).pvalue) if len(nz) >= 6 else None}
	gen = [r["name"] for r in records if r["family"] in ("rand", "factor", "col") and r["role"] == "phase" and r["status"] in ("SAT", "UNSAT")]
	groups = [(f"{title}, satisfiable, baseline >= 200 conflicts", [i for i in gen if byName[i]["family"] == fam and byName[i]["status"] == "SAT" and byName[i]["confs"] >= 200], "") for fam, title in (("rand", "random 3-SAT"), ("factor", "factoring"), ("col", "3-colouring"))]
	groups.append(("generated, unsatisfiable", [i for i in gen if byName[i]["status"] == "UNSAT"], ""))
	groups.append(("generated, satisfiable, baseline < 200 conflicts", [i for i in gen if byName[i]["status"] == "SAT" and byName[i]["confs"] < 200], ""))
	planner = [("planner `.plain`", "phase", True), ("planner `.lf`", "lf", True), ("planner `.nolf`, hints renewed per query", "nolf", False), ("planner recorded, hints renewed per query", "recorded", False)]
	for title, role, scores in planner:
		for suffix, what in (("", ""), ("PV", ", predictors without the instance-level features"), ("LF", ", predictors with the lf polarity as a feature")):
			groups.append((title + what, [r["name"] for r in records if r["family"] == "planner" and r["role"] == role], suffix))
	strategies = ["all", "t0.7", "all+conf_x1"]
	md = ["### Solving with the new predictors", "",
		f"The new configurations were added to `scripts/ngb_solve.py` and run with `--resume` next to the cached runs ({len(rows)} runs in all, every model verified by the tool). **SAT-versus-UNSAT contradictions with the baseline: {len(bad)}"
		+ ("" if not bad else " (" + ", ".join(f"{i} under `{c}`" for i, c in bad[:20]) + ")") + ".**",
		"`ratio`: the geometric mean of (conflicts + 1) / (baseline + 1) with a 95% bootstrap interval over the instances; `> 10x` / `< 0.1x`: the instances with more than ten times, or less than a tenth of, the baseline's conflicts.",
		"`M:all`: the predicted polarity of every variable as a phase hint; `M:t0.7`: only where the confidence is at least 0.7; `M:all+conf_x1`: every variable, and the confident ones decided first.", ""]
	for title, group, suffix in groups:
		if not group:
			continue
		base = sum(int(by[(i, "base")]["confs"]) for i in group)
		md += [f"#### {title} ({len(group)} instances; baseline {base} conflicts)", "", "| configuration | n | ratio | 95% interval | median ratio | wins/losses | total conflicts | > 10x | < 0.1x |", "|---|---:|---:|---|---:|---:|---:|---:|---:|"]
		configs = (["flip1000", "rand_phase", "perm_x1", "prior:all"] if suffix == "" else []) + [f"{m}{suffix}:{s}" for s in strategies for m in SHOWN]
		for c in configs:
			s = stats(group, c, "base")
			if s:
				out[f"solve/{title}/{c}"] = s
				md.append(f"| `{c}` | {s['n']} | {s['ratio']:.3f} | {s['lo']:.3f}-{s['hi']:.3f} | {s['median']:.3f} | {s['wins']}/{s['losses']} | {s['total']:.0f} | {s['over10']} | {s['under01']} |")
		md.append("")
	md += ["### Paired comparisons", "",
		"Per instance, the logarithm of (conflicts of A + 1) / (conflicts of B + 1): its geometric mean with a 95% bootstrap interval over the instances, the instances on which A needs fewer / more / as many conflicts, and the two-sided p-value of the Wilcoxon signed-rank test (not adjusted for the number of comparisons in this table).", "",
		"| group | A | B | n | A / B | 95% interval | median | A better / worse / equal | Wilcoxon p | total conflicts A / B |", "|---|---|---|---:|---:|---|---:|---|---:|---|"]
	for title, group, suffix in groups:
		for s in strategies:
			for A, B in (("ngb_lr", "logreg"), ("ngb_stack", "logreg"), ("ngb_es", "logreg"), ("ngb", "logreg"), ("ngb_es", "ngb"), ("ngb_lr", "ngb_es")):
				ca, cb = f"{A}{suffix}:{s}", f"{B}{suffix}:{s}"
				st = stats(group, ca, cb) if group else None
				if st:
					out[f"paired/{title}/{ca}/{cb}"] = st
					tb = sum(value(i, cb) for i in group if value(i, ca) is not None and value(i, cb) is not None)
					md.append(f"| {title} | `{ca}` | `{cb}` | {st['n']} | {st['ratio']:.3f} | {st['lo']:.3f}-{st['hi']:.3f} | {st['median']:.3f} | {st['wins']} / {st['losses']} / {st['ties']} | " + ("-" if st["wilcoxon"] is None else f"{st['wilcoxon']:.3f}") + f" | {st['total']:.0f} / {tb:.0f} |")
	md.append("")
	# The noise of the search: the confident-first configurations again under the uninformative perturbation of the flip controls
	flips = [k for k in (500, 1000, 2000) if any(c == f"ngb_lr:all+conf_x1+flip{k}" for _, c in by)]
	if flips:
		md += ["### The confident-first configurations under an uninformative perturbation", "",
			"Added after the tables above were seen, as a check of their differences against the noise of the search: the same hints with `/decision/polarity/flip_factor K` (the perturbation of the controls `flipK`), which changes the search and adds no information.",
			"`ratio`: against the unperturbed baseline. `A / logreg`: the paired comparison with `logreg:all+conf_x1` under the same perturbation. `median of the perturbed`: per instance the median conflicts of the runs with K = " + ", ".join(map(str, flips)) + ".", "",
			"| group | K | flipK alone | logreg: ratio | ngb_lr: ratio | ngb_stack: ratio | ngb_lr / logreg (95% interval; better / worse; Wilcoxon p) | ngb_stack / logreg (95% interval; better / worse; Wilcoxon p) |", "|---|---|---:|---:|---:|---:|---|---|"]
		med = lambda i, m: float(np.median([value(i, f"{m}:all+conf_x1+flip{k}") for k in flips])) if all(value(i, f"{m}:all+conf_x1+flip{k}") is not None for k in flips) else None
		def stats_of(pairs):
			logs = np.array([np.log((x + 1) / (b + 1)) for x, b in pairs if x is not None and b is not None])
			boot = np.exp(np.percentile([rng.choice(logs, len(logs)).mean() for _ in range(2000)], [2.5, 97.5]))
			nz = logs[logs != 0]
			return {"n": len(logs), "ratio": float(np.exp(logs.mean())), "lo": float(boot[0]), "hi": float(boot[1]), "wins": int((logs < 0).sum()), "losses": int((logs > 0).sum()), "wilcoxon": float(wilcoxon(nz).pvalue) if len(nz) >= 6 else None}
		show = lambda st: f"{st['ratio']:.3f} ({st['lo']:.3f}-{st['hi']:.3f}; {st['wins']} / {st['losses']}; " + ("-" if st["wilcoxon"] is None else f"{st['wilcoxon']:.3f}") + ")"
		basec = lambda i: float(by[(i, "base")]["confs"])
		for title, group, suffix in groups[:4]:
			for k in [0] + flips + ["median of the perturbed"]:
				tag = "" if k == 0 else f"+flip{k}"
				conf = (lambda i, m: med(i, m)) if isinstance(k, str) else (lambda i, m, tag=tag: value(i, f"{m}:all+conf_x1{tag}"))
				r = {m: stats_of([(conf(i, m), basec(i)) for i in group]) for m in ("logreg", "ngb_lr", "ngb_stack")}
				pr = {m: stats_of([(conf(i, m), conf(i, "logreg")) for i in group]) for m in ("ngb_lr", "ngb_stack")}
				alone = "-" if k == 0 or isinstance(k, str) else f"{stats_of([(value(i, f'flip{k}'), basec(i)) for i in group])['ratio']:.3f}"
				out[f"flip/{title}/{k}"] = {"ratio": r, "paired": pr}
				md.append(f"| {title} | {'none' if k == 0 else k} | {alone} | " + " | ".join(f"{r[m]['ratio']:.3f}" for m in r) + " | " + " | ".join(show(pr[m]) for m in pr) + " |")
		md.append("")
	return md

def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("--work", required=True)
	ap.add_argument("--stage", choices=["fit", "report", "all"], default="all")
	ap.add_argument("--folds", type=int, default=5)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
	a = ap.parse_args()
	warnings.filterwarnings("ignore")
	if a.stage in ("fit", "all"):
		fit(a)
	if a.stage in ("report", "all"):
		report(a)

if __name__ == "__main__":
	main()
