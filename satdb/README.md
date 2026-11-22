# satdb

A Datomic-inspired immutable database whose query planner runs on the Rust fork of the Intel(R) SAT Solver (`../rust`).

* **Database values** (`src/db.rs`). A `Db` is an immutable value: datoms `[e a v]` in three covering indexes (EAVT, AEVT, AVET),
  the schema (value type, cardinality one/many) and per-attribute statistics (datoms, distinct entities, distinct values).
  `Db::transact` returns a new value; `Conn` keeps every past one, so `as_of(t)` is a lookup.
* **Persistent indexes** (`src/pmap.rs`). A treap with `Arc`-shared nodes and hash-of-key priorities: an update copies one root-to-leaf path,
  and the tree's shape depends on its key set only.
* **Queries** (`src/query.rs`). Datalog-style conjunctions of patterns `[e attr v]` and comparison predicates, run as nested loops,
  each pattern using the index selected by its bound positions.
* **The planner** (`src/planner.rs`). It chooses the join order with one incremental SAT solver per query, *while the query is being assembled*:
  every added clause extends the encoding (nothing is retracted) and is followed by a short, conflict-bounded refinement, so a good plan exists
  before the query is complete, and `run` finishes the optimization. The module documentation has the details; in short:
  * the order is an acyclic tournament (one variable per pair of clauses), so adding a clause only adds variables and clauses;
  * "variable bound before clause c" points to an open disjunction of the binders seen so far plus a *tail* literal assumed false,
    which a binder added later extends;
  * a predicate's requirement that its variables be bound is an assumption: when a variable has no binder yet, the UNSAT core
    (`IsAssumptionRequired`) names the predicate, which is reported as pending while the rest of the query is planned;
  * the cost, the sum of the estimated log-sizes of the intermediate results, is linear over pairwise indicators and is summed
    by a lower-bounding binary adder network, which grows with the query;
  * optimization tightens `sum <= K` bounds guarded by activation literals, starting below the greedy plan, until UNSAT (proved optimal)
    or until the conflict budget runs out (the best plan so far, which is never worse than greedy).

## Building and testing

The crate depends on the solver by path (`../rust`), with the solver's profiles mirrored (it relies on wrap-around arithmetic).

    cargo test              # unit tests, incl. the planner against brute force (every order of random queries of up to 6 clauses)
    cargo test --release

## Status and next steps

This is a start. What is missing, roughly in order of value:

* Pull, rules/recursion, aggregates, `not`/`or` clauses, and a Datalog text syntax (queries are built through the Rust API).
* Unique/identity attributes, upserts and lookup refs; `retractEntity`; transaction entities and a reified `:db/txInstant`.
* Hard user constraints in the planner (e.g., "run clause A before B"). They would be assumptions like the predicates' requirements,
  and that's where the UNSAT cores become non-trivial: today a predicate is unsatisfiable exactly when one of its variables has no binder,
  which the tests check against the cores.
* Planner scale: on the random 12-clause query of the tests, finding and proving the optimum takes about 6 s (release build);
  most of it is the first improvements over greedy and the final UNSAT proof. Within a conflict budget the result is the best plan so far.
  Candidates: core-guided optimization (lower bounds from cores), and exact DP over subsets for small queries, as a fast path and a cross-check.
  (Measured: lower-bounding adders instead of exact ones cut this from ~40 s; reducing the adder columns in rounds instead of chains made it much slower.)
* Cost model: independence assumptions and fixed predicate selectivities; no histograms.
* Storage: everything is in memory; no durable log or segment storage.
