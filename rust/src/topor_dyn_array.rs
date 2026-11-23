// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use std::alloc::{alloc, alloc_zeroed, dealloc, realloc, Layout};
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

// Test-only allocation fault injection: makes the n-th allocation request (counting from 0) and all the subsequent ones fail
// This is how the tests reach the STATUS_ALLOC_FAILED paths of the solver
#[cfg(any(test, fuzzing))]
pub mod alloc_fault
{
	use std::cell::Cell;
	thread_local!
	{
		static ALLOCS_TILL_FAILURE: Cell<u64> = const { Cell::new(u64::MAX) };
		static ALLOCS_SO_FAR: Cell<u64> = const { Cell::new(0) };
		static SUSPENDED: Cell<u32> = const { Cell::new(0) };
		static FAIL_ONLY_ONE: Cell<bool> = const { Cell::new(false) };
	}

	pub fn fail_after(allocs: u64)
	{
		FAIL_ONLY_ONE.with(|c| c.set(false));
		ALLOCS_TILL_FAILURE.with(|c| c.set(allocs));
	}

	// Makes only the n-th allocation request (counting from 0) fail; the subsequent ones succeed
	pub fn fail_only(allocs: u64)
	{
		FAIL_ONLY_ONE.with(|c| c.set(true));
		ALLOCS_TILL_FAILURE.with(|c| c.set(allocs));
	}

	pub fn never_fail()
	{
		ALLOCS_TILL_FAILURE.with(|c| c.set(u64::MAX));
	}

	pub fn allocs_so_far() -> u64
	{
		ALLOCS_SO_FAR.with(|c| c.get())
	}

	// Suspends the fault injection while f runs: for the allocations, whose failure the solver (precisely as the C++ version) isn't designed to survive
	pub fn suspended<R>(f: impl FnOnce() -> R) -> R
	{
		SUSPENDED.with(|c| c.set(c.get() + 1));
		let r = f();
		SUSPENDED.with(|c| c.set(c.get() - 1));
		r
	}

	pub fn should_fail() -> bool
	{
		if SUSPENDED.with(|c| c.get()) != 0
		{
			return false;
		}
		ALLOCS_SO_FAR.with(|c| c.set(c.get() + 1));
		ALLOCS_TILL_FAILURE.with(|c|
		{
			let v = c.get();
			if v == u64::MAX
			{
				false
			}
			else if v == 0
			{
				if FAIL_ONLY_ONE.with(|o| o.get())
				{
					c.set(u64::MAX);
				}
				true
			}
			else
			{
				c.set(v - 1);
				false
			}
		})
	}
}

// The equivalent of a C++ std::span: a non-owning (pointer, length) view
// Exactly as in C++, it is the responsibility of the user to make sure the underlying buffer outlives the view and isn't reallocated while the view is in use
pub struct Span<T>
{
	m_ptr: *mut T,
	m_len: usize,
}

impl<T> Clone for Span<T>
{
	#[inline(always)]
	fn clone(&self) -> Self
	{
		*self
	}
}
impl<T> Copy for Span<T> {}

impl<T: Copy> Default for Span<T>
{
	fn default() -> Self
	{
		Span::empty()
	}
}

impl<T: Copy> Span<T>
{
	#[inline(always)]
	pub fn new(ptr: *mut T, len: usize) -> Self
	{
		Span { m_ptr: ptr, m_len: len }
	}

	#[inline(always)]
	pub fn empty() -> Self
	{
		Span { m_ptr: std::ptr::NonNull::dangling().as_ptr(), m_len: 0 }
	}

	#[inline(always)]
	pub fn from_slice(s: &mut [T]) -> Self
	{
		Span { m_ptr: s.as_mut_ptr(), m_len: s.len() }
	}

	#[inline(always)]
	pub fn from_const_slice(s: &[T]) -> Self
	{
		Span { m_ptr: s.as_ptr() as *mut T, m_len: s.len() }
	}

	#[inline(always)]
	pub fn size(&self) -> usize
	{
		self.m_len
	}

	#[inline(always)]
	pub fn is_empty(&self) -> bool
	{
		self.m_len == 0
	}

	#[inline(always)]
	pub fn ptr(&self) -> *mut T
	{
		self.m_ptr
	}

	#[inline(always)]
	pub fn get(&self, i: usize) -> T
	{
		debug_assert!(i < self.m_len && !self.m_ptr.is_null());
		unsafe { *self.m_ptr.add(i) }
	}

	#[inline(always)]
	pub fn set(&self, i: usize, v: T)
	{
		debug_assert!(i < self.m_len && !self.m_ptr.is_null());
		unsafe { *self.m_ptr.add(i) = v }
	}

	#[inline(always)]
	pub fn swap(&self, i: usize, j: usize)
	{
		debug_assert!(i < self.m_len && j < self.m_len);
		unsafe { std::ptr::swap(self.m_ptr.add(i), self.m_ptr.add(j)) }
	}

	#[inline(always)]
	pub fn back(&self) -> T
	{
		self.get(self.m_len - 1)
	}

	#[inline(always)]
	pub fn set_back(&self, v: T)
	{
		self.set(self.m_len - 1, v)
	}

	#[inline(always)]
	pub fn subspan(&self, start: usize, len: usize) -> Span<T>
	{
		debug_assert!(start + len <= self.m_len);
		Span { m_ptr: unsafe { self.m_ptr.add(start) }, m_len: len }
	}

	// The sub-span starting at start till the end
	#[inline(always)]
	pub fn subspan_from(&self, start: usize) -> Span<T>
	{
		debug_assert!(start <= self.m_len);
		Span { m_ptr: unsafe { self.m_ptr.add(start) }, m_len: self.m_len - start }
	}

	#[inline(always)]
	pub fn as_slice<'a>(&self) -> &'a [T]
	{
		// The view of an empty (e.g., never allocated) buffer may hold a null pointer, which a slice must not
		if self.m_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(self.m_ptr, self.m_len) } }
	}

	#[inline(always)]
	#[allow(clippy::mut_from_ref)]
	pub fn as_mut_slice<'a>(&self) -> &'a mut [T]
	{
		if self.m_len == 0 { &mut [] } else { unsafe { std::slice::from_raw_parts_mut(self.m_ptr, self.m_len) } }
	}

	#[inline(always)]
	pub fn iter(&self) -> SpanIter<T>
	{
		SpanIter { m_Span: *self, m_I: 0, m_marker: PhantomData }
	}

	#[inline(always)]
	pub fn to_vec(self) -> Vec<T>
	{
		self.as_slice().to_vec()
	}
}

impl<T: Copy + PartialEq> Span<T>
{
	// std::find: returns the index of the first appearance or size(), if none
	#[inline(always)]
	pub fn find(&self, v: T) -> usize
	{
		let mut i = 0;
		while i < self.m_len && self.get(i) != v
		{
			i += 1;
		}
		i
	}

	#[inline(always)]
	pub fn contains(&self, v: T) -> bool
	{
		self.find(v) != self.m_len
	}
}

impl<T: Copy> Index<usize> for Span<T>
{
	type Output = T;
	#[inline(always)]
	fn index(&self, i: usize) -> &T
	{
		debug_assert!(i < self.m_len);
		unsafe { &*self.m_ptr.add(i) }
	}
}

pub struct SpanIter<T>
{
	m_Span: Span<T>,
	m_I: usize,
	m_marker: PhantomData<T>,
}

impl<T: Copy> Iterator for SpanIter<T>
{
	type Item = T;
	#[inline(always)]
	fn next(&mut self) -> Option<T>
	{
		if self.m_I < self.m_Span.m_len
		{
			let v = self.m_Span.get(self.m_I);
			self.m_I += 1;
			Some(v)
		}
		else
		{
			None
		}
	}
}

impl<T: Copy> IntoIterator for Span<T>
{
	type Item = T;
	type IntoIter = SpanIter<T>;
	#[inline(always)]
	fn into_iter(self) -> SpanIter<T>
	{
		self.iter()
	}
}

// A dynamic array with malloc/realloc semantics: no construction/destruction of the elements; an allocation failure doesn't abort, but
// makes the array "uninitialized_or_erroneous" instead (the solver then moves to an unrecoverable STATUS_ALLOC_FAILED status)
// T must be a plain-old-data type for which the all-zero bit pattern is valid
pub struct CDynArray<T: Copy>
{
	m_Cap: usize,
	m_Multiplier: f64,
	m_B: *mut T,
}

unsafe impl<T: Copy + Send> Send for CDynArray<T> {}

pub const MultiplierDef: f64 = 1.625;

impl<T: Copy> CDynArray<T>
{
	pub const MultiplierDef: f64 = MultiplierDef;
	const TSize: usize = std::mem::size_of::<T>();
	const MaxCapacity: usize = usize::MAX / Self::TSize;

	#[inline(always)]
	fn layout(cap: usize) -> Option<Layout>
	{
		#[cfg(any(test, fuzzing))]
		if alloc_fault::should_fail()
		{
			return None;
		}
		Layout::array::<T>(cap).ok()
	}

	fn malloc(cap: usize) -> *mut T
	{
		match Self::layout(cap)
		{
			Some(l) => unsafe { alloc(l) as *mut T },
			None => std::ptr::null_mut(),
		}
	}

	fn calloc(cap: usize) -> *mut T
	{
		match Self::layout(cap)
		{
			Some(l) => unsafe { alloc_zeroed(l) as *mut T },
			None => std::ptr::null_mut(),
		}
	}

	// realloc(m_B, newCap * TSize), where a nullptr m_B means malloc; returns nullptr on failure (the old block is then left untouched)
	fn realloc(&mut self, old_cap: usize, new_cap: usize) -> *mut T
	{
		if self.m_B.is_null()
		{
			return Self::malloc(new_cap);
		}
		match Self::layout(new_cap)
		{
			Some(l) => unsafe { realloc(self.m_B as *mut u8, Layout::array::<T>(old_cap).unwrap_unchecked(), l.size()) as *mut T },
			None => std::ptr::null_mut(),
		}
	}

	fn free(&mut self, cap: usize)
	{
		if !self.m_B.is_null()
		{
			unsafe { dealloc(self.m_B as *mut u8, Layout::array::<T>(cap).unwrap_unchecked()) };
		}
	}

	// m_AllocCap is the capacity the buffer was actually allocated with; it always equals m_Cap, but for a transient state inside the reserve functions
	pub fn new(init_cap: usize) -> Self
	{
		let b = if init_cap > Self::MaxCapacity || init_cap == 0 { std::ptr::null_mut() } else { Self::malloc(init_cap) };
		// Unlike in C++, where free copes with any pointer, we must remember precisely the allocated capacity; hence a failed allocation zeroes the capacity
		CDynArray { m_Cap: if b.is_null() { 0 } else { init_cap }, m_Multiplier: MultiplierDef, m_B: b }
	}

	pub fn new_init(init_cap: usize, init_val: u8) -> Self
	{
		let b = Self::InitAlloc(init_cap, init_val);
		CDynArray { m_Cap: if b.is_null() { 0 } else { init_cap }, m_Multiplier: MultiplierDef, m_B: b }
	}

	// Assignment operator
	pub fn assign(&mut self, da: &CDynArray<T>)
	{
		self.reserve_exactly(da.m_Cap);
		if self.uninitialized_or_erroneous()
		{
			self.m_Cap = 0;
		}
		else
		{
			unsafe { std::ptr::copy_nonoverlapping(da.m_B, self.m_B, da.m_Cap) };
			self.m_Cap = da.m_Cap;
			self.m_Multiplier = da.m_Multiplier;
		}
	}

	#[inline(never)]
	pub fn reserve_exactly(&mut self, new_cap: usize)
	{
		if new_cap > Self::MaxCapacity || new_cap == 0
		{
			self.ClearB();
		}
		else
		{
			let tmp = self.realloc(self.m_Cap, new_cap);
			if tmp.is_null()
			{
				self.ClearB();
			}
			else
			{
				self.m_Cap = new_cap;
				self.m_B = tmp;
			}
		}
	}

	// reserve_exactly for the buffers, whose allocation failure the solver isn't designed to survive: the test-only fault injection skips them
	#[inline(always)]
	pub fn reserve_exactly_unchecked_by_user(&mut self, new_cap: usize)
	{
		#[cfg(any(test, fuzzing))]
		return alloc_fault::suspended(|| self.reserve_exactly(new_cap));
		#[cfg(not(any(test, fuzzing)))]
		self.reserve_exactly(new_cap)
	}

	pub fn reserve_exactly_init(&mut self, new_cap: usize, init_val: u8)
	{
		if new_cap > Self::MaxCapacity || new_cap == 0
		{
			self.ClearB();
		}
		else if self.m_B.is_null()
		{
			debug_assert!(self.m_Cap == 0);
			self.m_B = Self::InitAlloc(new_cap, init_val);
			self.m_Cap = if self.m_B.is_null() { 0 } else { new_cap };
		}
		else
		{
			let tmp = self.realloc(self.m_Cap, new_cap);
			if tmp.is_null()
			{
				self.ClearB();
			}
			else
			{
				self.m_B = tmp;
				if new_cap > self.m_Cap
				{
					unsafe { std::ptr::write_bytes(self.m_B.add(self.m_Cap), init_val, new_cap - self.m_Cap) };
				}
				self.m_Cap = new_cap;
			}
		}
	}

	pub fn reserve_atleast(&mut self, new_cap: usize)
	{
		self.reserve_exactly(self.GetNewCap(new_cap));
	}

	pub fn reserve_atleast_with_max(&mut self, new_cap: usize, MaxCap: usize)
	{
		let tentative_new_cap = self.GetNewCap(new_cap);
		self.reserve_exactly(if tentative_new_cap > MaxCap { MaxCap } else { tentative_new_cap });
	}

	pub fn reserve_atleast_with_max_init(&mut self, new_cap: usize, MaxCap: usize, init_val: u8)
	{
		let tentative_new_cap = self.GetNewCap(new_cap);
		self.reserve_exactly_init(if tentative_new_cap > MaxCap { MaxCap } else { tentative_new_cap }, init_val);
	}

	pub fn reserve_atleast_init(&mut self, new_cap: usize, init_val: u8)
	{
		self.reserve_exactly_init(self.GetNewCap(new_cap), init_val);
	}

	pub fn reserve_beyond_if_requried(&mut self, ind_to_include: usize, is_resize_at_least: bool)
	{
		if ind_to_include >= self.cap()
		{
			if is_resize_at_least
			{
				self.reserve_atleast(ind_to_include + 1);
			}
			else
			{
				self.reserve_exactly(ind_to_include + 1);
			}
		}
	}

	pub fn memset(&mut self, new_val: u8)
	{
		self.memset_range(new_val, 0, self.m_Cap);
	}

	pub fn memset_from(&mut self, new_val: u8, start_ind_incl: usize)
	{
		self.memset_range(new_val, start_ind_incl, self.m_Cap);
	}

	pub fn memset_range(&mut self, new_val: u8, start_ind_incl: usize, end_ind_excl: usize)
	{
		debug_assert!(start_ind_incl < end_ind_excl && end_ind_excl <= self.cap());
		unsafe { std::ptr::write_bytes(self.m_B.add(start_ind_incl), new_val, end_ind_excl - start_ind_incl) };
	}

	// Non-overlapping memcpy
	pub fn memcpy(&mut self, out_start_ind: usize, inp_start_ind: usize, entries_to_copy: usize)
	{
		debug_assert!(inp_start_ind < self.m_Cap && out_start_ind < self.m_Cap);
		debug_assert!(inp_start_ind + entries_to_copy > inp_start_ind && inp_start_ind + entries_to_copy <= self.m_Cap);
		debug_assert!(out_start_ind + entries_to_copy > out_start_ind && out_start_ind + entries_to_copy <= self.m_Cap);
		// No overlap
		debug_assert!(out_start_ind >= inp_start_ind + entries_to_copy || inp_start_ind >= out_start_ind + entries_to_copy);
		unsafe { std::ptr::copy_nonoverlapping(self.m_B.add(inp_start_ind), self.m_B.add(out_start_ind), entries_to_copy) };
	}

	// Potentially-overlapping memmove
	pub fn memmove(&mut self, out_start_ind: usize, inp_start_ind: usize, entries_to_copy: usize)
	{
		if out_start_ind >= inp_start_ind + entries_to_copy || inp_start_ind >= out_start_ind + entries_to_copy
		{
			self.memcpy(out_start_ind, inp_start_ind, entries_to_copy);
		}
		else
		{
			debug_assert!(inp_start_ind < self.m_Cap && out_start_ind < self.m_Cap);
			debug_assert!(inp_start_ind + entries_to_copy > inp_start_ind && inp_start_ind + entries_to_copy <= self.m_Cap);
			debug_assert!(out_start_ind + entries_to_copy > out_start_ind && out_start_ind + entries_to_copy <= self.m_Cap);
			unsafe { std::ptr::copy(self.m_B.add(inp_start_ind), self.m_B.add(out_start_ind), entries_to_copy) };
		}
	}

	#[inline(always)]
	pub fn get_span_cap(&self, start_ind_incl: usize) -> Span<T>
	{
		debug_assert!(start_ind_incl < self.m_Cap);
		Span::new(unsafe { self.m_B.add(start_ind_incl) }, self.m_Cap - start_ind_incl)
	}

	#[inline(always)]
	pub fn get_span_cap_sz(&self, start_ind_incl: usize, sz: usize) -> Span<T>
	{
		debug_assert!(start_ind_incl + sz >= start_ind_incl);
		debug_assert!(start_ind_incl + sz <= self.m_Cap);
		Span::new(unsafe { self.m_B.add(start_ind_incl) }, sz)
	}

	#[inline(always)]
	pub fn get_ptr(&self, i: usize) -> *mut T
	{
		debug_assert!(i < self.m_Cap);
		unsafe { self.m_B.add(i) }
	}

	#[inline(always)]
	pub fn get_ptr_no_assert(&self, i: usize) -> *mut T
	{
		self.m_B.wrapping_add(i)
	}

	#[inline(always)]
	pub fn cap(&self) -> usize
	{
		self.m_Cap
	}

	#[inline(always)]
	pub fn memMb(&self) -> usize
	{
		(self.m_Cap * Self::TSize) / 1000
	}

	#[inline(always)]
	pub fn empty(&self) -> bool
	{
		self.cap() == 0
	}

	#[inline(always)]
	pub fn uninitialized_or_erroneous(&self) -> bool
	{
		self.m_B.is_null()
	}

	#[inline(always)]
	pub fn SetMultiplier(&mut self, multiplier: f64)
	{
		debug_assert!(multiplier >= 1.);
		self.m_Multiplier = multiplier;
	}

	#[inline(always)]
	pub fn GetMultiplier(&self) -> f64
	{
		self.m_Multiplier
	}

	fn InitAlloc(init_cap: usize, init_val: u8) -> *mut T
	{
		// We're using calloc directly for 0-initialized memory (see the discussion in the C++ version)
		if init_cap == 0 || init_cap > Self::MaxCapacity
		{
			return std::ptr::null_mut();
		}

		if init_val == 0
		{
			return Self::calloc(init_cap);
		}

		let malloc_res = Self::malloc(init_cap);
		if malloc_res.is_null()
		{
			return std::ptr::null_mut();
		}

		unsafe { std::ptr::write_bytes(malloc_res, init_val, init_cap) };

		malloc_res
	}

	#[inline(always)]
	fn GetNewCap(&self, cap: usize) -> usize
	{
		let new_cap_double = cap as f64 * self.m_Multiplier + 2.;
		if new_cap_double > usize::MAX as f64 { usize::MAX } else { new_cap_double as usize }
	}

	#[inline(always)]
	fn ClearB(&mut self)
	{
		self.free(self.m_Cap);
		self.m_B = std::ptr::null_mut();
		self.m_Cap = 0;
	}
}

impl<T: Copy + PartialEq> CDynArray<T>
{
	pub fn remove_if_equal_and_cut_capacity(&mut self, eq_val: T)
	{
		let mut curr_ind_write = 0;

		for curr_ind_read in 0..self.m_Cap
		{
			if self[curr_ind_read] != eq_val
			{
				self[curr_ind_write] = self[curr_ind_read];
				curr_ind_write += 1;
			}
		}
		self.reserve_exactly(curr_ind_write);
	}
}

impl<T: Copy> Clone for CDynArray<T>
{
	// Copy constructor
	fn clone(&self) -> Self
	{
		let mut res = CDynArray::new(self.m_Cap);
		if res.uninitialized_or_erroneous()
		{
			res.m_Cap = 0;
		}
		else
		{
			unsafe { std::ptr::copy_nonoverlapping(self.m_B, res.m_B, self.m_Cap) };
		}
		res
	}
}

impl<T: Copy> Default for CDynArray<T>
{
	fn default() -> Self
	{
		CDynArray::new(0)
	}
}

impl<T: Copy> Drop for CDynArray<T>
{
	fn drop(&mut self)
	{
		self.free(self.m_Cap);
	}
}

impl<T: Copy> Index<usize> for CDynArray<T>
{
	type Output = T;
	#[inline(always)]
	fn index(&self, i: usize) -> &T
	{
		debug_assert!(i < self.cap());
		unsafe { &*self.m_B.add(i) }
	}
}

impl<T: Copy> IndexMut<usize> for CDynArray<T>
{
	#[inline(always)]
	fn index_mut(&mut self, i: usize) -> &mut T
	{
		debug_assert!(i < self.cap());
		unsafe { &mut *self.m_B.add(i) }
	}
}

// CDynArray<T>::RemoveGarbage
// The C++ version is a member, which is provided lambdas capturing the solver; here the solver (or any other context) is passed explicitly as ctx,
// where arr projects the array out of the context

pub fn RemoveGarbage<C, T: Copy>(ctx: &mut C, arr: fn(&mut C) -> &mut CDynArray<T>, start_ind: usize, end_ind: &mut usize, is_chunk_deleted: &mut dyn FnMut(&mut C, usize) -> bool,
	chunk_end: &mut dyn FnMut(&mut C, usize) -> usize, mut notify_about_remaining_chunk_move: Option<&mut dyn FnMut(&mut C, usize, usize)>)
{
	fn find_next_deleted<C>(ctx: &mut C, mut first_ind: usize, last_ind: usize, is_chunk_deleted: &mut dyn FnMut(&mut C, usize) -> bool, chunk_end: &mut dyn FnMut(&mut C, usize) -> usize, to_ind: usize,
		notify_about_remaining_chunk_move: &mut Option<&mut dyn FnMut(&mut C, usize, usize)>) -> usize
	{
		let init_first_ind = first_ind;
		while first_ind < last_ind && !is_chunk_deleted(ctx, first_ind)
		{
			let next_first_ind = chunk_end(ctx, first_ind);
			if let Some(notify) = notify_about_remaining_chunk_move
			{
				notify(ctx, first_ind, to_ind + first_ind - init_first_ind);
			}
			first_ind = next_first_ind;
		}
		first_ind
	}

	fn find_next_not_deleted<C>(ctx: &mut C, mut first_ind: usize, last_ind: usize, is_chunk_deleted: &mut dyn FnMut(&mut C, usize) -> bool, chunk_end: &mut dyn FnMut(&mut C, usize) -> usize) -> usize
	{
		while first_ind < last_ind && is_chunk_deleted(ctx, first_ind)
		{
			first_ind = chunk_end(ctx, first_ind);
		}
		first_ind
	}

	// Find the first deleted chunk and put its index in toInd
	let mut to_ind = find_next_deleted(ctx, start_ind, *end_ind, is_chunk_deleted, chunk_end, start_ind, &mut notify_about_remaining_chunk_move);
	// Find the first clause after the first deleted chunk and put its index in fromInd
	let mut from_ind = find_next_not_deleted(ctx, to_ind, *end_ind, is_chunk_deleted, chunk_end);

	// Copy chunks from fromInd to toInd
	while from_ind < *end_ind
	{
		let from_ind_end = find_next_deleted(ctx, from_ind, *end_ind, is_chunk_deleted, chunk_end, to_ind, &mut notify_about_remaining_chunk_move);
		let copied_inds = from_ind_end - from_ind;
		// memmove will still use memcpy, if there is no overlap
		arr(ctx).memmove(to_ind, from_ind, copied_inds);
		to_ind += copied_inds;
		from_ind = find_next_not_deleted(ctx, from_ind_end, *end_ind, is_chunk_deleted, chunk_end);
	}
	*end_ind = to_ind;
}
