#!/usr/bin/env python3
# NGBoost experiment, step 1: the instances, their runs and the data set.
#
# Generates fresh seeds of the generated families of the search-quality scripts (uniform random 3-SAT, factoring, 3-colouring, pigeonhole; none of the seeds is used elsewhere),
# derives the single-query versions of the recorded planner instances (phase_oracle_ceiling.planner_variants), runs every instance under the baseline (which writes its model
# as a hints file), target phases, vivification and the flip_factor control, and extracts the per-variable features (ngb_common.features) with the label of each variable:
# its value in the model of the baseline run. Every run verifies its model.
#
# Usage: ngb_data.py --exe PATH --work DIR [--instances DIR] [--timeout SEC] [--jobs N]
#   Writes DIR/inst/*.cnf, DIR/hints/<inst>.base.hints, DIR/runs.csv (every run), DIR/instances.json (one record per instance) and DIR/vars.npz (the per-variable data set).
import argparse, csv, glob, json, os, random, sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from search_quality_bench import bench, contradictions
from phase_oracle_ceiling import planner_variants, random_3sat, pigeonhole
from vivify_bench import factoring, colouring, is_prime
from ngb_common import VERIFY, FEATURES, Timer, features, inst_name, read_cnf, read_model

TARGET = ["/phase/target/on", "1", "/phase/rephase/conf_init", "1000"]
VIVIFY = ["/inprocessing/on", "1", "/inprocessing/conflicts_before_next", "5000", "/inprocessing/vivify/on", "1"]
CONFIGS = [("target", TARGET), ("vivify", VIVIFY), ("flip1000", ["/decision/polarity/flip_factor", "1000"])]

# (path, family, role): role "phase" marks the instances of the per-variable experiment (a fixed setting per family), "spread" those generated over a range of sizes
# and densities for the instance-level regression only
def generate(work):
	os.makedirs(work, exist_ok=True)
	res = []
	def add(name, family, role):
		res.append((os.path.join(work, name), family, role))
		return res[-1][0]
	for n in (175, 200, 225):
		for seed in range(1001, 1046):
			random_3sat(add(f"ngb_rand_n{n}_r4.20_s{seed}.cnf", "rand", "phase"), n, 4.2, seed)
	r = random.Random(7001)
	for i in range(100):
		n, ratio = r.randrange(150, 226), r.uniform(3.6, 4.8)
		random_3sat(add(f"ngb_randx_n{n}_r{ratio:.2f}_s{i}.cnf", "rand", "spread"), n, ratio, 7100 + i)
	def prime(lo, hi):
		while True:
			p = r.randrange(lo, hi) | 1
			if is_prime(p):
				return p
	for bits in (15, 16, 17, 18, 19):
		for i in range(8):
			p, q = prime(1 << (bits - 1), 1 << bits), prime(1 << (bits - 1), 1 << bits)
			factoring(add(f"ngb_factor_b{bits}_{p}x{q}.cnf", "factor", "phase"), p * q, bits)
	for bits in (12, 13, 14, 15):
		for i in range(4):
			N = prime(1 << (2 * bits - 2), 1 << (2 * bits - 1))
			factoring(add(f"ngb_prime_b{bits}_{N}.cnf", "factor", "phase"), N, bits)
	for seed in range(2001, 2061):
		colouring(add(f"ngb_col3_n350_r2.30_s{seed}.cnf", "col", "phase"), 350, round(350 * 2.30), 3, seed)
	for i in range(60):
		n, ratio = r.randrange(250, 401), r.uniform(2.10, 2.45)
		colouring(add(f"ngb_col3x_n{n}_r{ratio:.2f}_s{i}.cnf", "col", "spread"), n, round(n * ratio), 3, 7300 + i)
	for n in (7, 8):
		pigeonhole(add(f"ngb_pigeonhole_{n}.cnf", "php", "spread"), n)
	return res

def main():
	here = os.path.dirname(os.path.abspath(__file__))
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", required=True)
	ap.add_argument("--work", required=True)
	ap.add_argument("--instances", default=os.path.join(here, "..", "..", "regression_instances"))
	ap.add_argument("--timeout", type=float, default=60)
	ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
	a = ap.parse_args()
	instDir, hintsDir = os.path.join(a.work, "inst"), os.path.join(a.work, "hints")
	os.makedirs(hintsDir, exist_ok=True)
	pool = generate(instDir)
	# The planner instances: the recorded sequences, and their versions; the per-variable samples are those of the clause-only single query (.plain), whose model is the label;
	# the lf literals of the recording are a feature (the polarity the planner itself prefers)
	planner = sorted(glob.glob(os.path.join(a.instances, "planner_*.cnf")))
	lfOf = {}
	for p in planner:
		nolf, lf, plain = planner_variants(p, instDir)
		pool += [(p, "planner", "recorded"), (nolf, "planner", "nolf"), (lf, "planner", "lf"), (plain, "planner", "phase")]
		lfOf[inst_name(plain)] = p
	paths = [p for p, _, _ in pool]
	hintsFile = os.path.join(hintsDir, "{inst}.base.hints")
	rows = bench(a.exe, paths, [("base", VERIFY + ["/topor_tool/write_hints_file", hintsFile])], a.timeout, a.jobs, verbose=False)
	print("baseline runs done", flush=True)
	rows += bench(a.exe, paths, [(n, VERIFY + p) for n, p in CONFIGS], a.timeout, a.jobs, verbose=False)
	bad = contradictions(rows)
	by = {(r["instance"], r["config"]): r for r in rows}
	with open(os.path.join(a.work, "runs.csv"), "w", newline="") as f:
		w = csv.DictWriter(f, fieldnames=["instance", "config", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts"], extrasaction="ignore")
		w.writeheader()
		w.writerows(rows)

	records, Xs, ys, vs, ids = [], [], [], [], []
	for path, family, role in pool:
		base = by[(os.path.basename(path), "base")]
		rec = {"name": inst_name(path), "path": path, "family": family, "role": role, "status": base["status"], "confs": base["confs"], "decs": base["decs"], "cpu": base["cpu"]}
		for n, _ in CONFIGS:
			r = by[(os.path.basename(path), n)]
			rec[n + "_status"], rec[n + "_confs"], rec[n + "_cpu"] = r["status"], r["confs"], r["cpu"]
		# The per-variable samples: not for the planner sequences (the samples of a planner instance are those of its .plain version)
		if role in ("phase", "spread"):
			with Timer() as tp:
				clauses, lf = read_cnf(path)
			if family == "planner":
				lf = read_cnf(lfOf[inst_name(path)])[1]
			with Timer() as tf:
				v, X, instF = features(clauses, lf)
			rec.update({"parse_s": tp.s, "feature_s": tf.s, "vars": int(len(v)), "clauses": len(clauses), "inst_features": [float(x) for x in instF], "first_row": int(sum(len(x) for x in vs))})
			model = read_model(hintsFile.replace("{inst}", inst_name(path))) if base["status"] == "SAT" else {}
			y = np.array([int(model[int(x)]) if int(x) in model else -1 for x in v])
			rec["labelled"] = int((y >= 0).sum())
			Xs.append(X); ys.append(y); vs.append(v); ids.append(np.full(len(v), len(records)))
		records.append(rec)
	np.savez_compressed(os.path.join(a.work, "vars.npz"), X=np.vstack(Xs), y=np.concatenate(ys), var=np.concatenate(vs), inst=np.concatenate(ids), features=np.array(FEATURES))
	with open(os.path.join(a.work, "instances.json"), "w") as f:
		json.dump(records, f, indent=1)
	for family in ("rand", "factor", "col", "php", "planner"):
		rs = [r for r in records if r["family"] == family]
		print(family, {s: sum(r["status"] == s for r in rs) for s in sorted({r["status"] for r in rs})}, "SAT with >= 200 conflicts, phase role:", sum(r["status"] == "SAT" and r["confs"] >= 200 and r["role"] == "phase" for r in rs))
	print("contradictions:", bad)
	sys.exit(1 if bad else 0)

if __name__ == "__main__":
	main()
