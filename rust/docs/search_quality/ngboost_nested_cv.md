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
2. **Diverging ngboost fits.** In the first launch of the fit stage, an inner fit of `ngb_stack` on 3-colouring raised ngboost's "the fit diverged at iteration 138" (a logit overflows, the natural gradient is not finite) and stopped the stage
   (no result had been looked at; the 45 `logreg`, 45 `ngb_lr` and 16 `ngb_stack` fold files written before were kept, none of them affected). The protocol had no rule for this. Rule added:
   an inner fold whose fit diverges keeps the rounds fitted before the divergence, and its validation curve is infinite from the diverging round on, so the fold-averaged curve, and therefore the choice of rounds, stays before it;
   if the final refit of the chosen setting diverges, the next grid point by inner score is used (recorded per fold as `final_diverged`). The number of inner folds that diverged is stored per grid point.

## Results

All the generated tables (every method, strategy, flip setting, repeat and group) are in [`ngboost_nested_cv_tables.md`](ngboost_nested_cv_tables.md); below are the ones the answer rests on.

### Answer

* **Under nested CV no learnt predictor is distinguishable from the logistic regression in solving, on any family.** By the pre-registered decision rule (Holm-adjusted p < 0.05 and the interval on the same side of 1 at all three flip settings), none of `gbm`, `gbm_iso`, `ngb`, `ngb_lr`, `ngb_stack` differs from `logreg` under `:all+conf_x1` on random 3-SAT, factoring or 3-colouring.
* **Two claims pass the rule, both about the rule without learning, `jw`:** on random 3-SAT it needs **4.0 to 4.4 times** the conflicts of `logreg` (Holm p < 0.001 at every setting), and on 3-colouring **0.47 to 0.50 times** (Holm p 0.003 to 0.019).
  The second is the one place where the pre-registered expected outcome ("logreg wins or ties") does not hold. It is a property of the Jeroslow-Wang order on colouring, not of any fitted model, and is consistent with the post-hoc `jw:all+conf_x1` 0.605 of the earlier study (here 0.61 / 0.47 / 0.48 of the baseline).
* **The positive result of the earlier study is reproduced exactly where it was:** `logreg:all+conf_x1` takes 0.104 / 0.082 / 0.102 of the baseline's conflicts on the 89 random 3-SAT instances at flip 0 / 1000 / 2000 (earlier, single split, flip 0: 0.105), 68 to 80 better, 9 to 21 worse.
* **Hyperparameter selection does not close the gap of NGBoost to the logistic regression.** Tuned, plain `ngb` needs 1.62 to 1.69 times `logreg`'s conflicts on random 3-SAT (intervals 1.07 to 2.55, Holm p 0.16 to 0.44: not claimed); its log loss improves from the earlier 0.600 to 0.578, still 0.014 behind `logreg`.
* **`ngb_lr` is the logistic regression, now with a tight interval:** 0.968 / 0.988 / 0.990 of `logreg` (intervals 0.88-1.06, 0.89-1.09, 0.90-1.08) where the earlier single split gave 1.134 (0.90-1.43); log loss +0.0000 (-0.0001 to +0.0001). The `ngb_stack` "halved total" and the `ngb_lr` factoring "win" (0.603) of section 6 are gone (random 3-SAT 1.14 to 1.22; factoring `ngb_lr` 0.96 to 1.05).
* **Calibration:** the trees beat `logreg` on factoring log loss by about 0.01 (intervals exclude 0) and lose by 0.013 to 0.015 on random 3-SAT; on colouring everything is the base rate. None of these log-loss differences carries over to paired conflicts, so, as expected, **the conclusion of the earlier study is unchanged**: what helps is the confident-first order of a cheap predictor on random 3-SAT, and the logistic regression supplies it best at the lowest cost.
* **Intervals:** not uniformly wider than before. Averaging each instance over three repeats narrows the paired intervals of the near-identical predictors (`ngb_lr`), while those of the others stay as wide as before (roughly a factor of 1.5 to 2 either side on random 3-SAT).

### Binary provenance

The release binary was built from this branch (commit 6f24d81, the pre-registration; `rust/` sources unchanged since, built through the queue into a separate target directory).
A sample of 30 cached baseline runs and 30 cached `logreg:all+conf_x1` runs (10 instances per family, satisfiable and unsatisfiable, drawn with `RandomState(0)`) was rerun with it and with the earlier binary:
**all 120 runs reproduce the cached conflicts and statuses exactly**, so the hint-store change of 9bc53e8 (one pending entry per variable, deferred phase hints) does not affect these runs.
As pre-registered, every run of this re-evaluation was nevertheless made with the new binary, baselines included; nothing cached is paired with anything new.

### Runs

24219 distinct solver runs (32739 instance x repeat x method x strategy x flip cells, deduplicated by instance, parameters and hint content: `jw` is the same in every repeat, `ngb_lr` with 0 rounds gives the `logreg` hints),
about 35 minutes on 8 processes; 120 provenance runs. **0 SAT-versus-UNSAT contradictions, 0 timeouts or errors**; every model verified by the tool. No reduction of the run count was needed (pre-registered threshold 40000).
Fitting: about 12 CPU-hours over the 45 outer folds (`ngb_stack` 4.3, `ngb` 3.7, `ngb_lr` 2.9, `gbm` 0.9, `logreg` 0.03), on 8 processes through the queue.

### Calibration (outer-test variables)

Mean over the 3 repeats of the metric over all labelled outer-test variables; in brackets the range over the 15 outer folds. `d log loss`: minus `logreg`, pooled over the repeats, 95% bootstrap interval over instances.

| family | method | accuracy | log loss | ECE | d log loss vs logreg |
|---|---|---|---|---|---|
| random 3-SAT | jw | 0.659 | 0.6189 (0.613-0.623) | 0.041 | +0.0543 (+0.049 to +0.060) |
| | **logreg** | **0.704** | **0.5646** (0.554-0.578) | 0.011 | - |
| | gbm | 0.693 | 0.5777 (0.568-0.586) | 0.006 | +0.0131 (+0.011 to +0.015) |
| | gbm_iso | 0.693 | 0.5792 (0.569-0.587) | 0.004 | +0.0146 (+0.012 to +0.017) |
| | ngb | 0.693 | 0.5782 (0.569-0.587) | 0.008 | +0.0136 (+0.011 to +0.016) |
| | ngb_lr | 0.704 | 0.5646 (0.554-0.578) | 0.011 | +0.0000 (-0.0001 to +0.0001) |
| | ngb_stack | 0.704 | 0.5663 (0.557-0.577) | 0.009 | +0.0017 (+0.0007 to +0.0028) |
| factoring | jw | 0.633 | 0.6661 | 0.095 | +0.0384 (+0.026 to +0.051) |
| | logreg | 0.625 | 0.6277 (0.607-0.652) | 0.018 | - |
| | gbm | 0.636 | 0.6179 (0.593-0.653) | 0.012 | -0.0099 (-0.016 to -0.004) |
| | gbm_iso | 0.636 | 0.6190 | 0.007 | -0.0087 (-0.014 to -0.003) |
| | **ngb** | **0.637** | **0.6175** (0.594-0.654) | 0.010 | -0.0102 (-0.017 to -0.004) |
| | ngb_lr | 0.627 | 0.6245 | 0.018 | -0.0033 (-0.006 to -0.000) |
| | ngb_stack | 0.635 | 0.6192 | 0.011 | -0.0085 (-0.014 to -0.004) |
| 3-colouring | jw | 0.669 | 0.8262 | 0.214 | +0.1949 |
| | logreg / gbm / gbm_iso / ngb / ngb_lr / ngb_stack | 0.669 | 0.6311 to 0.6318 | 0.0004 to 0.0012 | -0.0002 to +0.0005 |

The fold spread of the log loss (0.02 to 0.06 on random 3-SAT and factoring) is larger than every difference between learnt methods; the paired intervals are what separates them.

### Primary: paired against `logreg`, `:all+conf_x1`

Geometric-mean ratio of conflicts to `logreg`'s (per-instance log ratio averaged over the 3 repeats), 95% bootstrap interval over instances, wins / losses / ties of A, Holm-adjusted Wilcoxon p (over the six methods, per family and flip setting).

| family | A | flip 0 | flip 1000 | flip 2000 | claimed |
|---|---|---|---|---|---|
| random 3-SAT (89) | jw | 4.03 (2.51-6.44) 22/67, Holm 0.000 | 3.98 (2.44-6.49) 23/66, Holm 0.000 | 4.39 (2.67-7.20) 23/66, Holm 0.000 | **worse** |
| | gbm | 1.47 (1.00-2.14) 39/50, Holm 0.41 | 1.54 (1.03-2.30) 38/51, Holm 0.42 | 1.52 (1.03-2.26) 37/52, Holm 0.45 | no |
| | gbm_iso | 1.43 (0.96-2.13) 44/45, Holm 0.41 | 1.59 (1.07-2.36) 42/47, Holm 0.34 | 1.47 (0.99-2.18) 42/47, Holm 0.45 | no |
| | ngb | 1.69 (1.12-2.55) 35/53, Holm 0.16 | 1.63 (1.08-2.43) 40/48, Holm 0.34 | 1.62 (1.07-2.45) 39/49, Holm 0.44 | no |
| | ngb_lr | 0.97 (0.88-1.06) 37/45/7, Holm 0.70 | 0.99 (0.89-1.09) 39/43/7, Holm 0.78 | 0.99 (0.90-1.08) 38/44/7, Holm 0.70 | no |
| | ngb_stack | 1.22 (1.02-1.47) 39/49, Holm 0.21 | 1.14 (0.95-1.37) 41/47, Holm 0.50 | 1.14 (0.94-1.39) 41/47, Holm 0.68 | no |
| factoring (40) | jw | 1.06 (0.78-1.41), Holm 1.00 | 1.05 (0.80-1.35), Holm 1.00 | 1.25 (0.89-1.71), Holm 0.30 | no |
| | gbm | 1.16 (0.98-1.37), Holm 0.90 | 0.96 (0.78-1.19), Holm 1.00 | 1.08 (0.87-1.34), Holm 1.00 | no |
| | gbm_iso | 1.14 (0.95-1.38), Holm 1.00 | 0.95 (0.79-1.15), Holm 1.00 | 1.23 (1.03-1.47), Holm 0.30 | no |
| | ngb | 0.95 (0.77-1.16), Holm 1.00 | 0.86 (0.69-1.06), Holm 1.00 | 0.97 (0.80-1.16), Holm 1.00 | no |
| | ngb_lr | 0.99 (0.86-1.13), Holm 1.00 | 1.05 (0.90-1.22), Holm 1.00 | 0.96 (0.83-1.11), Holm 1.00 | no |
| | ngb_stack | 1.00 (0.80-1.25), Holm 1.00 | 0.98 (0.82-1.19), Holm 1.00 | 0.99 (0.81-1.23), Holm 1.00 | no |
| 3-colouring (35) | jw | 0.49 (0.32-0.73) 24/11, Holm 0.011 | 0.50 (0.32-0.74) 24/11, Holm 0.019 | 0.47 (0.31-0.68) 26/9, Holm 0.003 | **better** |
| | gbm | 0.89 (0.76-1.03), Holm 0.28 | 0.97 (0.81-1.17), Holm 1.00 | 0.87 (0.73-1.02), Holm 0.20 | no |
| | gbm_iso | 0.86 (0.75-0.98), Holm 0.12 | 0.94 (0.79-1.12), Holm 1.00 | 0.82 (0.71-0.93), Holm 0.019 | no |
| | ngb | 0.77 (0.63-0.92), Holm 0.041 | 0.89 (0.72-1.10), Holm 1.00 | 0.73 (0.61-0.88), Holm 0.007 | no |
| | ngb_lr | 0.79 (0.68-0.90), Holm 0.028 | 0.82 (0.68-0.98), Holm 0.30 | 0.77 (0.63-0.94), Holm 0.063 | no |
| | ngb_stack | 0.77 (0.62-0.95), Holm 0.12 | 0.81 (0.69-0.94), Holm 0.024 | 0.77 (0.65-0.92), Holm 0.011 | no |

On 3-colouring the learnt methods are below `logreg` in all 15 cells (0.73 to 0.97), and two of them (`ngb`, `ngb_stack`) pass at two of the three settings. This is not claimed (the rule requires all three), and it has a plain reading:
`logreg:all+conf_x1` is itself *worse* than the baseline on colouring (1.23 / 0.95 / 1.03), while the others are near 1 (0.75 to 1.09); the comparison is "predictors of the base rate with different orders of near-ties", not a calibration effect.
On random 3-SAT the bootstrap intervals of `gbm`, `gbm_iso` and `ngb` exclude 1 at most settings while their Wilcoxon p does not survive Holm (and is 0.03 to 0.2 before it). These are consistent: the deficit of the geometric mean comes from a minority of instances with large ratios (wins / losses about 38 / 50), not from a uniform shift that a rank test would pick up.
For `jw` on colouring, "beats the baseline" is against the baseline only: as pre-registered, this protocol ran no uninformative controls (`perm_x1`, `rand_phase`) on the generated families. The controls of the earlier study on these colouring instances were at 0.83 to 1.23 of the baseline, and that is the context in which 0.47 to 0.61 stands out.
Repeats separately (generated tables, last primary table): on random 3-SAT `ngb` is 1.43 to 2.13 of `logreg` per repeat, `ngb_lr` 0.93 to 1.07; the repeats agree in direction for every method on random 3-SAT except `ngb_lr` (which straddles 1) and disagree in direction for most on factoring.

**Phases only (`:all` against `logreg:all`, secondary):** nothing on any family has a Holm p below 0.18; on random 3-SAT all six are 0.87 to 1.15 of `logreg`, and `ngb_lr:all` gives the `logreg` hints on 83 of 89 instances.

### Against the baseline (selected; same flip setting)

| group | configuration | flip 0 | flip 1000 | flip 2000 |
|---|---|---|---|---|
| random 3-SAT, sat. (89) | baseline at that flip vs unperturbed baseline | - | 1.27 | 1.02 |
| | `logreg:all+conf_x1` | 0.104 (0.06-0.17) 68/21 | 0.082 (0.05-0.13) 80/9 | 0.102 (0.06-0.16) 73/16 |
| | `ngb_lr:all+conf_x1` | 0.100 | 0.081 | 0.101 |
| | `ngb_stack:all+conf_x1` / `gbm` / `gbm_iso` / `ngb` | 0.126 / 0.152 / 0.148 / 0.175 | 0.094 / 0.126 / 0.130 / 0.133 | 0.117 / 0.156 / 0.150 / 0.166 |
| | `jw:all+conf_x1` | 0.417 | 0.326 | 0.449 |
| | `M:all` (the seven methods) | 0.69 to 0.81 | 0.55 to 0.64 | 0.73 to 0.89 |
| factoring, sat. (40) | every `M:all` and `M:all+conf_x1` | 0.76 to 0.93 | 0.66 to 1.03 | 0.82 to 1.07 |
| 3-colouring, sat. (35) | `jw:all+conf_x1` | 0.605 (0.39-0.92) | 0.473 (0.30-0.75) | 0.479 (0.30-0.76) |
| | `logreg:all+conf_x1` | 1.228 (0.94-1.60) | 0.953 | 1.031 |
| | every `M:all` | 1.34 to 1.40 (hurts) | 0.88 to 0.94 | 1.00 to 1.02 |
| unsatisfiable (43 / 16 / 25) | every configuration | 0.93 to 1.02 | 0.92 to 1.05 | 0.94 to 1.03 |
| satisfiable, < 200 conflicts (3, random 3-SAT) | phase hints | 0.85 to 24 (any hint can cost a factor) | | |

Unsatisfiable instances are not hurt: no configuration has an interval above 1 on any unsatisfiable group except `logreg:all` / `ngb_lr:all` at flip 1000 on random 3-SAT (1.028 / 1.030, interval lower end 1.00); `jw:all+conf_x1` is slightly *helpful* on unsatisfiable colouring (0.92 to 0.94, intervals below 1).
Factoring shows no effect of any advice beyond the baseline's own perturbation noise (flip 1000 / 2000 alone: 0.91 / 0.87).

### Chosen hyperparameters

Per outer fold (`depth / learning rate / rounds`; the full per-fold table is in the generated tables):

| family | logreg C | gbm | ngb | ngb_lr | ngb_stack |
|---|---|---|---|---|---|
| random 3-SAT | 1 in 11 of 15, 10 in 3, 100 in 1 | depth 2, 4 and 8, both rates; 39 to 999 rounds | depth 3-4, lr 0.03 in 15 of 15; **51 to 67 rounds** | depth 2-4, either rate; **0 to 18 rounds** (0 in 3, 1 in 4) | depth 2 in 14; lr 0.03 in 14; 56 to 78 rounds (and 20 at lr 0.1) |
| factoring | 0.1 to 100, every value but 0.001 / 0.01 | depth 2-8, both rates; 58 to 637 | depth 3-4, both rates; 35 to 319 | **0 to 376** (0 in 6 of 15) | depth 3-4, both rates; 15 to 177 |
| 3-colouring | 10 or 100 | depth 2 (14 of 15); 69 to 922 | depth 2-3; 19 to 122 | depth 2-3, lr 0.03 (14); 6 to 47 | depth 2-3; 17 to 100 |

* **The choices are unstable and do not matter.** The median margin between the best and the second-best grid point is 0.00001 to 0.0006 in inner log loss (logreg on random 3-SAT: 0.00001), so most choices are a coin toss between nearly equivalent settings; `gbm`'s round counts span an order of magnitude on every family.
* `ngb` on random 3-SAT is the stable exception (lr 0.03, 51 to 67 rounds, i.e. about 25 to 35 rounds at the earlier lr 0.05, matching the early-stopped `ngb_es` of section 6 at 28 to 46). The 300 rounds of the original study were too many; that correction stands.
* `ngb_lr` on random 3-SAT: the median inner gain of its chosen rounds over 0 rounds is 0.00008 in log loss. There is nothing for the booster to add to the logistic regression, as section 6 found, now without a single noisy split behind it.
* No chosen round count is at its cap. 6 inner `ngb_stack` fits diverged (5 random 3-SAT, 1 colouring, of 360 each; Deviations, 2); no final refit did.

### Planner (leave-one-out, fixed settings; descriptive)

Settings from the random 3-SAT inner loops: `logreg` C = 1 (11 of 15 folds); `gbm` depth 4, lr 0.03, 172 rounds (6 of 15); `ngb` depth 4, lr 0.03, 56 rounds (10 of 15); `ngb_lr` depth 2, lr 0.03, 2 rounds (6 of 15); `ngb_stack` depth 2, lr 0.03, 64 rounds (13 of 15). Per-variable features, the 4 `.plain` instances.

| method | accuracy | log loss | ECE |
|---|---:|---:|---:|
| majority polarity | 0.554 | 0.698 (section A) | 0 |
| jw | 0.477 | 0.734 | 0.136 |
| logreg / ngb_lr | 0.530 / 0.529 | 0.785 / 0.784 | 0.178 / 0.178 |
| gbm / gbm_iso | 0.568 / 0.532 | 0.686 / 0.684 | 0.057 / 0.065 |
| ngb / ngb_stack | 0.562 / 0.546 | 0.688 / 0.686 | 0.051 / 0.056 |

Geometric mean over the 4 instances, `:all+conf_x1`, against `logreg` at flip 0 / 1000 / 2000: on `.plain` `ngb` 0.28 / 0.36 / 0.31, `jw` 0.47 / 0.51 / 0.47, the others 0.76 to 2.0; on `.lf` everything 0.85 to 1.97.
Every hint configuration on `.plain` is far below the baseline (0.05 to 0.88), as in the earlier study, where random score hints (`perm_x1`) reached 0 conflicts on all four; those controls were not part of this protocol, so **none of the planner cells is evidence for a predictor**.
The predictors still know nothing about the planner's models (best accuracy 0.568 against 0.554 for the majority polarity).

### Comparison with the earlier single-split conclusions

| earlier claim (ngboost.md) | under nested CV |
|---|---|
| `logreg:all+conf_x1` 0.105 of the baseline on random 3-SAT | **holds**: 0.104 / 0.082 / 0.102 |
| NGBoost not better than `logreg`; `ngb` 1.64 of `logreg` (confident-first, random 3-SAT) | **holds**, tuned or not: 1.62 to 1.69, not significant after Holm |
| `ngb_lr` reduces to `logreg`; 1.134 (0.90-1.43) | **holds, sharper**: 0.97 to 0.99 (0.88-1.09) |
| `ngb_stack` 0.858 (0.67-1.11), "halved total" | gone: 1.14 to 1.22 (worse side, not claimed) |
| `ngb_lr` on factoring 0.603 (p = 0.009) | gone: 0.96 to 1.05 |
| `ngb` early-stopped is better calibrated than 300 rounds; still behind `logreg` on random 3-SAT, ahead on factoring | holds: log loss 0.578 vs 0.565 (random), 0.6175 vs 0.6277 (factoring) |
| nothing on factoring / colouring is distinguishable from the controls | factoring: holds. Colouring: `jw:all+conf_x1` beats `logreg` under the pre-registered rule and the baseline at every setting; the learnt models do not |
| unsatisfiable instances unaffected | holds (0.92 to 1.05) |

### Caveats

* **The perturbation is weaker than it looks on hinted runs.** On random 3-SAT 38 to 42% of the `:all+conf_x1` runs end with exactly the conflicts of the unperturbed run at flip 1000 / 2000 (14 to 21% for `:all`; 1 to 16% on factoring and colouring), mostly the runs that finish before the first flip would apply.
  The three settings are therefore partly the same measurement on random 3-SAT; "holds at all three" is a weaker requirement there than three independent replicates would be.
* **The labels and the data are those of the earlier study**, including the satisfiable/unsatisfiable split and the 200-conflict selection (re-derived from the new baseline, which equals the old one). One family (random 3-SAT, 89 instances) still carries the only positive result.
* **`jw` on colouring was visible in the earlier tables before this protocol was written** (`jw:all+conf_x1` 0.605 against the baseline, then called "not reliable"). The pre-registered test confirms it at three settings; it was not a new hypothesis, and it is a statement about an untrained order on one generated family.
* **Within-repeat averaging.** The pooled statistic averages the 3 repeats per instance before bootstrapping, so the interval reflects instance-to-instance variation, not the variation between fold assignments; the per-repeat table shows the latter (the direction agrees across repeats on random 3-SAT for every method but `ngb_lr`).
* **Inner selection is by log loss, the solver uses an order.** A setting with the best log loss is not the one with the best confident-first order; nothing here selects on conflicts, deliberately, since that would need solver runs inside the inner loop.
* **Two deviations** (clipped validation curves; a rule for diverging ngboost fits), both forced by ngboost behaviour, both before any result was seen, neither touching a primary quantity beyond the 6 diverged inner fits.
* The planner block has no controls and four instances; it is descriptive only.

### Reproduction

From the repository root, with the cached data of the earlier study in `W/work` (the `ngb_data.py` and `ngb_solve.py` commands of [`ngboost.md`](ngboost.md); `solve_runs.csv` and `hints/` are needed only for the provenance check):

    (cd rust && CARGO_TARGET_DIR=W/ncv/target ../scripts/queue.sh cargo build --release) && mkdir -p W/ncv/bin && cp W/ncv/target/release/intel_sat_solver W/ncv/bin/
    scripts/queue.sh W/venv/bin/python rust/scripts/ngb_nested_cv.py --work W/work --out W/ncv --stage provenance --exe W/ncv/bin/intel_sat_solver --old-exe OLD_BINARY   # 120 runs, 1 min
    scripts/queue.sh W/venv/bin/python rust/scripts/ngb_nested_cv.py --work W/work --out W/ncv --stage fit --jobs 8      # about 12 CPU-hours, roughly 1.5 h on 8 processes
    scripts/queue.sh W/venv/bin/python rust/scripts/ngb_nested_cv.py --work W/work --out W/ncv --stage solve --exe W/ncv/bin/intel_sat_solver --jobs 8   # 24219 runs, about 35 min; exits 1 on a contradiction
    W/venv/bin/python rust/scripts/ngb_nested_cv.py --work W/work --out W/ncv --stage report                            # writes W/ncv/report.md (= ngboost_nested_cv_tables.md)

Every stage checkpoints (one file per fitted outer fold and method, one CSV row per solver run, hint files by content hash); rerunning the same command continues where it stopped.
`--stage solve --dry-run` counts the runs without making them. The report's bootstrap and the folds are seeded; `ngb_lr.py`'s `OffsetBernoulliNGB` gained parameters for its rate, depth and patience whose defaults are the earlier constants. This was checked by rerunning `ngb_lr.py --stage fit` on a copy of the earlier inputs: all 12 arrays of `preds_lr.npz` are bit-identical to the earlier ones.
The fit stage took about 1.5 h of wall time in two launches (the first stopped at the divergence of Deviations, 2; the second, 53 minutes, finished the remaining 119 fold-method fits and the planner).
