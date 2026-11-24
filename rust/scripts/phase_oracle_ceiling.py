#!/usr/bin/env python3
# The oracle-ceiling experiment: how much can phase advice and score carry-over help the search at best?
#
# (1) Phase oracle. Every satisfiable instance is solved once for a model; then it is solved again with phase hints (/phase/hints/on 1, /topor_tool/hints_file),
#     where a fraction p of the variables is hinted the polarity of the model, and the rest is hinted the opposite polarity ("flip") or isn't hinted at all ("none"),
#     for several p and several random choices of the variables (seeds).
# (2) Family oracle. A planner_* instance is solved with the final saved phases of a DIFFERENT planner_* instance as hints (the variables of the same index).
# (3) Carry-over. Every instance is solved again with the hints its own first run has written (/topor_tool/write_hints_file): the decision scores
#     (/decision/score_hints/mult M), the saved phases, or both. For an unsatisfiable instance this is the only advice there is.
#     The control for the scores is the same scores, randomly permuted among the variables.
#
# The instances: the regression instances; versions of the planner_* ones (the recorded ones are sequences of queries under conflict thresholds,
# with a polarity fixed by the planner (lf) for almost every variable): NAME.nolf (the recorded queries without the lf commands), and the single-query
# NAME.lf (the clauses and the lf commands) and NAME.plain (the clauses only);
# and generated uniform random 3-SAT and pigeonhole formulas, since all the regression instances but the planner ones take under 200 conflicts.
# Every run verifies its models (/topor_tool/verify_model 1), and the script fails, if two runs of an instance contradict each other.
#
# Usage: phase_oracle_ceiling.py [--exe PATH] [--instances DIR] [--work DIR] [--out FILE.md] [--seeds N] [--timeout SEC] [--jobs N]
#   The tables are written to FILE.md (docs/search_quality/ceiling_tables.md by default); the generated instances, the hints files and runs.csv (every run) are left in the work directory.
import argparse, csv, glob, math, os, random, re, statistics, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from search_quality_bench import bench, contradictions

PS = [0.0, 0.5, 0.75, 0.9, 0.95, 1.0]
SCORE_MULTS = [1, 10, 100]
VERIFY = ["/topor_tool/verify_model", "1"]

def write(path, text):
	with open(path, "w") as f:
		f.write(text)

# The versions of a recorded planner instance: the recorded queries without the lf commands (nolf), and the single-query ones (the queries and their thresholds are dropped):
# the clauses with the lf commands (lf) and the clauses only (plain)
def planner_variants(path, work):
	base = os.path.splitext(os.path.basename(path))[0]
	recorded = [l for l in open(path).read().splitlines() if l and not l.startswith(("c", "p "))]
	lines = [l for l in recorded if not l.startswith(("s", "ot ", "oc "))]
	res = []
	for suffix, keep in (("nolf", [l for l in recorded if not l.startswith("lf ")]), ("lf", lines), ("plain", [l for l in lines if re.match(r"-?\d", l)])):
		res.append(os.path.join(work, f"{base}.{suffix}.cnf"))
		write(res[-1], "\n".join(keep) + "\n")
	return res

def random_3sat(path, n, ratio, seed):
	r = random.Random(seed)
	m = round(n * ratio)
	write(path, f"p cnf {n} {m}\n" + "".join(" ".join(str(v if r.random() < 0.5 else -v) for v in r.sample(range(1, n + 1), 3)) + " 0\n" for _ in range(m)))

# n + 1 pigeons in n holes
def pigeonhole(path, n):
	var = lambda p, h: p * n + h + 1
	clss = [[var(p, h) for h in range(n)] for p in range(n + 1)] + [[-var(p, h), -var(q, h)] for h in range(n) for p in range(n + 1) for q in range(p)]
	write(path, f"p cnf {(n + 1) * n} {len(clss)}\n" + "".join(" ".join(map(str, c)) + " 0\n" for c in clss))

# The phases (signed literals) and the scores of a hints file
def read_hints(path):
	phases, scores = [], []
	for l in open(path):
		t = l.split()
		if t and t[0] == "v":
			phases += [int(x) for x in t[1:] if x != "0"]
		elif t and t[0] == "b":
			scores.append(l)
	return phases, scores

def phases_text(lits):
	return "".join("v " + " ".join(map(str, lits[i:i + 20])) + "\n" for i in range(0, len(lits), 20)) + "v 0\n"

def median(xs):
	return statistics.median(xs) if xs else None

def fmt(x):
	return "-" if x is None else str(int(x)) if float(x).is_integer() else f"{x:.1f}"

def geomean_ratio(pairs):
	pairs = [(x, b) for x, b in pairs if x is not None and b is not None]
	return math.exp(sum(math.log((x + 1) / (b + 1)) for x, b in pairs) / len(pairs)) if pairs else float("nan")

def main():
	here = os.path.dirname(os.path.abspath(__file__))
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", default=os.path.join(here, "..", "target", "release", "intel_sat_solver"))
	ap.add_argument("--instances", default=os.path.join(here, "..", "..", "regression_instances"))
	ap.add_argument("--work", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "phase_oracle_ceiling"))
	ap.add_argument("--out", default=os.path.join(here, "..", "docs", "search_quality", "ceiling_tables.md"))
	ap.add_argument("--seeds", type=int, default=5)
	ap.add_argument("--timeout", type=float, default=60)
	ap.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
	a = ap.parse_args()
	hintsDir = os.path.join(a.work, "hints")
	os.makedirs(hintsDir, exist_ok=True)

	# The instances
	recorded = sorted(glob.glob(os.path.join(a.instances, "*.cnf")))
	planner = [p for p in recorded if os.path.basename(p).startswith("planner_")]
	derived = [v for p in planner for v in planner_variants(p, a.work)]
	candidates = []
	for seed in range(1, 13):
		candidates.append(os.path.join(a.work, f"rand3sat_n200_r4.1_s{seed}.cnf"))
		random_3sat(candidates[-1], 200, 4.1, seed)
		candidates.append(os.path.join(a.work, f"rand3sat_n175_r4.7_s{seed}.cnf"))
		random_3sat(candidates[-1], 175, 4.7, seed)
	for n in (7, 8):
		candidates.append(os.path.join(a.work, f"pigeonhole_{n}.cnf"))
		pigeonhole(candidates[-1], n)

	run = lambda instances, configs: bench(a.exe, instances, configs, a.timeout, a.jobs, verbose=False)
	hintsOf = lambda config: os.path.join(hintsDir, "{inst}." + config + ".hints")

	# The first runs: the baseline, which also writes the model/the final phases and the scores
	rows = run(recorded + derived + candidates, [("base", VERIFY + ["/topor_tool/write_hints_file", hintsOf("base")])])
	status = {r["instance"]: r["status"] for r in rows}
	# Of the generated random formulas, the first 6 satisfiable and the first 6 unsatisfiable ones are kept
	kept, quota = [], {"SAT": 6, "UNSAT": 6}
	for p in candidates:
		s = status[os.path.basename(p)]
		if "pigeonhole" in p or quota.get(s, 0) > 0:
			kept.append(p)
			if "pigeonhole" not in p:
				quota[s] -= 1
	instances = recorded + derived + kept
	names = {os.path.basename(p) for p in instances}
	rows = [r for r in rows if r["instance"] in names]
	sat = [p for p in instances if status[os.path.basename(p)] == "SAT"]
	inst = lambda p: os.path.splitext(os.path.basename(p))[0]
	baseHints = {inst(p): read_hints(hintsOf("base").replace("{inst}", inst(p))) for p in instances}

	# (1) The phase oracle
	oracle = []
	for mode in ("flip", "none"):
		for p in PS:
			# The fully correct hints don't depend on the seed, and neither do the fully flipped ones
			for seed in range(1 if p in (0.0, 1.0) else a.seeds):
				name = f"oracle_{mode}_p{p}_s{seed}"
				oracle.append((name, mode, p))
				for path in sat:
					model = baseHints[inst(path)][0]
					r = random.Random(f"{inst(path)}/{seed}")
					correct = set(r.sample(range(len(model)), round(p * len(model))))
					lits = [l if i in correct else -l for i, l in enumerate(model) if i in correct or mode == "flip"]
					write(hintsOf(name).replace("{inst}", inst(path)), phases_text(lits))
	rows += run(sat, [(name, VERIFY + ["/phase/hints/on", "1", "/topor_tool/hints_file", hintsOf(name)]) for name, _, _ in oracle])

	# The oracle, refreshed: in a sequence of queries the hints given before the first query are overwritten by phase saving on the way,
	# so the model is also hinted again before every query, by lh commands in the instance (they are ignored, unless /phase/hints/on is 1)
	rehint = []
	for path in planner + [p for p in derived if ".nolf." in p]:
		if path in sat:
			lh = "".join(f"lh {l}\n" for l in baseHints[inst(path)][0])
			rehint.append((os.path.join(a.work, inst(path) + ".rehint.cnf"), path))
			write(rehint[-1][0], "".join(lh + l if l.startswith("s") else l for l in open(path).read().splitlines(keepends=True)))
	rows += run([r for r, _ in rehint], [("rehint_off", VERIFY), ("rehint", VERIFY + ["/phase/hints/on", "1"])])

	# (2) The family oracle: the phases of another planner instance (the recorded ones and their versions are hinted)
	family = []
	for donor in planner:
		name = "family_" + inst(donor)
		family.append((name, inst(donor)))
		for path in planner + derived:
			write(hintsOf(name).replace("{inst}", inst(path)), phases_text(baseHints[inst(donor)][0]))
	rows += run(planner + derived, [(name, VERIFY + ["/phase/hints/on", "1", "/topor_tool/hints_file", hintsOf(name)]) for name, _ in family])

	# (3) The carry-over from the first run of the same instance (the hints file holds both the phases and the scores; the parameters select what is used)
	carry = [(f"scores_x{m}", ["/decision/score_hints/mult", str(m)]) for m in SCORE_MULTS] + [("phases", ["/phase/hints/on", "1"]), ("phases_scores_x10", ["/phase/hints/on", "1", "/decision/score_hints/mult", "10"])]
	rows += run(instances, [(name, VERIFY + params + ["/topor_tool/hints_file", hintsOf("base")]) for name, params in carry])
	# The control: the same scores, randomly permuted among the same variables (advice without information; it tells the effect of the carried scores
	# from that of merely perturbing the decision order, to which the search is very sensitive)
	control = []
	for seed in range(a.seeds):
		for path in instances:
			lines = [l.split() for l in baseHints[inst(path)][1]]
			scores = [t[2] for t in lines]
			random.Random(f"{inst(path)}/control/{seed}").shuffle(scores)
			write(hintsOf(f"control_s{seed}").replace("{inst}", inst(path)), "".join(f"b {t[1]} {x}\n" for t, x in zip(lines, scores)))
		control += [(f"control_x{m}_s{seed}", m, seed) for m in SCORE_MULTS]
	rows += run(instances, [(name, VERIFY + ["/decision/score_hints/mult", str(m), "/topor_tool/hints_file", hintsOf(f"control_s{seed}")]) for name, m, seed in control])

	# Every run, for a closer look
	with open(os.path.join(a.work, "runs.csv"), "w", newline="") as f:
		w = csv.DictWriter(f, fieldnames=["instance", "config", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts"], extrasaction="ignore")
		w.writeheader()
		w.writerows(rows)

	# The tables
	by = {(r["instance"], r["config"]): r for r in rows}
	def get(path, config, counter="confs"):
		r = by.get((os.path.basename(path), config))
		return r[counter] if r and r["status"] in ("SAT", "UNSAT") else None
	trivial = lambda path: (get(path, "base") or 0) < 200
	bad = contradictions(rows)
	verdicts = lambda path, config: by[(os.path.basename(path), config)]["verdicts"].split()
	for r, path in rehint:
		# The lh commands change nothing, unless the parameter is on; with it on, no answer may contradict that of the source instance
		if by[(os.path.basename(r), "rehint_off")]["confs"] != by[(os.path.basename(path), "base")]["confs"] or verdicts(r, "rehint_off") != verdicts(path, "base"):
			sys.exit(f"{r}: the hints aren't ignored with /phase/hints/on 0")
		bad += [(os.path.basename(r), i) for i, (x, y) in enumerate(zip(verdicts(r, "rehint"), verdicts(path, "base"))) if {x, y} == {"S", "U"}]
	errors = sorted({(r["instance"], r["config"], r["status"]) for r in rows if r["status"] not in ("SAT", "UNSAT")})
	md = ["# Oracle ceiling: the tables", "", "Generated by `scripts/phase_oracle_ceiling.py` (see `ceiling.md` for the reading). All the numbers are conflicts, cumulative over the queries of an instance, unless stated otherwise.",
		f"Runs: {len(rows)}; instances: {len(instances)} ({len(sat)} satisfiable); seeds per fraction: {a.seeds}; timeout: {a.timeout:g} s. A `-` is a run without an answer (timeout or error).", "",
		f"Soundness: {len(bad)} contradicting answers (SAT versus UNSAT for the same query of an instance, over all the runs); every model is verified by the tool; runs without an answer: {len(errors)}."]
	md += [f"* {i}, {c}: {s}" for i, c, s in errors]

	groups = [("recorded planner instances (query sequences under conflict thresholds, lf in force)", [p for p in planner if p in sat]),
		("recorded planner instances without the lf commands", [p for p in derived if ".nolf." in p and p in sat]),
		("planner instances, single query, lf in force", [p for p in derived if ".lf." in p and p in sat]),
		("planner instances, single query, clauses only", [p for p in derived if ".plain." in p and p in sat]),
		("generated instances", [p for p in kept if p in sat])]
	trivialSat = [p for p in recorded if p in sat and p not in planner]
	for mode, what in (("flip", "the other variables are hinted the opposite polarity"), ("none", "the other variables aren't hinted")):
		md += ["", f"## Phase oracle: a fraction p of the variables is hinted the polarity of the model; {what}", "",
			"The median over the seeds. `base` is the run without hints; `perturbed` is the control without information: no phase hints, and random score hints with `/decision/score_hints/mult 1`",
			"(the scores of the first run, randomly permuted among the variables), which only perturb the decision order.", "",
			"| instance | vars | base | perturbed | " + " | ".join(f"p={p:g}" for p in PS) + " |", "|---|---:|---:|---:|" + "---:|" * len(PS)]
		cell = lambda path, p, counter="confs": median([x for x in (get(path, name, counter) for name, m, pp in oracle if m == mode and pp == p) if x is not None])
		perturbed = lambda path, counter="confs": median([x for x in (get(path, name, counter) for name, m, _ in control if m == 1) if x is not None])
		for title, paths in groups:
			md.append(f"| *{title}* | | | |" + " |" * len(PS))
			for path in paths:
				md.append(f"| {os.path.basename(path)} | {len(baseHints[inst(path)][0])} | {fmt(get(path, 'base'))} | {fmt(perturbed(path))} | " + " | ".join(fmt(cell(path, p)) for p in PS) + " |")
		md.append(f"| *the other {len(trivialSat)} satisfiable regression instances, total* | | {sum(get(path, 'base') or 0 for path in trivialSat)} | {fmt(sum(perturbed(path) or 0 for path in trivialSat))} | " + " | ".join(fmt(sum(cell(path, p) or 0 for path in trivialSat)) for p in PS) + " |")
		gmRow = lambda label, paths, counter: f"| **geometric mean of the {label} ratio to base, {label2}: {len(paths)} instances** | | 1 | {geomean_ratio([(perturbed(path, counter), get(path, 'base', counter)) for path in paths]):.3f} | " + " | ".join(f"{geomean_ratio([(cell(path, p, counter), get(path, 'base', counter)) for path in paths]):.3f}" for p in PS) + " |"
		for label2, paths in groups[1:]:
			md.append(gmRow("conflicts", [path for path in paths if not trivial(path)], "confs"))
		hard = [path for _, paths in groups for path in paths if not trivial(path)]
		label2 = "all the instances above with base >= 200 conflicts"
		for counter, label in (("confs", "conflicts"), ("decs", "decisions"), ("cpu", "CPU time")):
			md.append(gmRow(label, hard, counter))

	md += ["", "## The oracle in a sequence of queries: the model hinted once, or again before every query", "",
		"`hinted once`: the p=1 oracle above (`/topor_tool/hints_file`, applied by the first query); `re-hinted`: the same model hinted before every query (`lh` commands in the instance).", "",
		"| instance | queries | base | hinted once | re-hinted before every query |", "|---|---:|---:|---:|---:|"]
	for r, path in rehint:
		md.append(f"| {os.path.basename(path)} | {len(verdicts(path, 'base'))} | {fmt(get(path, 'base'))} | {fmt(get(path, 'oracle_flip_p1.0_s0'))} | {fmt(get(r, 'rehint'))} |")

	md += ["", "## Family oracle: a planner instance hinted the final saved phases of another planner instance", "",
		"The donor's phases are those of its recorded run (its final model). `own model` is the p=1 oracle of the instance itself, for reference.", "",
		"| instance | base | own model | " + " | ".join(f"from {d.replace('planner_', '')}" for _, d in family) + " |", "|---|---:|---:|" + "---:|" * len(family)]
	for path in planner + derived:
		md.append(f"| {os.path.basename(path)} | {fmt(get(path, 'base'))} | {fmt(get(path, 'oracle_flip_p1.0_s0'))} | " + " | ".join("(itself)" if inst(path).startswith(d) else fmt(get(path, name)) for name, d in family) + " |")

	for title, paths in (("unsatisfiable instances", [p for p in instances if status[os.path.basename(p)] == "UNSAT"]), ("satisfiable instances (the carried phases are the model, that is, the p=1 oracle)", sat)):
		# The columns: a carried configuration, or the median over the seeds of the control with the given multiplier
		cols = [(name.replace("_", " "), [name]) for name, _ in carry[:len(SCORE_MULTS)]] + [(f"random scores x{m}", [name for name, mm, _ in control if mm == m]) for m in SCORE_MULTS] + [(name.replace("_", " "), [name]) for name, _ in carry[len(SCORE_MULTS):]]
		val = lambda p, configs, counter="confs": median([x for x in (get(p, c, counter) for c in configs) if x is not None])
		md += ["", f"## Carry-over from a previous run of the same instance: {title}", "",
			"`scores xM`: the decision scores at the end of the first run (their ranks, normalized to (0, 1]) boost the initial scores with `/decision/score_hints/mult M`;",
			"`random scores xM`: the control, the same scores randomly permuted among the same variables (the median over the seeds); `phases`: the final saved phases as hints.", "",
			"| instance | status | base | " + " | ".join(label for label, _ in cols) + " |", "|---|---|---:|" + "---:|" * len(cols)]
		for path in [p for p in paths if not trivial(p)]:
			md.append(f"| {os.path.basename(path)} | {status[os.path.basename(path)]} | {fmt(get(path, 'base'))} | " + " | ".join(fmt(val(path, configs)) for _, configs in cols) + " |")
		rest = [p for p in paths if trivial(p)]
		md.append(f"| *the other {len(rest)} instances (base < 200 conflicts), total* | | {sum(get(p, 'base') or 0 for p in rest)} | " + " | ".join(fmt(sum(val(p, configs) or 0 for p in rest)) for _, configs in cols) + " |")
		for counter, label in (("confs", "conflicts"), ("decs", "decisions"), ("cpu", "CPU time")):
			md.append(f"| **geometric mean of the {label} ratio to base, the {len(paths) - len(rest)} instances with base >= 200 conflicts** | | 1 | " + " | ".join(f"{geomean_ratio([(val(p, configs, counter), get(p, 'base', counter)) for p in paths if not trivial(p)]):.3f}" for _, configs in cols) + " |")

	os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
	write(a.out, "\n".join(md) + "\n")
	print("\n".join(md))
	sys.exit(1 if bad else 0)

if __name__ == "__main__":
	main()
