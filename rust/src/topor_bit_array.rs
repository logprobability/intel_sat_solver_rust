// Copyright(C) 2021 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topor_bit_array_buffer::*;
use crate::topor_dyn_array::CDynArray;
use std::ops::{Deref, DerefMut};

#[derive(Default)]
pub struct CBitArray
{
	m_par: CDynArray<u64>,
	m_NextBit: u64,
}

impl Deref for CBitArray
{
	type Target = CDynArray<u64>;
	#[inline(always)]
	fn deref(&self) -> &CDynArray<u64>
	{
		&self.m_par
	}
}

impl DerefMut for CBitArray
{
	#[inline(always)]
	fn deref_mut(&mut self) -> &mut CDynArray<u64>
	{
		&mut self.m_par
	}
}

impl CBitArray
{
	pub fn new() -> Self
	{
		CBitArray::default()
	}

	#[inline(always)]
	fn b(&self) -> *mut u64
	{
		debug_assert!(!self.m_par.uninitialized_or_erroneous());
		self.m_par.get_ptr_no_assert(0)
	}

	#[inline(always)]
	const fn BitEntry2Uint64Entry(bit_entry: u64) -> usize
	{
		(bit_entry >> 6) as usize
	}

	pub fn bit_reserve_new_chunk(&mut self, new_bits_required: u64, init_val: u8)
	{
		// The C++ version reserves one word less. However, getting/setting an element always accesses also the word after the one the element starts in,
		// so an element in the last word made the C++ version read and (re)write 8 bytes beyond the buffer. The extra word makes that access legal.
		let new_cap = Self::BitEntry2Uint64Entry(self.m_NextBit + new_bits_required) + 2;
		if new_cap > self.cap()
		{
			self.m_par.reserve_atleast_init(new_cap, init_val);
		}
	}

	#[inline(always)]
	pub fn bit_remove_last_bits(&mut self, bits_removed: u64)
	{
		debug_assert!(self.m_NextBit >= bits_removed);
		self.m_NextBit -= bits_removed;
	}

	pub fn bit_resize_and_compress(&mut self, new_bits_required: u64)
	{
		self.m_NextBit = new_bits_required;
		// One word more than in the C++ version: see bit_reserve_new_chunk
		self.m_par.reserve_exactly(Self::BitEntry2Uint64Entry(self.m_NextBit) + 2 * (self.m_NextBit != 0) as usize);
	}

	pub fn compress(&mut self)
	{
		self.bit_resize_and_compress(self.m_NextBit);
	}

	#[inline(always)]
	pub fn bit_push(&mut self, elem: u64, elem_width: u8)
	{
		unsafe { CSetElemAndGoToNext(self.b(), &mut self.m_NextBit, elem_width, elem) };
	}

	#[inline(always)]
	pub fn bit_set(&mut self, elem: u64, elem_width: u8, bit_num: u64)
	{
		unsafe { CSetElem(self.b(), bit_num, elem_width, elem) };
	}

	#[inline(always)]
	pub fn bit_set_and_advance(&mut self, elem: u64, elem_width: u8, bit_num: &mut u64)
	{
		unsafe { CSetElemAndGoToNext(self.b(), bit_num, elem_width, elem) };
	}

	#[inline(always)]
	pub fn bit_get(&self, b: u64, elem_width: u8) -> u64
	{
		unsafe { CGetElem(self.b(), b, elem_width) }
	}

	#[inline(always)]
	pub fn bit_get_and_advance(&self, b: &mut u64, elem_width: u8) -> u64
	{
		unsafe { CGetElemAndGoToNext(self.b(), b, elem_width) }
	}

	#[inline(always)]
	pub fn bit_get_next_bit(&self) -> u64
	{
		self.m_NextBit
	}

	#[inline(always)]
	pub fn bit_set_next_bit(&mut self, next_bit: u64)
	{
		self.m_NextBit = next_bit;
	}

	pub fn copy(&mut self, b_from: u64, b_to: u64, elem_width: u8)
	{
		debug_assert!(self.m_NextBit - (elem_width as u64) < self.m_NextBit);
		debug_assert!(b_from <= self.m_NextBit - elem_width as u64);
		debug_assert!(b_to <= self.m_NextBit - elem_width as u64);
		unsafe
		{
			let v = CGetElem(self.b(), b_from, elem_width);
			CSetElem(self.b(), b_to, elem_width, v);
			debug_assert!(CGetElem(self.b(), b_to, elem_width) == v);
		}
	}

	pub fn copy_block(&mut self, mut b_from: u64, mut b_to: u64, mut overall_width: u64)
	{
		debug_assert!(self.m_NextBit - overall_width < self.m_NextBit);
		debug_assert!(b_from <= self.m_NextBit - overall_width);
		debug_assert!(b_to <= self.m_NextBit - overall_width);

		if overall_width <= 64
		{
			self.copy(b_from, b_to, overall_width as u8);
			return;
		}

		#[inline(always)]
		fn get_div(n: u64) -> u64
		{
			n >> 6
		}
		#[inline(always)]
		fn get_rem(n: u64) -> u64
		{
			n & 63
		}

		if get_rem(b_from) == 0 && get_rem(b_to) == 0
		{
			// std::ptr::copy is memmove, which is what the C++ version boils down to (it picks memcpy, if there is no overlap)
			unsafe { std::ptr::copy(self.b().add(get_div(b_from) as usize), self.b().add(get_div(b_to) as usize), get_div(overall_width) as usize) };
			let overall_width_rem = get_rem(overall_width);
			if overall_width_rem != 0
			{
				self.copy(b_from + overall_width - overall_width_rem, b_to + overall_width - overall_width_rem, overall_width_rem as u8);
			}
		}
		else
		{
			let b_to_rem = get_rem(b_to);
			if b_to_rem != 0
			{
				self.copy(b_from, b_to, b_to_rem as u8);
			}
			b_from += b_to_rem;
			b_to += b_to_rem;
			overall_width -= b_to_rem;
			while overall_width >= 64
			{
				self.copy(b_from, b_to, 64);
				overall_width -= 64;
				b_from += 64;
				b_to += 64;
			}
			if overall_width != 0
			{
				self.copy(b_from, b_to, overall_width as u8);
			}
		}
	}
}
