// Unit tests: TopiVarScores, TopiParams, TopiStatistics, TimeMeasure, BasicMemoryUsage, ColorPrint, ToporExternalTypes

use super::containers::with_alloc_failure;
use crate::basic_memory_usage;
use crate::color_print::*;
use crate::time_measure::CTimeMeasure;
use crate::topi_params::*;
use crate::topi_statistics::*;
use crate::topi_var_scores::CVarScores;
use crate::topor_external_types::*;

#[test]
fn var_scores_heap_order()
{
	for initOrder in [false, true]
	{
		let mut inc = 1.0;
		let mut h = CVarScores::default();
		h.SetInitOrder(initOrder);
		h.SetMultiplier(1.5);
		assert!(h.empty());
		for v in 1..=20u32
		{
			h.insert(v, 0.);
		}
		assert_eq!(h.size(), 20);
		assert!(h.in_heap(7) && h.var_score_exists(20) && !h.var_score_exists(1000));
		// With equal scores, the order is determined by the insertion order semantics
		assert_eq!(h.get_min(), if initOrder { 1 } else { 20 });
		assert!(!h.increase_score(5, 3., &mut inc));
		assert!(!h.increase_score(9, 2., &mut inc));
		assert!(!h.increase_score(13, 5., &mut inc));
		assert_eq!(h.get_var_score(13), 5.);
		assert_eq!(h.remove_min(), 13);
		assert!(!h.in_heap(13));
		// Increasing the score of a variable outside of the heap
		assert!(!h.increase_score(13, 1., &mut inc));
		assert_eq!(h.remove_min(), 5);
		assert_eq!(h.remove_min(), 9);
		h.reinsert_if_not_in_heap(13);
		h.reinsert_if_not_in_heap(13);
		assert_eq!(h.get_min(), 13);
		h.var_inc_update(0.5, &mut inc);
		assert_eq!(inc, 2.);
		h.set_var_score(2, 100.);
		h.rebuild();
		assert_eq!(h.get_min(), 2);
		assert_eq!(h.size(), 18);
		h.replace_pos_score_vars(2, 5);
		assert_eq!(h.get_var_score(5), 100.);
		h.set_var_score(5, 0.);
		// Draining keeps the heap property
		h.set_var_score(2, 6.);
		let mut prev = f64::MAX;
		let mut drained = 0;
		while !h.empty()
		{
			let v = h.remove_min();
			assert!(h.get_var_score(v) <= prev);
			prev = h.get_var_score(v);
			drained += 1;
		}
		assert_eq!(drained, 18);
		let _ = h.memMb();
	}
}

#[test]
fn var_scores_rescale_reserve_and_failures()
{
	let mut inc = 1e99;
	let mut h = CVarScores::new();
	h.reserve_exactly(10);
	assert!(!h.uninitialized_or_erroneous());
	h.insert(1, 0.);
	h.insert(2, 0.);
	assert!(!h.increase_score(1, 1., &mut inc));
	assert!(h.increase_score(1, 100., &mut inc));
	assert!((h.get_var_score(1) - 1.01e1).abs() < 1e-9);
	assert_eq!(inc, 1e-1);

	let mut h = CVarScores::new();
	with_alloc_failure(0, || h.insert(5, 0.));
	assert!(h.uninitialized_or_erroneous());
	let mut h = CVarScores::new();
	with_alloc_failure(1, || h.insert(5, 0.));
	assert!(h.uninitialized_or_erroneous());
}

#[test]
fn params_descriptions_and_values()
{
	let mut p = CTopiParams::new(2);
	assert!(!p.IsError());
	assert_eq!(p.GetErrorDescr(), "");
	assert_eq!(p.m_ParamInitEntriesPerWL.get(), 4);
	assert_eq!(p.m_ParamVerbosity.get(), 0);
	let descr = p.GetAllParamsDescr();
	assert!(descr.starts_with("\x1b[1;31mc solver library parameters:\x1b[0m\n************************************************************\n"));
	// A single-default integer, a per-mode Boolean, the special floating-point values
	assert!(descr.contains("c \x1b[1;35m/verbosity/level\x1b[0m : h; default = \x1b[1;32m0 in [0, 3]\x1b[0m : Verbosity level"));
	assert!(descr.contains("c \x1b[1;35m/decision/vsids/init_order\x1b[0m : b; default = \x1b[1;32m{0, 1, 0, 1, 1, 0, 1, 0, 0} in [0, 1]\x1b[0m : "));
	assert!(descr.contains("/timeout/global\x1b[0m : d; default = \x1b[1;32mmax(d) in [epsilon(d), max(d)]\x1b[0m"));
	assert!(descr.contains("/decision/init_clss_boost/mult_lowest\x1b[0m : d; default = \x1b[1;32m1.000000 in [lowest(d), max(d)]\x1b[0m"));
	assert!(descr.contains("/decision/init_clss_boost/mult_highest\x1b[0m : d; default = \x1b[1;32m{10.000000, 10.000000, 10.000000, 10.000000, 10.000000, 10.000000, 10.000000, 5.000000, 10.000000} in [min(d), max(d)]\x1b[0m"));
	assert!(descr.contains("/deletion/clause/frac_to_delete\x1b[0m : f; default = \x1b[1;32m{0.700000, 0.800000"));
	assert!(descr.contains("/phase/restarts_block_size\x1b[0m : t; default = \x1b[1;32m10 in [1, 65535]\x1b[0m"));
	assert!(descr.contains("/query_type/short_query_conf_thr\x1b[0m : j; default = \x1b[1;32m10000 in [0, 4294967295]\x1b[0m"));
	// A separator line between every two parameter classes
	assert!(descr.contains("************************************************************\nc \x1b[1;35m/assumptions/allow_reorder"));
	assert!(descr.ends_with("************************************************************\n"));

	// The first invocation prints all the values; the next ones only the modified values
	let all = p.GetAllParamsCurrValues();
	assert!(all.starts_with("\x1b[1;31mc Parameter values:\x1b[0m\n"));
	assert!(all.contains("c \x1b[1;35m/verbosity/level\x1b[0m 0\n"));
	assert_eq!(p.GetAllParamsCurrValues(), "************************************************************\n");
	p.SetParam("/verbosity/level", 2.);
	assert_eq!(p.m_ParamVerbosity.get(), 2);
	assert_eq!(p.GetAllParamsCurrValues(), "\x1b[1;31mc Parameter values:\x1b[0m\x1b[1;31m (only the modified ones are printed)\x1b[0m\n************************************************************\nc \x1b[1;35m/verbosity/level\x1b[0m 2\n************************************************************\n");
}

#[test]
fn params_modes()
{
	let mut p = CTopiParams::new(4);
	assert_eq!(p.m_ParamInitEntriesPerWL.get(), 4);
	assert_eq!(p.GetMode(), 0);
	assert_eq!(p.m_ParamVarDecayMax.get(), 0.99);
	p.SetParam("/restarts/numeric/conflict_thr", 77.);
	p.SetParam(m_ModeParamName, 4.);
	assert!(!p.IsError());
	assert_eq!(p.GetMode(), 4);
	assert_eq!(p.m_ModeParam.get(), 4);
	assert_eq!(p.m_ParamVarDecayMax.get(), 0.95);
	assert_eq!(p.m_ParamExistingBinWLStrat.get(), 2);
	assert!(p.m_ParamVarActivityUseMapleLevelBreaker.get());
	// A mode change overrides the previously set values of mode-dependent parameters only
	assert_eq!(p.m_ParamRestartNumericInitConfThr.get(), 100);
	p.SetParam("/verbosity/level", 1.);
	p.SetParam(m_ModeParamName, 7.);
	assert_eq!(p.m_ParamVerbosity.get(), 1);
	assert_eq!(p.m_ParamAddClsRemoveClssGloballySatByLitMinSize.get(), 2);
	assert_eq!(p.m_ParamClsDelLowFracToDelete.get(), 0.7f32);
	assert_eq!(p.m_ParamChronoBtIfHigherInit.get(), 0);
	// An out-of-range mode is an error; the modes aren't changed
	p.SetParam(m_ModeParamName, 9.);
	assert!(p.IsError());
	assert!(p.GetErrorDescr().contains("the value 9 is above the maximal value"));
	assert_eq!(p.GetMode(), 7);
	assert_eq!(p.m_ParamChronoBtIfHigherInit.get(), 0);
}

#[test]
fn params_errors()
{
	let mut p = CTopiParams::new(2);
	p.SetParam("/no/such/param", 1.);
	assert!(p.IsError());
	assert!(p.GetErrorDescr().starts_with("Parameter /no/such/param doesn't exists. See all parameters description below:\n"));
	// Nothing can be set after an error
	p.SetParam("/verbosity/level", 1.);
	assert_eq!(p.m_ParamVerbosity.get(), 0);

	let check = |name: &str, val: f64, err: &str|
	{
		let mut p = CTopiParams::new(2);
		p.SetParam(name, val);
		assert!(p.IsError(), "{} {}", name, val);
		assert!(p.GetErrorDescr().starts_with(name), "{}", p.GetErrorDescr());
		assert!(p.GetErrorDescr().ends_with(&(": couldn't set this parameter : ".to_string() + err)), "{}", p.GetErrorDescr());
	};
	check("/verbosity/level", 4., "the value 4 is above the maximal value");
	check("/verbosity/level", 1.5, "the value is 1.500000, but it must be an integer");
	check("/verbosity/print_out_confs", 0., "the value 0 is below the minimal value");
	check("/deletion/wasted_fraction_thr", 0., "the value 0 is below the minimal value");
	check("/deletion/wasted_fraction_thr", 1.5, "the value 1 is above the maximal value");
	check("/verbosity/print_mem_prof", 2., "the value 2 is above the maximal value");
	check("/verbosity/print_mem_prof", 0.5, "the value is 0.500000, but it must be an integer");
	check("/bcp/init_entries_per_wl", 1., "the value 1 is below the minimal value");

	// Legal values of every type
	let mut p = CTopiParams::new(2);
	p.SetParam("/verbosity/print_mem_prof", 1.);
	p.SetParam("/deletion/wasted_fraction_thr", 0.5);
	p.SetParam("/deletion/clause/frac_to_delete", 0.25);
	p.SetParam("/phase/restarts_block_size", 3.);
	p.SetParam("/timeout/global", f64::NAN);
	assert!(!p.IsError());
	assert!(p.m_ParamPrintMemoryProfiling.get());
	assert_eq!(p.m_ParamWastedFractionThrToDelete.get(), 0.5);
	assert_eq!(p.m_ParamClsDelLowFracToDelete.get(), 0.25);
	assert_eq!(p.m_ParamPhaseMngRestartsBlockSize.get(), 3);
	assert!(p.GetAllParamsCurrValues().contains("/timeout/global\x1b[0m nan\n"));
	p.SetParam("/timeout/global", 0.125);
	p.SetParam("/deletion/clause/frac_to_delete", f64::NAN);
	let vals = p.GetAllParamsCurrValues();
	assert!(vals.contains("/timeout/global\x1b[0m 0.125000\n") && vals.contains("/deletion/clause/frac_to_delete\x1b[0m nan\n"));
}

#[test]
fn params_internal_errors()
{
	let mut p = CTopiParams::new(2);
	// A class-less parameter name and a duplicate parameter name
	assert_eq!(CTopiParams::GetParamClass("noslash"), "");
	assert_eq!(CTopiParams::GetParamClass("/onlyone"), "");
	assert_eq!(CTopiParams::GetParamClass("/a/b/c"), "/a");
	p.NewParam("/verbosity/level", "duplicate".to_string(), |_, _| String::new(), |_| String::new());
	assert!(p.IsError());
	assert!(p.GetErrorDescr().starts_with("Parameter already exists; internal error; create Topor developers. The parameter: /verbosity/level : h; default"));
	// No new parameters after an error
	p.NewParam("/brand/new", "new".to_string(), |_, _| String::new(), |_| String::new());
	assert!(!p.GetAllParamsDescr().contains("/brand/new"));

	// Initial values outside of [min, max]
	let mut errs: Vec<String> = vec![];
	let (param, descr) = CTopiParam::<u8>::new_single(&mut |_, d: &str| errs.push(d.to_string()), false, "d", 0, Some(1), Some(2));
	assert!(descr.is_none() && param.get() == 0);
	let (_, descr) = CTopiParam::<u8>::new_single(&mut |_, d: &str| errs.push(d.to_string()), false, "d", 3, Some(1), Some(2));
	assert!(descr.is_none());
	let (_, descr) = CTopiParam::<u8>::new_modes(&mut |_, d: &str| errs.push(d.to_string()), false, "d", [1, 1, 1, 1, 3, 1, 1, 1, 1], Some(1), Some(2));
	assert!(descr.is_none());
	assert_eq!(errs, vec![ERR_VAL_BELOW_MIN, ERR_VAL_ABOVE_MAX, ERR_VAL_ABOVE_MAX]);
	// A parameter isn't registered, if there already is an error
	let (_, descr) = CTopiParam::<u8>::new_single(&mut |_, _| panic!(), true, "d", 1, None, None);
	assert!(descr.is_none());
	let (_, descr) = CTopiParam::<u8>::new_modes(&mut |_, _| panic!(), true, "d", [1; 9], None, None);
	assert!(descr.is_none());
	let (_, descr) = CTopiParam::<bool>::new_single(&mut |_, _| panic!(), false, "d", true, None, None);
	assert_eq!(descr.unwrap(), "b; default = \x1b[1;32m1 in [0, 1]\x1b[0m : d");
	let (_, descr) = CTopiParam::<f32>::new_single(&mut |_, _| panic!(), false, "d", f32::EPSILON, None, None);
	assert_eq!(descr.unwrap(), "f; default = \x1b[1;32mepsilon(f) in [lowest(f), max(f)]\x1b[0m : d");
	let (_, descr) = CTopiParam::<u16>::new_modes(&mut |_, _| panic!(), false, "d", [7; 9], None, None);
	assert_eq!(descr.unwrap(), "t; default = \x1b[1;32m{7, 7, 7, 7, 7, 7, 7, 7, 7} in [0, 65535]\x1b[0m : d");
}

#[test]
fn statistics_counters()
{
	let mut s = TToporStatistics::new(1.);
	s.NewClause(1, false);
	s.NewClause(2, false);
	s.NewClause(3, false);
	s.NewClause(5, true);
	assert_eq!((s.m_ActiveBinaryClss, s.m_ActiveLongClss, s.m_ActiveLongLearntClss, s.m_EverAddedBinaryClss, s.GetActiveClss()), (1, 2, 1, 1, 3));
	assert_eq!(s.GetActiveLongClsLen(), 8);
	s.RecordDeletedLitsFromCls(1);
	assert_eq!(s.GetActiveLongClsLen(), 7);
	s.DeleteClause(1, false);
	s.DeleteClause(4, true);
	s.DeleteClause(3, false);
	s.DeleteClause(2, false);
	assert_eq!((s.m_ActiveBinaryClss, s.m_ActiveLongClss, s.m_ActiveLongLearntClss, s.m_EverAddedBinaryClss), (0, 0, 0, 1));
	s.NewClause(2, true);
	s.NewClause(2, true);
	s.DeleteBinClauses(2);
	assert_eq!((s.m_ActiveBinaryClss, s.GetActiveLongClsLen()), (0, 0));
	s.UpdateMaxInternalVar(17);
	assert_eq!(s.m_MaxInternalVar, 17);
	s.NewSolveInvocation(false);
	s.NewSolveInvocation(true);
	assert_eq!((s.m_SolveInvs, s.m_ShortIncSolveInvs), (2, 1));
	assert_eq!((s.ChronoBtsPerc(), s.BCPBtsPerc()), (0., 0.));
	s.m_Backtracks = 4;
	s.m_ChronoBacktracks = 1;
	s.m_BCPBacktracks = 2;
	assert_eq!((s.ChronoBtsPerc(), s.BCPBtsPerc()), (25., 50.));
}

#[test]
fn statistics_string()
{
	let mut s = TToporStatistics::new(1.);
	let bi = TStatBufferInfo { m_b_num: 1, m_b_cap: 2000000, m_b_size: 500000, m_extra_string: String::new() };
	let plain = s.StatStrShort(&bi, false, false);
	let lines: Vec<&str> = plain.lines().collect();
	assert_eq!(lines.len(), 2);
	assert_eq!(lines[0], "c ToporStt CpuT0 WallT0 CPUTSolve WallTSolve CurrMemMb PeakMemMb Confs Decs D/C BCPs Asngs ImplPr ImplPerCPUT Inprocs DupBins Bufs BufSzMb BufCapMb SolveInvs ShortIncrInvs UserVars IntrVars AddClss ActClss BinAClss LongAClss AvrgAClsLen AvrgALongClsLen LongALrnts Simplfs ClssDels SubsLRem FlpRecs FlpSw FlpUnit Bts ChBtsPr BCPBtPr AvrgDecLev LvlsSvdAsmp RTAsg VSIDSInc VSIDSDecay Rsts RstBlocked");
	let fields: Vec<&str> = lines[1].split(' ').collect();
	assert_eq!(&fields[..2], ["c", "ToporStt"]);
	// Confs Decs D/C: 0/0 is printed as nan
	assert_eq!(&fields[8..11], ["0", "0", "nan"]);
	// Bufs BufSzMb BufCapMb
	assert_eq!(&fields[17..20], ["1", "0.5", "2.0"]);
	assert_eq!(&fields[fields.len() - 5..], ["1.0e+00", "0.000", "0", "0", ""]);
	// The header is printed once in 50 invocations, unless forced
	assert_eq!(s.StatStrShort(&bi, false, false).lines().count(), 1);
	assert_eq!(s.StatStrShort(&bi, true, false).lines().count(), 2);
	for _ in 3..50
	{
		assert_eq!(s.StatStrShort(&bi, false, false).lines().count(), 1);
	}
	assert_eq!(s.StatStrShort(&bi, false, false).lines().count(), 2);
	// Colors & the extra string
	s.NewClause(2, false);
	s.NewClause(4, false);
	s.m_Decisions = 3;
	s.m_Conflicts = 2;
	s.m_SumOfAllDecLevels = 6;
	let bi = TStatBufferInfo { m_b_num: 1, m_b_cap: 1, m_b_size: 1, m_extra_string: "extra".to_string() };
	let colored = s.StatStrShort(&bi, true, true);
	assert!(colored.starts_with("c ToporStt \x1b[1;36mCpuT0 WallT0 CPUTSolve WallTSolve CurrMemMb PeakMemMb\x1b[0m\x1b[1;35m Confs"));
	assert!(colored.contains("\x1b[1;35m2\x1b[0m \x1b[1;35m3\x1b[0m \x1b[1;35m1.5\x1b[0m "));
	assert!(colored.contains("\x1b[1;35m3.0\x1b[0m \x1b[1;35m4.0\x1b[0m "));
	assert!(colored.contains("\x1b[1;32m2.0\x1b[0m "));
	assert!(colored.ends_with("\nextra\n"));

	assert_eq!(to_string_scientific(0.), "0.0e+00");
	assert_eq!(to_string_scientific(12345.), "1.2e+04");
	assert_eq!(to_string_scientific(1e-100), "1.0e-100");
	assert_eq!(to_string_scientific(0.00123), "1.2e-03");
	assert_eq!(to_string_scientific(f64::NAN), "nan");
	assert_eq!(to_string_scientific(f64::INFINITY), "inf");
}

#[test]
fn time_measure()
{
	let mut t = CTimeMeasure::new(false, 1);
	assert_eq!(t.GetTimeout(), CTimeMeasure::TIME_INFINITY);
	assert!(!t.IsTimeoutSet());
	assert!(!t.IsTimeout());
	assert_eq!(t.TimeLeftTillTimeout(), CTimeMeasure::TIME_INFINITY);
	assert!(t.TimePassedSinceStartOrReset() >= 0.);
	t.SetTimeout(-1.);
	assert!(!t.IsTimeoutSet());
	t.SetTimeout(1000.);
	assert!(t.IsTimeoutSet() && !t.IsTimeout());
	assert!(t.TimeLeftTillTimeout() > 999.);
	t.Stop();
	std::thread::sleep(std::time::Duration::from_millis(1));
	assert!(t.IsTimeout());
	assert!(t.WallTimeLeftTillTimeout() < 0.);
	t.Reset();

	// CPU mode: whole seconds
	t.SetModeCpuTime();
	assert_eq!(t.CpuTimeLeftTillTimeout(), 0.00000001);
	assert!(!t.IsTimeout());
	assert_eq!(t.TimePassedSinceStartOrReset(), 0.);
	assert_eq!(t.TimeLeftTillTimeout(), 0.00000001);
	let mut c = CTimeMeasure::new(true, 1);
	assert_eq!(c.CpuTimeLeftTillTimeout(), CTimeMeasure::TIME_INFINITY);
	assert!(!c.IsTimeout());
	c.SetModeWallTime();
	assert!(!c.IsTimeout());

	// The modulo factor: the clock is consulted only once in a while
	let mut m = CTimeMeasure::new(false, 3);
	let first = m.WallTimePassedSinceStartOrReset();
	std::thread::sleep(std::time::Duration::from_millis(2));
	assert_eq!(m.WallTimePassedSinceStartOrReset(), first);
	assert_eq!(m.WallTimePassedSinceStartOrReset(), first);
	assert!(m.WallTimePassedSinceStartOrReset() > first);
	let first = m.CpuTimePassedSinceStartOrReset();
	assert_eq!(m.CpuTimePassedSinceStartOrReset(), first);
	assert_eq!(m.CpuTimePassedSinceStartOrReset(), 0.);
	assert_eq!(m.CpuTimePassedSinceStartOrReset(), 0.);
	m.SetTestModuloFactor(1);
	assert!(m.WallTimePassedSinceStartOrReset() > first);
	assert_eq!(m.CpuTimePassedSinceStartOrResetConst(), 0.);
}

#[test]
fn memory_usage()
{
	assert!(basic_memory_usage::GetPeakRSS() > 0);
	assert!(basic_memory_usage::GetPeakRSSMb() <= basic_memory_usage::GetPeakRSS());
	// The current RSS is read from /proc, which doesn't exist on every platform
	assert_eq!(basic_memory_usage::GetCurrentRSSMb(), basic_memory_usage::GetCurrentRSS() >> 20);
	assert_eq!(basic_memory_usage::parse_statm(None), 0);
	assert_eq!(basic_memory_usage::parse_statm(Some("garbage".to_string())), 0);
	assert_eq!(basic_memory_usage::parse_statm(Some("100 x".to_string())), 0);
	assert!(basic_memory_usage::parse_statm(Some("100 7 3".to_string())) >= 7 * 4096);
}

#[test]
fn color_print_and_external_types()
{
	assert_eq!(print_as_color("x", ansi_color_code::none), "x");
	assert_eq!(print_as_color(5, ansi_color_code::red), "\x1b[1;31m5\x1b[0m");
	assert_eq!(print_as_color("x", ansi_color_code::bright_white), "\x1b[1;97mx\x1b[0m");

	let rets = [
		(TToporReturnVal::RET_SAT, "SAT"), (TToporReturnVal::RET_UNSAT, "UNSAT"), (TToporReturnVal::RET_TIMEOUT_LOCAL, "TIMEOUT_LOCAL"), (TToporReturnVal::RET_CONFLICT_OUT, "CONFLICT_OUT"),
		(TToporReturnVal::RET_MEM_OUT, "MEM_OUT"), (TToporReturnVal::RET_USER_INTERRUPT, "USER_INTERRUPT"), (TToporReturnVal::RET_INDEX_TOO_NARROW, "INDEX_TOO_NARROW"), (TToporReturnVal::RET_PARAM_ERROR, "PARAM_ERROR"),
		(TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR, "ASSUMPTION_REQUIRED_ERROR"), (TToporReturnVal::RET_TIMEOUT_GLOBAL, "TIMEOUT_GLOBAL"), (TToporReturnVal::RET_DRAT_FILE_PROBLEM, "DRAT_FILE_PROBLEM"), (TToporReturnVal::RET_EXOTIC_ERROR, "EXOTIC_ERROR"),
	];
	for (r, s) in rets
	{
		assert_eq!(r.to_string(), s);
	}
	assert!(TToporStatus::STATUS_FIRST_UNRECOVERABLE == TToporStatus::STATUS_CONTRADICTORY);
	assert!(TToporStatus::STATUS_FIRST_PERMANENT_ERROR > TToporStatus::STATUS_FIRST_UNRECOVERABLE);
	assert!(TToporStatus::STATUS_USER_INTERRUPT < TToporStatus::STATUS_FIRST_UNRECOVERABLE);
}
