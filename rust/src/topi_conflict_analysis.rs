// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use std::collections::BinaryHeap;
use std::io::Write;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn AddClsToBufferAndWatch(&mut self, cls: TSpanTULit, isLearntNotForDeletion: bool, isPartOfProof: bool) -> Ind
	{
		if isPartOfProof && self.IsCbLearntOrDrat()
		{
			self.NewLearntClsApplyCbLearntDrat(cls);
		}

		let mut clsStart = Self::BadClsInd;

		if cls.size() == 2
		{
			if self.m_Params.m_ParamExistingBinWLStrat.get() >= 2 || !self.WLBinaryWatchExists(cls[0], cls[1])
			{
				self.m_Stat.NewClause(cls.size(), isLearntNotForDeletion);
				self.WLAddBinaryWatch(cls[0], cls[1]);
				if self.IsUnrecoverable()
				{
					return clsStart;
				}
				self.WLAddBinaryWatch(cls[1], cls[0]);
				if self.IsUnrecoverable()
				{
					return clsStart;
				}
			}
			else
			{
				debug_assert!(self.WLBinaryWatchExists(cls[1], cls[0]));
				if self.m_Params.m_ParamExistingBinWLStrat.get() == 1
				{
					self.UpdateScoreVar(GetVar(cls[0]), self.m_Params.m_ParamBinWLScoreBoostFactor.get());
					self.UpdateScoreVar(GetVar(cls[1]), self.m_Params.m_ParamBinWLScoreBoostFactor.get());
				}
			}
		}
		else if cls.size() > 2
		{
			self.m_Stat.NewClause(cls.size(), isLearntNotForDeletion);
			// Long clause

			self.WLAddLongWatch(cls[0], cls[1], Self::BadClsInd);
			if self.IsUnrecoverable()
			{
				return clsStart;
			}

			self.WLAddLongWatch(cls[1], cls[0], Self::BadClsInd);
			if self.IsUnrecoverable()
			{
				return clsStart;
			}

			// PointFromWatches
			macro_rules! PointFromWatches
			{
				($clsInd:expr) =>
				{{
					let clsInd: Ind = $clsInd;
					let clsIndPtrs: [usize; 2] = [self.LastWLEntry(cls[0]), self.LastWLEntry(cls[1])];
					unsafe
					{
						clsInd.write(self.m_W.get_ptr(clsIndPtrs[1]));
						clsInd.write(self.m_W.get_ptr(clsIndPtrs[0]));
					}
					clsInd
				}};
			}

			if !Compress
			{
				let isOversized = isLearntNotForDeletion && cls.size() > ClsLearntMaxSizeWithGlue as usize;

				let newBNext: Ind = self.m_BNext + Ind::from_usize(cls.size()) + Ind::from_u32(self.EClsLitsStartOffset(isLearntNotForDeletion, isOversized));

				if newBNext < self.m_BNext
				{
					self.SetStatus(TToporStatus::STATUS_INDEX_TOO_NARROW, "AddClsToBufferAndWatch: too many literals in all the clauses combined".to_string());
					return clsStart;
				}

				if newBNext.as_usize() >= self.m_B.cap()
				{
					self.m_B.reserve_atleast(newBNext.as_usize());
					if self.m_B.uninitialized_or_erroneous()
					{
						self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "AddClsToBufferAndWatch: couldn't reserve the buffer".to_string());
						return clsStart;
					}
				}

				// The clause will start at m_BNext; point to it correctly from both watches
				clsStart = PointFromWatches!(self.m_BNext);
				// Order of setting the fields is important, since ClsSetSize depends on ClsSetSize and ClsSetGlue depends on both
				self.EClsSetIsLearnt(self.m_BNext, isLearntNotForDeletion);
				self.ClsSetSize(self.m_BNext, cls.size() as TUV);
				if isLearntNotForDeletion
				{
					let glue = self.GetGlueAndMarkCurrDecLevels(cls);
					self.ClsSetGlue(self.m_BNext, glue);
					if self.m_BNext < self.m_FirstLearntClsInd
					{
						self.m_FirstLearntClsInd = self.m_BNext;
					}
					if self.m_Params.m_ParamClsDelStrategy.get() > 0
					{
						self.ClsSetActivityAndSkipdelTo0(self.m_BNext);
					}
				}
				// Copy the clause from the input span
				unsafe
				{
					std::ptr::copy_nonoverlapping(cls.ptr(), self.m_B.get_ptr(self.m_BNext.as_usize() + self.EClsLitsStartOffset(isLearntNotForDeletion, isOversized) as usize), cls.size());
				}
				// Update m_BNext
				self.m_BNext = newBNext;
			}
			else
			{
				let glue = if isLearntNotForDeletion { self.GetGlueAndMarkCurrDecLevels(cls) } else { 0 };
				let bcInd = self.BCCompress(cls, isLearntNotForDeletion, glue, false, 0.0, false, None);
				clsStart = PointFromWatches!(bcInd.ind());
			}
		}

		clsStart
	}

	pub(crate) fn UpdateAllUipInfoAfterRestart(&mut self)
	{
		if self.m_Params.m_ParamAllUipMode.get() == 0 || self.m_Stat.m_Restarts <= self.m_Params.m_ParamAllUipFirstRestart.get() as u64 || (self.m_Params.m_ParamAllUipLastRestart.get() != u32::MAX && self.m_Stat.m_Restarts >= self.m_Params.m_ParamAllUipLastRestart.get() as u64)
		{
			return;
		}

		let allUipFailed = self.m_AllUipAttemptedCurrRestart > 0 && (self.m_AllUipSucceededCurrRestart as f64 / self.m_AllUipAttemptedCurrRestart as f64) < self.m_Params.m_ParamAllUipFailureThr.get();
		if allUipFailed
		{
			self.m_AllUipGap += 1;
		}
		else
		{
			self.m_AllUipGap = if self.m_AllUipGap == 0 { 0 } else { self.m_AllUipGap - 1 };
		}

		self.m_AllUipSucceededCurrRestart = 0;
		self.m_AllUipAttemptedCurrRestart = 0;
	}

	// cls is a raw pointer (the C++ reference); the callers move m_HandyLitsClearBefore[i] out of the solver first, so that cls never aliases self
	pub(crate) fn MinimizeClauseBin(&mut self, cls: *mut CVector<TULit>)
	{
		let cls = unsafe { &mut *cls };
		let clsSizeBefore = cls.size();

		debug_assert!(self.NV(2) || self.P(&("Minimize-clause-binary start: ".to_string() + &self.SLits(cls.get_span(), false) + "\n")));

		debug_assert!(self.m_RootedVars.empty());

		let wi = self.m_Watches[cls[0] as usize];
		if wi.m_BinaryWatches != Ind::ZERO
		{
			let binWatches: TSpanTULit = self.m_W.get_span_cap_sz(wi.m_WBInd.as_usize() + wi.GetLongEntries(), wi.m_BinaryWatches.as_usize());
			let mut someMarked = false;
			for l in binWatches
			{
				if self.IsSatisfied(l)
				{
					someMarked = true;
					self.MarkRooted(l);
				}
			}

			if someMarked
			{
				cls.erase_if_may_reorder(|l| self.IsRooted(l), 1);
			}
		}
		debug_assert!(self.NV(2) || self.P(&("Minimize-clause-binary finish; ".to_string() + &(if cls.size() == clsSizeBefore { "couldn't minimize".to_string() } else { "minimized and saved ".to_string() + &(clsSizeBefore - cls.size()).to_string() + " literals" }) + ": " + &self.SLits(cls.get_span(), false) + "\n")));

		// onExit
		self.CleanRooted();
	}

	// cls is a raw pointer (the C++ reference); the callers move m_HandyLitsClearBefore[i] out of the solver first, so that cls never aliases self
	pub(crate) fn GenerateAllUipClause(&mut self, cls: *mut CVector<TULit>) -> bool
	{
		let cls = unsafe { &mut *cls };
		if self.m_Params.m_ParamAllUipMode.get() == 0 || self.m_Stat.m_Restarts < self.m_Params.m_ParamAllUipFirstRestart.get() as u64
		{
			return false;
		}

		debug_assert!(self.m_RootedVars.empty());

		let initGlue = self.GetGlueAndMarkCurrDecLevels(cls.get_span());
		if cls.size() <= (self.m_AllUipGap + initGlue) as usize
		{
			debug_assert!(self.NV(2) || self.P(&("GenerateAllUipClause early exit: cls.size() <= m_AllUipGap + initGlue (".to_string() + &cls.size().to_string() + " " + &self.m_AllUipGap.to_string() + " " + &initGlue.to_string() + ")\n")));
			return false;
		}

		debug_assert!(self.NV(2) || self.P(&("GenerateAllUipClause attempt #".to_string() + &(self.m_Stat.m_AllUipAttempted + 1).to_string() + " started with the following clause " + &self.SLits(cls.get_span(), false) + "\n")));

		let mut res: CVector<TULit> = CVector::new(0, 0);
		let mut cancelAllUipClauseGeneration = false;

		for l in cls.get_span()
		{
			self.MarkRooted(l);
		}

		// The body, after which the onExit code is run on every path
		let retVal: bool = 'body:
		{
			let (initMarkedDecLevelsCounter, mut decLevels) = self.GetDecLevelsAndMarkInHugeCounter(cls.get_span());
			if self.IsUnrecoverable()
			{
				break 'body false;
			}

			macro_rules! UnvisitedNum
			{
				($decLevel:expr) =>
				{{
					let dl = $decLevel as usize;
					if self.m_HugeCounterPerDecLevel[dl] <= initMarkedDecLevelsCounter { 0 } else { self.m_HugeCounterPerDecLevel[dl] - initMarkedDecLevelsCounter }
				}};
			}

			while let Some(decLevel) = decLevels.pop()
			{
				debug_assert!(UnvisitedNum!(decLevel) >= 1);

				debug_assert!(self.NV(2) || self.P(&("Decision level ".to_string() + &decLevel.to_string() + " : started with " + &UnvisitedNum!(decLevel).to_string() + " unvisited variables\n")));

				let mut v: TUVar = self.m_TrailLastVarPerDecLevel[decLevel as usize];
				while UnvisitedNum!(decLevel) > 0
				{
					'iter:
					{
						if self.IsRootedVar(v)
						{
							debug_assert!(self.NV(2) || self.P(&("New rooted variable ".to_string() + &self.SVar(v) + "\n")));

							macro_rules! IsVPushedToRes
							{
								() => { res.size() != 0 && GetVar(res.back()) == v };
							}

							macro_rules! PushVToRes
							{
								() =>
								{{
									let l: TULit = Negate(self.GetAssignedLitForVar(v));
									debug_assert!(self.NV(2) || self.P(&("Pushing the following literal to res ".to_string() + &self.SLit(l) + "\n")));
									debug_assert!(!res.get_span().contains(l));
									res.push_back(l);
								}};
							}

							macro_rules! CancelAllUipClauseGenerationIfRequired
							{
								() =>
								{{
									if res.size() + decLevels.len() >= cls.size()
									{
										debug_assert!(self.NV(2) || self.P("Canceled AllUIP clause generation sue to exceeding size\n"));
										cancelAllUipClauseGeneration = true;
										true
									}
									else
									{
										false
									}
								}};
							}

							debug_assert!(!IsVPushedToRes!());

							self.m_HugeCounterPerDecLevel[decLevel as usize] -= 1;

							if decLevel == self.m_DecLevel || UnvisitedNum!(decLevel) == 0
							{
								// It's either the latest decision level, or
								// We reached the UIP for a certain level (in which case the for loop will exit)
								PushVToRes!();
								if CancelAllUipClauseGenerationIfRequired!()
								{
									break 'body false;
								}
								break 'iter;
							}

							let parentSpan = self.GetAssignedNonDecParentSpanVar(v);
							debug_assert!(self.NV(2) || self.P(&("\tParent ".to_string() + &self.SLits(parentSpan, false) + "\n")));
							for lParent in parentSpan
							{
								if self.IsRooted(lParent)
								{
									continue;
								}
								let lParentDecLevel: TUV = self.GetAssignedDecLevel(lParent);
								if UnvisitedNum!(lParentDecLevel) == 0
								{
									// The parent contains an unvisited level --> v will be included in the final clause as we don't want to resolve it away
									PushVToRes!();
									if CancelAllUipClauseGenerationIfRequired!()
									{
										break 'body false;
									}
									break;
								}
							}

							if !IsVPushedToRes!()
							{
								// Now we know that v can be resolved away without adding new levels
								debug_assert!(self.NV(2) || self.P(&("\tNow we know that ".to_string() + &self.SVar(v) + " can be resolved away without adding new levels\n")));
								for lParent in parentSpan
								{
									if self.IsRooted(lParent)
									{
										continue;
									}
									self.MarkRooted(lParent);
									let lParentDecLevel: TUV = self.GetAssignedDecLevel(lParent);
									self.m_HugeCounterPerDecLevel[lParentDecLevel as usize] += 1;
									if self.m_HugeCounterPerDecLevel[lParentDecLevel as usize] > self.m_HugeCounterDecLevels
									{
										self.m_HugeCounterDecLevels = self.m_HugeCounterPerDecLevel[lParentDecLevel as usize];
									}
								}
							}
						}
					}
					v = self.m_VarInfo[v as usize].m_TrailPrev;
				}
			}

			true
		};

		// onExit
		self.m_Stat.m_AllUipAttempted += 1;
		self.m_AllUipAttemptedCurrRestart += 1;

		if !cancelAllUipClauseGeneration
		{
			self.m_Stat.m_LitsRemovedByAllUip += (cls.size() - res.size()) as u64;
			cls.clear();
			cls.append(res.get_span());
			self.m_Stat.m_AllUipSucceeded += 1;
			self.m_AllUipSucceededCurrRestart += 1;
			debug_assert!(self.NV(2) || self.P(&("GenerateAllUipClause succeeded, exiting; the new clause is: ".to_string() + &self.SLits(cls.get_span(), false) + "\n")));
		}
		else
		{
			debug_assert!(self.NV(2) || self.P(&("GenerateAllUipClause failed, exiting: ".to_string() + &self.SLits(cls.get_span(), false) + "\n")));
		}

		self.CleanRooted();

		retVal
	}

	// The ProcessParentDecideIfRemovalStillPossible lambda of MinimizeClauseMinisat
	fn MinimizeClauseMinisatProcessParentDecideIfRemovalStillPossible(&mut self, currVar: TUVar, parent: TSpanTULit, canBeRemoved: &mut bool, toTestForRemoval: &mut CVector<TUVar>)
	{
		for parentLit in parent
		{
			if GetVar(parentLit) == currVar
			{
				continue;
			}

			if !self.IsAssignedMarkedDecLevel(parentLit)
			{
				*canBeRemoved = false;
				break;
			}

			if !self.IsRooted(parentLit)
			{
				if self.IsAssignedDec(parentLit)
				{
					*canBeRemoved = false;
					break;
				}
				toTestForRemoval.push_back(GetVar(parentLit));
				// The C++ version re-tests the status here, which nothing since its test on entry can change
				debug_assert!(!self.IsUnrecoverable());
			}
		}
	}

	// The erase-condition lambda of MinimizeClauseMinisat
	fn MinimizeClauseMinisatEraseCond(&mut self, l: TULit, toTestForRemoval: &mut CVector<TUVar>) -> bool
	{
		// The C++ version re-tests the status here, which nothing since its test on entry can change
		debug_assert!(!self.IsUnrecoverable());

		toTestForRemoval.clear();

		let mut canBeRemoved = true;

		debug_assert!(self.IsAssigned(l) && self.GetAssignedDecLevel(l) != 0);

		if self.IsAssignedDec(l)
		{
			// The current literal is a decision literal, so it must have a marked decision level, and we cannot remove it
			canBeRemoved = false;
			debug_assert!(self.IsAssignedMarkedDecLevel(l));
		}
		else
		{
			let parent = self.GetAssignedNonDecParentSpan(l);
			self.MinimizeClauseMinisatProcessParentDecideIfRemovalStillPossible(GetVar(l), parent, &mut canBeRemoved, toTestForRemoval);
			// The C++ version re-tests the status here, which nothing since its test on entry can change
			debug_assert!(!self.IsUnrecoverable());

			while canBeRemoved && !toTestForRemoval.empty()
			{
				let currTestedVar: TUVar = toTestForRemoval.back();

				debug_assert!(!self.IsAssignedDecVar(currTestedVar));
				debug_assert!(self.IsAssignedMarkedDecLevelVar(currTestedVar));

				// We never add a rooted var to toTestForRemoval, but a tested variable may be marked as rooted after being successfully tested for removal
				if !self.IsRootedVar(currTestedVar)
				{
					let toTestBefore = toTestForRemoval.size();
					let parent = self.GetAssignedNonDecParentSpanVar(currTestedVar);
					self.MinimizeClauseMinisatProcessParentDecideIfRemovalStillPossible(currTestedVar, parent, &mut canBeRemoved, toTestForRemoval);
					// The C++ version re-tests the status here, which nothing since its test on entry can change
					debug_assert!(!self.IsUnrecoverable());
					if canBeRemoved && toTestForRemoval.size() == toTestBefore
					{
						self.MarkRootedVar(currTestedVar);
						// This line is unnecessary for correctness, since the next iteration would've removed currTestedVar from toTestForRemoval anyway,
						// since the variable is marked as rooted, but we save an iteration by doing it here
						toTestForRemoval.pop_back();
					}
				}
				else
				{
					toTestForRemoval.pop_back();
				}
			}
		}

		canBeRemoved
	}

	// cls is a raw pointer (the C++ reference); the callers move m_HandyLitsClearBefore[i] out of the solver first, so that cls never aliases self
	pub(crate) fn MinimizeClauseMinisat(&mut self, cls: *mut CVector<TULit>)
	{
		let cls = unsafe { &mut *cls };
		let clsSizeBefore = cls.size();

		debug_assert!(self.NV(2) || self.P(&("Minimize-clause-Minisat start: ".to_string() + &self.SLits(cls.get_span(), false) + "\n")));

		debug_assert!(self.m_RootedVars.empty());

		// The body, after which the onExit code is run on every path
		'body:
		{
			if self.IsUnrecoverable()
			{
				break 'body;
			}

			// After this function, all the decision levels in cls have m_DecLevelsLastAppearenceCounter[decLevel] == m_CurrDecLevelsCounter
			self.GetGlueAndMarkCurrDecLevels(cls.get_span());

			// All the variables in clause are the root
			// The minimization algorithm in a nutshell: for every root literal, if we reach the roots only by parent chain visiting, the literal can be removed
			self.m_RootedVars.reserve_exactly(cls.size());
			// The C++ version re-tests the status here, which nothing since its test on entry can change
			debug_assert!(!self.IsUnrecoverable());

			for l in cls.get_span()
			{
				self.MarkRooted(l);
			}

			let mut toTestForRemoval: CVector<TUVar> = CVector::new(0, 0);

			cls.erase_if_may_reorder(|l| self.MinimizeClauseMinisatEraseCond(l, &mut toTestForRemoval), 0);

			debug_assert!(self.NV(2) || self.P(&("Minimize-clause-Minisat finish; ".to_string() + &(if cls.size() == clsSizeBefore { "couldn't minimize".to_string() } else { "minimized and saved ".to_string() + &(clsSizeBefore - cls.size()).to_string() + " literals" }) + ": " + &self.SLits(cls.get_span(), false) + "\n")));
		}

		// onExit
		self.CleanRooted();
	}

	pub(crate) fn SizeWithoutDecLevel0(&self, cls: TSpanTULit) -> usize
	{
		let mut sum: usize = 0;
		for l in cls
		{
			sum += (self.GetAssignedDecLevel(l) != 0) as usize;
		}
		sum
	}

	// The VisitLit lambda of LearnAndUpdateHeuristics
	#[inline(always)]
	fn LearnAndUpdateHeuristicsVisitLit(&mut self, l: TULit, varsToVisitCurrDecLevel: &mut TUV, visitedNegLitsPrevDecLevels: *mut CVector<TULit>)
	{
		let v: TUVar = GetVar(l);
		let decLevel: TUV = self.GetAssignedDecLevel(l);

		if !self.m_AssignmentInfo[v as usize].m_Visit() && decLevel != 0
		{
			self.MarkVisitedVar(v);

			if decLevel == self.m_DecLevel
			{
				*varsToVisitCurrDecLevel += 1;
			}
			else
			{
				unsafe { (*visitedNegLitsPrevDecLevels).push_back(if self.IsSatisfied(l) { Negate(l) } else { l }) };
			}
			self.UpdateScoreVar(v, if self.m_Params.m_ParamVarActivityUseMapleLevelBreaker.get() { 0.5 } else { 1.0 });
		}
	}

	// The VisitCls lambda of LearnAndUpdateHeuristics
	#[inline(always)]
	fn LearnAndUpdateHeuristicsVisitCls(&mut self, cls: TSpanTULit, longClsInd: Ind, updateClsCounter: bool, varsToVisitCurrDecLevel: &mut TUV, visitedNegLitsPrevDecLevels: *mut CVector<TULit>)
	{
		if updateClsCounter
		{
			self.m_CurrClsCounter = self.m_CurrClsCounter.wrapping_add(1);
			if self.m_CurrClsCounter < 0
			{
				self.m_CurrClsCounters.memset(0);
				self.m_CurrClsCounter = 1;
			}
		}

		for l in cls
		{
			self.LearnAndUpdateHeuristicsVisitLit(l, varsToVisitCurrDecLevel, visitedNegLitsPrevDecLevels);
			if updateClsCounter
			{
				self.m_CurrClsCounters[GetVar(l) as usize] = self.m_CurrClsCounter;
			}
		}
		if longClsInd != Self::BadClsInd && self.ClsGetIsLearnt(longClsInd)
		{
			let newGlue: TUV = self.GetGlueAndMarkCurrDecLevels(cls);
			let oldGlue: TUV = self.ClsGetGlue(longClsInd);
			if newGlue < oldGlue
			{
				self.ClsSetGlue(longClsInd, newGlue);
			}
			self.ClsDelNewLearntOrGlueUpdate(longClsInd, oldGlue);
		}

		debug_assert!(self.NV(2) || !self.IsCbLearntOrDrat() || self.P(&("Deriving external clause for conflict ".to_string() + &self.m_Stat.m_Conflicts.to_string() + ": " + &self.SLits(cls, true) + " 0\n")));
	}

	// Return: (1) asserting clause span; (2) Clause index (for long clauses)
	pub(crate) fn LearnAndUpdateHeuristics(&mut self, contradictionInfo: &mut TContradictionInfo<Ind>, clsBeforeAllUipOrEmptyIfAllUipFailed: &mut CVector<TULit>) -> (TSpanTULit, Ind)
	{
		let res = self.LearnAndUpdateHeuristicsBody(contradictionInfo, clsBeforeAllUipOrEmptyIfAllUipFailed);
		// onExit
		self.CleanVisited();
		res
	}

	// The body of LearnAndUpdateHeuristics, after which the onExit code is run on every path
	// The alias macro yields a mutable reference for reads and writes alike
	#[allow(clippy::unnecessary_mut_passed)]
	fn LearnAndUpdateHeuristicsBody(&mut self, contradictionInfo: &mut TContradictionInfo<Ind>, clsBeforeAllUipOrEmptyIfAllUipFailed: &mut CVector<TULit>) -> (TSpanTULit, Ind)
	{
		clsBeforeAllUipOrEmptyIfAllUipFailed.clear();
		self.m_Stat.m_Conflicts += 1;
		self.m_ConfsSinceRestart += 1;
		self.m_ConfsSinceNewInv += 1;

		if self.m_Params.m_ParamVerbosity.get() > 0 && self.m_Stat.m_Conflicts.is_multiple_of(self.m_Params.m_ParamStatPrintOutConfs.get() as u64)
		{
			stdout_print!("{}", self.StatStrShort(false));
		}

		// Giving an alias to m_VisitedLits to reflect the current usage for readability
		// The vector is moved out of the solver while in use, so that the borrows of the solver don't alias it (moving it doesn't move its buffer)
		let mut visitedNegLitsPrevDecLevelsVec = std::mem::take(&mut self.m_HandyLitsClearBefore[0]);
		macro_rules! visitedNegLitsPrevDecLevels
		{
			() => { (&mut visitedNegLitsPrevDecLevelsVec) };
		}
		let res = 'body:
		{
			// Clear from any previous usage
			visitedNegLitsPrevDecLevels!().clear();

			debug_assert!(self.CiIsLegal(contradictionInfo, true));

			let mut contradictingCls = self.CiGetSpan(contradictionInfo, TUV::MAX);

			debug_assert!(self.NV(2) || self.P(&("************************ Conflict #".to_string() + &self.m_Stat.m_Conflicts.to_string() + "\n" + &self.STrail() + "\n" + "Contradicting clause: " + &self.SLits(contradictingCls, false) + "\n")));

			// The other case is taken care of by the CDCL-loop in Solve
			debug_assert!(self.GetAssignedDecLevel(contradictingCls[0]) == self.GetAssignedDecLevel(contradictingCls[1]));
			// Again, any backtracking is taken care of by the CDCL-loop in Solve
			debug_assert!(self.GetAssignedDecLevel(contradictingCls[0]) == self.m_DecLevel);

			let mut varsToVisitCurrDecLevel: TUV = 0;

			self.LearnAndUpdateHeuristicsVisitCls(contradictingCls, if contradictionInfo.m_IsContradictionInBinaryCls { Self::BadClsInd } else { contradictionInfo.m_ParentClsInd }, false, &mut varsToVisitCurrDecLevel, visitedNegLitsPrevDecLevels!());

			// Used to maintain all the heuristics working if the contradicting clause manages to stay the 1UIP
			let trailEndBeforeOnTheFlySubsumption: TUVar = self.m_TrailEnd;

			self.m_VarsParentSubsumed.clear();

			let mut v: TUVar = self.m_TrailEnd;
			let vDecLevel: TUV = self.GetAssignedDecLevelVar(v);
			let vDecVar: TUVar = self.GetDecVar(vDecLevel);
			let isAssumpLevel = self.IsAssumpVar(vDecVar);
			// Removing a subsumed contradicting clause at assumption level might result in a correctness problem,
			// because the algorithm doesn't stop at first UIP, so it won't record the subsuming clause
			let mut contradictingIsLearnt = self.IsOnTheFlySubsumptionContradictingOn() && !isAssumpLevel;
			while varsToVisitCurrDecLevel != 1 || (isAssumpLevel && !(self.IsSatisfiedAssump(v) && self.m_AssignmentInfo[v as usize].m_Visit()))
			{
				'iter:
				{
					// The references to the entries of v
					let ai: *mut TAssignmentInfo = self.m_AssignmentInfo.get_ptr(v as usize);
					let vi: *mut TVarInfo<Ind> = self.m_VarInfo.get_ptr(v as usize);
					macro_rules! ai
					{
						() => { (unsafe { &mut *ai }) };
					}
					macro_rules! vi
					{
						() => { (unsafe { &mut *vi }) };
					}

					if ai!().m_Visit()
					{
						varsToVisitCurrDecLevel -= 1;
						debug_assert!(ai!().m_IsAssigned());
						if ai!().IsAssignedBinary() || vi!().m_ParentClsInd != Self::BadClsInd
						{
							// If the clause is binary, the parent will contain only the other literal (without l), but we don't need l anyway
							// If the clause isn't binary, the parent will be complete, which is fine too
							let parent = self.GetAssignedNonDecParentSpanVI(v);
							let psNo0: usize = if parent.size() == 1 { 2 } else { self.SizeWithoutDecLevel0(parent) };
							debug_assert!(self.NV(2) || self.P(&("Visited var: ".to_string() + &self.SVar(v) + &(if !self.IsCbLearntOrDrat() { String::new() } else { "; external-lit = ".to_string() + &self.GetExternalLit(self.GetAssignedLitForVar(v)).to_string() }) + "; Visited clause: " + &self.SLits(parent, false) + "\n")));
							let visitedBefore = self.m_VisitedVars.size();
							let updateClsCounter = self.IsOnTheFlySubsumptionParentOn() && psNo0 > 2 && (self.IsParentLongInitial(*ai!(), *vi!()) || psNo0 < self.m_Params.m_ParamOnTheFlySubsumptionParentMinGlueToDisable.get() as usize);
							self.LearnAndUpdateHeuristicsVisitCls(parent, if ai!().IsAssignedBinary() { Self::BadClsInd } else { vi!().m_ParentClsInd }, updateClsCounter, &mut varsToVisitCurrDecLevel, visitedNegLitsPrevDecLevels!());
							if contradictingIsLearnt
							{
								if GetVar(self.m_FlippedLit) == v
								{
									debug_assert!(self.NV(2) || self.P(&("Visited flipped var while still minimizing the contradicting clause, hence disable flipped recording for this conflict; var = ".to_string() + &self.SVar(v) + "\n")));
									self.m_FlippedLit = BadULit;
								}

								let csNo0 = self.SizeWithoutDecLevel0(contradictingCls);

								if self.m_VisitedVars.size() == visitedBefore && ((!contradictionInfo.m_IsContradictionInBinaryCls && !self.ClsGetIsLearnt(contradictionInfo.m_ParentClsInd)) || csNo0 < self.m_Params.m_ParamOnTheFlySubsumptionContradictingMinGlueToDisable.get() as usize)
								{
									// The contradicting clause can be subsumed!
									// Is the parent clause subsumed by the contradicting clause too?
									let parentSubsumedByContradicting = psNo0 == csNo0;
									let mut longInitParentSubsumedByLearntContradicting = parentSubsumedByContradicting && psNo0 > 2 && !self.ClsGetIsLearnt(vi!().m_ParentClsInd) && self.ClsGetIsLearnt(contradictionInfo.m_ParentClsInd);

									if contradictingCls.size() == 2
									{
										// A binary clause --> it can be deleted
										debug_assert!(contradictionInfo.m_IsContradictionInBinaryCls);
										// We have a new unit clause, which will serve as the learnt clause
										debug_assert!(varsToVisitCurrDecLevel == 1);
										self.DeleteBinaryCls(contradictingCls);
										let unitInd = if v == GetVar(contradictionInfo.m_BinClause[0]) { 1 } else { 0 };
										contradictingCls = Span::new(unsafe { contradictionInfo.m_BinClause.as_mut_ptr().add(unitInd) }, 1);
										if self.IsCbLearntOrDrat()
										{
											self.NewLearntClsApplyCbLearntDrat(contradictingCls);
										}
									}
									else
									{
										// A trinary clause, which will now convert into a binary one
										if contradictingCls.size() == 3
										{
											// ContradictingTrinary2BinaryByResolvingCurrVar
											{
												let l1: TULit = contradictingCls[if GetVar(contradictingCls[0]) == v { 1 } else { 0 }];
												let l2: TULit = contradictingCls[if GetVar(contradictingCls[2]) == v { 1 } else { 2 }];
												self.DeleteCls(contradictionInfo.m_ParentClsInd, None);
												contradictionInfo.m_IsContradictionInBinaryCls = true;
												contradictionInfo.m_BinClause = [l1, l2];
												// The 2nd parameter doesn't matter since the clause is binary
												self.AddClsToBufferAndWatch(Span::new(contradictionInfo.m_BinClause.as_mut_ptr(), 2), true, true);
											}
											longInitParentSubsumedByLearntContradicting = false;
										}
										else
										{
											debug_assert!(contradictingCls.size() > 3);
											let l = Negate(self.GetAssignedLitForVar(v));
											self.DeleteLitFromCls(&mut contradictionInfo.m_ParentClsInd, l);
										}
										contradictingCls = self.CiGetSpan(contradictionInfo, TUV::MAX);
									}
									self.m_Stat.m_LitsRemovedByConfSubsumption += 1;

									debug_assert!(self.NV(2) || self.P(&("Contradicting clause after reduction: ".to_string() + &self.SLits(contradictingCls, false) + "\n")));

									if parentSubsumedByContradicting
									{
										debug_assert!(self.NV(2) || self.P("Parent clause deleted, since subsumed by contradicting!\n"));
										if parent.size() == 1
										{
											debug_assert!(ai!().m_IsAssignedInBinary());
											let mut binCls: [TULit; 2] = [vi!().m_BinOtherLit(), self.GetAssignedLitForVar(v)];
											self.DeleteBinaryCls(Span::new(binCls.as_mut_ptr(), 2));
											self.m_Stat.m_LitsRemovedByConfSubsumption += 2;
										}
										else
										{
											if longInitParentSubsumedByLearntContradicting
											{
												debug_assert!(self.NV(2) || self.P("Parent and contradicting swapped, being a long-initial and learnt, respectively!\n"));
												self.m_Stat.m_LitsRemovedByConfSubsumption += 1;
												if self.ClsGetSize(vi!().m_ParentClsInd) == 3
												{
													// Contradicting2BinaryByRemovingLevel0
													{
														let mut currGoodLitInd: usize = 0;
														for l in contradictingCls
														{
															if self.GetAssignedDecLevel(l) != 0
															{
																contradictionInfo.m_BinClause[currGoodLitInd] = l;
																currGoodLitInd += 1;
															}
														}
														debug_assert!(currGoodLitInd == 2);
														contradictionInfo.m_IsContradictionInBinaryCls = true;
														// The 2nd parameter doesn't matter since the clause is binary
														self.AddClsToBufferAndWatch(Span::new(contradictionInfo.m_BinClause.as_mut_ptr(), 2), true, true);
													}
												}
												else
												{
													let l = self.GetAssignedLitForVar(v);
													let mut parentClsInd = vi!().m_ParentClsInd;
													self.DeleteLitFromCls(&mut parentClsInd, l);
													vi!().m_ParentClsInd = parentClsInd;
													std::mem::swap(&mut vi!().m_ParentClsInd, &mut contradictionInfo.m_ParentClsInd);
												}
												contradictingCls = self.CiGetSpan(contradictionInfo, TUV::MAX);
											}
											self.m_Stat.m_LitsRemovedByConfSubsumption += self.ClsGetSize(vi!().m_ParentClsInd) as u64;
											self.DeleteCls(vi!().m_ParentClsInd, None);
										}
									}

									while self.m_TrailEnd != v
									{
										self.UnassignVar(self.m_TrailEnd);
									}
									self.UnassignVar(self.m_TrailEnd);
									while !self.IsVisitedVar(self.m_TrailEnd)
									{
										self.UnassignVar(self.m_TrailEnd);
									}
									if self.m_CurrCustomBtStrat > 0
									{
										let dl = self.GetAssignedDecLevelVar(self.m_TrailEnd);
										let maxDecLevelScore = self.CalcMaxDecLevelScore(dl);
										self.m_BestScorePerDecLevel[dl as usize] = maxDecLevelScore;
									}
								}
								else
								{
									contradictingIsLearnt = false;
								}
							}
							else if self.IsOnTheFlySubsumptionParentOn() && psNo0 > 2 && (self.IsParentLongInitial(*ai!(), *vi!()) || psNo0 < self.m_Params.m_ParamOnTheFlySubsumptionParentMinGlueToDisable.get() as usize)
							{
								let parentSubsumedByCurrResolvent = visitedNegLitsPrevDecLevels!().size() + varsToVisitCurrDecLevel as usize + 1 <= psNo0;
								if !parentSubsumedByCurrResolvent
								{
									break 'iter;
								}

								// The C++ version now tests that every literal of the current resolvent (the visited literals of the previous decision levels and the visited variables
								// of the current one) appears in the parent. Once the size test above has passed, this always holds: the resolvent contains the parent without the pivot,
								// and it's not larger than that, so they are equal
								#[cfg(debug_assertions)]
								{
									let visitedNegLitsPrevDecLevelsSpan = visitedNegLitsPrevDecLevels!().get_span();
									debug_assert!(visitedNegLitsPrevDecLevelsSpan.iter().all(|l| self.m_CurrClsCounters[GetVar(l) as usize] == self.m_CurrClsCounter));
									let mut varsVisitedNum: TUV = 0;
									let mut u: TUVar = self.m_VarInfo[v as usize].m_TrailPrev;
									while varsVisitedNum < varsToVisitCurrDecLevel
									{
										if self.IsVisitedVar(u)
										{
											debug_assert!(self.m_CurrClsCounters[u as usize] == self.m_CurrClsCounter);
											varsVisitedNum += 1;
										}
										u = self.m_VarInfo[u as usize].m_TrailPrev;
									}
								}

								// try_reserve is the counterpart of the C++ catch block (a plain push would abort on an allocation failure)
								let parentSubsumed = TParentSubsumed::new(self.GetAssignedLitForVar(v), self.m_AssignmentInfo[v as usize].IsAssignedBinary(), self.m_VarInfo[v as usize].m_ParentClsInd);
								if self.m_VarsParentSubsumed.try_reserve(1).is_ok()
								{
									self.m_VarsParentSubsumed.push(parentSubsumed);
								}
								else
								{
									self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "m_VarsParentSubsumed.push_back allocation failure".to_string());
								}

								if self.IsUnrecoverable()
								{
									break 'body (visitedNegLitsPrevDecLevels!().get_span(), Self::BadClsInd);
								}
								debug_assert!(self.NV(2) || self.P(&("On-the-fly subsumption will remove the pivot ".to_string() + &self.SVar(v) + " from the parent " + &HexStrInd(self.m_VarInfo[v as usize].m_ParentClsInd) + " : " + &self.SLits(parent, false) + "\n")));
							}
						}
					}
				}
				v = self.m_VarInfo[v as usize].m_TrailPrev;
			}

			if self.m_Params.m_ParamMinimizeClausesMinisat.get() && visitedNegLitsPrevDecLevels!().size() > 1
			{
				self.MinimizeClauseMinisat(visitedNegLitsPrevDecLevels!());
				if self.IsUnrecoverable()
				{
					break 'body (visitedNegLitsPrevDecLevels!().get_span(), Self::BadClsInd);
				}
			}

			// Find the first UIP
			while !self.m_AssignmentInfo[v as usize].m_Visit()
			{
				v = self.m_VarInfo[v as usize].m_TrailPrev;
			}
			let firstUIPNegated: TULit = Negate(self.GetAssignedLitForVar(v));

			if self.m_Params.m_ParamAllUipMode.get() == 1 || self.m_Params.m_ParamAllUipMode.get() == 3
			{
				clsBeforeAllUipOrEmptyIfAllUipFailed.assign(visitedNegLitsPrevDecLevels!());
				let allUipGenerated = self.GenerateAllUipClause(visitedNegLitsPrevDecLevels!());
				if !allUipGenerated
				{
					clsBeforeAllUipOrEmptyIfAllUipFailed.clear();
				}
				else
				{
					clsBeforeAllUipOrEmptyIfAllUipFailed.push_back(firstUIPNegated);
				}
				if clsBeforeAllUipOrEmptyIfAllUipFailed.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "clsBeforeAllUipOrEmptyIfAllUipFailed allocation failure".to_string());
				}
				if self.IsUnrecoverable()
				{
					break 'body (visitedNegLitsPrevDecLevels!().get_span(), Self::BadClsInd);
				}
			}

			// Forming the 1UIP conflict clause, ensuring that literal 0 has the highest decision level, while literal 1 is the second highest
			visitedNegLitsPrevDecLevels!().push_back(firstUIPNegated);
			{
				let s = visitedNegLitsPrevDecLevels!().get_span();
				s.swap(0, s.size() - 1);
			}

			if visitedNegLitsPrevDecLevels!().size() <= self.m_Params.m_ParamMinimizeClausesBinMaxSize.get() as usize && self.GetGlueAndMarkCurrDecLevels(visitedNegLitsPrevDecLevels!().get_span()) <= self.m_Params.m_ParamMinimizeClausesBinMaxLbd.get()
			{
				self.MinimizeClauseBin(visitedNegLitsPrevDecLevels!());
			}

			if self.IsUnrecoverable()
			{
				break 'body (visitedNegLitsPrevDecLevels!().get_span(), Self::BadClsInd);
			}

			if visitedNegLitsPrevDecLevels!().size() > 2
			{
				let s = visitedNegLitsPrevDecLevels!().get_span();
				let highestDecLevelLitIt = self.GetAssignedLitsHighestDecLevelIt(s, 1);

				if s[highestDecLevelLitIt] != s[1]
				{
					s.swap(highestDecLevelLitIt, 1);
				}
			}

			debug_assert!(self.NV(1) || self.P(&("New-clause at level ".to_string() + &self.m_DecLevel.to_string() + " : " + &self.SLits(visitedNegLitsPrevDecLevels!().get_span(), false) + "\n")));

			let mut addInitCls = false;
			let mut clsStart: Ind = Self::BadClsInd;

			if contradictingIsLearnt
			{
				debug_assert!(self.NV(2) || self.P("The contradicting clause was reduced all the way to 1UIP\n"));
				// The contradicting clause was reduced all the way to 1UIP

				// Disabling flipped clause recording
				self.m_FlippedLit = BadULit;
				if !contradictionInfo.m_IsContradictionInBinaryCls
				{
					// Is clause initial
					addInitCls = !self.ClsGetIsLearnt(contradictionInfo.m_ParentClsInd);
				}
				if contradictingCls.size() > visitedNegLitsPrevDecLevels!().size()
				{
					debug_assert!(self.NV(2) || self.P("The contradicting clause was reduced further by minimization and/or binary-resolution\n"));
					if contradictionInfo.m_IsContradictionInBinaryCls
					{
						self.m_Stat.m_LitsRemovedByConfSubsumption += 2;
						self.DeleteBinaryCls(contradictingCls);
					}
					else
					{
						self.m_Stat.m_LitsRemovedByConfSubsumption += self.ClsGetSize(contradictionInfo.m_ParentClsInd) as u64;
						self.DeleteCls(contradictionInfo.m_ParentClsInd, None);
					}

					contradictingIsLearnt = false;
				}
				else
				{
					debug_assert!(self.NV(2) || self.P("The contradicting clause will serve as the asserting clause, no need to record a new one!\n"));
					if !contradictionInfo.m_IsContradictionInBinaryCls
					{
						clsStart = contradictionInfo.m_ParentClsInd;
					}
				}
			}

			if !contradictingIsLearnt
			{
				// Always part of proof, even if stays initial (that is, not deletable)
				// Added this part to fix a DRAT bug
				clsStart = self.AddClsToBufferAndWatch(visitedNegLitsPrevDecLevels!().get_span(), !addInitCls, true);
			}

			debug_assert!(self.m_Params.m_ParamAssertConsistency.get() < 2 || self.m_Stat.m_Conflicts < self.m_Params.m_ParamAssertConsistencyStartConf.get() as u64 || self.WLAssertConsistency(false));

			if self.m_Params.m_ParamVarActivityUseMapleLevelBreaker.get()
			{
				let s = visitedNegLitsPrevDecLevels!().get_span();
				let secondHighestDecLevel: TUV = if s.size() <= 1 { 0 } else { std::cmp::min(self.GetAssignedDecLevel(s[0]), self.GetAssignedDecLevel(s[1])) };
				let mut decLevelMinToUpdate: TUV = secondHighestDecLevel.wrapping_sub(self.m_Params.m_ParamVarActivityMapleLevelBreakerDecrease.get());
				if decLevelMinToUpdate > secondHighestDecLevel
				{
					decLevelMinToUpdate = 0;
				}
				for vv in self.m_VisitedVars.get_span()
				{
					if self.m_VarInfo[vv as usize].m_DecLevel >= decLevelMinToUpdate
					{
						self.UpdateScoreVar(vv, 1.0);
					}
				}
			}

			let updateGlue = visitedNegLitsPrevDecLevels!().size() > 2 && !addInitCls;
			let glue: TUV = if updateGlue { self.ClsGetGlue(clsStart) } else { 0 };
			debug_assert!(self.IsOnTheFlySubsumptionContradictingOn() || trailEndBeforeOnTheFlySubsumption == self.m_TrailEnd);
			self.UpdateDecisionStrategyOnNewConflict(glue, GetVar(firstUIPNegated), trailEndBeforeOnTheFlySubsumption);

			if updateGlue
			{
				self.ClsDelNewLearntOrGlueUpdate(clsStart, glue);
			}

			(visitedNegLitsPrevDecLevels!().get_span(), clsStart)
		};
		self.m_HandyLitsClearBefore[0] = visitedNegLitsPrevDecLevelsVec;
		res
	}

	// The VisitLit lambda of RecordFlipped
	#[inline(always)]
	fn RecordFlippedVisitLit(&mut self, l: TULit, varsToVisitCurrFlippedLevel: &mut TUV, visitedNegLitsPrevFlippedLevels: *mut CVector<TULit>)
	{
		let v: TUVar = GetVar(l);
		let decLevel: TUV = self.GetAssignedDecLevel(l);

		if !self.m_AssignmentInfo[v as usize].m_Visit() && decLevel != 0
		{
			self.MarkVisitedVar(v);

			if self.IsRootedVar(v)
			{
				*varsToVisitCurrFlippedLevel += 1;
			}
			else
			{
				unsafe { (*visitedNegLitsPrevFlippedLevels).push_back(if self.IsSatisfied(l) { Negate(l) } else { l }) };
			}
		}
	}

	// The VisitCls lambda of RecordFlipped
	#[inline(always)]
	fn RecordFlippedVisitCls(&mut self, cls: TSpanTULit, _longClsInd: Ind, varsToVisitCurrFlippedLevel: &mut TUV, visitedNegLitsPrevFlippedLevels: *mut CVector<TULit>)
	{
		for l in cls
		{
			self.RecordFlippedVisitLit(l, varsToVisitCurrFlippedLevel, visitedNegLitsPrevFlippedLevels);
		}
	}

	// Return a flipped clause if:
	// (1) Flipped recording is on; (2) The current level is flipped; (3) The flipped clause is not subsumed by the main clause
	// The return span is empty iff the recording is unsuccessful (in which case TUInd is BadParentClsInd too)
	// The alias macro yields a mutable reference for reads and writes alike
	#[allow(clippy::unnecessary_mut_passed)]
	pub(crate) fn RecordFlipped(&mut self, contradictionInfo: &mut TContradictionInfo<Ind>, mainClsBeforeAllUip: TSpanTULit) -> (TSpanTULit, Ind)
	{
		// Giving an alias to m_VisitedLits to reflect the current usage for readability
		// The vector is moved out of the solver while in use, so that the borrows of the solver don't alias it (moving it doesn't move its buffer)
		let mut visitedNegLitsPrevFlippedLevelsVec = std::mem::take(&mut self.m_HandyLitsClearBefore[1]);
		macro_rules! visitedNegLitsPrevFlippedLevels
		{
			() => { (&mut visitedNegLitsPrevFlippedLevelsVec) };
		}
		// Clear from any previous usage
		visitedNegLitsPrevFlippedLevels!().clear();

		if self.m_Params.m_ParamFlippedRecordingMaxLbdToRecord.get() == 0 || self.m_FlippedLit == BadULit || self.GetAssignedDecLevel(self.m_FlippedLit) != self.m_DecLevel
		{
			visitedNegLitsPrevFlippedLevels!().clear();
			let res = (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
			self.m_HandyLitsClearBefore[1] = visitedNegLitsPrevFlippedLevelsVec;
			return res;
		}

		let flippedVar: TUVar = GetVar(self.m_FlippedLit);
		let mut v: TUVar = self.m_TrailEnd;
		while v != flippedVar
		{
			debug_assert!(v != BadUVar);
			self.MarkRootedVar(v);
			v = self.m_VarInfo[v as usize].m_TrailPrev;
		}
		self.MarkRootedVar(flippedVar);

		// The body, after which the onExit code is run on every path
		let res: (TSpanTULit, Ind) = 'body:
		{
			debug_assert!(self.NV(2) || contradictionInfo.m_IsContradictionInBinaryCls || self.P(&("Contradicting cls: ".to_string() + &self.SLits(self.Cls(contradictionInfo.m_ParentClsInd), false) + "\n")));

			debug_assert!(self.CiIsLegal(contradictionInfo, true));

			let contradictingCls = self.CiGetSpan(contradictionInfo, TUV::MAX);

			debug_assert!(self.NV(2) || self.P("Will try to record a flipped clause\n"));

			// The other case is taken care of by the CDCL-loop in Solve
			debug_assert!(self.GetAssignedDecLevel(contradictingCls[0]) == self.GetAssignedDecLevel(contradictingCls[1]));
			// Again, any backtracking is taken care of by the CDCL-loop in Solve
			debug_assert!(self.GetAssignedDecLevel(contradictingCls[0]) == self.m_DecLevel);

			let mut varsToVisitCurrFlippedLevel: TUV = 0;

			self.RecordFlippedVisitCls(contradictingCls, if contradictionInfo.m_IsContradictionInBinaryCls { Self::BadClsInd } else { contradictionInfo.m_ParentClsInd }, &mut varsToVisitCurrFlippedLevel, visitedNegLitsPrevFlippedLevels!());

			if varsToVisitCurrFlippedLevel == 1
			{
				debug_assert!(self.NV(2) || self.P("Flipped is skipped, since the contradicting clause has only one literal beyond the flipped (which might have happened because of on-the-fly-subsumption)\n"));
				visitedNegLitsPrevFlippedLevels!().clear();
				break 'body (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
			}

			let mut v: TUVar = self.m_TrailEnd;

			while varsToVisitCurrFlippedLevel != 1
			{
				let ai = self.m_AssignmentInfo[v as usize];
				let vi = self.m_VarInfo[v as usize];

				if ai.m_Visit()
				{
					varsToVisitCurrFlippedLevel -= 1;
					debug_assert!(ai.m_IsAssigned());
					if ai.IsAssignedBinary() || vi.m_ParentClsInd != Self::BadClsInd
					{
						// If the clause is binary, the parent will contain only the other literal (without l), but we don't need l anyway
						// If the clause isn't binary, the parent will be complete, which is fine too
						let parent = self.GetAssignedNonDecParentSpanVI(v);
						debug_assert!(self.NV(2) || self.P(&("Visited var: ".to_string() + &self.SVar(v) + "; Visited clause: " + &self.SLits(parent, false) + "\n")));
						self.RecordFlippedVisitCls(parent, if ai.IsAssignedBinary() { Self::BadClsInd } else { vi.m_ParentClsInd }, &mut varsToVisitCurrFlippedLevel, visitedNegLitsPrevFlippedLevels!());
					}
				}
				v = self.m_VarInfo[v as usize].m_TrailPrev;
			}

			// Find the first UIP w.r.t the flipped level
			while !self.m_AssignmentInfo[v as usize].m_Visit()
			{
				v = self.m_VarInfo[v as usize].m_TrailPrev;
			}

			if self.m_Params.m_ParamFlippedRecordDropIfSubsumed.get()
			{
				let mut isSubsumedByMain = true;
				for mainClsLit in mainClsBeforeAllUip
				{
					if !self.IsVisited(mainClsLit) && GetVar(mainClsLit) != v
					{
						isSubsumedByMain = false;
						break;
					}
				}

				if isSubsumedByMain
				{
					debug_assert!(self.NV(2) || self.P(&("Flipped is subsumed by the main clause: skipping; the clause so far: ".to_string() + &self.SLit(Negate(self.GetAssignedLitForVar(v))) + " " + &self.SLits(visitedNegLitsPrevFlippedLevels!().get_span(), false) + "\n")));
					visitedNegLitsPrevFlippedLevels!().clear();
					break 'body (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
				}
			}

			// Must be clean before MinimizeClauseMinisat, since we don't need it anymore in this function, whereas MinimizeClauseMinisat uses it
			self.CleanRooted();

			if self.m_Params.m_ParamMinimizeClausesMinisat.get() && visitedNegLitsPrevFlippedLevels!().size() > 1
			{
				self.MinimizeClauseMinisat(visitedNegLitsPrevFlippedLevels!());
				if self.IsUnrecoverable()
				{
					visitedNegLitsPrevFlippedLevels!().clear();
					break 'body (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
				}
			}

			if self.m_Params.m_ParamAllUipMode.get() == 2 || self.m_Params.m_ParamAllUipMode.get() == 3
			{
				self.GenerateAllUipClause(visitedNegLitsPrevFlippedLevels!());
				if self.IsUnrecoverable()
				{
					visitedNegLitsPrevFlippedLevels!().clear();
					break 'body (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
				}
			}

			let firstUIPNegated: TULit = Negate(self.GetAssignedLitForVar(v));

			// Forming the 1UIP conflict clause, ensuring that literal 0 has the highest decision level, while literal 1 is the second highest
			visitedNegLitsPrevFlippedLevels!().push_back(firstUIPNegated);
			{
				let s = visitedNegLitsPrevFlippedLevels!().get_span();
				s.swap(0, s.size() - 1);
			}

			if visitedNegLitsPrevFlippedLevels!().size() <= self.m_Params.m_ParamMinimizeClausesBinMaxSize.get() as usize && self.GetGlueAndMarkCurrDecLevels(visitedNegLitsPrevFlippedLevels!().get_span()) <= self.m_Params.m_ParamMinimizeClausesBinMaxLbd.get()
			{
				self.MinimizeClauseBin(visitedNegLitsPrevFlippedLevels!());
			}

			if self.IsUnrecoverable()
			{
				break 'body (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
			}

			let glue = self.GetGlueAndMarkCurrDecLevels(visitedNegLitsPrevFlippedLevels!().get_span());
			if glue > self.m_Params.m_ParamFlippedRecordingMaxLbdToRecord.get()
			{
				debug_assert!(self.NV(2) || self.P(&("Flipped glue ".to_string() + &glue.to_string() + " is higher than the threshold " + &self.m_Params.m_ParamFlippedRecordingMaxLbdToRecord.get().to_string() + ", so flipped recording will be skipped\n")));
				visitedNegLitsPrevFlippedLevels!().clear();
				break 'body (visitedNegLitsPrevFlippedLevels!().get_span(), Self::BadClsInd);
			}

			if visitedNegLitsPrevFlippedLevels!().size() > 2
			{
				let s = visitedNegLitsPrevFlippedLevels!().get_span();
				let highestDecLevelLitIt = self.GetAssignedLitsHighestDecLevelIt(s, 1);

				if s[highestDecLevelLitIt] != s[1]
				{
					s.swap(highestDecLevelLitIt, 1);
				}
			}

			debug_assert!(self.NV(1) || self.P(&("New flipped clause at level ".to_string() + &self.m_DecLevel.to_string() + " : " + &self.SLits(visitedNegLitsPrevFlippedLevels!().get_span(), false) + "\n")));

			let clsStart = self.AddClsToBufferAndWatch(visitedNegLitsPrevFlippedLevels!().get_span(), true, true);

			debug_assert!(self.NV(2) || self.P(&("Flipped clause recorded: ".to_string() + &self.SLits(visitedNegLitsPrevFlippedLevels!().get_span(), false) + "\n")));
			self.m_Stat.m_FlippedClauses += 1;

			if visitedNegLitsPrevFlippedLevels!().size() > 2
			{
				self.ClsDelNewLearntOrGlueUpdate(clsStart, glue);
			}

			(visitedNegLitsPrevFlippedLevels!().get_span(), clsStart)
		};

		// onExit
		self.CleanVisited();
		self.CleanRooted();
		self.m_HandyLitsClearBefore[1] = visitedNegLitsPrevFlippedLevelsVec;

		res
	}

	pub(crate) fn OnBadDratFile(&mut self)
	{
		// The only way for the output to go bad here is a failed write, which corresponds to the badbit of a C++ stream
		self.SetStatus(TToporStatus::STATUS_DRAT_FILE_PROBLEM, "Problem with DRAT file generation: ".to_string() + "Read/writing error on i/o operation");
	}

	pub(crate) fn NewLearntClsApplyCbLearntDrat(&mut self, learntCls: TSpanTULit)
	{
		debug_assert!(self.IsCbLearntOrDrat());

		self.m_UserCls.resize(learntCls.size());
		let userClsSpan = self.m_UserCls.get_span();
		for i in 0..learntCls.size()
		{
			userClsSpan.set(i, self.GetExternalLit(learntCls[i]));
		}

		if self.m_DratSortEveryClause
		{
			// std::sort isn't stable, but there are no two literals of the same variable in a clause, so the result is the same
			userClsSpan.as_mut_slice().sort_unstable_by_key(|l| if *l < 0 { l.wrapping_neg() } else { *l });
		}

		if self.m_OpenedDratFile.is_some()
		{
			if self.m_Status.get() == TToporStatus::STATUS_DRAT_FILE_PROBLEM
			{
				// !o.good()
				self.OnBadDratFile();
				return;
			}

			let mut buf: Vec<u8> = Vec::new();

			if self.m_IsDratBinary
			{
				buf.push(b'a');
				for l in userClsSpan
				{
					let mut ul64: u64 = if l > 0 { (l as u64) << 1 } else { ((l.wrapping_neg() as i64 as u64) << 1) + 1 };

					while ul64 & !0x7f != 0
					{
						let ch: u8 = ((ul64 & 0x7f) | 0x80) as u8;
						buf.push(ch);
						ul64 >>= 7;
					}
					buf.push(ul64 as u8);
				}
				buf.push(0);
			}
			else
			{
				for l in userClsSpan
				{
					buf.extend_from_slice(l.to_string().as_bytes());
					buf.push(b' ');
				}
				buf.extend_from_slice(b"0\n");
			}

			let isDratBinary = self.m_IsDratBinary;
			let o = self.m_OpenedDratFile.as_mut().unwrap();
			// endl flushes the stream in the textual mode
			let isGood = o.write_all(&buf).is_ok() && (isDratBinary || o.flush().is_ok());
			if !isGood
			{
				self.OnBadDratFile();
				return;
			}

			debug_assert!(isDratBinary || self.NV(2) || self.P(&("New DRAT line: ".to_string() + &self.SLits(learntCls, true) + " 0\n")));
		}

		if let Some(CbNewLearntCls) = &mut self.M_CbNewLearntCls
		{
			let stopTopor = CbNewLearntCls(userClsSpan.as_slice());
			if stopTopor == TStopTopor::VAL_STOP
			{
				self.SetStatus(TToporStatus::STATUS_USER_INTERRUPT, "User interrupt requested during callback (new-learnt-clause)".to_string());
			}
		}
	}

	pub(crate) fn CiIsLegal(&mut self, ci: &mut TContradictionInfo<Ind>, assertTwoLitsSameDecLevel: bool) -> bool
	{
		debug_assert!(ci.IsContradiction());
		let cls = self.CiGetSpan(ci, TUV::MAX);
		// The contradicting clause's size is at least 2
		debug_assert!(cls.size() >= 2);
		// All the literals in the contradicting clause must be assigned false
		debug_assert!(cls.iter().all(|l| self.IsFalsified(l)));
		// The first two literals must have the same highest decision level(BCP ensures this invariant)
		debug_assert!(!assertTwoLitsSameDecLevel || self.GetAssignedDecLevel(cls[0]) == self.GetAssignedDecLevel(cls[1]));
		debug_assert!(cls.size() == 2 || self.GetAssignedDecLevel(cls[0]) >= self.GetAssignedDecLevel(cls[self.GetAssignedLitsHighestDecLevelIt(cls, 2)]));
		debug_assert!(cls.size() == 2 || self.GetAssignedDecLevel(cls[1]) >= self.GetAssignedDecLevel(cls[self.GetAssignedLitsHighestDecLevelIt(cls, 2)]));
		let _ = (cls, assertTwoLitsSameDecLevel);
		true
	}

	// ret.first: for every decision level m_HugeCounterPerDecLevel[decLevel] - ret.first is the number of literals in cls with that decision level
	// ret.second: all the decision levels in cls in a priority queue (top will return the greatest first)
	pub(crate) fn GetDecLevelsAndMarkInHugeCounter(&mut self, cls: TSpanTULit) -> (u64, BinaryHeap<TUV>)
	{
		let mut decLevels: BinaryHeap<TUV> = BinaryHeap::new();

		if self.m_HugeCounterPerDecLevel.cap() <= self.m_DecLevel as usize
		{
			self.m_HugeCounterPerDecLevel.reserve_atleast_with_max_init(self.m_DecLevel as usize, self.GetNextVar() as usize, 0);
			if self.m_HugeCounterPerDecLevel.uninitialized_or_erroneous()
			{
				self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "GetDecLevelsAndMarkInHugeCounter: allocation failed".to_string());
				// The C++ version goes on to access the (now null) array; the caller returns immediately upon the unrecoverable status
				return (self.m_HugeCounterDecLevels, decLevels);
			}
		}

		let initMarkedDecLevelsCounter = self.m_HugeCounterDecLevels;

		for l in cls
		{
			debug_assert!(self.IsAssigned(l));
			let decLevel: TUV = self.GetAssignedDecLevel(l);
			if self.m_HugeCounterPerDecLevel[decLevel as usize] <= initMarkedDecLevelsCounter
			{
				decLevels.push(decLevel);
				self.m_HugeCounterPerDecLevel[decLevel as usize] = initMarkedDecLevelsCounter + 1;
			}
			else
			{
				self.m_HugeCounterPerDecLevel[decLevel as usize] += 1;
			}

			if self.m_HugeCounterPerDecLevel[decLevel as usize] > self.m_HugeCounterDecLevels
			{
				self.m_HugeCounterDecLevels = self.m_HugeCounterPerDecLevel[decLevel as usize];
			}
		}

		(initMarkedDecLevelsCounter, decLevels)
	}

	pub(crate) fn ConflictAnalysisLoop(&mut self, contradictionInfo: &mut TContradictionInfo<Ind>)
	{
		while self.m_Status.get() == TToporStatus::STATUS_UNDECIDED && contradictionInfo.IsContradiction()
		{
			// See the contradicting clause invariants asserted in GetContradictingClauseSpan
			debug_assert!(self.CiIsLegal(contradictionInfo, true));
			let contradictingCls = self.CiGetSpan(contradictionInfo, 2);
			// BCP guarantees that the two first literals (comprising the watched literals)
			// have the highest decision level in the contradicting clause
			let maxDecLevelInContradictingCls = std::cmp::max(self.GetAssignedDecLevel(contradictingCls[0]), self.GetAssignedDecLevel(contradictingCls[1]));

			if maxDecLevelInContradictingCls == 0
			{
				// Global contradiction!
				self.SetStatus(TToporStatus::STATUS_CONTRADICTORY, "Global contradiction!".to_string());
				continue;
			}

			// The case of one literal of the highest decision level is taken care of in BCP
			debug_assert!(self.GetAssignedDecLevel(contradictingCls[0]) == self.GetAssignedDecLevel(contradictingCls[1]));

			self.Backtrack(maxDecLevelInContradictingCls as TLit, false, false);
			let mut clsBeforeAllUipOrEmptyIfAllUipFailed: CVector<TULit> = CVector::new(0, 0);
			let (mut cls, mut assertingClsInd) = self.LearnAndUpdateHeuristics(contradictionInfo, &mut clsBeforeAllUipOrEmptyIfAllUipFailed);
			if self.IsUnrecoverable()
			{
				return;
			}
			let conflictAtAssumptionLevel = self.m_DecLevel <= self.m_DecLevelOfLastAssignedAssumption;
			if self.m_EarliestFalsifiedAssump != BadULit || conflictAtAssumptionLevel
			{
				self.m_FlippedLit = BadULit;
			}
			let (mut additionalCls, mut additionalAssertingClsInd) = self.RecordFlipped(contradictionInfo, if clsBeforeAllUipOrEmptyIfAllUipFailed.empty() { cls } else { clsBeforeAllUipOrEmptyIfAllUipFailed.get_span() });
			if self.IsUnrecoverable()
			{
				return;
			}

			if !additionalCls.is_empty() &&
				((cls.size() > 1 && additionalCls.size() == 1) ||
					(cls.size() != 1 && additionalCls.size() != 1 &&
						self.GetAssignedDecLevel(additionalCls[0]) != self.GetAssignedDecLevel(additionalCls[1]) &&
						self.GetAssignedDecLevel(additionalCls[1]) < self.GetAssignedDecLevel(cls[1])))
			{
				// Swap the flipped clause with the 1UIP clause, since the former one is also an asserting clause of a lower 2nd dec. level
				debug_assert!(self.NV(2) || self.P("Swapped the flipped clause with the 1UIP clause\n"));
				self.m_Stat.m_FlippedClausesSwapped += 1;
				std::mem::swap(&mut cls, &mut additionalCls);
				std::mem::swap(&mut assertingClsInd, &mut additionalAssertingClsInd);
			}

			// The asserting clause must be non-empty
			debug_assert!(cls.size() > 0);
			// All the literals in the asserting clause must be assigned false
			debug_assert!(cls.iter().all(|l| self.IsFalsified(l)));
			// The first literal's level must not be smaller than the rest
			debug_assert!(cls.size() == 1 || self.GetAssignedDecLevel(cls[0]) >= self.GetAssignedDecLevel(cls[self.GetAssignedLitsHighestDecLevelIt(cls, 1)]));
			// The second literal's level must not be smaller than the rest, but the first one
			debug_assert!(cls.size() <= 2 || self.GetAssignedDecLevel(cls[1]) >= self.GetAssignedDecLevel(cls[self.GetAssignedLitsHighestDecLevelIt(cls, 2)]));

			let mut ncbBtLevel: TUV = if cls.size() > 1 { self.GetAssignedDecLevel(cls[1]) } else { 0 };

			if !conflictAtAssumptionLevel && ncbBtLevel < self.m_DecLevelOfLastAssignedAssumption
			{
				// We don't want to backtrack lower than the assumptions to prevent assumption re-propagation
				ncbBtLevel = self.m_DecLevelOfLastAssignedAssumption;
			}

			if cls.size() > 2
			{
				self.m_RstGlueAssertingGluedClss += 1;
				if self.m_CurrRestartStrat == RESTART_STRAT_LBD
				{
					// Might be initial because of on-the-fly subsumption
					let glue = if self.ClsGetIsLearnt(assertingClsInd) { self.ClsGetGlue(assertingClsInd) } else { self.GetGlueAndMarkCurrDecLevels(self.ConstClsSpan(assertingClsInd, TUV::MAX)) };
					self.RstNewAssertingGluedCls(glue);
				}
			}

			// Determine how to backtrack
			let isChronoBt = self.m_EarliestFalsifiedAssump != BadULit || conflictAtAssumptionLevel || (self.m_ConfsSinceNewInv >= self.m_Params.m_ParamConflictsToPostponeChrono.get() as u64 && self.m_DecLevel.wrapping_sub(ncbBtLevel) > self.m_CurrChronoBtIfHigher) || maxDecLevelInContradictingCls <= self.m_DecLevelOfLastAssignedAssumption;
			let btLevel: TUV = if isChronoBt
			{
				if self.m_EarliestFalsifiedAssump != BadULit || conflictAtAssumptionLevel || self.m_CurrCustomBtStrat == 0 || ncbBtLevel + 1 == self.m_DecLevel { self.m_DecLevel - 1 } else { self.GetDecLevelWithBestScore(ncbBtLevel + 1, self.m_DecLevel) }
			}
			else
			{
				ncbBtLevel
			};
			self.Backtrack(btLevel as TLit, false, false);
			self.m_FlippedLit = cls[0];
			self.Assign(self.m_FlippedLit, if cls.size() >= 2 { assertingClsInd } else { Self::BadClsInd }, if cls.size() == 1 { BadULit } else { cls[1] }, if cls.size() == 1 { 0 } else { self.GetAssignedDecLevel(cls[1]) }, true, false);
			debug_assert!(self.NV(2) || self.P(&("***** Flipped former UIP to ".to_string() + &self.SLit(cls[0]) + "\n")));

			self.AdditionalAssign(additionalCls, additionalAssertingClsInd);
			if self.IsOnTheFlySubsumptionParentOn() && !self.m_VarsParentSubsumed.is_empty()
			{
				debug_assert!(self.IsOnTheFlySubsumptionParentOn());
				for vpsInd in 0..self.m_VarsParentSubsumed.len()
				{
					let mut vps = self.m_VarsParentSubsumed[vpsInd];
					let v: TUVar = GetVar(vps.m_L);
					// The C++ version also handles a binary parent here, but only the parents with more than two literals (not at level 0) are recorded as subsumed
					debug_assert!(!vps.m_IsBinary);
					let parentCls = self.Cls(vps.m_ParentClsInd);
					if parentCls.size() == 3
					{
						let v: TUVar = GetVar(vps.m_L);
						debug_assert!(self.NV(2) || self.P(&("On-the-fly subsumption converted the long parent to a binary; pivot = ".to_string() + &self.SVar(v) + "\n")));
						let l1: TULit = parentCls.get(if GetVar(parentCls.get(0)) == v { 1 } else { 0 });
						let l2: TULit = parentCls.get(if GetVar(parentCls.get(2)) == v { 1 } else { 2 });
						self.DeleteCls(vps.m_ParentClsInd, None);
						let mut binCls: [TULit; 2] = [l1, l2];
						let binClsSpan = Span::new(binCls.as_mut_ptr(), 2);
						// The 2nd parameter doesn't matter since the clause is binary
						self.AddClsToBufferAndWatch(binClsSpan, true, true);
						self.AdditionalAssign(binClsSpan, Self::BadClsInd);
					}
					else
					{
						debug_assert!(self.NV(2) || self.P(&("Before on-the-fly subsumption deleted the pivot ".to_string() + &self.SVar(v) + " from the long parent " + &HexStrInd(vps.m_ParentClsInd) + "; clause: " + &self.SLits(parentCls, false) + "\n")));
						self.DeleteLitFromCls(&mut vps.m_ParentClsInd, vps.m_L);
						self.m_VarsParentSubsumed[vpsInd] = vps;
						// The C++ version prints the stale view of the clause here; we re-derive it, since it might have moved to another buffer
						debug_assert!(self.NV(2) || self.P(&("After on-the-fly subsumption deleted the pivot ".to_string() + &self.SVar(v) + " from the long parent " + &HexStrInd(vps.m_ParentClsInd) + "; clause: " + &self.SLits(self.Cls(vps.m_ParentClsInd), false) + "\n")));
						self.AdditionalAssign(self.Cls(vps.m_ParentClsInd), vps.m_ParentClsInd);
					}
					self.m_Stat.m_LitsRemovedByConfSubsumption += 1;
				}

				self.m_VarsParentSubsumed.clear();
			}

			*contradictionInfo = self.BCP();
			self.ClsDeletionDecayActivity();
		}
	}

	// For marking used assumptions, but can also be used for any other purpose
	pub(crate) fn MarkDecisionsInConeAsVisited(&mut self, triggeringLit: TULit)
	{
		debug_assert!(self.IsAssigned(triggeringLit));

		self.MarkVisited(triggeringLit);

		if self.IsAssignedDec(triggeringLit)
		{
			return;
		}

		let mut v: TUVar = GetVar(triggeringLit);
		while v != BadUVar && self.GetAssignedDecLevelVar(v) != 0
		{
			if self.IsVisitedVar(v) && !self.IsAssignedDecVar(v)
			{
				let cls = self.GetAssignedNonDecParentSpanVar(v);
				for clsLit in cls
				{
					self.MarkVisited(clsLit);
				}
				if v != GetVar(triggeringLit)
				{
					self.m_AssignmentInfo[v as usize].set_m_Visit(false);
				}
			}
			v = self.m_VarInfo[v as usize].m_TrailPrev;
		}

		let ai = &self.m_AssignmentInfo;
		self.m_VisitedVars.erase_if_may_reorder(|v| !ai[v as usize].m_Visit(), 0);

		debug_assert!(self.IsVisitedConsistent());
	}
}
