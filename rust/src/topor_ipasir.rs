// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

// The IPASIR interface (see ToporIpasir.h for the license and the documentation of IPASIR): the C symbols are identical to those of the C++ version

use crate::std_string::stod;
use crate::topor::*;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::rc::Rc;

type TTerminateFunc = Option<unsafe extern "C" fn(state: *mut c_void) -> c_int>;

// class CToporIpasirWrapper : public CTopor<>
pub struct CToporIpasirWrapper
{
	m_Topor: CTopor<u32, false>,
	m_CurrCls: Vec<c_int>,
	m_CurrAssumps: Vec<c_int>,
	m_Assump2Ind: HashMap<c_int, usize>,
	m_PrevAssump2Ind: HashMap<c_int, usize>,
	// The terminate state and function are shared with the stop-now callback (which captures the wrapper by reference in the C++ version)
	m_CurrTerminateState: Rc<Cell<*mut c_void>>,
	m_CurrTerminateFunc: Rc<Cell<TTerminateFunc>>,
}

impl Default for CToporIpasirWrapper
{
	fn default() -> Self
	{
		CToporIpasirWrapper::new()
	}
}

impl CToporIpasirWrapper
{
	pub fn new() -> Self
	{
		CToporIpasirWrapper
		{
			m_Topor: CTopor::new(0),
			m_CurrCls: Vec::new(),
			m_CurrAssumps: Vec::new(),
			m_Assump2Ind: HashMap::new(),
			m_PrevAssump2Ind: HashMap::new(),
			m_CurrTerminateState: Rc::new(Cell::new(std::ptr::null_mut())),
			m_CurrTerminateFunc: Rc::new(Cell::new(None)),
		}
	}

	pub fn Add(&mut self, lit: c_int)
	{
		if lit == 0
		{
			self.m_Topor.AddClause(&self.m_CurrCls);
			self.m_CurrCls.clear();
		}
		else
		{
			self.m_CurrCls.push(lit);
		}
	}

	pub fn Assume(&mut self, lit: c_int)
	{
		if !self.m_Assump2Ind.contains_key(&lit)
		{
			self.m_Assump2Ind.insert(lit, self.m_CurrAssumps.len());
			self.m_CurrAssumps.push(lit);
		}
	}

	pub fn Solve(&mut self) -> c_int
	{
		let trv = self.m_Topor.Solve(&self.m_CurrAssumps, (f64::MAX, true), u64::MAX);
		self.m_PrevAssump2Ind = std::mem::take(&mut self.m_Assump2Ind);
		self.m_Assump2Ind.clear();
		self.m_CurrAssumps.clear();

		match trv
		{
			TToporReturnVal::RET_SAT => 10,
			TToporReturnVal::RET_UNSAT => 20,
			_ => 0,
		}
	}

	pub fn Val(&mut self, lit: c_int) -> c_int
	{
		let tlv = self.m_Topor.GetLitValue(lit);
		match tlv
		{
			TToporLitVal::VAL_SATISFIED | TToporLitVal::VAL_DONT_CARE => lit,
			TToporLitVal::VAL_UNSATISFIED => -lit,
			_ => 0,
		}
	}

	pub fn Failed(&mut self, lit: c_int) -> c_int
	{
		// m_PrevAssump2Ind[lit]: inserts 0, if lit is missing
		let assumpInd = *self.m_PrevAssump2Ind.entry(lit).or_insert(0);
		self.m_Topor.IsAssumptionRequired(assumpInd) as c_int
	}

	pub fn SetTerminate(&mut self, state: *mut c_void, terminate: TTerminateFunc)
	{
		if terminate.is_none()
		{
			self.m_CurrTerminateFunc.set(None);
			self.m_CurrTerminateState.set(std::ptr::null_mut());
			return;
		}

		self.m_CurrTerminateFunc.set(terminate);
		self.m_CurrTerminateState.set(state);
		let currTerminateFunc = self.m_CurrTerminateFunc.clone();
		let currTerminateState = self.m_CurrTerminateState.clone();
		let StopTopor = move || match currTerminateFunc.get()
		{
			Some(terminateFunc) if unsafe { terminateFunc(currTerminateState.get()) } != 0 => TStopTopor::VAL_STOP,
			_ => TStopTopor::VAL_CONTINUE,
		};

		self.m_Topor.SetCbStopNow(Some(Box::new(StopTopor)));
	}

	pub fn SetParam(&mut self, paramName: &str, paramVal: &str)
	{
		// std::stod throws in the C++ version, if the conversion fails (which terminates the program, since the exception isn't caught)
		self.m_Topor.SetParam(paramName, stod(paramVal).expect("ipasir_set_parameter: couldn't convert the parameter value to double"));
	}

	// The underlying solver
	pub fn Topor(&mut self) -> &mut CTopor<u32, false>
	{
		&mut self.m_Topor
	}
}

#[no_mangle]
pub extern "C" fn ipasir_signature() -> *const c_char
{
	c"IntelSatSolver".as_ptr()
}

#[no_mangle]
pub extern "C" fn ipasir_init() -> *mut c_void
{
	let tiw = Box::new(CToporIpasirWrapper::new());

	Box::into_raw(tiw) as *mut c_void
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet
#[no_mangle]
pub unsafe extern "C" fn ipasir_release(solver: *mut c_void)
{
	if !solver.is_null()
	{
		drop(Box::from_raw(solver as *mut CToporIpasirWrapper));
	}
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet
#[no_mangle]
pub unsafe extern "C" fn ipasir_add(solver: *mut c_void, lit: c_int)
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.Add(lit);
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet
#[no_mangle]
pub unsafe extern "C" fn ipasir_assume(solver: *mut c_void, lit: c_int)
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.Assume(lit);
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet
#[no_mangle]
pub unsafe extern "C" fn ipasir_solve(solver: *mut c_void) -> c_int
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.Solve()
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet
#[no_mangle]
pub unsafe extern "C" fn ipasir_val(solver: *mut c_void, lit: c_int) -> c_int
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.Val(lit)
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet
#[no_mangle]
pub unsafe extern "C" fn ipasir_failed(solver: *mut c_void, lit: c_int) -> c_int
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.Failed(lit)
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet; terminate (if any) must be callable with state for as long as it is set
#[no_mangle]
pub unsafe extern "C" fn ipasir_set_terminate(solver: *mut c_void, state: *mut c_void, terminate: TTerminateFunc)
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.SetTerminate(state, terminate);
}

/// # Safety
/// solver must have been returned by ipasir_init and not released yet; paramName and paramVal must be valid 0-terminated strings
#[no_mangle]
pub unsafe extern "C" fn ipasir_set_parameter(solver: *mut c_void, paramName: *const c_char, paramVal: *const c_char)
{
	let tiw = &mut *(solver as *mut CToporIpasirWrapper);
	tiw.SetParam(&CStr::from_ptr(paramName).to_string_lossy(), &CStr::from_ptr(paramVal).to_string_lossy());
}
