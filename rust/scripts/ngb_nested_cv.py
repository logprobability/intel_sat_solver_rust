#!/usr/bin/env python3
# NGBoost experiment, nested ("double") cross-validation of the per-variable phase predictors: the protocol pre-registered in docs/search_quality/ngboost_nested_cv.md.
#
# Methods: jw (no fitting), logreg, gbm, gbm_iso, ngb, ngb_lr (ngb_lr.OffsetBernoulliNGB from cross-fitted logreg logits), ngb_stack (ngb with the logreg logit as a feature).
# Outer loop: per family (random 3-SAT, factoring, 3-colouring) 5 folds by instance, repeated with the seeds 101, 202, 303; inner loop: 4 folds by instance on the labelled
# outer-training instances, where every method's grid point and round count are chosen by the mean inner log loss (rounds: the argmin of the fold-averaged validation curve).
# Planner: leave-one-instance-out over the 4 .plain instances, per-variable features only, with each method's settings fixed from the random 3-SAT inner loops (descriptive).
#
# Stages (every fitted fold and every solver run is checkpointed in OUT; a restart with the same command continues where it stopped):
#   provenance  rerun a sample of the cached runs (WORK/solve_runs.csv) with --exe and compare the conflicts exactly; writes OUT/provenance.json
#   fit         the nested cross-validation; writes OUT/fits/<family>_<seed>_<fold>/<method>.npz and OUT/fits/planner_<k>/<method>.npz
#   solve       the solver runs of both strategies x 3 flip settings, and the baselines; appends to OUT/runs.csv; exits 1 on a SAT-versus-UNSAT contradiction
#   report      the tables; writes OUT/report.md and OUT/report.json
#
# Usage: ngb_nested_cv.py --work WORK --out OUT --stage provenance|fit|solve|report [--exe PATH] [--jobs N] [--dry-run]
#   WORK: the cached data of ngb_data.py (vars.npz, instances.json, and for provenance solve_runs.csv and hints/); it is only read.
import os
for v in ("OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "MKL_NUM_THREADS", "VECLIB_MAXIMUM_THREADS"):
	os.environ.setdefault(v, "1")	# one thread per worker process: the parallelism is over the folds
import argparse, csv, hashlib, json, sys, threading, time, warnings
from concurrent.futures import ThreadPoolExecutor, as_completed

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from scipy.special import expit
from scipy.stats import wilcoxon
from sklearn.ensemble import HistGradientBoostingClassifier
from sklearn.isotonic import IsotonicRegression
from sklearn.linear_model import LogisticRegression
from sklearn.pipeline import make_pipeline
from sklearn.preprocessing import StandardScaler
from sklearn.tree import DecisionTreeRegressor
from ngboost import NGBClassifier
from ngboost.distns import Bernoulli
from ngb_common import VERIFY, hints_text, write
from ngb_lr import OffsetBernoulliNGB, LOGIT_CLIP, paired_metrics
from ngb_phase import metrics
from search_quality_bench import run_one

# The pre-registered protocol
FAMILIES = ["rand", "factor", "col"]
TITLES = {"rand": "random 3-SAT", "factor": "factoring", "col": "3-colouring", "planner": "planner"}
SEEDS = [101, 202, 303]
K_OUTER, K_INNER = 5, 4
METHODS = ["jw", "logreg", "gbm", "gbm_iso", "ngb", "ngb_lr", "ngb_stack"]
FITTED = ["logreg", "gbm", "ngb", "ngb_lr", "ngb_stack"]	# gbm_iso is stored with gbm
CHALLENGERS = ["jw", "gbm", "gbm_iso", "ngb", "ngb_lr", "ngb_stack"]
C_GRID = [0.001, 0.01, 0.1, 1, 10, 100]
TREE_GRID = [(d, lr) for d in (2, 3, 4) for lr in (0.03, 0.1)]	# ngb, ngb_lr, ngb_stack: (tree depth, learning rate)
GBM_GRID = [(d, lr) for d in (2, 4, 8) for lr in (0.03, 0.1)]	# gbm: (max_depth, learning rate)
NGB_CAP = {0.03: 400, 0.1: 200}
GBM_CAP = {0.03: 1000, 0.1: 400}
MAX_ROWS = 30000
STRATEGIES = ["all", "all+conf_x1"]
FLIPS = [0, 1000, 2000]
MIN_CONFS = 200
TIMEOUT = 120
BOOT = 10000
ON = ["/phase/hints/on", "1"]
MULT1 = ["/decision/score_hints/mult", "1"]

# ---------------------------------------------------------------- data and folds

_D = {}
def data(work):
	if "X" not in _D:
		d = np.load(os.path.join(work, "vars.npz"))
		_D.update(X=d["X"], y=d["y"], inst=d["inst"], var=d["var"], feat=[str(f) for f in d["features"]])
		_D["records"] = json.load(open(os.path.join(work, "instances.json")))
		feat = _D["feat"]
		_D["noLf"] = [i for i, f in enumerate(feat) if f != "lf_pol"]
		_D["perVar"] = [i for i in _D["noLf"] if not feat[i].startswith("i_")]
		_D["rowsOf"] = {}
		order = np.argsort(_D["inst"], kind="stable")
		bounds = np.searchsorted(_D["inst"][order], np.arange(len(_D["records"]) + 1))
		for i in range(len(_D["records"])):
			_D["rowsOf"][i] = np.sort(order[bounds[i]:bounds[i + 1]])
	return _D

def rows_of(D, ids):
	return np.concatenate([D["rowsOf"][int(i)] for i in ids]) if len(ids) else np.array([], dtype=int)

# The instances of a family in the experiment (phase role, decided): (satisfiable ids, unsatisfiable ids), ascending
def family_ids(D, fam):
	rs = D["records"]
	sat = [i for i, r in enumerate(rs) if r["family"] == fam and r["role"] == "phase" and r["status"] == "SAT"]
	unsat = [i for i, r in enumerate(rs) if r["family"] == fam and r["role"] == "phase" and r["status"] == "UNSAT"]
	return np.array(sat), np.array(unsat)

# Outer folds of one repeat: fold index per instance id (satisfiable and unsatisfiable permuted separately, dealt round-robin)
def outer_folds(D, fam, seed):
	sat, unsat = family_ids(D, fam)
	rs = np.random.RandomState(seed)
	ps, pu = rs.permutation(sat), rs.permutation(unsat)
	fold = {}
	for j, i in enumerate(ps):
		fold[int(i)] = j % K_OUTER
	for j, i in enumerate(pu):
		fold[int(i)] = j % K_OUTER
	return fold

def split(D, fam, seed, k):
	fold = outer_folds(D, fam, seed)
	sat, _ = family_ids(D, fam)
	trIds = np.array(sorted(int(i) for i in sat if fold[int(i)] != k))
	teIds = np.array(sorted(i for i, f in fold.items() if f == k))
	perm = np.random.RandomState(1000 * seed + k).permutation(trIds)
	inner = [np.sort(perm[j::K_INNER]) for j in range(K_INNER)]
	assert not set(trIds) & set(teIds)
	return trIds, teIds, inner

def cap(rows):
	return rows if len(rows) <= MAX_ROWS else np.random.RandomState(0).choice(rows, MAX_ROWS, replace=False)

def ll(p, y):
	p = np.clip(p, 1e-6, 1 - 1e-6)
	return float(-(y * np.log(p) + (1 - y) * np.log(1 - p)).mean())

def jw_p(D, rows):
	jp, jn = D["feat"].index("jw_pos"), D["feat"].index("jw_neg")
	X = D["X"][rows]
	return (X[:, jp] + 1e-9) / (X[:, jp] + X[:, jn] + 2e-9)

def pad(curve, n):
	curve = list(curve)
	return np.array(curve + [curve[-1]] * (n - len(curve)), dtype=float)

# ---------------------------------------------------------------- the methods: inner selection and final fit
# Each takes the outer-training rows (Xtr, ytr, their instance ids gtr), the inner folds (lists of instance ids), the outer-test rows Xte,
# and (ngb_lr, ngb_stack) the logreg result; returns a dict with the test probabilities "p" and what was chosen.
# fixed: settings given (planner), no inner loop.

def inner_masks(gtr, inner):
	res = []
	for ids in inner:
		va = np.isin(gtr, ids)
		assert va.any() and (~va).any() and not set(gtr[va]) & set(gtr[~va])
		res.append((np.nonzero(~va)[0], np.nonzero(va)[0]))
	return res

def new_logreg(C):
	return make_pipeline(StandardScaler(), LogisticRegression(C=C, max_iter=3000))

def fit_logreg(Xtr, ytr, gtr, inner, Xte, fixed=None):
	masks = inner_masks(gtr, inner)
	scores = {}
	oofByC = {}
	for C in ([fixed["C"]] if fixed else C_GRID):
		oof = np.zeros(len(ytr))
		s = []
		for tr, va in masks:
			m = new_logreg(C).fit(Xtr[tr], ytr[tr])
			oof[va] = m.decision_function(Xtr[va])
			s.append(ll(expit(oof[va]), ytr[va]))
		scores[C] = float(np.mean(s))
		oofByC[C] = oof
	C = fixed["C"] if fixed else min(C_GRID, key=lambda c: (scores[c], C_GRID.index(c)))
	m = new_logreg(C).fit(Xtr, ytr)
	logitTe = np.clip(m.decision_function(Xte), -LOGIT_CLIP, LOGIT_CLIP)
	return {"p": expit(logitTe), "logit_te": logitTe, "oof": np.clip(oofByC[C], -LOGIT_CLIP, LOGIT_CLIP), "C": C, "scores": {str(c): v for c, v in scores.items()}}

def new_gbm(d, lr, n):
	return HistGradientBoostingClassifier(max_iter=n, learning_rate=lr, max_depth=d, max_leaf_nodes=15, early_stopping=False, random_state=0)

def fit_gbm(Xtr, ytr, gtr, inner, Xte, fixed=None):
	masks = inner_masks(gtr, inner)
	grid = {}
	if fixed:
		d, lr, n = fixed["depth"], fixed["lr"], fixed["rounds"]
	else:
		for d, lr in GBM_GRID:
			cmax = GBM_CAP[lr]
			curves = []
			for tr, va in masks:
				tr = cap(tr)
				m = new_gbm(d, lr, cmax).fit(Xtr[tr], ytr[tr])
				curves.append(pad([ll(p[:, 1], ytr[va]) for p in m.staged_predict_proba(Xtr[va])], cmax))
			avg = np.mean(curves, axis=0)
			grid[(d, lr)] = (float(avg.min()), int(np.argmin(avg)) + 1, cmax, float(avg[-1]))
		d, lr = min(GBM_GRID, key=lambda g: (grid[g][0], GBM_GRID.index(g)))
		n = grid[(d, lr)][1]
	# gbm_iso: the isotonic map on the inner out-of-fold predictions at the chosen settings
	oof = np.zeros(len(ytr))
	for tr, va in masks:
		tr = cap(tr)
		oof[va] = new_gbm(d, lr, n).fit(Xtr[tr], ytr[tr]).predict_proba(Xtr[va])[:, 1]
	iso = IsotonicRegression(y_min=0, y_max=1, out_of_bounds="clip").fit(oof, ytr)
	full = cap(np.arange(len(ytr)))
	p = new_gbm(d, lr, n).fit(Xtr[full], ytr[full]).predict_proba(Xte)[:, 1]
	return {"p": p, "p_iso": iso.predict(p), "depth": d, "lr": lr, "rounds": n, "cap": GBM_CAP[lr], "grid": {f"{g[0]}/{g[1]}": v for g, v in grid.items()}}

def new_ngb(d, lr, n):
	return NGBClassifier(Dist=Bernoulli, n_estimators=n, learning_rate=lr, minibatch_frac=0.5, Base=DecisionTreeRegressor(criterion="friedman_mse", max_depth=d), verbose=False, random_state=0)

# ngb and ngb_stack (stack: the logreg logit appended as a feature: out-of-fold for the training rows, from the full logreg for the test rows)
def fit_ngb(Xtr, ytr, gtr, inner, Xte, lr_res=None, fixed=None):
	if lr_res is not None:
		Xtr, Xte = np.column_stack([Xtr, lr_res["oof"]]), np.column_stack([Xte, lr_res["logit_te"]])
	masks = inner_masks(gtr, inner)
	grid = {}
	if fixed:
		d, lr, n = fixed["depth"], fixed["lr"], fixed["rounds"]
	else:
		for d, lr in TREE_GRID:
			cmax = NGB_CAP[lr]
			curves = []
			for tr, va in masks:
				tr = cap(tr)
				m = new_ngb(d, lr, cmax).fit(Xtr[tr], ytr[tr])
				# the log loss with p clipped as in metrics(): ngboost's own validation score is unclipped and becomes infinite on near-deterministic variables
				curves.append(pad([ll(q[:, 1], ytr[va]) for q in m.staged_predict_proba(Xtr[va])], cmax))
			avg = np.mean(curves, axis=0)
			grid[(d, lr)] = (float(avg.min()), int(np.argmin(avg)) + 1, cmax, float(avg[-1]))
		d, lr = min(TREE_GRID, key=lambda g: (grid[g][0], TREE_GRID.index(g)))
		n = grid[(d, lr)][1]
	full = cap(np.arange(len(ytr)))
	p = new_ngb(d, lr, n).fit(Xtr[full], ytr[full]).predict_proba(Xte)[:, 1]
	return {"p": p, "depth": d, "lr": lr, "rounds": n, "cap": NGB_CAP[lr], "grid": {f"{g[0]}/{g[1]}": v for g, v in grid.items()}}

def fit_ngb_lr(Xtr, ytr, gtr, inner, Xte, lr_res, fixed=None):
	oof, offTe = lr_res["oof"], lr_res["logit_te"]
	masks = inner_masks(gtr, inner)
	grid = {}
	if fixed:
		d, lr, n = fixed["depth"], fixed["lr"], fixed["rounds"]
	else:
		for d, lr in TREE_GRID:
			cmax = NGB_CAP[lr]
			curves = []
			for tr, va in masks:
				tr = cap(tr)
				m = OffsetBernoulliNGB(rate=lr, depth=d, patience=None).fit(Xtr[tr], ytr[tr], oof[tr], cmax)
				theta = np.array(oof[va], dtype=float)
				curve = [ll(expit(theta), ytr[va])]	# index = rounds, 0 included; p clipped as in metrics()
				for tree, scale in zip(m.trees, m.scales):
					theta -= lr * scale * tree.predict(Xtr[va])
					curve.append(ll(expit(theta), ytr[va]))
				curves.append(pad(curve, cmax + 1))
			avg = np.mean(curves, axis=0)
			grid[(d, lr)] = (float(avg.min()), int(np.argmin(avg)), cmax, float(avg[0]))
		d, lr = min(TREE_GRID, key=lambda g: (grid[g][0], TREE_GRID.index(g)))
		n = grid[(d, lr)][1]
	full = cap(np.arange(len(ytr)))
	m = OffsetBernoulliNGB(rate=lr, depth=d, patience=None).fit(Xtr[full], ytr[full], oof[full], n)
	return {"p": expit(m.logit(Xte, offTe)), "depth": d, "lr": lr, "rounds": n, "cap": NGB_CAP[lr], "grid": {f"{g[0]}/{g[1]}": v for g, v in grid.items()}}

# ---------------------------------------------------------------- the fit stage

def save(path, res):
	arrays = {k: v for k, v in res.items() if isinstance(v, np.ndarray)}
	info = {k: v for k, v in res.items() if not isinstance(v, np.ndarray)}
	tmp = path + ".tmp.npz"
	np.savez_compressed(tmp, info=json.dumps(info), **arrays)
	os.replace(tmp, path)

def load(path):
	z = np.load(path)
	res = json.loads(str(z["info"]))
	res.update({k: z[k] for k in z.files if k != "info"})
	return res

def fold_dir(out, fam, seed, k):
	return os.path.join(out, "fits", f"{fam}_{seed}_{k}")

# One (family, repeat, outer fold, method): checkpointed as its own file
def fit_task(work, out, fam, seed, k, method):
	warnings.filterwarnings("ignore")
	D = data(work)
	dd = fold_dir(out, fam, seed, k)
	os.makedirs(dd, exist_ok=True)
	path = os.path.join(dd, method + ".npz")
	if os.path.exists(path):
		return path, 0.0
	t0 = time.time()
	trIds, teIds, inner = split(D, fam, seed, k)
	trRows, teRows = rows_of(D, trIds), rows_of(D, teIds)
	cols = D["noLf"]
	Xtr, ytr, gtr, Xte = D["X"][trRows][:, cols], D["y"][trRows], D["inst"][trRows], D["X"][teRows][:, cols]
	assert (ytr >= 0).all()
	if method == "logreg":
		res = fit_logreg(Xtr, ytr, gtr, inner, Xte)
	else:
		lr_res = load(os.path.join(dd, "logreg.npz")) if method in ("ngb_lr", "ngb_stack") else None
		if method == "gbm":
			res = fit_gbm(Xtr, ytr, gtr, inner, Xte)
		elif method == "ngb":
			res = fit_ngb(Xtr, ytr, gtr, inner, Xte)
		elif method == "ngb_stack":
			res = fit_ngb(Xtr, ytr, gtr, inner, Xte, lr_res=lr_res)
		elif method == "ngb_lr":
			res = fit_ngb_lr(Xtr, ytr, gtr, inner, Xte, lr_res)
	res["te_rows"] = teRows
	res["te_ids"] = teIds
	res["seconds"] = time.time() - t0
	save(path, res)
	return path, res["seconds"]

# The planner: leave-one-instance-out over the .plain instances, per-variable features, settings fixed from the random 3-SAT inner loops
def planner_settings(out):
	fixed = {}
	for method in FITTED:
		chosen = []
		for seed in SEEDS:
			for k in range(K_OUTER):
				r = load(os.path.join(fold_dir(out, "rand", seed, k), method + ".npz"))
				if method == "logreg":
					chosen.append(((r["C"],), r["scores"][str(r["C"])], 0))
				else:
					g = f"{r['depth']}/{r['lr']}"
					chosen.append(((r["depth"], r["lr"]), r["grid"][g][0], r["rounds"]))
		keys = sorted({c[0] for c in chosen})
		count = {key: sum(c[0] == key for c in chosen) for key in keys}
		score = {key: np.mean([c[1] for c in chosen if c[0] == key]) for key in keys}
		best = min(keys, key=lambda key: (-count[key], score[key]))
		rounds = int(np.floor(np.median([c[2] for c in chosen if c[0] == best])))
		fixed[method] = {"C": best[0]} if method == "logreg" else {"depth": best[0], "lr": best[1], "rounds": rounds}
		fixed[method]["count"] = count[best]
	return fixed

def planner_ids(D):
	return [i for i, r in enumerate(D["records"]) if r["family"] == "planner" and r["role"] == "phase"]

def planner_task(work, out, k, fixed):
	warnings.filterwarnings("ignore")
	D = data(work)
	ids = planner_ids(D)
	dd = os.path.join(out, "fits", f"planner_{k}")
	os.makedirs(dd, exist_ok=True)
	path = os.path.join(dd, "all.npz")
	if os.path.exists(path):
		return path
	trIds = np.array([i for j, i in enumerate(ids) if j != k])
	teIds = np.array([ids[k]])
	inner = [np.array([i]) for i in trIds]	# leave-one-instance-out over the 3 training instances (gbm_iso's map, the cross-fitted logits)
	trRows, teRows = rows_of(D, trIds), rows_of(D, teIds)
	cols = D["perVar"]
	Xtr, ytr, gtr, Xte = D["X"][trRows][:, cols], D["y"][trRows], D["inst"][trRows], D["X"][teRows][:, cols]
	lr_res = fit_logreg(Xtr, ytr, gtr, inner, Xte, fixed=fixed["logreg"])
	g = fit_gbm(Xtr, ytr, gtr, inner, Xte, fixed=fixed["gbm"])
	res = {"logreg": lr_res["p"], "gbm": g["p"], "gbm_iso": g["p_iso"], "ngb": fit_ngb(Xtr, ytr, gtr, inner, Xte, fixed=fixed["ngb"])["p"],
		"ngb_stack": fit_ngb(Xtr, ytr, gtr, inner, Xte, lr_res=lr_res, fixed=fixed["ngb_stack"])["p"], "ngb_lr": fit_ngb_lr(Xtr, ytr, gtr, inner, Xte, lr_res, fixed=fixed["ngb_lr"])["p"],
		"jw": jw_p(D, teRows), "te_rows": teRows, "te_ids": teIds, "fixed": fixed}
	save(path, res)
	return path

def stage_fit(a):
	from joblib import Parallel, delayed
	D = data(a.work)
	logs = []
	first = [(f, s, k, "logreg") for f in FAMILIES for s in SEEDS for k in range(K_OUTER)]
	# the heavy methods, the largest families first
	rest = [(f, s, k, m) for m in ("ngb_lr", "ngb_stack", "ngb", "gbm") for f in ("factor", "col", "rand") for s in SEEDS for k in range(K_OUTER)]
	for batch in (first, rest):
		todo = [t for t in batch if not os.path.exists(os.path.join(fold_dir(a.out, t[0], t[1], t[2]), t[3] + ".npz"))]
		print(f"{len(batch) - len(todo)} of {len(batch)} fits already done; fitting {len(todo)}", flush=True)
		for path, sec in Parallel(n_jobs=a.jobs, verbose=10, return_as="generator_unordered")(delayed(fit_task)(a.work, a.out, *t) for t in todo):
			logs.append((path, sec))
	fixed = planner_settings(a.out)
	print("planner settings (from the random 3-SAT inner loops):", fixed, flush=True)
	with open(os.path.join(a.out, "planner_settings.json"), "w") as f:
		json.dump(fixed, f, indent=1)
	Parallel(n_jobs=min(a.jobs, 4))(delayed(planner_task)(a.work, a.out, k, fixed) for k in range(len(planner_ids(D))))
	print("fit done", flush=True)

# ---------------------------------------------------------------- predictions per (family, repeat, instance)

# {(fam, seed, instance id): {method: p over the rows of the instance}}, and the rows of each instance
def predictions(a):
	D = data(a.work)
	P = {}
	for fam in FAMILIES:
		for seed in SEEDS:
			for k in range(K_OUTER):
				dd = fold_dir(a.out, fam, seed, k)
				res = {m: load(os.path.join(dd, m + ".npz")) for m in FITTED}
				teRows = res["logreg"]["te_rows"]
				for m in FITTED:
					assert np.array_equal(res[m]["te_rows"], teRows)
				probs = {m: res[m]["p"] for m in FITTED}
				probs["gbm_iso"] = res["gbm"]["p_iso"]
				probs["jw"] = jw_p(D, teRows)
				for i in res["logreg"]["te_ids"]:
					sel = D["inst"][teRows] == i
					P[(fam, seed, int(i))] = {m: probs[m][sel] for m in METHODS}
	for k in range(len(planner_ids(D))):
		r = load(os.path.join(a.out, "fits", f"planner_{k}", "all.npz"))
		P[("planner", 0, int(r["te_ids"][0]))] = {m: r[m] for m in METHODS}
	return P

# ---------------------------------------------------------------- solver runs

FIELDS = ["key", "instance", "params", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts"]

def flip_params(f):
	return ["/decision/polarity/flip_factor", str(f)] if f else []

def run_key(path, params):
	return hashlib.sha1((os.path.basename(path) + "\0" + " ".join(params)).encode()).hexdigest()

def hints_path(out, text):
	h = hashlib.sha1(text.encode()).hexdigest()
	p = os.path.join(out, "hints", h + ".hints")
	if not os.path.exists(p):
		write(p + ".tmp", text)
		os.replace(p + ".tmp", p)
	return p

def strategy_hints(var, p, strategy):
	lits = [int(v) if q > 0.5 else -int(v) for v, q in zip(var, p)]
	if strategy == "all":
		return ON, hints_text(lits)
	return ON + MULT1, hints_text(lits, list(zip(var, 2 * np.abs(p - 0.5) + 1e-3)))

# The solver runs of the protocol: [(cell, path, params)], cell = (fam, seed, instance id, method or "base", strategy or "", flip, version)
def plan(a, P):
	D = data(a.work)
	os.makedirs(os.path.join(a.out, "hints"), exist_ok=True)
	recs = D["records"]
	byName = {r["name"]: i for i, r in enumerate(recs)}
	cells = []
	baseDone = set()
	for (fam, seed, i), probs in P.items():
		rows = D["rowsOf"][i]
		var = D["var"][rows]
		paths = [(recs[i]["path"], "plain" if fam == "planner" else "")]
		if fam == "planner":
			paths.append((recs[byName[recs[i]["name"].replace(".plain", ".lf")]]["path"], "lf"))
		for path, version in paths:
			for f in FLIPS:
				if (path, f) not in baseDone:
					baseDone.add((path, f))
					cells.append(((fam, 0, i, "base", "", f, version), path, VERIFY + flip_params(f)))
				for m in METHODS:
					for s in STRATEGIES:
						params, text = strategy_hints(var, probs[m], s)
						cells.append(((fam, seed, i, m, s, f, version), path, VERIFY + params + flip_params(f) + ["/topor_tool/hints_file", hints_path(a.out, text)]))
	return cells

def read_runs(out):
	path = os.path.join(out, "runs.csv")
	runs = {}
	if os.path.exists(path):
		for r in csv.DictReader(open(path)):
			r["confs"] = int(r["confs"])
			r["cpu"] = float(r["cpu"])
			runs[r["key"]] = r
	return runs

def execute(exe, jobs, out, todo, csvName="runs.csv"):
	path = os.path.join(out, csvName)
	new = not os.path.exists(path)
	lock = threading.Lock()
	f = open(path, "a", newline="")
	w = csv.DictWriter(f, fieldnames=FIELDS, extrasaction="ignore")
	if new:
		w.writeheader()
	done = 0
	t0 = time.time()
	def one(key, inst, params):
		r = run_one(exe, inst, params, TIMEOUT)
		r.update({"key": key, "instance": os.path.basename(inst), "params": " ".join(params)})
		return r
	with ThreadPoolExecutor(max_workers=jobs) as ex:
		futs = [ex.submit(one, *t) for t in todo]
		for fu in as_completed(futs):
			r = fu.result()
			with lock:
				w.writerow(r)
				f.flush()
				done += 1
				if done % 1000 == 0 or done == len(todo):
					print(f"{done}/{len(todo)} runs, {time.time() - t0:.0f} s", flush=True)
	f.close()

def stage_solve(a):
	P = predictions(a)
	cells = plan(a, P)
	runs = read_runs(a.out)
	todo, seen = [], set()
	for _, path, params in cells:
		key = run_key(path, params)
		if key not in runs and key not in seen:
			seen.add(key)
			todo.append((key, path, params))
	uniq = len({run_key(p, q) for _, p, q in cells})
	print(f"{len(cells)} cells, {uniq} distinct runs, {uniq - len(todo)} already done, {len(todo)} to run", flush=True)
	if a.dry_run:
		return
	if todo:
		execute(a.exe, a.jobs, a.out, todo)
	check(a)

# SAT versus UNSAT between any two runs of the same instance file
def check(a):
	runs = read_runs(a.out)
	by = {}
	for r in runs.values():
		by.setdefault(r["instance"], []).append(r)
	bad = []
	for inst, rs in by.items():
		vs = {v for r in rs for v in r["verdicts"].split()[-1:]}
		if "S" in vs and "U" in vs:
			bad.append(inst)
	other = [r for r in runs.values() if r["status"] not in ("SAT", "UNSAT")]
	print(f"{len(runs)} runs; SAT-versus-UNSAT contradictions: {len(bad)} {bad[:10]}; runs without an answer: {len(other)}", flush=True)
	if bad:
		sys.exit(1)

# ---------------------------------------------------------------- provenance

def stage_provenance(a):
	D = data(a.work)
	recs = D["records"]
	cached = {(os.path.splitext(r["instance"])[0], r["config"]): r for r in csv.DictReader(open(os.path.join(a.work, "solve_runs.csv")))}
	rng = np.random.RandomState(0)
	sample = []
	for fam in FAMILIES:
		sat, unsat = family_ids(D, fam)
		sample += list(rng.choice(np.concatenate([sat, unsat]), 10, replace=False))
	todo = []
	for i in sample:
		r = recs[int(i)]
		todo.append((r["name"], "base", r["path"], VERIFY))
		todo.append((r["name"], "logreg:all+conf_x1", r["path"], VERIFY + ON + MULT1 + ["/topor_tool/hints_file", os.path.join(a.work, "hints", f"{r['name']}.logreg:all+conf_x1.hints")]))
	res = []
	for exe in (a.exe, a.old_exe):
		if not exe:
			continue
		with ThreadPoolExecutor(max_workers=a.jobs) as ex:
			outs = list(ex.map(lambda t: run_one(exe, t[2], t[3], TIMEOUT), todo))
		for (name, config, _, _), o in zip(todo, outs):
			c = cached[(name, config)]
			res.append({"exe": exe, "instance": name, "config": config, "cached": int(c["confs"]), "now": o["confs"], "cached_status": c["status"], "status": o["status"]})
	summary = {}
	for exe in {r["exe"] for r in res}:
		for config in ("base", "logreg:all+conf_x1"):
			rs = [r for r in res if r["exe"] == exe and r["config"] == config]
			summary[f"{exe}|{config}"] = {"n": len(rs), "identical": sum(r["cached"] == r["now"] and r["cached_status"] == r["status"] for r in rs)}
	print(json.dumps(summary, indent=1))
	with open(os.path.join(a.out, "provenance.json"), "w") as f:
		json.dump({"summary": summary, "runs": res}, f, indent=1)

# ---------------------------------------------------------------- report

def holm(ps):
	order = np.argsort(ps)
	adj = np.empty(len(ps))
	run = 0.0
	for j, i in enumerate(order):
		run = max(run, min(1.0, (len(ps) - j) * ps[i]))
		adj[i] = run
	return adj

def summarize(logs, rng):
	logs = np.asarray(logs, dtype=float)
	if len(logs) == 0:
		return None
	idx = rng.randint(len(logs), size=(BOOT, len(logs)))
	means = logs[idx].mean(axis=1)
	nz = logs[logs != 0]
	return {"n": len(logs), "ratio": float(np.exp(logs.mean())), "lo": float(np.exp(np.percentile(means, 2.5))), "hi": float(np.exp(np.percentile(means, 97.5))),
		"wins": int((logs < 0).sum()), "losses": int((logs > 0).sum()), "ties": int((logs == 0).sum()), "p": float(wilcoxon(nz).pvalue) if len(nz) >= 6 else 1.0}

def fmt(s, p_adj=None):
	if s is None:
		return "-"
	t = f"{s['ratio']:.3f} ({s['lo']:.2f}-{s['hi']:.2f}) {s['wins']}/{s['losses']}/{s['ties']}, p {s['p']:.3f}"
	return t + (f", Holm {p_adj:.3f}" if p_adj is not None else "")

def stage_report(a):
	D = data(a.work)
	recs = D["records"]
	P = predictions(a)
	cells = plan(a, P)
	runs = read_runs(a.out)
	missing = [c for c, p, q in cells if run_key(p, q) not in runs]
	assert not missing, f"{len(missing)} runs missing, e.g. {missing[:3]}: run the solve stage"
	R = {c: runs[run_key(p, q)] for c, p, q in cells}
	out = {"runs": len(runs), "cells": len(cells)}
	md = ["# Nested cross-validation of the phase predictors: generated tables", "", f"Generated by `scripts/ngb_nested_cv.py --stage report`: {len(runs)} distinct solver runs for {len(cells)} cells (instance x repeat x method x strategy x flip setting, and the baselines).", ""]
	# Soundness
	byInst = {}
	for r in runs.values():
		byInst.setdefault(r["instance"], set()).update(r["verdicts"].split()[-1:])
	bad = [i for i, v in byInst.items() if {"S", "U"} <= v]
	noAns = [r for r in runs.values() if r["status"] not in ("SAT", "UNSAT")]
	out["contradictions"], out["no_answer"] = bad, len(noAns)
	md += [f"**SAT-versus-UNSAT contradictions: {len(bad)}**" + (f" ({', '.join(bad)})" if bad else "") + f". Runs without an answer (timeout {TIMEOUT} s or error): {len(noAns)}.", ""]

	# ---- calibration
	md += ["## Calibration on the outer-test variables", "", "Per family and method: the metric over all labelled outer-test variables of a repeat, the mean over the 3 repeats, and in brackets the min to max over the 15 outer folds.",
		"`d log loss`: the method minus `logreg`, pooled over the variables of the 3 repeats, with a 95% bootstrap interval over the instances.", "",
		"| family | method | accuracy | Brier | log loss | ECE | d log loss vs logreg |", "|---|---|---|---|---|---|---|"]
	rng = np.random.RandomState(0)
	for fam in FAMILIES:
		sat, _ = family_ids(D, fam)
		for m in METHODS:
			rep, folds = [], []
			pa, pb, yy, gg = [], [], [], []
			for seed in SEEDS:
				fold = outer_folds(D, fam, seed)
				ps, ys = [], []
				for k in range(K_OUTER):
					ids = [int(i) for i in sat if fold[int(i)] == k]
					p = np.concatenate([P[(fam, seed, i)][m] for i in ids]); yk = np.concatenate([D["y"][D["rowsOf"][i]] for i in ids])
					folds.append(metrics(p, yk)); ps.append(p); ys.append(yk)
				rep.append(metrics(np.concatenate(ps), np.concatenate(ys)))
				for i in sat:
					i = int(i)
					pa.append(P[(fam, seed, i)][m]); pb.append(P[(fam, seed, i)]["logreg"]); yy.append(D["y"][D["rowsOf"][i]]); gg.append(np.full(len(D["rowsOf"][i]), i))
			cell = lambda key, f="{:.4f}": f.format(np.mean([r[key] for r in rep])) + " (" + f.format(min(r[key] for r in folds)) + "-" + f.format(max(r[key] for r in folds)) + ")"
			d = paired_metrics(np.concatenate(pa), np.concatenate(pb), np.concatenate(yy), np.concatenate(gg), rng)["logloss"] if m != "logreg" else None
			out[f"calib/{fam}/{m}"] = {k: float(np.mean([r[k] for r in rep])) for k in ("acc", "brier", "logloss", "ece")} | {"folds": {k: [float(r[k]) for r in folds] for k in ("acc", "brier", "logloss", "ece")}, "d_logloss": d}
			md.append(f"| {TITLES[fam]} | {m} | {cell('acc', '{:.3f}')} | {cell('brier')} | {cell('logloss')} | {cell('ece')} | " + ("-" if d is None else f"{d[0]:+.4f} ({d[1]:+.4f} to {d[2]:+.4f})") + " |")
	md.append("")

	# ---- chosen hyperparameters
	md += ["## Chosen hyperparameters per outer fold", "", "Per repeat (seed), the five outer folds in order. `logreg`: C. Boosting: depth / learning rate / rounds (`*`: at the cap of that learning rate). `ngb_lr` rounds 0 = the logistic regression itself.", "",
		"| family | seed | logreg C | gbm | ngb | ngb_lr | ngb_stack |", "|---|---|---|---|---|---|---|"]
	hyper = {}
	for fam in FAMILIES:
		for seed in SEEDS:
			row = []
			for m in FITTED:
				cs = []
				for k in range(K_OUTER):
					r = load(os.path.join(fold_dir(a.out, fam, seed, k), m + ".npz"))
					if m == "logreg":
						cs.append(f"{r['C']:g}"); hyper.setdefault(f"{fam}/{m}", []).append(r["C"])
					else:
						cs.append(f"{r['depth']}/{r['lr']:g}/{r['rounds']}" + ("*" if r["rounds"] >= r["cap"] else "")); hyper.setdefault(f"{fam}/{m}", []).append([r["depth"], r["lr"], r["rounds"]])
				row.append(", ".join(cs))
			md.append(f"| {TITLES[fam]} | {seed} | " + " | ".join(row) + " |")
	out["hyper"] = hyper
	# the inner-selection margins: how much the best grid point beats the second
	md += ["", "Inner-selection margin: per method and family, the median over the 15 outer folds of (second-best minus best mean inner log loss) over the grid; `ngb_lr`: also the median inner gain of the chosen rounds over 0 rounds.", "",
		"| family | logreg | gbm | ngb | ngb_lr | ngb_lr gain over 0 rounds | ngb_stack |", "|---|---:|---:|---:|---:|---:|---:|"]
	for fam in FAMILIES:
		cellsM = []
		gains = []
		for m in FITTED:
			margins = []
			for seed in SEEDS:
				for k in range(K_OUTER):
					r = load(os.path.join(fold_dir(a.out, fam, seed, k), m + ".npz"))
					vals = sorted(r["scores"].values()) if m == "logreg" else sorted(v[0] for v in r["grid"].values())
					margins.append(vals[1] - vals[0])
					if m == "ngb_lr":
						g = r["grid"][f"{r['depth']}/{r['lr']}"]
						gains.append(g[3] - g[0])
			cellsM.append(f"{np.median(margins):.5f}")
		cellsM.insert(4, f"{np.median(gains):.5f}")
		md.append(f"| {TITLES[fam]} | " + " | ".join(cellsM) + " |")
	md.append("")

	# ---- solving
	base = {(c[2], c[5], c[6]): R[c] for c in R if c[3] == "base"}
	def conf(fam, seed, i, m, s, f, version=""):
		r = R[(fam, seed, i, m, s, f, version)]
		return r["confs"] if r["status"] == base[(i, 0, version)]["status"] else None
	def bconf(i, f, version=""):
		r = base[(i, f, version)]
		return r["confs"] if r["status"] == base[(i, 0, version)]["status"] else None
	# per-instance log ratio of A against B (B: a method, or "base"), averaged over the repeats; seeds: which repeats
	def logs(fam, ids, A, B, s, f, seeds=SEEDS, version=""):
		res = []
		for i in ids:
			xs = []
			for seed in seeds:
				ca = conf(fam, seed, i, A, s, f, version)
				cb = bconf(i, f, version) if B == "base" else conf(fam, seed, i, B, s, f, version)
				if ca is not None and cb is not None:
					xs.append(np.log((ca + 1) / (cb + 1)))
			if xs:
				res.append(float(np.mean(xs)))
		return res
	groups = {}
	for fam in FAMILIES:
		sat, unsat = family_ids(D, fam)
		groups[fam] = [int(i) for i in sat if base[(int(i), 0, "")]["status"] == "SAT" and base[(int(i), 0, "")]["confs"] >= MIN_CONFS]
		groups[fam + "_easy"] = [int(i) for i in sat if int(i) not in groups[fam]]
		groups[fam + "_unsat"] = [int(i) for i in unsat]
	out["groups"] = {k: len(v) for k, v in groups.items()}

	md += ["## Primary: paired against `logreg`, confident-first (`:all+conf_x1`)", "",
		f"Satisfiable instances with at least {MIN_CONFS} conflicts in the unperturbed baseline. Per instance the log of (conflicts of A + 1) / (conflicts of `logreg` + 1) at the same flip setting, averaged over the 3 repeats;",
		"geometric mean ratio (95% bootstrap interval over instances, 10000 resamples), wins/losses/ties of A, two-sided Wilcoxon p (zeros dropped), and the Holm-adjusted p over the six methods of the family at that flip setting.",
		"`median of the three`: per instance the median of the three per-setting averages (summary only). Decision rule: a difference is claimed only if Holm p < 0.05 and the interval excludes 1 on the same side at all three settings.", ""]
	primary = {}
	for fam in FAMILIES:
		ids = groups[fam]
		md += [f"### {TITLES[fam]} ({len(ids)} instances)", "", "| A | flip 0 | flip 1000 | flip 2000 | median of the three | claimed |", "|---|---|---|---|---|---|"]
		per = {f: {A: summarize(logs(fam, ids, A, "logreg", "all+conf_x1", f), np.random.RandomState(0)) for A in CHALLENGERS} for f in FLIPS}
		adj = {f: dict(zip(CHALLENGERS, holm(np.array([per[f][A]["p"] for A in CHALLENGERS])))) for f in FLIPS}
		for A in CHALLENGERS:
			L = [logs(fam, ids, A, "logreg", "all+conf_x1", f) for f in FLIPS]
			# instances present at every setting
			medS = summarize(np.median(np.array(L), axis=0), np.random.RandomState(0)) if len({len(l) for l in L}) == 1 else None
			sides = [np.sign(np.log(per[f][A]["hi"])) if per[f][A]["hi"] < 1 else (np.sign(np.log(per[f][A]["lo"])) if per[f][A]["lo"] > 1 else 0) for f in FLIPS]
			claimed = all(adj[f][A] < 0.05 for f in FLIPS) and sides[0] != 0 and len(set(sides)) == 1
			verdict = ("A better" if sides[0] < 0 else "A worse") if claimed else "no"
			primary[f"{fam}/{A}"] = {"per_flip": {str(f): per[f][A] | {"holm": float(adj[f][A])} for f in FLIPS}, "median": medS, "claimed": verdict}
			md.append(f"| `{A}` | " + " | ".join(fmt(per[f][A], adj[f][A]) for f in FLIPS) + f" | {fmt(medS)} | {verdict} |")
		md.append("")
	out["primary"] = primary
	md += ["### The primary comparison per repeat (not averaged over repeats)", "", "Geometric-mean ratio against `logreg`, `:all+conf_x1`, per seed and flip setting (interval over instances).", "",
		"| family | A | " + " | ".join(f"seed {s}, flip {f}" for s in SEEDS for f in FLIPS) + " |", "|---|---|" + "---|" * (len(SEEDS) * len(FLIPS))]
	for fam in FAMILIES:
		for A in CHALLENGERS:
			cs = []
			for s in SEEDS:
				for f in FLIPS:
					st = summarize(logs(fam, groups[fam], A, "logreg", "all+conf_x1", f, seeds=[s]), np.random.RandomState(0))
					out[f"repeat/{fam}/{A}/{s}/{f}"] = st
					cs.append(f"{st['ratio']:.2f} ({st['lo']:.2f}-{st['hi']:.2f})")
			md.append(f"| {TITLES[fam]} | `{A}` | " + " | ".join(cs) + " |")
	md.append("")

	md += ["## Secondary: phases only (`:all`) paired against `logreg:all`", "", "Same statistics as the primary; Holm over the six methods per family and flip setting (separately from the primary).", "", "| family | A | flip 0 | flip 1000 | flip 2000 |", "|---|---|---|---|---|"]
	for fam in FAMILIES:
		per = {f: {A: summarize(logs(fam, groups[fam], A, "logreg", "all", f), np.random.RandomState(0)) for A in CHALLENGERS} for f in FLIPS}
		adj = {f: dict(zip(CHALLENGERS, holm(np.array([per[f][A]["p"] for A in CHALLENGERS])))) for f in FLIPS}
		for A in CHALLENGERS:
			out[f"all/{fam}/{A}"] = {str(f): per[f][A] for f in FLIPS}
			md.append(f"| {TITLES[fam]} | `{A}` | " + " | ".join(fmt(per[f][A], adj[f][A]) for f in FLIPS) + " |")
	md.append("")

	md += ["## Secondary: against the baseline", "", "Per method and strategy, the geometric-mean ratio of conflicts to those of the baseline at the same flip setting (interval over instances; per-instance log ratios averaged over the repeats). Also the baseline at flip 1000 / 2000 against flip 0, the noise of the perturbation alone.", ""]
	for key, title in [(f, f"{TITLES[f]}, satisfiable, >= {MIN_CONFS} conflicts") for f in FAMILIES] + [(f + "_unsat", f"{TITLES[f]}, unsatisfiable") for f in FAMILIES] + [(f + "_easy", f"{TITLES[f]}, satisfiable, < {MIN_CONFS} conflicts") for f in FAMILIES]:
		ids = groups[key]
		if not ids:
			continue
		fam = key.split("_")[0]
		md += [f"### {title} ({len(ids)} instances)", "", "| configuration | flip 0 | flip 1000 | flip 2000 |", "|---|---|---|---|"]
		noise = []
		for f in FLIPS[1:]:
			ls = [np.log((bconf(i, f) + 1) / (bconf(i, 0) + 1)) for i in ids if bconf(i, f) is not None]
			noise.append(summarize(ls, np.random.RandomState(0)))
		md.append("| baseline vs unperturbed baseline | - | " + " | ".join(fmt(s) for s in noise) + " |")
		for s in STRATEGIES:
			for m in METHODS:
				sts = [summarize(logs(fam, ids, m, "base", s, f), np.random.RandomState(0)) for f in FLIPS]
				out[f"base/{key}/{m}:{s}"] = sts
				md.append(f"| `{m}:{s}` | " + " | ".join(fmt(st) for st in sts) + " |")
		md.append("")

	# ---- planner
	pl = planner_ids(D)
	fixed = json.load(open(os.path.join(a.out, "planner_settings.json")))
	md += ["## Planner: leave-one-instance-out with fixed settings (descriptive, not nested)", "",
		"Per-variable features only. Settings fixed from the random 3-SAT inner loops (the grid point chosen most often over its 15 outer folds, the median round count of those folds): "
		+ "; ".join(f"{m}: " + (f"C {v['C']:g}" if m == "logreg" else f"depth {v['depth']}, lr {v['lr']:g}, {v['rounds']} rounds") + f" (chosen in {v['count']} of 15)" for m, v in fixed.items()) + ".", "",
		"| method | accuracy | Brier | log loss | ECE |", "|---|---:|---:|---:|---:|"]
	yP = np.concatenate([D["y"][D["rowsOf"][i]] for i in pl])
	for m in METHODS:
		p = np.concatenate([P[("planner", 0, i)][m] for i in pl])
		mt = metrics(p, yP)
		out[f"planner/calib/{m}"] = {k: mt[k] for k in ("acc", "brier", "logloss", "ece")}
		md.append(f"| {m} | {mt['acc']:.3f} | {mt['brier']:.4f} | {mt['logloss']:.4f} | {mt['ece']:.4f} |")
	md += ["", f"Prior (majority polarity) accuracy {max(yP.mean(), 1 - yP.mean()):.3f}.", "",
		"Conflicts per instance (`.plain` / `.lf`), baseline and `:all+conf_x1` and `:all` per method, at flip 0 / 1000 / 2000:", "",
		"| version | instance | configuration | flip 0 | flip 1000 | flip 2000 |", "|---|---|---|---:|---:|---:|"]
	for version in ("plain", "lf"):
		for i in pl:
			name = recs[i]["name"].replace(".plain", "")
			md.append(f"| .{version} | {name} | baseline | " + " | ".join(str(base[(i, f, version)]["confs"]) for f in FLIPS) + " |")
			for s in STRATEGIES:
				for m in METHODS:
					md.append(f"| .{version} | {name} | `{m}:{s}` | " + " | ".join(str(R[('planner', 0, i, m, s, f, version)]["confs"]) + ("" if R[('planner', 0, i, m, s, f, version)]["status"] == base[(i, 0, version)]["status"] else " (" + R[('planner', 0, i, m, s, f, version)]["status"] + ")") for f in FLIPS) + " |")
	md += ["", "Geometric-mean ratio over the 4 instances against `logreg` (and against the baseline), same flip setting; descriptive, no test:", "", "| version | strategy | A | vs logreg: flip 0 / 1000 / 2000 | vs baseline: flip 0 / 1000 / 2000 |", "|---|---|---|---|---|"]
	for version in ("plain", "lf"):
		for s in STRATEGIES:
			for A in METHODS:
				g = lambda B, f: np.exp(np.mean(logs("planner", pl, A, B, s, f, seeds=[0], version=version)))
				out[f"planner/{version}/{A}:{s}"] = {"vs_logreg": [float(g("logreg", f)) for f in FLIPS], "vs_base": [float(g("base", f)) for f in FLIPS]}
				md.append(f"| .{version} | {s} | `{A}` | " + " / ".join(f"{g('logreg', f):.3f}" for f in FLIPS) + " | " + " / ".join(f"{g('base', f):.3f}" for f in FLIPS) + " |")
	md.append("")
	# ---- run counts and fit times
	secs = {}
	for fam in FAMILIES:
		for seed in SEEDS:
			for k in range(K_OUTER):
				for m in FITTED:
					secs[m] = secs.get(m, 0) + load(os.path.join(fold_dir(a.out, fam, seed, k), m + ".npz"))["seconds"]
	out["fit_seconds"] = secs
	md += ["Fit time (CPU-process seconds summed over the 45 outer folds, including the inner loops): " + ", ".join(f"{m} {v:.0f}" for m, v in secs.items()) + ".", ""]
	with open(os.path.join(a.out, "report.md"), "w") as f:
		f.write("\n".join(md) + "\n")
	with open(os.path.join(a.out, "report.json"), "w") as f:
		json.dump(out, f, indent=1, default=float)
	print("\n".join(md))

def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("--work", required=True)
	ap.add_argument("--out", required=True)
	ap.add_argument("--stage", choices=["provenance", "fit", "solve", "report"], required=True)
	ap.add_argument("--exe")
	ap.add_argument("--old-exe", help="provenance: also rerun the sample with this binary")
	ap.add_argument("--jobs", type=int, default=8)
	ap.add_argument("--dry-run", action="store_true", help="solve: only count the runs")
	a = ap.parse_args()
	warnings.filterwarnings("ignore")
	os.makedirs(a.out, exist_ok=True)
	{"provenance": stage_provenance, "fit": stage_fit, "solve": stage_solve, "report": stage_report}[a.stage](a)

if __name__ == "__main__":
	main()
