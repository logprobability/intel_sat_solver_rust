// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use crate::ReserveExactlyInit;
use std::io::Write;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	// Get the decision literal
	pub(crate) fn Decide(&mut self) -> TULit
	{
		// #topor : we should try other decision heuristics
		// Maple uses DISTANCE for the first 50000 conflicts, then switches between VSIDS and LRB
		// Cadical switches between VSIDS and VMTF
		// Fiver uses CBH
		while !self.m_VsidsHeap.empty()
		{
			let v = self.m_VsidsHeap.remove_min();
			if !self.IsAssignedVar(v)
			{
				self.m_Stat.m_Decisions += 1;
				self.m_Stat.m_SumOfAllDecLevels += self.m_DecLevel as u64;
				return GetLit(v, self.GetNextPolarityIsNegated(v));
			}
		}
		BadULit
	}

	// Changing the configuration to # configNum, so that every configNum generates a unique configuration
	// This is used for enabling different configs in parallel solving
	pub fn ChangeConfigToGiven(&mut self, configNum: u16) -> String
	{
		//	cout << "\tc n <ConfigNum>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "n {}", configNum);
		}

		if configNum == 0
		{
			return String::new();
		}

		let newMode = (self.m_Params.m_ModeParam.get() as i32 + configNum as i32) % m_Modes as i32;
		self.SetParam("/mode/value", newMode as f64);

		let mut ret = "/topor/mode/value ".to_string() + &newMode.to_string();

		let flipCategory: u16 = configNum / m_Modes as u16;
		if flipCategory != 0
		{
			self.SetParam("/decision/polarity/flip_factor", (1000 + flipCategory as i32) as f64);
			ret = ret + " /topor/decision/polarity/flip_factor " + &(1000 + flipCategory as i32).to_string();
		}
		ret
	}

	// Choose the next assigned polarity for the given variable
	pub(crate) fn GetNextPolarityIsNegated(&mut self, v: TUVar) -> bool
	{
		debug_assert!(!self.IsAssignedVar(v));
		let res = if self.IsNotForced(v)
		{
			if self.m_Params.m_ParamPolarityStrat.get() == 1 { (rand() % 2) != 0 }
			// Rust-only extension (topi_ext_target_phase.rs): the target phase in preference to the saved phase
			else if self.m_Params.m_ParamTargetPhaseOn.get() != 0 { self.ExtTargetPhaseGetIsNegated(v) }
			else { self.m_AssignmentInfo[v as usize].m_IsNegated() }
		}
		else
		{
			self.m_PolarityInfo[v as usize].GetNextPolarityIsNegated()
		};

		if self.m_Params.m_ParamPolarityFlipFactor.get() != 0
		{
			self.m_NonForcedPolaritySelectionForFlip += 1;
			if self.m_NonForcedPolaritySelectionForFlip.is_multiple_of(self.m_Params.m_ParamPolarityFlipFactor.get()) { !res } else { res }
		}
		else
		{
			res
		}
	}

	// This function is invoked after every conflict
	pub(crate) fn UpdateDecisionStrategyOnNewConflict(&mut self, glueLearnt: TUV, lowestGlueUpdateVar: TUVar, fakeTrailEnd: TUVar)
	{
		let D1LowerOrEqD2UpToPrecision = |d1: f64, d2: f64|
		{
			const precision: f64 = 0.000001;
			d1 <= d2 || d1 - d2 <= precision
		};

		debug_assert!(self.m_Stat.m_VarDecay != 0.0);

		let varDecayInc: f64 = if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT { self.m_Params.m_ParamVarDecayIncS.get() } else if self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL { self.m_Params.m_ParamVarDecayIncAi.get() } else { self.m_Params.m_ParamVarDecayInc.get() };
		let varDecayMax: f64 = if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT { self.m_Params.m_ParamVarDecayMaxS.get() } else if self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL { self.m_Params.m_ParamVarDecayMaxAi.get() } else { self.m_Params.m_ParamVarDecayMax.get() };

		if self.m_Params.m_ParamVarDecayUpdateConfRate.get() > 0 && self.m_Stat.m_Conflicts.is_multiple_of(self.m_Params.m_ParamVarDecayUpdateConfRate.get() as u64)
			&& D1LowerOrEqD2UpToPrecision(self.m_Stat.m_VarDecay + varDecayInc, varDecayMax)
		{
			self.m_Stat.m_VarDecay += varDecayInc;
		}

		if self.m_Params.m_ParamVarActivityGlueUpdate.get() && glueLearnt != 0
		{
			debug_assert!(fakeTrailEnd != lowestGlueUpdateVar);
			let mut v = fakeTrailEnd;
			while v != BadUVar && self.m_VarInfo[v as usize].m_TrailNext != lowestGlueUpdateVar
			{
				let ai = self.m_AssignmentInfo[v as usize];
				debug_assert!(ai.m_IsAssigned() || fakeTrailEnd != self.m_TrailEnd);
				let vi = self.m_VarInfo[v as usize];

				if ai.m_Visit() && (ai.IsAssignedBinary() ||
					(vi.m_ParentClsInd != Self::BadClsInd && self.ClsGetIsLearnt(vi.m_ParentClsInd) && self.ClsGetGlue(vi.m_ParentClsInd) < glueLearnt))
				{
					self.UpdateScoreVar(v, 1.0);
				}
				v = self.m_VarInfo[v as usize].m_TrailPrev;
			}
		}

		// Decay activity
		let varDecay = self.m_Stat.m_VarDecay;
		self.m_VsidsHeap.var_inc_update(varDecay, &mut self.m_Stat.m_VarActivityInc);
	}

	pub(crate) fn DecisionInit(&mut self)
	{
		debug_assert!(self.m_QueryCurr != TQueryType::QUERY_NONE);
		if (self.m_QueryCurr == TQueryType::QUERY_INIT) || (self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL && self.m_Params.m_ParamVarActivityIncDecayReinitN.get()) ||
			(self.m_QueryCurr == TQueryType::QUERY_INC_SHORT && self.m_Params.m_ParamVarActivityIncDecayReinitS.get() &&
				(self.m_Params.m_ParamVarActivityIncDecayStopReinitSInv.get() == 0 || self.m_Stat.m_ShortIncSolveInvs < self.m_Params.m_ParamVarActivityIncDecayStopReinitSInv.get() as u64) &&
				(self.m_Params.m_ParamVarActivityIncDecayStopReinitRestart.get() == 0 || self.m_Stat.m_Restarts < self.m_Params.m_ParamVarActivityIncDecayStopReinitRestart.get() as u64) &&
				(self.m_Params.m_ParamVarActivityIncDecayStopReinitConflict.get() == 0 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamVarActivityIncDecayStopReinitConflict.get() as u64) &&
				(self.m_Params.m_ParamVarActivityIncDecayStopReinitTime.get() == 0.0 || self.m_Stat.m_OverallTime.WallTimePassedSinceStartOrResetConst() < self.m_Params.m_ParamVarActivityIncDecayStopReinitTime.get()))
		{
			self.m_Stat.m_VarDecay = if self.m_QueryCurr == TQueryType::QUERY_INIT { self.m_Params.m_ParamVarActivityIncDecay.get() } else { self.m_Params.m_ParamVarActivityIncDecayReinitVal.get() };
		}

		if self.InitClssBoostScoreStratOn()
		{
			self.m_CurrInitClssBoostScoreMult = if self.InitClssBoostScoreStratIsReversedOrder() { self.m_Params.m_ParamInitClssBoostMultLowest.get() } else { self.m_Params.m_ParamInitClssBoostMultHighest.get() };
		}

		if self.m_Params.m_ParamRandomizePolarityAtEachIncrementalCall.get() && self.m_Stat.m_SolveInvs > 1
		{
			let mut v: TUVar = 1;
			while v < self.GetNextVar()
			{
				self.FixPolarityInternal(GetLit(v, (rand() % 2) != 0), true);
				v += 1;
			}
		}
	}

	pub(crate) fn UpdateScoreVar(&mut self, v: TUVar, mult: f64)
	{
		let isRescaled = self.m_VsidsHeap.increase_score(v, mult, &mut self.m_Stat.m_VarActivityInc);
		if self.m_CurrCustomBtStrat > 0
		{
			if isRescaled
			{
				for s in self.m_BestScorePerDecLevel.get_span_cap(0).as_mut_slice()
				{
					*s *= 1e-100;
				}
			}

			if self.IsAssignedVar(v)
			{
				let decLevel = self.GetAssignedDecLevelVar(v);
				let currScore = self.m_VsidsHeap.get_var_score(v);

				if decLevel as usize >= self.m_BestScorePerDecLevel.cap()
				{
					debug_assert!(decLevel <= self.GetNextVar());
					ReserveExactlyInit!(self, self.m_BestScorePerDecLevel, self.GetNextVar() as usize, 0, "m_BestScorePerDecLevel in UpdateScoreVar");
					if self.IsUnrecoverable()
					{
						return;
					}
				}

				if currScore > self.m_BestScorePerDecLevel[decLevel as usize]
				{
					self.m_BestScorePerDecLevel[decLevel as usize] = currScore;
					debug_assert!(self.NV(2) || self.P(&("m_BestScorePerDecLevel[".to_string() + &decLevel.to_string() + "] updated to " + &format!("{:.6}", currScore) + " in UpdateScoreVar\n")));
				}

				debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.CalcMaxDecLevelScore(decLevel) == self.m_BestScorePerDecLevel[decLevel as usize]);
			}
		}
	}
}
