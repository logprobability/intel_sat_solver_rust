// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use crate::topor_dyn_array::RemoveGarbage;
use crate::{ReserveExactly, ReserveExactlyInit};
use std::cmp::Ordering;
#[cfg(debug_assertions)]
use std::collections::HashSet;

// std::sort's "less" predicate (a strict weak ordering) --> Ordering
#[inline(always)]
fn LessToOrdering<T: Copy>(a: T, b: T, mut less: impl FnMut(T, T) -> bool) -> Ordering
{
	if less(a, b)
	{
		Ordering::Less
	}
	else if less(b, a)
	{
		Ordering::Greater
	}
	else
	{
		Ordering::Equal
	}
}

// std::remove_if over [first, last): returns the index of the new logical end
#[inline(always)]
fn remove_if<S: TULitSpan>(lits: S, mut first: usize, last: usize, mut p: impl FnMut(TULit) -> bool) -> usize
{
	while first != last && !p(lits.get(first))
	{
		first += 1;
	}
	// The C++ std::remove_if tests first != last here; the loop below does nothing in that case anyway
	let mut i = first + 1;
	while i < last
	{
		if !p(lits.get(i))
		{
			lits.set(first, lits.get(i));
			first += 1;
		}
		i += 1;
	}
	first
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn ReserveVarAndLitData(&mut self, maxAssumps: usize)
	{
		// To surely take into account: (1) any collapsed decision levels +
		// (2) the current decision level (which might still be greater than GetNextVar() + maxAssumps because of the previous call with potentially more assumptions)
		let perDecLevelAlloc = std::cmp::max(self.GetNextVar() as usize + maxAssumps, self.m_DecLevel as usize) + 1;
		ReserveExactlyInit!(self, self.m_Watches, self.GetNextLit() as usize, 0, "m_Watches in ReserveVarAndLitData");
		ReserveExactlyInit!(self, self.m_AssignmentInfo, self.GetNextVar() as usize, 0, "m_AssignmentInfo in ReserveVarAndLitData");
		if self.m_PolarityInfoActivated
		{
			ReserveExactlyInit!(self, self.m_PolarityInfo, self.GetNextVar() as usize, 0, "m_PolarityInfo in ReserveVarAndLitData");
		}
		ReserveExactlyInit!(self, self.m_VarInfo, self.GetNextVar() as usize, 0, "m_VarInfo in ReserveVarAndLitData");
		ReserveExactly!(self, self.m_ToPropagate, self.GetNextVar() as usize, "m_ToPropagate in ReserveVarAndLitData");
		ReserveExactlyInit!(self, self.m_TrailLastVarPerDecLevel, perDecLevelAlloc, BadUVar as u8, "m_TrailLastVarPerDecLevel in ReserveVarAndLitData");
		ReserveExactly!(self, self.m_VsidsHeap, self.GetNextVar() as usize, "m_VsidsHeap in ReserveVarAndLitData");
		ReserveExactly!(self, self.m_HandyLitsClearBefore[0], self.GetNextVar() as usize, "m_HandyLitsCleanBefore[0] in ReserveVarAndLitData");
		if self.m_Params.m_ParamFlippedRecordingMaxLbdToRecord.get() != 0
		{
			ReserveExactly!(self, self.m_HandyLitsClearBefore[1], self.GetNextVar() as usize, "m_HandyLitsCleanBefore[1] in ReserveVarAndLitData");
		}
		ReserveExactly!(self, self.m_VisitedVars, self.GetNextVar() as usize, "m_VisitedVars in ReserveVarAndLitData");
		ReserveExactlyInit!(self, self.m_DecLevelsLastAppearenceCounter, perDecLevelAlloc, 0, "m_DecLevelsLastAppearenceCounter in ReserveVarAndLitData");
		if self.UseI2ELitMap()
		{
			ReserveExactlyInit!(self, self.m_I2ELitMap, self.GetNextVar() as usize, 0, "m_I2ELitMap in ReserveVarAndLitData");
		}
		if self.IsCbLearntOrDrat()
		{
			ReserveExactly!(self, self.m_UserCls, self.GetNextVar() as usize, "m_UserCls in ReserveVarAndLitData");
		}
		if self.m_Params.m_ParamOnTheFlySubsumptionParentMinGlueToDisable.get() > 0
		{
			ReserveExactlyInit!(self, self.m_CurrClsCounters, self.GetNextVar() as usize, 0, "m_CurrClsCounters in ReserveVarAndLitData");
		}
		if self.m_Params.m_ParamRestartStrategyInit.get() == RESTART_STRAT_NUMERIC || self.m_Params.m_ParamRestartStrategyS.get() == RESTART_STRAT_NUMERIC || self.m_Params.m_ParamRestartStrategyN.get() == RESTART_STRAT_NUMERIC
		{
			ReserveExactlyInit!(self, self.m_RstNumericLocalConfsSinceRestartAtDecLevelCreation, perDecLevelAlloc, 0, "m_RstArithLocalConfsSinceRestartAtDecLevelCreation in ReserveVarAndLitData");
		}
		if self.m_Params.m_ParamCustomBtStratInit.get() > 0 || self.m_Params.m_ParamCustomBtStratS.get() > 0 || self.m_Params.m_ParamCustomBtStratN.get() > 0
		{
			ReserveExactlyInit!(self, self.m_BestScorePerDecLevel, perDecLevelAlloc, 0, "m_ParamCustomBtStrat in ReserveVarAndLitData");
		}
		ReserveExactly!(self, self.m_HandleNewUserCls, self.GetNextVar() as usize, "m_HandleNewUserCls in ReserveVarAndLitData");
	}

	pub(crate) fn RemoveVarAndLitData(&mut self, v: TUVar)
	{
		debug_assert!(!self.IsAssignedVar(v));
		for wInd in 0..2u8
		{
			// MarkWatchBufferChunkDeleted doesn't modify the watch info, so it's provided a copy
			let mut wi = self.m_Watches[GetLit(v, wInd != 0) as usize];
			if !wi.IsEmpty()
			{
				self.MarkWatchBufferChunkDeleted(&mut wi);
			}
		}
	}

	pub(crate) fn MoveVarAndLitData(&mut self, vFrom: TUVar, vTo: TUVar)
	{
		debug_assert!(!self.IsAssignedVar(vTo));

		self.RemoveVarAndLitData(vTo);
		self.m_Watches[GetLit(vTo, false) as usize] = self.m_Watches[GetLit(vFrom, false) as usize];
		self.m_Watches[GetLit(vTo, true) as usize] = self.m_Watches[GetLit(vFrom, true) as usize];
		self.m_AssignmentInfo[vTo as usize] = self.m_AssignmentInfo[vFrom as usize];
		if self.m_PolarityInfoActivated
		{
			self.m_PolarityInfo[vTo as usize] = self.m_PolarityInfo[vFrom as usize];
		}
		self.m_VarInfo[vTo as usize] = self.m_VarInfo[vFrom as usize];
		if self.IsAssignedVar(vFrom)
		{
			if self.m_VarInfo[vTo as usize].m_TrailPrev != BadUVar
			{
				let trailPrev = self.m_VarInfo[vTo as usize].m_TrailPrev;
				self.m_VarInfo[trailPrev as usize].m_TrailNext = vTo;
			}

			if self.m_VarInfo[vTo as usize].m_TrailNext != BadUVar
			{
				let trailNext = self.m_VarInfo[vTo as usize].m_TrailNext;
				self.m_VarInfo[trailNext as usize].m_TrailPrev = vTo;
			}

			if self.m_TrailStart == vFrom
			{
				self.m_TrailStart = vTo;
			}

			if self.m_TrailEnd == vFrom
			{
				self.m_TrailEnd = vTo;
			}

			if self.m_TrailLastVarPerDecLevel[self.GetAssignedDecLevelVar(vFrom) as usize] == vFrom
			{
				let decLevel = self.GetAssignedDecLevelVar(vFrom);
				self.m_TrailLastVarPerDecLevel[decLevel as usize] = vTo;
			}
		}

		debug_assert!(self.m_ToPropagate.empty());

		// Will have to replace in heap later!
		self.m_VsidsHeap.replace_pos_score_vars(vFrom, vTo);

		if self.UseI2ELitMap()
		{
			self.m_I2ELitMap[vTo as usize] = self.m_I2ELitMap[vFrom as usize];
		}
	}

	pub(crate) fn RecordDeletedLitsFromCls(&mut self, litsNum: TUV, bitsForLit: u16)
	{
		self.m_Stat.RecordDeletedLitsFromCls(litsNum as u64);
		if Compress
		{
			debug_assert!(bitsForLit != 0);
			self.m_BWasted += Ind::from_u32(litsNum.wrapping_mul(bitsForLit as TUV));
		}
		else
		{
			debug_assert!(bitsForLit == 0);
			self.m_BWasted += Ind::from_u32(litsNum);
		}
	}

	// Delete a binary clause
	pub(crate) fn DeleteBinaryCls(&mut self, binCls: TSpanTULit)
	{
		debug_assert!(binCls.size() == 2);
		self.WLRemoveBinaryWatch(binCls.get(0), binCls.get(1));
		self.WLRemoveBinaryWatch(binCls.get(1), binCls.get(0));
		self.m_Stat.DeleteBinClauses(1);
	}

	pub(crate) fn ClsDeletionInit(&mut self)
	{
		if !self.m_ClsDelInfo.m_Initialized
		{
			self.m_ClsDelInfo.m_ConfsPrev = 0;
			self.m_ClsDelInfo.m_TriggerNext = self.m_Params.m_ParamClsDelLowTriggerInit.get() as u64;
			self.m_ClsDelInfo.m_TriggerInc = self.m_Params.m_ParamClsDelLowTriggerInc.get() as u64;
			self.m_ClsDelInfo.m_TriggerMult = self.m_Params.m_ParamClsDelS1LowTriggerMult.get();
			if self.m_Params.m_ParamClsDelStrategy.get() == 1
			{
				self.m_ClsDelInfo.m_TriggerMax = self.m_Params.m_ParamClsDelS1LowTriggerMax.get() as u64;
			}
			else
			{
				*self.m_ClsDelInfo.m_CurrChange() = self.m_Params.m_ParamClsDelLowTriggerInit.get() as u64;
			}

			self.m_ClsDelInfo.m_FracToDelete = self.m_Params.m_ParamClsDelLowFracToDelete.get();
			self.m_ClsDelInfo.m_GlueNeverDelete = self.m_Params.m_ParamClsDelGlueNeverDelete.get() as u8;
			self.m_ClsDelInfo.m_Clusters = self.m_Params.m_ParamClsDelGlueClusters.get() as u8;
			self.m_ClsDelInfo.m_MaxClusteredGlue = self.m_Params.m_ParamClsDelGlueMaxCluster.get() as u8;

			self.m_ClsDelInfo.m_Initialized = true;
		}
	}

	pub(crate) fn ClsDeletionDecayActivity(&mut self)
	{
		if self.m_Params.m_ParamClsDelStrategy.get() > 0
		{
			self.m_ClsDelOneTierActivityIncrease *= 1. / self.m_Params.m_ParamClsLowDelActivityDecay.get();
		}
	}

	pub(crate) fn ClsDelNewLearntOrGlueUpdate(&mut self, clsInd: Ind, prevGlue: TUV)
	{
		if self.m_Params.m_ParamClsDelStrategy.get() > 0
		{
			let glue = self.ClsGetGlue(clsInd);
			let glueDecreased = glue < prevGlue;

			let mut currActivity: f32 = self.ClsGetActivity(clsInd);
			currActivity += self.m_ClsDelOneTierActivityIncrease as f32;
			self.ClsSetActivity(clsInd, currActivity);
			if currActivity as f64 > 1e20
			{
				// Rescale:
				let mut clsIndLocal = self.ClsLoopFirst(true);
				while !self.ClsLoopCompleted()
				{
					if !(self.ClsChunkDeleted(clsIndLocal) || !self.ClsGetIsLearnt(clsIndLocal))
					{
						let clsRescaledActivity = self.ClsGetActivity(clsIndLocal);
						self.ClsSetActivity(clsIndLocal, clsRescaledActivity * 1e-20f32);
					}
					clsIndLocal = self.ClsLoopNext();
				}

				self.m_ClsDelOneTierActivityIncrease *= 1e-20;
			}

			if glueDecreased && glue <= self.GetGlueMinFreeze()
			{
				self.ClsSetSkipdel(clsInd, true);
			}
		}
	}

	pub(crate) fn DeleteClausesIfRequired(&mut self)
	{
		if self.m_Params.m_ParamClsDelStrategy.get() == 0 || self.IsUnrecoverable() || self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT
			|| (self.ClsDeletionTrigger() < self.m_ClsDelInfo.m_TriggerNext) || (self.m_Params.m_ParamClsDelDeleteOnlyAssumpDecLevel.get() && self.m_DecLevel > self.m_DecLevelOfLastAssignedAssumption)
		{
			return;
		}

		debug_assert!(self.NV(1) || self.P("Clause deletion started for priority\n"));
		debug_assert!(self.NV(2) || self.P(&("The trail: ".to_string() + &self.STrail() + "\n")));

		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.TrailAssertConsistency());
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(true));
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.DebugAssertWaste());

		// The body of the function; the on-exit code follows the block
		'body:
		{
			let learntsSz: usize = self.m_Stat.m_ActiveLongLearntClss as usize;
			let mut learnts: CVector<Ind> = CVector::new(learntsSz, 0);
			if learnts.uninitialized_or_erroneous()
			{
				self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::DeleteClausesIfRequired: couldn't allocate learnts".to_string());
				break 'body;
			}

			let mut undeletableButNotTouched: usize = 0;
			// Mark parent clauses as skipped for this round
			let mut v = self.m_TrailStart;
			while v != BadUVar
			{
				let ai = self.m_AssignmentInfo[v as usize];
				let vi = self.m_VarInfo[v as usize];

				debug_assert!(ai.m_IsAssigned());

				let clsInd = vi.m_ParentClsInd;
				// The "(vi.m_DecLevel > 0 || clsInd + 2 < m_B.cap())" part (and also "if (vi.m_DecLevel > 0)" below) is a work-around,
				// so that parents of globally satisfied variables wouldn't be actually visited (with ClsSetSkipdel),
				// yet they will be counted for clause deletion heuristic. Skipping this condition causes a correctness bug, since global parent aren't maintained,
				// while dealing with the global parents more aggressively (e.g., nullifying them in Assign) hurts the clause deletion heuristic
				if !ai.IsAssignedBinary() && (vi.m_DecLevel > 0 || (clsInd + Ind::from_u32(2)).as_usize() < self.m_B.cap()) && clsInd != Self::BadClsInd && self.ClsGetIsLearnt(clsInd)
				{
					debug_assert!(!self.ClsChunkDeleted(clsInd));

					if !self.ClsGetSkipdel(clsInd)
					{
						undeletableButNotTouched += 1;
						if vi.m_DecLevel > 0
						{
							self.ClsSetSkipdel(clsInd, true);
						}
					}
				}
				v = self.GetTrailNextVar(v);
			}

			let mut clsInd = self.ClsLoopFirst(true);
			while !self.ClsLoopCompleted()
			{
				'iter:
				{
					if self.ClsChunkDeleted(clsInd) || !self.ClsGetIsLearnt(clsInd) || self.ClsGetGlue(clsInd) <= self.m_ClsDelInfo.m_GlueNeverDelete as TUV
					{
						if !self.ClsChunkDeleted(clsInd) && self.ClsGetIsLearnt(clsInd)
						{
							debug_assert!(self.ClsGetGlue(clsInd) <= self.m_ClsDelInfo.m_GlueNeverDelete as TUV);
							undeletableButNotTouched += 1;
						}
						break 'iter;
					}

					if self.ClsGetSkipdel(clsInd)
					{
						self.ClsSetSkipdel(clsInd, false);
						break 'iter;
					}

					learnts.push_back(clsInd);
				}
				clsInd = self.ClsLoopNext();
			}

			let learntsSpan = learnts.get_span();

			if self.m_ClsDelInfo.m_Clusters == 0
			{
				learntsSpan.as_mut_slice().sort_unstable_by(|&a, &b| LessToOrdering(a, b, |clsInd1: Ind, clsInd2: Ind|
				{
					self.ClsGetActivity(clsInd1) < self.ClsGetActivity(clsInd2) || ((self.ClsGetActivity(clsInd1) == self.ClsGetActivity(clsInd2)) && (self.ClsGetGlue(clsInd1) > self.ClsGetGlue(clsInd2)))
				}));
			}
			else
			{
				learntsSpan.as_mut_slice().sort_unstable_by(|&a, &b| LessToOrdering(a, b, |clsInd1: Ind, clsInd2: Ind|
				{
					let glues: [TUV; 2] = [self.ClsGetGlue(clsInd1), self.ClsGetGlue(clsInd2)];
					let clusters: [u8; 2] = [self.m_ClsDelInfo.GetCluster(glues[0]), self.m_ClsDelInfo.GetCluster(glues[1])];
					if clusters[0] != clusters[1]
					{
						return clusters[0] > clusters[1];
					}

					let acts: [f32; 2] = [self.ClsGetActivity(clsInd1), self.ClsGetActivity(clsInd2)];
					acts[0] < acts[1] || (acts[0] == acts[1] && glues[0] > glues[1])
				}));
			}

			let mut iLastExcl: usize = ((self.ClsDeletionTrigger() - undeletableButNotTouched as u64) as f32 * self.m_ClsDelInfo.m_FracToDelete) as usize;
			if iLastExcl > learnts.size()
			{
				iLastExcl = learnts.size();
			}

			if self.m_Params.m_ParamClsDelStrategy.get() == 1
			{
				self.m_ClsDelInfo.m_TriggerNext = self.ClsDeletionTrigger() + self.m_ClsDelInfo.m_TriggerInc;
				let nextTriggerIncD: f64 = self.m_ClsDelInfo.m_TriggerInc as f64 * self.m_ClsDelInfo.m_TriggerMult;
				if nextTriggerIncD >= self.m_ClsDelInfo.m_TriggerMax as f64
				{
					self.m_ClsDelInfo.m_TriggerInc = self.m_ClsDelInfo.m_TriggerMax;
				}
				else
				{
					self.m_ClsDelInfo.m_TriggerInc = nextTriggerIncD as u64;
				}
			}
			else
			{
				let nextChange: f32 = ((self.m_Stat.m_Conflicts / *self.m_ClsDelInfo.m_CurrChange()) as f64 + 1.0) as f32;
				*self.m_ClsDelInfo.m_CurrChange() += self.m_ClsDelInfo.m_TriggerInc;
				if self.ClsGetGlue(learnts[iLastExcl >> 1]) <= self.m_Params.m_ParamClsDelS2LowGlue.get() as TUV
				{
					*self.m_ClsDelInfo.m_CurrChange() += self.m_Params.m_ParamClsDelS2LowMediumIncValue.get() as u64;
				}

				if self.ClsGetGlue(learnts.back()) <= self.m_Params.m_ParamClsDelS2MediumGlue.get() as TUV
				{
					*self.m_ClsDelInfo.m_CurrChange() += self.m_Params.m_ParamClsDelS2LowMediumIncValue.get() as u64;
				}

				let triggerNext: f64 = nextChange as f64 * *self.m_ClsDelInfo.m_CurrChange() as f64;
				if triggerNext > u64::MAX as f64
				{
					self.m_ClsDelInfo.m_TriggerNext = u64::MAX;
				}
				else
				{
					self.m_ClsDelInfo.m_TriggerNext = triggerNext as u64;
				}
			}

			// Now mark bad learnt clauses as deleted

			for i in 0..iLastExcl
			{
				let clsInd = learnts[i];
				debug_assert!(!self.ClsChunkDeleted(clsInd));
				debug_assert!(self.ClsGetIsLearnt(clsInd));

				self.DeleteCls(clsInd, None);
				if !Compress
				{
					if self.m_FirstLearntClsInd == self.ClsEnd(clsInd)
					{
						self.m_FirstLearntClsInd = self.ClsEnd(clsInd);
						while self.m_FirstLearntClsInd < self.m_BNext && (self.ClsChunkDeleted(self.m_FirstLearntClsInd) || !self.ClsGetIsLearnt(self.m_FirstLearntClsInd))
						{
							self.m_FirstLearntClsInd = self.ClsEnd(self.m_FirstLearntClsInd);
						}
						debug_assert!(self.m_FirstLearntClsInd >= self.m_BNext || self.ClsGetIsLearnt(self.m_FirstLearntClsInd));
					}
				}
			}
		}

		// onExit
		{
			self.m_Stat.m_ClssDel += 1;

			self.m_ClsDelInfo.m_ConfsPrev = self.m_Stat.m_Conflicts;

			if self.m_FirstLearntClsInd != Ind::MAX && self.m_FirstLearntClsInd >= self.m_BNext
			{
				self.m_FirstLearntClsInd = Ind::MAX;
			}

			debug_assert!(self.NV(1) || self.P("Clause deletion finished for priority\n"));
			debug_assert!(self.NV(2) || self.P(&("The trail: ".to_string() + &self.STrail() + "\n")));

			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.TrailAssertConsistency());
			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(true));
			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.DebugAssertWaste());
		}
	}

	// The RetSiftedLit lambda of SimplifyIfRequired
	#[inline(always)]
	fn SimplifyRetSiftedLit(&self, newLastExistingVar: TUVar, l: TULit) -> TULit
	{
		let v = GetVar(l);
		if v > newLastExistingVar
		{
			if IsNeg(l) { Negate(self.m_HandyLitsClearBefore[0][v as usize]) } else { self.m_HandyLitsClearBefore[0][v as usize] }
		}
		else
		{
			l
		}
	}

	// The RemoveSatisfiedLitsFromBinWatches lambda of SimplifyIfRequired
	fn SimplifyRemoveSatisfiedLitsFromBinWatches(&mut self, l: TULit)
	{
		debug_assert!(!self.IsGloballyAssignedVar(GetVar(l)));

		let wi = self.m_Watches[l as usize];
		let binWatches = TSpanTULit::new(unsafe { self.m_W.get_ptr(wi.m_WBInd.as_usize()).add(wi.GetLongEntries()) }, wi.m_BinaryWatches.as_usize());

		let itEndRemaining = remove_if(binWatches, 0, binWatches.size(), |otherLit| self.IsGloballySatisfied(otherLit));
		debug_assert!(binWatches.size() - itEndRemaining != 0);
		self.m_Watches[l as usize].m_BinaryWatches = Ind::from_usize(itEndRemaining);
	}

	// The HandleBinaryParents lambda of SimplifyIfRequired
	fn SimplifyHandleBinaryParents(&mut self, newLastExistingVar: TUVar)
	{
		if self.m_DecLevel > 0
		{
			let mut v = self.GetTrailNextVar(self.m_TrailLastVarPerDecLevel[0]);
			while v != BadUVar
			{
				debug_assert!(self.m_AssignmentInfo[v as usize].m_IsAssigned());
				if self.m_AssignmentInfo[v as usize].m_IsAssignedInBinary()
				{
					let siftedLit = self.SimplifyRetSiftedLit(newLastExistingVar, self.m_VarInfo[v as usize].m_BinOtherLit());
					self.m_VarInfo[v as usize].set_m_BinOtherLit(siftedLit);
				}
				else if self.m_VarInfo[v as usize].m_ParentClsInd != Self::BadClsInd && self.ClsChunkDeleted(self.m_VarInfo[v as usize].m_ParentClsInd)
				{
					self.m_AssignmentInfo[v as usize].set_m_IsAssignedInBinary(true);
					debug_assert!(self.ClsGetSize(self.m_VarInfo[v as usize].m_ParentClsInd) >= 3);
					let cls = self.ConstClsSpan(self.m_VarInfo[v as usize].m_ParentClsInd, 3);
					debug_assert!(self.NV(2) || self.P(&("Var; parent cls: ".to_string() + &v.to_string() + " " + &HexStrInd(self.m_VarInfo[v as usize].m_ParentClsInd) + ": " + &self.SLits(self.Cls(self.m_VarInfo[v as usize].m_ParentClsInd), false) + "\n")));
					if Compress
					{
						debug_assert!(cls.get(0) == BadULit);
						debug_assert!(GetVar(cls.get(1)) == v || GetVar(cls.get(2)) == v);
						let binOtherLit = if GetVar(cls.get(1)) == v { cls.get(2) } else { cls.get(1) };
						self.m_VarInfo[v as usize].set_m_BinOtherLit(binOtherLit);
					}
					else
					{
						debug_assert!(cls.get(1) == BadULit);
						debug_assert!(GetVar(cls.get(0)) == v || GetVar(cls.get(2)) == v);
						let binOtherLit = if GetVar(cls.get(0)) == v { cls.get(2) } else { cls.get(0) };
						self.m_VarInfo[v as usize].set_m_BinOtherLit(binOtherLit);
					}
				}
				v = self.GetTrailNextVar(v);
			}
		}
	}

	// The SiftLitsInWatches lambda of SimplifyIfRequired
	fn SimplifySiftLitsInWatches(&mut self, newLastExistingVar: TUVar, l: TULit)
	{
		let wi = self.m_Watches[l as usize];
		let binWatches = TSpanTULit::new(unsafe { self.m_W.get_ptr(wi.m_WBInd.as_usize()).add(wi.GetLongEntries()) }, wi.m_BinaryWatches.as_usize());
		for i in 0..binWatches.size()
		{
			binWatches.set(i, self.SimplifyRetSiftedLit(newLastExistingVar, binWatches.get(i)));
		}

		let mut currLongWatchInd: usize = 0;
		let mut currLongWatchPtr: *mut TULit = self.m_W.get_ptr(wi.m_WBInd.as_usize());
		while currLongWatchInd < wi.m_LongWatches.as_usize()
		{
			unsafe
			{
				let cachedLit = *currLongWatchPtr;
				*currLongWatchPtr = self.SimplifyRetSiftedLit(newLastExistingVar, cachedLit);
				currLongWatchInd += 1;
				currLongWatchPtr = currLongWatchPtr.add(TWatchInfo::<Ind>::BinsInLong);
			}
		}
	}

	pub(crate) fn SimplifyIfRequired(&mut self)
	{
		if self.m_DecLevel > self.m_DecLevelOfLastAssignedAssumption || self.m_TrailLastVarPerDecLevel[0] == self.m_LastGloballySatisfiedLitAfterSimplify || self.m_ImplicationsTillNextSimplify > 0 || self.IsUnrecoverable() || self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT
		{
			return;
		}

		debug_assert!(self.NV(1) || self.P("Simplification started\n"));
		debug_assert!(self.NV(2) || self.P(&("The trail: ".to_string() + &self.STrail() + "\n")));

		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.TrailAssertConsistency());
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(true));
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.DebugAssertWaste());

		// The body of the function; the on-exit code follows the block
		'body:
		{
			// We assume that there is no conflict and no unit propagation now

			// We sift variable indices, if there are at least two globally assigned variables
			// We need to always leave one globally assigned variable to be able to map external globally assigned variables to it
			let siftVarIndices = self.m_TrailLastVarPerDecLevel[0] != BadUVar && self.GetTrailPrevVar(self.m_TrailLastVarPerDecLevel[0]) != BadUVar;
			let mut newLastExistingVar: TUVar = self.m_LastExistingVar;
			let mut globallySatifiedVarLowestIndex: TUVar = BadUVar;

			if siftVarIndices
			{
				// Visited will contain all the globally assigned variables
				let mut currV = self.m_TrailLastVarPerDecLevel[0];
				while currV != BadUVar
				{
					self.MarkVisitedVar(currV);
					currV = self.GetTrailPrevVar(currV);
				}

				// Sorting the visited, so that m_VisitedVars.back() would have the smallest globaly assigned variable index
				let vvSpan = self.m_VisitedVars.get_span();
				vvSpan.as_mut_slice().sort_unstable_by(|a, b| b.cmp(a));

				globallySatifiedVarLowestIndex = self.VisitedPopBack();
				debug_assert!(self.m_VisitedVars.size() > 0 && globallySatifiedVarLowestIndex < self.m_VisitedVars.back());
				let globallySatifiedLitLowestIndex: TULit = self.GetAssignedLitForVar(globallySatifiedVarLowestIndex);

				// m_HandyLitsClearBefore[0] will hold the new variable indices for the sifted and removed (highest) variable indices
				self.m_HandyLitsClearBefore[0].memset(BadULit as u8);

				let GetGlobalSatLit = |this: &Self, v: TUVar| -> TULit
				{
					let l = GetLit(v, false);
					if this.IsGloballySatisfied(l) { globallySatifiedLitLowestIndex } else { Negate(globallySatifiedLitLowestIndex) }
				};
				let globalSatLit = GetGlobalSatLit(self, globallySatifiedVarLowestIndex);
				self.m_HandyLitsClearBefore[0][globallySatifiedVarLowestIndex as usize] = globalSatLit;

				newLastExistingVar = self.m_LastExistingVar;
				while (!self.m_VisitedVars.empty() && self.m_VisitedVars.back() < newLastExistingVar)
					|| (self.IsGloballyAssignedVar(newLastExistingVar) && self.m_HandyLitsClearBefore[0][newLastExistingVar as usize] != GetGlobalSatLit(self, newLastExistingVar))
				{
					if self.IsGloballyAssignedVar(newLastExistingVar)
					{
						let globalSatLit = GetGlobalSatLit(self, newLastExistingVar);
						self.m_HandyLitsClearBefore[0][newLastExistingVar as usize] = globalSatLit;
						debug_assert!(self.NV(1) || self.P(&("\tGlobal variable removed: ".to_string() + &self.SVar(newLastExistingVar) + " (mapped to " + &self.SLit(self.m_HandyLitsClearBefore[0][newLastExistingVar as usize]) + ")\n")));
					}
					else
					{
						let toVar = self.VisitedPopBack();
						let globalSatLit = GetGlobalSatLit(self, toVar);
						self.m_HandyLitsClearBefore[0][toVar as usize] = globalSatLit;
						debug_assert!(self.NV(1) || self.P(&("\tGlobal variable removed: ".to_string() + &self.SVar(toVar) + " (mapped to " + &self.SLit(self.m_HandyLitsClearBefore[0][toVar as usize]) + ")\n")));
						self.m_HandyLitsClearBefore[0][newLastExistingVar as usize] = GetLit(toVar, false);
						debug_assert!(self.NV(1) || self.P(&("\tVariable re-indexed to literal: ".to_string() + &self.SVar(newLastExistingVar) + " to " + &self.SLit(self.m_HandyLitsClearBefore[0][newLastExistingVar as usize]) + "\n")));
					}
					newLastExistingVar -= 1;
				}

				if self.m_CurrCustomBtStrat > 0 && self.m_BestScorePerDecLevel.cap() != 0
				{
					if self.m_Params.m_ParamSimplifyGlobalLevelScoreStrat.get() == 0
					{
						self.m_BestScorePerDecLevel[0] = self.m_VsidsHeap.get_var_score(globallySatifiedVarLowestIndex);
					}
					else if self.m_Params.m_ParamSimplifyGlobalLevelScoreStrat.get() == 1
					{
						self.m_BestScorePerDecLevel[0] = self.CalcMinDecLevelScore(0);
						self.m_VsidsHeap.set_var_score(globallySatifiedVarLowestIndex, self.m_BestScorePerDecLevel[0]);
					}
					else
					{
						debug_assert!(self.m_Params.m_ParamSimplifyGlobalLevelScoreStrat.get() == 2);
						debug_assert!(self.m_BestScorePerDecLevel[0] == self.CalcMaxDecLevelScore(0));
						self.m_VsidsHeap.set_var_score(globallySatifiedVarLowestIndex, self.m_BestScorePerDecLevel[0]);
					}
				}

				self.CleanVisited();
			}

			debug_assert!(self.NV(1) || self.P(&("Previous --> new last variable: ".to_string() + &self.m_LastExistingVar.to_string() + " --> " + &newLastExistingVar.to_string() + "\n")));

			// Go over the clause buffer
			// Delete globally falsified literals (that is, mark them for deletion for the garbage collector)
			// Delete globally satisfied clauses (that is, mark them for deletion for the garbage collector)
			// Move watches to the earliest satisfied literals for clauses satisfied by assumptions
			// Sift assumption-falsified literals towards the end of the clause in clauses, which are not satisfied
			// Sift variable indices, if required

			#[cfg(debug_assertions)]
			let mut bcClssSimplifiedAndMovedToNewBuffer: HashSet<Ind> = HashSet::new();
			let mut clsInd = self.ClsLoopFirst(false);
			while !self.ClsLoopCompleted()
			{
				'iter:
				{
					// The C++ version skips the clauses, which have just been moved to this buffer by the simplification (an unordered_map may visit their buffer later);
					// a shortened clause always moves to a buffer with a smaller hash-id, which the ascending iteration over m_BC never revisits
					#[cfg(debug_assertions)]
					debug_assert!(!Compress || !bcClssSimplifiedAndMovedToNewBuffer.contains(&clsInd));

					let mut isGloballySatisfied = false;
					let mut globallyFalsifiedLitsNum: TUV = 0;
					let mut isAssumpSatisfied = false;
					let mut assumpFalsifiedLitsNum: TUV = 0;

					if self.ClsChunkDeleted(clsInd)
					{
						if !Compress
						{
							if self.m_FirstLearntClsInd == clsInd
							{
								self.m_FirstLearntClsInd = self.ClsEnd(clsInd);
							}
						}

						debug_assert!(self.NV(2) || self.P(&("\tChunk at ".to_string() + &HexStrInd(clsInd) + " deleted!\n")));
						break 'iter;
					}

					debug_assert!(self.NV(2) || self.P(&("New clause ".to_string() + &HexStrInd(clsInd) + ": " + &self.SLits(self.Cls(clsInd), false) + "\n")));

					let mut cls = self.Cls(clsInd);
					// Binary clauses are inlined into WL's
					debug_assert!(cls.size() > 2);
					// Both the watches cannot be falsified, since there is no conflict now
					debug_assert!(!self.IsFalsified(cls.get(0)) || !self.IsFalsified(cls.get(1)));

					let mut anySiftedVars = false;

					let mut i: usize = 0;
					while i < cls.size() && !isGloballySatisfied
					{
						let currLit = cls.get(i);
						if self.IsAssigned(currLit)
						{
							let isSatisfied = self.IsSatisfied(currLit);
							let decLevel: TUV = if self.m_DecLevelOfLastAssignedAssumption == 0 { 0 } else { self.GetAssignedDecLevel(currLit) };
							if isSatisfied
							{
								if decLevel == 0
								{
									isGloballySatisfied = true;
								}
								else
								{
									isAssumpSatisfied = true;
								}
							}
							else if decLevel == 0
							{
								globallyFalsifiedLitsNum += 1;
							}
							else
							{
								assumpFalsifiedLitsNum += 1;
							}
						}

						if siftVarIndices && !anySiftedVars && GetVar(currLit) > newLastExistingVar
						{
							anySiftedVars = true;
						}
						i += 1;
					}

					if isGloballySatisfied
					{
						debug_assert!(self.NV(2) || self.P("\tGlobally satisfied -- deleting...\n"));
						self.DeleteCls(clsInd, None);
						break 'iter;
					}

					let mut isCachedSetForWatch: [bool; 2] = [false, false];

					if globallyFalsifiedLitsNum > 0
					{
						debug_assert!(self.NV(2) || self.P(&("\t".to_string() + &globallyFalsifiedLitsNum.to_string() + " globally falsified literals -- removing them from the clause...\n")));
						debug_assert!((globallyFalsifiedLitsNum as usize) < cls.size());
						// Having all literals but one globally falsified is impossible at this point,
						// since then the remaining literal must have been a globally satisfied one (since BCP guarantees no delayed implications at the end),
						// which would have made the clause deleted and the loop continued before reaching this point
						debug_assert!(cls.size() - globallyFalsifiedLitsNum as usize >= 2);

						if globallyFalsifiedLitsNum as usize + 2 == cls.size()
						{
							// If all the literals but two are globally falsified,
							// The clause will now become binary, so we add a new binary clause and delete this one
							let mut it1 = 0;
							while it1 != cls.size() && self.IsGloballyFalsified(cls.get(it1))
							{
								it1 += 1;
							}
							debug_assert!(it1 != cls.size());
							let mut it2 = it1 + 1;
							while it2 != cls.size() && self.IsGloballyFalsified(cls.get(it2))
							{
								it2 += 1;
							}
							debug_assert!(it2 != cls.size());
							debug_assert!((it2 + 1..cls.size()).all(|i| self.IsGloballyFalsified(cls.get(i))));

							let mut newBinCls: [TULit; 2] = [cls.get(it1), cls.get(it2)];
							let newBinClsSifted: [TULit; 2] = [self.SimplifyRetSiftedLit(newLastExistingVar, newBinCls[0]), self.SimplifyRetSiftedLit(newLastExistingVar, newBinCls[1])];

							self.DeleteCls(clsInd, Some(&newBinClsSifted));
							// The 2nd parameter doesn't matter since the clause is binary
							self.AddClsToBufferAndWatch(Span::from_slice(&mut newBinCls), true, true);
							debug_assert!(self.NV(2) || self.P(&("\tThe clause has become binary: ".to_string() + &self.SLits(Span::from_slice(&mut newBinCls), false) + "\n")));
							if self.IsUnrecoverable()
							{
								break 'body;
							}

							break 'iter;
						}

						// Now we need to remove the globally falsified literals and let the clause be

						// If the globally falsified literal is one of the watches, swap it
						for currWatchI in 0..=1usize
						{
							if self.IsGloballyFalsified(cls.get(currWatchI))
							{
								debug_assert!(self.IsSatisfied(cls.get(1 - currWatchI)));
								let it = self.FindBestWLCand(&mut cls, self.m_DecLevelOfLastAssignedAssumption);
								self.SwapWatch(clsInd, currWatchI != 0, it);
								isCachedSetForWatch[currWatchI] = true;
								break;
							}
						}

						// Set the cached literals for watches unset by the previous loop to the 2nd watch (they might be incorrect, because some literals have been removed)
						for currWatchI in 0..=1usize
						{
							if !isCachedSetForWatch[currWatchI]
							{
								self.WLSetCached(cls.get(currWatchI), clsInd, cls.get(1 - currWatchI));
								isCachedSetForWatch[currWatchI] = true;
							}
						}

						// remove_if makes sure globally falsified literals are no longer part of the beginning of the clause, which is exactly what we need!
						let _itEndRemaining = remove_if(cls, 2, cls.size(), |l| self.IsGloballyFalsified(l));

						if !Compress
						{
							// Resizing our clause
							self.ClsSetSize(clsInd, cls.size() as TUV - globallyFalsifiedLitsNum);
							// Renewing the span
							self.ClsUpdate(&mut cls);
							debug_assert!(self.NV(2) || self.P(&("\tAfter removing the globally falsified literals: ".to_string() + &self.SLits(cls, false) + "\n")));

							// Creating a deleted chunk out of the removed literals
							let deletedChunkInd = self.ClsEnd(clsInd);
							let deletedChunkSize = globallyFalsifiedLitsNum - 1;
							self.m_B[deletedChunkInd.as_usize()] = deletedChunkSize;
							debug_assert!(self.ClsGetSize(deletedChunkInd) == deletedChunkSize);
							debug_assert!(!self.ClsGetIsLearnt(deletedChunkInd));
							if deletedChunkSize > 2
							{
								self.m_B[deletedChunkInd.as_usize() + 2] = BadULit;
							}
							// Updating the number of literal in long clauses
							self.RecordDeletedLitsFromCls(globallyFalsifiedLitsNum, 0);
						}
						else
						{
							// Updating the number of literal in long clauses
							self.RecordDeletedLitsFromCls(globallyFalsifiedLitsNum, TBCInd::from_ind(clsInd).GetHashId().m_BitsForLit() as u16);

							let oldClsInd = clsInd;
							let (deletionHandled, _spareUsed) = self.BCDeleteLitsByMovingToOtherBufferIfRequiredAssumingLastDeleted(&mut clsInd, cls.size() as TUV, globallyFalsifiedLitsNum, true);
							if !deletionHandled
							{
								for it in (cls.size() - globallyFalsifiedLitsNum as usize)..cls.size()
								{
									cls.set(it, BadULit);
								}
								debug_assert!(self.NV(2) || self.P(&("\tCompressed: after removing the globally falsified literals: ".to_string() + &self.SLits(cls, false) + "\n")));
								self.ClsSetSize(clsInd, cls.size() as TUV - globallyFalsifiedLitsNum);
								self.ClsUpdate(&mut cls);
							}
							else
							{
								// The spare buffer is never used, since m_BC is a direct-index table
								cls = self.Cls(clsInd);

								#[cfg(debug_assertions)]
								bcClssSimplifiedAndMovedToNewBuffer.insert(clsInd);
								for currWatchI in 0..=1usize
								{
									let l = cls.get(currWatchI);
									let v = GetVar(l);
									if self.m_VarInfo[v as usize].m_ParentClsInd == oldClsInd
									{
										self.m_VarInfo[v as usize].m_ParentClsInd = clsInd;
									}
								}
							}
						}
					}

					if isAssumpSatisfied
					{
						// Make cls[0] contain the lowest satisfied literal
						{
							let lowestSatisfiedIt = self.GetSatisfiedLitLowestDecLevelIt(cls, 0);
							debug_assert!(self.IsSatisfied(cls.get(lowestSatisfiedIt)));
							if lowestSatisfiedIt != 0
							{
								if lowestSatisfiedIt == 1
								{
									cls.swap(0, 1);
								}
								else
								{
									self.SwapWatch(clsInd, false, lowestSatisfiedIt);
									self.WLSetCached(cls.get(1), clsInd, cls.get(0));
									isCachedSetForWatch[0] = true;
									isCachedSetForWatch[1] = true;
								}
							}
						}

						// Make cls[1] contain the second lowest satisfied literal, if any
						{
							let lowestSatisfiedIt = self.GetSatisfiedLitLowestDecLevelIt(cls, 1);
							if self.IsSatisfied(cls.get(lowestSatisfiedIt)) && lowestSatisfiedIt != 1
							{
								self.SwapWatch(clsInd, true, lowestSatisfiedIt);
								self.WLSetCached(cls.get(0), clsInd, cls.get(1));
								isCachedSetForWatch[0] = true;
								isCachedSetForWatch[1] = true;
							}
						}

						debug_assert!(self.NV(2) || self.P(&("\tSatisfied by assumption propagation; the new clause: ".to_string() + &self.SLits(cls, false) + "\n")));
					}

					// If there are some assumption-falsified literals and the clause is not assumption-satisfied,
					// we sift the assumption-falsified literals towards the end.
					// Note that if the clause is assumption-satisfied, the rest of the clause won't be visited anyway (with the current assumptions), so we pass in that case
					if assumpFalsifiedLitsNum > 0 && !self.IsSatisfied(cls.get(0)) && !self.IsSatisfied(cls.get(1))
					{
						// remove_if wouldn't work, since it doesn't move the removed elements towards the end of the span
						// so we created move_it, which is identical to remove_it, expect that the  removed elements are moved towards the end of the span,
						// but their order is not preserved. The order of the remaining elements is still preserved
						let _itEndRemaining = move_if(cls, 2, cls.size(), |l| self.IsFalsified(l));
						debug_assert!(cls.size() - _itEndRemaining == assumpFalsifiedLitsNum as usize);
						debug_assert!(self.NV(2) || self.P(&("\tSome literals falsified by assumption propagation; the new clause: ".to_string() + &self.SLits(cls, false) + "\n")));
					}

					if siftVarIndices && anySiftedVars
					{
						for currWatchI in 0..=1usize
						{
							let cw = currWatchI;
							if !isCachedSetForWatch[cw]
							{
								self.WLSetCached(cls.get(cw), clsInd, cls.get(1 - cw));
							}
						}

						for i in 0..cls.size()
						{
							let l = cls.get(i);
							let v = GetVar(l);
							let isNeg = IsNeg(l);
							cls.set(i, if v > newLastExistingVar { if isNeg { Negate(self.m_HandyLitsClearBefore[0][v as usize]) } else { self.m_HandyLitsClearBefore[0][v as usize] } } else { l });
						}

						debug_assert!(self.NV(2) || self.P(&("\tSifted some variable indices to: ".to_string() + &self.SLits(cls, false) + "\n")));
					}
				}
				clsInd = self.ClsLoopNext();
			}

			// (There is no spare clause buffer to move the entries from, since m_BC is a direct-index table)

			// Go over the watches: delete satisfied clauses
			// Remove the globally satisfied variables, except for one and reuse their numbers by other variables.
			// One globally satisfied variable is still required to map globally satisfied external variables into it.

			// We Mark all the relevant positive literals and Root all the relevant negative literals
			// We also remove all the watches of the globally satisfied literals (that is, the remaining binary watches, since the long ones have already been removed),
			// and mark the chunks as deleted
			let mut binClssCountOnce: usize = 0;
			let mut binClssCountTwice: usize = 0;
			let mut currV = self.m_TrailLastVarPerDecLevel[0];
			while currV != BadUVar
			{
				let currL = self.GetAssignedLitForVar(currV);
				let mut wi = self.m_Watches[currL as usize];
				if !wi.IsEmpty()
				{
					// There should be no long watches for the globally satisfied literal
					debug_assert!(wi.m_LongWatches == Ind::ZERO);
					let binWatches = TSpanTULit::new(unsafe { self.m_W.get_ptr(wi.m_WBInd.as_usize()).add(wi.GetLongEntries()) }, wi.m_BinaryWatches.as_usize());
					for secondLit in binWatches
					{
						if !self.IsGloballyAssignedVar(GetVar(secondLit))
						{
							if IsNeg(secondLit)
							{
								self.MarkRooted(secondLit);
							}
							else
							{
								self.MarkVisited(secondLit);
							}
							binClssCountOnce += 1;
						}
						else
						{
							binClssCountTwice += 1;
						}
					}

					// Removing all the watches (including binary watches) of the globally satisfied literal
					self.MarkWatchBufferChunkDeleted(&mut wi);
					self.m_Watches[currL as usize].m_AllocatedEntries = Ind::ZERO;
					self.m_Watches[currL as usize].m_BinaryWatches = Ind::ZERO;
				}

				// Removing all the watches (including binary watches) of the globally falsified literal
				let mut wiNeg = self.m_Watches[Negate(currL) as usize];
				if !wiNeg.IsEmpty()
				{
					self.MarkWatchBufferChunkDeleted(&mut wiNeg);
					debug_assert!(wiNeg.m_LongWatches == Ind::ZERO);
					binClssCountTwice += wiNeg.m_BinaryWatches.as_usize();
					self.m_Watches[Negate(currL) as usize].m_AllocatedEntries = Ind::ZERO;
					self.m_Watches[Negate(currL) as usize].m_BinaryWatches = Ind::ZERO;
				}
				currV = self.GetTrailPrevVar(currV);
			}

			debug_assert!((binClssCountTwice & 1) == 0);
			self.m_Stat.DeleteBinClauses(binClssCountOnce + binClssCountTwice / 2);

			// Now, we remove any required binary watches

			for i in 0..self.m_VisitedVars.size()
			{
				let varOfPositiveLit = self.m_VisitedVars[i];
				self.SimplifyRemoveSatisfiedLitsFromBinWatches(GetLit(varOfPositiveLit, false));
			}
			self.CleanVisited();

			for i in 0..self.m_RootedVars.size()
			{
				let varOfNegativeLit = self.m_RootedVars[i];
				self.SimplifyRemoveSatisfiedLitsFromBinWatches(GetLit(varOfNegativeLit, true));
			}
			self.CleanRooted();

			debug_assert!(self.NV(1) || self.P("Removed globally satisfied literals from binary\n"));

			// Now we want to sift the variable indices (to the removed-by-now globally assigned variables), if required
			// One globally assigned variable will remain in any case

			if !siftVarIndices
			{
				// Still need to HandleBinaryParents, since some non-binary parents could have been turned binary
				self.SimplifyHandleBinaryParents(newLastExistingVar);
				debug_assert!(self.NV(1) || self.P("No need to sift indices, exiting...\n"));
				break 'body;
			}

			if self.m_Assumps.cap() > 0
			{
				debug_assert!(self.m_Assumps.get_span_cap(0).iter().all(|assumpLit| self.IsSatisfied(assumpLit)));
				let mut assumpI: usize = 0;
				while assumpI < self.m_Assumps.cap()
				{
					let lAssump = self.m_Assumps[assumpI];
					debug_assert!(lAssump != BadULit);

					if self.IsGloballyAssigned(lAssump)
					{
						debug_assert!(self.IsSatisfied(lAssump));
						// RemoveAssump
						if self.m_Params.m_ParamAssumpsSimpAllowReorder.get()
						{
							let lastAssump = self.m_Assumps[self.m_Assumps.cap() - 1];
							self.m_Assumps[assumpI] = lastAssump;
							assumpI = assumpI.wrapping_sub(1);
							let newCap = self.m_Assumps.cap() - 1;
							self.m_Assumps.reserve_exactly(newCap);
						}
						else
						{
							self.m_Assumps[assumpI] = BadULit;
						}
					}
					assumpI = assumpI.wrapping_add(1);
				}

				if !self.m_Params.m_ParamAssumpsSimpAllowReorder.get()
				{
					self.m_Assumps.remove_if_equal_and_cut_capacity(BadULit);
				}

				if self.IsAssumpVar(globallySatifiedVarLowestIndex)
				{
					self.m_AssignmentInfo[globallySatifiedVarLowestIndex as usize].set_m_IsAssump(false);
				}
			}

			// Sift the indices
			debug_assert!(self.m_HandyLitsClearBefore[0].cap() - 1 == self.m_LastExistingVar as usize);
			while self.m_LastExistingVar != newLastExistingVar
			{
				let vTo = GetVar(self.m_HandyLitsClearBefore[0][self.m_LastExistingVar as usize]);
				if vTo == globallySatifiedVarLowestIndex
				{
					if self.IsAssignedVar(self.m_LastExistingVar)
					{
						self.UnassignVar(self.m_LastExistingVar);
						self.RemoveVarAndLitData(self.m_LastExistingVar);
					}
				}
				else
				{
					self.Unassign(self.GetAssignedLitForVar(vTo));
					self.MoveVarAndLitData(self.m_LastExistingVar, vTo);
				}
				self.m_LastExistingVar -= 1;
			}

			// Mark all the variables, whose watches (binary and cached lits in long) contain the sifted indices
			let mut vBefore = newLastExistingVar + 1;
			while (vBefore as usize) < self.m_HandyLitsClearBefore[0].cap()
			{
				let vTo = GetVar(self.m_HandyLitsClearBefore[0][vBefore as usize]);
				if !self.IsGloballyAssignedVar(vTo)
				{
					for isNeg in 0..2u8
					{
						let l = GetLit(vTo, isNeg != 0);
						let wi = self.m_Watches[l as usize];
						if wi.IsEmpty()
						{
							continue;
						}
						let binWatches = TSpanTULit::new(unsafe { self.m_W.get_ptr(wi.m_WBInd.as_usize()).add(wi.GetLongEntries()) }, wi.m_BinaryWatches.as_usize());
						for secondLit in binWatches
						{
							if IsNeg(secondLit)
							{
								self.MarkRooted(secondLit);
							}
							else
							{
								self.MarkVisited(secondLit);
							}
						}
						let mut currLongWatchInd: usize = 0;
						let mut currLongWatchPtr: *mut TULit = self.m_W.get_ptr(wi.m_WBInd.as_usize());
						while currLongWatchInd < wi.m_LongWatches.as_usize()
						{
							let clsInd = unsafe { Ind::read(currLongWatchPtr.add(1)) };
							let cls = self.ConstClsSpan(clsInd, 2);
							debug_assert!(cls.get(0) == l || cls.get(1) == l);
							let secondLit = cls.get((cls.get(0) == l) as usize);
							if IsNeg(secondLit)
							{
								self.MarkRooted(secondLit);
							}
							else
							{
								self.MarkVisited(secondLit);
							}
							currLongWatchInd += 1;
							currLongWatchPtr = unsafe { currLongWatchPtr.add(TWatchInfo::<Ind>::BinsInLong) };
						}
					}
				}
				vBefore += 1;
			}

			for i in 0..self.m_VisitedVars.size()
			{
				let varOfPositiveLit = self.m_VisitedVars[i];
				let l = GetLit(varOfPositiveLit, false);
				self.SimplifySiftLitsInWatches(newLastExistingVar, l);
			}
			self.CleanVisited();

			for i in 0..self.m_RootedVars.size()
			{
				let varOfNegativeLit = self.m_RootedVars[i];
				let l = GetLit(varOfNegativeLit, true);
				self.SimplifySiftLitsInWatches(newLastExistingVar, l);
			}
			self.CleanRooted();

			// Handle the trail
			self.m_AssignmentInfo[globallySatifiedVarLowestIndex as usize].set_m_IsAssignedInBinary(false);
			self.m_VarInfo[globallySatifiedVarLowestIndex as usize].m_ParentClsInd = Self::BadClsInd;
			self.SimplifyHandleBinaryParents(newLastExistingVar);

			// Handle the assumptions
			if self.m_Assumps.cap() != 0
			{
				let assumpSpan = self.m_Assumps.get_span_cap(0);

				for i in 0..assumpSpan.size()
				{
					assumpSpan.set(i, self.SimplifyRetSiftedLit(newLastExistingVar, assumpSpan.get(i)));
				}

				// std::unique
				let mut endUniquesIt: usize = 0;
				for i in 1..assumpSpan.size()
				{
					if assumpSpan.get(endUniquesIt) != assumpSpan.get(i)
					{
						endUniquesIt += 1;
						assumpSpan.set(endUniquesIt, assumpSpan.get(i));
					}
				}
				endUniquesIt += 1;
				self.m_Assumps.reserve_exactly(endUniquesIt);

				debug_assert!(self.m_Assumps.get_span_cap(0).iter().all(|assumpLit| self.IsSatisfied(assumpLit) && self.IsAssump(assumpLit)));
			}

			debug_assert!(self.NV(2) || self.P(&("E2I before: ".to_string() + &self.SE2I() + "\n")));

			let e2iSpan = self.m_E2ILitMap.get_span_cap(0);
			for i in 0..e2iSpan.size()
			{
				// RemovedLit2NewLit
				let l = e2iSpan.get(i);
				let v = GetVar(l);
				let newLitForVar = self.m_HandyLitsClearBefore[0][v as usize];
				if newLitForVar != BadULit
				{
					e2iSpan.set(i, if IsNeg(l) { Negate(newLitForVar) } else { newLitForVar });
				}
			}

			debug_assert!(self.NV(2) || self.P(&("E2I after: ".to_string() + &self.SE2I() + "\n")));

			// This function will shrink all the variable- and literal-dependent data structures
			self.ReserveVarAndLitData(self.m_DecLevel as usize);

			self.m_Stat.UpdateMaxInternalVar(self.m_LastExistingVar as u64);

			self.m_VsidsHeap.rebuild();

			debug_assert!(self.NV(1) || self.P("Finished renaming globally satisfied literals, but one\n"));
		}

		// onExit
		{
			self.m_LastGloballySatisfiedLitAfterSimplify = self.m_TrailLastVarPerDecLevel[0];
			self.m_ImplicationsTillNextSimplify = self.m_Stat.GetActiveLongClsLen() as i64;
			self.m_Stat.m_Simplifies += 1;
			if !Compress
			{
				if self.m_FirstLearntClsInd != Ind::MAX && self.m_FirstLearntClsInd >= self.m_BNext
				{
					self.m_FirstLearntClsInd = Ind::MAX;
				}
			}

			debug_assert!(self.NV(1) || self.P("Simplification finished\n"));
			debug_assert!(self.NV(2) || self.P(&("The trail: ".to_string() + &self.STrail() + "\n")));

			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.TrailAssertConsistency());
			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(true));
			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.DebugAssertWaste());
		}
	}

	pub(crate) fn DebugAssertWaste(&mut self) -> bool
	{
		debug_assert!(self.NV(2) || self.P("DebugAssertWaste\n"));
		let mut w: Ind = Ind::ZERO;

		let mut clsInd = self.ClsLoopFirst(false);
		while !self.ClsLoopCompleted()
		{
			if self.ClsChunkDeleted(clsInd)
			{
				debug_assert!(self.NV(2) || self.P(&("Chunk ".to_string() + &HexStrInd(clsInd) + " was deleted!\n")));
				if Compress
				{
					let bcInd = TBCInd::from_ind(clsInd);
					let bcHashInd = bcInd.GetHashId();
					let ba = self.BCGetBitArrayConst(bcHashInd.val());
					if bcHashInd.m_BitsForClsSize() != 0 && ba.bit_get(bcInd.m_BitStart(), bcInd.m_BitsForClsSize()) == 0
					{
						debug_assert!(ba.bit_get(bcInd.m_BitStart(), bcInd.m_BitsForLit()) == 0);
						w += Ind::from_u32(bcInd.m_BitsForLit() as u32);
						debug_assert!(self.NV(2) || self.P(&("Lit --> w = 0x".to_string() + &HexStrInd(w) + "\n")));
					}
					else
					{
						debug_assert!(ba.bit_get(bcInd.BitFirstLit(), bcInd.m_BitsForLit()) == 0);
						w += Ind::from_u64(bcHashInd.GetFirstLitOffset() as u64 + bcHashInd.m_BitsForLit() as u64 * self.ClsGetSize(clsInd) as u64);
						debug_assert!(self.NV(2) || self.P(&("Cls --> w = 0x".to_string() + &HexStrInd(w) + "\n")));
					}
				}
				else
				{
					let sz = self.ClsGetSize(clsInd);
					w += Ind::from_u32(sz + 1);
				}
			}
			clsInd = self.ClsLoopNext();
		}

		debug_assert!(self.m_BWasted == w);
		let _ = w;

		true
	}

	pub(crate) fn CompressWLs(&mut self)
	{
		// Save aside the two initial watch entries for every literal l
		// and place log2(allocated) && l to be able to, respectively: (1) Skip to the next chunk during compression;
		// (2) Point from the watches to their updated location in the watch-buffer

		let mut firstWLEntriesPerLit: CDynArray<[TULit; 2]> = CDynArray::new_init(self.GetNextLit() as usize, 0);
		if self.IsUnrecoverable()
		{
			return;
		}
		// The C++ version doesn't test the allocation (and would crash, had it failed)
		if firstWLEntriesPerLit.uninitialized_or_erroneous()
		{
			self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CompressWLs: couldn't allocate firstWLEntriesPerLit".to_string());
			return;
		}

		let mut l = GetFirstLit();
		while l < self.GetNextLit()
		{
			let mut wi = self.m_Watches[l as usize];
			let usedEntries = wi.GetUsedEntries();

			if self.m_Params.m_ParamCompressAllocatedPerWatch.get() && wi.m_AllocatedEntries > Ind::ZERO && usedEntries == 0
			{
				self.MarkWatchBufferChunkDeleted(&mut wi);
				wi.m_AllocatedEntries = Ind::ZERO;
			}

			if wi.m_AllocatedEntries > Ind::ZERO
			{
				debug_assert!(wi.m_AllocatedEntries.as_u64().is_power_of_two());
				debug_assert!(wi.m_AllocatedEntries.as_usize() >= TWatchInfo::<Ind>::BinsInLongBitCeil);

				if self.m_Params.m_ParamCompressAllocatedPerWatch.get()
				{
					let allocatedEntriesBefore = wi.m_AllocatedEntries;
					wi.m_AllocatedEntries = Ind::from_usize(usedEntries.next_power_of_two());
					debug_assert!(wi.m_AllocatedEntries != Ind::ZERO);
					if wi.m_AllocatedEntries.as_usize() < TWatchInfo::<Ind>::BinsInLongBitCeil
					{
						wi.m_AllocatedEntries = Ind::from_usize(TWatchInfo::<Ind>::BinsInLongBitCeil);
					}

					// Marking deleted regions
					let mut nextEntry = wi.m_WBInd + wi.m_AllocatedEntries;
					let mut remainingEntriesToMark = allocatedEntriesBefore - wi.m_AllocatedEntries;
					while remainingEntriesToMark != Ind::ZERO
					{
						// bit_floor
						let bitFloor = Ind::from_u64(1u64 << (63 - remainingEntriesToMark.as_u64().leading_zeros()));
						self.MarkWatchBufferChunkDeletedOrByLiteral(nextEntry, bitFloor, BadULit);
						remainingEntriesToMark -= bitFloor;
						nextEntry = self.WLEnd(nextEntry);
					}
				}

				firstWLEntriesPerLit[l as usize][0] = self.m_W[wi.m_WBInd.as_usize()];
				firstWLEntriesPerLit[l as usize][1] = self.m_W[wi.m_WBInd.as_usize() + 1];

				self.MarkWatchBufferChunkDeletedOrByLiteral(wi.m_WBInd, wi.m_AllocatedEntries, l);
			}
			self.m_Watches[l as usize] = wi;
			l += 1;
		}

		// Remove garbage from the the watch buffer
		let mut wNext = self.m_WNext.as_usize();
		RemoveGarbage(self, |c: &mut Self| &mut c.m_W, LitsInPage, &mut wNext, &mut |c: &mut Self, wlInd: usize| c.WLChunkDeleted(Ind::from_usize(wlInd)), &mut |c: &mut Self, wlInd: usize| c.WLEnd(Ind::from_usize(wlInd)).as_usize(),
			Some(&mut |c: &mut Self, oldWlInd: usize, newWlInd: usize|
			{
				debug_assert!(newWlInd >= LitsInPage);
				let l = c.m_W[oldWlInd + 1];
				c.m_W[oldWlInd] = firstWLEntriesPerLit[l as usize][0];
				c.m_W[oldWlInd + 1] = firstWLEntriesPerLit[l as usize][1];

				c.m_Watches[l as usize].m_WBInd = Ind::from_usize(newWlInd);
			}));
		self.m_WNext = Ind::from_usize(wNext);

		if self.m_W.cap() > (self.m_WNext.as_u64() as f64 * self.m_Params.m_ParamMultWatches.get()) as usize
		{
			self.m_W.reserve_exactly((self.m_WNext.as_u64() as f64 * self.m_Params.m_ParamMultWatches.get()) as usize);
			// The C++ version tests IsUnrecoverable() only (and would crash later, had the reallocation failed)
			if self.m_W.uninitialized_or_erroneous()
			{
				self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CompressWLs: couldn't reallocate m_W".to_string());
			}
			if self.IsUnrecoverable()
			{
				return;
			}
		}
		self.m_WWasted = Ind::ZERO;
	}

	// The NotifyAboutRemainingChunkMove lambda of CompressBuffersIfRequired
	fn CompressNotifyAboutRemainingChunkMove(&mut self, isAboveGlobalDecLevel: bool, oldInd: Ind, newInd: Ind)
	{
		if isAboveGlobalDecLevel
		{
			let cls = self.ConstClsSpan(oldInd, 2);
			for i in 0..2
			{
				// UpdateParentIfRequired
				let l = cls.get(i);
				let v = GetVar(l);
				let isCurrClsParent = self.IsAssignedVar(v) && !self.m_AssignmentInfo[v as usize].IsAssignedBinary() && self.m_VarInfo[v as usize].m_ParentClsInd == oldInd;
				if isCurrClsParent
				{
					self.m_VarInfo[v as usize].m_ParentClsInd = newInd;
				}
			}
		}

		if !Compress
		{
			if self.m_FirstLearntClsInd == oldInd
			{
				self.m_FirstLearntClsInd = newInd;
			}
		}
	}

	// The AddLongWatchLocal lambda of CompressBuffersIfRequired
	fn CompressAddLongWatchLocal(&mut self, watchInd: bool, clsInd: Ind)
	{
		let cls = self.ConstClsSpan(clsInd, 2);
		let currLit = cls.get(watchInd as usize);
		let mut inlinedLit = cls.get(!watchInd as usize);
		if self.IsFalsified(currLit) && !(self.IsSatisfied(inlinedLit) && self.GetAssignedDecLevel(inlinedLit) <= self.GetAssignedDecLevel(currLit))
		{
			let clsFull = self.ConstClsSpan(clsInd, TUV::MAX);
			let currLitDecLevel = self.GetAssignedDecLevel(currLit);
			let mut visitedLitIt = 2;
			while visitedLitIt != clsFull.size() && !(self.IsSatisfied(clsFull.get(visitedLitIt)) && self.GetAssignedDecLevel(clsFull.get(visitedLitIt)) <= currLitDecLevel)
			{
				visitedLitIt += 1;
			}
			debug_assert!(visitedLitIt != clsFull.size());
			inlinedLit = clsFull.get(visitedLitIt);
		}

		let wi = self.m_Watches[currLit as usize];
		let watchArena: *mut TULit = self.m_W.get_ptr(wi.m_WBInd.as_usize());
		let mut indBeyondLongWatches = wi.GetLongEntries();
		unsafe
		{
			*watchArena.add(indBeyondLongWatches) = inlinedLit;
			indBeyondLongWatches += 1;
			clsInd.write(watchArena.add(indBeyondLongWatches));
		}
		self.m_Watches[currLit as usize].m_LongWatches += Ind::ONE;
	}

	pub(crate) fn CompressBuffersIfRequired(&mut self)
	{
		let nextBitOverall: f64 = if Compress { self.BCNextBitSum() as f64 } else { self.m_BNext.as_u64() as f64 };
		if self.m_BWasted.as_u64() as f64 / nextBitOverall <= self.m_Params.m_ParamWastedFractionThrToDelete.get() || self.IsUnrecoverable() || self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT
		{
			return;
		}

		debug_assert!(self.NV(1) || self.P(&("Compression started: wasted fraction is ".to_string() + &format!("{:.6}", self.m_BWasted.as_u64() as f64 / nextBitOverall) + " > " + &format!("{:.6}", self.m_Params.m_ParamWastedFractionThrToDelete.get()) + "\n")));
		debug_assert!(self.NV(2) || self.P(&("The trail: ".to_string() + &self.STrail() + "\n")));

		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.TrailAssertConsistency());
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(true));
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.DebugAssertWaste());

		// ************************************
		// Compress the clause buffer
		// ************************************

		let isAboveGlobalDecLevel = self.m_DecLevel != 0;

		if !Compress
		{
			let nextBefore = self.m_BNext;

			let mut bNext = self.m_BNext.as_usize();
			RemoveGarbage(self, |c: &mut Self| &mut c.m_B, LitsInPage, &mut bNext, &mut |c: &mut Self, clsInd: usize| c.ClsChunkDeleted(Ind::from_usize(clsInd)), &mut |c: &mut Self, clsInd: usize| c.ClsEnd(Ind::from_usize(clsInd)).as_usize(),
				Some(&mut |c: &mut Self, oldInd: usize, newInd: usize| c.CompressNotifyAboutRemainingChunkMove(isAboveGlobalDecLevel, Ind::from_usize(oldInd), Ind::from_usize(newInd))));
			self.m_BNext = Ind::from_usize(bNext);

			debug_assert!(nextBefore - self.m_BWasted == self.m_BNext);
			let _ = nextBefore;
			self.m_BWasted = Ind::ZERO;
			if self.m_B.cap() > (self.m_BNext.as_u64() as f64 * self.m_Params.m_ParamMultClss.get()) as usize
			{
				self.m_B.reserve_exactly((self.m_BNext.as_u64() as f64 * self.m_Params.m_ParamMultClss.get()) as usize);
				// The C++ version tests IsUnrecoverable() only (and would crash later, had the reallocation failed)
				if self.m_B.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CompressBuffersIfRequired: couldn't reallocate m_B".to_string());
				}
				if self.IsUnrecoverable()
				{
					return;
				}
			}
			if self.m_FirstLearntClsInd != Ind::MAX && self.m_FirstLearntClsInd >= self.m_BNext
			{
				self.m_FirstLearntClsInd = Ind::MAX;
			}
		}
		else
		{
			self.BCRemoveGarbage(Some(&mut |c: &mut Self, oldInd: u64, newInd: u64| c.CompressNotifyAboutRemainingChunkMove(isAboveGlobalDecLevel, Ind::from_u64(oldInd), Ind::from_u64(newInd))));
			self.m_BWasted = Ind::ZERO;
		}

		// ************************************
		// Update and compress the watch buffer
		// ************************************

		// Update all the long watches with new locations

		// Start with initializing m_LongWatches with 0 for every literal
		// The counter is expected to go back to its value after the next loop
		let mut l = GetFirstLit();
		while l < self.GetNextLit()
		{
			self.m_Watches[l as usize].m_LongWatches = Ind::ZERO;
			l += 1;
		}

		// Will now update all the clause indices

		debug_assert!(self.NV(2) || self.P("Going over all the clauses after compression to repopulate the watch lists\n"));
		let mut clsInd = self.ClsLoopFirst(false);
		while !self.ClsLoopCompleted()
		{
			debug_assert!(!self.ClsChunkDeleted(clsInd));
			// Note that we cannot because of correctness (and also should not because of efficiency) use the standard
			// WLAddLongWatch procedure, since the number of long watches doesn't change
			self.CompressAddLongWatchLocal(false, clsInd);
			self.CompressAddLongWatchLocal(true, clsInd);
			clsInd = self.ClsLoopNext();
		}

		self.CompressWLs();

		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 1 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.TrailAssertConsistency());
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(true));
		debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.DebugAssertWaste());

		debug_assert!(self.NV(1) || self.P("Compression finished\n"));
		debug_assert!(self.NV(2) || self.P(&("The trail: ".to_string() + &self.STrail() + "\n")));
	}
}
