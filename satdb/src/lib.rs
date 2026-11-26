//! satdb: a Datomic-inspired immutable database whose query planner runs on the Intel(R) SAT Solver.
//!
//! * `db`: database values (`Db`) made of datoms (Bool, Int, Float, Str, Instant and Ref values) in persistent EAVT/AEVT/AVET/VAET indexes; `transact` applies a transaction
//!   (a set of assertions and retractions, with upserts, lookup refs, `retract_entity` and a reified transaction entity) and returns a new value,
//!   sharing the unchanged index nodes. Every value also holds its history (every assertion and retraction, with its transaction, and every
//!   schema change), which the `history()`, `since(t)` and `as_of(t)` views expose; so `Conn` keeps only its latest value.
//!   `Conn::open(path)` makes the connection durable: it replays the log at `path` (from its checkpoint, `Conn::checkpoint`, when it has one),
//!   and every committed transaction is appended and synced before `transact` returns.
//! * `log` (private): the durable log's format, commit and recovery (a torn final record is truncated, any other damage refused), and checkpoints.
//! * `query`: Datalog-style queries (patterns, comparison predicates, `not`/`not-join`, `or`/`or-join`, over any view), planned online while
//!   they are assembled, with aggregates in the find spec; and `Db::pull`, which reads an entity as a tree (with attribute options and recursion).
//! * `rules`: Datalog rules, recursive and mutually recursive ones included, evaluated bottom-up (stratified, semi-naive,
//!   each body planned by the SAT planner once per stratum) when a query first calls them.
//! * `parse`: the text syntax of queries (a subset of Datomic's Datalog, in EDN), read into the query builder (`Db::q`, `Db::q_rules`), and of pull patterns (`Db::pull_str`).
//! * `planner`: the join order, by an exact dynamic program for queries of up to 16 clauses and an incremental SAT encoding beyond,
//!   with hard ordering constraints (`Query::require_before`), whose conflicts the SAT solver's UNSAT cores explain (see its documentation).
//! * `pmap`: the persistent sorted map under the current indexes, a B+-tree with in-place updates of unshared nodes; `bemap`: a persistent
//!   Bε-tree measured against it and not used (see its documentation).
//! * `hist` and `succinct` (private): the history indexes, which only grow: immutable compressed segments (Elias-Fano sequences and bit-packed rows) and a small tail.
//!
//! ```
//! use satdb::{Cardinality, Cmp, Conn, Entity, Tx, Value, ValueType};
//!
//! let mut conn = Conn::new();
//! let mut tx = Tx::new();
//! tx.define("person/name", ValueType::Str, Cardinality::One).define("person/age", ValueType::Int, Cardinality::One);
//! let (alice, bob) = (tx.temp(), tx.temp());
//! tx.add(alice, "person/name", "Alice").add(alice, "person/age", 30).add(bob, "person/name", "Bob").add(bob, "person/age", 50);
//! let first = conn.transact(&tx).unwrap();
//!
//! let mut q = conn.db().query();
//! let (p, name, age) = (q.var("?p"), q.var("?name"), q.var("?age"));
//! q.pattern(p.clone(), "person/age", age.clone()).unwrap();
//! q.pred(age.clone(), Cmp::Gt, 40).unwrap();
//! q.pattern(p.clone(), "person/name", name.clone()).unwrap();
//! assert_eq!(q.run(&[name]).unwrap(), vec![vec![Value::from("Bob")]]);
//! assert!(q.plan().optimal);
//!
//! // The same query as text, with an input and an aggregate
//! let text = "[:find (count ?p) :in $ ?min :where [?p :person/age ?age] [(> ?age ?min)] (not [?p :person/name \"Carol\"])]";
//! assert_eq!(conn.db().q(text, &[Value::from(20).into()]).unwrap(), vec![vec![Value::from(2)]]);
//!
//! // A recursive rule: everyone above Alice in a chain of bosses
//! let (alice, bob) = (Entity::Id(first.eid(alice)), Entity::Id(first.eid(bob)));
//! let mut tx = Tx::new();
//! let carol = tx.temp();
//! tx.define("person/boss", ValueType::Ref, Cardinality::One);
//! tx.add(carol, "person/name", "Carol").add(bob, "person/boss", carol).add(alice, "person/boss", bob);
//! conn.transact(&tx).unwrap();
//! let rules = "[[(above ?a ?b) [?a :person/boss ?b]]
//!               [(above ?a ?b) [?a :person/boss ?c] (above ?c ?b)]]";
//! let text = "[:find ?name :in $ % :where [?a :person/name \"Alice\"] (above ?a ?b) [?b :person/name ?name]]";
//! assert_eq!(conn.db().q_rules(text, rules, &[]).unwrap(), vec![vec![Value::from("Bob")], vec![Value::from("Carol")]]);
//! ```

pub mod db;
mod hist;
mod log;
pub mod parse;
pub mod planner;
pub mod bemap;
pub mod pmap;
pub mod query;
pub mod rules;
mod succinct;

pub use db::{Attr, AttrId, AttrStats, Cardinality, Conn, Datom, Db, Eid, Entity, Error, Tx, TxLogEntry, TxReport, TxValue, Unique, Value, ValueType, T, TX_INSTANT};
pub use parse::{Cell, Found, Input, TextQuery};
pub use planner::{Plan, PlanSource};
pub use query::{Agg, Clause, Cmp, Find, Nested, Pull, PullOpts, PullValue, Pulled, Query, Term};
pub use rules::RuleStats;

#[cfg(test)]
pub(crate) mod testutil
{
	/// A tiny deterministic generator (xorshift), to avoid a dependency
	pub(crate) struct Rng(u64);

	impl Rng
	{
		pub(crate) fn new(seed: u64) -> Self
		{
			Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
		}

		pub(crate) fn next(&mut self) -> u64
		{
			self.0 ^= self.0 << 13;
			self.0 ^= self.0 >> 7;
			self.0 ^= self.0 << 17;
			self.0
		}
	}
}
