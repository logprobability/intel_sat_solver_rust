#!/usr/bin/env python3
# The final comparison of the Rust-only search extensions: the baseline, every extension alone, all of them together, the tuned configurations (--config) and a control,
# over the regression instances and over a small set of generated formulas (the regression instances but the planner ones are trivial), which no setting was chosen on.
# Per group of instances and configuration: how many instances get the verdicts of the baseline for every query, the geometric means of the ratios of the conflicts,
# the decisions and the CPU time to the baseline, the wins/losses in conflicts, and the totals. Every run verifies its models; SAT versus UNSAT for the same query
# of an instance under two configurations is a soundness bug: it is reported first, and the exit code is 1.
#
# Usage: search_quality_final.py [--exe PATH] [--instances DIR] [--work DIR] [--out FILE.md] [--timeout SEC] [--jobs N] [--repeats N] [--config NAME="<Param> <Val> ..."]...
#        search_quality_final.py --only-generate [--work DIR]     # writes the generated formulas and prints their paths (to validate a tuned configuration on)
#   The CPU time of a run is the minimum over the repeats (the counters don't depend on the repeat: this is checked).
import argparse, glob, math, os, random, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from search_quality_bench import bench, contradictions
from phase_oracle_ceiling import random_3sat, pigeonhole
from vivify_bench import factoring, colouring, is_prime

VERIFY = ["/topor_tool/verify_model", "1"]
TARGET = ["/phase/target/on", "1", "/phase/rephase/conf_init", "1000"]
VIVIFY = ["/inprocessing/on", "1", "/inprocessing/conflicts_before_next", "5000", "/inprocessing/vivify/on", "1"]
# {hints}: the file the baseline run of the instance writes (its final saved phases, the model after SAT, and its scores)
HINTS = ["/phase/hints/on", "1", "/decision/score_hints/mult", "10", "/topor_tool/hints_file", "{hints}"]
CONFIGS = [
	("base", [], "the defaults: the search of the C++ solver"),
	("hints", HINTS, "phase and score hints carried over from the baseline run of the same instance (`/phase/hints/on 1 /decision/score_hints/mult 10 /topor_tool/hints_file <file>`): "
		"advice from a previous run of the very same instance, so an upper bound of what carrying state over can give, not a setting one can just switch on"),
	("target", TARGET, "`" + " ".join(TARGET) + "`"),
	("vivify", VIVIFY, "`" + " ".join(VIVIFY) + "`"),
	("all", TARGET + VIVIFY, "`target` and `vivify` together (everything, which needs no advice)"),
	("all+hints", HINTS + TARGET + VIVIFY, "`hints`, `target` and `vivify` together"),
	("flip1000", ["/decision/polarity/flip_factor", "1000"], "the control, a perturbation without information (`/decision/polarity/flip_factor 1000`, a parameter of the C++ solver): what a change of no merit does to the counters"),
]

# Formulas, which none of the settings above was chosen on (other seeds and sizes than those of the benchmarks of the extensions)
def generated(work):
	os.makedirs(work, exist_ok=True)
	paths = []
	def add(name):
		paths.append(os.path.join(work, name))
		return paths[-1]
	r = random.Random(2026)
	def prime(lo, hi):
		while True:
			p = r.randrange(lo, hi) | 1
			if is_prime(p):
				return p
	for bits in (19, 19, 20, 20):
		N = prime(1 << (bits - 1), 1 << bits) * prime(1 << (bits - 1), 1 << bits)
		factoring(add(f"final_factor_{bits}_{N}.cnf"), N, bits)
	for bits in (15, 15, 16, 16):
		N = prime(1 << (2 * bits - 2), 1 << (2 * bits - 1))
		factoring(add(f"final_prime_{bits}_{N}.cnf"), N, bits)
	for seed in range(101, 105):
		colouring(add(f"final_col3_n350_s{seed}.cnf"), 350, round(350 * 2.34), 3, seed)
	for seed in range(101, 109):
		random_3sat(add(f"final_rand3sat_n225_s{seed}.cnf"), 225, 4.26, seed)
	pigeonhole(add("final_pigeonhole_8.cnf"), 8)
	pigeonhole(add("final_pigeonhole_9.cnf"), 9)
	return paths

def geo(xs):
	return math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")

def main():
	here = os.path.dirname(os.path.abspath(__file__))
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", default=os.path.join(here, "..", "target", "release", "intel_sat_solver"))
	ap.add_argument("--instances", default=os.path.join(here, "..", "..", "regression_instances"))
	ap.add_argument("--work", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "search_quality_final"))
	ap.add_argument("--out", default=os.path.join(here, "..", "docs", "search_quality", "final_tables.md"))
	ap.add_argument("--timeout", type=float, default=60)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
	ap.add_argument("--repeats", type=int, default=3)
	ap.add_argument("--config", action="append", default=[])
	ap.add_argument("--only-generate", action="store_true")
	a = ap.parse_args()
	gen = generated(os.path.join(a.work, "inst"))
	if a.only_generate:
		print("\n".join(gen))
		return
	hintsDir = os.path.join(a.work, "hints")
	os.makedirs(hintsDir, exist_ok=True)
	hintsFile = os.path.join(hintsDir, "{inst}.hints")
	configs = CONFIGS[:-1] + [(n, p.split(), f"`{p}`") for n, _, p in (c.partition("=") for c in a.config)] + CONFIGS[-1:]
	names = [n for n, _, _ in configs]
	recorded = sorted(glob.glob(os.path.join(a.instances, "*.cnf")))
	instances = recorded + gen

	# The baseline goes first, since it writes the hints (writing them doesn't change the search); then the rest, a.repeats times
	withHints = lambda ps: [hintsFile if p == "{hints}" else p for p in ps]
	rows = {}
	for rep in range(a.repeats):
		for part in ([("base", VERIFY + ["/topor_tool/write_hints_file", hintsFile])], [(n, VERIFY + withHints(p)) for n, p, _ in configs[1:]]):
			for r in bench(a.exe, instances, part, a.timeout, a.jobs, verbose=False):
				key = (r["instance"], r["config"])
				old = rows.setdefault(key, r)
				if (old["verdicts"], old["confs"], old["decs"]) != (r["verdicts"], r["confs"], r["decs"]) and "TIMEOUT" not in (old["status"], r["status"]):
					sys.exit(f"the run of {key} is not deterministic")
				if r["status"] != "TIMEOUT" and (old["status"] == "TIMEOUT" or r["cpu"] < old["cpu"]):
					rows[key] = r
		print(f"repeat {rep + 1} of {a.repeats} done", flush=True)
	bad = contradictions(list(rows.values()))

	base = lambda p: os.path.basename(p)
	isPlanner = lambda p: base(p).startswith("planner_")
	status = lambda p: rows[(base(p), "base")]["status"]
	groups = [
		("All the regression instances", recorded),
		("The planner instances", [p for p in recorded if isPlanner(p)]),
		("The other regression instances", [p for p in recorded if not isPlanner(p)]),
		("Generated, satisfiable", [p for p in gen if status(p) == "SAT"]),
		("Generated, unsatisfiable", [p for p in gen if status(p) == "UNSAT"]),
		("Generated, not decided by the baseline within the timeout", [p for p in gen if status(p) not in ("SAT", "UNSAT")]),
	]
	out = ["# The final comparison of the search extensions", "",
		f"Generated by `scripts/search_quality_final.py` with the release build: {len(instances)} instances ({len(recorded)} regression instances, {len(gen)} generated formulas), {len(configs)} configurations, timeout {a.timeout:g} s, "
		f"{a.jobs} jobs; the CPU time is the minimum of {a.repeats} runs. Every model was verified by the tool (`/topor_tool/verify_model 1`).", ""]
	out.append("**SAT versus UNSAT contradictions between the configurations: " + ("none.**" if not bad else ", ".join(f"{i} (query {q})" for i, q in bad) + "**"))
	out += ["", "The configurations:", ""] + [f"* `{n}`: {d}" for n, _, d in configs] + ["",
		"The columns: *same verdicts*: the instances, whose every query gets the verdict of the baseline (a difference, which is not a contradiction, is a query cut off by its conflict budget or by the timeout under one of the two); "
		"*decided*: the instances, whose last query is answered; the ratios are geometric means of (x + 1) / (baseline + 1) for the counters and (x + 0.001) / (baseline + 0.001) for the time "
		"over the instances both decide with the same answer; *wins/losses*: the instances with fewer/more conflicts than the baseline.", ""]
	for title, group in groups:
		if not group:
			continue
		out += [f"## {title} ({len(group)})", "", "| configuration | same verdicts | decided | conflicts ratio | wins/losses | decisions ratio | CPU ratio | total conflicts | total decisions | total CPU s |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
		for n in names:
			rs, bs = [rows[(base(p), n)] for p in group], [rows[(base(p), "base")] for p in group]
			common = [(r, b) for r, b in zip(rs, bs) if r["status"] == b["status"] and r["status"] in ("SAT", "UNSAT")]
			ratio = lambda c, eps: geo([(r[c] + eps) / (b[c] + eps) for r, b in common])
			out.append(f"| {n} | {sum(r['verdicts'] == b['verdicts'] and r['status'] == b['status'] for r, b in zip(rs, bs))}/{len(group)} | {sum(r['status'] in ('SAT', 'UNSAT') for r in rs)}/{len(group)} | "
				f"{ratio('confs', 1):.3f} | {sum(r['confs'] < b['confs'] for r, b in common)}/{sum(r['confs'] > b['confs'] for r, b in common)} | {ratio('decs', 1):.3f} | {ratio('cpu', 0.001):.3f} | "
				f"{sum(r['confs'] for r, _ in common)} | {sum(r['decs'] for r, _ in common)} | {sum(r['cpu'] for r, _ in common):.2f} |")
		out.append("")
	out += ["## Per instance: conflicts (the instances, which take more than 20 conflicts under some configuration)", "", "| instance | answer | " + " | ".join(names) + " |", "|---|---|" + "---:|" * len(names)]
	for p in instances:
		rs = [rows[(base(p), n)] for n in names]
		if any((r["confs"] or 0) > 20 or r["status"] not in ("SAT", "UNSAT") for r in rs):
			out.append(f"| {base(p)} | {rs[0]['status']} | " + " | ".join(str(r["confs"]) if r["status"] == rs[0]["status"] else f"{r['status']} ({r['confs']})" for r in rs) + " |")
	differ = [(base(p), n) for p in instances for n in names if rows[(base(p), n)]["verdicts"] != rows[(base(p), "base")]["verdicts"] or rows[(base(p), n)]["status"] != rows[(base(p), "base")]["status"]]
	out += ["", "Runs, whose verdicts differ from those of the baseline (none of them is a contradiction, unless reported at the top): " + (", ".join(f"{i} under `{n}` ({rows[(i, n)]['status']})" for i, n in differ) or "none") + "."]
	text = "\n".join(out) + "\n"
	print(text)
	with open(a.out, "w") as f:
		f.write(text)
	sys.exit(1 if bad else 0)

if __name__ == "__main__":
	main()
