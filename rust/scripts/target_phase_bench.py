#!/usr/bin/env python3
# The measurements of target/best-phase saving and rephasing (src/topi_ext_target_phase.rs): the baseline versus the extension under several settings.
#
# The instances: the regression instances; the versions of the planner_* ones of phase_oracle_ceiling.py (NAME.nolf: the recorded queries without the lf commands;
# the single-query NAME.lf: the clauses and the lf commands, and NAME.plain: the clauses only); and generated formulas, since all the regression instances
# but the planner ones take under 200 conflicts: uniform random 3-SAT at the threshold (200 variables, ratio 4.26: satisfiable and unsatisfiable ones)
# and below it (250 and 350 variables, ratio 4.0: satisfiable, and hard for a CDCL solver), and pigeonhole formulas (unsatisfiable).
# Every run verifies its models (/topor_tool/verify_model 1), and the script fails, if two runs of an instance contradict each other.
#
# Usage: target_phase_bench.py [--exe PATH] [--instances DIR] [--work DIR] [--out FILE.md] [--timeout SEC] [--jobs N]
#   The tables are written to FILE.md (docs/search_quality/target_phase_tables.md by default); the generated instances and runs.csv (every run) are left in the work directory.
import argparse, csv, glob, math, os, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from search_quality_bench import bench, contradictions
from phase_oracle_ceiling import planner_variants, random_3sat, pigeonhole

VERIFY = ["/topor_tool/verify_model", "1"]
T, R = ["/phase/target/on", "1"], lambda init, incr=None, mult=1: ["/phase/rephase/conf_init", str(init), "/phase/rephase/conf_incr", str(init if incr is None else incr), "/phase/rephase/conf_mult", str(mult)]
M1 = ["/mode/value", "1"]
# (name, the configuration it is compared with, parameters, description)
CONFIGS = [
	("base", "base", [], "the defaults (the mode 0: a numeric restart every 1000 conflicts)"),
	("target", "base", T, "`/phase/target/on 1`"),
	("rephase", "base", R(1000), "`/phase/rephase/conf_init 1000` (`conf_incr 1000`: after 1000, 3000, 6000, ... conflicts)"),
	("t+r1000", "base", T + R(1000), "both: the schedule of CaDiCaL"),
	("t+r300", "base", T + R(300), "both, `conf_init 300 conf_incr 300`"),
	("t+r100", "base", T + R(100), "both, `conf_init 100 conf_incr 100`"),
	("t+r1000x1.5", "base", T + R(1000, 0, 1.5), "both, geometric: `conf_init 1000 conf_incr 0 conf_mult 1.5`"),
	("t+r1000rnd", "base", T + R(1000) + ["/phase/rephase/random", "1"], "both, `/phase/rephase/random 1`: random phases instead of the flipped ones"),
	("flip1000", "base", ["/decision/polarity/flip_factor", "1000"], "the control, a perturbation without information: `/decision/polarity/flip_factor 1000` (the C++ solver's own parameter: every 1000th polarity selection is flipped)"),
	("m1", "m1", M1, "`/mode/value 1`, whose restarts are LBD-based: the baseline of the next three"),
	("m1:t+r1000", "m1", M1 + T + R(1000), "the mode 1 with both"),
	("m1:t2+r1000", "m1", M1 + ["/phase/target/on", "2"] + R(1000), "the mode 1 with `/phase/target/on 2`: no target phases under LBD-based restarts, so rephasing to the best phases only"),
	("m1:t+r300", "m1", M1 + T + R(300), "the mode 1 with both, `conf_init 300 conf_incr 300`"),
]

def geomean(ratios):
	return math.exp(sum(math.log(r) for r in ratios) / len(ratios)) if ratios else float("nan")

def main():
	here = os.path.dirname(os.path.abspath(__file__))
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", default=os.path.join(here, "..", "target", "release", "intel_sat_solver"))
	ap.add_argument("--instances", default=os.path.join(here, "..", "..", "regression_instances"))
	ap.add_argument("--work", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "target_phase_bench"))
	ap.add_argument("--out", default=os.path.join(here, "..", "docs", "search_quality", "target_phase_tables.md"))
	ap.add_argument("--timeout", type=float, default=60)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
	a = ap.parse_args()
	os.makedirs(a.work, exist_ok=True)

	# The instances by group
	recorded = sorted(glob.glob(os.path.join(a.instances, "*.cnf")))
	planner = [p for p in recorded if os.path.basename(p).startswith("planner_")]
	groups = [("regression, not planner", [p for p in recorded if p not in planner]), ("planner, recorded", planner)]
	variants = [planner_variants(p, a.work) for p in planner]
	for i, suffix in enumerate(("nolf", "lf", "plain")):
		groups.append((f"planner .{suffix}", [v[i] for v in variants]))
	for name, n, ratio, seeds in (("random 3-SAT n=200 r=4.26", 200, 4.26, 24), ("random 3-SAT n=250 r=4.0", 250, 4.0, 16), ("random 3-SAT n=350 r=4.0", 350, 4.0, 8)):
		groups.append((name, []))
		for seed in range(1, seeds + 1):
			groups[-1][1].append(os.path.join(a.work, f"rand3sat_n{n}_r{ratio}_s{seed}.cnf"))
			random_3sat(groups[-1][1][-1], n, ratio, seed)
	groups.append(("pigeonhole", []))
	for n in (8, 9):
		groups[-1][1].append(os.path.join(a.work, f"pigeonhole_{n}.cnf"))
		pigeonhole(groups[-1][1][-1], n)
	instances = [p for _, ps in groups for p in ps]

	rows = bench(a.exe, instances, [(name, VERIFY + params) for name, _, params, _ in CONFIGS], a.timeout, a.jobs, verbose=False)
	with open(os.path.join(a.work, "runs.csv"), "w", newline="") as f:
		w = csv.DictWriter(f, fieldnames=["instance", "config", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts"])
		w.writeheader()
		w.writerows(rows)
	bad = contradictions(rows)
	byKey = {(r["instance"], r["config"]): r for r in rows}
	inst = os.path.basename
	decided = lambda r: r["status"] in ("SAT", "UNSAT")
	# The answer of an instance: that of any run, which decided it (the runs don't contradict each other, or the script fails)
	answer = {}
	for r in rows:
		if decided(r):
			answer[r["instance"]] = r["status"]

	md = ["# Target/best-phase saving and rephasing: the tables", "",
		"Generated by `scripts/target_phase_bench.py`; the reading is in [`target_phase.md`](target_phase.md).", "",
		f"Runs: {len(rows)}; instances: {len(instances)}; timeout: {a.timeout:g} s CPU+wall per run; every model is verified. A `-` is a run without an answer (a timeout).",
		"The conflicts, decisions and restarts are cumulative over the queries of an instance. The status of a recorded planner instance is that of its last query.", "",
		"The configurations:", ""]
	md += [f"* `{name}`: {descr}" for name, _, _, descr in CONFIGS]
	if bad:
		md += [""] + [f"**CONTRADICTION: {i}, query {q}: SAT versus UNSAT**" for i, q in bad]

	# The summary: per group and answer, the geometric mean of (conflicts + 1) / (conflicts of the reference + 1) over the instances, which both decide and the reference takes at least 200 conflicts for
	def summary(title, counter):
		res = ["", f"## {title}", "",
			f"Per group of instances and answer: the geometric mean of the ratio of the {counter} to those of the reference configuration (`base`; `m1` for the `m1:` ones) over the instances,",
			"which both configurations decide and the reference run takes at least 200 conflicts for (n of them; `w/l`: on how many the configuration takes fewer/more conflicts);",
			"`+k`/`-k`: the instances only the configuration/only the reference decides within the timeout.", "",
			"| group | answer | n | " + " | ".join(f"{name}" for name, ref, _, _ in CONFIGS if name != ref) + " |", "|---|---|---:|" + "---:|" * sum(name != ref for name, ref, _, _ in CONFIGS)]
		def line(label, ans, ps):
			cells, ns = [], set()
			for name, ref, _, _ in CONFIGS:
				if name == ref:
					continue
				both = [(byKey[(inst(p), name)], byKey[(inst(p), ref)]) for p in ps]
				common = [(x, b) for x, b in both if decided(x) and decided(b) and b["confs"] >= 200]
				only = sum(decided(x) and not decided(b) for x, b in both), sum(decided(b) and not decided(x) for x, b in both)
				ns.add(len(common))
				eps = 0.001 if counter == "cpu" else 1
				cell = f"{geomean([(x[counter] + eps) / (b[counter] + eps) for x, b in common]):.3f}" if common else "-"
				if counter == "confs" and common:
					cell += f" ({sum(x['confs'] < b['confs'] for x, b in common)}/{sum(x['confs'] > b['confs'] for x, b in common)})"
				cell += (f" +{only[0]}" if only[0] else "") + (f" -{only[1]}" if only[1] else "")
				cells.append(cell)
			if max(ns) > 0 or any("+" in c or " -" in c for c in cells):
				res.append(f"| {label} | {ans} | {'/'.join(map(str, sorted(ns)))} | " + " | ".join(cells) + " |")
		for ans in ("SAT", "UNSAT"):
			for g, ps in groups:
				line(g, ans, [p for p in ps if answer.get(inst(p)) == ans])
		for ans in ("SAT", "UNSAT"):
			line("**all the generated formulas**", ans, [p for g, ps in groups if g.startswith(("random", "pigeonhole")) for p in ps if answer.get(inst(p)) == ans])
			line("**all**", ans, [p for p in instances if answer.get(inst(p)) == ans])
		return res
	md += summary("Conflicts", "confs") + summary("Decisions", "decs") + summary("CPU time", "cpu")

	# The instances, which no configuration takes 200 conflicts for, are summed up in one line per group
	md += ["", "## Conflicts per instance", "", "The instances, for which some configuration takes at least 200 conflicts.", "",
		"| instance | answer | " + " | ".join(name for name, _, _, _ in CONFIGS) + " |", "|---|---|" + "---:|" * len(CONFIGS)]
	trivial = []
	for g, ps in groups:
		for p in ps:
			rs = [byKey[(inst(p), name)] for name, _, _, _ in CONFIGS]
			if all(decided(r) and r["confs"] < 200 for r in rs):
				trivial.append((g, rs))
			else:
				md.append(f"| {inst(p)} | {answer.get(inst(p), '?')} | " + " | ".join(str(r["confs"]) if decided(r) else "-" for r in rs) + " |")
	md += ["", "The other instances (under 200 conflicts in every configuration): the total conflicts per group.", "",
		"| group | instances | " + " | ".join(name for name, _, _, _ in CONFIGS) + " |", "|---|---:|" + "---:|" * len(CONFIGS)]
	for g, _ in groups:
		rss = [rs for gg, rs in trivial if gg == g]
		if rss:
			md.append(f"| {g} | {len(rss)} | " + " | ".join(str(sum(rs[i]["confs"] for rs in rss)) for i in range(len(CONFIGS))) + " |")

	md += ["", "## CPU seconds per instance", "", "The instances, for which the `base` run takes at least 0.5 s or has no answer.", "",
		"| instance | " + " | ".join(name for name, _, _, _ in CONFIGS) + " |", "|---|" + "---:|" * len(CONFIGS)]
	for p in instances:
		rs = [byKey[(inst(p), name)] for name, _, _, _ in CONFIGS]
		if not decided(rs[0]) or rs[0]["cpu"] >= 0.5:
			md.append(f"| {inst(p)} | " + " | ".join(f"{r['cpu']:.2f}" if decided(r) else "-" for r in rs) + " |")

	with open(a.out, "w") as f:
		f.write("\n".join(md) + "\n")
	print("\n".join(md))
	sys.exit(1 if bad else 0)

if __name__ == "__main__":
	main()
