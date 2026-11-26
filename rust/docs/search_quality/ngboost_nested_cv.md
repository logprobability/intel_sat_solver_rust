# NGBoost study, re-evaluated with nested ("double") cross-validation

A re-evaluation of the per-variable phase predictors of [`ngboost.md`](ngboost.md) (sections A to C and the follow-up of section 6) under nested cross-validation.
Script: `scripts/ngb_nested_cv.py` (stages `fit`, `solve`, `report`).

## Pre-registration

Written and committed before any result of this re-evaluation was computed. The protocol below is not changed afterwards; anything the data forces is recorded under "Deviations", with the reason.

### Why

The earlier study selected no hyperparameters, early-stopped on one validation split per fold (15, 6 and 1 validation instances for random 3-SAT, factoring/colouring and the planner),
added strategies after seeing held-out results, and took solving results from single deterministic runs (two apparent wins of section 6 vanished under perturbation).
This re-evaluation fixes the methods, grids, folds, seeds, strategies, metrics and decision rule in advance, selects every hyperparameter inside an inner loop that never sees the outer-test instances,
and requires every claimed solving difference to hold under three settings of an uninformative perturbation.

### Data (reused, not regenerated)

The instances, labels, features and backbone data of `ngb_data.py` as cached (`vars.npz`, `instances.json`): the "phase"-role instances of the three generated families.

| family | satisfiable (labelled) | of them >= 200 baseline conflicts | unsatisfiable |
|---|---:|---:|---:|
| random 3-SAT (n = 175, 200, 225; ratio 4.2) | 92 | 89 | 43 |
| factoring (semiprimes; primes) | 40 | 40 | 16 |
| 3-colouring (350 vertices, 2.30 edges per vertex) | 35 | 35 | 25 |

A sample is a variable of a satisfiable instance; its label is its value in the model found by the cached baseline run. Features: the 41 columns of the earlier "within" protocol (`ngb_common.FEATURES` without `lf_pol`).
Training uses the labelled (satisfiable) instances only; unsatisfiable instances are only ever test instances.

### Methods

| method | what | grid (inner selection) |
|---|---|---|
| `jw` | Jeroslow-Wang sign rule: p = jw_pos / (jw_pos + jw_neg); no fitting | none |
| `logreg` | `StandardScaler` + `LogisticRegression(max_iter=3000)` | C in {0.001, 0.01, 0.1, 1, 10, 100} (6 points) |
| `gbm` | `HistGradientBoostingClassifier(max_leaf_nodes=15, early_stopping=False, random_state=0)` | max_depth in {2, 4, 8} x learning_rate in {0.03, 0.1} (6 points); max_iter = argmin of the fold-averaged curve, at most 1000 (lr 0.03) / 400 (lr 0.1), at least 1 |
| `gbm_iso` | `gbm` at its chosen settings, then an isotonic map (`IsotonicRegression(y_min=0, y_max=1, out_of_bounds="clip")`) | none of its own; the map is fitted on the inner out-of-fold `gbm` predictions (the 4 inner `gbm` models refitted at the chosen settings and round count) |
| `ngb` | `NGBClassifier(Dist=Bernoulli, minibatch_frac=0.5, Base=DecisionTreeRegressor(criterion="friedman_mse", max_depth=d), random_state=0)` | d in {2, 3, 4} x learning_rate in {0.03, 0.1} (6 points); rounds = argmin of the fold-averaged curve, at most 400 (lr 0.03) / 200 (lr 0.1), at least 1 |
| `ngb_lr` | `ngb_lr.OffsetBernoulliNGB`: natural-gradient boosting of the logit from the per-sample offset of the logistic regression | d in {2, 3, 4} x learning rate in {0.03, 0.1} (6 points); rounds as for `ngb`, **0 allowed** (= `logreg`) |
| `ngb_stack` | `ngb` with the logistic-regression logit as a 42nd feature | as `ngb` |

Common to all tree methods: every fit (inner and final) uses at most 30000 training rows, a subsample drawn with `RandomState(0)` when there are more.

**Round selection.** Each inner fold of a boosting method is run to the cap of its learning rate with no per-fold early stopping, recording the validation log loss after every round
(for `ngb`/`ngb_stack`: ngboost's `evals_result["val"]`, which is a mean over the rows; for `gbm`: `staged_predict_proba`; for `ngb_lr`: its own curve, including round 0).
A curve which ends before the cap (ngboost's or `OffsetBernoulliNGB`'s gradient tolerance) is padded with its last value. The fold-averaged curve is the unweighted mean of the four per-fold mean-log-loss curves.
The chosen round count of a grid point is the argmin of that averaged curve, its score the minimum. A chosen count at the cap is reported as such.

**Selection score.** Per method, the grid point with the lowest mean inner log loss (unweighted mean over the 4 inner folds of their per-fold mean log loss; for boosting methods at the chosen round count). Ties: the first in grid order.
Then the method is refitted on all outer-training instances (row cap as above) with the chosen settings and round count.

**Offsets and the stacked feature.** The logistic-regression logits that `ngb_lr` and `ngb_stack` train on are cross-fitted by instance inside the outer-training set: each outer-training instance gets its logit from the inner-fold
logistic regression (at `logreg`'s inner-chosen C) whose validation fold it is in. These out-of-fold logits are used for both the inner-training and the inner-validation rows of the inner loop
(so the offset of an inner-training row may come from a logistic regression that saw the inner-validation instances; this is the accepted consequence of "no fourth nesting level"),
and for the final refit. The logits of the outer-test instances come from the logistic regression of all outer-training instances at the chosen C, i.e. exactly the `logreg` predictions.
Logits are clipped to +-log((1 - 1e-6) / 1e-6), as in `ngb_lr.py`.

### Folds and seeds

* **Outer loop:** per family, K = 5 folds by instance, repeated with 3 instance shuffles, seeds **101, 202, 303**. Per seed, the satisfiable instances and the unsatisfiable instances are permuted separately
  (`numpy.random.RandomState(seed).permutation`, satisfiable first, then unsatisfiable, from the same generator) and dealt round-robin to the 5 folds, so that every fold has 18-19 / 8 / 7 labelled instances and its share of unsatisfiable ones.
  Every instance (satisfiable, the 3 easy ones, unsatisfiable) is in exactly one outer-test fold per repeat, and its predictions in that repeat come from the models fitted on that fold's outer-training instances.
* **Inner loop:** the labelled outer-training instances, K = 4 folds by instance: permuted with `RandomState(1000 * seed + outer fold index)` and dealt round-robin.
* The variables of one instance are never split across any train / validation / test boundary (asserted in the script).

### Strategies (no others)

As `ngb_solve.py` builds the hint files, per method M and its probabilities p of the positive polarity:
* `M:all`: every variable hinted its more probable polarity (`/phase/hints/on 1`).
* `M:all+conf_x1`: the same phases and score hints 2|p - 0.5| + 0.001 with `/decision/score_hints/mult 1` (the confident variables are decided first).

### Solver runs

* Binary: a release build of this branch (built through the queue), used for every run of this re-evaluation, baselines included. No cached run from the earlier binary is paired with a new one.
  Before any solving, a sample of cached runs (30 baseline, 30 `logreg:all+conf_x1`, across the families) is rerun with the new binary and compared exactly (conflicts), and the outcome reported.
* Every instance x method x strategy x outer repeat runs unperturbed (flip 0) and with `/decision/polarity/flip_factor` **1000** and **2000**; the baseline (no hints) also runs at each of the three settings.
  Every comparison pairs runs at the same flip setting. Every run has `/topor_tool/verify_model 1`; timeout 120 s. A SAT-versus-UNSAT contradiction with the baseline is a blocker and is reported prominently.
* Runs are keyed by (instance, solver parameters, content hash of the hints file); identical keys are run once and shared (e.g. `jw` is the same in every repeat; an `ngb_lr` with 0 rounds gives the `logreg` hints).
* Expected count: 251 instances x 7 methods x 2 strategies x 3 flip settings x 3 repeats = 31626, less the deduplicated `jw` repeats (3012) and others, plus 753 baseline runs and the planner runs below: about 30000.
  **Reduction rule** (applied only if the estimate exceeds 40000 runs or 6 hours through the queue): drop outer repeats 202 and 303 for factoring, then for 3-colouring, in that order, recorded under Deviations.

### Metrics

**Primary.** Per family, for each method A in {jw, gbm, gbm_iso, ngb, ngb_lr, ngb_stack} against `logreg`, under `:all+conf_x1`, on the satisfiable instances with at least 200 conflicts in the **unperturbed** baseline (89 / 40 / 35; the same set at every flip setting):
the per-instance log ratio log((conflicts of A + 1) / (conflicts of logreg + 1)) at the same flip setting and repeat, **averaged over the 3 repeats per instance**, then
* the geometric-mean ratio exp(mean) with a 95% percentile bootstrap interval over instances (10000 resamples, `RandomState(0)`),
* wins / losses / ties (sign of the averaged log ratio),
* the two-sided Wilcoxon signed-rank p on the averaged log ratios, zeros dropped (p = 1 if fewer than 6 are non-zero).

A pair (instance, repeat) whose run of A or of `logreg` does not end with the baseline's status (timeout, error) is left out of that instance's average; an instance with no remaining pair is left out; the counts are reported.
Reported per flip setting (0, 1000, 2000), and the per-instance median over the three settings as a summary. Repeats are also reported separately (unaveraged), as a secondary table.

**Multiple comparisons.** Per family and per flip setting, the six primary p-values are Holm-adjusted.

**Decision rule.** Method A is declared different from `logreg` on a family only if, at **all three** flip settings, its Holm-adjusted p < 0.05 **and** the pooled bootstrap interval excludes 1, on the same side each time. Otherwise: not distinguishable.

**Secondary.**
* Calibration on the outer-test labelled variables: log loss, Brier, ECE (10 equal-width bins), accuracy; per repeat over all variables of the family, the mean over the 3 repeats reported, with the spread (min to max) over the 15 outer folds.
  The difference in log loss from `logreg` with a 95% bootstrap interval over instances (per-instance log loss averaged over the repeats).
* Conflicts ratio against the baseline (same flip setting), per method and strategy, per family, geometric mean with bootstrap interval, pooled over repeats as above.
* Unsatisfiable instances (and, separately, the 3 easy satisfiable ones): ratio against the baseline per method and strategy; "not hurt" is read as an interval that includes or is below 1.
* The per-fold chosen hyperparameters and round counts of every method (instability is a finding in itself).
* The `:all` strategy paired against `logreg:all` with the same statistics as the primary (secondary, not Holm-adjusted together with the primary).

### Planner (not nested)

Four instances cannot support nested cross-validation, so the planner gets no inner loop and no hypothesis test. Descriptive only, labelled as such:
* Leave-one-instance-out over the 4 `.plain` instances, with the per-variable features only (the earlier `within-pv` protocol: no instance-level features, no `lf_pol`).
* Each method's settings are **fixed** from the random 3-SAT inner loops: per method, the grid point chosen most often over the 15 random 3-SAT outer folds (ties: the lower mean selection score over the folds that chose it),
  with the round count the median (rounded down) of the round counts of the folds that chose that point. `gbm_iso`'s map is fitted on leave-one-instance-out predictions over the 3 training instances; `ngb_lr`'s and `ngb_stack`'s logits are cross-fitted over the 3 training instances.
* Calibration per method, and solving on `.plain` and `.lf` (single queries; file hints from the `.plain` predictions), both strategies, flip 0 / 1000 / 2000, each against the baseline and `logreg` at the same setting. No p-values.

### Expected outcome

`logreg` wins or ties under nested CV with wider intervals than the earlier study; if NGBoost variants improve only log loss and not paired conflicts, the conclusion is unchanged.

## Deviations

1. **Validation curves of `ngb`, `ngb_stack` and `ngb_lr` are computed with p clipped to [1e-6, 1 - 1e-6]** instead of from ngboost's `evals_result["val"]` (and `OffsetBernoulliNGB`'s own unclipped curve).
   Found in a smoke test on one 3-colouring fold with tiny round caps, before the real fit: ngboost's log score is unclipped, and on the near-deterministic variables of 3-colouring (isolated vertices) it reaches an infinite mean after a few rounds at learning rate 0.1,
   which makes the fold-averaged curve infinite from there on. The clipped log loss is the one used everywhere else (the inner scores of `logreg` and `gbm`, and the reported metric); where both are finite they agreed in the smoke test to every printed digit.
   The curves are computed from `staged_predict_proba` (ngboost) and from the trees and scales of `OffsetBernoulliNGB`.

## Results

(to be added after the runs)
