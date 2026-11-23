# satdb

A Datomic-inspired immutable database whose query planner runs on the Rust fork of the Intel(R) SAT Solver (`../rust`).

* **Database values** (`src/db.rs`). A `Db` is an immutable value: the current datoms `[e a v]` in four indexes (EAVT, AEVT, AVET, and VAET for refs; the history has three more, see below),
  the schema (value type, cardinality one/many, `Unique::Identity`/`Unique::Value`, component refs) and per-attribute statistics
  (datoms, distinct entities, distinct values). `Db::transact` returns a new value; `Conn` keeps every past one, so `as_of(t)` is a lookup.
* **Transactions** (`src/db.rs`). As in Datomic, a transaction is a *set* of assertions and retractions against the database before it,
  so the order of its operations doesn't matter:
  * a tempid asserting an existing `:db.unique/identity` value upserts to that entity, and tempids asserting the same new identity value
    unify into one entity (to a fixed point, so ref-valued identities chain); a tempid resolving to two entities is an `UpsertConflict`;
  * lookup refs (`Tx::lookup(attr, v)` in transactions, `Query::lookup` in queries) name an entity by a unique attribute's value, resolved against
    the database before the transaction;
  * `retract_entity` retracts an entity's datoms and the refs to it (through VAET), and cascades to the entities it owns through component attributes;
  * conflicting operations are errors: a datom both asserted and retracted, two values of a cardinality-one attribute, and a unique value
    that would belong to two entities after the transaction (checked on the result, so handing a value over in one transaction is fine);
  * every transaction is an entity with a `db/txInstant` supplied by the caller (`Tx::with_instant`; by default the previous instant;
    it may not go backwards), which `Datom::tx` refers to and the transaction itself can annotate (`Entity::Tx`). `db/` names are reserved.
* **History and time** (`src/db.rs`). The current indexes map each datom to the transaction that asserted it, and three append-only
  history indexes (EAVT/AEVT/AVET order, keys ending in `(tx, added)`) keep every assertion and retraction, all persistent, so a past value
  holds exactly the history up to its basis. Two views, each a cheap clone with a marker, filter the reads: `Db::history()` (every change, with
  its `added` flag; history statistics for the planner) and `Db::since(t)` (only the datoms of transactions after basis `t`, for every read,
  pull included); they compose. `Db::datoms(e, a, v)` scans a view; transacting against a view is an error.
* **Pull** (`src/query.rs`). `Db::pull(pattern, eid)` reads an entity as a tree (`Pulled`): attribute lists, the wildcard (which pulls
  components recursively, stopping at cycles), nested patterns for Ref attributes, and reverse refs through VAET (`Pull::reverse`, key `ns/_name`).
  Cardinality-one attributes and reverse components give one value, the others a list; absent attributes are left out. The pattern is
  validated before anything is pulled.
* **Persistent indexes** (`src/pmap.rs`). A treap with `Arc`-shared nodes and hash-of-key priorities: an update copies one root-to-leaf path,
  and the tree's shape depends on its key set only.
* **Queries** (`src/query.rs`). Datalog-style queries over any view: patterns `[e attr v]` (or `[e attr v tx added]`, `Query::pattern_tx`),
  comparison predicates, `not`/`not-join` and `or`/`or-join` (nested to any depth), run as nested loops, each pattern using the index selected by its bound positions.
  * `not` keeps the rows for which its body has no solution; its join variables (all of its body's, or the `not-join` list) must be bound when it runs,
    and the body's other variables are its own. `or` is the union of its branches on its join variables (the branches' variables, which must be
    the same in every branch, or the `or-join` list). An `or` whose every branch binds every join variable by itself *binds* them, like a pattern;
    any other `or` is a filter, which needs them bound, like a `not`.
  * To the SAT planner, a filter is a clause that needs its join variables (like a predicate: pending until they have binders) and a binding `or`
    is a clause that binds them, at a cost estimated from its branches; no clause both needs and binds, which keeps the encoding sound.
    Each run plans the bodies once (with the SAT planner, within a budget), counting the join variables bound where they run as constants,
    and runs them in an environment of their own.
  * Find specs (`Query::find`) take plain terms and the aggregates `count`, `count-distinct`, `sum`, `min`, `max` and `avg`, grouped by the plain terms,
    over the distinct tuples of the find and `with` variables, as in Datomic. There is no floating-point type, so `avg` is the integer mean rounded down.
* **Text queries** (`src/parse.rs`). `Db::q(text, inputs)` reads a subset of Datomic's Datalog in EDN into the builder, clause by clause (so it is planned
  as it is read): `[:find ?a (count ?b) :with ?c :in $ ?x :where [?e :attr ?v] [(> ?v ?x)] (not ...) (or-join [?e] ... (and ...))]`, with `_`,
  2- to 5-element patterns, lookup refs `[:attr value]`, string, integer and boolean constants, and scalar inputs, which become constants of the query.
  Syntax errors carry their line and column; keywords only name attributes (there is no keyword value type).
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

    cargo test              # unit tests, incl. the planner against brute force (every order of random queries of up to 6 clauses),
                            # transactions and retract_entity against naive reference models, the history and since views of
                            # every past value against the transactions' logs, history queries and pull against brute force,
                            # random queries with (nested) not and or in every clause order against a bottom-up relational
                            # evaluator, aggregates against a hand grouping, and text queries against the builder
    cargo test --release

## Status and next steps

This is a start. What is missing, roughly in order of value:

* Query language: rules and recursion; `or-join` with required variables (`[[?a] ?b]`: a clause that both needs and binds, which the planner's
  encoding would have to support: a binder's boundness justified only when its own requirements hold, pending computed as a fixpoint, and the
  brute-force test extended to such clauses); attribute variables in patterns; function and binding expressions (`[(f ?x) ?y]`);
  collection, tuple and relation inputs and `%` rules in `:in`; scalar/collection find specs (`.`, `...`) and pull in the find spec;
  a floating-point value type (for a true `avg`, and `median`/`variance`). The cost model doesn't see inside `not` and `or`
  (a `not` has a fixed selectivity, a binding `or` the cost of its branches with nothing bound).
* Time: `as_of` on a `Db` (today `Conn` keeps every past value; the history indexes would allow a filter instead) and by transaction
  entity or instant, a transaction-log API (Datomic's `tx-range`), and the planner's estimates for a `since` view (it uses the whole database's).
  In a history view, the current-state accessors (`values`, `lookup`, ...) still read the current datoms; only scans and queries see the history.
* Pull: recursion limits (`...`), attribute options (defaults, limits, renaming) and pulling from a query's find spec.
* Schema evolution (altering an attribute's cardinality or uniqueness, which needs validating the existing datoms); today a definition
  that differs from the existing one is an `AttrConflict`. Also: tempid unification through identity values that are themselves new tempids
  (a ref-valued identity pointing at a tempid that resolves to nothing existing doesn't unify), transaction functions (e.g. CAS),
  and an Instant value type.
* Hard user constraints in the planner (e.g., "run clause A before B"). They would be assumptions like the predicates' requirements,
  and that's where the UNSAT cores become non-trivial: today a predicate is unsatisfiable exactly when one of its variables has no binder,
  which the tests check against the cores.
* Planner scale: on the random 12-clause query of the tests, finding and proving the optimum takes about 6 s (release build);
  most of it is the first improvements over greedy and the final UNSAT proof. Within a conflict budget the result is the best plan so far.
  Candidates: core-guided optimization (lower bounds from cores), and exact DP over subsets for small queries, as a fast path and a cross-check.
  (Measured: lower-bounding adders instead of exact ones cut this from ~40 s; reducing the adder columns in rounds instead of chains made it much slower.)
* Cost model: independence assumptions and fixed predicate selectivities; no histograms.
* Storage: everything is in memory; no durable log or segment storage.
