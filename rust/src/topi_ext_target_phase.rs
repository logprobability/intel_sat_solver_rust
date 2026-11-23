// Rust-only extension (no C++ counterpart): target/best-phase saving and rephasing (the self-generated phase oracle of CaDiCaL and Kissat)
//
// Target phase: the assignment at the largest conflict-free trail seen since the latest rephase. With /phase/target/on, a decision on a variable, whose polarity isn't forced,
// takes the target phase of the variable in preference to its saved phase, so the search keeps extending the largest consistent assignment it has found.
// Best phase: the same, but since the latest rephase to the best phases (and the start of the query).
// Rephasing: on a conflict schedule (/phase/rephase/conf_init, conf_incr, conf_mult), right after a restart, the saved phases of the unassigned variables are reset to
// the original phases (O: those of the start of the query), their inversion (I), the best phases (B), or they are flipped (F; random phases with /phase/rephase/random):
// O I (B F B O B I)*, as in CaDiCaL. A rephase resets the target.
//
// No second polarity store: the saved phase is the m_IsNegated bit of m_AssignmentInfo, as ever, and the target, best and original phases live in the 5 spare bits
// of the variable's entry in m_PolarityInfo (so they follow the variable, when the variables are compressed).
// The existing phase management takes precedence: a polarity, which is forced by the user (FixPolarity) or by /phase/force_solution, wins over the target phase,
// unless the phase stage is PHASE_STAGE_DONT_FORCE, during which the solver picks the polarity itself (so, the target phase), and a rephase doesn't touch the saved phase of such a variable.
// A phase hint (topi_ext_hints.rs) seeds the saved phase before the query, so it is the original phase of its variable, and it is in effect until the variable gets a target phase
// (the target is invalidated at the start of every query, and the first target of a query is taken at its first conflict, where the hinted phase of a variable,
// which hasn't been assigned yet, becomes its target phase).
//
// Everything is off by default: with /phase/target/on 0 and /phase/rephase/conf_init 0 the hooks return after testing the parameters, and m_PolarityInfo is not touched.
// Only polarities of decisions are affected: neither the clauses nor the proof, so the answers stay correct, whatever the phases are.

use crate::topi::*;
use crate::ReserveExactlyInit;

// The state of the extension (the per-variable phases are in m_PolarityInfo; the counters are in m_Stat)
#[derive(Default)]
pub(crate) struct CExtTargetPhase
{
	// The size of the largest conflict-free trail since the latest rephase (or the start of the query), which the target phases hold
	pub(crate) m_TargetAssigned: TUVar,
	// The size of the largest conflict-free trail since the latest rephase to the best phases (or the start of the query), which the best phases hold
	pub(crate) m_BestAssigned: TUVar,
	// The current interval between two rephases in conflicts; 0: the schedule hasn't been started
	pub(crate) m_RephaseInterval: u64,
	// The number of conflicts, starting from which the next restart rephases
	pub(crate) m_RephaseNextConf: u64,
}

// The spare bits of TPolarityInfo (the bits 0 to 2 are those of the C++ struct)
impl TPolarityInfo
{
	const ExtTargetValid: u8 = 1 << 3;
	const ExtTargetNegated: u8 = 1 << 4;
	const ExtBestValid: u8 = 1 << 5;
	const ExtBestNegated: u8 = 1 << 6;
	const ExtOrigNegated: u8 = 1 << 7;

	#[inline(always)]
	fn ExtSet(&mut self, validBit: u8, negatedBit: u8, isNegated: bool)
	{
		self.m_Bits = (self.m_Bits & !negatedBit) | validBit | (if isNegated { negatedBit } else { 0 });
	}
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	#[inline(always)]
	fn ExtTargetPhaseIsOn(&self) -> bool
	{
		self.m_Params.m_ParamTargetPhaseOn.get() != 0 || self.m_Params.m_ParamRephaseConfInit.get() != 0
	}

	// Are the phases of all the variables tracked (so that every existing variable has an entry in m_PolarityInfo)?
	#[inline(always)]
	fn ExtTargetPhaseIsTracked(&self) -> bool
	{
		self.m_PolarityInfoActivated && self.m_PolarityInfo.cap() >= self.GetNextVar() as usize
	}

	// The hook of Solve, right before the search loop (after the phase hints have been applied): the saved phases become the original ones, and the target and the best phases are invalidated,
	// since the trail sizes of different queries aren't comparable, and the saved phases (the latest model or the hints) are a better start than an old target
	pub(crate) fn ExtTargetPhaseInitQuery(&mut self)
	{
		if !self.ExtTargetPhaseIsOn()
		{
			self.m_ExtTargetPhase.m_RephaseInterval = 0;
			return;
		}

		if !self.ExtTargetPhaseIsTracked()
		{
			// Precisely as FixPolarityInternal does (an all-zero entry holds no user polarity information)
			self.m_PolarityInfoActivated = true;
			ReserveExactlyInit!(self, self.m_PolarityInfo, self.GetNextVar() as usize, 0, "m_PolarityInfo in ExtTargetPhaseInitQuery");
		}

		// (should the allocation have failed, there are no entries, and the search loop returns the error right away)
		for v in 1..std::cmp::min(self.GetNextVar() as usize, self.m_PolarityInfo.cap())
		{
			let isNegated = self.m_AssignmentInfo[v].m_IsNegated();
			let pi = &mut self.m_PolarityInfo[v];
			pi.m_Bits = (pi.m_Bits & 0b111) | (if isNegated { TPolarityInfo::ExtOrigNegated } else { 0 });
		}
		self.m_ExtTargetPhase.m_TargetAssigned = 0;
		self.m_ExtTargetPhase.m_BestAssigned = 0;

		let confInit = self.m_Params.m_ParamRephaseConfInit.get() as u64;
		if confInit == 0
		{
			self.m_ExtTargetPhase.m_RephaseInterval = 0;
		}
		else if self.m_ExtTargetPhase.m_RephaseInterval == 0
		{
			self.m_ExtTargetPhase.m_RephaseInterval = confInit;
			self.m_ExtTargetPhase.m_RephaseNextConf = self.m_Stat.m_Conflicts + confInit;
		}
	}

	// The hook of Solve, when BCP has returned a contradiction (before the conflict is analyzed): the assignment below the conflicting decision level is conflict-free;
	// if it is the largest one so far, it becomes the target (and, respectively, the best) phases
	pub(crate) fn ExtTargetPhaseOnConflict(&mut self)
	{
		if !self.ExtTargetPhaseIsOn() || !self.ExtTargetPhaseIsTracked()
		{
			return;
		}

		// The trail is ordered by the decision levels, so the variables of the conflicting decision level are at its end
		let mut assigned = self.m_AssignedVarsNum;
		let mut v = self.m_TrailEnd;
		while v != BadUVar && self.m_VarInfo[v as usize].m_DecLevel >= self.m_DecLevel
		{
			assigned -= 1;
			v = self.m_VarInfo[v as usize].m_TrailPrev;
		}

		let updateTarget = assigned > self.m_ExtTargetPhase.m_TargetAssigned;
		let updateBest = assigned > self.m_ExtTargetPhase.m_BestAssigned;
		if !updateTarget && !updateBest
		{
			return;
		}

		// The value of an assigned variable or the saved phase of an unassigned one (for a variable of the conflicting level it is the phase, which backtracking is about to save)
		for v in 1..self.GetNextVar() as usize
		{
			let isNegated = self.m_AssignmentInfo[v].m_IsNegated();
			let pi = &mut self.m_PolarityInfo[v];
			if updateTarget
			{
				pi.ExtSet(TPolarityInfo::ExtTargetValid, TPolarityInfo::ExtTargetNegated, isNegated);
			}
			if updateBest
			{
				pi.ExtSet(TPolarityInfo::ExtBestValid, TPolarityInfo::ExtBestNegated, isNegated);
			}
		}
		if updateTarget
		{
			self.m_ExtTargetPhase.m_TargetAssigned = assigned;
			self.m_Stat.m_ExtTargetPhaseUpdates += 1;
		}
		if updateBest
		{
			self.m_ExtTargetPhase.m_BestAssigned = assigned;
		}
	}

	// The hook of GetNextPolarityIsNegated for a variable, whose polarity isn't forced, when /phase/target/on is non-0: the target phase, if there is one, and the saved phase otherwise
	// /phase/target/on 2: only while the current restart strategy is the numeric one (arithmetic or Luby: the solver's counterpart of the stable mode)
	#[inline]
	pub(crate) fn ExtTargetPhaseGetIsNegated(&mut self, v: TUVar) -> bool
	{
		if (self.m_Params.m_ParamTargetPhaseOn.get() == 1 || self.m_CurrRestartStrat == RESTART_STRAT_NUMERIC) && self.m_PolarityInfoActivated && (v as usize) < self.m_PolarityInfo.cap()
		{
			let bits = self.m_PolarityInfo[v as usize].m_Bits;
			if bits & TPolarityInfo::ExtTargetValid != 0
			{
				self.m_Stat.m_ExtTargetPhaseDecisions += 1;
				return bits & TPolarityInfo::ExtTargetNegated != 0;
			}
		}
		self.m_AssignmentInfo[v as usize].m_IsNegated()
	}

	// The hook of Solve, right after a restart has backtracked to the assumptions: rephase, if the conflict schedule says so
	pub(crate) fn ExtTargetPhaseRephaseIfRequired(&mut self)
	{
		if self.m_ExtTargetPhase.m_RephaseInterval == 0 || self.m_Params.m_ParamRephaseConfInit.get() == 0 || self.m_Stat.m_Conflicts < self.m_ExtTargetPhase.m_RephaseNextConf || !self.ExtTargetPhaseIsTracked()
		{
			return;
		}

		// O I (B F B O B I)*
		let kind = match self.m_Stat.m_ExtRephases { 0 => 'O', 1 => 'I', n => ['B', 'F', 'B', 'O', 'B', 'I'][((n - 2) % 6) as usize] };
		let isRandom = self.m_Params.m_ParamRephaseRandom.get();
		for v in 1..self.GetNextVar() as usize
		{
			let pi = &mut self.m_PolarityInfo[v];
			let bits = pi.m_Bits;
			// The target is reset: the decisions take the new saved phases, until a conflict-free trail is recorded again
			pi.m_Bits &= !(TPolarityInfo::ExtTargetValid | TPolarityInfo::ExtTargetNegated);
			// The value of an assigned variable (at the level of the assumptions or below it) isn't a saved phase, and a polarity determined by the user isn't the solver's to change
			if self.m_AssignmentInfo[v].m_IsAssigned() || pi.IsNextPolarityDetermined()
			{
				continue;
			}
			let ai = &mut self.m_AssignmentInfo[v];
			match kind
			{
				'O' => ai.set_m_IsNegated(bits & TPolarityInfo::ExtOrigNegated != 0),
				'I' => ai.set_m_IsNegated(bits & TPolarityInfo::ExtOrigNegated == 0),
				'B' => if bits & TPolarityInfo::ExtBestValid != 0 { ai.set_m_IsNegated(bits & TPolarityInfo::ExtBestNegated != 0) },
				_ => { let isNegated = if isRandom { rand() % 2 != 0 } else { !ai.m_IsNegated() }; ai.set_m_IsNegated(isNegated) }
			}
		}
		self.m_ExtTargetPhase.m_TargetAssigned = 0;
		if kind == 'B'
		{
			self.m_ExtTargetPhase.m_BestAssigned = 0;
		}

		self.m_Stat.m_ExtRephases += 1;
		let interval = self.m_ExtTargetPhase.m_RephaseInterval as f64 * self.m_Params.m_ParamRephaseConfMult.get() + self.m_Params.m_ParamRephaseConfIncr.get() as f64;
		// (a float-to-integer cast saturates)
		self.m_ExtTargetPhase.m_RephaseInterval = std::cmp::max(interval as u64, 1);
		self.m_ExtTargetPhase.m_RephaseNextConf = self.m_Stat.m_Conflicts.saturating_add(self.m_ExtTargetPhase.m_RephaseInterval);
		debug_assert!(self.NV(1) || self.P(&("Rephase #".to_string() + &self.m_Stat.m_ExtRephases.to_string() + " (" + &kind.to_string() + ")\n")));
	}

	// The extra line of the short statistics, if the extension is on
	pub(crate) fn ExtTargetPhaseStatStr(&self) -> String
	{
		if !self.ExtTargetPhaseIsOn()
		{
			return String::new();
		}
		format!("c ToporExt Rephases {} TargetUpds {} TargetDecs {}\n", self.m_Stat.m_ExtRephases, self.m_Stat.m_ExtTargetPhaseUpdates, self.m_Stat.m_ExtTargetPhaseDecisions)
	}
}
