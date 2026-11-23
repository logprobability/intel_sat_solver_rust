// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topor_dyn_array::{CDynArray, Span};
use std::ops::{Deref, DerefMut};

pub struct CVector<T: Copy>
{
	m_par: CDynArray<T>,
	m_Next: usize,
}

impl<T: Copy> Deref for CVector<T>
{
	type Target = CDynArray<T>;
	#[inline(always)]
	fn deref(&self) -> &CDynArray<T>
	{
		&self.m_par
	}
}

impl<T: Copy> DerefMut for CVector<T>
{
	#[inline(always)]
	fn deref_mut(&mut self) -> &mut CDynArray<T>
	{
		&mut self.m_par
	}
}

impl<T: Copy> Default for CVector<T>
{
	fn default() -> Self
	{
		CVector::new(0, 0)
	}
}

impl<T: Copy> Clone for CVector<T>
{
	fn clone(&self) -> Self
	{
		CVector { m_par: self.m_par.clone(), m_Next: self.m_Next }
	}
}

impl<T: Copy> CVector<T>
{
	pub fn new(init_sz: usize, next: usize) -> Self
	{
		CVector { m_par: CDynArray::new(init_sz), m_Next: next }
	}

	pub fn new_init(init_sz: usize, init_val: u8, next: usize) -> Self
	{
		CVector { m_par: CDynArray::new_init(init_sz, init_val), m_Next: next }
	}

	// Assignment operator
	pub fn assign(&mut self, v: &CVector<T>)
	{
		self.m_par.assign(&v.m_par);
		self.m_Next = v.m_Next;
	}

	#[inline(always)]
	pub fn push_back(&mut self, elem: T)
	{
		if self.m_Next >= self.m_par.cap()
		{
			self.m_par.reserve_atleast(self.m_Next + 1);
			if self.m_par.uninitialized_or_erroneous()
			{
				return;
			}
		}
		self.m_par[self.m_Next] = elem;
		self.m_Next += 1;
	}

	#[inline(always)]
	pub fn emplace_back(&mut self, elem: T)
	{
		self.push_back(elem);
	}

	#[inline(always)]
	pub fn append(&mut self, s: Span<T>)
	{
		debug_assert!(self.m_Next + s.size() >= self.m_Next);
		if self.m_Next + s.size() >= self.m_par.cap()
		{
			self.m_par.reserve_atleast(self.m_Next + s.size());
			if self.m_par.uninitialized_or_erroneous()
			{
				return;
			}
		}
		unsafe { std::ptr::copy_nonoverlapping(s.ptr(), self.m_par.get_ptr_no_assert(self.m_Next), s.size()) };
		self.m_Next += s.size();
	}

	#[inline(always)]
	pub fn pop_back(&mut self) -> T
	{
		self.m_Next -= 1;
		self.m_par[self.m_Next]
	}

	#[inline(always)]
	pub fn back(&self) -> T
	{
		self.m_par[self.m_Next - 1]
	}

	#[inline(always)]
	pub fn back_mut(&mut self) -> &mut T
	{
		let i = self.m_Next - 1;
		&mut self.m_par[i]
	}

	#[inline(always)]
	pub fn clear(&mut self)
	{
		self.m_Next = 0;
	}

	#[inline(always)]
	pub fn clear_apply<F: FnMut(T)>(&mut self, mut apply_before_cleaning: F)
	{
		if !self.empty()
		{
			for elem in self.get_span()
			{
				apply_before_cleaning(elem);
			}
		}
		self.clear();
	}

	#[inline(always)]
	pub fn size(&self) -> usize
	{
		self.m_Next
	}

	#[inline(always)]
	pub fn empty(&self) -> bool
	{
		self.m_Next == 0
	}

	pub fn resize(&mut self, sz: usize)
	{
		if sz > self.m_par.cap()
		{
			self.m_par.reserve_atleast(sz + 1);
			if self.m_par.uninitialized_or_erroneous()
			{
				return;
			}
		}
		self.m_Next = sz;
	}

	#[inline(always)]
	pub fn get_span(&self) -> Span<T>
	{
		// After a failed reallocation the C++ vector is left with a null buffer and a non-zero size, so the C++ span would point at null;
		// the view is empty here instead, so that a memory-out wouldn't turn into a crash before the solver reports it
		if self.m_par.uninitialized_or_erroneous()
		{
			return Span::empty();
		}
		Span::new(self.m_par.get_ptr_no_assert(0), self.m_Next)
	}

	#[inline(always)]
	pub fn get_span_from(&self, start_ind_incl: usize) -> Span<T>
	{
		debug_assert!(start_ind_incl == 0 || start_ind_incl < self.m_Next);
		Span::new(self.m_par.get_ptr_no_assert(start_ind_incl), self.m_Next - start_ind_incl)
	}

	#[inline(always)]
	pub fn get_span_sz(&self, start_ind_incl: usize, sz: usize) -> Span<T>
	{
		debug_assert!(start_ind_incl + sz >= start_ind_incl);
		debug_assert!(start_ind_incl + sz <= self.m_Next);
		Span::new(self.m_par.get_ptr_no_assert(start_ind_incl), sz)
	}

	#[inline(always)]
	pub fn erase_if_may_reorder<F: FnMut(T) -> bool>(&mut self, mut erase_cond: F, start_ind: usize)
	{
		let mut curr_ind = start_ind;
		while curr_ind < self.m_Next
		{
			if erase_cond(self.m_par[curr_ind])
			{
				self.m_Next -= 1;
				self.m_par[curr_ind] = self.m_par[self.m_Next];
				// The C++ version decrements currInd here and increments it in the loop header, so the same index is revisited
			}
			else
			{
				curr_ind += 1;
			}
		}
	}
}
