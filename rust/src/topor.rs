// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::CTopi;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub use crate::topi::{TGetNextUnitClause, TReportUnitCls};
pub use crate::topi_types::TUInd;
pub use crate::topor_external_types::*;

// The template types are:
// (1) The unsigned index into the clause and watch buffers TUInd (Ind)
// The solver is designed to be able to work with any index type as long as sizeof(TUInd) >= sizeof(TLit) and sizeof(TUInd) <= sizeof(size_t)
// (2) A Boolean specifying whether to compress the clause buffer
// The signed literal type TLit (used to provide literals by the user; Internally, the solver uses the corresponding unsigned type of the same width) is always int32_t,
// precisely as in the instantiations provided by the C++ library: CTopor<int32_t, uint32_t, false>, CTopor<int32_t, uint64_t, false> and CTopor<int32_t, uint64_t, true>
pub struct CTopor<Ind: TUInd = u32, const Compress: bool = false>
{
	pub(crate) m_Topi: Box<CTopi<Ind, Compress>>,
}

// TUInd must be at least as wide as TLit (since the clause and watch buffers will store literals, amongst others)
const _: () = assert!(std::mem::size_of::<u32>() >= std::mem::size_of::<TLit>());
// TUInd must not be wider than size_t
const _: () = assert!(std::mem::size_of::<u64>() <= std::mem::size_of::<usize>());

impl<Ind: TUInd, const Compress: bool> CTopor<Ind, Compress>
{
	// varNumHint is the expected number of variables; it is a non-mandatory hint, which, if provided correctly, helps the solver initialize faster
	pub fn new(varsNumHint: TLit) -> Self
	{
		CTopor { m_Topi: Box::new(CTopi::new(varsNumHint)) }
	}

	// Add a clause, where, currently, clauses are permanent and cannot be deleted
	// A 0 in the span is interpreted as end-of-clause (so, the solver stops reading the clause, if 0 is encountered)
	// However, the span doesn't have to contain a 0.
	pub fn AddClause(&mut self, c: &[TLit])
	{
		self.m_Topi.AddUserClause(c);
	}

	// Solve, given the added clauses and the following information, relevant only for the current invocation:
	// - A set of assumptions (can, optionally, be 0-ended)
	// - A time-out in seconds in toInSecIsCpuTime
	//		where the second member of the pair is true, iff the timeout refers to CPU time, otherwise, it's Wall time.
	// - A conflict threshold on the current invocation
	// The C++ defaults are: assumps = {}, toInSecIsCpuTime = (numeric_limits<double>::max(), true), confThr = numeric_limits<uint64_t>::max()
	pub fn Solve(&mut self, assumps: &[TLit], toInSecIsCpuTime: (f64, bool), confThr: u64) -> TToporReturnVal
	{
		self.m_Topi.Solve(assumps, toInSecIsCpuTime, confThr)
	}

	// Must be invoked immediately after a Solve invocation, which returned TToporReturnVal::RET_UNSAT
	// Is assumption which appeared at assumps[assumpInd] during the latest Solve call, that is, is it part of the UNSAT core
	// Sets the status to permanent error, if the latest Solve invocation didn't return TToporReturnVal::RET_UNSAT or if assumpInd >= AssumpsSize()
	pub fn IsAssumptionRequired(&mut self, assumpInd: usize) -> bool
	{
		self.m_Topi.IsAssumptionRequired(assumpInd)
	}

	// Boost the score of the variable v by value (the greater the value is, the greater the bump is); the C++ default is value = 1.0
	pub fn BoostScore(&mut self, v: TLit, value: f64)
	{
		self.m_Topi.BoostScore(v, value);
	}

	// Fix the polarity of the variable |l| to l
	// onlyOnce = false: change until ClearUserPolarityInfo(|l|) is invoked (or, otherwise, forever)
	// onlyOnce = true: change only once right now
	// The C++ default is onlyOnce = false
	pub fn FixPolarity(&mut self, l: TLit, onlyOnce: bool)
	{
		self.m_Topi.FixPolarity(l, onlyOnce);
	}

	// Create an internal literal for l: for advanced usages to play with internal literal ordering
	pub fn CreateInternalLit(&mut self, l: TLit)
	{
		self.m_Topi.CreateInternalLit(l);
	}

	// Clear any polarity information of the variable v, provided by the user (so, the default solver's heuristic will be used to determine polarity)
	pub fn ClearUserPolarityInfo(&mut self, v: TLit)
	{
		self.m_Topi.ClearUserPolarityInfo(v);
	}

	// Set a parameter value; using double for the value, since it encompasses all the arithmetic types, which can be used for parameters, that is:
	// Signed and unsigned integers of at most 32 bits and floating-points of at most the size of a C++ double
	// Sets the status to permanent error, if the name/value combination is wrong. Run GetErrorStatus for details.
	pub fn SetParam(&mut self, paramName: &str, newVal: f64)
	{
		self.m_Topi.SetParam(paramName, newVal);
	}

	// Get the value of a literal
	pub fn GetLitValue(&self, l: TLit) -> TToporLitVal
	{
		self.m_Topi.GetValue(l)
	}

	// Get the decision level of the given literal (which must be assigned!)
	pub fn GetLitDecLevel(&self, l: TLit) -> TLit
	{
		self.m_Topi.GetLitDecLevel(l)
	}

	// Get the number of Solve invocations
	pub fn GetSolveInvs(&self) -> u64
	{
		self.m_Topi.GetSolveInvs()
	}

	// Get the maximal user-provided variable that wasn't simplified away (since it, e.g., only participated in tautologies)
	pub fn GetMaxUserVar(&self) -> TLit
	{
		self.m_Topi.GetMaxUserVar()
	}

	// Get a string with some statistics; the C++ default is forcePrintingHead = false
	pub fn GetStatStrShort(&mut self, forcePrintingHead: bool) -> String
	{
		self.m_Topi.GetStatStrShort(forcePrintingHead)
	}

	// Get the number of conflicts so far
	pub fn GetConflictsNumber(&self) -> u64
	{
		self.m_Topi.GetConflictsNumber()
	}

	// Get the number of active clauses in the solver (undeleted binary and long clauses both initial & learnt)
	pub fn GetActiveClss(&self) -> u64
	{
		self.m_Topi.GetActiveClss()
	}

	// Get the number of backtracks
	pub fn GetBacktracks(&self) -> u64
	{
		self.m_Topi.GetBacktracks()
	}

	// The number of backtrack levels, saved by reusing assumptions
	pub fn GetAssumpReuseBacktrackLevelsSaved(&self) -> u64
	{
		self.m_Topi.GetAssumpReuseBacktrackLevelsSaved()
	}

	// Get the  number of propagations (implications)
	pub fn GetPropagations(&self) -> u64
	{
		self.m_Topi.GetPropagations()
	}

	// The maximal internal variable number (can be thought of as the number of active variables in the solver)
	pub fn GetMaxInternalVar(&self) -> TLit
	{
		self.m_Topi.GetMaxInternalVar()
	}

	// Get the number of active long (non-binary) learnt clauses in the solver
	pub fn GetActiveLongLearntClss(&self) -> u64
	{
		self.m_Topi.GetActiveLongLearntClss()
	}

	// Get the whole model
	pub fn GetModel(&self) -> Vec<TToporLitVal>
	{
		self.m_Topi.GetModel()
	}

	// Is there an error in the solver?
	pub fn IsError(&self) -> bool
	{
		self.m_Topi.IsError()
	}

	// Get the explanation of the current status
	// If IsError() holds, a non-empty explanation is mandatory, otherwise it might be empty (or not)
	pub fn GetStatusExplanation(&self) -> String
	{
		self.m_Topi.GetStatusExplanation()
	}

	// Get the description of the parameters
	pub fn GetParamsDescr(&self) -> String
	{
		self.m_Topi.GetParamsDescr()
	}

	// Dump DRAT
	pub fn DumpDrat(&mut self, openedDratFile: Box<dyn std::io::Write>, isDratBinary: bool, dratSortEveryClause: bool)
	{
		self.m_Topi.DumpDrat(openedDratFile, isDratBinary, dratSortEveryClause);
	}

	// Set callback: stop now
	pub fn SetCbStopNow(&mut self, CbStopNow: Option<TCbStopNow>)
	{
		self.m_Topi.SetCbStopNow(CbStopNow);
	}

	// Interrupt now
	pub fn InterruptNow(&self)
	{
		self.m_Topi.InterruptNow();
	}

	// The handle, which can be used to interrupt the solver asynchronously (e.g., from a signal handler or from another thread) by storing true into it
	pub fn GetInterruptNowHandle(&self) -> Arc<AtomicBool>
	{
		self.m_Topi.GetInterruptNowHandle()
	}

	// Set callback: new learnt clause
	pub fn SetCbNewLearntCls(&mut self, CbNewLearntCls: Option<TCbNewLearntCls>)
	{
		self.m_Topi.SetCbNewLearntCls(CbNewLearntCls);
	}

	// Backtrack to the provided decision level
	pub fn Backtrack(&mut self, decLevel: TLit)
	{
		self.m_Topi.Backtrack(decLevel, false, true);
	}

	// Changing the configuration to # configNum, so that every configNum generates a unique configuration, where 0 makes no change. This is used for enabling different configs in parallel solving. Returns a unique configuration string"
	pub fn ChangeConfigToGiven(&mut self, configNum: u16) -> String
	{
		self.m_Topi.ChangeConfigToGiven(configNum)
	}

	pub fn SetParallelData(&mut self, threadId: u32, ReportUnitClause: Option<TReportUnitCls>, GetNextUnitClause: Option<TGetNextUnitClause>)
	{
		self.m_Topi.SetParallelData(threadId, ReportUnitClause, GetNextUnitClause);
	}
}
