#!/usr/bin/env python3
# NGBoost experiment, step 4: are the predicted phases, and the uncertainty around them, of use to the solver? Everything goes through the hints mechanism
# (/phase/hints/on 1, /decision/score_hints/mult M, /topor_tool/hints_file), with predictions that are out-of-sample for the instance (ngb_phase.py).
#
# Strategies, per predictor M (ngb, gbm, gbm_iso, logreg, jw, prior):
#   M:all            every variable is hinted its more probable polarity
#   M:tC             only the variables with the confidence max(p, 1-p) >= C are hinted
#   M:all+conf_xK    M:all, and score hints 2|p - 0.5| with the multiplier K: the confident variables are decided first
#   M:all+unc_xK     M:all, and score hints 1 - 2|p - 0.5|: the uncertain variables are decided first
#   M:conf_xK, M:unc_xK   the score hints alone
#   ngb:t0.7+conf_x1      the confident-first score hints of ngb, with the phase hinted only where the confidence is at least 0.7
# Controls (advice without information): flip500/1000/2000 (/decision/polarity/flip_factor), rand_phase (a random polarity for every variable),
#   perm_xK (the confidence scores of ngb, permuted among the variables), ngb:all+perm_xK (the phases of ngb with the permuted scores),
#   rand_phase+ngb_conf_x1 (random phases with the confident-first scores of ngb).
# Oracle (satisfiable instances): oracle:1 (the model of the baseline run); oracle_match:S (as many variables hinted as ngb:S hints, as many of them correctly, chosen at random:
#   the true-model oracle at the coverage and the accuracy of the predictor).
# ngbX:* uses the cross-family predictions; planner only: ngbLF:* the predictions with the lf polarity as a feature, ngbPV:* those without the instance-level features;
# lf:all hints the planner's lf polarities themselves.
# If DIR/preds_lr.npz exists (ngb_lr.py: boosting on a logistic-regression baseline), its models ngb_es, ngb_lr and ngb_stack get M:all, M:t0.7 and M:all+conf_x1;
# on the planner instances also with the suffixes LF and PV (the two other protocols), next to logregLF and logregPV, and M:all, M:t0.7 on the sequences;
# and M:all+conf_x1+flipK for logreg, ngb_lr and ngb_stack: the confident-first hints under the perturbation of the control flipK (a check against the noise of the search).
# The recorded planner sequences and their versions without lf are hinted again before every query (lh commands), since phase saving overwrites a hint.
# Every run verifies its model; an answer which differs from that of the baseline is reported, and SAT versus UNSAT is a soundness bug (exit code 1).
#
# Usage: ngb_solve.py --exe PATH --work DIR [--seeds N] [--timeout SEC] [--jobs N]
#   Reads vars.npz, instances.json, preds.npz; writes DIR/solve_runs.csv and DIR/tables/3_solving.md.
import argparse, csv, json, os, random, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from search_quality_bench import bench
from ngb_common import VERIFY, geo, hints_text, write

PHASE_MODELS = ["ngb", "gbm", "gbm_iso", "logreg", "jw", "prior"]
EXTRA_MODELS = ["ngb_es", "ngb_lr", "ngb_stack"]	# those of ngb_lr.py, if it has been run
EXTRA_SEQ = ["logreg", "logregLF", "logregPV"] + [m + s for m in EXTRA_MODELS for s in ("", "LF", "PV")]	# hinted on the planner sequences as well, once ngb_lr.py has been run
THRESHOLDS = [0.55, 0.6, 0.7, 0.8, 0.9]
MULTS = [1, 10]
FLIPS = [500, 1000, 2000]
ON = ["/phase/hints/on", "1"]
MULT = lambda k: ["/decision/score_hints/mult", str(k)]

# The hints of every configuration for one instance: {name: (params, lits, scores)}; var, y (-1: no label) and P (model -> probabilities) are the instance's variables
def strategies(name, var, y, P, seeds, lfLits):
	res = {}
	lits = lambda p, mask=None: [int(v) if q > 0.5 else -int(v) for v, q in (zip(var, p) if mask is None else zip(var[mask], p[mask]))]
	conf = lambda p: np.maximum(p, 1 - p)
	def add_phase(tag, p):
		res[f"{tag}:all"] = (ON, lits(p), [])
		for t in THRESHOLDS:
			res[f"{tag}:t{t}"] = (ON, lits(p, conf(p) >= t), [])
	for m in PHASE_MODELS:
		add_phase(m, P[m])
	for m in ("ngb", "gbm"):
		c = 2 * np.abs(P[m] - 0.5)
		for tag, s in (("conf", c + 1e-3), ("unc", 1 - c + 1e-3)):
			for k in MULTS:
				res[f"{m}:all+{tag}_x{k}"] = (ON + MULT(k), lits(P[m]), list(zip(var, s)))
			res[f"{m}:{tag}_x10"] = (MULT(10), [], list(zip(var, s)))
	# The confident-first order with the other predictors, the rule without learning (jw) among them
	for m in ("gbm_iso", "logreg", "jw"):
		res[f"{m}:all+conf_x1"] = (ON + MULT(1), lits(P[m]), list(zip(var, 2 * np.abs(P[m] - 0.5) + 1e-3)))
	# Which half of the advice carries the gain: the confident-first order of ngb with random phases, and with the phases hinted only where the confidence is at least 0.7
	c = 2 * np.abs(P["ngb"] - 0.5) + 1e-3
	r = random.Random(f"{name}/randphase_conf")
	res["rand_phase+ngb_conf_x1"] = (ON + MULT(1), [int(v) if r.random() < 0.5 else -int(v) for v in var], list(zip(var, c)))
	res["ngb:t0.7+conf_x1"] = (ON + MULT(1), lits(P["ngb"], conf(P["ngb"]) >= 0.7), list(zip(var, c)))
	for x in ("ngbX", "ngbLF", "ngbPV"):
		if x in P:
			for s in ("all", "t0.7", "t0.8"):
				p = P[x]
				res[f"{x}:{s}"] = (ON, lits(p) if s == "all" else lits(p, conf(p) >= float(s[1:])), [])
			res[f"{x}:all+conf_x1"] = (ON + MULT(1), lits(P[x]), list(zip(var, 2 * np.abs(P[x] - 0.5) + 1e-3)))
	# The models of ngb_lr.py, and the logistic regression of the two other planner protocols: the three strategies of that comparison
	for m in EXTRA_SEQ[1:]:
		if m in P:
			res[f"{m}:all"] = (ON, lits(P[m]), [])
			res[f"{m}:t0.7"] = (ON, lits(P[m], conf(P[m]) >= 0.7), [])
			res[f"{m}:all+conf_x1"] = (ON + MULT(1), lits(P[m]), list(zip(var, 2 * np.abs(P[m] - 0.5) + 1e-3)))
	# Is a difference between two confident-first configurations more than the noise of the search? The same hints under the uninformative perturbation of the flip controls
	if "ngb_lr" in P:
		for m in ("logreg", "ngb_lr", "ngb_stack"):
			for k in FLIPS:
				res[f"{m}:all+conf_x1+flip{k}"] = (ON + MULT(1) + ["/decision/polarity/flip_factor", str(k)], lits(P[m]), list(zip(var, 2 * np.abs(P[m] - 0.5) + 1e-3)))
	if lfLits:
		res["lf:all"] = (ON, lfLits, [])
	c = 2 * np.abs(P["ngb"] - 0.5) + 1e-3
	for seed in range(seeds):
		r = random.Random(f"{name}/{seed}")
		res[f"rand_phase_s{seed}"] = (ON, [int(v) if r.random() < 0.5 else -int(v) for v in var], [])
		perm = list(c)
		r.shuffle(perm)
		for k in MULTS:
			res[f"perm_x{k}_s{seed}"] = (MULT(k), [], list(zip(var, perm)))
			res[f"ngb:all+perm_x{k}_s{seed}"] = (ON + MULT(k), lits(P["ngb"]), list(zip(var, perm)))
	if (y >= 0).all():
		truth = [int(v) if t else -int(v) for v, t in zip(var, y)]
		res["oracle:1"] = (ON, truth, [])
		for s in ["all"] + [f"t{t}" for t in THRESHOLDS]:
			mask = np.ones(len(var), dtype=bool) if s == "all" else conf(P["ngb"]) >= float(s[1:])
			nHinted, nRight = int(mask.sum()), int(((P["ngb"] > 0.5) == (y == 1))[mask].sum())
			for seed in range(seeds):
				r = random.Random(f"{name}/oracle/{s}/{seed}")
				chosen = r.sample(range(len(var)), nHinted)
				res[f"oracle_match:{s}_s{seed}"] = (ON, [truth[i] if j < nRight else -truth[i] for j, i in enumerate(chosen)], [])
	return res

def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", required=True)
	ap.add_argument("--work", required=True)
	ap.add_argument("--seeds", type=int, default=3)
	ap.add_argument("--timeout", type=float, default=120)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
	ap.add_argument("--min-confs", type=int, default=200)
	ap.add_argument("--resume", action="store_true", help="reuse the runs of DIR/solve_runs.csv and run only the configurations missing there")
	a = ap.parse_args()
	d = np.load(os.path.join(a.work, "vars.npz"))
	y, inst, var = d["y"], d["inst"], d["var"]
	records = json.load(open(os.path.join(a.work, "instances.json")))
	preds = np.load(os.path.join(a.work, "preds.npz"))
	extra = np.load(os.path.join(a.work, "preds_lr.npz")) if os.path.exists(os.path.join(a.work, "preds_lr.npz")) else None
	byName = {r["name"]: i for i, r in enumerate(records)}
	hintsDir = os.path.join(a.work, "hints")
	instDir = os.path.join(a.work, "inst")

	# The instances: the phase-role ones of the generated families, and the planner ones; (path, record of the instance, record index of the variables, how the hints are given)
	todo = []
	for i, r in enumerate(records):
		if r["status"] not in ("SAT", "UNSAT"):
			continue
		if r["family"] in ("rand", "factor", "col") and r["role"] == "phase":
			todo.append((r, i, "file"))
		elif r["family"] == "planner":
			src = byName[r["name"].split(".")[0] + ".plain"]
			todo.append((r, src, "file" if r["role"] in ("phase", "lf") else "lh"))
	configs, hinted = {}, {}	# configs: name -> params (the same for every instance); hinted: (instance, config) -> (number of the hinted variables, of them right)
	rehintPaths = []
	for r, src, how in todo:
		rows = np.nonzero(inst == src)[0]
		P = {m: preds[f"within:{m}"][rows] for m in PHASE_MODELS}
		if not np.isnan(preds["cross:ngb"][rows]).any():
			P["ngbX"] = preds["cross:ngb"][rows]
		lfLits = []
		if r["family"] == "planner":
			P["ngbLF"] = preds["within+lf:ngb"][rows]
			P["ngbPV"] = preds["within-pv:ngb"][rows]
		if extra is not None:
			P.update({m: extra[f"within:{m}"][rows] for m in EXTRA_MODELS})
			if r["family"] == "planner":
				for suffix, protocol in (("LF", "within+lf"), ("PV", "within-pv")):
					P["logreg" + suffix] = preds[f"{protocol}:logreg"][rows]
					P.update({m + suffix: extra[f"{protocol}:{m}"][rows] for m in EXTRA_MODELS})
			assert not any(np.isnan(P[m]).any() for m in P), r["name"]
			lfCol = list(d["features"]).index("lf_pol")
			lfLits = [int(v) * int(s) for v, s in zip(var[rows], d["X"][rows, lfCol]) if s != 0]
		# The labels are those of the model of this very instance, if it is the one the samples come from; a planner version has its own model, so no oracle there
		strat = strategies(r["name"], var[rows], y[rows] if src == byName[r["name"]] else np.full(len(rows), -1), P, a.seeds, lfLits)
		truth = dict(zip(var[rows], y[rows]))
		for name, (params, lits, scores) in strat.items():
			hinted[(r["name"], name)] = (len(lits), sum(truth.get(abs(l), -1) == (l > 0) for l in lits))
			if how == "file":
				configs[name] = params + ["/topor_tool/hints_file", os.path.join(hintsDir, "{inst}." + name + ".hints")]
				write(os.path.join(hintsDir, f"{r['name']}.{name}.hints"), hints_text(lits, scores))
			elif scores == [] and (name.split(":")[0] in ("ngb", "ngbLF", "ngbPV", "lf", "prior") and name.split(":")[-1] in ("all", "t0.8") or name.startswith("rand_phase")
					or extra is not None and name.split(":")[0] in EXTRA_SEQ and name.split(":")[-1] in ("all", "t0.7")):
				# A sequence of queries: the phase hints are given again before every query
				path = os.path.join(instDir, f"{r['name']}.rehint.{name.replace(':', '_')}.cnf")
				lh = "".join(f"lh {l}\n" for l in lits)
				write(path, "".join(lh + l if l.startswith("s") else l for l in open(r["path"]).read().splitlines(keepends=True)))
				rehintPaths.append((path, r["name"], name))
	fileInst = [r["path"] for r, _, how in todo if how == "file"]
	seqInst = [r["path"] for r, _, how in todo if how == "lh"]
	controls = [(f"flip{k}", ["/decision/polarity/flip_factor", str(k)]) for k in FLIPS]
	rows = []
	if a.resume and os.path.exists(os.path.join(a.work, "solve_runs.csv")):
		for r in csv.DictReader(open(os.path.join(a.work, "solve_runs.csv"))):
			r.update({"confs": int(r["confs"]), "decs": int(r["decs"]), "rsts": int(r["rsts"]), "cpu": float(r["cpu"])})
			rows.append(r)
	have = {(r["instance"], r["config"]) for r in rows}
	# Not every configuration exists for every instance (the oracle needs a model, ngbX a cross-family prediction): a missing hints file is an empty one
	for r, _, how in todo:
		if how == "file":
			for name in configs:
				path = os.path.join(hintsDir, f"{r['name']}.{name}.hints")
				if (r["name"], name) not in hinted:
					write(path, "")
	names = sorted(configs)
	allNames = names
	# The configurations of the two other planner protocols which ngb_lr.py adds exist for the planner instances only, and are run on those only
	plannerOnly = {n for n in names if n.split(":")[0] in EXTRA_SEQ and n.split(":")[0].endswith(("LF", "PV"))}
	plannerInst = [r["path"] for r, _, how in todo if how == "file" and r["family"] == "planner"]
	instOf = lambda n: plannerInst if n in plannerOnly else fileInst
	names = [n for n in names if any((os.path.basename(p), n) not in have for p in instOf(n))]
	rehintAll = rehintPaths
	rehintPaths = [x for x in rehintPaths if (x[1] + ".cnf", x[2]) not in have]
	print(f"{len(fileInst)} instances x {len(names) + 4} configurations, {len(seqInst)} sequences, {len(rehintPaths)} re-hinted sequence runs", flush=True)
	if not have:
		rows += bench(a.exe, fileInst + seqInst, [("base", VERIFY)] + [(n, VERIFY + p) for n, p in controls], a.timeout, a.jobs, verbose=False)
	names = [n for n in names if n not in plannerOnly] + [n for n in names if n in plannerOnly]
	for k in range(0, len(names), 10):
		for insts in (fileInst, plannerInst):
			batch = [(n, VERIFY + configs[n]) for n in names[k:k + 10] if instOf(n) is insts]
			rows += bench(a.exe, insts, batch, a.timeout, a.jobs, verbose=False) if batch else []
		print(f"{min(k + 10, len(names))}/{len(names)} configurations done", flush=True)
	for r in bench(a.exe, [p for p, _, _ in rehintPaths], [("x", VERIFY + ON)], a.timeout, a.jobs, verbose=False) if rehintPaths else []:
		_, src, name = next(x for x in rehintPaths if os.path.basename(x[0]) == r["instance"])
		r.update({"instance": src + ".cnf", "config": name})
		rows.append(r)
	with open(os.path.join(a.work, "solve_runs.csv"), "w", newline="") as f:
		w = csv.DictWriter(f, fieldnames=["instance", "config", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts"], extrasaction="ignore")
		w.writeheader()
		w.writerows(rows)

	by = {(os.path.splitext(r["instance"])[0], r["config"]): r for r in rows}
	# Checks: the baseline is that of ngb_data.py; no answer contradicts the baseline's
	contradictions, differ = [], []
	for (i, c), r in by.items():
		b = by[(i, "base")]
		if c == "base" and r["confs"] != records[byName[i]]["confs"]:
			sys.exit(f"the baseline of {i} is not deterministic")
		for q, (x, z) in enumerate(zip(r["verdicts"].split(), b["verdicts"].split())):
			if {x, z} == {"S", "U"}:
				contradictions.append((i, c, q))
		if r["status"] != b["status"] or len(r["verdicts"].split()) != len(b["verdicts"].split()):
			differ.append((i, c, r["status"]))
		elif r["verdicts"] != b["verdicts"]:
			differ.append((i, c, "another verdict of a budget-limited query"))

	# The tables. A configuration with seeds is reported as the median over its seeds per instance.
	def value(i, c, counter):
		rs = [by[k] for k in ([(i, c)] if (i, c) in by else [(i, f"{c}_s{s}") for s in range(a.seeds)]) if k in by]
		rs = [r for r in rs if r["status"] == by[(i, "base")]["status"]]
		return float(np.median([r[counter] for r in rs])) if rs else None
	def exists(i, c):
		return (i, c) in hinted or (i, c + "_s0") in hinted or c.startswith("flip") or c == "base"
	def line(group, c, rng=np.random.RandomState(0)):
		pairs = [(value(i, c, "confs"), by[(i, "base")]["confs"], value(i, c, "cpu"), by[(i, "base")]["cpu"]) for i in group if exists(i, c)]
		n = len(pairs)
		pairs = [p for p in pairs if p[0] is not None]
		if not pairs:
			return None
		logs = np.array([np.log((x + 1) / (b + 1)) for x, b, _, _ in pairs])
		boot = np.exp(np.percentile([rng.choice(logs, len(logs)).mean() for _ in range(2000)], [2.5, 97.5])) if len(logs) > 1 else (float("nan"), float("nan"))
		hs = [hinted.get((i, c)) or hinted.get((i, c + "_s0")) for i in group]
		hs = [h for h in hs if h]
		nv = {i: records[byName[i]].get("vars") or records[byName[i.split(".")[0] + ".plain"]]["vars"] for i in group}
		cov = np.mean([h[0] / nv[i] for h, i in zip(hs, group)]) if hs and len(hs) == len(group) else None
		acc = sum(h[1] for h in hs) / max(sum(h[0] for h in hs), 1) if hs else None
		return {"n": n, "decided": len(pairs), "ratio": float(np.exp(logs.mean())), "lo": float(boot[0]), "hi": float(boot[1]), "wins": int((logs < 0).sum()), "losses": int((logs > 0).sum()),
			"total": sum(p[0] for p in pairs), "cpu": geo([(x + 0.001) / (b + 0.001) for _, _, x, b in pairs]), "cov": cov, "acc": acc}
	rec = lambda i: records[byName[i]]
	instNames = [r["name"] for r, _, _ in todo]
	groups = []
	for fam, title in (("rand", "random 3-SAT"), ("factor", "factoring"), ("col", "3-colouring")):
		groups.append((f"{title}, satisfiable, baseline >= {a.min_confs} conflicts", [i for i in instNames if rec(i)["family"] == fam and rec(i)["status"] == "SAT" and rec(i)["confs"] >= a.min_confs], True))
	groups.append((f"all three generated families, satisfiable, baseline >= {a.min_confs} conflicts", [i for _, g, _ in groups[:3] for i in g], True))
	groups.append((f"generated, satisfiable, baseline < {a.min_confs} conflicts (easy)", [i for i in instNames if rec(i)["family"] != "planner" and rec(i)["status"] == "SAT" and rec(i)["confs"] < a.min_confs], True))
	groups.append(("generated, unsatisfiable (random 3-SAT, factoring of primes, 3-colouring; the predictors are trained on the satisfiable instances of the family)", [i for i in instNames if rec(i)["family"] != "planner" and rec(i)["status"] == "UNSAT"], False))
	groups.append(("planner, single query, clauses only (.plain)", [i for i in instNames if rec(i)["role"] == "phase" and rec(i)["family"] == "planner"], True))
	groups.append(("planner, single query, lf in force (.lf)", [i for i in instNames if rec(i)["role"] == "lf"], False))
	groups.append(("planner, recorded queries without lf (.nolf), hints renewed before every query", [i for i in instNames if rec(i)["role"] == "nolf"], False))
	groups.append(("planner, recorded sequences (lf in force), hints renewed before every query", [i for i in instNames if rec(i)["role"] == "recorded"], False))
	seeded = lambda c: c.rsplit("_s", 1)[0] if "_s" in c and c.rsplit("_s", 1)[1].isdigit() else c
	allConfigs = ["base"] + [n for n, _ in controls]
	for n in allNames + sorted({c for _, _, c in rehintAll}):
		if seeded(n) not in allConfigs:
			allConfigs.append(seeded(n))
	order = lambda c: (0 if c == "base" else 1 if c.startswith(("flip", "rand_phase", "perm")) else 2 if c.startswith("oracle") else 3, c.split(":")[0], c)
	allConfigs.sort(key=order)
	md = ["## 3. Solving with the predicted phases and their confidence", "",
		f"Generated by `scripts/ngb_solve.py`: {len(rows)} runs, timeout {a.timeout:g} s, every model verified by the tool. The predictions are out-of-sample for the instance (within-family folds, unless the name says `ngbX`: cross-family).",
		"`ratio`: the geometric mean of (conflicts + 1) / (baseline conflicts + 1) over the instances decided with the baseline's answer, with a 95% bootstrap interval over the instances;",
		f"configurations with random choices are the median of {a.seeds} seeds per instance. `hinted`: the mean share of the variables with a phase hint; `right`: the share of the phase hints that agree with the baseline's model.", "",
		f"**SAT-versus-UNSAT contradictions with the baseline: {len(contradictions)}.** Runs whose final status or number of answered queries differs from the baseline's (timeouts or errors): {len([x for x in differ if 'another' not in x[2]])}"
		+ "".join(f"; {i} under `{c}`: {s}" for i, c, s in differ if "another" not in s) + f". Runs of a planner sequence in which a budget-limited query ends differently (CONFLICT_OUT versus an answer): {len([x for x in differ if 'another' in x[2]])}.", ""]
	summary = {}
	extraRng = np.random.RandomState(1)
	for title, group, hasModel in groups:
		if not group:
			continue
		md += [f"### {title} ({len(group)} instances; baseline: {sum(by[(i, 'base')]['confs'] for i in group)} conflicts in total)", "", "| configuration | n | ratio | 95% interval | wins/losses | total conflicts | CPU ratio | hinted | right |", "|---|---:|---:|---|---:|---:|---:|---:|---:|"]
		for c in allConfigs:
			# The rows which ngb_lr.py adds draw their bootstrap samples from a generator of their own, so that the intervals of the other rows do not depend on its having been run
			added = c.split(":")[0] in EXTRA_SEQ[1:] or "+flip" in c or c.split(":")[0] == "logreg" and rec(group[0])["role"] in ("nolf", "recorded")
			l = line(group, c, extraRng) if added else line(group, c)
			if l is None or c == "base":
				continue
			summary[f"{title}|{c}"] = l
			md.append(f"| `{c}` | {l['decided']}/{l['n']} | {l['ratio']:.3f} | {l['lo']:.3f}-{l['hi']:.3f} | {l['wins']}/{l['losses']} | {l['total']:.0f} | {l['cpu']:.3f} | " + ("-" if l["cov"] is None else f"{l['cov']:.2f}") + " | " + ("-" if l["acc"] is None or not hasModel else f"{l['acc']:.3f}") + " |")
		md.append("")
	# The planner, per instance
	md += ["### The planner instances, per instance: conflicts", "", "| instance | base | flip500 / 1000 / 2000 | rand_phase (seeds) | prior:all | ngb:all | ngb:t0.8 | ngbLF:all | ngbPV:all | ngbPV:t0.8 | lf:all | oracle:1 |", "|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
	cell = lambda i, c: "-" if (i, c) not in by or not exists(i, c) else str(by[(i, c)]["confs"]) if by[(i, c)]["status"] == by[(i, "base")]["status"] else f"{by[(i, c)]['status']} ({by[(i, c)]['confs']})"
	for i in instNames:
		if rec(i)["family"] == "planner":
			md.append(f"| {i} | {cell(i, 'base')} | " + " / ".join(cell(i, f"flip{k}") for k in (500, 1000, 2000)) + " | " + " / ".join(cell(i, f"rand_phase_s{s}") for s in range(a.seeds)) + " | " + " | ".join(cell(i, c) for c in ("prior:all", "ngb:all", "ngb:t0.8", "ngbLF:all", "ngbPV:all", "ngbPV:t0.8", "lf:all", "oracle:1")) + " |")
	with open(os.path.join(a.work, "tables", "3_solving.md"), "w") as f:
		f.write("\n".join(md) + "\n")
	with open(os.path.join(a.work, "solve_summary.json"), "w") as f:
		json.dump(summary, f, indent=1)
	print("\n".join(md))
	sys.exit(1 if contradictions else 0)

if __name__ == "__main__":
	main()
