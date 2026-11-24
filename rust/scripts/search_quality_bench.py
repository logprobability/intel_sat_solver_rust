#!/usr/bin/env python3
# Search-quality benchmark: runs the solver over a set of instances under several named configurations (sets of solver parameters) and tabulates,
# per instance and configuration, the status, the conflicts, the decisions, the restarts and the CPU time, plus the geometric means of the ratios versus the baseline.
# Search quality is measured in conflicts first (decisions and time second): the counters are those of the last statistics line, cumulative over the queries of the instance.
#
# Usage: search_quality_bench.py [--exe PATH] [--timeout SEC] [--jobs N] [--out PREFIX] [--baseline NAME]
#                                --config NAME="<Param1> <Val1> <Param2> <Val2> ..." [--config ...] instance.cnf ...
#   The first configuration is the baseline, unless --baseline names another one; NAME= alone is a configuration without parameters (the defaults).
#   In a parameter value, {inst} stands for the instance's file name without the extension, {path} for its path and {dir} for its directory,
#   e.g., --config hinted="/phase/hints/on 1 /topor_tool/hints_file hints/{inst}.hints"
#   The table is printed, and written to PREFIX.csv and PREFIX.md, if --out is given.
#   The exit code is 1, if two configurations contradict each other (SAT versus UNSAT for the same query of an instance), which is a soundness bug.
#
# As a module: run_one(exe, inst, params, timeout) runs one instance; bench(exe, instances, configs, timeout, jobs) returns the rows; summarize(rows, configs, baseline)
# returns the geometric means; contradictions(rows) the soundness violations; write_tables(...) the files.
import argparse, csv, math, os, subprocess, sys, tempfile, threading
from concurrent.futures import ThreadPoolExecutor

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench_vs_cpp import parse

COUNTERS = ["confs", "decs", "rsts", "cpu"]

# Run the solver once; the CPU time (user + system) is that of the child process itself (wait4), so the runs may be carried out in parallel
def run_one(exe, inst, params, timeout):
	with tempfile.TemporaryFile() as outFile:
		# The parameters of the configuration go first, since /topor_tool/solver_mode must precede any parameter of the solver
		p = subprocess.Popen([exe, inst] + params + ["/topor_tool/print_model", "0", "/verbosity/level", "1"], stdout=outFile, stderr=subprocess.STDOUT)
		timedOut = threading.Event()
		def kill():
			timedOut.set()
			p.kill()
		timer = threading.Timer(timeout, kill)
		timer.start()
		_, status, ru = os.wait4(p.pid, 0)
		timer.cancel()
		p.returncode = os.waitstatus_to_exitcode(status)
		outFile.seek(0)
		out = outFile.read().decode(errors="replace")
	res = parse(out)
	# The statistics lines of the Rust-only extensions (for the scripts of the extensions)
	res["ext"] = [l for l in out.splitlines() if l.startswith("c ToporExt ")]
	res["cpu"] = ru.ru_utime + ru.ru_stime
	res["code"] = "timeout" if timedOut.is_set() else p.returncode
	# The status of the instance: the verdict of its last query, or why there is none
	if timedOut.is_set():
		res["status"] = "TIMEOUT"
	elif p.returncode not in (10, 20, 30) or not res["verdicts"]:
		res["status"] = "ERROR"
	else:
		res["status"] = {"S": "SAT", "U": "UNSAT", "C": "CONFLICT_OUT"}.get(res["verdicts"].split()[-1], "ERROR")
	return res

def expand(params, inst):
	base = os.path.splitext(os.path.basename(inst))[0]
	return [p.replace("{inst}", base).replace("{path}", inst).replace("{dir}", os.path.dirname(inst) or ".") for p in params]

# configs: a list of (name, [param, val, ...]); returns one row per (instance, configuration)
def bench(exe, instances, configs, timeout=60, jobs=1, verbose=True):
	def job(inst, name, params):
		res = run_one(exe, inst, expand(params, inst), timeout)
		res.update({"instance": os.path.basename(inst), "config": name})
		if verbose:
			print(f"{res['instance']:<32} {name:<20} {res['status']:<12} confs {res['confs']} decs {res['decs']} rsts {res['rsts']} cpu {res['cpu']:.3f}s", flush=True)
		return res
	with ThreadPoolExecutor(max_workers=jobs) as ex:
		return list(ex.map(lambda a: job(*a), [(inst, name, params) for inst in instances for name, params in configs]))

# SAT versus UNSAT for the same query of the same instance under two configurations (a conflict-out or a missing verdict contradicts nothing)
def contradictions(rows):
	bad = []
	byInst = {}
	for r in rows:
		byInst.setdefault(r["instance"], []).append(r)
	for inst, rs in byInst.items():
		for i in range(max(len(r["verdicts"].split()) for r in rs)):
			vs = {r["verdicts"].split()[i] for r in rs if i < len(r["verdicts"].split())}
			if "S" in vs and "U" in vs:
				bad.append((inst, i))
	return bad

# The geometric mean, over the instances both configurations decided with the same status, of (x + eps) / (baseline + eps) per counter (eps keeps a zero count legal)
def summarize(rows, configs, baseline):
	byKey = {(r["instance"], r["config"]): r for r in rows}
	instances = sorted({r["instance"] for r in rows})
	summary = {}
	for name, _ in configs:
		common = [i for i in instances if byKey[(i, name)]["status"] == byKey[(i, baseline)]["status"] and byKey[(i, name)]["status"] in ("SAT", "UNSAT") and byKey[(i, name)]["confs"] is not None]
		s = {"n": len(common), "decided": sum(byKey[(i, name)]["status"] in ("SAT", "UNSAT") for i in instances)}
		for c in COUNTERS:
			eps = 0.001 if c == "cpu" else 1
			s[c] = math.exp(sum(math.log((byKey[(i, name)][c] + eps) / (byKey[(i, baseline)][c] + eps)) for i in common) / len(common)) if common else float("nan")
			s["sum_" + c] = sum(byKey[(i, name)][c] for i in common)
		summary[name] = s
	return summary

def write_tables(rows, configs, baseline, prefix=None, out=sys.stdout):
	byKey = {(r["instance"], r["config"]): r for r in rows}
	instances = sorted({r["instance"] for r in rows})
	names = [n for n, _ in configs]
	summary = summarize(rows, configs, baseline)
	md = ["| instance | " + " | ".join(f"{n}: status | confs | decs | rsts | cpu s" for n in names) + " |", "|---|" + "---|---:|---:|---:|---:|" * len(names)]
	for i in instances:
		md.append(f"| {i} | " + " | ".join("{status} | {confs} | {decs} | {rsts} | {cpu:.3f}".format(**byKey[(i, n)]) for n in names) + " |")
	md += ["", f"Geometric means of the ratios versus `{baseline}` over the instances both decide with the same answer (n), and the totals over them:", "",
		"| config | decided | n | confs ratio | decs ratio | rsts ratio | cpu ratio | total confs | total decs | total cpu s |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
	for n in names:
		s = summary[n]
		md.append(f"| {n} | {s['decided']}/{len(instances)} | {s['n']} | {s['confs']:.3f} | {s['decs']:.3f} | {s['rsts']:.3f} | {s['cpu']:.3f} | {s['sum_confs']} | {s['sum_decs']} | {s['sum_cpu']:.2f} |")
	noAnswer = sum(r["status"] in ("TIMEOUT", "ERROR") for r in rows)
	if noAnswer:
		md.append(f"\nRuns without an answer (TIMEOUT or ERROR): {noAnswer}")
	bad = contradictions(rows)
	for inst, q in bad:
		md.append(f"\n**CONTRADICTION: {inst}, query {q}: SAT versus UNSAT**")
	text = "\n".join(md) + "\n"
	print(text, file=out)
	if prefix:
		with open(prefix + ".md", "w") as f:
			f.write(text)
		with open(prefix + ".csv", "w", newline="") as f:
			w = csv.DictWriter(f, fieldnames=["instance", "config", "status", "confs", "decs", "rsts", "cpu", "code", "verdicts"], extrasaction="ignore")
			w.writeheader()
			w.writerows(rows)
	return summary, bad

def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("--exe", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", "intel_sat_solver"))
	ap.add_argument("--timeout", type=float, default=60)
	ap.add_argument("--jobs", type=int, default=1)
	ap.add_argument("--out")
	ap.add_argument("--baseline")
	ap.add_argument("--config", action="append", required=True)
	ap.add_argument("instances", nargs="+")
	a = ap.parse_args()
	configs = []
	for c in a.config:
		name, _, params = c.partition("=")
		params = params.split()
		if len(params) % 2 != 0:
			sys.exit(f"the configuration {name} must be a list of <Param> <Val> pairs")
		configs.append((name, params))
	baseline = a.baseline or configs[0][0]
	if baseline not in [n for n, _ in configs]:
		sys.exit(f"unknown baseline {baseline}")
	rows = bench(a.exe, a.instances, configs, a.timeout, a.jobs)
	_, bad = write_tables(rows, configs, baseline, a.out)
	sys.exit(1 if bad else 0)

if __name__ == "__main__":
	main()
