// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	// <Deletion-handled, spare-buffer-used>
	pub(crate) fn BCDeleteLitsByMovingToOtherBufferIfRequiredAssumingLastDeleted(&mut self, clsInd: &mut Ind, origSize: TUV, lits2Remove: TUV, insertToSpareIfIteratorsBecomeInvalidated: bool) -> (bool, bool)
	{
		if !self.BCDeleteLitsCausesMoveToAnotherBuffer(origSize, lits2Remove)
		{
			return (false, false);
		}

		// This is the corner-case, when, after deleting the literals, the clause must move to another compressed clause buffer
		let ccs = self.ConstClsSpan(*clsInd, TUV::MAX);
		let mut spareUsed = false;
		let isLearnt = self.ClsGetIsLearnt(*clsInd);
		let glue = if isLearnt { self.ClsGetGlue(*clsInd) } else { 0 };
		let skipDel = if isLearnt { self.ClsGetSkipdel(*clsInd) } else { false };
		let activity = if isLearnt { self.ClsGetActivity(*clsInd) } else { 0.0f32 };
		let newBcInd = self.BCCompress(ccs.subspan(0, (origSize - lits2Remove) as usize), isLearnt, glue, skipDel, activity, insertToSpareIfIteratorsBecomeInvalidated, Some(&mut spareUsed));

		for currWatchI in 0..=1usize
		{
			self.WLReplaceInd(ccs.get(currWatchI), *clsInd, newBcInd.ind());
		}

		// Marking the old clause as removed
		let bcInd = TBCInd::from_ind(*clsInd);
		let bcHashInd = bcInd.GetHashId();
		self.BCGetBitArray(bcHashInd.val()).bit_set(0, bcInd.m_BitsForLit(), bcInd.BitFirstLit());

		// Counting the removed header and the rest of the removed literals as waste (the removed literals themselves are supposed to be counted as waste by the caller)
		self.m_BWasted += Ind::from_u64(bcHashInd.GetFirstLitOffset() as u64 + bcHashInd.m_BitsForLit() as u64 * (origSize - lits2Remove) as u64);

		// Updating clsInd to the new clause location
		*clsInd = newBcInd.ind();

		debug_assert!(self.NV(2) || insertToSpareIfIteratorsBecomeInvalidated || self.P(&("\tBCDeleteLitsByMovingToOtherBufferIfRequiredAssumingLastDeleted, new clause: ".to_string() + &self.SLits(self.Cls(*clsInd), false) + "\n")));

		(true, spareUsed)
	}

	// Adds a compressed clause
	pub(crate) fn BCCompress(&mut self, cls: TSpanTULit, isLearnt: bool, mut glue: TUV, stayForAnotherRound: bool, activity: f32, _insertToSpareIfIteratorsBecomeInvalidated: bool, spareUsedinCompressed: Option<&mut bool>) -> TBCInd<Ind>
	{
		let bcHashId = TBCHashId::new(isLearnt, BCClsSize2Bits(cls.size() as u64), self.BCMaxLitWidth(cls));

		// m_BC is a direct-index table, so an insertion never invalidates the iterators; hence, there is no spare buffer
		let baPtr = self.m_BC.insert(bcHashId.val());
		if let Some(spareUsedinCompressed) = spareUsedinCompressed
		{
			*spareUsedinCompressed = false;
		}

		if baPtr.is_null()
		{
			self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "BAddInitClause: couldn't insert into m_BC".to_string());
			return TBCInd::from_ind(Self::BadClsInd);
		}

		let ba = unsafe { &mut *baPtr };

		let newBitsRequired: u64 = bcHashId.m_BitsForClsSize() as u64 + cls.size() as u64 * bcHashId.m_BitsForLit() as u64 + (if isLearnt { bcHashId.GetBitsGlue() as u64 + 32 } else { 0 });

		ba.bit_reserve_new_chunk(newBitsRequired, 0);
		if ba.uninitialized_or_erroneous()
		{
			self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "BCCompress: couldn't reserve new bit chunk".to_string());
			return TBCInd::from_ind(Self::BadClsInd);
		}

		let bitStart: u64 = ba.bit_get_next_bit();

		// Push the size
		if bcHashId.m_BitsForClsSize() != 0
		{
			ba.bit_push(BCClsSize2EncodedClsSize(cls.size() as u64) as u64, bcHashId.m_BitsForClsSize());
			debug_assert!(ba.bit_get(bitStart, bcHashId.m_BitsForClsSize()) == BCClsSize2EncodedClsSize(cls.size() as u64) as u64);
		}

		if isLearnt
		{
			// Push the glue
			if glue > bcHashId.MaxGlue()
			{
				glue = bcHashId.MaxGlue();
			}
			ba.bit_push(glue as u64, bcHashId.GetBitsGlue());
			debug_assert!(ba.bit_get(bitStart + bcHashId.m_BitsForClsSize() as u64, bcHashId.GetBitsGlue()) == glue as u64);
			// Push stay-for-another-round
			ba.bit_push(stayForAnotherRound as u64, 1);
			debug_assert!(ba.bit_get(bitStart + bcHashId.m_BitsForClsSize() as u64 + bcHashId.GetBitsGlue() as u64, 1) == stayForAnotherRound as u64);
			// Push the activity
			debug_assert!(activity >= 0.);
			let uint32Activity: u32 = activity.to_bits();
			ba.bit_push(uint32Activity as u64, 31);
			debug_assert!(ba.bit_get(bitStart + bcHashId.m_BitsForClsSize() as u64 + bcHashId.GetBitsGlue() as u64 + 1, 31) == uint32Activity as u64);
		}

		// Push the literals
		for l in cls
		{
			ba.bit_push(l as u64, bcHashId.m_BitsForLit());
			debug_assert!(ba.bit_get(ba.bit_get_next_bit() - bcHashId.m_BitsForLit() as u64, bcHashId.m_BitsForLit()) == l as u64);
		}

		debug_assert!(ba.bit_get_next_bit() == bitStart + newBitsRequired);

		let bci = TBCInd::<Ind>::new(bcHashId, bitStart);
		if bci.IsError()
		{
			let ss = format!("BCCompress: index too narrow -- bcHashId = {:x}; bitStart = {}", bcHashId.val(), bitStart);
			self.SetStatus(TToporStatus::STATUS_INDEX_TOO_NARROW, ss);
		}

		bci
	}

	// The explicit <false> instantiations of the clause accessors (that is, the versions working on the explicit buffer m_B independently of Compress),
	// required by BCIsIdenticalToDecompressed and BCDecompress only
	// Exactly as in C++, the nested calls still go to the default (Compress-dependent) versions

	fn ClsGetSizeStd(&self, clsInd: Ind) -> TUV
	{
		if !self.ClsGetIsLearnt(clsInd) || self.EClsGetIsOversizedAssumeLearnt(clsInd)
		{
			self.m_B[clsInd.as_usize()] & !ClsIsLearntMask
		}
		else
		{
			self.m_B[clsInd.as_usize()] & !ClsIsLearntAndGlueMask
		}
	}

	fn ClsGetIsLearntStd(&self, clsInd: Ind) -> bool
	{
		self.m_B[clsInd.as_usize()] & ClsIsLearntMask != 0
	}

	fn ClsGetGlueStd(&self, clsInd: Ind) -> TUV
	{
		debug_assert!(self.ClsGetIsLearnt(clsInd));
		if !self.EClsGetIsOversizedAssumeLearnt(clsInd)
		{
			(self.m_B[clsInd.as_usize()] & ClsGlueMask) >> ClsSizeBits
		}
		else
		{
			self.ClsGetSize(clsInd)
		}
	}

	fn ClsSetGlueStd(&mut self, clsInd: Ind, glue: TUV)
	{
		debug_assert!(self.ClsGetIsLearnt(clsInd));
		if !self.EClsGetIsOversizedAssumeLearnt(clsInd)
		{
			// The C++ version clamps the glue to ClsMaxGlue here; the only caller provides a glue, read from at most 11 bits, which never exceeds it
			debug_assert!(glue <= ClsMaxGlue);
			self.m_B[clsInd.as_usize()] = (self.m_B[clsInd.as_usize()] & !ClsGlueMask) | (glue << ClsSizeBits);
			debug_assert!(self.ClsGetGlue(clsInd) == glue);
		}
		else
		{
			debug_assert!(self.ClsGetGlue(clsInd) == self.ClsGetSize(clsInd));
		}
	}

	fn ClsGetSkipdelStd(&self, clsInd: Ind) -> bool
	{
		self.m_B[self.EClsGetActivityAndSkipdelIndex(clsInd)] & ClsSkipdelMask != 0
	}

	fn ClsSetSkipdelStd(&mut self, clsInd: Ind, skipDel: bool)
	{
		let i = self.EClsGetActivityAndSkipdelIndex(clsInd);
		self.m_B[i] = (self.m_B[i] & ClsNotSkipdelMask) | ((skipDel as u32) << ClsLShiftSkipDel);
	}

	fn ClsGetActivityStd(&self, clsInd: Ind) -> f32
	{
		f32::from_bits(self.m_B[self.EClsGetActivityAndSkipdelIndex(clsInd)]).abs()
	}

	fn ClsSetActivityStd(&mut self, clsInd: Ind, a: f32)
	{
		debug_assert!(a >= 0.);
		let prevSkipDel = self.ClsGetSkipdel(clsInd);
		let updatedA: f32 = if prevSkipDel { -a } else { a };
		let i = self.EClsGetActivityAndSkipdelIndex(clsInd);
		self.m_B[i] = updatedA.to_bits();
	}

	// The function below is intended for debugging and validation
	pub(crate) fn BCIsIdenticalToDecompressed(&mut self, clsInd: Ind, currHashId: TBCHashId, bStart: u64) -> bool
	{
		let ba = self.m_BC.at(currHashId.val());
		let bcInd = TBCInd::<Ind>::new(currHashId, bStart);
		let cc = CCls::<Ind, Compress>::new_compressed(ba, bcInd);
		let ba = unsafe { &*ba };

		let isOversized = currHashId.m_IsLearnt() && cc.m_ClsSize > ClsLearntMaxSizeWithGlue as usize;

		if isOversized
		{
			debug_assert!(false);
			return false;
		}

		if self.ClsGetSizeStd(clsInd) as usize != cc.m_ClsSize
		{
			debug_assert!(false);
			return false;
		}

		if self.ClsGetIsLearntStd(clsInd) != bcInd.m_IsLearnt()
		{
			debug_assert!(false);
			return false;
		}

		if bcInd.m_IsLearnt()
		{
			let glueC = self.ClsGetGlueStd(clsInd);
			let glueCC = ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64, bcInd.GetBitsGlue()) as TUV;
			if glueC != glueCC
			{
				debug_assert!(false);
				return false;
			}

			let skipDelC = self.ClsGetSkipdelStd(clsInd);
			let skipDelCC = ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcInd.GetBitsGlue() as u64, 1) != 0;
			if skipDelC != skipDelCC
			{
				debug_assert!(false);
				return false;
			}

			// The activities are compared as Booleans in the C++ version
			let activityC = self.ClsGetActivityStd(clsInd) != 0.;
			let activityCC = f32::from_bits(ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcInd.GetBitsGlue() as u64 + 1, 31) as u32) != 0.;
			if activityC != activityCC
			{
				debug_assert!(false);
				return false;
			}
		}

		// CCompressedClsConstSnapshotDense cls(ba, clsInd)
		let cls = CCls::<Ind, Compress>::new_compressed(ba as *const CBitArray as *mut CBitArray, TBCInd::from_ind(clsInd));
		for i in 0..cls.m_ClsSize
		{
			if cls.get(i) != cc.get(i)
			{
				debug_assert!(false);
				return false;
			}
		}

		true
	}

	// Decompresses the given compressed clause to m_BNext in m_B and also moves b to the next bit after the clause
	pub(crate) fn BCDecompress(&mut self, currHashId: TBCHashId, b: &mut u64)
	{
		let bStart = *b;
		let baPtr = self.m_BC.insert(currHashId.val());
		if baPtr.is_null()
		{
			// The C++ version would throw from unordered_map::operator[]
			self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "BCExtractClause: couldn't insert into m_BC".to_string());
			return;
		}
		let ba = unsafe { &*baPtr };
		let encodedClsSize = ba.bit_get_and_advance(b, currHashId.m_BitsForClsSize()) as u16;
		let clsSize = Ind::from_u32(BCEncodedClsSize2ClsSizeConst(encodedClsSize as u32, currHashId.m_BitsForClsSize() as u16));

		let isOversized = currHashId.m_IsLearnt() && clsSize.as_u64() > ClsLearntMaxSizeWithGlue as u64;
		debug_assert!(!isOversized);

		let clsLitsStartOffset = self.EClsLitsStartOffset(currHashId.m_IsLearnt(), isOversized);
		let newBNext = self.m_BNext + Ind::from_u32(clsLitsStartOffset) + clsSize;

		if newBNext < self.m_BNext
		{
			self.SetStatus(TToporStatus::STATUS_INDEX_TOO_NARROW, "BCExtractClause: too many literals in all the clauses combined".to_string());
			return;
		}

		if newBNext.as_usize() >= self.m_B.cap()
		{
			self.m_B.reserve_atleast(newBNext.as_usize());
			if self.m_B.uninitialized_or_erroneous()
			{
				self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "BCExtractClause: couldn't reserve the buffer".to_string());
				return;
			}
		}

		// Order of setting the fields is important, since ClsSetSize depends on ClsSetSize and ClsSetGlue depends on both
		self.EClsSetIsLearnt(self.m_BNext, currHashId.m_IsLearnt());
		self.ClsSetSize(self.m_BNext, clsSize.as_u32());
		if currHashId.m_IsLearnt()
		{
			let glueBits = currHashId.GetBitsGlue();
			let glue = ba.bit_get_and_advance(b, glueBits) as TUV;
			self.ClsSetGlueStd(self.m_BNext, glue);

			let skipDel = ba.bit_get_and_advance(b, 1) != 0;
			self.ClsSetSkipdelStd(self.m_BNext, skipDel);

			let activityUint32 = ba.bit_get_and_advance(b, 31) as u32;
			let activity = f32::from_bits(activityUint32);
			self.ClsSetActivityStd(self.m_BNext, activity);
		}

		let litsStart = self.m_BNext.as_usize() + clsLitsStartOffset as usize;
		for i in 0..clsSize.as_usize()
		{
			let l = ba.bit_get_and_advance(b, currHashId.m_BitsForLit()) as TUV;
			self.m_B[litsStart + i] = l;
		}

		debug_assert!(self.BCIsIdenticalToDecompressed(self.m_BNext, currHashId, bStart));
		let _ = bStart;
	}

	pub(crate) fn BCNextBitSum(&self) -> usize
	{
		let mut r: usize = 0;
		let mut it = self.m_BC.begin();
		while it != self.m_BC.end()
		{
			r += self.BCGetBitArrayConst(it as u16).bit_get_next_bit() as usize;
			it = self.m_BC.next(it);
		}

		r
	}

	pub(crate) fn BCCapacitySum(&self) -> usize
	{
		let mut r: usize = 0;
		let mut it = self.m_BC.begin();
		while it != self.m_BC.end()
		{
			r += self.BCGetBitArrayConst(it as u16).cap();
			it = self.m_BC.next(it);
		}

		r
	}
}
