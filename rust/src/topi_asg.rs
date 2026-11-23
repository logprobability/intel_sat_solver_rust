// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use crate::ReserveExactlyInit;
use std::io::Write;

// The equivalent of streaming a double into an ostream with the default formatting (%g with 6 significant digits)
fn OstreamDouble(v: f64) -> String
{
	if v.is_nan()
	{
		return "nan".to_string();
	}
	if v.is_infinite()
	{
		return if v < 0. { "-inf".to_string() } else { "inf".to_string() };
	}
	if v == 0.
	{
		return if v.is_sign_negative() { "-0".to_string() } else { "0".to_string() };
	}

	let sci = format!("{:.5e}", v);
	let (mantissa, exp) = sci.split_once('e').unwrap();
	let exp: i32 = exp.parse().unwrap();

	let strip = |s: &str| -> String
	{
		if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
	};

	if !(-4..6).contains(&exp)
	{
		format!("{}e{}{:02}", strip(mantissa), if exp < 0 { '-' } else { '+' }, exp.abs())
	}
	else
	{
		strip(&format!("{:.*}", (5 - exp) as usize, v))
	}
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn DebugLongImplicationInvariantHolds(&mut self, l: TULit, decLevel: TUV, parentClsIndIfLong: Ind) -> bool
	{
		// Parent clause is well-defined
		debug_assert!(parentClsIndIfLong != Self::BadClsInd);
		// Implied literal is well-defined
		debug_assert!(l != BadULit);
		// Implied literal is unassigned
		debug_assert!(!self.IsAssigned(l));

		// Get the const clause span
		let cls = self.ConstClsSpan(parentClsIndIfLong, TUV::MAX);

		// The implied literal l must be one of the first literals in the clause
		let lOneOfFirstLits = l == cls.get(0) || l == cls.get(1);
		debug_assert!(lOneOfFirstLits);

		// The second WL in the clause must be assigned false
		let secondLit = if l == cls.get(0) { cls.get(1) } else { cls.get(0) };
		debug_assert!(self.IsFalsified(secondLit));

		// Our literal must be implied at the same level as the second WL
		debug_assert!(decLevel == self.GetAssignedDecLevel(secondLit));

		// The decision level of the implication is the highest in the clause
		let highestNonWLDecLevel = self.GetAssignedDecLevel(cls.get(self.GetAssignedLitsHighestDecLevelIt(cls, 2)));
		debug_assert!(decLevel >= highestNonWLDecLevel);

		parentClsIndIfLong != Self::BadClsInd && l != BadULit && !self.IsAssigned(l) && lOneOfFirstLits && self.IsFalsified(secondLit) && decLevel == self.GetAssignedDecLevel(secondLit) && decLevel >= highestNonWLDecLevel
	}

	// Returns true iff the assignment is contradictory
	pub(crate) fn Assign(&mut self, l: TULit, parentClsInd: Ind, otherWatch: TULit, decLevel: TUV, toPropagate: bool, externalAssignment: bool) -> bool
	{
		debug_assert!(parentClsInd == Self::BadClsInd || self.DebugLongImplicationInvariantHolds(l, decLevel, parentClsInd));
		self.m_Stat.m_Assignments += 1;

		let v = GetVar(l);
		debug_assert!((v as usize) < self.m_VarInfo.cap());

		let isAssigned = self.IsAssigned(l);
		let isNegated = self.IsAssignedNegated(l);
		if isAssigned
		{
			// Ignore if already assigned the same value; return false (contradiction), if assigned the opposite value
			return isNegated;
		}

		if self.M_ReportUnitCls.is_some() && decLevel == 0 && !externalAssignment
		{
			let externalLit = self.GetExternalLit(l);
			let threadId = self.m_ThreadId;
			if let Some(ReportUnitCls) = self.M_ReportUnitCls.as_mut()
			{
				ReportUnitCls(threadId, externalLit);
			}
		}

		// It is either that: (1) The trail is empty, or (2) The level is 0, or (3) A new level is started, or (4) The level is non-empty
		// So, there cannot be an empty intermediate level
		debug_assert!(self.m_TrailEnd == BadUVar || decLevel == 0 || decLevel == self.m_DecLevel || self.m_TrailLastVarPerDecLevel[decLevel as usize] != BadUVar);

		let trailPrev: TUVar = if self.m_TrailLastVarPerDecLevel[decLevel as usize] == BadUVar && decLevel != 0 { self.m_TrailEnd } else { self.m_TrailLastVarPerDecLevel[decLevel as usize] };
		let trailNext: TUVar;

		if trailPrev == BadUVar
		{
			trailNext = self.m_TrailStart;
			self.m_TrailStart = v;
		}
		else
		{
			trailNext = self.m_VarInfo[trailPrev as usize].m_TrailNext;
			self.m_VarInfo[trailPrev as usize].m_TrailNext = v;
		}

		if trailNext != BadUVar
		{
			self.m_VarInfo[trailNext as usize].m_TrailPrev = v;
		}
		else
		{
			self.m_TrailEnd = v;
		}

		if self.m_Params.m_ParamPhaseBoostFlippedForced.get() && self.IsForced(v) && self.m_PolarityInfo[v as usize].GetNextPolarityIsNegated() != IsNeg(l)
		{
			self.UpdateScoreVar(v, 1.0);
		}

		self.m_AssignmentInfo[v as usize].Assign(IsNeg(l), parentClsInd, otherWatch);
		self.m_VarInfo[v as usize].Assign(parentClsInd, otherWatch, decLevel, trailPrev, trailNext);

		self.m_TrailLastVarPerDecLevel[decLevel as usize] = v;

		if toPropagate
		{
			self.ToPropagatePushBack(l);
		}

		self.m_AssignedVarsNum += 1;

		if self.m_AssignmentInfo[v as usize].m_IsAssump() && self.IsAssumpFalsifiedGivenVar(v) &&
			(self.m_EarliestFalsifiedAssump == BadULit || !self.IsAssigned(self.m_EarliestFalsifiedAssump) || self.GetAssignedDecLevel(l) <= self.GetAssignedDecLevel(self.m_EarliestFalsifiedAssump))
		{
			self.m_EarliestFalsifiedAssump = self.GetAssumpLitForVar(v);
			self.m_LatestEarliestFalsifiedAssump = self.m_EarliestFalsifiedAssump;
			self.m_LatestEarliestFalsifiedAssumpSolveInv = self.m_Stat.m_SolveInvs;
		}

		if self.m_CurrCustomBtStrat > 0 && self.m_VsidsHeap.var_score_exists(v)
		{
			let currScore = self.m_VsidsHeap.get_var_score(v);

			if decLevel as usize >= self.m_BestScorePerDecLevel.cap()
			{
				debug_assert!(decLevel <= self.GetNextVar());
				ReserveExactlyInit!(self, self.m_BestScorePerDecLevel, self.GetNextVar() as usize, 0, "m_BestScorePerDecLevel in Assign");
				if self.IsUnrecoverable()
				{
					return false;
				}
			}

			if self.IsAssignedDecVar(v) || currScore > self.m_BestScorePerDecLevel[decLevel as usize]
			{
				self.m_BestScorePerDecLevel[decLevel as usize] = currScore;
				debug_assert!(self.NV(2) || self.P(&("m_BestScorePerDecLevel[".to_string() + &decLevel.to_string() + "] updated to " + &format!("{:.6}", currScore) + " in Assign\n")));
			}
			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.CalcMaxDecLevelScore(decLevel) == self.m_BestScorePerDecLevel[decLevel as usize]);
		}

		false
	}

	pub(crate) fn UnassignVar(&mut self, v: TUVar)
	{
		debug_assert!((v as usize) < self.m_VarInfo.cap());
		debug_assert!(self.m_AssignmentInfo[v as usize].m_IsAssigned());

		if self.m_VarInfo[v as usize].m_TrailNext != BadUVar
		{
			let trailNext = self.m_VarInfo[v as usize].m_TrailNext;
			self.m_VarInfo[trailNext as usize].m_TrailPrev = self.m_VarInfo[v as usize].m_TrailPrev;
		}
		else
		{
			debug_assert!(self.m_TrailEnd == v);
			self.m_TrailEnd = self.m_VarInfo[v as usize].m_TrailPrev;
		}
		if self.m_VarInfo[v as usize].m_TrailPrev != BadUVar
		{
			let trailPrev = self.m_VarInfo[v as usize].m_TrailPrev;
			self.m_VarInfo[trailPrev as usize].m_TrailNext = self.m_VarInfo[v as usize].m_TrailNext;
		}
		else
		{
			debug_assert!(self.m_TrailStart == v);
			self.m_TrailStart = self.m_VarInfo[v as usize].m_TrailNext;
		}

		let lDecLevel = self.GetAssignedDecLevelVar(v);

		if self.m_TrailLastVarPerDecLevel[lDecLevel as usize] == v
		{
			if self.m_VarInfo[v as usize].m_TrailPrev == BadUVar || self.GetAssignedDecLevelVar(self.m_VarInfo[v as usize].m_TrailPrev) != lDecLevel
			{
				self.m_TrailLastVarPerDecLevel[lDecLevel as usize] = BadUVar;
			}
			else
			{
				self.m_TrailLastVarPerDecLevel[lDecLevel as usize] = self.m_VarInfo[v as usize].m_TrailPrev;
			}
		}

		self.m_AssignmentInfo[v as usize].Unassign();

		self.m_VsidsHeap.reinsert_if_not_in_heap(v);

		if GetVar(self.m_FlippedLit) == v
		{
			self.m_FlippedLit = BadULit;
		}

		self.m_AssignedVarsNum -= 1;
	}

	pub(crate) fn Unassign(&mut self, l: TULit)
	{
		let v = GetVar(l);
		self.UnassignVar(v);
	}

	// Boost the score of the variable v by mult
	pub fn BoostScore(&mut self, vExternal: TLit, value: f64)
	{
		// cout << "\tc lb <BoostScoreLit> <Mult>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "lb {} {}", vExternal, OstreamDouble(value));
		}

		self.HandleIncomingUserVar(vExternal, false);

		if self.IsUnrecoverable()
		{
			return;
		}

		let l = self.E2I(vExternal);
		let v = GetVar(l);
		self.UpdateScoreVar(v, if self.m_Params.m_ParamIfExternalBoostScoreValueOverride.get() { self.m_Params.m_ParamExternalBoostScoreValueOverride.get() } else { value });
	}

	pub(crate) fn FixPolarityInternal(&mut self, l: TULit, onlyOnce: bool)
	{
		let v = GetVar(l);
		if !self.m_PolarityInfoActivated || v as usize >= self.m_PolarityInfo.cap()
		{
			self.m_PolarityInfoActivated = true;
			ReserveExactlyInit!(self, self.m_PolarityInfo, self.GetNextVar() as usize, 0, "m_PolarityInfo in FixPolarityInternal");
		}
		if self.IsUnrecoverable()
		{
			return;
		}

		let updateParamsWhenVarFixed = self.m_Params.m_ParamUpdateParamsWhenVarFixed.get();
		if updateParamsWhenVarFixed > 0 && !self.m_UpdateParamsWhenVarFixedDone && ((onlyOnce && updateParamsWhenVarFixed == 1) || (!onlyOnce && updateParamsWhenVarFixed == 2) || updateParamsWhenVarFixed == 3)
		{
			self.SetParam("/decision/init_clss_boost/strat", 4.);
			self.m_UpdateParamsWhenVarFixedDone = true;
		}
		self.m_PolarityInfo[v as usize] = TPolarityInfo::new(!onlyOnce, IsNeg(l));
	}

	// Fix the polarity of the variable |l| to l
	// onlyOnce = false: change until ClearUserPolarityInfo(|l|) is invoked (or, otherwise, forever)
	// onlyOnce = true: change only once right now
	pub fn FixPolarity(&mut self, lExternal: TLit, onlyOnce: bool)
	{
		// cout << "\tc lf <FixPolarityLit> <OnlyOnce>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "lf {} {}", lExternal, onlyOnce as i32);
		}
		let vExternal = ExternalLit2ExternalVar(lExternal);

		self.HandleIncomingUserVar(vExternal, false);

		if self.IsUnrecoverable()
		{
			return;
		}

		let l = self.E2I(lExternal);

		self.FixPolarityInternal(l, onlyOnce);
	}

	// Create an internal literal for l *now*
	pub fn CreateInternalLit(&mut self, lExternal: TLit)
	{
		// cout << "\tc ll <Lit>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "ll {}", lExternal);
		}
		let vExternal = ExternalLit2ExternalVar(lExternal);

		self.HandleIncomingUserVar(vExternal, false);
	}

	pub(crate) fn ClearUserPolarityInfoInternal(&mut self, v: TUVar)
	{
		if self.m_PolarityInfoActivated
		{
			if v as usize >= self.m_PolarityInfo.cap()
			{
				ReserveExactlyInit!(self, self.m_PolarityInfo, self.GetNextVar() as usize, 0, "m_PolarityInfo in ClearUserPolarityInfoInternal");
			}
			self.m_PolarityInfo[v as usize].Clear();
		}
	}

	// Clear any polarity information of the variable v, provided by the user (so, the default solver's heuristic will be used to determine polarity)
	pub fn ClearUserPolarityInfo(&mut self, vExternal: TLit)
	{
		//	cout << "\tc lc <ClearUserPolarityInfoLit>" << endl;
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "lc {}", vExternal);
		}

		self.HandleIncomingUserVar(vExternal, false);

		if self.IsUnrecoverable()
		{
			return;
		}

		if self.m_PolarityInfoActivated
		{
			let l = self.E2I(vExternal);
			let v = GetVar(l);
			self.ClearUserPolarityInfoInternal(v);
		}
	}

	pub(crate) fn TrailAssertConsistency(&mut self) -> bool
	{
		let mut v = self.m_TrailStart;
		while v != BadUVar
		{
			let _l = self.GetAssignedLitForVar(v);
			let vTrailNext = self.m_VarInfo[v as usize].m_TrailNext;

			let c1 = (vTrailNext == BadUVar && self.m_TrailEnd == v) || (vTrailNext != BadUVar && self.m_VarInfo[vTrailNext as usize].m_TrailPrev == v);
			debug_assert!(c1 || self.P(&("***** TrailAssertConsistency failure: ".to_string() + &self.STrail() + "\n")));
			debug_assert!(c1);

			let vDecLevel = self.GetAssignedDecLevelVar(v);
			let nextDecLevel = if vTrailNext == BadUVar { TUV::MAX } else { self.GetAssignedDecLevelVar(vTrailNext) };

			let c2 = vDecLevel == nextDecLevel || self.m_TrailLastVarPerDecLevel[vDecLevel as usize] == v;
			debug_assert!(c2 || self.P(&("***** TrailAssertConsistency failure 2: ".to_string() + &self.STrail() + "\n")));
			debug_assert!(c2);

			v = self.m_VarInfo[v as usize].m_TrailNext;
		}

		true
	}
}

#[cfg(test)]
mod tests
{
	use super::OstreamDouble;

	#[test]
	fn ostream_double_formats_like_cpp()
	{
		assert_eq!(OstreamDouble(1.0), "1");
		assert_eq!(OstreamDouble(0.5), "0.5");
		assert_eq!(OstreamDouble(0.0), "0");
		assert_eq!(OstreamDouble(-0.0), "-0");
		assert_eq!(OstreamDouble(1234567.0), "1.23457e+06");
		assert_eq!(OstreamDouble(0.00001234), "1.234e-05");
		assert_eq!(OstreamDouble(100000.0), "100000");
		assert_eq!(OstreamDouble(0.0001), "0.0001");
		assert_eq!(OstreamDouble(-2.25), "-2.25");
		assert_eq!(OstreamDouble(1e100), "1e+100");
		assert_eq!(OstreamDouble(f64::NAN), "nan");
		assert_eq!(OstreamDouble(f64::INFINITY), "inf");
		assert_eq!(OstreamDouble(f64::NEG_INFINITY), "-inf");
	}
}
