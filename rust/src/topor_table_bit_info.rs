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
pub(crate) const fn BitsInd(bits: usize) -> usize
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
const _: () = assert!(cMasksByBitConst[BitsInd(1)][63].w1ReadShl == 0);

// 2 bits starting from bit 0
const _: () = assert!(cMasksByBitConst[BitsInd(2)][0].w0ReadMask == 3);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][0].w0ReadShr == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][0].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][0].w1ReadShl == 0);

// 2 bits starting from bit 4
const _: () = assert!(cMasksByBitConst[BitsInd(2)][4].w0ReadMask == 16 + 32);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][4].w0ReadShr == 4);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][4].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][4].w1ReadShl == 0);

// 2 bits starting from bit 63
const _: () = assert!(cMasksByBitConst[BitsInd(2)][63].w0ReadMask == 1u64 << 63);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][63].w0ReadShr == 63);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][63].w1ReadMask == 1);
const _: () = assert!(cMasksByBitConst[BitsInd(2)][63].w1ReadShl == 1);

// 3 bits starting from bit 0
const _: () = assert!(cMasksByBitConst[BitsInd(3)][0].w0ReadMask == 7);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][0].w0ReadShr == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][0].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][0].w1ReadShl == 0);

// 3 bits starting from bit 4
const _: () = assert!(cMasksByBitConst[BitsInd(3)][4].w0ReadMask == 16 + 32 + 64);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][4].w0ReadShr == 4);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][4].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][4].w1ReadShl == 0);

// 3 bits starting from bit 61
const _: () = assert!(cMasksByBitConst[BitsInd(3)][61].w0ReadMask == (1u64 << 63) + (1u64 << 62) + (1u64 << 61));
const _: () = assert!(cMasksByBitConst[BitsInd(3)][61].w0ReadShr == 61);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][61].w1ReadMask == 0);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][61].w1ReadShl == 0);

// 3 bits starting from bit 62
const _: () = assert!(cMasksByBitConst[BitsInd(3)][62].w0ReadMask == (1u64 << 63) + (1u64 << 62));
const _: () = assert!(cMasksByBitConst[BitsInd(3)][62].w0ReadShr == 62);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][62].w1ReadMask == 1);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][62].w1ReadShl == 2);

// 3 bits starting from bit 63
const _: () = assert!(cMasksByBitConst[BitsInd(3)][63].w0ReadMask == (1u64 << 63));
const _: () = assert!(cMasksByBitConst[BitsInd(3)][63].w0ReadShr == 63);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][63].w1ReadMask == 3);
const _: () = assert!(cMasksByBitConst[BitsInd(3)][63].w1ReadShl == 1);

#[derive(Clone, Copy)]
pub struct TBitInfo
{
	pub w0ReadMask: u64,
	pub w1ReadMask: u64,
	pub w0ReadShr: u8,
}

impl TBitInfo
{
	#[inline(always)]
	pub const fn W0WriteMask(&self) -> u64
	{
		!self.w1ReadMask
	}

	#[inline(always)]
	pub const fn W0WriteShl(&self) -> u8
	{
		self.w0ReadShr
	}

	#[inline(always)]
	pub const fn W1WriteMask(&self) -> u64
	{
		self.w1ReadMask
	}

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
}

// The goal of the compile-time-created array tableByBit is to enable writing and reading any number b of bits
// (b<=64 bits; it can be easily extended to 128 bits, if a uint128_t type is available),
// *without branching*, starting from any bit-number in a uint64_t* buffer
// An entry i-1 in tableByBit (whose simple syntax works only starting from C++17) corresponds to i bits
// The idea is to decompose the b-bit number into MSB part, which goes into 64-word-0, and the LSB part (if any), which goes into 64-word-1
// Information per b bits:
//          For every initial bit number (from 1 to b):
//          (a) A mask to extract the MSB's from word-0 (w0ReadMask)
//          (b) Right-shift value to adjust the MSB's from word-0 (w0ReadShr)
//          (c) A mask to extract the LSB's from word-1
// For an example, consider i+1=26
// tableByBit[25][52].w1ReadMask = (1 << (52+26-64))-1 = (1 << 14)-1
// tableByBit[25][52].w0ReadMask = ~((1 << 52) - 1)
// tableByBit[25][52].w0ReadShr = 52-(52+26-64=14)=38;
// See also the static_assert's after the table for more examples

const fn build_table_by_bit() -> [[TBitInfo; 64]; MAX_BITS_NUM as usize]
{
	let mut byBitInfo = [[TBitInfo { w0ReadMask: 0, w1ReadMask: 0, w0ReadShr: 0 }; 64]; MAX_BITS_NUM as usize];

	let mut bitsInLit: u8 = 1;
	while bitsInLit <= MAX_BITS_NUM
	{
		let mut currBit: u8 = 0;
		while currBit < MAX_BITS_NUM
		{
			let e = &mut byBitInfo[(bitsInLit - 1) as usize][currBit as usize];
			if currBit + bitsInLit > 64
			{
				let lsbBits: u8 = currBit + bitsInLit - 64;
				e.w0ReadMask = !LsbMask(currBit);
				e.w0ReadShr = currBit - lsbBits;
				e.w1ReadMask = LsbMask(lsbBits);
			}
			else
			{
				e.w0ReadMask = LsbMask(bitsInLit) << currBit;
				e.w0ReadShr = currBit;
				e.w1ReadMask = 0;
			}
			currBit += 1;
		}
		bitsInLit += 1;
	}
	byBitInfo
}

pub static tableByBit: [[TBitInfo; 64]; MAX_BITS_NUM as usize] = build_table_by_bit();

// For the tests only: the table builder is otherwise evaluated at compile time only
#[cfg(test)]
pub(crate) fn BuildTableByBitAtRunTime() -> [[TBitInfo; 64]; MAX_BITS_NUM as usize]
{
	build_table_by_bit()
}

// Only read by the compile-time asserts below
#[allow(clippy::large_const_arrays)]
const tableByBitConst: [[TBitInfo; 64]; MAX_BITS_NUM as usize] = build_table_by_bit();

// Some static asserts to assert correctness

// 1 bit

const _: () = assert!(tableByBitConst[BitsInd(1)][0].w0ReadMask == 1);
const _: () = assert!(tableByBitConst[BitsInd(1)][0].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(1)][0].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(1)][1].w0ReadMask == 2);
const _: () = assert!(tableByBitConst[BitsInd(1)][1].w0ReadShr == 1);
const _: () = assert!(tableByBitConst[BitsInd(1)][1].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(1)][2].w0ReadMask == 4);
const _: () = assert!(tableByBitConst[BitsInd(1)][2].w0ReadShr == 2);
const _: () = assert!(tableByBitConst[BitsInd(1)][2].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(1)][9].w0ReadMask == 1u64 << 9);
const _: () = assert!(tableByBitConst[BitsInd(1)][9].w0ReadShr == 9);
const _: () = assert!(tableByBitConst[BitsInd(1)][9].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(1)][63].w0ReadMask == 1u64 << 63);
const _: () = assert!(tableByBitConst[BitsInd(1)][63].w0ReadShr == 63);
const _: () = assert!(tableByBitConst[BitsInd(1)][63].w1ReadMask == 0);

// 2 bits

const _: () = assert!(tableByBitConst[BitsInd(2)][0].w0ReadMask == 3);
const _: () = assert!(tableByBitConst[BitsInd(2)][0].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(2)][0].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(2)][1].w0ReadMask == 3 << 1);
const _: () = assert!(tableByBitConst[BitsInd(2)][1].w0ReadShr == 1);
const _: () = assert!(tableByBitConst[BitsInd(2)][1].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(2)][2].w0ReadMask == 3 << 2);
const _: () = assert!(tableByBitConst[BitsInd(2)][2].w0ReadShr == 2);
const _: () = assert!(tableByBitConst[BitsInd(2)][2].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(2)][62].w0ReadMask == 3u64 << 62);
const _: () = assert!(tableByBitConst[BitsInd(2)][62].w0ReadShr == 62);
const _: () = assert!(tableByBitConst[BitsInd(2)][62].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(2)][63].w0ReadMask == 1u64 << 63);
const _: () = assert!(tableByBitConst[BitsInd(2)][63].w0ReadShr == 62);
const _: () = assert!(tableByBitConst[BitsInd(2)][63].w1ReadMask == 1);

// 26 bits

const _: () = assert!(tableByBitConst[BitsInd(26)][52].w0ReadMask == !((1u64 << 52) - 1));
const _: () = assert!(tableByBitConst[BitsInd(26)][52].w0ReadShr == 52 - (52 + 26 - 64));
const _: () = assert!(tableByBitConst[BitsInd(26)][52].w1ReadMask == (1u64 << (52 + 26 - 64)) - 1);

// 64 bits

const _: () = assert!(tableByBitConst[BitsInd(64)][0].w0ReadMask == u64::MAX);
const _: () = assert!(tableByBitConst[BitsInd(64)][0].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(64)][0].w1ReadMask == 0);

const _: () = assert!(tableByBitConst[BitsInd(64)][1].w0ReadMask == u64::MAX - 1);
const _: () = assert!(tableByBitConst[BitsInd(64)][1].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(64)][1].w1ReadMask == 1);

const _: () = assert!(tableByBitConst[BitsInd(64)][2].w0ReadMask == u64::MAX - 3);
const _: () = assert!(tableByBitConst[BitsInd(64)][2].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(64)][2].w1ReadMask == 3);

const _: () = assert!(tableByBitConst[BitsInd(64)][3].w0ReadMask == u64::MAX - 7);
const _: () = assert!(tableByBitConst[BitsInd(64)][3].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(64)][3].w1ReadMask == 7);

const _: () = assert!(tableByBitConst[BitsInd(64)][63].w0ReadMask == 1u64 << 63);
const _: () = assert!(tableByBitConst[BitsInd(64)][63].w0ReadShr == 0);
const _: () = assert!(tableByBitConst[BitsInd(64)][63].w1ReadMask == u64::MAX - (1u64 << 63));

// The goal of the compile-time-created array tableByEntry is to enable writing and reading *elements* of b of bits
// (b<=64 bits; it can be trivially extended to 128 bits, if a uint128_t type is available),
// *without branching*, starting from any element in a uint64_t* buffer, representing a b-bit array
// The difference between tableByEntry and tableByBit is that the latter enables indexing by bit, while the former enables indexing by element number
// Hence, tableByEntry enables implementing b-bit arrays, while tableByBit enables working with a bit-indexed buffer, knowing the starting bit number and the bit width
// An entry i-1 in tableByEntry (whose simple syntax works only starting from C++17) corresponds to i bits
// The idea is similar to tableByBit: we decompose the b-bit number into MSB part, which goes into 64-word-0, and the LSB part (if any), which goes into 64-word-1
// Information per b bits:
//		(1) Given an entry number in an b-bit array, it holds a mask M to get the starting bit-number in its entry in the uint64_t* buffer
//			In an b-bit array, this information saves a costly multiplication operation (entry*b) to get the first bit number
//      (2) For every initial bit number from 1 to b, the following information:
//          (a) A mask to extract the MSB's from word-0 (w0ReadMask)
//          (b) Right-shift value to adjust the MSB's from word-0 (w0ReadShr)
//          (c) A mask to extract the LSB's from word-1
// For an example, consider i+1=26
// We have the following cycle of 32 possible entries, starting from 0:
// 0, 26, 52, 14, 40, 2, 28, 54, 16, 42, 4, 30, 56, 18, 44, 6, 32, 58, 20, 46, 8, 34, 60, 22, 48, 10, 36, 62, 24, 50, 12, 38
// Hence, we have tableByEntry[25].first = 31, while:
// tableByEntry[25].second[0] = tableByBit[25][0]
// tableByEntry[25].second[1] = tableByBit[25][26]
// tableByEntry[25].second[2] = tableByBit[25][52]
// tableByEntry[25].second[3] = tableByBit[25][14]
// ...............................................
// tableByEntry[25].second[31] = tableByBit[25][38]

const fn build_table_by_entry() -> [(u8, [TBitInfo; 64]); MAX_BITS_NUM as usize]
{
	let tableByBitC = build_table_by_bit();
	let mut byEntryInfo = [(0u8, [TBitInfo { w0ReadMask: 0, w1ReadMask: 0, w0ReadShr: 0 }; 64]); MAX_BITS_NUM as usize];
	let mut bitsInLit: u8 = 1;
	while bitsInLit <= MAX_BITS_NUM
	{
		// 0-initialization is the default
		let mut isVisited = [false; MAX_BITS_NUM as usize];
		let mut currRem: u8 = 0;
		let mut currBit: u8 = 0;
		while !isVisited[currBit as usize]
		{
			isVisited[currBit as usize] = true;
			let e = &mut byEntryInfo[(bitsInLit - 1) as usize].1[currRem as usize];
			e.w0ReadMask = tableByBitC[(bitsInLit - 1) as usize][currBit as usize].w0ReadMask;
			e.w0ReadShr = tableByBitC[(bitsInLit - 1) as usize][currBit as usize].w0ReadShr;
			e.w1ReadMask = tableByBitC[(bitsInLit - 1) as usize][currBit as usize].w1ReadMask;
			currBit = ((currBit as u32 + bitsInLit as u32) % 64) as u8;
			currRem += 1;
		}
		// currRem must always be a power of 2 for our table to work
		// This is what indeed happens: we static_assert it below for all possible values, but a mathematician should be able to prove it
		byEntryInfo[(bitsInLit - 1) as usize].0 = currRem - 1;
		bitsInLit += 1;
	}
	byEntryInfo
}

pub static tableByEntry: [(u8, [TBitInfo; 64]); MAX_BITS_NUM as usize] = build_table_by_entry();

// For the tests only: the table builder is otherwise evaluated at compile time only
#[cfg(test)]
pub(crate) fn BuildTableByEntryAtRunTime() -> [(u8, [TBitInfo; 64]); MAX_BITS_NUM as usize]
{
	build_table_by_entry()
}

// Only read by the compile-time asserts below
#[allow(clippy::large_const_arrays)]
const tableByEntryConst: [(u8, [TBitInfo; 64]); MAX_BITS_NUM as usize] = build_table_by_entry();

// Some static asserts to assert correctness

// In order for tableByEntry to work, all the cycles must be power of 2
const _: () = assert!((tableByEntryConst[0].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[1].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[2].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[3].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[4].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[5].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[6].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[7].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[8].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[9].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[10].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[11].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[12].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[13].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[14].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[15].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[16].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[17].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[18].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[19].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[20].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[21].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[22].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[23].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[24].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[25].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[26].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[27].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[28].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[29].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[30].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[31].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[32].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[33].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[34].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[35].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[36].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[37].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[38].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[39].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[40].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[41].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[42].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[43].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[44].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[45].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[46].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[47].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[48].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[49].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[50].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[51].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[52].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[53].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[54].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[55].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[56].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[57].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[58].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[59].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[60].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[61].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[62].0 + 1).is_power_of_two());
const _: () = assert!((tableByEntryConst[63].0 + 1).is_power_of_two());
// Asserting some of the hard-wired cycle values
const _: () = assert!(tableByEntryConst[BitsInd(1)].0 == 63);
const _: () = assert!(tableByEntryConst[BitsInd(2)].0 == 31);
const _: () = assert!(tableByEntryConst[BitsInd(26)].0 == 31);
// Asserting some of the corresponding entries between tableByEntry and tableByBit
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[0].w0ReadMask == tableByBitConst[BitsInd(26)][0].w0ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[0].w0ReadShr == tableByBitConst[BitsInd(26)][0].w0ReadShr);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[0].w1ReadMask == tableByBitConst[BitsInd(26)][0].w1ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[1].w0ReadMask == tableByBitConst[BitsInd(26)][26].w0ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[1].w0ReadShr == tableByBitConst[BitsInd(26)][26].w0ReadShr);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[1].w1ReadMask == tableByBitConst[BitsInd(26)][26].w1ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[2].w0ReadMask == tableByBitConst[BitsInd(26)][52].w0ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[2].w0ReadShr == tableByBitConst[BitsInd(26)][52].w0ReadShr);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[2].w1ReadMask == tableByBitConst[BitsInd(26)][52].w1ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[3].w0ReadMask == tableByBitConst[BitsInd(26)][14].w0ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[3].w0ReadShr == tableByBitConst[BitsInd(26)][14].w0ReadShr);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[3].w1ReadMask == tableByBitConst[BitsInd(26)][14].w1ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[31].w0ReadMask == tableByBitConst[BitsInd(26)][38].w0ReadMask);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[31].w0ReadShr == tableByBitConst[BitsInd(26)][38].w0ReadShr);
const _: () = assert!(tableByEntryConst[BitsInd(26)].1[31].w1ReadMask == tableByBitConst[BitsInd(26)][38].w1ReadMask);
