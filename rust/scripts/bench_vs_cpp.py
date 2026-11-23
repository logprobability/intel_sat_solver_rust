#!/usr/bin/env python3
# Benchmarks the Rust fork against the C++ solver: every instance is solved by every executable in turn (interleaved, one process at a time),
# REPEATS times; the CPU time (user + system) and the wall time of each run are recorded, along with the verdicts and the number of conflicts.
# Usage: bench_vs_cpp.py --exe NAME=PATH [--exe NAME=PATH ...] [--repeats N] [--timeout SEC] [--csv OUT] instance.cnf ... [-- extra solver parameters]
import argparse, csv, os, re, resource, statistics, subprocess, sys, time

def run(exe, inst, params, timeout):
	before = resource.getrusage(resource.RUSAGE_CHILDREN)
	start = time.perf_counter()
	try:
		p = subprocess.run([exe, inst, "/topor_tool/print_model", "0", "/verbosity/level", "1"] + params, capture_output=True, text=True, timeout=timeout)
		out, code = p.stdout, p.returncode
	except subprocess.TimeoutExpired as e:
		out, code = (e.stdout or b"").decode() if isinstance(e.stdout, bytes) else (e.stdout or ""), "timeout"
	wall = time.perf_counter() - start
	after = resource.getrusage(resource.RUSAGE_CHILDREN)
	cpu = (after.ru_utime - before.ru_utime) + (after.ru_stime - before.ru_stime)
	verdicts = " ".join(l.split()[1][0] for l in out.splitlines() if l.startswith("s "))
	# The conflicts of the last statistics line (the column "Confs")
	confs = None
	for l in out.splitlines():
		l = re.sub(r"\x1b\[[0-9;]*m", "", l)
		if l.startswith("c ToporStt ") and "CpuT0" not in l:
			confs = int(l.split()[8])
	return {"wall": wall, "cpu": cpu, "code": code, "verdicts": verdicts, "confs": confs}

def main():
	argv = sys.argv[1:]
	params = []
	if "--" in argv:
		i = argv.index("--")
		argv, params = argv[:i], argv[i + 1:]
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", action="append", required=True)
	ap.add_argument("--repeats", type=int, default=3)
	ap.add_argument("--timeout", type=float, default=300)
	ap.add_argument("--csv")
	ap.add_argument("instances", nargs="+")
	a = ap.parse_args(argv)
	exes = [e.split("=", 1) for e in a.exe]
	rows = []
	for inst in a.instances:
		for r in range(a.repeats):
			# Alternate the order of the executables between the repeats
			for name, exe in (exes if r % 2 == 0 else exes[::-1]):
				res = run(exe, inst, params, a.timeout)
				res.update({"instance": os.path.basename(inst), "exe": name, "repeat": r})
				rows.append(res)
				print(f"{res['instance']:<20} {name:<12} rep {r} cpu {res['cpu']:8.3f}s wall {res['wall']:8.3f}s confs {res['confs']} {res['code']} {res['verdicts'][:40]}", flush=True)
	if a.csv:
		with open(a.csv, "w", newline="") as f:
			w = csv.DictWriter(f, fieldnames=["instance", "exe", "repeat", "cpu", "wall", "code", "confs", "verdicts"])
			w.writeheader()
			w.writerows(rows)

main()
