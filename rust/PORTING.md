# Porting conventions (C++ → Rust)

The Rust fork mirrors the C++ sources one-to-one. This file is the contract that keeps the modules consistent with one another and with upstream.

## File map

| C++ | Rust |
|---|---|
| `Topi.hpp` (class body, inline members) | `src/topi.rs`, `src/topi_types.rs` |
| `Topi.cc` | `src/topi_main.rs` |
| `TopiAsg.cc`, `TopiBacktrack.cc`, `TopiBcp.cc`, `TopiBitCompression.cc`, `TopiCompression.cc`, `TopiConflictAnalysis.cc`, `TopiDebugPrinting.cc`, `TopiDecision.cc`, `TopiInprocess.cc`, `TopiRestart.cc`, `TopiWL.cc` | `src/topi_asg.rs`, `src/topi_backtrack.rs`, `src/topi_bcp.rs`, `src/topi_bit_compression.rs`, `src/topi_compression.rs`, `src/topi_conflict_analysis.rs`, `src/topi_debug_printing.rs`, `src/topi_decision.rs`, `src/topi_inprocess.rs`, `src/topi_restart.rs`, `src/topi_wl.rs` |
| `Topor.hpp`, `Topor.cc` | `src/topor.rs` |
| `ToporIpasir.h`, `ToporIpasir.cc` | `src/topor_ipasir.rs` |
| `Main.cc` | `src/main.rs` |
| `ToporDynArray.hpp`, `ToporVector.hpp`, `ToporBitArray.hpp`, `ToporBitArrayBuffer.hpp`, `ToporTableBitInfo.hpp`, `ToporWinAverage.hpp`, `TopiVarScores.hpp`, `TopiParams.hpp`, `TopiStatistics.hpp`, `TimeMeasure.h`, `BasicMemoryUsage.h`, `ColorPrint.h`, `ToporExternalTypes.hpp` | the corresponding `snake_case.rs` |

Every `.cc` translation unit becomes one `impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress> { ... }` block in its module
(start the file with `use crate::topi::*;`). Functions appear in the same order as in the C++ file, with the same comments.

## Identifiers

* All identifiers are **identical to C++**: `m_DecLevel`, `WLAddLongWatch`, `TContradictionInfo`, `push_back`, `reserve_exactly`, ... Locals keep their C++ names too (`clsInd`, `currLongWatchPtr`).
* Template parameters: `TLit` is always `i32` (the C++ library only instantiates `int32_t`), `TULit`/`TUVar`/`TUV`/`TUL` are `u32`, `TUInd` is the generic `Ind: TUInd` (`u32` or `u64`), `Compress` is `const Compress: bool`.
* `if constexpr (Compress)` / `template <bool TCompress = Compress>` → plain `if Compress { .. } else { .. }` (resolved at monomorphization).
* Parameters: C++ `m_ParamFoo` → `self.m_Params.m_ParamFoo.get()`; the mode parameter `m_Mode` → `self.m_Params.m_ModeParam.get()`.
* Bit-fields: `x.m_IsAssigned` → `x.m_IsAssigned()`, `x.m_IsAssigned = v` → `x.set_m_IsAssigned(v)` (`TAssignmentInfo`, `TPolarityInfo`, `TBCHashId`, `TBCInd`).
* Unions: `vi.m_BinOtherLit` → `vi.m_BinOtherLit()` / `vi.set_m_BinOtherLit(l)` (shares storage with `m_ParentClsInd`); `m_ClsDelInfo.m_CurrChange` → `*self.m_ClsDelInfo.m_CurrChange()` (shares storage with `m_TriggerMax`).

## Signatures (mechanical rule)

A C++ member `R F(A a, B b = dflt)` becomes `pub(crate) fn F(&mut self, a: A, b: B) -> R` (`&self` iff the function mutates nothing; public API functions are `pub`):

* **No default arguments**: every caller passes every argument explicitly, in C++ order, using the C++ default value.
* `size_t`→`usize`, `uint64_t`→`u64`, `double`→`f64`, `float`→`f32`, `uint8_t`→`u8`, `uint16_t`→`u16`, `unsigned`→`u32`, `string` result→`String`, `const string&` parameter→`&str`.
* `span<TULit>` / `TSpanTULit` (by value) → `TSpanTULit` (= `Span<TULit>`, a raw `(ptr, len)` view exactly like `std::span`; `s[i]` read → `s.get(i)`/`s[i]`, write → `s.set(i, v)`, `swap(s[i], s[j])` → `s.swap(i, j)`, iteration → `for l in s`/`s.iter()`). `span<TLit>` (user literals) → `&[TLit]`.
* `TUInd& clsInd` → `clsInd: &mut Ind`; `bool* out` → `Option<&mut bool>`; `array<TULit, 2>* p` → `Option<&[TULit; 2]>`; `X& x` for a non-member → `&mut X`.
* `pair<A, B>` → `(A, B)`; `tuple` → tuple; `priority_queue<TUV>` → `std::collections::BinaryHeap<TUV>`; `vector<T>` → `Vec<T>`.
* `CCls::TIterator` → `usize` (an index into the clause; `cls.end()` is `cls.size()`); `CCls& cls` → `cls: &mut CCls<Ind, Compress>`.
* `TContradictionInfo`, `TWatchInfo`, `TVarInfo`, `TBCInd`, `TDelImpl`, `TParentSubsumed` are generic over `Ind`.
* `TULit* p` (raw pointer into `m_W`/`m_B`) → `*mut TULit`; reading/writing a `TUInd` stored in the literal buffer (`*(TUInd*)p`) → `Ind::read(p)` / `ind.write(p)`.
* A member passed by reference to another member (e.g. `m_Cis` as `CVector<TContradictionInfo>& cis`): keep the parameter out of the Rust signature and use `self.m_X` directly if the function is only ever called with that member; otherwise pass a raw pointer.

The explicit Rust signatures of the functions called across modules are listed below; anything not listed follows the mechanical rule.

```rust
// topi_main.rs (Topi.cc)
pub(crate) fn DumpSetUp(&mut self, filePrefix: &str);
pub(crate) fn ReadAnyParamsFromFile(&mut self);
pub fn SetParam(&mut self, paramName: &str, newVal: f64);
pub fn IsError(&self) -> bool;
pub(crate) fn StatusToRetVal(&self) -> TToporReturnVal;
pub(crate) fn UnrecStatusToRetVal(&self) -> TToporReturnVal;
pub(crate) fn HandleIncomingUserVar(&mut self, v: TLit, isUndoable: bool);
pub(crate) fn DumpSpan(&mut self, c: &[TLit], prefix: &str, suffix: &str, addNewLine: bool);
pub fn AddUserClause(&mut self, c: &[TLit]);
pub fn IsAssumptionRequired(&mut self, assumpInd: usize) -> bool;
pub(crate) fn AssignAssumptions(&mut self, firstUnassignedAssumpInd: usize);
pub(crate) fn FindFirstUnassignedAssumpIndex(&mut self, indexBeyondHandledAssumps: usize) -> usize;
pub(crate) fn HandleAssumptionsIfBacktrackedBeyondThem(&mut self);
pub(crate) fn HandleAssumptions(&mut self, userAssumps: &[TLit]);
pub fn Solve(&mut self, userAssumps: &[TLit], toInSecIsCpuTime: (f64, bool), confThr: u64) -> TToporReturnVal;
pub fn GetValue(&self, l: TLit) -> TToporLitVal;
pub fn GetLitDecLevel(&self, l: TLit) -> TLit;
pub fn GetSolveInvs(&self) -> u64;  pub fn GetMaxUserVar(&self) -> TLit;  pub fn GetMaxInternalVar(&self) -> TLit;
pub fn GetActiveLongLearntClss(&self) -> u64;  pub fn GetStatStrShort(&mut self, forcePrintingHead: bool) -> String;
pub fn GetConflictsNumber(&self) -> u64;  pub fn GetActiveClss(&self) -> u64;  pub fn GetBacktracks(&self) -> u64;
pub fn GetPropagations(&self) -> u64;  pub fn GetAssumpReuseBacktrackLevelsSaved(&self) -> u64;
pub fn GetModel(&self) -> Vec<TToporLitVal>;
pub fn GetStatistics(&self) -> &TToporStatistics;
pub fn GetParamsDescr(&self) -> String;
pub(crate) fn SetOverallTimeoutIfAny(&mut self);
pub(crate) fn PrintAxe(&mut self);
pub(crate) fn GetDecLevelWithBestScore(&mut self, dlLowestIncl: TUV, dlHighestExcl: TUV) -> TUV;
pub(crate) fn NewDecLevel(&mut self);
pub(crate) fn SetMultipliers(&mut self);
pub(crate) fn MarkWatchBufferChunkDeletedOrByLiteral(&mut self, wlbInd: Ind, allocatedEntries: Ind, l: TULit);
pub(crate) fn MarkWatchBufferChunkDeleted(&mut self, wi: &mut TWatchInfo<Ind>);   // wi may alias self.m_Watches[..]: pass a copy or a raw pointer deref
pub(crate) fn IsVisitedConsistent(&self) -> bool;
pub(crate) fn CalcMaxDecLevelScore(&self, dl: TUV) -> f64;  pub(crate) fn CalcMinDecLevelScore(&self, dl: TUV) -> f64;
pub(crate) fn GetMemoryLayout(&self) -> String;
pub fn SetParallelData(&mut self, threadId: u32, ReportUnitClause: Option<TReportUnitCls>, GetNextUnitClause: Option<TGetNextUnitClause>);

// topi_asg.rs
pub(crate) fn Assign(&mut self, l: TULit, parentClsInd: Ind, otherWatch: TULit, decLevel: TUV, toPropagate: bool, externalAssignment: bool) -> bool;
pub(crate) fn Unassign(&mut self, l: TULit);  pub(crate) fn UnassignVar(&mut self, v: TUVar);
pub(crate) fn DebugLongImplicationInvariantHolds(&mut self, l: TULit, decLevel: TUV, parentClsIndIfLong: Ind) -> bool;
pub(crate) fn TrailAssertConsistency(&mut self) -> bool;
pub(crate) fn FixPolarityInternal(&mut self, l: TULit, onlyOnce: bool);  pub(crate) fn ClearUserPolarityInfoInternal(&mut self, v: TUVar);
pub fn BoostScore(&mut self, vExternal: TLit, mult: f64);  pub fn FixPolarity(&mut self, lExternal: TLit, onlyOnce: bool);
pub fn CreateInternalLit(&mut self, l: TLit);  pub fn ClearUserPolarityInfo(&mut self, vExternal: TLit);

// topi_backtrack.rs
pub fn Backtrack(&mut self, decLevel: TLit, isBCPBacktrack: bool, isAPICall: bool);
pub(crate) fn BacktrackingInit(&mut self);

// topi_bcp.rs
pub(crate) fn SwapWatch(&mut self, clsInd: Ind, watchInd: bool, newWatchIt: usize);
pub(crate) fn SwapCurrWatch(&mut self, l: TULit, newWatchIt: usize, clsInd: Ind, cls: &mut CCls<Ind, Compress>, currLongWatchInd: &mut usize, currLongWatchPtr: &mut *mut TULit, wi: *mut TWatchInfo<Ind>);
pub(crate) fn FindBestWLCand(&mut self, cls: &mut CCls<Ind, Compress>, maxDecLevel: TUV) -> usize;
pub(crate) fn BCPBacktrack(&mut self, decLevel: TUV, eraseDecLevel: bool);
pub(crate) fn BCP(&mut self) -> TContradictionInfo<Ind>;
pub(crate) fn ProcessDelayedImplication(&mut self, diL: TULit, otherWatch: TULit, parentClsInd: Ind) -> bool;   // cis is always m_Cis

// topi_bit_compression.rs
pub(crate) fn BCDeleteLitsByMovingToOtherBufferIfRequiredAssumingLastDeleted(&mut self, clsInd: &mut Ind, origSize: TUV, lits2Remove: TUV, insertToSpareIfIteratorsBecomeInvalidated: bool) -> (bool, bool);
pub(crate) fn BCCompress(&mut self, cls: TSpanTULit, isLearnt: bool, glue: TUV, stayForAnotherRound: bool, activity: f32, insertToSpareIfIteratorsBecomeInvalidated: bool, spareUsedinCompressed: Option<&mut bool>) -> TBCInd<Ind>;
pub(crate) fn BCDecompress(&mut self, currHashId: TBCHashId, b: &mut u64);
pub(crate) fn BCIsIdenticalToDecompressed(&mut self, clsInd: Ind, currHashId: TBCHashId, bStart: u64) -> bool;
pub(crate) fn BCNextBitSum(&self) -> usize;  pub(crate) fn BCCapacitySum(&self) -> usize;

// topi_compression.rs
pub(crate) fn ReserveVarAndLitData(&mut self, maxAssumps: usize);
pub(crate) fn MoveVarAndLitData(&mut self, vFrom: TUVar, vTo: TUVar);  pub(crate) fn RemoveVarAndLitData(&mut self, v: TUVar);
pub(crate) fn RecordDeletedLitsFromCls(&mut self, litsNum: TUV, bitsForLit: u16);
pub(crate) fn DeleteBinaryCls(&mut self, binCls: TSpanTULit);
pub(crate) fn ClsDeletionInit(&mut self);  pub(crate) fn ClsDeletionDecayActivity(&mut self);
pub(crate) fn ClsDelNewLearntOrGlueUpdate(&mut self, clsInd: Ind, prevGlue: TUV);
pub(crate) fn DeleteClausesIfRequired(&mut self);  pub(crate) fn SimplifyIfRequired(&mut self);
pub(crate) fn DebugAssertWaste(&mut self) -> bool;  pub(crate) fn CompressWLs(&mut self);  pub(crate) fn CompressBuffersIfRequired(&mut self);

// topi_conflict_analysis.rs
pub(crate) fn AddClsToBufferAndWatch(&mut self, cls: TSpanTULit, isLearntNotForDeletion: bool, isPartOfProof: bool) -> Ind;
pub(crate) fn UpdateAllUipInfoAfterRestart(&mut self);
pub(crate) fn SizeWithoutDecLevel0(&self, cls: TSpanTULit) -> usize;
pub(crate) fn OnBadDratFile(&mut self);
pub(crate) fn NewLearntClsApplyCbLearntDrat(&mut self, learntCls: TSpanTULit);
pub(crate) fn CiIsLegal(&mut self, ci: &mut TContradictionInfo<Ind>, assertTwoLitsSameDecLevel: bool) -> bool;
pub(crate) fn ConflictAnalysisLoop(&mut self, contradictionInfo: &mut TContradictionInfo<Ind>);
pub(crate) fn MarkDecisionsInConeAsVisited(&mut self, triggeringLit: TULit);
// (MinimizeClauseBin, GenerateAllUipClause, MinimizeClauseMinisat, LearnAndUpdateHeuristics, RecordFlipped, GetDecLevelsAndMarkInHugeCounter are internal to the module;
//  LearnAndUpdateHeuristics and RecordFlipped move m_HandyLitsClearBefore[0] / [1] into a local with mem::take while in use, and the cls: *mut CVector
//  parameter of MinimizeClause* / GenerateAllUipClause points at that local, never into the solver)

// topi_debug_printing.rs
pub(crate) fn SVar(&self, v: TUVar) -> String;  pub(crate) fn SLit(&self, l: TULit) -> String;
pub(crate) fn STrail(&self) -> String;  pub(crate) fn SUserLits(&self, litSpan: &[TLit]) -> String;
pub(crate) fn SVars(&self, varSpan: TSpanTULit) -> String;  pub(crate) fn SE2I(&self) -> String;
pub(crate) fn P(&self, s: &str) -> bool;
pub(crate) fn PrintDebugModel(&mut self, trv: TToporReturnVal);  pub(crate) fn VerifyDebugModel(&mut self);

// topi_decision.rs
pub(crate) fn Decide(&mut self) -> TULit;
pub(crate) fn GetNextPolarityIsNegated(&mut self, v: TUVar) -> bool;
pub(crate) fn UpdateScoreVar(&mut self, v: TUVar, mult: f64);
pub(crate) fn UpdateDecisionStrategyOnNewConflict(&mut self, glueScoreOfLearnt: TUV, lowestGlueUpdateVar: TUVar, fakeTrailEnd: TUVar);
pub(crate) fn DecisionInit(&mut self);
pub fn ChangeConfigToGiven(&mut self, configNum: u16) -> String;

// topi_inprocess.rs
pub(crate) fn InprocessIfRequired(&mut self);  pub(crate) fn IngRemoveBinaryWatchesIfRequired(&mut self);

// topi_restart.rs
pub(crate) fn RstNewAssertingGluedCls(&mut self, glue: TUV);  pub(crate) fn Restart(&mut self) -> bool;
pub(crate) fn RestartInit(&mut self);  pub(crate) fn RestartLubySequence(y: f64, x: u64) -> f64;   // static

// topi_wl.rs
pub(crate) fn WLAddBinaryWatch(&mut self, l: TULit, otherWatch: TULit);  pub(crate) fn WLRemoveBinaryWatch(&mut self, l: TULit, otherWatch: TULit);
pub(crate) fn WLBinaryWatchExists(&mut self, l: TULit, otherWatch: TULit) -> bool;
pub(crate) fn WLAddLongWatch(&mut self, l: TULit, inlinedLit: TULit, clsInd: Ind);
pub(crate) fn WLRemoveLongWatch(&mut self, l: TULit, longWatchInd: usize);
pub(crate) fn WLGetLongWatchInd(&mut self, l: TULit, clsInd: Ind) -> usize;
pub(crate) fn WLSetCached(&mut self, l: TULit, clsInd: Ind, cachedLit: TULit);
pub(crate) fn WLReplaceInd(&mut self, l: TULit, clsInd: Ind, newClsInd: Ind);
pub(crate) fn WLPrepareArena(&mut self, l: TULit, allowNewBinaryWatch: bool, allowNewLongWatch: bool) -> *mut TULit;
pub(crate) fn WLAssertConsistency(&mut self, testMissedImplications: bool) -> bool;  pub(crate) fn WLAssertNoMissedImplications(&mut self) -> bool;
pub(crate) fn WLIsLitBetter(&self, lCand: TULit, lOther: TULit) -> bool;
```

Members already provided by `src/topi.rs` (everything defined inline in `Topi.hpp`), with the following adapted signatures:

* `Cls(clsInd) -> CCls<Ind, Compress>` (a `Copy` view: `cls.get(i)`, `cls.set(i, l)`, `cls.swap(i, j)`, `cls.size()`, `cls.back()`, `cls.set_back(l)`, `cls.find(l) -> usize`, `cls.lits()` iterator; `cls.Update()` → `self.ClsUpdate(&mut cls)`); in standard mode `cls.span()` gives the `TSpanTULit`.
* `ConstClsSpan(clsInd, maxSpanElems: TUV)` (pass `TUV::MAX` for the default), `ConstClsSpanDebug`, `ClsConstSnapshotDense(clsInd, maxLitsNum) -> CClsConstSnapshotDense` (`.span()`, `.get(i)`, `.size()`, `.iter()`).
* `SetStatus(&self, s, expl: String)`, `IsUnrecoverable()`, `NV(vl)`, `SLits(span_or_cls, toExternal: bool)`, `CiGetSpan(&mut ci, max)`, `CiString(&mut ci)`.
* `DeleteCls(clsInd, newBinCls: Option<&[TULit; 2]>)`, `DeleteLitFromCls(&mut clsInd, l)`, `AdditionalAssign(span, ind)`.
* `GetAssignedLitsHighestDecLevelIt(lits, startInd) -> usize` and friends return **indices**; free helpers `min_element`, `max_element`, `std_min_element`, `move_if` (in `topi_types.rs`) work on anything implementing `TULitSpan` and return indices.
* `GetAssignedNonDecParentSpanVI(v)` takes the variable (the C++ version takes `m_AssignmentInfo[v]`, `m_VarInfo[v]`).
* `BCRemoveGarbage(Option<&mut dyn FnMut(&mut Self, u64, u64)>)`; `RemoveGarbage(ctx, |c| &mut c.m_W, start, &mut end, &mut is_deleted, &mut chunk_end, notify)` is a free function in `topor_dyn_array.rs` taking the solver as `ctx`.
* `ReserveExactly!(self, self.m_X, newCap, "errSuffix")` and `ReserveExactlyInit!(self, self.m_X, newCap, initVal, "errSuffix")` macros replace the `ReserveExactly` member templates.
* `m_Stat.StatStrShort()` → `self.StatStrShort(forcePrintingHead)`; the heap's activity increment lives in `self.m_Stat.m_VarActivityInc` and is passed explicitly: `self.m_VsidsHeap.increase_score(v, mult, &mut self.m_Stat.m_VarActivityInc)`, `self.m_VsidsHeap.var_inc_update(decay, &mut self.m_Stat.m_VarActivityInc)`.
* `m_BC` is `TCompressedClss` (a direct-index table by hash-id): `at(i) -> *mut CBitArray`, `exists(i)`, `insert(i) -> *mut CBitArray` (null on allocation failure), `erase(i)`, `size()`, iteration `begin()/next(it)/end()` over hash-ids in ascending order. `BCGetBitArray(i) -> &mut CBitArray`. Insertion never invalidates iterators or pointers, so **`m_BCSpare` does not exist**: `insertToSpareIfIteratorsBecomeInvalidated` is kept in the signatures, the reported `spareUsed` is always `false`, and the C++ code that only runs when the spare was used is omitted.
* `m_OpenedDratFile: Option<Box<dyn Write>>`, `m_DumpFile: Option<File>`, callbacks are `Option<Box<dyn FnMut ..>>`; `m_InterruptNow` is an `Arc<AtomicBool>` (read with `load(Ordering::Relaxed)`).

## Semantics to preserve

* **Control flow is translated statement by statement.** No reordering, no "more idiomatic" restructuring of the algorithms, no replacement of the solver's data-structures by standard Rust collections.
* `assert(...)` → `debug_assert!(...)` (compiled out of release builds exactly as with `NDEBUG`), including the printing idiom `assert(NV(2) || P("..."))`.
* `unlikely(...)`/`likely(...)` hints are dropped.
* Unsigned arithmetic wraps as in C++ (`overflow-checks` is off in every profile); signed counters that rely on wrap-around use `wrapping_add`.
* `CApplyFuncOnExitFromScope` / `CSetInScope`: put the body of the function in a `let res = 'body: { .. }` labeled block (every `return x` of the body becomes `break 'body x`) and run the exit code right after it, preserving the order of the exit actions (C++ destructors run in reverse declaration order).
* Raw views (`Span`, `CCls`, `*mut TULit`) are exactly as fragile as their C++ counterparts: re-derive them wherever the C++ code re-derives them (after a reallocation of `m_B`/`m_W`).
* Allocation failures never panic: they surface through `uninitialized_or_erroneous()` and `STATUS_ALLOC_FAILED`, as in C++ (the one exception is the C++ `throw` of `CBitArraySingleElemWidth`, which becomes a `panic!` with the same message).
* Overloads: a C++ overload that differs by an extra argument gets the argument's name as a suffix (`reserve_exactly` / `reserve_exactly_init`); a compile-time template overload (`BAGetElem<startBitNum, elemWidth>(buf)`) gets the suffix `ConstExpr` and const generics (`BAGetElemConstExpr::<S, W>(buf)`), and its `static_assert`s become `const { assert!(..) }` blocks.
* `static_assert`s become `const _: () = assert!(..);` items in the same order, reading a `const` copy of the table they check.

## Rust-side conventions

These keep the port sound and lint-clean without changing its behavior; new code follows them too.

* Only the API modules are public (`topor`, `topor_external_types`, `topor_ipasir`, `color_print`, `std_string`); the solver internals are `pub(crate)`, since their raw views are as unchecked as in C++. `TUInd` is sealed (implemented for `u32` and `u64` only) and its width bound is asserted per implementation.
* A solver member must never be mutated through a raw pointer while `self` is borrowed (e.g., by a closure passed to `erase_if_may_reorder`): move the member out with `std::mem::take` for the duration and restore it afterwards (moving a `CVector`/`CDynArray` doesn't move its buffer, so spans stay valid). A watch-info that the callee may rewrite (e.g., across `CompressWLs`) is re-read by index rather than held as a pointer.
* `BCGetBitArray(&mut self, i) -> &mut CBitArray` / `BCGetBitArrayConst(&self, i) -> &CBitArray`: readers use the latter.
* Output to stdout from the library goes through `stdout_print!` / `stdout_println!` (they ignore a failed write, e.g., to a closed pipe, as C++ does), never `print!`.
* `rand()` is the safe wrapper of the C library's generator in `topi_types.rs` (the same random choices as C++); don't declare it again.
* Helpers for C++ idioms repeated across modules: `AssertConsistencyOn(level)` for `m_ParamAssertConsistency >= level && m_Stat.m_Conflicts >= m_ParamAssertConsistencyStartConf`, `WLBinWatchesSpan(&wi)` for the binary watches of a watch-info, `IsE2IInBounds(l)` before `E2I` on a literal that comes from the user.
* Allocations the C++ version doesn't check (and that the callers can't survive) abort through `std::alloc::handle_alloc_error` rather than writing through a null buffer.
* `cargo clippy --all-targets` is clean: the lints that the C++ naming and the statement-by-statement translation conflict with are allowed by name at the crate roots; anything else is fixed or allowed locally with a reason. `#[inline(always)]` is for one-line accessors; non-trivial functions get `#[inline]` at most. Add a `// SAFETY:` comment to every new or changed `unsafe` block.
* Measured performance exceptions to the rules above (each one changes how the same steps run, never which steps run or their order; keep them when editing the code around them):
  * `SwapCurrWatch` is `#[inline(always)]` (clang inlines it into `BCP` too): out of line, the watch index and pointer of `BCP`'s long-watch loop, which it takes by reference, live in memory instead of registers (about 10% of the cycles).
  * `WLPrepareArena` is an `#[inline(always)]` wrapper that carries out the first two tests of the C++ function (the arena exists and has room) and otherwise calls the `#[inline(never)]` `WLPrepareArenaSlow`, the C++ body with the early return for an arena with room replaced by a `debug_assert!` (the wrapper has just tested it). A change to the tests of the C++ function has to be made in both.
  * `Cls` reads the clause header once and tests its learnt bit before `m_AnyOversized` (the C++ `GetIsOversized` tests the flag first; neither test has a side effect), which lets the compiler decode the header without duplicating the code that follows per clause kind (about 4% of the cycles).
  * `LearnAndUpdateHeuristicsVisitCls` is `#[inline(always)]` (two call sites, both in the resolution loop of `LearnAndUpdateHeuristics`), and `CVarScores::reinsert_if_not_in_heap` / `remove_min` are `#[inline]` (one caller each: `UnassignVar` and `Decide`): the inlining decisions of a profile-guided build, which save about 0.5% of the instructions (the cycles change by less than the measurement noise).
  * When looking for more: the fork retires fewer instructions than the C++, so measure cycles as well as instructions (`scripts/perf_instr.sh`, interleaved with the previous build, same md5 of the output), and keep a change only if the gain exceeds the noise (about 1.5% of the cycles). A function which takes a loop's state by `&mut` forces that state into memory unless it is inlined; a rare path that is split off a hot function is `#[inline(never)]` (or `#[cold]`) and needs a test that reaches it, since `scripts/coverage.sh` counts its lines.
