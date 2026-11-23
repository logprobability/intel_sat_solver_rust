// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use std::collections::HashSet;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn SwapWatch(&mut self, clsInd: Ind, watchInd: bool, newWatchIt: usize)
	{
		let cls = self.Cls(clsInd);
		let cls1WlInd = self.WLGetLongWatchInd(cls.get(watchInd as usize), clsInd);
		debug_assert!(cls1WlInd != usize::MAX);
		self.WLRemoveLongWatch(cls.get(watchInd as usize), cls1WlInd);
		cls.swap(watchInd as usize, newWatchIt);
		self.WLAddLongWatch(cls.get(watchInd as usize), cls.get(!watchInd as usize), clsInd);
	}

	pub(crate) fn SwapCurrWatch(&mut self, l: TULit, newWatchIt: usize, clsInd: Ind, cls: &mut CCls<Ind, Compress>, currLongWatchInd: &mut usize, currLongWatchPtr: &mut *mut TULit, wi: *mut TWatchInfo<Ind>)
	{
		// Remove the current watch and adjust the indices, since removal moves the last watch into the current location
		self.WLRemoveLongWatch(Negate(l), *currLongWatchInd);
		*currLongWatchInd = currLongWatchInd.wrapping_sub(1);
		*currLongWatchPtr = currLongWatchPtr.wrapping_sub(TWatchInfo::<Ind>::BinsInLong);

		// Swap l and the visited literal in the clause and add watch to the visited literal
		cls.swap(0, newWatchIt);
		self.WLAddLongWatch(cls.get(0), cls.get(1), clsInd);
		// The line below is required to support the (very rare) occasions  of WLB's realloc actually moving m_W in WLAddLongWatch
		// Using get_ptr_no_assert, since WLAddLongWatch might reallocate m_W, which might cause,
		// when there are no long watches in wi, wi.m_WBInd to be higher than the capacity of m_W, which causes a failure assertion
		*currLongWatchPtr = self.m_W.get_ptr_no_assert(unsafe { (*wi).m_WBInd.as_usize() }).wrapping_add(currLongWatchInd.wrapping_mul(TWatchInfo::<Ind>::BinsInLong));
	}

	pub(crate) fn FindBestWLCand(&mut self, cls: &mut CCls<Ind, Compress>, maxDecLevel: TUV) -> usize
	{
		let clsEnd = cls.size();
		let mut visitedLitIt = 2;
		while visitedLitIt != clsEnd && !self.UnassignedOrSatisfied(cls.get(visitedLitIt))
		{
			visitedLitIt += 1;
		}

		if visitedLitIt != clsEnd
		{
			return visitedLitIt;
		}

		let mut maxDecLevelClsSoFar: TUV = 0;
		let mut maxIt = 2;
		let mut it = 2;
		while it != clsEnd
		{
			let lDecLevel = self.GetAssignedDecLevel(cls.get(it));
			if lDecLevel >= maxDecLevel
			{
				return it;
			}
			if lDecLevel > maxDecLevelClsSoFar
			{
				maxDecLevelClsSoFar = lDecLevel;
				maxIt = it;
			}
			it += 1;
		}

		maxIt
	}

	pub(crate) fn BCPBacktrack(&mut self, decLevel: TUV, eraseDecLevel: bool)
	{
		// Only one literal of the highest decision level appears in the clause
		self.Backtrack(decLevel as TLit, true, false);

		// EraseLit
		#[inline(always)]
		fn EraseLit<Ind: TUInd, const Compress: bool>(topi: &CTopi<Ind, Compress>, l: TULit, decLevel: TUV, eraseDecLevel: bool) -> bool
		{
			!topi.IsAssigned(l) || (eraseDecLevel && topi.GetAssignedDecLevel(l) == decLevel)
		}

		// The closure needs the solver, while m_ToPropagate is being modified; it only touches the visited-related members,
		// so m_ToPropagate is moved out for the duration (moving the vector doesn't move its buffer)
		let mut toPropagate = std::mem::take(&mut self.m_ToPropagate);
		toPropagate.erase_if_may_reorder(
			|l: TULit|
			{
				debug_assert!(!self.IsAssigned(l) || self.GetAssignedDecLevel(l) <= decLevel);
				let eraseLit = EraseLit(self, l, decLevel, eraseDecLevel);
				if eraseLit
				{
					// Marking literals erased from toPropagate
					// for implementing ProcessDelayedImplication correctly (see the comments there)
					self.MarkVisited(l);
				}
				eraseLit
			},
			0,
		);
		self.m_ToPropagate = toPropagate;

		debug_assert!(self.m_CurrentlyPropagatedLit != BadULit);
		if EraseLit(self, self.m_CurrentlyPropagatedLit, decLevel, eraseDecLevel)
		{
			self.MarkVisited(self.m_CurrentlyPropagatedLit);
		}
	}

	// The NewContradiction lambda of BCP
	// Returns true iff BCP should stop propagating the current literal
	fn BCPNewContradiction(&mut self, mut newCi: TContradictionInfo<Ind>) -> bool
	{
		let propagatedDecLevel = self.GetAssignedDecLevel(self.m_CurrentlyPropagatedLit);
		// Assert that we indeed have a new contradiction,
		// where all the literals are falsified and the two first literals' decision level is higher than that of the rest
		debug_assert!(self.CiIsLegal(&mut newCi, false));

		debug_assert!(self.NV(2) || self.P(&("***** previous contradictions = ".to_string() + &self.CisString(self.m_Cis.get_span()) + "\n")));

		let newCiSpanFirst2Lits = self.CiGetSpan(&mut newCi, 2);
		let newCiSpanDecLevels: [TUV; 2] = [self.GetAssignedDecLevel(newCiSpanFirst2Lits[0]), self.GetAssignedDecLevel(newCiSpanFirst2Lits[1])];
		let maxDecLevelInContradictingCls = std::cmp::max(newCiSpanDecLevels[0], newCiSpanDecLevels[1]);

		if newCiSpanDecLevels[0] != newCiSpanDecLevels[1]
		{
			self.BCPBacktrack(maxDecLevelInContradictingCls - 1, false);
			if !self.m_Cis.empty()
			{
				// Clear the previous contradictions, if any, which must be unassigned after the backtracking
				debug_assert!((0..self.m_Cis.size()).all(|i|
				{
					let ci = unsafe { &mut *self.m_Cis.get_ptr(i) };
					!self.IsFalsified(self.CiGetSpanDebug(ci, TUV::MAX)[0]) && !self.IsFalsified(self.CiGetSpanDebug(ci, TUV::MAX)[1])
				}));
				self.m_Cis.clear();
			}
			debug_assert!(self.IsAssigned(newCiSpanFirst2Lits[0]) != self.IsAssigned(newCiSpanFirst2Lits[1]) || self.P(&("Failure: ".to_string() + &self.SLits(newCiSpanFirst2Lits, false) + "; trail: " + &self.STrail() + "\n")));
			debug_assert!(self.IsAssigned(newCiSpanFirst2Lits[0]) != self.IsAssigned(newCiSpanFirst2Lits[1]));
			let unassignedLit = if self.IsAssigned(newCiSpanFirst2Lits[0]) { newCiSpanFirst2Lits[1] } else { newCiSpanFirst2Lits[0] };
			let assignedLit = if self.IsAssigned(newCiSpanFirst2Lits[0]) { newCiSpanFirst2Lits[0] } else { newCiSpanFirst2Lits[1] };
			debug_assert!(self.IsFalsified(assignedLit));
			if !newCi.m_IsContradictionInBinaryCls && newCi.m_ParentClsInd != Self::BadClsInd
			{
				self.WLSetCached(assignedLit, newCi.m_ParentClsInd, unassignedLit);
			}
			self.Assign(unassignedLit, if newCi.m_IsContradictionInBinaryCls { Self::BadClsInd } else { newCi.m_ParentClsInd }, assignedLit, self.GetAssignedDecLevel(assignedLit), true, false);
			debug_assert!(self.NV(2) || self.P(&("***** NewContradiction finished; stop-propagation = ".to_string() + &((propagatedDecLevel > maxDecLevelInContradictingCls - 1) as u8).to_string() + ": turned out to be a delayed implication; to-propagate: " + &self.SLits(self.m_ToPropagate.get_span(), false) + "; trail: " + &self.STrail() + "\n")));
			propagatedDecLevel > maxDecLevelInContradictingCls - 1
		}
		else
		{
			debug_assert!(self.m_Cis.empty() || (0..self.m_Cis.size()).all(|i|
			{
				let ci = unsafe { &mut *self.m_Cis.get_ptr(i) };
				maxDecLevelInContradictingCls <= self.GetAssignedDecLevel(self.CiGetSpan(ci, TUV::MAX)[0]) && maxDecLevelInContradictingCls <= self.GetAssignedDecLevel(self.CiGetSpan(ci, TUV::MAX)[1])
			}));

			self.BCPBacktrack(maxDecLevelInContradictingCls, true);

			// The closure needs the solver (read-only), while m_Cis is being modified, so m_Cis is moved out for the duration
			let mut cis = std::mem::take(&mut self.m_Cis);
			cis.erase_if_may_reorder(
				|mut ci: TContradictionInfo<Ind>|
				{
					debug_assert!(ci.IsContradiction());
					debug_assert!(self.IsAssigned(self.CiGetSpanDebug(&mut ci, TUV::MAX)[0]) == self.IsAssigned(self.CiGetSpanDebug(&mut ci, TUV::MAX)[1]));
					!self.IsAssigned(self.CiGetSpanDebug(&mut ci, 1)[0])
				},
				0,
			);
			self.m_Cis = cis;

			self.m_Cis.emplace_back(newCi);

			debug_assert!(self.NV(2) || self.P(&("***** NewContradiction finished; stop-propagation = ".to_string() + &((propagatedDecLevel > maxDecLevelInContradictingCls - 1) as u8).to_string() + ": NOT a delayed implication; to-propagate = " + &self.SLits(self.m_ToPropagate.get_span(), false) + "\n")));
			propagatedDecLevel >= maxDecLevelInContradictingCls
		}
	}

	pub(crate) fn BCP(&mut self) -> TContradictionInfo<Ind>
	{
		// Returns true iff BCP should stop propagating the current literal
		debug_assert!(!self.AssertConsistencyOn(1) || self.TrailAssertConsistency());
		debug_assert!(!self.AssertConsistencyOn(2) || self.WLAssertConsistency(false));

		let res = self.BCPBody();

		// CApplyFuncOnExitFromScope<> onExit
		self.ToPropagateClear();
		self.CleanVisited();
		self.m_CurrentlyPropagatedLit = BadULit;
		debug_assert!(!self.AssertConsistencyOn(1) || self.TrailAssertConsistency());
		debug_assert!(!self.AssertConsistencyOn(2) || self.WLAssertConsistency(self.m_Cis.empty()));
		self.m_Cis.clear();

		res
	}

	// The body of BCP, which is followed by the on-exit actions in BCP
	#[inline(always)]
	fn BCPBody(&mut self) -> TContradictionInfo<Ind>
	{
		self.m_Stat.m_BCPs += 1;

		debug_assert!(self.NV(2) || self.P(&("***** BCP started; #".to_string() + &self.m_Stat.m_BCPs.to_string() + "\n")));

		while !self.m_ToPropagate.empty()
		{
			let mut stopPropagating = false;
			self.m_CurrentlyPropagatedLit = self.ToPropagateBackAndPop();

			debug_assert!(self.NV(2) || self.P(&("Propagating literal ".to_string() + &self.SLit(self.m_CurrentlyPropagatedLit) + "\n")));

			// IsLStillPropagated
			debug_assert!(self.IsAssigned(self.m_CurrentlyPropagatedLit) && self.IsSatisfied(self.m_CurrentlyPropagatedLit));
			let v = GetVar(self.m_CurrentlyPropagatedLit);
			let vi: *const TVarInfo<Ind> = self.m_VarInfo.get_ptr(v as usize);
			let wi: *mut TWatchInfo<Ind> = self.m_Watches.get_ptr(Negate(self.m_CurrentlyPropagatedLit) as usize);
			unsafe
			{
				if (*wi).IsEmpty()
				{
					continue;
				}
				let lDecLevel = self.GetAssignedDecLevel(self.m_CurrentlyPropagatedLit);
				self.m_Stat.m_Implications += 1;
				if self.m_Params.m_ParamSimplify.get()
				{
					self.m_ImplicationsTillNextSimplify -= 1;
				}

				// Go over the binary watches first. We would like to pre-fetch the longs too for cache reasons, otherwise we would have used
				// TSpanTULit binWatches = b.get_span(wi.m_BInd + wi.GetLongEntries(), wi.m_BinaryWatches);
				let allWatches = self.m_W.get_ptr((*wi).m_WBInd.as_usize());
				let mut binWatches = TSpanTULit::new(allWatches.wrapping_add((*wi).GetLongEntries()), (*wi).m_BinaryWatches.as_usize());

				// Have to use an old-fashioned index-based for loop, since binWatches might change inside the loop because of reallocation
				let mut otherWatchI: usize = 0;
				while otherWatchI < binWatches.size()
				{
					let otherWatch = binWatches.get(otherWatchI);

					let isOtherWatchAssigned = self.IsAssigned(otherWatch);
					let isOtherWatchNegated = self.IsAssignedNegated(otherWatch);

					debug_assert!(self.NV(2) || self.P(&("Visiting binary clause ".to_string() + &self.SLit(otherWatch) + "\n")));

					if !isOtherWatchAssigned
					{
						// Imply otherWatch at the decision level of l
						self.Assign(otherWatch, Self::BadClsInd, Negate(self.m_CurrentlyPropagatedLit), (*vi).m_DecLevel, true, false);
					}
					else if isOtherWatchNegated
					{
						// Contradiction
						stopPropagating = self.BCPNewContradiction(TContradictionInfo::from_bin([Negate(self.m_CurrentlyPropagatedLit), otherWatch]));
						if stopPropagating
						{
							break;
						}
					}
					else
					{
						// Otherwise (that is, if both if conditions above do not hold), the clause is satisfied
						if lDecLevel < self.m_DecLevel && self.GetAssignedDecLevel(otherWatch) > lDecLevel
						{
							// If the other watch is satisfied at a level higher than the current unsatisfied literal l, we have a delayed implication
							stopPropagating = self.ProcessDelayedImplication(otherWatch, Negate(self.m_CurrentlyPropagatedLit), Self::BadClsInd);
							if stopPropagating
							{
								break;
							}
							// ProcessDelayedImplication might realloc, hence updating binWatches
							binWatches = TSpanTULit::new(self.m_W.get_ptr((*wi).m_WBInd.as_usize()).wrapping_add((*wi).GetLongEntries()), (*wi).m_BinaryWatches.as_usize());
							if self.m_CurrPropWatchModifiedDuringProcessDelayedImplication
							{
								otherWatchI = usize::MAX;
							}
						}
					}
					otherWatchI = otherWatchI.wrapping_add(1);
				}

				// Go over the long watches
				let mut currLongWatchInd: usize = 0;
				let mut currLongWatchPtr: *mut TULit = self.m_W.get_ptr((*wi).m_WBInd.as_usize());
				while !stopPropagating && currLongWatchInd < (*wi).m_LongWatches.as_usize()
				{
					// The labeled block stands for the body of the C++ for loop: breaking out of it is the C++ continue
					'currLongWatch:
					{
						// TULit& cachedLit = *currLongWatchPtr;

						debug_assert!(self.NV(2) || self.P(&("BCP: visiting long clause ".to_string() + &HexStrInd(Ind::read(currLongWatchPtr.add(1))) + ": cached " + &self.SLit(*currLongWatchPtr) + "; clause: " + &self.SLits(self.Cls(Ind::read(currLongWatchPtr.add(1))), false) + "\n")));

						if self.IsSatisfied(*currLongWatchPtr) && self.GetAssignedDecLevel(*currLongWatchPtr) <= lDecLevel
						{
							// The cached literal is satisfied at decision level not higher than the current watch, we can continue without visiting the clause!
							break 'currLongWatch;
						}

						// Fetching the clause
						let clsInd = Ind::read(currLongWatchPtr.add(1));

						let mut cls = self.Cls(clsInd);

						debug_assert!(!self.ClsChunkDeleted(clsInd));
						debug_assert!(Compress || clsInd >= self.m_FirstLearntClsInd || !self.ClsGetIsLearnt(clsInd));

						// Making sure, our literal is the first one in the clause
						if cls.get(1) == Negate(self.m_CurrentlyPropagatedLit)
						{
							cls.swap(0, 1);
						}
						debug_assert!(cls.get(0) == Negate(self.m_CurrentlyPropagatedLit));
						let otherWatch = cls.get(1);
						let isOtherWatchSatisfied = self.IsSatisfied(otherWatch);

						// Update the cached literal to the second satisfied watch
						// If the current watch is removed, this operation doesn't matter
						// However, it is essential for correctness of delayed implications processing,
						// if our literal turns out to be the implying literal for the 2nd watch
						*currLongWatchPtr = otherWatch;

						// Check the other watch
						if isOtherWatchSatisfied
						{
							let otherWatchDecLevel = self.GetAssignedDecLevel(cls.get(1));
							if otherWatchDecLevel <= lDecLevel
							{
								// The decision level of the other watch is not higher than that of l, so, fortunately, no chance of a delayed implication
								// Continue to the next clause
								break 'currLongWatch;
							}
						}

						// Going over the rest of the clause (that is, skipping the watches) to find the best WL candidate to swap with l
						let bestWLCandIt = self.FindBestWLCand(&mut cls, self.m_DecLevel);
						let bestWLCandLit = cls.get(bestWLCandIt);
						let bestWLCandAssigned = self.IsAssigned(bestWLCandLit);
						let bestWLCandSatisfied = self.IsSatisfied(bestWLCandLit);
						let bestWLUnassignedOrSatisfied = (!bestWLCandAssigned) | bestWLCandSatisfied;

						if bestWLUnassignedOrSatisfied || (lDecLevel < self.m_DecLevel && self.GetAssignedDecLevel(bestWLCandLit) > lDecLevel)
						{
							// If the candidate is unassigned, satisfied or has a greater decision level, swap it with the current watch
							self.SwapCurrWatch(self.m_CurrentlyPropagatedLit, bestWLCandIt, clsInd, &mut cls, &mut currLongWatchInd, &mut currLongWatchPtr, wi);
							if self.IsUnrecoverable()
							{
								return TContradictionInfo::new();
							}

							if bestWLUnassignedOrSatisfied
							{
								// The candidate satisfied or it's unassigned --> we're done with this clause
								break 'currLongWatch;
							}
						}

						if self.IsFalsified(cls.get(0)) && self.IsFalsified(cls.get(1))
						{
							// Contradiction!

							// There might be a case, when a literal in the clause is assigned after the 2nd watch w.r.t decision levels,
							// in which case we must swap them to prevent missed implications.
							// An illustrating example:
							// (1) We have a conflict at decision level 20
							// (2) The new conflict clause C's highest level (excluding the new 1UIP luip) is 7
							// (3) We chronologically backtrack to 19 and run BCP, which implied luip at level 7
							// (4) As a result, a literal l10 of dec. level 10 is implied in some clause and a literal l15 of dec. level 15 is implied at another clause
							// (5) A clause D becomes unsatisfied at decision level 15 because of the two above implications.
							// (6) D also has two non-WL literals, l17 and l18, assigned at levels 17 and 18, respectively
							// (7) Assume, the contradicting clause looks like this [l10, l15, l17, l18] and the contradiction is discovered when implying l10
							// (8) The code above will swap l10 and l18, so we get [l18, l15, l17, l10]
							// (9) Now we need to swap l15 and l17, since, otherwise, backtracking to decision level 16 and assigning l17 would result in a missed implication
							let cls1DecLevel = self.GetAssignedDecLevel(cls.get(1));
							if cls1DecLevel < self.GetAssignedDecLevel(cls.get(0))
							{
								let maxNonWLDecLevelIt = self.GetAssignedLitsHighestDecLevelIt(cls, 2);
								if cls1DecLevel < self.GetAssignedDecLevel(cls.get(maxNonWLDecLevelIt))
								{
									self.SwapWatch(clsInd, true, maxNonWLDecLevelIt);
									if self.IsUnrecoverable()
									{
										return TContradictionInfo::new();
									}
									// The line below is required to support the (very rare) occasions of b's realloc actually moving b in WLAddLongWatch
									currLongWatchPtr = self.m_W.get_ptr_no_assert((*wi).m_WBInd.as_usize()).wrapping_add(currLongWatchInd.wrapping_mul(TWatchInfo::<Ind>::BinsInLong));
								}
							}

							stopPropagating = self.BCPNewContradiction(TContradictionInfo::from_parent(clsInd));
						}
						else if self.IsFalsified(cls.get(0)) && self.IsSatisfied(cls.get(1)) && self.GetAssignedDecLevel(cls.get(1)) > self.GetAssignedDecLevel(cls.get(0))
						// Note that it is possible that the other watch is satisfied, the other watch's dl is greater than that of l,
						// but it's not a delayed implication any longer, since we swapped cls[0] with another falsified literal having a higher decision level
						{
							// Delayed implication
							stopPropagating = self.ProcessDelayedImplication(cls.get(1), cls.get(0), clsInd);
							if !stopPropagating && self.m_CurrPropWatchModifiedDuringProcessDelayedImplication
							{
								currLongWatchInd = usize::MAX;
								currLongWatchPtr = self.m_W.get_ptr_no_assert((*wi).m_WBInd.as_usize()).wrapping_sub(TWatchInfo::<Ind>::BinsInLong);
							}
							else
							{
								// The line below is required to support the (very rare) occasions  of b's realloc in ProcessDelayedImplication actually moving b in WLAddLongWatch
								currLongWatchPtr = self.m_W.get_ptr_no_assert((*wi).m_WBInd.as_usize()).wrapping_add(currLongWatchInd.wrapping_mul(TWatchInfo::<Ind>::BinsInLong));
							}
						}
						else if self.IsFalsified(cls.get(0)) && !self.IsAssigned(cls.get(1))
						{
							// The clause is unit
							debug_assert!((2..cls.size()).all(|i| self.IsFalsified(cls.get(i))));
							// Imply the other watch now
							self.Assign(cls.get(1), clsInd, cls.get(0), self.GetAssignedDecLevel(cls.get(0)), true, false);
						}
						else
						{
							// If we are here, it could have been a delayed implication candidate, but our watch was swapped with another one of a higher decision level
							debug_assert!((self.IsFalsified(cls.get(0)) && self.IsSatisfied(cls.get(1)) && self.GetAssignedDecLevel(cls.get(1)) <= self.GetAssignedDecLevel(cls.get(0))) || self.P(&("Failure: ".to_string() + &self.SLits(cls, false))));
							debug_assert!(self.IsFalsified(cls.get(0)) && self.IsSatisfied(cls.get(1)) && self.GetAssignedDecLevel(cls.get(1)) <= self.GetAssignedDecLevel(cls.get(0)));
						}
					}
					currLongWatchInd = currLongWatchInd.wrapping_add(1);
					currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
				}
			}
		}

		if self.m_Cis.empty()
		{
			TContradictionInfo::new()
		}
		else
		{
			let cisSpan = self.m_Cis.get_span();
			if cisSpan.size() == 1 || self.m_Params.m_ParamBestContradictionStrat.get() == 2
			{
				return self.m_Cis[0];
			}

			if self.m_Params.m_ParamBestContradictionStrat.get() == 3
			{
				return self.m_Cis.back();
			}

			if self.m_Params.m_ParamBestContradictionStrat.get() == 0
			{
				// min_element(cisSpan.begin(), cisSpan.end(), CiGetSize)
				let mut smallest = 0;
				let mut smallestVal = self.CiGetSize(&cisSpan[0]);
				for first in 1..cisSpan.size()
				{
					let currentVal = self.CiGetSize(&cisSpan[first]);
					if currentVal < smallestVal
					{
						smallest = first;
						smallestVal = currentVal;
					}
				}
				cisSpan.get(smallest)
			}
			else
			{
				debug_assert!(self.m_Params.m_ParamBestContradictionStrat.get() == 1);
				// min_element(cisSpan.begin(), cisSpan.end(), GetGlueAndMarkCurrDecLevels(CiGetSpan(ci)))
				let mut smallest = 0;
				let ciSpan = self.CiGetSpan(unsafe { &mut *cisSpan.ptr() }, TUV::MAX);
				let mut smallestVal = self.GetGlueAndMarkCurrDecLevels(ciSpan);
				for first in 1..cisSpan.size()
				{
					let ciSpan = self.CiGetSpan(unsafe { &mut *cisSpan.ptr().add(first) }, TUV::MAX);
					let currentVal = self.GetGlueAndMarkCurrDecLevels(ciSpan);
					if currentVal < smallestVal
					{
						smallest = first;
						smallestVal = currentVal;
					}
				}
				cisSpan.get(smallest)
			}
		}
	}

	// The DisEmplaceBack lambda of ProcessDelayedImplication, where the statistics to increment is
	// m_Stat.m_DelayedImplicationsTrigerring, if isTrigerring holds, and m_Stat.m_DelayedImplicationsPropagated, otherwise
	#[inline(always)]
	fn DisEmplaceBack(&mut self, diL: TULit, otherWatch: TULit, parentClsInd: Ind, isTrigerring: bool)
	{
		debug_assert!(self.NV(2) || self.P(&("+++++++++++++++ ProcessDelayedImplication adding new literal: diL=".to_string() +
			&self.SLit(diL) + "; otherWatch = " + &(if otherWatch == BadULit { "BAD".to_string() } else { self.SLit(otherWatch) }) + "; parent = " + &(if parentClsInd == Self::BadClsInd { "BAD".to_string() } else { self.SLits(self.Cls(parentClsInd), false) }) + "\n")));

		debug_assert!(self.IsSatisfied(diL) && ((otherWatch == BadULit && self.GetAssignedDecLevel(diL) > 0) || (self.IsFalsified(otherWatch) && self.GetAssignedDecLevel(diL) > self.GetAssignedDecLevel(otherWatch))));

		self.m_Dis.emplace_back(TDelImpl::new(diL, otherWatch, parentClsInd));
		if isTrigerring
		{
			self.m_Stat.m_DelayedImplicationsTrigerring += 1;
		}
		else
		{
			self.m_Stat.m_DelayedImplicationsPropagated += 1;
		}
	}

	// The SwapWatchWithMaxNonWLIfRequired lambda of ProcessDelayedImplication
	#[inline(always)]
	fn SwapWatchWithMaxNonWLIfRequired(&mut self, ciParentClsInd: Ind, ciSpan: TSpanTULit, watchInd: bool)
	{
		let cls = self.Cls(ciParentClsInd);
		let maxNonWLDecLevelIt = self.GetAssignedLitsHighestDecLevelIt(cls, 2);
		let litMaxDecLevel = cls.get(maxNonWLDecLevelIt);
		let maxDecLevel = self.GetAssignedDecLevel(litMaxDecLevel);

		if maxDecLevel > self.GetAssignedDecLevel(ciSpan[watchInd as usize])
		{
			if cls.get(watchInd as usize) == Negate(self.m_CurrentlyPropagatedLit)
			{
				self.m_CurrPropWatchModifiedDuringProcessDelayedImplication = true;
			}
			self.SwapWatch(ciParentClsInd, watchInd, maxNonWLDecLevelIt);
			if cls.get(watchInd as usize) == Negate(self.m_CurrentlyPropagatedLit)
			{
				self.m_CurrPropWatchModifiedDuringProcessDelayedImplication = true;
			}
		}
	}

	// Process a new delayed implication in-place
	// Returns true iff BCP should stop propagating the current literal
	// The cis parameter of the C++ version is always m_Cis
	pub(crate) fn ProcessDelayedImplication(&mut self, mut diL: TULit, mut otherWatch: TULit, mut parentClsInd: Ind) -> bool
	{
		let mut decLevelsRecalcBestScore: HashSet<TUV> = HashSet::new();
		let initDl = self.m_DecLevel;
		self.m_CurrPropWatchModifiedDuringProcessDelayedImplication = false;
		let decLevelStart = self.m_DecLevel;

		let res = self.ProcessDelayedImplicationBody(&mut diL, &mut otherWatch, &mut parentClsInd, &mut decLevelsRecalcBestScore, initDl);

		// CApplyFuncOnExitFromScope<> onExit
		debug_assert!(self.NV(2) || self.P(&("************************ ProcessDelayedImplication finish: l=".to_string() +
			&self.SLit(diL) + "; otherWatch = " + &(if otherWatch == BadULit { "BAD".to_string() } else { self.SLit(otherWatch) }) + "; parent = " + &(if parentClsInd == Self::BadClsInd { "BAD".to_string() } else { self.SLits(self.Cls(parentClsInd), false) }) + "\n" + &self.STrail() + "\n")));
		//assert(m_ParamAssertConsistency < 2 || m_Stat.m_Conflicts < (uint64_t)m_ParamAssertConsistencyStartConf || WLAssertConsistency(false));
		debug_assert!(!self.AssertConsistencyOn(1) || self.TrailAssertConsistency());
		self.m_Dis.clear();
		debug_assert!(decLevelStart == self.m_DecLevel || self.GetAssignedDecLevelVar(self.m_TrailEnd) != decLevelStart);
		let _ = decLevelStart;
		if self.m_CurrCustomBtStrat > 0 && !decLevelsRecalcBestScore.is_empty()
		{
			for dl in decLevelsRecalcBestScore
			{
				if dl <= self.m_DecLevel && !self.DecLevelIsCollapsed(dl)
				{
					self.m_BestScorePerDecLevel[dl as usize] = self.CalcMaxDecLevelScore(dl);
				}
			}
		}

		res
	}

	// The body of ProcessDelayedImplication, which is followed by the on-exit actions in ProcessDelayedImplication
	// diL, otherWatch and parentClsInd are the (modified in the body) parameters of ProcessDelayedImplication, which are printed out on exit
	#[inline(always)]
	fn ProcessDelayedImplicationBody(&mut self, diLRef: &mut TULit, otherWatchRef: &mut TULit, parentClsIndRef: &mut Ind, decLevelsRecalcBestScore: &mut HashSet<TUV>, initDl: TUV) -> bool
	{
		let mut diL = *diLRef;
		let mut otherWatch = *otherWatchRef;
		let mut parentClsInd = *parentClsIndRef;

		// m_CurrentlyPropagatedLit can be BadULit only if the delayed implication emerged in a user-added clause
		let propagatedDecLevel: TUV = if self.m_CurrentlyPropagatedLit == BadULit { BadUVar } else { self.GetAssignedDecLevel(self.m_CurrentlyPropagatedLit) };
		let cisMaxDecLevel: TUV = if self.m_Cis.empty() { 0 } else { self.GetAssignedDecLevel(self.CiGetSpan(unsafe { &mut *self.m_Cis.get_ptr(0) }, 1)[0]) };
		debug_assert!(self.m_Cis.empty() || (0..self.m_Cis.size()).all(|i|
		{
			let ci = unsafe { &mut *self.m_Cis.get_ptr(i) };
			self.CiIsLegal(ci, true) && self.GetAssignedDecLevel(self.CiGetSpanDebug(ci, TUV::MAX)[0]) == cisMaxDecLevel && self.GetAssignedDecLevel(self.CiGetSpanDebug(ci, TUV::MAX)[1]) == cisMaxDecLevel
		}));

		debug_assert!(self.NV(2) || self.P(&("***** ProcessDelayedImplication start: diL=".to_string() +
			&self.SLit(diL) + "; otherWatch = " + &(if otherWatch == BadULit { "BAD".to_string() } else { self.SLit(otherWatch) }) + "; parent = " + &(if parentClsInd == Self::BadClsInd { "BAD".to_string() } else { self.SLits(self.Cls(parentClsInd), false) }) + "\n" + &self.STrail() + "\n")));

		debug_assert!(!self.AssertConsistencyOn(1) || self.TrailAssertConsistency());

		debug_assert!(self.IsSatisfied(diL) && ((otherWatch == BadULit && self.GetAssignedDecLevel(diL) > 0) || (self.IsFalsified(otherWatch) && self.GetAssignedDecLevel(diL) > self.GetAssignedDecLevel(otherWatch))));

		self.DisEmplaceBack(diL, otherWatch, parentClsInd, true);
		if self.IsUnrecoverable()
		{
			return false;
		}

		// The macro keeps the caller's view of the modified parameters up-to-date for every exit from the body
		macro_rules! Return
		{
			($res:expr) =>
			{{
				*diLRef = diL;
				*otherWatchRef = otherWatch;
				*parentClsIndRef = parentClsInd;
				return $res;
			}};
		}

		while !self.m_Dis.empty()
		{
			diL = self.m_Dis.back().m_L;
			parentClsInd = self.m_Dis.back().m_ParentClsInd;
			otherWatch = self.m_Dis.back().m_OtherWatch;
			// The C++ version invokes ConstClsSpan for a bad parent too, but the result is then unused (and there are no side-effects)
			let cls: TSpanTULit = if parentClsInd == Self::BadClsInd { TSpanTULit::new(&mut otherWatch as *mut TULit, 1) } else { self.ConstClsSpan(parentClsInd, TUV::MAX) };

			if otherWatch != BadULit && parentClsInd != Self::BadClsInd && otherWatch != cls[0] && otherWatch != cls[1]
			{
				// This can happen, if l had already been processed during this very invocation of our function and the watch was changed
				debug_assert!(diL == cls[0] || diL == cls[1]);
				otherWatch = cls[(diL == cls[0]) as usize];
			}

			self.m_Dis.pop_back();

			let oldDecLevel = self.GetAssignedDecLevel(diL);
			let newDecLevel: TUV = if otherWatch == BadULit { 0 } else { self.GetAssignedDecLevel(otherWatch) };

			debug_assert!(self.NV(2) || self.P(&("**** ProcessDelayedImplication processing new literal: l=".to_string() +
				&self.SLit(diL) + "; otherWatch = " + &(if otherWatch == BadULit { "BAD".to_string() } else { self.SLit(otherWatch) }) + "; parent = " + &(if parentClsInd == Self::BadClsInd { "BAD".to_string() } else { self.SLits(cls, false) }) + "\n")));

			debug_assert!(self.IsSatisfied(diL) && (otherWatch == BadULit || self.IsFalsified(otherWatch)));

			if oldDecLevel <= newDecLevel
			{
				// This can happen, if l has already been processed during this very invocation of our function
				continue;
			}

			let decisionLevelCollapse = self.IsAssignedDec(diL);

			self.Unassign(diL);
			if decisionLevelCollapse
			{
				self.m_Stat.m_DelayedImplicationDecLevelsCollapsed += 1;
				if oldDecLevel == self.m_DecLevel
				{
					// If l is the only (decision) literal at its decision level, the level will disappear after unassigning l
					// and propagating the unassignment below (in this invocation of the function)
					self.m_DecLevel -= 1;
				}
			}
			else if self.m_CurrCustomBtStrat > 0 && self.m_BestScorePerDecLevel[oldDecLevel as usize] == self.m_VsidsHeap.get_var_score(GetVar(diL))
			{
				decLevelsRecalcBestScore.insert(oldDecLevel);
			}

			self.Assign(diL, parentClsInd, otherWatch, newDecLevel, false, false);
			if self.IsVisited(diL)
			{
				// If the literal was added to toPropagate, but its propagation hasn't been completed during the current BCP,
				// we must re-propagate it in order not to miss implications
				// This might happen, e.g., if we discover a conflict at decision level 10 and remove l from toPropagate, since it was assigned at dec. level 10,
				// but then if the conflicting clause turns into an implication in this function, we need to re-propagate l
				self.ToPropagatePushBack(diL);
			}

			let wi: *mut TWatchInfo<Ind> = self.m_Watches.get_ptr(Negate(diL) as usize);
			unsafe
			{
				if (*wi).IsEmpty()
				{
					continue;
				}
				let allWatches = self.m_W.get_ptr((*wi).m_WBInd.as_usize());
				let binWatches = TSpanTULit::new(allWatches.wrapping_add((*wi).GetLongEntries()), (*wi).m_BinaryWatches.as_usize());

				for otherWatchLocal in binWatches
				{
					let isOtherWatchSatisfied = self.IsSatisfied(otherWatchLocal);
					if isOtherWatchSatisfied && self.GetAssignedDecLevel(otherWatchLocal) > newDecLevel
					{
						self.DisEmplaceBack(otherWatchLocal, Negate(diL), Self::BadClsInd, false);
						if self.IsUnrecoverable()
						{
							Return!(false);
						}
					}
				}

				let mut currLongWatchInd: usize = 0;
				let mut currLongWatchPtr: *mut TULit = self.m_W.get_ptr((*wi).m_WBInd.as_usize());
				while currLongWatchInd < (*wi).m_LongWatches.as_usize()
				{
					let mut cachedLit = *currLongWatchPtr;
					let clsInd = Ind::read(currLongWatchPtr.add(1));

					debug_assert!(self.NV(2) || self.P(&("ProcessDelayedImplication: visiting long clause ".to_string() + &HexStrInd(Ind::read(currLongWatchPtr.add(1))) + ": cached " + &self.SLit(cachedLit) + "; clause: " + &self.SLits(self.Cls(Ind::read(currLongWatchPtr.add(1))), false) + "\n")));

					// Some extra-care must be taken if the literal hasn't been fully propagated
					// There might be a satisfied literal, which is not yet cached
					if self.IsVisited(diL) && !self.IsSatisfied(cachedLit)
					{
						let clsLocal = self.Cls(clsInd);
						debug_assert!(!self.ClsChunkDeleted(clsInd));
						let clsLocalEnd = clsLocal.size();
						let mut itSat = 0;
						while itSat != clsLocalEnd &&
							!{
								let clsLit = clsLocal.get(itSat);
								self.IsSatisfied(clsLit) && self.GetAssignedDecLevel(clsLit) > newDecLevel
							}
						{
							itSat += 1;
						}
						if itSat != clsLocalEnd
						{
							*currLongWatchPtr = clsLocal.get(itSat);
							cachedLit = *currLongWatchPtr;
							if itSat >= 2
							{
								debug_assert!(clsLocal.get(0) == Negate(diL) || clsLocal.get(1) == Negate(diL));
								let watchInd = clsLocal.get(0) == Negate(diL);
								if clsLocal.get(watchInd as usize) == Negate(self.m_CurrentlyPropagatedLit)
								{
									self.m_CurrPropWatchModifiedDuringProcessDelayedImplication = true;
								}
								self.SwapWatch(clsInd, watchInd, itSat);

								if self.IsUnrecoverable()
								{
									Return!(false);
								}
								// The line below is required to support the (very rare) occasions  of b's realloc actually moving b in WLAddLongWatch
								currLongWatchPtr = self.m_W.get_ptr((*wi).m_WBInd.as_usize()).wrapping_add(currLongWatchInd.wrapping_mul(TWatchInfo::<Ind>::BinsInLong));
							}
						}
					}
					// It is guaranteed that if the clause is satisfied with 1 literal and the rest are falsified, that literal must be cached by the falsified watch,
					// unless BCP hasn't been completed for l, which is handled by the code above
					if self.IsSatisfied(cachedLit)
					{
						let mut clsLocal = self.Cls(clsInd);
						debug_assert!(!self.ClsChunkDeleted(clsInd));
						// Making sure, l is the first literal in the clause
						if clsLocal.get(1) == Negate(diL)
						{
							clsLocal.swap(0, 1);
						}
						debug_assert!(clsLocal.get(0) == Negate(diL));

						// Going over the rest of the clause (that is, skipping the watches) to find the best WL candidate to swap with l
						let bestWLCandIt = self.FindBestWLCand(&mut clsLocal, initDl);
						let bestWLCandLit = clsLocal.get(bestWLCandIt);

						if !self.IsFalsified(bestWLCandLit)
						{
							// If the best-of-the-rest literal is not falsified, we just swap it with the current watch without further complications
							self.SwapCurrWatch(diL, bestWLCandIt, clsInd, &mut clsLocal, &mut currLongWatchInd, &mut currLongWatchPtr, wi);
						}
						else
						{
							debug_assert!(self.IsFalsified(bestWLCandLit));

							let litMaxDecLevel = clsLocal.get(bestWLCandIt);
							let maxDecLevel = self.GetAssignedDecLevel(litMaxDecLevel);
							debug_assert!(maxDecLevel == self.GetAssignedDecLevel(clsLocal.get(self.GetAssignedLitsHighestDecLevelIt(clsLocal, 2))));
							let cachedDecLevel = self.GetAssignedDecLevel(cachedLit);

							if maxDecLevel <= newDecLevel
							{
								// If the maximal decision level of the rest is not greater the new l's level newDecLevel, then
								// l stays as the watch, but cachedLit should now implied at newDecLevel, if its current level is higher
								if newDecLevel < cachedDecLevel
								{
									self.DisEmplaceBack(cachedLit, Negate(diL), clsInd, false);
								}
								if self.IsUnrecoverable()
								{
									Return!(false);
								}
							}
							else
							{
								// If the maximal decision level of the rest is greater the new l's level newDecLevel, then
								// the maximal literal becomes the watch instead of l
								self.SwapCurrWatch(diL, bestWLCandIt, clsInd, &mut clsLocal, &mut currLongWatchInd, &mut currLongWatchPtr, wi);

								// We re-imply cachedLit at maxDecLevel, if maxDecLevel < cachedDecLevel
								if maxDecLevel < cachedDecLevel
								{
									self.DisEmplaceBack(cachedLit, litMaxDecLevel, clsInd, false);
									if self.IsUnrecoverable()
									{
										Return!(false);
									}
								}
							}
						}
					}

					currLongWatchInd = currLongWatchInd.wrapping_add(1);
					currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
				}
			}
		}

		if !self.m_Cis.empty()
		{
			// The  loop below:
			// (1) Update the contradicting clauses, if required
			// (2) Checks if there was any change in the decision levels
			// (3) If there was a change, manages the backtrack level and whether that level is contradictory
			let mut anyChange = false;
			let mut isBacktrackLevelContradictory = false;
			let mut backtrackLevel = TUV::MAX;

			let cisSpan = self.m_Cis.get_span();
			for i in 0..cisSpan.size()
			{
				let ci: &mut TContradictionInfo<Ind> = unsafe { &mut *cisSpan.ptr().add(i) };
				let ciSpan = self.CiGetSpan(ci, TUV::MAX);

				debug_assert!(self.NV(2) || self.P(&("Delayed implication contradiction handling start; clause: ".to_string() + &self.SLits(ciSpan, false) + "\n")));

				if !ci.m_IsContradictionInBinaryCls
				{
					self.SwapWatchWithMaxNonWLIfRequired(ci.m_ParentClsInd, ciSpan, false);
					if self.IsUnrecoverable()
					{
						Return!(false);
					}
					self.SwapWatchWithMaxNonWLIfRequired(ci.m_ParentClsInd, ciSpan, true);
					if self.IsUnrecoverable()
					{
						Return!(false);
					}
				}

				let currChanged = self.GetAssignedDecLevel(ciSpan[0]) != cisMaxDecLevel || self.GetAssignedDecLevel(ciSpan[1]) != cisMaxDecLevel;
				debug_assert!(self.NV(2) || self.P(&("Delayed implication contradiction handling end; clause: ".to_string() + &self.SLits(ciSpan, false) + "; changed? = " + &(currChanged as u8).to_string() + "\n")));

				if currChanged
				{
					anyChange = true;
					let currContradictory = self.GetAssignedDecLevel(ciSpan[0]) == self.GetAssignedDecLevel(ciSpan[1]);
					let currBacktrackLevel: TUV = if currContradictory { self.GetAssignedDecLevel(ciSpan[0]) } else { std::cmp::max(self.GetAssignedDecLevel(ciSpan[0]), self.GetAssignedDecLevel(ciSpan[1])) - 1 };

					if currBacktrackLevel < backtrackLevel
					{
						backtrackLevel = currBacktrackLevel;
						isBacktrackLevelContradictory = currContradictory;
					}
					else if currBacktrackLevel == backtrackLevel && !isBacktrackLevelContradictory && currContradictory
					{
						isBacktrackLevelContradictory = true;
					}
				}
			}

			if anyChange
			{
				self.BCPBacktrack(backtrackLevel, isBacktrackLevelContradictory);

				// Assign any units and mark for removal
				let cisSpanLocal = self.m_Cis.get_span();
				for i in 0..cisSpanLocal.size()
				{
					let ci: &mut TContradictionInfo<Ind> = unsafe { &mut *cisSpanLocal.ptr().add(i) };
					let ciSpan = self.CiGetSpan(ci, 2);

					// Note that, in a contradiction, one of the watches may have become satisfied after assigning a delayed implication for an earlier former contradiction
					// hence the condition IsAssigned(ciSpan[0]) != IsAssigned(ciSpan[1]) in the if below wouldn't have been sufficient
					if (self.IsFalsified(ciSpan[0]) && !self.IsAssigned(ciSpan[1])) || (self.IsFalsified(ciSpan[1]) && !self.IsAssigned(ciSpan[0]))
					{
						let unassignedLit = if self.IsAssigned(ciSpan[0]) { ciSpan[1] } else { ciSpan[0] };
						let assignedLit = if self.IsAssigned(ciSpan[0]) { ciSpan[0] } else { ciSpan[1] };
						debug_assert!(self.IsFalsified(assignedLit));
						if !ci.m_IsContradictionInBinaryCls && ci.m_ParentClsInd != Self::BadClsInd
						{
							// Update the cached literal in assignedLit to point to unassignedLit
							// Otherwise a *correctness* bug is possible, since the following condition, required for the correctness of ProcessDelayedImplication would not hold:
							// It is guaranteed that if the clause is satisfied with 1 literal and the rest are falsified, that literal must be cached by the falsified watch
							self.WLSetCached(assignedLit, ci.m_ParentClsInd, unassignedLit);
						}
						self.Assign(unassignedLit, if ci.m_IsContradictionInBinaryCls { Self::BadClsInd } else { ci.m_ParentClsInd }, assignedLit, self.GetAssignedDecLevel(assignedLit), true, false);
						// Marking the ci to be removed after this loop
						ci.m_IsContradiction = false;
					}
					else if !(self.IsFalsified(ciSpan[0]) && self.IsFalsified(ciSpan[1]))
					{
						// Marking the ci to be removed after this loop
						ci.m_IsContradiction = false;
					}
				}

				// Remove any non-contradictory ci's (marked as non-contradictory in the previous loop)
				self.m_Cis.erase_if_may_reorder(|ci: TContradictionInfo<Ind>| !ci.IsContradiction(), 0);

				Return!(if self.m_CurrentlyPropagatedLit == BadULit { false } else if isBacktrackLevelContradictory { propagatedDecLevel >= backtrackLevel } else { propagatedDecLevel > backtrackLevel });
			}
		}

		Return!(false);
	}
}
