// Unit tests of the Rust-only vivification of learnt clauses (topi_ext_vivify.rs)

use crate::fuzzing::*;
use crate::topi::*;
use crate::topor::*;
use std::cell::RefCell;
use std::rc::Rc;

const NoTimeout: (f64, bool) = (f64::MAX, true);
const Sat: TToporLitVal = TToporLitVal::VAL_SATISFIED;
const Unsat: TToporLitVal = TToporLitVal::VAL_UNSATISFIED;
const RetSat: TToporReturnVal = TToporReturnVal::RET_SAT;
const RetUnsat: TToporReturnVal = TToporReturnVal::RET_UNSAT;

// Inprocessing at the start of every query and after every conflict, which leaves the solver at the level of the assumptions, and no limit on the effort
const VivifyOn: [(&str, f64); 4] = [("/inprocessing/on", 1.), ("/inprocessing/conflicts_before_next", 1.), ("/inprocessing/vivify/on", 1.), ("/inprocessing/vivify/effort", 1e9)];

fn Solve<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, assumps: &[TLit]) -> TToporReturnVal
{
	s.Solve(assumps, NoTimeout, u64::MAX)
}

// A query of a crafted test, which starts with a round: the model of the previous query is taken off the trail (the query would be over before inprocessing otherwise),
// and the search is given a propagation (the budget of a round is a fraction of the propagations of the search since the previous round, and there might have been none)
fn SolveWithBudget<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, assumps: &[TLit]) -> TToporReturnVal
{
	s.Backtrack(0);
	s.m_Topi.m_Stat.m_Implications += 1;
	Solve(s, assumps)
}

// Set the saved phases of the variables of the literals (the solver is at level 0), so that the search decides them to the literals
fn SetPhases<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, lits: &[TLit])
{
	for l in lits
	{
		let li = s.m_Topi.E2I(*l);
		s.m_Topi.m_AssignmentInfo[GetVar(li) as usize].set_m_IsNegated(IsNeg(li));
	}
}

// (tried, shortened, literals removed, units)
fn Counts<Ind: TUInd, const Compress: bool>(s: &CTopor<Ind, Compress>) -> [u64; 4]
{
	let st = Stat(s);
	[st[1], st[2], st[3], st[4]]
}

// (rounds, tried, shortened, literals removed, units, propagations)
fn Stat<Ind: TUInd, const Compress: bool>(s: &CTopor<Ind, Compress>) -> [u64; 6]
{
	let st = s.m_Topi.GetStatistics();
	[st.m_ExtVivifyRounds, st.m_ExtVivifyTried, st.m_ExtVivifyShortened, st.m_ExtVivifyLitsRemoved, st.m_ExtVivifyUnits, st.m_ExtVivifyPropagations]
}

// The same of a statistics line of the extension
fn ParseStat(line: &str) -> [u64; 6]
{
	let t: Vec<&str> = line.split_whitespace().collect();
	assert_eq!((t[0], t[1], t[2]), ("c", "ToporExt", "VivifyRounds"));
	[3, 5, 7, 9, 11, 13].map(|i| t[i].parse().unwrap())
}

fn Sorted(mut c: Vec<TLit>) -> Vec<TLit>
{
	c.sort_unstable_by_key(|l| (l.abs(), *l));
	c.dedup();
	c
}

// The long learnt clauses of the solver in external literals, each one sorted
fn LearntLongs<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>) -> Vec<Vec<TLit>>
{
	let t = &mut s.m_Topi;
	let mut res = vec![];
	let mut clsInd = t.ClsLoopFirst(true);
	while !t.ClsLoopCompleted()
	{
		if !t.ClsChunkDeleted(clsInd) && t.ClsGetIsLearnt(clsInd)
		{
			let cls = t.Cls(clsInd);
			res.push(Sorted((0..cls.size()).map(|i| t.GetExternalLit(cls.get(i))).collect()));
		}
		clsInd = t.ClsLoopNext();
	}
	res.sort();
	res
}

// (glue, activity, skipdel) of the only long learnt clause
fn OnlyLearntInfo<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>) -> (TUV, f32, bool)
{
	let t = &mut s.m_Topi;
	let mut res = vec![];
	let mut clsInd = t.ClsLoopFirst(true);
	while !t.ClsLoopCompleted()
	{
		if !t.ClsChunkDeleted(clsInd) && t.ClsGetIsLearnt(clsInd)
		{
			res.push((t.ClsGetGlue(clsInd), t.ClsGetActivity(clsInd), t.ClsGetSkipdel(clsInd)));
		}
		clsInd = t.ClsLoopNext();
	}
	assert_eq!(res.len(), 1);
	res[0]
}

fn HasBin<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, a: TLit, b: TLit) -> bool
{
	let (la, lb) = (s.m_Topi.E2I(a), s.m_Topi.E2I(b));
	s.m_Topi.WLBinaryWatchExists(la, lb) && s.m_Topi.WLBinaryWatchExists(lb, la)
}

type TSink = CSharedSink;

// The textual proof without the comments (the solver writes one at the end of every query)
fn DratText(sink: &TSink) -> String
{
	String::from_utf8(sink.m_Buf.borrow().clone()).unwrap().lines().filter(|l| !l.starts_with('c')).map(|l| l.to_string() + "\n").collect()
}

// A solver with the given clauses and, after a first query (which is satisfiable without a conflict), one more long learnt clause, as if conflict analysis had learnt it:
// the next query starts with inprocessing, so with a vivification round over that clause
// glue: the number of decision levels, which the literals of the learnt clause are spread over, when it is added; activity, skipdel: its other fields
fn Craft<Ind: TUInd, const Compress: bool>(params: &[(&str, f64)], isDratBinary: bool, clss: &[&[TLit]], learnt: &[TLit], glue: usize, activity: f32, skipdel: bool) -> (CTopor<Ind, Compress>, TSink)
{
	CraftWithSink(CSharedSink::default(), params, isDratBinary, clss, learnt, glue, activity, skipdel)
}

// The same with the given sink of the proof
fn CraftWithSink<Ind: TUInd, const Compress: bool>(sink: TSink, params: &[(&str, f64)], isDratBinary: bool, clss: &[&[TLit]], learnt: &[TLit], glue: usize, activity: f32, skipdel: bool) -> (CTopor<Ind, Compress>, TSink)
{
	let mut s = CTopor::<Ind, Compress>::new(0);
	s.DumpDrat(Box::new(sink.clone()), isDratBinary, false);
	for (name, val) in params
	{
		s.SetParam(name, *val);
	}
	for c in clss
	{
		s.AddClause(c);
	}
	// (a variable of the learnt clause, which is in no clause of the user)
	learnt.iter().for_each(|l| s.CreateInternalLit(*l));
	assert_eq!(Solve(&mut s, &[]), RetSat);
	assert_eq!(s.m_Topi.GetStatistics().m_Conflicts, 0);
	s.Backtrack(0);

	// The literals of the clause are falsified for the moment (the glue is computed out of their decision levels)
	let t = &mut s.m_Topi;
	let mut lits: Vec<TULit> = learnt.iter().map(|l| t.E2I(*l)).collect();
	for (i, l) in lits.iter().enumerate()
	{
		if i < glue
		{
			t.NewDecLevel();
		}
		let decLevel = t.m_DecLevel;
		t.Assign(Negate(*l), CTopi::<Ind, Compress>::BadClsInd, BadULit, decLevel, false, false);
	}
	let clsInd = t.AddClsToBufferAndWatch(Span::from_slice(&mut lits), true, false);
	t.ClsSetActivity(clsInd, activity);
	t.ClsSetSkipdel(clsInd, skipdel);
	t.Backtrack(0, false, false);
	assert_eq!(LearntLongs(&mut s), [Sorted(learnt.to_vec())]);
	(s, sink)
}

// The model of the latest query satisfies the clauses
fn AssertModel<Ind: TUInd, const Compress: bool>(s: &CTopor<Ind, Compress>, clss: &[&[TLit]])
{
	assert!(clss.iter().all(|c| c.iter().any(|l| s.GetLitValue(*l) != Unsat)));
}

// The test is instantiated for the three solver modes of the tool
macro_rules! ForAllModes
{
	($test:ident) =>
	{
		$test::<u32, false>();
		$test::<u64, false>();
		$test::<u64, true>();
	};
}

#[test]
fn off_by_default()
{
	fn Run(params: &[(&str, f64)]) -> (TToporReturnVal, u64, u64, u64, [u64; 6], String)
	{
		// A random 3-SAT formula, which takes a few hundred conflicts
		let mut rand = TRand(5);
		let mut s = CTopor::<u32, false>::new(0);
		for (name, val) in params
		{
			s.SetParam(name, *val);
		}
		for _ in 0..850
		{
			s.AddClause(&[rand.Lit(200), rand.Lit(200), rand.Lit(200)]);
		}
		let ret = Solve(&mut s, &[]);
		let st = s.m_Topi.GetStatistics();
		(ret, st.m_Conflicts, st.m_Decisions, st.m_Implications, Stat(&s), s.GetStatStrShort(false))
	}

	let base = Run(&[]);
	assert!(base.1 > 100);
	assert_eq!(base.4, [0; 6]);
	assert!(!base.5.contains("Vivify"));
	// Neither the parameters of vivification without /inprocessing/vivify/on, nor /inprocessing/vivify/on without inprocessing change anything
	let inprocessing = [("/inprocessing/on", 1.), ("/inprocessing/conflicts_before_next", 1.)];
	for params in [&[("/inprocessing/vivify/effort", 5.), ("/inprocessing/vivify/max_glue", 3.)][..], &[("/inprocessing/vivify/on", 1.), ("/inprocessing/vivify/effort", 1e9)], &inprocessing, &[inprocessing[0], inprocessing[1], ("/inprocessing/vivify/effort", 1e9)]]
	{
		let res = Run(params);
		assert_eq!((res.0, res.1, res.2, res.3, res.4), (base.0, base.1, base.2, base.3, [0; 6]), "{:?}", params);
	}
	// With everything on the clauses are vivified
	let on = Run(&VivifyOn);
	assert_eq!(on.0, base.0);
	assert!(on.4[0] > 0 && on.4[1] > 0 && on.4[2] > 0 && on.4[3] >= on.4[2] && on.4[5] > 0, "{:?}", on.4);
	assert!(on.5.contains(&format!("c ToporExt VivifyRounds {} Tried {} Shortened {} LitsRemoved {} Units {} Props {} SearchProps {}\n", on.4[0], on.4[1], on.4[2], on.4[3], on.4[4], on.4[5], on.3 - on.4[5])));
	assert_eq!(ParseStat(on.5.lines().find(|l| l.contains("VivifyRounds")).unwrap()), on.4);
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
}

// (a) BCP finds a contradiction after a prefix of the literals has been decided: the clause shrinks to the decisions, which the contradiction depends on
#[test]
fn contradiction_shortens_to_the_decisions_in_its_cone()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// not 1, not 3 => 6 and not 6 (2 has nothing to do with it); the learnt clause 1 2 3 4 5 becomes 1 3
		let clss: [&[TLit]; 2] = [&[1, 3, 6], &[1, 3, -6]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4, 5], 5, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 3, 0]);
		assert!(LearntLongs(&mut s).is_empty() && HasBin(&mut s, 1, 3));
		assert!(s.GetLitValue(1) == Sat || s.GetLitValue(3) == Sat);
		// The proof: the new clause is added before the old one is deleted
		assert_eq!(DratText(&sink), "1 3 0\nd 1 2 3 4 5 0\n");

		// The new clause is long: 1 2 4 out of 1 2 3 4 5 6; it inherits the glue (which is lower than the 3 decision levels of the test), the activity and the skip-deletion flag
		let clss: [&[TLit]; 2] = [&[1, 2, 4, 7], &[1, 2, 4, -7]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4, 5, 6], 2, 3.5, true);
		assert_eq!(OnlyLearntInfo(&mut s), (2, 3.5, true));
		let longsBefore = s.m_Topi.GetStatistics().m_ActiveLongClss;
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 3, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 4]]);
		assert_eq!(OnlyLearntInfo(&mut s), (2, 3.5, true));
		assert_eq!(s.m_Topi.GetStatistics().m_ActiveLongClss, longsBefore);
		assert_eq!(s.GetActiveLongLearntClss(), 1);
		assert_eq!(DratText(&sink), "1 2 4 0\nd 1 2 3 4 5 6 0\n");

		// The glue of the old clause is higher than the number of the literals left: the new glue is that of the test
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4, 5, 6], 6, 0., false);
		assert_eq!(OnlyLearntInfo(&mut s), (6, 0., false));
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 4]]);
		assert_eq!(OnlyLearntInfo(&mut s), (3, 0., false));

		// The same without clause deletion (the activity isn't kept then), and in a binary proof: a -1 -2 -4 0 d -1 -2 3 -4 5 70 0, a literal l being encoded as 2 * |l| + (l < 0)
		// in groups of 7 bits, the lowest group first (140 for the literal 70: 0x8c 0x01)
		let (mut s, sink) = Craft::<Ind, Compress>(&[VivifyOn[0], VivifyOn[1], VivifyOn[2], VivifyOn[3], ("/deletion/clause/strategy", 0.)], true, &[&[-1, -2, -4, 7], &[-1, -2, -4, -7]], &[-1, -2, 3, -4, 5, 70], 6, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(LearntLongs(&mut s), [[-1, -2, -4]]);
		assert_eq!(*sink.m_Buf.borrow(), [b'a', 3, 5, 9, 0, b'd', 3, 5, 6, 9, 10, 0x8c, 1, 0]);
	}
	ForAllModes!(Test);
}

// (b) A literal of the clause is implied by the negations of the literals before it: the clause shrinks to that literal and the decisions it depends on
#[test]
fn implied_literal_shortens_to_itself_and_the_decisions_in_its_cone()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// not 1, not 2 => 4, through a long clause: 1 2 3 4 5 becomes 4 1 2
		let clss: [&[TLit]; 1] = [&[1, 2, 4, -6]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4, 5], 5, 0., false);
		// (6 is satisfied at level 0, so the clause 1 2 4 -6 implies 4, once 1 and 2 are falsified)
		s.AddClause(&[6]);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 2, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 4]]);
		assert_eq!(DratText(&sink), "4 1 2 0\nd 1 2 3 4 5 0\n");

		// not 1 => 3, through a binary clause: 1 2 3 4 becomes 3 1
		let clss: [&[TLit]; 1] = [&[1, 3]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4], 4, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [1, 1, 2, 0]);
		assert!(LearntLongs(&mut s).is_empty() && HasBin(&mut s, 1, 3));
		assert_eq!(DratText(&sink), "3 1 0\nd 1 2 3 4 0\n");
	}
	ForAllModes!(Test);
}

// (c) Literals of the clause are falsified by BCP (or at level 0) rather than by a decision: once all the literals are falsified, the clause itself is the contradiction, and those literals are removed
#[test]
fn falsified_literals_are_removed()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// not 1 => not 2, and 5 is falsified at level 0: 1 2 3 4 5 becomes 1 3 4
		let clss: [&[TLit]; 2] = [&[1, -2], &[-5]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4, 5], 5, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 2, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 3, 4]]);
		assert_eq!(DratText(&sink), "1 3 4 0\nd 1 2 3 4 5 0\n");
		// The new clause is tried at the next query, and it stays; it isn't tried again at the query after that one
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [2, 1, 2, 0]);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [2, 1, 2, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 3, 4]]);

		// A clause, which is satisfied at level 0, is left alone
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &[&[5]], &[1, 2, 3, 4, 5], 5, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [0, 0, 0, 0]);
		assert_eq!(DratText(&sink), "");
	}
	ForAllModes!(Test);
}

// The clause under test is detached, so it is never the reason of its own shortening
#[test]
fn clause_is_not_a_reason_for_its_own_shortening()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// not 3 => 4 and not 4. Were the learnt clause 1 2 -3 watched during its test, it would imply not 3 after not 1 and not 2, and the contradiction, which follows,
		// would shorten it to 1 2 through itself as a reason. It is ignored instead: not 1, not 2 and 3 are decided without a contradiction, and the clause stays
		let clss: [&[TLit]; 2] = [&[3, 4], &[3, -4]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, -3], 3, 1.5, false);
		// (the search then satisfies the clauses without a conflict)
		SetPhases(&mut s, &[1, 2, 3, 4]);
		let longsBefore = s.m_Topi.GetStatistics().m_ActiveLongClss;
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 0, 0, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, -3]]);
		assert_eq!(OnlyLearntInfo(&mut s), (3, 1.5, false));
		assert_eq!(s.m_Topi.GetStatistics().m_ActiveLongClss, longsBefore);
		assert_eq!(DratText(&sink), "");
		// The clause is watched again: under not 1 and not 2 it implies not 3, which is contradictory
		assert_eq!(SolveWithBudget(&mut s, &[-1, -2]), RetUnsat);
		assert!(s.IsAssumptionRequired(0) && s.IsAssumptionRequired(1));
		// It is not tried again
		assert_eq!(Stat(&s)[1], 1);

		// Likewise, the clause doesn't satisfy itself: were 1 2 3 watched, 3 would be implied by the clause itself, once 1 and 2 are falsified
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &[&[1, 2, 4]], &[1, 2, 3], 3, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [1, 0, 0, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 3]]);
	}
	ForAllModes!(Test);
}

// A clause shrinks to a unit clause, which is assigned and propagated at level 0
#[test]
fn unit_clauses_and_global_contradiction()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// not 1 => 4 and not 4: 1 2 3 becomes 1, and 1 => 5 at level 0
		let clss: [&[TLit]; 3] = [&[1, 4], &[1, -4], &[-1, 5]];
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3], 3, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 2, 1]);
		assert!(LearntLongs(&mut s).is_empty());
		assert_eq!((s.GetLitValue(1), s.GetLitValue(5)), (Sat, Sat));
		assert_eq!((s.GetLitDecLevel(1), s.GetLitDecLevel(5)), (0, 0));
		assert_eq!(DratText(&sink), "1 0\nd 1 2 3 0\n");

		// The unit clause completes the assignment: the round ends the query with a model, and there is nothing left to decide
		let clss: [&[TLit]; 5] = [&[1, 4], &[1, -4], &[-1, 2], &[-1, 3], &[-1, 4]];
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3], 3, 0., false);
		let decisions = s.m_Topi.GetStatistics().m_Decisions;
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(s.m_Topi.GetStatistics().m_Decisions, decisions);
		assert_eq!(*s.m_Topi.m_StatusExplanation.borrow(), "A model: discovered after vivification");

		// The unit clause is contradictory at level 0: 1 => 5 and not 5 (the last clause is added after the learnt one, since the formula is unsatisfiable with it)
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, &[&[1, 4], &[1, -4], &[-1, 5]], &[1, 2, 3], 3, 0., false);
		s.AddClause(&[-1, -5]);
		let conflicts = s.m_Topi.GetStatistics().m_Conflicts;
		// (the search itself needs conflicts for it; the round runs before the first decision)
		assert_eq!(SolveWithBudget(&mut s, &[]), RetUnsat);
		assert_eq!(s.m_Topi.GetStatistics().m_Conflicts, conflicts);
		assert_eq!(Counts(&s), [1, 1, 2, 1]);
		assert_eq!(*s.m_Topi.m_StatusExplanation.borrow(), "Global contradiction: discovered by vivification");
		assert_eq!(DratText(&sink), "1 0\nd 1 2 3 0\n0\n");
		// The solver stays contradictory
		assert_eq!(SolveWithBudget(&mut s, &[2]), RetUnsat);
		assert!(!s.IsAssumptionRequired(0));
	}
	ForAllModes!(Test);
}

// A round under assumptions: the assumptions are taken off the trail and re-assigned afterwards
#[test]
fn assumptions_are_reassigned_after_a_round()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// not 1 => 4 and not 4, so the learnt clause 1 2 3 becomes the unit clause 1; the assumptions 6 and 7 imply 8, and 9 is free
		let clss: [&[TLit]; 3] = [&[1, 4], &[1, -4], &[-6, -7, 8]];
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3], 3, 0., false);
		s.AddClause(&[9, 10]);
		assert_eq!(SolveWithBudget(&mut s, &[6, 7, -9]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 2, 1]);
		assert_eq!((s.GetLitValue(1), s.GetLitValue(6), s.GetLitValue(7), s.GetLitValue(8), s.GetLitValue(9), s.GetLitValue(10)), (Sat, Sat, Sat, Sat, Unsat, Sat));
		// The unit clause is at level 0, below the assumptions, which are decisions again
		assert_eq!(s.GetLitDecLevel(1), 0);
		assert!(s.GetLitDecLevel(6) > 0 && s.GetLitDecLevel(7) > s.GetLitDecLevel(6) && s.GetLitDecLevel(8) == s.GetLitDecLevel(7) && s.GetLitDecLevel(-9) > s.GetLitDecLevel(7));

		// The unit clause of the round falsifies an assumption at level 0 (1 => 5, and 1 and 5 => 6; not 6 on its own implies nothing): the query is unsatisfiable, and the core is that assumption
		let clss: [&[TLit]; 4] = [&[1, 4], &[1, -4], &[-1, 5], &[-1, -5, 6]];
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3], 3, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[7, -6, -2]), RetUnsat);
		assert_eq!(Counts(&s), [1, 1, 2, 1]);
		assert_eq!(*s.m_Topi.m_StatusExplanation.borrow(), "Assumption flipped after vivification!");
		assert_eq!((s.IsAssumptionRequired(0), s.IsAssumptionRequired(1), s.IsAssumptionRequired(2)), (false, true, false));
		// The solver goes on: without that assumption the query is satisfiable
		assert_eq!(SolveWithBudget(&mut s, &[7, -2]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!((s.GetLitValue(1), s.GetLitValue(5), s.GetLitValue(6), s.GetLitValue(7), s.GetLitValue(2)), (Sat, Sat, Sat, Sat, Unsat));

		// The unit clause of the round contradicts an assumption, once the assumption is back on the trail (1 and not 6 => 8 and not 8)
		let clss: [&[TLit]; 4] = [&[1, 4], &[1, -4], &[-1, 6, 8], &[-1, 6, -8]];
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3], 3, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[7, -6, -2]), RetUnsat);
		assert_eq!(Counts(&s), [1, 1, 2, 1]);
		assert_eq!((s.IsAssumptionRequired(0), s.IsAssumptionRequired(1), s.IsAssumptionRequired(2)), (false, true, false));
		assert_eq!(SolveWithBudget(&mut s, &[7, -2]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!((s.GetLitValue(1), s.GetLitValue(6), s.GetLitValue(7), s.GetLitValue(2)), (Sat, Sat, Sat, Unsat));

		// The test of the clause falsifies assumptions (by a decision: not 1, and by an implication: not 1 => not 6), which doesn't make the query unsatisfiable:
		// the clause 1 2 3 5 becomes 1 2 3 (5 is falsified at level 0), and the query is satisfiable under 1 and 6
		let clss: [&[TLit]; 2] = [&[1, -6], &[-5]];
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 5], 4, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[1, 6]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 1, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 3]]);
		assert_eq!((s.GetLitValue(1), s.GetLitValue(6)), (Sat, Sat));
		// The assumptions contradict the formula on their own: the core is right, whatever the test of the clause recorded
		assert_eq!(SolveWithBudget(&mut s, &[6, 2, -1]), RetUnsat);
		assert_eq!((s.IsAssumptionRequired(0), s.IsAssumptionRequired(1), s.IsAssumptionRequired(2)), (true, false, true));
	}
	ForAllModes!(Test);
}

// The saved phases are those of the search: backtracking after the test of a clause would have saved the negations of its literals
#[test]
fn saved_phases_and_user_polarities_are_kept()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// The test of the clause 1 2 3 4 (which stays) decides not 1, not 2, not 3 and not 4; the saved phases are 1, 2, not 3, 4 and 5, and the user fixes not 2
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &[&[1, 2, 3, 4, 5]], &[1, 2, 3, 4], 4, 0., false);
		SetPhases(&mut s, &[1, 2, -3, 4, 5]);
		s.FixPolarity(-2, false);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [1, 0, 0, 0]);
		assert_eq!((1..=5).map(|v| s.GetLitValue(v)).collect::<Vec<_>>(), [Sat, Unsat, Unsat, Sat, Sat]);
		assert_eq!(s.m_Topi.GetStatistics().m_Conflicts, 0);
	}
	ForAllModes!(Test);
}

// The budget of a round is a fraction of the propagations of the search since the previous round, and the candidates are ordered by glue
#[test]
fn effort_and_candidates()
{
	fn Run(effort: f64, maxGlue: f64) -> ([u64; 6], u64, u64)
	{
		let mut rand = TRand(11);
		let mut s = CTopor::<u32, false>::new(0);
		for (name, val) in [("/inprocessing/on", 1.), ("/inprocessing/conflicts_before_next", 100.), ("/inprocessing/vivify/on", 1.), ("/inprocessing/vivify/effort", effort), ("/inprocessing/vivify/max_glue", maxGlue), ("/restarts/numeric/conflict_thr", 50.)]
		{
			s.SetParam(name, val);
		}
		for _ in 0..850
		{
			s.AddClause(&[rand.Lit(200), rand.Lit(200), rand.Lit(200)]);
		}
		let ret = Solve(&mut s, &[]);
		assert!(ret == RetSat || ret == RetUnsat);
		let st = s.m_Topi.GetStatistics();
		(Stat(&s), st.m_Implications, st.m_Conflicts)
	}

	// No effort, no rounds
	assert_eq!(Run(0., 1e9).0, [0; 6]);
	for effort in [0.05, 0.3]
	{
		let (st, props, confs) = Run(effort, 1e9);
		assert!(confs > 1000 && st[0] > 2 && st[1] > 0);
		// The search propagated props - st[5] literals; a round may exceed its budget by the propagations of its last clause only
		let searchProps = (props - st[5]) as f64;
		assert!(st[5] as f64 <= effort * searchProps + (st[0] * 200) as f64, "effort {}: {} of {}", effort, st[5], searchProps);
		assert!(st[5] as f64 >= 0.5 * effort * searchProps, "effort {}: {} of {}", effort, st[5], searchProps);
	}
	// Only the clauses of the glue 2 at most are tried: fewer than without the bound
	let (all, bounded) = (Run(1e9, 1e9).0, Run(1e9, 2.).0);
	assert!(bounded[1] > 0 && bounded[1] < all[1] / 2, "{:?} {:?}", bounded, all);
}

// A stop request of the new-learnt-clause callback, when it is given the new clause: the query is interrupted, and the solver is consistent
#[test]
fn stop_request_during_a_round()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		let clss: [&[TLit]; 2] = [&[1, 2, 4, 7], &[1, 2, 4, -7]];
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &[1, 2, 3, 4, 5, 6], 6, 0., false);
		let reported: Rc<RefCell<Vec<Vec<TLit>>>> = Default::default();
		let reportedCb = reported.clone();
		s.SetCbNewLearntCls(Some(Box::new(move |c: &[TLit]|
		{
			reportedCb.borrow_mut().push(c.to_vec());
			TStopTopor::VAL_STOP
		})));
		assert_eq!(SolveWithBudget(&mut s, &[5]), TToporReturnVal::RET_USER_INTERRUPT);
		assert_eq!(*reported.borrow(), [[1, 2, 4]]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 4]]);
		s.SetCbNewLearntCls(None);
		assert_eq!(SolveWithBudget(&mut s, &[-1, -2, 5]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!((s.GetLitValue(4), s.GetLitValue(5)), (Sat, Sat));
	}
	ForAllModes!(Test);
}

// A failure to write the proof during a round makes the solver unusable, as any other failure to write it does; its clauses are consistent
#[test]
fn proof_write_failures()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		let clss: [&[TLit]; 2] = [&[1, 2, 4, 7], &[1, 2, 4, -7]];
		// The proof before the round is the comment "c query completed 1"; the round writes "1 2 4 0" and "d 1 2 3 4 5 6 0"
		let written = "c query completed 1\n".len();
		// The deletion can't be written: the new clause has been added, and the old one is not deleted
		let sink = CSharedSink { m_Buf: Default::default(), m_FailAfter: Some(written + "1 2 4 0\n".len() + 3) };
		let (mut s, sink) = CraftWithSink::<Ind, Compress>(sink, &VivifyOn, false, &clss, &[1, 2, 3, 4, 5, 6], 6, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), TToporReturnVal::RET_DRAT_FILE_PROBLEM);
		assert_eq!(DratText(&sink), "1 2 4 0\n");
		assert_eq!(LearntLongs(&mut s), [vec![1, 2, 3, 4, 5, 6], vec![1, 2, 4]]);
		assert_eq!(Counts(&s), [1, 0, 0, 0]);
		assert!(s.IsError());

		// The new clause can't be written
		let sink = CSharedSink { m_Buf: Default::default(), m_FailAfter: Some(written + 3) };
		let (mut s, sink) = CraftWithSink::<Ind, Compress>(sink, &VivifyOn, false, &clss, &[1, 2, 3, 4, 5, 6], 6, 0., false);
		assert_eq!(SolveWithBudget(&mut s, &[]), TToporReturnVal::RET_DRAT_FILE_PROBLEM);
		assert_eq!(DratText(&sink), "");
		assert!(LearntLongs(&mut s).contains(&vec![1, 2, 3, 4, 5, 6]));
		assert!(s.IsError());

		// The lines of a simplified clause (see simplification_is_reported_to_the_proof; the round is skipped: it has no budget): the deletion can't be written, or the simplified clause can't
		for failAfter in ["1 2 3 4 0\n".len() + 3, 3]
		{
			let sink = CSharedSink { m_Buf: Default::default(), m_FailAfter: Some(written + failAfter) };
			let (mut s, sink) = CraftWithSink::<Ind, Compress>(sink, &[VivifyOn[0], VivifyOn[1], VivifyOn[2], ("/inprocessing/vivify/effort", 1e-9)], false, &[&[1, 2, 3, 4, 5, 6]], &[1, 2, 3, 4, 5], 5, 0., false);
			s.AddClause(&[-5]);
			s.m_Topi.m_ImplicationsTillNextSimplify = 0;
			assert_eq!(SolveWithBudget(&mut s, &[]), TToporReturnVal::RET_DRAT_FILE_PROBLEM);
			assert_eq!(DratText(&sink).lines().count(), (failAfter > 3) as usize);
			assert_eq!(Stat(&s), [0; 6]);
			assert!(s.IsError());
		}
	}
	ForAllModes!(Test);
}

// The hashes of the clauses, which were tried in vain, are dropped, once there are too many of them: the set of the hashes is bounded
#[test]
fn tried_clauses_are_forgotten_eventually()
{
	use crate::topi_ext_vivify::{CExtVivifyTried, ExtVivifyMaxTried};
	let (mut s, _) = Craft::<u32, false>(&VivifyOn, false, &[&[1, 2, 3, 4, 5]], &[1, 2, 3, 4], 4, 0., false);
	assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
	assert_eq!((Counts(&s), s.m_Topi.m_ExtVivify.m_Tried.size()), ([1, 0, 0, 0], 1));
	assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
	assert_eq!((Counts(&s), s.m_Topi.m_ExtVivify.m_Tried.size()), ([1, 0, 0, 0], 1));
	// The set is full of other hashes: the clause is tried again, and its hash is then the only one
	let mut tried = CExtVivifyTried::default();
	assert!(!tried.contains(1));
	for hash in 1..=ExtVivifyMaxTried as u64
	{
		// (the multiplier is odd, so the hashes differ, and none of them is 0)
		assert!(tried.insert(hash.wrapping_mul(0x9E3779B97F4A7C15)));
	}
	assert_eq!(tried.size(), ExtVivifyMaxTried);
	assert!((1..=1000u64).all(|hash| tried.contains(hash.wrapping_mul(0x9E3779B97F4A7C15))) && !tried.contains(12345));
	// (a hash, which the set holds, is not added again)
	assert!(tried.insert(0x9E3779B97F4A7C15));
	assert_eq!(tried.size(), ExtVivifyMaxTried);
	s.m_Topi.m_ExtVivify.m_Tried = tried;
	assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
	assert_eq!((Counts(&s), s.m_Topi.m_ExtVivify.m_Tried.size()), ([2, 0, 0, 0], 1));
	assert!(!s.m_Topi.m_ExtVivify.m_Tried.contains(0x9E3779B97F4A7C15));
	assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
	assert_eq!((Counts(&s), s.m_Topi.m_ExtVivify.m_Tried.size()), ([2, 0, 0, 0], 1));
}

// A request to stop the query, which arrives during a round, ends the round before its next clause, as it ends the search before its next decision:
// an interrupt and a stop request of the StopNow callback make Solve return right away, and a timeout is reported by the loop of Solve after its next conflict analysis
#[test]
fn a_round_is_interruptible()
{
	use std::cell::Cell;
	use std::sync::atomic::Ordering;

	fn Test<Ind: TUInd, const Compress: bool>()
	{
		let clss: [&[TLit]; 2] = [&[1, 2, 4, 7], &[1, 2, 4, -7]];
		let learnt = [1, 2, 3, 4, 5, 6];
		// A round was started, no clause was tried, and the learnt clause is still there
		fn AssertStopped<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>, learnt: &[TLit])
		{
			assert_eq!(Stat(s), [1, 0, 0, 0, 0, 0]);
			assert_eq!(LearntLongs(s), [learnt.to_vec()]);
			assert_eq!(s.m_Topi.m_DecLevel, 0);
		}

		// The StopNow callback: the loop of Solve asks it before inprocessing (the first call), and the round asks it before every clause (the second call)
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &learnt, 6, 0., false);
		let calls: Rc<Cell<u32>> = Default::default();
		let callsCb = calls.clone();
		s.SetCbStopNow(Some(Box::new(move ||
		{
			callsCb.set(callsCb.get() + 1);
			if callsCb.get() == 2 { TStopTopor::VAL_STOP } else { TStopTopor::VAL_CONTINUE }
		})));
		assert_eq!(SolveWithBudget(&mut s, &[5]), TToporReturnVal::RET_USER_INTERRUPT);
		assert_eq!(calls.get(), 2);
		assert_eq!(*s.m_Topi.m_StatusExplanation.borrow(), "Interrupt by the StopNow callback");
		AssertStopped(&mut s, &learnt);
		// The solver goes on: the next query starts with a round, which isn't stopped
		assert_eq!(SolveWithBudget(&mut s, &[5]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 3, 0]);
		assert_eq!(LearntLongs(&mut s), [[1, 2, 4]]);
		assert_eq!(s.GetLitValue(5), Sat);
		assert!(calls.get() > 3);

		// The interrupt: it arrives during the first iteration of the loop of Solve, after the loop has tested it (here: from the StopNow callback, which the loop asks right after that test)
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &learnt, 6, 0., false);
		let interruptNow = s.GetInterruptNowHandle();
		let interruptNowCb = interruptNow.clone();
		s.SetCbStopNow(Some(Box::new(move ||
		{
			interruptNowCb.store(true, Ordering::Relaxed);
			TStopTopor::VAL_CONTINUE
		})));
		assert_eq!(SolveWithBudget(&mut s, &[5, -1]), TToporReturnVal::RET_USER_INTERRUPT);
		assert_eq!(*s.m_Topi.m_StatusExplanation.borrow(), "Interrupt by the Interrupt callback");
		assert!(!interruptNow.load(Ordering::Relaxed));
		AssertStopped(&mut s, &learnt);
		s.SetCbStopNow(None);
		assert_eq!(SolveWithBudget(&mut s, &[5, -1]), RetSat);
		AssertModel(&s, &clss);
		assert_eq!(Counts(&s), [1, 1, 3, 0]);
		assert_eq!((s.GetLitValue(5), s.GetLitValue(1)), (Sat, Unsat));

		// The timeouts expire during the first iteration of the loop of Solve, before the round (the StopNow callback sleeps through them, when the loop asks it): the round tries no clause,
		// and the loop reports the timeout after its first decision. The clocks are read at every test (otherwise at every 1000th one)
		for isGlobal in [false, true]
		{
			let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &clss, &learnt, 6, 0., false);
			let calls: Rc<Cell<u32>> = Default::default();
			let callsCb = calls.clone();
			s.SetCbStopNow(Some(Box::new(move ||
			{
				callsCb.set(callsCb.get() + 1);
				if callsCb.get() == 1
				{
					std::thread::sleep(std::time::Duration::from_millis(400));
				}
				TStopTopor::VAL_CONTINUE
			})));
			s.Backtrack(0);
			let stat = &mut s.m_Topi.m_Stat;
			stat.m_Implications += 1;
			stat.m_OverallTime.SetTestModuloFactor(1);
			stat.m_TimeSinceLastSolveStart.SetTestModuloFactor(1);
			if isGlobal
			{
				stat.m_OverallTime.Reset();
				stat.m_OverallTime.SetTimeout(0.2);
			}
			let ret = s.Solve(&[5], if isGlobal { NoTimeout } else { (0.2, false) }, u64::MAX);
			assert_eq!(ret, if isGlobal { TToporReturnVal::RET_TIMEOUT_GLOBAL } else { TToporReturnVal::RET_TIMEOUT_LOCAL });
			// The loop asked the callback once, and the round asked it once
			assert_eq!(calls.get(), 2);
			assert_eq!(Stat(&s), [1, 0, 0, 0, 0, 0]);
			assert_eq!(LearntLongs(&mut s), [learnt.to_vec()]);
		}
	}
	ForAllModes!(Test);
}

// SimplifyIfRequired removes the literals falsified at level 0 from a learnt clause in place: with vivification on, the proof gets the shortened clause and the deletion of the original one,
// so that the deletion of the clause by a later round names the clause as the proof holds it
#[test]
fn simplification_is_reported_to_the_proof()
{
	fn Test<Ind: TUInd, const Compress: bool>()
	{
		// The learnt clause 1 2 3 4 5 stays in the first round. Then 5 is falsified at level 0; the next round skips the clause (its hash is known), and simplification removes 5 from it
		// (and from the clause of the user, which the proof doesn't hold)
		let Simplified = |params: &[(&str, f64)], isDratBinary: bool| -> (CTopor<Ind, Compress>, TSink)
		{
			let (mut s, sink) = Craft::<Ind, Compress>(params, isDratBinary, &[&[1, 2, 3, 4, 5, 6]], &[1, 2, 3, 4, 5], 5, 0., false);
			assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
			assert!(isDratBinary || DratText(&sink).is_empty());
			s.AddClause(&[-5]);
			s.m_Topi.m_ImplicationsTillNextSimplify = 0;
			assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
			assert_eq!(LearntLongs(&mut s), [[1, 2, 3, 4]]);
			(s, sink)
		};
		let (mut s, sink) = Simplified(&VivifyOn, false);
		assert_eq!(Counts(&s), [1, 0, 0, 0]);
		let ProofClss = |sink: &TSink| -> Vec<(bool, Vec<TLit>)> { DratText(sink).lines().map(|l| (l.starts_with('d'), Sorted(l.split_whitespace().filter_map(|t| t.parse().ok()).filter(|l| *l != 0).collect()))).collect() };
		assert_eq!(ProofClss(&sink), [(false, vec![1, 2, 3, 4]), (true, vec![1, 2, 3, 4, 5])]);
		// The next round tries the clause as simplification has left it and shortens it: the clause deleted is the one the proof holds
		s.AddClause(&[1, 2, 7]);
		s.AddClause(&[1, 2, -7]);
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(Counts(&s), [2, 1, 2, 0]);
		assert_eq!(ProofClss(&sink)[2..], [(false, vec![1, 2]), (true, vec![1, 2, 3, 4])]);

		// A binary proof: a and d records
		let (_, sink) = Simplified(&VivifyOn, true);
		let mut records: Vec<Vec<u8>> = sink.m_Buf.borrow().split(|b| *b == 0).filter(|r| !r.is_empty()).map(|r| { let mut r = r.to_vec(); r[1..].sort(); r }).collect();
		records.sort();
		assert_eq!(records, [vec![b'a', 2, 4, 6, 8], vec![b'd', 2, 4, 6, 8, 10]]);

		// Without the extension the proof says nothing about the removal, as that of the C++ solver
		let (_, sink) = Simplified(&VivifyOn[..2], false);
		assert_eq!(DratText(&sink), "");
		// Likewise if there is no proof to write (the hook has nothing to do)
		let (mut s, _) = Craft::<Ind, Compress>(&VivifyOn, false, &[&[1, 2, 3, 4, 5, 6]], &[1, 2, 3, 4, 5], 5, 0., false);
		s.m_Topi.m_OpenedDratFile = None;
		s.AddClause(&[-5]);
		s.m_Topi.m_ImplicationsTillNextSimplify = 0;
		assert_eq!(SolveWithBudget(&mut s, &[]), RetSat);
		assert_eq!(s.GetActiveLongLearntClss(), 1);
	}
	ForAllModes!(Test);
}

// Memory-out during the rounds: every allocation of a fuzz scenario with vivification on is made to fail in turn (see FuzzAllocFailures in fuzz.rs: whatever happens after
// the injected failure is tolerated, as long as the process survives)
#[test]
#[cfg_attr(not(debug_assertions), ignore = "relies on the debug assertions of the solver: without them it proceeds past a failed assertion, as the C++ solver does with NDEBUG")]
fn alloc_failures_during_rounds()
{
	use crate::topor_dyn_array::alloc_fault;
	SetExtraParams(&VivifyOn);
	let iters: u64 = std::env::var("TOPOR_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(2000) / 100;
	for seed in 0..iters
	{
		for solverMode in 0..3
		{
			let s = GenScenario(&RandomBytes(7000003 * (seed + 1) + solverMode as u64, 20000), true);
			let before = alloc_fault::allocs_so_far();
			RunScenario(&s, solverMode, None);
			let allocs = alloc_fault::allocs_so_far() - before;
			let step = 1 + allocs / 60;
			let mut failAt = seed % step;
			while failAt < allocs
			{
				let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RunScenario(&s, solverMode, Some(failAt))));
				alloc_fault::never_fail();
				failAt += step;
			}
		}
	}
	SetExtraParams(&[]);
	let _ = TakeExtStats();
}

// Memory-out in the allocations of the extension itself, each of which is made to fail (on its own, and together with all the allocations after it): the status is that of any other memory-out,
// nothing has been changed by then, and the solver reports the memory-out from then on. The hooks are invoked directly, so that every allocation is one of the extension
// (the test doesn't depend on what the solver proper does after an allocation of its own fails)
#[test]
fn alloc_failures_of_the_extension_directly()
{
	use crate::topor_dyn_array::alloc_fault;

	fn OnlyLearntInd<Ind: TUInd, const Compress: bool>(s: &mut CTopor<Ind, Compress>) -> Ind
	{
		let t = &mut s.m_Topi;
		let mut res = vec![];
		let mut clsInd = t.ClsLoopFirst(true);
		while !t.ClsLoopCompleted()
		{
			if !t.ClsChunkDeleted(clsInd) && t.ClsGetIsLearnt(clsInd)
			{
				res.push(clsInd);
			}
			clsInd = t.ClsLoopNext();
		}
		assert_eq!(res.len(), 1);
		res[0]
	}

	// The hook of the given kind over a solver at level 0 with one long learnt clause, as at the start of a query: a round, which shortens the clause (0), a round, which doesn't (1),
	// or the hook of the simplification of the clause (2); failAt: the allocation, which fails, and whether it is the only one
	// Returns the number of the allocations
	fn Run<Ind: TUInd, const Compress: bool>(kind: u8, failAt: Option<(u64, bool)>, expected: &str) -> u64
	{
		let (clss, learnt): (&[&[TLit]], &[TLit]) = if kind == 0 { (&[&[1, 2, 4, 7], &[1, 2, 4, -7]], &[1, 2, 3, 4, 5, 6]) } else { (&[&[1, 2, 3, 4, 5]], &[1, 2, 3, 4]) };
		let (mut s, sink) = Craft::<Ind, Compress>(&VivifyOn, false, clss, learnt, learnt.len(), 0., false);
		s.m_Topi.m_Stat.m_Implications += 1;
		s.m_Topi.m_Status.set(TToporStatus::STATUS_UNDECIDED);
		let clsInd = OnlyLearntInd(&mut s);
		let proofBefore = sink.m_Buf.borrow().len();
		let allocsBefore = alloc_fault::allocs_so_far();
		match failAt
		{
			Some((allocs, true)) => alloc_fault::fail_only(allocs),
			Some((allocs, false)) => alloc_fault::fail_after(allocs),
			None => (),
		}
		if kind == 2 { s.m_Topi.ExtVivifyOnGloballyFalsifiedLitsRemoval(clsInd) } else { s.m_Topi.ExtVivifyIfRequired() };
		alloc_fault::never_fail();
		let allocs = alloc_fault::allocs_so_far() - allocsBefore;
		if failAt.is_some()
		{
			assert_eq!(*s.m_Topi.m_StatusExplanation.borrow(), expected);
			assert!(s.m_Topi.m_Status.get() == TToporStatus::STATUS_ALLOC_FAILED && s.IsError());
			// The clause is as it has been, and it is watched; nothing was written to the proof; the solver is at level 0
			assert_eq!(LearntLongs(&mut s), [Sorted(learnt.to_vec())]);
			let (w0, w1) = (s.m_Topi.Cls(clsInd).get(0), s.m_Topi.Cls(clsInd).get(1));
			assert!(s.m_Topi.WLGetLongWatchInd(w0, clsInd) != usize::MAX && s.m_Topi.WLGetLongWatchInd(w1, clsInd) != usize::MAX);
			assert_eq!(sink.m_Buf.borrow().len(), proofBefore);
			assert_eq!(s.m_Topi.m_DecLevel, 0);
			assert_eq!(Stat(&s)[2..5], [0, 0, 0]);
			assert_eq!(Solve(&mut s, &[]), TToporReturnVal::RET_MEM_OUT);
		}
		else
		{
			assert!(!s.IsError());
		}
		allocs
	}

	fn Test<Ind: TUInd, const Compress: bool>()
	{
		let reserve = ["m_SavedPhases", "m_Lits", "m_NewLits", "m_Stack", "m_UserLits"].map(|b| format!("Couldn't ReserveExactly : m_ExtVivify.{} in ExtVivifyReserve", b));
		// A round: the candidates and the buffers are reserved before the round changes anything; the hashes of the clauses tried in vain grow, after such a clause has been re-attached
		let cands = "Couldn't reserve the candidates of vivification".to_string();
		let hashes = "Couldn't reserve the hashes of the clauses tried by vivification".to_string();
		let roundAllocs = [vec![cands.clone()], reserve.to_vec(), vec![hashes]].concat();
		// (a round, which doesn't shorten the clause, allocates nothing else; a round, which does, doesn't add a hash, and the solver proper allocates for the new clause)
		assert_eq!(Run::<Ind, Compress>(1, None, ""), 7);
		assert!(Run::<Ind, Compress>(0, None, "") >= 6);
		for isOnly in [false, true]
		{
			for (failAt, expected) in roundAllocs.iter().enumerate()
			{
				// (once an allocation has failed, the later ones aren't attempted)
				assert_eq!(Run::<Ind, Compress>(1, Some((failAt as u64, isOnly)), expected), failAt as u64 + 1);
				if failAt < 6
				{
					assert_eq!(Run::<Ind, Compress>(0, Some((failAt as u64, isOnly)), expected), failAt as u64 + 1);
				}
			}
			// The hook of simplification reserves the buffers, if no round has done it
			for (failAt, expected) in reserve.iter().enumerate()
			{
				assert_eq!(Run::<Ind, Compress>(2, Some((failAt as u64, isOnly)), expected), failAt as u64 + 1);
			}
		}
	}
	ForAllModes!(Test);
}

// Memory-out in the allocations of the extension itself: every allocation of a query, which starts with a round (or with a simplification, which the proof is told about), is made to fail in turn,
// both on its own and together with all the allocations after it. A failure of an allocation of the extension makes the status that of any other memory-out, and leaves the clauses in place
#[test]
#[cfg_attr(not(debug_assertions), ignore = "relies on the debug assertions of the solver: without them it proceeds past a failed assertion, as the C++ solver does with NDEBUG")]
fn alloc_failures_of_the_extension()
{
	use crate::topor_dyn_array::alloc_fault;
	use std::collections::BTreeSet;

	fn Test<Ind: TUInd, const Compress: bool>() -> BTreeSet<String>
	{
		let mut explanations = BTreeSet::new();
		// The scenarios: a round, which shortens a clause; a round, which doesn't; no round (there is no budget for it) and a simplification of a learnt clause
		for scenario in 0..3
		{
			// The number of the allocations of the query, its return value and the explanation of its status
			let Run = |failAt: Option<(u64, bool)>| -> (u64, TToporReturnVal, String)
			{
				let (mut s, _) = match scenario
				{
					0 => Craft::<Ind, Compress>(&VivifyOn, false, &[&[1, 2, 4, 7], &[1, 2, 4, -7]], &[1, 2, 3, 4, 5, 6], 6, 0., false),
					1 => Craft::<Ind, Compress>(&VivifyOn, false, &[&[1, 2, 3, 4, 5]], &[1, 2, 3, 4], 4, 0., false),
					_ => Craft::<Ind, Compress>(&[VivifyOn[0], VivifyOn[1], VivifyOn[2], ("/inprocessing/vivify/effort", 1e-9)], false, &[&[1, 2, 3, 4, 5, 6]], &[1, 2, 3, 4, 5], 5, 0., false),
				};
				let learntBefore = LearntLongs(&mut s);
				if scenario == 2
				{
					s.AddClause(&[-5]);
					s.m_Topi.m_ImplicationsTillNextSimplify = 0;
				}
				s.Backtrack(0);
				s.m_Topi.m_Stat.m_Implications += 1;
				let allocsBefore = alloc_fault::allocs_so_far();
				match failAt
				{
					Some((allocs, true)) => alloc_fault::fail_only(allocs),
					Some((allocs, false)) => alloc_fault::fail_after(allocs),
					None => (),
				}
				// (a failure of an allocation of the solver proper may be anything but a crash of the process, see FuzzAllocFailures in fuzz.rs)
				let ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Solve(&mut s, &[])));
				alloc_fault::never_fail();
				let allocs = alloc_fault::allocs_so_far() - allocsBefore;
				let explanation = s.m_Topi.m_StatusExplanation.borrow().clone();
				if explanation.starts_with("Couldn't") && explanation.contains("ivif")
				{
					// A failure of the extension: the solver reports a memory-out, and the learnt clause is still there (simplified or not)
					assert_eq!(ret.as_ref().ok(), Some(&TToporReturnVal::RET_MEM_OUT), "{}", explanation);
					assert!(s.IsError());
					assert_eq!(s.m_Topi.m_DecLevel, 0);
					let learnt = LearntLongs(&mut s);
					assert!(learnt == learntBefore || (scenario == 2 && learnt == [[1, 2, 3, 4]]), "{}: {:?}", explanation, learnt);
				}
				(allocs, ret.unwrap_or(TToporReturnVal::RET_MEM_OUT), explanation)
			};

			let (allocs, ret, _) = Run(None);
			assert_eq!(ret, RetSat);
			assert!(allocs > 5);
			for isOnly in [false, true]
			{
				for failAt in 0..allocs
				{
					let (_, ret, explanation) = Run(Some((failAt, isOnly)));
					if ret == TToporReturnVal::RET_MEM_OUT
					{
						explanations.insert(explanation);
					}
				}
			}
		}
		explanations
	}

	for explanations in [Test::<u32, false>(), Test::<u64, false>(), Test::<u64, true>()]
	{
		// Every allocation of the extension has failed
		for expected in ["Couldn't reserve the candidates of vivification", "Couldn't reserve the hashes of the clauses tried by vivification", "Couldn't ReserveExactly : m_ExtVivify.m_SavedPhases in ExtVivifyReserve",
			"Couldn't ReserveExactly : m_ExtVivify.m_Lits in ExtVivifyReserve", "Couldn't ReserveExactly : m_ExtVivify.m_NewLits in ExtVivifyReserve", "Couldn't ReserveExactly : m_ExtVivify.m_Stack in ExtVivifyReserve",
			"Couldn't ReserveExactly : m_ExtVivify.m_UserLits in ExtVivifyReserve"]
		{
			assert!(explanations.contains(expected), "{}: {:?}", expected, explanations);
		}
	}
}

// The clauses of the formula, the proof so far and a naive unit propagation: every clause added to the proof must be RUP at the time it is added,
// and every clause deleted must be one of the clauses (that is, the proof follows every change the solver makes to a learnt clause, which is deleted later), if m_IsDeletionExact
struct CRupChecker
{
	m_Vars: usize,
	m_Clss: Vec<Vec<TLit>>,
	// The number of bytes of the proof, which have been checked
	m_Checked: usize,
	m_Deleted: u64,
	// Vivification has been on throughout (the proof isn't told about the literals simplification removes from a clause, while vivification is off)
	m_IsDeletionExact: bool,
}

impl CRupChecker
{
	fn IsRup(&self, c: &[TLit]) -> bool
	{
		// 0: unassigned, 1: true, -1: false
		let mut val = vec![0i8; self.m_Vars + 1];
		let Val = |val: &[i8], l: TLit| if l > 0 { val[l as usize] } else { -val[(-l) as usize] };
		for l in c
		{
			if Val(&val, *l) == 1
			{
				return true;
			}
			val[l.unsigned_abs() as usize] = if *l > 0 { -1 } else { 1 };
		}
		loop
		{
			let mut isChanged = false;
			for d in &self.m_Clss
			{
				if d.iter().any(|l| Val(&val, *l) == 1)
				{
					continue;
				}
				let mut unassigned = d.iter().filter(|l| Val(&val, **l) == 0);
				match (unassigned.next(), unassigned.next())
				{
					(None, _) => return true,
					(Some(l), None) =>
					{
						val[l.unsigned_abs() as usize] = if *l > 0 { 1 } else { -1 };
						isChanged = true;
					}
					_ => (),
				}
			}
			if !isChanged
			{
				return false;
			}
		}
	}

	// Check the part of the textual proof, which hasn't been checked yet
	fn Check(&mut self, sink: &TSink, context: &str)
	{
		let text = String::from_utf8(sink.m_Buf.borrow()[self.m_Checked..].to_vec()).unwrap();
		self.m_Checked = sink.m_Buf.borrow().len();
		for line in text.lines()
		{
			let isDeletion = line.starts_with("d ");
			if line.starts_with('c')
			{
				continue;
			}
			let mut c: Vec<TLit> = line.split_whitespace().skip(isDeletion as usize).map(|t| t.parse().unwrap()).collect();
			assert_eq!(c.pop(), Some(0), "{}: {}", context, line);
			let c = Sorted(c);
			if isDeletion
			{
				// A deletion follows the addition of a clause over a proper subset of the literals
				let last = self.m_Clss.last().unwrap();
				assert!(last.len() < c.len() && last.iter().all(|l| c.contains(l)), "{}: {} after {:?}", context, line, last);
				let i = self.m_Clss.iter().position(|d| *d == c);
				// (a deleted clause, from which simplification removed literals, while vivification was off, is not found; it stays)
				assert!(i.is_some() || !self.m_IsDeletionExact, "{}: the clause deleted by {} is not in the proof", context, line);
				if let Some(i) = i
				{
					self.m_Clss.swap_remove(i);
					self.m_Deleted += 1;
				}
			}
			// (the empty clause is reported for every unsatisfiable query, including the ones, which are unsatisfiable only under their assumptions)
			else if !c.is_empty()
			{
				assert!(self.IsRup(&c), "{}: the clause {} of the proof is not RUP", context, line);
				self.m_Clss.push(c);
			}
		}
	}
}

// Random formulas, parameters, user polarities and incremental queries under assumptions in every instantiation, with vivification after (almost) every conflict:
// every answer and every core is checked against the truth table, every model against the clauses, every reported clause against the truth table, and the proof by unit propagation
#[test]
fn vivification_never_changes_an_answer()
{
	// The models of the formula as bit-sets over the variables
	fn Models(vars: TLit, clss: &[Vec<TLit>]) -> Vec<u32>
	{
		let holds = |a: u32, l: TLit| ((a >> (l.abs() - 1)) & 1 == 1) == (l > 0);
		(0..1u32 << vars).filter(|a| clss.iter().all(|c| c.iter().any(|l| holds(*a, *l)))).collect()
	}
	fn Holds(a: u32, l: TLit) -> bool
	{
		((a >> (l.abs() - 1)) & 1 == 1) == (l > 0)
	}

	fn Test<Ind: TUInd, const Compress: bool>(seed: u64) -> [u64; 6]
	{
		let mut rand = TRand(seed);
		let vars = 9 + rand.Next(6) as TLit;
		// Clauses of 3 to 5 literals, around the satisfiability threshold
		let Clause = |rand: &mut TRand| -> Vec<TLit>
		{
			let mut c: Vec<TLit> = vec![];
			let len = 3 + rand.Next(3);
			while c.len() < len as usize
			{
				let l = rand.Lit(vars);
				if !c.contains(&l) && !c.contains(&-l)
				{
					c.push(l);
				}
			}
			c
		};
		let mut clss: Vec<Vec<TLit>> = (0..(vars as f64 * (5.5 + rand.Next(4) as f64))  as usize).map(|_| Clause(&mut rand)).collect();

		let mut s = CTopor::<Ind, Compress>::new(0);
		let sink = CSharedSink::default();
		s.DumpDrat(Box::new(sink.clone()), false, rand.Next(2) == 0);
		let reported: Rc<RefCell<Vec<Vec<TLit>>>> = Default::default();
		let reportedCb = reported.clone();
		s.SetCbNewLearntCls(Some(Box::new(move |c: &[TLit]|
		{
			reportedCb.borrow_mut().push(c.to_vec());
			TStopTopor::VAL_CONTINUE
		})));
		if rand.Next(3) == 0
		{
			s.SetParam("/mode/value", 1. + rand.Next(8) as f64);
		}
		s.SetParam("/restarts/numeric/conflict_thr", 1. + rand.Next(3) as f64);
		s.SetParam("/inprocessing/on", 1.);
		s.SetParam("/inprocessing/conflicts_before_next", 1. + rand.Next(3) as f64);
		s.SetParam("/inprocessing/vivify/on", 1.);
		s.SetParam("/inprocessing/vivify/effort", if rand.Next(4) == 0 { 30. } else { 1e9 });
		if rand.Next(4) == 0
		{
			s.SetParam("/inprocessing/vivify/max_glue", 2. + rand.Next(3) as f64);
		}
		if rand.Next(4) == 0
		{
			s.SetParam("/deletion/clause/strategy", 0.);
		}
		if rand.Next(4) == 0
		{
			// Frequent clause deletion
			s.SetParam("/deletion/clause/trigger_init", 3.);
			s.SetParam("/deletion/clause/trigger_linc", 2.);
		}
		if rand.Next(4) == 0
		{
			s.SetParam("/conflicts/all_uip/mode", rand.Next(4) as f64);
			s.SetParam("/conflicts/all_uip/first_restart", 0.);
		}
		if rand.Next(4) == 0
		{
			s.SetParam("/backtracking/chrono_bt_if_higher_init", rand.Next(3) as f64);
			s.SetParam("/backtracking/chrono_bt_if_higher_n", rand.Next(3) as f64);
			s.SetParam("/backtracking/chrono_bt_if_higher_s", rand.Next(3) as f64);
			s.SetParam("/backtracking/conflicts_to_postpone_chrono", 0.);
		}
		assert!(!s.IsError(), "seed {}: {}", seed, s.GetStatusExplanation());

		for c in &clss
		{
			s.AddClause(c);
		}
		let mut rup = CRupChecker { m_Vars: vars as usize, m_Clss: clss.iter().map(|c| Sorted(c.clone())).collect(), m_Checked: 0, m_Deleted: 0, m_IsDeletionExact: true };
		for query in 0..6
		{
			if rand.Next(4) == 0
			{
				let l = rand.Lit(vars);
				if rand.Next(4) == 0 { s.ClearUserPolarityInfo(l.abs()) } else { s.FixPolarity(l, rand.Next(2) == 0) };
			}
			if rand.Next(6) == 0
			{
				// Vivification is turned off and on between the queries
				let isOn = rand.Next(2);
				s.SetParam("/inprocessing/vivify/on", isOn as f64);
				rup.m_IsDeletionExact &= isOn == 1;
			}
			let assumps: Vec<TLit> = (0..rand.Next(5)).map(|_| rand.Lit(vars)).collect();
			let ret = Solve(&mut s, &assumps);
			let context = format!("seed {} query {}", seed, query);
			let models = Models(vars, &clss);
			let IsSatUnder = |assumps: &[TLit]| models.iter().any(|a| assumps.iter().all(|l| Holds(*a, *l)));
			assert_eq!(ret, if IsSatUnder(&assumps) { RetSat } else { RetUnsat }, "{}", context);
			if ret == RetSat
			{
				let holds = |l: TLit| s.GetLitValue(l) != Unsat;
				assert!(assumps.iter().all(|l| holds(*l)) && clss.iter().all(|c| c.iter().any(|l| holds(*l))), "{}", context);
			}
			else
			{
				let core: Vec<TLit> = (0..assumps.len()).filter(|i| s.IsAssumptionRequired(*i)).map(|i| assumps[i]).collect();
				assert!(!IsSatUnder(&core), "{}: the core {:?} of {:?}", context, core, assumps);
			}
			for c in reported.borrow_mut().drain(..)
			{
				assert!(c.is_empty() || models.iter().all(|a| c.iter().any(|l| Holds(*a, *l))), "{}: the reported clause {:?} is not implied", context, c);
			}
			rup.Check(&sink, &context);
			if models.is_empty()
			{
				break;
			}
			for _ in 0..rand.Next(3)
			{
				let c = Clause(&mut rand);
				s.AddClause(&c);
				rup.m_Clss.push(Sorted(c.clone()));
				clss.push(c);
			}
			// (a clause of the user may make the solver report clauses too)
			rup.Check(&sink, &context);
			reported.borrow_mut().clear();
		}
		assert!(!s.IsError(), "seed {}", seed);
		let mut st = Stat(&s);
		// (the last counter stands for the deletions found in the proof here)
		st[5] = rup.m_Deleted;
		st
	}

	let mut tot = [0u64; 6];
	let iters: u64 = std::env::var("TOPOR_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(2000) / 5;
	for seed in 0..iters
	{
		for st in [Test::<u32, false>(seed), Test::<u64, false>(seed), Test::<u64, true>(seed)]
		{
			(0..6).for_each(|i| tot[i] += st[i]);
		}
	}
	println!("vivification in the random formulas: rounds {} tried {} shortened {} literals removed {} units {} deletions checked in the proofs {}", tot[0], tot[1], tot[2], tot[3], tot[4], tot[5]);
	// The scenarios do exercise the extension
	assert!(tot[0] > 2 * iters && tot[1] > 4 * iters && tot[2] > 2 * iters && tot[4] > 0 && tot[5] > 2 * iters, "{:?}", tot);
}

// Larger formulas (so, longer learnt clauses, restarts, clause deletion, simplification and compression of the buffers), which the truth table is out of reach for:
// the answers are those of a solver without vivification, the models are checked against the clauses, the cores by the other solver, and the proofs by unit propagation
#[test]
fn vivification_agrees_with_the_solver_without_it()
{
	fn Test<Ind: TUInd, const Compress: bool>(seed: u64) -> [u64; 6]
	{
		let mut rand = TRand(seed.wrapping_mul(977) + 13);
		let vars = 40 + rand.Next(25) as TLit;
		let clss: Vec<Vec<TLit>> = (0..(vars as f64 * 4.2) as usize).map(|_| vec![rand.Lit(vars), rand.Lit(vars), rand.Lit(vars)]).collect();
		let mut s = CTopor::<Ind, Compress>::new(0);
		let mut reference = CTopor::<u32, false>::new(0);
		let sink = CSharedSink::default();
		s.DumpDrat(Box::new(sink.clone()), false, false);
		s.SetParam("/restarts/numeric/conflict_thr", 5. + rand.Next(30) as f64);
		s.SetParam("/inprocessing/on", 1.);
		s.SetParam("/inprocessing/conflicts_before_next", 1. + rand.Next(40) as f64);
		s.SetParam("/inprocessing/vivify/on", 1.);
		s.SetParam("/inprocessing/vivify/effort", [0.2, 2., 1e9][rand.Next(3) as usize]);
		if rand.Next(3) == 0
		{
			s.SetParam("/deletion/clause/trigger_init", 30.);
			s.SetParam("/deletion/clause/trigger_linc", 10.);
		}
		if rand.Next(3) == 0
		{
			s.SetParam("/mode/value", 1. + rand.Next(8) as f64);
		}
		for c in &clss
		{
			s.AddClause(c);
			reference.AddClause(c);
		}
		let mut rup = CRupChecker { m_Vars: vars as usize, m_Clss: clss.iter().map(|c| Sorted(c.clone())).collect(), m_Checked: 0, m_Deleted: 0, m_IsDeletionExact: true };
		for query in 0..4
		{
			let assumps: Vec<TLit> = (0..rand.Next(4)).map(|_| rand.Lit(vars)).collect();
			let context = format!("seed {} query {}", seed, query);
			let ret = Solve(&mut s, &assumps);
			assert_eq!(ret, Solve(&mut reference, &assumps), "{}", context);
			if ret == RetSat
			{
				let holds = |l: TLit| s.GetLitValue(l) != Unsat;
				assert!(assumps.iter().all(|l| holds(*l)) && clss.iter().all(|c| c.iter().any(|l| holds(*l))), "{}", context);
			}
			else
			{
				assert_eq!(ret, RetUnsat, "{}", context);
				let core: Vec<TLit> = (0..assumps.len()).filter(|i| s.IsAssumptionRequired(*i)).map(|i| assumps[i]).collect();
				assert_eq!(Solve(&mut reference, &core), RetUnsat, "{}: the core {:?} of {:?}", context, core, assumps);
			}
			rup.Check(&sink, &context);
		}
		assert!(!s.IsError(), "seed {}", seed);
		let mut st = Stat(&s);
		st[5] = rup.m_Deleted;
		st
	}

	let mut tot = [0u64; 6];
	let iters: u64 = std::env::var("TOPOR_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(2000) / 100;
	for seed in 0..iters
	{
		for st in [Test::<u32, false>(seed), Test::<u64, false>(seed), Test::<u64, true>(seed)]
		{
			(0..6).for_each(|i| tot[i] += st[i]);
		}
	}
	println!("vivification in the larger random formulas: rounds {} tried {} shortened {} literals removed {} units {} deletions checked in the proofs {}", tot[0], tot[1], tot[2], tot[3], tot[4], tot[5]);
	assert!(tot[0] > iters && tot[2] > 5 * iters && tot[5] > 5 * iters, "{:?}", tot);
}

// The consistency assertions of the solver (the trail, the watches of every clause and the count of the long clauses, no missed implications, the wasted space of the buffers)
// hold throughout the rounds, in which a clause is detached, tested and replaced
// (a single query without assumptions in the default mode: the assertions themselves are not robust otherwise, see "Upstream issues" in README.md)
#[test]
fn consistency_assertions_hold_during_rounds()
{
	fn Test<Ind: TUInd, const Compress: bool>(seed: u64) -> [u64; 6]
	{
		let mut rand = TRand(seed.wrapping_mul(31) + 7);
		let vars = 30 + rand.Next(15) as TLit;
		let mut s = CTopor::<Ind, Compress>::new(0);
		VivifyOn.iter().for_each(|(name, val)| s.SetParam(name, *val));
		s.SetParam("/restarts/numeric/conflict_thr", 3. + rand.Next(10) as f64);
		s.SetParam("/debugging/assert_consistency", 3.);
		let clss: Vec<Vec<TLit>> = (0..(vars as f64 * 4.2) as usize).map(|_| vec![rand.Lit(vars), rand.Lit(vars), rand.Lit(vars)]).collect();
		clss.iter().for_each(|c| s.AddClause(c));
		let ret = Solve(&mut s, &[]);
		assert!(ret == RetUnsat || (ret == RetSat && clss.iter().all(|c| c.iter().any(|l| s.GetLitValue(*l) != Unsat))), "seed {}", seed);
		Stat(&s)
	}

	let mut tot = [0u64; 6];
	for seed in 0..20
	{
		for st in [Test::<u32, false>(seed), Test::<u64, false>(seed), Test::<u64, true>(seed)]
		{
			(0..6).for_each(|i| tot[i] += st[i]);
		}
	}
	assert!(tot[1] > 500 && tot[2] > 100, "{:?}", tot);
}

// The fuzz scenarios (see fuzzing.rs) with vivification on: every model, every UNSAT verdict, every core and every clause reported by the solver is verified
#[test]
fn fuzz_scenarios_with_vivification()
{
	SetExtraParams(&VivifyOn);
	let _ = TakeExtStats();
	let iters: u64 = std::env::var("TOPOR_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(2000);
	for seed in 0..iters
	{
		FuzzOne(&RandomBytes(seed, 200 + (seed as usize * 37) % 3000), false);
	}
	for seed in 0..iters / 10
	{
		FuzzOne(&RandomBytes(seed, 20000), true);
	}
	SetExtraParams(&[]);
	let mut tot = [0u64; 6];
	for line in TakeExtStats().iter().filter(|l| l.contains("VivifyRounds"))
	{
		let st = ParseStat(line);
		(0..6).for_each(|i| tot[i] += st[i]);
	}
	println!("vivification in the fuzz scenarios: rounds {} tried {} shortened {} literals removed {} units {} propagations {}", tot[0], tot[1], tot[2], tot[3], tot[4], tot[5]);
	assert!(tot[1] > 0 && tot[2] > 0, "{:?}", tot);
}
