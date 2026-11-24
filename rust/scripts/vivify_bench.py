#!/usr/bin/env python3
# The measurements of the vivification of learnt clauses (src/topi_ext_vivify.rs): the baseline versus vivification under several settings.
#
# The instances: the regression instances; the versions of the planner_* ones of phase_oracle_ceiling.py; and generated formulas, since all the regression instances
# but the planner ones take under 200 conflicts: structured ones (factoring by an array multiplier: a semiprime is satisfiable, a prime is unsatisfiable;
# the colouring of random graphs with 3 colours around the threshold; pigeonhole formulas) and uniform random 3-SAT at the threshold.
# Every run verifies its models (/topor_tool/verify_model 1), and the script fails, if two runs of an instance contradict each other.
#
# Usage: vivify_bench.py [--exe PATH] [--instances DIR] [--work DIR] [--out FILE.md] [--timeout SEC] [--jobs N] [--quick]
#   The tables are written to FILE.md (docs/search_quality/vivification_tables.md by default); the generated instances and runs.csv (every run) are left in the work directory.
#   --quick: only the generated formulas and the first four configurations.
import argparse, csv, glob, math, os, random, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from search_quality_bench import bench, contradictions
from phase_oracle_ceiling import planner_variants, random_3sat, pigeonhole, write

VERIFY = ["/topor_tool/verify_model", "1"]
ING = lambda n: ["/inprocessing/on", "1", "/inprocessing/conflicts_before_next", str(n)]
V = lambda n, effort=None, glue=None: ING(n) + ["/inprocessing/vivify/on", "1"] + (["/inprocessing/vivify/effort", str(effort)] if effort is not None else []) + (["/inprocessing/vivify/max_glue", str(glue)] if glue is not None else [])
M1 = ["/mode/value", "1"]
# (name, the configuration it is compared with, parameters, description)
CONFIGS = [
	("base", "base", [], "the defaults (no inprocessing)"),
	("ing", "base", ING(5000), "the control: `/inprocessing/on 1 /inprocessing/conflicts_before_next 5000` without vivification (inprocessing itself does nothing else in the default mode)"),
	("viv", "base", V(5000), "`ing` and `/inprocessing/vivify/on 1` (`/inprocessing/vivify/effort 0.1`, the default)"),
	("viv:e0.3", "base", V(5000, 0.3), "the same with `/inprocessing/vivify/effort 0.3`"),
	("viv:e0.03", "base", V(5000, 0.03), "the same with `/inprocessing/vivify/effort 0.03`"),
	("viv:e1", "base", V(5000, 1), "the same with `/inprocessing/vivify/effort 1` (a round may propagate as much as the search has)"),
	("viv:n1000", "base", V(1000), "a round every 1000 conflicts (at the next restart) instead of 5000"),
	("viv:n20000", "base", V(20000), "a round every 20000 conflicts"),
	("viv:g2", "base", V(5000, None, 2), "`viv` with `/inprocessing/vivify/max_glue 2`: only the clauses of the glue 2 at most"),
	("viv:g6", "base", V(5000, None, 6), "`viv` with `/inprocessing/vivify/max_glue 6`"),
	("flip1000", "base", ["/decision/polarity/flip_factor", "1000"], "the control, a perturbation without information: `/decision/polarity/flip_factor 1000` (the C++ solver's own parameter: every 1000th polarity selection is flipped)"),
	("m1", "m1", M1, "`/mode/value 1`, whose restarts are LBD-based: the baseline of the next two"),
	("m1:viv", "m1", M1 + V(5000), "the mode 1 with `viv`"),
	("m1:viv:e0.3", "m1", M1 + V(5000, 0.3), "the mode 1 with `viv:e0.3`"),
]

# n-bit times n-bit array multiplier, whose product is N, and neither factor is 1: satisfiable iff N has a factorization into two factors of n bits at most
def factoring(path, N, n):
	clss = []
	nv = [0]
	def new():
		nv[0] += 1
		return nv[0]
	def and2(a, b):
		o = new()
		clss.extend([[-a, -b, o], [a, -o], [b, -o]])
		return o
	def xor2(a, b):
		o = new()
		clss.extend([[-a, -b, -o], [a, b, -o], [a, -b, o], [-a, b, o]])
		return o
	def half(a, b):
		return xor2(a, b), and2(a, b)
	def full(a, b, c):
		s = xor2(xor2(a, b), c)
		o = new()
		clss.extend([[-a, -b, o], [-a, -c, o], [-b, -c, o], [a, b, -o], [a, c, -o], [b, c, -o]])
		return s, o
	x = [new() for _ in range(n)]
	y = [new() for _ in range(n)]
	# The columns of the partial products, reduced by adders till one bit is left in every column
	cols = [[] for _ in range(2 * n + 1)]
	for i in range(n):
		for j in range(n):
			cols[i + j].append(and2(x[i], y[j]))
	out = []
	for k in range(2 * n):
		col = cols[k]
		while len(col) > 1:
			if len(col) >= 3:
				s, c = full(col.pop(), col.pop(), col.pop())
			else:
				s, c = half(col.pop(), col.pop())
			col.insert(0, s)
			cols[k + 1].append(c)
		out.append(col[0] if col else None)
	assert N < (1 << (2 * n))
	for k, o in enumerate(out):
		if o is not None:
			clss.append([o if (N >> k) & 1 else -o])
		else:
			assert not (N >> k) & 1
	clss.append(x[1:])
	clss.append(y[1:])
	write(path, f"p cnf {nv[0]} {len(clss)}\n" + "".join(" ".join(map(str, c)) + " 0\n" for c in clss))

def is_prime(n):
	return n > 1 and all(n % d for d in range(2, int(n ** 0.5) + 1))

# The colouring of a random graph of n vertices and m edges with k colours
def colouring(path, n, m, k, seed):
	r = random.Random(seed)
	edges = set()
	while len(edges) < m:
		a, b = r.sample(range(n), 2)
		edges.add((min(a, b), max(a, b)))
	var = lambda v, c: v * k + c + 1
	clss = [[var(v, c) for c in range(k)] for v in range(n)] + [[-var(a, c), -var(b, c)] for a, b in sorted(edges) for c in range(k)]
	write(path, f"p cnf {n * k} {len(clss)}\n" + "".join(" ".join(map(str, c)) + " 0\n" for c in clss))

def geomean(ratios):
	return math.exp(sum(math.log(r) for r in ratios) / len(ratios)) if ratios else float("nan")

def generated(work):
	groups = []
	r = random.Random(1)
	# Semiprimes of two primes of bits bits each, and primes of 2 * bits - 1 bits
	for bits, count in ((20, 6), (21, 6)):
		groups.append((f"factoring a semiprime, {bits}x{bits} bits", []))
		for i in range(count):
			ps = []
			while len(ps) < 2:
				p = r.randrange(1 << (bits - 1), 1 << bits) | 1
				if is_prime(p):
					ps.append(p)
			groups[-1][1].append(os.path.join(work, f"factor_{bits}_{ps[0] * ps[1]}.cnf"))
			factoring(groups[-1][1][-1], ps[0] * ps[1], bits)
	for bits, count in ((17, 4), (18, 4), (19, 4)):
		groups.append((f"factoring a prime, {bits}x{bits} bits", []))
		for i in range(count):
			while True:
				p = r.randrange(1 << (2 * bits - 2), 1 << (2 * bits - 1)) | 1
				if is_prime(p):
					break
			groups[-1][1].append(os.path.join(work, f"prime_{bits}_{p}.cnf"))
			factoring(groups[-1][1][-1], p, bits)
	for n, ratio, seeds in ((400, 2.34, 8), (600, 2.30, 8)):
		groups.append((f"3-colouring, n={n}, edges/n={ratio}", []))
		for seed in range(1, seeds + 1):
			groups[-1][1].append(os.path.join(work, f"col3_n{n}_r{ratio}_s{seed}.cnf"))
			colouring(groups[-1][1][-1], n, round(n * ratio), 3, seed)
	groups.append(("pigeonhole", []))
	for n in (9, 10):
		groups[-1][1].append(os.path.join(work, f"pigeonhole_{n}.cnf"))
		pigeonhole(groups[-1][1][-1], n)
	for name, n, ratio, seeds in (("random 3-SAT n=250 r=4.26", 250, 4.26, 12),):
		groups.append((name, []))
		for seed in range(1, seeds + 1):
			groups[-1][1].append(os.path.join(work, f"rand3sat_n{n}_r{ratio}_s{seed}.cnf"))
			random_3sat(groups[-1][1][-1], n, ratio, seed)
	return groups

def main():
	here = os.path.dirname(os.path.abspath(__file__))
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", default=os.path.join(here, "..", "target", "release", "intel_sat_solver"))
	ap.add_argument("--instances", default=os.path.join(here, "..", "..", "regression_instances"))
	ap.add_argument("--work", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "vivify_bench"))
	ap.add_argument("--out", default=os.path.join(here, "..", "docs", "search_quality", "vivification_tables.md"))
	ap.add_argument("--timeout", type=float, default=120)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
	ap.add_argument("--quick", action="store_true")
	a = ap.parse_args()
	os.makedirs(a.work, exist_ok=True)
	configs = CONFIGS[:4] if a.quick else CONFIGS

	# The instances by group
	groups = []
	if not a.quick:
		recorded = sorted(glob.glob(os.path.join(a.instances, "*.cnf")))
		planner = [p for p in recorded if os.path.basename(p).startswith("planner_")]
		groups = [("regression, not planner", [p for p in recorded if p not in planner]), ("planner, recorded", planner)]
		variants = [planner_variants(p, a.work) for p in planner]
		for i, suffix in enumerate(("nolf", "lf", "plain")):
			groups.append((f"planner .{suffix}", [v[i] for v in variants]))
	groups += generated(a.work)
	instances = [p for _, ps in groups for p in ps]

	rows = bench(a.exe, instances, [(name, VERIFY + params) for name, _, params, _ in configs], a.timeout, a.jobs, verbose=False)
	with open(os.path.join(a.work, "runs.csv"), "w", newline="") as f:
		w = csv.DictWriter(f, fieldnames=["instance", "config", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts", "ext"])
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
	# The counters of vivification of a run: rounds, tried, shortened, literals removed, units, its propagations and those of the search (the statistics line of the extension)
	def viv(r):
		t = [l for l in r["ext"] if "VivifyRounds" in l]
		return [int(x) for x in t[-1].split()[3::2]] if t else [0] * 7

	md = ["# Vivification of learnt clauses: the tables", "",
		"Generated by `scripts/vivify_bench.py`; the reading is in [`vivification.md`](vivification.md).", "",
		f"Runs: {len(rows)}; instances: {len(instances)}; timeout: {a.timeout:g} s per run; every model is verified. A `-` is a run without an answer (a timeout).",
		"The conflicts and decisions are cumulative over the queries of an instance (those of the search: vivification counts neither). The status of a recorded planner instance is that of its last query.", "",
		"The configurations:", ""]
	md += [f"* `{name}`: {descr}" for name, _, _, descr in configs]
	if bad:
		md += [""] + [f"**CONTRADICTION: {i}, query {q}: SAT versus UNSAT**" for i, q in bad]

	# The summary: per group and answer, the geometric mean of (counter + eps) / (counter of the reference + eps) over the instances, which both decide and the reference takes at least 200 conflicts for
	def summary(title, counter):
		res = ["", f"## {title}", "",
			f"Per group of instances and answer: the geometric mean of the ratio of the {counter} to those of the reference configuration (`base`; `m1` for the `m1:` ones) over the instances,",
			"which both configurations decide and the reference run takes at least 200 conflicts for (n of them; `w/l`: on how many the configuration takes fewer/more conflicts);",
			"`+k`/`-k`: the instances only the configuration/only the reference decides within the timeout.", "",
			"| group | answer | n | " + " | ".join(f"{name}" for name, ref, _, _ in configs if name != ref) + " |", "|---|---|---:|" + "---:|" * sum(name != ref for name, ref, _, _ in configs)]
		def line(label, ans, ps):
			cells, ns = [], set()
			for name, ref, _, _ in configs:
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
			if ns and (max(ns) > 0 or any("+" in c or " -" in c for c in cells)):
				res.append(f"| {label} | {ans} | {'/'.join(map(str, sorted(ns)))} | " + " | ".join(cells) + " |")
		for ans in ("SAT", "UNSAT"):
			for g, ps in groups:
				line(g, ans, [p for p in ps if answer.get(inst(p)) == ans])
		for ans in ("SAT", "UNSAT"):
			line("**all the structured generated formulas**", ans, [p for g, ps in groups if g.startswith(("factoring", "3-colouring", "pigeonhole")) for p in ps if answer.get(inst(p)) == ans])
			line("**all**", ans, [p for p in instances if answer.get(inst(p)) == ans])
		return res
	md += summary("Conflicts", "confs") + summary("Decisions", "decs") + summary("CPU time", "cpu")

	# What vivification did, per group: the totals over the instances
	vivConfigs = [name for name, _, params, _ in configs if "/inprocessing/vivify/on" in params]
	md += ["", "## What vivification did", "",
		"Per group and configuration, the totals over the runs with an answer: rounds / clauses tried / clauses shortened (the share of the tried ones) / literals removed per shortened clause / new unit clauses /",
		"the propagations of vivification as a share of the propagations of the search (the budget is `/inprocessing/vivify/effort`).", ""]
	md += ["| group | " + " | ".join(vivConfigs) + " |", "|---|" + "---|" * len(vivConfigs)]
	for g, ps in groups:
		cells = []
		for name in vivConfigs:
			rs = [byKey[(inst(p), name)] for p in ps if decided(byKey[(inst(p), name)])]
			t = [sum(viv(r)[i] for r in rs) for i in range(7)]
			cells.append(f"{t[0]} / {t[1]} / {t[2]} ({100 * t[2] / t[1]:.0f}%) / {t[3] / t[2] if t[2] else 0:.1f} / {t[4]} / {100 * t[5] / t[6]:.1f}%" if t[1] and t[6] else "-")
		md.append(f"| {g} | " + " | ".join(cells) + " |")

	md += ["", "## Conflicts per instance", "", "The instances, for which some configuration takes at least 200 conflicts.", "",
		"| instance | answer | " + " | ".join(name for name, _, _, _ in configs) + " |", "|---|---|" + "---:|" * len(configs)]
	trivial = []
	for g, ps in groups:
		for p in ps:
			rs = [byKey[(inst(p), name)] for name, _, _, _ in configs]
			if all(decided(r) and r["confs"] < 200 for r in rs):
				trivial.append((g, rs))
			else:
				md.append(f"| {inst(p)} | {answer.get(inst(p), '?')} | " + " | ".join(str(r["confs"]) if decided(r) else "-" for r in rs) + " |")
	md += ["", "The other instances (under 200 conflicts in every configuration): the total conflicts per group.", "",
		"| group | instances | " + " | ".join(name for name, _, _, _ in configs) + " |", "|---|---:|" + "---:|" * len(configs)]
	for g, _ in groups:
		rss = [rs for gg, rs in trivial if gg == g]
		if rss:
			md.append(f"| {g} | {len(rss)} | " + " | ".join(str(sum(rs[i]["confs"] for rs in rss)) for i in range(len(configs))) + " |")

	md += ["", "## CPU seconds per instance", "", "The instances, for which the `base` run takes at least 0.5 s or has no answer.", "",
		"| instance | " + " | ".join(name for name, _, _, _ in configs) + " |", "|---|" + "---:|" * len(configs)]
	for p in instances:
		rs = [byKey[(inst(p), name)] for name, _, _, _ in configs]
		if not decided(rs[0]) or rs[0]["cpu"] >= 0.5:
			md.append(f"| {inst(p)} | " + " | ".join(f"{r['cpu']:.2f}" if decided(r) else "-" for r in rs) + " |")

	with open(a.out, "w") as f:
		f.write("\n".join(md) + "\n")
	print("\n".join(md))
	sys.exit(1 if bad else 0)

if __name__ == "__main__":
	main()
