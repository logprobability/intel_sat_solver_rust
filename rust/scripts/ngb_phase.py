#!/usr/bin/env python3
# NGBoost experiment, step 2: the per-variable phase predictors and their calibration, strictly out-of-sample by instance.
#
# Samples are the variables of the satisfiable instances of ngb_data.py; the label is the value of the variable in the model of the baseline run.
# Models: ngb (NGBClassifier, Bernoulli), logreg, gbm (sklearn HistGradientBoostingClassifier), gbm_iso (the same, isotonic-calibrated on inner folds split by instance),
# jw (the Jeroslow-Wang rule: the share of the positive polarity in the JW weight, read as a probability), prior (the training base rate).
# Protocols: "within": grouped k-fold inside a family (the variables of an instance are never split between training and test; the 4 planner instances: leave-one-out);
# "cross": trained on the other generated families; the planner only: "within+lf", with the planner's own lf polarity as a feature, and "within-pv", without the instance-level features.
# The unsatisfiable instances of a family get the predictions of a model trained on all the satisfiable ones of the family (stored under "within").
#
# Usage: ngb_phase.py --work DIR [--folds K] [--ngb-rows N] [--ngb-trees N]
#   Reads DIR/vars.npz and DIR/instances.json; writes DIR/preds.npz (the out-of-sample probabilities per protocol and model), DIR/phase_metrics.json and DIR/tables/1_calibration.md.
import argparse, json, os, sys, time, warnings

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from sklearn.ensemble import HistGradientBoostingClassifier
from sklearn.isotonic import IsotonicRegression
from sklearn.linear_model import LogisticRegression
from sklearn.model_selection import GroupKFold
from sklearn.pipeline import make_pipeline
from sklearn.preprocessing import StandardScaler
from sklearn.tree import DecisionTreeRegressor
from ngboost import NGBClassifier
from ngboost.distns import Bernoulli

MODELS = ["ngb", "gbm", "gbm_iso", "logreg", "jw", "prior"]
THRESHOLDS = [0.6, 0.7, 0.8, 0.9]
A = None
TIMES = {m: [0.0, 0.0, 0, 0] for m in MODELS}	# fit seconds, predict seconds, rows fitted, rows predicted

def new_gbm():
	return HistGradientBoostingClassifier(max_iter=200, learning_rate=0.1, max_leaf_nodes=15, early_stopping=False, random_state=0)

# Fit the model on the training variables (groups: their instances) and return the probabilities of the positive polarity for Xte
def fit_predict(model, Xtr, ytr, gtr, Xte, feat):
	jp, jn = feat.index("jw_pos"), feat.index("jw_neg")
	t0 = time.perf_counter()
	if model == "prior":
		predict = lambda X: np.full(len(X), ytr.mean())
	elif model == "jw":
		predict = lambda X: (X[:, jp] + 1e-9) / (X[:, jp] + X[:, jn] + 2e-9)
	elif model == "logreg":
		m = make_pipeline(StandardScaler(), LogisticRegression(C=1.0, max_iter=3000)).fit(Xtr, ytr)
		predict = lambda X: m.predict_proba(X)[:, 1]
	elif model == "gbm":
		m = new_gbm().fit(Xtr, ytr)
		predict = lambda X: m.predict_proba(X)[:, 1]
	elif model == "gbm_iso":
		# The calibration map is fitted on out-of-fold predictions of inner folds, which are split by instance too
		k = min(3, len(set(gtr)))
		if k < 2:
			return fit_predict("gbm", Xtr, ytr, gtr, Xte, feat)
		oof = np.zeros(len(ytr))
		for tr, te in GroupKFold(n_splits=k).split(Xtr, ytr, gtr):
			oof[te] = new_gbm().fit(Xtr[tr], ytr[tr]).predict_proba(Xtr[te])[:, 1]
		iso = IsotonicRegression(y_min=0, y_max=1, out_of_bounds="clip").fit(oof, ytr)
		m = new_gbm().fit(Xtr, ytr)
		predict = lambda X: iso.predict(m.predict_proba(X)[:, 1])
	elif model == "ngb":
		# NGBoost fits one tree per boosting stage on all the rows it is given; the training rows are subsampled to keep the fit to about a minute
		rows = np.arange(len(ytr))
		if len(rows) > A.ngb_rows:
			rows = np.random.RandomState(0).choice(rows, A.ngb_rows, replace=False)
		m = NGBClassifier(Dist=Bernoulli, n_estimators=A.ngb_trees, learning_rate=0.05, minibatch_frac=0.5, Base=DecisionTreeRegressor(criterion="friedman_mse", max_depth=3), verbose=False, random_state=0)
		m.fit(Xtr[rows], ytr[rows])
		predict = lambda X: m.predict_proba(X)[:, 1]
	t1 = time.perf_counter()
	p = np.asarray(predict(Xte), dtype=float)
	t2 = time.perf_counter()
	T = TIMES[model]
	T[0] += t1 - t0; T[1] += t2 - t1; T[2] += min(len(ytr), A.ngb_rows) if model == "ngb" else len(ytr); T[3] += len(Xte)
	return p

def metrics(p, y):
	p = np.clip(p, 1e-6, 1 - 1e-6)
	conf = np.maximum(p, 1 - p)
	right = (p > 0.5) == (y == 1)
	bins = np.minimum((p * 10).astype(int), 9)
	rel = []
	ece = 0.0
	for b in range(10):
		m = bins == b
		if m.any():
			rel.append((b, int(m.sum()), float(p[m].mean()), float(y[m].mean())))
			ece += m.mean() * abs(p[m].mean() - y[m].mean())
	res = {"n": int(len(y)), "pos_rate": float(y.mean()), "acc": float(right.mean()), "brier": float(((p - y) ** 2).mean()), "logloss": float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).mean()), "ece": float(ece),
		"rel": rel, "entropy": float(-(p * np.log2(p) + (1 - p) * np.log2(1 - p)).mean())}
	for t in THRESHOLDS:
		m = conf >= t
		res[f"cov{t}"] = float(m.mean())
		res[f"acc{t}"] = float(right[m].mean()) if m.any() else float("nan")
	return res

def main():
	global A
	ap = argparse.ArgumentParser()
	ap.add_argument("--work", required=True)
	ap.add_argument("--folds", type=int, default=5)
	ap.add_argument("--ngb-rows", type=int, default=30000)
	ap.add_argument("--ngb-trees", type=int, default=300)
	A = ap.parse_args()
	warnings.filterwarnings("ignore")
	d = np.load(os.path.join(A.work, "vars.npz"))
	X, y, inst = d["X"], d["y"], d["inst"]
	feat = [str(f) for f in d["features"]]
	records = json.load(open(os.path.join(A.work, "instances.json")))
	noLf = [i for i, f in enumerate(feat) if f != "lf_pol"]
	allF = list(range(len(feat)))
	perVar = [i for i in noLf if not feat[i].startswith("i_")]
	family = np.array([records[i]["family"] for i in inst])
	phase = np.array([records[i]["role"] == "phase" for i in inst])
	lab = (y >= 0) & phase
	preds = {}
	def store(protocol, model, rows, p):
		preds.setdefault(f"{protocol}:{model}", np.full(len(y), np.nan))[rows] = p
	fams = ["rand", "factor", "col", "planner"]
	importance = {}
	for fam in fams:
		rows = np.nonzero(lab & (family == fam))[0]
		groups = inst[rows]
		k = min(A.folds, len(set(groups)))
		for protocol, cols in [("within", noLf)] + ([("within+lf", allF), ("within-pv", perVar)] if fam == "planner" else []):
			fnames = [feat[c] for c in cols]
			for fold, (tr, te) in enumerate(GroupKFold(n_splits=k).split(rows, y[rows], groups)):
				assert not set(groups[tr]) & set(groups[te])
				for model in MODELS:
					store(protocol, model, rows[te], fit_predict(model, X[rows[tr]][:, cols], y[rows[tr]], groups[tr], X[rows[te]][:, cols], fnames))
				print(f"{fam} {protocol} fold {fold + 1}/{k} done", flush=True)
		# The other instances of the family (unsatisfiable, or satisfiable ones outside the phase experiment): a model of all the labelled ones
		other = np.nonzero((family == fam) & ~lab)[0]
		if len(other):
			fnames = [feat[c] for c in noLf]
			for model in MODELS:
				store("within", model, other, fit_predict(model, X[rows][:, noLf], y[rows], groups, X[other][:, noLf], fnames))
		# Cross-family: trained on the generated families other than this one
		tr = np.nonzero(lab & (family != fam) & (family != "planner"))[0]
		fnames = [feat[c] for c in noLf]
		for model in MODELS:
			store("cross", model, rows, fit_predict(model, X[tr][:, noLf], y[tr], inst[tr], X[rows][:, noLf], fnames))
		# What the gradient boosting of the family looks at (permutation-free: the split gains of a plain GBM don't exist for HistGradientBoosting, so NGBoost's own importances)
		sub = rows if len(rows) <= A.ngb_rows else np.random.RandomState(1).choice(rows, A.ngb_rows, replace=False)
		m = NGBClassifier(Dist=Bernoulli, n_estimators=100, learning_rate=0.05, minibatch_frac=0.5, Base=DecisionTreeRegressor(criterion="friedman_mse", max_depth=3), verbose=False, random_state=0).fit(X[sub][:, noLf], y[sub])
		imp = m.feature_importances_[0]
		importance[fam] = sorted(zip(map(float, imp), fnames), reverse=True)[:6]
		print(f"{fam} cross done", flush=True)
	np.savez_compressed(os.path.join(A.work, "preds.npz"), **preds)

	out = {}
	md = ["## 1. Per-variable phase prediction: calibration, out-of-sample by instance", "",
		"Generated by `scripts/ngb_phase.py`. A sample is a variable of a satisfiable instance; the label is its value in the model of the baseline run. `p` is the predicted probability of the positive polarity.",
		"ECE: 10 equal-width bins of `p`. `acc@c (cov)`: the accuracy on the variables whose confidence max(p, 1-p) is at least c, and the share of the variables they are.",
		"`entropy`: the mean predictive entropy in bits (1 = no information claimed). The majority class rate is max(positive rate, 1 - positive rate): what always answering one polarity gets.", ""]
	for protocol, title in (("within", "Within-family (grouped folds; the planner: leave-one-instance-out)"), ("within+lf", "The planner, with the planner's lf polarity as a feature (leave-one-instance-out)"),
			("within-pv", "The planner, with the per-variable features only: no instance-level feature (leave-one-instance-out)"), ("cross", "Cross-family (trained on the generated families other than the one tested)")):
		md += [f"### {title}", "", "| family | instances | variables | positive rate | model | accuracy | Brier | log loss | ECE | entropy | " + " | ".join(f"acc@{t} (cov)" for t in THRESHOLDS) + " |", "|---|---:|---:|---:|---|---:|---:|---:|---:|---:|" + "---:|" * len(THRESHOLDS)]
		for fam in fams:
			rows = np.nonzero(lab & (family == fam))[0]
			if f"{protocol}:ngb" not in preds or np.isnan(preds[f"{protocol}:ngb"][rows]).all():
				continue
			for model in MODELS:
				m = metrics(preds[f"{protocol}:{model}"][rows], y[rows])
				# The accuracy per instance
				accs = [float(((preds[f"{protocol}:{model}"][rows][inst[rows] == i] > 0.5) == (y[rows][inst[rows] == i] == 1)).mean()) for i in sorted(set(inst[rows]))]
				m["inst_acc"] = [min(accs), float(np.median(accs)), max(accs)]
				out[f"{protocol}/{fam}/{model}"] = m
				md.append(f"| {fam} | {len(accs)} | {m['n']} | {m['pos_rate']:.3f} | {model} | {m['acc']:.3f} | {m['brier']:.4f} | {m['logloss']:.4f} | {m['ece']:.4f} | {m['entropy']:.3f} | " + " | ".join(f"{m[f'acc{t}']:.3f} ({m[f'cov{t}']:.2f})" if m[f'cov{t}'] > 0 else "- (0.00)" for t in THRESHOLDS) + " |")
		md.append("")
	md += ["### Accuracy per held-out instance (within-family): minimum / median / maximum over the instances", "", "| family | " + " | ".join(MODELS) + " |", "|---|" + "---|" * len(MODELS)]
	for fam in fams:
		md.append(f"| {fam} | " + " | ".join("{:.3f} / {:.3f} / {:.3f}".format(*out[f"within/{fam}/{m}"]["inst_acc"]) for m in MODELS) + " |")
	md += ["", "### Reliability tables (within-family): per bin of p, the number of variables, the mean predicted p and the observed frequency of the positive polarity", ""]
	for fam in fams:
		md += [f"**{fam}**", "", "| bin of p | " + " | ".join(f"{m}: n | mean p | observed" for m in ("ngb", "gbm", "gbm_iso", "logreg", "jw")) + " |", "|---|" + "---:|---:|---:|" * 5]
		for b in range(10):
			cells = []
			for m in ("ngb", "gbm", "gbm_iso", "logreg", "jw"):
				r = [x for x in out[f"within/{fam}/{m}"]["rel"] if x[0] == b]
				cells.append(f"{r[0][1]} | {r[0][2]:.3f} | {r[0][3]:.3f}" if r else "0 | - | -")
			md.append(f"| {b / 10:.1f}-{(b + 1) / 10:.1f} | " + " | ".join(cells) + " |")
		md.append("")
	md += ["### The features NGBoost uses most (its feature importances for the logit, a model of all the satisfiable instances of the family)", "", "| family | features (importance) |", "|---|---|"]
	for fam in fams:
		md.append(f"| {fam} | " + ", ".join(f"`{n}` ({v:.2f})" for v, n in importance[fam]) + " |")
	md += ["", "### Training and inference time of the predictors (all the fits of this script)", "", "| model | fit: seconds per 10000 training variables | inference: microseconds per variable |", "|---|---:|---:|"]
	for m in MODELS:
		T = TIMES[m]
		md.append(f"| {m} | {T[0] / max(T[2], 1) * 1e4:.2f} | {T[1] / max(T[3], 1) * 1e6:.2f} |")
	out["times"] = TIMES
	os.makedirs(os.path.join(A.work, "tables"), exist_ok=True)
	with open(os.path.join(A.work, "tables", "1_calibration.md"), "w") as f:
		f.write("\n".join(md) + "\n")
	with open(os.path.join(A.work, "phase_metrics.json"), "w") as f:
		json.dump(out, f, indent=1)
	print("\n".join(md))

if __name__ == "__main__":
	main()
