// Unit tests of the Rust-only phase and score hints (topi_ext_hints.rs)

use crate::topor::*;

const NoTimeout: (f64, bool) = (f64::MAX, true);
const Sat: TToporLitVal = TToporLitVal::VAL_SATISFIED;
const Unsat: TToporLitVal = TToporLitVal::VAL_UNSATISFIED;

// The variables 1..=6 are free (the solver's default polarity is positive), and 7 is implied by the negation of any of them
fn NewSolver<Ind: TUInd, const Compress: bool>(hintsOn: bool) -> CTopor<Ind, Compress>
{
	let mut s = CTopor::<Ind, Compress>::new(0);
	if hintsOn
	{
		s.SetParam("/phase/hints/on", 1.);
	}
	for v in 1..=6
	{
		s.AddClause(&[v, 7]);
	}
	s
}

fn Solve<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, assumps: &[TLit]) -> TToporReturnVal
{
	s.Solve(assumps, NoTimeout, u64::MAX)
}

fn Model<Ind: TUInd, const Compress: bool>(s: &CTopor<Ind, Compress>, vars: std::ops::RangeInclusive<TLit>) -> Vec<TLit>
{
	vars.map(|v| if s.GetLitValue(v) == Unsat { -v } else { v }).collect()
}

#[test]
fn hints_are_off_by_default()
{
	let mut s = NewSolver::<u32, false>(false);
	assert!(!s.HintPhase(-1));
	assert!(!s.HintScore(1, 1.));
	assert_eq!(s.ReadHints("v -1 -2 0\nb 3 0.5\n"), Ok((0, 0)));
	assert!(s.m_Topi.m_ExtHints.m_PendingPhases.is_empty() && s.m_Topi.m_ExtHints.m_PendingScores.is_empty());
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(Model(&s, 1..=6), [1, 2, 3, 4, 5, 6]);

	// Illegal hints are ignored, even if the parameters are on
	let mut s = NewSolver::<u32, false>(true);
	s.SetParam("/decision/score_hints/mult", 1.);
	assert!(!s.HintPhase(0) && !s.HintPhase(TLit::MIN));
	assert!(!s.HintScore(0, 1.) && !s.HintScore(-1, 1.) && !s.HintScore(1, 0.) && !s.HintScore(1, -1.) && !s.HintScore(1, f64::NAN) && !s.HintScore(1, f64::INFINITY));
	assert!(s.HintPhase(-1) && s.HintScore(1, 1.));
	// Variables the solver doesn't know
	assert_eq!(s.GetSavedPhase(0), TToporLitVal::VAL_DONT_CARE);
	assert_eq!(s.GetSavedPhase(100), TToporLitVal::VAL_DONT_CARE);
	assert_eq!(s.GetVarScore(0), 0.);
	assert_eq!(s.GetVarScore(100), 0.);
}

#[test]
fn hinted_but_not_fixed()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		let mut s = NewSolver::<Ind, Compress>(true);
		for v in 1..=6
		{
			assert!(s.HintPhase(-v));
		}
		assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
		// The solver follows the hints
		assert_eq!(Model(&s, 1..=7), [-1, -2, -3, -4, -5, -6, 7]);
		assert_eq!(s.GetConflictsNumber(), 0);
		assert!(s.m_Topi.m_ExtHints.m_PendingPhases.is_empty());
		for v in 1..=6
		{
			assert_eq!(s.GetSavedPhase(v), Unsat);
		}
		assert_eq!(s.GetSavedPhase(7), Sat);

		// The hint doesn't fix the variable: an assumption flips it, and then phase saving keeps the new value (a fixed polarity would restore the old one)
		assert_eq!(Solve(&mut s, &[2]), TToporReturnVal::RET_SAT);
		assert_eq!(Model(&s, 1..=7), [-1, 2, -3, -4, -5, -6, 7]);
		s.AddClause(&[7, 8]);
		assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
		assert_eq!(Model(&s, 1..=7), [-1, 2, -3, -4, -5, -6, 7]);
		assert_eq!(s.GetSavedPhase(2), Sat);

		// A hint, which contradicts the formula, costs nothing but (possibly) time
		s.AddClause(&[1]);
		assert!(s.HintPhase(-1));
		assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
		assert_eq!(s.GetLitValue(1), Sat);
		assert_eq!(s.GetSavedPhase(1), Sat);
		s.AddClause(&[-1, -3]);
		s.AddClause(&[-1, 3]);
		assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_UNSAT);
	}
	Test::<u32, false>();
	Test::<u64, false>();
	Test::<u64, true>();
}

#[test]
fn hints_and_user_fixed_polarities()
{
	let mut s = NewSolver::<u32, false>(true);
	// A fixed polarity (forever or only once) takes precedence over a hint, whatever their order
	s.FixPolarity(1, false);
	assert!(s.HintPhase(-1));
	assert!(s.HintPhase(-2));
	assert!(s.HintPhase(-3));
	s.FixPolarity(3, true);
	s.FixPolarity(-4, false);
	assert!(s.HintPhase(4));
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(Model(&s, 1..=6), [1, -2, 3, -4, 5, 6]);

	// Once the polarity isn't fixed anymore, a new hint is followed: the variables are still assigned by the previous query, so the solver backtracks to apply it
	s.ClearUserPolarityInfo(1);
	s.ClearUserPolarityInfo(4);
	assert!(s.HintPhase(-1));
	assert!(s.HintPhase(4));
	assert!(s.HintPhase(-5));
	// (the previous model stands, unless there is something new to decide)
	s.AddClause(&[7, 8]);
	let backtracks = s.GetBacktracks();
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(s.GetBacktracks(), backtracks + 1);
	assert_eq!(Model(&s, 1..=6), [-1, -2, 3, 4, -5, 6]);

	// A hint doesn't override a polarity, which is fixed afterwards
	assert!(s.HintPhase(6));
	s.FixPolarity(-6, false);
	s.AddClause(&[7, 9]);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(Model(&s, 1..=6), [-1, -2, 3, 4, -5, -6]);
}

#[test]
fn hints_in_incremental_solving()
{
	let mut s = NewSolver::<u32, false>(true);
	// A hint for a variable, which doesn't exist yet, waits for it
	assert!(s.HintPhase(-9));
	assert!(s.HintPhase(-1));
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(s.m_Topi.m_ExtHints.m_PendingPhases, [-9]);
	assert_eq!(s.GetSavedPhase(9), TToporLitVal::VAL_DONT_CARE);
	s.AddClause(&[9, 7]);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert!(s.m_Topi.m_ExtHints.m_PendingPhases.is_empty());
	assert_eq!(Model(&s, 1..=9)[8], -9);
	assert_eq!(s.GetLitValue(1), Unsat);

	// An assumption wins over a hint, and the hint for an assumed variable is dropped rather than applied later
	assert!(s.HintPhase(-2));
	assert!(s.HintPhase(-3));
	assert_eq!(Solve(&mut s, &[2, 9]), TToporReturnVal::RET_SAT);
	assert_eq!(s.GetLitValue(2), Sat);
	assert_eq!(s.GetLitValue(9), Sat);
	assert_eq!(s.GetLitValue(3), Unsat);
	assert!(s.m_Topi.m_ExtHints.m_PendingPhases.is_empty());

	// Unsatisfiable under the assumptions, with the hints pointing the other way: the answer and the core are intact
	s.AddClause(&[-4, -5]);
	assert!(s.HintPhase(4));
	assert!(s.HintPhase(5));
	assert_eq!(Solve(&mut s, &[4, 6, 5]), TToporReturnVal::RET_UNSAT);
	assert!(s.IsAssumptionRequired(0) && !s.IsAssumptionRequired(1) && s.IsAssumptionRequired(2));
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert!(s.GetLitValue(4) == Unsat || s.GetLitValue(5) == Unsat);
	assert!(!s.IsError());
}

#[test]
fn score_hints()
{
	let mut plain = NewSolver::<u32, false>(false);
	let mut s = NewSolver::<u32, false>(false);
	s.SetParam("/decision/score_hints/mult", 3.);
	assert!(s.HintScore(5, 0.5));
	// A variable, which doesn't exist yet
	assert!(s.HintScore(9, 2.));
	assert_eq!(Solve(&mut plain, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	// The boost is mult * score (times the activity increment, which is 1 before the first conflict)
	assert_eq!(s.GetVarScore(5) - plain.GetVarScore(5), 1.5);
	assert_eq!(s.GetVarScore(4), plain.GetVarScore(4));
	// The variable with the highest score is decided first
	assert_eq!(s.GetLitDecLevel(5), 1);
	assert_eq!(s.m_Topi.m_ExtHints.m_PendingScores, [(9, 2.)]);
	s.AddClause(&[9, 7]);
	assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_SAT);
	assert!(s.m_Topi.m_ExtHints.m_PendingScores.is_empty());
	plain.AddClause(&[9, 7]);
	assert_eq!(Solve(&mut plain, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(s.GetVarScore(9) - plain.GetVarScore(9), 6.);
	// Phase hints are still off
	assert!(!s.HintPhase(-1));
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

	fn Clauses(&mut self, vars: TLit, clss: usize) -> Vec<[TLit; 3]>
	{
		(0..clss).map(|_| [self.Lit(vars), self.Lit(vars), self.Lit(vars)]).collect()
	}
}

#[test]
fn hints_file_round_trip()
{
	// A satisfiable formula with conflicts: the first such one of the random ones
	let vars = 60;
	let (clss, first) = (0..).find_map(|seed|
	{
		let clss = TRand(seed).Clauses(vars, 225);
		let mut first = CTopor::<u32, false>::new(0);
		for c in &clss
		{
			first.AddClause(c);
		}
		(Solve(&mut first, &[]) == TToporReturnVal::RET_SAT && first.GetConflictsNumber() > 0).then_some((clss, first))
	}).unwrap();
	let hints = first.WriteHints();
	assert!(hints.starts_with("c Intel(R) SAT Solver hints"));
	// 20 phases a line, and the terminating 0
	assert_eq!(hints.lines().filter(|l| l.starts_with("v ")).count(), 4);
	assert!(hints.contains("\nv 0\nb "));
	// The scores are ranks, normalized to (0, 1]
	let scores: Vec<f64> = hints.lines().filter(|l| l.starts_with("b ")).map(|l| l.split(' ').nth(2).unwrap().parse().unwrap()).collect();
	assert!(scores.iter().all(|s| *s > 0. && *s <= 1.) && scores.contains(&1.));

	// The model as phase hints: the second run decides its way straight into the model
	let mut second = CTopor::<u32, false>::new(0);
	second.SetParam("/phase/hints/on", 1.);
	second.SetParam("/decision/score_hints/mult", 10.);
	for c in &clss
	{
		second.AddClause(c);
	}
	assert_eq!(second.ReadHints(&hints), Ok((vars as usize, scores.len())));
	assert_eq!(Solve(&mut second, &[]), TToporReturnVal::RET_SAT);
	assert_eq!(second.GetConflictsNumber(), 0);
	assert_eq!(Model(&second, 1..=vars), Model(&first, 1..=vars));

	// The formats ReadHints accepts and rejects
	let mut s = NewSolver::<u32, false>(true);
	s.SetParam("/decision/score_hints/mult", 1.);
	assert_eq!(s.ReadHints("c a comment\ns SATISFIABLE\nv 1 -2 0\n\n  3 -4 0\nb 2 0.5\nv 0\n"), Ok((4, 1)));
	assert_eq!(s.m_Topi.m_ExtHints.m_PendingPhases, [1, -2, 3, -4]);
	assert_eq!(s.m_Topi.m_ExtHints.m_PendingScores, [(2, 0.5)]);
	assert_eq!(s.ReadHints("v 1\nv 2 x 0\n"), Err("hints: cannot parse line 2: v 2 x 0".to_string()));
	assert_eq!(s.ReadHints("b 1\n"), Err("hints: cannot parse line 1: b 1".to_string()));
	assert_eq!(s.ReadHints("b x 1\n"), Err("hints: cannot parse line 1: b x 1".to_string()));
	assert_eq!(s.ReadHints("b 1 y\n"), Err("hints: cannot parse line 1: b 1 y".to_string()));

	// An empty solver
	assert_eq!(CTopor::<u32, false>::new(0).WriteHints().lines().last(), Some("v 0"));
	// A variable, which the solver doesn't know, is skipped
	let mut s = CTopor::<u32, false>::new(0);
	s.AddClause(&[1, -3]);
	assert_eq!(s.WriteHints().lines().nth(1), Some("v 1 3 0"));
	// Exactly one full line of phases
	let mut s = CTopor::<u32, false>::new(0);
	s.AddClause(&(1..=20).collect::<Vec<TLit>>());
	assert_eq!(s.WriteHints().lines().filter(|l| l.starts_with('v')).collect::<Vec<_>>(), ["v 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20", "v 0"]);
}

#[test]
fn hints_are_dumped()
{
	let d = std::env::temp_dir().join(format!("intel_sat_solver_tests_{}_ext_hints_dump", std::process::id()));
	std::fs::create_dir_all(&d).unwrap();
	let mut s = CTopor::<u32, false>::new(0);
	s.m_Topi.DumpSetUp(d.join("hints").to_str().unwrap());
	s.SetParam("/phase/hints/on", 1.);
	s.SetParam("/decision/score_hints/mult", 2.);
	assert!(s.HintPhase(-3));
	assert!(s.HintScore(5, 0.25));
	drop(s);
	let files: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().path()).collect();
	assert_eq!(files.len(), 1);
	assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), "r /phase/hints/on 1\nr /decision/score_hints/mult 2\nlh -3\nls 5 0.25\n");
	let _ = std::fs::remove_dir_all(&d);
}

// Random formulas, hints and incremental queries in every instantiation: every answer is checked against the truth table, and every model against the clauses
#[test]
fn hints_never_change_an_answer()
{
	fn IsSat(vars: TLit, clss: &[Vec<TLit>], assumps: &[TLit]) -> bool
	{
		let holds = |a: u32, l: TLit| ((a >> (l.abs() - 1)) & 1 == 1) == (l > 0);
		(0..1u32 << vars).any(|a| assumps.iter().all(|l| holds(a, *l)) && clss.iter().all(|c| c.iter().any(|l| holds(a, *l))))
	}

	fn Test<Ind: TUInd, const Compress: bool>(seed: u64)
	{
		let mut rand = TRand(seed);
		let vars = 6 + rand.Next(6) as TLit;
		let mut clss: Vec<Vec<TLit>> = rand.Clauses(vars, (vars as f64 * 3.6) as usize).iter().map(|c| c.to_vec()).collect();
		let mut s = CTopor::<Ind, Compress>::new(0);
		s.SetParam("/phase/hints/on", 1.);
		s.SetParam("/decision/score_hints/mult", [0., 1., 100.][rand.Next(3) as usize]);
		if rand.Next(4) == 0
		{
			s.SetParam("/decision/polarity/init_strategy", rand.Next(3) as f64);
		}
		for c in &clss
		{
			s.AddClause(c);
		}
		for query in 0..6
		{
			for _ in 0..rand.Next(2 * vars as u64)
			{
				// Also for variables, which don't exist
				s.HintPhase(rand.Lit(vars + 2));
				s.HintScore(rand.Lit(vars + 2).abs(), rand.Next(100) as f64 / 10.);
			}
			if rand.Next(3) == 0
			{
				// (of a variable, which exists: fixing the polarity of a new variable and then adding a clause is an upstream issue, see README.md)
				let l = clss[rand.Next(clss.len() as u64) as usize][0];
				s.FixPolarity(if rand.Next(2) == 0 { l } else { -l }, rand.Next(2) == 0);
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
	}

	for seed in 0..150
	{
		Test::<u32, false>(seed);
		Test::<u64, false>(seed);
		Test::<u64, true>(seed);
	}
}
