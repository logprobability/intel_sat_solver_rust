// Targeted unit tests (see mod.rs): the search-related parts of the solver, that is,
// conflict analysis, restarts, decisions, assignments, backtracking and inprocessing

use crate::fuzzing::*;
use crate::topi::*;

type TParams<'a> = &'a [(&'a str, f64)];

// The compressed mode (CTopi<u64, true>) is used by these tests as little as possible, and never through the fuzzing harness. This is historical:
// the C++ bit arrays reserve one word too few (CSetElem always touches the word after the one, in which an element starts), which used to corrupt the heap of the test process here;
// the fork has since fixed it by reserving an extra word (see CBitArray::bit_reserve_new_chunk), and the compressed mode is fuzzed by the other tests

// The pigeonhole principle: unsatisfiable and requires many conflicts
fn Php(pigeons: TLit, holes: TLit) -> Vec<Vec<TLit>>
{
	let X = |p: TLit, h: TLit| p * holes + h + 1;
	let mut clss: Vec<Vec<TLit>> = vec![];
	for p in 0..pigeons
	{
		clss.push((0..holes).map(|h| X(p, h)).collect());
	}
	for h in 0..holes
	{
		for p1 in 0..pigeons
		{
			for p2 in p1 + 1..pigeons
			{
				clss.push(vec![-X(p1, h), -X(p2, h)]);
			}
		}
	}
	clss
}

// A deterministic random k-SAT instance
fn RandSat(seed: u64, vars: usize, clssNum: usize, len: usize) -> Vec<Vec<TLit>>
{
	let bytes = RandomBytes(seed, clssNum * len * 64);
	let mut inp = CFuzzInput::new(&bytes);
	let mut clss = vec![];
	for _ in 0..clssNum
	{
		let mut c: Vec<TLit> = vec![];
		while c.len() < len
		{
			let v = 1 + inp.Below(vars) as TLit;
			if !c.contains(&v) && !c.contains(&-v)
			{
				c.push(if inp.Byte() & 1 == 0 { v } else { -v });
			}
		}
		clss.push(c);
	}
	clss
}

// setUp is applied before the clauses are added (e.g., DRAT and the callbacks must be set up before the first clause)
fn NewTopiSetUp<Ind: TUInd, const Compress: bool>(params: TParams, clss: &[Vec<TLit>], setUp: impl FnOnce(&mut CTopi<Ind, Compress>)) -> CTopi<Ind, Compress>
{
	let mut t = CTopi::<Ind, Compress>::new(0);
	for (name, val) in params
	{
		t.SetParam(name, *val);
	}
	setUp(&mut t);
	for c in clss
	{
		t.AddUserClause(c);
	}
	assert!(!t.IsError(), "{}", t.GetStatusExplanation());
	t
}

fn NewTopi<Ind: TUInd, const Compress: bool>(params: TParams, clss: &[Vec<TLit>]) -> CTopi<Ind, Compress>
{
	NewTopiSetUp(params, clss, |_| ())
}

fn Unlimited() -> (f64, bool)
{
	(f64::MAX, true)
}

// Solves and verifies the model, if any
fn SolveAndCheck<Ind: TUInd, const Compress: bool>(t: &mut CTopi<Ind, Compress>, clss: &[Vec<TLit>], assumps: &[TLit], confThr: u64) -> TToporReturnVal
{
	let ret = t.Solve(assumps, Unlimited(), confThr);
	if ret == TToporReturnVal::RET_SAT
	{
		let IsSat = |l: TLit| t.GetValue(l) == TToporLitVal::VAL_SATISFIED;
		for c in clss
		{
			assert!(c.iter().any(|l| IsSat(*l)), "The model doesn't satisfy the clause {:?}", c);
		}
		for a in assumps
		{
			assert!(IsSat(*a), "The model doesn't satisfy the assumption {}", a);
		}
	}
	ret
}

// The verdict of the instance by the default configuration
fn RefVerdict(clss: &[Vec<TLit>], assumps: &[TLit]) -> TToporReturnVal
{
	let mut t = NewTopi::<u32, false>(&[], clss);
	SolveAndCheck(&mut t, clss, assumps, u64::MAX)
}

// Solves the instance with the given parameters in the given instantiation; verifies the verdict against the default configuration; returns the solver for inspection
fn RunCfg<Ind: TUInd, const Compress: bool>(params: TParams, clss: &[Vec<TLit>]) -> CTopi<Ind, Compress>
{
	let mut t = NewTopi::<Ind, Compress>(params, clss);
	let ret = SolveAndCheck(&mut t, clss, &[], u64::MAX);
	assert!(!t.IsError(), "{}", t.GetStatusExplanation());
	assert_eq!(ret, RefVerdict(clss, &[]));
	t
}

fn Instances() -> Vec<Vec<Vec<TLit>>>
{
	vec![Php(6, 5), Php(5, 4), RandSat(1, 40, 170, 3), RandSat(2, 40, 172, 3), RandSat(3, 30, 128, 3), RandSat(4, 50, 213, 3), RandSat(5, 25, 240, 4), RandSat(6, 60, 255, 3)]
}

// Runs the scenario through the fuzzing harness (which verifies everything against an exact oracle) on the uncompressed solver modes
fn RunOnAllModes(s: &TFuzzScenario)
{
	for solverMode in 0..2
	{
		RunScenario(s, solverMode, None);
	}
}

fn P(name: &str, val: f64) -> TFuzzOp
{
	TFuzzOp::SetParam(name.to_string(), val)
}

// A scenario out of a small random instance with the given parameters
fn SmallScenario(seed: u64, vars: usize, clssNum: usize, len: usize, params: TParams, drat: u8) -> TFuzzScenario
{
	let mut s = TFuzzScenario { m_Vars: vars, m_VarsNumHint: vars as TLit, m_Drat: drat, m_CbLearnt: true, ..Default::default() };
	for (name, val) in params
	{
		s.m_Ops.push(P(name, *val));
	}
	for c in RandSat(seed, vars, clssNum, len)
	{
		s.m_Ops.push(TFuzzOp::AddClause(c));
	}
	s.m_Ops.push(TFuzzOp::Solve(vec![], u64::MAX));
	s
}

const ConfAnalysisCfgs: &[TParams] = &[
	// On-the-fly subsumption from the very beginning (both the contradicting clause and the parents)
	&[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/contradicting_first_restart", 0.),
		("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/parent_first_restart", 0.)],
	&[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/contradicting_first_restart", 0.),
		("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 0.), ("/conflicts/minimize_clauses", 0.), ("/conflicts/bin_res_max_size", 0.)],
	&[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/contradicting_first_restart", 0.),
		("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 0.), ("/backtracking/custom_bt_strat_init", 1.), ("/backtracking/chrono_bt_if_higher_init", 0.)],
	&[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 0.), ("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 1000.),
		("/conflicts/on_the_fly_subsumption/parent_first_restart", 0.), ("/conflicts/minimize_clauses", 0.), ("/conflicts/bin_res_max_size", 0.)],
	&[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 1.), ("/conflicts/on_the_fly_subsumption/contradicting_first_restart", 0.),
		("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 1.), ("/conflicts/on_the_fly_subsumption/parent_first_restart", 0.)],
	// ALL-UIP
	&[("/conflicts/all_uip/mode", 1.)],
	&[("/conflicts/all_uip/mode", 2.), ("/conflicts/flipped_drop_if_subsumed", 0.)],
	&[("/conflicts/all_uip/mode", 3.), ("/restarts/numeric/conflict_thr", 3.), ("/conflicts/all_uip/success_rate_failure_thr", 1.)],
	&[("/conflicts/all_uip/mode", 3.), ("/restarts/numeric/conflict_thr", 3.), ("/conflicts/all_uip/first_restart", 2.), ("/conflicts/all_uip/last_restart", 6.), ("/conflicts/minimize_clauses", 0.)],
	&[("/conflicts/all_uip/mode", 1.), ("/restarts/numeric/conflict_thr", 2.), ("/conflicts/all_uip/success_rate_failure_thr", 0.), ("/conflicts/minimize_clauses", 0.), ("/conflicts/bin_res_max_size", 0.)],
	// Flipped recording
	&[("/conflicts/flipped_drop_if_subsumed", 0.)],
	&[("/conflicts/flipped_drop_if_subsumed", 0.), ("/conflicts/flipped_recording_max_lbd", 1.), ("/conflicts/minimize_clauses", 0.)],
	&[("/conflicts/flipped_drop_if_subsumed", 0.), ("/conflicts/flipped_recording_max_lbd", 2.), ("/backtracking/chrono_bt_if_higher_init", 0.)],
	&[("/conflicts/flipped_recording_max_lbd", 0.), ("/conflicts/minimize_clauses", 0.), ("/conflicts/bin_res_max_lbd", 0.)],
	// Custom backtracking
	&[("/backtracking/custom_bt_strat_init", 1.), ("/backtracking/chrono_bt_if_higher_init", 0.), ("/debugging/assert_consistency", 1.)],
	&[("/backtracking/custom_bt_strat_init", 2.), ("/backtracking/chrono_bt_if_higher_init", 1.), ("/decision/vsids/var_activity_inc_decay", 0.01), ("/decision/vsids/var_decay_max", 0.02)],
	// Decision heuristic variations
	&[("/decision/vsids/var_decay_update_conf_rate", 1.), ("/decision/vsids/var_activity_use_maple_level_breaker", 1.), ("/decision/vsids/var_activity_glue_update", 1.), ("/debugging/assert_consistency", 2.)],
	&[("/decision/polarity/flip_factor", 3.), ("/decision/polarity/strategy", 1.), ("/decision/polarity/init_strategy", 2.), ("/phase/boost_flipped_forced", 1.)],
];

fn RunConfAnalysisCfgs<Ind: TUInd, const Compress: bool>()
{
	for clss in Instances().iter().take(6)
	{
		for cfg in ConfAnalysisCfgs
		{
			let _ = RunCfg::<Ind, Compress>(cfg, clss);
		}
	}
}

#[test]
fn conflict_analysis_configurations_u32()
{
	RunConfAnalysisCfgs::<u32, false>();
}

#[test]
fn conflict_analysis_configurations_u64()
{
	RunConfAnalysisCfgs::<u64, false>();
}

// The same configurations over small instances with the exact oracle, DRAT and the learnt-clause callback
#[test]
fn conflict_analysis_configurations_with_oracle()
{
	for (i, cfg) in ConfAnalysisCfgs.iter().enumerate()
	{
		for seed in 0..6
		{
			RunOnAllModes(&SmallScenario(100 + seed, 12, 52, 3, cfg, ((i as u64 + seed) % 5) as u8));
		}
	}
}

const RestartCfgs: &[TParams] = &[
	// Numeric: arithmetic, local and global
	&[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 1.), ("/restarts/arithmetic/conflict_increment", 1.), ("/phase/restarts_block_size", 4.), ("/phase/unforce_restarts_fraction_init", 0.5)],
	&[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 2.), ("/restarts/numeric/is_local", 0.), ("/phase/restarts_block_size", 4.), ("/phase/unforce_restarts_fraction_init", 0.5), ("/phase/start_inv_strat", 1.)],
	// Numeric: Luby
	&[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 2.), ("/restarts/numeric/sub_strat", 1.), ("/phase/restarts_block_size", 3.), ("/phase/unforce_restarts_fraction_init", 0.3), ("/phase/start_inv_strat", 2.)],
	&[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 1.), ("/phase/unforce_restarts_fraction_init", 1.)],
	&[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 1.), ("/phase/unforce_restarts_fraction_init", 0.)],
	// LBD-based with and without blocking
	&[("/mode/value", 1.), ("/restarts/lbd/win_size", 2.), ("/restarts/lbd/average_mult", 5.), ("/restarts/lbd/blocking/enable", 0.), ("/restarts/lbd/thr_glue_val", 3.)],
	&[("/mode/value", 1.), ("/restarts/lbd/win_size", 3.), ("/restarts/lbd/average_mult", 5.), ("/restarts/lbd/blocking/enable", 1.), ("/restarts/lbd/blocking/conflicts_to_consider_blocking", 0.),
		("/restarts/lbd/blocking/win_size", 2.), ("/restarts/lbd/blocking/average_mult", 0.5)],
	// Restarts enable on-the-fly subsumption with the default settings
	&[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 4.), ("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 1000.)],
	&[("/mode/value", 7.), ("/restarts/numeric/conflict_thr", 3.)],
	&[("/mode/value", 8.), ("/restarts/lbd/win_size", 2.), ("/restarts/lbd/average_mult", 3.)],
];

fn RunRestartCfgs<Ind: TUInd, const Compress: bool>()
{
	for clss in Instances().iter().take(5)
	{
		for cfg in RestartCfgs
		{
			let _ = RunCfg::<Ind, Compress>(cfg, clss);
		}
	}
}

#[test]
fn restart_configurations_u32()
{
	RunRestartCfgs::<u32, false>();
	// Restarts do happen
	let t = RunCfg::<u32, false>(RestartCfgs[0], &Php(6, 5));
	assert!(t.m_Stat.m_Restarts > 5);
	let t = RunCfg::<u32, false>(RestartCfgs[5], &Php(6, 5));
	assert!(t.m_Stat.m_Restarts > 5);
	let t = RunCfg::<u32, false>(RestartCfgs[6], &Php(6, 5));
	assert!(t.m_Stat.m_RestartsBlocked > 0);
}

#[test]
fn restart_configurations_with_oracle()
{
	for (i, cfg) in RestartCfgs.iter().enumerate()
	{
		for seed in 0..4
		{
			RunOnAllModes(&SmallScenario(200 + seed, 12, 52, 3, cfg, ((i as u64 + seed) % 5) as u8));
		}
	}
}

#[test]
fn restart_luby_sequence()
{
	let expected = [1., 1., 2., 1., 1., 2., 4., 1., 1., 2., 1., 1., 2., 4., 8.];
	for (x, e) in expected.iter().enumerate()
	{
		assert_eq!(CTopi::<u32, false>::RestartLubySequence(2., x as u64), *e);
	}
}

// A restart strategy, which is neither numeric nor LBD-based, never restarts
#[test]
fn restart_unknown_strategy()
{
	let clss = Php(4, 3);
	let mut t = NewTopi::<u32, false>(&[], &clss);
	assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_UNSAT);
	t.m_CurrRestartStrat = 2;
	assert!(!t.Restart());
}

// The restart strategy is switched between the query types: LBD (initial) -> numeric (short) -> LBD (normal) -> numeric (short)
#[test]
fn restart_strategy_switching()
{
	for blocking in [0., 1.]
	{
		let clss = RandSat(11, 40, 150, 3);
		let params: TParams = &[("/restarts/strategy_init", 1.), ("/restarts/strategy_s", 0.), ("/restarts/strategy_n", 1.), ("/restarts/lbd/blocking/enable", blocking),
			("/restarts/lbd/win_size", 2.), ("/restarts/lbd/average_mult", 5.), ("/restarts/numeric/conflict_thr", 2.), ("/phase/unforce_restarts_fraction_s", 0.5), ("/phase/unforce_restarts_fraction_n", 0.5),
			("/phase/restarts_block_size", 2.), ("/phase/start_inv_strat", 1.)];
		let mut t = NewTopi::<u32, false>(params, &clss);
		let mut prevRet = None;
		for (assumps, confThr) in [(vec![1, 2], u64::MAX), (vec![-1, 3], 5000), (vec![2, -3], u64::MAX), (vec![-2, -3, 4], 5000), (vec![5], 5000), (vec![-5, 6], u64::MAX)]
		{
			let ret = SolveAndCheck(&mut t, &clss, &assumps, confThr);
			assert_eq!(ret, RefVerdictUnder(&clss, &assumps));
			prevRet = Some(ret);
		}
		assert!(prevRet.is_some() && !t.IsError());
		assert_eq!(t.m_CurrRestartStrat, RESTART_STRAT_LBD);
	}
}

fn RefVerdictUnder(clss: &[Vec<TLit>], assumps: &[TLit]) -> TToporReturnVal
{
	let mut all = clss.to_vec();
	for a in assumps
	{
		all.push(vec![*a]);
	}
	RefVerdict(&all, &[])
}

// Incremental solving with assumptions under the search configurations
#[test]
fn incremental_assumptions_configurations()
{
	let clss = RandSat(21, 30, 120, 3);
	for cfg in ConfAnalysisCfgs.iter().chain(RestartCfgs.iter())
	{
		let mut t = NewTopi::<u32, false>(cfg, &clss);
		t.SetParam("/decision/polarity/randomize_at_incremental_calls", 1.);
		for (i, assumps) in [vec![1, -2, 3], vec![-1, 2], vec![4, 5, -6, 7], vec![-3, -4], vec![8, 9, 10, 11, 12], vec![]].iter().enumerate()
		{
			let ret = SolveAndCheck(&mut t, &clss, assumps, if i % 2 == 0 { u64::MAX } else { 100000 });
			assert!(!t.IsError(), "{}", t.GetStatusExplanation());
			if ret != TToporReturnVal::RET_CONFLICT_OUT
			{
				assert_eq!(ret, RefVerdictUnder(&clss, assumps));
			}
		}
	}
}

// Inprocessing: removing the duplicate binary watches
#[test]
fn inprocessing_duplicate_binaries()
{
	for strat in [3., 4.]
	{
		for solverMode in 0..2
		{
			let mut s = TFuzzScenario { m_Vars: 8, m_VarsNumHint: 8, m_CbLearnt: true, ..Default::default() };
			s.m_Ops.push(P("/inprocessing/on", 1.));
			s.m_Ops.push(P("/bcp/existing_bin_wl_start", strat));
			for c in [vec![1, 2], vec![2, 1], vec![1, 2], vec![-3, 4], vec![4, -3], vec![5, 6, 7], vec![-5, 6], vec![-5, 6], vec![7, 8], vec![1, 3], vec![1, 3], vec![1, 2], vec![-8, -7], vec![-7, -8]]
			{
				s.m_Ops.push(TFuzzOp::AddClause(c));
			}
			s.m_Ops.push(TFuzzOp::Solve(vec![], u64::MAX));
			s.m_Ops.push(TFuzzOp::AddClause(vec![-1, -2]));
			s.m_Ops.push(TFuzzOp::AddClause(vec![-2, -1]));
			s.m_Ops.push(TFuzzOp::Solve(vec![3], u64::MAX));
			s.m_Ops.push(TFuzzOp::Solve(vec![-4], u64::MAX));
			s.m_Ops.push(TFuzzOp::Stats);
			RunScenario(&s, solverMode, None);
		}
	}

	let clss = vec![vec![1, 2], vec![2, 1], vec![1, 2], vec![-3, 4], vec![4, -3], vec![5, 6, 7], vec![3, 4], vec![3, 4]];
	let mut t = NewTopi::<u32, false>(&[("/inprocessing/on", 1.), ("/bcp/existing_bin_wl_start", 3.)], &clss);
	assert_eq!(SolveAndCheck(&mut t, &clss, &[], u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(t.m_Stat.m_Ings, 1);
	assert_eq!(t.m_Stat.m_IngsDuplicateBinsRemoved, 4);
	// Nothing to do without new binary clauses
	t.IngRemoveBinaryWatchesIfRequired();
	assert_eq!(t.m_Stat.m_IngsDuplicateBinsRemoved, 4);
}

// The directives are dumped, when dumping is on
#[test]
fn dump_file_directives()
{
	let path = std::env::temp_dir().join(format!("topor_cov_search_dump_{}.cnf", std::process::id()));
	{
		let mut t = CTopi::<u32, false>::new(0);
		t.m_DumpFile = std::fs::File::create(&path).ok();
		assert!(t.m_DumpFile.is_some());
		t.AddUserClause(&[1, 2, 3]);
		t.AddUserClause(&[-1, 2]);
		t.BoostScore(2, 1.5);
		t.FixPolarity(-3, true);
		t.CreateInternalLit(4);
		t.ClearUserPolarityInfo(3);
		assert_eq!(t.ChangeConfigToGiven(0), "");
		assert_eq!(t.ChangeConfigToGiven(3), "/topor/mode/value 3");
		assert_eq!(t.Solve(&[1], Unlimited(), u64::MAX), TToporReturnVal::RET_SAT);
		t.Backtrack(0, false, true);
		assert_eq!(t.m_DecLevel, 0);
	}
	let dump = std::fs::read_to_string(&path).unwrap();
	let _ = std::fs::remove_file(&path);
	for line in ["lb 2 1.5\n", "lf -3 1\n", "ll 4\n", "lc 3\n", "n 0\n", "n 3\n", "b 0\n"]
	{
		assert!(dump.contains(line), "{} is missing in {}", line, dump);
	}
}

#[test]
fn change_config_to_given()
{
	let mut t = CTopi::<u32, false>::new(0);
	assert_eq!(t.ChangeConfigToGiven(0), "");
	assert_eq!(t.ChangeConfigToGiven(2), "/topor/mode/value 2");
	assert_eq!(t.m_Params.m_ModeParam.get(), 2);
	let modes = m_Modes as u16;
	assert_eq!(t.ChangeConfigToGiven(2 * modes + 1), "/topor/mode/value 3 /topor/decision/polarity/flip_factor 1002");
	assert_eq!(t.m_Params.m_ParamPolarityFlipFactor.get(), 1002);
	assert!(!t.IsError());
}

// Unsatisfiable cores under the search configurations: verified against the exact oracle by the harness
#[test]
fn assumption_cores_configurations()
{
	for (i, cfg) in ConfAnalysisCfgs.iter().chain(RestartCfgs.iter()).enumerate()
	{
		// The heavy consistency checks are known to fail upstream when combined with UNSAT-core queries
		if cfg.iter().any(|(name, _)| *name == "/debugging/assert_consistency")
		{
			continue;
		}
		for seed in 0..3u64
		{
			let mut s = SmallScenario(300 + seed, 12, 44, 3, cfg, ((i as u64 + seed) % 5) as u8);
			let bytes = RandomBytes(1000 + seed + 10 * i as u64, 400);
			let mut inp = CFuzzInput::new(&bytes);
			for q in 0..12
			{
				let assumps: Vec<TLit> = (0..2 + inp.Below(6)).map(|_| (1 + inp.Below(12) as TLit) * if inp.Byte() & 1 == 0 { 1 } else { -1 }).collect();
				s.m_Ops.push(TFuzzOp::Solve(assumps, if q % 3 == 2 { 1000 } else { u64::MAX }));
			}
			RunOnAllModes(&s);
		}
	}
}

// Allocation failures at the specific points of the search, which test their allocations
#[test]
fn alloc_failures_during_search()
{
	use crate::topor_dyn_array::alloc_fault;

	// The windows of the LBD-based restart strategy
	for (blocking, failAt) in [(1., 0), (1., 1), (0., 0)]
	{
		let clss = vec![vec![1, 2, 3], vec![4, 5, 6]];
		let mut t = Prepared::<u32, false>(&[("/mode/value", 1.), ("/restarts/lbd/blocking/enable", blocking)], &clss, &[]);
		assert_eq!(t.m_CurrRestartStrat, RESTART_STRAT_LBD);
		t.m_QueryCurr = TQueryType::QUERY_INIT;
		alloc_fault::fail_after(failAt);
		t.RestartInit();
		alloc_fault::never_fail();
		assert_eq!(t.m_Status.get(), TToporStatus::STATUS_ALLOC_FAILED);
		// The LBD window is initialized last, so its failure is the one reported
		assert!(t.GetStatusExplanation().contains("m_RstGlueLbdWin"));
	}

	// ALL-UIP for the main clause (-2 -5 -6), which would become (-2 -1): neither the copy of the clause nor the counters per decision level can be allocated
	for fail in [false, true]
	{
		let clss = vec![vec![-1, 5], vec![-1, 6], vec![-2, -5, 7], vec![-2, -6, -7], vec![8, 9, 10]];
		let mut t = Prepared::<u32, false>(&[("/conflicts/all_uip/mode", 1.)], &clss, &[]);
		assert!(!DecideManually(&mut t, 1).IsContradiction());
		let mut ci = DecideManually(&mut t, 2);
		assert!(ci.IsContradiction());
		let mut clsBeforeAllUip: CVector<TULit> = CVector::new(0, 0);
		if fail
		{
			alloc_fault::fail_after(0);
		}
		let (cls, clsInd) = t.LearnAndUpdateHeuristics(&mut ci, &mut clsBeforeAllUip);
		alloc_fault::never_fail();
		assert_eq!(t.m_Stat.m_AllUipAttempted, 1);
		assert!(t.m_RootedVars.empty() && t.m_VisitedVars.empty());
		if fail
		{
			assert_eq!(clsInd, CTopi::<u32, false>::BadClsInd);
			assert_eq!(t.m_Status.get(), TToporStatus::STATUS_ALLOC_FAILED);
		}
		else
		{
			assert!(!t.IsError());
			assert_eq!(t.m_Stat.m_AllUipSucceeded, 1);
			assert_eq!(cls.size(), 2);
			assert!(cls[0] == t.E2I(-2) && cls[1] == t.E2I(-1));
			assert_eq!(clsBeforeAllUip.size(), 3);
		}
	}

	// ALL-UIP for the flipped clause: the counters per decision level cannot be allocated
	{
		let (_, mut t, mut ci, mut mainCls) = FlippedConflict(&[("/conflicts/flipped_drop_if_subsumed", 0.), ("/conflicts/all_uip/mode", 2.)]);
		// So that the only allocation would be the one of the counters
		t.m_RootedVars.reserve_exactly(16);
		assert!(!t.m_RootedVars.uninitialized_or_erroneous());
		alloc_fault::fail_after(0);
		let (flippedCls, flippedClsInd) = t.RecordFlipped(&mut ci, Span::new(mainCls.as_mut_ptr(), mainCls.len()));
		alloc_fault::never_fail();
		assert!(flippedCls.is_empty());
		assert_eq!(flippedClsInd, CTopi::<u32, false>::BadClsInd);
		assert_eq!(t.m_Status.get(), TToporStatus::STATUS_ALLOC_FAILED);
		assert!(t.GetStatusExplanation().contains("GetDecLevelsAndMarkInHugeCounter"));
		assert!(t.m_RootedVars.empty() && t.m_VisitedVars.empty());
	}

	// The same inside the conflict analysis loop, which gives up after the failed flipped recording
	{
		let (clss, _, _, _) = FlippedConflict(&[]);
		// No minimization: it allocates, but doesn't test its allocations
		let mut t = Prepared::<u32, false>(&[("/conflicts/flipped_drop_if_subsumed", 0.), ("/conflicts/all_uip/mode", 2.), ("/conflicts/minimize_clauses", 0.)], &clss, &[]);
		t.m_RootedVars.reserve_exactly(16);
		assert!(!DecideManually(&mut t, 1).IsContradiction());
		assert!(!DecideManually(&mut t, 2).IsContradiction());
		let mut ci = DecideManually(&mut t, 3);
		alloc_fault::fail_after(0);
		t.ConflictAnalysisLoop(&mut ci);
		alloc_fault::never_fail();
		assert_eq!(t.m_Status.get(), TToporStatus::STATUS_ALLOC_FAILED);
		assert!(t.GetStatusExplanation().contains("GetDecLevelsAndMarkInHugeCounter"));
		assert_eq!(t.m_Stat.m_Conflicts, 2);
		assert_eq!(t.m_Stat.m_FlippedClauses, 0);
		assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_MEM_OUT);
	}
}

// A DRAT sink, which fails: the solver moves to the DRAT-file-problem state
// The instance consists of binary clauses only, so that every learnt clause is short: a failure to write a long learnt clause makes AddClsToBufferAndWatch return a bad clause index,
// whose glue is then read by LearnAndUpdateHeuristics (asserted against; an upstream issue, which the fork reproduces)
#[test]
fn drat_bad_file()
{
	let mut clss: Vec<Vec<TLit>> = vec![];
	for i in 0..6
	{
		let (a, b) = (2 * i + 1, 2 * i + 2);
		clss.extend_from_slice(&[vec![-a, b], vec![-a, -b], vec![a, b]]);
	}
	for isDratBinary in [false, true]
	{
		let mut failures = 0;
		for failAfter in [0, 1, 2, 3, 5, 8, 12, 17, 1000000]
		{
			let sink = CSharedSink { m_FailAfter: Some(failAfter), ..Default::default() };
			let mut t = NewTopiSetUp::<u32, false>(&[("/decision/polarity/init_strategy", 1.), ("/decision/vsids/init_order", 1.)], &clss, |t| t.DumpDrat(Box::new(sink.clone()), isDratBinary, false));
			let ret = t.Solve(&[], Unlimited(), u64::MAX);
			if failAfter == 1000000
			{
				assert_eq!(ret, TToporReturnVal::RET_SAT);
				assert!(!t.IsError());
				assert!(sink.m_Buf.borrow().len() >= 6);
				continue;
			}
			assert!(sink.m_Buf.borrow().len() <= failAfter);
			if ret == TToporReturnVal::RET_SAT
			{
				// Only the comment about the completion of the query (if anything) didn't fit
				continue;
			}
			assert_eq!(ret, TToporReturnVal::RET_DRAT_FILE_PROBLEM);
			assert!(t.IsError());
			assert!(t.GetStatusExplanation().contains("Problem with DRAT file generation"));
			assert!(sink.m_Buf.borrow().len() <= failAfter);
			failures += 1;
			// Once the file is bad, nothing more is written
			let written = sink.m_Buf.borrow().len();
			t.NewLearntClsApplyCbLearntDrat(Span::empty());
			assert_eq!(sink.m_Buf.borrow().len(), written);
			assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_DRAT_FILE_PROBLEM);
		}
		assert!(failures >= 3);
	}
}

// Decodes a binary DRAT proof into its clauses
fn DecodeBinaryDrat(buf: &[u8]) -> Vec<Vec<TLit>>
{
	let mut res = vec![];
	let mut i = 0;
	while i < buf.len()
	{
		assert_eq!(buf[i], b'a');
		i += 1;
		let mut c = vec![];
		loop
		{
			let (mut ul, mut shift) = (0u64, 0);
			loop
			{
				let b = buf[i];
				i += 1;
				ul |= ((b & 0x7f) as u64) << shift;
				shift += 7;
				if b & 0x80 == 0
				{
					break;
				}
			}
			if ul == 0
			{
				break;
			}
			c.push(if ul & 1 == 1 { -((ul >> 1) as TLit) } else { (ul >> 1) as TLit });
		}
		res.push(c);
	}
	res
}

// Binary DRAT with literals, which require more than one byte; the proof is identical to the textual one and to the clauses reported by the callback
#[test]
fn drat_binary_text_and_callback_agree()
{
	use std::cell::RefCell;
	use std::rc::Rc;
	let clss = RandSat(41, 90, 420, 3);
	for sortEveryClause in [false, true]
	{
		let mut proofs: Vec<Vec<Vec<TLit>>> = vec![];
		for isDratBinary in [false, true]
		{
			let sink = CSharedSink::default();
			let learnts: Rc<RefCell<Vec<Vec<TLit>>>> = Rc::new(RefCell::new(vec![]));
			let learntsCb = learnts.clone();
			let mut t = NewTopiSetUp::<u32, false>(&[], &clss, |t|
			{
				t.DumpDrat(Box::new(sink.clone()), isDratBinary, sortEveryClause);
				t.SetCbNewLearntCls(Some(Box::new(move |c: &[TLit]|
				{
					learntsCb.borrow_mut().push(c.to_vec());
					TStopTopor::VAL_CONTINUE
				})));
			});
			let ret = SolveAndCheck(&mut t, &clss, &[], u64::MAX);
			assert_eq!(ret, RefVerdict(&clss, &[]));
			assert!(t.m_Stat.m_Conflicts > 10);
			let buf = sink.m_Buf.borrow().clone();
			let proof: Vec<Vec<TLit>> = if isDratBinary
			{
				DecodeBinaryDrat(&buf)
			}
			else
			{
				String::from_utf8(buf).unwrap().lines().filter(|l| !l.starts_with('c')).map(|l| l.split(' ').map(|x| x.parse::<TLit>().unwrap()).take_while(|x| *x != 0).collect()).collect()
			};
			assert_eq!(proof, *learnts.borrow());
			assert!(proof.iter().any(|c| c.iter().any(|l| l.abs() >= 64)));
			if sortEveryClause
			{
				assert!(proof.iter().all(|c| c.windows(2).all(|w| w[0].abs() < w[1].abs())));
			}
			proofs.push(proof);
		}
		assert_eq!(proofs[0], proofs[1]);
	}
}

// The new-learnt-clause callback requests to stop
// The solver isn't used after that: the interrupt takes effect in the middle of the conflict analysis loop, so a contradiction found by the BCP, which follows the learning, is lost
// (an upstream issue, which the fork reproduces: e.g., the pigeonhole instance below is reported satisfiable by a subsequent query)
#[test]
fn learnt_clause_callback_stops()
{
	let clss = Php(5, 4);
	for stopAfter in [0u32, 1, 3, 10]
	{
		let mut invs = 0;
		let mut t = NewTopiSetUp::<u32, false>(&[], &clss, |t| t.SetCbNewLearntCls(Some(Box::new(move |_c: &[TLit]|
		{
			invs += 1;
			if invs > stopAfter { TStopTopor::VAL_STOP } else { TStopTopor::VAL_CONTINUE }
		}))));
		assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
		assert!(!t.IsError());
		assert!(t.m_Stat.m_Conflicts >= 1 && t.m_Stat.m_Conflicts <= stopAfter as u64 + 1);
		assert!(t.GetStatusExplanation().is_empty());
	}
}

// Statistics are printed every conflict
#[test]
fn verbosity_print_out_every_conflict()
{
	let clss = Php(4, 3);
	let mut t = NewTopi::<u32, false>(&[("/verbosity/level", 1.), ("/verbosity/print_out_confs", 1.), ("/restarts/numeric/conflict_thr", 2.), ("/phase/restarts_block_size", 2.), ("/phase/unforce_restarts_fraction_init", 0.5)], &clss);
	assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_UNSAT);
}

// The clause counter of the on-the-fly subsumption wraps around
#[test]
fn on_the_fly_subsumption_cls_counter_wrap_around()
{
	let clss = Php(6, 5);
	let mut t = NewTopi::<u32, false>(ConfAnalysisCfgs[0], &clss);
	t.m_CurrClsCounter = i32::MAX - 3;
	assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_UNSAT);
	assert!(t.m_CurrClsCounter > 0 && t.m_CurrClsCounter < 100000);
	assert!(t.m_Stat.m_LitsRemovedByConfSubsumption > 0);
}

// A restart strategy, which is neither numeric nor LBD-based (impossible through the parameters), never restarts
#[test]
fn restart_unknown_strategy_from_init()
{
	let clss = Php(5, 4);
	let mut t = NewTopi::<u32, false>(&[], &clss);
	t.m_Params.m_ParamRestartStrategyInit.set(2);
	t.m_Params.m_ParamRestartStrategyS.set(2);
	t.m_Params.m_ParamRestartStrategyN.set(2);
	assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_UNSAT);
	assert_eq!(t.m_CurrRestartStrat, 2);
	assert_eq!(t.m_Stat.m_Restarts, 0);
}

// The decision heuristic has nothing to decide upon, when the heap is empty or contains only assigned variables
#[test]
fn decide_with_all_assigned()
{
	let clss = vec![vec![1, 2], vec![-1, 2], vec![3, -2]];
	let mut t = NewTopi::<u32, false>(&[], &clss);
	assert_eq!(SolveAndCheck(&mut t, &clss, &[], u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(t.Decide(), BadULit);
	assert!(t.m_VsidsHeap.empty());
}

// The polarity and score manipulations in an erroneous state are ignored
#[test]
fn polarity_and_score_api_when_unrecoverable()
{
	let mut t = NewTopi::<u32, false>(&[], &[vec![1, 2, 3]]);
	t.FixPolarity(1, false);
	// A new variable, which is beyond the polarity information
	t.CreateInternalLit(7);
	t.ClearUserPolarityInfo(7);
	assert!(!t.IsError());
	t.SetParam("/no/such/param", 1.);
	assert!(t.IsError() && t.IsUnrecoverable());
	t.BoostScore(2, 2.);
	t.FixPolarity(-2, true);
	t.FixPolarityInternal(GetLit(1, true), true);
	t.ClearUserPolarityInfo(1);
	t.CreateInternalLit(9);
	assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_PARAM_ERROR);
}

// The interface to a higher-level parallel solver: the unit clauses are reported, and the unit clauses of the other threads are received at restarts
#[test]
fn parallel_data_unit_clauses()
{
	use std::cell::RefCell;
	use std::rc::Rc;
	let clss = Php(5, 4);
	// 0: nothing to provide; 1: a few consistent units (some are provided twice); 2: units, which make the formula contradictory at level 0
	for kind in 0..3
	{
		let mut satClss = RandSat(51, 30, 105, 3);
		let reported: Rc<RefCell<Vec<(u32, TLit)>>> = Rc::new(RefCell::new(vec![]));
		let reportedCb = reported.clone();
		let cfg: TParams = &[("/mode/value", 0.), ("/restarts/numeric/conflict_thr", 1.)];
		let mut ref_ = NewTopi::<u32, false>(cfg, &satClss);
		assert_eq!(SolveAndCheck(&mut ref_, &satClss, &[], u64::MAX), TToporReturnVal::RET_SAT);
		let model: Vec<TLit> = (1..=30).map(|v| if ref_.GetValue(v) == TToporLitVal::VAL_SATISFIED { v } else { -v }).collect();
		// Block the reference model, so that there would be conflicts on the way to another one
		let mut units: Vec<TLit> = match kind
		{
			0 => vec![],
			1 => vec![model[0], model[3], model[0], model[7]],
			_ => vec![model[0], -model[0]],
		};
		units.reverse();
		let mut pending: Vec<TLit> = vec![];
		let mut t = NewTopiSetUp::<u32, false>(cfg, &satClss, |t| t.SetParallelData(7, Some(Box::new(move |threadId: u32, l: TLit| reportedCb.borrow_mut().push((threadId, l)))), Some(Box::new(move |threadId: u32, isFirst: bool|
		{
			assert_eq!(threadId, 7);
			if isFirst
			{
				pending = units.clone();
			}
			pending.pop().unwrap_or(0)
		}))));
		let ret = t.Solve(&[], Unlimited(), u64::MAX);
		assert!(t.m_Stat.m_Restarts > 0);
		if kind == 2
		{
			assert_eq!(ret, TToporReturnVal::RET_UNSAT);
		}
		else
		{
			assert_eq!(ret, TToporReturnVal::RET_SAT);
			for c in &satClss
			{
				assert!(c.iter().any(|l| t.GetValue(*l) == TToporLitVal::VAL_SATISFIED));
			}
			if kind == 1
			{
				assert!(t.GetValue(model[0]) == TToporLitVal::VAL_SATISFIED && t.GetValue(model[3]) == TToporLitVal::VAL_SATISFIED && t.GetValue(model[7]) == TToporLitVal::VAL_SATISFIED);
			}
		}
		assert!(reported.borrow().iter().all(|(threadId, _)| *threadId == 7));
		satClss.clear();

		// The unit clauses learnt by the solver itself are reported
		let reported: Rc<RefCell<Vec<(u32, TLit)>>> = Rc::new(RefCell::new(vec![]));
		let reportedCb = reported.clone();
		let mut t = NewTopiSetUp::<u32, false>(cfg, &clss, |t| t.SetParallelData(3, Some(Box::new(move |threadId: u32, l: TLit| reportedCb.borrow_mut().push((threadId, l)))), None));
		assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_UNSAT);
		assert!(!reported.borrow().is_empty());
		assert!(reported.borrow().iter().all(|(threadId, l)| *threadId == 3 && *l != 0 && l.abs() <= 20));
	}
}

// A solver, which is ready for driving the search manually through the internal functions:
// the initial query is stopped by the stop-now callback right before the first decision, so everything is initialized, and nothing is assigned beyond level 0
fn Prepared<Ind: TUInd, const Compress: bool>(params: TParams, clss: &[Vec<TLit>], extraVars: &[TLit]) -> CTopi<Ind, Compress>
{
	let mut t = NewTopiSetUp::<Ind, Compress>(params, clss, |t| t.SetCbStopNow(Some(Box::new(|| TStopTopor::VAL_STOP))));
	for v in extraVars
	{
		t.CreateInternalLit(*v);
	}
	assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	t.SetCbStopNow(None);
	t.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());
	assert_eq!(t.m_DecLevel, 0);
	t
}

// Assigns the literal with no parent at the current decision level or at a new one
fn AssignManually<Ind: TUInd, const Compress: bool>(t: &mut CTopi<Ind, Compress>, lExternal: TLit, newDecLevel: bool, toPropagate: bool)
{
	if newDecLevel
	{
		t.NewDecLevel();
	}
	let l = t.E2I(lExternal);
	let decLevel = t.m_DecLevel;
	let isContradictory = t.Assign(l, CTopi::<Ind, Compress>::BadClsInd, BadULit, decLevel, toPropagate, false);
	assert!(!isContradictory);
}

// A decision followed by BCP, as in the CDCL loop
fn DecideManually<Ind: TUInd, const Compress: bool>(t: &mut CTopi<Ind, Compress>, lExternal: TLit) -> TContradictionInfo<Ind>
{
	t.m_FlippedLit = BadULit;
	AssignManually(t, lExternal, true, true);
	t.BCP()
}

// Adds a learnt clause, which the search would have to be very lucky to learn at the right moment
// The glue of a learnt clause is calculated when it's added, so its literals are temporarily assigned: the first one is satisfied at the highest level, the rest are falsified
fn InjectLearnt<Ind: TUInd, const Compress: bool>(t: &mut CTopi<Ind, Compress>, cls: &[TLit]) -> Ind
{
	let decLevelBefore = t.m_DecLevel;
	for l in cls[1..].iter().rev()
	{
		if t.GetValue(*l) == TToporLitVal::VAL_UNASSIGNED
		{
			AssignManually(t, -*l, true, false);
		}
	}
	AssignManually(t, cls[0], true, false);
	let mut lits: Vec<TULit> = cls.iter().map(|l| t.E2I(*l)).collect();
	let clsInd = t.AddClsToBufferAndWatch(Span::new(lits.as_mut_ptr(), lits.len()), true, false);
	assert!(!t.IsError() && t.ClsGetIsLearnt(clsInd));
	t.Backtrack(decLevelBefore as TLit, false, false);
	clsInd
}

// Completes the query after the manual part and verifies the model
fn FinishManually<Ind: TUInd, const Compress: bool>(t: &mut CTopi<Ind, Compress>, clss: &[Vec<TLit>])
{
	assert!(!t.IsError(), "{}", t.GetStatusExplanation());
	t.Backtrack(0, false, false);
	assert_eq!(SolveAndCheck(t, clss, &[], u64::MAX), TToporReturnVal::RET_SAT);
}

const OtfBothCfg: TParams = &[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/contradicting_first_restart", 0.),
	("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 1000.), ("/conflicts/on_the_fly_subsumption/parent_first_restart", 0.)];
const OtfParentCfg: TParams = &[("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", 0.), ("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", 1000.),
	("/conflicts/on_the_fly_subsumption/parent_first_restart", 0.)];

// On-the-fly subsumption: a long initial parent is subsumed by the learnt contradicting clause, which has the same size (not counting the literals falsified at level 0), so the two are swapped
fn RunOtfLongInitParentSubsumedByLearntContradicting<Ind: TUInd, const Compress: bool>()
{
	// The parent has more than 3 literals: the pivot is removed from both the clauses, the parent replaces the contradicting clause
	{
		let clss = vec![vec![4, -1, -2, -3], vec![5, 6, 7]];
		let mut t = Prepared::<Ind, Compress>(OtfBothCfg, &clss, &[]);
		assert!(!DecideManually(&mut t, 1).IsContradiction());
		assert!(!DecideManually(&mut t, 2).IsContradiction());
		InjectLearnt(&mut t, &[-4, -3, -1, -2]);
		let mut ci = DecideManually(&mut t, 3);
		// The initial clause implies 4, which falsifies the learnt clause
		assert!(ci.IsContradiction() && !ci.m_IsContradictionInBinaryCls && t.ClsGetIsLearnt(ci.m_ParentClsInd));
		let before = t.m_Stat.m_LitsRemovedByConfSubsumption;
		t.ConflictAnalysisLoop(&mut ci);
		assert!(!ci.IsContradiction());
		// The pivot removed from the contradicting clause, the swap, and the 3 literals of the deleted clause
		assert_eq!(t.m_Stat.m_LitsRemovedByConfSubsumption - before, 5);
		assert_eq!(t.GetValue(-3), TToporLitVal::VAL_SATISFIED);
		if !Compress
		{
			FinishManually(&mut t, &clss);
		}
	}
	// The parent has 3 literals, while the contradicting clause has an additional literal falsified at level 0: the contradicting clause becomes a binary clause
	{
		let clss = vec![vec![4, -1, -2], vec![-5], vec![6, 7, 8]];
		let mut t = Prepared::<Ind, Compress>(OtfBothCfg, &clss, &[]);
		assert_eq!(t.GetValue(-5), TToporLitVal::VAL_SATISFIED);
		assert!(!DecideManually(&mut t, 1).IsContradiction());
		InjectLearnt(&mut t, &[-4, -2, -1, 5]);
		let mut ci = DecideManually(&mut t, 2);
		assert!(ci.IsContradiction() && !ci.m_IsContradictionInBinaryCls && t.ClsGetIsLearnt(ci.m_ParentClsInd));
		let before = t.m_Stat.m_LitsRemovedByConfSubsumption;
		t.ConflictAnalysisLoop(&mut ci);
		assert!(!ci.IsContradiction());
		assert!(t.m_Stat.m_LitsRemovedByConfSubsumption - before >= 5);
		assert_eq!(t.GetValue(-2), TToporLitVal::VAL_SATISFIED);
		if !Compress
		{
			FinishManually(&mut t, &clss);
		}
	}
}

#[test]
fn otf_long_init_parent_subsumed_by_learnt_contradicting()
{
	RunOtfLongInitParentSubsumedByLearntContradicting::<u32, false>();
	RunOtfLongInitParentSubsumedByLearntContradicting::<u64, false>();
	RunOtfLongInitParentSubsumedByLearntContradicting::<u64, true>();
}

// The conflict, whose analysis discovers that the parent (4 -2 -1 -3) of the pivot 4 is subsumed by the resolvent (-2 -1 -3)
fn ParentSubsumedConflict<Ind: TUInd, const Compress: bool>(params: TParams) -> (Vec<Vec<TLit>>, CTopi<Ind, Compress>, TContradictionInfo<Ind>)
{
	let clss = vec![vec![4, -2, -1, -3], vec![-4, -2, -1], vec![5, 6, 7]];
	let mut t = Prepared::<Ind, Compress>(params, &clss, &[]);
	assert!(!DecideManually(&mut t, 1).IsContradiction());
	assert!(!DecideManually(&mut t, 3).IsContradiction());
	let ci = DecideManually(&mut t, 2);
	assert!(ci.IsContradiction() && !ci.m_IsContradictionInBinaryCls);
	(clss, t, ci)
}

#[test]
fn otf_parent_subsumed_by_resolvent()
{
	let (clss, mut t, mut ci) = ParentSubsumedConflict::<u32, false>(OtfParentCfg);
	// The contradicting clause is the ternary one; 4 is implied by the long one
	assert_eq!(t.CiGetSpan(&mut ci, TUV::MAX).size(), 3);
	t.ConflictAnalysisLoop(&mut ci);
	assert!(!ci.IsContradiction());
	assert_eq!(t.m_Stat.m_LitsRemovedByConfSubsumption, 1);
	FinishManually(&mut t, &clss);
}

// The solver is already in an unrecoverable state, when the conflict is analyzed: every stage of the analysis gives up
// (in the solver itself the state may change in the middle of the analysis only because of an allocation failure)
#[test]
fn conflict_analysis_when_unrecoverable()
{
	let Fail = |t: &mut CTopi<u32, false>| t.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "Injected by the test".to_string());

	// On-the-fly subsumption of the parent
	{
		let (_, mut t, mut ci) = ParentSubsumedConflict::<u32, false>(OtfParentCfg);
		Fail(&mut t);
		let mut clsBeforeAllUip: CVector<TULit> = CVector::new(0, 0);
		let (_, clsInd) = t.LearnAndUpdateHeuristics(&mut ci, &mut clsBeforeAllUip);
		assert_eq!(clsInd, CTopi::<u32, false>::BadClsInd);
		assert_eq!(t.m_VarsParentSubsumed.len(), 1);
		assert_eq!(t.m_Stat.m_ActiveLongLearntClss, 0);
	}
	// Minimization (on and off)
	for minimize in [1., 0.]
	{
		let (_, mut t, mut ci) = ParentSubsumedConflict::<u32, false>(&[("/conflicts/minimize_clauses", minimize)]);
		Fail(&mut t);
		let mut clsBeforeAllUip: CVector<TULit> = CVector::new(0, 0);
		let (_, clsInd) = t.LearnAndUpdateHeuristics(&mut ci, &mut clsBeforeAllUip);
		assert_eq!(clsInd, CTopi::<u32, false>::BadClsInd);
		assert_eq!(t.m_Stat.m_ActiveLongLearntClss, 0);
		assert!(t.m_RootedVars.empty() && t.m_VisitedVars.empty());
		// The whole loop gives up too
		t.ConflictAnalysisLoop(&mut ci);
		assert!(ci.IsContradiction());
		// As in the solver, the clause is moved out of the solver, so that it doesn't alias it
		let mut c = std::mem::take(&mut t.m_HandyLitsClearBefore[0]);
		t.MinimizeClauseMinisat(&mut c);
		t.m_HandyLitsClearBefore[0] = c;
		assert!(t.m_RootedVars.empty());
	}
}

// Two conflicts at the same decision level: the second one is in the level flipped by the first one, so a flipped clause is recorded
// Returns the solver right before RecordFlipped is to be invoked for the second conflict along with the main clause of that conflict
fn FlippedConflict(params: TParams) -> (Vec<Vec<TLit>>, CTopi<u32, false>, TContradictionInfo<u32>, Vec<TULit>)
{
	let clss = vec![vec![-1, 9], vec![-1, 10], vec![-3, -2, 4], vec![-3, -2, -4], vec![3, 5, -2], vec![3, -5, -9, -10], vec![6, 7, 8]];
	let mut t = Prepared::<u32, false>(params, &clss, &[]);
	assert!(!DecideManually(&mut t, 1).IsContradiction());
	assert!(!DecideManually(&mut t, 2).IsContradiction());
	let mut ci = DecideManually(&mut t, 3);
	assert!(ci.IsContradiction());
	// One iteration of the conflict analysis loop
	let mut clsBeforeAllUip: CVector<TULit> = CVector::new(0, 0);
	let (cls, clsInd) = t.LearnAndUpdateHeuristics(&mut ci, &mut clsBeforeAllUip);
	assert_eq!(cls.size(), 2);
	assert_eq!(clsInd, CTopi::<u32, false>::BadClsInd);
	let (l0, l1) = (cls[0], cls[1]);
	assert!(l0 == t.E2I(-3) && l1 == t.E2I(-2));
	// Not a flipped level yet
	let (additionalCls, _) = t.RecordFlipped(&mut ci, cls);
	assert!(additionalCls.is_empty());
	let btLevel = t.GetAssignedDecLevel(l1);
	assert_eq!(btLevel, 2);
	t.Backtrack(btLevel as TLit, false, false);
	t.m_FlippedLit = l0;
	assert!(!t.Assign(l0, CTopi::<u32, false>::BadClsInd, l1, btLevel, true, false));
	ci = t.BCP();
	assert!(ci.IsContradiction() && !ci.m_IsContradictionInBinaryCls);
	let (mainCls, _) = t.LearnAndUpdateHeuristics(&mut ci, &mut clsBeforeAllUip);
	let mainCls: Vec<TULit> = mainCls.iter().collect();
	// The first UIP is the decision of the level
	assert_eq!(mainCls.len(), 3);
	assert!(mainCls.contains(&t.E2I(-2)) && mainCls.contains(&t.E2I(-9)) && mainCls.contains(&t.E2I(-10)));
	assert_eq!(t.m_FlippedLit, l0);
	(clss, t, ci, mainCls)
}

#[test]
fn flipped_recording_manual()
{
	// The flipped clause (3 -2 -9 -10) is recorded, when not dropped because of being subsumed by the main clause (-2 -9 -10)
	for (dropIfSubsumed, minimize) in [(0., 1.), (0., 0.), (1., 1.)]
	{
		let (_, mut t, mut ci, mut mainCls) = FlippedConflict(&[("/conflicts/flipped_drop_if_subsumed", dropIfSubsumed), ("/conflicts/minimize_clauses", minimize)]);
		let (flippedCls, flippedClsInd) = t.RecordFlipped(&mut ci, Span::new(mainCls.as_mut_ptr(), mainCls.len()));
		if dropIfSubsumed == 1.
		{
			assert!(flippedCls.is_empty());
			assert_eq!(t.m_Stat.m_FlippedClauses, 0);
		}
		else
		{
			let mut flipped: Vec<TULit> = flippedCls.iter().collect();
			flipped.sort();
			let mut expected = vec![t.E2I(3), t.E2I(-2), t.E2I(-9), t.E2I(-10)];
			expected.sort();
			assert_eq!(flipped, expected);
			assert!(t.ClsGetIsLearnt(flippedClsInd));
			assert_eq!(t.m_Stat.m_FlippedClauses, 1);
		}
		assert!(t.m_RootedVars.empty() && t.m_VisitedVars.empty());
	}

	// An unrecoverable state: the recording gives up after the minimization or after the binary minimization
	for minimize in [1., 0.]
	{
		let (_, mut t, mut ci, mut mainCls) = FlippedConflict(&[("/conflicts/flipped_drop_if_subsumed", 0.), ("/conflicts/minimize_clauses", minimize)]);
		t.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "Injected by the test".to_string());
		let (flippedCls, flippedClsInd) = t.RecordFlipped(&mut ci, Span::new(mainCls.as_mut_ptr(), mainCls.len()));
		assert_eq!(flippedClsInd, CTopi::<u32, false>::BadClsInd);
		assert!(minimize == 0. || flippedCls.is_empty());
		assert_eq!(t.m_Stat.m_FlippedClauses, 0);
		assert!(t.m_RootedVars.empty() && t.m_VisitedVars.empty());
	}

	// The same conflicts by the solver itself
	let (clss, _, _, _) = FlippedConflict(&[]);
	let mut t = Prepared::<u32, false>(&[("/conflicts/flipped_drop_if_subsumed", 0.)], &clss, &[]);
	assert!(!DecideManually(&mut t, 1).IsContradiction());
	assert!(!DecideManually(&mut t, 2).IsContradiction());
	let mut ci = DecideManually(&mut t, 3);
	t.ConflictAnalysisLoop(&mut ci);
	assert!(!ci.IsContradiction());
	assert_eq!(t.m_Stat.m_Conflicts, 2);
	assert_eq!(t.m_Stat.m_FlippedClauses, 1);
	FinishManually(&mut t, &clss);
}

// States, which the search itself never creates: several variables with no parent in one decision level
// The conflict analysis visits a variable with no parent (which isn't the first UIP) only in such a state
#[test]
fn conflict_analysis_visits_parentless_variables()
{
	// The main clause
	{
		let clss = vec![vec![-1, -2], vec![1, 3, 4], vec![2, 5, 6]];
		let mut t = Prepared::<u32, false>(&[], &clss, &[]);
		AssignManually(&mut t, 1, true, false);
		AssignManually(&mut t, 2, false, false);
		let mut ci = TContradictionInfo::<u32>::from_bin([t.E2I(-1), t.E2I(-2)]);
		let mut clsBeforeAllUip: CVector<TULit> = CVector::new(0, 0);
		let (cls, clsInd) = t.LearnAndUpdateHeuristics(&mut ci, &mut clsBeforeAllUip);
		assert_eq!(cls.size(), 1);
		assert_eq!(cls[0], t.E2I(-1));
		assert_eq!(clsInd, CTopi::<u32, false>::BadClsInd);
		assert_eq!(t.m_Stat.m_Conflicts, 1);
	}
	// The flipped clause
	{
		let clss = vec![vec![-2, -3], vec![1, 2, 4], vec![3, 5, 6]];
		let mut t = Prepared::<u32, false>(&[], &clss, &[]);
		AssignManually(&mut t, 1, true, false);
		AssignManually(&mut t, 2, false, false);
		AssignManually(&mut t, 3, false, false);
		t.m_FlippedLit = t.E2I(1);
		let mut ci = TContradictionInfo::<u32>::from_bin([t.E2I(-2), t.E2I(-3)]);
		let mut mainCls = [t.E2I(-2)];
		let (flippedCls, flippedClsInd) = t.RecordFlipped(&mut ci, Span::new(mainCls.as_mut_ptr(), 1));
		// Subsumed by the main clause
		assert!(flippedCls.is_empty());
		assert_eq!(flippedClsInd, CTopi::<u32, false>::BadClsInd);
		assert!(t.m_RootedVars.empty() && t.m_VisitedVars.empty());
	}
}

#[test]
fn mark_decisions_in_cone_for_a_decision()
{
	let clss = vec![vec![1, 2, 3], vec![-1, 4]];
	let mut t = Prepared::<u32, false>(&[], &clss, &[]);
	assert!(!DecideManually(&mut t, 1).IsContradiction());
	assert!(!DecideManually(&mut t, 2).IsContradiction());
	// A decision: nothing but the literal itself is marked
	let l = t.E2I(2);
	t.MarkDecisionsInConeAsVisited(l);
	assert_eq!(t.m_VisitedVars.size(), 1);
	assert!(t.IsVisited(l));
	t.CleanVisited();
	// An implied literal: the decisions in its cone are marked
	let l = t.E2I(4);
	t.MarkDecisionsInConeAsVisited(l);
	assert_eq!(t.m_VisitedVars.size(), 2);
	assert!(t.IsVisited(l) && t.IsVisited(t.E2I(1)) && !t.IsVisited(t.E2I(2)));
	t.CleanVisited();
	FinishManually(&mut t, &clss);
}

// AddClsToBufferAndWatch fails to allocate the watches or the clause buffer, or the index is too narrow
#[test]
fn add_cls_to_buffer_and_watch_failures()
{
	use crate::topor_dyn_array::alloc_fault;
	// The variable 4 has a watch list with some spare room; the watch lists of 8, 9 and 10 are empty
	let clss = vec![vec![1, 2, 3], vec![4, 5]];
	let BadClsInd = CTopi::<u32, false>::BadClsInd;
	// 0, 1: binary, the 1st/2nd watch fails; 2, 3: long, the 1st/2nd watch fails; 4: the clause buffer cannot be reallocated; 5: too many literals for the index
	for kind in 0..6
	{
		let mut t = Prepared::<u32, false>(&[], &clss, &[8, 9, 10]);
		let (l4, l8, l9, l10) = (t.E2I(4), t.E2I(8), t.E2I(9), t.E2I(10));
		let mut cls: Vec<TULit> = match kind
		{
			0 => vec![l8, l4],
			1 => vec![l4, l8],
			2 => vec![l8, l4, l9],
			3 => vec![l4, l8, l9],
			_ => vec![l8, l9, l10],
		};
		match kind
		{
			// No room left in the watch buffer
			0..=3 => t.m_WNext = t.m_W.cap() as u32,
			// No room left in the clause buffer
			4 => t.m_BNext = t.m_B.cap() as u32 - 2,
			_ => t.m_BNext = u32::MAX - 1,
		}
		let span = Span::new(cls.as_mut_ptr(), cls.len());
		alloc_fault::fail_after(0);
		let clsInd = t.AddClsToBufferAndWatch(span, false, false);
		alloc_fault::never_fail();
		assert_eq!(clsInd, BadClsInd);
		assert!(t.IsUnrecoverable());
		assert_eq!(t.m_Status.get(), if kind == 5 { TToporStatus::STATUS_INDEX_TOO_NARROW } else { TToporStatus::STATUS_ALLOC_FAILED });
		assert_eq!(t.Solve(&[], Unlimited(), u64::MAX), if kind == 5 { TToporReturnVal::RET_INDEX_TOO_NARROW } else { TToporReturnVal::RET_MEM_OUT });
	}
}

// The custom backtracking strategy is on, while the best score per decision level hasn't been allocated
// (impossible in the solver itself, where both depend on the same parameters of the same query)
#[test]
fn best_score_per_dec_level_late_allocation()
{
	use crate::topor_dyn_array::alloc_fault;
	let clss = vec![vec![1, 2, 3], vec![4, 5, 6]];
	let noCustomBt: TParams = &[("/backtracking/custom_bt_strat_init", 0.), ("/backtracking/custom_bt_strat_n", 0.), ("/backtracking/custom_bt_strat_s", 0.)];
	// Assign allocates
	{
		let mut t = Prepared::<u32, false>(noCustomBt, &clss, &[]);
		assert_eq!(t.m_BestScorePerDecLevel.cap(), 0);
		t.m_CurrCustomBtStrat = 1;
		assert!(!DecideManually(&mut t, 1).IsContradiction());
		assert!(t.m_BestScorePerDecLevel.cap() > 1 && !t.IsError());
		let v = GetVar(t.E2I(1));
		assert_eq!(t.m_BestScorePerDecLevel[1], t.m_VsidsHeap.get_var_score(v));
		// The score of an assigned variable is boosted
		t.UpdateScoreVar(v, 5.);
		assert_eq!(t.m_BestScorePerDecLevel[1], t.m_VsidsHeap.get_var_score(v));
		assert_eq!(t.CalcMaxDecLevelScore(1), t.m_BestScorePerDecLevel[1]);
	}
	// Assign fails to allocate
	{
		let mut t = Prepared::<u32, false>(noCustomBt, &clss, &[]);
		t.m_CurrCustomBtStrat = 1;
		t.NewDecLevel();
		let l = t.E2I(1);
		alloc_fault::fail_after(0);
		let isContradictory = t.Assign(l, CTopi::<u32, false>::BadClsInd, BadULit, 1, true, false);
		alloc_fault::never_fail();
		assert!(!isContradictory && t.IsUnrecoverable());
		assert_eq!(t.m_BestScorePerDecLevel.cap(), 0);
	}
	// UpdateScoreVar allocates or fails to allocate
	for fail in [false, true]
	{
		let mut t = Prepared::<u32, false>(noCustomBt, &clss, &[]);
		assert!(!DecideManually(&mut t, 1).IsContradiction());
		assert_eq!(t.m_BestScorePerDecLevel.cap(), 0);
		t.m_CurrCustomBtStrat = 1;
		let v = GetVar(t.E2I(1));
		if fail
		{
			alloc_fault::fail_after(0);
		}
		t.UpdateScoreVar(v, 5.);
		alloc_fault::never_fail();
		assert_eq!(t.IsUnrecoverable(), fail);
		assert_eq!(t.m_BestScorePerDecLevel.cap() == 0, fail);
		if !fail
		{
			assert_eq!(t.m_BestScorePerDecLevel[1], t.m_VsidsHeap.get_var_score(v));
		}
	}
}

#[test]
fn clear_user_polarity_info_when_not_activated()
{
	let mut t = Prepared::<u32, false>(&[], &[vec![1, 2, 3]], &[]);
	assert!(!t.m_PolarityInfoActivated);
	t.ClearUserPolarityInfoInternal(1);
	assert!(!t.m_PolarityInfoActivated && t.m_PolarityInfo.cap() == 0);
}

// Backtracking to a collapsed (empty) decision level, which is the level of the last assigned assumption
#[test]
fn backtrack_to_collapsed_assumption_level()
{
	let clss = vec![vec![1, 2, 3], vec![4, 5, 6]];
	let mut t = Prepared::<u32, false>(&[], &clss, &[]);
	assert!(!DecideManually(&mut t, 1).IsContradiction());
	assert!(!DecideManually(&mut t, 4).IsContradiction());
	assert!(!DecideManually(&mut t, 5).IsContradiction());
	// The 2nd level collapses
	let v = GetVar(t.E2I(4));
	t.UnassignVar(v);
	assert!(t.DecLevelIsCollapsed(2));
	t.m_DecLevelOfLastAssignedAssumption = 2;
	t.Backtrack(2, false, false);
	assert_eq!(t.m_DecLevel, 1);
	assert_eq!(t.m_DecLevelOfLastAssignedAssumption, 1);
	assert_eq!(t.GetValue(5), TToporLitVal::VAL_UNASSIGNED);
	assert_eq!(t.GetValue(1), TToporLitVal::VAL_SATISFIED);
	t.m_DecLevelOfLastAssignedAssumption = 0;
	FinishManually(&mut t, &clss);
}


