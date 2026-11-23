// Targeted unit tests (see mod.rs)
// The front-end of the library: the numeric conversions (std_string.rs), the public API wrapper (topor.rs) and the IPASIR interface (topor_ipasir.rs)

use crate::std_string::*;
use crate::topor::*;
use crate::topor_ipasir::*;
use std::cell::RefCell;
use std::ffi::{c_int, c_void, CStr, CString};
use std::rc::Rc;
use std::sync::atomic::Ordering;

/*
* std_string.rs
*/

#[test]
fn std_string_stod()
{
	assert_eq!(stod("1.5"), Some(1.5));
	assert_eq!(stod("  \t\n\r-2.25xyz"), Some(-2.25));
	assert_eq!(stod("+3"), Some(3.));
	assert_eq!(stod("5."), Some(5.));
	assert_eq!(stod(".5"), Some(0.5));
	assert_eq!(stod("1e3"), Some(1000.));
	assert_eq!(stod("1E+3"), Some(1000.));
	assert_eq!(stod("25e-1"), Some(2.5));
	// The exponent is ignored, unless it has digits
	assert_eq!(stod("7e"), Some(7.));
	assert_eq!(stod("7e+"), Some(7.));
	assert_eq!(stod("7e-x"), Some(7.));
	assert_eq!(stod("inf"), Some(f64::INFINITY));
	assert_eq!(stod("+INFinity"), Some(f64::INFINITY));
	assert_eq!(stod(" -Inf"), Some(f64::NEG_INFINITY));
	assert!(stod("nan").unwrap().is_nan());
	assert!(stod("-NaN").unwrap().is_nan());
	// std::invalid_argument
	assert_eq!(stod(""), None);
	assert_eq!(stod("   "), None);
	assert_eq!(stod("abc"), None);
	assert_eq!(stod("-"), None);
	assert_eq!(stod("."), None);
	assert_eq!(stod("-.e5"), None);
	assert_eq!(stod("in"), None);
	// std::out_of_range
	assert_eq!(stod("1e999"), None);
	assert_eq!(stod("-1e999"), None);
}

#[test]
fn std_string_integers()
{
	assert_eq!(stoll("0"), Some(0));
	assert_eq!(stoll(" \x0b\x0c+17abc"), Some(17));
	assert_eq!(stoll("-17 5"), Some(-17));
	assert_eq!(stoll("9223372036854775807"), Some(i64::MAX));
	assert_eq!(stoll("9223372036854775808"), None);
	assert_eq!(stoll("-9223372036854775808"), Some(i64::MIN));
	assert_eq!(stoll("-9223372036854775809"), None);
	assert_eq!(stoll(""), None);
	assert_eq!(stoll("-"), None);
	assert_eq!(stoll("+x"), None);
	assert_eq!(stoll("x1"), None);
	// The magnitude doesn't fit into 128 bits: both the multiplication and the addition overflow checks
	assert_eq!(stoll("999999999999999999999999999999999999999999"), None);
	assert_eq!(stoll("340282366920938463463374607431768211456"), None);
	assert_eq!(stoll("340282366920938463463374607431768211455"), None);

	assert_eq!(stoi("2147483647"), Some(i32::MAX));
	assert_eq!(stoi("-2147483648"), Some(i32::MIN));
	assert_eq!(stoi("2147483648"), None);
	assert_eq!(stoi("-2147483649"), None);
	assert_eq!(stoi("1,2"), Some(1));
	assert_eq!(stoi("q"), None);

	assert_eq!(stoull("18446744073709551615"), Some(u64::MAX));
	assert_eq!(stoull("18446744073709551616"), None);
	assert_eq!(stoull("-1"), Some(u64::MAX));
	assert_eq!(stoull("-18446744073709551615"), Some(1));
	assert_eq!(stoull("42\n"), Some(42));
	assert_eq!(stoull("z"), None);

	assert_eq!(stoul("12"), Some(12));
	assert_eq!(stoul("-2"), Some(u64::MAX - 1));
	assert_eq!(stoul(""), None);
}

/*
* topor.rs
*/

// The pigeon-hole principle: pigeons pigeons don't fit into pigeons-1 holes; the variable of pigeon p (0-based) in hole h (0-based) is p * holes + h + 1
fn PigeonHole(pigeons: TLit) -> Vec<Vec<TLit>>
{
	let holes = pigeons - 1;
	let V = |p: TLit, h: TLit| p * holes + h + 1;
	let mut clss: Vec<Vec<TLit>> = vec![];
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

// A writer, whose contents can be inspected after the solver had taken the ownership of it
#[derive(Clone)]
struct CSharedBuf(Rc<RefCell<Vec<u8>>>);

impl std::io::Write for CSharedBuf
{
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize>
	{
		self.0.borrow_mut().extend_from_slice(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()>
	{
		Ok(())
	}
}

fn ToporApi<Ind: TUInd, const Compress: bool>()
{
	// A satisfiable query
	let mut topor = CTopor::<Ind, Compress>::new(4);
	assert!(!topor.IsError());
	assert!(topor.GetParamsDescr().contains("/mode/value"));
	assert_eq!(topor.GetSolveInvs(), 0);
	topor.SetParam("/verbosity/level", 0.);
	assert!(!topor.IsError());
	topor.AddClause(&[1, 2]);
	topor.AddClause(&[-1, 2, 0, 3]);
	topor.AddClause(&[-2, 3, 4]);
	topor.AddClause(&[-2, -3, 4]);
	topor.BoostScore(1, 2.);
	topor.FixPolarity(-1, false);
	topor.ClearUserPolarityInfo(1);
	topor.FixPolarity(4, true);
	assert_eq!(topor.ChangeConfigToGiven(0), "");
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topor.GetSolveInvs(), 1);
	assert_eq!(topor.GetMaxUserVar(), 4);
	assert_eq!(topor.GetMaxInternalVar(), 4);
	assert_eq!(topor.GetLitValue(2), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topor.GetLitValue(-2), TToporLitVal::VAL_UNSATISFIED);
	assert_eq!(topor.GetLitValue(4), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topor.GetLitValue(100), TToporLitVal::VAL_DONT_CARE);
	// TLit::MIN has no positive counterpart; it used to index the literal map with a huge index
	assert_eq!(topor.GetLitValue(TLit::MIN), TToporLitVal::VAL_DONT_CARE);
	assert!(topor.GetLitDecLevel(2) >= 0);
	// Literals beyond the literal map used to be read out of bounds
	assert_eq!(topor.GetLitDecLevel(TLit::MAX), 0);
	assert_eq!(topor.GetLitDecLevel(TLit::MIN), 0);
	let model = topor.GetModel();
	assert_eq!(model.len(), 5);
	assert_eq!(model[2], TToporLitVal::VAL_SATISFIED);
	assert_eq!(model[4], TToporLitVal::VAL_SATISFIED);
	assert!(topor.GetActiveClss() >= 3);
	assert_eq!(topor.GetActiveLongLearntClss(), 0);
	assert_eq!(topor.GetConflictsNumber(), 0);
	let _ = topor.GetBacktracks();
	let _ = topor.GetPropagations();
	let _ = topor.GetAssumpReuseBacktrackLevelsSaved();
	assert!(!topor.GetStatStrShort(true).is_empty());
	let _ = topor.GetStatusExplanation();
	topor.Backtrack(0);
	assert!(!topor.IsError());

	// An unsatisfiable-under-assumptions query
	assert_eq!(topor.Solve(&[2, -4, 1], (f64::MAX, false), u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(topor.IsAssumptionRequired(0));
	assert!(topor.IsAssumptionRequired(1));
	assert!(!topor.IsAssumptionRequired(2));
	assert!(!topor.IsError());

	// Internal literals created up-front
	let mut topor = CTopor::<Ind, Compress>::new(0);
	topor.CreateInternalLit(3);
	topor.CreateInternalLit(-1);
	topor.AddClause(&[1, 3]);
	topor.AddClause(&[-3]);
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topor.GetLitValue(1), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topor.GetLitValue(3), TToporLitVal::VAL_UNSATISFIED);

	// A wrong parameter
	let mut topor = CTopor::<Ind, Compress>::new(0);
	topor.SetParam("/no/such/parameter", 1.);
	assert!(topor.IsError());
	assert!(!topor.GetStatusExplanation().is_empty());
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_PARAM_ERROR);

	// An unsatisfiable instance, which requires conflicts: the DRAT proof, the callbacks and the parallel data
	let learnts = Rc::new(RefCell::new(0u64));
	let stopNowCalls = Rc::new(RefCell::new(0u64));
	let reportedUnits = Rc::new(RefCell::new(Vec::<(u32, i32)>::new()));
	let drat = CSharedBuf(Rc::new(RefCell::new(vec![])));
	let mut topor = CTopor::<Ind, Compress>::new(0);
	topor.DumpDrat(Box::new(drat.clone()), false, true);
	let learntsCb = learnts.clone();
	topor.SetCbNewLearntCls(Some(Box::new(move |_cls: &[TLit]|
	{
		*learntsCb.borrow_mut() += 1;
		TStopTopor::VAL_CONTINUE
	})));
	let stopNowCallsCb = stopNowCalls.clone();
	topor.SetCbStopNow(Some(Box::new(move ||
	{
		*stopNowCallsCb.borrow_mut() += 1;
		TStopTopor::VAL_CONTINUE
	})));
	let reportedUnitsCb = reportedUnits.clone();
	topor.SetParallelData(7, Some(Box::new(move |threadId: u32, l: i32| reportedUnitsCb.borrow_mut().push((threadId, l)))), Some(Box::new(|_threadId: u32, _reinit: bool| 0)));
	for cls in PigeonHole(5)
	{
		topor.AddClause(&cls);
	}
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(!topor.IsError());
	assert!(topor.GetConflictsNumber() > 0);
	assert!(*learnts.borrow() > 0);
	assert!(*stopNowCalls.borrow() > 0);
	assert!(reportedUnits.borrow().iter().all(|(threadId, _)| *threadId == 7));
	assert!(!drat.0.borrow().is_empty());

	// Interrupts: by the stop-now callback, by InterruptNow and through the handle
	let mut topor = CTopor::<Ind, Compress>::new(0);
	for cls in PigeonHole(5)
	{
		topor.AddClause(&cls);
	}
	topor.SetCbStopNow(Some(Box::new(|| TStopTopor::VAL_STOP)));
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	topor.SetCbStopNow(None);
	topor.InterruptNow();
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	let handle = topor.GetInterruptNowHandle();
	assert!(!handle.load(Ordering::Relaxed));
	handle.store(true, Ordering::Relaxed);
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	assert!(!handle.load(Ordering::Relaxed));
	// A conflict threshold
	assert_eq!(topor.Solve(&[], (f64::MAX, true), 1), TToporReturnVal::RET_CONFLICT_OUT);
	assert_eq!(topor.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(!topor.IsError());
}

#[test]
fn topor_api_32()
{
	ToporApi::<u32, false>();
}

#[test]
fn topor_api_64()
{
	ToporApi::<u64, false>();
}

#[test]
fn topor_api_compressed()
{
	ToporApi::<u64, true>();
}

/*
* topor_ipasir.rs
*/

fn IpasirAddCls(solver: *mut c_void, cls: &[c_int])
{
	for l in cls
	{
		unsafe { ipasir_add(solver, *l) };
	}
	unsafe { ipasir_add(solver, 0) };
}

// The terminate callback: state points to a pair of the number of calls so far and the number of calls, starting from which the solver must be stopped
unsafe extern "C" fn IpasirTerminate(state: *mut c_void) -> c_int
{
	let state = &mut *(state as *mut (u64, u64));
	state.0 += 1;
	(state.0 >= state.1) as c_int
}

#[test]
fn ipasir_c_api()
{
	assert_eq!(unsafe { CStr::from_ptr(ipasir_signature()) }.to_str().unwrap(), "IntelSatSolver");

	// Releasing a null solver is a no-op
	unsafe { ipasir_release(std::ptr::null_mut()) };

	let solver = ipasir_init();
	assert!(!solver.is_null());
	unsafe
	{
		let paramName = CString::new("/verbosity/level").unwrap();
		let paramVal = CString::new("0").unwrap();
		ipasir_set_parameter(solver, paramName.as_ptr(), paramVal.as_ptr());

		// (1 | 2) & (-1 | 2) & (-2 | 3)
		IpasirAddCls(solver, &[1, 2]);
		// Before the first query: the variables are unassigned
		assert_eq!(ipasir_val(solver, 1), 0);
		IpasirAddCls(solver, &[-1, 2]);
		IpasirAddCls(solver, &[-2, 3]);
		assert_eq!(ipasir_solve(solver), 10);
		assert_eq!(ipasir_val(solver, 2), 2);
		assert_eq!(ipasir_val(solver, -2), 2);
		assert_eq!(ipasir_val(solver, 3), 3);
		assert_eq!(ipasir_val(solver, -3), 3);
		assert_eq!(ipasir_val(solver, 1).abs(), 1);
		// A variable, which is unknown to the solver, is don't care
		assert_eq!(ipasir_val(solver, 50), 50);

		// Assumptions (duplicates are ignored): -3 contradicts the clauses
		ipasir_assume(solver, 1);
		ipasir_assume(solver, -3);
		ipasir_assume(solver, 1);
		ipasir_assume(solver, -3);
		assert_eq!(ipasir_solve(solver), 20);
		assert_eq!(ipasir_failed(solver, -3), 1);
		// A literal, which wasn't assumed, is treated as the first assumption
		assert_eq!(ipasir_failed(solver, 17), ipasir_failed(solver, 1));

		// The assumptions hold for one query only
		assert_eq!(ipasir_solve(solver), 10);
		ipasir_assume(solver, 3);
		assert_eq!(ipasir_solve(solver), 10);
		assert_eq!(ipasir_val(solver, 3), 3);

		ipasir_release(solver);
	}
}

#[test]
fn ipasir_terminate()
{
	let solver = ipasir_init();
	let mut state: (u64, u64) = (0, 1);
	let statePtr = &mut state as *mut (u64, u64) as *mut c_void;
	unsafe
	{
		for cls in PigeonHole(5)
		{
			IpasirAddCls(solver, &cls);
		}

		// Stop at once
		ipasir_set_terminate(solver, statePtr, Some(IpasirTerminate));
		assert_eq!(ipasir_solve(solver), 0);
		assert_eq!((*(statePtr as *mut (u64, u64))).0, 1);

		// Removing the callback: the solver stops consulting it
		ipasir_set_terminate(solver, statePtr, None);
		(*(statePtr as *mut (u64, u64))).0 = 0;
		assert_eq!(ipasir_solve(solver), 20);
		assert_eq!((*(statePtr as *mut (u64, u64))).0, 0);

		ipasir_release(solver);
	}

	// Never stop
	let solver = ipasir_init();
	let mut state: (u64, u64) = (0, u64::MAX);
	let statePtr = &mut state as *mut (u64, u64) as *mut c_void;
	unsafe
	{
		for cls in PigeonHole(5)
		{
			IpasirAddCls(solver, &cls);
		}
		ipasir_set_terminate(solver, statePtr, Some(IpasirTerminate));
		assert_eq!(ipasir_solve(solver), 20);
		assert!((*(statePtr as *mut (u64, u64))).0 > 1);
		ipasir_release(solver);
	}
}

#[test]
fn ipasir_wrapper()
{
	let mut tiw = CToporIpasirWrapper::default();
	tiw.SetParam("/verbosity/level", "0");
	assert!(!tiw.Topor().IsError());
	tiw.Add(1);
	tiw.Add(-2);
	tiw.Add(0);
	tiw.Add(2);
	tiw.Add(0);
	assert_eq!(tiw.Solve(), 10);
	assert_eq!(tiw.Val(1), 1);
	assert_eq!(tiw.Val(-1), 1);
	assert_eq!(tiw.Val(2), 2);
	tiw.Assume(-1);
	assert_eq!(tiw.Solve(), 20);
	assert_eq!(tiw.Failed(-1), 1);
	assert_eq!(tiw.Topor().GetSolveInvs(), 2);
	// A wrong parameter: any query fails from now on
	tiw.SetParam("/no/such/parameter", "1");
	assert!(tiw.Topor().IsError());
	assert_eq!(tiw.Solve(), 0);

	// A parameter value, which isn't a number (the C++ version is terminated by the uncaught exception of std::stod)
	let res = std::panic::catch_unwind(||
	{
		let mut tiw = CToporIpasirWrapper::new();
		tiw.SetParam("/verbosity/level", "abc");
	});
	assert!(res.is_err());
}
