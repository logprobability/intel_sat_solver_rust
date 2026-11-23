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

use crate::topi::*;
use crate::topor::CTopor;
use std::io::Write;

// The hints, which haven't been applied yet (external literals and variables, since the internal ones may not exist yet or may be renumbered)
#[derive(Default)]
pub(crate) struct CExtHints
{
	pub(crate) m_PendingPhases: Vec<TLit>,
	pub(crate) m_PendingScores: Vec<(TLit, f64)>,
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	// The internal literal of the external variable vExternal > 0; BadULit, if there is none (yet)
	fn ExtHintsE2IVar(&self, vExternal: TLit) -> TULit
	{
		if vExternal > self.m_Stat.m_MaxUserVar || !self.IsE2IInBounds(vExternal) { BadULit } else { self.E2I(vExternal) }
	}

	// Hint that the solver should try the polarity of lExternal first, when it decides on its variable (see the top of the file)
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
		self.m_ExtHints.m_PendingPhases.push(lExternal);
		true
	}

	// Hint that the initial decision score of vExternal should be boosted by /decision/score_hints/mult * score (see the top of the file)
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
		self.m_ExtHints.m_PendingScores.push((vExternal, score));
		true
	}

	// The hook of Solve: apply the recorded hints, whose variables exist; the others stay recorded for the next Solve
	pub(crate) fn ExtHintsApplyPending(&mut self)
	{
		if self.m_ExtHints.m_PendingPhases.is_empty() && self.m_ExtHints.m_PendingScores.is_empty()
		{
			return;
		}

		let pendingPhases = std::mem::take(&mut self.m_ExtHints.m_PendingPhases);

		// A variable which is still assigned above the assumptions (by the previous query) holds its value rather than a saved polarity,
		// so backtrack to the assumptions first, precisely as a restart does
		let assumpDecLevel = self.m_DecLevelOfLastAssignedAssumption;
		if pendingPhases.iter().any(|&l| { let lv = self.ExtHintsE2IVar(l.abs()); lv != BadULit && self.IsAssigned(lv) && self.GetAssignedDecLevel(lv) > assumpDecLevel })
		{
			self.Backtrack(assumpDecLevel as TLit, false, false);
		}

		for l in pendingPhases
		{
			let lv = self.ExtHintsE2IVar(l.abs());
			if lv == BadULit
			{
				self.m_ExtHints.m_PendingPhases.push(l);
			}
			else if !self.IsAssigned(lv)
			{
				// The external variable may be mapped to a negated internal literal
				self.m_AssignmentInfo[GetVar(lv) as usize].set_m_IsNegated(IsNeg(lv) != (l < 0));
			}
		}

		let mult = self.m_Params.m_ParamScoreHintsMult.get();
		for (vExternal, score) in std::mem::take(&mut self.m_ExtHints.m_PendingScores)
		{
			let lv = self.ExtHintsE2IVar(vExternal);
			if lv == BadULit
			{
				self.m_ExtHints.m_PendingScores.push((vExternal, score));
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
	pub fn HintPhase(&mut self, l: TLit) -> bool
	{
		self.m_Topi.HintPhase(l)
	}

	// Boost the initial decision score of v by /decision/score_hints/mult * score at the next Solve; ignored if the parameter is 0
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
