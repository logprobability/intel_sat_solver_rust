#!/usr/bin/env python3
# Shared helpers of the NGBoost experiment (docs/search_quality/ngboost.md): reading a CNF (plain DIMACS or a recorded planner instance), the per-variable and
# per-instance features, hints files, and small utilities. The solver runs go through search_quality_bench.bench, as in the other search-quality scripts.
# The scripts ngb_*.py need numpy, scikit-learn and ngboost (offline-script dependencies only; see ngboost.md for the pinned versions); nothing in the solver does.
import math, os, re, sys, time

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np

VERIFY = ["/topor_tool/verify_model", "1"]

def write(path, text):
	with open(path, "w") as f:
		f.write(text)

def inst_name(path):
	return os.path.splitext(os.path.basename(path))[0]

# The clauses of a DIMACS file or of a recorded planner instance (the lines which start with a literal), as lists of ints, and the lf (FixPolarity) literals
def read_cnf(path):
	clauses, lf = [], []
	isLit = re.compile(r"-?\d")
	for l in open(path):
		if isLit.match(l):
			c = [int(x) for x in l.split()]
			if c and c[-1] == 0:
				c.pop()
			if c:
				clauses.append(c)
		elif l.startswith("lf "):
			lf += [int(x) for x in l.split()[1:] if x != "0"]
	return clauses, lf

PER_VAR = ["pos", "neg", "occ", "bal", "pos_frac", "pos_bin", "neg_bin", "pos_ter", "neg_ter", "pos_long", "neg_long", "pos_unit", "neg_unit", "jw_pos", "jw_neg", "jw_bal",
	"min_size", "mean_size", "degree", "need_pos1", "need_neg1", "need_bal1", "need_pos2", "need_neg2", "need_bal2", "nb_bal", "rel_index", "occ_rel", "lf_pol"]
PER_INST = ["log_vars", "log_clauses", "ratio", "frac_bin", "frac_ter", "frac_long", "mean_len", "pos_lit_frac", "occ_cv", "bal_abs_mean", "bal_std", "frac_pure", "jw_bal_abs_mean"]
FEATURES = PER_VAR + ["i_" + f for f in PER_INST]

# The features of a formula, all computed from the clauses in time linear in its size (a few passes over the literal occurrences):
# returns (vars, X, instFeatures): the variables which occur in a clause (ascending), their feature rows (the columns are FEATURES) and the instance-level features (PER_INST)
def features(clauses, lf=()):
	sizes = np.fromiter((len(c) for c in clauses), dtype=np.int64, count=len(clauses))
	lits = np.fromiter((l for c in clauses for l in c), dtype=np.int64, count=int(sizes.sum()))
	cid = np.repeat(np.arange(len(clauses)), sizes)
	n = int(np.abs(lits).max())
	var = np.abs(lits)
	isPos = lits > 0
	size = sizes[cid]
	cnt = lambda mask, w=None: np.bincount(var[mask], weights=None if w is None else w[mask], minlength=n + 1).astype(float)
	everything = np.ones(len(lits), dtype=bool)
	pos, neg = cnt(isPos), cnt(~isPos)
	occ = pos + neg
	f = {"pos": pos, "neg": neg, "occ": occ, "bal": (pos - neg) / np.maximum(occ, 1), "pos_frac": (pos + 0.5) / (occ + 1)}
	for name, m in (("unit", size == 1), ("bin", size == 2), ("ter", size == 3), ("long", size > 3)):
		f["pos_" + name], f["neg_" + name] = cnt(isPos & m), cnt(~isPos & m)
	w = np.power(2.0, -size.astype(float))
	f["jw_pos"], f["jw_neg"] = cnt(isPos, w), cnt(~isPos, w)
	f["jw_bal"] = (f["jw_pos"] - f["jw_neg"]) / np.maximum(f["jw_pos"] + f["jw_neg"], 1e-12)
	minSize = np.full(n + 1, 1e9)
	np.minimum.at(minSize, var, size)
	f["min_size"] = np.where(occ > 0, minSize, 0)
	f["mean_size"] = cnt(everything, size.astype(float)) / np.maximum(occ, 1)
	f["degree"] = cnt(everything, size.astype(float) - 1)
	# "Need": how much the clauses of a literal depend on it, if the other literals are true independently with the probability q: the sum over its clauses of the product
	# of (1 - q) of the other literals (a single round of a belief-propagation-like estimate; the second round takes q from the first one)
	q = np.where(isPos, f["pos_frac"][var], 1 - f["pos_frac"][var])
	for rnd in (1, 2):
		logNot = np.log(np.clip(1 - q, 1e-6, 1))
		others = np.exp(np.bincount(cid, weights=logNot, minlength=len(clauses))[cid] - logNot)
		needPos, needNeg = cnt(isPos, others), cnt(~isPos, others)
		f[f"need_pos{rnd}"], f[f"need_neg{rnd}"] = needPos, needNeg
		f[f"need_bal{rnd}"] = (needPos - needNeg) / np.maximum(needPos + needNeg, 1e-12)
		p = (needPos + 0.5) / (needPos + needNeg + 1)
		q = np.where(isPos, p[var], 1 - p[var])
	# The mean polarity balance of the other literals in the clauses of the variable, signed by the variable's own sign there
	litBal = np.where(isPos, f["bal"][var], -f["bal"][var])
	clsBal = np.bincount(cid, weights=litBal, minlength=len(clauses))
	otherBal = (clsBal[cid] - litBal) / np.maximum(size - 1, 1)
	f["nb_bal"] = cnt(everything, np.where(isPos, otherBal, -otherBal)) / np.maximum(occ, 1)
	f["rel_index"] = np.arange(n + 1) / max(n, 1)
	vs = np.nonzero(occ > 0)[0]
	f["occ_rel"] = occ / occ[vs].mean()
	lfPol = np.zeros(n + 1)
	for l in lf:
		if abs(l) <= n:
			lfPol[abs(l)] = 1 if l > 0 else -1
	f["lf_pol"] = lfPol
	m = len(clauses)
	inst = {"log_vars": math.log10(len(vs)), "log_clauses": math.log10(m), "ratio": m / len(vs), "frac_bin": float((sizes == 2).mean()), "frac_ter": float((sizes == 3).mean()),
		"frac_long": float((sizes > 3).mean()), "mean_len": float(sizes.mean()), "pos_lit_frac": float(isPos.mean()), "occ_cv": float(occ[vs].std() / occ[vs].mean()),
		"bal_abs_mean": float(np.abs(f["bal"][vs]).mean()), "bal_std": float(f["bal"][vs].std()), "frac_pure": float(((pos[vs] == 0) | (neg[vs] == 0)).mean()),
		"jw_bal_abs_mean": float(np.abs(f["jw_bal"][vs]).mean())}
	X = np.column_stack([f[k][vs] for k in PER_VAR] + [np.full(len(vs), inst[k]) for k in PER_INST])
	return vs, X, np.array([inst[k] for k in PER_INST])

# The phases (a dictionary var -> bool) of a hints file written by /topor_tool/write_hints_file (after a satisfiable query: the model)
def read_model(path):
	model = {}
	for l in open(path):
		t = l.split()
		if t and t[0] == "v":
			for x in t[1:]:
				if x != "0":
					model[abs(int(x))] = int(x) > 0
	return model

# A hints file: phases (signed literals) and scores (pairs var, score > 0)
def hints_text(lits=(), scores=()):
	lits = [int(l) for l in lits]
	text = "".join("v " + " ".join(map(str, lits[i:i + 20])) + "\n" for i in range(0, len(lits), 20))
	return text + "".join(f"b {int(v)} {s:.6g}\n" for v, s in scores)

def geo(xs):
	xs = list(xs)
	return math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")

class Timer:
	def __enter__(self):
		self.t = time.perf_counter()
		return self
	def __exit__(self, *a):
		self.s = time.perf_counter() - self.t
