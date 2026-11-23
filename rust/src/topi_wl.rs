// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use std::collections::{HashMap, HashSet};

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn WLBinaryWatchExists(&mut self, l: TULit, otherWatch: TULit) -> bool
	{
		debug_assert!((l as usize) < self.m_Watches.cap());
		debug_assert!((otherWatch as usize) < self.m_Watches.cap());
		let wi = &self.m_Watches[l as usize];
		if wi.m_BinaryWatches == Ind::ZERO
		{
			return false;
		}
		let binWatches = TSpanTULit::new(self.m_W.get_ptr(wi.m_WBInd.as_usize()).wrapping_add(wi.GetLongEntries()), wi.m_BinaryWatches.as_usize());
		binWatches.find(otherWatch) != binWatches.size()
	}

	pub(crate) fn WLRemoveBinaryWatch(&mut self, l: TULit, otherWatch: TULit)
	{
		debug_assert!(self.WLBinaryWatchExists(l, otherWatch));

		let wi = &mut self.m_Watches[l as usize];
		let binWatches = TSpanTULit::new(self.m_W.get_ptr(wi.m_WBInd.as_usize()).wrapping_add(wi.GetLongEntries()), wi.m_BinaryWatches.as_usize());
		let it = binWatches.find(otherWatch);
		binWatches.set(it, binWatches.back());

		debug_assert!(wi.m_BinaryWatches > Ind::ZERO);
		wi.m_BinaryWatches -= Ind::ONE;
	}

	pub(crate) fn WLAddBinaryWatch(&mut self, l: TULit, otherWatch: TULit)
	{
		debug_assert!((l as usize) < self.m_Watches.cap());
		debug_assert!((otherWatch as usize) < self.m_Watches.cap());

		// Prepare the watch arena for our literal
		let watchArena = self.WLPrepareArena(l, true, false);
		debug_assert!(!watchArena.is_null() || self.IsUnrecoverable());
		// || watchArena == nullptr was added because of a Klocwork warning. WLPrepareArena guarantees that IsUnrecoverable() holds if watchArena == nullptr.
		if self.IsUnrecoverable() || watchArena.is_null()
		{
			return;
		}

		// Add the binary watch
		unsafe { *watchArena.add(self.m_Watches[l as usize].GetUsedEntries()) = otherWatch };

		// Increase the number of binary watches in the watch-info
		self.m_Watches[l as usize].m_BinaryWatches += Ind::ONE;
	}

	pub(crate) fn WLGetLongWatchInd(&mut self, l: TULit, clsInd: Ind) -> usize
	{
		// Go over the long watches
		let wi = &self.m_Watches[l as usize];
		let longWatches = wi.m_LongWatches.as_usize();
		let mut currLongWatchInd: usize = 0;
		let mut currLongWatchPtr = self.m_W.get_ptr(wi.m_WBInd.as_usize());
		while currLongWatchInd < longWatches
		{
			let currClsInd = unsafe { Ind::read(currLongWatchPtr.add(1)) };
			if currClsInd == clsInd
			{
				return currLongWatchInd;
			}
			currLongWatchInd += 1;
			currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
		}

		usize::MAX
	}

	pub(crate) fn WLSetCached(&mut self, l: TULit, clsInd: Ind, cachedLit: TULit)
	{
		// Go over the long watches
		let wi = &self.m_Watches[l as usize];
		let longWatches = wi.m_LongWatches.as_usize();
		let mut currLongWatchInd: usize = 0;
		let mut currLongWatchPtr = self.m_W.get_ptr(wi.m_WBInd.as_usize());
		while currLongWatchInd < longWatches
		{
			let currClsInd = unsafe { Ind::read(currLongWatchPtr.add(1)) };
			if currClsInd == clsInd
			{
				unsafe { *currLongWatchPtr = cachedLit };
				return;
			}
			currLongWatchInd += 1;
			currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
		}

		debug_assert!(false);
	}

	pub(crate) fn WLReplaceInd(&mut self, l: TULit, clsInd: Ind, newClsInd: Ind)
	{
		// Go over the long watches
		let wi = &self.m_Watches[l as usize];
		let longWatches = wi.m_LongWatches.as_usize();
		let mut currLongWatchInd: usize = 0;
		let mut currLongWatchPtr = self.m_W.get_ptr(wi.m_WBInd.as_usize());
		while currLongWatchInd < longWatches
		{
			let currClsInd = unsafe { Ind::read(currLongWatchPtr.add(1)) };
			if currClsInd == clsInd
			{
				unsafe { newClsInd.write(currLongWatchPtr.add(1)) };
				return;
			}
			currLongWatchInd += 1;
			currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
		}

		debug_assert!(false);
	}

	pub(crate) fn WLRemoveLongWatch(&mut self, l: TULit, longWatchInd: usize)
	{
		debug_assert!((l as usize) < self.m_Watches.cap());

		let wi = &mut self.m_Watches[l as usize];
		debug_assert!(longWatchInd < wi.m_LongWatches.as_usize());
		let watchArena = self.m_W.get_ptr(wi.m_WBInd.as_usize());

		wi.m_LongWatches -= Ind::ONE;
		unsafe
		{
			if longWatchInd != wi.m_LongWatches.as_usize()
			{
				std::ptr::copy_nonoverlapping(watchArena.add(wi.GetLongEntries()), watchArena.add(wi.GetLongEntry(longWatchInd)), TWatchInfo::<Ind>::BinsInLong);
			}

			match wi.m_BinaryWatches.as_usize()
			{
				0 => {}
				1 =>
				{
					*watchArena.add(wi.GetLongEntries()) = *watchArena.add(wi.GetLongEntries() + TWatchInfo::<Ind>::BinsInLong);
				}
				_ =>
				{
					if TWatchInfo::<Ind>::BinsInLong <= 2
					{
						// Copy the binary watches from the end of the binary arena to the end of the updated long arena
						std::ptr::copy_nonoverlapping(watchArena.add(wi.GetUsedEntries()), watchArena.add(wi.GetLongEntries()), TWatchInfo::<Ind>::BinsInLong);
					}
					else if wi.m_BinaryWatches.as_usize() >= TWatchInfo::<Ind>::BinsInLong
					{
						std::ptr::copy_nonoverlapping(watchArena.add(wi.GetUsedEntries()), watchArena.add(wi.GetLongEntries()), TWatchInfo::<Ind>::BinsInLong);
					}
					else
					{
						std::ptr::copy_nonoverlapping(watchArena.add(wi.GetUsedEntries() + TWatchInfo::<Ind>::BinsInLong - wi.m_BinaryWatches.as_usize()), watchArena.add(wi.GetLongEntries()), wi.m_BinaryWatches.as_usize());
					}
				}
			}
		}
	}

	pub(crate) fn WLAddLongWatch(&mut self, l: TULit, inlinedLit: TULit, clsInd: Ind)
	{
		debug_assert!((l as usize) < self.m_Watches.cap());
		debug_assert!(Compress || clsInd.as_usize() < self.m_B.cap());

		// Prepare the watch arena for our literal
		let watchArena = self.WLPrepareArena(l, false, true);
		debug_assert!(!watchArena.is_null() || self.IsUnrecoverable());
		// || watchArena == nullptr was added because of a Klocwork warning. WLPrepareArena guarantees that IsUnrecoverable() holds if watchArena == nullptr.
		if self.IsUnrecoverable() || watchArena.is_null()
		{
			return;
		}

		// Add the long watch
		let wi = &mut self.m_Watches[l as usize];

		unsafe
		{
			// Free space by moving binary watches, if required
			match wi.m_BinaryWatches.as_usize()
			{
				0 => {}
				1 =>
				{
					*watchArena.add(wi.GetUsedEntries() + Ind::LitsInInd) = *watchArena.add(wi.GetLongEntries());
				}
				_ =>
				{
					if TWatchInfo::<Ind>::BinsInLong <= 2
					{
						std::ptr::copy_nonoverlapping(watchArena.add(wi.GetLongEntries()), watchArena.add(wi.GetUsedEntries()), TWatchInfo::<Ind>::BinsInLong);
					}
					else if wi.m_BinaryWatches.as_usize() >= TWatchInfo::<Ind>::BinsInLong
					{
						std::ptr::copy_nonoverlapping(watchArena.add(wi.GetLongEntries()), watchArena.add(wi.GetUsedEntries()), TWatchInfo::<Ind>::BinsInLong);
					}
					else
					{
						std::ptr::copy_nonoverlapping(watchArena.add(wi.GetLongEntries()), watchArena.add(wi.GetUsedEntries() + TWatchInfo::<Ind>::BinsInLong - wi.m_BinaryWatches.as_usize()), wi.m_BinaryWatches.as_usize());
					}
				}
			}

			// Insert the long watch
			let mut indBeyondLongWatches = wi.GetLongEntries();
			*watchArena.add(indBeyondLongWatches) = inlinedLit;
			indBeyondLongWatches += 1;
			clsInd.write(watchArena.add(indBeyondLongWatches));
		}

		// Increase the number of long watches in the watch-info
		wi.m_LongWatches += Ind::ONE;
	}

	// The AllocateNewArenaAndCopyOldArenaIfAny lambda of WLPrepareArena
	fn WLPrepareArenaAllocateNewArenaAndCopyOldArenaIfAny(&mut self, wi: *mut TWatchInfo<Ind>, sz: Ind) -> *mut TULit
	{
		let mut allocatedWordsRequired = self.m_WNext + sz;
		if allocatedWordsRequired < self.m_WNext || ((self.m_WWasted + self.m_WNext).as_u64() as f64 > self.m_WNext.as_u64() as f64 * self.m_Params.m_ParamMultWasteWatches.get())
		{
			self.CompressWLs();
			allocatedWordsRequired = self.m_WNext + sz;
			if allocatedWordsRequired < self.m_WNext
			{
				self.SetStatus(TToporStatus::STATUS_INDEX_TOO_NARROW, "WLPrepareArena: reached the end of the buffer".to_string());
				return std::ptr::null_mut();
			}
		}

		self.m_W.reserve_beyond_if_requried(allocatedWordsRequired.as_usize(), true);
		if self.m_W.uninitialized_or_erroneous()
		{
			self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "BReallocBeyond: couldn't reserve m_B".to_string());
			return std::ptr::null_mut();
		}

		unsafe
		{
			if !(*wi).IsEmpty()
			{
				self.m_W.memcpy(self.m_WNext.as_usize(), (*wi).m_WBInd.as_usize(), (*wi).GetUsedEntries());
				self.MarkWatchBufferChunkDeleted(&mut *wi);
			}
			(*wi).PointToNewArena(self.m_WNext, sz);
			self.m_WNext += (*wi).m_AllocatedEntries;
			self.m_W.get_ptr((self.m_WNext - (*wi).m_AllocatedEntries).as_usize())
		}
	}

	pub(crate) fn WLPrepareArena(&mut self, l: TULit, allowNewBinaryWatch: bool, allowNewLongWatch: bool) -> *mut TULit
	{
		let wi: *mut TWatchInfo<Ind> = self.m_Watches.get_ptr(l as usize);

		unsafe
		{
			let currRequiredEntries = Ind::from_usize(allowNewBinaryWatch as usize + (allowNewLongWatch as usize * TWatchInfo::<Ind>::BinsInLong));
			if (*wi).IsEmpty()
			{
				let initEntriesPerWL = Ind::from_usize(self.m_Params.m_ParamInitEntriesPerWL.get() as usize);
				return self.WLPrepareArenaAllocateNewArenaAndCopyOldArenaIfAny(wi, if initEntriesPerWL < currRequiredEntries { Ind::from_usize(currRequiredEntries.as_usize().next_power_of_two()) } else { initEntriesPerWL });
			}

			debug_assert!(!(*wi).IsEmpty());

			let usedEntries = Ind::from_usize((*wi).GetUsedEntries());
			let requiredEntries = usedEntries + currRequiredEntries;

			if requiredEntries <= (*wi).m_AllocatedEntries
			{
				return self.m_W.get_ptr((*wi).m_WBInd.as_usize());
			}

			if requiredEntries < usedEntries
			{
				// Overflow
				self.SetStatus(TToporStatus::STATUS_INDEX_TOO_NARROW, "The actual number of entries for one literal doesn't fit into the buffer".to_string());
				return std::ptr::null_mut();
			}

			// Re-allocation is required

			if (*wi).m_AllocatedEntries == TWatchInfo::<Ind>::MaxWatchInfoAlloc()
			{
				// Reached the maximum allocation!
				self.SetStatus(TToporStatus::STATUS_INDEX_TOO_NARROW, "The watch list for one literal doesn't fit into the buffer".to_string());
				return std::ptr::null_mut();
			}

			self.m_WWasted += (*wi).m_AllocatedEntries;
			self.WLPrepareArenaAllocateNewArenaAndCopyOldArenaIfAny(wi, Ind::from_u64((*wi).m_AllocatedEntries.as_u64() << 1))
		}
	}

	// The test of one binary clause, shared by WLAssertNoMissedImplications and WLAssertConsistency
	fn WLAssertBinaryClsNoMissedImplication(&self, cls: TSpanTULit)
	{
		if self.IsFalsified(cls[0]) && self.IsFalsified(cls[1])
		{
			stdout_println!("***ASSERTION-FAILURE FF-for-binary at {}\n{}", self.SLits(cls, false), self.STrail());
		}
		debug_assert!(!(self.IsFalsified(cls[0]) && self.IsFalsified(cls[1])));

		if !(self.IsSatisfied(cls[0]) || self.IsSatisfied(cls[1]))
		{
			stdout_println!("***ASSERTION-FAILURE one-falsified-no-satisfied-for-binary at {}\n{}", self.SLits(cls, false), self.STrail());
		}
		debug_assert!(self.IsSatisfied(cls[0]) || self.IsSatisfied(cls[1]));

		let satLit = cls[self.IsSatisfied(cls[1]) as usize];
		let falseLit = cls[self.IsFalsified(cls[1]) as usize];
		debug_assert!(satLit != falseLit);

		if !(self.GetAssignedDecLevel(satLit) <= self.GetAssignedDecLevel(falseLit))
		{
			stdout_println!("***ASSERTION-FAILURE one-falsified-satisfied-higherdl-for-binary at {}\n{}", self.SLits(cls, false), self.STrail());
		}
		debug_assert!(self.GetAssignedDecLevel(satLit) <= self.GetAssignedDecLevel(falseLit));
	}

	// The test of one long clause with a falsified watch, shared by WLAssertNoMissedImplications and WLAssertConsistency
	fn WLAssertLongClsNoMissedImplication(&self, cls: TSpanTULit)
	{
		let falseLit = cls[self.IsFalsified(cls[1]) as usize];
		let falseLitDecLevel = self.GetAssignedDecLevel(falseLit);
		let otherLit = cls[!self.IsFalsified(cls[1]) as usize];
		let otherLitDecLevel = if self.IsAssigned(otherLit) { self.GetAssignedDecLevel(otherLit) } else { BadUVar };

		// find_if(cls.begin() + 2, cls.end(), pred) != cls.end()
		let found = |pred: &dyn Fn(TULit) -> bool| -> bool
		{
			let mut it = 2;
			while it < cls.size() && !pred(cls[it])
			{
				it += 1;
			}
			it < cls.size()
		};

		if self.IsSatisfied(otherLit)
		{
			let itFound = found(&|l: TULit| self.IsSatisfied(l) && self.GetAssignedDecLevel(l) <= falseLitDecLevel);
			if !(otherLitDecLevel <= falseLitDecLevel || itFound)
			{
				stdout_println!("***ASSERTION-FAILURE FS at {}\n{}", self.SLits(cls, false), self.STrail());
			}
			debug_assert!(otherLitDecLevel <= falseLitDecLevel || itFound);
		}

		if !self.IsAssigned(otherLit)
		{
			let itFound = found(&|l: TULit| self.IsSatisfied(l) && self.GetAssignedDecLevel(l) <= falseLitDecLevel);
			if !itFound
			{
				stdout_println!("***ASSERTION-FAILURE F- at {}\n{}", self.SLits(cls, false), self.STrail());
			}
			debug_assert!(itFound);
		}

		if self.IsFalsified(otherLit)
		{
			let itFound = found(&|l: TULit| self.IsSatisfied(l) && self.GetAssignedDecLevel(l) <= falseLitDecLevel && self.GetAssignedDecLevel(l) <= otherLitDecLevel);
			if !itFound
			{
				stdout_println!("***ASSERTION-FAILURE FF at {}\n{}", self.SLits(cls, false), self.STrail());
			}
			debug_assert!(itFound);
		}
	}

	pub(crate) fn WLAssertNoMissedImplications(&mut self) -> bool
	{
		let mut v = self.m_TrailStart;
		while v != BadUVar
		{
			let l = Negate(self.GetAssignedLitForVar(v));
			let wi = self.m_Watches[l as usize];
			v = self.m_VarInfo[v as usize].m_TrailNext;

			let bInd = wi.m_WBInd;
			let longWatches = wi.m_LongWatches;
			let allocatedEntries = wi.m_AllocatedEntries;
			let binaryWatches = wi.m_BinaryWatches;

			if allocatedEntries == Ind::ZERO
			{
				continue;
			}

			debug_assert!(bInd.as_usize() < self.m_W.cap());
			debug_assert!(bInd.as_usize() + allocatedEntries.as_usize() <= self.m_W.cap());
			debug_assert!(longWatches.as_usize() * 2 + binaryWatches.as_usize() <= allocatedEntries.as_usize());

			if binaryWatches != Ind::ZERO
			{
				let binWatches = self.m_W.get_span_cap_sz(wi.m_WBInd.as_usize() + wi.GetLongEntries(), wi.m_BinaryWatches.as_usize());
				for secondLit in binWatches
				{
					debug_assert!(secondLit != 0);
					debug_assert!(secondLit < self.GetNextLit());
					let mut clsArray: [TULit; 2] = [l, secondLit];
					let cls = TSpanTULit::from_slice(&mut clsArray);
					if self.IsFalsified(cls[0]) || self.IsFalsified(cls[1])
					{
						self.WLAssertBinaryClsNoMissedImplication(cls);
					}
				}
			}

			let mut currLongWatchInd: usize = 0;
			let mut currLongWatchPtr = self.m_W.get_ptr(wi.m_WBInd.as_usize());
			while currLongWatchInd < wi.m_LongWatches.as_usize()
			{
				let cachedLit = unsafe { *currLongWatchPtr };

				let clsInd = unsafe { Ind::read(currLongWatchPtr.add(1)) };
				debug_assert!(Compress || clsInd.as_usize() < self.m_B.cap() - 1);

				let cls = self.ConstClsSpan(clsInd, TUV::MAX);

				if !Compress
				{
					if !(clsInd >= self.m_FirstLearntClsInd || !self.ClsGetIsLearnt(clsInd))
					{
						stdout_println!("***ASSERTION-FAILURE First-Learnt at {}\n{}", self.SLits(cls, false), self.STrail());
					}
					debug_assert!(clsInd >= self.m_FirstLearntClsInd || !self.ClsGetIsLearnt(clsInd));
				}

				debug_assert!(cls[0] == l || cls[1] == l);
				if cls.find(cachedLit) == cls.size()
				{
					stdout_println!("***ASSERTION-FAILURE CL- at cached {}; cls = {}\n{}", self.SLit(cachedLit), self.SLits(cls, false), self.STrail());
					debug_assert!(false);
				}

				if self.IsFalsified(cls[0]) || self.IsFalsified(cls[1])
				{
					self.WLAssertLongClsNoMissedImplication(cls);
				}

				currLongWatchInd += 1;
				currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
			}
		}

		true
	}

	pub(crate) fn WLAssertConsistency(&mut self, testMissedImplications: bool) -> bool
	{
		if self.m_Params.m_ParamAssertConsistency.get() < 3
		{
			return if testMissedImplications { self.WLAssertNoMissedImplications() } else { true };
		}

		let mut longCls2Watches: HashMap<Ind, u8> = HashMap::new();

		type TBinCls = (TULit, TULit);

		let mut binClssTwice: HashSet<TBinCls> = HashSet::new();

		for l in 1..self.GetNextLit()
		{
			let wi = self.m_Watches[l as usize];

			let allocatedEntries = wi.m_AllocatedEntries;
			if allocatedEntries == Ind::ZERO
			{
				continue;
			}

			let wbInd = wi.m_WBInd;
			let longWatches = wi.m_LongWatches;

			let binaryWatches = wi.m_BinaryWatches;

			debug_assert!(wbInd.as_usize() < self.m_W.cap());
			debug_assert!(wbInd.as_usize() + allocatedEntries.as_usize() <= self.m_W.cap());
			debug_assert!(longWatches.as_usize() * 2 + binaryWatches.as_usize() <= allocatedEntries.as_usize());

			if binaryWatches != Ind::ZERO
			{
				let binWatches = self.m_W.get_span_cap_sz(wi.m_WBInd.as_usize() + wi.GetLongEntries(), wi.m_BinaryWatches.as_usize());
				for secondLit in binWatches
				{
					debug_assert!(secondLit != 0);
					debug_assert!(secondLit < self.GetNextLit());
					let clsBinCls: TBinCls = (l, secondLit);
					debug_assert!(!binClssTwice.contains(&clsBinCls));
					binClssTwice.insert(clsBinCls);
					let mut clsArray: [TULit; 2] = [clsBinCls.0, clsBinCls.1];
					let cls = TSpanTULit::from_slice(&mut clsArray);
					if testMissedImplications && (self.IsFalsified(cls[0]) || self.IsFalsified(cls[1]))
					{
						self.WLAssertBinaryClsNoMissedImplication(cls);
					}
				}
			}

			let mut currLongWatchInd: usize = 0;
			let mut currLongWatchPtr = self.m_W.get_ptr(wi.m_WBInd.as_usize());
			while currLongWatchInd < wi.m_LongWatches.as_usize()
			{
				let cachedLit = unsafe { *currLongWatchPtr };

				let clsInd = unsafe { Ind::read(currLongWatchPtr.add(1)) };
				debug_assert!(Compress || clsInd.as_usize() < self.m_B.cap() - 1);

				let cls = self.ConstClsSpan(clsInd, TUV::MAX);

				if !Compress
				{
					if !(clsInd >= self.m_FirstLearntClsInd || !self.ClsGetIsLearnt(clsInd))
					{
						stdout_println!("***ASSERTION-FAILURE First-Learnt at {}\n{}", self.SLits(cls, false), self.STrail());
					}
					debug_assert!(clsInd >= self.m_FirstLearntClsInd || !self.ClsGetIsLearnt(clsInd));
				}

				debug_assert!(cls[0] == l || cls[1] == l);
				if !(cls.find(cachedLit) != cls.size())
				{
					stdout_println!("***ASSERTION-FAILURE F- at {}\n{}", self.SLits(cls, false), self.STrail());
				}
				debug_assert!(cls.find(cachedLit) != cls.size());

				if testMissedImplications && (self.IsFalsified(cls[0]) || self.IsFalsified(cls[1]))
				{
					self.WLAssertLongClsNoMissedImplication(cls);
				}

				match longCls2Watches.get_mut(&clsInd)
				{
					None =>
					{
						longCls2Watches.insert(clsInd, 1);
					}
					Some(it) =>
					{
						debug_assert!(*it == 1);
						*it += 1;
					}
				}

				currLongWatchInd += 1;
				currLongWatchPtr = currLongWatchPtr.wrapping_add(TWatchInfo::<Ind>::BinsInLong);
			}
		}

		debug_assert!(longCls2Watches.len() as u64 == self.m_Stat.m_ActiveLongClss);
		debug_assert!(longCls2Watches.iter().all(|p| *p.1 == 2));

		debug_assert!(binClssTwice.len() as u64 == self.m_Stat.m_ActiveBinaryClss * 2);

		for binCls in &binClssTwice
		{
			let l1 = binCls.0;
			let l2 = binCls.1;
			let binClsReversed: TBinCls = (l2, l1);
			debug_assert!(binClssTwice.contains(&binClsReversed));
			let _ = binClsReversed;
		}

		true
	}

	pub(crate) fn WLIsLitBetter(&self, lCand: TULit, lOther: TULit) -> bool
	{
		let wlChoice = self.m_Params.m_ParamBCPWLChoice.get();
		let lCandWLEntries = if wlChoice == 0 { self.m_Watches[lCand as usize].GetUsedEntries() } else if wlChoice == 1 { self.m_Watches[lOther as usize].GetUsedEntries() } else { 1 };
		let lOtherWLEntries = if wlChoice == 0 { self.m_Watches[lOther as usize].GetUsedEntries() } else if wlChoice == 1 { self.m_Watches[lCand as usize].GetUsedEntries() } else { 0 };

		if self.IsSatisfied(lCand)
		{
			if !self.IsSatisfied(lOther)
			{
				return true;
			}

			self.GetAssignedDecLevel(lCand) < self.GetAssignedDecLevel(lOther) || (self.GetAssignedDecLevel(lCand) == self.GetAssignedDecLevel(lOther) && lCandWLEntries < lOtherWLEntries)
		}
		else if !self.IsAssigned(lCand)
		{
			if self.IsSatisfied(lOther)
			{
				return false;
			}

			if self.IsFalsified(lOther)
			{
				return true;
			}

			debug_assert!(!self.IsAssigned(lOther));

			lCandWLEntries < lOtherWLEntries
		}
		else
		{
			debug_assert!(self.IsFalsified(lCand));

			if self.IsSatisfied(lOther) || !self.IsAssigned(lOther)
			{
				return false;
			}

			self.GetAssignedDecLevel(lCand) > self.GetAssignedDecLevel(lOther) || (self.GetAssignedDecLevel(lCand) == self.GetAssignedDecLevel(lOther) && lCandWLEntries < lOtherWLEntries)
		}
	}
}
