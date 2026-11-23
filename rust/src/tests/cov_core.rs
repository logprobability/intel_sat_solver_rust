// Targeted unit tests (see mod.rs): Topi.cc (topi_main.rs), TopiWL.cc, TopiBcp.cc, TopiDebugPrinting.cc and the leftovers of a few containers

use crate::fuzzing::*;
use crate::topi::*;
use crate::topi_main::DoubleToOstreamStr;
use crate::topor_dyn_array::alloc_fault;
use std::cell::RefCell;
use std::rc::Rc;

type TTopi = CTopi<u32, false>;
type TTopiC = CTopi<u64, true>;

const NoTo: (f64, bool) = (f64::MAX, true);

// A fresh directory for the files of one test
fn TmpDir(name: &str) -> std::path::PathBuf
{
	let d = std::env::temp_dir().join(format!("topor_cov_core_{}_{}", std::process::id(), name));
	let _ = std::fs::remove_dir_all(&d);
	std::fs::create_dir_all(&d).unwrap();
	d
}

// The pigeon-hole principle: pigeons into holes; unsatisfiable iff pigeons > holes
fn Php(pigeons: TLit, holes: TLit) -> Vec<Vec<TLit>>
{
	let V = |p: TLit, h: TLit| p * holes + h + 1;
	let mut clss = vec![];
	for p in 0..pigeons
	{
		clss.push((0..holes).map(|h| V(p, h)).collect());
	}
	for h in 0..holes
	{
		for p1 in 0..pigeons
		{
			for p2 in p1 + 1..pigeons
			{
				clss.push(vec![-V(p1, h), -V(p2, h)]);
			}
		}
	}
	clss
}

fn AddClss<Ind: TUInd, const Compress: bool>(topi: &mut CTopi<Ind, Compress>, clss: &[Vec<TLit>])
{
	for c in clss
	{
		topi.AddUserClause(c);
	}
}

// A satisfiable instance with long clauses only, which isn't solved by the first decision
fn LongSat() -> Vec<Vec<TLit>>
{
	vec![vec![1, 2, 3], vec![-1, -2, -3], vec![4, 5, 6], vec![-4, -5, -6], vec![7, 8, 9], vec![-7, -8, -9], vec![1, 4, 7], vec![-3, -6, -9]]
}

fn ModelSatisfies<Ind: TUInd, const Compress: bool>(topi: &CTopi<Ind, Compress>, clss: &[Vec<TLit>]) -> bool
{
	clss.iter().all(|c| c.iter().any(|l| topi.GetValue(*l) == TToporLitVal::VAL_SATISFIED))
}

// Runs the given test of this file in a child process with the given environment; the child test is expected to print CHILD_OK
// This is how the code depending on the (process-global) environment variables is tested without any impact on the tests running in parallel
fn RunChild(testName: &str, envs: &[(&str, &str)])
{
	let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
	cmd.args(["--exact", &format!("tests::cov_core::{}", testName), "--nocapture", "--test-threads=1"]).env("TOPOR_COV_CORE_CHILD", "1");
	for (k, v) in envs
	{
		cmd.env(k, v);
	}
	let out = cmd.output().unwrap();
	let stdout = String::from_utf8_lossy(&out.stdout);
	assert!(out.status.success() && stdout.contains("CHILD_OK"), "The child test {} failed:\n{}\n{}", testName, stdout, String::from_utf8_lossy(&out.stderr));
}

fn IsChild() -> bool
{
	std::env::var("TOPOR_COV_CORE_CHILD").is_ok()
}

/*
* Topi.cc
*/

#[test]
fn double_to_ostream_str()
{
	assert_eq!(DoubleToOstreamStr(f64::NAN), "nan");
	assert_eq!(DoubleToOstreamStr(f64::INFINITY), "inf");
	assert_eq!(DoubleToOstreamStr(f64::NEG_INFINITY), "-inf");
	assert_eq!(DoubleToOstreamStr(0.), "0");
	assert_eq!(DoubleToOstreamStr(-0.), "-0");
	assert_eq!(DoubleToOstreamStr(1.), "1");
	assert_eq!(DoubleToOstreamStr(1.5), "1.5");
	assert_eq!(DoubleToOstreamStr(-0.25), "-0.25");
	assert_eq!(DoubleToOstreamStr(100000.), "100000");
	assert_eq!(DoubleToOstreamStr(1000000.), "1e+06");
	assert_eq!(DoubleToOstreamStr(123456789.), "1.23457e+08");
	assert_eq!(DoubleToOstreamStr(0.0001), "0.0001");
	assert_eq!(DoubleToOstreamStr(0.00001234), "1.234e-05");
	assert_eq!(DoubleToOstreamStr(f64::MAX), "1.79769e+308");
	assert_eq!(DoubleToOstreamStr(3.14159265), "3.14159");
}

#[test]
fn dump_set_up_creates_the_file()
{
	let d = TmpDir("dump_set_up");
	let mut topi = TTopi::new(3);
	topi.DumpSetUp(d.join("dsu").to_str().unwrap());
	assert!(topi.m_DumpFile.is_some());
	topi.AddUserClause(&[1, -2]);
	drop(topi);
	let files: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().path()).collect();
	assert_eq!(files.len(), 1);
	let name = files[0].file_name().unwrap().to_str().unwrap().to_string();
	assert!(name.starts_with("dsu_0x") && name.ends_with(".cnf") && name.contains(&format!("_{}_", std::process::id())), "{}", name);
	assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), "1 -2 0\n");
	// A file, which cannot be created
	let mut topi = TTopi::new(3);
	topi.DumpSetUp(d.join("no_such_dir").join("dsu").to_str().unwrap());
	assert!(topi.m_DumpFile.is_none());
	let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn dump_file_writes()
{
	let d = TmpDir("dump_file_writes");
	let path = d.join("dump.cnf");
	let mut topi = TTopi::new(0);
	topi.m_DumpFile = Some(std::fs::File::create(&path).unwrap());
	topi.SetParam("/restarts/numeric/conflict_thr", 500.);
	topi.SetParam("/deletion/clause/activity_decay", 0.5);
	topi.AddUserClause(&[1, -2, 3]);
	topi.AddUserClause(&[-1]);
	topi.AddUserClause(&[2, 0, 3]);
	assert_eq!(topi.Solve(&[2, 3], (2.5, false), 100), TToporReturnVal::RET_SAT);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	topi.Backtrack(0, false, true);
	// Only the directives are dumped from now on
	topi.SetParam("/debugging/dont_dump_clauses", 1.);
	topi.AddUserClause(&[4, 5]);
	assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	// The parameters are dumped even if the solver is in an unrecoverable state
	topi.AddUserClause(&[]);
	topi.SetParam("/verbosity/level", 0.);
	assert_eq!(topi.Solve(&[], NoTo, 7), TToporReturnVal::RET_UNSAT);
	drop(topi);
	let dump = std::fs::read_to_string(&path).unwrap();
	let expected = format!("r /restarts/numeric/conflict_thr 500\nr /deletion/clause/activity_decay 0.5\n1 -2 3 0\n-1 0\n2 0 3 0\not 2.5 0\noc 100\ns 2 3 0\nc {}\not 1.79769e+308 1\noc {}\ns  0\nc {}\nb 0\nr /debugging/dont_dump_clauses 1\not 1.79769e+308 1\noc {}\ns 1 0\nc {}\nr /verbosity/level 0\not 1.79769e+308 1\noc 7\ns  0\n",
		TToporReturnVal::RET_SAT, u64::MAX, TToporReturnVal::RET_SAT, u64::MAX, TToporReturnVal::RET_UNSAT);
	assert_eq!(dump, expected);
	let _ = std::fs::remove_dir_all(&d);
}

// The child of config_file: the only test running in its process, so it may freely set the environment variable
#[test]
fn config_file_child()
{
	if !IsChild()
	{
		return;
	}
	let d = std::path::PathBuf::from(std::env::var("TOPOR_COV_CORE_DIR").unwrap());
	let path = d.join("config.txt");
	let New = |content: Option<&str>| -> TTopi
	{
		match content
		{
			Some(content) => std::fs::write(&path, content).unwrap(),
			None =>
			{
				let _ = std::fs::remove_file(&path);
			}
		}
		std::env::set_var("TOPOR_CONFIG_FILE", &path);
		let topi = TTopi::new(0);
		std::env::remove_var("TOPOR_CONFIG_FILE");
		topi
	};

	// Legal files: the value is the longest prefix, which is a number (after skipping any leading white-space)
	let topi = New(Some("/restarts/numeric/conflict_thr 17\n/deletion/clause/trigger_init   \t23abc\n/deletion/clause/activity_decay 0.25e0é"));
	assert!(!topi.IsError(), "{}", topi.GetStatusExplanation());
	assert_eq!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 17);
	assert_eq!(topi.m_Params.m_ParamClsDelLowTriggerInit.get(), 23);
	assert_eq!(topi.m_Params.m_ParamClsLowDelActivityDecay.get(), 0.25);
	let topi = New(Some(""));
	assert!(!topi.IsError());
	let mut topi = New(Some("/verbosity/level 0\n"));
	assert!(!topi.IsError());
	topi.AddUserClause(&[1, 2]);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);

	// The file cannot be opened
	let mut topi = New(None);
	assert!(topi.IsError() && topi.GetStatusExplanation().starts_with("Cannot open the configuration parameter file "), "{}", topi.GetStatusExplanation());
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_PARAM_ERROR);

	// No space
	let topi = New(Some("/restarts/numeric/conflict_thr 17\n/verbosity/level\n"));
	assert!(topi.IsError());
	assert_eq!(topi.GetStatusExplanation(), "Could parse line2 : /verbosity/level (after trimming \\t\\r\\v\\n); Reason: space not found");
	assert_eq!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 17);

	// Not a number
	let topi = New(Some("/verbosity/level abc\n"));
	assert!(topi.IsError());
	assert_eq!(topi.GetStatusExplanation(), "Could parse line1 : /verbosity/level abc (after trimming \\t\\r\\v\\n); Reason: couldn't convert the value abc to double");
	let topi = New(Some("/verbosity/level \n"));
	assert!(topi.IsError());

	// A leading space is not trimmed, so the name of the parameter is empty
	let topi = New(Some(" /verbosity/level 1\n"));
	assert!(topi.IsError());

	// A wrong parameter name; the rest of the file is skipped
	let topi = New(Some("/no/such/param 1\n/restarts/numeric/conflict_thr 17\n"));
	assert!(topi.IsError());
	assert_ne!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 17);

	// A wrong value
	let topi = New(Some("/verbosity/level 55\n"));
	assert!(topi.IsError());

	println!("CHILD_OK");
}

#[test]
fn config_file()
{
	let d = TmpDir("config_file");
	RunChild("config_file_child", &[("TOPOR_COV_CORE_DIR", d.to_str().unwrap())]);
	let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn set_param_contexts_and_errors()
{
	let mut topi = TTopi::new(0);
	// The context prefixes: the parameter is set after the initial invocation / for the lifetime of every short invocation
	topi.SetParam("/__ai/restarts/numeric/conflict_thr", 11.);
	topi.SetParam("/__s/restarts/numeric/conflict_thr", 22.);
	assert_eq!(topi.m_AfterInitInvParamVals, vec![("/restarts/numeric/conflict_thr".to_string(), 11.)]);
	assert_eq!(topi.m_ShortInvLifetimeParamVals, vec![("/restarts/numeric/conflict_thr".to_string(), 22.)]);
	assert_eq!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 1000);
	AddClss(&mut topi, &LongSat());
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 11);
	// A short query
	assert_eq!(topi.Solve(&[1], NoTo, 5), TToporReturnVal::RET_SAT);
	assert_eq!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 22);
	// A normal query
	assert_eq!(topi.Solve(&[2], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert!(!topi.IsError());

	// The multipliers and the initial order of the VSIDS heap
	{
		let mut topi = TTopi::new(0);
		topi.SetParam("/multiplier/clauses", 1.5);
		topi.SetParam("/multiplier/variables", 1.25);
		topi.SetParam("/multiplier/watches", 1.125);
		topi.SetParam("/decision/vsids/init_order", 1.);
		topi.SetParam("/mode/value", 3.);
		assert!(!topi.IsError());
		assert!(topi.m_B.GetMultiplier() == 1.5 && topi.m_VarInfo.GetMultiplier() == 1.25 && topi.m_W.GetMultiplier() == 1.125);
		// The C++ version considers this non-existing parameter to be a multiplier parameter
		topi.SetParam("/multiplier/watches_if_separate", 1.);
		assert!(topi.IsError());
	}

	// A wrong short-invocation parameter is discovered only by the next short query
	topi.SetParam("/__s/no/such/param", 1.);
	assert!(!topi.IsError());
	assert_eq!(topi.Solve(&[], NoTo, 5), TToporReturnVal::RET_PARAM_ERROR);
	assert!(topi.IsError());
	// No impact, once in an unrecoverable state
	topi.SetParam("/restarts/numeric/conflict_thr", 33.);
	assert_ne!(topi.m_Params.m_ParamRestartNumericInitConfThr.get(), 33);

	let mut topi = TTopi::new(0);
	topi.SetParam("/no/such/param", 1.);
	assert!(topi.IsError() && !topi.GetStatusExplanation().is_empty());
	assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_PARAM_ERROR);
	let mut topi = TTopi::new(0);
	topi.SetParam("/verbosity/level", 4.);
	assert!(topi.IsError());
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_PARAM_ERROR);
}

#[test]
fn status_to_ret_val()
{
	let topi = TTopi::new(0);
	let expected = [
		(TToporStatus::STATUS_UNDECIDED, TToporReturnVal::RET_EXOTIC_ERROR),
		(TToporStatus::STATUS_SAT, TToporReturnVal::RET_SAT),
		(TToporStatus::STATUS_UNSAT, TToporReturnVal::RET_UNSAT),
		(TToporStatus::STATUS_USER_INTERRUPT, TToporReturnVal::RET_USER_INTERRUPT),
		(TToporStatus::STATUS_CONTRADICTORY, TToporReturnVal::RET_UNSAT),
		(TToporStatus::STATUS_ALLOC_FAILED, TToporReturnVal::RET_MEM_OUT),
		(TToporStatus::STATUS_INDEX_TOO_NARROW, TToporReturnVal::RET_INDEX_TOO_NARROW),
		(TToporStatus::STATUS_PARAM_ERROR, TToporReturnVal::RET_PARAM_ERROR),
		(TToporStatus::STATUS_ASSUMPTION_REQUIRED_ERROR, TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR),
		(TToporStatus::STATUS_GLOBAL_TIMEOUT, TToporReturnVal::RET_TIMEOUT_GLOBAL),
		(TToporStatus::STATUS_DRAT_FILE_PROBLEM, TToporReturnVal::RET_DRAT_FILE_PROBLEM),
		(TToporStatus::STATUS_COMPRESSED_MISMATCH, TToporReturnVal::RET_EXOTIC_ERROR),
		(TToporStatus::STATUS_EXOTIC_ERROR, TToporReturnVal::RET_EXOTIC_ERROR),
	];
	for (status, ret) in expected
	{
		topi.SetStatus(status, "test".to_string());
		assert_eq!(topi.StatusToRetVal(), ret, "{:?}", status as u8);
		if topi.IsUnrecoverable()
		{
			assert_eq!(topi.UnrecStatusToRetVal(), ret);
		}
		assert_eq!(topi.IsError(), status >= TToporStatus::STATUS_ALLOC_FAILED);
	}
}

#[test]
fn is_assumption_required_errors()
{
	// No Solve invocations so far
	let mut topi = TTopi::new(0);
	topi.AddUserClause(&[1, 2]);
	assert!(!topi.IsAssumptionRequired(0));
	assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR);
	assert_eq!(topi.GetStatusExplanation(), "No Solve invocations so far");

	// The latest Solve wasn't unsatisfiable
	let mut topi = TTopi::new(0);
	topi.AddUserClause(&[1, 2]);
	assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert!(!topi.IsAssumptionRequired(0));
	assert_eq!(topi.GetStatusExplanation(), "The latest Solve didn't return TToporStatus::STATUS_UNSAT");
	assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR);

	// The index is beyond the assumptions
	let mut topi = TTopi::new(0);
	topi.AddUserClause(&[1, 2]);
	topi.AddUserClause(&[-1, 2]);
	assert_eq!(topi.Solve(&[1, -2], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(topi.IsAssumptionRequired(1));
	assert!(!topi.IsError());
	assert!(!topi.IsAssumptionRequired(2));
	assert_eq!(topi.GetStatusExplanation(), "The assumption ID is beyond the number of assumptions in the latest Solve invocation");

	// A contradictory instance: no assumption is required
	let mut topi = TTopi::new(0);
	topi.AddUserClause(&[1]);
	topi.AddUserClause(&[-1]);
	assert!(!topi.IsAssumptionRequired(5));
	assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(!topi.IsAssumptionRequired(0));
	assert!(!topi.IsError());
}

#[test]
fn local_timeouts()
{
	// A zero timeout
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &LongSat());
	assert_eq!(topi.Solve(&[], (0., true), u64::MAX), TToporReturnVal::RET_TIMEOUT_LOCAL);
	assert_eq!(topi.Solve(&[], (0., false), u64::MAX), TToporReturnVal::RET_TIMEOUT_LOCAL);
	assert!(!topi.IsError());
	topi.SetParam("/verbosity/level", 1.);
	assert_eq!(topi.Solve(&[], (0., false), u64::MAX), TToporReturnVal::RET_TIMEOUT_LOCAL);
	assert_eq!(topi.GetStatusExplanation(), "Wall timeout of 0.000000 for the current Solve invocation reached");
	topi.SetParam("/verbosity/level", 0.);
	// Timeouts, which aren't reached
	assert_eq!(topi.Solve(&[], (1e6, true), u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topi.Solve(&[-1], (1000., false), u64::MAX), TToporReturnVal::RET_SAT);

	// A wall timeout, which is reached after the first decision: the stop-now callback makes sure the time passes
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &Php(5, 4));
	topi.SetCbStopNow(Some(Box::new(||
	{
		std::thread::sleep(std::time::Duration::from_millis(2));
		TStopTopor::VAL_CONTINUE
	})));
	assert_eq!(topi.Solve(&[], (1e-3, false), u64::MAX), TToporReturnVal::RET_TIMEOUT_LOCAL);
	assert!(!topi.IsError());
	topi.SetCbStopNow(None);
	// The CPU time is measured in whole seconds for the whole process, so reaching a CPU timeout cannot be tested deterministically
	assert_eq!(topi.Solve(&[], (1e6, true), u64::MAX), TToporReturnVal::RET_UNSAT);
}

#[test]
fn global_timeouts()
{
	// The global timeout is discovered at the beginning of Solve
	for isCpu in [false, true]
	{
		let mut topi = TTopi::new(0);
		AddClss(&mut topi, &LongSat());
		topi.SetParam("/timeout/global_is_cpu", isCpu as u8 as f64);
		topi.SetParam("/timeout/global", if isCpu { 1e6 } else { 1e-9 });
		assert!(topi.m_Stat.m_OverallTime.IsTimeoutSet());
		std::thread::sleep(std::time::Duration::from_millis(1));
		let ret = topi.Solve(&[1, 2], (5., isCpu), u64::MAX);
		if isCpu
		{
			assert_eq!(ret, TToporReturnVal::RET_SAT);
		}
		else
		{
			assert_eq!(ret, TToporReturnVal::RET_TIMEOUT_GLOBAL);
			assert!(topi.IsError());
			assert_eq!(topi.GetStatusExplanation(), "Global Wall timeout of 5.000000 reached");
			assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_TIMEOUT_GLOBAL);
		}
	}

	// The global timeout is discovered in the main loop
	// The time is measured once in 1000 tests: the first one is carried out here (way before the timeout), the last one before the next measurement is carried out
	// at the beginning of Solve, and the stop-now callback makes sure the timeout is reached before the next measurement, which occurs after the first decision
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &Php(5, 4));
	topi.SetParam("/timeout/global", 0.3);
	topi.m_Stat.m_OverallTime.Reset();
	for _ in 0..999
	{
		assert!(!topi.m_Stat.m_OverallTime.IsTimeout());
	}
	let mut slept = false;
	topi.SetCbStopNow(Some(Box::new(move ||
	{
		if !slept
		{
			std::thread::sleep(std::time::Duration::from_millis(350));
			slept = true;
		}
		TStopTopor::VAL_CONTINUE
	})));
	assert_eq!(topi.Solve(&[], (1000., true), u64::MAX), TToporReturnVal::RET_TIMEOUT_GLOBAL);
	assert_eq!(topi.GetStatusExplanation(), "Global CPU timeout of 1000.000000 reached");
	assert_eq!(topi.m_Stat.m_Conflicts, 0);
}

#[test]
fn stop_callback_and_interrupt()
{
	// Interrupted before the first decision
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &Php(5, 4));
	topi.InterruptNow();
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	assert_eq!(topi.m_Stat.m_Conflicts, 0);
	assert!(!topi.m_InterruptNow.load(std::sync::atomic::Ordering::Relaxed));
	assert!(!topi.IsError());

	// Interrupted through the handle from within the stop-now callback (which itself doesn't request to stop)
	let handle = topi.GetInterruptNowHandle();
	let mut invs = 0;
	topi.SetCbStopNow(Some(Box::new(move ||
	{
		invs += 1;
		if invs == 3
		{
			handle.store(true, std::sync::atomic::Ordering::Relaxed);
		}
		TStopTopor::VAL_CONTINUE
	})));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	topi.m_Params.m_ParamVerbosity.set(1);
	assert_eq!(topi.GetStatusExplanation(), "Interrupt by the Interrupt callback");
	topi.m_Params.m_ParamVerbosity.set(0);

	// Stopped by the stop-now callback
	let mut invs = 0;
	topi.SetCbStopNow(Some(Box::new(move ||
	{
		invs += 1;
		if invs == 2 { TStopTopor::VAL_STOP } else { TStopTopor::VAL_CONTINUE }
	})));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	topi.m_Params.m_ParamVerbosity.set(1);
	assert_eq!(topi.GetStatusExplanation(), "Interrupt by the StopNow callback");
	topi.m_Params.m_ParamVerbosity.set(0);

	// Completed
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	// A conflict threshold
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &Php(6, 5));
	assert_eq!(topi.Solve(&[], NoTo, 2), TToporReturnVal::RET_CONFLICT_OUT);
	assert_eq!(topi.GetConflictsNumber(), 2);
}

#[test]
fn getters()
{
	let mut topi = TTopi::new(0);
	// A value of a variable, which is beyond the variables of the solver
	assert_eq!(topi.GetValue(7), TToporLitVal::VAL_DONT_CARE);
	topi.AddUserClause(&[4]);
	topi.AddUserClause(&[4, 5, 6, 7]);
	topi.AddUserClause(&[-5, -6]);
	// 1..3 don't appear in any clause
	assert_eq!(topi.GetValue(2), TToporLitVal::VAL_DONT_CARE);
	assert_eq!(topi.GetValue(-4), TToporLitVal::VAL_UNSATISFIED);
	assert_eq!(topi.GetValue(5), TToporLitVal::VAL_UNASSIGNED);
	assert_eq!(topi.Solve(&[6], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topi.GetValue(4), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topi.GetValue(-5), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topi.GetLitDecLevel(4), 0);
	assert_eq!(topi.GetLitDecLevel(-6), 1);
	assert_eq!(topi.GetLitDecLevel(5), 1);
	let m = topi.GetModel();
	assert_eq!(m.len(), topi.m_E2ILitMap.cap());
	assert_eq!(m[0], TToporLitVal::VAL_SATISFIED);
	assert_eq!(m[1], TToporLitVal::VAL_DONT_CARE);
	assert_eq!(m[4], TToporLitVal::VAL_SATISFIED);
	assert_eq!(m[5], TToporLitVal::VAL_UNSATISFIED);
	assert_eq!(m[6], TToporLitVal::VAL_SATISFIED);
	assert_eq!(topi.GetStatistics().m_SolveInvs, 1);
	assert_eq!(topi.GetSolveInvs(), 1);
	assert_eq!(topi.GetMaxUserVar(), 7);
	assert!(topi.GetMaxInternalVar() >= 3);
	assert!(topi.GetParamsDescr().contains("/verbosity/level"));

	// The memory layout
	assert_eq!(topi.GetMemoryLayout(), "");
	topi.SetParam("/verbosity/print_mem_prof", 1.);
	let ml = topi.GetMemoryLayout();
	assert!(ml.starts_with("c MEMORY -- all : ") && ml.contains("m_Watches : ") && ml.contains("m_TmpClssDebug : "), "{}", ml);
	let mut topiC = TTopiC::new(0);
	topiC.SetParam("/verbosity/print_mem_prof", 1.);
	AddClss(&mut topiC, &Php(4, 3));
	assert_eq!(topiC.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(topiC.GetMemoryLayout().contains("m_BC : "));

	// The scores of the decision levels
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &LongSat());
	assert_eq!(topi.Solve(&[1, 2], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert!(topi.m_DecLevel >= 2);
	for dl in 0..=topi.m_DecLevel
	{
		assert!(topi.CalcMinDecLevelScore(dl) <= topi.CalcMaxDecLevelScore(dl) || topi.CalcMinDecLevelScore(dl) == f64::MAX);
	}
	assert!(topi.IsVisitedConsistent());
}

#[test]
fn verbosity_axe()
{
	let mut topi = TTopi::new(0);
	topi.SetParam("/verbosity/level", 1.);
	AddClss(&mut topi, &Php(4, 3));
	assert!(!topi.m_AxePrinted);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(topi.m_AxePrinted);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	let mut topi = TTopiC::new(0);
	topi.m_AxePrinted = false;
	topi.PrintAxe();
	assert!(topi.m_AxePrinted);
}

// Restart after every conflict
fn RestartAlways<Ind: TUInd, const Compress: bool>(topi: &mut CTopi<Ind, Compress>)
{
	topi.SetParam("/restarts/numeric/conflict_thr", 1.);
	topi.SetParam("/restarts/numeric/is_local", 0.);
}

#[test]
fn parallel_data_unit_clauses()
{
	let New = |nextUnits: Vec<Vec<TLit>>| -> (TTopi, Rc<RefCell<Vec<(u32, TLit)>>>, Rc<RefCell<usize>>)
	{
		let mut topi = TTopi::new(0);
		RestartAlways(&mut topi);
		// Negative polarities: the selector is not chosen right away
		topi.SetParam("/decision/polarity/init_strategy", 0.);
		let reported: Rc<RefCell<Vec<(u32, TLit)>>> = Rc::new(RefCell::new(vec![]));
		let reportedCb = reported.clone();
		// Every restart receives the next list of units
		let restarts = Rc::new(RefCell::new(0usize));
		let restartsCb = restarts.clone();
		let mut pos = 0;
		topi.SetParallelData(7, Some(Box::new(move |threadId: u32, l: TLit| reportedCb.borrow_mut().push((threadId, l)))), Some(Box::new(move |threadId: u32, isFirst: bool| -> TLit
		{
			assert_eq!(threadId, 7);
			if isFirst
			{
				*restartsCb.borrow_mut() += 1;
				pos = 0;
			}
			let r = *restartsCb.borrow() - 1;
			let res = if r < nextUnits.len() && pos < nextUnits[r].len() { nextUnits[r][pos] } else { 0 };
			pos += 1;
			res
		})));
		(topi, reported, restarts)
	};

	// The pigeon-hole principle with 6 pigeons and 5 holes, where every pigeon may avoid any hole, if the selector variable 31 holds
	let mut clss = Php(6, 5);
	for c in clss.iter_mut().take(6)
	{
		c.push(31);
	}

	// Units, which are consistent with the instance: no units upon the first restart, units upon the second and the third restarts (some of which are already assigned by then)
	let (mut topi, reported, restarts) = New(vec![vec![], vec![-1, -2, -6, -1], vec![-7, -12, 31, -1], vec![31]]);
	AddClss(&mut topi, &clss);
	topi.AddUserClause(&[32]);
	// The selector is the first decision variable and it is assigned false
	topi.FixPolarity(-31, false);
	topi.BoostScore(31, 1e6);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert!(*restarts.borrow() >= 3, "{} {}", restarts.borrow(), topi.m_Stat.m_Conflicts);
	assert!(ModelSatisfies(&topi, &clss));
	assert_eq!(topi.GetValue(31), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topi.GetLitDecLevel(31), 0);
	// The solver reports its own units (and, precisely as the C++ version, also the ones it received)
	assert!(reported.borrow().contains(&(7, 32)) && reported.borrow().contains(&(7, 31)));

	// A unit, which contradicts a globally assigned literal
	let (mut topi, _, restarts) = New(vec![vec![-31], vec![2, 31, 3]]);
	AddClss(&mut topi, &clss);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert_eq!(*restarts.borrow(), 2);

	// A unit, which contradicts an assumption
	let (mut topi, _, restarts) = New(vec![vec![-1, 4]]);
	AddClss(&mut topi, &clss);
	assert_eq!(topi.Solve(&[1, 3], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert_eq!(*restarts.borrow(), 1);

	// Units, which are contradictory given the clauses
	let (mut topi, _, restarts) = New(vec![vec![1, 6]]);
	AddClss(&mut topi, &clss);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert_eq!(*restarts.borrow(), 1);

	// No callbacks
	let mut topi = TTopi::new(0);
	RestartAlways(&mut topi);
	topi.SetParallelData(3, None, None);
	AddClss(&mut topi, &Php(5, 4));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(topi.m_Stat.m_Restarts > 0);
}

#[test]
fn drat_file_problems()
{
	// The only write is the comment at the end of the query
	let mut topi = TTopi::new(0);
	topi.DumpDrat(Box::new(CSharedSink { m_Buf: Default::default(), m_FailAfter: Some(0) }), false, false);
	topi.AddUserClause(&[1, 2]);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert!(topi.IsError());
	assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_DRAT_FILE_PROBLEM);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_DRAT_FILE_PROBLEM);

	// A text DRAT file, which is fine
	let sink = CSharedSink::default();
	let mut topi = TTopi::new(0);
	topi.DumpDrat(Box::new(sink.clone()), false, false);
	AddClss(&mut topi, &Php(3, 2));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(String::from_utf8_lossy(&sink.m_Buf.borrow()).ends_with("c query completed 1\n"));
}

#[test]
fn debug_model()
{
	// Printing the debug model
	let clss = LongSat();
	let mut topi = TTopi::new(0);
	topi.SetParam("/debugging/print_debug_model_invocation", 2.);
	AddClss(&mut topi, &clss);
	// 110 variables to have a line break in the printed model
	for v in 10..110
	{
		topi.AddUserClause(&[v, v + 1]);
	}
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	// Not satisfiable
	let mut topi = TTopi::new(0);
	topi.SetParam("/debugging/print_debug_model_invocation", 1.);
	AddClss(&mut topi, &clss);
	assert_eq!(topi.Solve(&[1, 2, 3], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	let mut topi = TTopi::new(0);
	topi.PrintDebugModel(TToporReturnVal::RET_CONFLICT_OUT);

	// Verifying the debug model
	fn Verify<Ind: TUInd, const Compress: bool>()
	{
		let mut clss = Php(4, 4);
		clss.push(vec![-1, 17]);
		clss.push(vec![17, 18, 19]);
		// The model
		let mut topi = CTopi::<Ind, Compress>::new(0);
		AddClss(&mut topi, &clss);
		assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
		let debugModel: Vec<bool> = topi.GetModel().iter().take(20).map(|v| *v == TToporLitVal::VAL_SATISFIED).collect();
		assert_eq!(debugModel.len(), 20);
		let unit: TLit = (1..=4).find(|v| debugModel[*v as usize]).unwrap();

		// No debug model: nothing to verify
		let mut topi = CTopi::<Ind, Compress>::new(0);
		topi.SetParam("/debugging/verify_debug_model_invocation", 1.);
		AddClss(&mut topi, &clss);
		assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);

		// The debug model is verified at the beginning of the third query, when there are globally assigned variables, deleted clauses and learnt clauses
		let mut topi = CTopi::<Ind, Compress>::new(0);
		topi.SetParam("/debugging/verify_debug_model_invocation", 3.);
		// Don't compress the buffer after the simplification
		topi.SetParam("/deletion/wasted_fraction_thr", 1.);
		AddClss(&mut topi, &clss);
		let mut debugModel = debugModel;
		debugModel.push(false);
		topi.m_DebugModel = debugModel.clone();
		assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
		topi.AddUserClause(&[unit]);
		topi.AddUserClause(&[if debugModel[18] { 18 } else { -18 }]);
		// A new variable in a unit clause is mapped to the negation of an already globally assigned internal literal
		topi.AddUserClause(&[-20]);
		assert!(IsNeg(topi.m_E2ILitMap[20]));
		assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
		assert_eq!(topi.Solve(&[if debugModel[10] { 10 } else { -10 }], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
		assert!(ModelSatisfies(&topi, &clss));
		assert_eq!(topi.GetValue(20), TToporLitVal::VAL_UNSATISFIED);
		assert!(!topi.IsError());
	}
	Verify::<u32, false>();
	Verify::<u64, false>();
	Verify::<u64, true>();

	// The debug model is verified, when the clause buffer contains deleted clauses
	// The pigeon-hole principle with 6 pigeons and 5 holes, where every pigeon may avoid any hole, if the selector variable 31 holds
	let mut clss = Php(6, 5);
	for c in clss.iter_mut().take(6)
	{
		c.push(31);
	}
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &clss);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
	let debugModel: Vec<bool> = topi.GetModel().iter().take(32).map(|v| *v == TToporLitVal::VAL_SATISFIED).collect();
	let mut topi = TTopi::new(0);
	topi.SetParam("/debugging/verify_debug_model_invocation", 2.);
	// The learnt clauses are deleted often, but the buffer is never compressed
	topi.SetParam("/deletion/clause/trigger_init", 3.);
	topi.SetParam("/deletion/wasted_fraction_thr", 1.);
	AddClss(&mut topi, &clss);
	topi.m_DebugModel = debugModel;
	assert_eq!(topi.Solve(&[-31], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(topi.m_BWasted > 0 && topi.m_Stat.m_ActiveLongLearntClss > 0);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
}

#[test]
fn alloc_failures_in_add_user_clause_and_solve()
{
	// Every allocation of an AddUserClause invocation, which introduces new variables beyond the allocated ones, fails in turn
	for (hint, cls) in [(1, vec![3, -200, 5000]), (1, vec![70, 80]), (1, vec![9000]), (0, vec![1, 2, 3]), (100, vec![150, -20000, 3])]
	{
		for withCb in [false, true]
		{
			let mut n = 0;
			loop
			{
				let mut topi = TTopi::new(hint);
				if withCb
				{
					topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
				}
				topi.AddUserClause(&[1, 2]);
				let allocsBefore = alloc_fault::allocs_so_far();
				alloc_fault::fail_after(n);
				let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| topi.AddUserClause(&cls)));
				alloc_fault::never_fail();
				let allocs = alloc_fault::allocs_so_far() - allocsBefore;
				if allocs <= n
				{
					// No allocation failed
					assert!(r.is_ok() && !topi.IsError());
					assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_SAT);
					break;
				}
				if r.is_ok() && topi.IsError()
				{
					assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);
					assert!(topi.GetStatusExplanation().contains("couldn't") || topi.GetStatusExplanation().contains("allocation failed"), "{}", topi.GetStatusExplanation());
					assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_MEM_OUT);
				}
				n += 1;
				assert!(n < 100);
			}
		}
	}

	// Every allocation of a Solve invocation fails in turn
	for assumps in [vec![], vec![40, -50], vec![-1, 2, 0]]
	{
		let mut n = 0;
		loop
		{
			let mut topi = TTopi::new(1);
			AddClss(&mut topi, &Php(4, 3));
			topi.AddUserClause(&[1]);
			topi.AddUserClause(&[13, 14, 15, 16]);
			topi.AddUserClause(&[-13]);
			let allocsBefore = alloc_fault::allocs_so_far();
			alloc_fault::fail_after(n);
			let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| topi.Solve(&assumps, NoTo, u64::MAX)));
			alloc_fault::never_fail();
			if alloc_fault::allocs_so_far() - allocsBefore <= n
			{
				// No allocation failed
				assert!(matches!(r, Ok(TToporReturnVal::RET_UNSAT)) && !topi.IsError());
				break;
			}
			if let Ok(TToporReturnVal::RET_MEM_OUT) = r
			{
				assert!(topi.IsError());
			}
			n += 1;
			assert!(n < 300);
		}
		assert!(n > 0);
	}
}

// Runs a hand-written scenario over the public API on all the solver modes; every answer is verified against the exact oracle of the fuzzing harness
fn RunOps(vars: usize, verbosity: u8, drat: u8, cbLearnt: bool, ops: Vec<TFuzzOp>) -> Vec<TFuzzOutcome>
{
	let s = TFuzzScenario { m_Vars: vars, m_VarsNumHint: 0, m_Drat: drat, m_CbLearnt: cbLearnt, m_CbStopAfter: None, m_Verbosity: verbosity, m_Ops: ops };
	let o0 = RunScenario(&s, 0, None);
	for solverMode in 1..=2
	{
		let o = RunScenario(&s, solverMode, None);
		assert_eq!(o0.len(), o.len());
		for (r0, r) in o0.iter().zip(&o)
		{
			assert_eq!(r0.m_Ret, r.m_Ret);
		}
	}
	o0
}

fn Cls(c: &[TLit]) -> TFuzzOp
{
	TFuzzOp::AddClause(c.to_vec())
}

fn Slv(assumps: &[TLit]) -> TFuzzOp
{
	TFuzzOp::Solve(assumps.to_vec(), u64::MAX)
}

fn Prm(name: &str, val: f64) -> TFuzzOp
{
	TFuzzOp::SetParam(name.to_string(), val)
}

fn Rets(outcomes: &[TFuzzOutcome]) -> Vec<TToporReturnVal>
{
	outcomes.iter().map(|o| o.m_Ret).collect()
}

const SAT: TToporReturnVal = TToporReturnVal::RET_SAT;
const UNSAT: TToporReturnVal = TToporReturnVal::RET_UNSAT;

#[test]
fn assumptions_handling()
{
	for allowReorder in [1., 0.]
	{
		for verbosity in [0, 2]
		{
			let o = RunOps(8, verbosity, 0, false, vec![
				Prm("/assumptions/allow_reorder", allowReorder),
				Cls(&[1, 2, 3]), Cls(&[-1, -2]), Cls(&[4, 5]), Cls(&[-4, 6, 7]), Cls(&[8]), Cls(&[-2, -3, 7]),
				// No assumptions; a 0-ended list; an empty 0-ended list
				Slv(&[]), Slv(&[1, 4, 0]), Slv(&[0]),
				// Duplicate assumptions; an assumption, which is satisfied globally (in the middle and at the end)
				Slv(&[1, 1, 4, 1, -2]), Slv(&[8, 1, 8, 4]), Slv(&[1, 4, 8]), Slv(&[8]),
				// An assumption, which is falsified globally
				Slv(&[1, -8, 4]), Slv(&[-8]),
				// An assumption in both polarities
				Slv(&[4, 1, -4]), Slv(&[-4, 4]),
				// The assumptions of the previous query are reused: as is, with a different suffix, with the first assumption flipped
				Slv(&[5, 6, -7]), Slv(&[5, 6, -7]), Slv(&[5, 6, 7, 1]), Slv(&[5, -6, 7, 1]), Slv(&[-5, -6, 7, 1]), Slv(&[-5, -4]),
				// An assumption, which is implied false by the previous ones
				Slv(&[1, 2]), Slv(&[1, 4, -6, -7]), Slv(&[2, 3, -7]), Slv(&[-1, -2, -3]), Slv(&[-7, 3, 2, 5]), Slv(&[-1, -3, 2, -7, 3]),
				Slv(&[3, 5, 6]), Slv(&[3, 5, 6, 2, -7]), Slv(&[3, 5, 6, -7, 2]),
				// New variables amongst the assumptions
				Cls(&[-1, -3]), Slv(&[1, 3]), Slv(&[]),
			]);
			assert_eq!(Rets(&o), vec![SAT, SAT, SAT, SAT, SAT, SAT, SAT, UNSAT, UNSAT, UNSAT, UNSAT, SAT, SAT, SAT, SAT, SAT, UNSAT, UNSAT, UNSAT, UNSAT, UNSAT, UNSAT, UNSAT, SAT, UNSAT, UNSAT, UNSAT, SAT]);
			// The cores
			assert_eq!(o[7].m_RequiredAssumps, vec![false, true, false]);
			assert_eq!(o[9].m_RequiredAssumps.iter().filter(|r| **r).count(), 2);
			assert_eq!(o[16].m_RequiredAssumps, vec![true, true]);
			assert_eq!(o[17].m_RequiredAssumps, vec![true, true]);
		}
	}
}

#[test]
fn add_user_clause_cases()
{
	for verbosity in [0, 3]
	{
		for level0 in [0., 1.]
		{
			let o = RunOps(10, verbosity, 1, true, vec![
				Prm("/add_user_clause/guarantee_level_0", level0),
				Prm("/add_user_clause/remove_clss_globally_sat_by_literal_larger_size", 2.),
				Prm("/add_user_clause/remove_clss_globally_sat_by_literal_larger_size_ai", 2.),
				// Tautologies, duplicate literals, a clause after the end-of-clause mark
				Cls(&[1, -1]), Cls(&[2, 3, 2, 3]), Cls(&[4, 5, 0, 6]), Cls(&[2, 3, -2, 5]), Cls(&[6, 7, 8, 9]), Cls(&[-6, -7, 8]),
				Slv(&[]),
				// Unit clauses: unassigned, satisfied and falsified at a non-zero level
				Slv(&[-2, 4, 6, -7]), Cls(&[10]), Cls(&[4]), Cls(&[-6]), Cls(&[7]),
				Slv(&[]),
				// Literals, which are falsified and satisfied globally
				Cls(&[6, -7, 2]), Cls(&[-4, 3, -2, 5]), Cls(&[7, 1, 2, 3]), Cls(&[-10, -4, 6, 8, 9]),
				Slv(&[]),
				// Clauses, which are unit, falsified or delayed implications given the current assignment
				Slv(&[-8, -3, 1, 5]), Cls(&[8, -5, 3]), Slv(&[-3, 1, -5]), Cls(&[3, 5, -1]), Cls(&[-1, 5]), Cls(&[3, -9]), Slv(&[1]), Cls(&[-1, -5, -9, 2]), Cls(&[-2, -9]), Slv(&[]),
				Slv(&[-1, 9]), Cls(&[1, 9]), Cls(&[1, -9]), Slv(&[-2]), Cls(&[1, 2, -9]), Cls(&[2, 1]),
				Slv(&[]),
				// A contradictory clause, given the global assignment; an empty clause
				Cls(&[-4, 6]),
				Slv(&[]), Slv(&[1]),
				Cls(&[]),
				Slv(&[]),
			]);
			assert_eq!(*Rets(&o).last().unwrap(), UNSAT);
			assert_eq!(Rets(&o)[..5], [SAT, SAT, SAT, SAT, SAT]);
		}
	}

	// A unit clause with a new variable, while there are globally assigned variables: the new variable is mapped to the globally satisfied literal
	let mut topi = TTopi::new(0);
	topi.AddUserClause(&[1]);
	topi.AddUserClause(&[-2]);
	topi.AddUserClause(&[3, 0]);
	assert_eq!(topi.m_LastExistingVar, 1);
	assert_eq!(topi.m_E2ILitMap[2], Negate(topi.m_E2ILitMap[1]));
	assert_eq!(topi.m_E2ILitMap[3], topi.m_E2ILitMap[1]);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), SAT);
	assert!(topi.GetValue(1) == TToporLitVal::VAL_SATISFIED && topi.GetValue(2) == TToporLitVal::VAL_UNSATISFIED && topi.GetValue(3) == TToporLitVal::VAL_SATISFIED);
	// An empty clause; nothing is added afterwards
	topi.AddUserClause(&[0, 1]);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), UNSAT);
	topi.AddUserClause(&[5, 6]);
	assert_eq!(topi.m_LastExistingVar, 1);

	// A clause, whose literals are falsified at the same decision level: both are unassigned after backtracking
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &[vec![-1, 2], vec![3, 4]]);
	assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), SAT);
	assert!(topi.GetLitDecLevel(1) == 1 && topi.GetLitDecLevel(2) == 1);
	topi.AddUserClause(&[-1, -2]);
	assert!(topi.GetValue(1) == TToporLitVal::VAL_UNASSIGNED && topi.GetValue(2) == TToporLitVal::VAL_UNASSIGNED);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), SAT);
	assert_eq!(topi.GetValue(1), TToporLitVal::VAL_UNSATISFIED);

	// A contradiction, which is discovered by the BCP at level 0 at the beginning of Solve
	for withCb in [false, true]
	{
		let mut topi = TTopi::new(0);
		if withCb
		{
			topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
		}
		AddClss(&mut topi, &[vec![-1, 2], vec![-1, -2], vec![1]]);
		assert_eq!(topi.Solve(&[], NoTo, u64::MAX), UNSAT);
		assert_eq!(topi.m_Status.get(), TToporStatus::STATUS_CONTRADICTORY);
		assert_eq!(topi.Solve(&[], NoTo, u64::MAX), UNSAT);
		assert!(!topi.IsAssumptionRequired(0));
	}
}

#[test]
fn score_boost_and_after_init_params()
{
	// The strategies of boosting the scores of the variables of the user clauses
	for strat in 0..=4
	{
		let o = RunOps(9, 0, 0, false, vec![
			Prm("/decision/init_clss_boost/strat", strat as f64),
			Prm("/decision/init_clss_boost/strat_after_init", (4 - strat) as f64),
			Prm("/decision/init_clss_boost/mult_delta", 3.),
			Cls(&[1, 2, 3]), Cls(&[-1, -2, -3]), Cls(&[4, 5, 6]), Cls(&[-4, -5, -6]), Cls(&[7, 8, 9]), Cls(&[-7, -8, -9]), Cls(&[1, 4, 7]), Cls(&[-3, -6, -9]),
			Slv(&[]),
			Cls(&[1, 5, 9]), Cls(&[-1, -5, -9]), Cls(&[3, 5]), Cls(&[2, 7, 4, 6]),
			Slv(&[]),
		]);
		assert_eq!(Rets(&o), vec![SAT, SAT]);
	}

	// The parameters, which change after the initial query; forcing the latest solution
	let o = RunOps(9, 0, 0, false, vec![
		Prm("/decision/vsids/var_activity_use_maple_level_breaker", 0.),
		Prm("/decision/vsids/var_activity_use_maple_level_breaker_ai", 1.),
		Prm("/add_user_clause/remove_clss_globally_sat_by_literal_larger_size", 5.),
		Prm("/add_user_clause/remove_clss_globally_sat_by_literal_larger_size_ai", 3.),
		Prm("/decision/vsids/init_order_ai", 1.),
		Prm("/phase/force_solution", 1.),
		Prm("/__ai/conflicts/minimize_clauses", 0.),
		Cls(&[1, 2, 3]), Cls(&[-1, -2, -3]), Cls(&[4, 5, 6]), Cls(&[-4, -5, -6]), Cls(&[7, 8, 9]), Cls(&[-7, -8, -9]), Cls(&[1, 4, 7]), Cls(&[-3, -6, -9]),
		Slv(&[]),
		Cls(&[1, 5, 9]), Cls(&[-1, -5, -9]), Cls(&[3, 5]), Cls(&[2, 7, 4, 6]),
		Slv(&[]), Slv(&[-1, -5]),
		TFuzzOp::Stats,
	]);
	assert_eq!(Rets(&o), vec![SAT, SAT, SAT]);

	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &Php(5, 4));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), UNSAT);
	assert!(topi.GetConflictsNumber() > 0 && topi.GetBacktracks() > 0 && topi.GetPropagations() > 0 && topi.GetActiveClss() > 0);
	let _ = (topi.GetActiveLongLearntClss(), topi.GetAssumpReuseBacktrackLevelsSaved());
	assert!(topi.GetStatStrShort(true).contains("c ToporStt "));
}

#[test]
fn custom_backtracking()
{
	// Backtracking to the decision level with the best score instead of chronological backtracking
	for strat in [1., 2.]
	{
		for solverMode in 0..3
		{
			let mut topi = NewFuzzSolver(solverMode, 0);
			topi.FSetParam("/backtracking/custom_bt_strat_init", strat);
			topi.FSetParam("/backtracking/chrono_bt_if_higher_init", 0.);
			topi.FSetParam("/backtracking/conflicts_to_postpone_chrono", 0.);
			for c in Php(7, 6)
			{
				topi.FAddClause(&c);
			}
			assert_eq!(topi.FSolve(&[], NoTo, u64::MAX), UNSAT);
			assert!(!topi.FIsError());
		}
	}
}

#[test]
fn is_assumption_required_with_equivalent_assumptions()
{
	// The external variables 1, 2 and 3 are mapped to the same internal variable (which is assigned globally)
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &[vec![1], vec![-2], vec![3], vec![4, 5]]);
	assert_eq!(topi.Solve(&[1, 3, -1, 4], NoTo, u64::MAX), UNSAT);
	let required: Vec<bool> = (0..4).map(|i| topi.IsAssumptionRequired(i)).collect();
	assert_eq!(required, vec![false, false, true, false]);
	assert_eq!(topi.Solve(&[4, -1, -3, 2, -5], NoTo, u64::MAX), UNSAT);
	let required: Vec<bool> = (0..5).map(|i| topi.IsAssumptionRequired(i)).collect();
	assert_eq!(required, vec![false, true, false, false, false]);
	assert_eq!(topi.Solve(&[-2, 3, 1, -4, 5], NoTo, u64::MAX), SAT);
	// A 0-ended list of assumptions
	assert_eq!(topi.Solve(&[4, -4, 0], NoTo, u64::MAX), UNSAT);
	assert!(topi.IsAssumptionRequired(0) && topi.IsAssumptionRequired(1) && !topi.IsAssumptionRequired(2));
	assert!(!topi.IsError());
}

#[test]
fn conflicts_while_assigning_assumptions()
{
	// The conflict analysis backtracks beyond the assumptions, which have already been assigned, while there is no falsified assumption
	let o = RunOps(4, 0, 0, true, vec![Cls(&[-1, -2, 3]), Cls(&[-3, 4]), Cls(&[-3, -4]), Slv(&[1, 2]), Slv(&[2, 1]), Slv(&[2]), Slv(&[-3, 1])]);
	assert_eq!(Rets(&o), vec![UNSAT, UNSAT, SAT, SAT]);
	assert_eq!(o[0].m_RequiredAssumps, vec![true, true]);

	// The same, but an assumption is falsified by the BCP before the conflict is discovered (and it is unassigned, when the conflict analysis backtracks)
	for verbosity in [0, 2]
	{
		let o = RunOps(6, verbosity, 0, true, vec![Cls(&[-1, -2, 3]), Cls(&[-3, 4]), Cls(&[-3, -4]), Cls(&[-2, -5]), Cls(&[5, 6]), Slv(&[1, 2, 5]), Slv(&[6, 1, 2, 5]), Slv(&[2, 5]), Slv(&[1, 5])]);
		assert_eq!(Rets(&o), vec![UNSAT, UNSAT, UNSAT, SAT]);
	}

	// The assumption-related functions, which are invoked only if the solver backtracks beyond the assumptions during the search, on a crafted state
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &LongSat());
	assert_eq!(topi.Solve(&[1, 2], NoTo, u64::MAX), SAT);
	topi.m_Assumps.reserve_exactly(2);
	topi.m_Assumps[0] = topi.E2I(1);
	topi.m_Assumps[1] = topi.E2I(2);
	assert_eq!(topi.FindFirstUnassignedAssumpIndex(2), 2);
	topi.m_DecLevelOfLastAssignedAssumption = topi.m_DecLevel + 1;
	topi.HandleAssumptionsIfBacktrackedBeyondThem();
	assert_eq!(topi.m_DecLevelOfLastAssignedAssumption, topi.m_DecLevel);
	topi.Backtrack(1, false, false);
	assert_eq!(topi.FindFirstUnassignedAssumpIndex(2), 1);
	topi.m_Assumps.reserve_exactly(0);
	topi.m_DecLevelOfLastAssignedAssumption = 0;
	assert_eq!(topi.Solve(&[-1, -2], NoTo, u64::MAX), SAT);
}

#[test]
fn contradiction_in_the_initial_bcp()
{
	// isAssump: the decision variable of the first level is crafted to be an assumption variable (the flag is never set at the beginning of Solve otherwise)
	for isAssump in [false, true]
	{
		let mut topi = TTopi::new(0);
		AddClss(&mut topi, &LongSat());
		assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), SAT);
		assert_eq!(topi.GetLitDecLevel(1), 1);
		// 20 is implied at the first decision level, where it implies both 21 and -21, once propagated
		AddClss(&mut topi, &[vec![-20, 21], vec![-20, -21], vec![-1, 20]]);
		assert_eq!(topi.GetLitDecLevel(20), 1);
		if isAssump
		{
			let l = topi.E2I(1);
			topi.m_AssignmentInfo[GetVar(l) as usize].set_m_IsAssump(true);
			topi.m_AssignmentInfo[GetVar(l) as usize].set_m_IsAssumpNegated(IsNeg(l));
		}
		let ret = topi.Solve(&[], NoTo, u64::MAX);
		assert_eq!(ret, if isAssump { UNSAT } else { SAT });
		if !isAssump
		{
			assert_eq!(topi.GetValue(20), TToporLitVal::VAL_UNSATISFIED);
		}
		assert_eq!(topi.GetValue(1), TToporLitVal::VAL_UNSATISFIED);
		assert_eq!(topi.GetLitDecLevel(1), 0);
	}

	// Every allocation of the Solve invocation fails in turn
	let mut n = 0;
	loop
	{
		let mut topi = TTopi::new(0);
		AddClss(&mut topi, &LongSat());
		assert_eq!(topi.Solve(&[1], NoTo, u64::MAX), SAT);
		AddClss(&mut topi, &[vec![-20, 21, 22], vec![-20, 21, -22], vec![-20, -21, 23], vec![-20, -21, -23], vec![-1, 20]]);
		let allocsBefore = alloc_fault::allocs_so_far();
		alloc_fault::fail_after(n);
		let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| topi.Solve(&[2, 3], NoTo, u64::MAX)));
		alloc_fault::never_fail();
		if alloc_fault::allocs_so_far() - allocsBefore <= n
		{
			assert!(matches!(r, Ok(SAT)));
			break;
		}
		n += 1;
		assert!(n < 300);
	}
}

#[test]
fn dec_level_with_best_score_and_collapsed_levels()
{
	let mut topi = TTopi::new(0);
	topi.SetParam("/backtracking/custom_bt_strat_init", 1.);
	AddClss(&mut topi, &LongSat());
	assert_eq!(topi.Solve(&[1, 4, 7], NoTo, u64::MAX), SAT);
	assert!(topi.m_DecLevel >= 3 && topi.m_CurrCustomBtStrat == 1);
	let best = topi.GetDecLevelWithBestScore(1, 4);
	assert!((1..4).contains(&best));
	assert_eq!(topi.GetDecLevelWithBestScore(2, 3), 2);
	// The third level is crafted to be collapsed
	let lastVar = topi.m_TrailLastVarPerDecLevel[3];
	topi.m_TrailLastVarPerDecLevel[3] = BadUVar;
	let bestWithCollapsed = topi.GetDecLevelWithBestScore(1, 4);
	assert!((1..3).contains(&bestWithCollapsed));
	topi.m_TrailLastVarPerDecLevel[3] = lastVar;
	topi.m_CurrCustomBtStrat = 2;
	let best2 = topi.GetDecLevelWithBestScore(1, 4);
	assert!(best2 <= best);
}

#[test]
fn multipliers_with_the_internal_to_external_map()
{
	let mut topi = TTopi::new(0);
	topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
	topi.SetParam("/multiplier/variables", 1.75);
	assert_eq!(topi.m_I2ELitMap.GetMultiplier(), 1.75);
	AddClss(&mut topi, &Php(4, 3));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), UNSAT);
}

/*
* TopiWL.cc
*/

// A solver with the binary clauses [1, 2] and [6, 7] and the long clause [3, 4, 5], where nothing is assigned; all the per-variable data is allocated
fn WlSolver() -> TTopi
{
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &[vec![1, 2], vec![3, 4, 5], vec![6, 7]]);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), SAT);
	topi.Backtrack(0, false, false);
	assert_eq!(topi.m_AssignedVarsNum, 0);
	topi
}

// The watches and the non-watch of the long clause of WlSolver
fn WlLongCls(topi: &TTopi) -> (TULit, TULit, TULit)
{
	let lits = [topi.E2I(3), topi.E2I(4), topi.E2I(5)];
	let watches: Vec<TULit> = lits.iter().copied().filter(|l| topi.m_Watches[*l as usize].m_LongWatches != 0).collect();
	assert_eq!(watches.len(), 2);
	(watches[0], watches[1], *lits.iter().find(|l| !watches.contains(l)).unwrap())
}

// Assigns l as if it were a decision at a new level, but doesn't propagate it
fn AssignAtNewLevel(topi: &mut TTopi, l: TULit)
{
	topi.NewDecLevel();
	let decLevel = topi.m_DecLevel;
	assert!(!topi.Assign(l, TTopi::BadClsInd, BadULit, decLevel, false, false));
}

fn Panics(f: impl FnOnce()) -> bool
{
	std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

#[test]
fn wl_assert_consistency_on_consistent_states()
{
	for level in [2., 3.]
	{
		// The assertions are on throughout the search
		for solverMode in 0..3
		{
			let mut topi = NewFuzzSolver(solverMode, 0);
			topi.FSetParam("/debugging/assert_consistency", level);
			for c in Php(5, 4)
			{
				topi.FAddClause(&c);
			}
			assert_eq!(topi.FSolve(&[], NoTo, u64::MAX), UNSAT);
			let mut topi = NewFuzzSolver(solverMode, 0);
			topi.FSetParam("/debugging/assert_consistency", level);
			for c in Php(5, 5).iter().chain(&[vec![5, -10], vec![-12]])
			{
				topi.FAddClause(c);
			}
			assert_eq!(topi.FSolve(&[1, -7], NoTo, u64::MAX), SAT);
			assert_eq!(topi.FSolve(&[1, 7, 13], NoTo, u64::MAX), SAT);
			assert_eq!(topi.FSolve(&[-1, -2, -3], NoTo, u64::MAX), SAT);
			assert!(!topi.FIsError());
		}

		// Direct invocations
		let mut topi = WlSolver();
		topi.SetParam("/debugging/assert_consistency", level);
		assert!(topi.WLAssertConsistency(true) && topi.WLAssertConsistency(false) && topi.WLAssertNoMissedImplications());
		let (w0, w1, other) = WlLongCls(&topi);
		// A falsified watch, while the clause is satisfied by a non-watch at a lower level, by the other watch at the same level, or the cached literal
		AssignAtNewLevel(&mut topi, other);
		AssignAtNewLevel(&mut topi, Negate(w0));
		let (l1, l2) = (topi.E2I(1), topi.E2I(2));
		AssignAtNewLevel(&mut topi, l1);
		let decLevel = topi.m_DecLevel;
		topi.Assign(Negate(l2), TTopi::BadClsInd, BadULit, decLevel, false, false);
		assert!(topi.WLAssertConsistency(true) && topi.WLAssertNoMissedImplications());
		topi.Assign(Negate(w1), TTopi::BadClsInd, BadULit, decLevel, false, false);
		assert!(topi.WLAssertConsistency(true) && topi.WLAssertNoMissedImplications());
		topi.Backtrack(0, false, false);
		AssignAtNewLevel(&mut topi, w1);
		let decLevel = topi.m_DecLevel;
		topi.Assign(Negate(w0), TTopi::BadClsInd, BadULit, decLevel, false, false);
		assert!(topi.WLAssertConsistency(true) && topi.WLAssertNoMissedImplications());
	}
}

#[test]
fn wl_assert_consistency_failures()
{
	// Every crafted inconsistency is discovered by both WLAssertNoMissedImplications and the heavy WLAssertConsistency
	let Check = |craft: &dyn Fn(&mut TTopi)|
	{
		let mut topi = WlSolver();
		craft(&mut topi);
		assert!(Panics(|| { topi.WLAssertNoMissedImplications(); }));
		let mut topi = WlSolver();
		craft(&mut topi);
		topi.SetParam("/debugging/assert_consistency", 3.);
		assert!(Panics(|| { topi.WLAssertConsistency(true); }));
	};

	// A binary clause: both literals are falsified
	Check(&|topi: &mut TTopi|
	{
		let (l1, l2) = (topi.E2I(1), topi.E2I(2));
		AssignAtNewLevel(topi, Negate(l1));
		AssignAtNewLevel(topi, Negate(l2));
	});
	// A binary clause: one literal is falsified, the other one is unassigned
	Check(&|topi: &mut TTopi|
	{
		let l1 = topi.E2I(1);
		AssignAtNewLevel(topi, Negate(l1));
	});
	// A binary clause: one literal is falsified, the other one is satisfied at a higher level
	Check(&|topi: &mut TTopi|
	{
		let (l1, l2) = (topi.E2I(1), topi.E2I(2));
		AssignAtNewLevel(topi, Negate(l1));
		AssignAtNewLevel(topi, l2);
	});
	// A long clause: a watch is falsified, the other one is satisfied at a higher level
	Check(&|topi: &mut TTopi|
	{
		let (w0, w1, _) = WlLongCls(topi);
		AssignAtNewLevel(topi, Negate(w0));
		AssignAtNewLevel(topi, w1);
	});
	// A long clause: a watch is falsified, the other one is unassigned
	Check(&|topi: &mut TTopi|
	{
		let (w0, _, _) = WlLongCls(topi);
		AssignAtNewLevel(topi, Negate(w0));
	});
	// A long clause: both watches are falsified
	Check(&|topi: &mut TTopi|
	{
		let (w0, w1, _) = WlLongCls(topi);
		AssignAtNewLevel(topi, Negate(w0));
		AssignAtNewLevel(topi, Negate(w1));
	});
	// A long clause: the cached literal is not in the clause
	Check(&|topi: &mut TTopi|
	{
		let (w0, w1, other) = WlLongCls(topi);
		let l1 = topi.E2I(1);
		let wbInd = topi.m_Watches[w0 as usize].m_WBInd as usize;
		topi.m_W[wbInd] = l1;
		// Otherwise, the state is consistent
		AssignAtNewLevel(topi, other);
		AssignAtNewLevel(topi, Negate(w0));
		AssignAtNewLevel(topi, Negate(w1));
	});
	// A learnt clause before the first learnt clause
	Check(&|topi: &mut TTopi|
	{
		let mut cls = [topi.E2I(-3), topi.E2I(6), topi.E2I(7)];
		AssignAtNewLevel(topi, Negate(cls[0]));
		AssignAtNewLevel(topi, cls[2]);
		AssignAtNewLevel(topi, Negate(cls[1]));
		let clsInd = topi.AddClsToBufferAndWatch(TSpanTULit::from_slice(&mut cls), true, false);
		assert!(topi.ClsGetIsLearnt(clsInd) && topi.m_FirstLearntClsInd == clsInd);
		topi.m_FirstLearntClsInd = u32::MAX;
	});
}

#[test]
fn wl_direct()
{
	let mut topi = WlSolver();
	let (l1, l2, l6) = (topi.E2I(1), topi.E2I(2), topi.E2I(6));
	let (w0, w1, other) = WlLongCls(&topi);
	let clsInd: u32 = unsafe { u32::read(topi.m_W.get_ptr(topi.m_Watches[w0 as usize].m_WBInd as usize).add(1)) };

	// Binary watches
	assert!(topi.WLBinaryWatchExists(l1, l2) && topi.WLBinaryWatchExists(l2, l1));
	assert!(!topi.WLBinaryWatchExists(l1, l6) && !topi.WLBinaryWatchExists(other, l6));
	topi.WLAddBinaryWatch(l1, l6);
	topi.WLAddBinaryWatch(l1, w0);
	assert!(topi.WLBinaryWatchExists(l1, l6));
	topi.WLRemoveBinaryWatch(l1, l2);
	assert!(!topi.WLBinaryWatchExists(l1, l2) && topi.WLBinaryWatchExists(l1, l6) && topi.WLBinaryWatchExists(l1, w0));
	topi.WLRemoveBinaryWatch(l1, w0);
	topi.WLRemoveBinaryWatch(l1, l6);
	assert_eq!(topi.m_Watches[l1 as usize].m_BinaryWatches, 0);

	// Long watches
	assert_eq!(topi.WLGetLongWatchInd(w0, clsInd), 0);
	assert_eq!(topi.WLGetLongWatchInd(w0, clsInd + 1), usize::MAX);
	assert_eq!(topi.WLGetLongWatchInd(l1, clsInd), usize::MAX);
	topi.WLSetCached(w0, clsInd, other);
	assert_eq!(topi.m_W[topi.m_Watches[w0 as usize].m_WBInd as usize], other);
	topi.WLReplaceInd(w1, clsInd, clsInd + 5);
	assert_eq!(topi.WLGetLongWatchInd(w1, clsInd + 5), 0);
	topi.WLReplaceInd(w1, clsInd + 5, clsInd);
	// The watch doesn't exist
	assert!(Panics(|| topi.WLSetCached(l1, clsInd, other)));
	assert!(Panics(|| topi.WLReplaceInd(l1, clsInd, clsInd)));
	assert!(Panics(|| topi.WLSetCached(w0, clsInd + 1, other)));
	assert!(Panics(|| topi.WLReplaceInd(w0, clsInd + 1, clsInd)));

	// The watch buffer cannot be reallocated
	let mut topi = WlSolver();
	let (l1, l6) = (topi.E2I(1), topi.E2I(6));
	let wNext = topi.m_WNext as usize;
	topi.m_W.reserve_exactly(wNext);
	// Fill up the arena of l1
	while topi.m_Watches[l1 as usize].GetUsedEntries() < topi.m_Watches[l1 as usize].m_AllocatedEntries as usize
	{
		topi.WLAddBinaryWatch(l1, l6);
	}
	alloc_fault::fail_after(0);
	topi.WLAddBinaryWatch(l1, l6);
	alloc_fault::never_fail();
	assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);
	let mut topi = WlSolver();
	let (w0, _, other) = WlLongCls(&topi);
	let wNext = topi.m_WNext as usize;
	topi.m_W.reserve_exactly(wNext);
	alloc_fault::fail_after(0);
	topi.WLAddLongWatch(other, w0, 1);
	alloc_fault::never_fail();
	assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);

	// The index is too narrow for the watch buffer: there is no place for a new arena, even after compressing the watches (which does nothing in an unrecoverable state)
	let mut topi = WlSolver();
	let other = WlLongCls(&topi).2;
	topi.m_WNext = u32::MAX - 1;
	topi.SetStatus(TToporStatus::STATUS_GLOBAL_TIMEOUT, "crafted".to_string());
	assert!(topi.WLPrepareArena(other, true, false).is_null());
	assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_INDEX_TOO_NARROW);
	assert_eq!(topi.GetStatusExplanation(), "WLPrepareArena: reached the end of the buffer");

	// The number of the used entries of one literal overflows
	let mut topi = WlSolver();
	let l1 = topi.E2I(1);
	topi.m_Watches[l1 as usize].m_BinaryWatches = u32::MAX;
	topi.m_Watches[l1 as usize].m_AllocatedEntries = 1;
	assert!(topi.WLPrepareArena(l1, true, true).is_null());
	assert_eq!(topi.GetStatusExplanation(), "The actual number of entries for one literal doesn't fit into the buffer");

	// The arena of one literal has reached the maximal size
	let mut topi = WlSolver();
	let l1 = topi.E2I(1);
	topi.m_Watches[l1 as usize].m_AllocatedEntries = TWatchInfo::<u32>::MaxWatchInfoAlloc();
	topi.m_Watches[l1 as usize].m_BinaryWatches = TWatchInfo::<u32>::MaxWatchInfoAlloc();
	assert!(topi.WLPrepareArena(l1, true, false).is_null());
	assert_eq!(topi.GetStatusExplanation(), "The watch list for one literal doesn't fit into the buffer");
}

/*
* TopiDebugPrinting.cc and the containers
*/

#[test]
fn debug_printing_strings()
{
	let mut topi = WlSolver();
	let (w0, w1, other) = WlLongCls(&topi);
	let (l1, l2) = (topi.E2I(1), topi.E2I(2));
	assert_eq!(topi.SVar(GetVar(l1)), GetVar(l1).to_string());
	assert_eq!(topi.SLit(Negate(l1)), format!("-{}", GetVar(l1)));
	AssignAtNewLevel(&mut topi, Negate(l1));
	topi.Assign(l2, TTopi::BadClsInd, Negate(l1), 1, false, false);
	AssignAtNewLevel(&mut topi, Negate(other));
	AssignAtNewLevel(&mut topi, Negate(w0));
	let clsInd: u32 = unsafe { u32::read(topi.m_W.get_ptr(topi.m_Watches[w0 as usize].m_WBInd as usize).add(1)) };
	topi.Assign(w1, clsInd, Negate(w0), 3, false, false);
	assert_eq!(topi.SVar(GetVar(l1)), format!("{}@1", GetVar(l1)));
	assert_eq!(topi.SLit(Negate(l1)), format!("-{}@1[S]", GetVar(l1)));
	assert_eq!(topi.SLit(l1), format!("{}@1[U]", GetVar(l1)));
	let mut vars = [GetVar(l1), GetVar(w0), GetVar(topi.E2I(7))];
	assert_eq!(topi.SVars(TSpanTULit::from_slice(&mut vars)), format!("{}@1 {}@3 {}", vars[0], vars[1], vars[2]));
	assert_eq!(topi.SVars(TSpanTULit::from_slice(&mut vars[..0])), "");
	assert_eq!(topi.SUserLits(&[1, -2, 3]), "1 -2 3");
	assert_eq!(topi.SUserLits(&[]), "");
	let trail = topi.STrail();
	assert!(trail.starts_with("Current trail (reversed):\n\t") && trail.contains(" DL 3 *** ") && trail.contains(" DL 1 *** ") && trail.ends_with("{}; \n"), "{}", trail);
	let e2i = topi.SE2I();
	assert!(e2i.starts_with("E2I:\n\t1 : ") && e2i.contains("\t7 : "), "{}", e2i);
	assert!(topi.P(""));
}

#[test]
fn container_leftovers()
{
	// The table of the bit masks, which is built at compile time: build it at run time too
	let table = crate::topor_table_bit_info::BuildCMasksByBitAtRunTime();
	for bits in 0..64
	{
		for bit in 0..64
		{
			let (a, b) = (&table[bits][bit], &crate::topor_table_bit_info::cMasksByBit[bits][bit]);
			assert!(a.w0ReadMask == b.w0ReadMask && a.w1ReadMask == b.w1ReadMask && a.w0ReadShr == b.w0ReadShr && a.w1ReadShl == b.w1ReadShl);
			assert_eq!(a.W0CleanMask(), !a.w0ReadMask);
			assert_eq!(a.W1CleanMask(), !a.w1ReadMask);
			assert_eq!((a.ElemW0Mask() << a.ElemW0Shl()), a.w0ReadMask);
			assert_eq!(a.ElemW1Mask() >> a.ElemW1Shr(), a.w1ReadMask);
			// The masks cover exactly bits+1 bits
			assert_eq!(a.w0ReadMask.count_ones() + a.w1ReadMask.count_ones(), bits as u32 + 1);
		}
	}
	assert_eq!(crate::topor_table_bit_info::LsbMask(64), u64::MAX);
	assert_eq!(crate::topor_table_bit_info::LsbMask(5), 31);

	// A bit array is used as its underlying array
	let mut ba = CBitArray::new();
	ba.reserve_exactly(3);
	assert_eq!(ba.cap(), 3);
	ba.memset(0);
	ba[1] = 5;
	assert_eq!(ba[1], 5);

	// The limits of the parameter types
	assert_eq!(<u8 as TParamType>::limits_epsilon(), 0);
	assert_eq!(<u16 as TParamType>::limits_epsilon(), 0);
	assert_eq!(<u32 as TParamType>::limits_epsilon(), 0);
	assert!(!<bool as TParamType>::limits_epsilon());
	assert_eq!(<f64 as TParamType>::limits_epsilon(), f64::EPSILON);
}

// Runs f on a fresh solver, prepared by prepare, where every allocation of f fails in turn, until f completes with no failed allocations
// Precisely as in the C++ version, not every allocation is tested by the solver, hence f might panic after a failed allocation
// Returns the return values of f (along with the solver) for the runs with failed allocations, which didn't panic
fn WithEveryAllocFailing<R>(prepare: &dyn Fn() -> TTopi, f: &dyn Fn(&mut TTopi) -> R) -> Vec<(R, TTopi)>
{
	let mut res = vec![];
	let mut n = 0;
	loop
	{
		let mut topi = prepare();
		let allocsBefore = alloc_fault::allocs_so_far();
		alloc_fault::fail_after(n);
		let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut topi)));
		alloc_fault::never_fail();
		if alloc_fault::allocs_so_far() - allocsBefore <= n
		{
			assert!(r.is_ok() && !topi.IsError());
			break;
		}
		if let Ok(r) = r
		{
			res.push((r, topi));
		}
		n += 1;
		assert!(n < 500);
	}
	res
}

#[test]
fn alloc_failures_more()
{
	let MemOuts = |res: &Vec<(TToporReturnVal, TTopi)>| res.iter().filter(|(r, topi)| *r == TToporReturnVal::RET_MEM_OUT && topi.IsError()).count();

	// The very first clause of a solver
	let res = WithEveryAllocFailing(&|| TTopi::new(0), &|topi: &mut TTopi| topi.AddUserClause(&[1, 2, 3]));
	assert!(res.iter().any(|(_, topi)| topi.GetStatusExplanation() == "HandleIncomingUserVar: couldn't push into m_NewExternalVarsAddUserCls"));

	// New variables, which are created by the user or by the assumptions
	// The hint on the number of the variables is such that only the VSIDS heap is to be reallocated
	let res = WithEveryAllocFailing(&|| TTopi::new(1000), &|topi: &mut TTopi|
	{
		for v in 1..500
		{
			topi.CreateInternalLit(-v);
			if topi.IsError()
			{
				break;
			}
		}
	});
	assert!(res.iter().any(|(_, topi)| topi.GetStatusExplanation() == "HandleIncomingUserVar: couldn't insert into m_VsidsHeap"), "{:?}", res.iter().map(|(_, topi)| topi.GetStatusExplanation()).collect::<Vec<_>>());
	let res = WithEveryAllocFailing(&|| { let mut topi = TTopi::new(1); topi.AddUserClause(&[1, 2]); topi }, &|topi: &mut TTopi| topi.Solve(&[-100, 2000], NoTo, u64::MAX));
	assert!(MemOuts(&res) > 0);
	let res = WithEveryAllocFailing(&|| { let mut topi = TTopi::new(1); topi.AddUserClause(&[1, 2]); topi }, &|topi: &mut TTopi| { topi.FixPolarity(-100, false); topi.BoostScore(200, 2.); topi.ClearUserPolarityInfo(300); });
	assert!(res.iter().any(|(_, topi)| topi.IsError()));

	// The clause buffer and the watch buffer are full
	for cls in [vec![1, -2, 3], vec![3, -4], vec![9, 10, 11, 12]]
	{
		let res = WithEveryAllocFailing(&||
		{
			let mut topi = TTopi::new(0);
			AddClss(&mut topi, &[vec![1, 2, 3], vec![3, 4]]);
			let (bNext, wNext) = (topi.m_BNext as usize, topi.m_WNext as usize);
			topi.m_B.reserve_exactly(bNext);
			topi.m_W.reserve_exactly(wNext);
			topi
		}, &|topi: &mut TTopi| topi.AddUserClause(&cls));
		assert!(res.iter().any(|(_, topi)| topi.StatusToRetVal() == TToporReturnVal::RET_MEM_OUT));
	}

	// A conflict, while the assumptions are assigned
	let res = WithEveryAllocFailing(&||
	{
		let mut topi = TTopi::new(0);
		AddClss(&mut topi, &[vec![-1, -2, 3], vec![-3, 5, 7], vec![-3, 6, 7], vec![-5, -6, 4, 7], vec![-5, -6, -4, 7], vec![8, 9]]);
		assert_eq!(topi.Solve(&[8], NoTo, u64::MAX), SAT);
		topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
		// The ALL-UIP scheme tests its allocations
		topi.SetParam("/conflicts/all_uip/mode", 3.);
		topi
	}, &|topi: &mut TTopi| topi.Solve(&[-7, 1, 2], NoTo, u64::MAX));
	assert!(res.iter().any(|(r, topi)| *r == TToporReturnVal::RET_MEM_OUT && topi.GetStatusExplanation() == "clsBeforeAllUipOrEmptyIfAllUipFailed allocation failure"));

	// The watch buffer is full, when the BCP at the beginning of the query has to move a watch to a literal with no watches
	let res = WithEveryAllocFailing(&||
	{
		let mut topi = TTopi::new(0);
		AddClss(&mut topi, &[vec![-1, 2, 3, 4], vec![-1, 5, 6, 7], vec![-1, 8, 9, 10], vec![1]]);
		let wNext = topi.m_WNext as usize;
		topi.m_W.reserve_exactly(wNext);
		topi
	}, &|topi: &mut TTopi| topi.Solve(&[], NoTo, u64::MAX));
	assert!(res.iter().any(|(r, topi)| *r == TToporReturnVal::RET_MEM_OUT && topi.GetStatusExplanation() == "BReallocBeyond: couldn't reserve m_B"), "{:?}", res.iter().map(|(_, topi)| topi.GetStatusExplanation()).collect::<Vec<_>>());

	// A contradiction, which is discovered by the BCP at a non-zero level at the beginning of the query
	let res = WithEveryAllocFailing(&||
	{
		let mut topi = TTopi::new(0);
		AddClss(&mut topi, &LongSat());
		topi.AddUserClause(&[20, 21, 22, 23]);
		assert_eq!(topi.Solve(&[1, 2], NoTo, u64::MAX), SAT);
		topi.Backtrack(2, false, false);
		// 20 is implied at the first level; once propagated, there is a contradiction at the second level
		AddClss(&mut topi, &[vec![-20, -2, 21], vec![-20, -2, -21, 22], vec![-20, -2, -21, -22], vec![-1, 20]]);
		topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
		topi.SetParam("/conflicts/all_uip/mode", 3.);
		topi
	}, &|topi: &mut TTopi| topi.Solve(&[], NoTo, u64::MAX));
	assert!(res.iter().any(|(r, topi)| *r == TToporReturnVal::RET_MEM_OUT && topi.GetStatusExplanation() == "clsBeforeAllUipOrEmptyIfAllUipFailed allocation failure"));
}

// The scenarios, which were found by a search over the seeds of the random scenarios of the fuzzing harness for the ones reaching rarely visited code
// kind 1: chronological backtracking is always on, which makes delayed implications way more frequent
// kind 2: the same, but the scenario is generated differently: few variables, short clauses and many queries with many assumptions
// kind 3: the same as kind 2, but the queries share most of their assumptions and most of them have a tiny conflict threshold
pub fn SearchScenario(seed: u64, kind: u64) -> TFuzzScenario
{
	if kind == 2
	{
		let data = RandomBytes(seed, 400);
		let mut inp = CFuzzInput::new(&data);
		let mut s = TFuzzScenario::default();
		s.m_Vars = 4 + inp.Below(6);
		s.m_CbLearnt = inp.Chance(30);
		if inp.Chance(50)
		{
			s.m_Ops.push(Prm("/mode/value", inp.Below(9) as f64));
		}
		for q in ["init", "n", "s"]
		{
			s.m_Ops.push(Prm(&format!("/backtracking/chrono_bt_if_higher_{}", q), 0.));
			s.m_Ops.push(Prm(&format!("/backtracking/custom_bt_strat_{}", q), ((seed / 4) % 3) as f64));
		}
		s.m_Ops.push(Prm("/backtracking/conflicts_to_postpone_chrono", 0.));
		s.m_Ops.push(Prm("/bcp/best_contradiction_strat", (seed % 4) as f64));
		s.m_Ops.push(Prm("/assumptions/allow_reorder", inp.Below(2) as f64));
		let Lit = |inp: &mut CFuzzInput, vars: usize| -> TLit
		{
			let v = 1 + inp.Below(vars) as TLit;
			if inp.Byte() & 1 == 0 { v } else { -v }
		};
		let opsNum = 8 + inp.Below(50);
		for _ in 0..opsNum
		{
			if inp.Chance(60)
			{
				let len = [1, 2, 2, 2, 3, 3, 3, 4][inp.Below(8)];
				s.m_Ops.push(TFuzzOp::AddClause((0..len).map(|_| Lit(&mut inp, s.m_Vars)).collect()));
			}
			else
			{
				let assumpsNum = inp.Below(1 + s.m_Vars.min(7));
				let assumps: Vec<TLit> = (0..assumpsNum).map(|_| Lit(&mut inp, s.m_Vars)).collect();
				let confThr = if inp.Chance(25) { 1 + inp.Below(3) as u64 } else { u64::MAX };
				s.m_Ops.push(TFuzzOp::Solve(assumps, confThr));
			}
		}
		s.m_Ops.push(TFuzzOp::Solve(vec![], u64::MAX));
		return s;
	}
	if kind == 3
	{
		let data = RandomBytes(seed, 600);
		let mut inp = CFuzzInput::new(&data);
		let mut s = TFuzzScenario::default();
		s.m_Vars = 5 + inp.Below(6);
		if inp.Chance(50)
		{
			s.m_Ops.push(Prm("/mode/value", inp.Below(9) as f64));
		}
		for q in ["init", "n", "s"]
		{
			s.m_Ops.push(Prm(&format!("/backtracking/chrono_bt_if_higher_{}", q), 0.));
			s.m_Ops.push(Prm(&format!("/backtracking/custom_bt_strat_{}", q), ((seed / 4) % 3) as f64));
		}
		s.m_Ops.push(Prm("/backtracking/conflicts_to_postpone_chrono", 0.));
		s.m_Ops.push(Prm("/bcp/best_contradiction_strat", (seed % 4) as f64));
		s.m_Ops.push(Prm("/assumptions/allow_reorder", inp.Below(2) as f64));
		let Lit = |inp: &mut CFuzzInput, vars: usize| -> TLit
		{
			let v = 1 + inp.Below(vars) as TLit;
			if inp.Byte() & 1 == 0 { v } else { -v }
		};
		let Clause = |inp: &mut CFuzzInput, vars: usize| -> TFuzzOp
		{
			let len = [2, 2, 3, 3, 3, 4][inp.Below(6)];
			TFuzzOp::AddClause((0..len).map(|_| Lit(inp, vars)).collect())
		};
		for _ in 0..(s.m_Vars + inp.Below(2 * s.m_Vars))
		{
			let c = Clause(&mut inp, s.m_Vars);
			s.m_Ops.push(c);
		}
		// The queries share most of their assumptions
		let mut assumps: Vec<TLit> = (0..2 + inp.Below(5)).map(|_| Lit(&mut inp, s.m_Vars)).collect();
		for _ in 0..(4 + inp.Below(16))
		{
			match inp.Below(6)
			{
				0 if !assumps.is_empty() =>
				{
					let i = inp.Below(assumps.len());
					assumps.remove(i);
				}
				1 if !assumps.is_empty() =>
				{
					let i = inp.Below(assumps.len());
					assumps[i] = -assumps[i];
				}
				2 => assumps.push(Lit(&mut inp, s.m_Vars)),
				3 if assumps.len() > 1 =>
				{
					let (i, j) = (inp.Below(assumps.len()), inp.Below(assumps.len()));
					assumps.swap(i, j);
				}
				_ => (),
			}
			if inp.Chance(30)
			{
				let c = Clause(&mut inp, s.m_Vars);
				s.m_Ops.push(c);
			}
			let confThr = if inp.Chance(60) { 1 + inp.Below(4) as u64 } else { u64::MAX };
			s.m_Ops.push(TFuzzOp::Solve(assumps.clone(), confThr));
		}
		s.m_Ops.push(TFuzzOp::Solve(vec![], u64::MAX));
		return s;
	}
	let mut s = GenScenario(&RandomBytes(seed, 200 + (seed as usize * 37) % 3000), false);
	if kind >= 1
	{
		let pos = s.m_Ops.iter().position(|op| !matches!(op, TFuzzOp::SetParam(..))).unwrap_or(s.m_Ops.len());
		let mut extra = vec![];
		for q in ["init", "n", "s"]
		{
			extra.push(Prm(&format!("/backtracking/chrono_bt_if_higher_{}", q), 0.));
			extra.push(Prm(&format!("/backtracking/custom_bt_strat_{}", q), ((seed / 4) % 3) as f64));
		}
		extra.push(Prm("/backtracking/conflicts_to_postpone_chrono", 0.));
		extra.push(Prm("/bcp/best_contradiction_strat", (seed % 4) as f64));
		for e in extra.into_iter().rev()
		{
			s.m_Ops.insert(pos, e);
		}
		s.m_Verbosity = 0;
	}
	s
}

fn RunSearchScenario(seed: u64, kind: u64)
{
	let s = SearchScenario(seed, kind);
	for solverMode in 0..3
	{
		RunScenario(&s, solverMode, None);
	}
}

#[test]
fn bcp_rare_paths_by_seed()
{
	// BCP: a delayed implication in a binary clause stops the propagation of the current literal
	RunSearchScenario(129568, 1);
	// BCP: the last one of several contradictions is picked
	RunSearchScenario(140443, 1);
	// BCP: the contradiction with the lowest glue is picked out of several ones
	RunSearchScenario(257093, 1);
	// ProcessDelayedImplication: a watch of a contradicting clause is swapped with a non-watch of a higher decision level; a former contradiction is neither a contradiction nor unit
	RunSearchScenario(103037, 1);
	// ProcessDelayedImplication: the literal hasn't been fully propagated and there is a satisfied literal, which isn't cached
	RunSearchScenario(268676, 1);
	// BCP: the smallest contradiction is not the first one
	RunSearchScenario(385332, 1);
	// ProcessDelayedImplication: the other watch of the parent clause has been changed during this very invocation
	RunSearchScenario(481396, 1);
	// ProcessDelayedImplication: two changed contradictions with the same backtrack level
	RunSearchScenario(337414, 1);
	// BCP: the watches of the propagated literal are modified by a delayed implication in a binary clause
	RunSearchScenario(1193285, 1);
}

#[test]
fn assumption_rare_paths_by_seed()
{
	// The solver backtracks beyond the assumptions with no falsified assumption (a reimplication in BCP): the assumptions are reassigned
	RunSearchScenario(1492, 2);
	// An assumption is flipped during the search
	RunSearchScenario(2082, 2);
	// The instance turns out to be globally contradictory, when the assumptions are reassigned after the solver backtracked beyond them
	RunSearchScenario(30889, 3);
}

/*
* TopiBcp.cc: crafted states
*/

// A solver with the long clauses [3, 4, 5], [8, 9, 10, 11] and [12, 13, 14, 15] and the binary clause [16, 17], where nothing is assigned; all the per-variable data is allocated
fn CraftSolver() -> TTopi
{
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, &[vec![3, 4, 5], vec![8, 9, 10, 11], vec![12, 13, 14, 15], vec![16, 17]]);
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), SAT);
	topi.Backtrack(0, false, false);
	assert_eq!(topi.m_AssignedVarsNum, 0);
	topi
}

fn ExtLit(topi: &TTopi, l: TULit) -> TLit
{
	(1..=17).flat_map(|v: TLit| [v, -v]).find(|e: &TLit| topi.m_E2ILitMap[e.unsigned_abs() as usize] != 0 && topi.E2I(*e) == l).unwrap()
}

// The index of the (only) long clause watched by l
fn LongClsInd(topi: &TTopi, l: TULit) -> u32
{
	assert_eq!(topi.m_Watches[l as usize].m_LongWatches, 1);
	unsafe { u32::read(topi.m_W.get_ptr(topi.m_Watches[l as usize].m_WBInd as usize).add(1)) }
}

fn AssignAt(topi: &mut TTopi, l: TULit, parentClsInd: u32, otherWatch: TULit, decLevel: TUV)
{
	assert!(!topi.Assign(l, parentClsInd, otherWatch, decLevel, false, false));
}

#[derive(Clone, Copy, PartialEq)]
enum TPdiState
{
	// The watches of the negation of the delayed implication include a binary clause, satisfied at a higher level
	Binary,
	// ... a long clause, where the cached literal is satisfied at a higher level and the non-watch is falsified at the new level of the delayed implication
	LongKeepWatch,
	// ... a long clause, where the cached literal is satisfied at a higher level and the non-watch is falsified at a level between the new and the old levels of the delayed implication
	LongSwapWatch,
	// There is a contradiction in a long clause, where the negation of the delayed implication is the first watch
	ContradictionFirstWatch,
	// ... the second watch
	ContradictionSecondWatch,
}

// A state, where the literal a is a decision at level 3 out of 4, while the binary clause (a, o) implies it at level 1; returns the solver, a, o and the other literals of the long clause (-a, c, f)
fn PdiState(state: TPdiState) -> (TTopi, TULit, TULit, TULit, TULit)
{
	let mut topi = CraftSolver();
	let (w0, w1, other) = WlLongCls(&topi);
	let a = Negate(w0);
	let ea = ExtLit(&topi, a);
	topi.AddUserClause(&[ea, 8]);
	if state == TPdiState::Binary
	{
		topi.AddUserClause(&[-ea, 9]);
	}
	let (o, y, z, z2) = (topi.E2I(8), topi.E2I(9), topi.E2I(10), topi.E2I(11));
	let clsInd = LongClsInd(&topi, w0);
	// The cached literal of the watch of -a is the other watch
	let wbInd = topi.m_Watches[w0 as usize].m_WBInd as usize;
	topi.m_W[wbInd] = w1;
	if state == TPdiState::ContradictionSecondWatch || state == TPdiState::ContradictionFirstWatch
	{
		let cls = topi.Cls(clsInd);
		if (cls.get(0) == w0) != (state == TPdiState::ContradictionFirstWatch)
		{
			cls.swap(0, 1);
		}
	}
	let Bad = TTopi::BadClsInd;

	AssignAtNewLevel(&mut topi, Negate(o));
	if state == TPdiState::LongKeepWatch
	{
		AssignAt(&mut topi, Negate(other), Bad, BadULit, 1);
	}
	AssignAtNewLevel(&mut topi, z);
	if state != TPdiState::Binary && state != TPdiState::LongKeepWatch
	{
		AssignAt(&mut topi, Negate(other), Bad, BadULit, 2);
	}
	AssignAtNewLevel(&mut topi, a);
	match state
	{
		TPdiState::Binary => AssignAt(&mut topi, y, Bad, w0, 3),
		TPdiState::LongKeepWatch | TPdiState::LongSwapWatch => AssignAt(&mut topi, w1, clsInd, w0, 3),
		_ =>
		{
			AssignAt(&mut topi, Negate(w1), Bad, BadULit, 3);
			topi.m_Cis.emplace_back(TContradictionInfo::from_parent(clsInd));
		}
	}
	AssignAtNewLevel(&mut topi, z2);
	(topi, a, o, w1, other)
}

#[test]
fn process_delayed_implication_on_crafted_states()
{
	let Bad = TTopi::BadClsInd;

	// The delayed implications are processed
	let (mut topi, a, o, _, _) = PdiState(TPdiState::Binary);
	let y = topi.E2I(9);
	assert!(!topi.ProcessDelayedImplication(a, o, Bad));
	assert!(topi.GetAssignedDecLevel(a) == 1 && topi.GetAssignedDecLevel(y) == 1 && topi.m_DecLevel == 4);
	assert_eq!(topi.m_Stat.m_DelayedImplicationDecLevelsCollapsed, 1);
	assert!(topi.DecLevelIsCollapsed(3) && topi.TrailAssertConsistency());
	let (mut topi, a, o, c, _) = PdiState(TPdiState::LongKeepWatch);
	assert!(!topi.ProcessDelayedImplication(a, o, Bad));
	assert!(topi.GetAssignedDecLevel(a) == 1 && topi.GetAssignedDecLevel(c) == 1 && topi.TrailAssertConsistency());
	assert!(topi.m_Watches[Negate(a) as usize].m_LongWatches == 1 && topi.m_Watches[c as usize].m_LongWatches == 1);
	let (mut topi, a, o, c, f) = PdiState(TPdiState::LongSwapWatch);
	assert!(!topi.ProcessDelayedImplication(a, o, Bad));
	assert!(topi.GetAssignedDecLevel(a) == 1 && topi.GetAssignedDecLevel(c) == 2 && topi.TrailAssertConsistency());
	assert!(topi.m_Watches[Negate(a) as usize].m_LongWatches == 0 && topi.m_Watches[f as usize].m_LongWatches == 1);

	// The solver is in an unrecoverable state already
	let (mut topi, a, o, _, _) = PdiState(TPdiState::Binary);
	topi.SetStatus(TToporStatus::STATUS_GLOBAL_TIMEOUT, "crafted".to_string());
	assert!(!topi.ProcessDelayedImplication(a, o, Bad));
	assert_eq!(topi.GetAssignedDecLevel(a), 3);

	// The solver moves to an unrecoverable state, when the delayed implication is reassigned: the best scores of the decision levels
	// (which are otherwise allocated at the beginning of the query, if required) cannot be allocated
	for state in [TPdiState::Binary, TPdiState::LongKeepWatch, TPdiState::LongSwapWatch]
	{
		let (mut topi, a, o, _, _) = PdiState(state);
		topi.m_CurrCustomBtStrat = 1;
		topi.m_BestScorePerDecLevel.reserve_exactly(0);
		// The vector of the delayed implications is allocated already
		topi.m_Dis.emplace_back(TDelImpl::new(a, o, Bad));
		topi.m_Dis.clear();
		let allocsBefore = alloc_fault::allocs_so_far();
		alloc_fault::fail_after(0);
		let res = topi.ProcessDelayedImplication(a, o, Bad);
		alloc_fault::never_fail();
		assert_eq!(alloc_fault::allocs_so_far() - allocsBefore, 1);
		assert!(!res);
		assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);
		assert_eq!(topi.GetAssignedDecLevel(a), 1);
	}

	// A watch of a contradicting clause is to be moved to a literal with no watches, while the watch buffer is full and cannot be reallocated
	for state in [TPdiState::ContradictionFirstWatch, TPdiState::ContradictionSecondWatch]
	{
		let (mut topi, a, o, _, f) = PdiState(state);
		assert!(topi.m_Watches[f as usize].IsEmpty());
		let wNext = topi.m_WNext as usize;
		topi.m_W.reserve_exactly(wNext);
		topi.m_Dis.emplace_back(TDelImpl::new(a, o, Bad));
		topi.m_Dis.clear();
		let allocsBefore = alloc_fault::allocs_so_far();
		alloc_fault::fail_after(0);
		let res = topi.ProcessDelayedImplication(a, o, Bad);
		alloc_fault::never_fail();
		assert_eq!(alloc_fault::allocs_so_far() - allocsBefore, 1);
		assert!(!res);
		assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);
		assert_eq!(topi.GetStatusExplanation(), "BReallocBeyond: couldn't reserve m_B");
	}
}

#[test]
fn bcp_watch_swap_failure_in_a_contradiction()
{
	// The clause (A, B, C, D) is falsified at the levels 1, 2, 3 and 4, respectively, where A and B are the watches and the falsification of A hasn't been propagated yet
	// BCP moves the watch of A to D and then has to move the watch of B to C, which has no watches, while the watch buffer is full and cannot be reallocated
	for fail in [true, false]
	{
		let mut topi = CraftSolver();
		let lits = [topi.E2I(12), topi.E2I(13), topi.E2I(14), topi.E2I(15)];
		let watches: Vec<TULit> = lits.iter().copied().filter(|l| topi.m_Watches[*l as usize].m_LongWatches != 0).collect();
		let others: Vec<TULit> = lits.iter().copied().filter(|l| !watches.contains(l)).collect();
		assert!(watches.len() == 2 && others.len() == 2);
		let (A, B, C, D) = (watches[0], watches[1], others[0], others[1]);
		// D has watches already
		let eD = ExtLit(&topi, D);
		topi.AddUserClause(&[eD, 16]);
		assert!(topi.m_Watches[C as usize].IsEmpty() && !topi.m_Watches[D as usize].IsEmpty());
		for l in [A, B, C, D]
		{
			AssignAtNewLevel(&mut topi, Negate(l));
		}
		topi.ToPropagatePushBack(Negate(A));
		let wNext = topi.m_WNext as usize;
		topi.m_W.reserve_exactly(wNext);
		if fail
		{
			alloc_fault::fail_after(0);
		}
		let ci = topi.BCP();
		alloc_fault::never_fail();
		if fail
		{
			assert!(!ci.IsContradiction());
			assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);
		}
		else
		{
			// The contradiction turns out to be a delayed implication: D is implied at level 3
			assert!(!ci.IsContradiction() && !topi.IsError());
			assert!(topi.m_DecLevel == 3 && topi.IsSatisfied(D) && topi.GetAssignedDecLevel(D) == 3);
			assert!(topi.m_Watches[C as usize].m_LongWatches == 1 && topi.m_Watches[D as usize].m_LongWatches == 1);
		}
	}
}

#[test]
fn process_delayed_implication_modifies_the_watch_of_the_propagated_literal()
{
	// A contradicting clause, whose watch is moved, while the watch is either the negation of the currently propagated literal or is moved to it
	for state in [TPdiState::ContradictionFirstWatch, TPdiState::ContradictionSecondWatch]
	{
		for isOldWatchPropagated in [true, false]
		{
			let (mut topi, a, o, g, f) = PdiState(state);
			topi.m_CurrentlyPropagatedLit = if isOldWatchPropagated { a } else { Negate(f) };
			let stopPropagating = topi.ProcessDelayedImplication(a, o, TTopi::BadClsInd);
			assert!(topi.m_CurrPropWatchModifiedDuringProcessDelayedImplication);
			// The contradicting clause (-a, g, f) is not a contradiction anymore: -a is falsified at level 1, f is falsified at level 2, while g is implied at level 2 (after backtracking to it)
			assert!(topi.m_Cis.empty() && !topi.IsError());
			assert!(topi.m_DecLevel == 2 && topi.GetAssignedDecLevel(a) == 1 && topi.IsSatisfied(g) && topi.GetAssignedDecLevel(g) == 2);
			assert!(topi.m_Watches[Negate(a) as usize].m_LongWatches == 0 && topi.m_Watches[f as usize].m_LongWatches == 1 && topi.m_Watches[g as usize].m_LongWatches == 1);
			// The propagation stops iff the currently propagated literal was assigned at a level higher than the one the solver backtracked to (a was assigned at level 3)
			assert_eq!(stopPropagating, isOldWatchPropagated);
			topi.m_CurrentlyPropagatedLit = BadULit;
			topi.CleanVisited();
			topi.ToPropagateClear();
		}
	}
}

#[test]
fn bcp_restarts_the_watches_after_a_delayed_implication()
{
	let mut topi = CraftSolver();
	// F = (-p, s, x)
	let (w0, w1, other) = WlLongCls(&topi);
	let (p, s, x, t) = (Negate(w0), w1, other, topi.E2I(8));
	let (z, z2) = (topi.E2I(16), topi.E2I(17));
	let _ = z;
	// H = (-s, -p, t), where -s and -p are the watches and -p is the cached literal of the watch of -s
	let (es, ep) = (ExtLit(&topi, s), ExtLit(&topi, p));
	topi.AddUserClause(&[-es, -ep, 8]);
	let clsIndH = LongClsInd(&topi, Negate(s));
	{
		let cls = topi.Cls(clsIndH);
		let tIt = (0..3).find(|i| cls.get(*i) == t).unwrap();
		if tIt < 2
		{
			let pIt = (0..3).find(|i| cls.get(*i) == Negate(p)).unwrap();
			assert_eq!(pIt, 2);
			topi.SwapWatch(clsIndH, tIt == 1, 2);
		}
	}
	assert_eq!(topi.m_Watches[w0 as usize].m_LongWatches, 2);
	let wbInd = topi.m_Watches[Negate(s) as usize].m_WBInd as usize;
	topi.m_W[wbInd] = Negate(p);

	// p is a decision at level 1, which hasn't been propagated yet; x is falsified at level 1; t and s are the decisions of the levels 2 and 3
	let Bad = TTopi::BadClsInd;
	AssignAtNewLevel(&mut topi, p);
	AssignAt(&mut topi, Negate(x), Bad, BadULit, 1);
	AssignAtNewLevel(&mut topi, t);
	AssignAtNewLevel(&mut topi, s);
	AssignAtNewLevel(&mut topi, z2);
	// As if s was removed from the literals to propagate by a backtracking of the current BCP
	topi.MarkVisited(s);
	topi.ToPropagatePushBack(p);

	let ci = topi.BCP();
	// s is a delayed implication of F at level 1; hence, t is a delayed implication of H at level 1, where the watch of -p in H is moved to t
	assert!(!ci.IsContradiction() && !topi.IsError());
	assert!(topi.m_DecLevel == 4 && topi.GetAssignedDecLevel(s) == 1 && topi.GetAssignedDecLevel(t) == 1);
	assert!(topi.DecLevelIsCollapsed(2) && topi.DecLevelIsCollapsed(3));
	assert_eq!(topi.m_Watches[w0 as usize].m_LongWatches, 1);
	assert_eq!(topi.m_Stat.m_DelayedImplicationDecLevelsCollapsed, 2);
	assert!(topi.TrailAssertConsistency() && topi.WLAssertNoMissedImplications());
}

// The index of the long clause, which consists of the given literals
fn ClsIndOf(topi: &TTopi, lits: &[TULit]) -> u32
{
	for &l in lits
	{
		let wi = topi.m_Watches[l as usize];
		for i in 0..wi.m_LongWatches as usize
		{
			let clsInd = unsafe { u32::read(topi.m_W.get_ptr(wi.m_WBInd as usize + i * 2 + 1)) };
			let cls = topi.ConstClsSpan(clsInd, TUV::MAX);
			if cls.size() == lits.len() && lits.iter().all(|x| cls.contains(*x))
			{
				return clsInd;
			}
		}
	}
	panic!("no such clause");
}

// Makes the given two literals the watches of the clause
fn SetWatches(topi: &mut TTopi, clsInd: u32, watches: [TULit; 2])
{
	for watchInd in 0..2
	{
		let cls = topi.Cls(clsInd);
		if !watches.contains(&cls.get(watchInd))
		{
			let newWatchIt = (2..cls.size()).find(|i| watches.contains(&cls.get(*i))).unwrap();
			topi.SwapWatch(clsInd, watchInd == 1, newWatchIt);
		}
	}
	let cls = topi.Cls(clsInd);
	assert!(watches.contains(&cls.get(0)) && watches.contains(&cls.get(1)));
}

#[test]
fn process_delayed_implication_two_contradictions()
{
	// a is a decision at level 3 out of 4, while the binary clause (a, o) implies it at level 1; y is implied by a
	// D1 = (-a, g, h) and D2 = (-a, -y, h2, h3) are contradictions at level 3, where h, h2 and h3 are falsified at level 2
	let mut topi = CraftSolver();
	let (w0, g, h) = WlLongCls(&topi);
	let a = Negate(w0);
	let ea = ExtLit(&topi, a);
	AddClss(&mut topi, &[vec![ea, 8], vec![-ea, 9], vec![-ea, -9, 10, 11]]);
	let (o, y, h2, h3, z, z2) = (topi.E2I(8), topi.E2I(9), topi.E2I(10), topi.E2I(11), topi.E2I(16), topi.E2I(17));
	let clsInd1 = ClsIndOf(&topi, &[w0, g, h]);
	let clsInd2 = ClsIndOf(&topi, &[w0, Negate(y), h2, h3]);
	SetWatches(&mut topi, clsInd2, [w0, Negate(y)]);
	// The cached literals of the watches of -a aren't satisfied
	topi.WLSetCached(w0, clsInd1, g);
	topi.WLSetCached(w0, clsInd2, Negate(y));
	topi.WLSetCached(Negate(y), clsInd2, w0);
	let Bad = TTopi::BadClsInd;
	AssignAtNewLevel(&mut topi, Negate(o));
	AssignAtNewLevel(&mut topi, z);
	for l in [h, h2, h3]
	{
		AssignAt(&mut topi, Negate(l), Bad, BadULit, 2);
	}
	AssignAtNewLevel(&mut topi, a);
	AssignAt(&mut topi, y, Bad, w0, 3);
	AssignAt(&mut topi, Negate(g), Bad, BadULit, 3);
	AssignAtNewLevel(&mut topi, z2);
	topi.m_Cis.emplace_back(TContradictionInfo::from_parent(clsInd1));
	topi.m_Cis.emplace_back(TContradictionInfo::from_parent(clsInd2));
	topi.m_CurrentlyPropagatedLit = Negate(o);

	let stopPropagating = topi.ProcessDelayedImplication(a, o, Bad);
	// a and y are implied at level 1; the solver backtracks to level 2, where D1 implies g and D2 is still a contradiction (watched by h2 and h3)
	assert!(!stopPropagating && !topi.IsError());
	assert!(topi.m_DecLevel == 2 && topi.GetAssignedDecLevel(a) == 1 && topi.GetAssignedDecLevel(y) == 1 && topi.IsSatisfied(g) && topi.GetAssignedDecLevel(g) == 2);
	assert_eq!(topi.m_Cis.size(), 1);
	assert!(topi.m_Cis[0].m_ParentClsInd == clsInd2 && topi.m_Cis[0].IsContradiction());
	assert!(topi.m_Watches[h2 as usize].m_LongWatches == 1 && topi.m_Watches[h3 as usize].m_LongWatches == 1 && topi.m_Watches[w0 as usize].m_LongWatches == 0);
	topi.m_Cis.clear();
	topi.m_CurrentlyPropagatedLit = BadULit;
}

#[test]
fn process_delayed_implication_watch_swap_failure_for_a_visited_literal()
{
	// The state of bcp_restarts_the_watches_after_a_delayed_implication, but t has no watches, while the watch buffer is full and cannot be reallocated
	for fail in [true, false]
	{
		let mut topi = CraftSolver();
		let (w0, w1, other) = WlLongCls(&topi);
		let (p, s, x) = (Negate(w0), w1, other);
		let lits = [topi.E2I(8), topi.E2I(9), topi.E2I(10), topi.E2I(11)];
		let t = *lits.iter().find(|l| topi.m_Watches[**l as usize].IsEmpty()).unwrap();
		let et = ExtLit(&topi, t);
		let (es, ep) = (ExtLit(&topi, s), ExtLit(&topi, p));
		topi.AddUserClause(&[-es, -ep, et]);
		let clsIndF = ClsIndOf(&topi, &[w0, w1, other]);
		let clsIndH = ClsIndOf(&topi, &[Negate(s), Negate(p), t]);
		SetWatches(&mut topi, clsIndH, [Negate(s), Negate(p)]);
		topi.WLSetCached(Negate(s), clsIndH, Negate(p));
		// The arena of t has been deallocated
		topi.m_Watches[t as usize] = TWatchInfo::default();
		let Bad = TTopi::BadClsInd;
		AssignAtNewLevel(&mut topi, p);
		AssignAt(&mut topi, Negate(x), Bad, BadULit, 1);
		AssignAtNewLevel(&mut topi, t);
		AssignAtNewLevel(&mut topi, s);
		let z2 = topi.E2I(17);
		AssignAtNewLevel(&mut topi, z2);
		topi.MarkVisited(s);
		topi.m_CurrentlyPropagatedLit = p;
		let wNext = topi.m_WNext as usize;
		topi.m_W.reserve_exactly(wNext);
		topi.m_Dis.emplace_back(TDelImpl::new(s, Negate(p), clsIndF));
		topi.m_Dis.clear();
		if fail
		{
			alloc_fault::fail_after(0);
		}
		let stopPropagating = topi.ProcessDelayedImplication(s, Negate(p), clsIndF);
		alloc_fault::never_fail();
		assert!(!stopPropagating && topi.m_CurrPropWatchModifiedDuringProcessDelayedImplication);
		assert_eq!(topi.GetAssignedDecLevel(s), 1);
		if fail
		{
			assert_eq!(topi.StatusToRetVal(), TToporReturnVal::RET_MEM_OUT);
		}
		else
		{
			assert!(!topi.IsError() && topi.GetAssignedDecLevel(t) == 1 && topi.m_Watches[t as usize].m_LongWatches == 1);
		}
		topi.m_CurrentlyPropagatedLit = BadULit;
		topi.CleanVisited();
		topi.ToPropagateClear();
	}
}

/*
* TopiDebugPrinting.cc: the debug model doesn't hold (the process exits)
*/

// The child of verify_debug_model_mismatches: the verification of the debug model, selected by the environment, fails, hence the process exits with -1
#[test]
fn verify_debug_model_mismatch_child()
{
	if !IsChild()
	{
		return;
	}
	let kind = std::env::var("TOPOR_COV_CORE_VDM").unwrap();
	let mut topi = TTopi::new(0);
	topi.SetParam("/debugging/verify_debug_model_invocation", 1.);
	let clss: Vec<Vec<TLit>> = match kind.as_str()
	{
		// The unit clause is satisfied on the trail, but falsified by the debug model
		"trail" => vec![vec![-1], vec![2, 3, 4]],
		// A binary clause, falsified by the debug model
		"binary" => vec![vec![1, 2, 3], vec![-3, 4]],
		// A long clause, falsified by the debug model
		_ => vec![vec![1, 3], vec![-1, 2, 4]],
	};
	AddClss(&mut topi, &clss);
	topi.m_DebugModel = vec![false, true, false, true, false];
	println!("CHILD_STARTS_SOLVING");
	topi.Solve(&[], NoTo, u64::MAX);
	// Not reachable
	println!("CHILD_OK");
}

#[test]
fn verify_debug_model_mismatches()
{
	for (kind, msg) in [("trail", "VerifyDebugModel ERROR in verifying the trail at literal {index "), ("binary", "VerifyDebugModel ERROR in verifying a binary clause! Exiting..."), ("long", "VerifyDebugModel ERROR in verifying a long clause! Exiting...")]
	{
		let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
		cmd.args(["--exact", "tests::cov_core::verify_debug_model_mismatch_child", "--nocapture", "--test-threads=1"]).env("TOPOR_COV_CORE_CHILD", "1").env("TOPOR_COV_CORE_VDM", kind);
		let out = cmd.output().unwrap();
		let stdout = String::from_utf8_lossy(&out.stdout);
		// exit(-1)
		assert_eq!(out.status.code(), Some(255), "{}:\n{}\n{}", kind, stdout, String::from_utf8_lossy(&out.stderr));
		assert!(stdout.contains("CHILD_STARTS_SOLVING") && stdout.contains(msg) && !stdout.contains("CHILD_OK") && !stdout.contains("VERIFIED"), "{}:\n{}", kind, stdout);
		assert_eq!(stdout.matches("debugModel = ").count(), match kind { "trail" => 1, "binary" => 2, _ => 3 }, "{}:\n{}", kind, stdout);
	}
}

/*
* Topi.cc: the statements, which follow a failing assertion (reachable only if the debug assertions are compiled out)
*/

#[test]
fn nda_is_visited_consistent()
{
	let mut topi = WlSolver();
	assert!(topi.IsVisitedConsistent());
	// The flag of a variable is set, but the variable isn't in the vector of the visited variables
	let v = GetVar(topi.E2I(3));
	topi.m_AssignmentInfo[v as usize].set_m_Visit(true);
	if cfg!(debug_assertions)
	{
		assert!(Panics(|| { topi.IsVisitedConsistent(); }));
	}
	else
	{
		assert!(!topi.IsVisitedConsistent());
	}
	topi.m_AssignmentInfo[v as usize].set_m_Visit(false);
	assert!(topi.IsVisitedConsistent());
}


/*
* AssignAssumptions: the BCP-backtracked branches, driven directly
*/

// A solver over the given clauses, initialized by a query stopped right before its first decision, whose assumptions are then set manually (as HandleAssumptions does)
fn WithManualAssumps(clss: &[Vec<TLit>], assumps: &[TLit]) -> TTopi
{
	let mut topi = TTopi::new(0);
	AddClss(&mut topi, clss);
	topi.SetCbStopNow(Some(Box::new(|| TStopTopor::VAL_STOP)));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	topi.SetCbStopNow(None);
	topi.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());
	assert_eq!((topi.m_DecLevel, topi.m_AssignedVarsNum), (0, 0));
	topi.m_Assumps.reserve_exactly(assumps.len());
	for (i, a) in assumps.iter().enumerate()
	{
		let l = topi.E2I(*a);
		topi.m_Assumps[i] = l;
		let ai = &mut topi.m_AssignmentInfo[GetVar(l) as usize];
		ai.set_m_IsAssump(true);
		ai.set_m_IsAssumpNegated(IsNeg(l));
	}
	topi
}

// The earliest falsified assumption is assigned and satisfied after an assumption is propagated (that is, BCP has reimplied it): the rest of the assumptions are assigned recursively
#[test]
fn assign_assumptions_earliest_falsified_reimplied()
{
	let mut topi = WithManualAssumps(&[vec![1, 2, 3]], &[1, -2]);
	topi.m_EarliestFalsifiedAssump = topi.E2I(1);
	topi.AssignAssumptions(0);
	assert_eq!(topi.m_Status.get(), TToporStatus::STATUS_UNDECIDED);
	assert_eq!(topi.m_DecLevel, 2);
	assert_eq!(topi.m_DecLevelOfLastAssignedAssumption, 2);
	assert_eq!((topi.GetValue(1), topi.GetValue(-2)), (TToporLitVal::VAL_SATISFIED, TToporLitVal::VAL_SATISFIED));
	assert_eq!((topi.GetLitDecLevel(1), topi.GetLitDecLevel(2)), (1, 2));
}

// Propagating an assumption turns out to falsify a clause, which should have implied a literal at a lower level (a delayed implication):
// BCP backtracks below the level of the assumption and reimplies that literal at the lower level, so the assumption becomes unassigned without being falsified.
// The assumption is then assigned again, and its second propagation falsifies it.
// The state is crafted: the earlier assumption is assigned without propagation, the way the trail of an earlier query may be reused
#[test]
fn assign_assumptions_unassigned_by_bcp_backtrack()
{
	// b = 1, a = 2, c = 3, x = 4: b -> x; a -> c; a & c -> -x
	let clss = vec![vec![4, -1], vec![-2, 3], vec![-2, -3, -4]];
	let mut topi = WithManualAssumps(&clss, &[1, 2]);
	topi.NewDecLevel();
	let b = topi.E2I(1);
	assert!(!topi.Assign(b, TTopi::BadClsInd, BadULit, 1, false, false));
	let delayedBefore = topi.m_Stat.m_BCPBacktracks;
	topi.AssignAssumptions(1);
	assert!(topi.m_Stat.m_BCPBacktracks > delayedBefore);
	// The assumptions are contradictory under the clauses: the second assumption is falsified at the level of the first one
	assert_eq!(topi.m_Status.get(), TToporStatus::STATUS_UNSAT);
	assert_eq!(topi.m_EarliestFalsifiedAssump, topi.E2I(2));
	assert_eq!((topi.GetValue(1), topi.GetLitDecLevel(1)), (TToporLitVal::VAL_SATISFIED, 1));
	assert_eq!((topi.GetValue(2), topi.GetLitDecLevel(2)), (TToporLitVal::VAL_UNSATISFIED, 1));
	assert_eq!((topi.GetValue(4), topi.GetLitDecLevel(4)), (TToporLitVal::VAL_SATISFIED, 1));
	// The verdict is right
	let mut fresh = TTopi::new(0);
	AddClss(&mut fresh, &clss);
	assert_eq!(fresh.Solve(&[1, 2], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(fresh.IsAssumptionRequired(0) && fresh.IsAssumptionRequired(1));
}

// The corner case of a reimplication in BCP in the main loop of Solve: the earliest falsified assumption turns out to be satisfied, while another assumption is falsified; the latter replaces it.
// The trail of an earlier query is reused, where some implications were missed (a crafted state; the trail of an earlier query may be reused that way):
// b = 1, c = 2, a = 3 are the assumptions, decided at levels 1, 2, 3, e = 4 is implied by c at level 2, -w = -5 is assigned at level 0, and nothing is propagated.
// The only unassigned variable d = 6 is decided positively at level 4, and its propagation falsifies [-d, -e, -c]: a delayed implication, so BCP backtracks and implies -d at level 2;
// propagating -d falsifies [d, -a], so BCP backtracks to level 2 and implies -a, which falsifies the assumption a;
// propagating -a falsifies [a, w], so BCP backtracks to level 1 and implies a at level 0, so a is satisfied again;
// propagating a implies -c at level 1 by [-c, -a, -b]: the assumption c is falsified at a higher level than a, so Assign doesn't record it as the earliest falsified assumption
#[test]
fn solve_reimplied_earliest_falsified_assumption_replaced()
{
	let clss = vec![vec![-6, -4, -2], vec![6, -3], vec![3, 5], vec![-3, -2, -1], vec![-2, 4]];
	let mut topi = TTopi::new(0);
	// The watches are the first two literals of each clause: [-a, -c, -b] must be visited when a is propagated
	topi.SetParam("/bcp/wl_choice", 2.);
	AddClss(&mut topi, &clss);
	topi.SetCbStopNow(Some(Box::new(|| TStopTopor::VAL_STOP)));
	assert_eq!(topi.Solve(&[], NoTo, u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	topi.SetCbStopNow(None);
	topi.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());
	assert_eq!((topi.m_DecLevel, topi.m_AssignedVarsNum), (0, 0));
	topi.FixPolarity(6, false);

	let L = |topi: &TTopi, l: TLit| topi.E2I(l);
	let bad = TTopi::BadClsInd;
	assert!(!topi.Assign(L(&topi, -5), bad, BadULit, 0, false, false));
	topi.NewDecLevel();
	assert!(!topi.Assign(L(&topi, 1), bad, BadULit, 1, false, false));
	topi.NewDecLevel();
	assert!(!topi.Assign(L(&topi, 2), bad, BadULit, 2, false, false));
	assert!(!topi.Assign(L(&topi, 4), bad, L(&topi, -2), 2, false, false));
	topi.NewDecLevel();
	assert!(!topi.Assign(L(&topi, 3), bad, BadULit, 3, false, false));

	// Simplification (triggered by the new assignment at level 0) assumes a fully propagated trail, which the crafted one isn't
	topi.m_ImplicationsTillNextSimplify = i64::MAX;
	assert_eq!(topi.Solve(&[1, 2, 3], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
	// The replacement is the latest earliest falsified assumption, which the unsatisfiable core is built from (m_EarliestFalsifiedAssump itself is cleared at the end of Solve)
	assert_eq!(topi.m_LatestEarliestFalsifiedAssump, L(&topi, 2));
	// The verdict is right
	let mut fresh = TTopi::new(0);
	AddClss(&mut fresh, &clss);
	assert_eq!(fresh.Solve(&[1, 2, 3], NoTo, u64::MAX), TToporReturnVal::RET_UNSAT);
}

// The earliest falsified assumption is unassigned at the end of an iteration of the main loop of Solve: an internal error, as in the C++
// This happens, when a stop requested by the new-learnt-clause callback interrupts conflict analysis after a lower-level backtracking has unassigned the earliest falsified assumption
// (the scenario was found by a search; it is pseudo-random). The preceding debug_assert! (assert in the C++) fails then, so the test runs only with the debug assertions off
#[test]
fn nda_solve_earliest_falsified_assumption_unassigned()
{
	if cfg!(debug_assertions)
	{
		return;
	}
	let data = RandomBytes(528, 4000);
	let mut inp = CFuzzInput::new(&data);
	let vars = 5 + inp.Below(25);
	let stopAfter = inp.Below(30) as u32;
	let mut t = TTopi::new(0);
	for q in ["init", "n", "s"]
	{
		t.SetParam(&format!("/backtracking/chrono_bt_if_higher_{}", q), [0., 0., 1., 100.][inp.Below(4)]);
		t.SetParam(&format!("/backtracking/custom_bt_strat_{}", q), inp.Below(3) as f64);
	}
	if inp.Chance(50)
	{
		t.SetParam("/mode/value", inp.Below(9) as f64);
	}
	let mut n = 0u32;
	t.SetCbNewLearntCls(Some(Box::new(move |_c: &[TLit]| { n += 1; if n > stopAfter { TStopTopor::VAL_STOP } else { TStopTopor::VAL_CONTINUE } })));
	let Lit = |inp: &mut CFuzzInput| { let v = 1 + inp.Below(vars) as TLit; if inp.Byte() & 1 == 0 { v } else { -v } };
	let clss = (vars as f64 * (3.0 + inp.Below(20) as f64 / 10.)) as usize;
	for _ in 0..clss
	{
		let len = [2, 3, 3, 3, 4][inp.Below(5)];
		let c: Vec<TLit> = (0..len).map(|_| Lit(&mut inp)).collect();
		t.AddUserClause(&c);
	}
	for _ in 0..1 + inp.Below(6)
	{
		let assumps: Vec<TLit> = (0..inp.Below(vars.min(10))).map(|_| Lit(&mut inp)).collect();
		let ret = t.Solve(&assumps, NoTo, u64::MAX);
		if ret == TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR
		{
			assert_eq!(t.m_Status.get(), TToporStatus::STATUS_ASSUMPTION_REQUIRED_ERROR);
			assert!(t.IsError());
			return;
		}
		assert!(!t.IsError());
	}
	panic!("The scenario no longer reaches the internal error");
}
