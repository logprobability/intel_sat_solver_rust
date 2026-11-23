// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

// The main solver class: the data members and whatever is defined inline in Topi.hpp
// The rest of the member functions reside in the files, which correspond to the C++ translation units:
// topi_main.rs (Topi.cc), topi_asg.rs, topi_backtrack.rs, topi_bcp.rs, topi_bit_compression.rs, topi_compression.rs, topi_conflict_analysis.rs,
// topi_debug_printing.rs, topi_decision.rs, topi_inprocess.rs, topi_restart.rs, topi_wl.rs

use std::cell::{Cell, RefCell, UnsafeCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use crate::topi_params::*;
pub use crate::topi_statistics::*;
pub use crate::topi_types::*;
pub use crate::topi_var_scores::CVarScores;
pub use crate::topor_bit_array::CBitArray;
pub use crate::topor_dyn_array::{CDynArray, Span};
pub use crate::topor_external_types::*;
pub use crate::topor_vector::CVector;
pub use crate::topor_win_average::TWinAverage;

// Parameters: restart strategy
pub const RESTART_STRAT_NUMERIC: u8 = 0;
pub const RESTART_STRAT_LBD: u8 = 1;
pub const RESTART_STRAT_NONE: u8 = 2;

// Change the parameter to this value immediately after the initial invocation; don't undo the change
pub const m_ContextParamAfterInitInvPrefix: &str = "/__ai";
// Change the parameter to this value immediately before any invocation with conflict threshold <= /meta_strategy/short_inv_conf_thr; undo the change immediately after Solve
pub const m_ContextParamShortInvLifetimePrefix: &str = "/__s";

// The unit-clause report function of a higher-level parallel solver
pub type TReportUnitCls = Box<dyn FnMut(u32, i32)>;
// The get-next-unit-clause function of a higher-level parallel solver
pub type TGetNextUnitClause = Box<dyn FnMut(u32, bool) -> i32>;

// ReserveExactly(toReserve, newCap, initVal, errSuffix)
#[macro_export]
macro_rules! ReserveExactlyInit
{
	($self:expr, $to_reserve:expr, $new_cap:expr, $init_val:expr, $err_suffix:expr) =>
	{
		if !$self.IsUnrecoverable()
		{
			let new_cap = $new_cap;
			let init_val: u8 = $init_val;
			$to_reserve.reserve_exactly_init(new_cap, init_val);
			if $to_reserve.uninitialized_or_erroneous()
			{
				$self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "Couldn't ReserveExactly with initial value ".to_string() + &init_val.to_string() + " : " + $err_suffix);
			}
		}
	};
}

// ReserveExactly(toReserve, newCap, errSuffix)
#[macro_export]
macro_rules! ReserveExactly
{
	($self:expr, $to_reserve:expr, $new_cap:expr, $err_suffix:expr) =>
	{
		if !$self.IsUnrecoverable()
		{
			let new_cap = $new_cap;
			$to_reserve.reserve_exactly(new_cap);
			if $to_reserve.uninitialized_or_erroneous()
			{
				$self.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "Couldn't ReserveExactly : ".to_string() + $err_suffix);
			}
		}
	};
}

// The main solver class
// The template parameters are:
// (1) The unsigned index into the clause and watch buffers TUInd (Ind)
// (2) A Boolean specifying whether to compress the clause buffer
// The signed literal type TLit is always int32_t (see topor_external_types.rs)
pub struct CTopi<Ind: TUInd, const Compress: bool>
{
	/*
	* Parameters
	*/

	// Contains a map from parameter name to parameter description & function to set it for every parameter, and the parameters themselves
	pub(crate) m_Params: CTopiParams,

	/*
	* INITIALIZATION-RELATED
	*/

	// The number of initially allocated entries in variable-indexed arrays
	pub(crate) m_InitVarNumAlloc: usize,

	/*
	* STATUS
	*/

	pub(crate) m_IsSolveOngoing: bool,
	pub(crate) m_Status: Cell<TToporStatus>,
	pub(crate) m_StatusExplanation: RefCell<String>,

	/*
	* INITIALIZATION AND EXTERNAL->INTERNAL VARIABLE MAP
	*/

	// external variable-->internal literal map
	pub(crate) m_E2ILitMap: CDynArray<TULit>,
	pub(crate) m_NewExternalVarsAddUserCls: CVector<TLit>,
	// internal variable-->external literal map (initialized only, if required, e.g., for callbacks)
	pub(crate) m_I2ELitMap: CDynArray<TLit>,

	// Used to make sure tautologies are discovered and duplicates are removed in new clauses
	pub(crate) m_HandleNewUserCls: CHandleNewCls,

	// The last internal variable so-far
	pub(crate) m_LastExistingVar: TUVar,

	/*
	* CLAUSE-BUFFER
	*/

	// Bit-compressed buffers, indexed by TBCHashId
	pub(crate) m_BC: TCompressedClss,

	// Fixed mode buffer
	pub(crate) m_B: CDynArray<TULit>,
	pub(crate) m_BNext: Ind,
	pub(crate) m_BWasted: Ind,

	pub(crate) m_AnyOversized: bool,

	pub(crate) m_TmpClss: UnsafeCell<[CDynArray<TULit>; m_TmpClssCountMax]>,
	pub(crate) m_CurrTmpClssCount: Cell<u8>,
	pub(crate) m_TmpClssDebug: UnsafeCell<[CDynArray<TULit>; m_TmpClssCountMax]>,
	pub(crate) m_CurrTmpClssCountDebug: Cell<u8>,

	/*
	* WATCHES
	*/

	// Literal-indexed watch information
	pub(crate) m_Watches: CDynArray<TWatchInfo<Ind>>,

	// Separate buffer for the watches
	// The beginning of moved regions are marked as deleted in the following format:
	// [long_2(allocated-entries) 0]
	pub(crate) m_W: CDynArray<TULit>,
	pub(crate) m_WNext: Ind,
	pub(crate) m_WWasted: Ind,

	/*
	* DECISION LEVELS, ASSIGNMENTS, TRAIL
	*/

	// The current decision level
	pub(crate) m_DecLevel: TUV,
	// The last variable on the trail per decision level
	pub(crate) m_TrailLastVarPerDecLevel: CDynArray<TUVar>,
	pub(crate) m_BestScorePerDecLevel: CDynArray<f64>,
	// The pointer to trail start
	pub(crate) m_TrailStart: TUVar,
	// The pointer to trail end
	pub(crate) m_TrailEnd: TUVar,
	// Literals to propagate
	pub(crate) m_ToPropagate: CVector<TULit>,

	pub(crate) m_PolarityInfoActivated: bool,

	pub(crate) m_AssignedVarsNum: TUV,
	pub(crate) m_AssignmentInfo: CDynArray<TAssignmentInfo>,
	pub(crate) m_PrevAiCap: usize,
	pub(crate) m_VarInfo: CDynArray<TVarInfo<Ind>>,
	pub(crate) m_PolarityInfo: CDynArray<TPolarityInfo>,
	pub(crate) m_UpdateParamsWhenVarFixedDone: bool,
	pub(crate) m_NonForcedPolaritySelectionForFlip: u32,

	// Literal currently propagated by BCP
	pub(crate) m_CurrentlyPropagatedLit: TULit,
	// Contradictions stashed during BCP
	pub(crate) m_Cis: CVector<TContradictionInfo<Ind>>,
	// Holding delayed implications
	pub(crate) m_Dis: CVector<TDelImpl<Ind>>,
	pub(crate) m_CurrPropWatchModifiedDuringProcessDelayedImplication: bool,

	/*
	* Assumptions
	*/

	// The vector of assumptions
	// During Solve invocation, m_Assumps will hold all the assumptions,
	// where m_Assumps's capacity is precisely the number of assumptions. It will always be cleared at the end of Solve.
	pub(crate) m_Assumps: CDynArray<TULit>,
	// A falsified assumption of the earliest decision level (if any)
	pub(crate) m_EarliestFalsifiedAssump: TULit,
	// The decision level of the last assigned assumption
	pub(crate) m_DecLevelOfLastAssignedAssumption: TUV,

	// Assumption-UNSAT-core-related
	//
	// If there is an internal contradiction in the assumption list, this is a literal of the problematic variable
	pub(crate) m_SelfContrOrGloballyUnsatAssump: TULit,
	// Latest m_EarliestFalsifiedAssump for assumption-UNSAT-core
	pub(crate) m_LatestEarliestFalsifiedAssump: TULit,
	// The Solve invocation corresponding to the value in m_SelfContradictingAssump
	pub(crate) m_SelfContrOrGloballyUnsatAssumpSolveInv: u64,
	// The Solve invocation corresponding to the latest m_EarliestFalsifiedAssump for assumption-UNSAT-core
	pub(crate) m_LatestEarliestFalsifiedAssumpSolveInv: u64,
	// The latest user assumptions
	pub(crate) m_UserAssumps: Vec<TLit>,
	// The latest invocation of Solve, after which an assumption UNSAT core was requested and extracted
	pub(crate) m_LatestAssumpUnsatCoreSolveInvocation: u64,

	/*
	* Statistics
	*/

	pub(crate) m_Stat: TToporStatistics,

	/*
	* Decision Strategy
	*/

	pub(crate) m_VsidsHeap: CVarScores,
	pub(crate) m_CurrInitClssBoostScoreMult: f64,

	/*
	* Backtracking
	*/

	pub(crate) m_CurrChronoBtIfHigher: TUV,
	pub(crate) m_CurrCustomBtStrat: u8,
	pub(crate) m_ConfsSinceNewInv: u64,

	/*
	* Conflict Analysis and Learning
	*/

	pub(crate) m_VarsParentSubsumed: Vec<TParentSubsumed<Ind>>,

	// An array of vectors of literals for various algorithms (which must cleaned up *before* every usage)
	// The capacity, assigned at the beginning of Solve, is the number of variables
	pub(crate) m_HandyLitsClearBefore: [CVector<TULit>; 2],
	// All the variables, whose m_Visit flag is on
	pub(crate) m_VisitedVars: CVector<TUVar>,
	// All the variables, whose m_Root flag is on
	pub(crate) m_RootedVars: CVector<TUVar>,

	pub(crate) m_HugeCounterDecLevels: u64,
	pub(crate) m_HugeCounterPerDecLevel: CDynArray<u64>,
	pub(crate) m_MarkedDecLevelsCounter: TCounterType,
	pub(crate) m_DecLevelsLastAppearenceCounter: CDynArray<TCounterType>,
	// For flipped recording
	pub(crate) m_FlippedLit: TULit,

	// Counters required for on-the-fly subsuming all the parents
	pub(crate) m_CurrClsCounter: TCounterType,
	pub(crate) m_CurrClsCounters: CDynArray<TCounterType>,

	// All-UIP-related

	// The gap (see the SAT'20 All-UIP paper)
	pub(crate) m_AllUipGap: TUV,
	pub(crate) m_AllUipAttemptedCurrRestart: u32,
	pub(crate) m_AllUipSucceededCurrRestart: u32,

	/*
	* RESTARTS
	*/

	pub(crate) m_RstNumericCurrConfThr: u64,
	pub(crate) m_ConfsSinceRestart: u64,
	pub(crate) m_RstNumericLocalConfsSinceRestartAtDecLevelCreation: CDynArray<u64>,

	pub(crate) m_RstGlueLbdWin: TWinAverage,
	pub(crate) m_RstGlueGlobalLbdSum: f64,

	pub(crate) m_RstGlueBlckAsgnWin: TWinAverage,
	pub(crate) m_RstGlueBlckGlobalAsgnSum: u64,
	pub(crate) m_RstGlueAssertingGluedClss: u64,

	pub(crate) m_CurrRestartStrat: u8,
	pub(crate) m_RestartsSinceInvStart: u64,

	/*
	* Compression, simplification, deletion,  (of the clause buffer, the watch buffer, the literals)
	*/

	pub(crate) m_LastGloballySatisfiedLitAfterSimplify: TUVar,
	pub(crate) m_ImplicationsTillNextSimplify: i64,

	// The index of the first learnt clause: it is guaranteed that no conflict clause appears before m_FirstLearntClsInd in the buffer
	// For non-incremental instances, there may be some initial clauses after m_FirstLearntClsInd
	// The above is correct for non-compressed mode only as m_FirstLearntClsInd is irrelevant for the compressed mode
	pub(crate) m_FirstLearntClsInd: Ind,

	// Clause loop
	pub(crate) m_CurrLoopIsLearntOnly: bool,
	// Clause loop: current info for standard clauses
	pub(crate) m_ClsLoopCurrStandardCls: Ind,
	pub(crate) m_ClsLoopNextStandardCls: Ind,
	// Clause loop: current info for compressed clauses (an "iterator" into m_BC, that is, a hash-id, where m_BC.end() stands for the end)
	pub(crate) m_ClsLoopCurrCompressedBA: usize,
	pub(crate) m_ClsLoopCurrCompressedBACurrBit: u64,

	pub(crate) m_ClsDelInfo: TClsDelInfo,

	pub(crate) m_ClsDelOneTierActivityIncrease: f64,

	/*
	* Query-type, including parameters
	*/

	pub(crate) m_AfterInitInvParamVals: Vec<(String, f64)>,
	pub(crate) m_ShortInvLifetimeParamVals: Vec<(String, f64)>,

	pub(crate) m_QueryCurr: TQueryType,
	pub(crate) m_QueryPrev: TQueryType,

	/*
	* Phase management
	*/

	pub(crate) m_PhaseStage: TPhaseStage,
	pub(crate) m_PhaseInitStage: TPhaseStage,

	/*
	* DRAT
	*/

	pub(crate) m_OpenedDratFile: Option<Box<dyn std::io::Write>>,
	pub(crate) m_IsDratBinary: bool,
	pub(crate) m_DratSortEveryClause: bool,

	/*
	* Callbacks
	*/

	pub(crate) M_CbStopNow: Option<TCbStopNow>,
	// Can be raised asynchronously, e.g., from a signal handler or from another thread, hence the atomic
	pub(crate) m_InterruptNow: Arc<AtomicBool>,
	pub(crate) M_CbNewLearntCls: Option<TCbNewLearntCls>,
	pub(crate) m_UserCls: CVector<TLit>,

	/*
	* Parallel support
	*/

	pub(crate) m_ThreadId: u32,
	pub(crate) M_ReportUnitCls: Option<TReportUnitCls>,
	pub(crate) M_GetNextUnitClause: Option<TGetNextUnitClause>,

	/*
	* Inprocessing
	*/

	// m_Stat.m_SolveInvs last time inprocessing was invoked
	pub(crate) m_IngLastSolveInv: u64,
	// m_Stat.m_Conflicts last time inprocessing was invoked
	pub(crate) m_IngLastConflicts: u64,
	// m_Stat.m_EverAddedBinaryClss last time inprocessing was invoked
	pub(crate) m_IngLastEverAddedBinaryClss: u64,

	/*
	* Debugging
	*/

	pub(crate) m_DebugModel: Vec<bool>,

	/*
	* Dump file
	*/

	pub(crate) m_DumpFile: Option<std::fs::File>,

	/*
	* Misc
	*/

	pub(crate) m_AxePrinted: bool,
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	/*
	* Internal types
	*/

	pub const Standard: bool = !Compress;

	// The number of literal-entries in an index-entry
	pub const LitsInInd: usize = Ind::LitsInInd;

	// Bad parent clause index
	pub const BadClsInd: Ind = Ind::ZERO;

	// varNumHint is the expected number of variables -- a non-mandatory hint, which, if provided correctly, helps the solver initialize faster
	pub fn new(varNumHint: TLit) -> Self
	{
		let m_InitVarNumAlloc = if varNumHint <= 0 { InitEntriesInB } else { varNumHint as usize + 1 };
		let m_Params = CTopiParams::new(TWatchInfo::<Ind>::BinsInLongBitCeil as u8);
		let varActivityInc = m_Params.m_ParamVarActivityInc.get();

		let mut topi = CTopi
		{
			m_Params,
			m_InitVarNumAlloc,
			m_IsSolveOngoing: false,
			m_Status: Cell::new(TToporStatus::STATUS_UNDECIDED),
			m_StatusExplanation: RefCell::new(String::new()),
			m_E2ILitMap: CDynArray::new_init(m_InitVarNumAlloc, 0),
			m_NewExternalVarsAddUserCls: CVector::new(0, 0),
			m_I2ELitMap: CDynArray::new(0),
			m_HandleNewUserCls: CHandleNewCls::new(m_InitVarNumAlloc),
			m_LastExistingVar: 0,
			m_BC: TCompressedClss::default(),
			m_B: CDynArray::new(if Compress { 1 } else { InitEntriesInB }),
			m_BNext: Ind::from_usize(if Compress { 1 } else { LitsInPage }),
			m_BWasted: Ind::ZERO,
			m_AnyOversized: false,
			m_TmpClss: UnsafeCell::new([CDynArray::new(0), CDynArray::new(0), CDynArray::new(0)]),
			m_CurrTmpClssCount: Cell::new(0),
			m_TmpClssDebug: UnsafeCell::new([CDynArray::new(0), CDynArray::new(0), CDynArray::new(0)]),
			m_CurrTmpClssCountDebug: Cell::new(0),
			m_Watches: CDynArray::new_init(Self::GetInitLitNumAllocStatic(m_InitVarNumAlloc), 0),
			m_W: CDynArray::new(InitEntriesInB),
			m_WNext: Ind::from_usize(LitsInPage),
			m_WWasted: Ind::ZERO,
			m_DecLevel: 0,
			m_TrailLastVarPerDecLevel: CDynArray::new_init(1, 0),
			m_BestScorePerDecLevel: CDynArray::new(0),
			m_TrailStart: BadUVar,
			m_TrailEnd: BadUVar,
			m_ToPropagate: CVector::new(0, 0),
			m_PolarityInfoActivated: false,
			m_AssignedVarsNum: 0,
			m_AssignmentInfo: CDynArray::new(0),
			m_PrevAiCap: 0,
			m_VarInfo: CDynArray::new_init(m_InitVarNumAlloc, 0),
			m_PolarityInfo: CDynArray::new(0),
			m_UpdateParamsWhenVarFixedDone: false,
			m_NonForcedPolaritySelectionForFlip: 0,
			m_CurrentlyPropagatedLit: BadULit,
			m_Cis: CVector::new(0, 0),
			m_Dis: CVector::new(0, 0),
			m_CurrPropWatchModifiedDuringProcessDelayedImplication: false,
			m_Assumps: CDynArray::new(0),
			m_EarliestFalsifiedAssump: BadULit,
			m_DecLevelOfLastAssignedAssumption: 0,
			m_SelfContrOrGloballyUnsatAssump: BadULit,
			m_LatestEarliestFalsifiedAssump: BadULit,
			m_SelfContrOrGloballyUnsatAssumpSolveInv: 0,
			m_LatestEarliestFalsifiedAssumpSolveInv: 0,
			m_UserAssumps: Vec::new(),
			m_LatestAssumpUnsatCoreSolveInvocation: u64::MAX,
			m_Stat: TToporStatistics::new(varActivityInc),
			m_VsidsHeap: CVarScores::new(),
			m_CurrInitClssBoostScoreMult: 0.,
			m_CurrChronoBtIfHigher: 0,
			m_CurrCustomBtStrat: 0,
			m_ConfsSinceNewInv: 0,
			m_VarsParentSubsumed: Vec::new(),
			m_HandyLitsClearBefore: [CVector::new(0, 0), CVector::new(0, 0)],
			m_VisitedVars: CVector::new(0, 0),
			m_RootedVars: CVector::new(0, 0),
			m_HugeCounterDecLevels: 0,
			m_HugeCounterPerDecLevel: CDynArray::new(0),
			m_MarkedDecLevelsCounter: 0,
			m_DecLevelsLastAppearenceCounter: CDynArray::new(0),
			m_FlippedLit: BadULit,
			m_CurrClsCounter: 0,
			m_CurrClsCounters: CDynArray::new(0),
			m_AllUipGap: 0,
			m_AllUipAttemptedCurrRestart: 0,
			m_AllUipSucceededCurrRestart: 0,
			m_RstNumericCurrConfThr: 0,
			m_ConfsSinceRestart: 0,
			m_RstNumericLocalConfsSinceRestartAtDecLevelCreation: CDynArray::new(0),
			m_RstGlueLbdWin: TWinAverage::new(),
			m_RstGlueGlobalLbdSum: 0.,
			m_RstGlueBlckAsgnWin: TWinAverage::new(),
			m_RstGlueBlckGlobalAsgnSum: 0,
			m_RstGlueAssertingGluedClss: 0,
			m_CurrRestartStrat: RESTART_STRAT_NONE,
			m_RestartsSinceInvStart: 0,
			m_LastGloballySatisfiedLitAfterSimplify: BadUVar,
			m_ImplicationsTillNextSimplify: 0,
			m_FirstLearntClsInd: Ind::MAX,
			m_CurrLoopIsLearntOnly: false,
			m_ClsLoopCurrStandardCls: Self::BadClsInd,
			m_ClsLoopNextStandardCls: Self::BadClsInd,
			m_ClsLoopCurrCompressedBA: BCTableSize,
			m_ClsLoopCurrCompressedBACurrBit: u64::MAX,
			m_ClsDelInfo: TClsDelInfo::default(),
			m_ClsDelOneTierActivityIncrease: 1.0,
			m_AfterInitInvParamVals: Vec::new(),
			m_ShortInvLifetimeParamVals: Vec::new(),
			m_QueryCurr: TQueryType::QUERY_NONE,
			m_QueryPrev: TQueryType::QUERY_NONE,
			m_PhaseStage: TPhaseStage::PHASE_STAGE_STANDARD,
			m_PhaseInitStage: TPhaseStage::PHASE_STAGE_STANDARD,
			m_OpenedDratFile: None,
			m_IsDratBinary: true,
			m_DratSortEveryClause: false,
			M_CbStopNow: None,
			m_InterruptNow: Arc::new(AtomicBool::new(false)),
			M_CbNewLearntCls: None,
			m_UserCls: CVector::new(0, 0),
			m_ThreadId: u32::MAX,
			M_ReportUnitCls: None,
			M_GetNextUnitClause: None,
			m_IngLastSolveInv: 0,
			m_IngLastConflicts: 0,
			m_IngLastEverAddedBinaryClss: 0,
			m_DebugModel: Vec::new(),
			m_DumpFile: None,
			m_AxePrinted: false,
		};

		topi.m_AssignmentInfo.reserve_exactly_init(topi.m_InitVarNumAlloc, 0);

		if let Ok(dumpFileName) = std::env::var("TOPOR_DUMP_NAME")
		{
			topi.DumpSetUp(&dumpFileName);
			// The C++ version asserts that the stream object exists, which holds also when the file couldn't be opened; here m_DumpFile is None in that case
			if let Some(dumpFile) = &mut topi.m_DumpFile
			{
				use std::io::Write;
				let _ = writeln!(dumpFile, "p cnf {} {}", varNumHint, 0);
			}
		}

		if topi.m_Params.IsError()
		{
			topi.SetStatus(TToporStatus::STATUS_PARAM_ERROR, topi.m_Params.GetErrorDescr());
		}
		else if topi.m_B.uninitialized_or_erroneous()
		{
			topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::CTopi: couldn't allocate the main buffer".to_string());
		}
		else if topi.m_InitVarNumAlloc != 0 && topi.m_E2ILitMap.uninitialized_or_erroneous()
		{
			topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::CTopi: couldn't allocate m_E2IVarMap".to_string());
		}
		else if topi.GetInitLitNumAlloc() != 0 && topi.m_Watches.uninitialized_or_erroneous()
		{
			topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::CTopi: couldn't allocate m_Watches".to_string());
		}
		else if topi.m_InitVarNumAlloc != 0 && topi.m_AssignmentInfo.uninitialized_or_erroneous()
		{
			topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::CTopi: couldn't allocate m_AssignmentInfo".to_string());
		}
		else if topi.m_InitVarNumAlloc != 0 && topi.m_VarInfo.uninitialized_or_erroneous()
		{
			topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::CTopi: couldn't allocate m_VarInfo".to_string());
		}
		else if topi.m_TrailLastVarPerDecLevel.uninitialized_or_erroneous()
		{
			topi.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "CTopi<TLit,TUInd,Compress>::CTopi: couldn't allocate m_TrailLastVarPerDecLevel".to_string());
		}

		topi.SetMultipliers();

		topi.ReadAnyParamsFromFile();

		topi
	}

	fn GetInitLitNumAllocStatic(initVarNumAlloc: usize) -> usize
	{
		debug_assert!(initVarNumAlloc <= 1usize.rotate_right(1));
		let initLitNumAlloc = initVarNumAlloc << 1;
		if initLitNumAlloc == 0 { usize::MAX } else { initLitNumAlloc }
	}

	// The number of initially allocated entries in literal-indexed arrays
	#[inline(always)]
	pub(crate) fn GetInitLitNumAlloc(&self) -> usize
	{
		Self::GetInitLitNumAllocStatic(self.m_InitVarNumAlloc)
	}

	// Get the explanation for the current status (returns the empty string, if there is no error and verbosity is off)
	pub fn GetStatusExplanation(&self) -> String
	{
		if self.IsError() || self.m_Params.m_ParamVerbosity.get() > 0 { self.m_StatusExplanation.borrow().clone() } else { String::new() }
	}

	// Dump DRAT, if invoked
	pub fn DumpDrat(&mut self, openedDratFile: Box<dyn std::io::Write>, isDratBinary: bool, dratSortEveryClause: bool)
	{
		self.m_OpenedDratFile = Some(openedDratFile);
		self.m_IsDratBinary = isDratBinary;
		self.m_DratSortEveryClause = dratSortEveryClause;
	}

	// Set callback: stop-now
	pub fn SetCbStopNow(&mut self, CbStopNow: Option<TCbStopNow>)
	{
		self.M_CbStopNow = CbStopNow;
	}

	// Interrupt now
	pub fn InterruptNow(&self)
	{
		self.m_InterruptNow.store(true, Ordering::Relaxed);
	}

	// The handle, which can be used to interrupt the solver asynchronously (e.g., from a signal handler or from another thread) by storing true into it
	pub fn GetInterruptNowHandle(&self) -> Arc<AtomicBool>
	{
		self.m_InterruptNow.clone()
	}

	// Set callback: new-learnt-clause
	pub fn SetCbNewLearntCls(&mut self, CbNewLearntCls: Option<TCbNewLearntCls>)
	{
		self.M_CbNewLearntCls = CbNewLearntCls;
	}

	#[inline(always)]
	pub(crate) fn IsVsidsInitOrderParam(&self, paramName: &str) -> bool
	{
		paramName == "/decision/vsids/init_order"
	}

	#[inline(always)]
	pub(crate) fn IsMultiplierParam(&self, paramName: &str) -> bool
	{
		paramName == "/multiplier/clauses" || paramName == "/multiplier/variables" || paramName == "/multiplier/watches_if_separate"
	}

	/*
	* STATUS
	*/

	#[inline(always)]
	pub(crate) fn IsUnrecoverable(&self) -> bool
	{
		self.m_Status.get() >= TToporStatus::STATUS_FIRST_UNRECOVERABLE
	}

	#[inline(always)]
	pub(crate) fn IsErroneous(&self) -> bool
	{
		self.m_Status.get() >= TToporStatus::STATUS_FIRST_PERMANENT_ERROR
	}

	#[inline(always)]
	pub(crate) fn SetStatus(&self, s: TToporStatus, expl: String)
	{
		self.m_Status.set(s);
		*self.m_StatusExplanation.borrow_mut() = expl;
	}

	// An explicit-clauses-only function invoked in compressed mode
	#[cold]
	pub(crate) fn CompressedMismatch(&self, funcName: &str)
	{
		self.SetStatus(TToporStatus::STATUS_COMPRESSED_MISMATCH, funcName.to_string() + " invoked in compressed mode");
	}

	/*
	* INITIALIZATION AND EXTERNAL->INTERNAL VARIABLE MAP
	*/

	#[inline(always)]
	pub(crate) fn GetExternalLit(&self, iLit: TULit) -> TLit
	{
		debug_assert!(self.UseI2ELitMap());
		if IsNeg(iLit) { -self.m_I2ELitMap[GetVar(iLit) as usize] } else { self.m_I2ELitMap[GetVar(iLit) as usize] }
	}

	pub(crate) fn UseI2ELitMap(&self) -> bool
	{
		self.m_Params.m_ParamVerifyDebugModelInvocation.get() != 0 || self.IsCbLearntOrDrat() || self.M_ReportUnitCls.is_some()
	}

	#[inline(always)]
	pub(crate) fn E2I(&self, l: TLit) -> TULit
	{
		let externalV = ExternalLit2ExternalVar(l);
		let internalL = self.m_E2ILitMap[externalV as usize];
		if l < 0 { Negate(internalL) } else { internalL }
	}

	// Does E2I(l) index inside m_E2ILitMap? Not for TLit::MIN, whose negation wraps to itself, and not after a failed allocation emptied the map
	#[inline]
	pub(crate) fn IsE2IInBounds(&self, l: TLit) -> bool
	{
		let externalV = ExternalLit2ExternalVar(l);
		externalV >= 0 && (externalV as usize) < self.m_E2ILitMap.cap()
	}

	#[inline(always)]
	pub(crate) fn GetNextVar(&self) -> TUVar
	{
		self.m_LastExistingVar + 1
	}

	#[inline(always)]
	pub(crate) fn GetLastExistingLit(&self) -> TULit
	{
		GetLit(self.m_LastExistingVar, true)
	}

	#[inline(always)]
	pub(crate) fn GetNextLit(&self) -> TULit
	{
		self.GetLastExistingLit() + 1
	}

	#[inline(always)]
	pub(crate) fn GetAssignedLitForVar(&self, v: TUVar) -> TULit
	{
		debug_assert!(self.IsAssignedVar(v));
		GetLit(v, self.m_AssignmentInfo[v as usize].m_IsNegated())
	}

	/*
	* CLAUSE-BUFFER
	*/

	// BIT-COMPRESSED BUFFERS

	pub(crate) fn BCRemoveGarbage(&mut self, mut NotifyAboutRemainingChunkMove: Option<&mut dyn FnMut(&mut Self, u64, u64)>)
	{
		debug_assert!(self.NV(2) || self.P("BCRemoveGarbage\n"));
		let mut bcIt = self.m_BC.begin();
		while bcIt != self.m_BC.end()
		{
			let bcHashInd = TBCHashId::from_val(bcIt as u16);
			// A raw pointer, dereferenced at each use: ClsEnd and Notify access the same bit-array through the solver in-between
			// SAFETY: the bit-array is boxed, so the pointer stays valid until it is erased below, after its last use
			let ba: *mut CBitArray = self.m_BC.at(bcIt as u16);

			let mut fromInd: u64 = 0;
			let mut toInd: u64 = 0;

			while fromInd < unsafe { (*ba).bit_get_next_bit() }
			{
				// Update fromInd to the next remaining clause (or to beyond the buffer capacity)
				let mut bcInd = TBCInd::<Ind>::new(bcHashInd, fromInd);
				while fromInd < unsafe { (*ba).bit_get_next_bit() } && ((bcHashInd.m_BitsForClsSize() != 0 && unsafe { &*ba }.bit_get(fromInd, bcHashInd.m_BitsForClsSize()) == 0) || unsafe { &*ba }.bit_get(bcInd.BitFirstLit(), bcHashInd.m_BitsForLit()) == 0)
				{
					fromInd = self.ClsEnd(bcInd.ind()).as_u64();
					bcInd = TBCInd::<Ind>::new(bcHashInd, fromInd);
				}

				if fromInd >= unsafe { (*ba).bit_get_next_bit() }
				{
					// Nothing useful to move --> we're done!
					break;
				}

				// Copy the current clause (starting at fromInd)
				let bcInd = TBCInd::<Ind>::new(bcHashInd, fromInd);
				let clsEndInd = self.ClsEnd(bcInd.ind()).as_u64();
				let bits2Copy = clsEndInd - fromInd;
				if toInd != fromInd
				{
					debug_assert!(self.NV(2) || self.P(&("Copying the clause: ".to_string() + &self.SLits(self.Cls(bcInd.ind()), false) + " from " + &HexStrInd(bcInd.ind()) + "," + &HexStr(fromInd, 16) + " to " + &HexStr(toInd, 16) + ", where " + &bits2Copy.to_string() + " bits are copied\n")));

					if let Some(Notify) = &mut NotifyAboutRemainingChunkMove
					{
						let bcToInd = TBCInd::<Ind>::new(bcHashInd, toInd);
						Notify(self, bcInd.ind().as_u64(), bcToInd.ind().as_u64());
					}

					unsafe { &mut *ba }.copy_block(fromInd, toInd, bits2Copy);
				}

				toInd += bits2Copy;
				fromInd += bits2Copy;
			}

			unsafe { &mut *ba }.bit_resize_and_compress(toInd);

			let bcItNext = self.m_BC.next(bcIt);
			if toInd == 0
			{
				self.m_BC.erase(bcIt as u16);
			}
			bcIt = bcItNext;
		}
	}

	#[inline(always)]
	pub(crate) fn BCGetBitArray(&mut self, i: u16) -> &mut CBitArray
	{
		// SAFETY: at(i) points at the boxed bit-array of the existing hash-id i, which m_BC owns; the borrow is tied to self
		unsafe { &mut *self.m_BC.at(i) }
	}

	#[inline(always)]
	pub(crate) fn BCGetBitArrayConst(&self, i: u16) -> &CBitArray
	{
		// SAFETY: as in BCGetBitArray
		unsafe { &*self.m_BC.at(i) }
	}

	#[inline(always)]
	pub(crate) fn BCMaxLitWidth(&self, cls: TSpanTULit) -> u16
	{
		let maxElem = cls.iter().max().unwrap();
		// This assertion should hold as long as 5 bits are allocated for m_BitsForLit
		// For having literals of up to 2^64, it should be sufficient to allocate 6 bits for m_BitsForLit, in which case the assert below should be removed
		debug_assert!(maxElem as u64 <= 0x100000000u64);
		(TULit::BITS - maxElem.leading_zeros()) as u16
	}

	// CCompressedClsConstSnapshotDense/CStandardClsConstSnapshotDense
	pub(crate) fn ClsConstSnapshotDense(&self, clsInd: Ind, mut maxLitsNum: TUV) -> CClsConstSnapshotDense
	{
		if Compress
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let cc = CCls::<Ind, Compress>::new_compressed(self.m_BC.at(bcInd.GetHashId().val()), bcInd);

			if maxLitsNum as usize > cc.size()
			{
				maxLitsNum = cc.size() as TUV;
			}

			let mut clsLits = Vec::with_capacity(maxLitsNum as usize);

			for i in 0..maxLitsNum
			{
				clsLits.push(cc.get(i as usize));
			}
			CClsConstSnapshotDense::new_compressed(clsLits)
		}
		else
		{
			let isLearnt = self.ClsGetIsLearnt(clsInd);
			let isOversized = self.m_AnyOversized && isLearnt && self.EClsGetStandardActivityAsInt(clsInd) == ClsOversizeActivity;
			let litsStartOffset = self.EClsLitsStartOffset(isLearnt, isOversized);
			let sz = if !isLearnt || isOversized { self.m_B[clsInd.as_usize()] & !ClsIsLearntMask } else { self.m_B[clsInd.as_usize()] & !ClsIsLearntAndGlueMask };
			CClsConstSnapshotDense::new_standard(self.m_B.get_span_cap_sz(clsInd.as_usize() + litsStartOffset as usize, std::cmp::min(sz, maxLitsNum) as usize))
		}
	}

	#[inline(always)]
	pub(crate) fn EClsSetIsLearnt(&mut self, clsInd: Ind, isLearnt: bool)
	{
		if Compress
		{
			self.CompressedMismatch("EClsSetIsLearnt");
		}
		else
		{
			self.m_B[clsInd.as_usize()] = (self.m_B[clsInd.as_usize()] & !ClsIsLearntMask) | ((isLearnt as TUV) << ClsLShiftToIsLearntOn);
			debug_assert!(self.ClsGetIsLearnt(clsInd) == isLearnt);
		}
	}

	#[inline(always)]
	pub(crate) fn ClsGetIsLearnt(&self, clsInd: Ind) -> bool
	{
		if !Compress
		{
			self.m_B[clsInd.as_usize()] & ClsIsLearntMask != 0
		}
		else
		{
			TBCInd::from_ind(clsInd).m_IsLearnt()
		}
	}

	// *EClsGetStandardActivityAsConstIntPtr(clsInd)
	#[inline(always)]
	pub(crate) fn EClsGetStandardActivityAsInt(&self, clsInd: Ind) -> i32
	{
		if !Compress
		{
			self.m_B[clsInd.as_usize() + 1] as i32
		}
		else
		{
			self.CompressedMismatch("EClsGetStandardActivityAsConstIntPtr");
			0
		}
	}

	// *EClsGetStandardActivityAsIntPtr(clsInd) = val
	#[inline(always)]
	pub(crate) fn EClsSetStandardActivityAsInt(&mut self, clsInd: Ind, val: i32)
	{
		if !Compress
		{
			self.m_B[clsInd.as_usize() + 1] = val as TULit;
		}
		else
		{
			self.CompressedMismatch("EClsGetStandardActivityAsIntPtr");
		}
	}

	#[inline(always)]
	pub(crate) fn EClsGetIsOversized(&self, clsInd: Ind) -> bool
	{
		if !Compress
		{
			if self.m_AnyOversized { self.ClsGetIsLearnt(clsInd) && self.EClsGetStandardActivityAsInt(clsInd) == ClsOversizeActivity } else { false }
		}
		else
		{
			self.CompressedMismatch("EClsGetIsOversized");
			false
		}
	}

	#[inline(always)]
	pub(crate) fn EClsGetIsOversizedAssumeLearnt(&self, clsInd: Ind) -> bool
	{
		if !Compress
		{
			debug_assert!(self.ClsGetIsLearnt(clsInd));
			if self.m_AnyOversized { self.EClsGetStandardActivityAsInt(clsInd) == ClsOversizeActivity } else { false }
		}
		else
		{
			self.CompressedMismatch("EClsGetIsOversizedAssumeLearnt");
			false
		}
	}

	#[inline(always)]
	pub(crate) fn ClsSetSize(&mut self, clsInd: Ind, sz: TUV)
	{
		if !Compress
		{
			debug_assert!((sz & ClsIsLearntMask) == 0);
			let cleaned = self.m_B[clsInd.as_usize()] & !(if !self.ClsGetIsLearnt(clsInd) || sz > ClsLearntMaxSizeWithGlue { ClsLearntMaxSizeWithoutGlue } else { ClsLearntMaxSizeWithGlue });
			self.m_B[clsInd.as_usize()] = cleaned | sz;

			if self.ClsGetIsLearnt(clsInd) && sz > ClsLearntMaxSizeWithGlue
			{
				self.m_AnyOversized = true;
				// Glue cannot be held with this size
				self.EClsSetStandardActivityAsInt(clsInd, ClsOversizeActivity);
			}
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			if bcInd.m_BitsForClsSize() == 0
			{
				debug_assert!(sz == BCMinClsSize as TUV);
				return;
			}
			let ba = self.BCGetBitArray(bcInd.GetHashId().val());
			ba.bit_set(BCClsSize2EncodedClsSize(sz as u64) as u64, bcInd.m_BitsForClsSize(), bcInd.m_BitStart());
		}
		debug_assert!(self.ClsGetSize(clsInd) == sz);
	}

	#[inline(always)]
	pub(crate) fn BCGetEncodedClsSize(&self, clsInd: Ind) -> TUV
	{
		let bcInd = TBCInd::from_ind(clsInd);
		debug_assert!(bcInd.m_BitsForClsSize() != 0);
		let bcHashInd = bcInd.GetHashId();
		let ba = self.BCGetBitArrayConst(bcHashInd.val());
		ba.bit_get(bcInd.m_BitStart(), bcInd.m_BitsForClsSize()) as TUV
	}

	#[inline(always)]
	pub(crate) fn ClsGetSize(&self, clsInd: Ind) -> TUV
	{
		if !Compress
		{
			if !self.ClsGetIsLearnt(clsInd) || self.EClsGetIsOversizedAssumeLearnt(clsInd)
			{
				self.m_B[clsInd.as_usize()] & !ClsIsLearntMask
			}
			else
			{
				self.m_B[clsInd.as_usize()] & !ClsIsLearntAndGlueMask
			}
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			if bcInd.m_BitsForClsSize() == 0
			{
				return BCMinClsSize as TUV;
			}
			CCls::<Ind, Compress>::compressed_cls_size(self.m_BC.at(bcInd.GetHashId().val()), bcInd) as TUV
		}
	}

	#[inline(always)]
	pub(crate) fn ClsSetGlue(&mut self, clsInd: Ind, mut glue: TUV)
	{
		debug_assert!(self.ClsGetIsLearnt(clsInd));

		if !Compress
		{
			if !self.EClsGetIsOversizedAssumeLearnt(clsInd)
			{
				if glue > ClsMaxGlue
				{
					glue = ClsMaxGlue;
				}
				self.m_B[clsInd.as_usize()] = (self.m_B[clsInd.as_usize()] & !ClsGlueMask) | (glue << ClsSizeBits);
				debug_assert!(self.ClsGetGlue(clsInd) == glue);
			}
			else
			{
				debug_assert!(self.ClsGetGlue(clsInd) == self.ClsGetSize(clsInd));
			}
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();

			if glue > bcHashInd.MaxGlue()
			{
				glue = bcHashInd.MaxGlue();
			}

			let ba = self.BCGetBitArray(bcHashInd.val());
			ba.bit_set(glue as u64, bcHashInd.GetBitsGlue(), bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64);
		}
	}

	#[inline(always)]
	pub(crate) fn ClsGetGlue(&self, clsInd: Ind) -> TUV
	{
		debug_assert!(self.ClsGetIsLearnt(clsInd));

		if !Compress
		{
			if !self.EClsGetIsOversizedAssumeLearnt(clsInd)
			{
				(self.m_B[clsInd.as_usize()] & ClsGlueMask) >> ClsSizeBits
			}
			else
			{
				self.ClsGetSize(clsInd)
			}
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArrayConst(bcHashInd.val());
			ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64, bcHashInd.GetBitsGlue()) as TUV
		}
	}

	#[inline(always)]
	pub(crate) fn EClsGetActivityAndSkipdelIndex(&self, clsInd: Ind) -> usize
	{
		clsInd.as_usize() + 1 + self.EClsGetIsOversizedAssumeLearnt(clsInd) as usize
	}

	#[inline(always)]
	pub(crate) fn ClsGetActivityAndSkipdel(&self, clsInd: Ind) -> Ind
	{
		if !Compress
		{
			Ind::from_u32(self.m_B[self.EClsGetActivityAndSkipdelIndex(clsInd)])
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArrayConst(bcHashInd.val());
			Ind::from_u64(ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcHashInd.GetBitsGlue() as u64, TBCHashId::GetBitsActivityAndSkipDel()))
		}
	}

	#[inline(always)]
	pub(crate) fn ClsSetActivityAndSkipdelTo0(&mut self, clsInd: Ind)
	{
		debug_assert!(self.ClsGetIsLearnt(clsInd));

		if !Compress
		{
			let i = self.EClsGetActivityAndSkipdelIndex(clsInd);
			self.m_B[i] = 0;
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArray(bcHashInd.val());
			ba.bit_set(0, TBCHashId::GetBitsActivityAndSkipDel(), bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcHashInd.GetBitsGlue() as u64);
		}
	}

	#[inline(always)]
	pub(crate) fn ClsGetSkipdel(&self, clsInd: Ind) -> bool
	{
		if !Compress
		{
			self.m_B[self.EClsGetActivityAndSkipdelIndex(clsInd)] & ClsSkipdelMask != 0
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArrayConst(bcHashInd.val());
			ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcHashInd.GetBitsGlue() as u64, TBCHashId::GetBitsSkipDel()) != 0
		}
	}

	#[inline(always)]
	pub(crate) fn ClsSetActivity(&mut self, clsInd: Ind, a: f32)
	{
		debug_assert!(a >= 0.);
		if !Compress
		{
			let prevSkipDel = self.ClsGetSkipdel(clsInd);
			let updatedA: f32 = if prevSkipDel { -a } else { a };
			let i = self.EClsGetActivityAndSkipdelIndex(clsInd);
			self.m_B[i] = updatedA.to_bits();
			debug_assert!(prevSkipDel == self.ClsGetSkipdel(clsInd));
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArray(bcHashInd.val());
			ba.bit_set(a.to_bits() as u64, TBCHashId::GetBitsActivity(), bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcHashInd.GetBitsGlue() as u64 + TBCHashId::GetBitsSkipDel() as u64);
		}
		debug_assert!(self.ClsGetActivity(clsInd) == a);
	}

	#[inline(always)]
	pub(crate) fn ClsGetActivity(&self, clsInd: Ind) -> f32
	{
		if !Compress
		{
			f32::from_bits(self.m_B[self.EClsGetActivityAndSkipdelIndex(clsInd)]).abs()
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArrayConst(bcHashInd.val());
			let asd = ba.bit_get(bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcHashInd.GetBitsGlue() as u64 + TBCHashId::GetBitsSkipDel() as u64, TBCHashId::GetBitsActivity());
			f32::from_bits(asd as u32)
		}
	}

	#[inline(always)]
	pub(crate) fn ClsSetSkipdel(&mut self, clsInd: Ind, skipDel: bool)
	{
		if !Compress
		{
			#[cfg(debug_assertions)]
			let prevAct = self.ClsGetActivity(clsInd);
			let i = self.EClsGetActivityAndSkipdelIndex(clsInd);
			self.m_B[i] = (self.m_B[i] & ClsNotSkipdelMask) | ((skipDel as u32) << ClsLShiftSkipDel);
			#[cfg(debug_assertions)]
			debug_assert!(prevAct == self.ClsGetActivity(clsInd));
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			let ba = self.BCGetBitArray(bcHashInd.val());
			ba.bit_set(skipDel as u64, TBCHashId::GetBitsSkipDel(), bcInd.m_BitStart() + bcInd.m_BitsForClsSize() as u64 + bcHashInd.GetBitsGlue() as u64);
			debug_assert!(self.ClsGetSkipdel(clsInd) == skipDel);
		}
	}

	#[inline(always)]
	pub(crate) fn EClsLitsStartOffset(&self, isLearnt: bool, isOversized: bool) -> TUV
	{
		if Compress
		{
			self.CompressedMismatch("EClsLitsStartOffset");
			0
		}
		else
		{
			1 + ((isLearnt as TUV) << ClsActivityFieldsLshift) + ((isOversized as TUV) << ClsActivityFieldsLshift)
		}
	}

	#[inline(always)]
	pub(crate) fn ConstClsSpan(&self, clsInd: Ind, maxSpanElems: TUV) -> TSpanTULit
	{
		self.ConstClsSpanInternal(clsInd, maxSpanElems, &self.m_TmpClss, &self.m_CurrTmpClssCount)
	}

	#[inline(always)]
	pub(crate) fn ConstClsSpanDebug(&self, clsInd: Ind, maxSpanElems: TUV) -> TSpanTULit
	{
		self.ConstClsSpanInternal(clsInd, maxSpanElems, &self.m_TmpClssDebug, &self.m_CurrTmpClssCountDebug)
	}

	#[inline(always)]
	pub(crate) fn ConstClsSpanInternal(&self, clsInd: Ind, mut maxSpanElems: TUV, tmpClss: &UnsafeCell<[CDynArray<TULit>; m_TmpClssCountMax]>, tmpClssCount: &Cell<u8>) -> TSpanTULit
	{
		if !Compress
		{
			let sz = self.ClsGetSize(clsInd);
			if maxSpanElems > sz
			{
				maxSpanElems = sz;
			}
			let litsStartOffset = self.EClsLitsStartOffset(self.ClsGetIsLearnt(clsInd), self.EClsGetIsOversized(clsInd));
			Span::new(self.m_B.get_ptr(clsInd.as_usize() + litsStartOffset as usize), maxSpanElems as usize)
		}
		else
		{
			if clsInd == Ind::ZERO
			{
				return Span::new(self.m_B.get_ptr(0), maxSpanElems as usize);
			}
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();

			let cc = CCls::<Ind, Compress>::new_compressed(self.m_BC.at(bcHashInd.val()), bcInd);

			if maxSpanElems as usize > cc.size()
			{
				maxSpanElems = cc.size() as TUV;
			}

			// The temporary clauses are only accessed here, and the returned span is a raw view, precisely as in C++
			let tmpCls = unsafe { &mut (*tmpClss.get())[tmpClssCount.get() as usize] };
			// The C++ version doesn't test this allocation (and would crash, had it failed); the callers can't recover either,
			// so a failure aborts, as for a standard container, rather than writing through the null buffer
			tmpCls.reserve_exactly_unchecked_by_user(maxSpanElems as usize);
			if maxSpanElems != 0 && tmpCls.uninitialized_or_erroneous()
			{
				std::alloc::handle_alloc_error(std::alloc::Layout::array::<TULit>(maxSpanElems as usize).unwrap_or(std::alloc::Layout::new::<TULit>()));
			}
			for i in 0..maxSpanElems as usize
			{
				tmpCls[i] = cc.get(i);
			}

			let lswoc = Span::new(tmpCls.get_ptr_no_assert(0), maxSpanElems as usize);
			tmpClssCount.set(tmpClssCount.get() + 1);
			if tmpClssCount.get() as usize >= m_TmpClssCountMax
			{
				tmpClssCount.set(0);
			}
			lswoc
		}
	}

	#[inline(always)]
	pub(crate) fn Cls(&self, clsInd: Ind) -> CCls<Ind, Compress>
	{
		if !Compress
		{
			let isLearnt = self.m_B[clsInd.as_usize()] & ClsIsLearntMask != 0;
			let isOversized = self.m_AnyOversized && isLearnt && self.EClsGetStandardActivityAsInt(clsInd) == ClsOversizeActivity;
			let litsStartOffset = 1 + ((isLearnt as TUV) << ClsActivityFieldsLshift) + ((isOversized as TUV) << ClsActivityFieldsLshift);
			let clsSize = if !isLearnt || isOversized { self.m_B[clsInd.as_usize()] & !ClsIsLearntMask } else { self.m_B[clsInd.as_usize()] & !ClsIsLearntAndGlueMask };
			CCls::new_standard(clsInd, self.m_B.get_span_cap_sz(clsInd.as_usize() + litsStartOffset as usize, clsSize as usize))
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			CCls::new_compressed(self.m_BC.at(bcHashInd.val()), bcInd)
		}
	}

	// cls.Update(): must be invoked, if the size of the clause has been changed or the buffer has been reallocated
	#[inline(always)]
	pub(crate) fn ClsUpdate(&self, cls: &mut CCls<Ind, Compress>) -> bool
	{
		*cls = self.Cls(cls.cls_ind());
		true
	}

	// EXPLICIT (NON-COMPRESSED) CLAUSE BUFFER EXPLANATION:
	// To delete an initial clause [n l1 ... ln] we just put 0 instead of l2
	// To delete a learnt clause [{1<<ClsLShiftToMSB | n} Glue l1 ... ln] we replace the first entry by n+1 and put 0 instead of l2
	// This way, any deleted chunks > 1 is [n l1 0 ... ln], so one can walk clauses and deleted chunks in a similar manner,
	// e.g., ClsEnd(clsInd) works both when clsInd is a clause index and when it's a deleted chunk index
	// Any entry of size < 4 is considered to be deleted, including [0]
	// We also, optionally place newBinCls[0] at clsStart + 1 and newBinCls[1] at clsStart + 3.
	//		This trick is used to replace long parents with binary parents for assumption decision levels
	// COMPRESSED CLAUSE BUFFER EXPLANATION:
	// To delete, mark the first literal as 0
	// To store a binary clause, replace literals 1 and 2 with it.

	#[inline(always)]
	pub(crate) fn ClsChunkDeleted(&self, clsInd: Ind) -> bool
	{
		if !Compress
		{
			if self.ClsGetIsLearnt(clsInd)
			{
				return false;
			}
			let sz = self.ClsGetSize(clsInd);
			sz < 3 || self.m_B[clsInd.as_usize() + 2] == BadULit
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let ba = self.BCGetBitArrayConst(bcInd.GetHashId().val());
			if bcInd.m_BitsForClsSize() == 0
			{
				return ba.bit_get(bcInd.BitFirstLit(), bcInd.m_BitsForLit()) == 0;
			}

			ba.bit_get(bcInd.m_BitStart(), bcInd.m_BitsForClsSize()) == 0 || ba.bit_get(bcInd.BitFirstLit(), bcInd.m_BitsForLit()) == 0
		}
	}

	pub(crate) fn DeleteCls(&mut self, clsInd: Ind, newBinCls: Option<&[TULit; 2]>)
	{
		let cls = self.Cls(clsInd);

		if !Compress
		{
			if clsInd == self.m_FirstLearntClsInd
			{
				self.m_FirstLearntClsInd = self.ClsEnd(clsInd);
				while self.m_FirstLearntClsInd < self.m_BNext && self.ClsChunkDeleted(self.m_FirstLearntClsInd)
				{
					self.m_FirstLearntClsInd = self.ClsEnd(self.m_FirstLearntClsInd);
				}

				// Thought the following code a bug fix, but it only deteriorated the performance, so I brought back the old code
				// The code is not really necessary fopr correctness, since the solver never assumes m_FirstLearntClsInd *must* be learnt
				// Also, there is a loop like that after DeleteCls calls in DeleteClausesIfRequired, but not in the other DeleteCls instances
			}

			self.m_BWasted += Ind::from_usize(cls.size() + self.EClsLitsStartOffset(self.ClsGetIsLearnt(clsInd), self.EClsGetIsOversized(clsInd)) as usize);
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			self.m_BWasted += Ind::from_u64(bcHashInd.GetFirstLitOffset() as u64 + bcHashInd.m_BitsForLit() as u64 * self.ClsGetSize(clsInd) as u64);
		}

		self.m_Stat.DeleteClause(cls.size(), self.ClsGetIsLearnt(clsInd));
		for currWatchI in 0..=1
		{
			let clsWlInd = self.WLGetLongWatchInd(cls.get(currWatchI), clsInd);
			debug_assert!(clsWlInd != usize::MAX);
			self.WLRemoveLongWatch(cls.get(currWatchI), clsWlInd);
		}

		if !Compress
		{
			if self.ClsGetIsLearnt(clsInd)
			{
				self.m_B[clsInd.as_usize()] = cls.size() as TULit + self.EClsLitsStartOffset(self.ClsGetIsLearnt(clsInd), self.EClsGetIsOversized(clsInd)) as TULit - 1;
			}
			debug_assert!(!self.ClsGetIsLearnt(clsInd));

			self.m_B[clsInd.as_usize() + 2] = BadULit;
			if let Some(newBinCls) = newBinCls
			{
				self.m_B[clsInd.as_usize() + 1] = newBinCls[0];
				self.m_B[clsInd.as_usize() + 3] = newBinCls[1];
			}
		}
		else
		{
			let bcInd = TBCInd::from_ind(clsInd);
			let bcHashInd = bcInd.GetHashId();
			self.BCGetBitArray(bcHashInd.val()).bit_set(0, bcInd.m_BitsForLit(), bcInd.BitFirstLit());
			if let Some(newBinCls) = newBinCls
			{
				self.BCGetBitArray(bcHashInd.val()).bit_set(newBinCls[0] as u64, bcInd.m_BitsForLit(), bcInd.BitFirstLit() + bcInd.m_BitsForLit() as u64);
				self.BCGetBitArray(bcHashInd.val()).bit_set(newBinCls[1] as u64, bcInd.m_BitsForLit(), bcInd.BitFirstLit() + bcInd.m_BitsForLit() as u64 + bcInd.m_BitsForLit() as u64);
			}
		}
	}

	pub(crate) fn BCDeleteLitsCausesMoveToAnotherBuffer(&self, origSize: TUV, lits2Remove: TUV) -> bool
	{
		debug_assert!(origSize > 3);
		debug_assert!(lits2Remove < origSize);
		debug_assert!(origSize - lits2Remove >= 3);
		BCClsSize2Bits(origSize as u64) != BCClsSize2Bits((origSize - lits2Remove) as u64)
	}

	pub(crate) fn DeleteLitFromCls(&mut self, clsInd: &mut Ind, l: TULit)
	{
		let mut cls = self.Cls(*clsInd);
		debug_assert!(self.ClsGetSize(*clsInd) > 3);

		let it = cls.find(l);
		debug_assert!(it != cls.size());

		if it < 2
		{
			let myWatchInd = it != 0;
			let bestWLCandIt = self.FindBestWLCand(&mut cls, self.m_DecLevel);
			self.SwapWatch(*clsInd, myWatchInd, bestWLCandIt);
			self.WLSetCached(cls.get(!myWatchInd as usize), *clsInd, cls.get(myWatchInd as usize));
			cls.swap(bestWLCandIt, cls.size() - 1);
		}
		else
		{
			cls.swap(it, cls.size() - 1);
			// Cached update is required, since the cache might point at the deleted literal
			self.WLSetCached(cls.get(0), *clsInd, cls.get(1));
			self.WLSetCached(cls.get(1), *clsInd, cls.get(0));
		}

		if !Compress
		{
			let mut nextChunkInd = self.ClsEnd(*clsInd);
			if nextChunkInd < self.m_BNext && self.ClsChunkDeleted(nextChunkInd)
			{
				// Extending the deleted chunk
				let newSz = self.ClsGetSize(nextChunkInd) + 1;
				nextChunkInd -= Ind::ONE;
				self.m_B[nextChunkInd.as_usize()] = newSz;
				debug_assert!(cls.back() == newSz);
				if newSz >= 3
				{
					self.m_B[nextChunkInd.as_usize() + 2] = BadULit;
				}
				debug_assert!(self.ClsChunkDeleted(nextChunkInd));
				debug_assert!(self.ClsGetSize(nextChunkInd) == newSz);
			}
			else
			{
				cls.set_back(BadULit);
			}
			// Resizing our clause
			self.ClsSetSize(*clsInd, cls.size() as TUV - 1);
			self.RecordDeletedLitsFromCls(1, 0);
		}
		else
		{
			let (deletionHandled, _) = self.BCDeleteLitsByMovingToOtherBufferIfRequiredAssumingLastDeleted(clsInd, cls.size() as TUV, 1, false);
			if !deletionHandled
			{
				cls.set_back(BadULit);
				// Resizing our clause
				self.ClsSetSize(*clsInd, cls.size() as TUV - 1);
			}
			self.RecordDeletedLitsFromCls(1, TBCInd::from_ind(*clsInd).GetHashId().m_BitsForLit() as u16);
		}

		if self.IsCbLearntOrDrat()
		{
			let cls = self.ConstClsSpan(*clsInd, TUV::MAX);
			self.NewLearntClsApplyCbLearntDrat(cls);
		}
	}

	/*
	* WATCHES
	*/

	#[inline(always)]
	pub(crate) fn LastWLEntry(&self, l: TULit) -> usize
	{
		self.m_Watches[l as usize].m_WBInd.as_usize() + self.m_Watches[l as usize].GetLongEntries() - Self::LitsInInd
	}

	/*
	* DECISION LEVELS, ASSIGNMENTS, TRAIL
	*/

	// Get the decision variable of the given decision level
	#[inline(always)]
	pub(crate) fn GetDecVar(&self, mut decLevel: TUV) -> TUVar
	{
		debug_assert!(decLevel > 0 && decLevel <= self.m_DecLevel);
		// Skip any collapsed decision levels
		let mut prevDlLastVar = self.m_TrailLastVarPerDecLevel[decLevel as usize - 1];
		while decLevel > 0 &&
			{
				prevDlLastVar = self.m_TrailLastVarPerDecLevel[decLevel as usize - 1];
				prevDlLastVar == BadUVar
			}
		{
			decLevel -= 1;
		}
		if prevDlLastVar == BadUVar { self.m_TrailStart } else { self.m_VarInfo[prevDlLastVar as usize].m_TrailNext }
	}

	#[inline(always)]
	pub(crate) fn DecLevelIsCollapsed(&self, decLevel: TUV) -> bool
	{
		if decLevel == 0
		{
			return false;
		}
		debug_assert!(decLevel <= self.m_DecLevel);

		self.m_TrailLastVarPerDecLevel[decLevel as usize] == BadUVar
	}

	#[inline(always)]
	pub(crate) fn ToPropagatePushBack(&mut self, l: TULit)
	{
		self.m_ToPropagate.push_back(l);
	}

	#[inline(always)]
	pub(crate) fn ToPropagateBackAndPop(&mut self) -> TULit
	{
		self.m_ToPropagate.pop_back()
	}

	#[inline(always)]
	pub(crate) fn ToPropagateClear(&mut self)
	{
		self.m_ToPropagate.clear();
	}

	#[inline(always)]
	pub(crate) fn CiGetSpan(&self, ci: &mut TContradictionInfo<Ind>, maxSpanElems: TUV) -> TSpanTULit
	{
		if ci.m_IsContradictionInBinaryCls
		{
			if maxSpanElems >= 2
			{
				Span::new(ci.m_BinClause.as_mut_ptr(), 2)
			}
			else
			{
				Span::new(ci.m_BinClause.as_mut_ptr(), maxSpanElems as usize)
			}
		}
		else
		{
			self.ConstClsSpan(ci.m_ParentClsInd, maxSpanElems)
		}
	}

	#[inline(always)]
	pub(crate) fn CiGetSpanDebug(&self, ci: &mut TContradictionInfo<Ind>, maxSpanElems: TUV) -> TSpanTULit
	{
		if ci.m_IsContradictionInBinaryCls
		{
			self.CiGetSpan(ci, maxSpanElems)
		}
		else
		{
			self.ConstClsSpanDebug(ci.m_ParentClsInd, maxSpanElems)
		}
	}

	#[inline(always)]
	pub(crate) fn CiGetSize(&self, ci: &TContradictionInfo<Ind>) -> usize
	{
		if ci.m_IsContradictionInBinaryCls
		{
			2
		}
		else
		{
			self.ClsGetSize(ci.m_ParentClsInd) as usize
		}
	}

	pub(crate) fn CiString(&self, ci: &mut TContradictionInfo<Ind>) -> String
	{
		if !ci.m_IsContradiction
		{
			String::new()
		}
		else if ci.m_IsContradictionInBinaryCls
		{
			self.SLits(Span::new(ci.m_BinClause.as_mut_ptr(), 2), false)
		}
		else
		{
			self.SLits(self.CiGetSpanDebug(ci, TUV::MAX), false)
		}
	}

	pub(crate) fn CisString(&self, cis: Span<TContradictionInfo<Ind>>) -> String
	{
		let mut res = String::new();
		for ci in cis.as_mut_slice()
		{
			res = res + &self.CiString(ci) + "; ";
		}
		res
	}

	#[inline(always)]
	pub(crate) fn GetTrailPrevVar(&self, v: TUVar) -> TUVar
	{
		self.m_VarInfo[v as usize].m_TrailPrev
	}

	#[inline(always)]
	pub(crate) fn GetTrailNextVar(&self, v: TUVar) -> TUVar
	{
		self.m_VarInfo[v as usize].m_TrailNext
	}

	#[inline(always)]
	pub(crate) fn IsAssignedVar(&self, v: TUVar) -> bool
	{
		self.m_AssignmentInfo[v as usize].m_IsAssigned()
	}

	#[inline(always)]
	pub(crate) fn IsAssigned(&self, l: TULit) -> bool
	{
		self.IsAssignedVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn IsAssignedNegated(&self, l: TULit) -> bool
	{
		self.m_AssignmentInfo[GetVar(l) as usize].m_IsNegated() ^ IsNeg(l)
	}

	#[inline(always)]
	pub(crate) fn IsFalsified(&self, l: TULit) -> bool
	{
		let ai = self.m_AssignmentInfo[GetVar(l) as usize];
		ai.m_IsAssigned() && (ai.m_IsNegated() ^ IsNeg(l))
	}

	#[inline(always)]
	pub(crate) fn IsGloballyAssigned(&self, l: TULit) -> bool
	{
		self.GetAssignedDecLevel(l) == 0
	}

	#[inline(always)]
	pub(crate) fn IsGloballyFalsified(&self, l: TULit) -> bool
	{
		self.IsFalsified(l) && self.GetAssignedDecLevel(l) == 0
	}

	#[inline(always)]
	pub(crate) fn IsGloballySatisfied(&self, l: TULit) -> bool
	{
		self.IsSatisfied(l) && self.GetAssignedDecLevel(l) == 0
	}

	#[inline(always)]
	pub(crate) fn IsGloballyAssignedVar(&self, v: TUVar) -> bool
	{
		self.IsAssignedVar(v) && self.GetAssignedDecLevelVar(v) == 0
	}

	#[inline(always)]
	pub(crate) fn IsSatisfied(&self, l: TULit) -> bool
	{
		let ai = self.m_AssignmentInfo[GetVar(l) as usize];
		ai.m_IsAssigned() && ai.m_IsNegated() == IsNeg(l)
	}

	#[inline(always)]
	pub(crate) fn UnassignedOrSatisfied(&self, l: TULit) -> bool
	{
		let ai = self.m_AssignmentInfo[GetVar(l) as usize];
		!ai.m_IsAssigned() || ai.m_IsNegated() == IsNeg(l)
	}

	#[inline(always)]
	pub(crate) fn GetAssignedDecLevel(&self, l: TULit) -> TUV
	{
		debug_assert!(self.IsAssigned(l));
		self.m_VarInfo[GetVar(l) as usize].m_DecLevel
	}

	#[inline(always)]
	pub(crate) fn GetDecLevel0ForUnassigned(&self, l: TULit) -> TUV
	{
		if self.IsAssigned(l) { self.m_VarInfo[GetVar(l) as usize].m_DecLevel } else { 0 }
	}

	#[inline(always)]
	pub(crate) fn GetAssignedDecLevelVar(&self, v: TUVar) -> TUV
	{
		debug_assert!(self.IsAssignedVar(v));
		self.m_VarInfo[v as usize].m_DecLevel
	}

	#[inline(always)]
	pub(crate) fn GetAssignedParentClsInd(&self, l: TULit) -> Ind
	{
		debug_assert!(self.IsAssigned(l));
		self.m_VarInfo[GetVar(l) as usize].m_ParentClsInd
	}

	#[inline(always)]
	pub(crate) fn IsAssignedDec(&self, l: TULit) -> bool
	{
		self.IsAssignedDecVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn IsAssignedDecVar(&self, v: TUVar) -> bool
	{
		debug_assert!(self.IsAssignedVar(v));
		self.m_VarInfo[v as usize].IsDecVar()
	}

	#[inline(always)]
	pub(crate) fn IsAssignedAndDecVar(&self, v: TUVar) -> bool
	{
		self.IsAssignedVar(v) && self.m_VarInfo[v as usize].IsDecVar()
	}

	#[inline(always)]
	pub(crate) fn GetAssignedNonDecParentSpan(&self, l: TULit) -> TSpanTULit
	{
		self.GetAssignedNonDecParentSpanVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn GetAssignedNonDecParentSpanVar(&self, v: TUVar) -> TSpanTULit
	{
		debug_assert!(self.IsAssignedVar(v) && !self.IsAssignedDecVar(v));
		self.GetAssignedNonDecParentSpanVI(v)
	}

	// GetAssignedNonDecParentSpanVI(m_AssignmentInfo[v], m_VarInfo[v]): the C++ version receives the references to the entries of v
	#[inline(always)]
	pub(crate) fn GetAssignedNonDecParentSpanVI(&self, v: TUVar) -> TSpanTULit
	{
		if self.m_AssignmentInfo[v as usize].IsAssignedBinary()
		{
			// &m_BinOtherLit: TVarInfo is repr(C) and starts with m_ParentClsInd, whose low bytes hold m_BinOtherLit;
			// the raw cast avoids forming a mutable reference to the entry from &self
			Span::new(self.m_VarInfo.get_ptr(v as usize).cast::<TULit>(), 1)
		}
		else
		{
			self.ConstClsSpan(self.m_VarInfo[v as usize].m_ParentClsInd, TUV::MAX)
		}
	}

	#[inline(always)]
	pub(crate) fn IsParentLongInitial(&self, ai: TAssignmentInfo, vi: TVarInfo<Ind>) -> bool
	{
		!ai.IsAssignedBinary() && vi.m_ParentClsInd != Self::BadClsInd && !self.ClsGetIsLearnt(vi.m_ParentClsInd)
	}

	#[inline(always)]
	pub(crate) fn AdditionalAssign<S: TULitSpan>(&mut self, acSpan: S, acInd: Ind)
	{
		if acSpan.size() > 1 && self.IsAssigned(acSpan.get(0)) && !self.IsAssigned(acSpan.get(1))
		{
			acSpan.swap(0, 1);
		}

		if acSpan.size() > 0 && !self.IsAssigned(acSpan.get(0)) && (1..acSpan.size()).all(|i| self.IsFalsified(acSpan.get(i)))
		{
			self.m_Stat.m_FlippedClausesUnit += 1;
			self.Assign(acSpan.get(0), if acSpan.size() >= 2 { acInd } else { Self::BadClsInd }, if acSpan.size() == 1 { BadULit } else { acSpan.get(1) }, if acSpan.size() == 1 { 0 } else { self.GetAssignedDecLevel(acSpan.get(1)) }, true, false);
		}
	}

	// TULitSpan can be: (1) const span<TULit>, (2) TSpanTULit, (3) a clause; the iterator returned by the C++ version is replaced by an index
	#[inline(always)]
	pub(crate) fn GetAssignedLitsHighestDecLevelIt<S: TULitSpan>(&self, lits: S, startInd: usize) -> usize
	{
		max_element(lits, startInd, lits.size(), |l| self.GetAssignedDecLevel(l))
	}

	#[inline(always)]
	pub(crate) fn GetLitsHighestDecLevel0ForUnassignedIt<S: TULitSpan>(&self, lits: S, startInd: usize) -> usize
	{
		max_element(lits, startInd, lits.size(), |l| if self.IsAssigned(l) { self.GetAssignedDecLevel(l) } else { 0 })
	}

	#[inline(always)]
	pub(crate) fn GetAssignedLitsLowestDecLevelIt<S: TULitSpan>(&self, lits: S, startInd: usize) -> usize
	{
		min_element(lits, startInd, lits.size(), |l| self.GetAssignedDecLevel(l))
	}

	#[inline(always)]
	pub(crate) fn GetSatisfiedLitLowestDecLevelIt<S: TULitSpan>(&self, lits: S, startInd: usize) -> usize
	{
		std_min_element(lits, startInd, lits.size(), |l1, l2| (self.IsSatisfied(l1) && !self.IsSatisfied(l2)) || (self.IsSatisfied(l1) && self.IsSatisfied(l2) && self.GetAssignedDecLevel(l1) < self.GetAssignedDecLevel(l2)))
	}

	/*
	* Assumptions
	*/

	#[inline(always)]
	pub(crate) fn IsAssump(&self, l: TULit) -> bool
	{
		self.IsAssumpVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn IsAssumpVar(&self, v: TUVar) -> bool
	{
		self.m_AssignmentInfo[v as usize].m_IsAssump()
	}

	#[inline(always)]
	pub(crate) fn IsAssumpFalsifiedGivenVar(&self, v: TUVar) -> bool
	{
		debug_assert!(self.IsAssumpVar(v));
		self.IsFalsified(GetLit(v, false)) != self.m_AssignmentInfo[v as usize].m_IsAssumpNegated()
	}

	#[inline(always)]
	pub(crate) fn GetAssumpLitForVar(&self, v: TUVar) -> TULit
	{
		debug_assert!(self.IsAssumpVar(v));
		GetLit(v, self.m_AssignmentInfo[v as usize].m_IsAssumpNegated())
	}

	#[inline(always)]
	pub(crate) fn IsSatisfiedAssump(&self, v: TUVar) -> bool
	{
		self.IsAssumpVar(v) && self.IsSatisfied(self.GetAssumpLitForVar(v))
	}

	#[inline(always)]
	pub(crate) fn AssumpUnsatCoreCleanUpIfRequired(&mut self)
	{
		if self.m_Stat.m_SolveInvs == self.m_LatestAssumpUnsatCoreSolveInvocation
		{
			self.CleanVisited();
		}
	}

	/*
	* Decision Strategy
	*/

	#[inline(always)]
	pub(crate) fn IsForced(&self, v: TUVar) -> bool
	{
		self.m_PolarityInfoActivated && (v as usize) < self.m_PolarityInfo.cap() && self.m_PolarityInfo[v as usize].IsNextPolarityDetermined() && self.m_PhaseStage != TPhaseStage::PHASE_STAGE_DONT_FORCE
	}

	#[inline(always)]
	pub(crate) fn IsNotForced(&self, v: TUVar) -> bool
	{
		!self.m_PolarityInfoActivated || (v as usize) >= self.m_PolarityInfo.cap() || !self.m_PolarityInfo[v as usize].IsNextPolarityDetermined() || self.m_PhaseStage == TPhaseStage::PHASE_STAGE_DONT_FORCE
	}

	pub(crate) fn InitClssBoostScoreStrat(&self) -> u8
	{
		if self.m_QueryPrev == TQueryType::QUERY_NONE { self.m_Params.m_ParamInitClssBoostScoreStrat.get() } else { self.m_Params.m_ParamInitClssBoostScoreStratAfterInit.get() }
	}

	pub(crate) fn InitClssBoostScoreStratOn(&self) -> bool
	{
		self.InitClssBoostScoreStrat() > 0
	}

	pub(crate) fn InitClssBoostScoreStratIsReversedOrder(&self) -> bool
	{
		self.InitClssBoostScoreStrat() >= 3
	}

	pub(crate) fn InitClssBoostScoreStratIsClauseSizeAware(&self) -> bool
	{
		self.InitClssBoostScoreStrat() == 2 || self.InitClssBoostScoreStrat() == 4
	}

	/*
	* Conflict Analysis and Learning
	*/

	#[inline(always)]
	pub(crate) fn IsCbLearntOrDrat(&self) -> bool
	{
		self.M_CbNewLearntCls.is_some() || self.m_OpenedDratFile.is_some()
	}

	#[inline(always)]
	pub(crate) fn CleanVisited(&mut self)
	{
		let ai = &mut self.m_AssignmentInfo;
		self.m_VisitedVars.clear_apply(|v| ai[v as usize].set_m_Visit(false));
	}

	#[inline(always)]
	pub(crate) fn MarkVisitedVar(&mut self, v: TUVar)
	{
		if !self.IsVisitedVar(v)
		{
			self.m_VisitedVars.push_back(v);
			self.m_AssignmentInfo[v as usize].set_m_Visit(true);
		}
	}

	#[inline(always)]
	pub(crate) fn MarkVisited(&mut self, l: TULit)
	{
		self.MarkVisitedVar(GetVar(l));
	}

	#[inline(always)]
	pub(crate) fn IsVisitedVar(&self, v: TUVar) -> bool
	{
		self.m_AssignmentInfo[v as usize].m_Visit()
	}

	#[inline(always)]
	pub(crate) fn IsVisited(&self, l: TULit) -> bool
	{
		self.IsVisitedVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn VisitedPopBack(&mut self) -> TUVar
	{
		let v = self.m_VisitedVars.back();
		self.m_AssignmentInfo[v as usize].set_m_Visit(false);
		self.m_VisitedVars.pop_back()
	}

	#[inline(always)]
	pub(crate) fn CleanRooted(&mut self)
	{
		let ai = &mut self.m_AssignmentInfo;
		self.m_RootedVars.clear_apply(|v| ai[v as usize].set_m_Root(false));
	}

	#[inline(always)]
	pub(crate) fn MarkRootedVar(&mut self, v: TUVar)
	{
		if !self.IsRootedVar(v)
		{
			self.m_RootedVars.push_back(v);
			self.m_AssignmentInfo[v as usize].set_m_Root(true);
		}
	}

	#[inline(always)]
	pub(crate) fn MarkRooted(&mut self, l: TULit)
	{
		self.MarkRootedVar(GetVar(l));
	}

	#[inline(always)]
	pub(crate) fn IsRootedVar(&self, v: TUVar) -> bool
	{
		self.m_AssignmentInfo[v as usize].m_Root()
	}

	#[inline(always)]
	pub(crate) fn IsRooted(&self, l: TULit) -> bool
	{
		self.IsRootedVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn RootedPopBack(&mut self) -> TUVar
	{
		let v = self.m_RootedVars.back();
		self.m_AssignmentInfo[v as usize].set_m_Root(false);
		self.m_RootedVars.pop_back()
	}

	// Get the # of decision levels (glue) in the clause
	pub(crate) fn GetGlueAndMarkCurrDecLevels<S: TULitSpan>(&mut self, cls: S) -> TUV
	{
		self.m_MarkedDecLevelsCounter = self.m_MarkedDecLevelsCounter.wrapping_add(1);
		if self.m_MarkedDecLevelsCounter < 0
		{
			self.m_DecLevelsLastAppearenceCounter.memset(0);
			self.m_MarkedDecLevelsCounter = 1;
		}

		let mut decLevels: TUV = 0;
		for l in cls.lits()
		{
			if self.m_Params.m_ParamAssumpsIgnoreInGlue.get() && self.IsAssump(l)
			{
				continue;
			}
			debug_assert!(self.IsAssigned(l));
			let decLevel = self.GetAssignedDecLevel(l);
			if self.m_DecLevelsLastAppearenceCounter[decLevel as usize] != self.m_MarkedDecLevelsCounter
			{
				decLevels += 1;
				self.m_DecLevelsLastAppearenceCounter[decLevel as usize] = self.m_MarkedDecLevelsCounter;
			}
		}

		decLevels
	}

	#[inline(always)]
	pub(crate) fn IsAssignedMarkedDecLevel(&self, l: TULit) -> bool
	{
		self.IsAssignedMarkedDecLevelVar(GetVar(l))
	}

	#[inline(always)]
	pub(crate) fn IsAssignedMarkedDecLevelVar(&self, v: TUVar) -> bool
	{
		debug_assert!(self.IsAssignedVar(v));
		self.m_DecLevelsLastAppearenceCounter[self.GetAssignedDecLevelVar(v) as usize] == self.m_MarkedDecLevelsCounter
	}

	// On-th-fly subsumption related

	#[inline(always)]
	pub(crate) fn IsOnTheFlySubsumptionContradictingOn(&self) -> bool
	{
		self.m_Params.m_ParamOnTheFlySubsumptionContradictingMinGlueToDisable.get() > 0 && self.m_Stat.m_Restarts >= self.m_Params.m_ParamOnTheFlySubsumptionContradictingFirstRestart.get() as u64
	}

	#[inline(always)]
	pub(crate) fn IsOnTheFlySubsumptionParentOn(&self) -> bool
	{
		self.m_Params.m_ParamOnTheFlySubsumptionParentMinGlueToDisable.get() > 0 && self.m_Stat.m_Restarts >= self.m_Params.m_ParamOnTheFlySubsumptionParentFirstRestart.get() as u64
	}

	/*
	* RESTARTS
	*/

	#[inline(always)]
	pub(crate) fn GetCurrUnforceRestartsFraction(&self) -> f64
	{
		if self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL
		{
			self.m_Params.m_ParamPhaseMngUnforceRestartsFractionN.get()
		}
		else if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT
		{
			self.m_Params.m_ParamPhaseMngUnforceRestartsFractionS.get()
		}
		else
		{
			self.m_Params.m_ParamPhaseMngUnforceRestartsFractionInit.get()
		}
	}

	/*
	* Compression, simplification, deletion,  (of the clause buffer, the watch buffer, the literals)
	*/

	// IMPORTANT: in compressed mode, returns the next bit in the current buffer (rather than the whole BC index, including, e.g., the 11 hash bits)
	#[inline(always)]
	pub(crate) fn ClsEnd(&self, clsInd: Ind) -> Ind
	{
		if Compress
		{
			let bcInd = TBCInd::from_ind(clsInd);
			if bcInd.m_BitsForClsSize() == 0
			{
				Ind::from_u64(bcInd.BitFirstLit() + bcInd.m_BitsForLit() as u64 * self.ClsGetSize(clsInd) as u64)
			}
			else
			{
				let bcHashInd = bcInd.GetHashId();
				let ba = self.BCGetBitArrayConst(bcHashInd.val());
				let bcClsSize = ba.bit_get(bcInd.m_BitStart(), bcInd.m_BitsForClsSize()) as u32;
				Ind::from_u64(if bcClsSize == 0 { bcInd.m_BitStart() + bcInd.m_BitsForLit() as u64 } else { bcInd.BitFirstLit() + bcInd.m_BitsForLit() as u64 * BCEncodedClsSize2ClsSizeConst(bcClsSize, bcInd.m_BitsForClsSize() as u16) as u64 })
			}
		}
		else
		{
			Ind::from_usize(clsInd.as_usize() + self.EClsLitsStartOffset(self.ClsGetIsLearnt(clsInd), self.EClsGetIsOversized(clsInd)) as usize + self.ClsGetSize(clsInd) as usize)
		}
	}

	#[inline(always)]
	pub(crate) fn WLChunkDeleted(&self, wlInd: Ind) -> bool
	{
		wlInd.as_usize() + 1 >= self.m_W.cap() || self.m_W[wlInd.as_usize() + 1] == BadULit
	}

	#[inline(always)]
	pub(crate) fn WLEnd(&self, wlInd: Ind) -> Ind
	{
		Ind::from_u64(wlInd.as_u64() + (1u64 << self.m_W[wlInd.as_usize()]))
	}

	#[inline(always)]
	pub(crate) fn ClsLoopCurrBCInd(&self) -> TBCInd<Ind>
	{
		let bcHashId = TBCHashId::from_val(self.m_ClsLoopCurrCompressedBA as u16);
		TBCInd::new(bcHashId, self.m_ClsLoopCurrCompressedBACurrBit)
	}

	#[inline(always)]
	pub(crate) fn ClsLoopCompressedNewBC(&mut self) -> Ind
	{
		while self.m_ClsLoopCurrCompressedBA != self.m_BC.end()
			&& ((self.m_ClsLoopCurrCompressedBACurrBit >= self.BCGetBitArrayConst(self.m_ClsLoopCurrCompressedBA as u16).bit_get_next_bit()) || (self.m_CurrLoopIsLearntOnly && !TBCHashId::from_val(self.m_ClsLoopCurrCompressedBA as u16).m_IsLearnt()))
		{
			self.m_ClsLoopCurrCompressedBA = self.m_BC.next(self.m_ClsLoopCurrCompressedBA);
		}

		if self.m_ClsLoopCurrCompressedBA == self.m_BC.end()
		{
			self.m_ClsLoopCurrCompressedBACurrBit = u64::MAX;
			return Ind::MAX;
		}

		self.ClsLoopCurrBCInd().ind()
	}

	#[inline(always)]
	pub(crate) fn ClsLoopFirst(&mut self, isLearntOnly: bool) -> Ind
	{
		self.m_CurrLoopIsLearntOnly = isLearntOnly;
		if Compress
		{
			self.m_ClsLoopCurrCompressedBA = self.m_BC.begin();
			self.m_ClsLoopCurrCompressedBACurrBit = 0;
			self.ClsLoopCompressedNewBC()
		}
		else
		{
			self.m_ClsLoopCurrStandardCls = if self.m_CurrLoopIsLearntOnly { self.m_FirstLearntClsInd } else { Ind::from_usize(LitsInPage) };
			// The C++ version reads m_B beyond m_BNext here, if there are no clauses to loop over (the result is then unused, since the loop is completed)
			self.m_ClsLoopNextStandardCls = if self.ClsLoopCompleted() { Self::BadClsInd } else { self.ClsEnd(self.m_ClsLoopCurrStandardCls) };
			self.m_ClsLoopCurrStandardCls
		}
	}

	#[inline(always)]
	pub(crate) fn ClsLoopNext(&mut self) -> Ind
	{
		if Compress
		{
			debug_assert!(!self.ClsLoopCompleted());
			self.m_ClsLoopCurrCompressedBACurrBit = self.ClsEnd(self.ClsLoopCurrBCInd().ind()).as_u64();
			let baNextBit = self.BCGetBitArrayConst(self.m_ClsLoopCurrCompressedBA as u16).bit_get_next_bit();
			if self.m_ClsLoopCurrCompressedBACurrBit >= baNextBit
			{
				self.m_ClsLoopCurrCompressedBA = self.m_BC.next(self.m_ClsLoopCurrCompressedBA);
				self.m_ClsLoopCurrCompressedBACurrBit = 0;
				self.ClsLoopCompressedNewBC()
			}
			else
			{
				self.ClsLoopCurrBCInd().ind()
			}
		}
		else
		{
			self.m_ClsLoopCurrStandardCls = self.m_ClsLoopNextStandardCls;
			self.m_ClsLoopNextStandardCls = if self.ClsLoopCompleted() { Self::BadClsInd } else { self.ClsEnd(self.m_ClsLoopCurrStandardCls) };
			self.m_ClsLoopCurrStandardCls
		}
	}

	#[inline(always)]
	pub(crate) fn ClsLoopCompleted(&self) -> bool
	{
		if Compress
		{
			self.m_ClsLoopCurrCompressedBACurrBit == u64::MAX
		}
		else
		{
			self.m_ClsLoopCurrStandardCls >= self.m_BNext
		}
	}

	#[inline(always)]
	pub(crate) fn ClsDeletionTrigger(&self) -> u64
	{
		self.m_Stat.m_ActiveLongLearntClss
	}

	#[inline(always)]
	pub(crate) fn GetGlueMinFreeze(&self) -> TUV
	{
		if self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL
		{
			self.m_Params.m_ParamClsDelLowMinGlueFreezeAi.get()
		}
		else if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT
		{
			self.m_Params.m_ParamClsDelLowMinGlueFreezeS.get()
		}
		else
		{
			self.m_Params.m_ParamClsDelLowMinGlueFreeze.get()
		}
	}

	/*
	* Phase management
	*/

	#[inline(always)]
	pub(crate) fn GetPhaseStageStr(&self) -> String
	{
		if self.m_PhaseStage == TPhaseStage::PHASE_STAGE_STANDARD { "Standard".to_string() } else { "Don't-force".to_string() }
	}

	/*
	* Debugging
	* The printing functions print out some useful info and return true (to be used inside assert statements)
	*/

	pub(crate) fn SLits<S: TULitSpan>(&self, tulitSpan: S, toExternal: bool) -> String
	{
		let mut ss = String::new();
		if toExternal && self.m_DratSortEveryClause
		{
			let mut externalLits: Vec<TLit> = tulitSpan.lits().map(|l| self.GetExternalLit(l)).collect();
			// std::sort isn't stable, but there are no two literals of the same variable in a clause, so the result is the same
			externalLits.sort_by_key(|l| if *l < 0 { -*l } else { *l });

			for l in externalLits
			{
				ss += &l.to_string();
				ss += " ";
			}
		}
		else
		{
			for l in tulitSpan.lits()
			{
				ss += &(if toExternal { self.GetExternalLit(l).to_string() } else { self.SLit(l) });
				ss += " ";
			}
		}

		// ss.str().substr(0, ss.str().size() - 1)
		ss.pop();
		ss
	}

	/*
	* Misc
	*/

	// Not-verbose: use inside asserts
	#[inline(always)]
	pub(crate) fn NV(&self, vl: u8) -> bool
	{
		self.m_Params.m_ParamVerbosity.get() <= vl || self.m_Stat.m_Conflicts < self.m_Params.m_ParamHeavyVerbosityStartConf.get() as u64
	}

	// The information on the main buffer(s), required for printing the statistics
	pub(crate) fn GetStatBufferInfo(&self) -> TStatBufferInfo
	{
		TStatBufferInfo
		{
			m_b_num: if Compress { self.m_BC.size() } else { 1 },
			m_b_cap: if Compress { self.BCCapacitySum() } else { self.m_B.cap() },
			m_b_size: if Compress { self.BCNextBitSum() / 64 + 1 } else { self.m_BNext.as_usize() },
			m_extra_string: self.GetMemoryLayout(),
		}
	}

	// m_Stat.StatStrShort()
	pub(crate) fn StatStrShort(&mut self, forcePrintingHead: bool) -> String
	{
		let bi = self.GetStatBufferInfo();
		self.m_Stat.StatStrShort(&bi, forcePrintingHead, true)
	}
}
