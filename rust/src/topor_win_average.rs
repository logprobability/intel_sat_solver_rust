// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topor_dyn_array::CDynArray;

// Stores a window of the last N values, including a possibility to get an average
// Used for Glucose-inspired restart strategies
#[derive(Default)]
pub struct TWinAverage
{
	m_DynArray: CDynArray<u32>,
	m_Sum: f64,
	m_NextInd: u16,
}

impl TWinAverage
{
	pub fn new() -> Self
	{
		TWinAverage::default()
	}

	#[inline(always)]
	pub fn Init(&mut self, maxCap: u16)
	{
		self.m_DynArray.reserve_exactly_init(maxCap as usize, 0);
	}

	pub fn Enqueue(&mut self, new_val: u32)
	{
		debug_assert!(new_val != 0);

		if self.IsError()
		{
			return;
		}

		self.m_Sum -= self.m_DynArray[self.m_NextInd as usize] as f64;
		self.m_DynArray[self.m_NextInd as usize] = new_val;
		self.m_NextInd = (self.m_NextInd + 1) % self.MaxCap();

		self.m_Sum += new_val as f64;
	}

	#[inline(always)]
	pub fn IsFullWindow(&self) -> bool
	{
		self.m_DynArray[self.m_NextInd as usize] != 0
	}

	#[inline(always)]
	pub fn GetAverage(&self) -> f64
	{
		self.m_Sum / (if self.IsFullWindow() { self.MaxCap() } else { self.m_NextInd }) as f64
	}

	#[inline(always)]
	pub fn Clear(&mut self, invalidate_forever: bool)
	{
		if invalidate_forever
		{
			self.m_DynArray.reserve_exactly(0);
		}
		else
		{
			self.m_DynArray.memset(0);
		}
		self.m_Sum = 0.;
		self.m_NextInd = 0;
	}

	#[inline(always)]
	pub fn IsError(&self) -> bool
	{
		self.m_DynArray.uninitialized_or_erroneous()
	}

	#[inline(always)]
	pub fn MaxCap(&self) -> u16
	{
		self.m_DynArray.cap() as u16
	}
}
