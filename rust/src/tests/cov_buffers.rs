// Targeted unit tests (see mod.rs): the clause buffers (the explicit one and the bit-compressed ones), clause deletion, simplification and compression,
// that is, topi_compression.rs, topi_bit_compression.rs and the inline members of the solver class (topi.rs)

use crate::topi::*;

// A deterministic pseudo-random generator (xorshift64*)
fn Rnd(x: &mut u64) -> u64
{
	*x ^= *x >> 12;
	*x ^= *x << 25;
	*x ^= *x >> 27;
	x.wrapping_mul(0x2545F4914F6CDD1D) >> 16
}

// A random k-SAT instance with clauses of lenMin..=lenMax distinct variables
fn RandomSat(seed: u64, vars: usize, clssNum: usize, lenMin: usize, lenMax: usize) -> Vec<Vec<TLit>>
{
	let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
	let mut clss = vec![];
	for _ in 0..clssNum
	{
		let len = lenMin + (Rnd(&mut x) as usize) % (lenMax - lenMin + 1);
		let mut c: Vec<TLit> = vec![];
		while c.len() < len
		{
			let v = 1 + (Rnd(&mut x) as usize % vars) as TLit;
			if !c.contains(&v) && !c.contains(&-v)
			{
				c.push(if Rnd(&mut x) & 1 == 0 { v } else { -v });
			}
		}
		clss.push(c);
	}
	clss
}

// Solves and verifies the model against the clauses and the assumptions, if satisfiable
fn SolveAndCheck<Ind: TUInd, const Compress: bool>(topi: &mut CTopi<Ind, Compress>, clss: &[Vec<TLit>], assumps: &[TLit], confThr: u64) -> TToporReturnVal
{
	let ret = topi.Solve(assumps, (f64::MAX, true), confThr);
	assert!(!topi.IsError(), "{}", topi.GetStatusExplanation());
	if ret == TToporReturnVal::RET_SAT
	{
		for c in clss
		{
			assert!(c.iter().any(|l| topi.GetValue(*l) == TToporLitVal::VAL_SATISFIED), "The model doesn't satisfy {:?}", c);
		}
		for a in assumps
		{
			assert!(topi.GetValue(*a) == TToporLitVal::VAL_SATISFIED);
		}
	}
	ret
}

fn AddClss<Ind: TUInd, const Compress: bool>(topi: &mut CTopi<Ind, Compress>, clss: &[Vec<TLit>])
{
	for c in clss
	{
		topi.AddUserClause(c);
	}
}

// Clause deletion (both the strategies, with and without glue clusters), simplification and compression of the buffers on an incremental random instance
fn DeletionSimplificationCompression<Ind: TUInd, const Compress: bool>()
{
	// (mode, strategy, clusters, activity decay, seed)
	for (mode, strategy, clusters, decay, seed) in [(1., 1., 0., 0.999, 1u64), (1., 1., 11., 0.999, 2), (1., 2., 0., 0.999, 3), (1., 2., 11., 0.999, 4), (0., 1., 11., 1e-15, 5), (5., 2., 3., 1e-15, 6), (7., 0., 0., 0.5, 7), (4., 1., 8., 0.9, 8), (8., 2., 0., 0.9, 9), (3., 1., 0., 0.9, 10), (6., 2., 5., 0.9, 11)]
	{
		let vars = 80;
		let mut clss = RandomSat(seed, vars, 336, 3, 3);
		let mut topi = CTopi::<Ind, Compress>::new(vars as TLit);
		topi.SetParam("/mode/value", mode);
		topi.SetParam("/deletion/clause/strategy", strategy);
		topi.SetParam("/deletion/clause/activity_decay", decay);
		topi.SetParam("/deletion/clause/trigger_init", 8.);
		topi.SetParam("/deletion/clause/trigger_linc", 4.);
		topi.SetParam("/deletion/clause/s1/trigger_max", if seed % 2 == 0 { 4. } else { 30. });
		topi.SetParam("/deletion/clause/s2/low_medium_inc", 3.);
		topi.SetParam("/deletion/clause/glue_never_delete", 1.);
		topi.SetParam("/deletion/clause/glue_clusters", clusters);
		topi.SetParam("/deletion/clause/glue_max_lex_cluster", 16.);
		topi.SetParam("/deletion/wasted_fraction_thr", 0.02);
		topi.SetParam("/debugging/assert_consistency", 2.);
		assert!(!topi.IsError());
		AddClss(&mut topi, &clss);
		topi.FixPolarity(3, false);
		topi.FixPolarity(-4, true);
		let mut x = seed + 100;
		for q in 0..6
		{
			// No assumptions together with the consistency assertions on and custom backtracking (see the upstream issues in fuzzing.rs)
			let assumps: Vec<TLit> = if q % 2 == 1 && mode == 1. { (0..3).map(|i| (1 + (Rnd(&mut x) as usize % vars) as TLit) * if i == 1 { -1 } else { 1 }).collect() } else { vec![] };
			let ret = SolveAndCheck(&mut topi, &clss, &assumps, 3000);
			assert!(ret == TToporReturnVal::RET_SAT || ret == TToporReturnVal::RET_UNSAT);
			// A few more clauses, including units, so that there is something to simplify
			let newClss = RandomSat(seed * 1000 + q, vars, 6, 1, 3);
			AddClss(&mut topi, &newClss);
			clss.extend(newClss);
		}
		assert!(strategy == 0. || topi.m_Stat.m_ClssDel > 0, "{}", seed);
	}
}

#[test]
fn deletion_simplification_compression()
{
	DeletionSimplificationCompression::<u32, false>();
	DeletionSimplificationCompression::<u64, false>();
	DeletionSimplificationCompression::<u64, true>();
}

// A random instance with clauses of lenMin..=lenMax literals, which is satisfied by the planted assignment (variable v is true iff v is even)
fn Planted(v: usize) -> TLit
{
	if v.is_multiple_of(2) { v as TLit } else { -(v as TLit) }
}

fn PlantedSat(seed: u64, vars: usize, clssNum: usize, lenMin: usize, lenMax: usize) -> Vec<Vec<TLit>>
{
	let mut clss = RandomSat(seed, vars, clssNum, lenMin, lenMax);
	for c in clss.iter_mut()
	{
		if !c.iter().any(|l| Planted(l.unsigned_abs() as usize) == *l)
		{
			c[0] = -c[0];
		}
	}
	clss
}

// Simplification: units are added between the queries, so that long clauses are shortened, become binary or get deleted, and variable indices are sifted
fn SimplificationPlanted<Ind: TUInd, const Compress: bool>(seed: u64, params: &[(&str, f64)], withAssumps: bool, withCb: bool)
{
	let vars = 60;
	let mut clss = PlantedSat(seed, vars, 150, 3, 12);
	clss.extend(PlantedSat(seed + 1, vars, 30, 2, 2));
	let mut topi = CTopi::<Ind, Compress>::new(0);
	for (name, val) in params
	{
		topi.SetParam(name, *val);
	}
	topi.SetParam("/debugging/assert_consistency", 2.);
	topi.SetParam("/deletion/wasted_fraction_thr", 0.05);
	assert!(!topi.IsError(), "{}", topi.GetStatusExplanation());
	if withCb
	{
		topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
	}
	AddClss(&mut topi, &clss);
	if withCb
	{
		topi.FixPolarity(5, false);
	}
	let mut x = seed.wrapping_mul(77) + 5;
	for q in 0..14
	{
		let assumps: Vec<TLit> = if withAssumps && q % 3 != 0 { (0..1 + q % 4).map(|_| Planted(1 + Rnd(&mut x) as usize % vars)).collect() } else { vec![] };
		topi.m_ImplicationsTillNextSimplify = 0;
		let ret = SolveAndCheck(&mut topi, &clss, &assumps, u64::MAX);
		assert_eq!(ret, TToporReturnVal::RET_SAT);
		// Backtracking to the global level, so that the next query would start there (otherwise, the trail is reused, and nothing is simplified in the absence of conflicts)
		if q % 5 != 4
		{
			topi.Backtrack(0, false, true);
		}
		// A few more units and binary clauses
		let mut newClss: Vec<Vec<TLit>> = (0..1 + Rnd(&mut x) % 4).map(|_| vec![Planted(1 + Rnd(&mut x) as usize % vars)]).collect();
		newClss.extend(PlantedSat(seed * 1000 + q, vars, 3, 2, 5));
		AddClss(&mut topi, &newClss);
		clss.extend(newClss);
	}
	assert!(topi.m_Stat.m_Simplifies > 3);
}

#[test]
fn simplification_planted()
{
	for seed in 1..=3
	{
		SimplificationPlanted::<u32, false>(seed, &[], seed != 1, seed == 3);
		SimplificationPlanted::<u64, false>(seed, &[("/assumptions/allow_reorder", 0.)], seed != 2, false);
		SimplificationPlanted::<u64, true>(seed, &[], seed != 1, seed == 3);
		SimplificationPlanted::<u64, true>(seed, &[("/assumptions/allow_reorder", 0.), ("/deletion/compress_allocated_per_watch", 0.)], true, false);
	}
	for mode in [0., 4., 5., 7., 8.]
	{
		SimplificationPlanted::<u32, false>(10 + mode as u64, &[("/mode/value", mode)], false, mode == 4.);
		SimplificationPlanted::<u64, true>(20 + mode as u64, &[("/mode/value", mode), ("/deletion/simplify_global_level_strat", 1.)], false, mode == 0.);
		SimplificationPlanted::<u64, false>(30 + mode as u64, &[("/mode/value", mode), ("/deletion/simplify_global_level_strat", 2.)], false, false);
	}
}

// The indices of all the non-deleted long clauses
fn AllClss<Ind: TUInd, const Compress: bool>(topi: &mut CTopi<Ind, Compress>, isLearntOnly: bool) -> Vec<Ind>
{
	let mut res = vec![];
	let mut clsInd = topi.ClsLoopFirst(isLearntOnly);
	while !topi.ClsLoopCompleted()
	{
		if !topi.ClsChunkDeleted(clsInd) && (!isLearntOnly || topi.ClsGetIsLearnt(clsInd))
		{
			res.push(clsInd);
		}
		clsInd = topi.ClsLoopNext();
	}
	res
}

fn AssignGlobally<Ind: TUInd, const Compress: bool>(topi: &mut CTopi<Ind, Compress>, l: TLit)
{
	let il = topi.E2I(l);
	assert!(!topi.IsAssigned(il));
	topi.Assign(il, CTopi::<Ind, Compress>::BadClsInd, BadULit, 0, false, false);
}

// Simplification over a crafted state: literals of variables, which appear in one clause only, are falsified globally (without propagation),
// while the clauses are satisfied by an assumption, so that the globally falsified literals are still watched
fn SimplifyCrafted<Ind: TUInd, const Compress: bool>()
{
	let clss: Vec<Vec<TLit>> = vec![vec![1, 2, 3, 4, 5], vec![6, 7, 8], vec![9, 10, 11], vec![12, 13, 14], vec![15, 16, 17, 18], vec![19, 20, 21, 22, 23, 24], vec![25, 26, 27], vec![-28, 29], vec![28, 30, 31]];
	let mut topi = CTopi::<Ind, Compress>::new(0);
	topi.SetParam("/debugging/assert_consistency", 2.);
	AddClss(&mut topi, &clss);
	let I2E = |topi: &CTopi<Ind, Compress>, il: TULit| -> TLit { (1..=31).flat_map(|v| [v, -v]).find(|l| topi.E2I(*l) == il).unwrap() };
	// For the first clauses, one of the watches becomes an assumption, while the other one is to be falsified globally
	let mut assumps: Vec<TLit> = vec![];
	let mut falsified: Vec<TLit> = vec![];
	for (i, clsInd) in AllClss(&mut topi, false).into_iter().enumerate().take(6)
	{
		let c = topi.Cls(clsInd);
		assumps.push(I2E(&topi, c.get(i % 2)));
		falsified.push(I2E(&topi, Negate(c.get(1 - i % 2))));
	}
	assert_eq!(SolveAndCheck(&mut topi, &clss, &assumps, u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topi.m_DecLevelOfLastAssignedAssumption, 6);
	topi.Backtrack(6, false, true);
	assert_eq!(topi.m_DecLevel, 6);
	for (i, clsInd) in AllClss(&mut topi, false).into_iter().enumerate().take(6)
	{
		let c = topi.Cls(clsInd);
		assert!(c.get(i % 2) == topi.E2I(assumps[i]) && c.get(1 - i % 2) == Negate(topi.E2I(falsified[i])));
	}
	for l in &falsified
	{
		AssignGlobally(&mut topi, *l);
	}
	// The solver itself never falsifies a watch at a level, which is lower than the level of the satisfied literals of the clause (BCP visits the clause right away),
	// so the consistency assertions are off for the simplification of the crafted state, which makes the state consistent again
	let simplifies = topi.m_Stat.m_Simplifies;
	topi.m_ImplicationsTillNextSimplify = 0;
	topi.SetParam("/debugging/assert_consistency", 0.);
	topi.SimplifyIfRequired();
	topi.SetParam("/debugging/assert_consistency", 2.);
	assert_eq!(topi.m_Stat.m_Simplifies, simplifies + 1);
	assert!(!topi.IsError());
	assert!(topi.WLAssertConsistency(true));

	if !Compress
	{
		// A deleted chunk, which the first-learnt-clause index points at
		let mut clsInd = topi.ClsLoopFirst(false);
		while !topi.ClsLoopCompleted() && !topi.ClsChunkDeleted(clsInd)
		{
			clsInd = topi.ClsLoopNext();
		}
		assert!(!topi.ClsLoopCompleted());
		assert!(topi.m_FirstLearntClsInd == Ind::MAX);
		topi.m_FirstLearntClsInd = clsInd;
		AssignGlobally(&mut topi, 29);
		topi.m_ImplicationsTillNextSimplify = 0;
		topi.SimplifyIfRequired();
		assert_eq!(topi.m_Stat.m_Simplifies, simplifies + 2);
		assert!(topi.m_FirstLearntClsInd != clsInd);
		topi.m_FirstLearntClsInd = Ind::MAX;
	}

	assert_eq!(SolveAndCheck(&mut topi, &clss, &assumps, u64::MAX), TToporReturnVal::RET_SAT);
	for l in falsified
	{
		assert!(topi.GetValue(l) == TToporLitVal::VAL_SATISFIED);
	}
}

// A long clause becomes binary during the simplification, but the watch buffer cannot be reallocated for the new binary watch
fn SimplifyCraftedAllocFailure<Ind: TUInd, const Compress: bool>()
{
	let clss: Vec<Vec<TLit>> = vec![vec![1, 2, 3], vec![4, 5, 6, 7]];
	let mut topi = CTopi::<Ind, Compress>::new(0);
	topi.SetParam("/multiplier/watches", 1.);
	AddClss(&mut topi, &clss);
	let all = AllClss(&mut topi, false);
	let c = topi.Cls(all[0]);
	let I2E = |topi: &CTopi<Ind, Compress>, il: TULit| -> TLit { (1..=7).flat_map(|v| [v, -v]).find(|l| topi.E2I(*l) == il).unwrap() };
	let (assump, falsified) = (I2E(&topi, c.get(0)), I2E(&topi, Negate(c.get(1))));
	assert_eq!(SolveAndCheck(&mut topi, &clss, &[assump], u64::MAX), TToporReturnVal::RET_SAT);
	topi.Backtrack(1, false, true);
	AssignGlobally(&mut topi, falsified);
	// No spare room in the watch buffer
	topi.CompressWLs();
	assert_eq!(topi.m_W.cap(), topi.m_WNext.as_usize());
	topi.m_ImplicationsTillNextSimplify = 0;
	super::containers::with_alloc_failure(0, || topi.SimplifyIfRequired());
	assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED, "{}", topi.GetStatusExplanation());
	assert_eq!(topi.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_MEM_OUT);
}

#[test]
fn simplify_crafted()
{
	SimplifyCrafted::<u32, false>();
	SimplifyCrafted::<u64, false>();
	SimplifyCrafted::<u64, true>();
	SimplifyCraftedAllocFailure::<u32, false>();
	SimplifyCraftedAllocFailure::<u64, false>();
	SimplifyCraftedAllocFailure::<u64, true>();
}

// An assumption becomes globally satisfied during the search (by a learnt unit clause): the simplification removes it from the list of the assumptions
fn SimplifyGlobalAssumps<Ind: TUInd, const Compress: bool>(allowReorder: bool, assumps: &[TLit])
{
	// 2 is decided first, which implies both 3 and 4, hence the unit clause -2 is learnt, which implies the assumption 1 globally
	let clss: Vec<Vec<TLit>> = vec![vec![1, 2], vec![-2, 3], vec![-2, 4], vec![-3, -4], vec![5, 6, 7], vec![8, 9, 10, 11], vec![2, 9]];
	let mut topi = CTopi::<Ind, Compress>::new(0);
	topi.SetParam("/debugging/assert_consistency", 2.);
	topi.SetParam("/assumptions/allow_reorder", allowReorder as u8 as f64);
	AddClss(&mut topi, &clss);
	topi.FixPolarity(2, false);
	topi.BoostScore(2, 1e10);
	assert_eq!(SolveAndCheck(&mut topi, &clss, assumps, u64::MAX), TToporReturnVal::RET_SAT);
	assert_eq!(topi.m_Stat.m_Conflicts, 1);
	assert!(topi.m_Stat.m_Simplifies > 0);
	assert_eq!(topi.GetLitDecLevel(1), 0);
	assert_eq!(SolveAndCheck(&mut topi, &clss, assumps, u64::MAX), TToporReturnVal::RET_SAT);
}

#[test]
fn simplify_global_assumps()
{
	for allowReorder in [false, true]
	{
		SimplifyGlobalAssumps::<u32, false>(allowReorder, &[1, 5]);
		SimplifyGlobalAssumps::<u64, false>(allowReorder, &[5, 1, 9, 8]);
		SimplifyGlobalAssumps::<u64, true>(allowReorder, &[8, 9, 5]);
		SimplifyGlobalAssumps::<u64, true>(allowReorder, &[1]);
	}
}

// Direct invocations of the functions, which the solver itself invokes only in states, where some of their branches are unreachable
fn CompressionDirect<Ind: TUInd, const Compress: bool>()
{
	let clss: Vec<Vec<TLit>> = vec![vec![1, 2, 3], vec![4, 5, 6, 7], vec![-1, 4], vec![2, 5], vec![8, 9, 10, 11, 12]];
	let NewSolver = || -> CTopi<Ind, Compress>
	{
		let mut topi = CTopi::<Ind, Compress>::new(12);
		AddClss(&mut topi, &clss);
		assert_eq!(SolveAndCheck(&mut topi, &clss, &[], u64::MAX), TToporReturnVal::RET_SAT);
		topi
	};

	// DeleteBinaryCls
	{
		let mut topi = NewSolver();
		topi.Backtrack(0, false, true);
		let mut binCls = [topi.E2I(-1), topi.E2I(4)];
		let binClss = topi.m_Stat.m_ActiveBinaryClss;
		assert!(topi.WLBinaryWatchExists(binCls[0], binCls[1]) && topi.WLBinaryWatchExists(binCls[1], binCls[0]));
		topi.DeleteBinaryCls(Span::from_slice(&mut binCls));
		assert!(!topi.WLBinaryWatchExists(binCls[0], binCls[1]) && !topi.WLBinaryWatchExists(binCls[1], binCls[0]));
		assert_eq!(topi.m_Stat.m_ActiveBinaryClss, binClss - 1);
		assert_eq!(SolveAndCheck(&mut topi, &clss[..2], &[], u64::MAX), TToporReturnVal::RET_SAT);
	}

	// MoveVarAndLitData for the first variable of the trail to an unassigned variable with watches (and then RemoveVarAndLitData of an unassigned variable)
	{
		let mut topi = NewSolver();
		topi.Backtrack(1, false, true);
		let vFrom = topi.m_TrailStart;
		assert!(vFrom != BadUVar && topi.m_TrailEnd == vFrom && topi.m_DecLevel == 1);
		let vTo = (1..=topi.m_LastExistingVar).find(|v| !topi.IsAssignedVar(*v) && !topi.m_Watches[GetLit(*v, false) as usize].IsEmpty() && !topi.m_Watches[GetLit(*v, true) as usize].IsEmpty()).unwrap();
		let wbInd = topi.m_Watches[GetLit(vTo, false) as usize].m_WBInd;
		assert!(!topi.WLChunkDeleted(wbInd));
		topi.MoveVarAndLitData(vFrom, vTo);
		assert!(topi.WLChunkDeleted(wbInd));
		assert!(topi.m_TrailStart == vTo && topi.m_TrailEnd == vTo && topi.m_TrailLastVarPerDecLevel[1] == vTo && topi.IsAssignedVar(vTo));
		let v = (1..=topi.m_LastExistingVar).find(|v| !topi.IsAssignedVar(*v) && !topi.m_Watches[GetLit(*v, false) as usize].IsEmpty()).unwrap();
		let wbInd = topi.m_Watches[GetLit(v, false) as usize].m_WBInd;
		topi.RemoveVarAndLitData(v);
		assert!(topi.WLChunkDeleted(wbInd));
	}

	// CompressWLs: an unrecoverable state and allocation failures
	{
		let mut topi = NewSolver();
		let wNext = topi.m_WNext;
		topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "test".to_string());
		topi.CompressWLs();
		assert!(topi.m_WNext == wNext);
		topi.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());
		super::containers::with_alloc_failure(0, || topi.CompressWLs());
		assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED);

		let mut topi = NewSolver();
		assert!(topi.m_W.cap() > 4 * topi.m_WNext.as_usize());
		super::containers::with_alloc_failure(1, || topi.CompressWLs());
		assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED);
	}

	// CompressBuffersIfRequired: the last clause, which the first-learnt-clause index points at, is deleted; an allocation failure
	for failAlloc in [false, true]
	{
		let mut topi = NewSolver();
		topi.SetParam("/deletion/wasted_fraction_thr", 0.001);
		topi.Backtrack(0, false, true);
		let all = AllClss(&mut topi, false);
		assert_eq!(all.len(), 3);
		if !Compress
		{
			topi.m_FirstLearntClsInd = all[2];
		}
		topi.DeleteCls(all[2], None);
		if failAlloc
		{
			if !Compress
			{
				super::containers::with_alloc_failure(0, || topi.CompressBuffersIfRequired());
				assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED);
			}
		}
		else
		{
			topi.CompressBuffersIfRequired();
			assert!(topi.m_BWasted == Ind::ZERO && topi.m_FirstLearntClsInd == Ind::MAX);
			assert_eq!(AllClss(&mut topi, false).len(), 2);
			assert_eq!(SolveAndCheck(&mut topi, &clss[..4], &[], u64::MAX), TToporReturnVal::RET_SAT);
		}
	}
}

#[test]
fn compression_direct()
{
	CompressionDirect::<u32, false>();
	CompressionDirect::<u64, false>();
	CompressionDirect::<u64, true>();
}

// Direct invocations of clause deletion with crafted triggers
fn DeletionDirect<Ind: TUInd, const Compress: bool>()
{
	let vars = 80;
	let clss = RandomSat(21, vars, 340, 3, 3);
	let NewSolver = |strategy: f64| -> CTopi<Ind, Compress>
	{
		let mut topi = CTopi::<Ind, Compress>::new(vars as TLit);
		topi.SetParam("/deletion/clause/strategy", strategy);
		topi.SetParam("/deletion/clause/glue_never_delete", 0.);
		topi.SetParam("/deletion/clause/trigger_linc", 4e9);
		AddClss(&mut topi, &clss);
		assert_eq!(topi.Solve(&[], (f64::MAX, true), 30), TToporReturnVal::RET_CONFLICT_OUT);
		assert!(topi.m_Stat.m_ActiveLongLearntClss > 5);
		topi
	};

	// Strategy 2: the next trigger doesn't fit into 64 bits
	{
		let mut topi = NewSolver(2.);
		topi.m_Stat.m_Conflicts = 1u64 << 40;
		*topi.m_ClsDelInfo.m_CurrChange() = 1;
		topi.m_ClsDelInfo.m_TriggerNext = 0;
		topi.DeleteClausesIfRequired();
		assert_eq!(topi.m_Stat.m_ClssDel, 1);
		assert_eq!(topi.m_ClsDelInfo.m_TriggerNext, u64::MAX);
	}

	// Strategy 1: the first-learnt-clause index points beyond the buffer; an allocation failure
	{
		let mut topi = NewSolver(1.);
		topi.m_ClsDelInfo.m_TriggerNext = 0;
		super::containers::with_alloc_failure(0, || topi.DeleteClausesIfRequired());
		assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED);
		topi.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());
		if !Compress
		{
			topi.m_FirstLearntClsInd = topi.m_BNext;
			topi.m_ClsDelInfo.m_TriggerNext = 0;
			topi.DeleteClausesIfRequired();
			assert!(topi.m_FirstLearntClsInd == Ind::MAX);
		}
	}
}

#[test]
fn deletion_direct()
{
	DeletionDirect::<u32, false>();
	DeletionDirect::<u64, false>();
	DeletionDirect::<u64, true>();
}

fn ResetStatus<Ind: TUInd, const Compress: bool>(topi: &CTopi<Ind, Compress>)
{
	topi.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());
}

// Runs f, which is expected to fail an assertion of the solver; the assertions of the solver are debug assertions (as in C++ with NDEBUG), so without them f is expected to run to its end
fn ExpectAssertionFailure(f: impl FnOnce())
{
	assert_eq!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err(), cfg!(debug_assertions));
}

// The inline members of the solver class over the state after an interrupted query: the explicit clause buffer
fn InlineMembersStandard<Ind: TUInd>()
{
	let vars = 80;
	let clss = RandomSat(21, vars, 340, 3, 3);
	let mut topi = CTopi::<Ind, false>::new(vars as TLit);
	topi.SetCbNewLearntCls(Some(Box::new(|_c: &[TLit]| TStopTopor::VAL_CONTINUE)));
	topi.SetCbStopNow(Some(Box::new(|| TStopTopor::VAL_CONTINUE)));
	topi.DumpDrat(Box::new(std::io::sink()), false, true);
	AddClss(&mut topi, &clss);
	topi.FixPolarity(1, false);
	assert_eq!(topi.Solve(&[], (f64::MAX, true), 30), TToporReturnVal::RET_CONFLICT_OUT);
	assert_eq!(topi.GetStatusExplanation(), "");
	assert!(topi.IsVsidsInitOrderParam("/decision/vsids/init_order") && topi.IsMultiplierParam("/multiplier/clauses") && !topi.IsMultiplierParam("/mode/value"));

	// A learnt clause
	let learnts = AllClss(&mut topi, true);
	assert!(learnts.len() > 5);
	let li = *learnts.iter().find(|li| topi.ClsGetSize(**li) > 3).unwrap();
	let sz = topi.ClsGetSize(li);
	let glue = topi.ClsGetGlue(li);
	topi.ClsSetGlue(li, ClsMaxGlue + 5);
	assert_eq!(topi.ClsGetGlue(li), ClsMaxGlue);
	topi.ClsSetGlue(li, glue);
	assert_eq!(topi.ClsGetSize(li), sz);
	assert_eq!(topi.ClsGetActivityAndSkipdel(li).as_u64() as u32 & ClsNotSkipdelMask, topi.ClsGetActivity(li).to_bits());
	assert_eq!(topi.ClsConstSnapshotDense(li, 2).size(), 2);
	let snapshot = topi.ClsConstSnapshotDense(li, TUV::MAX);
	assert_eq!(snapshot.size(), sz as usize);
	assert_eq!(snapshot.span().to_vec(), topi.ConstClsSpan(li, TUV::MAX).to_vec());
	assert_eq!(topi.ConstClsSpanDebug(li, 2).to_vec(), topi.ConstClsSpan(li, 2).to_vec());
	drop(snapshot);

	// Contradiction info
	let c = topi.ConstClsSpan(li, TUV::MAX).to_vec();
	let mut ciBin = TContradictionInfo::<Ind>::from_bin([c[0], c[1]]);
	let mut ciLong = TContradictionInfo::<Ind>::from_parent(li);
	let mut ciNone = TContradictionInfo::<Ind>::new();
	assert_eq!(topi.CiGetSpan(&mut ciBin, 1).to_vec(), vec![c[0]]);
	assert_eq!(topi.CiGetSpan(&mut ciBin, 5).to_vec(), vec![c[0], c[1]]);
	assert_eq!(topi.CiGetSpanDebug(&mut ciBin, 2).to_vec(), vec![c[0], c[1]]);
	assert_eq!(topi.CiGetSpan(&mut ciLong, TUV::MAX).to_vec(), c);
	assert_eq!(topi.CiGetSpanDebug(&mut ciLong, 3).to_vec(), c[..3].to_vec());
	assert_eq!((topi.CiGetSize(&ciBin), topi.CiGetSize(&ciLong)), (2, sz as usize));
	assert_eq!(topi.CiString(&mut ciNone), "");
	assert_eq!(topi.CiString(&mut ciBin), topi.SLit(c[0]) + " " + &topi.SLit(c[1]));
	assert_eq!(topi.CiString(&mut ciLong), topi.SLits(topi.Cls(li), false));
	let mut cis = [ciBin, ciLong];
	assert_eq!(topi.CisString(Span::from_slice(&mut cis)), topi.CiString(&mut ciBin) + "; " + &topi.CiString(&mut ciLong) + "; ");

	// Printing external literals: sorted and non-sorted
	let mut c2 = [c[0], c[1]];
	let (e0, e1) = (topi.GetExternalLit(c[0]), topi.GetExternalLit(c[1]));
	assert!(topi.E2I(e0) == c[0] && topi.E2I(e1) == c[1]);
	let sorted = if e0.abs() < e1.abs() { format!("{} {}", e0, e1) } else { format!("{} {}", e1, e0) };
	assert_eq!(topi.SLits(Span::from_slice(&mut c2), true), sorted);
	topi.m_DratSortEveryClause = false;
	assert_eq!(topi.SLits(Span::from_slice(&mut c2), true), format!("{} {}", e0, e1));
	topi.m_DratSortEveryClause = true;

	// The trail
	assert!(!topi.DecLevelIsCollapsed(0));
	assert!(topi.m_TrailStart != BadUVar && topi.m_DecLevel > 0);
	let mut assigned: Vec<TULit> = vec![];
	let mut v = topi.m_TrailStart;
	while v != BadUVar
	{
		let l = topi.GetAssignedLitForVar(v);
		assigned.push(l);
		assert_eq!(topi.GetDecLevel0ForUnassigned(l), topi.GetAssignedDecLevel(l));
		assert!(topi.GetAssignedParentClsInd(l) == topi.m_VarInfo[v as usize].m_ParentClsInd);
		assert_eq!(topi.IsAssignedAndDecVar(v), topi.IsAssignedDecVar(v));
		let (ai, vi) = (topi.m_AssignmentInfo[v as usize], topi.m_VarInfo[v as usize]);
		assert!(!topi.IsParentLongInitial(ai, vi) || !topi.ClsGetIsLearnt(vi.m_ParentClsInd));
		v = topi.GetTrailNextVar(v);
	}
	let unassignedVar = (1..=topi.m_LastExistingVar).find(|v| !topi.IsAssignedVar(*v)).unwrap();
	assert_eq!(topi.GetDecLevel0ForUnassigned(GetLit(unassignedVar, false)), 0);
	assert!(!topi.IsAssignedAndDecVar(unassignedVar));
	let assignedSpan = Span::from_slice(&mut assigned);
	let lowestIt = topi.GetAssignedLitsLowestDecLevelIt(assignedSpan, 0);
	assert!(assigned.iter().all(|l| topi.GetAssignedDecLevel(*l) >= topi.GetAssignedDecLevel(assigned[lowestIt])));
	let mut mixed = [GetLit(unassignedVar, false), *assigned.last().unwrap()];
	assert_eq!(topi.GetLitsHighestDecLevel0ForUnassignedIt(Span::from_slice(&mut mixed), 0), 1);

	// The glue: the counter of the marked decision levels wraps around; the assumptions are ignored
	let glueAll = topi.GetGlueAndMarkCurrDecLevels(assignedSpan);
	topi.m_MarkedDecLevelsCounter = TCounterType::MAX;
	assert_eq!(topi.GetGlueAndMarkCurrDecLevels(assignedSpan), glueAll);
	assert_eq!(topi.m_MarkedDecLevelsCounter, 1);
	let lastVar = GetVar(*assigned.last().unwrap());
	topi.m_Params.m_ParamAssumpsIgnoreInGlue.set(true);
	topi.m_AssignmentInfo[lastVar as usize].set_m_IsAssump(true);
	assert!(topi.GetGlueAndMarkCurrDecLevels(Span::from_slice(&mut [assigned[assigned.len() - 1]])) == 0);
	topi.m_AssignmentInfo[lastVar as usize].set_m_IsAssump(false);
	topi.m_Params.m_ParamAssumpsIgnoreInGlue.set(false);

	// Visited and rooted
	topi.MarkVisitedVar(unassignedVar);
	topi.AssumpUnsatCoreCleanUpIfRequired();
	assert!(topi.IsVisitedVar(unassignedVar));
	topi.m_LatestAssumpUnsatCoreSolveInvocation = topi.m_Stat.m_SolveInvs;
	topi.AssumpUnsatCoreCleanUpIfRequired();
	assert!(!topi.IsVisitedVar(unassignedVar) && topi.m_VisitedVars.empty());
	topi.m_LatestAssumpUnsatCoreSolveInvocation = u64::MAX;
	topi.MarkRootedVar(unassignedVar);
	assert!(topi.IsRootedVar(unassignedVar));
	assert_eq!(topi.RootedPopBack(), unassignedVar);
	assert!(!topi.IsRootedVar(unassignedVar) && topi.m_RootedVars.empty());

	// Forced polarities
	let v1 = GetVar(topi.E2I(1));
	assert!(topi.IsForced(v1) && !topi.IsNotForced(v1));

	// The query type and the phase stage
	let queryCurr = topi.m_QueryCurr;
	topi.m_QueryCurr = TQueryType::QUERY_INC_NORMAL;
	assert_eq!(topi.GetCurrUnforceRestartsFraction(), topi.m_Params.m_ParamPhaseMngUnforceRestartsFractionN.get());
	assert_eq!(topi.GetGlueMinFreeze(), topi.m_Params.m_ParamClsDelLowMinGlueFreezeAi.get());
	topi.m_QueryCurr = TQueryType::QUERY_INC_SHORT;
	assert_eq!(topi.GetCurrUnforceRestartsFraction(), topi.m_Params.m_ParamPhaseMngUnforceRestartsFractionS.get());
	assert_eq!(topi.GetGlueMinFreeze(), topi.m_Params.m_ParamClsDelLowMinGlueFreezeS.get());
	topi.m_QueryCurr = TQueryType::QUERY_INIT;
	assert_eq!(topi.GetCurrUnforceRestartsFraction(), topi.m_Params.m_ParamPhaseMngUnforceRestartsFractionInit.get());
	assert_eq!(topi.GetGlueMinFreeze(), topi.m_Params.m_ParamClsDelLowMinGlueFreeze.get());
	topi.m_QueryCurr = queryCurr;
	assert_eq!(topi.GetPhaseStageStr(), "Standard");
	topi.m_PhaseStage = TPhaseStage::PHASE_STAGE_DONT_FORCE;
	assert_eq!(topi.GetPhaseStageStr(), "Don't-force");
	assert!(!topi.IsForced(v1) && topi.IsNotForced(v1));
	topi.m_PhaseStage = TPhaseStage::PHASE_STAGE_STANDARD;

	// Statistics
	let bi = topi.GetStatBufferInfo();
	assert!(bi.m_b_num == 1 && bi.m_b_cap == topi.m_B.cap() && bi.m_b_size == topi.m_BNext.as_usize());
	assert!(topi.StatStrShort(true).contains("c ToporStt "));

	// Interrupt
	let handle = topi.GetInterruptNowHandle();
	topi.InterruptNow();
	assert!(handle.load(std::sync::atomic::Ordering::Relaxed));
	assert_eq!(topi.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_USER_INTERRUPT);
	assert!(!handle.load(std::sync::atomic::Ordering::Relaxed));

	// Additional assignments (of flipped clauses)
	topi.Backtrack(0, false, true);
	let mut unassigned: Vec<TULit> = (1..=topi.m_LastExistingVar).filter(|v| !topi.IsAssignedVar(*v)).map(|v| GetLit(v, false)).collect();
	let flippedUnits = topi.m_Stat.m_FlippedClausesUnit;
	topi.AdditionalAssign(Span::from_slice(&mut unassigned[..1]), CTopi::<Ind, false>::BadClsInd);
	assert!(topi.IsSatisfied(unassigned[0]) && topi.GetAssignedDecLevel(unassigned[0]) == 0 && topi.m_Stat.m_FlippedClausesUnit == flippedUnits + 1);
	topi.AdditionalAssign(Span::from_slice(&mut unassigned[..2]), CTopi::<Ind, false>::BadClsInd);
	assert!(GetVar(unassigned[0]) > GetVar(unassigned[1]) && !topi.IsAssigned(unassigned[0]) && topi.m_Stat.m_FlippedClausesUnit == flippedUnits + 1);
}

#[test]
fn inline_members_standard()
{
	InlineMembersStandard::<u32>();
	InlineMembersStandard::<u64>();
}

// Oversized learnt clauses (whose size doesn't fit into the header together with the glue): the header is crafted directly in the buffer
fn Oversized<Ind: TUInd>()
{
	let mut topi = CTopi::<Ind, false>::new(10);
	let clsInd = Ind::from_usize(topi.m_BNext.as_usize() + 64);
	assert!(!topi.EClsGetIsOversized(clsInd));
	topi.EClsSetIsLearnt(clsInd, true);
	assert!(topi.ClsGetIsLearnt(clsInd) && !topi.EClsGetIsOversized(clsInd) && !topi.EClsGetIsOversizedAssumeLearnt(clsInd));
	let hugeSize = ClsLearntMaxSizeWithGlue + 1;
	topi.ClsSetSize(clsInd, hugeSize);
	assert!(topi.m_AnyOversized);
	assert!(topi.EClsGetIsOversized(clsInd) && topi.EClsGetIsOversizedAssumeLearnt(clsInd));
	assert_eq!(topi.EClsGetStandardActivityAsInt(clsInd), ClsOversizeActivity);
	assert_eq!(topi.ClsGetSize(clsInd), hugeSize);
	// The glue of an oversized clause is its size
	assert_eq!(topi.ClsGetGlue(clsInd), hugeSize);
	topi.ClsSetGlue(clsInd, hugeSize);
	assert_eq!(topi.ClsGetGlue(clsInd), hugeSize);
	assert_eq!(topi.EClsLitsStartOffset(true, true), 3);
	assert_eq!(topi.EClsGetActivityAndSkipdelIndex(clsInd), clsInd.as_usize() + 2);
	topi.ClsSetActivityAndSkipdelTo0(clsInd);
	topi.ClsSetActivity(clsInd, 2.5);
	topi.ClsSetSkipdel(clsInd, true);
	assert!(topi.ClsGetActivity(clsInd) == 2.5 && topi.ClsGetSkipdel(clsInd));
	assert_eq!(topi.ClsEnd(clsInd).as_usize(), clsInd.as_usize() + 3 + hugeSize as usize);
	assert!(!topi.ClsChunkDeleted(clsInd));

	// An oversized-layout clause with 4 literals
	topi.m_B[clsInd.as_usize()] = ClsIsLearntMask | 4;
	for i in 0..4
	{
		topi.m_B[clsInd.as_usize() + 3 + i] = 10 + i as TULit;
	}
	assert_eq!(topi.ClsGetSize(clsInd), 4);
	assert_eq!(topi.Cls(clsInd).span().to_vec(), vec![10, 11, 12, 13]);
	assert_eq!(topi.ConstClsSpan(clsInd, TUV::MAX).to_vec(), vec![10, 11, 12, 13]);
	assert_eq!(topi.ClsConstSnapshotDense(clsInd, 3).span().to_vec(), vec![10, 11, 12]);
	assert_eq!(topi.ClsConstSnapshotDense(clsInd, TUV::MAX).span().to_vec(), vec![10, 11, 12, 13]);

	// Not an oversized clause: the same accessors
	topi.EClsSetStandardActivityAsInt(clsInd, 0);
	assert!(!topi.EClsGetIsOversized(clsInd) && topi.EClsGetStandardActivityAsInt(clsInd) == 0);
	let activityEntry = topi.m_B[clsInd.as_usize() + 2];
	assert_eq!(topi.ClsConstSnapshotDense(clsInd, TUV::MAX).span().to_vec(), vec![activityEntry, 10, 11, 12]);
	topi.EClsSetIsLearnt(clsInd, false);
	assert_eq!(topi.ClsConstSnapshotDense(clsInd, 2).span().to_vec(), vec![0, activityEntry]);
}

#[test]
fn oversized()
{
	Oversized::<u32>();
	Oversized::<u64>();
}

// Removing literals from clauses (as the on-the-fly subsumption does)
fn DeleteLits<Ind: TUInd, const Compress: bool>()
{
	let clss: Vec<Vec<TLit>> = vec![vec![1, 2, 3, 4, 5, 6, 7, 8], vec![9, 10, 11, 12, 13], vec![14, 15, 16]];
	let mut topi = CTopi::<Ind, Compress>::new(16);
	let learnts = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Vec<TLit>>::new()));
	let learntsCb = learnts.clone();
	topi.SetCbNewLearntCls(Some(Box::new(move |c: &[TLit]|
	{
		learntsCb.borrow_mut().push(c.to_vec());
		TStopTopor::VAL_CONTINUE
	})));
	AddClss(&mut topi, &clss);
	assert_eq!(SolveAndCheck(&mut topi, &clss, &[], u64::MAX), TToporReturnVal::RET_SAT);
	topi.Backtrack(0, false, true);
	let all = AllClss(&mut topi, false);
	assert_eq!(all.len(), 3);

	let Delete = |topi: &mut CTopi<Ind, Compress>, clsInd: &mut Ind, litInd: usize|
	{
		let before = topi.ConstClsSpan(*clsInd, TUV::MAX).to_vec();
		let l = before[litInd];
		let wasted = topi.m_BWasted;
		topi.DeleteLitFromCls(clsInd, l);
		let after = topi.ConstClsSpan(*clsInd, TUV::MAX).to_vec();
		assert_eq!(after.len(), before.len() - 1);
		assert!(!after.contains(&l) && after.iter().all(|a| before.contains(a)));
		assert!(topi.m_BWasted > wasted);
		// The shortened clause is reported
		let mut reported = learnts.borrow_mut().pop().unwrap();
		let mut expected: Vec<TLit> = after.iter().map(|a| topi.GetExternalLit(*a)).collect();
		reported.sort();
		expected.sort();
		assert_eq!(reported, expected);
		assert!(topi.WLAssertConsistency(false));
	};

	// A non-watched literal, then the watches (in the explicit buffer, the deleted chunk after the clause is extended every time)
	let first = *all.iter().find(|clsInd| topi.ClsGetSize(**clsInd) == 8).unwrap();
	let second = *all.iter().find(|clsInd| topi.ClsGetSize(**clsInd) == 5).unwrap();
	let mut clsInd = first;
	for litInd in [5, 0, 1, 3]
	{
		Delete(&mut topi, &mut clsInd, litInd);
	}
	assert_eq!(topi.ClsGetSize(clsInd), 4);
	// In the compressed mode, the clause is moved to another buffer
	let mut clsInd = second;
	Delete(&mut topi, &mut clsInd, 1);
	assert!(Compress == (clsInd != second));
	assert!(!topi.IsError());
	assert!(topi.DebugAssertWaste());
	assert_eq!(topi.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_SAT);
}

#[test]
fn delete_lits()
{
	DeleteLits::<u32, false>();
	DeleteLits::<u64, false>();
	DeleteLits::<u64, true>();
}

// The clause accessors over bit-compressed clauses, which are compressed directly
#[test]
fn inline_members_compressed()
{
	type TTopi = CTopi<u64, true>;
	let mut topi = TTopi::new(10);

	// A learnt clause of 4 literals: 1 bit for the clause size, hence 3 bits for the glue, which is truncated
	let mut lits4: [TULit; 4] = [9, 5, 6, 7];
	let mut spareUsed = true;
	let bc = topi.BCCompress(Span::from_slice(&mut lits4), true, 10000, true, 1.5, false, Some(&mut spareUsed));
	assert!(!bc.IsError() && !spareUsed && bc.m_IsLearnt() && bc.m_BitsForClsSize() == 1 && bc.m_BitsForLit() == 4);
	let ind = bc.ind();
	assert_eq!(bc.GetHashId().MaxGlue(), 7);
	assert!(topi.ClsGetIsLearnt(ind) && topi.ClsGetSize(ind) == 4 && topi.ClsGetGlue(ind) == 7 && topi.ClsGetSkipdel(ind) && topi.ClsGetActivity(ind) == 1.5);
	assert_eq!(topi.BCGetEncodedClsSize(ind), 1);
	assert_eq!(topi.ClsGetActivityAndSkipdel(ind), 1 | ((1.5f32.to_bits() as u64) << 1));
	topi.ClsSetGlue(ind, 3);
	assert_eq!(topi.ClsGetGlue(ind), 3);
	topi.ClsSetGlue(ind, 100);
	assert_eq!(topi.ClsGetGlue(ind), 7);
	topi.ClsSetActivityAndSkipdelTo0(ind);
	assert!(topi.ClsGetActivityAndSkipdel(ind) == 0 && !topi.ClsGetSkipdel(ind) && topi.ClsGetActivity(ind) == 0. && topi.ClsGetGlue(ind) == 7);
	assert_eq!(topi.ClsConstSnapshotDense(ind, 2).span().to_vec(), vec![9, 5]);
	assert_eq!(topi.ClsConstSnapshotDense(ind, TUV::MAX).span().to_vec(), lits4.to_vec());
	assert_eq!(topi.ConstClsSpan(ind, TUV::MAX).to_vec(), lits4.to_vec());
	assert_eq!(topi.ConstClsSpanDebug(ind, 3).to_vec(), lits4[..3].to_vec());
	// The bad clause index stands for the (empty) explicit buffer
	assert_eq!(topi.ConstClsSpan(0, 0).size(), 0);

	// An initial ternary clause: no bits for the clause size
	let mut lits3: [TULit; 3] = [2, 4, 6];
	let bc3 = topi.BCCompress(Span::from_slice(&mut lits3), false, 0, false, 0., false, None);
	assert!(!bc3.IsError() && !bc3.m_IsLearnt() && bc3.m_BitsForClsSize() == 0 && bc3.m_BitsForLit() == 3);
	topi.ClsSetSize(bc3.ind(), 3);
	assert_eq!(topi.ClsGetSize(bc3.ind()), 3);
	assert_eq!(topi.Cls(bc3.ind()).lits().collect::<Vec<_>>(), lits3.to_vec());
	assert_eq!(topi.BCNextBitSum(), 9 + 1 + 3 + 32 + 16);
	assert!(topi.BCCapacitySum() >= 2);
	let bi = topi.GetStatBufferInfo();
	assert!(bi.m_b_num == 2 && bi.m_b_cap == topi.BCCapacitySum() && bi.m_b_size == 1);

	// The explicit-only accessors
	assert!(!topi.IsError());
	let Mismatch = |topi: &TTopi, funcName: &str|
	{
		assert!(topi.m_Status.get() == TToporStatus::STATUS_COMPRESSED_MISMATCH);
		assert_eq!(topi.GetStatusExplanation(), funcName.to_string() + " invoked in compressed mode");
		ResetStatus(topi);
	};
	topi.EClsSetIsLearnt(ind, true);
	Mismatch(&topi, "EClsSetIsLearnt");
	assert_eq!(topi.EClsGetStandardActivityAsInt(ind), 0);
	Mismatch(&topi, "EClsGetStandardActivityAsConstIntPtr");
	topi.EClsSetStandardActivityAsInt(ind, 1);
	Mismatch(&topi, "EClsGetStandardActivityAsIntPtr");
	assert!(!topi.EClsGetIsOversized(ind));
	Mismatch(&topi, "EClsGetIsOversized");
	assert!(!topi.EClsGetIsOversizedAssumeLearnt(ind));
	Mismatch(&topi, "EClsGetIsOversizedAssumeLearnt");
	assert_eq!(topi.EClsLitsStartOffset(true, false), 0);
	Mismatch(&topi, "EClsLitsStartOffset");
}

// BCCompress: allocation failures and a too narrow index
#[test]
fn bc_compress_failures()
{
	let mut lits: Vec<TULit> = (0..30).map(|i| (1u32 << 30) + i).collect();
	let litsSpan = Span::from_slice(&mut lits);

	// The bit-array cannot be created
	let mut topi = CTopi::<u64, true>::new(10);
	let bc = super::containers::with_alloc_failure(0, || topi.BCCompress(litsSpan, false, 0, false, 0., false, None));
	assert!(bc.IsError() && topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED && topi.m_BC.size() == 0);
	// The bit-array cannot be extended
	ResetStatus(&topi);
	let bc = super::containers::with_alloc_failure(1, || topi.BCCompress(litsSpan, false, 0, false, 0., false, None));
	assert!(bc.IsError() && topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED && topi.m_BC.size() == 1);

	// A 32-bit index leaves 21 bits for the location of the clause in its bit-array
	let mut topi = CTopi::<u32, true>::new(10);
	let mut clssNum = 0;
	loop
	{
		let bc = topi.BCCompress(litsSpan, false, 0, false, 0., false, None);
		if bc.IsError()
		{
			break;
		}
		assert_eq!(bc.m_BitStart(), clssNum * (5 + 30 * 31));
		assert_eq!(topi.ConstClsSpan(bc.ind(), TUV::MAX).to_vec(), lits);
		clssNum += 1;
	}
	assert_eq!(clssNum, (1 << 21) / (5 + 30 * 31) + 1);
	assert!(topi.m_Status.get() == TToporStatus::STATUS_INDEX_TOO_NARROW);
}

// BCDecompress and BCIsIdenticalToDecompressed are intended for debugging and validation: the solver itself never invokes them.
// In the compressed mode, the explicit-only accessors, which they use, return 0, so that the explicit clause overlaps its own header.
// The functions are invoked over crafted buffers, where the explicit clause index is also the index of the compressed clause.
#[test]
fn bc_decompress()
{
	type TTopi = CTopi<u64, true>;

	// An initial clause, whose first literal has the value of the clause size: the decompressed clause is identical
	{
		let mut topi = TTopi::new(10);
		let mut lits: [TULit; 4] = [4, 5, 6, 7];
		let bc = topi.BCCompress(Span::from_slice(&mut lits), false, 0, false, 0., false, None);
		assert!(bc.m_BitStart() == 0 && bc.ind() == bc.GetHashId().val() as u64);
		topi.m_BNext = bc.ind();
		let mut b = 0;
		topi.BCDecompress(bc.GetHashId(), &mut b);
		assert_eq!(b, 1 + 4 * 3);
		assert_eq!(topi.m_B.get_span_cap_sz(bc.ind() as usize, 4).to_vec(), lits.to_vec());
		assert!(topi.m_Status.get() == TToporStatus::STATUS_COMPRESSED_MISMATCH);
		ResetStatus(&topi);
		assert!(topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));
		// Once again, when the explicit buffer is already large enough
		topi.m_B.reserve_atleast(bc.ind() as usize + 100);
		let mut b = 0;
		topi.BCDecompress(bc.GetHashId(), &mut b);
		assert_eq!(b, 1 + 4 * 3);
		ResetStatus(&topi);

		// Another clause in the same bit-array: the literals differ
		let mut lits2: [TULit; 4] = [4, 5, 6, 3];
		let bc2 = topi.BCCompress(Span::from_slice(&mut lits2), false, 0, false, 0., false, None);
		assert!(bc2.GetHashId() == bc.GetHashId() && bc2.m_BitStart() == 13);
		topi.m_B.reserve_atleast(bc2.ind() as usize + 8);
		topi.m_B[bc2.ind() as usize] = 4;
		assert!(topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13));
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc2.ind(), bc.GetHashId(), 0); });
		// The sizes differ
		topi.m_B[bc2.ind() as usize] = 5;
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13); });
		// Learnt vs. initial
		topi.m_B[bc2.ind() as usize] = 4 | ClsIsLearntMask;
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13); });
	}

	// A learnt clause: the decompressed clause can never be identical, since its first literal would have to hold the is-learnt bit
	{
		let mut topi = TTopi::new(10);
		let mut lits: [TULit; 4] = [4, 5, 6, 7];
		let bc = topi.BCCompress(Span::from_slice(&mut lits), true, 2, true, 1.5, false, None);
		assert!(bc.m_BitStart() == 0 && bc.m_IsLearnt());
		let ind = bc.ind() as usize;
		topi.m_BNext = bc.ind();
		let mut b = 0;
		ExpectAssertionFailure(|| topi.BCDecompress(bc.GetHashId(), &mut b));
		assert_eq!(b, 1 + 3 + 32 + 4 * 3);
		assert_eq!(topi.m_B.get_span_cap_sz(ind, 4).to_vec(), lits.to_vec());
		ResetStatus(&topi);

		// The explicit header is crafted
		let header = ClsIsLearntMask | (2 << ClsSizeBits) | 4;
		let activity = 1.5f32.to_bits() | ClsSkipdelMask;
		topi.m_B[ind] = header;
		topi.m_B[ind + 1] = activity;
		assert!(topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));
		// The glues differ
		topi.m_B[ind] = ClsIsLearntMask | (3 << ClsSizeBits) | 4;
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0); });
		topi.m_B[ind] = header;
		// The skip-deletion flags differ
		topi.m_B[ind + 1] = 1.5f32.to_bits();
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0); });
		// The activities differ (they are compared as Booleans)
		topi.m_B[ind + 1] = ClsSkipdelMask;
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0); });
		topi.m_B[ind + 1] = activity;
		assert!(topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));

		// An oversized learnt clause: only the size is crafted in the bit-array
		let hashId = TBCHashId::new(true, BCClsSize2Bits(ClsLearntMaxSizeWithGlue as u64 + 1), 21);
		assert_eq!(hashId.m_BitsForClsSize(), 20);
		let ba = unsafe { &mut *topi.m_BC.insert(hashId.val()) };
		ba.bit_reserve_new_chunk(64, 0);
		ba.bit_push(BCClsSize2EncodedClsSize(ClsLearntMaxSizeWithGlue as u64 + 1) as u64, 20);
		assert_eq!(topi.ClsGetSize(TBCInd::<u64>::new(hashId, 0).ind()), ClsLearntMaxSizeWithGlue + 1);
		ExpectAssertionFailure(|| { topi.BCIsIdenticalToDecompressed(bc.ind(), hashId, 0); });
	}

	// A learnt clause is decompressed by a solver, which works in the explicit mode, where the leftovers of an oversized clause reside in the buffer:
	// the glue is then taken to be the size of the clause; the validation fails eventually, since it reads the explicit clause as a compressed one
	{
		let mut topi = CTopi::<u64, false>::new(10);
		let mut lits: [TULit; 4] = [4, 5, 6, 7];
		let bc = topi.BCCompress(Span::from_slice(&mut lits), true, 4, false, 0., false, None);
		let ind = topi.m_BNext as usize;
		topi.m_AnyOversized = true;
		topi.m_B[ind] = 0;
		topi.m_B[ind + 1] = ClsOversizeActivity as TULit;
		let mut b = 0;
		ExpectAssertionFailure(|| topi.BCDecompress(bc.GetHashId(), &mut b));
		assert_eq!(b, 1 + 3 + 32 + 4 * 3);
		assert!(topi.ClsGetIsLearnt(topi.m_BNext) && topi.EClsGetIsOversized(topi.m_BNext) && topi.ClsGetSize(topi.m_BNext) == 4 && topi.ClsGetGlue(topi.m_BNext) == 4);
		assert!(!topi.IsError());
	}

	// Failures
	{
		let mut topi = TTopi::new(10);
		let mut lits: [TULit; 4] = [4, 5, 6, 7];
		let bc = topi.BCCompress(Span::from_slice(&mut lits), false, 0, false, 0., false, None);
		// The bit-array of an unknown hash-id cannot be created
		let mut b = 0;
		super::containers::with_alloc_failure(0, || topi.BCDecompress(TBCHashId::new(false, 2, 5), &mut b));
		assert!(b == 0 && topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED && topi.GetStatusExplanation().contains("couldn't insert into m_BC"));
		ResetStatus(&topi);
		// The explicit buffer cannot be extended
		super::containers::with_alloc_failure(0, || topi.BCDecompress(bc.GetHashId(), &mut b));
		assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED && topi.GetStatusExplanation().contains("couldn't reserve the buffer"));

		// The 32-bit index into the explicit buffer wraps around
		let mut topi = CTopi::<u32, true>::new(10);
		let bc = topi.BCCompress(Span::from_slice(&mut lits), false, 0, false, 0., false, None);
		topi.m_BNext = u32::MAX - 1;
		let mut b = 0;
		topi.BCDecompress(bc.GetHashId(), &mut b);
		assert!(topi.m_Status.get() == TToporStatus::STATUS_INDEX_TOO_NARROW);
	}
}

// The constructor: allocation failures and the dump file
#[test]
fn constructor()
{
	fn Check<Ind: TUInd, const Compress: bool>()
	{
		let mut explanations: Vec<String> = vec![];
		for failAt in 0..12
		{
			let mut topi = super::containers::with_alloc_failure(failAt, || CTopi::<Ind, Compress>::new(5));
			if topi.IsError()
			{
				assert!(topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED);
				assert_eq!(topi.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_MEM_OUT);
				if !explanations.contains(&topi.GetStatusExplanation())
				{
					explanations.push(topi.GetStatusExplanation());
				}
			}
			else
			{
				topi.AddUserClause(&[1, 2]);
				assert_eq!(topi.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_SAT);
			}
		}
		for arr in ["the main buffer", "m_Watches", "m_AssignmentInfo"]
		{
			assert!(explanations.iter().any(|e| e.ends_with(&("couldn't allocate ".to_string() + arr))), "{} {:?}", arr, explanations);
		}
	}
	Check::<u32, false>();
	Check::<u64, false>();
	Check::<u64, true>();

	// No explanation unless there is an error or the verbosity is on
	let mut topi = CTopi::<u32, false>::new(0);
	topi.SetStatus(TToporStatus::STATUS_UNDECIDED, "an explanation".to_string());
	assert_eq!(topi.GetStatusExplanation(), "");
	topi.m_Params.m_ParamVerbosity.set(1);
	assert_eq!(topi.GetStatusExplanation(), "an explanation");
}

// TOPOR_DUMP_NAME: the prefix of the name of the file, which the invocations of the solver are dumped to.
// The environment is process-wide, so the solvers constructed by the other tests in the meantime are dumped too: the files are created in a dedicated temporary directory
#[test]
fn constructor_dump_file()
{
	let dir = std::env::temp_dir().join(format!("topor_cov_buffers_dump_{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let prefix = dir.join("dump");
	std::env::set_var("TOPOR_DUMP_NAME", &prefix);
	let mut topi = CTopi::<u32, false>::new(7);
	// A second solver, created in the same second at the same address (that of the solver inside new, before it's moved out), gets a file of its own
	let other = CTopi::<u32, false>::new(5);
	std::env::remove_var("TOPOR_DUMP_NAME");
	assert!(topi.m_DumpFile.is_some() && other.m_DumpFile.is_some());
	topi.AddUserClause(&[1, -2]);
	topi.AddUserClause(&[2, 3, 4]);
	assert_eq!(topi.Solve(&[-1], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_SAT);
	drop(topi);
	drop(other);

	// The name of the file contains the address of the solver at the time of its construction, the process id and the number of the dump in the process
	let mut dumps: Vec<String> = vec![];
	for entry in std::fs::read_dir(&dir).unwrap()
	{
		let path = entry.unwrap().path();
		let name = path.file_name().unwrap().to_str().unwrap().to_string();
		assert!(name.starts_with("dump_0x") && name.ends_with(".cnf") && name.contains(&format!("_{}_", std::process::id())), "{}", name);
		dumps.push(std::fs::read_to_string(&path).unwrap());
	}
	assert!(dumps.iter().any(|d| d.starts_with("p cnf 7 0\n") && d.contains("1 -2 0\n") && d.contains("2 3 4 0\n")), "{:?}", dumps);
	assert!(dumps.iter().any(|d| d == "p cnf 5 0\n"), "{:?}", dumps);
	// The directory itself is left in place for the solvers of the other tests, which might have read the variable, but haven't created their files yet
	std::thread::sleep(std::time::Duration::from_millis(100));
	for entry in std::fs::read_dir(&dir).unwrap()
	{
		let _ = std::fs::remove_file(entry.unwrap().path());
	}
	let _ = std::fs::remove_dir(&dir);
}

// The value returned by BCIsIdenticalToDecompressed for the mismatches, crafted as in bc_decompress:
// every mismatch fails an assertion first, so false is returned only if the debug assertions are compiled out
#[test]
fn nda_bc_is_identical_to_decompressed_mismatches()
{
	type TTopi = CTopi<u64, true>;

	fn ExpectMismatch(f: impl FnOnce() -> bool)
	{
		if cfg!(debug_assertions)
		{
			ExpectAssertionFailure(|| { f(); });
		}
		else
		{
			assert!(!f());
		}
	}

	// Initial clauses
	{
		let mut topi = TTopi::new(10);
		let mut lits: [TULit; 4] = [4, 5, 6, 7];
		let bc = topi.BCCompress(Span::from_slice(&mut lits), false, 0, false, 0., false, None);
		assert!(bc.m_BitStart() == 0 && bc.ind() == bc.GetHashId().val() as u64);
		let mut lits2: [TULit; 4] = [4, 5, 6, 3];
		let bc2 = topi.BCCompress(Span::from_slice(&mut lits2), false, 0, false, 0., false, None);
		assert!(bc2.GetHashId() == bc.GetHashId() && bc2.m_BitStart() == 13);
		topi.m_B.reserve_atleast(bc2.ind() as usize + 8);
		topi.m_B[bc2.ind() as usize] = 4;
		assert!(topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13));
		// The literals differ
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc2.ind(), bc.GetHashId(), 0));
		// The sizes differ
		topi.m_B[bc2.ind() as usize] = 5;
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13));
		// Learnt vs. initial
		topi.m_B[bc2.ind() as usize] = 4 | ClsIsLearntMask;
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13));
		topi.m_B[bc2.ind() as usize] = 4;
		assert!(topi.BCIsIdenticalToDecompressed(bc2.ind(), bc2.GetHashId(), 13));
	}

	// A learnt clause
	{
		let mut topi = TTopi::new(10);
		let mut lits: [TULit; 4] = [4, 5, 6, 7];
		let bc = topi.BCCompress(Span::from_slice(&mut lits), true, 2, true, 1.5, false, None);
		assert!(bc.m_BitStart() == 0 && bc.m_IsLearnt());
		let ind = bc.ind() as usize;
		topi.m_B.reserve_atleast(ind + 8);

		// The explicit header is crafted
		let header = ClsIsLearntMask | (2 << ClsSizeBits) | 4;
		let activity = 1.5f32.to_bits() | ClsSkipdelMask;
		topi.m_B[ind] = header;
		topi.m_B[ind + 1] = activity;
		assert!(topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));
		// The glues differ
		topi.m_B[ind] = ClsIsLearntMask | (3 << ClsSizeBits) | 4;
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));
		topi.m_B[ind] = header;
		// The skip-deletion flags differ
		topi.m_B[ind + 1] = 1.5f32.to_bits();
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));
		// The activities differ (they are compared as Booleans)
		topi.m_B[ind + 1] = ClsSkipdelMask;
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));
		topi.m_B[ind + 1] = activity;
		assert!(topi.BCIsIdenticalToDecompressed(bc.ind(), bc.GetHashId(), 0));

		// An oversized learnt clause: only the size is crafted in the bit-array
		let hashId = TBCHashId::new(true, BCClsSize2Bits(ClsLearntMaxSizeWithGlue as u64 + 1), 21);
		assert_eq!(hashId.m_BitsForClsSize(), 20);
		let ba = unsafe { &mut *topi.m_BC.insert(hashId.val()) };
		ba.bit_reserve_new_chunk(64, 0);
		ba.bit_push(BCClsSize2EncodedClsSize(ClsLearntMaxSizeWithGlue as u64 + 1) as u64, 20);
		ExpectMismatch(|| topi.BCIsIdenticalToDecompressed(bc.ind(), hashId, 0));
	}
}
