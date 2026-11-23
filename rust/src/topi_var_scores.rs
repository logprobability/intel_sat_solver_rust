// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topor_dyn_array::CDynArray;
use crate::topor_vector::CVector;

// Variable scores handler, including a heap implementation

type TUVar = u32;
type TUV = u32;

// TPosScore: 1) position in the heap (0 means not in the heap); 2) score
#[derive(Clone, Copy)]
#[repr(C)]
pub struct TPosScore
{
	pub m_Pos: TUV,
	pub m_Score: f64,
}

// In C++, the class holds a reference to the variable activity increment (which lives in the statistics);
// here the functions which need it (increase_score and var_inc_update) receive it as a parameter instead
pub struct CVarScores
{
	// The activity
	m_Activity: CVector<f64>,
	// This variable allows one to simulate the two different insertion orders:
	// false: bigger variable indices first; true: smaller variable indices first (default)
	// We do the simulation by providing two implementations of the "better" predicate (see below)
	// See the parameter m_ParamVsidsInitOrder for more details
	m_InitOrder: bool,
	// Heap of variables
	m_Heap: CVector<TUVar>,
	// Variable-indexed array: position & score per variable
	m_PosScore: CDynArray<TPosScore>,
}

impl Default for CVarScores
{
	fn default() -> Self
	{
		CVarScores::new()
	}
}

impl CVarScores
{
	// The heap's 0 index must always be occupied, so that position 0 would mean not-in-the-heap
	pub fn new() -> Self
	{
		CVarScores { m_Activity: CVector::new(0, 0), m_InitOrder: false, m_Heap: CVector::new_init(1, 0, 1), m_PosScore: CDynArray::new(0) }
	}

	pub fn SetInitOrder(&mut self, init_order: bool)
	{
		self.m_InitOrder = init_order;
	}

	pub fn reserve_exactly(&mut self, beyond_max_var: usize)
	{
		self.m_PosScore.reserve_exactly_init(beyond_max_var, 0);
		self.m_Heap.reserve_exactly(beyond_max_var);
	}

	#[inline(always)]
	pub fn uninitialized_or_erroneous(&self) -> bool
	{
		self.m_PosScore.uninitialized_or_erroneous() || self.m_Heap.uninitialized_or_erroneous()
	}

	#[inline(always)]
	pub fn size(&self) -> usize
	{
		self.m_Heap.size() - 1
	}

	#[inline(always)]
	pub fn empty(&self) -> bool
	{
		self.size() == 0
	}

	#[inline(always)]
	pub fn in_heap(&self, v: TUVar) -> bool
	{
		debug_assert!((v as usize) < self.m_PosScore.cap());
		self.m_PosScore[v as usize].m_Pos > 0
	}

	pub fn increase_score(&mut self, v: TUVar, mult: f64, var_activity_inc: &mut f64) -> bool
	{
		let mut is_rescaled = false;
		self.m_PosScore[v as usize].m_Score += *var_activity_inc * mult;
		if self.m_PosScore[v as usize].m_Score > 1e100
		{
			// Rescale
			for curr_pos_score in self.m_PosScore.get_span_cap(0).as_mut_slice()
			{
				curr_pos_score.m_Score *= 1e-100;
			}

			*var_activity_inc *= 1e-100;
			is_rescaled = true;
		}

		if self.in_heap(v)
		{
			self.percolate_up(self.m_PosScore[v as usize].m_Pos);
		}

		is_rescaled
	}

	pub fn reinsert_if_not_in_heap(&mut self, v: TUVar)
	{
		debug_assert!(!self.uninitialized_or_erroneous());
		debug_assert!((v as usize) < self.m_PosScore.cap());
		if !self.in_heap(v)
		{
			self.m_PosScore[v as usize].m_Pos = self.m_Heap.size() as TUV;
			self.m_Heap.push_back(v);
			self.percolate_up(self.m_PosScore[v as usize].m_Pos);
		}
	}

	pub fn rebuild(&mut self)
	{
		self.m_Heap.reserve_exactly(self.m_PosScore.cap());
		self.m_Heap.clear();
		self.m_Heap.emplace_back(0);
		for v in 1..self.m_PosScore.cap() as TUVar
		{
			if self.in_heap(v)
			{
				self.m_PosScore[v as usize].m_Pos = 0;
				debug_assert!(!self.in_heap(v));
				self.insert(v, self.m_PosScore[v as usize].m_Score);
				debug_assert!(self.in_heap(v));
			}
		}
	}

	pub fn insert(&mut self, v: TUVar, score: f64)
	{
		if v as usize >= self.m_PosScore.cap()
		{
			self.m_PosScore.reserve_atleast_init(v as usize + 1, 0);
			self.m_Heap.reserve_atleast(v as usize + 1);
			if self.uninitialized_or_erroneous()
			{
				return;
			}
		}
		debug_assert!(!self.in_heap(v));

		self.m_PosScore[v as usize] = TPosScore { m_Pos: self.m_Heap.size() as TUV, m_Score: score };
		self.m_Heap.push_back(v);
		self.percolate_up(self.m_PosScore[v as usize].m_Pos);
	}

	pub fn remove_min(&mut self) -> TUVar
	{
		let v = self.m_Heap[1];
		let back_ind = self.m_Heap.size() - 1;
		let back = self.m_Heap[back_ind];
		self.m_Heap[back_ind] = v;
		self.m_Heap[1] = back;
		self.m_PosScore[self.m_Heap[1] as usize].m_Pos = 1;
		self.m_PosScore[v as usize].m_Pos = 0;

		self.m_Heap.pop_back();

		if self.m_Heap.size() > 2
		{
			self.percolate_down(1);
		}

		v
	}

	#[inline(always)]
	pub fn get_min(&self) -> TUVar
	{
		self.m_Heap[1]
	}

	#[inline(always)]
	pub fn var_score_exists(&self, v: TUVar) -> bool
	{
		(v as usize) < self.m_PosScore.cap()
	}

	#[inline(always)]
	pub fn get_var_score(&self, v: TUVar) -> f64
	{
		self.m_PosScore[v as usize].m_Score
	}

	// Use set_var_score only if rebuild is surely scheduled soon, otherwise it will botch the data structure!
	#[inline(always)]
	pub fn set_var_score(&mut self, v: TUVar, new_score: f64)
	{
		self.m_PosScore[v as usize].m_Score = new_score;
	}

	#[inline(always)]
	pub fn var_inc_update(&mut self, var_decay: f64, var_activity_inc: &mut f64)
	{
		*var_activity_inc *= 1. / var_decay;
	}

	#[inline(always)]
	pub fn SetMultiplier(&mut self, multiplier: f64)
	{
		self.m_PosScore.SetMultiplier(multiplier);
		self.m_Heap.SetMultiplier(multiplier);
	}

	#[inline(always)]
	pub fn replace_pos_score_vars(&mut self, v_from: TUVar, v_to: TUVar)
	{
		self.m_PosScore[v_to as usize] = self.m_PosScore[v_from as usize];
	}

	#[inline(always)]
	pub fn memMb(&self) -> usize
	{
		self.m_Activity.memMb() + self.m_Heap.memMb() + self.m_PosScore.memMb()
	}

	// Index "traversal" functions
	#[inline(always)]
	const fn left(i: TUV) -> TUV
	{
		i << 1
	}
	#[inline(always)]
	const fn right(i: TUV) -> TUV
	{
		(i << 1) + 1
	}
	#[inline(always)]
	const fn parent(i: TUV) -> TUV
	{
		i >> 1
	}

	fn percolate_up(&mut self, mut i: TUV)
	{
		let v = self.m_Heap[i as usize];
		let mut p = Self::parent(i);

		while i != 1 && self.better(v, self.m_Heap[p as usize])
		{
			self.m_Heap[i as usize] = self.m_Heap[p as usize];
			self.m_PosScore[self.m_Heap[p as usize] as usize].m_Pos = i;
			i = p;
			p = Self::parent(p);
		}
		self.m_Heap[i as usize] = v;
		self.m_PosScore[v as usize].m_Pos = i;
	}

	fn percolate_down(&mut self, mut i: TUV)
	{
		let v = self.m_Heap[i as usize];
		while (Self::left(i) as usize) < self.m_Heap.size()
		{
			let child = if (Self::right(i) as usize) < self.m_Heap.size() && self.better(self.m_Heap[Self::right(i) as usize], self.m_Heap[Self::left(i) as usize]) { Self::right(i) } else { Self::left(i) };
			if !self.better(self.m_Heap[child as usize], v)
			{
				break;
			}
			self.m_Heap[i as usize] = self.m_Heap[child as usize];
			self.m_PosScore[self.m_Heap[i as usize] as usize].m_Pos = i;
			i = child;
		}
		self.m_Heap[i as usize] = v;
		self.m_PosScore[v as usize].m_Pos = i;
	}

	#[inline(always)]
	fn better(&self, v1: TUVar, v2: TUVar) -> bool
	{
		if self.m_InitOrder { self.m_PosScore[v1 as usize].m_Score > self.m_PosScore[v2 as usize].m_Score } else { self.m_PosScore[v1 as usize].m_Score >= self.m_PosScore[v2 as usize].m_Score }
	}
}

const _: () = assert!(CVarScores::parent(2) == 1 && CVarScores::parent(3) == 1 && CVarScores::left(1) == 2 && CVarScores::right(1) == 3);
const _: () = assert!(CVarScores::parent(4) == 2 && CVarScores::parent(5) == 2 && CVarScores::left(2) == 4 && CVarScores::right(2) == 5);
const _: () = assert!(CVarScores::parent(6) == 3 && CVarScores::parent(7) == 3 && CVarScores::left(3) == 6 && CVarScores::right(3) == 7);
