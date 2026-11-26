// Rust-only extension (no C++ counterpart): phase and score hints
//
// A phase hint seeds the saved polarity of a variable (the m_IsNegated bit of m_AssignmentInfo, which phase saving reads for an unassigned variable),
// WITHOUT fixing it: the solver stays free to flip the variable, and phase saving overwrites the hint as usual.
// A user-fixed polarity (FixPolarity) takes precedence for as long as it is in force, since it lives in m_PolarityInfo, which the hints never touch.
// A score hint boosts the initial decision score of a variable through UpdateScoreVar, precisely as BoostScore does.
//
// Both are off by default: HintPhase is ignored unless /phase/hints/on is 1, and HintScore unless /decision/score_hints/mult is non-0.
// With the parameters off, nothing is recorded, so the hook in Solve is a test of two empty vectors.
//
// The hints are recorded when given and applied by the next Solve right before its search loop: all the variables exist by then, the initial BCP has been carried out
// and the initial polarities of the new variables (/decision/polarity/init_strategy) have been set, so that the hints override them.
// A hint is advice only: it changes neither the clauses nor the proof, so a wrong hint can only cost time.
//
// One hint of a kind is recorded per variable: a newer hint replaces the recorded one, which hasn't been applied yet (so the record is bounded by the number of the distinct variables hinted).
// A recorded hint is applied by the first Solve, which can apply it, and is forgotten then:
//	a score hint, once the solver knows the variable;
//	a phase hint, once the solver knows the variable and the variable is unassigned at that point of Solve, where a variable assigned by the previous query above the assumptions counts as unassigned
//	(the solver backtracks to the assumptions to apply the hint). A variable assigned at level 0 keeps its value forever, so its phase hint is forgotten. A variable held by the assumptions of the query
//	(an assumption or implied by the assumptions) holds its value rather than a saved phase, so its phase hint stays recorded for the first later Solve, which finds the variable unassigned
//	(the hint then overrides the phase saved in the meantime, unless a newer hint has replaced it).

use crate::topi::*;
use crate::topor::CTopor;
use std::collections::HashMap;
use std::io::Write;

// The recorded hints of one kind: one per external variable, in the order in which the variables were first hinted (the order of the application, so that it doesn't depend on the hash function)
pub(crate) struct CExtHintsPending<T>
{
	// (the external variable, its hint)
	pub(crate) m_Hints: Vec<(TLit, T)>,
	// The index in m_Hints of the hint of a variable
	m_IndOfVar: HashMap<TLit, usize>,
}

impl<T> Default for CExtHintsPending<T>
{
	fn default() -> Self
	{
		CExtHintsPending { m_Hints: Vec::new(), m_IndOfVar: HashMap::new() }
	}
}

impl<T> CExtHintsPending<T>
{
	pub(crate) fn is_empty(&self) -> bool
	{
		self.m_Hints.is_empty()
	}

	// Record the hint of the variable; the latest hint wins
	pub(crate) fn Set(&mut self, vExternal: TLit, hint: T)
	{
		match self.m_IndOfVar.get(&vExternal)
		{
			Some(&i) => self.m_Hints[i].1 = hint,
			None =>
			{
				self.m_IndOfVar.insert(vExternal, self.m_Hints.len());
				self.m_Hints.push((vExternal, hint));
			}
		}
	}

	// Move the recorded hints out
	pub(crate) fn Take(&mut self) -> Vec<(TLit, T)>
	{
		self.m_IndOfVar.clear();
		std::mem::take(&mut self.m_Hints)
	}
}

// The hints, which haven't been applied yet (by external variables, since the internal ones may not exist yet or may be renumbered)
#[derive(Default)]
pub(crate) struct CExtHints
{
	// The hinted polarity: is the variable hinted to be negative?
	pub(crate) m_PendingPhases: CExtHintsPending<bool>,
	// The hinted score
	pub(crate) m_PendingScores: CExtHintsPending<f64>,
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	// The internal literal of the external variable vExternal > 0; BadULit, if there is none (yet)
	fn ExtHintsE2IVar(&self, vExternal: TLit) -> TULit
	{
		if vExternal > self.m_Stat.m_MaxUserVar || !self.IsE2IInBounds(vExternal) { BadULit } else { self.E2I(vExternal) }
	}

	// Hint that the solver should try the polarity of lExternal first, when it decides on its variable (see the top of the file)
	// The hint replaces a hint for the same variable, which hasn't been applied yet, and it is applied by the first Solve, which finds the variable known and not held by a level-0 assignment
	// (the hint is dropped then) or by the assumptions (the hint waits for a later Solve then)
	// Returns false, iff the hint is ignored (the parameter is off or the literal is illegal)
	pub fn HintPhase(&mut self, lExternal: TLit) -> bool
	{
		if !self.m_Params.m_ParamPhaseHintsOn.get() || lExternal == 0 || lExternal == TLit::MIN
		{
			return false;
		}
		// cout << "\tc lh <PhaseHintLit>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "lh {}", lExternal);
		}
		self.m_ExtHints.m_PendingPhases.Set(lExternal.abs(), lExternal < 0);
		true
	}

	// Hint that the initial decision score of vExternal should be boosted by /decision/score_hints/mult * score (see the top of the file)
	// The hint replaces a hint for the same variable, which hasn't been applied yet (the boosts of the hints, which have been applied, add up)
	// Returns false, iff the hint is ignored (the parameter is off, the variable is illegal or the score isn't a positive number)
	pub fn HintScore(&mut self, vExternal: TLit, score: f64) -> bool
	{
		if self.m_Params.m_ParamScoreHintsMult.get() == 0. || vExternal <= 0 || !score.is_finite() || score <= 0.
		{
			return false;
		}
		// cout << "\tc ls <ScoreHintVar> <Score>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "ls {} {}", vExternal, score);
		}
		self.m_ExtHints.m_PendingScores.Set(vExternal, score);
		true
	}

	// The hook of Solve: apply the recorded hints, which can be applied; the others (those of the variables, which don't exist, and the phases of the variables held by the assumptions)
	// stay recorded for the next Solve
	pub(crate) fn ExtHintsApplyPending(&mut self)
	{
		if self.m_ExtHints.m_PendingPhases.is_empty() && self.m_ExtHints.m_PendingScores.is_empty()
		{
			return;
		}

		let pendingPhases = self.m_ExtHints.m_PendingPhases.Take();

		// A variable which is still assigned above the assumptions (by the previous query) holds its value rather than a saved polarity,
		// so backtrack to the assumptions first, precisely as a restart does
		let assumpDecLevel = self.m_DecLevelOfLastAssignedAssumption;
		if pendingPhases.iter().any(|&(vExternal, _)| { let lv = self.ExtHintsE2IVar(vExternal); lv != BadULit && self.IsAssigned(lv) && self.GetAssignedDecLevel(lv) > assumpDecLevel })
		{
			self.Backtrack(assumpDecLevel as TLit, false, false);
		}

		for (vExternal, isNegative) in pendingPhases
		{
			let lv = self.ExtHintsE2IVar(vExternal);
			if lv == BadULit || (self.IsAssigned(lv) && self.GetAssignedDecLevel(lv) != 0)
			{
				// The variable doesn't exist yet, or it is held by the assumptions (it is assigned, and neither above them nor at level 0): the hint waits
				self.m_ExtHints.m_PendingPhases.Set(vExternal, isNegative);
			}
			else if !self.IsAssigned(lv)
			{
				// The external variable may be mapped to a negated internal literal
				self.m_AssignmentInfo[GetVar(lv) as usize].set_m_IsNegated(IsNeg(lv) != isNegative);
			}
			// (a variable assigned at level 0 never gets to use a saved phase: its hint is dropped)
		}

		let mult = self.m_Params.m_ParamScoreHintsMult.get();
		for (vExternal, score) in self.m_ExtHints.m_PendingScores.Take()
		{
			let lv = self.ExtHintsE2IVar(vExternal);
			if lv == BadULit
			{
				self.m_ExtHints.m_PendingScores.Set(vExternal, score);
			}
			else
			{
				self.UpdateScoreVar(GetVar(lv), mult * score);
			}
		}
	}

	// The polarity the solver would currently pick for vExternal, if it weren't forced: its value, if it's assigned (so, after a satisfiable query, the model), and its saved phase otherwise
	// VAL_SATISFIED: positive; VAL_UNSATISFIED: negative; VAL_DONT_CARE: the solver doesn't know the variable
	pub fn GetSavedPhase(&self, vExternal: TLit) -> TToporLitVal
	{
		let lv = if vExternal <= 0 { BadULit } else { self.ExtHintsE2IVar(vExternal) };
		if lv == BadULit
		{
			TToporLitVal::VAL_DONT_CARE
		}
		else if self.m_AssignmentInfo[GetVar(lv) as usize].m_IsNegated() != IsNeg(lv)
		{
			TToporLitVal::VAL_UNSATISFIED
		}
		else
		{
			TToporLitVal::VAL_SATISFIED
		}
	}

	// The current decision score of vExternal (0, if the solver doesn't know the variable)
	pub fn GetVarScore(&self, vExternal: TLit) -> f64
	{
		let lv = if vExternal <= 0 { BadULit } else { self.ExtHintsE2IVar(vExternal) };
		if lv == BadULit || !self.m_VsidsHeap.var_score_exists(GetVar(lv)) { 0. } else { self.m_VsidsHeap.get_var_score(GetVar(lv)) }
	}
}

// The public API and the hints files
//
// A hints file is a text file, where:
//	c ...               a comment (a line starting with s, e.g., "s SATISFIABLE", is ignored too, so that the output of a SAT solver is a legal hints file)
//	v <Lit> ... <Lit>   phase hints, DIMACS-model style; a 0 is ignored, and the v is optional
//	b <Var> <Score>     a score hint
impl<Ind: TUInd, const Compress: bool> CTopor<Ind, Compress>
{
	// Hint the polarity of the variable |l| to l without fixing it: l seeds the saved phase, which the solver is free to overwrite; ignored unless /phase/hints/on is 1
	// One hint is recorded per variable (the latest one), and the next Solve applies it, unless its variable
	//	(1) is unknown to the solver: the hint waits for the variable;
	//	(2) is assigned at level 0: the hint is dropped, since the variable keeps its value;
	//	(3) is an assumption of that query or implied by its assumptions: the hint waits for the first later Solve, where the variable is free, and overrides the phase saved in the meantime
	//	    (give the variable a new hint or let the solver decide by then: a new hint replaces the waiting one)
	pub fn HintPhase(&mut self, l: TLit) -> bool
	{
		self.m_Topi.HintPhase(l)
	}

	// Boost the initial decision score of v by /decision/score_hints/mult * score at the next Solve; ignored if the parameter is 0
	// One hint is recorded per variable (the latest one); a hint for a variable, which is unknown to the solver, waits for the variable
	pub fn HintScore(&mut self, v: TLit, score: f64) -> bool
	{
		self.m_Topi.HintScore(v, score)
	}

	// The saved phase of v (its value, if assigned): VAL_SATISFIED: positive; VAL_UNSATISFIED: negative; VAL_DONT_CARE: unknown variable
	pub fn GetSavedPhase(&self, v: TLit) -> TToporLitVal
	{
		self.m_Topi.GetSavedPhase(v)
	}

	// The current decision score of v
	pub fn GetVarScore(&self, v: TLit) -> f64
	{
		self.m_Topi.GetVarScore(v)
	}

	// Provide the hints of the text to HintPhase and HintScore; returns the number of the accepted phase hints and score hints, or the description of the error
	pub fn ReadHints(&mut self, text: &str) -> Result<(usize, usize), String>
	{
		let (mut phases, mut scores) = (0, 0);
		for (lineInd, line) in text.lines().enumerate()
		{
			let mut toks = line.split_whitespace().peekable();
			let BadLine = || format!("hints: cannot parse line {}: {}", lineInd + 1, line);
			match toks.peek().and_then(|t| t.chars().next())
			{
				None | Some('c') | Some('s') => {}
				Some('b') =>
				{
					toks.next();
					let v = toks.next().and_then(|t| t.parse::<TLit>().ok()).ok_or_else(BadLine)?;
					let score = toks.next().and_then(|t| t.parse::<f64>().ok()).ok_or_else(BadLine)?;
					scores += self.HintScore(v, score) as usize;
				}
				first =>
				{
					if first == Some('v')
					{
						toks.next();
					}
					for t in toks
					{
						phases += self.HintPhase(t.parse::<TLit>().map_err(|_| BadLine())?) as usize;
					}
				}
			}
		}
		Ok((phases, scores))
	}

	// The hints for a future run in the format ReadHints accepts: the saved phases (after a satisfiable query, the model) and the ranks of the decision scores, normalized to (0, 1], of the variables the solver knows
	pub fn WriteHints(&self) -> String
	{
		let maxUserVar = self.m_Topi.GetMaxUserVar();
		let mut text = "c Intel(R) SAT Solver hints: v lines hold the saved phases, b lines hold <Var> <Rank of the decision score, normalized to (0, 1]>\n".to_string();
		let mut inLine = 0;
		for v in 1..=maxUserVar
		{
			let phase = self.GetSavedPhase(v);
			if phase != TToporLitVal::VAL_DONT_CARE
			{
				text += if inLine == 0 { "v" } else { "" };
				text += &format!(" {}", if phase == TToporLitVal::VAL_SATISFIED { v } else { -v });
				inLine += 1;
				if inLine == 20
				{
					text += "\n";
					inLine = 0;
				}
			}
		}
		text += if inLine == 0 { "v 0\n" } else { " 0\n" };

		// VSIDS scores grow exponentially with the conflicts, so their ratios are meaningless to a future run; the rank is what is carried over:
		// the score written for the variable with the i-th lowest positive score out of n is i/n
		let mut scored: Vec<(f64, TLit)> = (1..=maxUserVar).map(|v| (self.GetVarScore(v), v)).filter(|sv| sv.0 > 0.).collect();
		scored.sort_by(|a, b| a.partial_cmp(b).unwrap());
		let mut rankOf = vec![0usize; maxUserVar as usize + 1];
		for (i, sv) in scored.iter().enumerate()
		{
			rankOf[sv.1 as usize] = i + 1;
		}
		for v in 1..=maxUserVar
		{
			if rankOf[v as usize] != 0
			{
				text += &format!("b {} {}\n", v, rankOf[v as usize] as f64 / scored.len() as f64);
			}
		}
		text
	}
}
