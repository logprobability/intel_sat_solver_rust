// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topor_table_bit_info::cMasksByBit;

/*
* Contagious bit-array: the LSB's go into the lower word, so the order is preserved, but there is a cost of an additional shift
*/

/// # Safety
/// buf must point at a buffer, containing the word of start_bit_num and the next one
#[inline(always)]
pub unsafe fn CGetElem(buf: *const u64, start_bit_num: u64, elem_width: u8) -> u64
{
	let word_ind = (start_bit_num >> 6) as usize;
	let start_bit_in_word = (start_bit_num & 63) as usize;
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
