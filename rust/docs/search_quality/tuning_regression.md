# Tuning on 78 instances

`tune_params.py`: objective PAR-2 on `confs` (`--agg geo`), timeout 60 s, conflict cap 149260, 60 random samples, at most 2 passes of coordinate descent, minimal gain 0.02, seed 1. Scores are relative to the defaults (1.000).

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

* random search: 60 samples, scores min 0.862, median 0.964, max 1.013; 37 of them better than the defaults; the best one kept: 0.869
* coordinate descent, pass 1: 0.869
* after the simplification: 0.883: `/decision/vsids/init_order 1 /decision/vsids/init_order_ai 1 /phase/force_solution 1`

**The tuned configuration** (score 0.883): `/decision/vsids/init_order 1 /decision/vsids/init_order_ai 1 /phase/force_solution 1`

## On the tuned instances

| instance | defaults: status | confs | tuned: status | confs | ratio |
|---|---|---:|---|---:|---:|
| ddtbug_19_02_2025-23_42_40_icsl11502_32499.cnf | UNSAT | 129 | UNSAT | 104 | 0.808 |
| planner_constraint_cores.cnf | SAT | 69 | SAT | 62 | 0.900 |
| planner_huge_budget.cnf | SAT | 7562 | SAT | 8776 | 1.161 |
| planner_large_query.cnf | SAT | 14926 | SAT | 19083 | 1.278 |
| planner_small_refinements.cnf | SAT | 6724 | SAT | 10815 | 1.608 |

78 instances, 78 decided as by the defaults; geometric mean of the ratio 0.883; better on 25, worse on 12, equal on 41; total confs over them: 29664 -> 39056; PAR-2 score (geo) 0.883
Of them, the 3 instances the defaults take at least 200 conflicts for: geometric mean of the ratio 1.336; better on 0, worse on 3.

## Validation: the instances the configuration was not tuned on

| instance | defaults: status | confs | tuned: status | confs | ratio |
|---|---|---:|---|---:|---:|
| final_factor_19_215102417947.cnf | SAT | 20521 | SAT | 38762 | 1.889 |
| final_factor_19_129796251433.cnf | SAT | 22451 | SAT | 76071 | 3.388 |
| final_factor_20_437257030169.cnf | SAT | 173509 | SAT | 118883 | 0.685 |
| final_factor_20_615770916779.cnf | SAT | 200391 | SAT | 110214 | 0.550 |
| final_prime_15_377168641.cnf | UNSAT | 19065 | UNSAT | 19613 | 1.029 |
| final_prime_15_426476371.cnf | UNSAT | 16743 | UNSAT | 16980 | 1.014 |
| final_prime_16_1145940113.cnf | UNSAT | 37312 | UNSAT | 35295 | 0.946 |
| final_prime_16_1284998357.cnf | UNSAT | 35311 | UNSAT | 32951 | 0.933 |
| final_col3_n350_s101.cnf | UNSAT | 5329 | UNSAT | 5822 | 1.092 |
| final_col3_n350_s102.cnf | UNSAT | 7788 | UNSAT | 6965 | 0.894 |
| final_col3_n350_s103.cnf | UNSAT | 11319 | UNSAT | 9732 | 0.860 |
| final_col3_n350_s104.cnf | UNSAT | 35729 | UNSAT | 34269 | 0.959 |
| final_rand3sat_n225_s101.cnf | UNSAT | 45243 | UNSAT | 40324 | 0.891 |
| final_rand3sat_n225_s102.cnf | SAT | 5020 | SAT | 2155 | 0.429 |
| final_rand3sat_n225_s103.cnf | SAT | 12755 | SAT | 12150 | 0.953 |
| final_rand3sat_n225_s104.cnf | UNSAT | 12751 | UNSAT | 11676 | 0.916 |
| final_rand3sat_n225_s105.cnf | SAT | 30176 | SAT | 19035 | 0.631 |
| final_rand3sat_n225_s106.cnf | SAT | 15963 | SAT | 16320 | 1.022 |
| final_rand3sat_n225_s107.cnf | UNSAT | 36099 | UNSAT | 38123 | 1.056 |
| final_rand3sat_n225_s108.cnf | UNSAT | 34624 | UNSAT | 35440 | 1.024 |
| final_pigeonhole_8.cnf | UNSAT | 14052 | UNSAT | 9513 | 0.677 |
| final_pigeonhole_9.cnf | UNSAT | 106730 | UNSAT | 142811 | 1.338 |

22 instances, 22 decided as by the defaults; geometric mean of the ratio 0.957; better on 13, worse on 9, equal on 0; total confs over them: 898881 -> 833104; PAR-2 score (geo) 0.957

Solver runs: 8936.
