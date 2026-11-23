// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::basic_memory_usage;
use crate::color_print::{ansi_color_code, print_as_color};
use crate::time_measure::CTimeMeasure;
use crate::topor_external_types::TLit;

// std::to_string(double) and operator<< for floating-points print nan and inf in lower-case
fn fixed(v: f64, precision: usize) -> String
{
	if v.is_nan()
	{
		"nan".to_string()
	}
	else
	{
		format!("{:.*}", precision, v)
	}
}

// The equivalent of: stm << std::scientific << std::setprecision(1) << v
pub fn to_string_scientific(v: f64) -> String
{
	if v.is_nan() || v.is_infinite()
	{
		return fixed(v, 1);
	}
	let s = format!("{:.1e}", v);
	let (mantissa, exp) = s.split_once('e').unwrap();
	let exp: i32 = exp.parse().unwrap();
	format!("{}e{}{:02}", mantissa, if exp < 0 { '-' } else { '+' }, exp.abs())
}

// The values, which the C++ version receives through the M_BGetNum, M_BGetCap, M_BGetSize and M_GetExtraString callbacks, capturing the solver
// Here, the solver provides them explicitly, whenever the statistics string is requested
pub struct TStatBufferInfo
{
	// The number, capacity and sizes of the main buffer(s), containing the clauses, in bytes
	pub m_b_num: usize,
	pub m_b_cap: usize,
	pub m_b_size: usize,
	// An extra-string
	pub m_extra_string: String,
}

// Statistics, available to the user
pub struct TToporStatistics
{
	// The maximal variable number, provided by the user
	pub m_MaxUserVar: TLit,
	// The last existing internal variable
	pub m_MaxInternalVar: u64,

	// The number of invocations of Solve function
	pub m_SolveInvs: u64,
	// The number of short incremental invocations of Solve function
	pub m_ShortIncSolveInvs: u64,
	// The number of invocations of AddClause function
	pub m_AddClauseInvs: u64,

	// The number of active binary clauses
	pub m_ActiveBinaryClss: u64,
	// The number of active long (non-binary) clauses
	pub m_ActiveLongClss: u64,
	// The number of active long learnt clauses (of any quality: low, medium, high)
	pub m_ActiveLongLearntClss: u64,
	// The number of binary clauses ever added
	pub m_EverAddedBinaryClss: u64,

	// The number of backtracks
	pub m_Backtracks: u64,
	// The number of chronological backtracks
	pub m_ChronoBacktracks: u64,
	// The number of BCP-backtracks (required to handle multiple conflicts and delayed implications)
	pub m_BCPBacktracks: u64,

	// The number of backtrack levels, saved by reusing assumptions
	pub m_AssumpReuseBacktrackLevelsSaved: u64,
	// The sum of all the levels at which a decision was taken
	pub m_SumOfAllDecLevels: u64,

	// The number of conflicts
	pub m_Conflicts: u64,

	// The number of assignments
	pub m_Assignments: u64,

	// The number of decisions
	pub m_Decisions: u64,

	// The number of BCP invocations
	pub m_BCPs: u64,

	// The number of implications (in all BCPs)
	pub m_Implications: u64,

	// The number of triggering delayed implications (that is, not propagated)
	pub m_DelayedImplicationsTrigerring: u64,
	// The number of propagated delayed implications
	pub m_DelayedImplicationsPropagated: u64,
	// The number of decision levels, which collapsed (disappeared) as a result of delayed implications
	pub m_DelayedImplicationDecLevelsCollapsed: u64,

	// The current variable decay (initialized to m_ParamVarDecay in Solve)
	pub m_VarDecay: f64,
	pub m_VarActivityInc: f64,

	// The number of flipped clauses
	pub m_FlippedClauses: u64,
	// The number of flipped clauses, swapped with 1UIP clauses (since they're better asserting clauses after minimization)
	pub m_FlippedClausesSwapped: u64,
	// The number of flipped clauses, which became unit: note that every swapped clause is unit, but not every unit is swapped
	pub m_FlippedClausesUnit: u64,

	// The number of literals removed by subsumption during conflict analysis
	pub m_LitsRemovedByConfSubsumption: u64,

	// The number of restarts
	pub m_Restarts: u64,
	// The number of blocked restarts
	pub m_RestartsBlocked: u64,

	// The number of Simplify invocations
	pub m_Simplifies: u64,
	// The number of clause-deletion invocations
	pub m_ClssDel: u64,

	// The number of literals deleted from clauses by ALL-UIP scheme
	pub m_LitsRemovedByAllUip: u64,
	pub m_AllUipAttempted: u32,
	pub m_AllUipSucceeded: u32,

	// The number of inprocessings so far
	pub m_Ings: u32,
	// The number of duplicate binary clauses, removed by inprocessing
	pub m_IngsDuplicateBinsRemoved: u32,

	// Rust-only extension (topi_ext_target_phase.rs): the number of rephases, of updates of the target phases and of decisions, which took the target phase
	pub m_ExtRephases: u64,
	pub m_ExtTargetPhaseUpdates: u64,
	pub m_ExtTargetPhaseDecisions: u64,

	pub(crate) m_OverallTime: CTimeMeasure,
	pub(crate) m_TimeSinceLastSolveStart: CTimeMeasure,
	pub(crate) m_ShortStartInv: u8,
	pub(crate) m_ActiveOverallClsLen: u64,
}

impl TToporStatistics
{
	pub fn new(var_activity_inc: f64) -> Self
	{
		TToporStatistics
		{
			m_MaxUserVar: 0,
			m_MaxInternalVar: 0,
			m_SolveInvs: 0,
			m_ShortIncSolveInvs: 0,
			m_AddClauseInvs: 0,
			m_ActiveBinaryClss: 0,
			m_ActiveLongClss: 0,
			m_ActiveLongLearntClss: 0,
			m_EverAddedBinaryClss: 0,
			m_Backtracks: 0,
			m_ChronoBacktracks: 0,
			m_BCPBacktracks: 0,
			m_AssumpReuseBacktrackLevelsSaved: 0,
			m_SumOfAllDecLevels: 0,
			m_Conflicts: 0,
			m_Assignments: 0,
			m_Decisions: 0,
			m_BCPs: 0,
			m_Implications: 0,
			m_DelayedImplicationsTrigerring: 0,
			m_DelayedImplicationsPropagated: 0,
			m_DelayedImplicationDecLevelsCollapsed: 0,
			m_VarDecay: 0.0,
			m_VarActivityInc: var_activity_inc,
			m_FlippedClauses: 0,
			m_FlippedClausesSwapped: 0,
			m_FlippedClausesUnit: 0,
			m_LitsRemovedByConfSubsumption: 0,
			m_Restarts: 0,
			m_RestartsBlocked: 0,
			m_Simplifies: 0,
			m_ClssDel: 0,
			m_LitsRemovedByAllUip: 0,
			m_AllUipAttempted: 0,
			m_AllUipSucceeded: 0,
			m_Ings: 0,
			m_IngsDuplicateBinsRemoved: 0,
			m_ExtRephases: 0,
			m_ExtTargetPhaseUpdates: 0,
			m_ExtTargetPhaseDecisions: 0,
			m_OverallTime: CTimeMeasure::new(false, 1000),
			m_TimeSinceLastSolveStart: CTimeMeasure::new(false, 1000),
			m_ShortStartInv: 0,
			m_ActiveOverallClsLen: 0,
		}
	}

	pub fn StatStrShort(&mut self, bi: &TStatBufferInfo, force_printing_head: bool, is_color: bool) -> String
	{
		let mut ss_head = String::new();
		let mut ss_stat = String::new();
		let print_head = force_printing_head || self.m_ShortStartInv.is_multiple_of(50);
		if print_head
		{
			ss_head += "c ToporStt ";
		}
		ss_stat += "c ToporStt ";

		let col = |c: ansi_color_code| if is_color { c } else { ansi_color_code::none };
		let mut head = |c: ansi_color_code, s: &str|
		{
			if print_head
			{
				ss_head += &print_as_color(s, col(c));
			}
		};
		let mut stat = |c: ansi_color_code, s: String|
		{
			ss_stat += &print_as_color(s, col(c));
			ss_stat += " ";
		};
		// The to_string member of the C++ struct: precision 1 for floating-points
		let f = |v: f64| fixed(v, 1);

		head(ansi_color_code::cyan, "CpuT0 WallT0 CPUTSolve WallTSolve CurrMemMb PeakMemMb");
		stat(ansi_color_code::cyan, f(self.m_OverallTime.CpuTimePassedSinceStartOrResetConst()));
		stat(ansi_color_code::cyan, f(self.m_OverallTime.WallTimePassedSinceStartOrResetConst()));
		stat(ansi_color_code::cyan, f(self.m_TimeSinceLastSolveStart.CpuTimePassedSinceStartOrResetConst()));
		stat(ansi_color_code::cyan, f(self.m_TimeSinceLastSolveStart.WallTimePassedSinceStartOrResetConst()));
		stat(ansi_color_code::cyan, basic_memory_usage::GetCurrentRSSMb().to_string());
		stat(ansi_color_code::cyan, basic_memory_usage::GetPeakRSSMb().to_string());

		head(ansi_color_code::magenta, " Confs Decs D/C BCPs Asngs ImplPr ImplPerCPUT");
		stat(ansi_color_code::magenta, self.m_Conflicts.to_string());
		stat(ansi_color_code::magenta, self.m_Decisions.to_string());
		stat(ansi_color_code::magenta, f(self.m_Decisions as f64 / self.m_Conflicts as f64));
		stat(ansi_color_code::magenta, self.m_BCPs.to_string());
		stat(ansi_color_code::magenta, self.m_Assignments.to_string());
		stat(ansi_color_code::magenta, f(Self::Perc(self.m_Implications, self.m_Assignments)));
		stat(ansi_color_code::magenta, f(self.m_Implications as f64 / self.m_OverallTime.CpuTimePassedSinceStartOrResetConst()));

		head(ansi_color_code::black, " Inprocs DupBins");
		stat(ansi_color_code::black, self.m_Ings.to_string());
		stat(ansi_color_code::black, self.m_IngsDuplicateBinsRemoved.to_string());

		head(ansi_color_code::red, " Bufs BufSzMb BufCapMb");
		stat(ansi_color_code::red, bi.m_b_num.to_string());
		stat(ansi_color_code::red, f(bi.m_b_size as f64 / 1000000.));
		stat(ansi_color_code::red, f(bi.m_b_cap as f64 / 1000000.));

		head(ansi_color_code::blue, " SolveInvs ShortIncrInvs");
		stat(ansi_color_code::blue, self.m_SolveInvs.to_string());
		stat(ansi_color_code::blue, self.m_ShortIncSolveInvs.to_string());

		head(ansi_color_code::cyan, " UserVars IntrVars");
		stat(ansi_color_code::cyan, self.m_MaxUserVar.to_string());
		stat(ansi_color_code::cyan, self.m_MaxInternalVar.to_string());

		head(ansi_color_code::magenta, " AddClss ActClss BinAClss LongAClss AvrgAClsLen AvrgALongClsLen LongALrnts");
		stat(ansi_color_code::magenta, self.m_AddClauseInvs.to_string());
		stat(ansi_color_code::magenta, self.GetActiveClss().to_string());
		stat(ansi_color_code::magenta, self.m_ActiveBinaryClss.to_string());
		stat(ansi_color_code::magenta, self.m_ActiveLongClss.to_string());
		stat(ansi_color_code::magenta, f(if self.GetActiveClss() == 0 { 0.0 } else { self.m_ActiveOverallClsLen as f64 / self.GetActiveClss() as f64 }));
		stat(ansi_color_code::magenta, f(if self.m_ActiveLongClss == 0 { 0.0 } else { (self.m_ActiveOverallClsLen - (self.m_ActiveBinaryClss << 1)) as f64 / self.m_ActiveLongClss as f64 }));
		stat(ansi_color_code::magenta, self.m_ActiveLongLearntClss.to_string());

		head(ansi_color_code::black, " Simplfs ClssDels SubsLRem");
		stat(ansi_color_code::black, self.m_Simplifies.to_string());
		stat(ansi_color_code::black, self.m_ClssDel.to_string());
		stat(ansi_color_code::black, self.m_LitsRemovedByConfSubsumption.to_string());

		head(ansi_color_code::red, " FlpRecs FlpSw FlpUnit");
		stat(ansi_color_code::red, self.m_FlippedClauses.to_string());
		stat(ansi_color_code::red, self.m_FlippedClausesSwapped.to_string());
		stat(ansi_color_code::red, self.m_FlippedClausesUnit.to_string());

		head(ansi_color_code::green, " Bts");
		stat(ansi_color_code::green, self.m_Backtracks.to_string());
		head(ansi_color_code::green, " ChBtsPr");
		stat(ansi_color_code::green, f(self.ChronoBtsPerc()));
		head(ansi_color_code::green, " BCPBtPr");
		stat(ansi_color_code::green, f(self.BCPBtsPerc()));
		head(ansi_color_code::green, " AvrgDecLev");
		stat(ansi_color_code::green, f(if self.m_Decisions == 0 { 0.0 } else { self.m_SumOfAllDecLevels as f64 / self.m_Decisions as f64 }));
		head(ansi_color_code::green, " LvlsSvdAsmp");
		stat(ansi_color_code::green, self.m_AssumpReuseBacktrackLevelsSaved.to_string());
		head(ansi_color_code::green, " RTAsg");

		head(ansi_color_code::black, " VSIDSInc VSIDSDecay");
		stat(ansi_color_code::black, to_string_scientific(self.m_VarActivityInc));
		stat(ansi_color_code::black, fixed(self.m_VarDecay, 3));

		head(ansi_color_code::bright_cyan, " Rsts RstBlocked");
		stat(ansi_color_code::bright_cyan, self.m_Restarts.to_string());
		stat(ansi_color_code::bright_cyan, self.m_RestartsBlocked.to_string());

		self.m_ShortStartInv = self.m_ShortStartInv.wrapping_add(1);

		(if print_head { ss_head + "\n" } else { String::new() }) + &ss_stat + &(if bi.m_extra_string.is_empty() { "\n".to_string() } else { "\n".to_string() + &bi.m_extra_string + "\n" })
	}

	pub fn UpdateMaxInternalVar(&mut self, miv: u64)
	{
		self.m_MaxInternalVar = miv;
	}

	// The number of active clauses
	pub fn GetActiveClss(&self) -> u64
	{
		self.m_ActiveLongClss + self.m_ActiveBinaryClss
	}

	#[inline(always)]
	pub fn ChronoBtsPerc(&self) -> f64
	{
		Self::Perc(self.m_ChronoBacktracks, self.m_Backtracks)
	}

	#[inline(always)]
	pub fn BCPBtsPerc(&self) -> f64
	{
		Self::Perc(self.m_BCPBacktracks, self.m_Backtracks)
	}

	#[inline(always)]
	fn Perc(fraction: u64, total: u64) -> f64
	{
		if total == 0 { 0. } else { 100. * fraction as f64 / total as f64 }
	}

	#[inline]
	pub(crate) fn NewClause(&mut self, cls_len: usize, is_learnt: bool)
	{
		if cls_len >= 2
		{
			self.m_ActiveOverallClsLen += cls_len as u64;
			if cls_len == 2
			{
				self.m_ActiveBinaryClss += 1;
			}
			else
			{
				self.m_ActiveLongClss += 1;
			}
			if is_learnt && cls_len > 2
			{
				self.m_ActiveLongLearntClss += 1;
			}
			if cls_len == 2
			{
				self.m_EverAddedBinaryClss += 1;
			}
		}
	}

	#[inline]
	pub(crate) fn DeleteClause(&mut self, cls_len: usize, is_learnt: bool)
	{
		if cls_len >= 2
		{
			self.m_ActiveOverallClsLen -= cls_len as u64;
			if cls_len == 2
			{
				self.m_ActiveBinaryClss -= 1;
			}
			else
			{
				self.m_ActiveLongClss -= 1;
			}
			if is_learnt && cls_len > 2
			{
				self.m_ActiveLongLearntClss -= 1;
			}
		}
	}

	#[inline(always)]
	pub(crate) fn DeleteBinClauses(&mut self, num_of_clauses: usize)
	{
		self.m_ActiveOverallClsLen -= (num_of_clauses as u64) << 1;
		self.m_ActiveBinaryClss -= num_of_clauses as u64;
	}

	#[inline(always)]
	pub(crate) fn RecordDeletedLitsFromCls(&mut self, lits_to_delete: u64)
	{
		self.m_ActiveOverallClsLen -= lits_to_delete;
	}

	#[inline(always)]
	pub(crate) fn GetActiveLongClsLen(&self) -> u64
	{
		self.m_ActiveOverallClsLen - (self.m_ActiveBinaryClss << 1)
	}

	#[inline(always)]
	pub(crate) fn NewSolveInvocation(&mut self, is_short_inc_query: bool)
	{
		self.m_SolveInvs += 1;
		if is_short_inc_query
		{
			self.m_ShortIncSolveInvs += 1;
		}
		self.m_TimeSinceLastSolveStart.Reset();
	}
}
