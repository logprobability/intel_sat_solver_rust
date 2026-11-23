// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use std::collections::HashSet;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn IngRemoveBinaryWatchesIfRequired(&mut self)
	{
		// Remove duplicate binary watches, if any are there and not handled on-the-fly
		if self.m_Params.m_ParamExistingBinWLStrat.get() <= 2 || self.m_IngLastEverAddedBinaryClss >= self.m_Stat.m_EverAddedBinaryClss
		{
			return;
		}

		fn RemoveDuplicateBinWatches<Ind: TUInd, const Compress: bool>(this: &mut CTopi<Ind, Compress>, l: TULit)
		{
			// A reference into m_Watches in the C++ version (the watches aren't reallocated in this function)
			let wi: *mut TWatchInfo<Ind> = this.m_Watches.get_ptr(l as usize);
			let wi = unsafe { &mut *wi };
			if wi.m_BinaryWatches > Ind::ZERO
			{
				let mut litsInBinWatches: HashSet<TULit> = HashSet::new();
				let binWatches = TSpanTULit::new(unsafe { this.m_W.get_ptr(wi.m_WBInd.as_usize()).add(wi.GetLongEntries()) }, wi.m_BinaryWatches.as_usize());
				let mut currbwInd: usize = 0;
				while currbwInd < wi.m_BinaryWatches.as_usize()
				{
					let currSecondLit = binWatches.get(currbwInd);
					let isNew = litsInBinWatches.insert(currSecondLit);
					if !isNew
					{
						// Already there!
						let v = GetVar(l);
						let currSecondVar = GetVar(currSecondLit);
						// Make sure the code inside runs once per binary clause, rather than twice
						if v < currSecondVar
						{
							this.m_Stat.m_IngsDuplicateBinsRemoved += 1;
							// Boost the score of both the literals of the clause, if required.
							if this.m_Params.m_ParamExistingBinWLStrat.get() == 4
							{
								this.UpdateScoreVar(v, this.m_Params.m_ParamBinWLScoreBoostFactor.get());
								this.UpdateScoreVar(currSecondVar, this.m_Params.m_ParamBinWLScoreBoostFactor.get());
							}
						}

						// Replace the current watch by the last one and decrease the number of watched
						wi.m_BinaryWatches -= Ind::ONE;
						binWatches.set(currbwInd, binWatches.get(wi.m_BinaryWatches.as_usize()));
						currbwInd = currbwInd.wrapping_sub(1);
					}
					currbwInd = currbwInd.wrapping_add(1);
				}
			}
		}

		let mut v: TUVar = 1;
		while v < self.m_LastExistingVar
		{
			RemoveDuplicateBinWatches(self, GetLit(v, true));
			RemoveDuplicateBinWatches(self, GetLit(v, false));
			v += 1;
		}

		self.m_IngLastEverAddedBinaryClss = self.m_Stat.m_EverAddedBinaryClss;
	}

	pub(crate) fn InprocessIfRequired(&mut self)
	{
		// If postpone threshold m_ParamIngPostponeFirstInvConflicts not reached --> exit
		// If postpone threshold reached, but no inprocessing invocations so far --> inprocess ASAP
		//		Assertion: if inprocessing invoked at least once --> the postpone threashold must have been reached
		debug_assert!(self.m_Stat.m_Ings == 0 || self.m_Stat.m_Conflicts >= self.m_Params.m_ParamIngPostponeFirstInvConflicts.get() as u64);
		// Once inprocessing invoked at least once (m_Stat.m_Ings > 0), if the # conflicts since the last inprocessing < m_ParamIngConflictsBeforeNextInvocation,
		// wait, unless we must invoke after every query (m_ParamIngInvokeEveryQueryAfterInitPostpone) and we haven't invoked for this query yet (m_IngLastSolveInv < m_Stat.m_SolveInvs)
		if !self.m_Params.m_ParamInprocessingOn.get() || self.IsUnrecoverable() || self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT || self.m_DecLevel != self.m_DecLevelOfLastAssignedAssumption ||
			(self.m_Stat.m_Conflicts < self.m_Params.m_ParamIngPostponeFirstInvConflicts.get() as u64) ||
			(self.m_Stat.m_Ings > 0 && ((self.m_Stat.m_Conflicts - self.m_IngLastConflicts) < self.m_Params.m_ParamIngConflictsBeforeNextInvocation.get() as u64) &&
				!(self.m_Params.m_ParamIngInvokeEveryQueryAfterInitPostpone.get() && self.m_IngLastSolveInv < self.m_Stat.m_SolveInvs))
		{
			return;
		}

		self.m_Stat.m_Ings += 1;
		self.m_IngLastSolveInv = self.m_Stat.m_SolveInvs;
		self.m_IngLastConflicts = self.m_Stat.m_Conflicts;

		debug_assert!(self.NV(1) || self.P(&("Inprocessing started for time ".to_string() + &self.m_Stat.m_Ings.to_string() + "; m_SolveInvs = " + &self.m_IngLastSolveInv.to_string() + "; m_Conflicts = " + &self.m_IngLastConflicts.to_string())));

		self.IngRemoveBinaryWatchesIfRequired();
	}
}
