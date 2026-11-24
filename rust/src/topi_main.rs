// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::color_print::{ansi_color_code, print_as_color};
use crate::topi::*;
use crate::{ReserveExactly};
use std::io::Write;
use std::sync::atomic::Ordering;

// time_t is long on the supported Unix platforms (glibc and macOS)
#[cfg(unix)]
type time_t = std::ffi::c_long;

#[cfg(unix)]
extern "C"
{
	fn time(tloc: *mut time_t) -> time_t;
	// Fills result (a struct tm, which starts with the int fields: tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year) and returns it, or null on failure;
	// unlike localtime, it doesn't share a static buffer, so solvers in different threads don't race
	fn localtime_r(timep: *const time_t, result: *mut std::ffi::c_int) -> *mut std::ffi::c_int;
}

// operator<< for a double with the default ostream formatting (that is, %g with 6 significant digits)
pub(crate) fn DoubleToOstreamStr(v: f64) -> String
{
	if v.is_nan()
	{
		return "nan".to_string();
	}
	if v.is_infinite()
	{
		return if v < 0. { "-inf".to_string() } else { "inf".to_string() };
	}
	if v == 0.
	{
		return if v.is_sign_negative() { "-0".to_string() } else { "0".to_string() };
	}

	// 6 significant digits: d.ddddde[+-]x
	let sci = format!("{:.5e}", v);
	let (mantissa, exp) = sci.split_once('e').unwrap();
	let exp: i32 = exp.parse().unwrap();

	fn StripTrailingZeros(s: &str) -> &str
	{
		if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
	}

	if (-4..6).contains(&exp)
	{
		let fixed = format!("{:.*}", (5 - exp) as usize, v);
		StripTrailingZeros(&fixed).to_string()
	}
	else
	{
		format!("{}e{}{:02}", StripTrailingZeros(mantissa), if exp < 0 { '-' } else { '+' }, exp.abs())
	}
}

// std::stod: skips the leading whitespace and converts the longest valid prefix; None stands for an exception
fn stod(s: &str) -> Option<f64>
{
	let s = s.trim_start_matches(|c: char| c == ' ' || ('\t'..='\r').contains(&c));
	let mut end = s.len();
	while end > 0
	{
		if s.is_char_boundary(end)
		{
			if let Ok(d) = s[..end].parse::<f64>()
			{
				return Some(d);
			}
		}
		end -= 1;
	}
	None
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn DumpSetUp(&mut self, filePrefix: &str)
	{
		let mut ss = String::new();

		ss += filePrefix;
		ss += "_";

		ss += &format!("{:p}_{}_", self as *const Self, std::process::id());
		// Not in the C++ version: the number of the dump in the process, since the address (here, of the solver before it's boxed) and the time in seconds
		// don't tell apart the solvers created in the same second, whose dumps would overwrite each other
		static DumpsNum: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
		ss += &format!("{}_", DumpsNum.fetch_add(1, std::sync::atomic::Ordering::Relaxed));

		#[cfg(unix)]
		// SAFETY: tmBuf is larger than struct tm (56 bytes on the supported platforms) and as aligned; localtime_r only writes into it
		unsafe
		{
			let mut now: time_t = 0;
			time(&mut now);
			let mut tmBuf = [0i64; 8];
			let current: *const std::ffi::c_int = localtime_r(&now, tmBuf.as_mut_ptr().cast());
			if !current.is_null()
			{
				let (tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year) = (*current, *current.add(1), *current.add(2), *current.add(3), *current.add(4), *current.add(5));
				ss += &format!("{}.{}.{}_{}_{}_{}", tm_mday, tm_mon + 1, 1900 + tm_year, tm_hour, tm_min, tm_sec);
			}
		}
		ss += ".cnf";

		self.m_DumpFile = std::fs::File::create(ss).ok();
	}

	pub(crate) fn ReadAnyParamsFromFile(&mut self)
	{
		let configFileName = match std::env::var("TOPOR_CONFIG_FILE")
		{
			Ok(configFileName) => configFileName,
			Err(_) => return,
		};

		let tcf = match std::fs::read(&configFileName)
		{
			Ok(tcf) => String::from_utf8_lossy(&tcf).into_owned(),
			Err(_) =>
			{
				self.SetStatus(TToporStatus::STATUS_PARAM_ERROR, "Cannot open the configuration parameter file ".to_string() + &configFileName);
				return;
			}
		};

		// getline: the lines are separated by '\n'; there is no line after the last '\n'
		let mut lineNum: u32 = 1;
		for paramSetting in tcf.split_terminator('\n')
		{
			let SetErrStatus = |topi: &Self, reason: &str|
			{
				topi.SetStatus(TToporStatus::STATUS_PARAM_ERROR, "Could parse line".to_string() + &lineNum.to_string() + " : " + paramSetting + " (after trimming \\t\\r\\v\\n); Reason: " + reason);
			};

			// Consider paramSetting = " /q/w 1\n"
			// Note that the C++ version invokes Trim on paramSetting here, but ignores the result, so there is no actual trimming

			// spaceInd = 4 for "/q/w 1"
			let spaceInd = match paramSetting.find(' ')
			{
				Some(spaceInd) => spaceInd,
				None =>
				{
					SetErrStatus(self, "space not found");
					return;
				}
			};

			// paramName = "/q/w" for "/q/w 1"
			let paramName = &paramSetting[..spaceInd];
			// paramVal = "1" for "/q/w 1"
			let paramVal = &paramSetting[spaceInd + 1..];
			let paramValDouble = match stod(paramVal)
			{
				Some(paramValDouble) => paramValDouble,
				None =>
				{
					SetErrStatus(self, &("couldn't convert the value ".to_string() + paramVal + " to double"));
					return;
				}
			};

			self.SetParam(paramName, paramValDouble);
			if self.IsUnrecoverable()
			{
				return;
			}

			lineNum += 1;
		}
	}

	pub fn SetParam(&mut self, paramName: &str, newVal: f64)
	{
		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "r {} {}", paramName, DoubleToOstreamStr(newVal));
		}

		if self.IsUnrecoverable()
		{
			return;
		}

		fn HandleContextParam(paramName: &str, newVal: f64, contextParamPrefix: &str, paramVals: &mut Vec<(String, f64)>) -> bool
		{
			if let Some(paramNameInContext) = paramName.strip_prefix(contextParamPrefix)
			{
				paramVals.push((paramNameInContext.to_string(), newVal));
				return true;
			}
			false
		}

		if HandleContextParam(paramName, newVal, m_ContextParamAfterInitInvPrefix, &mut self.m_AfterInitInvParamVals)
		{
			return;
		}

		if HandleContextParam(paramName, newVal, m_ContextParamShortInvLifetimePrefix, &mut self.m_ShortInvLifetimeParamVals)
		{
			return;
		}

		self.m_Params.SetParam(paramName, newVal);
		if self.m_Params.IsError()
		{
			self.SetStatus(TToporStatus::STATUS_PARAM_ERROR, self.m_Params.GetErrorDescr());
		}

		self.SetOverallTimeoutIfAny();

		if self.IsMultiplierParam(paramName) || paramName == m_ModeParamName
		{
			self.SetMultipliers();
		}

		if self.IsVsidsInitOrderParam(paramName) || paramName == m_ModeParamName
		{
			self.m_VsidsHeap.SetInitOrder(self.m_Params.m_ParamVsidsInitOrder.get());
		}
	}

	pub fn IsError(&self) -> bool
	{
		self.IsErroneous()
	}

	pub(crate) fn StatusToRetVal(&self) -> TToporReturnVal
	{
		if self.IsUnrecoverable()
		{
			return self.UnrecStatusToRetVal();
		}

		if self.m_Status.get() == TToporStatus::STATUS_SAT
		{
			return TToporReturnVal::RET_SAT;
		}

		if self.m_Status.get() == TToporStatus::STATUS_UNSAT
		{
			return TToporReturnVal::RET_UNSAT;
		}

		if self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT
		{
			return TToporReturnVal::RET_USER_INTERRUPT;
		}

		TToporReturnVal::RET_EXOTIC_ERROR
	}

	pub(crate) fn UnrecStatusToRetVal(&self) -> TToporReturnVal
	{
		debug_assert!(self.IsUnrecoverable());

		let status = self.m_Status.get();
		if status == TToporStatus::STATUS_CONTRADICTORY { return TToporReturnVal::RET_UNSAT; }
		if status == TToporStatus::STATUS_ALLOC_FAILED { return TToporReturnVal::RET_MEM_OUT; }
		if status == TToporStatus::STATUS_INDEX_TOO_NARROW { return TToporReturnVal::RET_INDEX_TOO_NARROW; }
		if status == TToporStatus::STATUS_PARAM_ERROR { return TToporReturnVal::RET_PARAM_ERROR; }
		if status == TToporStatus::STATUS_ASSUMPTION_REQUIRED_ERROR { return TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR; }
		if status == TToporStatus::STATUS_GLOBAL_TIMEOUT { return TToporReturnVal::RET_TIMEOUT_GLOBAL; }
		if status == TToporStatus::STATUS_DRAT_FILE_PROBLEM { return TToporReturnVal::RET_DRAT_FILE_PROBLEM; }

		// The C++ version asserts m_Status == TToporStatus::STATUS_EXOTIC_ERROR here, which doesn't cover STATUS_COMPRESSED_MISMATCH

		TToporReturnVal::RET_EXOTIC_ERROR
	}

	pub(crate) fn HandleIncomingUserVar(&mut self, v: TLit, isUndoable: bool)
	{
		if v as usize >= self.m_E2ILitMap.cap()
		{
			self.m_E2ILitMap.reserve_atleast_init(v as usize + 1, BadULit as u8);
			if self.m_E2ILitMap.uninitialized_or_erroneous()
			{
				self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "HandleIncomingUserVar: couldn't reserve m_E2IVarMap".to_string());
				return;
			}
		}

		if self.m_E2ILitMap[v as usize] == 0
		{
			self.m_LastExistingVar += 1;
			self.m_E2ILitMap[v as usize] = GetLit(self.m_LastExistingVar, false);
			if self.UseI2ELitMap()
			{
				self.m_I2ELitMap.reserve_atleast_init(self.m_LastExistingVar as usize + 1, 0);
				if self.m_I2ELitMap.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "HandleIncomingUserVar: couldn't reserve m_I2ELitMap".to_string());
					return;
				}
				self.m_I2ELitMap[self.m_LastExistingVar as usize] = v;
			}

			self.m_Stat.UpdateMaxInternalVar(self.m_LastExistingVar as u64);

			if !isUndoable
			{
				self.m_VsidsHeap.insert(self.m_LastExistingVar, 0.0);

				if self.m_VsidsHeap.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "HandleIncomingUserVar: couldn't insert into m_VsidsHeap".to_string());
					return;
				}
			}
			else
			{
				self.m_NewExternalVarsAddUserCls.push_back(v);
				if self.m_NewExternalVarsAddUserCls.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "HandleIncomingUserVar: couldn't push into m_NewExternalVarsAddUserCls".to_string());
					return;
				}
			}

			if self.m_LastExistingVar as usize >= self.m_AssignmentInfo.cap()
			{
				self.m_AssignmentInfo.reserve_atleast_init(self.GetNextVar() as usize, 0);
				if self.m_AssignmentInfo.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "HandleIncomingUserVar: couldn't realloc m_AssignmentInfo".to_string());
					return;
				}
			}

			if self.m_LastExistingVar as usize >= self.m_VarInfo.cap()
			{
				self.m_VarInfo.reserve_atleast_init(self.GetNextVar() as usize, 0);
				if self.m_VarInfo.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "HandleIncomingUserVar: couldn't realloc m_VarInfo".to_string());
					return;
				}
			}
		}

		self.m_Stat.m_MaxUserVar = std::cmp::max(self.m_Stat.m_MaxUserVar, v);
	}

	pub(crate) fn DumpSpan(&mut self, c: &[TLit], prefix: &str, suffix: &str, addNewLine: bool)
	{
		let dumpFile = self.m_DumpFile.as_mut().unwrap();
		let _ = write!(dumpFile, "{}", prefix);
		for i in 0..c.len()
		{
			let _ = write!(dumpFile, "{}", c[i]);
			if i != c.len() - 1
			{
				let _ = write!(dumpFile, " ");
			}
		}
		let _ = write!(dumpFile, "{}", suffix);
		if addNewLine
		{
			let _ = writeln!(dumpFile);
		}
	}

	pub fn AddUserClause(&mut self, c: &[TLit])
	{
		if self.m_DumpFile.is_some() && !self.m_Params.m_ParamDontDumpClauses.get()
		{
			self.DumpSpan(c, "", " 0", true);
		}

		self.AssumpUnsatCoreCleanUpIfRequired();

		if self.m_Params.m_ParamAddClsAtLevel0.get() && self.m_DecLevel != 0
		{
			self.Backtrack(0, false, false);
		}

		let lastExistingVarStart = self.m_LastExistingVar;

		let mut isSuccess = false;
		let mut boostScores = false;

		// The body of the function; the on-exit function is applied right after it on every path
		self.AddUserClauseBody(c, lastExistingVarStart, &mut isSuccess, &mut boostScores);
		self.AddUserClauseOnExit(lastExistingVarStart, isSuccess, boostScores);
	}

	// CApplyFuncOnExitFromScope<> onExit of AddUserClause
	fn AddUserClauseOnExit(&mut self, lastExistingVarStart: TUVar, isSuccess: bool, boostScores: bool)
	{
		if isSuccess
		{
			for ev in self.m_NewExternalVarsAddUserCls.get_span()
			{
				self.m_VsidsHeap.insert(GetVar(self.m_E2ILitMap[ev as usize]), 0.0);

				if self.m_VsidsHeap.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "AddUserClause: couldn't insert into m_VsidsHeap".to_string());
					return;
				}
			}

			if boostScores
			{
				let cls = self.m_HandleNewUserCls.GetCurrCls();
				let mult = if self.InitClssBoostScoreStratIsClauseSizeAware() { self.m_CurrInitClssBoostScoreMult * (1. / (cls.size() - 1) as f64) } else { self.m_CurrInitClssBoostScoreMult };
				for l in cls
				{
					let v = GetVar(l);
					self.UpdateScoreVar(v, mult);
				}

				if self.InitClssBoostScoreStratIsReversedOrder()
				{
					if self.m_CurrInitClssBoostScoreMult < self.m_Params.m_ParamInitClssBoostMultHighest.get()
					{
						self.m_CurrInitClssBoostScoreMult += self.m_Params.m_ParamInitClssBoostMultDelta.get();
					}
				}
				else
				{
					if self.m_CurrInitClssBoostScoreMult > self.m_Params.m_ParamInitClssBoostMultLowest.get()
					{
						self.m_CurrInitClssBoostScoreMult -= self.m_Params.m_ParamInitClssBoostMultDelta.get();
					}
				}
			}
		}
		else
		{
			self.m_LastExistingVar = lastExistingVarStart;
			self.m_Stat.UpdateMaxInternalVar(self.m_LastExistingVar as u64);

			for ev in self.m_NewExternalVarsAddUserCls.get_span()
			{
				// The C++ version would write through a null pointer here, if the reallocation of m_E2ILitMap is what failed
				if self.m_E2ILitMap.uninitialized_or_erroneous()
				{
					break;
				}
				self.m_E2ILitMap[ev as usize] = BadULit;
			}
		}
		self.m_NewExternalVarsAddUserCls.clear();
	}

	// The OnContradiction lambda of AddUserClause
	fn AddUserClauseOnContradiction(&mut self, cls: TSpanTULit)
	{
		let decLevel = self.GetAssignedDecLevel(cls.get(0));
		debug_assert!(decLevel != 0);
		self.Backtrack((decLevel - 1) as TLit, false, false);
		debug_assert!(!self.IsAssigned(cls.get(0)));
		let ai = &self.m_AssignmentInfo;
		self.m_ToPropagate.erase_if_may_reorder(|l| !ai[GetVar(l) as usize].m_IsAssigned(), 0);
	}

	// ProcessDelayedImplication(diL, otherWatch, parentClsInd, cis), where cis is a local empty vector (rather than m_Cis)
	fn ProcessDelayedImplicationLocalCis(&mut self, diL: TULit, otherWatch: TULit, parentClsInd: Ind)
	{
		let savedCis = std::mem::take(&mut self.m_Cis);
		self.ProcessDelayedImplication(diL, otherWatch, parentClsInd);
		self.m_Cis = savedCis;
	}

	fn AddUserClauseBody(&mut self, c: &[TLit], lastExistingVarStart: TUVar, isSuccess: &mut bool, boostScores: &mut bool)
	{
		self.m_Stat.m_AddClauseInvs += 1;

		if self.IsUnrecoverable()
		{
			return;
		}

		if c.is_empty() || c[0] == 0
		{
			// An empty clause
			self.SetStatus(TToporStatus::STATUS_CONTRADICTORY, "AddClause: an empty clause provided".to_string());
			return;
		}

		// Storing the new clause while detecting tautologies and filtering out duplicates
		self.m_HandleNewUserCls.NewClause();

		for &l in c
		{
			if l == 0
			{
				// End-of-clause; we are done with the loop
				break;
			}

			// sizeof(TLit) < sizeof(size_t), so the last possible variable can always be accommodated

			// External variable
			let v = ExternalLit2ExternalVar(l);
			self.HandleIncomingUserVar(v, true);
			if self.IsUnrecoverable()
			{
				return;
			}

			let litInternal = self.E2I(l);

			if self.IsFalsified(litInternal) && self.GetAssignedDecLevel(litInternal) == 0
			{
				// The literal is falsified at decision level 0: no need to add it to the clause
				continue;
			}

			let (isTau, isDuplicate, isAllocFailed) = self.m_HandleNewUserCls.NewLitIsTauIsDuplicate(litInternal);

			if isAllocFailed
			{
				self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "AddClause: allocation failed during tautology&duplication test".to_string());
				return;
			}

			if isTau
			{
				return;
			}

			if isDuplicate
			{
				// Duplicate literal
				continue;
			}

			if self.m_Params.m_ParamAddClsRemoveClssGloballySatByLitMinSize.get() != u32::MAX && c.len() > self.m_Params.m_ParamAddClsRemoveClssGloballySatByLitMinSize.get() as usize && self.IsSatisfied(litInternal) && self.GetAssignedDecLevel(litInternal) == 0
			{
				// The clause is globally satisfied
				return;
			}

			// Handling the watches now
			if GetMaxLit(litInternal) as usize >= self.m_Watches.cap()
			{
				self.m_Watches.reserve_atleast_init(GetMaxLit(litInternal) as usize + 1, BadULit as u8);
				if self.m_Watches.uninitialized_or_erroneous()
				{
					self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "AddClause: couldn't reserve m_Watches".to_string());
					return;
				}
			}

			let cls = self.m_HandleNewUserCls.GetCurrCls();

			if cls.size() == 2 && self.WLIsLitBetter(cls.get(1), cls.get(0))
			{
				cls.swap(0, 1);
			}

			if cls.size() > 2
			{
				if self.WLIsLitBetter(litInternal, cls.get(0))
				{
					cls.swap(0, cls.size() - 1);
					cls.swap(cls.size() - 1, 1);
				}
				else if self.WLIsLitBetter(litInternal, cls.get(1))
				{
					cls.swap(1, cls.size() - 1);
				}
			}
		}

		let cls = self.m_HandleNewUserCls.GetCurrCls();

		if cls.size() == 0
		{
			// The clause must have been contradictory (with some 0-level literals removed)
			self.SetStatus(TToporStatus::STATUS_CONTRADICTORY, "AddClause: the clause is contradictory".to_string());
			return;
		}

		if cls.size() == 1 && self.m_NewExternalVarsAddUserCls.size() == 1 && self.m_TrailStart != BadUVar && self.GetAssignedDecLevelVar(self.m_TrailStart) == 0 && (!self.IsAssigned(cls.get(0)) || self.GetAssignedDecLevel(cls.get(0)) != 0)
		{
			// Exchange cls[0] by m_TrailStart's satisfied literal

			let iSatLit = self.GetAssignedLitForVar(self.m_TrailStart);
			let eSatVar = self.m_NewExternalVarsAddUserCls[0];

			let eSatLitIt = c.iter().position(|&l| ExternalLit2ExternalVar(l) == eSatVar);
			debug_assert!(eSatLitIt.is_some());

			self.m_E2ILitMap[eSatVar as usize] = if c[eSatLitIt.unwrap()] == eSatVar { iSatLit } else { Negate(iSatLit) };
			self.m_NewExternalVarsAddUserCls.clear();
			return;
		}

		*isSuccess = true;

		debug_assert!(self.NV(2) || self.P(&("The user clause to internal clause: ".to_string() + &self.SUserLits(c) + " to " + &self.SLits(cls, false) + "\n")));

		if cls.size() == 1
		{
			// Unary clause: add an implication (or a delayed implication)
			if self.IsSatisfied(cls.get(0))
			{
				if self.GetAssignedDecLevel(cls.get(0)) != 0
				{
					self.ProcessDelayedImplicationLocalCis(cls.get(0), BadULit, Self::BadClsInd);
				}
			}
			else
			{
				if self.IsFalsified(cls.get(0))
				{
					self.AddUserClauseOnContradiction(cls);
				}
				self.Assign(cls.get(0), Self::BadClsInd, BadULit, 0, true, false);

				// If we've just assigned the variable globally and the variable is new,
				// there is no need to run Simplify, since it doesn't satisfy or appear falsified in any existing clauses, and
				// this very function will make sure that any future clauses satisfied by it are skipped and any falsified appearances are skipped too
				if GetVar(cls.get(0)) == self.m_LastExistingVar && self.m_LastExistingVar != lastExistingVarStart && self.m_LastGloballySatisfiedLitAfterSimplify == self.m_VarInfo[GetVar(cls.get(0)) as usize].m_TrailPrev
				{
					self.m_LastGloballySatisfiedLitAfterSimplify = self.m_LastExistingVar;
				}
			}

			return;
		}

		let clsStart = self.AddClsToBufferAndWatch(cls, false, false);
		debug_assert!(self.NV(2) || self.P(&("Clause start is ".to_string() + &HexStrInd(clsStart) + "\n")));
		if self.IsUnrecoverable()
		{
			return;
		}

		if self.InitClssBoostScoreStrat() != 0 && cls.size() > 1
		{
			*boostScores = true;
		}

		// Take care of unit and contradictory clauses
		if self.IsFalsified(cls.get(0)) || (!self.IsAssigned(cls.get(0)) && self.IsFalsified(cls.get(1)))
		{
			if self.IsFalsified(cls.get(0))
			{
				// Contradictory!
				self.AddUserClauseOnContradiction(cls);
			}

			// By now the clause is either unit or with two unassigned literals
			if self.IsFalsified(cls.get(1))
			{
				debug_assert!(!self.IsAssigned(cls.get(0)));
				self.Assign(cls.get(0), clsStart, cls.get(1), self.GetAssignedDecLevel(cls.get(1)), true, false);
			}
		}

		// Take care of delayed implications
		if self.IsSatisfied(cls.get(0)) && self.IsFalsified(cls.get(1)) && self.GetAssignedDecLevel(cls.get(0)) > self.GetAssignedDecLevel(cls.get(1))
		{
			self.ProcessDelayedImplicationLocalCis(cls.get(0), cls.get(1), clsStart);
		}
	}

	pub fn IsAssumptionRequired(&mut self, assumpInd: usize) -> bool
	{
		if self.m_Status.get() == TToporStatus::STATUS_CONTRADICTORY
		{
			return false;
		}

		if self.m_Stat.m_SolveInvs == 0 || self.m_Status.get() != TToporStatus::STATUS_UNSAT || assumpInd >= self.m_UserAssumps.len()
		{
			self.SetStatus(TToporStatus::STATUS_ASSUMPTION_REQUIRED_ERROR, (if self.m_Stat.m_SolveInvs == 0 { "No Solve invocations so far" } else if self.m_Status.get() != TToporStatus::STATUS_UNSAT { "The latest Solve didn't return TToporStatus::STATUS_UNSAT" } else { "The assumption ID is beyond the number of assumptions in the latest Solve invocation" }).to_string());
			return false;
		}

		if self.m_LatestAssumpUnsatCoreSolveInvocation != self.m_Stat.m_SolveInvs
		{
			debug_assert!(self.m_SelfContrOrGloballyUnsatAssumpSolveInv == self.m_Stat.m_SolveInvs || self.m_LatestEarliestFalsifiedAssumpSolveInv == self.m_Stat.m_SolveInvs);
			debug_assert!(self.m_SelfContrOrGloballyUnsatAssumpSolveInv != self.m_LatestEarliestFalsifiedAssumpSolveInv);
			debug_assert!(self.m_LatestEarliestFalsifiedAssumpSolveInv != self.m_Stat.m_SolveInvs || self.IsAssigned(self.m_LatestEarliestFalsifiedAssump));

			// Making sure that we may return true only for one external variable out of a group mapped to the same internal variable
			for externalLitI in 0..self.m_UserAssumps.len()
			{
				let externalLit = self.m_UserAssumps[externalLitI];
				if externalLit != 0
				{
					let l = self.E2I(externalLit);
					debug_assert!(l != BadULit);
					if (IsNeg(l) && self.IsRooted(l)) || (!IsNeg(l) && self.IsVisited(l))
					{
						self.m_UserAssumps[externalLitI] = 0;
					}
					else if IsNeg(l)
					{
						self.MarkRooted(l);
					}
					else
					{
						self.MarkVisited(l);
					}
				}
			}

			self.CleanRooted();
			self.CleanVisited();

			if self.m_SelfContrOrGloballyUnsatAssumpSolveInv == self.m_Stat.m_SolveInvs
			{
				self.MarkVisited(self.m_SelfContrOrGloballyUnsatAssump);
			}

			if self.m_LatestEarliestFalsifiedAssumpSolveInv == self.m_Stat.m_SolveInvs
			{
				self.MarkDecisionsInConeAsVisited(self.m_LatestEarliestFalsifiedAssump);
			}

			self.m_LatestAssumpUnsatCoreSolveInvocation = self.m_Stat.m_SolveInvs;
		}

		let nonDuplicatedAndVisited = self.m_UserAssumps[assumpInd] != 0 && self.IsVisited(self.E2I(self.m_UserAssumps[assumpInd]));
		let if0DecLevelThenFalsified = self.m_LatestEarliestFalsifiedAssumpSolveInv == self.m_Stat.m_SolveInvs || !self.IsAssigned(self.m_SelfContrOrGloballyUnsatAssump) || self.GetAssignedDecLevel(self.m_SelfContrOrGloballyUnsatAssump) != 0 || self.IsFalsified(self.E2I(self.m_UserAssumps[assumpInd]));
		nonDuplicatedAndVisited && if0DecLevelThenFalsified
	}

	pub(crate) fn AssignAssumptions(&mut self, firstUnassignedAssumpInd: usize)
	{
		self.m_DecLevelOfLastAssignedAssumption = self.m_DecLevel;
		if firstUnassignedAssumpInd != self.m_Assumps.cap()
		{
			let potentiallyUnassignedAssumpsSpan = self.m_Assumps.get_span_cap(firstUnassignedAssumpInd);
			let mut someAssumpsAreUnassigned = true;
			while someAssumpsAreUnassigned
			{
				someAssumpsAreUnassigned = false;
				for assumpLitI in 0..potentiallyUnassignedAssumpsSpan.size()
				{
					let assumpLit = potentiallyUnassignedAssumpsSpan.get(assumpLitI);

					if !self.IsAssigned(assumpLit)
					{
						self.NewDecLevel();

						let isContraditory = self.Assign(assumpLit, Self::BadClsInd, BadULit, self.m_DecLevel, true, false);
						debug_assert!(!isContraditory);
						let _ = isContraditory;

						let mut contradictionInfo = self.BCP();
						self.ConflictAnalysisLoop(&mut contradictionInfo);
						if self.IsUnrecoverable()
						{
							return;
						}

						if self.m_EarliestFalsifiedAssump != BadULit
						{
							if self.IsAssigned(self.m_EarliestFalsifiedAssump) && self.IsAssumpFalsifiedGivenVar(GetVar(self.m_EarliestFalsifiedAssump))
							{
								self.SetStatus(TToporStatus::STATUS_UNSAT, "Falsified assumption discovered after setting and propagating an assumption at decision level ".to_string() + &self.m_DecLevel.to_string());
								return;
							}
							else
							{
								// BCP backtracked
								let newFirstUnassignedAssumpInd = self.FindFirstUnassignedAssumpIndex(firstUnassignedAssumpInd + assumpLitI + 1);
								self.AssignAssumptions(newFirstUnassignedAssumpInd);
								return;
							}
						}

						if !self.IsAssigned(assumpLit)
						{
							someAssumpsAreUnassigned = true;
						}
					}
					else
					{
						debug_assert!(!self.IsFalsified(assumpLit));
					}
				}
			}
			debug_assert!(potentiallyUnassignedAssumpsSpan.iter().all(|l| self.IsAssigned(l)));
		}
	}

	pub(crate) fn FindFirstUnassignedAssumpIndex(&mut self, indexBeyondHandledAssumps: usize) -> usize
	{
		debug_assert!(indexBeyondHandledAssumps <= self.m_Assumps.cap());
		for assumpI in 0..indexBeyondHandledAssumps
		{
			let lAssump = self.m_Assumps[assumpI];
			if !self.IsAssigned(lAssump)
			{
				return assumpI;
			}
		}

		indexBeyondHandledAssumps
	}

	pub(crate) fn HandleAssumptionsIfBacktrackedBeyondThem(&mut self)
	{
		debug_assert!(self.m_DecLevel < self.m_DecLevelOfLastAssignedAssumption);
		debug_assert!(self.m_Assumps.cap() > 0);
		let firstUnassignedAssumpInd = self.FindFirstUnassignedAssumpIndex(self.m_Assumps.cap());
		self.AssignAssumptions(firstUnassignedAssumpInd);
	}

	pub(crate) fn HandleAssumptions(&mut self, userAssumps: &[TLit])
	{
		self.m_UserAssumps.clear();
		self.m_UserAssumps.extend_from_slice(userAssumps);

		debug_assert!(self.NV(1) || self.P(&("Call ".to_string() + &self.m_Stat.m_SolveInvs.to_string() + "; User assumptions: " + &self.SUserLits(userAssumps) + "\n")));

		self.m_DecLevelOfLastAssignedAssumption = 0;

		if userAssumps.is_empty() || userAssumps[0] == 0
		{
			return;
		}

		ReserveExactly!(self, self.m_Assumps, userAssumps.len(), "m_Assumps in HandleAssumptions");
		if self.IsUnrecoverable()
		{
			return;
		}

		{
			let assumpsSpan = self.m_Assumps.get_span_cap(0);

			for (i, &userLit) in userAssumps.iter().enumerate()
			{
				assumpsSpan.set(i, self.E2I(userLit));
			}
		}

		debug_assert!(self.NV(1) || self.P(&("Call ".to_string() + &self.m_Stat.m_SolveInvs.to_string() + "; Intr assumptions: " + &self.SLits(self.m_Assumps.get_span_cap(0), false) + "\n")));

		// Remove the trailing zero (if any)
		if self.m_Assumps[self.m_Assumps.cap() - 1] == BadULit
		{
			self.m_Assumps.reserve_exactly(self.m_Assumps.cap() - 1);
		}

		debug_assert!(self.m_Assumps.cap() > 0 && self.m_Assumps[self.m_Assumps.cap() - 1] != BadULit);

		let mut assumpI: usize = 0;
		while assumpI < self.m_Assumps.cap()
		{
			let lAssump = self.m_Assumps[assumpI];
			debug_assert!(lAssump != BadULit);
			let vAssump = GetVar(lAssump);
			let ai = self.m_AssignmentInfo[vAssump as usize];

			// The RemoveAssump lambda
			macro_rules! RemoveAssump
			{
				() =>
				{
					if self.m_Params.m_ParamAssumpsSimpAllowReorder.get()
					{
						self.m_Assumps[assumpI] = self.m_Assumps[self.m_Assumps.cap() - 1];
						assumpI = assumpI.wrapping_sub(1);
						self.m_Assumps.reserve_exactly(self.m_Assumps.cap() - 1);
					}
					else
					{
						self.m_Assumps[assumpI] = BadULit;
					}
				};
			}

			if self.IsAssigned(lAssump) && self.GetAssignedDecLevel(lAssump) == 0
			{
				if self.IsFalsified(lAssump)
				{
					self.m_SelfContrOrGloballyUnsatAssump = lAssump;
					debug_assert!(self.NV(1) || self.P(&("The external assumption ".to_string() + &self.m_UserAssumps.iter().find(|&&l| self.E2I(l) == lAssump).unwrap().to_string() + " is falsified at level 0\n")));
					self.m_SelfContrOrGloballyUnsatAssumpSolveInv = self.m_Stat.m_SolveInvs;
					self.SetStatus(TToporStatus::STATUS_UNSAT, "An assumption is falsified at decision level 0".to_string());
					return;
				}
				else
				{
					RemoveAssump!();
				}
			}
			else if ai.m_IsAssump()
			{
				if IsNeg(lAssump) != ai.m_IsAssumpNegated()
				{
					self.m_SelfContrOrGloballyUnsatAssump = lAssump;
					self.m_SelfContrOrGloballyUnsatAssumpSolveInv = self.m_Stat.m_SolveInvs;
					self.SetStatus(TToporStatus::STATUS_UNSAT, "Discovered two assumption literals representing the same variable in the two different polarities".to_string());
					return;
				}
				else
				{
					RemoveAssump!();
				}
			}
			else
			{
				self.m_AssignmentInfo[vAssump as usize].set_m_IsAssump(true);
				self.m_AssignmentInfo[vAssump as usize].set_m_IsAssumpNegated(IsNeg(lAssump));
			}

			assumpI = assumpI.wrapping_add(1);
		}

		if !self.m_Params.m_ParamAssumpsSimpAllowReorder.get()
		{
			self.m_Assumps.remove_if_equal_and_cut_capacity(BadULit);
		}

		// Find the first decision level, whose decision variable doesn't appear on the list of assumptions
		let mut btLevel: TUV = 0;
		let mut dl: TUV = 1;
		while dl <= self.m_DecLevel
		{
			let currDecVar = self.GetDecVar(dl);
			debug_assert!(!self.m_AssignmentInfo[currDecVar as usize].m_IsAssignedInBinary() && self.m_VarInfo[currDecVar as usize].m_ParentClsInd == Self::BadClsInd);
			if !self.IsAssumpVar(currDecVar) || self.IsAssumpFalsifiedGivenVar(currDecVar)
			{
				break;
			}
			dl += 1;
			btLevel += 1;
		}

		// Backtrack to the first uncovered decision level
		self.m_Stat.m_AssumpReuseBacktrackLevelsSaved += btLevel as u64;
		self.Backtrack(btLevel as TLit, false, false);

		// Find the first unassigned assumption and check if there is no contradiction
		let mut firstUnassignedAssumpInd = self.m_Assumps.cap();
		for assumpI in 0..self.m_Assumps.cap()
		{
			let lAssump = self.m_Assumps[assumpI];
			if firstUnassignedAssumpInd == self.m_Assumps.cap() && !self.IsAssigned(lAssump)
			{
				firstUnassignedAssumpInd = assumpI;
			}

			let isAssigned = self.IsAssigned(lAssump);
			let isNegated = self.IsAssignedNegated(lAssump);
			if isAssigned && isNegated
			{
				// The C++ version distinguishes the case of a falsified decision here, which never happens: the backtrack level search above stops at exactly such a level
				debug_assert!(!self.IsAssignedDec(lAssump));
				self.m_LatestEarliestFalsifiedAssump = lAssump;
				self.m_LatestEarliestFalsifiedAssumpSolveInv = self.m_Stat.m_SolveInvs;
				self.SetStatus(TToporStatus::STATUS_UNSAT, "Contradiction between assumptions".to_string());
				return;
			}
		}

		self.AssignAssumptions(firstUnassignedAssumpInd);
	}

	pub fn Solve(&mut self, userAssumps: &[TLit], toInSecIsCpuTime: (f64, bool), confThr: u64) -> TToporReturnVal
	{
		if self.m_DumpFile.is_some()
		{
			let dumpFile = self.m_DumpFile.as_mut().unwrap();
			// cout << "\tc ot <TimeOut> <IsCpuTimeOut>" << endl;
			let _ = writeln!(dumpFile, "ot {} {}", DoubleToOstreamStr(toInSecIsCpuTime.0), toInSecIsCpuTime.1 as i32);
			// cout << "\tc oc <ConflictThreshold>" << endl;
			let _ = writeln!(dumpFile, "oc {}", confThr);
			self.DumpSpan(userAssumps, "s ", " 0", true);
		}

		debug_assert!((self.m_Stat.m_SolveInvs == 0) == (self.m_QueryCurr == TQueryType::QUERY_NONE));
		self.m_QueryCurr = if self.m_QueryCurr == TQueryType::QUERY_NONE { TQueryType::QUERY_INIT } else if confThr <= self.m_Params.m_ParamShortQueryConfThrInv.get() as u64 { TQueryType::QUERY_INC_SHORT } else { TQueryType::QUERY_INC_NORMAL };

		let restoreParamsOnExit = self.m_QueryCurr == TQueryType::QUERY_INC_SHORT && !self.m_ShortInvLifetimeParamVals.is_empty() && !self.IsUnrecoverable();
		// Note that the C++ CTopiParams doesn't contain the values of the parameters (which are members of the solver class), hence
		// restoring m_Params on exit restores everything, but the parameter values
		let mut paramsToRestore: Option<TTopiParamsNonValueState> = None;

		let mut trv = TToporReturnVal::RET_EXOTIC_ERROR;

		if restoreParamsOnExit
		{
			debug_assert!(self.m_QueryCurr == TQueryType::QUERY_INC_SHORT && !self.m_ShortInvLifetimeParamVals.is_empty());

			paramsToRestore = Some(self.m_Params.CopyNonValueState());

			for pvI in 0..self.m_ShortInvLifetimeParamVals.len()
			{
				let pv = self.m_ShortInvLifetimeParamVals[pvI].clone();
				self.SetParam(&pv.0, pv.1);
				if self.IsUnrecoverable()
				{
					return self.UnrecStatusToRetVal();
				}
			}
		}

		self.AssumpUnsatCoreCleanUpIfRequired();

		if self.m_Params.m_ParamAddClsAtLevel0.get() && self.m_DecLevel != 0
		{
			self.Backtrack(0, false, false);
		}

		if self.m_Params.m_ParamVerbosity.get() > 0
		{
			if !self.m_AxePrinted
			{
				self.PrintAxe();
				stdout_println!("{}{}{}{}{}{}", print_as_color("c Literal bit-width= ", ansi_color_code::blue), print_as_color((std::mem::size_of::<TLit>() << 3).to_string(), ansi_color_code::blue),
					print_as_color("; Clause-index bit-width= ", ansi_color_code::blue), print_as_color((Ind::DIGITS as usize).to_string(), ansi_color_code::blue),
					print_as_color("; Compress = ", ansi_color_code::blue), print_as_color(if Compress { "1" } else { "0" }, ansi_color_code::blue));
			}
			stdout_print!("{}", self.m_Params.GetAllParamsCurrValues());
		}

		self.m_Stat.NewSolveInvocation(self.m_QueryCurr == TQueryType::QUERY_INC_SHORT);
		if self.IsCbLearntOrDrat() && self.m_Status.get() == TToporStatus::STATUS_CONTRADICTORY
		{
			self.NewLearntClsApplyCbLearntDrat(Span::empty());
		}

		// CApplyFuncOnExitFromScope<> textDratAddCommentOnExit
		let textDratAddCommentOnExit = self.m_OpenedDratFile.is_some() && !self.m_IsDratBinary;

		// The scope-exit functions are applied in the reverse order of their declaration, exactly as in C++: onExit (if already declared) and then textDratAddCommentOnExit
		let res = if self.IsUnrecoverable()
		{
			trv = self.UnrecStatusToRetVal();
			trv
		}
		else
		{
			self.m_IsSolveOngoing = true;

			let res = self.SolveBody(userAssumps, toInSecIsCpuTime, confThr, &mut trv);
			self.SolveOnExit(trv, restoreParamsOnExit, paramsToRestore);
			res
		};

		if textDratAddCommentOnExit
		{
			self.SolveTextDratAddCommentOnExit();
		}

		res
	}

	// CApplyFuncOnExitFromScope<> textDratAddCommentOnExit of Solve
	fn SolveTextDratAddCommentOnExit(&mut self)
	{
		let solveInvs = self.m_Stat.m_SolveInvs;
		// Invoked only if the DRAT file exists
		let isGood = writeln!(self.m_OpenedDratFile.as_mut().unwrap(), "c query completed {}", solveInvs).is_ok();
		if !isGood
		{
			self.OnBadDratFile();
		}
	}

	// CApplyFuncOnExitFromScope<> onExit of Solve
	fn SolveOnExit(&mut self, trv: TToporReturnVal, restoreParamsOnExit: bool, paramsToRestore: Option<TTopiParamsNonValueState>)
	{
		if self.IsCbLearntOrDrat() && (self.m_Status.get() == TToporStatus::STATUS_UNSAT || self.m_Status.get() == TToporStatus::STATUS_CONTRADICTORY)
		{
			self.NewLearntClsApplyCbLearntDrat(Span::empty());
		}

		// Clean-up the assumptions
		if self.m_Assumps.cap() != 0
		{
			for lAssump in self.m_Assumps.get_span_cap(0)
			{
				let vAssump = GetVar(lAssump);
				self.m_AssignmentInfo[vAssump as usize].set_m_IsAssump(false);
			}

			self.m_Assumps.reserve_exactly(0);
		}

		self.m_EarliestFalsifiedAssump = BadULit;

		if self.m_Params.m_ParamVerbosity.get() > 0
		{
			stdout_print!("{}", self.StatStrShort(false));
		}

		if self.m_Stat.m_SolveInvs == self.m_Params.m_ParamPrintDebugModelInvocation.get() as u64
		{
			self.PrintDebugModel(trv);
		}

		if let Some(dumpFile) = &mut self.m_DumpFile
		{
			let _ = writeln!(dumpFile, "c {}", trv);
		}

		self.m_IsSolveOngoing = false;

		if restoreParamsOnExit
		{
			debug_assert!(paramsToRestore.is_some());
			if let Some(paramsToRestore) = paramsToRestore
			{
				self.m_Params.MoveNonValueState(paramsToRestore);
			}
		}

		if self.m_QueryCurr == TQueryType::QUERY_INIT && self.m_Params.m_ParamVarActivityUseMapleLevelBreakerAi.get() != self.m_Params.m_ParamVarActivityUseMapleLevelBreaker.get()
		{
			let v = self.m_Params.m_ParamVarActivityUseMapleLevelBreakerAi.get();
			self.m_Params.m_ParamVarActivityUseMapleLevelBreaker.set(v);
		}

		if self.m_QueryCurr == TQueryType::QUERY_INIT && self.m_Params.m_ParamAddClsRemoveClssGloballySatByLitMinSizeAi.get() != self.m_Params.m_ParamAddClsRemoveClssGloballySatByLitMinSize.get()
		{
			let v = self.m_Params.m_ParamAddClsRemoveClssGloballySatByLitMinSizeAi.get();
			self.m_Params.m_ParamAddClsRemoveClssGloballySatByLitMinSize.set(v);
		}

		if self.m_QueryCurr == TQueryType::QUERY_INIT
		{
			self.m_VsidsHeap.SetInitOrder(self.m_Params.m_ParamVsidsInitOrderAi.get());
		}

		if self.m_QueryCurr == TQueryType::QUERY_INIT && !self.m_AfterInitInvParamVals.is_empty()
		{
			for pvI in 0..self.m_AfterInitInvParamVals.len()
			{
				let pv = self.m_AfterInitInvParamVals[pvI].clone();
				self.SetParam(&pv.0, pv.1);
			}
		}

		self.m_QueryPrev = self.m_QueryCurr;

		if self.m_Params.m_ParamPhaseMngForceSolution.get() && self.m_Status.get() == TToporStatus::STATUS_SAT
		{
			for v in 1..self.GetNextVar()
			{
				self.FixPolarityInternal(self.GetAssignedLitForVar(v), false);
			}
		}
	}

	// The SetStatusLocalTimeout lambda of Solve
	fn SolveSetStatusLocalTimeout(&mut self, toInSecIsCpuTime: (f64, bool))
	{
		self.SetStatus(TToporStatus::STATUS_UNDECIDED, (if toInSecIsCpuTime.1 { "CPU" } else { "Wall" }).to_string() + " timeout of " + &format!("{:.6}", toInSecIsCpuTime.0) + " for the current Solve invocation reached");
	}

	// The SetStatusGlobalTimeout lambda of Solve
	fn SolveSetStatusGlobalTimeout(&mut self, toInSecIsCpuTime: (f64, bool))
	{
		self.SetStatus(TToporStatus::STATUS_GLOBAL_TIMEOUT, "Global ".to_string() + (if toInSecIsCpuTime.1 { "CPU" } else { "Wall" }) + " timeout of " + &format!("{:.6}", toInSecIsCpuTime.0) + " reached");
	}

	// The part of Solve after the declaration of onExit
	// trv is the return value as seen by onExit: precisely as in the C++ version, some of the return statements don't update it
	fn SolveBody(&mut self, userAssumps: &[TLit], toInSecIsCpuTime: (f64, bool), confThr: u64, trv: &mut TToporReturnVal) -> TToporReturnVal
	{
		// return trv = val
		macro_rules! ReturnTrv
		{
			($val:expr) =>
			{
				{
					*trv = $val;
					return *trv;
				}
			};
		}

		if toInSecIsCpuTime.0 < 1e100
		{
			if toInSecIsCpuTime.0 == 0.
			{
				self.SolveSetStatusLocalTimeout(toInSecIsCpuTime);
				ReturnTrv!(TToporReturnVal::RET_TIMEOUT_LOCAL);
			}
			if toInSecIsCpuTime.1
			{
				self.m_Stat.m_TimeSinceLastSolveStart.SetModeCpuTime();
			}
			else
			{
				self.m_Stat.m_TimeSinceLastSolveStart.SetModeWallTime();
			}
			self.m_Stat.m_TimeSinceLastSolveStart.SetTimeout(toInSecIsCpuTime.0);
		}

		if self.m_Stat.m_OverallTime.IsTimeoutSet() && self.m_Stat.m_OverallTime.IsTimeout()
		{
			// Global timeout
			self.SolveSetStatusGlobalTimeout(toInSecIsCpuTime);
		}

		// Make sure to create an internal variable for any new variables amongst the assumptions (may increase m_Stat.m_MaxUserVar)
		for &userLit in userAssumps
		{
			if userLit != 0
			{
				let userVar = ExternalLit2ExternalVar(userLit);
				self.HandleIncomingUserVar(userVar, false);
				if self.IsUnrecoverable()
				{
					ReturnTrv!(self.UnrecStatusToRetVal());
				}
			}
		}

		ReserveExactly!(self, self.m_E2ILitMap, self.m_Stat.m_MaxUserVar as usize + 1, "m_E2IVarMap in Solve");
		self.ReserveVarAndLitData(userAssumps.len() - (userAssumps.last() == Some(&0)) as usize);
		self.ClsDeletionInit();
		self.RestartInit();
		self.DecisionInit();
		self.BacktrackingInit();

		if self.IsUnrecoverable()
		{
			ReturnTrv!(self.UnrecStatusToRetVal());
		}
		debug_assert!(!self.AssertConsistencyOn(2) || self.WLAssertConsistency(false));

		let mut pi = self.BCP();
		if self.IsUnrecoverable()
		{
			ReturnTrv!(self.UnrecStatusToRetVal());
		}

		self.SetStatus(TToporStatus::STATUS_UNDECIDED, String::new());

		if pi.IsContradiction()
		{
			if self.m_DecLevel == 0
			{
				self.SetStatus(TToporStatus::STATUS_CONTRADICTORY, "Global contradiction: discovered by BCP at decision level 0".to_string());
				ReturnTrv!(TToporReturnVal::RET_UNSAT);
			}
			else
			{
				self.ConflictAnalysisLoop(&mut pi);
				if self.IsUnrecoverable()
				{
					ReturnTrv!(self.UnrecStatusToRetVal());
				}

				if self.m_EarliestFalsifiedAssump != BadULit
				{
					self.SetStatus(TToporStatus::STATUS_UNSAT, "Falsified assumption discovered after the initial BCP".to_string());
					ReturnTrv!(TToporReturnVal::RET_UNSAT);
				}
			}
		}

		// Handling the assumptions, if any
		self.HandleAssumptions(userAssumps);
		if self.m_Status.get() != TToporStatus::STATUS_UNDECIDED
		{
			ReturnTrv!(self.StatusToRetVal());
		}

		if self.m_Stat.m_SolveInvs == self.m_Params.m_ParamVerifyDebugModelInvocation.get() as u64
		{
			self.VerifyDebugModel();
		}

		if self.m_AssignedVarsNum == self.m_LastExistingVar
		{
			// All the variables have been assigned -> a model!
			self.SetStatus(TToporStatus::STATUS_SAT, "A model: discovered by the initial BCP before the search".to_string());
			ReturnTrv!(TToporReturnVal::RET_SAT);
		}

		let confThrAfterThisConfNumReached = if confThr == u64::MAX { u64::MAX } else { self.m_Stat.m_Conflicts + confThr };

		if self.m_Params.m_ParamVerbosity.get() > 0
		{
			stdout_print!("{}", self.StatStrShort(false));
		}

		self.m_DecLevelOfLastAssignedAssumption = if self.m_Assumps.cap() == 0
		{
			0
		}
		else
		{
			let assumpsSpan = self.m_Assumps.get_span_cap(0);
			self.GetAssignedDecLevel(assumpsSpan.get(self.GetAssignedLitsHighestDecLevelIt(assumpsSpan, 0)))
		};

		if self.m_Params.m_ParamInitPolarityStrat.get() != 1 && self.m_PrevAiCap < self.m_AssignmentInfo.cap()
		{
			let aiSpan = self.m_AssignmentInfo.get_span_cap(self.m_PrevAiCap);
			for currAi in aiSpan.as_mut_slice()
			{
				if !currAi.m_IsAssigned()
				{
					currAi.set_m_IsNegated(if self.m_Params.m_ParamInitPolarityStrat.get() == 0 { true } else { rand() % 2 != 0 });
				}
			}
			self.m_PrevAiCap = self.m_AssignmentInfo.cap();
		}

		// Rust-only extension (topi_ext_hints.rs): apply the phase and score hints, if any have been recorded (none can be, unless their parameters are on)
		self.ExtHintsApplyPending();
		// Rust-only extension (topi_ext_target_phase.rs): the start of a query for target/best-phase saving and rephasing (nothing, unless their parameters are on)
		self.ExtTargetPhaseInitQuery();

		// CDCL loop
		while self.m_Status.get() == TToporStatus::STATUS_UNDECIDED
		{
			if self.m_InterruptNow.load(Ordering::Relaxed) || (self.M_CbStopNow.is_some() && (self.M_CbStopNow.as_mut().unwrap())() == TStopTopor::VAL_STOP)
			{
				if self.m_InterruptNow.load(Ordering::Relaxed)
				{
					self.SetStatus(TToporStatus::STATUS_USER_INTERRUPT, "Interrupt by the Interrupt callback".to_string());
					self.m_InterruptNow.store(false, Ordering::Relaxed);
				}
				else
				{
					self.SetStatus(TToporStatus::STATUS_USER_INTERRUPT, "Interrupt by the StopNow callback".to_string());
				}
			}
			if self.IsUnrecoverable() || self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT
			{
				ReturnTrv!(self.StatusToRetVal());
			}

			self.InprocessIfRequired();
			// Rust-only extension (topi_ext_vivify.rs): vivification may decide the query (a global contradiction, a falsified assumption or a model) or serve a request to stop it; nothing, unless /inprocessing/vivify/on is set
			if self.m_Params.m_ParamVivifyOn.get() && self.m_Status.get() != TToporStatus::STATUS_UNDECIDED
			{
				ReturnTrv!(self.StatusToRetVal());
			}
			self.SimplifyIfRequired();
			self.DeleteClausesIfRequired();
			self.CompressBuffersIfRequired();

			if self.IsUnrecoverable() || self.m_Status.get() == TToporStatus::STATUS_USER_INTERRUPT
			{
				ReturnTrv!(self.StatusToRetVal());
			}

			self.NewDecLevel();
			self.m_FlippedLit = BadULit;
			let l = self.Decide();
			debug_assert!(l != BadULit);
			debug_assert!(self.NV(2) || self.P(&("***** Decision at level ".to_string() + &self.m_DecLevel.to_string() + ": " + &self.SLit(l) + "\n")));

			debug_assert!(!self.AssertConsistencyOn(1) || self.TrailAssertConsistency());
			let isContraditory = self.Assign(l, Self::BadClsInd, BadULit, self.m_DecLevel, true, false);
			debug_assert!(!isContraditory);
			let _ = isContraditory;
			debug_assert!(!self.AssertConsistencyOn(1) || self.TrailAssertConsistency());

			let mut contradictionInfo = self.BCP();
			let isContradictionBeforeConflictAnalysis = contradictionInfo.IsContradiction();
			// Rust-only extension (topi_ext_target_phase.rs): the assignment below the conflicting level may be the new target (nothing, unless the parameters are on)
			if isContradictionBeforeConflictAnalysis
			{
				self.ExtTargetPhaseOnConflict();
			}

			self.ConflictAnalysisLoop(&mut contradictionInfo);
			if self.IsUnrecoverable()
			{
				ReturnTrv!(self.StatusToRetVal());
			}

			// The earliest falsified assumption might be assigned&satisfied in the corner case of a reimplication in BCP
			if !contradictionInfo.IsContradiction() && (self.m_EarliestFalsifiedAssump == BadULit || !self.IsFalsified(self.m_EarliestFalsifiedAssump)) && self.m_DecLevel < self.m_DecLevelOfLastAssignedAssumption
			{
				debug_assert!(self.NV(2) || self.P(&("The earliest falsified assumption is assigned&satisfied (corner case of a reimplication in BCP). The trail: ".to_string() + &self.STrail() + "\n")));
				self.m_FlippedLit = BadULit;
				self.m_EarliestFalsifiedAssump = BadULit;
				// Trying to find a replacement (otherwise there would be a correctness bug)
				for l in self.m_Assumps.get_span_cap(0)
				{
					let v = GetVar(l);
					debug_assert!(self.m_AssignmentInfo[v as usize].m_IsAssump());
					if self.IsAssigned(l) && self.IsAssumpFalsifiedGivenVar(v) && (self.m_EarliestFalsifiedAssump == BadULit || self.GetAssignedDecLevel(l) <= self.GetAssignedDecLevel(self.m_EarliestFalsifiedAssump))
					{
						self.m_EarliestFalsifiedAssump = l;
						self.m_LatestEarliestFalsifiedAssump = l;
						self.m_LatestEarliestFalsifiedAssumpSolveInv = self.m_Stat.m_SolveInvs;
						debug_assert!(self.NV(2) || self.P(&("Found a new earliest latest earliest falsified assumption: ".to_string() + &self.SLit(l))));
					}
				}

				if self.m_EarliestFalsifiedAssump == BadULit
				{
					self.HandleAssumptionsIfBacktrackedBeyondThem();
					// The C++ version tests for m_Assumps.cap() == 0 here, which never holds: the decision level is below that of the last assigned assumption only if there are assumptions
					debug_assert!(self.m_Assumps.cap() != 0);
					let assumpsSpan = self.m_Assumps.get_span_cap(0);
					self.m_DecLevelOfLastAssignedAssumption = self.GetDecLevel0ForUnassigned(assumpsSpan.get(self.GetLitsHighestDecLevel0ForUnassignedIt(assumpsSpan, 0)));
				}
			}

			if self.IsUnrecoverable()
			{
				ReturnTrv!(self.StatusToRetVal());
			}

			if self.m_EarliestFalsifiedAssump != BadULit
			{
				debug_assert!(self.IsAssigned(self.m_EarliestFalsifiedAssump));
				if !self.IsAssigned(self.m_EarliestFalsifiedAssump)
				{
					self.SetStatus(TToporStatus::STATUS_ASSUMPTION_REQUIRED_ERROR, "Internal error: earliest falsified assumption must be assigned".to_string());
				}
				else
				{
					self.SetStatus(TToporStatus::STATUS_UNSAT, "Assumption flipped!".to_string());
				}
			}
			else if !contradictionInfo.IsContradiction() && self.m_AssignedVarsNum == self.m_LastExistingVar
			{
				// All the variables have been assigned -> a model!
				self.SetStatus(TToporStatus::STATUS_SAT, "A model!".to_string());
			}
			else if self.m_Stat.m_OverallTime.IsTimeoutSet() && self.m_Stat.m_OverallTime.IsTimeout()
			{
				// Global timeout
				self.SolveSetStatusGlobalTimeout(toInSecIsCpuTime);
			}
			else if self.m_Stat.m_Conflicts >= confThrAfterThisConfNumReached
			{
				// Local conflict threshold reached
				self.SetStatus(TToporStatus::STATUS_UNDECIDED, "Conflicts threshold of ".to_string() + &confThr.to_string() + " reached");
				ReturnTrv!(TToporReturnVal::RET_CONFLICT_OUT);
			}
			else if self.m_Stat.m_TimeSinceLastSolveStart.IsTimeoutSet() && self.m_Stat.m_TimeSinceLastSolveStart.IsTimeout()
			{
				self.SolveSetStatusLocalTimeout(toInSecIsCpuTime);
				ReturnTrv!(TToporReturnVal::RET_TIMEOUT_LOCAL);
			}

			if self.m_Status.get() == TToporStatus::STATUS_UNDECIDED && isContradictionBeforeConflictAnalysis && self.Restart()
			{
				self.Backtrack(self.m_DecLevelOfLastAssignedAssumption as TLit, false, false);
				// Rust-only extension (topi_ext_target_phase.rs): rephase, if it's time (never, unless /phase/rephase/conf_init is non-0)
				self.ExtTargetPhaseRephaseIfRequired();
				if self.M_GetNextUnitClause.is_some()
				{
					let assignedVarsNumBefore = self.m_AssignedVarsNum;

					let mut isContradictionWithAssumptions = false;
					let threadId = self.m_ThreadId;
					let mut eLit: TLit = (self.M_GetNextUnitClause.as_mut().unwrap())(threadId, true);
					while eLit != 0
					{
						let l = self.E2I(eLit);
						let isContraditory = self.Assign(l, Self::BadClsInd, BadULit, 0, true, false);
						if isContraditory
						{
							if self.GetAssignedDecLevel(l) == 0
							{
								debug_assert!(self.NV(1) || self.P("Global contradiction when assigning an external unit clause\n"));
								// Note that trv is not updated here in the C++ version
								return TToporReturnVal::RET_UNSAT;
							}
							else
							{
								isContradictionWithAssumptions = true;
								// Precisely as in the C++ version, the internal literal is provided to GetLitDecLevel, which expects an external literal
								self.Backtrack(self.GetLitDecLevel(l as TLit) - 1, false, false);
							}
						}
						eLit = (self.M_GetNextUnitClause.as_mut().unwrap())(threadId, false);
					}

					if isContradictionWithAssumptions
					{
						debug_assert!(self.NV(1) || self.P("Contradiction with assumptions when assigning an external unit clause\n"));
						return TToporReturnVal::RET_UNSAT;
					}

					if self.m_AssignedVarsNum > assignedVarsNumBefore
					{
						let contradictionInfo = self.BCP();
						if contradictionInfo.IsContradiction()
						{
							debug_assert!(self.NV(1) || self.P("Contradiction when propagating external unit clauses\n"));
							return TToporReturnVal::RET_UNSAT;
						}
					}
				}
			}
		}

		ReturnTrv!(self.StatusToRetVal());
	}

	pub fn GetValue(&self, l: TLit) -> TToporLitVal
	{
		// This is in order not to crash when the user asks for a value beyond m_MaxUserVar,
		// which can legally happen (e.g., consider the input {4 0} {4 5 6 7 0})

		if ExternalLit2ExternalVar(l) > self.m_Stat.m_MaxUserVar || !self.IsE2IInBounds(l)
		{
			return TToporLitVal::VAL_DONT_CARE;
		}

		let litInternal = self.E2I(l);
		debug_assert!(litInternal < self.GetNextLit());

		// litInternal can be BadULit if the external variable doesn't appear on any clauses
		if litInternal == BadULit
		{
			TToporLitVal::VAL_DONT_CARE
		}
		else if !self.IsAssigned(litInternal)
		{
			TToporLitVal::VAL_UNASSIGNED
		}
		else if self.IsFalsified(litInternal)
		{
			TToporLitVal::VAL_UNSATISFIED
		}
		else
		{
			TToporLitVal::VAL_SATISFIED
		}
	}

	pub fn GetLitDecLevel(&self, l: TLit) -> TLit
	{
		// The C++ version reads beyond the literal map for an unknown literal; 0 is returned instead
		if !self.IsE2IInBounds(l)
		{
			return 0;
		}

		let litInternal = self.E2I(l);
		debug_assert!(litInternal < self.GetNextLit());

		self.GetAssignedDecLevel(litInternal) as TLit
	}

	pub fn GetSolveInvs(&self) -> u64
	{
		self.m_Stat.m_SolveInvs
	}

	pub fn GetMaxUserVar(&self) -> TLit
	{
		self.m_Stat.m_MaxUserVar
	}

	pub fn GetMaxInternalVar(&self) -> TLit
	{
		self.m_Stat.m_MaxInternalVar as TLit
	}

	pub fn GetActiveLongLearntClss(&self) -> u64
	{
		self.m_Stat.m_ActiveLongLearntClss
	}

	pub fn GetStatStrShort(&mut self, forcePrintingHead: bool) -> String
	{
		self.StatStrShort(forcePrintingHead)
	}

	pub fn GetConflictsNumber(&self) -> u64
	{
		self.m_Stat.m_Conflicts
	}

	pub fn GetActiveClss(&self) -> u64
	{
		self.m_Stat.GetActiveClss()
	}

	pub fn GetBacktracks(&self) -> u64
	{
		self.m_Stat.m_Backtracks
	}

	pub fn GetPropagations(&self) -> u64
	{
		self.m_Stat.m_Implications
	}

	pub fn GetAssumpReuseBacktrackLevelsSaved(&self) -> u64
	{
		self.m_Stat.m_AssumpReuseBacktrackLevelsSaved
	}

	pub fn GetModel(&self) -> Vec<TToporLitVal>
	{
		let mut m = vec![TToporLitVal::VAL_SATISFIED; self.m_E2ILitMap.cap()];
		for v in 1..m.len() as TLit
		{
			m[v as usize] = self.GetValue(v);
		}
		m
	}

	pub fn GetStatistics(&self) -> &TToporStatistics
	{
		&self.m_Stat
	}

	pub fn GetParamsDescr(&self) -> String
	{
		self.m_Params.GetAllParamsDescr()
	}

	pub(crate) fn SetOverallTimeoutIfAny(&mut self)
	{
		if self.m_Params.m_ParamOverallTimeout.get() != f64::MAX
		{
			if self.m_Params.m_ParamOverallTimeoutIsCpu.get()
			{
				self.m_Stat.m_OverallTime.SetModeCpuTime();
			}
			else
			{
				self.m_Stat.m_OverallTime.SetModeWallTime();
			}
			self.m_Stat.m_OverallTime.SetTimeout(self.m_Params.m_ParamOverallTimeout.get());
		}
	}

	pub(crate) fn PrintAxe(&mut self)
	{
		stdout_print!("\
c Intel(R) SAT Solver by Alexander Nadel\n\
c          A\n\
c         /!\\\n\
c        / ! \\\n\
c  /\\    )___(\n\
c ( `.____(_)_________\n\
c |          __..--\"\"\n\
c (      _.-|\n\
c  \\   ,' | |\n\
c   \\ /   | |\n\
c    \\(   | |\n\
c     `   | |\n\
c         | |\n\
c         | |\n\
c         | |\n");
		self.m_AxePrinted = true;
	}

	pub(crate) fn GetDecLevelWithBestScore(&mut self, dlLowestIncl: TUV, dlHighestExcl: TUV) -> TUV
	{
		debug_assert!(self.m_CurrCustomBtStrat > 0);

		if dlHighestExcl <= dlLowestIncl + 1
		{
			return dlHighestExcl - 1;
		}

		let currDlSpan = self.m_BestScorePerDecLevel.get_span_cap_sz(dlLowestIncl as usize, (dlHighestExcl - dlLowestIncl) as usize);

		let mut maxElemI: TUV = 0;
		let mut maxElem: f64 = 0.;
		for i in 0..currDlSpan.size() as TUV
		{
			let currDecLevel = dlLowestIncl + i;

			if self.DecLevelIsCollapsed(currDecLevel)
			{
				continue;
			}
			let currElem = currDlSpan.get(i as usize);
			if currElem > maxElem || (self.m_CurrCustomBtStrat == 1 && currElem == maxElem)
			{
				maxElem = currElem;
				maxElemI = i;
			}
			debug_assert!(self.CalcMaxDecLevelScore(currDecLevel) == self.m_BestScorePerDecLevel[currDecLevel as usize]);
		}

		dlLowestIncl + maxElemI
	}

	pub(crate) fn NewDecLevel(&mut self)
	{
		self.m_DecLevel += 1;
		if self.m_RstNumericLocalConfsSinceRestartAtDecLevelCreation.cap() != 0
		{
			self.m_RstNumericLocalConfsSinceRestartAtDecLevelCreation[self.m_DecLevel as usize] = self.m_ConfsSinceRestart;
		}
	}

	pub(crate) fn SetMultipliers(&mut self)
	{
		self.m_B.SetMultiplier(self.m_Params.m_ParamMultClss.get());
		self.m_AssignmentInfo.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		self.m_VarInfo.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		self.m_E2ILitMap.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		if self.UseI2ELitMap()
		{
			self.m_I2ELitMap.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		}
		self.m_Watches.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		self.m_HandleNewUserCls.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		self.m_VsidsHeap.SetMultiplier(self.m_Params.m_ParamMultVars.get());
		self.m_W.SetMultiplier(self.m_Params.m_ParamMultWatches.get());
	}

	pub(crate) fn MarkWatchBufferChunkDeletedOrByLiteral(&mut self, wlbInd: Ind, allocatedEntries: Ind, l: TULit)
	{
		// Mark the moved region as deleted in the following format:
		// [log_2(allocated-entries) 0] or as marked by a literal if, l != 0
		// The latter mode is relevant during compression
		// countr_zero(allocatedEntries)
		self.m_W[wlbInd.as_usize()] = if allocatedEntries == Ind::ZERO { Ind::DIGITS } else { allocatedEntries.as_u64().trailing_zeros() };
		self.m_W[wlbInd.as_usize() + 1] = l;
	}

	pub(crate) fn MarkWatchBufferChunkDeleted(&mut self, wi: &mut TWatchInfo<Ind>)
	{
		debug_assert!(wi.m_AllocatedEntries.as_u64().is_power_of_two());
		debug_assert!(wi.m_AllocatedEntries >= Ind::from_u32(2));
		self.MarkWatchBufferChunkDeletedOrByLiteral(wi.m_WBInd, wi.m_AllocatedEntries, BadULit);
	}

	pub(crate) fn IsVisitedConsistent(&self) -> bool
	{
		for v in 1..self.GetNextVar()
		{
			let isVisitedFlag = self.m_AssignmentInfo[v as usize].m_Visit();
			let isInVisitedVec = self.m_VisitedVars.get_span().contains(v);
			debug_assert!(isVisitedFlag == isInVisitedVec);
			if isVisitedFlag != isInVisitedVec
			{
				return false;
			}
		}
		true
	}

	pub(crate) fn CalcMaxDecLevelScore(&self, dl: TUV) -> f64
	{
		let mut bestScore: f64 = 0.;
		let mut v = if dl == 0 { self.m_TrailStart } else { self.GetDecVar(dl) };
		while v != BadUVar && self.GetAssignedDecLevelVar(v) == dl
		{
			let currScore = self.m_VsidsHeap.get_var_score(v);
			if currScore > bestScore
			{
				bestScore = currScore;
			}
			v = self.m_VarInfo[v as usize].m_TrailNext;
		}
		bestScore
	}

	pub(crate) fn CalcMinDecLevelScore(&self, dl: TUV) -> f64
	{
		let mut bestScore = f64::MAX;
		let mut v = if dl == 0 { self.m_TrailStart } else { self.GetDecVar(dl) };
		while v != BadUVar && self.GetAssignedDecLevelVar(v) == dl
		{
			let currScore = self.m_VsidsHeap.get_var_score(v);
			if currScore < bestScore
			{
				bestScore = currScore;
			}
			v = self.m_VarInfo[v as usize].m_TrailNext;
		}
		bestScore
	}

	pub(crate) fn GetMemoryLayout(&self) -> String
	{
		if !self.m_Params.m_ParamPrintMemoryProfiling.get()
		{
			return String::new();
		}

		// The C++ version uses an unordered_map, so the order of the entries of the same size is implementation-defined there;
		// here, such entries appear in the order of their insertion
		let mut name2Mb: Vec<(&str, usize)> = Vec::new();

		let mut bcMb = 0;
		let mut bcIt = self.m_BC.begin();
		while bcIt != self.m_BC.end()
		{
			bcMb += self.BCGetBitArrayConst(bcIt as u16).memMb();
			bcIt = self.m_BC.next(bcIt);
		}
		name2Mb.push(("m_BC", bcMb));

		name2Mb.push(("m_E2ILitMap", self.m_E2ILitMap.memMb()));
		name2Mb.push(("m_I2ELitMap", self.m_I2ELitMap.memMb()));
		name2Mb.push(("m_B", self.m_B.memMb()));
		name2Mb.push(("m_W", self.m_W.memMb()));
		name2Mb.push(("m_Watches", self.m_Watches.memMb()));
		name2Mb.push(("m_TrailLastVarPerDecLevel", self.m_TrailLastVarPerDecLevel.memMb()));
		name2Mb.push(("m_BestScorePerDecLevel", self.m_BestScorePerDecLevel.memMb()));
		name2Mb.push(("m_AssignmentInfo", self.m_AssignmentInfo.memMb()));
		name2Mb.push(("m_VarInfo", self.m_VarInfo.memMb()));
		name2Mb.push(("m_PolarityInfo", self.m_PolarityInfo.memMb()));
		name2Mb.push(("m_Assumps", self.m_Assumps.memMb()));
		name2Mb.push(("m_HugeCounterPerDecLevel", self.m_HugeCounterPerDecLevel.memMb()));
		name2Mb.push(("m_DecLevelsLastAppearenceCounter", self.m_DecLevelsLastAppearenceCounter.memMb()));
		name2Mb.push(("m_CurrClsCounters", self.m_CurrClsCounters.memMb()));
		name2Mb.push(("m_RstNumericLocalConfsSinceRestartAtDecLevelCreation", self.m_RstNumericLocalConfsSinceRestartAtDecLevelCreation.memMb()));
		name2Mb.push(("m_HandleNewUserCls", self.m_HandleNewUserCls.memMb()));

		name2Mb.push(("m_NewExternalVarsAddUserCls", self.m_NewExternalVarsAddUserCls.memMb()));
		name2Mb.push(("m_ToPropagate", self.m_ToPropagate.memMb()));
		name2Mb.push(("m_Cis", self.m_Cis.memMb()));
		name2Mb.push(("m_Dis", self.m_Dis.memMb()));
		name2Mb.push(("m_VarsParentSubsumed", memMbVec(&self.m_VarsParentSubsumed)));
		name2Mb.push(("m_HandyLitsClearBefore", self.m_HandyLitsClearBefore[0].memMb() + self.m_HandyLitsClearBefore[1].memMb()));
		name2Mb.push(("m_VisitedVars", self.m_VisitedVars.memMb()));
		name2Mb.push(("m_RootedVars", self.m_RootedVars.memMb()));
		name2Mb.push(("m_UserCls", self.m_UserCls.memMb()));

		name2Mb.push(("m_VsidsHeap", self.m_VsidsHeap.memMb()));
		name2Mb.push(("m_TmpClss", unsafe { (*self.m_TmpClss.get()).iter().fold(0usize, |sum, it| sum + it.memMb()) }));
		name2Mb.push(("m_TmpClssDebug", unsafe { (*self.m_TmpClssDebug.get()).iter().fold(0usize, |sum, it| sum + it.memMb()) }));

		let mut overallSzInMb: usize = 0;
		let mut name2MbVec = name2Mb;
		for ssPair in &name2MbVec
		{
			overallSzInMb += ssPair.1;
		}
		name2MbVec.sort_by_key(|i| std::cmp::Reverse(i.1));

		let mut ss = String::new();
		ss += &format!("c MEMORY -- all : {} *** ", overallSzInMb);
		for nm in &name2MbVec
		{
			ss += &format!("{} : {}; ", nm.0, nm.1);
		}
		ss
	}

	pub fn SetParallelData(&mut self, threadId: u32, ReportUnitClause: Option<TReportUnitCls>, GetNextUnitClause: Option<TGetNextUnitClause>)
	{
		self.m_ThreadId = threadId;
		self.M_ReportUnitCls = ReportUnitClause;
		self.M_GetNextUnitClause = GetNextUnitClause;
	}
}

/*
* Future
*/

// #topor: conflict clause analysis: 1) vivification: see Kissat paper; 2) replace glue with 2glue?
