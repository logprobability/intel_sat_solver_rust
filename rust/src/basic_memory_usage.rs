// Copyright(C) 2021 Intel Corporation
// SPDX - License - Identifier: MIT

#[cfg(unix)]
mod sys
{
	#[repr(C)]
	struct Timeval
	{
		tv_sec: std::ffi::c_long,
		tv_usec: std::ffi::c_long,
	}

	#[repr(C)]
	struct Rusage
	{
		ru_utime: Timeval,
		ru_stime: Timeval,
		ru_maxrss: std::ffi::c_long,
		// The rest of the fields (14 long's on both Linux and macOS, including ru_maxrss), which we don't need
		ru_rest: [std::ffi::c_long; 13],
	}

	extern "C"
	{
		fn getrusage(who: std::ffi::c_int, usage: *mut Rusage) -> std::ffi::c_int;
		fn sysconf(name: std::ffi::c_int) -> std::ffi::c_long;
	}

	const RUSAGE_SELF: std::ffi::c_int = 0;
	#[cfg(target_os = "linux")]
	const SC_PAGESIZE: std::ffi::c_int = 30;
	#[cfg(not(target_os = "linux"))]
	const SC_PAGESIZE: std::ffi::c_int = 29;

	pub fn GetPeakRSS() -> usize
	{
		let mut rusage = std::mem::MaybeUninit::<Rusage>::zeroed();
		unsafe
		{
			getrusage(RUSAGE_SELF, rusage.as_mut_ptr());
			(rusage.assume_init().ru_maxrss * 1024) as usize
		}
	}

	pub fn parse_statm(statm: Option<String>) -> usize
	{
		// "%*s%ld": skip the first token, read the second
		let rss = match statm.as_ref().and_then(|s| s.split_whitespace().nth(1)).and_then(|t| t.parse::<i64>().ok())
		{
			// Can't open or can't read
			None => return 0,
			Some(rss) => rss,
		};
		rss as usize * unsafe { sysconf(SC_PAGESIZE) } as usize
	}

	pub fn GetCurrentRSS() -> usize
	{
		parse_statm(std::fs::read_to_string("/proc/self/statm").ok())
	}
}

#[cfg(not(unix))]
mod sys
{
	pub fn GetPeakRSS() -> usize
	{
		0
	}

	pub fn GetCurrentRSS() -> usize
	{
		0
	}
}

pub use sys::*;

pub fn GetPeakRSSMb() -> usize
{
	GetPeakRSS() >> 20
}

pub fn GetCurrentRSSMb() -> usize
{
	GetCurrentRSS() >> 20
}
