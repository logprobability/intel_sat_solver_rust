// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

// The types, constants and static helpers of the main solver class (see Topi.hpp), which don't require access to the solver's state

use crate::topor_bit_array::CBitArray;
use crate::topor_dyn_array::{CDynArray, Span};
use crate::topor_vector::CVector;
use std::fmt::{Debug, Display, LowerHex};
use std::hash::Hash;
use std::ops::{Add, AddAssign, Sub, SubAssign};

pub use crate::topor_external_types::TLit;

#[cfg(not(target_endian = "little"))]
compile_error!("The solver assumes a little-endian target (an index is stored in the watch buffer as a sequence of literals)");

// The TLit type, representing the external literals, unsigned
// Will be used for storing internal literals, whose sign is determined by the LSB, rather than the MSB
pub type TULit = u32;
const _: () = assert!(TULit::BITS == TLit::BITS);

// Bad literal
pub const BadULit: TULit = 0;

// TUL is used to hold unsigned integers, whose number is not greater than that of the literaks; used only for code clarity
pub type TUL = TULit;
// TUVar is used to hold variables; used only for code clarity
pub type TUVar = TULit;
// TUV is used to hold unsigned integers, whose number is not greater than that of the variables; used only for code clarity
pub type TUV = TUVar;
// Bad variable
pub const BadUVar: TULit = 0;

// counter type for discovering duplicates, tautologies, contradictions etc., e.g., in incoming clauses and assumptions
// In the context of handling new clauses,
// we did some tests and found that int32_t for the counter works better than int64_t, int16_t (both are 2x slower) and int8_t (significantly slower)
pub type TCounterType = i32;

// A span of internal literals (TULit)
pub type TSpanTULit = Span<TULit>;

#[inline(always)]
pub const fn GetVar(l: TULit) -> TUVar
{
	l >> 1
}
const _: () = assert!(GetVar(0b1001) == 0b100 && GetVar(0b1000) == 0b100);

#[inline(always)]
pub const fn GetLit(v: TUVar, is_negative: bool) -> TULit
{
	(v << 1) + is_negative as TULit
}
const _: () = assert!(GetLit(0b100, true) == 0b1001 && GetLit(0b100, false) == 0b1000);

#[inline(always)]
pub const fn GetFirstLit() -> TULit
{
	GetLit(1, false)
}
const _: () = assert!(GetFirstLit() == 2);

#[inline(always)]
pub const fn IsPos(l: TULit) -> bool
{
	(l & 1) == 0
}
const _: () = assert!(IsPos(0b1000));

#[inline(always)]
pub const fn IsNeg(l: TULit) -> bool
{
	(l & 1) != 0
}
const _: () = assert!(IsNeg(0b1001));

#[inline(always)]
pub const fn Negate(l: TULit) -> TULit
{
	l ^ 1
}
const _: () = assert!(Negate(0b1001) == 0b1000 && Negate(0b1000) == 0b1001);

#[inline(always)]
pub const fn GetMaxLit(l: TULit) -> TULit
{
	if IsNeg(l) { l } else { Negate(l) }
}
const _: () = assert!(GetMaxLit(0b1001) == 0b1001 && GetMaxLit(0b1000) == 0b1001);

#[inline(always)]
pub const fn ExternalLit2ExternalVar(l: TLit) -> TLit
{
	if l > 0 { l } else { l.wrapping_neg() }
}

// Initialization-related: the initial number of entries in the fused buffer: set the original size to 4Mb
pub const InitEntriesInB: usize = 0x400000 / std::mem::size_of::<TULit>();

// The assumed number of bytes in one page
pub const BytesInPage: usize = 64;
const _: () = assert!(BytesInPage.is_power_of_two());

// The number of literals in one page; relevant for watch allocation
pub const LitsInPage: usize = BytesInPage / std::mem::size_of::<TULit>();
const _: () = assert!(LitsInPage.is_power_of_two());
// Make sure that, initially, we allocate a sufficient number of entries in the buffer to accommodate LitsInPage literals
const _: () = assert!(LitsInPage <= InitEntriesInB);

// The unsigned index into the clause and watch buffers TUInd
// The solver is designed to be able to work with any index type as long as sizeof(TUInd) >= sizeof(TLit) and sizeof(TUInd) <= sizeof(size_t)
// The unsafe code relies on that bound, so the trait is sealed: it is implemented for u32 and u64 only, each checked at compile time in impl_tuind
mod sealed
{
	pub trait Sealed {}
}

pub trait TUInd: sealed::Sealed + Copy + PartialEq + Eq + PartialOrd + Ord + Hash + Default + Debug + Display + LowerHex + Add<Output = Self> + Sub<Output = Self> + AddAssign + SubAssign + 'static
{
	const DIGITS: u32;
	// The number of literal-entries in an index-entry
	const LitsInInd: usize = (Self::DIGITS / TULit::BITS) as usize;
	const MAX: Self;
	const ZERO: Self;
	const ONE: Self;
	// The conversions are C-style casts, that is, they truncate, if required
	fn from_usize(v: usize) -> Self;
	fn from_u64(v: u64) -> Self;
	fn from_u32(v: u32) -> Self;
	fn as_usize(self) -> usize;
	fn as_u64(self) -> u64;
	fn as_u32(self) -> u32;
	/// *(TUInd*)p
	/// # Safety
	/// p must point at LitsInInd valid literal entries
	unsafe fn read(p: *const TULit) -> Self;
	/// *(TUInd*)p = this
	/// # Safety
	/// p must point at LitsInInd valid literal entries
	unsafe fn write(self, p: *mut TULit);
}

macro_rules! impl_tuind
{
	($t:ty) =>
	{
		// TUInd must be at least as wide as TLit (since the clause and watch buffers will store literals, amongst others) and must not be wider than size_t
		const _: () = assert!(<$t>::BITS >= TULit::BITS && <$t>::BITS <= usize::BITS);

		impl sealed::Sealed for $t {}

		impl TUInd for $t
		{
			const DIGITS: u32 = <$t>::BITS;
			const MAX: Self = <$t>::MAX;
			const ZERO: Self = 0;
			const ONE: Self = 1;
			#[inline(always)]
			fn from_usize(v: usize) -> Self { v as $t }
			#[inline(always)]
			fn from_u64(v: u64) -> Self { v as $t }
			#[inline(always)]
			fn from_u32(v: u32) -> Self { v as $t }
			#[inline(always)]
			fn as_usize(self) -> usize { self as usize }
			#[inline(always)]
			fn as_u64(self) -> u64 { self as u64 }
			#[inline(always)]
			fn as_u32(self) -> u32 { self as u32 }
			#[inline(always)]
			unsafe fn read(p: *const TULit) -> Self { std::ptr::read_unaligned(p as *const $t) }
			#[inline(always)]
			unsafe fn write(self, p: *mut TULit) { std::ptr::write_unaligned(p as *mut $t, self) }
		}
	};
}

impl_tuind!(u32);
impl_tuind!(u64);

// A sequence of literals with random access: implemented by spans and by (potentially compressed) clauses
// This is the equivalent of the TULitSpan template parameter in the C++ version
pub trait TULitSpan: Copy
{
	fn size(&self) -> usize;
	fn get(&self, i: usize) -> TULit;
	fn set(&self, i: usize, l: TULit);
	#[inline(always)]
	fn swap(&self, i: usize, j: usize)
	{
		let li = self.get(i);
		let lj = self.get(j);
		self.set(i, lj);
		self.set(j, li);
	}
	#[inline(always)]
	fn back(&self) -> TULit
	{
		self.get(self.size() - 1)
	}
	#[inline(always)]
	fn set_back(&self, l: TULit)
	{
		self.set(self.size() - 1, l)
	}
	// std::find(begin(), end(), l) - begin()
	#[inline(always)]
	fn find(&self, l: TULit) -> usize
	{
		let mut i = 0;
		while i < self.size() && self.get(i) != l
		{
			i += 1;
		}
		i
	}
	#[inline(always)]
	fn lits(&self) -> TULitSpanIter<Self>
	{
		TULitSpanIter { m_s: *self, m_i: 0 }
	}
}

pub struct TULitSpanIter<S: TULitSpan>
{
	m_s: S,
	m_i: usize,
}

impl<S: TULitSpan> Iterator for TULitSpanIter<S>
{
	type Item = TULit;
	#[inline(always)]
	fn next(&mut self) -> Option<TULit>
	{
		if self.m_i < self.m_s.size()
		{
			let l = self.m_s.get(self.m_i);
			self.m_i += 1;
			Some(l)
		}
		else
		{
			None
		}
	}
}

impl TULitSpan for Span<TULit>
{
	#[inline(always)]
	fn size(&self) -> usize
	{
		Span::size(self)
	}
	#[inline(always)]
	fn get(&self, i: usize) -> TULit
	{
		Span::get(self, i)
	}
	#[inline(always)]
	fn set(&self, i: usize, l: TULit)
	{
		Span::set(self, i, l)
	}
}

// The custom min_element of the solver: the index of the first element in [first, last) with the minimal value (or last, if the range is empty)
#[inline(always)]
pub fn min_element<S: TULitSpan, V: PartialOrd>(lits: S, mut first: usize, last: usize, mut get_val: impl FnMut(TULit) -> V) -> usize
{
	if first == last
	{
		return last;
	}

	let mut smallest = first;
	let mut smallest_val = get_val(lits.get(first));
	first += 1;
	while first != last
	{
		let current_val = get_val(lits.get(first));
		if current_val < smallest_val
		{
			smallest = first;
			smallest_val = current_val;
		}
		first += 1;
	}
	smallest
}

// The custom max_element of the solver: the index of the first element in [first, last) with the maximal value (or last, if the range is empty)
#[inline(always)]
pub fn max_element<S: TULitSpan, V: PartialOrd>(lits: S, mut first: usize, last: usize, mut get_val: impl FnMut(TULit) -> V) -> usize
{
	if first == last
	{
		return last;
	}

	let mut largest = first;
	let mut largest_val = get_val(lits.get(first));
	first += 1;
	while first != last
	{
		let current_val = get_val(lits.get(first));
		if current_val > largest_val
		{
			largest = first;
			largest_val = current_val;
		}
		first += 1;
	}
	largest
}

// std::min_element with a "less" comparator: the index of the first minimal element in [first, last) (or last, if the range is empty)
#[inline(always)]
pub fn std_min_element<S: TULitSpan>(lits: S, mut first: usize, last: usize, mut less: impl FnMut(TULit, TULit) -> bool) -> usize
{
	if first == last
	{
		return last;
	}

	let mut smallest = first;
	first += 1;
	while first != last
	{
		if less(lits.get(first), lits.get(smallest))
		{
			smallest = first;
		}
		first += 1;
	}
	smallest
}

// Moves the elements in [first, last), for which p doesn't hold, to the beginning of the range by swapping; returns the index of the new logical end
#[inline(always)]
pub fn move_if<S: TULitSpan>(lits: S, mut first: usize, last: usize, mut p: impl FnMut(TULit) -> bool) -> usize
{
	while first != last && !p(lits.get(first))
	{
		first += 1;
	}
	if first != last
	{
		let mut i = first + 1;
		while i != last
		{
			if !p(lits.get(i))
			{
				lits.swap(first, i);
				first += 1;
			}
			i += 1;
		}
	}
	first
}

// To be used by compressed buffer management
pub const lowestClsSizePerBits: [u32; 32] =
{
	let mut lcspb = [0u32; 32];

	lcspb[0] = 3;
	let mut curr_bits = 1;
	while curr_bits < 32
	{
		// 2^n - n + 3
		lcspb[curr_bits] = (1u32 << curr_bits) - curr_bits as u32 + 3;
		curr_bits += 1;
	}
	lcspb
};

const _: () = assert!(lowestClsSizePerBits[0] == 3);
const _: () = assert!(lowestClsSizePerBits[1] == 4);
const _: () = assert!(lowestClsSizePerBits[2] == 5);
const _: () = assert!(lowestClsSizePerBits[3] == 8);
const _: () = assert!(lowestClsSizePerBits[4] == 15);
const _: () = assert!(lowestClsSizePerBits[5] == 30);
const _: () = assert!(lowestClsSizePerBits[6] == 61);
const _: () = assert!(lowestClsSizePerBits[7] == 124);
const _: () = assert!(lowestClsSizePerBits[8] == 251);
const _: () = assert!(lowestClsSizePerBits[9] == 506);

pub fn HexStr<T: LowerHex>(val: T, width: usize) -> String
{
	format!("{:0width$x}", val, width = width)
}

pub fn HexStrInd<Ind: TUInd>(val: Ind) -> String
{
	HexStr(val, (Ind::DIGITS / 4) as usize)
}

#[inline(always)]
pub fn memMbVec<T>(v: &[T]) -> usize
{
	std::mem::size_of_val(v) / 1000
}

/*
* CLAUSE-BUFFER
*/

// BIT-COMPRESSED BUFFERS

// The minimal clause size
pub const BCMinClsSize: u8 = 3;

// The structure of an index into the bit-compressed buffers
//		1 bit: is learnt clause
pub const BitsForLearnt: usize = 1;
//		5 bits: bits for [clause size]
//			hence, maximal clause size for M=3: N^max = 2^{S+1}+M-2 = 2^{31+1}+3-2 = 4,294,967,297, since S=2^5-1=31 for 5 bits here
pub const BitsForClsSize: usize = 5;
//		5 (if literal is 32-bit) bits: bits for each literal
//			hence, the maximal literal is 2^(2^5=32) = 4,294,967,296
pub const BitsForLit: usize = (usize::BITS - (TULit::BITS as usize - 1).leading_zeros()) as usize;
const _: () = assert!(BitsForLit == 5);
//		[is-learnt:1][size bits:5][max literal width:5]
pub const BitsForHashId: usize = BitsForLearnt + BitsForClsSize + BitsForLit;

// Buffer structure:
// Initial clause layout in the buffer with the widths
// [size: m_BitsForClsSize] [l1: BitsForLit] [l2: BitsForLit] ... [ln: BitsForLit]
// Deleted clause: [size: m_BitsForClsSize] [0: BitsForLit] [optional-binlit1: BitsForLit] [optional-binlit2: BitsForLit] ... [ln: BitsForLit]

// Conflict clause layout in the buffer:
// [size: m_BitsForClsSize] [glue: min(11, m_BitsForClsSize+2)] [stay-for-another-round: 1] [activity: 31] [l1: BitsForLit] [l2: BitsForLit] ... [ln: BitsForLit]
// Deleted clause: [size: m_BitsForClsSize] [glue: min(11, m_BitsForClsSize+2)] [stay-for-another-round: 1] [0: BitsForLit] [optional-binlit1: BitsForLit] [optional-binlit2: BitsForLit] ... [ln: BitsForLit]

// Deleted literal in any buffer: [0: BitsForLit]. Works, since:
// (1) There can be no deleted literals for clauses of size 3, whose number of bits for clause size is 0 (since binary clauses are stored separately)
// (2) the number of bits for a literal is never lower than for the clause size

// The bit-fields, starting from the LSB: [m_IsLearnt : BitsForLearnt][m_BitsForClsSize : BitsForClsSize][m_BitsForLit : BitsForLit][m_BitsRest]
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct TBCHashId
{
	pub m_Val: u16,
}

const HashIdClsSizeShift: usize = BitsForLearnt;
const HashIdLitShift: usize = BitsForLearnt + BitsForClsSize;
const HashIdClsSizeMask: u64 = (1 << BitsForClsSize) - 1;
const HashIdLitMask: u64 = (1 << BitsForLit) - 1;

impl TBCHashId
{
	#[inline(always)]
	pub fn from_val(v: u16) -> Self
	{
		TBCHashId { m_Val: v }
	}

	#[inline(always)]
	pub fn new(is_learnt: bool, bits_for_cls_size: u16, bits_for_lit: u16) -> Self
	{
		let h = TBCHashId { m_Val: is_learnt as u16 | ((bits_for_cls_size & HashIdClsSizeMask as u16) << HashIdClsSizeShift) | ((bits_for_lit & HashIdLitMask as u16) << HashIdLitShift) };
		debug_assert!(h.m_BitsForClsSize() <= h.m_BitsForLit());
		h
	}

	#[inline(always)]
	pub fn m_IsLearnt(&self) -> bool
	{
		(self.m_Val & 1) != 0
	}

	#[inline(always)]
	pub fn m_BitsForClsSize(&self) -> u8
	{
		((self.m_Val >> HashIdClsSizeShift) & HashIdClsSizeMask as u16) as u8
	}

	#[inline(always)]
	pub fn m_BitsForLit(&self) -> u8
	{
		((self.m_Val >> HashIdLitShift) & HashIdLitMask as u16) as u8
	}

	pub const fn BCHashIdBits() -> u8
	{
		BitsForHashId as u8
	}

	#[inline(always)]
	pub fn val(&self) -> u16
	{
		self.m_Val
	}

	#[inline(always)]
	pub fn IsError(&self) -> bool
	{
		self.m_BitsForLit() == 0
	}

	#[inline(always)]
	pub fn SetError(&mut self)
	{
		self.m_Val &= !((HashIdLitMask as u16) << HashIdLitShift);
	}

	#[inline(always)]
	pub fn GetBitsGlue(&self) -> u8
	{
		if (self.m_BitsForClsSize() as usize + 2) < BitsForHashId { self.m_BitsForClsSize() + 2 } else { BitsForHashId as u8 }
	}

	#[inline(always)]
	pub fn GetFirstLitOffset(&self) -> u8
	{
		self.m_BitsForClsSize() + self.m_IsLearnt() as u8 * (self.GetBitsGlue() + Self::GetBitsActivityAndSkipDel())
	}

	#[inline(always)]
	pub fn MaxGlue(&self) -> TUV
	{
		((1 as TUV) << self.GetBitsGlue()) - 1
	}

	pub const fn GetBitsSkipDel() -> u8
	{
		1
	}

	pub const fn GetBitsActivity() -> u8
	{
		31
	}

	pub const fn GetBitsActivityAndSkipDel() -> u8
	{
		Self::GetBitsActivity() + Self::GetBitsSkipDel()
	}
}

// Clause-sizes: bits-for-clause-size
// 3: 0 (special value not needed, since literals are not deleted for clauses of size 3; otherwise, special value 0 means that this is not a clause, but a deleted literal)
// 4: 1 (0 -- special, 1)
// 5-7: 2 (0 -- special, 1-3)
// 8-14: 3 (0 -- special, 1-7)
// 15-29: 4 (0 -- special, 1-15)
// 30-60: 5 (0 -- special, 1-31)
// 61-123: 6 (0 -- special, 1-63)
// 124-250: 7 (0 -- special, 1-127)
// 251-505: 8 (0 -- special, 1-255)
// 506-1016: 9 (0 -- special, 1-511)
// ...
// stored in lowestClsSizePerBits, where the function for low bound is 2^n - n + 3

pub const fn BCClsSize2Bits(cls_size: u64) -> u16
{
	debug_assert!(cls_size >= BCMinClsSize as u64);
	debug_assert!(cls_size <= u32::MAX as u64);
	// upper_bound: the first element greater than clsSize
	let mut it_first_greater = 0;
	while it_first_greater < lowestClsSizePerBits.len() && lowestClsSizePerBits[it_first_greater] <= cls_size as u32
	{
		it_first_greater += 1;
	}
	it_first_greater as u16 - 1
}

pub const fn BCClsSize2EncodedClsSize(cls_size: u64) -> u32
{
	debug_assert!(cls_size >= BCMinClsSize as u64);
	debug_assert!(cls_size <= u32::MAX as u64);
	if cls_size == BCMinClsSize as u64 { 0 } else { cls_size as u32 - lowestClsSizePerBits[BCClsSize2Bits(cls_size) as usize] + 1 }
}

// Encoded clause size + bits --> clause size
pub const fn BCEncodedClsSize2ClsSizeConst(encoded_cls_size: u32, bits_for_cls_size: u16) -> u32
{
	if bits_for_cls_size == 0
	{
		return BCMinClsSize as u32;
	}

	debug_assert!(encoded_cls_size > 0);
	encoded_cls_size + lowestClsSizePerBits[bits_for_cls_size as usize] - 1
}

const _: () = assert!(BCClsSize2Bits(3) == 0);
const _: () = assert!(BCClsSize2Bits(4) == 1);
const _: () = assert!(BCClsSize2Bits(5) == 2);
const _: () = assert!(BCClsSize2Bits(6) == 2);
const _: () = assert!(BCClsSize2Bits(7) == 2);
const _: () = assert!(BCClsSize2Bits(8) == 3);
const _: () = assert!(BCClsSize2Bits(9) == 3);
const _: () = assert!(BCClsSize2Bits(14) == 3);
const _: () = assert!(BCClsSize2Bits(15) == 4);
const _: () = assert!(BCClsSize2Bits(16) == 4);
const _: () = assert!(BCClsSize2Bits(29) == 4);
const _: () = assert!(BCClsSize2Bits(30) == 5);
const _: () = assert!(BCClsSize2Bits(31) == 5);

const _: () = assert!(BCClsSize2EncodedClsSize(3) == 0);
const _: () = assert!(BCClsSize2EncodedClsSize(4) == 1);
const _: () = assert!(BCClsSize2EncodedClsSize(5) == 1);
const _: () = assert!(BCClsSize2EncodedClsSize(6) == 2);
const _: () = assert!(BCClsSize2EncodedClsSize(7) == 3);
const _: () = assert!(BCClsSize2EncodedClsSize(8) == 1);
const _: () = assert!(BCClsSize2EncodedClsSize(9) == 2);
const _: () = assert!(BCClsSize2EncodedClsSize(14) == 7);
const _: () = assert!(BCClsSize2EncodedClsSize(15) == 1);

const _: () = assert!(BCEncodedClsSize2ClsSizeConst(0, 0) == 3);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(1, 1) == 4);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(1, 2) == 5);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(2, 2) == 6);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(3, 2) == 7);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(1, 3) == 8);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(2, 3) == 9);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(7, 3) == 14);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(1, 4) == 15);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(2, 4) == 16);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(15, 4) == 29);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(1, 5) == 30);
const _: () = assert!(BCEncodedClsSize2ClsSizeConst(2, 5) == 31);

// TUInd for the compressed buffer (used, e.g., for parents). Must have the same size as TUInd.
// The bit-fields, starting from the LSB: [m_IsLearnt : BitsForLearnt][m_BitsForClsSize : BitsForClsSize][m_BitsForLit : BitsForLit][m_BitStart : BitsForBCParentClsInd]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TBCInd<Ind: TUInd>
{
	pub m_Val: Ind,
}

impl<Ind: TUInd> TBCInd<Ind>
{
	// Bits in the parent, remaining for the index into the clause bit-array, after counting hash-index's BitsForHashId bits
	pub const BitsForBCParentClsInd: usize = Ind::DIGITS as usize - BitsForHashId;
	pub const m_MaxBitStart: u64 = (1u64 << Self::BitsForBCParentClsInd as u64) - 1;

	#[inline(always)]
	pub fn from_ind(val: Ind) -> Self
	{
		TBCInd { m_Val: val }
	}

	#[inline(always)]
	pub fn new(bc_hash_id: TBCHashId, bit_start: u64) -> Self
	{
		let res = if bc_hash_id.m_BitsForLit() == 0 || bit_start > Self::m_MaxBitStart
		{
			TBCInd { m_Val: Ind::ZERO }
		}
		else
		{
			TBCInd { m_Val: Ind::from_u64((bc_hash_id.m_Val as u64 & ((1u64 << BitsForHashId) - 1)) | (bit_start << BitsForHashId)) }
		};
		debug_assert!(res.m_BitsForClsSize() <= res.m_BitsForLit());
		res
	}

	#[inline(always)]
	pub fn m_IsLearnt(&self) -> bool
	{
		(self.m_Val.as_u64() & 1) != 0
	}

	#[inline(always)]
	pub fn m_BitsForClsSize(&self) -> u8
	{
		((self.m_Val.as_u64() >> HashIdClsSizeShift) & HashIdClsSizeMask) as u8
	}

	#[inline(always)]
	pub fn m_BitsForLit(&self) -> u8
	{
		((self.m_Val.as_u64() >> HashIdLitShift) & HashIdLitMask) as u8
	}

	#[inline(always)]
	pub fn m_BitStart(&self) -> u64
	{
		self.m_Val.as_u64() >> BitsForHashId
	}

	#[inline(always)]
	pub fn IsError(&self) -> bool
	{
		self.m_Val == Ind::ZERO
	}

	#[inline(always)]
	pub fn ind(&self) -> Ind
	{
		self.m_Val
	}

	#[inline(always)]
	pub fn GetHashId(&self) -> TBCHashId
	{
		TBCHashId::new(self.m_IsLearnt(), self.m_BitsForClsSize() as u16, self.m_BitsForLit() as u16)
	}

	#[inline(always)]
	pub fn BitFirstLit(&self) -> u64
	{
		self.m_BitStart() + self.GetHashId().GetFirstLitOffset() as u64
	}

	#[inline(always)]
	pub fn GetBitsGlue(&self) -> u8
	{
		self.GetHashId().GetBitsGlue()
	}
}

// Bit-compressed buffers, indexed by TBCHashId
// The C++ version uses unordered_map<uint16_t, CBitArray>. Since a hash-id is BitsForHashId (=11) bits wide, we use a direct-index table instead, where:
// (1) the look-up is strictly faster (so there is no need for caching the last accessed bit-array);
// (2) pointers and iterators are never invalidated by an insertion (so there is no need for the spare map m_BCSpare);
// (3) the iteration order is well-defined: ascending hash-id (it is implementation-defined for unordered_map)
pub struct TCompressedClss
{
	m_Table: Box<[*mut CBitArray]>,
	m_Size: usize,
}

pub const BCTableSize: usize = 1 << BitsForHashId;

impl Default for TCompressedClss
{
	fn default() -> Self
	{
		TCompressedClss { m_Table: vec![std::ptr::null_mut(); BCTableSize].into_boxed_slice(), m_Size: 0 }
	}
}

impl TCompressedClss
{
	#[inline(always)]
	pub fn size(&self) -> usize
	{
		self.m_Size
	}

	// m_BC.at(i)
	#[inline(always)]
	pub fn at(&self, i: u16) -> *mut CBitArray
	{
		debug_assert!(!self.m_Table[i as usize].is_null());
		unsafe { *self.m_Table.get_unchecked(i as usize) }
	}

	// m_BC.find(i) != m_BC.end()
	#[inline(always)]
	pub fn exists(&self, i: u16) -> bool
	{
		!self.m_Table[i as usize].is_null()
	}

	// m_BC.insert(make_pair(i, CBitArray())).first->second; returns nullptr, if the insertion fails because of an allocation failure
	pub fn insert(&mut self, i: u16) -> *mut CBitArray
	{
		if self.m_Table[i as usize].is_null()
		{
			#[cfg(any(test, fuzzing))]
			if crate::topor_dyn_array::alloc_fault::should_fail()
			{
				return std::ptr::null_mut();
			}
			// Box::new would abort on an allocation failure, so the box is allocated by hand (erase releases it with Box::from_raw, which expects this very layout)
			let layout = std::alloc::Layout::new::<CBitArray>();
			// SAFETY: CBitArray isn't zero-sized, so the layout is valid for alloc
			let p = unsafe { std::alloc::alloc(layout) } as *mut CBitArray;
			if p.is_null()
			{
				return std::ptr::null_mut();
			}
			// SAFETY: p is a fresh allocation, which is suitably sized and aligned for a CBitArray
			unsafe { p.write(CBitArray::new()) };
			self.m_Table[i as usize] = p;
			self.m_Size += 1;
		}
		self.m_Table[i as usize]
	}

	pub fn erase(&mut self, i: u16)
	{
		if !self.m_Table[i as usize].is_null()
		{
			drop(unsafe { Box::from_raw(self.m_Table[i as usize]) });
			self.m_Table[i as usize] = std::ptr::null_mut();
			self.m_Size -= 1;
		}
	}

	// The first existing hash-id, which is greater than or equal to i; BCTableSize stands for end()
	#[inline(always)]
	pub fn lower_bound(&self, mut i: usize) -> usize
	{
		while i < BCTableSize && self.m_Table[i].is_null()
		{
			i += 1;
		}
		i
	}

	#[inline(always)]
	pub fn begin(&self) -> usize
	{
		self.lower_bound(0)
	}

	#[inline(always)]
	pub fn end(&self) -> usize
	{
		BCTableSize
	}

	// ++it
	#[inline(always)]
	pub fn next(&self, it: usize) -> usize
	{
		self.lower_bound(it + 1)
	}

	pub fn memMb(&self) -> usize
	{
		let mut res = self.size() * std::mem::size_of::<u16>();
		let mut it = self.begin();
		while it != self.end()
		{
			res += unsafe { (*self.at(it as u16)).memMb() };
			it = self.next(it);
		}
		res
	}
}

impl Drop for TCompressedClss
{
	fn drop(&mut self)
	{
		let mut it = self.begin();
		while it != self.end()
		{
			self.erase(it as u16);
			it = self.next(it);
		}
	}
}

// Fixed layout for clause with n literals

// Not over-sized clause header:
// [is-learnt:1][glue:11][size:20] (for 32-bit-literals, otherwise see below)
// [1:stay-for-another-round][activity:31] (must not be -1 for not over-sized; 32 for <=32-bit-literals, n for n>32-bit-literals)

// Over-sized clause header:
// [is-learnt:1][size:31] (for 32 bits, otherwise see below)
// 11111111111111111111111111111111:32 (32 for <=32-bit-literals, n for n>32-bit-literals)
// [1:stay-for-another-round][activity:31] (32 for <=32-bit-literals, n for n>32-bit-literals)

// LITERALS:
// literal-1
// literal-2
// .........
// literal-n

pub const ClsIsLearntBits: TUV = 1;
pub const ClsLShiftToIsLearntOn: TUV = TUV::BITS - 1;
pub const ClsIsLearntMask: TUV = (1 as TUV) << ClsLShiftToIsLearntOn;

pub const ClsGlueBits: TUV = if TUV::BITS == 8 { 3 } else if TUV::BITS == 16 { 5 } else if TUV::BITS == 32 { 11 } else { 20 };
pub const ClsMaxGlue: TUV = ((1 as TUV) << ClsGlueBits) - 1;
pub const ClsGlueMask: TUV = ClsMaxGlue << (TUV::BITS - ClsIsLearntBits - ClsGlueBits);

pub const ClsIsLearntAndGlueMask: TUV = ClsIsLearntMask | ClsGlueMask;

pub const ClsSizeBits: TUV = TUV::BITS - ClsIsLearntBits - ClsGlueBits;
pub const ClsLearntMaxSizeWithGlue: TUV = ((1 as TUV) << ClsSizeBits) - 1;
pub const ClsLearntMaxSizeWithoutGlue: TUV = ((1 as TUV) << (ClsSizeBits + ClsGlueBits)) - 1;

pub const ClsOversizeActivity: i32 = -1;

pub const ClsActivityFields: TUV = if TUV::BITS >= 32 { 1 } else { 32 / TUV::BITS };
pub const ClsActivityFieldsLshift: TUV = ClsActivityFields.trailing_zeros();

pub const ClsLShiftSkipDel: u32 = u32::BITS - 1;
pub const ClsSkipdelMask: u32 = 1u32 << ClsLShiftSkipDel;
pub const ClsNotSkipdelMask: u32 = !ClsSkipdelMask;

// This is the limit on the number of ConstClsSpan's to be used simultaneously in the same context
// The current limit is 3. This is crucial to remember for the developers.
pub const m_TmpClssCountMax: usize = 3;

// The clause: a view of the literals of a clause in the clause buffer
// In the standard mode, it's a span into the explicit buffer; in the compressed mode, it simulates a standard span with literals, given a bit-array
// Exactly as in C++, any access is in fact always by reference to the buffer; the user must invoke Update, if the clause size is changed or the buffer is reallocated
// The C++ iterators are replaced by indices into the clause
pub struct CCls<Ind: TUInd, const Compress: bool>
{
	pub(crate) m_ClsInd: Ind,
	// Standard clause
	pub(crate) m_Span: TSpanTULit,
	// Compressed clause
	pub(crate) m_Ba: *mut CBitArray,
	pub(crate) m_BitFirstLit: u64,
	pub(crate) m_BitsForLit: u8,
	pub(crate) m_ClsSize: usize,
}

impl<Ind: TUInd, const Compress: bool> Clone for CCls<Ind, Compress>
{
	#[inline(always)]
	fn clone(&self) -> Self
	{
		*self
	}
}
impl<Ind: TUInd, const Compress: bool> Copy for CCls<Ind, Compress> {}

impl<Ind: TUInd, const Compress: bool> CCls<Ind, Compress>
{
	#[inline(always)]
	pub fn new_standard(cls_ind: Ind, span: TSpanTULit) -> Self
	{
		CCls { m_ClsInd: cls_ind, m_Span: span, m_Ba: std::ptr::null_mut(), m_BitFirstLit: 0, m_BitsForLit: 0, m_ClsSize: span.size() }
	}

	#[inline(always)]
	pub fn new_compressed(ba: *mut CBitArray, bc_ind: TBCInd<Ind>) -> Self
	{
		let cls_size = Self::compressed_cls_size(ba, bc_ind);
		debug_assert!(cls_size > 0);
		CCls { m_ClsInd: bc_ind.ind(), m_Span: Span::empty(), m_Ba: ba, m_BitFirstLit: bc_ind.BitFirstLit(), m_BitsForLit: bc_ind.m_BitsForLit(), m_ClsSize: cls_size }
	}

	#[inline(always)]
	pub(crate) fn compressed_cls_size(ba: *mut CBitArray, bc_ind: TBCInd<Ind>) -> usize
	{
		if bc_ind.m_BitsForClsSize() == 0
		{
			return BCMinClsSize as usize;
		}
		let encoded_cls_size = unsafe { (*ba).bit_get(bc_ind.m_BitStart(), bc_ind.m_BitsForClsSize()) } as u32;
		BCEncodedClsSize2ClsSizeConst(encoded_cls_size, bc_ind.m_BitsForClsSize() as u16) as usize
	}

	#[inline(always)]
	pub fn cls_ind(&self) -> Ind
	{
		self.m_ClsInd
	}

	// The clause as a standard span; legal in the standard mode only
	#[inline(always)]
	pub fn span(&self) -> TSpanTULit
	{
		debug_assert!(!Compress);
		self.m_Span
	}
}

impl<Ind: TUInd, const Compress: bool> TULitSpan for CCls<Ind, Compress>
{
	#[inline(always)]
	fn size(&self) -> usize
	{
		self.m_ClsSize
	}

	#[inline(always)]
	fn get(&self, i: usize) -> TULit
	{
		if Compress
		{
			unsafe { (*self.m_Ba).bit_get(self.m_BitFirstLit + self.m_BitsForLit as u64 * i as u64, self.m_BitsForLit) as TULit }
		}
		else
		{
			self.m_Span.get(i)
		}
	}

	#[inline(always)]
	fn set(&self, i: usize, l: TULit)
	{
		if Compress
		{
			unsafe { (*self.m_Ba).bit_set(l as u64, self.m_BitsForLit, self.m_BitFirstLit + self.m_BitsForLit as u64 * i as u64) }
		}
		else
		{
			self.m_Span.set(i, l)
		}
	}
}

// *** ConstDenseSnapshot ***
// Const: I don't modify
// Snapshot: others don't modify (otherwise, if they do, I'm expected to ignore)
// Dense: visiting all (up to maxLitsNum, if specified) literals
// Properties: always holds a TSpanTULit of the clause's literals
// Notes: in compressed mode, implemented by reading all (up to maxLitsNum, if specified) lits upfront into an array (which is then used for the span)
pub struct CClsConstSnapshotDense
{
	pub(crate) m_ClsLits: Vec<TULit>,
	pub(crate) m_Span: TSpanTULit,
	#[cfg(debug_assertions)]
	pub(crate) m_ClsInitially: Vec<TULit>,
}

impl CClsConstSnapshotDense
{
	pub(crate) fn new_standard(span: TSpanTULit) -> Self
	{
		CClsConstSnapshotDense
		{
			m_ClsLits: Vec::new(),
			m_Span: span,
			#[cfg(debug_assertions)]
			m_ClsInitially: span.to_vec(),
		}
	}

	pub(crate) fn new_compressed(cls_lits: Vec<TULit>) -> Self
	{
		let mut res = CClsConstSnapshotDense
		{
			#[cfg(debug_assertions)]
			m_ClsInitially: cls_lits.clone(),
			m_ClsLits: cls_lits,
			m_Span: Span::empty(),
		};
		res.m_Span = Span::from_slice(&mut res.m_ClsLits);
		res
	}

	#[inline(always)]
	pub fn span(&self) -> TSpanTULit
	{
		self.m_Span
	}

	#[inline(always)]
	pub fn get(&self, i: usize) -> TULit
	{
		self.m_Span.get(i)
	}

	#[inline(always)]
	pub fn back(&self) -> TULit
	{
		self.m_Span.back()
	}

	#[inline(always)]
	pub fn size(&self) -> usize
	{
		self.m_Span.size()
	}

	#[inline(always)]
	pub fn iter(&self) -> crate::topor_dyn_array::SpanIter<TULit>
	{
		self.m_Span.iter()
	}
}

impl Drop for CClsConstSnapshotDense
{
	fn drop(&mut self)
	{
		#[cfg(debug_assertions)]
		debug_assert!(self.m_Span.as_slice() == &self.m_ClsInitially[..]);
	}
}

/*
* WATCHES
*/

// The struct represents the Watch information per one literal
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct TWatchInfo<Ind: TUInd>
{
	pub m_WBInd: Ind,
	pub m_AllocatedEntries: Ind,
	pub m_LongWatches: Ind,
	pub m_BinaryWatches: Ind,
}

impl<Ind: TUInd> TWatchInfo<Ind>
{
	pub const BinsInLong: usize = 1 + Ind::LitsInInd;
	pub const BinsInLongBitCeil: usize = Self::BinsInLong.next_power_of_two();
	pub const BinsInLongBytes: usize = std::mem::size_of::<TULit>() * Self::BinsInLong;
	// rotr((TUInd)1, 1)
	pub const MaxWatchInfoAllocShift: u32 = Ind::DIGITS - 1;

	#[inline(always)]
	pub fn MaxWatchInfoAlloc() -> Ind
	{
		Ind::from_u64(1u64 << Self::MaxWatchInfoAllocShift)
	}

	#[inline(always)]
	pub fn PointToNewArena(&mut self, b_ind: Ind, allocated_entries: Ind)
	{
		self.m_WBInd = b_ind;
		self.m_AllocatedEntries = allocated_entries;
	}

	#[inline(always)]
	pub fn GetLongEntries(&self) -> usize
	{
		self.m_LongWatches.as_usize() * Self::BinsInLong
	}

	#[inline(always)]
	pub fn GetUsedEntries(&self) -> usize
	{
		self.GetLongEntries() + self.m_BinaryWatches.as_usize()
	}

	#[inline(always)]
	pub fn GetLongEntry(&self, long_watch_ind: usize) -> usize
	{
		long_watch_ind * Self::BinsInLong
	}

	#[inline(always)]
	pub fn GetBinEntry(&self, bin_watch_ind: usize) -> usize
	{
		self.GetLongEntries() + bin_watch_ind
	}

	#[inline(always)]
	pub fn IsEmpty(&self) -> bool
	{
		debug_assert!(self.m_WBInd != Ind::ZERO || (self.m_AllocatedEntries == Ind::ZERO && self.m_BinaryWatches == Ind::ZERO && self.m_LongWatches == Ind::ZERO));
		self.m_AllocatedEntries == Ind::ZERO
	}
}

/*
* DECISION LEVELS, ASSIGNMENTS, TRAIL
*/

// Contradiction information (meant to be returned by BCP for conflict analysis)
#[derive(Clone, Copy)]
pub struct TContradictionInfo<Ind: TUInd>
{
	// Parent clause index (for a contradiction in a long clause)
	pub m_ParentClsInd: Ind,
	// The binary parent (for a contradiction in a binary clause)
	pub m_BinClause: [TULit; 2],
	pub m_IsContradiction: bool,
	pub m_IsContradictionInBinaryCls: bool,
}

impl<Ind: TUInd> Default for TContradictionInfo<Ind>
{
	fn default() -> Self
	{
		TContradictionInfo { m_ParentClsInd: Ind::ZERO, m_BinClause: [BadULit; 2], m_IsContradiction: false, m_IsContradictionInBinaryCls: false }
	}
}

impl<Ind: TUInd> TContradictionInfo<Ind>
{
	#[inline(always)]
	pub fn new() -> Self
	{
		Self::default()
	}

	#[inline(always)]
	pub fn from_parent(parent_cls_ind: Ind) -> Self
	{
		debug_assert!(parent_cls_ind != Ind::ZERO);
		TContradictionInfo { m_ParentClsInd: parent_cls_ind, m_BinClause: [BadULit; 2], m_IsContradiction: true, m_IsContradictionInBinaryCls: false }
	}

	#[inline(always)]
	pub fn from_bin(bin_clause: [TULit; 2]) -> Self
	{
		debug_assert!(bin_clause[0] != BadULit && bin_clause[1] != BadULit);
		TContradictionInfo { m_ParentClsInd: Ind::ZERO, m_BinClause: bin_clause, m_IsContradiction: true, m_IsContradictionInBinaryCls: true }
	}

	#[inline(always)]
	pub fn IsContradiction(&self) -> bool
	{
		self.m_IsContradiction
	}
}

macro_rules! bit_field
{
	($getter:ident, $setter:ident, $bit:expr) =>
	{
		#[inline(always)]
		pub fn $getter(&self) -> bool
		{
			(self.m_Bits >> $bit) & 1 != 0
		}

		#[inline(always)]
		pub fn $setter(&mut self, v: bool)
		{
			self.m_Bits = (self.m_Bits & !(1u8 << $bit)) | ((v as u8) << $bit);
		}
	};
}

#[derive(Clone, Copy, Default)]
#[repr(transparent)]
pub struct TAssignmentInfo
{
	pub m_Bits: u8,
}

impl TAssignmentInfo
{
	// Assigned?
	bit_field!(m_IsAssigned, set_m_IsAssigned, 0);
	// Assigned with a binary clause parent?
	bit_field!(m_IsAssignedInBinary, set_m_IsAssignedInBinary, 1);
	// Assigned negated?
	bit_field!(m_IsNegated, set_m_IsNegated, 2);
	// Visited for various purposes
	bit_field!(m_Visit, set_m_Visit, 3);
	// Rooted for various purposes (there is no real difference between visited and rooted; we simply sometimes need several lists; the name is different (rather than, e.g., m_Visit2) for readability and error-proneness)
	bit_field!(m_Root, set_m_Root, 4);
	// Assumption variable?
	bit_field!(m_IsAssump, set_m_IsAssump, 5);
	// relevant only if m_IsAssump=true: is the negation of this variable is the assumption?
	bit_field!(m_IsAssumpNegated, set_m_IsAssumpNegated, 6);

	#[inline(always)]
	pub fn Assign<Ind: TUInd>(&mut self, is_negated: bool, parent_cls_ind: Ind, other_watch: TULit)
	{
		self.set_m_IsAssigned(true);
		self.set_m_IsAssignedInBinary(parent_cls_ind == Ind::ZERO && other_watch != BadULit);
		self.set_m_IsNegated(is_negated);
	}

	#[inline(always)]
	pub fn Unassign(&mut self)
	{
		self.set_m_IsAssigned(false);
	}

	#[inline(always)]
	pub fn IsAssignedBinary(&self) -> bool
	{
		self.m_IsAssignedInBinary()
	}
}
const _: () = assert!(std::mem::size_of::<TAssignmentInfo>() == 1);

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct TVarInfo<Ind: TUInd>
{
	// A union in the C++ version:
	// Parent clause for longs
	// The other literal in the clause for binaries (stored in the low bits)
	pub m_ParentClsInd: Ind,
	// Decision level
	pub m_DecLevel: TUV,
	// The previous variable on the trail
	pub m_TrailPrev: TUVar,
	// The next variable on the trail
	pub m_TrailNext: TUVar,
}

impl<Ind: TUInd> TVarInfo<Ind>
{
	#[inline(always)]
	pub fn Assign(&mut self, parent_cls_ind: Ind, other_watch: TULit, dec_level: TUV, trail_prev: TUVar, trail_next: TUVar)
	{
		if parent_cls_ind == Ind::ZERO && other_watch != BadULit
		{
			self.set_m_BinOtherLit(other_watch);
		}
		else
		{
			self.m_ParentClsInd = parent_cls_ind;
		}
		self.m_DecLevel = dec_level;
		self.m_TrailPrev = trail_prev;
		self.m_TrailNext = trail_next;
	}

	#[inline(always)]
	pub fn IsDecVar(&self) -> bool
	{
		self.m_DecLevel != 0 && self.m_ParentClsInd == Ind::ZERO
	}

	// The other literal in the clause for binaries
	#[inline(always)]
	pub fn m_BinOtherLit(&self) -> TULit
	{
		self.m_ParentClsInd.as_u32()
	}

	#[inline(always)]
	pub fn set_m_BinOtherLit(&mut self, l: TULit)
	{
		// The C++ union member m_BinOtherLit overlaps the low bytes of m_ParentClsInd (little-endian), so writing it leaves the rest of a wider index intact
		unsafe { *(&mut self.m_ParentClsInd as *mut Ind as *mut TULit) = l };
	}

	// &m_BinOtherLit
	#[inline(always)]
	pub fn m_BinOtherLitPtr(&mut self) -> *mut TULit
	{
		&mut self.m_ParentClsInd as *mut Ind as *mut TULit
	}
}

#[derive(Clone, Copy, Default)]
#[repr(transparent)]
pub struct TPolarityInfo
{
	pub m_Bits: u8,
}

impl TPolarityInfo
{
	bit_field!(m_IsNextPolarityDetermined, set_m_IsNextPolarityDetermined, 0);
	bit_field!(m_IsNextPolarityNegated, set_m_IsNextPolarityNegated, 1);
	bit_field!(m_IsPolarityFixed, set_m_IsPolarityFixed, 2);

	#[inline(always)]
	pub fn new(is_polarity_fixed: bool, is_next_polarity_negated: bool) -> Self
	{
		let mut pi = TPolarityInfo { m_Bits: 0 };
		pi.set_m_IsNextPolarityDetermined(true);
		pi.set_m_IsNextPolarityNegated(is_next_polarity_negated);
		pi.set_m_IsPolarityFixed(is_polarity_fixed);
		pi
	}

	#[inline(always)]
	pub fn IsNextPolarityDetermined(&self) -> bool
	{
		self.m_IsNextPolarityDetermined()
	}

	#[inline(always)]
	pub fn GetNextPolarityIsNegated(&mut self) -> bool
	{
		debug_assert!(self.m_IsNextPolarityDetermined());
		if !self.m_IsPolarityFixed()
		{
			self.set_m_IsNextPolarityDetermined(false);
		}
		self.m_IsNextPolarityNegated()
	}

	#[inline(always)]
	pub fn Clear(&mut self)
	{
		self.m_Bits &= !0b111;
	}
}
const _: () = assert!(std::mem::size_of::<TPolarityInfo>() == 1);

// Holding delayed implications
#[derive(Clone, Copy)]
pub struct TDelImpl<Ind: TUInd>
{
	pub m_L: TULit,
	pub m_OtherWatch: TULit,
	pub m_ParentClsInd: Ind,
}

impl<Ind: TUInd> TDelImpl<Ind>
{
	#[inline(always)]
	pub fn new(l: TULit, other_watch: TULit, parent_cls_ind: Ind) -> Self
	{
		TDelImpl { m_L: l, m_OtherWatch: other_watch, m_ParentClsInd: parent_cls_ind }
	}
}

#[derive(Clone, Copy)]
pub struct TParentSubsumed<Ind: TUInd>
{
	pub m_L: TULit,
	pub m_IsBinary: bool,
	// A union in the C++ version:
	// Parent clause for longs
	// The other literal in the clause for binaries (stored in the low bits)
	pub m_ParentClsInd: Ind,
}

impl<Ind: TUInd> TParentSubsumed<Ind>
{
	#[inline(always)]
	pub fn new(l: TULit, is_binary: bool, parent_cls_ind: Ind) -> Self
	{
		TParentSubsumed { m_L: l, m_IsBinary: is_binary, m_ParentClsInd: parent_cls_ind }
	}

	#[inline(always)]
	pub fn m_BinOtherLit(&self) -> TULit
	{
		self.m_ParentClsInd.as_u32()
	}
}

#[derive(Clone, Copy, Default)]
pub struct TClsDelInfo
{
	pub m_ConfsPrev: u64,
	pub m_TriggerNext: u64,
	pub m_TriggerInc: u64,
	pub m_TriggerMult: f64,
	// A union of m_TriggerMax and m_CurrChange in the C++ version
	pub m_TriggerMax: u64,
	pub m_FracToDelete: f32,
	pub m_GlueNeverDelete: u8,
	pub m_Clusters: u8,
	pub m_MaxClusteredGlue: u8,
	pub m_Initialized: bool,
}

impl TClsDelInfo
{
	#[inline(always)]
	pub fn m_CurrChange(&mut self) -> &mut u64
	{
		&mut self.m_TriggerMax
	}

	#[inline(always)]
	pub fn GetCluster(&self, glue: TUV) -> u8
	{
		debug_assert!(self.m_Clusters != 0);
		debug_assert!(glue >= self.m_GlueNeverDelete as TUV);

		if glue > self.m_MaxClusteredGlue as TUV
		{
			return u8::MAX;
		}

		// The arithmetic is carried out on int's in C++ (integral promotion), and only the results are truncated to uint8_t
		let mut one_cluster_size = ((self.m_MaxClusteredGlue as i32 - self.m_GlueNeverDelete as i32) / self.m_Clusters as i32) as u8;
		if one_cluster_size == 0
		{
			one_cluster_size = 1;
		}
		((glue as u8 as i32 - self.m_GlueNeverDelete as i32 - 1) / one_cluster_size as i32) as u8
	}
}

/*
* Query-type, including parameters
*/

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum TQueryType
{
	// Initial SAT solver query
	QUERY_INIT,
	// Incremental short query (conflict threshold <= m_ParamShortQueryConfThrInv)
	QUERY_INC_SHORT,
	// Incremental normal query (conflict threshold > m_ParamShortQueryConfThrInv)
	QUERY_INC_NORMAL,
	// Initial value
	QUERY_NONE,
}

/*
* Phase management
*/

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum TPhaseStage
{
	PHASE_STAGE_STANDARD,
	PHASE_STAGE_DONT_FORCE,
}

// Removing duplicates, discovering tautologies and storing incoming clauses
pub struct CHandleNewCls
{
	m_Cls: CVector<TULit>,
	m_LastAppearenceCounter: CDynArray<TCounterType>,
	m_Counter: TCounterType,
}

impl CHandleNewCls
{
	pub fn new(init_var_num: usize) -> Self
	{
		CHandleNewCls { m_Cls: CVector::new(0, 0), m_LastAppearenceCounter: CDynArray::new_init(init_var_num, 0), m_Counter: 0 }
	}

	#[inline(always)]
	pub fn NewClause(&mut self)
	{
		self.m_Counter = self.m_Counter.wrapping_add(1);
		if self.m_Counter < 0
		{
			self.m_LastAppearenceCounter.memset(0);
			self.m_Counter = 1;
		}
		self.m_Cls.clear();
	}

	// Returns <isTau, isDuplicate, isAllocFailed>
	pub fn NewLitIsTauIsDuplicate(&mut self, new_lit: TULit) -> (bool, bool, bool)
	{
		let mut is_tau = false;
		let mut is_duplicate = false;

		let is_negative = IsNeg(new_lit);
		let new_var = GetVar(new_lit);
		let var_exists = new_var < self.m_LastAppearenceCounter.cap() as TUVar;

		if !var_exists
		{
			self.m_LastAppearenceCounter.reserve_atleast_init(new_var as usize + 1, 0);
			if self.m_LastAppearenceCounter.uninitialized_or_erroneous()
			{
				return (is_tau, is_duplicate, true);
			}
			self.m_LastAppearenceCounter[new_var as usize] = if is_negative { -self.m_Counter } else { self.m_Counter };
		}
		else
		{
			let elem_val = self.m_LastAppearenceCounter[new_var as usize];
			if (elem_val as i64).abs() == (self.m_Counter as i64).abs()
			{
				is_duplicate = (elem_val < 0) == is_negative;
				is_tau = !is_duplicate;
			}
			else
			{
				self.m_LastAppearenceCounter[new_var as usize] = if is_negative { -self.m_Counter } else { self.m_Counter };
			}
		}

		if !is_tau && !is_duplicate
		{
			self.m_Cls.push_back(new_lit);
			// The C++ version tests m_LastAppearenceCounter here, which cannot be erroneous at this point; we test the vector the literal has just been pushed to
			if self.m_Cls.uninitialized_or_erroneous()
			{
				return (is_tau, is_duplicate, true);
			}
		}

		(is_tau, is_duplicate, false)
	}

	// Test-only: lets the tests reach the counter wrap-around
	#[cfg(test)]
	pub fn SetCounterForTest(&mut self, counter: TCounterType)
	{
		self.m_Counter = counter;
	}

	#[inline(always)]
	pub fn GetCurrCls(&self) -> TSpanTULit
	{
		self.m_Cls.get_span()
	}

	#[inline(always)]
	pub fn SetMultiplier(&mut self, multiplier: f64)
	{
		self.m_LastAppearenceCounter.SetMultiplier(multiplier);
	}

	#[inline(always)]
	pub fn memMb(&self) -> usize
	{
		self.m_Cls.memMb() + self.m_LastAppearenceCounter.memMb() + std::mem::size_of::<TCounterType>()
	}

	pub fn reserve_exactly_init(&mut self, new_cap: usize, init_val: u8)
	{
		self.m_LastAppearenceCounter.reserve_exactly_init(new_cap, init_val);
	}

	pub fn reserve_exactly(&mut self, new_cap: usize)
	{
		self.m_LastAppearenceCounter.reserve_exactly(new_cap);
	}

	#[inline(always)]
	pub fn uninitialized_or_erroneous(&self) -> bool
	{
		self.m_LastAppearenceCounter.uninitialized_or_erroneous()
	}
}
