# satdb

A Datomic-inspired immutable database whose query planner runs on the Rust fork of the Intel(R) SAT Solver (`../rust`).

* **Database values** (`src/db.rs`). A `Db` is an immutable value: the current datoms `[e a v]` in four indexes (EAVT, AEVT, AVET, and VAET for refs; the history has more, see below),
  the schema (value type, cardinality one/many, `Unique::Identity`/`Unique::Value`, component refs) and per-attribute statistics
  (datoms, distinct entities, distinct values). `Db::transact` returns a new value.
  * Value types: `Bool`, `Int` (i64), `Float` (f64), `Str`, `Instant` (milliseconds since the Unix epoch) and `Ref`. Values order by type,
    then within it; Floats are totally ordered (`f64::total_cmp`: -0.0 before 0.0, NaN at the ends), so that they can be index keys, and
    equality follows that order (NaN equals NaN, -0.0 doesn't equal 0.0). Comparison predicates compare an Int with a Float numerically.
* **Transactions** (`src/db.rs`). As in Datomic, a transaction is a *set* of assertions and retractions against the database before it,
  so the order of its operations doesn't matter:
  * a tempid asserting an existing `:db.unique/identity` value upserts to that entity, and tempids asserting the same new identity value
    unify into one entity (to a fixed point, so ref-valued identities chain, and tempids whose ref-valued identity is the same new entity unify too);
    a tempid resolving to two entities is an `UpsertConflict`;
  * a tempid or lookup ref carries the identity of its `Tx` (unique in the process), so using it in another transaction is an
    `Error::ForeignEntity` (and `TxReport::eid` panics on it) rather than silently naming that transaction's entity of the same index;
  * lookup refs (`Tx::lookup(attr, v)` in transactions, `Query::lookup` in queries) name an entity by a unique attribute's value, resolved against
    the database before the transaction;
  * `retract_entity` retracts an entity's datoms and the refs to it (through VAET), and cascades to the entities it owns through component attributes;
  * conflicting operations are errors: a datom both asserted and retracted, two values of a cardinality-one attribute, and a unique value
    that would belong to two entities after the transaction (checked on the result, so handing a value over in one transaction is fine);
  * every transaction is an entity with a `db/txInstant`, an Instant supplied by the caller (`Tx::with_instant`; by default the previous instant;
    it may not go backwards), which `Datom::tx` refers to and the transaction itself can annotate (`Entity::Tx`). `db/` names are reserved;
  * compare-and-swap (`Tx::cas(e, attr, expected, new)`, a cardinality-one attribute; `expected` None means no value) checks the value
    before the transaction and fails the whole transaction with `Error::CasFailed` otherwise;
  * schema alteration (`Tx::alter_attr`): an attribute's cardinality, uniqueness and component flag may change, not its value type.
    The change is validated against the attribute's data before the transaction (cardinality one: no entity with two values; unique: no value
    held by two entities), else `Error::AlterRefused`; the transaction's own data is then checked under the new schema. Past values keep their
    schema, and the durable log records alterations like definitions.
* **History and time** (`src/db.rs`). The current indexes map each datom to the transaction that asserted it, and five append-only
  history indexes (EAVT/AEVT/AVET/VAET order with keys ending in `(tx, added)`, and the log in transaction order) keep every assertion and
  retraction, all persistent; a schema history keeps every definition and alteration. Three views, each a cheap clone with a marker, filter the reads:
  * `Db::history()`: every change, with its `added` flag (and history statistics for the planner);
  * `Db::since(t)`: only the datoms of transactions after basis `t`, for every read, pull included;
  * `Db::as_of(t)`: the value as of basis `t`, with the schema of the time: every read derives that time's datoms from the history indexes
    (a datom is current when its last change by then is an assertion). So `Conn` keeps only its latest value, and `Conn::as_of(t)` is a view
    of it. Its planner statistics are the latest value's, an estimate.

  The views compose (`as_of(t).since(s)`, `history().as_of(t)`, ...); transacting against one is an error. `Db::datoms(e, a, v)` scans a view.
  `Db::tx_range(from, to)` reads the log: each transaction of the bases `from..to` with its entity and datoms (as Datomic's `tx-range`).
  The views take a basis; `Db::basis_of_instant(i)` (the last basis whose transaction is at or before instant `i`) and `Db::basis_of_tx(tx)`
  give the basis of a time or a transaction entity, so `db.as_of(db.basis_of_instant(i))` is the database at time `i` (binary searches,
  as instants never decrease and transaction entities increase with the basis).
* **Durability** (`src/log.rs`). `Conn::open(path)` backs a connection with an append-only log file (created if missing); `Conn::new()` stays in memory.
  * Each committed transaction is one record holding its *effect*: basis, transaction entity, the next entity id after it (tempids that
    unified away or only appear in `retract_entity` still use up ids), instant, the attributes it defined (with their ids), and the datoms
    it retracted and asserted. Replay doesn't resolve tempids, lookups or upserts again.
  * `transact` appends the record and syncs it (`sync_data`) before the transaction becomes visible or `transact` returns.
    If the write fails, the file is cut back to the committed data and the transaction is an `Error::Io`. If even the cut fails, the log
    refuses further appends until it is reopened.
  * Framing: a file header (magic, format version), then per record its length, a CRC-32 of the length, a CRC-32 of the payload, and the payload.
    The length's own checksum is what tells a corrupted length (which may point past the end of the file) from a torn record.
  * Recovery on open: a final record cut short (fewer bytes than its header, or than its valid header's length) is a torn, never
    acknowledged append, and is truncated away; so is a partial file header. Any other damage is `Error::CorruptLog { offset, msg }` at the
    record's offset, and the file is left untouched. That covers a checksum failure (in the last complete record too, since every
    acknowledged record was synced), a payload that doesn't decode, and a record that `Db::replay` refuses because it doesn't fit the value
    before it (basis, entity ids, schema, types, retracting an absent datom, asserting a present one, broken cardinality or uniqueness).
  * Replay applies records with the same code as `transact`, so a reopened connection has the same values at every basis, down to the
    index contents and statistics.
  * The file is locked while open (`File::try_lock`), so a second connection to it is an `Error::Io`.
  * Checkpoints (`Conn::checkpoint`, explicit): a file next to the log (`<log>.ckpt`) with a snapshot of the current value: only what can't
    be derived (counters, schema history, transactions, and the history in log order). It is written to a temporary file, synced and renamed.
    On open, an intact checkpoint (both checksums) whose covered log prefix ends at a record boundary, has the CRC-32 the checkpoint records
    (so it is *this* log's), and whose basis is the number of records it covers is used: `Db::from_snapshot` sorts the history into each index's
    order and builds every map in linear time (`PMap::from_sorted`), deriving the current indexes and the statistics, and only the later
    records are replayed. Any other checkpoint is silently ignored and the log replayed in full: the log holds the data, the checkpoint is a cache.
    The rebuild is a second implementation of the indexes, which the tests compare with `transact`'s at every basis of random workloads.

    Timings (release, transactions of 20 assertions, `cargo test --release open_timings -- --ignored --nocapture`; opening by replay costs
    the path-copying inserts into every index, about 25 µs per datom at 50k transactions, which the bulk build avoids):

    | transactions | log      | open by replay | checkpoint written | open from the checkpoint |
    |--------------|----------|----------------|--------------------|--------------------------|
    | 1,000        | 0.5 MB   | 139 ms         | 16 ms (0.7 MB)     | 16 ms                    |
    | 10,000       | 5.0 MB   | 3.96 s         | 62 ms (6.9 MB)     | 252 ms                   |
    | 50,000       | 25.1 MB  | 25.8 s         | 441 ms (34.5 MB)   | 1.20 s                   |
* **Pull** (`src/query.rs`). `Db::pull(pattern, eid)` reads an entity as a tree (`Pulled`): attribute lists, the wildcard (which pulls
  components recursively, stopping at cycles), nested patterns for Ref attributes, and reverse refs through VAET (`Pull::reverse`, key `ns/_name`).
  Cardinality-one attributes and reverse components give one value, the others a list; absent attributes are left out. The pattern is
  validated before anything is pulled.
* **Persistent indexes** (`src/pmap.rs`). A treap with `Arc`-shared nodes and hash-of-key priorities: an update copies one root-to-leaf path,
  and the tree's shape depends on its key set only.
* **Queries** (`src/query.rs`). Datalog-style queries over any view: patterns `[e attr v]` (or `[e attr v tx added]`, `Query::pattern_tx`),
  comparison predicates, `not`/`not-join` and `or`/`or-join` (nested to any depth) and rule calls (see Rules), run as nested loops, each pattern using the index selected by its bound positions.
  * `not` keeps the rows for which its body has no solution; its join variables (all of its body's, or the `not-join` list) must be bound when it runs,
    and the body's other variables are its own. `or` is the union of its branches on its join variables (the branches' variables, which must be
    the same in every branch, or the `or-join` list). An `or` whose every branch binds every join variable by itself *binds* them, like a pattern;
    any other `or` is a filter, which needs them bound, like a `not`.
  * To the SAT planner, a filter is a clause that needs its join variables (like a predicate: pending until they have binders) and a binding `or`
    is a clause that binds them, at a cost estimated from its branches; no clause both needs and binds, which keeps the encoding sound.
    Each run plans the bodies once (with the SAT planner, within a budget), counting the join variables bound where they run as constants,
    and runs them in an environment of their own.
  * Find specs (`Query::find`) take plain terms and the aggregates `count`, `count-distinct`, `sum`, `min`, `max` and `avg`, grouped by the plain terms,
    over the distinct tuples of the find and `with` variables, as in Datomic. `sum` of Ints is an exact Int (an error on overflow), and a Float
    once any value is a Float; `avg` is a Float; both take only numbers.
* **Text queries** (`src/parse.rs`). `Db::q(text, inputs)` reads a subset of Datomic's Datalog in EDN into the builder, clause by clause (so it is planned
  as it is read): `[:find ?a (count ?b) :with ?c :in $ ?x :where [?e :attr ?v] [(> ?v ?x)] (not ...) (or-join [?e] ... (and ...))]`, with `_`
  (a variable of its own at each occurrence, as is each missing position of a short pattern, and never a join variable of an implicit `not` or `or`,
  so `(not [?p :person/friend _])` keeps the people without friends, as in Datomic), 2- to 5-element patterns, lookup refs `[:attr value]`, string, integer, float (`1.5`, `-2e3`), boolean and instant (`#inst "2024-02-29T13:45:00Z"`, RFC 3339, or a date alone) constants, and scalar inputs, which become constants of the query.
  Syntax errors carry their line and column; keywords only name attributes (there is no keyword value type).
  `Db::q_rules(text, rules, inputs)` takes a rule set as `%`, written as in Datomic: `[[(reach ?a ?b) [?a :edge ?b]] [(reach ?a ?b) [?a :edge ?c] (reach ?c ?b)]]`.
* **Rules** (`src/rules.rs`). `Query::rule(name, head, body)` defines a rule (several definitions are a union) and `Query::call`/`call_clause`
  calls one, from the query, a `not`, an `or` or another rule's body; recursion and mutual recursion are allowed. To the planner a call is a binder,
  like a pattern. When a clause calling rules is added, the rules it depends on are evaluated bottom-up, so the online planner estimates
  the call with the relation's real size (and the rules are fixed from then on):
  * strata: mutually recursive rules (the call graph's strongly connected components) are evaluated together, after the rules they call;
    a rule that depends on itself through `not` has no stratification and is an error;
  * semi-naive evaluation to a fixpoint: round 0 runs every definition with the stratum's relations empty, then each positive occurrence of
    a call into the stratum (in an `or` branch too) gives a variant that reads only the previous round's new tuples there, until a round finds nothing new;
  * each body (and each variant) is planned by the SAT planner once per stratum, with the relations' sizes after round 0, and the plans are reused
    in every round (`Query::rule_stats` counts strata, rounds and plans);
  * the relations keep a hash index per combination of bound argument positions that a call uses, extended as tuples arrive.
  A rule's variables are its own (its body runs in an environment of its own), and every head variable must be bound by the body.
* **The planner** (`src/planner.rs`). It chooses the join order *while the query is being assembled*: every added clause is followed by
  a refinement, so a good plan exists before the query is complete, and `run` finishes the optimization within `RUN_BUDGET` (10k conflicts:
  the optimum up to 16 clauses, the best plan found past that, where SAT rarely proves the optimum); `Query::optimize(u64::MAX)` asks for the optimum.
  * Queries of up to 16 clauses (`DP_MAX`) are ordered exactly by a dynamic program over the subsets of clauses (a clause's cost depends only on
    the set before it and that set's size), constrained or not: 10 µs to 2.5 ms in release builds, see the timings below.
  * Hard ordering constraints: `Query::require_before(a, b)` makes clause `a` run before clause `b`. A constraint under which no order is valid
    (with the other constraints and the clauses' needs: a predicate, `not` or filtering `or` after a binder of each of its variables) is refused
    with `Error::OrderConflict`, naming a minimal conflicting subset of the constraints, and the query is unchanged. A predicate still pending
    (a variable without binder) must follow the binders its other variables have, so clauses added later never break accepted constraints.
    Feasibility itself is decided exactly by the greedy order (being able to run is monotone in the set of clauses that ran); the core comes
    from the SAT solver (`IsAssumptionRequired` over the constraints' assumptions, shrunk by deletion) and is re-confirmed by that test,
    falling back to deletion with the test if the solver runs out of budget.
  * Larger queries use one incremental SAT solver per query, encoded when first needed and then extended clause by clause (nothing is retracted),
    with a refinement bounded by a conflict budget after each clause (each solve is charged at least one conflict, so the budget also bounds
    the number of solves, and a spent budget solves nothing; a finite budget is clamped so that the solver's threshold, its conflict count
    plus the budget, doesn't wrap; and a solve can overshoot by the few conflicts of its last round of conflict analysis).
    The module documentation has the details; in short:
    * the order is an acyclic tournament (one variable per pair of clauses), so adding a clause only adds variables and clauses;
    * "variable bound before clause c" points to an open disjunction of the binders seen so far plus a *tail* literal assumed false,
      which a binder added later extends;
    * a predicate's requirement that its variables be bound is an assumption; a predicate with a variable that has no binder yet is
      pending (known in closed form, so no solve is spent finding it), and only the boundness of its other variables is assumed while the
      rest of the query is planned; an ordering constraint is an assumption too (`r -> before(a, b)`);
    * the cost, the sum of the estimated log-sizes of the intermediate results, is linear over pairwise indicators and is summed
      by a lower-bounding binary adder network, which grows with the query;
    * optimization tightens `sum <= K` bounds guarded by activation literals, starting below the greedy plan, until UNSAT (proved optimal)
      or until the conflict budget runs out (the best plan so far, which is never worse than greedy).

  Timings (release, random queries of the planner tests, `cargo test --release planner_timings -- --ignored --nocapture`):

  | clauses | DP (unconstrained / 2-3 constraints) | SAT to the optimum, or 200k conflicts |
  |---------|--------------------------------------|----------------------------------------|
  | 8       | 7-40 µs / 4 µs                       | 0.13-0.85 s, optimal                    |
  | 12      | 70-115 µs / 20-60 µs                 | 3-5.5 s, optimal in 1 of 6 runs (before this change: 12-595 s to the optimum) |
  | 16      | 0.8-2.5 ms / 0.2-1.1 ms              | 3-5 s, 1-13% above the optimum (before: no better than greedy in 300k conflicts) |
  | 20      | 28-120 ms / 11-75 ms                 | 3-5 s, 3-10% above the optimum           |

  So the DP also orders the small constrained queries (rather than SAT with the DP as a lower bound, which was measured and saved
  0.6 s in the one run where SAT reached the bound). Another lower bound that would end the SAT search early (each clause at its
  cheapest weight, the cheapest at the largest multipliers) is 2-11% below the optimum, which SAT's plans don't reach, so it isn't used either.

## Building and testing

The crate depends on the solver by path (`../rust`), with the solver's profiles mirrored (it relies on wrap-around arithmetic).

    cargo test              # unit tests, incl. the planner (SAT and the DP) against brute force (every order of random queries of up to 6 clauses,
                            # with random ordering constraints arriving between the clauses: acceptance, the minimality of each refused
                            # constraint's core and the constrained optimum), the DP against SAT on 8 clauses, the bounds of the online
                            # refinement on a 24-clause query, refinements of one conflict adding up to the optimum, a budget just below
                            # u64::MAX, a 17-clause query run within its budget against reachability, ordering constraints in queries,
                            # transactions and retract_entity against naive reference models, upserts and tempid unification
                            # through string and ref-valued identities against a naive fixpoint, foreign tempids and lookups, the history and since views of
                            # every past value against the transactions' logs, history queries and pull against brute force,
                            # random queries with (nested) not and or in every clause order against a bottom-up relational
                            # evaluator, aggregates against a hand grouping, text queries against the builder, blanks in not and or, transitive closure
                            # (four formulations, random graphs with cycles) against Floyd-Warshall, mutual recursion against a
                            # parity search, stratified negation, random rule programs against a naive simultaneous fixpoint,
                            # and the plan count of semi-naive evaluation on a long chain; the durable log: a random history reopened
                            # (every basis structurally equal to an in-memory connection's, and the next transaction's report identical),
                            # a file cut at every byte of its last record and header (truncated, then appendable), three changes of
                            # every byte of a middle record and damaged headers (reported at the record, file untouched), well-framed records that
                            # don't fit, decoding arbitrary bytes, injected write and truncation failures, and the file lock
                            # (test files go under target/satdb-log-tests/ and are removed)
    cargo test --release

## Status and next steps

This is a start. What is missing, roughly in order of value:

* Rules: demand-driven evaluation (magic sets, or rules with required variables `(name [?a] ?b)`): today a rule's whole relation is derived,
  whatever constants its calls have; rule sets shared between queries, and relations kept up to date across transactions instead of rederived
  per query; a better estimate of a call (only its first two argument positions are modeled, and the variants are planned with round 0's sizes,
  when the delta is the whole relation); aggregates in rule bodies.
* Query language: `or-join` with required variables (`[[?a] ?b]`: a clause that both needs and binds, which the planner's
  encoding would have to support: a binder's boundness justified only when its own requirements hold, pending computed as a fixpoint, and the
  brute-force test extended to such clauses); attribute variables in patterns; function and binding expressions (`[(f ?x) ?y]`);
  collection, tuple and relation inputs; scalar/collection find specs (`.`, `...`) and pull in the find spec;
  the aggregates `median`, `variance` and `stddev`. The cost model doesn't see inside `not` and `or`
  (a `not` has a fixed selectivity, a binding `or` the cost of its branches with nothing bound).
* Time: the planner's estimates for `since` and `as_of` views
  (they use the latest value's statistics). An as-of view derives each read from the history, so it costs more than a read of the current value.
  In a history view, the current-state accessors (`values`, `lookup`, ...) still read the current datoms; only scans and queries see the history.
* Pull: recursion limits (`...`), attribute options (defaults, limits, renaming) and pulling from a query's find spec.
* Schema: renaming attributes, altering the value type, and attribute retirement; user-defined transaction functions
  (CAS is the only built-in); a keyword value type.
* Planner scale beyond 16 clauses: the DP covers up to 16 clauses (20 would take up to 120 ms per refinement), and above that SAT
  ends 1-13% above the optimum after 200k conflicts and rarely proves it. The online refinement doesn't help there yet: on random
  24-clause queries (`cargo test --release online_refinement_budgets -- --ignored --nocapture`), no refinement after clauses 17 to 24
  beat the greedy plan, with 1k, 10k or 100k conflicts each (the first `sum < greedy` solve alone takes them all), and adding a clause
  resets the plan to the greedy one, dropping what earlier refinements found; so `run` past 16 clauses usually runs the greedy plan.
  Candidates: a local search (insertion moves) from the greedy plan before SAT, keeping the previous plan (with the new clause inserted)
  when a clause is added, core-guided optimization (lower bounds from cores;
  the simple rearrangement bound is too weak), a DP over connected subsets or with pruning by a bound, and a cheaper SAT refinement
  for each added clause (each solve re-propagates the whole encoding). (Measured earlier: lower-bounding adders instead of exact ones
  cut SAT's time from ~40 s; reducing the adder columns in rounds instead of chains made it much slower.)
* Ordering constraints: only `require_before` on the query's top-level clauses (not inside `not`, `or` or rule bodies), no text syntax,
  and no way to drop an accepted constraint.
* Cost model: independence assumptions and fixed predicate selectivities; no histograms.
* Storage: the log is durable (`Conn::open`) and checkpoints make opening fast, but the database is in memory, and so is its whole
  history (`Conn` keeps one value, whose history indexes serve every past one). Missing: index segments on disk, read lazily (so that memory
  isn't bounded by the database's size), log compaction (the log keeps every record, the checkpoint only caches them), an automatic
  checkpoint policy, access by several processes (today one connection holds the file's lock), and group commit (each transaction syncs on its own).
