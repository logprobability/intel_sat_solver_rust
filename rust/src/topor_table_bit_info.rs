// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

pub const MAX_BITS_NUM: u8 = 64;

pub const fn LsbMask(b: u8) -> u64
{
	if b == 64 { u64::MAX } else { (1u64 << b as u64) - 1u64 }
}

const _: () = assert!(LsbMask(1) == 1);
const _: () = assert!(LsbMask(2) == 3);
const _: () = assert!(LsbMask(3) == 7);
const _: () = assert!(LsbMask(10) == 1023);
const _: () = assert!(LsbMask(64) == u64::MAX);

/*
* Contagious bit-array data structures: the LSB's go into the lower word, so the order is preserved, but for
* the price of an additional shift
*/

#[derive(Clone, Copy)]
pub struct TCInfo
{
	pub w0ReadMask: u64,
	pub w1ReadMask: u64,
	pub w0ReadShr: u8,
	pub w1ReadShl: u8,
}

impl TCInfo
{
	#[inline(always)]
	pub const fn W0CleanMask(&self) -> u64
	{
		!self.w0ReadMask
	}

	#[inline(always)]
	pub const fn W1CleanMask(&self) -> u64
	{
		!self.w1ReadMask
	}

	#[inline(always)]
	pub const fn ElemW0Mask(&self) -> u64
	{
		self.w0ReadMask >> self.w0ReadShr
	}

	#[inline(always)]
	pub const fn ElemW0Shl(&self) -> u8
	{
		self.w0ReadShr
	}

	#[inline(always)]
	pub const fn ElemW1Mask(&self) -> u64
	{
		self.w1ReadMask << self.w1ReadShl
	}

	#[inline(always)]
	pub const fn ElemW1Shr(&self) -> u64
	{
		self.w1ReadShl as u64
	}
}

const fn build_c_masks_by_bit() -> [[TCInfo; 64]; MAX_BITS_NUM as usize]
{
	let mut by_bit_info = [[TCInfo { w0ReadMask: 0, w1ReadMask: 0, w0ReadShr: 0, w1ReadShl: 0 }; 64]; MAX_BITS_NUM as usize];

	let mut bits_in_lit: u8 = 1;
	while bits_in_lit <= MAX_BITS_NUM
	{
		let mut curr_bit: u8 = 0;
		while curr_bit < MAX_BITS_NUM
		{
			let e = &mut by_bit_info[(bits_in_lit - 1) as usize][curr_bit as usize];
			if curr_bit + bits_in_lit > 64
			{
				let lsb_bits: u8 = 64 - curr_bit;
				let msb_bits: u8 = bits_in_lit - lsb_bits;
				e.w0ReadMask = !LsbMask(curr_bit);
				e.w0ReadShr = curr_bit;
				e.w1ReadMask = LsbMask(msb_bits);
				e.w1ReadShl = lsb_bits;
			}
			else
			{
				e.w0ReadMask = LsbMask(bits_in_lit) << curr_bit;
				e.w0ReadShr = curr_bit;
				e.w1ReadMask = 0;
				e.w1ReadShl = 0;
			}
			curr_bit += 1;
		}
		bits_in_lit += 1;
	}
	by_bit_info
}

pub static cMasksByBit: [[TCInfo; 64]; MAX_BITS_NUM as usize] = build_c_masks_by_bit();

// For the tests only: the table builder is otherwise evaluated at compile time only
#[cfg(test)]
pub(crate) fn BuildCMasksByBitAtRunTime() -> [[TCInfo; 64]; MAX_BITS_NUM as usize]
{
	build_c_masks_by_bit()
}

// Only read by the compile-time asserts below, so the copies a large const array implies never materialize
#[allow(clippy::large_const_arrays)]
const cMasksByBitConst: [[TCInfo; 64]; MAX_BITS_NUM as usize] = build_c_masks_by_bit();

// The table is indexed by the number of bits minus 1
const fn BitsInd(bits: usize) -> usize
{
	bits - 1
}

// 1 bits starting from bit 0
const _: () = assert!(cMasksByBitConst[BitsInd(1)][0].w0ReadMask == 1);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][0].w0ReadShr == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][0].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][0].w1ReadShl == 0);

// 1 bits starting from bit 4
const _: () = assert!(cMasksByBitConst[BitsInd(1)][4].w0ReadMask == 16);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][4].w0ReadShr == 4);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][4].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][4].w1ReadShl == 0);

// 1 bits starting from bit 63
const _: () = assert!(cMasksByBitConst[BitsInd(1)][63].w0ReadMask == 1u64 << 63);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][63].w0ReadShr == 63);
const _: () = assert!(cMasksByBitConst[BitsInd(1)][63].w1ReadMask == 0);
