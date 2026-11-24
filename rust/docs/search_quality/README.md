# Search quality: what was built, what it measured, what to do next

This directory documents the Rust-only search extensions of the port and their measurements. Search quality is measured in **conflicts** first (cumulative over the queries
of an instance), decisions and CPU time second.

**Bottom line.** Three extensions exist, all off by default; with them off the solver is still bit-identical to the C++ one (`scripts/diff_vs_cpp.sh`: 78 of 78 identical in each solver mode).
No configuration found here is a reliable improvement on the planner workload: every movement on the four recorded planner instances is within what an uninformative perturbation does,
and the configuration tuned on them does not survive leave-one-out. The extensions do pay on other kinds of queries: target phases on hard satisfiable formulas, vivification on long structured refutations,
and accurate phase hints wherever a model is nearly known. **Recommended configuration for the planner: the defaults.**

## What was built

| piece | where | parameters (all default to off) | reading |
|---|---|---|---|
| Phase and score hints | `src/topi_ext_hints.rs` | `/phase/hints/on`, `/decision/score_hints/mult` | [`ceiling.md`](ceiling.md) |
| Target/best phases and rephasing | `src/topi_ext_target_phase.rs` | `/phase/target/on`, `/phase/rephase/conf_init`, `conf_incr`, `conf_mult`, `random` | [`target_phase.md`](target_phase.md) |
| Vivification of learnt clauses | `src/topi_ext_vivify.rs` | `/inprocessing/vivify/on`, `effort`, `max_glue` (with `/inprocessing/on 1`) | [`vivification.md`](vivification.md) |
| Benchmark harness | `scripts/search_quality_bench.py` | | `rust/README.md` |
| Configurator | `scripts/tune_params.py` | | [`tuning_planner.md`](tuning_planner.md), [`tuning_regression.md`](tuning_regression.md) |
| Final comparison | `scripts/search_quality_final.py` | | [`final_tables.md`](final_tables.md) |

Every extension is a module of its own with small hooks marked `Rust-only extension` in the mirrored files; none adds a dependency. The tests are in `src/tests/ext_hints.rs`, `ext_target_phase.rs` and `ext_vivify.rs`.

## How to use each piece

A parameter is set on the command line as `<path> <value>` after the instance, or through the parameter API of the library.

**Hints** (advice, never binding: a wrong hint costs time only; a user-fixed polarity wins over a hint).

    intel_sat_solver q.cnf /phase/hints/on 1 /decision/score_hints/mult 10 /topor_tool/hints_file prev.hints    # read hints before the first query
    intel_sat_solver q.cnf /topor_tool/write_hints_file out.hints                                                # write the saved phases and the score ranks after every query

* Library: `HintPhase(lit)`, `HintScore(var, score)`, `GetSavedPhase(var)`, `GetVarScore(var)`, `ReadHints(text)`, `WriteHints()` on `CTopor`. In-instance commands: `lh <Lit>`, `ls <Var> <Score>`.
* File format: `v <Lit> ... 0` lines for phases, `b <Var> <Score>` lines for scores.
* Hints apply at the next `Solve` and are overwritten by phase saving, so in an incremental sequence they have to be renewed before every query. Renewing is free of side effects: the solver keeps one pending hint
  per variable (the latest). A phase hint for a variable, which the assumptions of the next query hold, is not lost: it is applied by the first later query, in which the variable is free, unless a newer hint replaces it.

**Target phases and rephasing.**

    /phase/target/on 1 /phase/rephase/conf_init 1000     # hard queries expected to be satisfiable
    /phase/rephase/conf_init 1000                        # the cautious variant, when refutations matter as well

**Vivification.**

    /inprocessing/on 1 /inprocessing/conflicts_before_next 5000 /inprocessing/vivify/on 1     # long structured refutations; add /inprocessing/vivify/effort 0.03 if CPU time matters more

**Scripts** (run from `rust/`, after `cargo build --release`; each one verifies every model and exits 1 on a SAT-versus-UNSAT contradiction).

    scripts/search_quality_bench.py --config base= --config x="<Param> <Val> ..." [--out PREFIX] <instance.cnf>...   # any configurations over any instances
    scripts/phase_oracle_ceiling.py | target_phase_bench.py | vivify_bench.py                                          # the experiment of each extension; regenerate the *_tables.md
    scripts/tune_params.py [--agg mean|geo] [--metric confs|cpu] [--loo] --tune <instance>... [--validate <instance>...] [--out FILE.md]
    scripts/search_quality_final.py [--config NAME="<Param> <Val> ..."]...                                             # writes final_tables.md

`tune_params.py` is random search, then coordinate descent, then a pass that drops every setting not worth `--min-gain`, over the parameters declared in `SPACE` at its top
(13 dimensions of parameters of the C++ solver, among them `/mode/value`, and the knobs of target phases, rephasing and vivification). The objective is PAR-2 on conflicts or on CPU time:
an instance the run does not decide as the defaults do costs twice the cap. `--loo` repeats the tuning without each instance in turn and scores it on the one left out.
The hint parameters are not in the space: they do nothing without advice from outside the solver.

## Results

### The final comparison ([`final_tables.md`](final_tables.md))

Release build, 100 instances, 9 configurations, CPU time the minimum of 3 runs. **Status agreement: every configuration gives the verdict of the baseline for every query of every instance
(78/78 regression instances, 22/22 generated formulas); no SAT-versus-UNSAT contradiction.** Cells: geometric mean of the conflicts ratio to the baseline (wins/losses in conflicts); second line: decisions ratio, CPU ratio.

| configuration | 78 regression instances | of them: 4 planner | 74 others (383 conflicts in total) | generated SAT (8) | generated UNSAT (14) |
|---|---|---|---|---|---|
| `hints` (own previous run; an upper bound, see below) | 0.788 (36/8)<br>0.928, 0.960 | 1.145 (2/2)<br>1.138, 0.988 | 0.773 (34/6)<br>0.918, 0.958 | 0.000 (8/0)<br>0.001, 0.006 | 0.961 (9/5)<br>0.959, 0.973 |
| `target` (target phases + rephase 1000) | 1.004 (6/5)<br>1.018, 0.922 | 1.166 (1/2)<br>1.208, 0.994 | 0.996 (5/3)<br>1.008, 0.918 | 0.894 (6/2)<br>0.891, 0.892 | 0.987 (7/7)<br>0.981, 1.046 |
| `vivify` (round every 5000 conflicts) | 1.005 (0/3)<br>1.008, 0.919 | 1.099 (0/3)<br>1.179, 0.947 | 1.000 (0/0)<br>1.000, 0.917 | 0.822 (3/4)<br>0.830, 0.793 | 0.954 (13/1)<br>0.951, 1.090 |
| `all` (`target` + `vivify`) | 0.996 (7/4)<br>1.012, 0.932 | 0.995 (2/1)<br>1.075, 0.834 | 0.996 (5/3)<br>1.008, 0.937 | 0.765 (6/2)<br>0.767, 0.828 | 0.926 (11/3)<br>0.923, 1.089 |
| `all+hints` | 0.785 (36/8)<br>0.924, 0.943 | 0.930 (3/1)<br>0.986, 0.886 | 0.778 (33/7)<br>0.921, 0.946 | 0.000 (8/0)<br>0.001, 0.006 | 0.900 (11/3)<br>0.896, 1.064 |
| `tuned:planner` | 0.869 (32/15)<br>1.014, 0.907 | 0.814 (3/1)<br>1.022, 0.669 | 0.872 (29/14)<br>1.014, 0.922 | 1.052 (4/4)<br>1.102, 1.091 | **1.403 (0/14)**<br>1.448, 1.679 |
| `tuned:regression` | 0.883 (25/12)<br>1.058, 0.924 | 1.211 (1/3)<br>1.285, 0.978 | 0.868 (24/9)<br>1.047, 0.921 | 0.945 (5/3)<br>0.940, 0.906 | 0.963 (8/6)<br>0.962, 0.986 |
| `flip1000` (control: a perturbation without information) | 1.011 (1/4)<br>1.006, 0.932 | 1.176 (1/2)<br>1.121, 0.910 | 1.003 (0/2)<br>1.000, 0.933 | 1.296 (2/6)<br>1.301, 1.277 | 0.984 (8/6)<br>0.981, 0.997 |

Total conflicts on the four planner instances: baseline 29281; `hints` 39306; `target` 35480; `vivify` 32764; `all` 30784; `all+hints` 28346; `tuned:planner` 21573; `tuned:regression` 38736; control 39062.

How to read it:

* **The control sets the noise level.** On the planner instances a change of no merit moves the conflicts by 18% (1.176), on the generated satisfiable formulas by 30%. Anything inside that band is not a result.
* **Planner: nothing helps reliably.** `hints`, `target`, `vivify` and `tuned:regression` are all at or above the baseline and inside the band of the control; `all` is 0.995. Only `tuned:planner` is below it, and that is the set it was fitted to (next section).
* **CPU ratios on the regression set are not informative.** These runs take 3 to 180 ms; every configuration, the control included, shows 0.91 to 0.96 on the 74 trivial instances, where most of them do not change the search at all. Only the generated formulas run long enough for the time to mean something.
* **`hints` is an oracle here.** The advice is the final phases and scores of a previous run of the same instance. On the generated satisfiable formulas that is the model, hence 0 conflicts; on the 74 trivial regression instances it saves a fifth of 383 conflicts. It shows what carrying state over can give when the next query resembles the previous one, not what switching a parameter on gives.
* **Vivification is the one consistent gain on fresh instances:** fewer conflicts on 13 of the 14 generated refutations (0.954), paid for with 9% more CPU time at these sizes (5 to 107 thousand conflicts; [`vivification.md`](vivification.md) finds the time gain on longer refutations).
* **Target phases:** 0.894 on the generated satisfiable formulas against 1.296 for the control, 6 better and 2 worse; neutral on refutations here (0.987).

### Per-family tuning ([`tuning_planner.md`](tuning_planner.md), [`tuning_regression.md`](tuning_regression.md))

| tuned on | objective | configuration found | on the tuned set | leave-one-out | on instances not tuned on |
|---|---|---|---|---|---|
| 4 planner instances | PAR-2, mean of conflicts | `/mode/value 1 /decision/vsids/init_order 0 /decision/vsids/init_order_ai 0 /decision/polarity/init_strategy 0 /backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50 /deletion/clause/frac_to_delete 0.5` | 0.737 (29281 to 21573 conflicts; 3 better, 1 worse) | **1.116** | 22 non-trivial ones: **1.263** (4 better, 18 worse); all 96: 0.949 |
| 78 regression instances | PAR-2, geometric mean of conflicts | `/decision/vsids/init_order 1 /decision/vsids/init_order_ai 1 /phase/force_solution 1` | 0.883 (25 better, 12 worse) | not run | 22 generated: 0.957 (13 better, 9 worse) |

* **The planner-tuned configuration is indicative only, and the indication is negative.** Tuned on three of the instances and applied to the fourth, the configurations take 1.743, 0.738, 0.960 and 1.257 of the baseline's conflicts: 1.116 in the geometric mean, worse than the defaults. On the generated refutations it is worse on 14 of 14. The 0.737 is a fit to four recordings.
* **The regression-tuned configuration fits the 74 trivial instances** (their conflicts go from 383 to 320) and is worse on all three larger planner instances (29281 to 38736 conflicts in total). Its 0.957 on the generated formulas is inside the band of the control.
* **Neither tuning selected a Rust-only extension.** The only extension parameter that appears anywhere is `/phase/rephase/conf_init 100` in one leave-one-out fold.
* Of the 80 random samples on the planner instances the median scores 1.117 and 16 beat the defaults: the defaults of the C++ solver are already a good point of this space for this family.

### From the experiments of the single extensions

* **Phase oracle** ([`ceiling.md`](ceiling.md)): on hard satisfiable random 3-SAT a fully correct phase hint gives 0 conflicts and the gain degrades gracefully (0.027 at 90% accuracy when the wrong hints are withheld; a predictor that commits on every variable needs about 90% accuracy). On the recorded planner instances the fully correct oracle changes nothing (69, 7562, 14926 unchanged; 6724 to 6629): the planner's `lf` fixes nearly every polarity, and most conflicts are in unsatisfiable or budget-limited queries.
* **Target phases** ([`target_phase.md`](target_phase.md)): 0.44 of the conflicts on 39 generated satisfiable random 3-SAT formulas, 1.06 on refutations (worse on 9 of 11), 1.23 on the planner instances against 1.24 for the control.
* **Vivification** ([`vivification.md`](vivification.md)): 0.878 of the conflicts and 0.939 of the CPU time on 20 structured refutations (19 better, 1 worse; most of it from factoring), 1.13 on the planner instances against 1.24 for the control.

## Caveats

* **The instance set is small and mostly trivial.** 74 of the 78 regression instances take 383 conflicts in total. The planner evidence is four recordings, whose final queries were appended for the regression suite; three of them carry all the weight. Everything else is generated: uniform random 3-SAT, factoring, 3-colouring, pigeonhole. No hard structured instance from a real workload was available.
* **The planner baseline is fragile.** A single uninformative change (the control, or one score boost on an arbitrary variable) moves its conflicts by tens of percent in either direction, and on single queries can take them to 0. Each instance and configuration was run once (the runs are deterministic), so there is no seed variance to average over; the control is the only yardstick.
* **What did not help:** every extension on the planner instances; correct phase advice on the planner instances; per-family tuning (it overfits); the family oracle (phases from another planner instance by variable index: sometimes 2372 to 66, sometimes 2939 to 10394); target phases on refutations; vivification on satisfiable or random formulas (within noise, more CPU time).
* **Not done:** stable/focused restart alternation; hints through IPASIR; vivification of original clauses.
* **Known limitations of the extensions**, none of which affects an answer:
  * a hint for a variable the solver never sees stays pending for the solver's lifetime (one entry per such variable, however often it is hinted); a phase hint for a variable assigned at level 0 is dropped, since the variable keeps its value;
  * `/phase/target/on 2` behaves as 1 under numeric restarts and as 0 under LBD restarts;
  * a vivification round is stopped between two clauses, not inside the test of a clause (a test is a handful of decisions with their propagation); the solver reads its clocks at every 1000th test of a timeout,
    in a round as in the search, so a timeout is noticed with that granularity;
  * the DRAT deletion lines are exact only if `/inprocessing/vivify/on` is set before the first query and stays set: while it is off, the solver (as the C++ one) doesn't tell the proof about the literals,
    which simplification removes from a clause in place, and a later deletion of such a clause names a clause the proof doesn't hold (a checker ignores it; the proof still verifies).
* **Fixed since the first version of the extensions** (the tests are in `src/tests/ext_hints.rs`, `ext_vivify.rs` and `cov_buffers.rs`):
  * a phase hint for a variable held by the assumptions was dropped: it now waits for the first query, in which the variable is free; re-hinting before every query grew the pending store without bound: there is now one entry per variable, the latest;
  * a vivification round could not be interrupted: it now serves the interrupt flag, the StopNow callback and both timeouts before every clause, with the solver at level 0 and every clause attached;
  * the bookkeeping of a round used standard containers, so a failed allocation aborted the process: it is now held by the solver's `CVector`/`CDynArray`, reserved before the round changes anything, and a failure yields `STATUS_ALLOC_FAILED` (`RET_MEM_OUT`);
    the set of the clauses tried in vain was a hash set of up to 2^22 entries: it is now an open-addressing table over a `CDynArray` of 2^20 entries (16 Mb) at most;
  * some deletion lines of vivified clauses named a clause the proof didn't hold (74 of 7085 on 11 generated instances, 5 of 16820 on the regression instances with a round after every conflict): the cause was not the round but `SimplifyIfRequired`,
    which removes the literals falsified at level 0 from a clause in place without writing to the proof (harmless in the C++ solver, which writes no deletions). With vivification on, the proof is now told
    (the shortened clause is added, the original deleted), and every deletion names a clause of the proof: 0 of 21118 and 0 of 16829 on the same instances, and `drat-trim` reports no ignored deletion. The proofs with the extension off are unchanged (byte-identical to C++);
  * `cargo test --release` failed (on `main` too) in 9 tests, which rely on the debug assertions of the solver: `tests::cov_buffers::bc_decompress` expected their panics, and 8 tests
    (`cov_core::alloc_failures_in_add_user_clause_and_solve`, `alloc_failures_more`, `contradiction_in_the_initial_bcp`, `wl_assert_consistency_failures`, `wl_direct` and `fuzz::fuzz_alloc_failures_mode_0/1/2`)
    drive the solver into states, which only an assertion stops (without them 5 of the 8 end in a segmentation fault, as the C++ solver would with `NDEBUG`). The helper of `bc_decompress` now expects a panic only where
    debug assertions exist, and the 8 tests (and the two memory-out sweeps of `ext_vivify.rs`, which are of the same kind) are ignored in a build without them. `cargo test --release` passes.
* **Proof checking** with the extensions on was done with drat-trim by the implementer of vivification (78 of 78) and with forward RUP checkers by the reviewers; after the fix of the deletion lines, the proofs of the 8 unsatisfiable
  ones of the 11 generated instances above were verified by `drat-trim` again (8 of 8, no warning), and the forward RUP checker of the tests now asserts that every deletion names a clause of the proof. Hints and target phases add or delete no clauses.
* The generic fuzz harness excludes the extension parameters (they would remap its seed-searched scenarios); the extensions are fuzzed by their own tests and by `TOPOR_FUZZ_EXTRA_PARAMS`.

## Recommended configuration

* **Planner workload: the defaults.** Do not use `tuned:planner`.
* Hard queries expected to be satisfiable: `/phase/target/on 1 /phase/rephase/conf_init 1000`.
* Long structured refutations (tens of thousands of conflicts and more): `/inprocessing/on 1 /inprocessing/conflicts_before_next 5000 /inprocessing/vivify/on 1`.
* A caller with a nearly known model (a previous similar query, a predictor that is right on about 90% of what it commits to): hints, renewed before every query.

## Recommended next steps

1. **Record more of the planner.** Dozens of real query sequences, with the queries the planner issued, would turn every "within noise" above into a measurement, and are the precondition for tuning: rerun `tune_params.py --loo` on them.
2. **Find where the planner's conflicts are** before adding search features: per query, split by answer (SAT, UNSAT, budget-limited). The oracle experiment says most of them are in queries no model can advise, which points at refutation-side work (learnt-clause quality and retention across queries, the conflict budgets themselves) and away from phases.
3. **Measure hint carry-over inside the planner**, where consecutive queries are related: with `lf` removed, re-hinting before every query took two of the recordings from 14466 to 11004 and from 5062 to 136 conflicts, but `lf` overrides hints, so this needs the planner to hint instead of fix some polarities.
4. **Average over perturbations** in the harness (several `flip_factor` or initial-order variants per configuration, median reported), so that fragile instances stop dominating single-run comparisons.
5. If vivification is to be used in a product setting: measure the cost of a round under many assumptions (they are taken off the trail and propagated again), and consider reusing the trail between consecutive clauses.
