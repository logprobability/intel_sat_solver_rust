# NGBoost: calibrated uncertainty as advice for the solver?

The question: can natural-gradient boosted trees (NGBoost, Duan et al. 2020) put a well-calibrated uncertainty around a prediction, and can the solver use it?
Two predictions were tried: the value of each variable in a model (advice through the phase and score hints of [`ceiling.md`](ceiling.md)), and the number of conflicts of an instance.
All the tables are in [`ngboost_tables.md`](ngboost_tables.md) (sections 1 to 5, referred to below); the scripts are `scripts/ngb_*.py`.

## Answer

* **Is it possible?** Yes. NGBoost trains and predicts on features computed from the CNF in linear time, and everything it says reaches the solver through the existing hints.
* **Is it calibrated?** Inside a generated family, yes: expected calibration error 0.003 to 0.016 for the per-variable probabilities, and 88% coverage of the 90% interval for conflicts.
  It is **not better calibrated than plain logistic regression or sklearn gradient boosting**, and outside the family it was trained on it is badly overconfident
  (ECE 0.07 to 0.27 across generated families; 0.32 on the planner instances, where it puts 57% of the variables above 0.9 confidence and is right on 49% of them).
  The calibration is also only marginal: on the factoring formulas the predictors are 62% confident on variables whose value is a coin flip by symmetry.
* **Does it help solving?** On satisfiable uniform random 3-SAT, clearly: hinting the predicted phases **and deciding the confident variables first** takes 0.17 of the baseline's conflicts
  (geometric mean over 89 held-out instances, 95% interval 0.11 to 0.28, 67 better and 22 worse), where every uninformative control is between 1.02 and 1.40. This is the one positive result.
  It does not need NGBoost: logistic regression used the same way gives 0.105, sklearn gradient boosting 0.168, and the Jeroslow-Wang rule with no learning at all 0.417.
  It needs the *ranking* by confidence, not its calibration. On factoring and 3-colouring nothing clears the controls; unsatisfiable instances are not affected (0.98, as the controls);
  **on the planner instances nothing is distinguishable from the controls**, and on the recorded sequences nothing moves at all (1.03).
* **The instance-level predictive distribution** (NGBoost's real strength) is calibrated in-distribution and marginally sharper than a constant-variance GBM (NLL 0.798 against 0.813),
  but neither use tried gains anything: choosing a configuration by predicted mean or by predicted quantile is no better than always running the baseline (1.01), and a budget from a predicted quantile wastes as much as the constant-variance one.

So: NGBoost works as advertised and is not what matters. What mattered here is an ordering of the decisions by how sure a cheap predictor is, on one family of formulas; any probabilistic classifier supplies that.

## What was done

| step | script | what |
|---|---|---|
| data | `ngb_data.py` | 413 fresh generated formulas (seeds used nowhere else) and the 4 planner recordings with their 3 derived versions; each run under the baseline, target phases, vivification and `flip_factor 1000`; features and labels |
| A, B | `ngb_phase.py` | per-variable phase predictors, held out by instance; calibration tables (section 1) |
| B | `ngb_backbone.py` | label ambiguity: backbone against non-backbone variables (section 2) |
| C | `ngb_solve.py` | 104 configurations (the seeds of the randomized ones counted separately) over 259 instances and 8 planner sequences, 27064 solver runs (section 3) |
| D | `ngb_regress.py` | NGBRegressor for log10(conflicts), interval calibration, two uses (section 4) |
| E | `ngb_tables.py` | cost of the advice (section 5); assembles `ngboost_tables.md` |

**Instances.** For the per-variable experiment: uniform random 3-SAT at ratio 4.2 with 175, 200 and 225 variables (135 formulas: 92 satisfiable, 89 of them with at least 200 baseline conflicts; 43 unsatisfiable);
factoring of semiprimes of two 15- to 19-bit primes (40, all satisfiable, 1957 to 153686 conflicts) and of primes (16, unsatisfiable); 3-colouring of random graphs with 350 vertices and 2.30 edges per vertex (60: 35 satisfiable, 25 unsatisfiable).
That is 164 held-out satisfiable instances with a non-trivial search, and 84 unsatisfiable ones. For the regression, 160 more random 3-SAT and colouring formulas over a range of sizes and densities and two pigeonhole formulas.
The planner: the four recordings, and per recording the versions of `ceiling.md`: `.plain` (clauses only, one query), `.lf` (one query, the planner's `lf` polarities in force), `.nolf` (the recorded queries without `lf`).

**Features** (`ngb_common.features`; 29 per variable, 13 per instance, a few linear passes over the literal occurrences): positive and negative occurrence counts, their balance, the counts split by clause size (unit, binary, ternary, longer),
Jeroslow-Wang weights per polarity and their balance, minimum and mean clause size, degree, relative variable index, relative occurrence count; the mean balance of the neighbours;
and "need", added because it turned out to be the most predictive: per polarity, the sum over the clauses of the literal of the product of (1 - q) of the other literals, where q is the occurrence share of a literal
(how much the clauses depend on this literal if the others are satisfied independently), in two rounds, the second taking q from the first. Instance level: sizes, ratio, clause-length shares, the share of positive literals, the spread of occurrences and of balances, the share of pure variables.

**Label.** The value of the variable in the model the baseline run finds (`/topor_tool/write_hints_file`). For the planner, the model of the `.plain` query.

**Models.** `ngb`: `NGBClassifier(Dist=Bernoulli)`, 300 trees of depth 3, learning rate 0.05, minibatch fraction 0.5, at most 30000 training rows. `gbm`: sklearn `HistGradientBoostingClassifier` (200 iterations, 15 leaves).
`gbm_iso`: the same with an isotonic map fitted on out-of-fold predictions of inner folds, which are also split by instance. `logreg`: standardized logistic regression. `jw`: the Jeroslow-Wang rule, its positive share read as a probability. `prior`: the training base rate.
No hyperparameter was tuned on the held-out data; none was tuned at all beyond these first choices.

**For a Bernoulli target the predictive distribution is the probability.** NGBoost's "uncertainty" about a phase is p(1 - p), or the entropy; there is no second parameter. What can differ between the models is only how good and how calibrated p is.
NGBoost's distinctive output, a full distribution with its own variance, exists only for the regression of step D.

**Changes to the planned design.**
* sklearn's histogram gradient boosting instead of the classic `GradientBoostingClassifier` for the classifier (speed; the regression uses the classic one, which has the quantile loss).
* The planner's per-variable samples are those of the `.plain` query, since in the recordings `lf` decides nearly every polarity and a hint is overridden. Two extra planner protocols: with the `lf` polarity as a feature, and without the instance-level features.
* Added after the first results, to find out which part of the advice carries the gain: the confident-first order for every predictor, the same order with random phases, the predicted phases with permuted scores, and the cross-family predictor with the confident-first order.
* The oracle at matched accuracy is run inside this experiment (the same number of variables hinted as the predictor hints, the same number of them correct, chosen at random) instead of read from the table of `ceiling.md`.
* 3-colouring needs no backbone runs: any permutation of the colours maps a model to a model, so it has no backbone variable. For factoring the backbone is exact from the two models (the factors in either order; checked).

## A, B. Per-variable prediction and calibration (sections 1 and 2)

Held out by instance: 5 grouped folds inside a family (the variables of an instance are never split), leave-one-out for the 4 planner instances; cross-family: trained on the other generated families.

| family (variables) | model | accuracy | Brier | log loss | ECE | accuracy at confidence >= 0.8 (share of variables) | >= 0.9 |
|---|---|---:|---:|---:|---:|---:|---:|
| random 3-SAT (18450; majority 0.50) | ngb | 0.693 | 0.1988 | 0.5997 | 0.0160 | 0.862 (0.24) | 0.904 (0.12) |
| | gbm | 0.692 | 0.1995 | 0.5834 | 0.0163 | 0.853 (0.25) | 0.913 (0.09) |
| | gbm_iso | 0.691 | 0.1997 | 0.5838 | 0.0103 | 0.880 (0.17) | 0.933 (0.05) |
| | logreg | **0.703** | **0.1923** | **0.5651** | **0.0097** | 0.872 (0.27) | 0.948 (0.07) |
| | jw | 0.659 | 0.2152 | 0.6189 | 0.0406 | 0.908 (0.04) | 0.982 (0.01) |
| factoring (45200; majority 0.61) | ngb | 0.635 | 0.2166 | 0.6203 | 0.0108 | 0.869 (0.11) | 0.982 (0.03) |
| | gbm | 0.638 | 0.2157 | 0.6167 | 0.0214 | 0.834 (0.18) | 0.925 (0.07) |
| | gbm_iso | 0.641 | 0.2154 | 0.6166 | 0.0126 | 0.887 (0.08) | 0.969 (0.04) |
| | logreg | 0.628 | 0.2202 | 0.6257 | 0.0162 | 0.898 (0.07) | 0.985 (0.03) |
| | jw | 0.633 | 0.2370 | 0.6661 | 0.0954 | - | - |
| 3-colouring (36750; majority 0.66) | ngb | 0.668 | 0.2209 | 0.6330 | 0.0034 | 0.931 (0.01) | 0.964 (0.01) |
| | gbm / gbm_iso / logreg | 0.669 | 0.2209 / 0.2204 / 0.2205 | 0.632 / 0.631 / 0.631 | 0.0066 / 0.0002 / 0.0004 | (0.01) | (0.01) |
| planner `.plain`, leave-one-out (20944; majority 0.55) | ngb | 0.532 | 0.3957 | 2.1922 | **0.3170** | 0.493 (0.60) | 0.489 (0.57) |
| | gbm | 0.571 | 0.2624 | 0.8196 | 0.0941 | 0.633 (0.16) | 0.626 (0.10) |
| | gbm_iso | 0.524 | 0.2590 | 0.9579 | 0.0532 | 0.467 (0.04) | 0.466 (0.04) |
| | logreg | 0.527 | 0.2794 | 0.7714 | 0.1858 | 0.624 (0.24) | 0.802 (0.02) |
| | prior | 0.554 | 0.2525 | 0.6983 | 0.0000 | - | - |
| planner, per-variable features only | ngb | 0.566 | 0.2493 | 0.7078 | 0.0645 | 0.695 (0.12) | 0.695 (0.05) |
| cross-family: random 3-SAT | ngb | 0.680 | 0.2607 | 1.9238 | 0.1994 | 0.700 (0.81) | 0.725 (0.40) |
| cross-family: factoring | ngb | 0.628 | 0.2304 | 0.6525 | 0.0660 | - | - |
| cross-family: 3-colouring | ngb | 0.561 | 0.3141 | 1.1953 | 0.2694 | 0.638 (0.53) | 0.654 (0.50) |

Reading:

1. **Within a family NGBoost is calibrated, and so is everything else.** ECE of 0.016, 0.011 and 0.003; its only visible flaw is in the outermost bins (random 3-SAT: mean p 0.972, observed 0.900; 0.033 against 0.092), which is what costs it log loss against the plain GBM (0.600 against 0.583).
   Logistic regression is the best model on random 3-SAT on every measure. The isotonic step buys the GBM a third off its ECE and nothing else. There is no case here in which NGBoost is the best-calibrated model by a margin that matters.
2. **The predictions are weak.** 69% on random 3-SAT, 64% on factoring (61% for always answering "false"), and exactly the base rate on 3-colouring, where the symmetry of the colours leaves nothing to predict
   (the 1% of confident variables are the 333 colour variables of isolated vertices, which occur in one clause and are all true in the model). `ceiling.md` found that advice which commits on every variable needs about 90% accuracy. That level is reached only on the 5 to 12% of the variables with confidence above 0.9.
3. **Calibration does not transfer.** Across generated families NGBoost's ECE is 0.20 (random 3-SAT), 0.07 (factoring) and 0.27 (colouring), and on random 3-SAT and colouring its log loss is worse than that of the plain GBM. The cross-family logistic regression saturates to 0 or 1 on every variable of random 3-SAT, colouring and the planner instances.
4. **On the planner instances NGBoost is the worst model.** Left-one-out over four instances, it is at least 90% sure about 57% of the variables and right on 49% of them (ECE 0.32, log loss 2.19, against 0.70 for the base rate).
   The cause is the instance-level features: with three training instances the trees learn each instance's base rate from them and apply it with full confidence to the fourth. Without those features (per-variable only) NGBoost's ECE drops to 0.065 and its accuracy rises to 0.566,
   which is 1.2 points above always answering the majority polarity. Adding the planner's own `lf` polarity as a feature changes nothing (0.534). No model here predicts the planner's models.
5. **What it looks at.** The "need" balance carries 57 to 62% of NGBoost's importance on random 3-SAT and factoring. It is one round of a belief-propagation-like estimate, so the learnt models are close to a tuned, smoothed version of a known heuristic; that is consistent with logistic regression matching the trees.

**Label ambiguity (section 2).** The label is one model's value. Checked on 40 random 3-SAT instances (7225 solver runs, every variable tested with the opposite unit clause) and on the 40 factoring instances:

| family | subset (share of variables) | ngb accuracy | ngb mean confidence | ngb accuracy at confidence >= 0.8 (share) |
|---|---|---:|---:|---:|
| random 3-SAT | backbone (0.39) | 0.762 | 0.720 | 0.916 (0.27) |
| random 3-SAT | non-backbone (0.61) | 0.643 | 0.699 | 0.797 (0.22) |
| factoring | backbone (0.62) | 0.718 | 0.663 | 0.946 (0.15) |
| factoring | non-backbone (0.38) | **0.501** | **0.618** | 0.508 (0.05) |

* The predictors are better on backbone variables (0.76 against 0.64 on random 3-SAT; the other learnt models are within a point of NGBoost in every cell), and on random 3-SAT they still agree with the found model on 64% of the free variables, so the solver's model is itself biased towards what the features suggest.
* **Calibrated on average is not calibrated where it matters.** On the factoring formulas the non-backbone variables are exactly those the swap of the two factors flips, so nothing can predict them (0.501), yet every model is about 62% confident on them, and at confidence 0.8 still right on half.
  The pooled reliability table looks fine (ECE 0.011) because the overconfidence on these variables and the underconfidence on the backbone cancel. A confidence threshold therefore does not separate "known" from "unknowable" variables.
* The backbone share of a random 3-SAT instance ranges from 0 to 0.95 (median 0.30). 3-colouring has none.

## C. Using the predictions in the solver (section 3)

Predictions are out-of-sample for the instance. `ratio` is the geometric mean of (conflicts + 1) / (baseline + 1), with a 95% bootstrap interval over instances; wins/losses in conflicts. Every model was verified by the tool;
**no run contradicts the baseline (0 SAT-versus-UNSAT, no timeouts or errors in 27064 runs)**; one budget-limited planner query ended differently, which is not a contradiction.

Satisfiable instances with at least 200 baseline conflicts:

| configuration | random 3-SAT (89) | factoring (40) | 3-colouring (35) |
|---|---|---|---|
| *controls* | | | |
| `flip500` / `flip1000` / `flip2000` | 1.232 / 1.267 / 1.017 | 0.866 / 0.910 / 0.872 | 0.994 / 1.146 / 1.230 |
| `rand_phase` (random phase hints, median of 3) | 1.297 (1.05-1.63) | 1.008 (0.79-1.28) | 1.070 (0.84-1.38) |
| `perm_x1` (random score hints) | 1.352 (1.12-1.67) | 1.105 (0.81-1.46) | 1.170 (0.87-1.55) |
| `rand_phase+ngb_conf_x1` (random phases, ngb's order) | 1.397 (1.14-1.72) | 0.966 (0.74-1.28) | 0.828 (0.52-1.25) |
| *phases only, every variable* | | | |
| `prior:all` (the majority polarity) | 0.901 (0.75-1.06) 10/7 | 0.856 (0.62-1.20) 23/17 | 1.371 (1.08-1.75) 9/26 |
| `jw:all` | 0.808 (0.58-1.08) 46/43 | 0.790 (0.60-1.05) 24/16 | 1.399 (1.10-1.78) 9/26 |
| `logreg:all` | 0.716 (0.52-0.99) 51/38 | 0.774 (0.56-1.08) 23/17 | 1.399 (1.09-1.79) 9/26 |
| `gbm:all` | 0.670 (0.48-0.94) 49/40 | 0.684 (0.50-0.95) 24/16 | 1.305 (1.01-1.68) 10/25 |
| `gbm_iso:all` | 0.784 (0.55-1.10) 45/44 | 0.812 (0.58-1.12) 22/18 | 1.399 (1.10-1.80) 9/26 |
| `ngb:all` | 0.727 (0.53-1.01) 51/38 | 0.750 (0.56-0.97) 28/12 | 1.278 (0.93-1.70) 11/24 |
| `oracle_match:all` (true model, same accuracy, random errors) | 0.996 (0.76-1.30) | 0.941 (0.73-1.21) | 1.126 (0.84-1.50) |
| *phases only where confident (abstain below the threshold)* | | | |
| `ngb:t0.6` / `t0.7` / `t0.8` / `t0.9` | 0.728 / 0.787 / 1.098 / 1.162 | 0.822 / 0.853 / 1.062 / 0.992 | 1.150 / 0.981 / 1.001 / 1.000 |
| *phases and score hints* | | | |
| `ngb:all+conf_x1` (confident variables decided first) | **0.173 (0.11-0.28) 67/22** | 0.825 (0.60-1.16) 20/20 | 0.904 (0.65-1.29) 18/17 |
| `ngb:all+conf_x10` | 0.170 (0.11-0.27) 68/21 | 0.822 (0.57-1.18) 23/17 | 0.913 (0.64-1.31) 18/17 |
| `ngb:t0.7+conf_x1` | 0.172 (0.11-0.28) 67/22 | 0.698 (0.50-0.97) 22/18 | 0.770 (0.53-1.14) 18/17 |
| `gbm:all+conf_x1` | 0.168 (0.10-0.27) 64/25 | 0.799 (0.63-1.03) 25/15 | 1.014 (0.77-1.35) 17/18 |
| `gbm_iso:all+conf_x1` | 0.165 (0.10-0.27) 65/24 | 0.874 (0.59-1.25) 24/16 | 1.186 (0.84-1.65) 15/20 |
| `logreg:all+conf_x1` | **0.105 (0.06-0.17) 68/21** | 0.939 (0.69-1.28) 19/21 | 1.038 (0.80-1.35) 18/17 |
| `jw:all+conf_x1` (no learning) | 0.417 (0.29-0.60) 58/31 | 0.845 (0.61-1.16) 20/20 | 0.605 (0.39-0.93) 21/14 |
| `ngbX:all+conf_x1` (trained on the other families) | 0.530 (0.36-0.76) 54/35 | 0.727 (0.55-0.96) 25/15 | 0.633 (0.46-0.87) 21/14 |
| `ngb:all+perm_x1` (ngb's phases, random order) | 0.727 (0.56-0.94) 52/37 | 0.817 (0.63-1.06) 25/15 | 1.179 (0.90-1.57) 15/20 |
| `ngb:all+unc_x1` / `unc_x10` (uncertain variables first) | 1.082 / 0.948 | 0.769 / 0.710 | 0.808 / 0.697 (0.48-1.00) |
| *score hints only* | | | |
| `ngb:conf_x10` / `ngb:unc_x10` | 1.472 / 0.950 | 0.737 / 0.979 | 0.832 / 0.875 |
| *reference* | | | |
| `oracle:1` (the model itself) | 0.000 | 0.000 | 0.000 |

Other groups: 84 unsatisfiable instances (random 3-SAT, primes, colouring): controls 0.980 to 0.997; `ngb:all` 0.982, `ngb:all+conf_x1` 0.982 (per instance 0.64 to 1.45; `flip1000`: 0.74 to 1.33), `logreg:all+conf_x1` 0.990. **Unsatisfiable instances are neither hurt nor helped.**
The 3 satisfiable instances under 200 conflicts (333 in total) get worse by a factor of 2 to 25 under almost any hint, the random controls included (`rand_phase` 18x): on an instance the default phases solve in 100 conflicts, any other start costs a few thousand.

Reading:

1. **The gain on random 3-SAT is real and it is in the order, given the phases.** Phases alone: 0.73, an interval that touches 1 and wins on 51 of 89. The confident-first order alone: 1.47, worse than the baseline. The order with random phases: 1.40.
   Both together: 0.17, against 1.02 to 1.40 for six different controls. The multiplier does not matter (0.173 at 1, 0.170 at 10), and hinting the phase of only the confident half is the same (0.172): the solver decides the variables the predictor is surest about first, in the predicted polarity, and the rest follows by propagation and ordinary search.
   The reverse order, uncertain first, destroys the gain (1.08). This is decimation by a cheap marginal estimate, a known idea from survey and belief propagation solvers for random formulas, arrived at through the hints.
2. **It is a large gain in the typical instance and a small one in the total.** The median ratio is 0.25; 36 of 89 instances drop below a tenth of their conflicts and 23 finish within 100 conflicts (none under the baseline), but 10 take more than twice and 4 more than ten times the baseline's conflicts.
   The total is 692 thousand conflicts against 892 thousand (0.78), and the CPU time 10.9 s against 14.8 s (11.1 s with the advice computed in Python): the hardest instances keep their cost.
   The gain is concentrated where many models exist: 0.07 on the instances whose backbone is under 20% of the variables (15), 0.25 between 20 and 50% (8), 0.51 above (14). The advice steers into *a* model; it does not find *the* model.
3. **NGBoost is not the reason.** Logistic regression gives 0.105 with the same recipe, the uncalibrated GBM 0.168, the isotonic one 0.165, NGBoost 0.173; the intervals overlap. The Jeroslow-Wang rule, with no training, gives 0.417, so most of the effect is available without any learning, and the learnt features (the "need" balance) explain the rest.
   The cross-family NGBoost model, whose probabilities are badly miscalibrated on random 3-SAT (ECE 0.20), still gives 0.53: **the ranking is used, the calibration is not.** Any monotone distortion of the confidence would leave the order, and so the result, unchanged.
4. **The predictor's mistakes are better than random mistakes.** The true model with the same number of wrong hints placed at random gives 0.996, the predictor 0.67 to 0.78. The predictors are wrong mostly on variables that are free, or they agree with a different model. This is also why the matched-accuracy line of `ceiling.md` (worthless at 69%) understates a real predictor.
5. **Abstaining by confidence does not help; it only weakens.** With phases only, the effect fades as the threshold rises (0.73 at 0.6, 0.79 at 0.7, 1.10 at 0.8, 1.16 at 0.9, the last two inside the control band), and so does the matched oracle (1.06 to 1.25).
   `ceiling.md` found that a correct hint for a random half of the variables is worth 0.36; a hint which is right in 86% of the cases for the most confident quarter is worth nothing. The confident variables are the easy ones.
6. **Factoring and 3-colouring: nothing reliable.** On factoring the best cells are 0.68 to 0.75 with intervals that end near 1, while the controls are at 0.87 to 0.91 themselves and the majority polarity gives 0.86; the confident-first order does not help (0.83, 20 better and 20 worse).
   On 3-colouring the predicted phases are the base rate and *hurt* (1.28 to 1.40, as does hinting "all false": 1.37); several orderings land at 0.6 to 0.8 (`jw:all+conf_x1` 0.605, `ngb:all+unc_x10` 0.697, cross-family 0.633) with intervals that reach 0.9 to 1.0,
   but so do the uncertain-first order and the random-phase control with ngb's order (0.83). With some 40 configurations per family, a few intervals excluding 1 are expected by chance; no pattern on these two families survives that.

**The planner (4 instances; section 3, last five tables).**

| version | base (conflicts) | controls | `ngb:all` | `ngb:all+conf_x1` | best-looking cell |
|---|---:|---|---:|---:|---|
| `.plain` (one query, clauses only) | 11745 | flip 0.67 to 0.98; `rand_phase` 0.63; **`perm_x1` 0.002** | 0.163 | 0.014 | `ngbLF:t0.7` 0.003 |
| `.lf` (one query, `lf` in force) | 8762 | flip 1.05 to 1.29; `rand_phase` 0.57; `perm_x1` 0.45 | 0.441 | 0.743 | `logreg:all+conf_x1` 0.298; `prior:all` 0.390 |
| `.nolf` (recorded queries, hints renewed per query) | 27397 | flip 0.98 to 1.12; `rand_phase` 2.16 | 2.002 | not run | `ngbPV:t0.8` 0.870 |
| recorded (hints renewed per query) | 29281 | flip 1.04 to 1.18; `rand_phase` 1.04 | 1.031 | not run | `lf:all`, `ngb:t0.8` 1.025 |

* On the `.plain` queries NGBoost's hints reach the oracle's 0 conflicts on some instances, and so do **random score hints** (`perm_x1`: the median over the seeds is 0 conflicts on every one of the four). As in `ceiling.md`, these queries are easy and the default order is an unlucky one; any perturbation of the order fixes them. Nothing here is evidence for the predictor.
* With `lf` in force, hinting every variable the majority polarity (0.390) does as well as any predictor, and random phases (0.57) and random scores (0.45) are in the same range with intervals from 0.1 to 2.
* On the recorded sequences every hint configuration is between 1.03 and 1.08, and the controls are at 1.04 to 1.18. `lf` overrides the hints and the conflicts are in refutations and exhausted budgets, exactly as `ceiling.md` found.
* The intervals over four instances span one to two orders of magnitude. **No configuration is distinguishable from the controls on any planner version.**

## D. A predictive distribution for the search effort (section 4)

`NGBRegressor(Dist=Normal)` on log10(conflicts + 1), which is a LogNormal on the conflicts; depth-3 trees, learning rate 0.03, the number of stages (59 to 95) chosen by early stopping on a fifth of the training instances.
**The data set is small: 413 generated instances** (235 random 3-SAT, 120 colouring, 56 factoring, 2 pigeonhole), three configurations each, 1239 rows, 16 features; the target has a deviation of 0.76 (a factor of 5.8). No planner instance is in it: four is not a data set.

| held out by | model | RMSE | NLL | coverage of the 50% / 90% interval | width of the 90% interval (log10) | largest PIT-bin deviation from 0.10 |
|---|---|---:|---:|---|---:|---:|
| instance (5 folds) | ngb | 0.536 | **0.798** | 0.50 / 0.88 | 1.56 | 0.02 |
| | gbm + constant sigma | 0.545 | 0.813 | 0.57 / 0.91 | 1.81 | 0.03 |
| | gbm, quantile loss | 0.537 | - | 0.40 / 0.80 | 1.39 | - |
| | training mean and deviation | 0.761 | 1.148 | 0.56 / 0.89 | 2.50 | 0.06 |
| family | ngb | 0.761 | **2.213** | 0.41 / 0.71 | 1.72 | 0.14 |
| | gbm + constant sigma | 0.752 | 1.315 | 0.39 / 0.77 | 1.76 | 0.13 |
| | training mean and deviation | 0.776 | 1.195 | 0.57 / 0.90 | 2.56 | 0.05 |

* **In-distribution the intervals are calibrated** (0.50 and 0.88 for nominal 0.50 and 0.90, a flat PIT histogram), 14% narrower than the constant-variance ones, with a slightly better NLL. The quantile GBM is too narrow (0.80 for 0.90).
  The predicted sigma is informative but compressed: in the third of the rows with the lowest predicted sigma it is 0.37 against an actual error of 0.44; in the highest third 0.57 against 0.60.
  Per family it is less even: 0.95 coverage on factoring, 0.85 on colouring.
* **Out of distribution they are not.** Trained on two families and applied to the third, NGBoost's 90% interval covers 71% overall and **34% on 3-colouring**, where it is also the most confident (sigma 0.29); its NLL (2.21) is far worse than that of predicting the training mean and deviation (1.20). A learnt variance fails harder than a constant one when the input is unfamiliar.
* **Most of the variance cannot be predicted from the formula.** A perturbation without information (`flip_factor 1000`) moves log10(conflicts) of a satisfiable instance by 0.30 to 0.43 RMS, which is most of the model's 0.54 error; for unsatisfiable instances it is 0.04 to 0.06, but whether an instance near the threshold is satisfiable is itself unknown to the features.

**Use 1: choosing among baseline, target phases and vivification per instance.** Conflicts relative to always running the baseline (geometric mean, held out by instance, the 383 instances with at least 200 conflicts):

| rule | chosen: base / target / vivify | ratio |
|---|---|---:|
| always target | 0 / 413 / 0 | 0.949 |
| always vivify | 0 / 0 / 413 | 0.998 |
| ngb, lowest predicted mean | 403 / 7 / 3 | 1.010 |
| ngb, lowest predicted 90% quantile (risk-aware) | 400 / 8 / 5 | 1.009 |
| ngb, leave the baseline only if P(other is better) > 0.75 | 413 / 0 / 0 | 1.000 |
| gbm, lowest predicted mean | 214 / 189 / 10 | 0.985 |
| oracle, the best of the three | 159 / 179 / 75 | 0.760 |

A null. The predicted difference between two configurations of the same instance has a correlation of -0.06 to -0.12 with the actual one: the models predict how hard an instance is, not which configuration suits it. The risk-aware rule behaves sensibly (it almost never leaves the baseline, because the predicted distributions of the three configurations overlap almost entirely), and that is all it can do.
A fixed choice ("always target", 0.949, from the satisfiable instances: 0.904) beats every per-instance rule, and the oracle's 0.76 shows headroom the features do not reach.

**Use 2: a conflict budget from a predicted quantile.** With the budget at the predicted 90% quantile, 90.1% of the held-out instances finish (gbm + constant sigma: 91.8%; the training distribution: 93.5%), so the quantile means what it says.
But at an equal 90% solved, the conflicts wasted on the instances given up on are 19% of all the conflicts for NGBoost, 17% for the constant-variance GBM and 26% for one global budget. A per-instance budget is better than a global one; the learnt variance adds nothing to the learnt mean.

## E. Cost (section 5)

| group | variables | features ms | ngb / gbm / logreg inference ms | baseline solve ms | (features + ngb) / solve |
|---|---:|---:|---|---:|---:|
| random 3-SAT, satisfiable, >= 200 conflicts | 200 | 0.61 | 2.28 / 0.71 / 0.04 | 36.6 | 0.08 |
| factoring, satisfiable | 1122 | 3.15 | 12.77 / 4.00 / 0.21 | 707.8 | 0.02 |
| 3-colouring, satisfiable, >= 200 conflicts | 1050 | 1.39 | 11.95 / 3.74 / 0.20 | 57.5 | 0.24 |
| generated, < 200 conflicts | 493 | 1.08 | 5.61 / 1.76 / 0.09 | 4.2 | **1.6** |
| planner `.plain` queries | 4887 | 10.81 | 55.64 / 17.40 / 0.93 | 48.4 | **1.5** |

Medians. Features are numpy passes over the literal occurrences; NGBoost's inference is 11 microseconds per variable (300 Python-level trees), the GBM's 3.5, logistic regression's 0.2.
On the hard generated instances the advice costs 2 to 8% of the solve time, which the gain on random 3-SAT covers. **On the planner queries NGBoost's advice costs more than solving** (66 ms against 48 ms in the median; 0.5 to 2.0 times the solve time over the four, above 1 for three of them),
and logistic regression would be the only affordable predictor (12 ms, almost all of it the features). A native feature pass would be much cheaper than the numpy one; training (4 s per 10000 variables for NGBoost, offline) is not counted.

## What it would take to matter for the planner

1. **More recordings.** Four instances cannot train, calibrate or evaluate anything; leave-one-out over four is how NGBoost came to be 90% sure and half right. Dozens of recorded query sequences are the precondition (the first next step of [`README.md`](README.md) as well).
2. **Features that identify a variable across queries.** CNF-only features know a variable by its occurrence counts, and those predict the planner's models at 1.2 points above the base rate. The planner knows what each variable means (which join, which predicate, which refinement);
   the value of the "same" variable in a previous query, or the role of the variable, is the feature that could carry information. `ceiling.md` shows index-based carry-over does not work, so the identity has to come from the planner.
3. **A target that is where the conflicts are.** The planner's conflicts are in unsatisfiable and budget-limited queries, where no model exists and phase advice has no headroom (`ceiling.md`: the perfect oracle changes nothing in the recordings), and `lf` overrides hints anyway.
   A learnt predictor there would have to advise the decision *order* (carry-over ceiling: 20 to 25% on identical refutations) or the conflict budgets. For budgets a calibrated per-query distribution of the effort is exactly the right tool, and step D says it can be had in-distribution; it needs per-query records of conflicts, verdict and budget.
4. **A cheaper predictor than NGBoost.** At planner query sizes the advice must cost well under 10 ms; logistic regression on a native feature pass would, a 300-tree ensemble evaluated in Python does not.
5. **A control in every comparison.** The planner queries can be brought to 0 conflicts by random score hints. Any future claim there has to beat `perm_x1` and `rand_phase`, not the baseline.

Outside the planner, the finding worth following up is not NGBoost but the confident-first order on satisfiable random-like formulas: test it on more families and sizes, against the solver's own alternatives for such instances (target phases gave 0.44 on similar formulas in [`target_phase.md`](target_phase.md)), and with a predictor that is a few lines of code (the "need" balance alone).

## Caveats and what was not verified

* **One family carries the positive result.** Uniform random 3-SAT at ratio 4.2 with 175 to 225 variables, 89 instances, baseline 272 to 100311 conflicts. Not tested: larger formulas, other ratios, structured satisfiable families other than the two here, industrial instances. The effect was not compared against target phases or rephasing on the same instances.
* **The confident-first strategies for `jw`, `logreg`, `gbm_iso` and the cross-family model, and two of the controls, were added after seeing the first results** (to attribute the gain), on the same held-out instances. The original `ngb:all+conf_x1` and its controls were in the plan; the follow-ups are consistent with it but are not a pre-registered test.
* **Many configurations, few instances.** 104 configurations per group; intervals are per configuration and unadjusted. Only the random 3-SAT confident-first result (intervals far from 1, six controls on the other side, five predictors agreeing) is strong enough to survive that. Everything on factoring, colouring and the planner should be read as "not distinguishable".
* **Selection.** The headline groups are the instances with at least 200 baseline conflicts; on random 3-SAT that drops 3 of 92, so the effect of selecting on the baseline is small there. On those 3 easy instances the hints hurt badly, the controls too.
* **Single runs.** The solver is deterministic; each configuration ran once per instance (3 seeds for the randomized ones). The bootstrap interval reflects variation over instances only.
* **Labels are the baseline's own model**, which is biased by the default polarity (positive first); a predictor of "a model" trained on another solver's models would differ. The backbone analysis covers 40 random instances and the factoring formulas only.
* **Calibration tables pool the variables of all the held-out instances of a family**; per-instance accuracy varies (random 3-SAT: 0.59 to 0.81).
* **The regression has 413 instances from four generators** and no real workload; its in-distribution calibration says nothing about a new family, as the family-held-out rows show.
* **CPU times** were measured on a shared machine under load from other jobs; conflicts are exact, times indicative. The feature and inference times are Python times.
* **NGBoost hyperparameters were not tuned** (nor were the other models'); a tuned NGBoost might be somewhat sharper. Nothing here suggests it would change a conclusion, since calibration was not what the solver used.
* Not done: NGBoost with a categorical or Beta distribution; per-query labels for the planner sequences (only the final `.plain` model); score hints on the `.nolf` and recorded sequences (`ls` before each query); hints through IPASIR.

## Dependencies and reproduction

`ngboost`, `scikit-learn`, `numpy` and `scipy` are needed by the offline scripts `scripts/ngb_*.py` only. The solver, its build and its tests depend on none of them, and nothing was added to `Cargo.toml`.
Versions used: Python 3.14.7, ngboost 0.6.1, scikit-learn 1.9.1, numpy 2.5.5, scipy 1.18.1 (pulled in by ngboost).

From `rust/`, with a release build (`cargo build --release`); `W` is any scratch directory:

    python3 -m venv W/venv && W/venv/bin/pip install ngboost==0.6.1 scikit-learn==1.9.1 numpy==2.5.5 scipy==1.18.1
    W/venv/bin/python scripts/ngb_data.py     --exe target/release/intel_sat_solver --work W/work    # about 2.5 min: instances, 1716 runs, features
    W/venv/bin/python scripts/ngb_phase.py    --work W/work                                           # about 15 min: predictors, section 1
    W/venv/bin/python scripts/ngb_backbone.py --exe target/release/intel_sat_solver --work W/work    # about 15 min: 7265 runs, section 2
    W/venv/bin/python scripts/ngb_solve.py    --exe target/release/intel_sat_solver --work W/work    # about 45 min: 27064 runs, section 3; exits 1 on a contradiction
    W/venv/bin/python scripts/ngb_regress.py  --work W/work                                           # 30 s: section 4
    W/venv/bin/python scripts/ngb_tables.py   --work W/work                                           # section 5, writes docs/search_quality/ngboost_tables.md

Times are for 8 parallel solver processes on a 10-core machine that was shared with other jobs. The instance generation, the folds and the models are seeded; the predictions of two runs of `ngb_phase.py` were bit-identical, and `ngb_solve.py` checks that its baseline reproduces the conflicts recorded by `ngb_data.py`.
`ngb_solve.py --resume` reuses the runs of `W/work/solve_runs.csv` and runs only what is missing. The solver binary used was a copy of the `target/release/intel_sat_solver` present in this worktree when the experiment started (HEAD 22dcb26); it was not rebuilt for the experiment, so that it was built from exactly that commit was not verified.
