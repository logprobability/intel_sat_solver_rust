# Tuning on 4 instances

`tune_params.py`: objective PAR-2 on `confs` (`--agg mean`), timeout 60 s, conflict cap 149260, 80 random samples, at most 3 passes of coordinate descent, minimal gain 0.02, seed 1. Scores are relative to the defaults (1.000).

The space (the default of every dimension is a choice too):

* `/mode/value`: 1, 2, 3, 4, 5, 6, 7, 8
* `/restarts/numeric/conflict_thr`: 100, 300, 3000
* `/restarts/arithmetic/conflict_increment`: 0, 50, 200
* `/restarts/numeric/sub_strat`: 1
* `/decision/vsids/var_decay_max+`: `/decision/vsids/var_decay_max 0.9 /decision/vsids/var_decay_max_ai 0.9 /decision/vsids/var_decay_max_s 0.9`, `/decision/vsids/var_decay_max 0.95 /decision/vsids/var_decay_max_ai 0.95 /decision/vsids/var_decay_max_s 0.95`, `/decision/vsids/var_decay_max 0.99 /decision/vsids/var_decay_max_ai 0.99 /decision/vsids/var_decay_max_s 0.99`
* `/decision/vsids/init_order+`: `/decision/vsids/init_order 0 /decision/vsids/init_order_ai 0`, `/decision/vsids/init_order 1 /decision/vsids/init_order_ai 1`
* `/decision/polarity/init_strategy`: 0, 1, 2
* `/backtracking/chrono_bt_if_higher_init+`: `/backtracking/chrono_bt_if_higher_init 0 /backtracking/chrono_bt_if_higher_n 0 /backtracking/chrono_bt_if_higher_s 0`, `/backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50`, `/backtracking/chrono_bt_if_higher_init 1000 /backtracking/chrono_bt_if_higher_n 1000 /backtracking/chrono_bt_if_higher_s 1000`
* `/conflicts/all_uip/mode`: 1, 3
* `/conflicts/flipped_recording_max_lbd`: 0, 40
* `/deletion/clause/frac_to_delete`: 0.5, 0.7, 0.8
* `/deletion/clause/glue_never_delete`: 2, 3, 5
* `/phase/force_solution`: 1
* `/phase/target/on`: 1
* `/phase/rephase/conf_init`: 100, 1000, 10000
* `/phase/rephase/conf_incr`: 100, 10000
* `/phase/rephase/random`: 1
* `/inprocessing/vivify/on+`: `/inprocessing/on 1 /inprocessing/conflicts_before_next 1000 /inprocessing/vivify/on 1`, `/inprocessing/on 1 /inprocessing/conflicts_before_next 5000 /inprocessing/vivify/on 1`, `/inprocessing/on 1 /inprocessing/conflicts_before_next 20000 /inprocessing/vivify/on 1`
* `/inprocessing/vivify/effort`: 0.03, 0.3
* `/inprocessing/vivify/max_glue`: 2, 6

* random search: 80 samples, scores min 0.814, median 1.117, max 1.586; 16 of them better than the defaults; the best one kept: 0.814
* coordinate descent, pass 1: 0.737
* coordinate descent, pass 2: 0.737
* after the simplification: 0.737: `/mode/value 1 /decision/vsids/init_order 0 /decision/vsids/init_order_ai 0 /decision/polarity/init_strategy 0 /backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50 /deletion/clause/frac_to_delete 0.5`

**The tuned configuration** (score 0.737): `/mode/value 1 /decision/vsids/init_order 0 /decision/vsids/init_order_ai 0 /decision/polarity/init_strategy 0 /backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50 /deletion/clause/frac_to_delete 0.5`

## On the tuned instances

| instance | defaults: status | confs | tuned: status | confs | ratio |
|---|---|---:|---|---:|---:|
| planner_constraint_cores.cnf | SAT | 69 | SAT | 121 | 1.743 |
| planner_huge_budget.cnf | SAT | 7562 | SAT | 5579 | 0.738 |
| planner_large_query.cnf | SAT | 14926 | SAT | 13296 | 0.891 |
| planner_small_refinements.cnf | SAT | 6724 | SAT | 2577 | 0.383 |

4 instances, 4 decided as by the defaults; geometric mean of the ratio 0.814; better on 3, worse on 1, equal on 0; total confs over them: 29281 -> 21573; PAR-2 score (mean) 0.737
Of them, the 3 instances the defaults take at least 200 conflicts for: geometric mean of the ratio 0.632; better on 3, worse on 0.

## Leave-one-out

The tuning repeated without one instance (the runs are reused), and the result on the instance left out: what the tuned configuration is expected to be worth on an unseen instance of the family.

| left out | configuration tuned on the others | its score on them | confs: defaults | tuned on the others | ratio |
|---|---|---:|---:|---:|---:|
| planner_constraint_cores.cnf | `/mode/value 1 /decision/vsids/init_order 0 /decision/vsids/init_order_ai 0 /decision/polarity/init_strategy 0 /backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50 /deletion/clause/frac_to_delete 0.5` | 0.734 | 69 | 121 | 1.743 |
| planner_huge_budget.cnf | `/mode/value 1 /decision/vsids/init_order 0 /decision/vsids/init_order_ai 0 /decision/polarity/init_strategy 0 /backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50 /deletion/clause/frac_to_delete 0.5` | 0.736 | 7562 | 5579 | 0.738 |
| planner_large_query.cnf | `/mode/value 1 /decision/vsids/init_order 0 /decision/vsids/init_order_ai 0 /decision/polarity/init_strategy 0 /backtracking/chrono_bt_if_higher_init 50 /backtracking/chrono_bt_if_higher_n 50 /backtracking/chrono_bt_if_higher_s 50 /phase/rephase/conf_init 100` | 0.528 | 14926 | 14328 | 0.960 |
| planner_small_refinements.cnf | `/mode/value 4 /deletion/clause/frac_to_delete 0.5` | 0.767 | 6724 | 8453 | 1.257 |

Geometric mean of the ratio on the instances left out: 1.116 (a cost of twice the cap stands for a run, which doesn't decide the instance as the defaults do).

## Validation: the instances the configuration was not tuned on

| instance | defaults: status | confs | tuned: status | confs | ratio |
|---|---|---:|---|---:|---:|
| ddtbug_19_02_2025-23_42_40_icsl11502_32499.cnf | UNSAT | 129 | UNSAT | 95 | 0.738 |
| final_factor_19_215102417947.cnf | SAT | 20521 | SAT | 68353 | 3.331 |
| final_factor_19_129796251433.cnf | SAT | 22451 | SAT | 117684 | 5.242 |
| final_factor_20_437257030169.cnf | SAT | 173509 | SAT | 47093 | 0.271 |
| final_factor_20_615770916779.cnf | SAT | 200391 | SAT | 28750 | 0.143 |
| final_prime_15_377168641.cnf | UNSAT | 19065 | UNSAT | 23244 | 1.219 |
| final_prime_15_426476371.cnf | UNSAT | 16743 | UNSAT | 19818 | 1.184 |
| final_prime_16_1145940113.cnf | UNSAT | 37312 | UNSAT | 43776 | 1.173 |
| final_prime_16_1284998357.cnf | UNSAT | 35311 | UNSAT | 40802 | 1.155 |
| final_col3_n350_s101.cnf | UNSAT | 5329 | UNSAT | 5741 | 1.077 |
| final_col3_n350_s102.cnf | UNSAT | 7788 | UNSAT | 10427 | 1.339 |
| final_col3_n350_s103.cnf | UNSAT | 11319 | UNSAT | 16909 | 1.494 |
| final_col3_n350_s104.cnf | UNSAT | 35729 | UNSAT | 52265 | 1.463 |
| final_rand3sat_n225_s101.cnf | UNSAT | 45243 | UNSAT | 66911 | 1.479 |
| final_rand3sat_n225_s102.cnf | SAT | 5020 | SAT | 34933 | 6.958 |
| final_rand3sat_n225_s103.cnf | SAT | 12755 | SAT | 6867 | 0.538 |
| final_rand3sat_n225_s104.cnf | UNSAT | 12751 | UNSAT | 23788 | 1.866 |
| final_rand3sat_n225_s105.cnf | SAT | 30176 | SAT | 5242 | 0.174 |
| final_rand3sat_n225_s106.cnf | SAT | 15963 | SAT | 53997 | 3.382 |
| final_rand3sat_n225_s107.cnf | UNSAT | 36099 | UNSAT | 63257 | 1.752 |
| final_rand3sat_n225_s108.cnf | UNSAT | 34624 | UNSAT | 57919 | 1.673 |
| final_pigeonhole_8.cnf | UNSAT | 14052 | UNSAT | 22904 | 1.630 |
| final_pigeonhole_9.cnf | UNSAT | 106730 | UNSAT | 150169 | 1.407 |

96 instances, 96 decided as by the defaults; geometric mean of the ratio 0.949; better on 33, worse on 32, equal on 31; total confs over them: 899264 -> 961153; PAR-2 score (mean) 1.069
Of them, the 22 instances the defaults take at least 200 conflicts for: geometric mean of the ratio 1.263; better on 4, worse on 18.

Solver runs: 1324.
