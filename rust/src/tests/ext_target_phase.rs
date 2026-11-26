// Unit tests of the Rust-only target/best-phase saving and rephasing (topi_ext_target_phase.rs)

use crate::topi::*;
use crate::topor::*;

const NoTimeout: (f64, bool) = (f64::MAX, true);
const Sat: TToporLitVal = TToporLitVal::VAL_SATISFIED;
const Unsat: TToporLitVal = TToporLitVal::VAL_UNSATISFIED;

fn Solve<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, assumps: &[TLit]) -> TToporReturnVal
{
	s.Solve(assumps, NoTimeout, u64::MAX)
}

// A linear congruential generator
struct TRand(u64);

impl TRand
{
	fn Next(&mut self, n: u64) -> u64
	{
		self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
		(self.0 >> 33) % n
	}

	fn Lit(&mut self, vars: TLit) -> TLit
	{
		let v = self.Next(vars as u64) as TLit + 1;
		if self.Next(2) == 0 { v } else { -v }
	}

	fn Clauses(&mut self, vars: TLit, clss: usize) -> Vec<Vec<TLit>>
	{
		(0..clss).map(|_| vec![self.Lit(vars), self.Lit(vars), self.Lit(vars)]).collect()
	}
}

// A random 3-SAT formula, which takes the solver a few hundred conflicts
fn HardFormula(seed: u64) -> (TLit, Vec<Vec<TLit>>)
{
	let vars = 120;
	(vars, TRand(seed).Clauses(vars, (vars as f64 * 4.26) as usize))
}

fn SolveHard<Ind: TUInd, const Compress: bool>(seed: u64, params: &[(&str, f64)]) -> (CTopor<Ind, Compress>, TToporReturnVal)
{
	let (_, clss) = HardFormula(seed);
	let mut s = CTopor::<Ind, Compress>::new(0);
	for (name, val) in params
	{
		s.SetParam(name, *val);
	}
	for c in &clss
	{
		s.AddClause(c);
	}
	let ret = Solve(&mut s, &[]);
	if ret == TToporReturnVal::RET_SAT
	{
		assert!(clss.iter().all(|c| c.iter().any(|l| s.GetLitValue(*l) != Unsat)), "seed {}", seed);
	}
	(s, ret)
}

// (conflicts, decisions, restarts, rephases, target updates, target decisions)
fn Stat<Ind: TUInd, const Compress: bool>(s: &CTopor<Ind, Compress>) -> (u64, u64, u64, u64, u64, u64)
{
	let st = s.m_Topi.GetStatistics();
	(st.m_Conflicts, st.m_Decisions, st.m_Restarts, st.m_ExtRephases, st.m_ExtTargetPhaseUpdates, st.m_ExtTargetPhaseDecisions)
}

#[test]
fn off_by_default()
{
	for seed in 0..4
	{
		let (mut s, ret) = SolveHard::<u32, false>(seed, &[]);
		let (confs, decs, rsts, rephases, updates, targetDecs) = Stat(&s);
		assert!(confs > 0);
		assert_eq!((rephases, updates, targetDecs), (0, 0, 0));
		// Nothing has been touched: no user polarity was given, so the polarity information has never been activated
		assert!(!s.m_Topi.m_PolarityInfoActivated && s.m_Topi.m_PolarityInfo.cap() == 0);
		assert_eq!(s.m_Topi.m_ExtTargetPhase.m_RephaseInterval, 0);
		assert!(!s.GetStatStrShort(false).contains("ToporExt"));

		// The parameters of the schedule alone change nothing either
		let (s2, ret2) = SolveHard::<u32, false>(seed, &[("/phase/rephase/conf_incr", 7.), ("/phase/rephase/conf_mult", 3.), ("/phase/rephase/random", 1.)]);
		assert_eq!(ret, ret2);
		assert_eq!(Stat(&s2), (confs, decs, rsts, 0, 0, 0));
		assert!(!s2.m_Topi.m_PolarityInfoActivated);
	}
}

#[test]
fn target_phase_is_used()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		let (mut anyChanged, mut answers) = (false, Vec::new());
		for seed in 0..6
		{
			let (base, retBase) = SolveHard::<Ind, Compress>(seed, &[]);
			let (mut s, ret) = SolveHard::<Ind, Compress>(seed, &[("/phase/target/on", 1.)]);
			assert_eq!(ret, retBase, "seed {}", seed);
			let (confs, decs, _, rephases, updates, targetDecs) = Stat(&s);
			// The target is taken at the first conflict, and the decisions after it take the target phases
			assert!(updates > 0 && targetDecs > 0 && targetDecs < decs, "seed {}", seed);
			assert_eq!(rephases, 0);
			assert!(s.GetStatStrShort(false).contains(&format!("c ToporExt Rephases 0 TargetUpds {} TargetDecs {}\n", updates, targetDecs)));
			anyChanged |= (confs, decs) != (Stat(&base).0, Stat(&base).1);
			answers.push(ret);

			// 2: only under the numeric restart strategy: that of the default mode, but not of the mode 1, whose strategy is the LBD-based one
			let (s, ret) = SolveHard::<Ind, Compress>(seed, &[("/phase/target/on", 2.)]);
			assert_eq!(ret, retBase);
			assert_eq!(Stat(&s), (confs, decs, Stat(&s).2, 0, updates, targetDecs), "seed {}", seed);
			let (base1, retBase1) = SolveHard::<Ind, Compress>(seed, &[("/mode/value", 1.)]);
			let (s, ret) = SolveHard::<Ind, Compress>(seed, &[("/mode/value", 1.), ("/phase/target/on", 2.)]);
			assert_eq!((ret, retBase1), (retBase, retBase));
			// The target is tracked, but never used, so the search is that of the base run
			assert!(Stat(&s).4 > 0);
			assert_eq!((Stat(&s).0, Stat(&s).1, Stat(&s).5), (Stat(&base1).0, Stat(&base1).1, 0), "seed {}", seed);
			let (s, _) = SolveHard::<Ind, Compress>(seed, &[("/mode/value", 1.), ("/phase/target/on", 1.)]);
			assert!(Stat(&s).5 > 0);
		}
		// The target phases do change the search, and the formulas are of both kinds
		assert!(anyChanged);
		assert!(answers.contains(&TToporReturnVal::RET_SAT) && answers.contains(&TToporReturnVal::RET_UNSAT), "{:?}", answers);
	}
	Test::<u32, false>();
	Test::<u64, false>();
	Test::<u64, true>();
}

#[test]
fn rephasing_follows_the_conflict_schedule()
{
	let restarts = ("/restarts/numeric/conflict_thr", 5.);
	let mut anyRephase = false;
	for seed in 0..6
	{
		let (base, retBase) = SolveHard::<u32, false>(seed, &[restarts]);
		// Every 20 conflicts (at the first restart after them)
		let (s, ret) = SolveHard::<u32, false>(seed, &[restarts, ("/phase/rephase/conf_init", 20.), ("/phase/rephase/conf_incr", 0.)]);
		assert_eq!(ret, retBase, "seed {}", seed);
		let (confs, _, rsts, rephases, updates, targetDecs) = Stat(&s);
		assert!(rephases <= confs / 20 && rephases <= rsts, "seed {}", seed);
		assert!(confs < 60 || rephases >= 1, "seed {}", seed);
		// The best phases are tracked, but no decision takes a target phase
		assert!(updates > 0 || confs == 0);
		assert_eq!(targetDecs, 0);
		anyRephase |= rephases > 0;
		let _ = base;

		// Arithmetic: 20, 40, 60, ...: n rephases take at least 20 * n * (n + 1) / 2 conflicts; geometric: 20, 40, 80, ...: at least 20 * (2^n - 1)
		let (s, ret) = SolveHard::<u32, false>(seed, &[restarts, ("/phase/rephase/conf_init", 20.), ("/phase/rephase/conf_incr", 20.), ("/phase/target/on", 1.)]);
		assert_eq!(ret, retBase);
		let (confs, _, _, n, _, _) = Stat(&s);
		assert!(10 * n * (n + 1) <= confs, "seed {}: {} rephases in {} conflicts", seed, n, confs);
		let (s, ret) = SolveHard::<u32, false>(seed, &[restarts, ("/phase/rephase/conf_init", 20.), ("/phase/rephase/conf_incr", 0.), ("/phase/rephase/conf_mult", 2.), ("/phase/rephase/random", 1.)]);
		assert_eq!(ret, retBase);
		let (confs, _, _, n, _, _) = Stat(&s);
		assert!(20 * ((1u64 << n) - 1) <= confs, "seed {}: {} rephases in {} conflicts", seed, n, confs);
		assert_eq!(s.m_Topi.m_ExtTargetPhase.m_RephaseInterval, 20 << n);
	}
	assert!(anyRephase);
}

// The variables 1..=6 are free (the solver's default polarity is positive), and 7 is implied by the negation of any of them
fn NewFreeSolver(params: &[(&str, f64)]) -> CTopor<u32, false>
{
	let mut s = CTopor::<u32, false>::new(0);
	for (name, val) in params
	{
		s.SetParam(name, *val);
	}
	for v in 1..=6
	{
		s.AddClause(&[v, 7]);
	}
	s
}

fn SavedPhases(s: &CTopor<u32, false>) -> Vec<TLit>
{
	(1..=6).map(|v| if s.GetSavedPhase(v) == Sat { v } else { -v }).collect()
}

// Backtrack to the level 0 and rephase now, as the restart after the scheduled number of conflicts does
fn RephaseNow(s: &mut CTopor<u32, false>)
{
	s.Backtrack(0);
	s.m_Topi.m_ExtTargetPhase.m_RephaseNextConf = 0;
	let before = s.m_Topi.GetStatistics().m_ExtRephases;
	s.m_Topi.ExtTargetPhaseRephaseIfRequired();
	assert_eq!(s.m_Topi.GetStatistics().m_ExtRephases, before + 1);
}

#[test]
fn rephase_kinds_user_polarities_and_hints()
{
	let mut s = NewFreeSolver(&[("/phase/rephase/conf_init", 1000.), ("/phase/target/on", 1.), ("/phase/hints/on", 1.)]);
	// The polarity of 3 is fixed by the user, and that of 1 is hinted
	s.FixPolarity(-3, false);
	assert!(s.HintPhase(-1));
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!((1..=6).map(|v| if s.GetLitValue(v) == Sat { v } else { -v }).collect::<Vec<TLit>>(), [-1, 2, -3, 4, 5, 6]);
	assert_eq!(s.m_Topi.GetStatistics().m_Conflicts, 0);
	// The saved phase of 4 is changed, as phase saving would have done, had the search flipped it
	s.Backtrack(0);
	let l4 = s.m_Topi.E2I(4);
	s.m_Topi.m_AssignmentInfo[GetVar(l4) as usize].set_m_IsNegated(!IsNeg(l4));
	assert_eq!(SavedPhases(&s), [-1, 2, -3, -4, 5, 6]);

	let (orig, inverted) = ([-1, 2, -3, 4, 5, 6], [1, -2, -3, -4, -5, -6]);
	// O: the phases of the start of the query, where the hint is the original phase of its variable
	RephaseNow(&mut s);
	assert_eq!(SavedPhases(&s), orig);
	// I: inverted, but for the variable, whose polarity is the user's
	RephaseNow(&mut s);
	assert_eq!(SavedPhases(&s), inverted);
	// B: there are no best phases without a conflict, so nothing changes
	RephaseNow(&mut s);
	assert_eq!(SavedPhases(&s), inverted);
	// F: flipped
	RephaseNow(&mut s);
	assert_eq!(SavedPhases(&s), orig);
	// B O B I, and the cycle starts again: B F B O B I
	for expected in [orig, orig, orig, inverted, inverted, orig, orig, orig, orig, inverted]
	{
		RephaseNow(&mut s);
		assert_eq!(SavedPhases(&s), expected);
	}
	assert_eq!(s.m_Topi.GetStatistics().m_ExtRephases, 14);
	// The intervals: 1000, 2000, ... (the default increment is 1000)
	assert_eq!(s.m_Topi.m_ExtTargetPhase.m_RephaseInterval, 15000);

	// B F: back to the original phases, which the next query takes, but for the variables, whose polarity is the user's; once the user clears them, the saved phases are taken
	RephaseNow(&mut s);
	RephaseNow(&mut s);
	assert_eq!(SavedPhases(&s), orig);
	s.FixPolarity(-2, false);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!((1..=6).map(|v| if s.GetLitValue(v) == Sat { v } else { -v }).collect::<Vec<TLit>>(), [-1, -2, -3, 4, 5, 6]);
	s.ClearUserPolarityInfo(2);
	s.ClearUserPolarityInfo(3);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!((1..=6).map(|v| if s.GetLitValue(v) == Sat { v } else { -v }).collect::<Vec<TLit>>(), [-1, -2, -3, 4, 5, 6]);

	// Random phases instead of the flipped ones: the user's polarity is still not touched
	let mut s = NewFreeSolver(&[("/phase/rephase/conf_init", 1.), ("/phase/rephase/random", 1.)]);
	s.FixPolarity(-3, false);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	let mut seen = std::collections::HashSet::new();
	for i in 0..40
	{
		RephaseNow(&mut s);
		assert_eq!(SavedPhases(&s)[2], -3);
		if i >= 2 && (i - 2) % 6 == 1
		{
			seen.insert(SavedPhases(&s));
		}
	}
	assert!(seen.len() > 1);
}

#[test]
fn target_phases_and_forced_polarities()
{
	// /phase/force_solution: the polarities of the latest model are forced, so the next query repeats the model without a single target-phase decision
	for seed in 0..6
	{
		let (mut s, ret) = SolveHard::<u32, false>(seed, &[("/phase/target/on", 1.), ("/phase/rephase/conf_init", 10.), ("/restarts/numeric/conflict_thr", 5.), ("/phase/force_solution", 1.), ("/phase/unforce_restarts_fraction_n", 0.), ("/phase/unforce_restarts_fraction_s", 0.)]);
		if ret != TToporReturnVal::RET_SAT
		{
			continue;
		}
		let (vars, _) = HardFormula(seed);
		let model: Vec<TToporLitVal> = (1..=vars).map(|v| s.GetLitValue(v)).collect();
		let (confs, _, _, _, _, targetDecs) = Stat(&s);
		assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
		assert_eq!((1..=vars).map(|v| s.GetLitValue(v)).collect::<Vec<TToporLitVal>>(), model, "seed {}", seed);
		assert_eq!((Stat(&s).0, Stat(&s).5), (confs, targetDecs), "seed {}", seed);
	}
}

// Random formulas, parameters of the extension, user polarities, hints and incremental queries in every instantiation:
// every answer is checked against the truth table, and every model against the clauses
#[test]
fn target_phases_and_rephasing_never_change_an_answer()
{
	fn IsSat(vars: TLit, clss: &[Vec<TLit>], assumps: &[TLit]) -> bool
	{
		let holds = |a: u32, l: TLit| ((a >> (l.abs() - 1)) & 1 == 1) == (l > 0);
		(0..1u32 << vars).any(|a| assumps.iter().all(|l| holds(a, *l)) && clss.iter().all(|c| c.iter().any(|l| holds(a, *l))))
	}

	// Returns (rephases, target decisions)
	fn Test<Ind: TUInd, const Compress: bool>(seed: u64) -> (u64, u64)
	{
		let mut rand = TRand(seed);
		let vars = 10 + rand.Next(5) as TLit;
		let mut clss = rand.Clauses(vars, (vars as f64 * 4.0) as usize);
		let mut s = CTopor::<Ind, Compress>::new(0);
		if rand.Next(3) == 0
		{
			// The mode 1 restarts by the LBD-based strategy
			s.SetParam("/mode/value", 1.);
			s.SetParam("/restarts/lbd/win_size", 1. + rand.Next(3) as f64);
		}
		s.SetParam("/restarts/numeric/conflict_thr", 1. + rand.Next(3) as f64);
		s.SetParam("/phase/target/on", rand.Next(3) as f64);
		s.SetParam("/phase/rephase/conf_init", rand.Next(4) as f64);
		s.SetParam("/phase/rephase/conf_incr", rand.Next(3) as f64);
		s.SetParam("/phase/rephase/conf_mult", 1. + rand.Next(3) as f64 / 2.);
		s.SetParam("/phase/rephase/random", rand.Next(2) as f64);
		s.SetParam("/phase/hints/on", rand.Next(2) as f64);
		if rand.Next(4) == 0
		{
			s.SetParam("/decision/polarity/init_strategy", rand.Next(3) as f64);
		}
		if rand.Next(4) == 0
		{
			s.SetParam("/phase/force_solution", 1.);
		}
		if rand.Next(4) == 0
		{
			s.SetParam("/phase/unforce_restarts_fraction_init", 0.5);
			s.SetParam("/phase/unforce_restarts_fraction_n", 0.5);
			s.SetParam("/phase/unforce_restarts_fraction_s", 0.5);
			s.SetParam("/phase/restarts_block_size", 2.);
		}
		for c in &clss
		{
			s.AddClause(c);
		}
		for query in 0..6
		{
			for _ in 0..rand.Next(vars as u64)
			{
				s.HintPhase(rand.Lit(vars));
			}
			if rand.Next(3) == 0
			{
				// (of a variable, which exists: fixing the polarity of a new variable and then adding a clause is an upstream issue, see README.md)
				let l = clss[rand.Next(clss.len() as u64) as usize][0];
				if rand.Next(4) == 0 { s.ClearUserPolarityInfo(l.abs()) } else { s.FixPolarity(if rand.Next(2) == 0 { l } else { -l }, rand.Next(2) == 0) };
			}
			if rand.Next(5) == 0
			{
				// The extension is turned on and off between the queries
				s.SetParam("/phase/target/on", rand.Next(3) as f64);
				s.SetParam("/phase/rephase/conf_init", rand.Next(4) as f64);
			}
			let assumps: Vec<TLit> = (0..rand.Next(4)).map(|_| rand.Lit(vars)).collect();
			let ret = Solve(&mut s, &assumps);
			let exp = IsSat(vars, &clss, &assumps);
			assert_eq!(ret, if exp { TToporReturnVal::RET_SAT } else { TToporReturnVal::RET_UNSAT }, "seed {} query {}", seed, query);
			if exp
			{
				let holds = |l: TLit| s.GetLitValue(l) != Unsat;
				assert!(assumps.iter().all(|l| holds(*l)) && clss.iter().all(|c| c.iter().any(|l| holds(*l))), "seed {} query {}", seed, query);
			}
			for _ in 0..rand.Next(3)
			{
				let c = vec![rand.Lit(vars), rand.Lit(vars), rand.Lit(vars)];
				s.AddClause(&c);
				clss.push(c);
			}
		}
		assert!(!s.IsError(), "seed {}", seed);
		let st = s.m_Topi.GetStatistics();
		(st.m_ExtRephases, st.m_ExtTargetPhaseDecisions)
	}

	let (mut rephases, mut targetDecs) = (0, 0);
	for seed in 0..200
	{
		for (r, d) in [Test::<u32, false>(seed), Test::<u64, false>(seed), Test::<u64, true>(seed)]
		{
			rephases += r;
			targetDecs += d;
		}
	}
	// The scenarios do exercise the extension
	assert!(rephases > 50 && targetDecs > 300, "{} rephases, {} target decisions", rephases, targetDecs);
}
