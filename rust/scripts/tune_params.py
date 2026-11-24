#!/usr/bin/env python3
# A simple configurator for a family of instances: random search over a declared subset of the solver's parameters (SPACE below), followed by coordinate descent
# from the best sample and by a simplification pass, which drops every setting the result doesn't need. The runs are those of search_quality_bench.py.
#
# The objective is PAR-2 over the tuned instances, on the conflicts (the default; cumulative over the queries of an instance) or on the CPU time:
# an instance costs its conflicts (its time), if the run decides it as the defaults do, and twice the cap otherwise. A run decides an instance as the defaults do,
# if it ends within the timeout with the same answer to the last query and conflicts-out of no more queries than the defaults (the planner instances carry conflict budgets).
# The cap is the timeout for the time, and --conf-cap for the conflicts (by default 10 times the most conflicts the defaults take for a tuned instance).
#   --agg mean: the mean of the costs, the usual PAR-2 (the instances with the most conflicts dominate it);
#   --agg geo:  the geometric mean of (cost + eps) / (the cost of the defaults + eps), where every instance weighs the same.
# The score printed is relative to the defaults in both cases (1 is the defaults, lower is better).
#
# Against overfitting (a configuration tuned on a handful of instances fits their noise first):
#   * a step of the descent is taken only if it gains more than --min-gain (relative), and the simplification drops a setting unless it is worth as much;
#   * --loo: leave-one-out: the tuning is repeated without each tuned instance in turn (reusing the runs), and the configuration is scored on the instance left out;
#   * --validate INSTANCE...: the chosen configuration is compared with the defaults on instances it was not tuned on;
#   * the report gives the scores of the random samples, which show how much a configuration without any merit moves the objective.
# Every run verifies its models (/topor_tool/verify_model 1); SAT versus UNSAT for the same query between a configuration and the defaults is a soundness bug:
# it is reported and the exit code is 1.
#
# Usage: tune_params.py [--exe PATH] [--timeout SEC] [--jobs N] [--metric confs|cpu] [--agg mean|geo] [--conf-cap N] [--random N] [--passes N] [--min-gain F] [--seed N]
#                       [--loo] [--validate INSTANCE ...] [--out FILE.md] --tune INSTANCE ...
import argparse, math, os, random, statistics, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from search_quality_bench import bench, contradictions

VERIFY = ["/topor_tool/verify_model", "1"]

# One dimension: the parameters it sets (several, if they belong together) and the values to try; the default is always a choice (nothing is passed, so it stays that of the mode)
def P(params, *vals):
	params = params.split()
	return (params[0] if len(params) == 1 else params[0] + "+", [[]] + [[t for p in params for t in (p, str(v))] for v in vals])

def VIV(n):
	return ["/inprocessing/on", "1", "/inprocessing/conflicts_before_next", str(n), "/inprocessing/vivify/on", "1"]

# The parameters are passed in this order: /mode/value goes first, since it sets the defaults of the others.
# The phase and score hints (/phase/hints/on, /decision/score_hints/mult) are not here: they do nothing without advice from outside the solver.
SPACE = [
	P("/mode/value", 1, 2, 3, 4, 5, 6, 7, 8),
	P("/restarts/numeric/conflict_thr", 100, 300, 3000),
	P("/restarts/arithmetic/conflict_increment", 0, 50, 200),
	P("/restarts/numeric/sub_strat", 1),
	P("/decision/vsids/var_decay_max /decision/vsids/var_decay_max_ai /decision/vsids/var_decay_max_s", 0.9, 0.95, 0.99),
	P("/decision/vsids/init_order /decision/vsids/init_order_ai", 0, 1),
	P("/decision/polarity/init_strategy", 0, 1, 2),
	P("/backtracking/chrono_bt_if_higher_init /backtracking/chrono_bt_if_higher_n /backtracking/chrono_bt_if_higher_s", 0, 50, 1000),
	P("/conflicts/all_uip/mode", 1, 3),
	P("/conflicts/flipped_recording_max_lbd", 0, 40),
	P("/deletion/clause/frac_to_delete", 0.5, 0.7, 0.8),
	P("/deletion/clause/glue_never_delete", 2, 3, 5),
	P("/phase/force_solution", 1),
	# The Rust-only extensions and their main knobs
	P("/phase/target/on", 1),
	P("/phase/rephase/conf_init", 100, 1000, 10000),
	P("/phase/rephase/conf_incr", 100, 10000),
	P("/phase/rephase/random", 1),
	("/inprocessing/vivify/on+", [[], VIV(1000), VIV(5000), VIV(20000)]),
	P("/inprocessing/vivify/effort", 0.03, 0.3),
	P("/inprocessing/vivify/max_glue", 2, 6),
]

def params_of(config):
	return [t for (_, choices), c in zip(SPACE, config) for t in choices[c]]

def text_of(config):
	return " ".join(params_of(config)) or "(the defaults)"

class Tuner:
	def __init__(self, a):
		self.a = a
		self.runs = {}  # (parameters, instance) -> row
		self.bad = []   # contradictions with the defaults
		self.nRuns = 0
		self.default = (0,) * len(SPACE)

	def rows(self, params, instances):
		key = " ".join(params)
		todo = [i for i in instances if (key, i) not in self.runs]
		if todo:
			self.nRuns += len(todo)
			for r in bench(self.a.exe, todo, [("c", VERIFY + params)], self.a.timeout, self.a.jobs, verbose=False):
				self.runs[(key, next(i for i in todo if os.path.basename(i) == r["instance"]))] = r
			for i in todo:
				if key and ("", i) in self.runs and contradictions([self.runs[("", i)], self.runs[(key, i)]]):
					self.bad.append((os.path.basename(i), key))
					print(f"CONTRADICTION (SAT versus UNSAT): {os.path.basename(i)} under {key}", flush=True)
		return [self.runs[(key, i)] for i in instances]

	def decided(self, r, d):
		undecided = lambda x: x["verdicts"].split().count("C")
		return r["status"] in ("SAT", "UNSAT") and r["status"] == d["status"] and undecided(r) <= undecided(d) and r[self.a.metric] is not None

	# The costs of the instances: (cost, the cost of the defaults) per instance
	def costs(self, params, instances):
		defaults = self.rows([], instances)
		cap = self.a.timeout if self.a.metric == "cpu" else self.confCap
		return [(r[self.a.metric] if self.decided(r, d) else 2 * cap, d[self.a.metric] if d["status"] in ("SAT", "UNSAT") else 2 * cap) for r, d in zip(self.rows(params, instances), defaults)]

	def score_of(self, pairs):
		eps = 0.01 if self.a.metric == "cpu" else 1
		if self.a.agg == "mean":
			return (sum(c for c, _ in pairs) + eps) / (sum(d for _, d in pairs) + eps)
		return math.exp(sum(math.log((c + eps) / (d + eps)) for c, d in pairs) / len(pairs))

	# All the tuned instances are always run (so that leave-one-out reuses the runs); the score is over the given ones
	def score(self, config, instances):
		pairs = self.costs(params_of(config), self.a.tune)
		return self.score_of([p for p, i in zip(pairs, self.a.tune) if i in instances])

	def search(self, instances, log):
		r = random.Random(self.a.seed)
		# Random search: every dimension leaves its default with the probability 0.3
		samples = [tuple(r.randrange(1, len(choices)) if r.random() < 0.3 else 0 for _, choices in SPACE) for _ in range(self.a.random)]
		scores = [self.score(c, instances) for c in samples]
		best, bestScore = self.default, 1.0
		for c, s in zip(samples, scores):
			if s < bestScore * (1 - self.a.min_gain):
				best, bestScore = c, s
		log(f"random search: {len(samples)} samples, scores min {min(scores):.3f}, median {statistics.median(scores):.3f}, max {max(scores):.3f}; {sum(s < 1 for s in scores)} of them better than the defaults; the best one kept: {bestScore:.3f}")
		# Coordinate descent: one dimension at a time, every value of it
		for p in range(self.a.passes):
			improved = False
			for d, (name, choices) in enumerate(SPACE):
				for v in range(len(choices)):
					c = best[:d] + (v,) + best[d + 1:]
					if c != best:
						s = self.score(c, instances)
						if s < bestScore * (1 - self.a.min_gain):
							best, bestScore, improved = c, s, True
			log(f"coordinate descent, pass {p + 1}: {bestScore:.3f}")
			if not improved:
				break
		# Simplification: back to the default in every dimension, which isn't worth min_gain
		for d in range(len(SPACE)):
			if best[d] != 0:
				c = best[:d] + (0,) + best[d + 1:]
				s = self.score(c, instances)
				if s <= bestScore * (1 + self.a.min_gain):
					best, bestScore = c, min(s, bestScore)
		bestScore = self.score(best, instances)
		log(f"after the simplification: {bestScore:.3f}: `{text_of(best)}`")
		return best, bestScore, scores

def compare_table(t, config, instances, out):
	metric = t.a.metric
	rows, defaults = t.rows(params_of(config), instances), t.rows([], instances)
	out.append(f"| instance | defaults: status | {metric} | tuned: status | {metric} | ratio |")
	out.append("|---|---|---:|---|---:|---:|")
	eps = 0.01 if metric == "cpu" else 1
	ratios, hard = [], []
	for i, r, d in zip(instances, rows, defaults):
		ok = t.decided(r, d)
		if ok:
			ratios.append((r[metric] + eps) / (d[metric] + eps))
			if d["confs"] >= 200:
				hard.append(ratios[-1])
		fmt = (lambda x: "-" if x is None else f"{x:.3f}") if metric == "cpu" else (lambda x: "-" if x is None else str(x))
		# The rows of the instances, which the defaults settle within 20 conflicts, are left out of a long table
		if len(instances) <= 20 or not ok or r["status"] != d["status"] or max(r["confs"] or 0, d["confs"] or 0) > 20:
			out.append(f"| {os.path.basename(i)} | {d['status']} | {fmt(d[metric])} | {r['status']}{'' if ok else ' (not as the defaults)'} | {fmt(r[metric])} | {ratios[-1] if ok else float('nan'):.3f} |")
	wins, losses = sum(x < 1 for x in ratios), sum(x > 1 for x in ratios)
	geo = math.exp(sum(map(math.log, ratios)) / len(ratios)) if ratios else float("nan")
	tot = lambda rs: sum(r[metric] for r, d in zip(rs, defaults) if t.decided(r, d))
	out.append("")
	out.append(f"{len(instances)} instances, {len(ratios)} decided as by the defaults; geometric mean of the ratio {geo:.3f}; better on {wins}, worse on {losses}, equal on {len(ratios) - wins - losses}; "
		f"total {metric} over them: {tot(defaults) if metric == 'confs' else round(tot(defaults), 2)} -> {tot(rows) if metric == 'confs' else round(tot(rows), 2)}; PAR-2 score ({t.a.agg}) {t.score_of(t.costs(params_of(config), instances)):.3f}")
	if hard and len(hard) < len(ratios):
		out.append(f"Of them, the {len(hard)} instances the defaults take at least 200 conflicts for: geometric mean of the ratio {math.exp(sum(map(math.log, hard)) / len(hard)):.3f}; better on {sum(x < 1 for x in hard)}, worse on {sum(x > 1 for x in hard)}.")
	out.append("")

def main():
	here = os.path.dirname(os.path.abspath(__file__))
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", default=os.path.join(here, "..", "target", "release", "intel_sat_solver"))
	ap.add_argument("--timeout", type=float, default=20)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
	ap.add_argument("--metric", choices=["confs", "cpu"], default="confs")
	ap.add_argument("--agg", choices=["mean", "geo"], default="mean")
	ap.add_argument("--conf-cap", type=int)
	ap.add_argument("--random", type=int, default=40)
	ap.add_argument("--passes", type=int, default=2)
	ap.add_argument("--min-gain", type=float, default=0.02)
	ap.add_argument("--seed", type=int, default=1)
	ap.add_argument("--loo", action="store_true")
	ap.add_argument("--tune", nargs="+", required=True)
	ap.add_argument("--validate", nargs="*", default=[])
	ap.add_argument("--out")
	a = ap.parse_args()
	a.validate = [i for i in a.validate if i not in a.tune]
	t = Tuner(a)
	out = []
	def log(s):
		print(s, flush=True)
		out.append("* " + s)

	defaults = t.rows([], a.tune)
	t.confCap = a.conf_cap or 10 * max([r["confs"] or 0 for r in defaults] + [1])
	out += [f"# Tuning on {len(a.tune)} instances", "",
		f"`tune_params.py`: objective PAR-2 on `{a.metric}` (`--agg {a.agg}`), timeout {a.timeout:g} s" + (f", conflict cap {t.confCap}" if a.metric == "confs" else "") +
		f", {a.random} random samples, at most {a.passes} passes of coordinate descent, minimal gain {a.min_gain:g}, seed {a.seed}. Scores are relative to the defaults (1.000).", "",
		"The space (the default of every dimension is a choice too):", ""]
	out += [f"* `{name}`: " + ", ".join("`" + " ".join(c) + "`" for c in choices[1:]) if name.endswith("+") else f"* `{name}`: " + ", ".join(c[1] for c in choices[1:]) for name, choices in SPACE]
	out.append("")
	best, bestScore, _ = t.search(a.tune, log)
	out += ["", f"**The tuned configuration** (score {bestScore:.3f}): `{text_of(best)}`", "", "## On the tuned instances", ""]
	compare_table(t, best, a.tune, out)

	if a.loo and len(a.tune) > 1:
		out += ["## Leave-one-out", "", "The tuning repeated without one instance (the runs are reused), and the result on the instance left out: what the tuned configuration is expected to be worth on an unseen instance of the family.", "",
			f"| left out | configuration tuned on the others | its score on them | {a.metric}: defaults | tuned on the others | ratio |", "|---|---|---:|---:|---:|---:|"]
		ratios = []
		for i in a.tune:
			c, s, _ = t.search([j for j in a.tune if j != i], lambda s: None)
			(cost, base), = [p for p, j in zip(t.costs(params_of(c), a.tune), a.tune) if j == i]
			eps = 0.01 if a.metric == "cpu" else 1
			ratios.append((cost + eps) / (base + eps))
			out.append(f"| {os.path.basename(i)} | `{text_of(c)}` | {s:.3f} | {base:g} | {cost:g} | {ratios[-1]:.3f} |")
			print(f"leave-one-out {os.path.basename(i)}: {ratios[-1]:.3f} with {text_of(c)}", flush=True)
		out += ["", f"Geometric mean of the ratio on the instances left out: {math.exp(sum(map(math.log, ratios)) / len(ratios)):.3f} (a cost of twice the cap stands for a run, which doesn't decide the instance as the defaults do).", ""]

	if a.validate:
		out += ["## Validation: the instances the configuration was not tuned on", ""]
		compare_table(t, best, a.validate, out)

	out.append(f"Solver runs: {t.nRuns}.")
	for inst, key in t.bad:
		out.append(f"\n**CONTRADICTION (SAT versus UNSAT) with the defaults: {inst} under `{key}`**")
	text = "\n".join(out) + "\n"
	print("\n" + text)
	if a.out:
		with open(a.out, "w") as f:
			f.write(text)
	sys.exit(1 if t.bad else 0)

if __name__ == "__main__":
	main()
