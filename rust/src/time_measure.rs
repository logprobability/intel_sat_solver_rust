// Copyright(C) 2021 Intel Corporation
// SPDX - License - Identifier: MIT

use std::time::Instant;

#[cfg(unix)]
extern "C"
{
	fn clock() -> std::ffi::c_ulong;
}

// std::clock(): the processor time consumed by the program in CLOCKS_PER_SEC units
#[cfg(unix)]
#[inline(always)]
fn std_clock() -> i64
{
	unsafe { clock() as i64 }
}

#[cfg(not(unix))]
#[inline(always)]
fn std_clock() -> i64
{
	static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
	START.get_or_init(Instant::now).elapsed().as_micros() as i64
}

// XSI requires CLOCKS_PER_SEC to be 1000000 independent of the actual resolution
const CLOCKS_PER_SEC: i64 = 1000000;

pub struct CTimeMeasure
{
	c_start: i64,
	t_start: Instant,
	m_Timeout: f64,
	m_GenericModeCpuTime: bool,
	m_TimeoutTestModuloFactor: u32,
	m_TimeoutTestCounter: u32,
	m_LatestResForTestCounter: f64,
}

impl CTimeMeasure
{
	pub const TIME_INFINITY: f64 = f64::MAX;

	pub fn new(generic_mode_cpu_time: bool, timeout_test_modulo_factor: u32) -> Self
	{
		CTimeMeasure
		{
			c_start: std_clock(),
			t_start: Instant::now(),
			m_Timeout: Self::TIME_INFINITY,
			m_GenericModeCpuTime: generic_mode_cpu_time,
			m_TimeoutTestModuloFactor: timeout_test_modulo_factor,
			m_TimeoutTestCounter: 0,
			m_LatestResForTestCounter: 0.,
		}
	}

	// Generic functions, which don't depend on the value of m_GenericModeCpuTime

	#[inline(always)]
	pub fn GetTimeout(&self) -> f64
	{
		self.m_Timeout
	}

	#[inline(always)]
	pub fn SetModeWallTime(&mut self)
	{
		self.m_GenericModeCpuTime = false;
	}

	#[inline(always)]
	pub fn SetModeCpuTime(&mut self)
	{
		self.m_GenericModeCpuTime = true;
	}

	#[inline(always)]
	pub fn IsTimeoutSet(&self) -> bool
	{
		self.m_Timeout != Self::TIME_INFINITY
	}

	#[inline(always)]
	pub fn SetTimeout(&mut self, timeout: f64)
	{
		if timeout > 0.
		{
			self.m_Timeout = timeout;
		}
	}

	#[inline(always)]
	pub fn Stop(&mut self)
	{
		self.m_Timeout = 0.00000001;
	}

	#[inline(always)]
	pub fn Reset(&mut self)
	{
		self.c_start = std_clock();
		self.t_start = Instant::now();
	}

	// Generic functions, which depend on the value of m_GenericModeCpuTime

	#[inline(always)]
	pub fn TimePassedSinceStartOrReset(&mut self) -> f64
	{
		if self.m_GenericModeCpuTime { self.CpuTimePassedSinceStartOrReset() } else { self.WallTimePassedSinceStartOrReset() }
	}

	#[inline(always)]
	pub fn IsTimeout(&mut self) -> bool
	{
		if self.m_GenericModeCpuTime { self.CpuIsTimeout() } else { self.WallIsTimeout() }
	}

	#[inline(always)]
	pub fn TimeLeftTillTimeout(&mut self) -> f64
	{
		if self.m_GenericModeCpuTime { self.CpuTimeLeftTillTimeout() } else { self.WallTimeLeftTillTimeout() }
	}

	// Functionality per CPU time and per Wall time

	// Note that, precisely as in the C++ version, this is an integer division, so the CPU time is provided in whole seconds
	#[inline(always)]
	pub fn CpuTimePassedSinceStartOrResetConst(&self) -> f64
	{
		((std_clock() - self.c_start) / CLOCKS_PER_SEC) as f64
	}

	pub fn CpuTimePassedSinceStartOrReset(&mut self) -> f64
	{
		if self.m_TimeoutTestModuloFactor == 1
		{
			self.CpuTimePassedSinceStartOrResetConst()
		}
		else if self.m_TimeoutTestCounter % self.m_TimeoutTestModuloFactor == 0
		{
			self.m_TimeoutTestCounter += 1;
			self.m_LatestResForTestCounter = self.CpuTimePassedSinceStartOrResetConst();
			self.m_LatestResForTestCounter
		}
		else
		{
			self.m_TimeoutTestCounter += 1;
			self.m_LatestResForTestCounter
		}
	}

	#[inline(always)]
	pub fn WallTimePassedSinceStartOrResetConst(&self) -> f64
	{
		self.t_start.elapsed().as_secs_f64()
	}

	pub fn WallTimePassedSinceStartOrReset(&mut self) -> f64
	{
		if self.m_TimeoutTestModuloFactor == 1
		{
			self.WallTimePassedSinceStartOrResetConst()
		}
		else if self.m_TimeoutTestCounter % self.m_TimeoutTestModuloFactor == 0
		{
			self.m_TimeoutTestCounter += 1;
			self.m_LatestResForTestCounter = self.WallTimePassedSinceStartOrResetConst();
			self.m_LatestResForTestCounter
		}
		else
		{
			self.m_TimeoutTestCounter += 1;
			self.m_LatestResForTestCounter
		}
	}

	#[inline(always)]
	pub fn CpuIsTimeout(&mut self) -> bool
	{
		self.m_Timeout - self.CpuTimePassedSinceStartOrReset() <= 0.
	}

	#[inline(always)]
	pub fn WallIsTimeout(&mut self) -> bool
	{
		self.m_Timeout < Self::TIME_INFINITY && self.m_Timeout - self.WallTimePassedSinceStartOrReset() <= 0.
	}

	#[inline(always)]
	pub fn WallTimeLeftTillTimeout(&mut self) -> f64
	{
		if self.m_Timeout == Self::TIME_INFINITY { Self::TIME_INFINITY } else { self.m_Timeout - self.WallTimePassedSinceStartOrReset() }
	}

	#[inline(always)]
	pub fn CpuTimeLeftTillTimeout(&mut self) -> f64
	{
		if self.m_Timeout == Self::TIME_INFINITY { Self::TIME_INFINITY } else { self.m_Timeout - self.CpuTimePassedSinceStartOrReset() }
	}

	#[inline(always)]
	pub fn SetTestModuloFactor(&mut self, timeout_test_modulo_factor: u32)
	{
		self.m_TimeoutTestModuloFactor = timeout_test_modulo_factor;
	}
}
