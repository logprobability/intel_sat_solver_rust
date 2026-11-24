# Intel(R) SAT Solver: the Rust fork

A one-to-one Rust port of the C++ solver in the parent directory. The fork keeps the structure, the identifiers, the data layouts, the parameters and the control flow
of the C++ solver: every C++ header and translation unit has a corresponding module, and classes, functions and data members retain their C++ names.
`PORTING.md` is the contract (file map, naming and signature rules) to follow when porting upstream changes.

## Building

    cargo build --release          # target/release/intel_sat_solver (the executable) and libintel_sat_solver.{rlib,a} (the library, incl. the IPASIR C API)
    scripts/build_pgo.sh           # optional: target/pgo/release/intel_sat_solver, the executable built with profile-guided optimization

The executable has the same command line, input format, output and exit codes as the C++ one (as built by the Makefile, that is, with `-DSKIP_ZLIB`),
so the scripts in `../scripts` work with it unchanged.

`scripts/build_pgo.sh` trains on `../regression_instances` in the three solver modes (the whole script takes under a minute) and needs `llvm-profdata` of rustc's LLVM version.
The profile changes only inlining and code layout, so the output and the proofs are the same as those of the default build. Its gain over the default build is small
(see "Performance" below), and a profile isn't portable across toolchains, so the default build doesn't use it.

The library is used through `intel_sat_solver::topor::CTopor<Ind, Compress>`, where the three instantiations of the C++ library are
`CTopor<u32, false>`, `CTopor<u64, false>` and `CTopor<u64, true>` (the literal type is always `i32`, as in the C++ library).

## Testing

    cargo test                               # the unit tests and the fuzz tests (they need debug assertions, so not --release)
    TOPOR_FUZZ_ITERS=200000 cargo test fuzz  # more fuzzing (the default is 2000 scenarios)
    cargo clippy --all-targets               # clean: the lints the C++ conventions conflict with are allowed by name in lib.rs and main.rs
    scripts/coverage.sh                      # line coverage with LLVM source-based coverage (see "Coverage" below)
    scripts/diff_vs_cpp.sh [dir with .cnf]   # differential testing against the C++ solver (outputs and DRAT proofs must be byte-identical)

* `src/tests/fuzzing.rs` is the fuzzing harness: it turns a byte string into an incremental scenario over the public API and verifies every answer of the solver:
  every model against all the clauses and the assumptions, and (for up to 12 variables) every UNSAT verdict, every unsatisfiable core and every learnt clause
  against the truth table of the formula. The harness serves the in-crate fuzz tests (`src/tests/fuzz.rs`, deterministic pseudo-random inputs, including memory-out
  injection and a delta-debugger: `cargo test fuzz_delta_debug -- --ignored --nocapture`) and the libFuzzer targets in `fuzz/` (`cargo +nightly fuzz run api`).
* `scripts/diff_vs_cpp.sh` builds a reference copy of the C++ solver and compares the complete outputs and the text DRAT proofs (that is, every learnt clause, in order)
  of the two solvers on `../regression_instances` (and on any extra directory of instances) in all the three solver modes.
  The `planner_*.cnf` instances there are solver calls of the satdb join planner, recorded with `TOPOR_DUMP_NAME`: they cover conflict thresholds (`oc`, short and normal
  incremental queries, a threshold just below the maximal `uint64_t`), polarity hints (`lf`) and unsatisfiable cores, which the other instances don't use.
  (The C++ scripts in `../scripts` only run the `ddtbug*` and `regr*` instances.)

## Performance

    scripts/perf_instr.sh <executable> <instance.cnf>... [-- solver parameters]             # instructions retired and cycles per instance (macOS, /usr/bin/time -l)
    scripts/bench_vs_cpp.py --exe cpp=<path> --exe rust=<path> [--repeats N] <instance.cnf>...   # interleaved CPU/wall times, verdicts and conflicts of several executables

`perf_instr.sh` also prints the exit code and the md5 of the output, which must not change between builds. The instructions are reproducible to within about 0.5%;
the cycles depend on the load of the machine, so take the minimum of several runs (`PERF_RUNS`) and interleave the executables which are compared.

Totals over six instances (random 3-SAT, pigeonhole and an incremental one; 0.4 to 3.5 seconds each) in solver mode 0, on which the solvers do identical work
(the same output). Apple M5, rustc 1.97, C++ built by the Makefile with clang `-O3`; the minimum of 5 interleaved runs per instance:

| executable | instructions | cycles | instructions vs C++ | cycles vs C++ |
|---|---|---|---|---|
| C++ | 88.59G | 39.93G | 1.000 | 1.000 |
| the fork before the performance work | 86.36G | 43.63G | 0.975 | 1.093 |
| the fork, `cargo build --release` | 70.88G | 36.48G | 0.800 | 0.914 |
| the fork, `scripts/build_pgo.sh` | 68.09G | 36.51G | 0.769 | 0.914 |

That is 18% fewer instructions and 16% fewer cycles than before, and the fork is faster than the C++ on each of the six instances (0.90 to 0.94 of its cycles).
Solver modes 1 and 2 gain too (about 14% and 6% of the cycles). The profile-guided build retires 4% fewer instructions than the default build, at the same number of cycles.

The changes are confined to how the same statements are compiled; none changes a data layout, the statements executed or their order
(`scripts/diff_vs_cpp.sh` still reports byte-identical outputs and proofs). `PORTING.md` ("Rust-side conventions") lists them, so that they are kept when the code around them is edited:

* `SwapCurrWatch` is forced inline into `BCP`, as clang does for the C++: out of line, the index and the pointer of the current watch, which it takes by reference,
  were kept in memory throughout BCP's long-watch loop (most of the gain).
* `WLPrepareArena` is split into an inlined test for the common case (the arena exists and has room) and an out-of-line rest.
* `Cls` reads the clause header once and tests its learnt bit before the solver's `m_AnyOversized` flag (two tests without side effects, in the other order in the C++).
* `LearnAndUpdateHeuristicsVisitCls` and two heap functions of `CVarScores` are inlined into their callers, as a profile-guided build does on its own.

## Coverage

`scripts/coverage.sh` runs all the tests under LLVM source-based coverage and fails if any line is neither executed nor explicitly marked in the sources.
Of the 10175 lines, the tests execute 10168. The rarest solver paths (e.g., a reimplication in BCP which satisfies the earliest falsified assumption,
while another assumption is falsified) are driven by tests on crafted solver states, which the fuzzer never produces. The other 7 lines carry the mark
`COVERAGE-NOT-REACHED` with its reason: they run only if the system allocator fails, which the test-only allocation-fault injection doesn't simulate,
or if a decompressor is on the PATH but can't be run. (The script also supports a `COVERAGE-UNREACHABLE` mark; no line carries it.)

The script fails on a stale mark too (a marked line which is executed). It has a second pass, which builds with the debug assertions off and runs only the `nda_*` tests:
the statement following a failing assertion (the C++ `assert(0); return false;` idiom) can execute only in such a build.

## Deviations from the C++ solver

The control flow is the same statement by statement. The differences are confined to the following:

* **Behavior the C++ leaves to the compiler/standard library.** The C++ solver itself behaves differently on different platforms in these three respects;
  `scripts/diff_vs_cpp.sh` pins them in a reference build of the C++ solver to compare against:
  * the order of equal elements after `std::sort` of the learnt clauses in clause deletion (the fork uses `sort_unstable_by` with the same predicate);
  * the iteration order of the `unordered_map` of bit-compressed clause buffers (solver mode 2). The fork uses a direct-index table by hash-id (11 bits), which is strictly faster,
    iterates in ascending hash-id order, and never invalidates iterators, so the spare map `m_BCSpare` and the last-accessed-buffer cache are not needed;
  * floating-point contraction of the VSIDS score updates (Rust never fuses multiply-add; e.g., clang on AArch64 does by default).
* **The bit arrays are one word longer.** Getting/setting an element of a bit array always accesses also the word after the one the element starts in,
  so with the C++ sizes an element in the last word reads and rewrites 8 bytes beyond the buffer (solver mode 2). The extra word makes that access legal.
* **Memory-out.** Allocation failures surface as `STATUS_ALLOC_FAILED` exactly where the C++ tests for them. In a few places where the C++ would dereference a null pointer
  after an untested allocation, the fork reports the memory-out instead (each place is commented in the code).
* **Explicit-only accessors invoked in compressed mode** (`EClsSetIsLearnt` & co.) set `STATUS_COMPRESSED_MISMATCH` without the preceding `assert(0)`.
* **Statistics callbacks and the heap's reference to the activity increment** are replaced by explicit parameters (`GetStatBufferInfo`, `increase_score(.., &mut inc)`),
  scope-exit lambdas by explicit exit code, and `std::function`/`ofstream*` by boxed closures/`Box<dyn Write>`.
* **Out-of-range `/mode/value`**: the C++ indexes the per-mode default arrays out of bounds; the fork reports the parameter error and leaves the other parameters intact.
* **The names of the dump files** (`TOPOR_DUMP_NAME`) also have the number of the dump in the process, after the process id. The address in the name doesn't tell
  apart the solvers created in the same second: in the fork it is that of the solver inside `CTopi::new`, before it's moved out, so every solver has the same one,
  and in the C++ a solver may reuse a freed one; their dumps would overwrite each other.
* **Code that is unreachable by construction is replaced by assertions.** Where the C++ re-tests a condition that nothing since its previous test can change, handles
  a case that an assertion right above it excludes, or tests something that always holds (e.g., `expectedVal != -1` for a `uint8_t`), the fork drops the branch and keeps
  the condition as a `debug_assert!`. The reachable control flow is unchanged (verified by the differential testing). The places: the `IsUnrecoverable` re-tests in
  `MinimizeClauseMinisat`; the parent membership tests of on-the-fly subsumption after its size test has passed; the binary-parent case of the subsumed parents in
  `ConflictAnalysisLoop`; the falsified-decision case of `HandleAssumptions`; the `m_Assumps.cap() == 0` case after backtracking beyond the assumptions; the glue clamp of `ClsSetGlueStd`; the skipping of just-moved compressed clauses in `SimplifyIfRequired` (the ascending
  iteration over the direct-index table never revisits them); the `first != last` test in the `remove_if` helper; the early return of `WLPrepareArena`'s out-of-line part (its inlined part has just carried out the same test); the missing-DRAT-file case of the text-DRAT comment.
  Release builds don't check these conditions at all. (The unassigned-earliest-falsified-assumption error of `Solve` is kept as in the C++: it is reachable, when a stop requested
  from the new-learnt-clause callback interrupts conflict analysis.)
* **The C++ exceptions are panics.** The non-contagious bit-array class `CBitArraySingleElemWidth` (which the solver itself doesn't use) is the only C++ code that throws:
  `logic_error` from `ChangeElemWidth` and `bad_alloc` on an allocation failure. The port panics with the same messages.
* **Four headers aren't ported, since no translation unit includes them, and they aren't parts of the live code:** `BitArray.hpp` and `TableBitInfo.hpp` are older,
  diverged copies of `ToporBitArrayBuffer.hpp` and `ToporTableBitInfo.hpp` (e.g., their `BAGetElemAndGoToNext` advances the bit number before reading, and their class is
  named `CBitArray`), `TopiHandleNewUserCls.hpp` is an older copy of the `CHandleNewCls` class of `Topi.hpp` (ported in `topi_types.rs`), and `TopiGlobal.hpp` doesn't compile
  on its own (it uses `TLit` and `TUInd` without defining them). The live versions are ported, including the parts that the solver doesn't use (the non-contagious bit-array
  functions and classes of `ToporBitArrayBuffer.hpp` and the `tableByBit`/`tableByEntry` tables of `ToporTableBitInfo.hpp`, with all their static asserts).
* **The zlib input of `Main.cc` isn't ported:** the Makefile builds the C++ tool with `-DSKIP_ZLIB`, and the port implements that configuration (compressed inputs are read
  through the shell's decompressors, as the C++ tool does then).

## Rust-only extensions

Features without a C++ counterpart live in their own modules (`src/topi_ext_*.rs`) with small, marked hooks in the mirrored files. Each one is gated by a parameter,
which is off by default, and with the parameters off the solver's behavior, output and proofs are those of the C++ solver (`scripts/diff_vs_cpp.sh`).
A new parameter must also be listed in `IsParamExcluded` of `src/tests/fuzzing.rs`: the fuzz scenarios draw parameters by index, so a longer list changes all of them, including the ones
`scripts/coverage.sh` relies on to reach the rare paths. The extension gets its own tests instead (`src/tests/ext_hints.rs`, `src/tests/ext_target_phase.rs`, `src/tests/ext_vivify.rs`), and `TOPOR_FUZZ_EXTRA_PARAMS="<Param> <Val> ..." cargo test fuzz`
runs all the fuzz scenarios with the given parameters set first, that is, with the extension on.

**Phase and score hints** (`src/topi_ext_hints.rs`): advice for the search, which is never binding, so a wrong hint can only cost time.

* `CTopor::HintPhase(l)` seeds the saved polarity of the variable of `l` *without fixing it* (unlike `FixPolarity`): the solver decides the variable to `l` the next time,
  but stays free to flip it, and phase saving overwrites the hint as usual. A polarity fixed by `FixPolarity` wins for as long as it is in force. Requires `/phase/hints/on 1`.
* `CTopor::HintScore(v, score)` boosts the decision score of `v` by `/decision/score_hints/mult` times `score` at the next query, through the machinery of `BoostScore`.
  Ignored, if the parameter is 0 (the default).
* The hints are applied by the next `Solve` right before its search loop (if a hinted variable is still assigned
  above the assumptions by the previous query, the solver backtracks to the assumptions first, as a restart does).
  One hint of a kind is recorded per variable, and a newer hint replaces the one, which hasn't been applied yet, so hinting again before every query never grows the record beyond the number of the distinct variables hinted.
  A hint, which the next `Solve` can't apply, waits: a hint for a variable, which doesn't exist yet, waits for the variable; a phase hint for a variable held by the assumptions of the query
  (an assumption, or a variable they imply) waits for the first later query, in which the variable is free, and then overrides the phase saved in the meantime. A phase hint for a variable assigned at level 0 is dropped.
  In a sequence of queries an applied hint lasts only until phase saving overwrites it, so it is given again before the query it is meant for
  (`docs/search_quality/ceiling.md` measures what the hints can and cannot buy).
  `CTopor::GetSavedPhase(v)` and `CTopor::GetVarScore(v)` read the state to carry over; `CTopor::WriteHints()` / `ReadHints(text)` do it in the hints-file format.
* The executable: `/topor_tool/hints_file <file>` reads a hints file before the first query, `/topor_tool/write_hints_file <file>` writes one after every query
  (the saved phases, which are the model after a satisfiable query, and the ranks of the decision scores), and the commands `lh <Lit>` and `ls <Var> <Score>` give hints inside an instance
  (they are also what `TOPOR_DUMP_NAME` records). A hints file holds `v <Lit> ... 0` lines (phases, as in the model a SAT solver prints; the `v` is optional), `b <Var> <Score>` lines (scores) and comments.

**Target/best-phase saving and rephasing** (`src/topi_ext_target_phase.rs`): the phase oracle the solver generates for itself, as CaDiCaL and Kissat do.
Only the polarities of decisions change, so the answers, the models and the proofs stay correct.

* `/phase/target/on 1`: the solver records the assignment at the largest conflict-free trail since the latest rephase (the *target* phases) and since the latest rephase
  to the best phases (the *best* phases), and a decision takes the target phase of its variable in preference to the saved phase.
  `2`: only while the current restart strategy is the numeric one (arithmetic or Luby), which is this solver's counterpart of a stable mode (under LBD-based restarts the best phases are still tracked for rephasing).
  There is no alternation of the restart strategies within a query: the solver picks a strategy per query type, and switching in the middle of a query would need the LBD averages to be kept up under both.
* `/phase/rephase/conf_init N` (0: off): after N conflicts, at the next restart, the saved phases of the unassigned variables are reset: to the original phases (those of the start of the query,
  which include the phase hints), to their inversion, to the best phases, or they are flipped (`/phase/rephase/random 1`: random instead), in the order O I (B F B O B I)*; the target is reset.
  After every rephase the interval is multiplied by `/phase/rephase/conf_mult` (1) and incremented by `/phase/rephase/conf_incr` (1000), so the schedule is arithmetic, geometric or both.
* The existing phase management is in charge: a polarity fixed by `FixPolarity` or forced by `/phase/force_solution` wins over the target phase (but for the restarts, in which the phase stage
  doesn't force the polarities: `/phase/unforce_restarts_fraction_*`), and a rephase leaves the saved phase of such a variable alone. The target, best and original phases live in spare bits of `m_PolarityInfo`
  (`FixPolarity` on a variable drops them for that variable). Every query starts without a target, from the saved phases, that is, from the latest model or the hints.
* Statistics: `m_ExtRephases`, `m_ExtTargetPhaseUpdates` and `m_ExtTargetPhaseDecisions` of `GetStatistics()`; if the extension is on, the statistics lines are followed by
  `c ToporExt Rephases <N> TargetUpds <N> TargetDecs <N>`.
* Measurements and the recommended setting (`/phase/target/on 1 /phase/rephase/conf_init 1000` for hard satisfiable queries; off for refutations): `docs/search_quality/target_phase.md`.

**Vivification of learnt clauses** (`src/topi_ext_vivify.rs`): an inprocessing step, which shortens the long learnt clauses (the "vivification: see Kissat paper" of the C++ to-do list).
Every clause it adds is implied by the formula, so the answers, the models, the cores and the proofs stay correct.

* `/inprocessing/vivify/on 1` adds a vivification round to inprocessing, so it needs `/inprocessing/on 1`, and it runs on the schedule of inprocessing: at the start of every query
  and, with `/inprocessing/conflicts_before_next N`, at the first restart after every N conflicts (the C++ default of N is "never again in this query").
* A round backtracks to decision level 0 (the assumptions are taken off the trail and re-assigned after the round, as after a conflict below them) and tests the learnt clauses in the order of
  increasing glue and decreasing activity: the clause is detached from its watches (so it can't be the reason of its own shortening), the negations of its literals are decided one by one and propagated,
  and a contradiction, a literal of the clause implied by the previous ones, or literals falsified by propagation leave a clause over a subset of the literals, which replaces the clause
  (with the lower of the two glues and the activity of the old clause). A unit clause is propagated at level 0; a clause, which wasn't shortened, isn't tried again.
* `/inprocessing/vivify/effort F` (0.1): a round may propagate F times the literals, which the search has propagated since the previous round (the rounds of short queries are skipped, and their budget is carried over).
  `/inprocessing/vivify/max_glue G`: only the clauses of the glue G at most are candidates.
* The proof: the new clause is added before the old one is deleted (`d ...`), and the new-learnt-clause callback is given the new clause. Every deletion names a clause of the proof:
  while the extension is on, the DRAT file is also told, when simplification removes the literals falsified at level 0 from a learnt clause in place (the shortened clause is added, the original one is deleted;
  the C++ solver, and this one with the extension off, write nothing then). These are the only deletions the solver writes.
  The conflicts and the decisions of the statistics are those of the search; the saved phases, the user's polarities and the decision heap are not touched.
* A round ends before its next clause, when the query is asked to stop: an interrupt or a stop request of the StopNow callback makes `Solve` return right away, as between two decisions of the search,
  and a global or a local timeout is reported by the search loop right after the round. The memory of a round is held by the solver's own containers: a failed allocation gives `RET_MEM_OUT`, as anywhere else in the solver,
  and the record of the clauses, which were tried in vain, is bounded (2^20 hashes, 16 Mb).
* Statistics: `m_ExtVivifyRounds`, `m_ExtVivifyTried`, `m_ExtVivifyShortened`, `m_ExtVivifyLitsRemoved`, `m_ExtVivifyUnits` and `m_ExtVivifyPropagations` of `GetStatistics()`; if the extension is on, the statistics lines
  are followed by `c ToporExt VivifyRounds <N> Tried <N> Shortened <N> LitsRemoved <N> Units <N> Props <N> SearchProps <N>`.
* Measurements and the recommended setting: `docs/search_quality/vivification.md`.

**Search-quality measurements** (`scripts/search_quality_bench.py`, `scripts/phase_oracle_ceiling.py`, `scripts/target_phase_bench.py`, `scripts/vivify_bench.py`, `docs/search_quality/`):

    scripts/search_quality_bench.py --config base= --config hinted="/phase/hints/on 1 /topor_tool/hints_file hints/{inst}.hints" [--out PREFIX] <instance.cnf>...
    scripts/phase_oracle_ceiling.py          # the oracle-ceiling experiment; writes docs/search_quality/ceiling_tables.md (the reading is in ceiling.md)
    scripts/target_phase_bench.py            # target phases and rephasing versus the baseline; writes docs/search_quality/target_phase_tables.md (the reading is in target_phase.md)
    scripts/vivify_bench.py                  # vivification versus the baseline; writes docs/search_quality/vivification_tables.md (the reading is in vivification.md)
    scripts/tune_params.py [--loo] --tune <instance.cnf>... [--validate <instance.cnf>...] [--out FILE.md]   # the configurator: random search, then coordinate descent, PAR-2 on conflicts or time
    scripts/search_quality_final.py          # the baseline, every extension, all of them and the tuned configurations side by side; writes docs/search_quality/final_tables.md

`search_quality_bench.py` runs the release executable under named configurations (sets of parameters, the first one being the baseline) and tabulates the status, the conflicts,
the decisions, the restarts and the CPU time per instance and configuration, with the geometric means of the ratios to the baseline; it fails, if two configurations give contradicting answers.
`docs/search_quality/README.md` is the summary: what was built, how to use it, the final results and their caveats, and the recommended configuration (the defaults for the planner workload).

## Upstream issues found while testing the fork

The fuzzer found the following situations, in which the C++ solver fails an assertion in a debug build (and has undefined behavior in a release build).
They were confirmed with a debug build of the C++ solver; the fork reproduces them faithfully, and the fuzzer avoids them (see the "Upstream issue" comments in
`src/tests/fuzzing.rs` for the reproducing inputs in the format of the executable):

* changing the restart strategies of the query types individually, or to non-numeric ones between two queries when new variables are added;
* `Backtrack` through the API, or turning `/add_user_clause/guarantee_level_0` on, after a clause has been added since the latest query;
* `CreateInternalLit`/`BoostScore`/`FixPolarity`/`ClearUserPolarityInfo` on a new variable, followed by adding a clause before the next query (the watches of the new variable aren't reserved);
* `/debugging/assert_consistency` >= 1 with unsatisfiable-core queries, `/deletion/simplify_global_level_strat` != 0, `/assumptions/ignore_in_glue` = 1,
  `/restarts/lbd/thr_glue_val` = 0, `/bcp/init_entries_per_wl` not a power of two, raising `/inprocessing/postpone_first_inv_conflicts` after inprocessing has run;
* the `m_Assumps` assertions of `SimplifyIfRequired` read uninitialized entries (debug builds only);
* the out-of-bounds access of the bit arrays in solver mode 2 (see above; fixed in the fork by the extra word);
* a stop requested from the new-learnt-clause callback takes effect in the middle of conflict analysis and may drop a contradiction, so the solver shouldn't be reused after it:
  a later query may return wrong answers, fail assertions, or keep opening new decision levels beyond the capacity of the per-level arrays and crash (confirmed on the C++ library
  with the same inputs: identical decision trace, then SIGBUS; the C++ and the Rust builds just differ in when the out-of-bounds write hits unmapped memory);
* a DRAT write failure while a long clause is learnt makes `LearnAndUpdateHeuristics` read the glue of a non-existing clause;
* `DumpDrat`, `SetCbNewLearntCls` and `SetParallelData` must be invoked before the first clause is added (the internal-to-external map isn't maintained otherwise).
