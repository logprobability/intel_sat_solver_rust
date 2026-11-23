// Copyright(C) 2021 Intel Corporation
// SPDX - License - Identifier: MIT

// The fork ports every member the C++ tool defines, including the ones it never calls
#![allow(dead_code)]

// The Intel(R) SAT Solver executable (topor_tool)
// This is the port of Main.cc in the SKIP_ZLIB configuration (which is the one built by the Makefile):
// plain text files are read directly, whereas archive files (including .gz) are read through a pipe from the corresponding decompression command

// The fork retains the C++ identifiers and layout conventions
#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types, clippy::upper_case_acronyms)]
// The control flow is translated statement by statement (see PORTING.md), so the lints, whose fix would restructure a C++ statement, are off
#![allow(clippy::collapsible_if, clippy::collapsible_else_if, clippy::if_same_then_else, clippy::needless_range_loop, clippy::explicit_counter_loop, clippy::too_many_arguments)]

use intel_sat_solver::color_print::{ansi_color_code, print_as_color};
use intel_sat_solver::std_string::{stod, stoi, stoll, stoul, stoull};
use intel_sat_solver::topor::*;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};

// The supported archive file types
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum TArchiveFileType
{
	XZ = 0,
	LZMA = 1,
	BZ = 2,
	GZ = 3,
	SevenZ = 4,
	None = 5,
}

const EOF: i32 = -1;

// The supported archive file types' file signatures
const MaxSigLength: usize = 7;
const fileSig: [[i32; MaxSigLength + 1]; TArchiveFileType::None as usize] = [
	[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00, EOF],
	[0x5D, 0x00, 0x00, 0x80, 0x00, EOF, EOF, EOF],
	[0x42, 0x5A, 0x68, EOF, EOF, EOF, EOF, EOF],
	[0x1F, 0x8B, EOF, EOF, EOF, EOF, EOF, EOF],
	[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, EOF, EOF],
];

const fn AllSigsDifferByFirstInt() -> bool
{
	let mut allDiffer = true;
	let mut i = 0;
	while i < fileSig.len()
	{
		let mut j = i + 1;
		while j < fileSig.len()
		{
			allDiffer = allDiffer && fileSig[i][0] != fileSig[j][0];
			j += 1;
		}
		i += 1;
	}
	allDiffer
}

// We count all signatures' first integer being different in the code, which determines the file's type
const _: () = assert!(AllSigsDifferByFirstInt());

// Strings before and after the command for each archive type
// The C++ version runs "<program> <args> <file> <suffix>" through popen; here the program is spawned directly with the file as a single argument,
// so that the file name is never interpreted by a shell. The Boolean stands for the "2>/dev/null" suffix
const archiveCommands: [(&str, &[&str], bool); TArchiveFileType::None as usize] = [("xz", &["-c", "-d"], false), ("lzma", &["-c", "-d"], false), ("bzip2", &["-c", "-d"], false), ("gzip", &["-c", "-d"], false), ("7z", &["x", "-so"], true)];

const BadRetVal: i32 = -1;

// cout << ...
macro_rules! cout
{
	($out:expr, $($arg:tt)*) =>
	{
		{
			let _ = write!($out, $($arg)*);
		}
	};
}

// cout << ... << endl
macro_rules! coutln
{
	($out:expr, $($arg:tt)*) =>
	{
		{
			let _ = writeln!($out, $($arg)*);
			let _ = $out.flush();
		}
	};
}

// The interface to the three solver instantiations: CTopor<int32_t, uint32_t, false>, CTopor<int32_t, uint64_t, false> and CTopor<int32_t, uint64_t, true>
// It replaces the Topor* lambdas of the C++ version, which dispatch to the one existing instance out of topor32, topor64 and toporc
trait TToporInst
{
	fn ToporSetParam(&mut self, paramName: &str, newVal: f64);
	fn ToporIsError(&self) -> bool;
	fn ToporGetStatusExplanation(&self) -> String;
	fn ToporDumpDrat(&mut self, openedDratFile: Box<dyn Write>, isDratBinary: bool, dratSortEveryClause: bool);
	fn ToporGetLitValue(&self, l: TLit) -> TToporLitVal;
	fn ToporClearUserPolarityInfo(&mut self, v: TLit);
	fn ToporFixPolarity(&mut self, l: TLit, onlyOnce: bool);
	fn ToporCreateInternalLit(&mut self, v: TLit);
	fn ToporBoostScore(&mut self, v: TLit, value: f64);
	fn ToporBacktrack(&mut self, decLevel: TLit);
	fn ToporChangeConfigToGiven(&mut self, configNum: u16) -> String;
	fn ToporGetLitDecLevel(&self, l: TLit) -> TLit;
	fn ToporSolve(&mut self, assumps: &[TLit], toInSecIsCpuTime: (f64, bool), confThr: u64) -> TToporReturnVal;
	fn ToporAddClause(&mut self, c: &[TLit]);
	fn ToporGetSolveInvs(&self) -> u64;
	fn ToporIsAssumptionRequired(&mut self, assumpInd: usize) -> bool;
	fn ToporGetMaxUserVar(&self) -> TLit;
}

impl<Ind: TUInd, const Compress: bool> TToporInst for CTopor<Ind, Compress>
{
	fn ToporSetParam(&mut self, paramName: &str, newVal: f64)
	{
		self.SetParam(paramName, newVal)
	}
	fn ToporIsError(&self) -> bool
	{
		self.IsError()
	}
	fn ToporGetStatusExplanation(&self) -> String
	{
		self.GetStatusExplanation()
	}
	fn ToporDumpDrat(&mut self, openedDratFile: Box<dyn Write>, isDratBinary: bool, dratSortEveryClause: bool)
	{
		self.DumpDrat(openedDratFile, isDratBinary, dratSortEveryClause)
	}
	fn ToporGetLitValue(&self, l: TLit) -> TToporLitVal
	{
		self.GetLitValue(l)
	}
	fn ToporClearUserPolarityInfo(&mut self, v: TLit)
	{
		self.ClearUserPolarityInfo(v)
	}
	fn ToporFixPolarity(&mut self, l: TLit, onlyOnce: bool)
	{
		self.FixPolarity(l, onlyOnce)
	}
	fn ToporCreateInternalLit(&mut self, v: TLit)
	{
		self.CreateInternalLit(v)
	}
	fn ToporBoostScore(&mut self, v: TLit, value: f64)
	{
		self.BoostScore(v, value)
	}
	fn ToporBacktrack(&mut self, decLevel: TLit)
	{
		self.Backtrack(decLevel)
	}
	fn ToporChangeConfigToGiven(&mut self, configNum: u16) -> String
	{
		self.ChangeConfigToGiven(configNum)
	}
	fn ToporGetLitDecLevel(&self, l: TLit) -> TLit
	{
		self.GetLitDecLevel(l)
	}
	fn ToporSolve(&mut self, assumps: &[TLit], toInSecIsCpuTime: (f64, bool), confThr: u64) -> TToporReturnVal
	{
		self.Solve(assumps, toInSecIsCpuTime, confThr)
	}
	fn ToporAddClause(&mut self, c: &[TLit])
	{
		self.AddClause(c)
	}
	fn ToporGetSolveInvs(&self) -> u64
	{
		self.GetSolveInvs()
	}
	fn ToporIsAssumptionRequired(&mut self, assumpInd: usize) -> bool
	{
		self.IsAssumptionRequired(assumpInd)
	}
	fn ToporGetMaxUserVar(&self) -> TLit
	{
		self.GetMaxUserVar()
	}
}

fn OnFinishingSolving(out: &mut dyn Write, topor: &mut dyn TToporInst, ret: TToporReturnVal, printModel: bool, printUcore: bool, assumps: &[TLit], varsToPrint: Option<&Vec<TLit>>) -> i32
{
	let res = OnFinishingSolvingInternal(out, topor, ret, printModel, printUcore, assumps, varsToPrint);

	// printStatusExplanation on exit from scope
	let expl = topor.ToporGetStatusExplanation();
	if !expl.is_empty()
	{
		coutln!(out, "c {}", expl);
	}

	res
}

fn OnFinishingSolvingInternal(out: &mut dyn Write, topor: &mut dyn TToporInst, ret: TToporReturnVal, printModel: bool, printUcore: bool, assumps: &[TLit], varsToPrint: Option<&Vec<TLit>>) -> i32
{
	match ret
	{
		TToporReturnVal::RET_SAT =>
		{
			coutln!(out, "s SATISFIABLE");
			if printModel
			{
				let PrintVal = |out: &mut dyn Write, v: TLit|
				{
					let vVal = topor.ToporGetLitValue(v);
					debug_assert!(vVal != TToporLitVal::VAL_UNASSIGNED);
					cout!(out, " {}", if vVal != TToporLitVal::VAL_UNSATISFIED { v } else { -v });
				};

				cout!(out, "v");
				match varsToPrint
				{
					None =>
					{
						let mut v: TLit = 1;
						while v <= topor.ToporGetMaxUserVar()
						{
							PrintVal(out, v);
							v += 1;
						}
					}
					Some(varsToPrint) =>
					{
						for v in varsToPrint
						{
							PrintVal(out, *v);
						}
					}
				}

				coutln!(out, " 0");
			}
			10
		}
		TToporReturnVal::RET_UNSAT =>
		{
			coutln!(out, "s UNSATISFIABLE");
			if printUcore
			{
				cout!(out, "v");
				for assumpInd in 0..assumps.len()
				{
					let currAssump = assumps[assumpInd];
					if topor.ToporIsAssumptionRequired(assumpInd)
					{
						cout!(out, " {}", currAssump);
					}
				}
				coutln!(out, " 0");
			}
			20
		}
		TToporReturnVal::RET_TIMEOUT_LOCAL =>
		{
			coutln!(out, "s TIMEOUT_LOCAL");
			BadRetVal
		}
		TToporReturnVal::RET_CONFLICT_OUT =>
		{
			coutln!(out, "s CONFLICT_OUT");
			30
		}
		TToporReturnVal::RET_MEM_OUT =>
		{
			coutln!(out, "s MEMORY_OUT");
			BadRetVal
		}
		TToporReturnVal::RET_USER_INTERRUPT =>
		{
			coutln!(out, "s USER_INTERRUPT");
			BadRetVal
		}
		TToporReturnVal::RET_INDEX_TOO_NARROW =>
		{
			coutln!(out, "s INDEX_TOO_NARROW");
			BadRetVal
		}
		TToporReturnVal::RET_PARAM_ERROR =>
		{
			coutln!(out, "s PARAM_ERROR");
			BadRetVal
		}
		TToporReturnVal::RET_TIMEOUT_GLOBAL =>
		{
			coutln!(out, "s TIMEOUT_GLOBAL");
			BadRetVal
		}
		TToporReturnVal::RET_DRAT_FILE_PROBLEM =>
		{
			coutln!(out, "s DRAT_FILE_PROBLEM");
			BadRetVal
		}
		// The C++ version handles RET_ASSUMPTION_REQUIRED_ERROR in the default case
		TToporReturnVal::RET_EXOTIC_ERROR =>
		{
			coutln!(out, "s EXOTIC_ERROR");
			BadRetVal
		}
		TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR =>
		{
			coutln!(out, "s UNEXPECTED_ERROR");
			BadRetVal
		}
	}
}

// An ofstream, which failed to open: every write fails
struct CFailedOfstream;

impl Write for CFailedOfstream
{
	fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize>
	{
		Err(std::io::Error::other("the DRAT file couldn't be opened"))
	}

	fn flush(&mut self) -> std::io::Result<()>
	{
		Err(std::io::Error::other("the DRAT file couldn't be opened"))
	}
}

// A minimal emulation of the std::stringstream, used by ReadCommaSeparatedVarList in the C++ version
// The emulation is required, since the C++ code (unintentionally) writes its error message into the very stream it reads the variables from
struct CStringStream
{
	m_Buf: Vec<u8>,
	m_GetPos: usize,
	m_PutPos: usize,
	m_IsGood: bool,
}

impl CStringStream
{
	fn new(s: &[u8]) -> Self
	{
		CStringStream { m_Buf: s.to_vec(), m_GetPos: 0, m_PutPos: 0, m_IsGood: true }
	}

	fn good(&self) -> bool
	{
		self.m_IsGood
	}

	// getline(ss, substr, delim)
	fn getline(&mut self, delim: u8) -> Vec<u8>
	{
		let mut substr = Vec::new();
		if !self.m_IsGood
		{
			return substr;
		}
		loop
		{
			if self.m_GetPos >= self.m_Buf.len()
			{
				// eofbit (and also failbit, if nothing was extracted)
				self.m_IsGood = false;
				break;
			}
			let c = self.m_Buf[self.m_GetPos];
			self.m_GetPos += 1;
			if c == delim
			{
				break;
			}
			substr.push(c);
		}
		substr
	}

	// ss << str: overwrites the buffer starting from the put position; fails, if the stream isn't good
	fn put(&mut self, str: &[u8])
	{
		if !self.m_IsGood
		{
			return;
		}
		for c in str
		{
			if self.m_PutPos < self.m_Buf.len()
			{
				self.m_Buf[self.m_PutPos] = *c;
			}
			else
			{
				self.m_Buf.push(*c);
			}
			self.m_PutPos += 1;
		}
	}
}

// Returns the variable list and the error message (empty, if there is no error)
// The C++ version returns the list and the error message (empty if none) in an output parameter
fn ReadCommaSeparatedVarList(s: &[u8]) -> Result<Vec<TLit>, String>
{
	let mut varList: Vec<TLit> = Vec::new();

	// try
	{
		// create string stream from the string
		let mut ss = CStringStream::new(s);
		while ss.good()
		{
			// Get first string delimited by comma
			let substr = ss.getline(b',');
			match stoll(&String::from_utf8_lossy(&substr))
			{
				Some(ll) =>
				{
					let l = ll as TLit;
					if l <= 0
					{
						// Precisely as in the C++ version, the message goes into the inner stream (the one the variables are read from), rather than into the error message
						ss.put(format!("c ERROR: couldn't convert {} to a variable list, since {} is not a positive variable\n", String::from_utf8_lossy(s), l).as_bytes());
					}
					varList.push(l);
				}
				None =>
				{
					// catch (...)
					return Err(format!("c ERROR: couldn't convert {} to a variable list\n", String::from_utf8_lossy(s)));
				}
			}
		}
	}

	Ok(varList)
}

// The line, which is currently being parsed (a 0-terminated C string in the C++ version)
struct CLine
{
	line: Vec<u8>,
	len: usize,
	currLineI: usize,
}

impl CLine
{
	fn new(mut line: Vec<u8>) -> Self
	{
		// strlen(line)
		if let Some(nulInd) = line.iter().position(|c| *c == 0)
		{
			line.truncate(nulInd);
		}
		let len = line.len();
		CLine { line, len, currLineI: 0 }
	}

	// line[i], where the terminating 0 is returned for any index beyond the line
	fn at(&self, i: usize) -> u8
	{
		if i < self.len { self.line[i] } else { 0 }
	}

	fn curr(&self) -> u8
	{
		self.at(self.currLineI)
	}

	// lStr.substr(pos); None stands for std::out_of_range
	fn substr(&self, pos: usize) -> Option<String>
	{
		if pos > self.len { None } else { Some(String::from_utf8_lossy(&self.line[pos..]).into_owned()) }
	}

	// lStr.substr(pos, n); None stands for std::out_of_range
	fn substrn(&self, pos: usize, n: usize) -> Option<String>
	{
		if pos > self.len { None } else { Some(String::from_utf8_lossy(&self.line[pos..std::cmp::min(self.len, pos + n)]).into_owned()) }
	}

	// lStr.find(c, pos)
	fn find(&self, c: u8, pos: usize) -> Option<usize>
	{
		if pos >= self.len { None } else { self.line[pos..].iter().position(|x| *x == c).map(|i| i + pos) }
	}

	fn SkipWhitespaces(&mut self)
	{
		while self.curr() == b' ' && self.currLineI < self.len
		{
			self.currLineI += 1;
		}
	}

	// Err stands for the logic_error thrown by the C++ version
	fn ParseNumber(&mut self, lineNum: u64) -> Result<i64, String>
	{
		self.SkipWhitespaces();
		if self.currLineI >= self.len
		{
			return Err("c topor_tool ERROR: no number after skipping white-spaces at line number ".to_string() + &lineNum.to_string());
		}
		let isNeg = self.curr() == b'-';
		if isNeg
		{
			self.currLineI += 1;
		}
		if !self.curr().is_ascii_digit()
		{
			return Err("c topor_tool ERROR: the first character is expected to be a digit at line number ".to_string() + &lineNum.to_string());
		}

		let mut res: i64 = 0;

		while self.curr().is_ascii_digit()
		{
			let currDigit = self.curr() - b'0';
			self.currLineI += 1;
			res = res.wrapping_mul(10).wrapping_add(currDigit as i64);
		}

		if isNeg
		{
			res = res.wrapping_neg();
		}

		Ok(res)
	}

	// Returns the error string (empty, if there is no error) and the literals
	fn BufferToLits(&mut self, lineNum: u64) -> (String, Vec<TLit>)
	{
		let mut errorString = String::new();

		let mut lits: Vec<TLit> = Vec::new();

		let mut currLit = i64::MAX;
		while currLit != 0
		{
			match self.ParseNumber(lineNum)
			{
				Ok(l) =>
				{
					currLit = l;
					if currLit > TLit::MAX as i64 || currLit < TLit::MIN as i64
					{
						errorString = "c topor_tool ERROR: the literal ".to_string() + &currLit.to_string() + " is too big or too small\n";
						lits.clear();
						break;
					}
					lits.push(currLit as TLit);
				}
				Err(_) =>
				{
					errorString = "c topor_tool ERROR: couldn't translate the following line or parts of it into a vector of literals at line number ".to_string() + &lineNum.to_string() + "\n";
					lits.clear();
					break;
				}
			}
		}

		(errorString, lits)
	}
}

// The state of the tool: the local variables of main in the C++ version, where the lambdas of main become the member functions
struct CToporTool<'a>
{
	argv: &'a [String],
	out: &'a mut dyn Write,

	isDratBinary: bool,
	dratName: String,
	dratSortEveryClause: bool,
	printModel: bool,
	printUcore: bool,
	verifyModel: bool,
	verifyUcore: bool,
	ignoreFileParams: bool,
	allsatModels: u64,
	blockingVars: Vec<TLit>,
	allsatBlockingFromInstanceAlg: u64,
	// 0: 32-bit clause buffer index; 1: 64-bit clause buffer index; 2: 64-bit clause buffer index & bit-array-compression
	type_indexing_and_compression: u8,

	// The one existing instance out of topor32, topor64 and toporc (if any)
	topor: Option<Box<dyn TToporInst>>,

	ret: TToporReturnVal,

	lineNum: u64,
	pLineRead: bool,
	retValBasedOnLatestSolve: i32,

	// Populated if verify_model is on
	vmClss: Vec<Vec<TLit>>,

	nextSolveToInSecIsCpuTime: (f64, bool),
	nextSolveConfThr: u64,
	varsInPCnf: TLit,
}

impl<'a> CToporTool<'a>
{
	fn new(argv: &'a [String], out: &'a mut dyn Write) -> Self
	{
		CToporTool
		{
			argv,
			out,
			isDratBinary: true,
			dratName: String::new(),
			dratSortEveryClause: false,
			printModel: true,
			printUcore: false,
			verifyModel: false,
			verifyUcore: false,
			ignoreFileParams: false,
			allsatModels: 0,
			blockingVars: Vec::new(),
			allsatBlockingFromInstanceAlg: 3,
			type_indexing_and_compression: 0,
			topor: None,
			ret: TToporReturnVal::RET_EXOTIC_ERROR,
			lineNum: 1,
			pLineRead: false,
			retValBasedOnLatestSolve: BadRetVal,
			vmClss: Vec::new(),
			nextSolveToInSecIsCpuTime: (f64::MAX, false),
			nextSolveConfThr: u64::MAX,
			varsInPCnf: 0,
		}
	}

	fn AllToporsNull(&self) -> bool
	{
		self.topor.is_none()
	}

	// The existing solver instance; the C++ version asserts !AllToporsNull() and dereferences a null pointer otherwise
	fn Topor(&mut self) -> &mut dyn TToporInst
	{
		self.topor.as_mut().expect("topor_tool: the solver instance doesn't exist yet (a solver command appears before the p-line or the first clause?)").as_mut()
	}

	fn CreateToporsIfRequired(&mut self, varsNumHint: TLit)
	{
		if self.AllToporsNull()
		{
			if self.type_indexing_and_compression == 2
			{
				self.topor = Some(Box::new(CTopor::<u64, true>::new(varsNumHint)));
			}
			else if self.type_indexing_and_compression == 1
			{
				self.topor = Some(Box::new(CTopor::<u64, false>::new(varsNumHint)));
			}
			else
			{
				self.topor = Some(Box::new(CTopor::<u32, false>::new(varsNumHint)));
			}
		}
	}

	// The C++ versions return the value and the error message (empty if none) in an output parameter
	fn ReadBoolParam(paramValStr: &str) -> Result<bool, String>
	{
		match stoi(paramValStr)
		{
			None => Err(format!("c ERROR: couldn't convert {} to an integer\n", paramValStr)),
			Some(intVal @ (0 | 1)) => Ok(intVal != 0),
			Some(_) => Err(format!("c ERROR: {} must be 0 or 1\n", paramValStr)),
		}
	}

	fn Read0to2Param(paramValStr: &str) -> Result<u8, String>
	{
		match stoi(paramValStr)
		{
			None => Err(format!("c ERROR: couldn't convert {} to an integer\n", paramValStr)),
			Some(intVal @ 0..=2) => Ok(intVal as u8),
			Some(_) => Err(format!("c ERROR: {} must be 0 or 1 or 2\n", paramValStr)),
		}
	}

	fn ReadULongParam(paramValStr: &str) -> Result<u64, String>
	{
		stoul(paramValStr).ok_or_else(|| format!("c ERROR: couldn't convert {} to an unsigned long\n", paramValStr))
	}

	// Returns true iff there is an error
	fn ParseParameters(&mut self, varsNumHint: TLit) -> bool
	{
		// Parse parameters
		let argv = self.argv;
		let mut currArgNum = 2;
		while currArgNum < argv.len()
		{
			let paramNameStr = argv[currArgNum].as_str();
			let paramValStr = argv[currArgNum + 1].as_str();

			// /topor_tool/ prefix length
			const ttPrefixLen: usize = 12;
			if paramNameStr.as_bytes().get(..std::cmp::min(ttPrefixLen, paramNameStr.len())) == Some(b"/topor_tool/")
			{
				let param = &paramNameStr[ttPrefixLen..];

				// Echoes the parameter, then stores the value read into the field, or prints the error and fails
				macro_rules! ReadParam
				{
					($field:expr, $read:expr) =>
					{{
						coutln!(self.out, "c /topor_tool/{} {}", param, paramValStr);
						match $read
						{
							Ok(val) => $field = val,
							Err(errMsg) =>
							{
								cout!(self.out, "{}", errMsg);
								return true;
							}
						}
					}};
				}

				match param
				{
					"bin_drat_file" | "text_drat_file" =>
					{
						self.dratName = paramValStr.to_string();
						self.isDratBinary = param == "bin_drat_file";
						coutln!(self.out, "c /topor_tool/{} {}", param, self.dratName);
					}
					"drat_sort_every_clause" => ReadParam!(self.dratSortEveryClause, Self::ReadBoolParam(paramValStr)),
					"print_model" => ReadParam!(self.printModel, Self::ReadBoolParam(paramValStr)),
					"print_ucore" => ReadParam!(self.printUcore, Self::ReadBoolParam(paramValStr)),
					"verify_model" => ReadParam!(self.verifyModel, Self::ReadBoolParam(paramValStr)),
					"verify_ucore" => ReadParam!(self.verifyUcore, Self::ReadBoolParam(paramValStr)),
					"ignore_file_params" => ReadParam!(self.ignoreFileParams, Self::ReadBoolParam(paramValStr)),
					"solver_mode" =>
					{
						ReadParam!(self.type_indexing_and_compression, Self::Read0to2Param(paramValStr));
						if !self.AllToporsNull()
						{
							coutln!(self.out, "c topor_tool ERROR: /topor_tool/solver_mode should be provided before any other parameters");
							return true;
						}
					}
					"allsat_models_number" => ReadParam!(self.allsatModels, Self::ReadULongParam(paramValStr)),
					"allsat_blocking_variables" => ReadParam!(self.blockingVars, ReadCommaSeparatedVarList(paramValStr.as_bytes())),
					"allsat_blocking_variables_file_alg" => ReadParam!(self.allsatBlockingFromInstanceAlg, Self::ReadULongParam(paramValStr)),
					_ =>
					{
						coutln!(self.out, "c ERROR: unrecognized /topor_tool/ parameter: {}", paramNameStr);
						return true;
					}
				}
			}
			else
			{
				// The C++ version tests whether the instance is still null here to report "couldn't create Topor instance" (which cannot happen, since new throws on failure)
				self.CreateToporsIfRequired(varsNumHint);

				let paramName = paramNameStr;
				let paramValue = match stod(paramValStr)
				{
					Some(v) => v,
					None =>
					{
						coutln!(self.out, "c topor_tool ERROR: could not convert {} to double", argv[currArgNum + 1]);
						return true;
					}
				};

				self.Topor().ToporSetParam(paramName, paramValue);
				let isError = self.Topor().ToporIsError();
				if isError
				{
					let errorDescr = self.Topor().ToporGetStatusExplanation();
					coutln!(self.out, "c ERROR in Topor parameter: {}", errorDescr);
					return true;
				}
			}
			currArgNum += 2;
		}

		self.CreateToporsIfRequired(varsNumHint);

		false
	}

	// The C++ default is varsNumHint = 0
	fn CreateToporInst(&mut self, varsNumHint: TLit) -> i32
	{
		self.pLineRead = true;

		if self.ParseParameters(varsNumHint)
		{
			return BadRetVal;
		}

		if !self.dratName.is_empty()
		{
			// The C++ version tests dratFile.bad() after opening the file, which doesn't hold even if the file couldn't be opened (failbit is set then, rather than badbit)
			// Hence, the solver is provided the stream anyway, and discovers the problem when it tries to write to it
			let dratFile: Box<dyn Write> = match std::fs::File::create(&self.dratName)
			{
				Ok(f) => Box::new(std::io::BufWriter::new(f)),
				Err(_) => Box::new(CFailedOfstream),
			};
			let (isDratBinary, dratSortEveryClause) = (self.isDratBinary, self.dratSortEveryClause);
			self.Topor().ToporDumpDrat(dratFile, isDratBinary, dratSortEveryClause);
		}

		0
	}

	// Returns 10 upon success and BadRetVal upon failure
	fn VerifyModel(&mut self, assumps: Option<&Vec<TLit>>) -> i32
	{
		// Verify the model
		coutln!(self.out, "c topor_tool: before verifying that the model satisfies {}", if assumps.is_none() { "the clauses" } else { "the assumptions and the clauses" });
		if let Some(assumps) = assumps
		{
			// Verify the assumptions first
			for a in assumps
			{
				if *a != 0
				{
					let v = self.Topor().ToporGetLitValue(*a);
					if v != TToporLitVal::VAL_SATISFIED && v != TToporLitVal::VAL_DONT_CARE
					{
						coutln!(self.out, "c ERROR: assumptions {} is not satisfied!", a);
						return BadRetVal;
					}
				}
			}
			coutln!(self.out, "c topor_tool: assumptions verified!");
		}
		for clsInd in 0..self.vmClss.len()
		{
			let mut isVerified = false;
			for lInd in 0..self.vmClss[clsInd].len()
			{
				let l = self.vmClss[clsInd][lInd];
				if l != 0
				{
					let v = self.Topor().ToporGetLitValue(l);
					if v == TToporLitVal::VAL_SATISFIED || v == TToporLitVal::VAL_DONT_CARE
					{
						isVerified = true;
						break;
					}
				}
			}
			if !isVerified
			{
				cout!(self.out, "c ERROR: the following clause is not satisfied:");
				for l in &self.vmClss[clsInd]
				{
					cout!(self.out, " {}", l);
				}
				coutln!(self.out, "");
				return BadRetVal;
			}
		}
		coutln!(self.out, "c topor_tool: clauses verified!");
		10
	}

	fn ToporOnFinishedSolving(&mut self, assumps: &[TLit], varsToPrint: Option<&Vec<TLit>>) -> i32
	{
		let (ret, printModel, printUcore) = (self.ret, self.printModel, self.printUcore);
		match self.topor.as_mut()
		{
			None => BadRetVal,
			Some(topor) => OnFinishingSolving(&mut *self.out, topor.as_mut(), ret, printModel, printUcore, assumps, varsToPrint),
		}
	}

	fn Solve(&mut self, assumpsPtr: Option<&Vec<TLit>>) -> i32
	{
		let assumpsEmpty: Vec<TLit> = Vec::new();
		let assumps = assumpsPtr.unwrap_or(&assumpsEmpty);
		let (nextSolveToInSecIsCpuTime, nextSolveConfThr) = (self.nextSolveToInSecIsCpuTime, self.nextSolveConfThr);
		self.ret = self.Topor().ToporSolve(assumps, nextSolveToInSecIsCpuTime, nextSolveConfThr);
		self.nextSolveToInSecIsCpuTime = (f64::MAX, false);
		self.nextSolveConfThr = u64::MAX;

		self.retValBasedOnLatestSolve = self.ToporOnFinishedSolving(assumps, None);

		if self.verifyModel && self.retValBasedOnLatestSolve == 10
		{
			if self.VerifyModel(assumpsPtr) == BadRetVal
			{
				return BadRetVal;
			}
		}

		if self.verifyUcore && self.retValBasedOnLatestSolve == 20
		{
			// The C++ version dereferences assumpsPtr here even if it's null (that is, for the final Solve of a non-incremental instance); we treat it as an empty assumption list
			let mut ucAssumps: Vec<TLit> = Vec::new();
			let mut i = 0;
			while i < assumps.len() && assumps[i] != 0
			{
				let isAssumptionRequired = self.Topor().ToporIsAssumptionRequired(i);
				coutln!(self.out, "Assumption #{} -- {} : {}", i, assumps[i], isAssumptionRequired as u8);
				if self.Topor().ToporIsAssumptionRequired(i)
				{
					ucAssumps.push(assumps[i]);
				}
				i += 1;
			}
			let (nextSolveToInSecIsCpuTime, nextSolveConfThr) = (self.nextSolveToInSecIsCpuTime, self.nextSolveConfThr);
			self.ret = self.Topor().ToporSolve(&ucAssumps, nextSolveToInSecIsCpuTime, nextSolveConfThr);
			self.retValBasedOnLatestSolve = self.ToporOnFinishedSolving(assumps, None);
			if self.retValBasedOnLatestSolve != 20
			{
				cout!(self.out, "ret == {}: UNSAT CORE BUG!!!!!\n", self.retValBasedOnLatestSolve);
				return BadRetVal;
			}
		}
		self.retValBasedOnLatestSolve
	}

	// One iteration of the file reading loop
	// Inside the loop, we expect to read either:
	// c...: a comment
	// p cnf vars clss: the header (must appear before clauses; handled as recommendation only)
	// Lit1 Lit2 ... LitN 0: 0-ended clause
	// "s Lit1 Lit2 ... LitN": solve under the given assumption
	// Returns Ok(Some(retVal)), if main should return retVal; Ok(None) to continue to the next line; Err(what) stands for a logic_error thrown by the C++ version (which is never caught)
	fn HandleLine(&mut self, l: &mut CLine) -> Result<Option<i32>, String>
	{
		let lineNum = self.lineNum;

		l.SkipWhitespaces();
		if l.currLineI >= l.len
		{
			// Empty line
			return Ok(None);
		}

		if l.curr() == b'c'
		{
			// A comment
			if self.allsatModels > 1 && self.allsatBlockingFromInstanceAlg > 0 && self.blockingVars.is_empty()
			{
				let blockingVarsStr: &[u8] = if l.len >= 2 { &l.line[2..] } else { &[] };
				let errMsg = match ReadCommaSeparatedVarList(blockingVarsStr)
				{
					Ok(blockingVars) =>
					{
						self.blockingVars = blockingVars;
						String::new()
					}
					Err(errMsg) => errMsg,
				};
				if !errMsg.is_empty() || self.blockingVars.is_empty()
				{
					return Err("c topor_tool ERROR: expected the first comment to contain blocking variables at line number ".to_string() + &lineNum.to_string() + ". Error message: " + &errMsg);
				}

				if self.allsatBlockingFromInstanceAlg == 1
				{
					for i in 0..self.blockingVars.len()
					{
						let lit = self.blockingVars[i];
						self.Topor().ToporCreateInternalLit(lit);
					}
				}

				if self.allsatBlockingFromInstanceAlg == 2
				{
					let blockingVarsSet: HashSet<TLit> = self.blockingVars.iter().copied().collect();
					let mut lit: TLit = 1;
					while lit < self.varsInPCnf
					{
						if !blockingVarsSet.contains(&lit)
						{
							self.Topor().ToporCreateInternalLit(lit);
						}
						lit += 1;
					}
				}
			}
			return Ok(None);
		}

		// The solver commands need the solver instance, which only exists once the p-line or the first clause is read
		// (the C++ version dereferences a null pointer otherwise)
		if ((l.curr() == b'r' && !self.ignoreFileParams) || l.curr() == b'l' || l.curr() == b'b' || l.curr() == b'n') && self.AllToporsNull()
		{
			return Err("c topor_tool ERROR: a solver command appears before the p-line and the first clause at line number ".to_string() + &lineNum.to_string());
		}

		if l.curr() == b'r'
		{
			if !self.ignoreFileParams
			{
				let paramNameStart: usize = 2;
				let paramNameEnd = match l.find(b' ', 2)
				{
					None => return Err("c topor_tool ERROR: expected <paramName> never ended at line number ".to_string() + &lineNum.to_string()),
					Some(paramNameEnd) => paramNameEnd,
				};
				let paramName = l.substrn(paramNameStart, paramNameEnd - paramNameStart).unwrap();
				let paramVal = l.substr(paramNameEnd + 1).unwrap();

				let paramValDouble = match stod(&paramVal)
				{
					Some(v) => v,
					None => return Err("c topor_tool ERROR: couldn't convert the parameter value to double at line number ".to_string() + &lineNum.to_string()),
				};

				self.Topor().ToporSetParam(&paramName, paramValDouble);
			}

			return Ok(None);
		}

		if l.curr() == b'o'
		{
			// cout << "\tc ot <TimeOut> <IsCpuTimeOut>" << endl;
			// cout << "\tc oc <ConflictThreshold>" << endl;
			if l.at(1) != b't' && l.at(1) != b'c'
			{
				return Err("c topor_tool ERROR: The 2nd character must be either t or c at line number ".to_string() + &lineNum.to_string());
			}

			if l.at(2) != b' '
			{
				return Err("c topor_tool ERROR: The 3nd character must be a space at line number ".to_string() + &lineNum.to_string());
			}

			if l.at(1) == b't'
			{
				let toNameStart: usize = 2;
				let toNameEnd = match l.find(b' ', 3)
				{
					None => return Err("c topor_tool ERROR: expected <TimeOut> <IsCpuTimeOut> at line number ".to_string() + &lineNum.to_string()),
					Some(toNameEnd) => toNameEnd,
				};
				let toStr = l.substrn(toNameStart, toNameEnd - toNameStart).unwrap();
				let to = match stod(&toStr)
				{
					Some(v) => v,
					None => return Err("c topor_tool ERROR: couldn't convert <TimeOut> to double at line number ".to_string() + &lineNum.to_string()),
				};

				let isCpuTimeOutStr = l.substr(toNameEnd + 1).unwrap();
				let isCpuTimeOut = match stoi(&isCpuTimeOutStr)
				{
					Some(v) => v,
					None => return Err("c topor_tool ERROR: couldn't convert <IsCpuTimeOut> to int at line number ".to_string() + &lineNum.to_string()),
				};

				if !(0..=1).contains(&isCpuTimeOut)
				{
					return Err("c topor_tool ERROR: couldn't convert <IsCpuTimeOut> to 0 or 1 at line number ".to_string() + &lineNum.to_string());
				}
				self.nextSolveToInSecIsCpuTime = (to, isCpuTimeOut != 0);
			}
			else
			{
				debug_assert!(l.at(1) == b'c');
				let cThrStr = l.substr(3).unwrap();

				self.nextSolveConfThr = match stoull(&cThrStr)
				{
					Some(v) => v,
					None => return Err("c topor_tool ERROR: couldn't convert <ConflictThreshold> to uint64_t at line number ".to_string() + &lineNum.to_string()),
				};
			}

			return Ok(None);
		}

		if l.curr() == b'l'
		{
			// cout << "\tc lb <BoostScoreLit> <Mult>" << endl;
			// cout << "\tc lf <FixPolarityLit> <OnlyOnce>" << endl;
			// cout << "\tc lc <ClearUserPolarityInfoLit>" << endl;
			// cout << "\tc ll <LitToCreateInternalLit>" << endl;
			if l.at(1) != b'b' && l.at(1) != b'f' && l.at(1) != b'c' && l.at(1) != b'l'
			{
				return Err("c topor_tool ERROR: The 2nd character must be either b or f or c at line number ".to_string() + &lineNum.to_string());
			}

			if l.at(2) != b' '
			{
				return Err("c topor_tool ERROR: The 3nd character must be a space at line number ".to_string() + &lineNum.to_string());
			}

			l.currLineI += 2;
			let lit = l.ParseNumber(lineNum)? as TLit;

			if l.at(1) == b'c'
			{
				self.Topor().ToporClearUserPolarityInfo(lit);
			}
			else if l.at(1) == b'f'
			{
				// std::out_of_range, if there is nothing after the literal
				let isOnlyOnceStr = l.substr(l.currLineI + 1).ok_or("basic_string::substr")?;
				let isOnlyOnce = match stoi(&isOnlyOnceStr)
				{
					Some(v) => v,
					None => return Err("c topor_tool ERROR: couldn't convert <FixPolarityLit> to int at line number ".to_string() + &lineNum.to_string()),
				};

				if !(0..=1).contains(&isOnlyOnce)
				{
					return Err("c topor_tool ERROR: couldn't convert <FixPolarityLit> to 0 or 1 at line number ".to_string() + &lineNum.to_string());
				}
				self.Topor().ToporFixPolarity(lit, isOnlyOnce != 0);
			}
			else if l.at(1) == b'l'
			{
				self.Topor().ToporCreateInternalLit(lit);
			}
			else
			{
				debug_assert!(l.at(1) == b'b');
				// std::out_of_range, if there is nothing after the literal
				let multStr = l.substr(l.currLineI + 1).ok_or("basic_string::substr")?;

				let mult = match stod(&multStr)
				{
					Some(v) => v,
					None => return Err("c topor_tool ERROR: couldn't convert the <Mult> value to double at line number ".to_string() + &lineNum.to_string()),
				};

				self.Topor().ToporBoostScore(lit, mult);
			}

			return Ok(None);
		}

		if l.curr() == b'b'
		{
			// cout << "\tc b <BacktrackLevel>" << endl;
			if l.at(1) != b' '
			{
				return Err("c topor_tool ERROR: The 2nd character must be a space at line number ".to_string() + &lineNum.to_string());
			}

			l.currLineI += 1;
			let dl = l.ParseNumber(lineNum)?;

			self.Topor().ToporBacktrack(dl as TLit);

			return Ok(None);
		}

		if l.curr() == b'n'
		{
			// cout << "\tc n <ConfigNum>" << endl;
			if l.at(1) != b' '
			{
				return Err("c topor_tool ERROR: The 2nd character must be a space at line number ".to_string() + &lineNum.to_string());
			}

			l.currLineI += 1;
			let configNum = l.ParseNumber(lineNum)?;
			if configNum < 0 || configNum > u16::MAX as i64
			{
				return Err("c topor_tool ERROR: The configuration number ".to_string() + &configNum.to_string() + " must be a uint_16 integer at line number " + &lineNum.to_string());
			}

			let mut str = self.Topor().ToporChangeConfigToGiven(configNum as u16);

			// Erase all Occurrences of given substring from main string.
			let EraseAllSubStr = |mainStr: &mut String, toErase: &str|
			{
				// Search for the substring in string in a loop until nothing is found
				while let Some(pos) = mainStr.find(toErase)
				{
					// If found then erase it from string
					mainStr.replace_range(pos..pos + toErase.len(), "");
				}
			};

			EraseAllSubStr(&mut str, "/topor");

			coutln!(self.out, "c converted configuration number {} to parameters {}", configNum, str);

			return Ok(None);
		}

		if l.curr() == b'p'
		{
			if self.pLineRead
			{
				coutln!(self.out, "c topor_tool ERROR: second line starting with p at line number {}", lineNum);
				return Ok(Some(BadRetVal));
			}

			l.currLineI += 6;
			// currLineI should be at the first number now
			if l.at(l.currLineI - 5) != b' ' || l.at(l.currLineI - 4) != b'c' || l.at(l.currLineI - 3) != b'n' || l.at(l.currLineI - 2) != b'f' || l.at(l.currLineI - 1) != b' '
			{
				coutln!(self.out, "c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>' at line number {}", lineNum);
				return Ok(Some(BadRetVal));
			}

			// try
			let varsClss = l.ParseNumber(lineNum).and_then(|varsLL| l.ParseNumber(lineNum).map(|clssLL| (varsLL, clssLL)));
			match varsClss
			{
				Ok((varsLL, clssLL)) =>
				{
					coutln!(self.out, "c topor_tool: suggested #variables : {}; suggested #clauses : {}", varsLL, clssLL);
					if varsLL > TLit::MAX as i64 || varsLL <= 0
					{
						coutln!(self.out, "c topor_tool warning: the suggested #variables {} is greater than the maximal number or is <=0, thus it will be ignored", varsLL);
					}
					else
					{
						self.varsInPCnf = varsLL as TLit;
					}
				}
				// catch (const logic_error& le)
				Err(what) =>
				{
					coutln!(self.out, "c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': {} at line number {}", what, lineNum);
					return Ok(Some(BadRetVal));
				}
			}

			l.SkipWhitespaces();
			if l.curr() != b'\n'
			{
				coutln!(self.out, "c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': new-line wasn't found where expected at line number {}", lineNum);
				return Ok(Some(BadRetVal));
			}

			debug_assert!(self.AllToporsNull());
			if self.CreateToporInst(self.varsInPCnf) == BadRetVal
			{
				return Ok(Some(BadRetVal));
			}

			return Ok(None);
		}

		// If we're here and topor is still missing, create it without the number-of-variables hint -- no p-line is expected anymore
		if self.AllToporsNull()
		{
			if self.CreateToporInst(0) == BadRetVal
			{
				return Ok(Some(BadRetVal));
			}
		}

		if l.curr() == b's'
		{
			l.currLineI += 1;
			l.SkipWhitespaces();

			let (errString, assumps) = l.BufferToLits(lineNum);
			if !errString.is_empty()
			{
				cout!(self.out, "{}", errString);
				return Ok(Some(BadRetVal));
			}

			if self.Solve(Some(&assumps)) == BadRetVal
			{
				return Ok(Some(BadRetVal));
			}
			return Ok(None);
		}

		// New clause
		let (errString, cls) = l.BufferToLits(lineNum);
		if !errString.is_empty()
		{
			cout!(self.out, "{}", errString);
			return Ok(Some(BadRetVal));
		}
		if self.verifyModel
		{
			self.vmClss.push(cls.clone());
		}
		self.Topor().ToporAddClause(&cls);

		Ok(None)
	}

	// Whatever main does after the file reading loop
	fn OnInputCompleted(&mut self) -> i32
	{
		if !self.AllToporsNull() && self.Topor().ToporGetSolveInvs() == 0
		{
			if self.allsatModels > 1 && !self.blockingVars.is_empty()
			{
				let assumpsEmpty: Vec<TLit> = Vec::new();
				let blockingVars = self.blockingVars.clone();
				self.ret = self.Topor().ToporSolve(&[], (f64::MAX, true), u64::MAX);
				self.retValBasedOnLatestSolve = self.ToporOnFinishedSolving(&assumpsEmpty, Some(&blockingVars));
				if self.verifyModel && self.retValBasedOnLatestSolve == 10
				{
					if self.VerifyModel(None) == BadRetVal
					{
						return BadRetVal;
					}
				}

				let mut cls: Vec<TLit> = Vec::new();
				let mut currModelNum: u64 = 1;
				while currModelNum < self.allsatModels && self.retValBasedOnLatestSolve == 10
				{
					coutln!(self.out, "c topor_tool: before adding a blocking clause and calling the solver for time {} out of {}", currModelNum + 1, self.allsatModels);
					cls.clear();
					cls.reserve(blockingVars.len());
					for v in &blockingVars
					{
						let toporLitVal = self.Topor().ToporGetLitValue(*v);
						debug_assert!(toporLitVal != TToporLitVal::VAL_UNASSIGNED);
						cls.push(if toporLitVal == TToporLitVal::VAL_SATISFIED { -*v } else { *v });
					}
					self.Topor().ToporAddClause(&cls);
					self.ret = self.Topor().ToporSolve(&[], (f64::MAX, true), u64::MAX);
					self.retValBasedOnLatestSolve = self.ToporOnFinishedSolving(&assumpsEmpty, Some(&blockingVars));
					if self.verifyModel && self.retValBasedOnLatestSolve == 10
					{
						if self.VerifyModel(None) == BadRetVal
						{
							return BadRetVal;
						}
					}
					currModelNum += 1;
				}
			}
			else if self.Solve(None) == BadRetVal
			{
				return BadRetVal;
			}
		}

		self.retValBasedOnLatestSolve
	}
}

fn PrintUsage(out: &mut dyn Write)
{
	let red = |s: &str| print_as_color(s, ansi_color_code::red);
	let cyan = |s: &str| print_as_color(s, ansi_color_code::cyan);
	let green = |s: &str| print_as_color(s, ansi_color_code::green);

	coutln!(out, "{}", red("c Usage:"));
	coutln!(out, "\tc <Intel(R) SAT Solver Executable> <CNF> OPTIONAL: <Param1> <Val1> <Param2> <Val2> ... <ParamN> <ValN>");
	coutln!(out, "\tc <CNF> can either be a text file or an archive file in one of the following formats: .xz, .lzma, .bz2, .gz, .7z (the test is based on the file signature)");
	coutln!(out, "\tc <CNF> is expected to be in simplified DIMACS format, used at SAT Competitions (http://www.satcompetition.org/2011/format-benchmarks2011.html) with the following optional extension to support incrementality:");
	coutln!(out, "\tc The following Intel(R) SAT Solver Executable-specific commands are also legal (ignore \"c \" below): ");
	coutln!(out, "\tc r <ParamName> <ParamVal>");
	coutln!(out, "\tc ot <TimeOut> <IsCpuTimeOut>");
	coutln!(out, "\tc oc <ConflictThreshold>");
	coutln!(out, "\tc lb <BoostScoreLit> <Mult>");
	coutln!(out, "\tc lf <FixPolarityLit> <OnlyOnce>");
	coutln!(out, "\tc ll <LitToCreateInternalLit>");
	coutln!(out, "\tc lc <ClearUserPolarityInfoLit>");
	coutln!(out, "\tc b <BacktrackLevel>");
	coutln!(out, "\tc n <ConfigNumber>");
	coutln!(out, "\tc s <Lit1 <Lit2> ... <Litn>: solve under the assumptions {{<Lit1 <Lit2> ... <Litn>}}");
	coutln!(out, "\tc The solver parses the p cnf vars clss line, but it ignores the number of clauses and uses the number of variables as a non-mandatory hint");
	coutln!(out, "{}", red("c Intel(R) SAT Solver executable parameters:"));
	cout!(out, "\tc {} : enum (0, 1, or 2); default = {} : what type of solver to use in terms of clause buffer indexing and compression: 0 -- 32-bit index, uncompressed, 1 -- 64-bit index, uncompressed, 2 -- 64-bit index, bit-array compression \n", cyan("/topor_tool/solver_mode"), green("0"));
	cout!(out, "\tc {} : string; default = {} : path to a file to write down a binary DRAT proof\n", cyan("/topor_tool/bin_drat_file"), green("\"\""));
	cout!(out, "\tc {} : string; default = {} : path to a file to write down a text DRAT proof (if more than one /topor_tool/bin_drat_file and /topor_tool/text_drat_file parameters provided, only the last one is applied, rest are ignored)\n", cyan("/topor_tool/text_drat_file"), green("\"\""));
	cout!(out, "\tc {} : bool (0 or 1); default = {} : sort every clause in DRAT proof (can be helpful for debugging)\n", cyan("/topor_tool/drat_sort_every_clause"), green("0"));
	cout!(out, "\tc {} : bool (0 or 1); default = {} : print the models for satisfiable invocations?\n", cyan("/topor_tool/print_model"), green("1"));
	cout!(out, "\tc {} : bool (0 or 1); default = {} : print the indices of the assumptions in the unsatisfiable core for unsatisfiable invocations (0-indexed)?\n", cyan("/topor_tool/print_ucore"), green("1"));
	cout!(out, "\tc {} : bool (0 or 1); default = {} : verify the models for satisfiable invocations?\n", cyan("/topor_tool/verify_model"), green("0"));
	cout!(out, "\tc {} : bool (0 or 1); default = {} : verify the unsatisfiable cores in terms of assumptions for unsatisfiable invocations?\n", cyan("/topor_tool/verify_ucore"), green("0"));
	cout!(out, "\tc {} : bool (0 or 1); default = {} : ignore parameter settings in the input file (lines starting with 'r')?\n", cyan("/topor_tool/ignore_file_params"), green("0"));
	cout!(out, "\tc {} : unsigned long integer; default = 1{} : the maximal number of models for AllSAT. AllSAT with blocking clauses over /topor_tool/allsat_blocking_variables's variables is invoked if: (1) this parameter is greater than 1; (2) the CNF format is DIMACS without Topor-specific commands; (3) /topor_tool/allsat_blocking_variables is non-empty\n", cyan("/topor_tool/allsat_models_number"), green("1"));
	cout!(out, "\tc {} : string; default = {} : if /topor_tool/allsat_models_number > 1, specifies the variables which will be used for blocking clauses, sperated by a comma, e.g., 1,4,5,6,7,15.\n", cyan("/topor_tool/allsat_blocking_variables"), green("\"\""));
	cout!(out, "\tc {} : string; default = {} : if /topor_tool/allsat_models_number > 1 and our parameter > 0, read the blocking variables from the first comment line in the file (format: c 1,4,5,6,7,15), where the value means: 1 -- assign lowest internal SAT variables to blocking; 2 -- assign highest internal SAT variables to blocking; >=3 -- assign their own internal SAT variables to blocking \n", cyan("/topor_tool/allsat_blocking_variables_file_alg"), green("3"));

	let topor = CTopor::<u32, false>::new(0);
	cout!(out, "{}", topor.GetParamsDescr());
	let _ = out.flush();
}

// Does the signature match one of the archive types we support?
// getc provides the next character of the file (or EOF)
fn IdentifyArchiveFileType(getc: &mut dyn FnMut() -> i32) -> TArchiveFileType
{
	let mut aFileType = TArchiveFileType::None;

	let mut c = getc();
	let mut currType = 0;
	while c != EOF && currType < fileSig.len() && aFileType == TArchiveFileType::None
	{
		let currSig = &fileSig[currType];

		if c == currSig[0]
		{
			let mut rightType = true;
			c = getc();
			let mut i = 1;
			while c != EOF && currSig[i] != EOF
			{
				if c != currSig[i]
				{
					rightType = false;
					break;
				}
				i += 1;
				c = getc();
			}

			if rightType
			{
				aFileType = [TArchiveFileType::XZ, TArchiveFileType::LZMA, TArchiveFileType::BZ, TArchiveFileType::GZ, TArchiveFileType::SevenZ][currType];
			}
		}
		currType += 1;
	}

	aFileType
}

// The main function of the tool: argv includes the executable name; the output goes to out
// Returns Ok(the return value of main) or Err(what) for a logic_error thrown by the C++ version (which is never caught, so the C++ program is terminated)
fn ToporToolMain(argv: &[String], out: &mut dyn Write) -> Result<i32, String>
{
	let argc = argv.len();
	if argc <= 1 || argv[1] == "-help" || argv[1] == "--help" || argv[1] == "-h"
	{
		PrintUsage(out);
		return Ok(0);
	}

	coutln!(out, "c Intel(R) SAT Solver started");

	if argc & 1 != 0
	{
		coutln!(out, "c topor_tool ERROR: the number of arguments (excluding the executable name) must be odd. Run without parameters for more information.");
		return Ok(BadRetVal);
	}

	/*
	* Identify the input file type, read it, read the parameters too
	*/

	let inputFileName = &argv[1];
	if !std::path::Path::new(inputFileName).exists()
	{
		coutln!(out, "c topor_tool ERROR: the input file {} doesn't exist", inputFileName);
		return Ok(BadRetVal);
	}

	// Does the signature match one of the archive types we support?

	let aFileType = match std::fs::File::open(inputFileName)
	{
		Err(_) =>
		{
			coutln!(out, "c topor_tool ERROR: couldn't open the input file {} to verify the signature", inputFileName);
			return Ok(BadRetVal);
		}
		Ok(tmp) =>
		{
			let mut tmpBytes = BufReader::new(tmp).bytes();
			IdentifyArchiveFileType(&mut || match tmpBytes.next()
			{
				Some(Ok(c)) => c as i32,
				_ => EOF,
			})
		}
	};

	if aFileType != TArchiveFileType::None
	{
		let (program, args, discardStderr) = archiveCommands[aFileType as usize];
		coutln!(out, "c topor_tool: file type determined to an archive file. The following command will be used to read it through a pipe : {} {} {} {}", program, args.join(" "), inputFileName, if discardStderr { "2>/dev/null" } else { "" });
	}

	// Open the input file: it can be an archive file, in which case it is read as a pipe
	// or a plain text file, which is read as a file
	let useFopen = aFileType == TArchiveFileType::None;
	let mut pipe: Option<std::process::Child> = None;
	let mut f: Box<dyn BufRead> = if useFopen
	{
		match std::fs::File::open(inputFileName)
		{
			Ok(f) => Box::new(BufReader::new(f)),
			Err(_) =>
			{
				coutln!(out, "c topor_tool ERROR: couldn't open the input file");
				return Ok(BadRetVal);
			}
		}
	}
	else
	{
		// popen
		let (program, args, discardStderr) = archiveCommands[aFileType as usize];
		let mut command = std::process::Command::new(program);
		command.args(args).arg(inputFileName).stdout(std::process::Stdio::piped());
		if discardStderr
		{
			command.stderr(std::process::Stdio::null());
		}
		match command.spawn()
		{
			Ok(mut child) =>
			{
				let childStdout = child.stdout.take().unwrap();
				pipe = Some(child);
				Box::new(BufReader::new(childStdout))
			}
			// popen succeeds even if the program is missing (the shell reports it and the pipe provides nothing), so the input is empty
			Err(e) if e.kind() == std::io::ErrorKind::NotFound =>
			{
				if !discardStderr
				{
					eprintln!("sh: {}: command not found", program);
				}
				Box::new(std::io::empty())
			}
			Err(_) =>
			{
				coutln!(out, "c topor_tool ERROR: couldn't open the input file");
				return Ok(BadRetVal);
			}
		}
	};

	let mut tool = CToporTool::new(argv, out);

	/*
	* File reading loop
	*/

	// The maximal number of characters in a line, including the terminating 0 (fgets splits longer lines)
	const maxSz: u64 = 1u64 << 28;

	let mut res: Result<Option<i32>, String> = Ok(None);
	loop
	{
		// fgets(line, (int)maxSz, f)
		let mut line: Vec<u8> = Vec::new();
		match (&mut f).take(maxSz - 1).read_until(b'\n', &mut line)
		{
			Ok(n) if n > 0 => {}
			_ => break,
		}

		let mut l = CLine::new(line);
		res = tool.HandleLine(&mut l);
		// beforeNextLoop on exit from scope
		tool.lineNum += 1;

		if !matches!(res, Ok(None))
		{
			break;
		}
	}

	let res = match res
	{
		Ok(None) => Ok(tool.OnInputCompleted()),
		Ok(Some(retVal)) => Ok(retVal),
		Err(what) => Err(what),
	};

	// onExit: closing the input, closing the DRAT file (if any) and deleting the solver
	drop(tool);
	drop(f);
	if let Some(mut child) = pipe
	{
		// pclose
		let _ = child.wait();
	}

	res
}

fn main()
{
	let argv: Vec<String> = std::env::args().collect();
	let mut out = std::io::stdout();
	match ToporToolMain(&argv, &mut out)
	{
		Ok(retVal) =>
		{
			let _ = out.flush();
			std::process::exit(retVal);
		}
		Err(what) =>
		{
			// An uncaught exception terminates the C++ program
			let _ = out.flush();
			eprintln!("terminate called after throwing an instance of 'std::logic_error'\n  what():  {}", what);
			std::process::abort();
		}
	}
}

#[cfg(test)]
#[path = "tests/main_tests.rs"]
mod main_tests;
