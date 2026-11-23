// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topor_table_bit_info::{cMasksByBit, tableByBit, tableByEntry, MAX_BITS_NUM};

/*
* Contagious bit-array: the LSB's go into the lower word, so the order is preserved, but there is a cost of an additional shift
*/

/// # Safety
/// buf must be non-null, aligned and point at a buffer, containing the word of start_bit_num and the next one;
/// elem_width must be in 1..=MAX_BITS_NUM (it indexes the mask table without a bounds check)
#[inline(always)]
pub unsafe fn CGetElem(buf: *const u64, start_bit_num: u64, elem_width: u8) -> u64
{
	let word_ind = (start_bit_num >> 6) as usize;
	let start_bit_in_word = (start_bit_num & 63) as usize;
	debug_assert!((1..=MAX_BITS_NUM).contains(&elem_width) && !buf.is_null());
	let bit_info = cMasksByBit.get_unchecked((elem_width - 1) as usize).get_unchecked(start_bit_in_word);

	((*buf.add(word_ind) & bit_info.w0ReadMask) >> bit_info.w0ReadShr) | ((*buf.add(word_ind + 1) & bit_info.w1ReadMask) << bit_info.w1ReadShl)
}

/// # Safety
/// See CGetElem
#[inline(always)]
pub unsafe fn CGetElemAndGoToNext(buf: *const u64, bit_num: &mut u64, elem_width: u8) -> u64
{
	let res = CGetElem(buf, *bit_num, elem_width);
	*bit_num += elem_width as u64;
	res
}

/// # Safety
/// See CGetElem
#[inline(always)]
pub unsafe fn CSetElem(buf: *mut u64, bit_num: u64, elem_width: u8, elem: u64)
{
	let word_ind = (bit_num >> 6) as usize;
	let start_bit_in_word = (bit_num & 63) as usize;
	debug_assert!((1..=MAX_BITS_NUM).contains(&elem_width) && !buf.is_null());
	let bit_info = cMasksByBit.get_unchecked((elem_width - 1) as usize).get_unchecked(start_bit_in_word);

	*buf.add(word_ind) &= bit_info.W0CleanMask();
	*buf.add(word_ind) |= (elem & bit_info.ElemW0Mask()) << bit_info.ElemW0Shl() as u64;
	*buf.add(word_ind + 1) &= bit_info.W1CleanMask();
	// The shift amount is in [0,64): it is 0 only when the w1 mask is 0, in which case the result is masked out anyway
	*buf.add(word_ind + 1) |= (elem & bit_info.ElemW1Mask()) >> bit_info.ElemW1Shr();
}

// Sets elemWidth bits in the buffer buf, starting from bit bitNum to elem and advances bitNum by elemWidth
/// # Safety
/// See CGetElem
#[inline(always)]
pub unsafe fn CSetElemAndGoToNext(buf: *mut u64, bit_num: &mut u64, elem_width: u8, elem: u64)
{
	CSetElem(buf, *bit_num, elem_width, elem);
	*bit_num += elem_width as u64;
}

/*
* Non-contagious bit-array: the LSB's go into the higher word, so the order is *not* preserved, but there is an additional shift saving
* This implementation is preferable, when all the chunks have the same width
* (The solver itself uses only the contagious bit-array; the non-contagious one is ported for completeness)
*/

// GetElem extracts elemWidth bits from the buffer buf, starting from bit startBitNum
/// # Safety
/// buf must point at a buffer, containing the word of startBitNum and the next one; elemWidth must be in 1..=MAX_BITS_NUM
#[inline]
pub unsafe fn BAGetElem(buf: *const u64, startBitNum: u64, elemWidth: u8) -> u64
{
	debug_assert!((1..=MAX_BITS_NUM).contains(&elemWidth) && !buf.is_null());
	let bitInfo = &tableByBit[(elemWidth - 1) as usize];
	let wordInd = (startBitNum >> 6) as usize;
	let startBitInWord = (startBitNum & 63) as usize;

	((*buf.add(wordInd) & bitInfo[startBitInWord].w0ReadMask) >> bitInfo[startBitInWord].w0ReadShr) | (*buf.add(wordInd + 1) & bitInfo[startBitInWord].w1ReadMask)
}

// A more efficient constexpr version of BAGetElem, which should be used when
// (1) startBitNum and elemWidth are known at compile time
// (2) Both startBitNum and elemWidth fit into 64 bits
// (The C++ overload BAGetElem<startBitNum, elemWidth>(buf))
/// # Safety
/// buf must point at the word, the element starts in, and (if the element crosses a word boundary) the next one
#[inline]
pub unsafe fn BAGetElemConstExpr<const startBitNum: u8, const elemWidth: u8>(buf: *const u64) -> u64
{
	const { assert!(startBitNum <= 64 && elemWidth <= 64) };

	let bitInfo = &tableByBit[(elemWidth - 1) as usize];

	let w0Masked = *buf & bitInfo[startBitNum as usize].w0ReadMask;

	if startBitNum == 0
	{
		w0Masked
	}
	else
	{
		let w0MaskShifted = w0Masked >> bitInfo[startBitNum as usize].w0ReadShr;

		if startBitNum as u32 + elemWidth as u32 <= 64
		{
			w0MaskShifted
		}
		else
		{
			w0MaskShifted | (*buf.add(1) & bitInfo[startBitNum as usize].w1ReadMask)
		}
	}
}

/// # Safety
/// See BAGetElem
#[inline]
pub unsafe fn BAGetElemAndGoToNext(buf: *const u64, bitNum: &mut u64, elemWidth: u8) -> u64
{
	let res = BAGetElem(buf, *bitNum, elemWidth);
	*bitNum += elemWidth as u64;
	res
}

// Sets elemWidth bits in the buffer buf, starting from bit bitNum to elem
/// # Safety
/// See BAGetElem
#[inline]
pub unsafe fn BASetElem(buf: *mut u64, bitNum: u64, elemWidth: u8, elem: u64)
{
	debug_assert!((1..=MAX_BITS_NUM).contains(&elemWidth) && !buf.is_null());
	let bitInfo = &tableByBit[(elemWidth - 1) as usize];
	let wordInd = (bitNum >> 6) as usize;
	let startBitInWord = (bitNum & 63) as usize;

	*buf.add(wordInd) &= bitInfo[startBitInWord].W0CleanMask();
	*buf.add(wordInd) |= (elem & bitInfo[startBitInWord].W0WriteMask()) << bitInfo[startBitInWord].W0WriteShl() as u64;
	*buf.add(wordInd + 1) &= bitInfo[startBitInWord].W1CleanMask();
	*buf.add(wordInd + 1) |= elem & bitInfo[startBitInWord].W1WriteMask();
}

// A more efficient constexpr version of BASetElem, which should be used when
// (1) startBitNum and elemWidth are known at compile time
// (2) Both startBitNum and elemWidth fit into 64 bits
// (The C++ overload BASetElem<bitNum, elemWidth>(buf, elem))
/// # Safety
/// See BAGetElemConstExpr
#[inline]
pub unsafe fn BASetElemConstExpr<const bitNum: u8, const elemWidth: u8>(buf: *mut u64, elem: u64)
{
	const { assert!(bitNum <= 64 && elemWidth <= 64) };

	let bitInfo = &tableByBit[(elemWidth - 1) as usize];

	let elemW0Masked = elem & bitInfo[bitNum as usize].W0WriteMask();

	*buf &= bitInfo[bitNum as usize].W0CleanMask();

	if bitNum == 0
	{
		*buf |= elemW0Masked;
	}
	else
	{
		*buf |= elemW0Masked << bitInfo[bitNum as usize].W0WriteShl();
		if bitNum as u32 + elemWidth as u32 > 64
		{
			*buf.add(1) &= bitInfo[bitNum as usize].W1CleanMask();
			*buf.add(1) |= elem & bitInfo[bitNum as usize].W1WriteMask();
		}
	}
}

// SetElemAndGoToNext sets elemWidth bits in the buffer buf, starting from bit bitNum to elem and advances bitNum by elemWidth
/// # Safety
/// See BAGetElem
#[inline]
pub unsafe fn BASetElemAndGoToNext(buf: *mut u64, bitNum: &mut u64, elemWidth: u8, elem: u64)
{
	BASetElem(buf, *bitNum, elemWidth, elem);
	*bitNum += elemWidth as u64;
}

// A more efficient constexpr version of BAIncrAndGet, which should be used when
// (1) startBitNum and elemWidth are known at compile time
// (2) Both startBitNum and elemWidth fit into 64 bits
/// # Safety
/// See BAGetElem
#[inline]
pub unsafe fn BAIncrAndGet(buf: *mut u64, startBitNum: u64, elemWidth: u8, incrVal: u64) -> u64
{
	let mut currElem = BAGetElem(buf, startBitNum, elemWidth);
	currElem = currElem.wrapping_add(incrVal);
	debug_assert!(currElem != 0);
	BASetElem(buf, startBitNum, elemWidth, currElem);
	currElem
}

// (The C++ overload BAIncrAndGet<startBitNum, elemWidth>(buf, incrVal))
/// # Safety
/// See BAGetElemConstExpr
#[inline]
pub unsafe fn BAIncrAndGetConstExpr<const startBitNum: u8, const elemWidth: u8>(buf: *mut u64, incrVal: u64) -> u64
{
	let mut currElem = BAGetElemConstExpr::<startBitNum, elemWidth>(buf);
	currElem = currElem.wrapping_add(incrVal);
	debug_assert!(currElem != 0);
	BASetElemConstExpr::<startBitNum, elemWidth>(buf, currElem);
	currElem
}

// The bit-array can work with:
// (1) dynamic element size, if ElemWidthConstExpr = 0
// (2) compile-time element size ElemWidthConstExpr
// The compile-time size makes the array implementation for element sizes which are powers of 2 (1,2,4,8,16,32,64) more efficient
// The dynamic-time size allows one to change the size dynamically with the function ChangeSize
pub struct CBitArraySingleElemWidth<const ElemWidthConstExpr: u8>
{
	m_ElemWidth: u8,
	m_ElemsNum: u64,
	m_Buf: *mut u64,
	m_BufAllocated: usize,
}

impl<const ElemWidthConstExpr: u8> Default for CBitArraySingleElemWidth<ElemWidthConstExpr>
{
	fn default() -> Self
	{
		Self::new()
	}
}

impl<const ElemWidthConstExpr: u8> Drop for CBitArraySingleElemWidth<ElemWidthConstExpr>
{
	fn drop(&mut self)
	{
		if !self.m_Buf.is_null()
		{
			// SAFETY: m_Buf is non-null, so it was allocated by CallocOrRealloc with the layout of m_BufAllocated words
			unsafe { std::alloc::dealloc(self.m_Buf as *mut u8, std::alloc::Layout::array::<u64>(self.m_BufAllocated).unwrap()) };
		}
	}
}

impl<const ElemWidthConstExpr: u8> CBitArraySingleElemWidth<ElemWidthConstExpr>
{
	const IsSingleBit: bool = ElemWidthConstExpr.is_power_of_two();

	pub fn new() -> Self
	{
		const { assert!(ElemWidthConstExpr <= 64) };
		CBitArraySingleElemWidth { m_ElemWidth: ElemWidthConstExpr, m_ElemsNum: 0, m_Buf: std::ptr::null_mut(), m_BufAllocated: 0 }
	}

	#[inline]
	pub fn Push(&mut self, elemVal: u64)
	{
		let tableEntryInd = self.TableEntryInd(self.m_ElemsNum);
		let wordInd = self.WordInd(self.m_ElemsNum);

		self.ReallocIfReq(wordInd, false);

		self.SetElemInternal(elemVal, wordInd, tableEntryInd);

		self.m_ElemsNum += 1;
	}

	#[inline]
	pub fn ForceElemSetNonExistingTo0(&mut self, elemNum: u64, elemVal: u64)
	{
		let tableEntryInd = self.TableEntryInd(elemNum);
		let wordInd = self.WordInd(elemNum);

		if elemNum >= self.m_ElemsNum
		{
			self.m_ElemsNum = elemNum + 1;
			let wordInd = self.WordInd(elemNum);
			self.ReallocIfReq(wordInd, true);
		}

		self.SetElemInternal(elemVal, wordInd, tableEntryInd);
	}

	#[inline]
	pub fn Set(&mut self, elemNum: u64, elemVal: u64)
	{
		let tableEntryInd = self.TableEntryInd(elemNum);
		let wordInd = self.WordInd(elemNum);

		self.SetElemInternal(elemVal, wordInd, tableEntryInd);
	}

	#[inline]
	pub fn SetAllElemsTo0(&mut self)
	{
		if self.m_BufAllocated != 0
		{
			// SAFETY: m_BufAllocated != 0 only while m_Buf holds that many words
			unsafe { std::ptr::write_bytes(self.m_Buf, 0, self.m_BufAllocated) };
		}
	}

	#[inline]
	pub fn ElemsNum(&self) -> u64
	{
		self.m_ElemsNum
	}

	#[inline]
	pub fn ElemWidth(&self) -> u8
	{
		if ElemWidthConstExpr == 0
		{
			self.m_ElemWidth
		}
		else
		{
			ElemWidthConstExpr
		}
	}

	#[inline]
	pub fn Get(&self, elemNum: u64) -> u64
	{
		let tableEntryInd = self.TableEntryInd(elemNum) as usize;
		let wordInd = self.WordInd(elemNum);

		// SAFETY: as in the C++ version, the element must exist, so its word (and the next one, which the buffer always has) is allocated
		unsafe
		{
			if ElemWidthConstExpr == 64
			{
				*self.m_Buf.add(wordInd)
			}
			else if Self::IsSingleBit
			{
				let e = &tableByEntry[(ElemWidthConstExpr - 1) as usize].1[tableEntryInd];
				(*self.m_Buf.add(wordInd) & e.w0ReadMask) >> e.w0ReadShr
			}
			else
			{
				let e = &tableByEntry[(self.ElemWidth() - 1) as usize].1[tableEntryInd];
				((*self.m_Buf.add(wordInd) & e.w0ReadMask) >> e.w0ReadShr) | (*self.m_Buf.add(wordInd + 1) & e.w1ReadMask)
			}
		}
	}

	// The C++ version throws logic_error; there are no exceptions in Rust, so the error is a panic
	// The trailing early return is the C++ one, kept statement by statement
	#[allow(clippy::needless_return)]
	pub fn ChangeElemWidth(&mut self, newWidth: u8)
	{
		const { assert!(ElemWidthConstExpr == 0) };
		if newWidth > 64 || newWidth == 0
		{
			panic!("ChangeElemWidth failed, since the new width == 0 or > 64");
		}

		self.m_ElemWidth = newWidth;

		if self.m_BufAllocated == 0
		{
			return;
		}
	}

	#[inline]
	pub(crate) fn BitNum(&self, elemNum: u64) -> u64
	{
		if Self::IsSingleBit
		{
			elemNum << ElemWidthConstExpr.trailing_zeros()
		}
		else
		{
			elemNum * self.ElemWidth() as u64
		}
	}

	#[inline]
	fn WordInd(&self, elemNum: u64) -> usize
	{
		if Self::IsSingleBit
		{
			let shrVal = 6 - ElemWidthConstExpr.trailing_zeros();
			elemNum as usize >> shrVal
		}
		else
		{
			self.BitNum(elemNum) as usize >> 6
		}
	}

	#[inline]
	fn TableEntryInd(&self, elemNum: u64) -> u8
	{
		(elemNum & tableByEntry[(self.ElemWidth() - 1) as usize].0 as u64) as u8
	}

	#[inline]
	fn SetElemInternal(&mut self, elem: u64, wordInd: usize, tableEntryInd: u8)
	{
		let e = &tableByEntry[(self.ElemWidth() - 1) as usize].1[tableEntryInd as usize];
		// SAFETY: ReallocIfReq or the existence of the element guarantees that the word of the element and the next one are allocated
		unsafe
		{
			if ElemWidthConstExpr == 64
			{
				*self.m_Buf.add(wordInd) = elem;
			}
			else
			{
				*self.m_Buf.add(wordInd) |= (elem & e.W0WriteMask()) << e.W0WriteShl();
			}

			if !Self::IsSingleBit
			{
				*self.m_Buf.add(wordInd + 1) |= elem & e.W1WriteMask();
			}
		}
	}

	// calloc(allocated, sizeof(uint64_t)), or realloc to allocated words, where nullptr means failure
	fn CallocOrRealloc(buf: *mut u64, oldAllocated: usize, allocated: usize) -> *mut u64
	{
		#[cfg(any(test, fuzzing))]
		if crate::topor_dyn_array::alloc_fault::should_fail()
		{
			return std::ptr::null_mut();
		}
		let layout = std::alloc::Layout::array::<u64>(allocated).unwrap();
		// SAFETY: the layout has a non-zero size (allocated >= 2); a non-null buf was allocated with oldAllocated words
		unsafe
		{
			if buf.is_null()
			{
				std::alloc::alloc_zeroed(layout) as *mut u64
			}
			else
			{
				std::alloc::realloc(buf as *mut u8, std::alloc::Layout::array::<u64>(oldAllocated).unwrap(), layout.size()) as *mut u64
			}
		}
	}

	// The C++ version throws bad_alloc, if the allocation fails; there are no exceptions in Rust, so it panics instead
	// (as in C++, the old buffer is lost: the array becomes empty)
	#[inline]
	fn ReallocIfReq(&mut self, wordInd: usize, isTight: bool)
	{
		if wordInd + 1 >= self.m_BufAllocated
		{
			let saveAllocated = self.m_BufAllocated;
			// The +2 part is because we may need an extra-word after wordInd, if the element to insert extends to another word
			self.m_BufAllocated = (if isTight { wordInd } else { (13 * wordInd) >> 3 }) + 2;

			if self.m_Buf.is_null()
			{
				self.m_Buf = Self::CallocOrRealloc(self.m_Buf, saveAllocated, self.m_BufAllocated);
				if self.m_Buf.is_null()
				{
					self.m_BufAllocated = 0;
					panic!("bad_alloc");
				}
			}
			else
			{
				let oldBuf = self.m_Buf;
				self.m_Buf = Self::CallocOrRealloc(self.m_Buf, saveAllocated, self.m_BufAllocated);
				if self.m_Buf.is_null()
				{
					// The C++ version leaks the old buffer here
					// SAFETY: a failed realloc leaves the old block, of saveAllocated words, allocated
					unsafe { std::alloc::dealloc(oldBuf as *mut u8, std::alloc::Layout::array::<u64>(saveAllocated).unwrap()) };
					self.m_BufAllocated = 0;
					panic!("bad_alloc");
				}

				// SAFETY: the buffer has just been reallocated to m_BufAllocated > saveAllocated words
				unsafe { std::ptr::write_bytes(self.m_Buf.add(saveAllocated), 0, self.m_BufAllocated - saveAllocated) };
			}
		}
	}
}

// The unsigned type of the narrowest width, which fits ElemWidthConstExpr bits (TAlignedUInt in the C++ version): bool, uint8_t, uint16_t, uint32_t or uint64_t
pub trait TAlignedUInt: Copy + Default
{
	fn FromU64(v: u64) -> Self;
	fn ToU64(self) -> u64;
}

macro_rules! impl_aligned_uint
{
	($($t:ty),*) =>
	{
		$(
			impl TAlignedUInt for $t
			{
				#[inline]
				fn FromU64(v: u64) -> Self { v as $t }
				#[inline]
				fn ToU64(self) -> u64 { self as u64 }
			}
		)*
	};
}
impl_aligned_uint!(u8, u16, u32, u64);

impl TAlignedUInt for bool
{
	// (bool)elemVal
	#[inline]
	fn FromU64(v: u64) -> Self { v != 0 }
	#[inline]
	fn ToU64(self) -> u64 { self as u64 }
}

pub struct TElemWidth<const W: u8>;

pub trait TSelAlignedUInt
{
	type T: TAlignedUInt;
}

macro_rules! sel_aligned_uint
{
	($t:ty; $($w:literal),*) => { $( impl TSelAlignedUInt for TElemWidth<$w> { type T = $t; } )* };
}
sel_aligned_uint!(bool; 1);
sel_aligned_uint!(u8; 2, 3, 4, 5, 6, 7, 8);
sel_aligned_uint!(u16; 9, 10, 11, 12, 13, 14, 15, 16);
sel_aligned_uint!(u32; 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32);
sel_aligned_uint!(u64; 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64);

pub struct CBitArrayAligned<const ElemWidthConstExpr: u8>
where
	TElemWidth<ElemWidthConstExpr>: TSelAlignedUInt,
{
	m_Buf: Vec<<TElemWidth<ElemWidthConstExpr> as TSelAlignedUInt>::T>,
}

impl<const ElemWidthConstExpr: u8> Default for CBitArrayAligned<ElemWidthConstExpr>
where
	TElemWidth<ElemWidthConstExpr>: TSelAlignedUInt,
{
	fn default() -> Self
	{
		Self::new()
	}
}

impl<const ElemWidthConstExpr: u8> CBitArrayAligned<ElemWidthConstExpr>
where
	TElemWidth<ElemWidthConstExpr>: TSelAlignedUInt,
{
	pub fn new() -> Self
	{
		const { assert!(ElemWidthConstExpr > 0 && ElemWidthConstExpr <= 64) };
		CBitArrayAligned { m_Buf: Vec::new() }
	}

	#[inline]
	pub fn Push(&mut self, elemVal: u64)
	{
		self.m_Buf.push(TAlignedUInt::FromU64(elemVal));
	}

	#[inline]
	pub fn ForceElemSetNonExistingTo0(&mut self, elemNum: u64, elemVal: u64)
	{
		if elemNum as usize >= self.m_Buf.len()
		{
			self.m_Buf.resize(elemNum as usize + 1, Default::default());
		}

		self.Set(elemNum, elemVal);
	}

	#[inline]
	pub fn SetAllElemsTo0(&mut self)
	{
		self.m_Buf.fill(Default::default());
	}

	#[inline]
	pub fn ElemsNum(&self) -> u64
	{
		self.m_Buf.len() as u64
	}

	pub const fn ElemWidth(&self) -> u8
	{
		ElemWidthConstExpr
	}

	#[inline]
	pub fn Set(&mut self, elemNum: u64, elemVal: u64)
	{
		self.m_Buf[elemNum as usize] = TAlignedUInt::FromU64(elemVal);
	}

	#[inline]
	pub fn Get(&self, elemNum: u64) -> u64
	{
		self.m_Buf[elemNum as usize].ToU64()
	}
}

