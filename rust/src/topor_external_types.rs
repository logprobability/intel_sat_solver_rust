// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use std::fmt;

// The solver status
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum TToporStatus
{
	// Status unknown
	STATUS_UNDECIDED,
	// The latest invocation had returned SAT and no clauses contradicting the model were introduced since
	STATUS_SAT,
	// The latest invocation returned UNSAT, but this status might be temporary (under assumptions)
	STATUS_UNSAT,
	// The latest invocation returned USER_INTERRUPT
	STATUS_USER_INTERRUPT,
	/*
	* Only unrecoverable status values below
	*/
	// The instance is forever contradictory.
	STATUS_CONTRADICTORY,
	/*
	* Only permanently erroneous values below
	*/
	// The instance is in memory-out state, meaning that one of the allocations fails.
	STATUS_ALLOC_FAILED,
	// Data doesn't fit into the buffer
	STATUS_INDEX_TOO_NARROW,
	// Parameter-related error
	STATUS_PARAM_ERROR,
	// Error while processing assumption-required queries
	STATUS_ASSUMPTION_REQUIRED_ERROR,
	// Global timeout reached
	STATUS_GLOBAL_TIMEOUT,
	// Problem when trying to access/write the DRAT file
	STATUS_DRAT_FILE_PROBLEM,
	// An explicit-clauses-only function invoked in compressed mode
	STATUS_COMPRESSED_MISMATCH,
	// So-far: cannot accommodate the last possible variable if sizeof(TLit) == sizeof(size_t), since the allocation will fail
	STATUS_EXOTIC_ERROR,
}

impl TToporStatus
{
	pub const STATUS_FIRST_UNRECOVERABLE: TToporStatus = TToporStatus::STATUS_CONTRADICTORY;
	pub const STATUS_FIRST_PERMANENT_ERROR: TToporStatus = TToporStatus::STATUS_ALLOC_FAILED;
}

// Return value of Topor solving functions
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum TToporReturnVal
{
	// Satisfiable
	RET_SAT,
	// Unsatisfiable
	RET_UNSAT,
	// Time-out for the last Solve invocation (the run-time is beyond user-provided threshold value in Solve's parameter)
	RET_TIMEOUT_LOCAL,
	// Conflict-out (the number of conflicts is beyond user's threshold value)
	RET_CONFLICT_OUT,
	// Memory-out
	RET_MEM_OUT,
	// Interrupted by the user
	RET_USER_INTERRUPT,
	// Data doesn't fit into the buffer anymore; Try to run Topor with a wider TUInd, if you get this error
	RET_INDEX_TOO_NARROW,
	// Parameter-related error
	RET_PARAM_ERROR,
	// Error while processing assumption-required queries
	RET_ASSUMPTION_REQUIRED_ERROR,
	// Global time-out (the run-time is beyond user-provided threshold value in /global/overall_timeout parameter: the solver is unusable, if this value is returned)
	RET_TIMEOUT_GLOBAL,
	// Problem with DRAT file generation
	RET_DRAT_FILE_PROBLEM,
	// Exotic error: see the error string for more information
	RET_EXOTIC_ERROR,
}

impl fmt::Display for TToporReturnVal
{
	fn fmt(&self, os: &mut fmt::Formatter<'_>) -> fmt::Result
	{
		os.write_str(match self
		{
			TToporReturnVal::RET_SAT => "SAT",
			TToporReturnVal::RET_UNSAT => "UNSAT",
			TToporReturnVal::RET_TIMEOUT_LOCAL => "TIMEOUT_LOCAL",
			TToporReturnVal::RET_CONFLICT_OUT => "CONFLICT_OUT",
			TToporReturnVal::RET_MEM_OUT => "MEM_OUT",
			TToporReturnVal::RET_USER_INTERRUPT => "USER_INTERRUPT",
			TToporReturnVal::RET_INDEX_TOO_NARROW => "INDEX_TOO_NARROW",
			TToporReturnVal::RET_PARAM_ERROR => "PARAM_ERROR",
			TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR => "ASSUMPTION_REQUIRED_ERROR",
			TToporReturnVal::RET_TIMEOUT_GLOBAL => "TIMEOUT_GLOBAL",
			TToporReturnVal::RET_DRAT_FILE_PROBLEM => "DRAT_FILE_PROBLEM",
			TToporReturnVal::RET_EXOTIC_ERROR => "EXOTIC_ERROR",
		})
	}
}

// Literal value in Topor
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum TToporLitVal
{
	VAL_SATISFIED,
	VAL_UNSATISFIED,
	VAL_UNASSIGNED,
	VAL_DONT_CARE,
}

// Callbacks

// Callbacks return TStopTopor, which indicates, whether the solver should be stopped
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(non_camel_case_types)]
pub enum TStopTopor
{
	VAL_STOP,
	VAL_CONTINUE,
}

// The signed literal type, used to provide literals by the user
// The C++ library instantiates the solver for int32_t literals only: CTopor<int32_t, uint32_t, false>, CTopor<int32_t, uint64_t, false> and CTopor<int32_t, uint64_t, true>
pub type TLit = i32;

// New learnt clause report callback type
pub type TCbNewLearntCls = Box<dyn FnMut(&[TLit]) -> TStopTopor>;

// Stop-now callback type
pub type TCbStopNow = Box<dyn FnMut() -> TStopTopor>;
