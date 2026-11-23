# Intel(R) SAT Solver: the Rust fork

A one-to-one Rust port of the C++ solver in the parent directory. The fork keeps the structure, the identifiers, the data layouts, the parameters and the control flow
of the C++ solver: every C++ header and translation unit has a corresponding module, and classes, functions and data members retain their C++ names.
`PORTING.md` is the contract (file map, naming and signature rules) to follow when porting upstream changes.

## Building

    cargo build --release          # target/release/intel_sat_solver (the executable) and libintel_sat_solver.{rlib,a} (the library, incl. the IPASIR C API)

The executable has the same command line, input format, output and exit codes as the C++ one (as built by the Makefile, that is, with `-DSKIP_ZLIB`),
so the scripts in `../scripts` work with it unchanged.

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

## Coverage

`scripts/coverage.sh` runs all the tests under LLVM source-based coverage and fails if any line is neither executed nor explicitly marked in the sources.
All the 9354 lines are executed by the tests (100%). The rarest paths (e.g., a reimplication in BCP which satisfies the earliest falsified assumption,
while another assumption is falsified) are driven by tests on crafted solver states, which the fuzzer never produces. The script also supports the marks
`COVERAGE-UNREACHABLE` and `COVERAGE-NOT-REACHED` for lines, which aren't executed for a documented reason; no line carries them.

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
* **Code that is unreachable by construction is replaced by assertions.** Where the C++ re-tests a condition that nothing since its previous test can change, handles
  a case that an assertion right above it excludes, or tests something that always holds (e.g., `expectedVal != -1` for a `uint8_t`), the fork drops the branch and keeps
  the condition as a `debug_assert!`. The reachable control flow is unchanged (verified by the differential testing). The places: the `IsUnrecoverable` re-tests in
  `MinimizeClauseMinisat`; the parent membership tests of on-the-fly subsumption after its size test has passed; the binary-parent case of the subsumed parents in
  `ConflictAnalysisLoop`; the falsified-decision case of `HandleAssumptions`; the `m_Assumps.cap() == 0` case after backtracking beyond the assumptions; the glue clamp of `ClsSetGlueStd`; the skipping of just-moved compressed clauses in `SimplifyIfRequired` (the ascending
  iteration over the direct-index table never revisits them); the `first != last` test in the `remove_if` helper; the missing-DRAT-file case of the text-DRAT comment.
  Release builds don't check these conditions at all. (The unassigned-earliest-falsified-assumption error of `Solve` is kept as in the C++: it is reachable, when a stop requested
  from the new-learnt-clause callback interrupts conflict analysis.)
* Headers that no translation unit includes (`BitArray.hpp`, `TableBitInfo.hpp`, `TopiGlobal.hpp`, `TopiHandleNewUserCls.hpp`) and the unused non-contiguous bit-array
  classes of `ToporBitArrayBuffer.hpp` are not ported.

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
* a stop requested from the new-learnt-clause callback takes effect in the middle of conflict analysis and may drop a contradiction, so the solver shouldn't be reused after it;
* a DRAT write failure while a long clause is learnt makes `LearnAndUpdateHeuristics` read the glue of a non-existing clause;
* `DumpDrat`, `SetCbNewLearntCls` and `SetParallelData` must be invoked before the first clause is added (the internal-to-external map isn't maintained otherwise).
