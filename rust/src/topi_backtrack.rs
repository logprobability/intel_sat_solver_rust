// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use std::io::Write;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn BacktrackingInit(&mut self)
	{
		self.m_CurrChronoBtIfHigher = if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT { self.m_Params.m_ParamChronoBtIfHigherS.get() } else if self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL { self.m_Params.m_ParamChronoBtIfHigherN.get() } else { self.m_Params.m_ParamChronoBtIfHigherInit.get() };
		self.m_CurrCustomBtStrat = if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT { self.m_Params.m_ParamCustomBtStratS.get() } else if self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL { self.m_Params.m_ParamCustomBtStratN.get() } else { self.m_Params.m_ParamCustomBtStratInit.get() };
		self.m_ConfsSinceNewInv = self.m_Stat.m_Conflicts;
	}

	// Backtrack to the end of decLevel; the 2nd parameter is required for statistics only
	pub fn Backtrack(&mut self, decLevelL: TLit, isBCPBacktrack: bool, isAPICall: bool)
	{
		let mut decLevel = decLevelL as TUV;
		//	cout << "\tc b <BacktrackLevel>" << endl;
		if isAPICall
		{
			if let Some(dumpFile) = &mut self.m_DumpFile
			{
				let _ = writeln!(dumpFile, "b {}", decLevel);
			}
		}

		if decLevel >= self.m_DecLevel
		{
			return;
		}

		debug_assert!(self.NV(2) || self.P(&("***** Backtracking to ".to_string() + &decLevel.to_string() + "\n")));

		self.m_Stat.m_Backtracks += 1;
		if isBCPBacktrack
		{
			self.m_Stat.m_BCPBacktracks += 1;
		}
		self.m_Stat.m_ChronoBacktracks += (decLevel == self.m_DecLevel - 1) as u64;

		// Cleaning up collapsed decision levels
		while decLevel != 0 && self.m_TrailLastVarPerDecLevel[decLevel as usize] == BadUVar
		{
			if decLevel == self.m_DecLevelOfLastAssignedAssumption
			{
				self.m_DecLevelOfLastAssignedAssumption -= 1;
			}
			decLevel -= 1;
		}

		while self.m_TrailEnd != self.m_TrailLastVarPerDecLevel[decLevel as usize]
		{
			self.UnassignVar(self.m_TrailEnd);
		}

		self.m_DecLevel = decLevel;

		debug_assert!(self.NV(2) || self.P(&("***** Backtracked to ".to_string() + &self.m_DecLevel.to_string() + "\n")));
	}
}
