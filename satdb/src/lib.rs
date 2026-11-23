//! satdb: a Datomic-inspired immutable database whose query planner runs on the Intel(R) SAT Solver.
//!
//! * `db`: database values (`Db`) made of datoms in persistent EAVT/AEVT/AVET/VAET indexes; `transact` applies a transaction
//!   (a set of assertions and retractions, with upserts, lookup refs, `retract_entity` and a reified transaction entity) and returns a new value,
//!   and `Conn` keeps every past value (`as_of`) at the cost of the changed index paths only. Every value also holds its history
//!   (every assertion and retraction, with its transaction), which the `history()` and `since(t)` views expose.
//! * `query`: Datalog-style queries (patterns, comparison predicates, `not`/`not-join`, `or`/`or-join`, over any view), planned online while
//!   they are assembled, with aggregates in the find spec; and `Db::pull`, which reads an entity as a tree.
//! * `parse`: the text syntax of queries (a subset of Datomic's Datalog, in EDN), read into the query builder (`Db::q`).
//! * `planner`: the incremental SAT encoding of the join-order problem (see its documentation).
//! * `pmap`: the persistent sorted map under the indexes.
//!
//! ```
//! use satdb::{Cardinality, Cmp, Conn, Tx, Value, ValueType};
//!
//! let mut conn = Conn::new();
//! let mut tx = Tx::new();
//! tx.define("person/name", ValueType::Str, Cardinality::One).define("person/age", ValueType::Int, Cardinality::One);
//! let (alice, bob) = (tx.temp(), tx.temp());
//! tx.add(alice, "person/name", "Alice").add(alice, "person/age", 30).add(bob, "person/name", "Bob").add(bob, "person/age", 50);
//! conn.transact(&tx).unwrap();
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
//! assert_eq!(conn.db().q(text, &[Value::from(20)]).unwrap(), vec![vec![Value::from(2)]]);
//! ```

pub mod db;
pub mod parse;
pub mod planner;
pub mod pmap;
pub mod query;

pub use db::{Attr, AttrId, AttrStats, Cardinality, Conn, Datom, Db, Eid, Entity, Error, Tx, TxReport, TxValue, Unique, Value, ValueType, T, TX_INSTANT};
pub use parse::TextQuery;
pub use planner::{Plan, PlanSource};
pub use query::{Agg, Clause, Cmp, Find, Pull, PullValue, Pulled, Query, Term};

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
