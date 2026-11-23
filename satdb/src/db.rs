//! The database value: the current datoms in four persistent indexes, the history in three more, the schema and the per-attribute statistics the planner estimates with.
//!
//! A `Db` is immutable. `transact` returns a new `Db` that shares all the unchanged index nodes with the old one,
//! so keeping every past version (see `Conn::as_of`) costs only the changed paths.
//!
//! # Transactions
//! A transaction is a *set* of assertions and retractions, all of them against the database before it (as in Datomic):
//! their order doesn't matter, and operations that contradict each other are an error instead of "the last one wins".
//! `transact` runs in phases:
//! 1. The attribute definitions are applied, then the alterations (`Tx::alter_attr`: cardinality, uniqueness and the component flag may change,
//!    validated against the attribute's current data).
//! 2. Lookup refs (`[attr value]` of a unique attribute) resolve against the database before the transaction;
//!    one that doesn't resolve is an error, even if the transaction itself asserts the value.
//! 3. Tempids resolve: a tempid asserting a `Unique::Identity` value that some entity already has *upserts* to that entity,
//!    and tempids asserting the same new identity value unify into one new entity (to a fixed point, as identity values may be refs to tempids).
//!    The tempids that remain are allocated, after the transaction's own entity. Then each compare-and-swap (`Tx::cas`) checks its entity's
//!    value before the transaction against the expected one.
//! 4. `retract_entity` expands into the entity's datoms, the datoms referring to it (through the VAET index) and, recursively,
//!    the entities it owns through component attributes.
//! 5. The conflicts are checked: a datom both asserted and retracted, two values of a cardinality-one attribute of an entity,
//!    and any operation on a `db/` attribute (which only the library writes).
//! 6. The changes are applied (a cardinality-one assertion retracts the entity's previous value), the transaction entity gets its `db/txInstant`,
//!    and the unique attributes are checked on the result: a value may belong to one entity at most.
//!
//! # Time
//! The current indexes map each datom to the transaction that asserted it, and three more persistent indexes (EAVT, AEVT and AVET order,
//! each key ending in (tx, added)) hold every assertion and retraction ever applied: the history only grows, so a past value
//! (`Conn::as_of`) holds exactly the history up to its basis, sharing the nodes with the later ones. Two views filter what reads see:
//! * `history()`: `datoms` and queries see the history, with the `added` flag (and history statistics for the planner);
//! * `since(t)`: every read sees only the datoms of the transactions after basis t (the transactions' entities increase with t).
//!
//! * `as_of(t)`: the value as of basis t, its schema included (a schema history records every definition and alteration):
//!   every read derives the datoms of the time from the history indexes (a datom is current when its last change by then is an assertion).
//!   So a connection keeps only its latest value, and every past one is a view of it.
//!
//! A view is the same value with a marker (a clone of a few `Arc`s); views compose, and transacting against one is an error.
//! More history indexes serve the views: VAET order (reverse refs as of a time) and transaction order, the log (`tx_range`).
//!
//! # Durability
//! `Conn::open` backs a connection with a log file (see `log`): each committed transaction's effect (`TxRecord`: its entity, the next entity id,
//! its instant, the attributes it defined and the datoms it retracted and asserted) is appended and synced before the transaction is visible,
//! and opening replays the records through `Db::replay`, which validates each one against the value before it and applies it with the same
//! code as `transact`, so a reopened connection has the same values, down to the index contents and statistics.
//! `Conn::checkpoint` writes a `Snapshot` next to the log, from which `Db::from_snapshot` rebuilds the value without replaying (see `log`).

use crate::log::Log;
use crate::pmap::PMap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub type Eid = u64;
pub type AttrId = u32;
/// The basis of a database value: the number of transactions applied to it
pub type T = u64;

/// The built-in attribute holding the instant of every transaction entity (an Instant, milliseconds since the Unix epoch)
pub const TX_INSTANT: &str = "db/txInstant";

/// Values of different types order by their type (in the declaration order below). Floats are totally ordered (`f64::total_cmp`:
/// -0.0 before 0.0, NaNs at the ends), so that they can be index keys; equality and hashing follow that order (a value equals only itself).
/// (Comparison predicates compare an Int with a Float numerically, see `query::Cmp`.)
#[derive(Clone, Debug)]
pub enum Value
{
	Bool(bool),
	Int(i64),
	Float(f64),
	Str(Arc<str>),
	/// A point in time, in milliseconds since the Unix epoch (`#inst` in text queries)
	Instant(i64),
	Ref(Eid),
}

impl Value
{
	pub fn value_type(&self) -> ValueType
	{
		match self
		{
			Value::Bool(_) => ValueType::Bool,
			Value::Int(_) => ValueType::Int,
			Value::Float(_) => ValueType::Float,
			Value::Str(_) => ValueType::Str,
			Value::Instant(_) => ValueType::Instant,
			Value::Ref(_) => ValueType::Ref,
		}
	}

	fn rank(&self) -> u8
	{
		self.value_type() as u8
	}
}

impl Ord for Value
{
	fn cmp(&self, other: &Self) -> std::cmp::Ordering
	{
		match (self, other)
		{
			(Value::Bool(a), Value::Bool(b)) => a.cmp(b),
			(Value::Int(a), Value::Int(b)) | (Value::Instant(a), Value::Instant(b)) => a.cmp(b),
			(Value::Float(a), Value::Float(b)) => a.total_cmp(b),
			(Value::Str(a), Value::Str(b)) => a.cmp(b),
			(Value::Ref(a), Value::Ref(b)) => a.cmp(b),
			_ => self.rank().cmp(&other.rank()),
		}
	}
}

impl PartialOrd for Value
{
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering>
	{
		Some(self.cmp(other))
	}
}

impl PartialEq for Value
{
	fn eq(&self, other: &Self) -> bool
	{
		self.cmp(other).is_eq()
	}
}

impl Eq for Value {}

impl std::hash::Hash for Value
{
	fn hash<H: std::hash::Hasher>(&self, h: &mut H)
	{
		self.rank().hash(h);
		match self
		{
			Value::Bool(b) => b.hash(h),
			Value::Int(i) | Value::Instant(i) => i.hash(h),
			// total_cmp is equal exactly for equal bits
			Value::Float(f) => f.to_bits().hash(h),
			Value::Str(s) => s.hash(h),
			Value::Ref(r) => r.hash(h),
		}
	}
}

impl From<f64> for Value
{
	fn from(f: f64) -> Self
	{
		Value::Float(f)
	}
}

impl From<bool> for Value
{
	fn from(b: bool) -> Self
	{
		Value::Bool(b)
	}
}

impl From<i64> for Value
{
	fn from(i: i64) -> Self
	{
		Value::Int(i)
	}
}

impl From<&str> for Value
{
	fn from(s: &str) -> Self
	{
		Value::Str(s.into())
	}
}

impl From<String> for Value
{
	fn from(s: String) -> Self
	{
		Value::Str(s.into())
	}
}

/// The value types, in the order their values sort in
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValueType
{
	Bool,
	Int,
	Float,
	Str,
	Instant,
	Ref,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cardinality
{
	/// Asserting a new value retracts the entity's previous value of the attribute
	One,
	Many,
}

/// A unique attribute: each value belongs to one entity at most
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unique
{
	/// :db.unique/identity: a tempid asserting an existing value upserts to the entity that has it
	Identity,
	/// :db.unique/value: asserting a value another entity has is an error
	Value,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attr
{
	pub name: Arc<str>,
	pub value_type: ValueType,
	pub cardinality: Cardinality,
	pub unique: Option<Unique>,
	/// A component reference: the referenced entity is part of the referring one, and `retract_entity` cascades to it
	pub component: bool,
}

impl Attr
{
	pub fn new(name: &str, value_type: ValueType, cardinality: Cardinality) -> Self
	{
		Attr { name: name.into(), value_type, cardinality, unique: None, component: false }
	}

	pub fn unique(mut self, unique: Unique) -> Self
	{
		self.unique = Some(unique);
		self
	}

	/// Only a Ref attribute can be a component
	pub fn component(mut self) -> Self
	{
		self.component = true;
		self
	}
}

/// The statistics of an attribute's current datoms
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct AttrStats
{
	pub datoms: u64,
	/// The number of distinct entities having the attribute
	pub entities: u64,
	/// The number of distinct values of the attribute
	pub values: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Datom
{
	pub e: Eid,
	pub a: AttrId,
	pub v: Value,
	/// The transaction's entity
	pub tx: Eid,
	/// false for a retraction
	pub added: bool,
}

/// One transaction of the log (`Db::tx_range`)
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TxLogEntry
{
	/// The basis the transaction produced
	pub t: T,
	/// The transaction's entity
	pub tx: Eid,
	/// Its datoms (its `db/txInstant` included), in EAV order
	pub data: Vec<Datom>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Error
{
	UnknownAttr(String),
	/// An attribute redefined with a different type, cardinality, uniqueness or component flag
	AttrConflict(String),
	/// A definition the schema doesn't allow: a reserved `db/` name, or a component attribute that isn't a Ref
	InvalidSchema(String),
	/// An assertion or retraction of a `db/` attribute, which only the library writes
	Reserved(String),
	UnknownEntity(Eid),
	/// A tempid or lookup ref of another transaction
	ForeignEntity(Entity),
	TypeMismatch { attr: String, expected: ValueType, got: ValueType },
	/// A lookup ref on an attribute that isn't unique
	NotUnique(String),
	/// A lookup ref that no entity matches
	LookupNotFound { attr: String, value: Value },
	/// A tempid whose identity values belong to two different entities
	UpsertConflict(Eid, Eid),
	/// A unique value that the transaction would give to two entities
	UniqueConflict { attr: String, value: Value },
	/// A datom both asserted and retracted by one transaction
	AddRetractConflict { e: Eid, attr: String, value: Value },
	/// Two values asserted for a cardinality-one attribute of an entity in one transaction
	CardinalityConflict { e: Eid, attr: String, values: (Value, Value) },
	/// A transaction instant before the previous transaction's
	InstantBackwards { previous: i64, got: i64 },
	/// A schema alteration (`Tx::alter_attr`) that the attribute or its current data don't allow, with the reason
	AlterRefused { attr: String, reason: String },
	/// A compare-and-swap (`Tx::cas`) whose entity's value of the attribute before the transaction isn't the expected one (None: no value)
	CasFailed { e: Eid, attr: String, expected: Option<Value>, found: Option<Value> },
	/// A transaction against a history, `since` or `as_of` view, or a pull from a history view
	FilteredDb,
	/// A variable that must be bound (by a predicate, a `not`, a filtering `or`, or the find list) but that no clause binds before it is needed
	Unbound(String),
	/// The SAT planner was disabled after a solver error; the message is the solver's explanation
	Planner(String),
	/// An ordering constraint (`Query::require_before`) under which no order of the clauses is valid; the pairs `(a, b)`, clause a before clause b,
	/// are a minimal conflicting subset of the query's constraints, the refused one included
	OrderConflict(Vec<(usize, usize)>),
	/// A query that is malformed beyond its terms' types: a variable of another query, `or` branches over different variables,
	/// an empty `not` or `or`, the wrong number of inputs, or an aggregate over values it can't combine
	InvalidQuery(String),
	/// A syntax error in a text query, at a 1-based line and column (in characters)
	Parse { line: usize, col: usize, msg: String },
	/// An I/O error of the durable log (the `std::io::Error`'s message); a transaction that fails with it isn't committed
	Io(String),
	/// A durable log that can't be replayed: a damaged file header or record, or a record that doesn't fit the database before it,
	/// at a byte offset of the file (the record's start); the file is left as it is
	CorruptLog { offset: u64, msg: String },
}

impl fmt::Display for Error
{
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result
	{
		match self
		{
			Error::UnknownAttr(a) => write!(f, "unknown attribute {a}"),
			Error::AttrConflict(a) => write!(f, "attribute {a} is already defined differently"),
			Error::InvalidSchema(a) => write!(f, "invalid definition of attribute {a}"),
			Error::Reserved(a) => write!(f, "attribute {a} is reserved"),
			Error::UnknownEntity(e) => write!(f, "entity {e} was never allocated"),
			Error::ForeignEntity(e) => write!(f, "{e:?} belongs to another transaction"),
			Error::TypeMismatch { attr, expected, got } => write!(f, "attribute {attr} expects {expected:?}, got {got:?}"),
			Error::NotUnique(a) => write!(f, "attribute {a} is not unique, so it can't be used in a lookup ref"),
			Error::LookupNotFound { attr, value } => write!(f, "no entity has {attr} = {value:?}"),
			Error::UpsertConflict(a, b) => write!(f, "a tempid upserts to both entity {a} and entity {b}"),
			Error::UniqueConflict { attr, value } => write!(f, "{attr} = {value:?} would belong to two entities"),
			Error::AddRetractConflict { e, attr, value } => write!(f, "[{e} {attr} {value:?}] is both asserted and retracted"),
			Error::CardinalityConflict { e, attr, values } => write!(f, "entity {e} gets two values of {attr}: {:?} and {:?}", values.0, values.1),
			Error::InstantBackwards { previous, got } => write!(f, "transaction instant {got} is before the previous one, {previous}"),
			Error::AlterRefused { attr, reason } => write!(f, "attribute {attr} can't be altered: {reason}"),
			Error::CasFailed { e, attr, expected, found } => write!(f, "compare-and-swap of {attr} of entity {e}: expected {expected:?}, found {found:?}"),
			Error::FilteredDb => write!(f, "a history, since or as-of view can't be transacted against, and a history view can't be pulled from"),
			Error::Unbound(v) => write!(f, "variable {v} is not bound where it is needed"),
			Error::Planner(m) => write!(f, "planner error: {m}"),
			Error::OrderConflict(pairs) =>
			{
				let pairs: Vec<String> = pairs.iter().map(|(a, b)| format!("clause {a} before clause {b}")).collect();
				write!(f, "conflicting ordering constraints (with the clauses' needs, no order satisfies them all): {}", pairs.join(", "))
			}
			Error::InvalidQuery(m) => write!(f, "invalid query: {m}"),
			Error::Parse { line, col, msg } => write!(f, "parse error at {line}:{col}: {msg}"),
			Error::Io(m) => write!(f, "log I/O error: {m}"),
			Error::CorruptLog { offset, msg } => write!(f, "corrupt log at byte {offset}: {msg}"),
		}
	}
}

impl std::error::Error for Error {}

type PSet<K> = PMap<K, ()>;

/// One of two iterators of the same items: the current indexes', or a derivation from the history's (in an `as_of` view)
enum Either<A, B>
{
	Now(A),
	Then(B),
}

impl<A: Iterator, B: Iterator<Item = A::Item>> Iterator for Either<A, B>
{
	type Item = A::Item;

	fn next(&mut self) -> Option<Self::Item>
	{
		match self
		{
			Either::Now(a) => a.next(),
			Either::Then(b) => b.next(),
		}
	}
}

#[derive(Clone)]
pub struct Db
{
	basis_t: T,
	next_eid: Eid,
	/// The instant of the last transaction (instants never decrease)
	instant: i64,
	attrs: PMap<AttrId, Attr>,
	attr_ids: PMap<Arc<str>, AttrId>,
	stats: PMap<AttrId, AttrStats>,
	// The current datoms, each mapped to the transaction that asserted it
	eavt: PMap<(Eid, AttrId, Value), Eid>,
	aevt: PMap<(AttrId, Eid, Value), Eid>,
	avet: PMap<(AttrId, Value, Eid), Eid>,
	/// The reverse index of the Ref datoms: (referenced entity, attribute, referring entity)
	vaet: PMap<(Eid, AttrId, Eid), Eid>,
	// The history: every assertion and retraction ever applied, as (.., tx, added); it only grows
	heavt: PSet<(Eid, AttrId, Value, Eid, bool)>,
	haevt: PSet<(AttrId, Eid, Value, Eid, bool)>,
	havet: PSet<(AttrId, Value, Eid, Eid, bool)>,
	/// The history of the Ref datoms in VAET order, (target, a, e, tx, added)
	hvaet: PSet<(Eid, AttrId, Eid, Eid, bool)>,
	/// The log: the history in transaction order, (tx, e, a, v, added)
	tlog: PSet<(Eid, Eid, AttrId, Value, bool)>,
	/// Every definition and alteration of an attribute, by the basis that made it
	schema: PMap<(AttrId, T), Attr>,
	/// The statistics of the history (distinct entities and values ever)
	hstats: PMap<AttrId, AttrStats>,
	/// The transaction entity of each basis t
	txs: PMap<T, Eid>,
	/// The view (see `history` and `since`): whether queries see the history, and the transaction after which datoms are visible (0: all)
	history: bool,
	since: Eid,
	/// An `as_of` view: the last transaction whose datoms are visible (0: none); its reads derive the datoms from the history
	upto: Option<Eid>,
}

impl Default for Db
{
	fn default() -> Self
	{
		Self::new()
	}
}

/// An entity in a transaction: an existing one, a new one (which the transaction allocates or upserts),
/// a lookup ref (`Tx::lookup`) or the transaction's own entity.
/// A tempid or a lookup ref carries the identity of its `Tx` (unique in the process) and its index in it,
/// so that using it in another transaction is an `Error::ForeignEntity` rather than another entity of that transaction
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entity
{
	Id(Eid),
	/// (transaction, index)
	Temp(u64, u32),
	/// (transaction, index)
	Lookup(u64, u32),
	Tx,
}

/// A value in a transaction: either a value, or a reference to an entity of the transaction (see `Entity`)
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TxValue
{
	Val(Value),
	Entity(Entity),
}

impl From<Entity> for TxValue
{
	fn from(e: Entity) -> Self
	{
		match e
		{
			Entity::Id(e) => TxValue::Val(Value::Ref(e)),
			e => TxValue::Entity(e),
		}
	}
}

impl From<Value> for TxValue
{
	fn from(v: Value) -> Self
	{
		TxValue::Val(v)
	}
}

impl From<bool> for TxValue
{
	fn from(b: bool) -> Self
	{
		TxValue::Val(b.into())
	}
}

impl From<f64> for TxValue
{
	fn from(f: f64) -> Self
	{
		TxValue::Val(f.into())
	}
}

impl From<i64> for TxValue
{
	fn from(i: i64) -> Self
	{
		TxValue::Val(i.into())
	}
}

impl From<&str> for TxValue
{
	fn from(s: &str) -> Self
	{
		TxValue::Val(s.into())
	}
}

enum Op
{
	/// (asserted, entity, attribute, value)
	Datom(bool, Entity, Arc<str>, TxValue),
	RetractEntity(Entity),
	/// (entity, attribute, expected value before the transaction, new value)
	Cas(Entity, Arc<str>, Option<TxValue>, TxValue),
}

/// A transaction: attribute definitions (applied first), then a set of assertions and retractions (see the module documentation)
pub struct Tx
{
	/// The identity of this transaction's tempids and lookup refs
	id: u64,
	temps: u32,
	lookups: Vec<(Arc<str>, Value)>,
	defs: Vec<Attr>,
	alters: Vec<Attr>,
	ops: Vec<Op>,
	instant: Option<i64>,
}

impl Default for Tx
{
	fn default() -> Self
	{
		static NEXT_ID: AtomicU64 = AtomicU64::new(0);
		Tx { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), temps: 0, lookups: Vec::new(), defs: Vec::new(), alters: Vec::new(), ops: Vec::new(), instant: None }
	}
}

impl Tx
{
	pub fn new() -> Self
	{
		Self::default()
	}

	/// A new entity, allocated when the transaction is applied (unless it upserts to an existing one)
	pub fn temp(&mut self) -> Entity
	{
		self.temps += 1;
		Entity::Temp(self.id, self.temps - 1)
	}

	/// The lookup ref [attr v]: the entity whose unique attribute `attr` has the value `v` before the transaction
	pub fn lookup(&mut self, attr: &str, v: impl Into<Value>) -> Entity
	{
		self.lookups.push((attr.into(), v.into()));
		Entity::Lookup(self.id, self.lookups.len() as u32 - 1)
	}

	/// Define an attribute (neither unique nor a component); redefining an existing attribute identically is a no-op
	pub fn define(&mut self, name: &str, value_type: ValueType, cardinality: Cardinality) -> &mut Self
	{
		self.define_attr(Attr::new(name, value_type, cardinality))
	}

	pub fn define_attr(&mut self, attr: Attr) -> &mut Self
	{
		self.defs.push(attr);
		self
	}

	/// Alter an existing attribute (named by `attr.name`) to `attr`: its cardinality, uniqueness and component flag may change, its value type may not.
	/// The change is validated against the attribute's data before the transaction (applied after the definitions, before the data):
	/// cardinality one needs at most one value per entity, uniqueness at most one entity per value
	pub fn alter_attr(&mut self, attr: Attr) -> &mut Self
	{
		self.alters.push(attr);
		self
	}

	/// Compare-and-swap: assert `new` as the value of the cardinality-one attribute `attr` of `e`, provided that its value before the transaction
	/// is `expected` (None: no value), else the transaction fails with `Error::CasFailed`
	pub fn cas(&mut self, e: Entity, attr: &str, expected: Option<TxValue>, new: impl Into<TxValue>) -> &mut Self
	{
		self.ops.push(Op::Cas(e, attr.into(), expected, new.into()));
		self
	}

	pub fn add(&mut self, e: Entity, attr: &str, v: impl Into<TxValue>) -> &mut Self
	{
		self.ops.push(Op::Datom(true, e, attr.into(), v.into()));
		self
	}

	/// Retracting an absent datom is a no-op
	pub fn retract(&mut self, e: Entity, attr: &str, v: impl Into<TxValue>) -> &mut Self
	{
		self.ops.push(Op::Datom(false, e, attr.into(), v.into()));
		self
	}

	/// Retract every datom of `e`, every Ref datom referring to it, and (recursively) the entities its component attributes refer to
	pub fn retract_entity(&mut self, e: Entity) -> &mut Self
	{
		self.ops.push(Op::RetractEntity(e));
		self
	}

	/// The transaction's `db/txInstant` (by default the previous transaction's instant, or 0); it must not be before the previous one
	pub fn with_instant(&mut self, instant: i64) -> &mut Self
	{
		self.instant = Some(instant);
		self
	}
}

pub struct TxReport
{
	pub db_before: Db,
	pub db_after: Db,
	/// The transaction's entity
	pub tx: Eid,
	/// The entity of each `Tx::temp`, in order; tempids that upserted or unified share an entity
	pub tempids: Vec<Eid>,
	/// The entity of each `Tx::lookup`, in order
	pub lookups: Vec<Eid>,
	/// The transaction's `db/txInstant` datom, then the retractions and the assertions (each in EAV order)
	pub tx_data: Vec<Datom>,
	/// The identity of the `Tx` (see `Entity`)
	tx_id: u64,
}

impl TxReport
{
	/// The entity that `e` of this report's transaction resolved to (panics for an entity of another transaction)
	pub fn eid(&self, e: Entity) -> Eid
	{
		match e
		{
			Entity::Id(e) => e,
			Entity::Temp(x, t) if x == self.tx_id => self.tempids[t as usize],
			Entity::Lookup(x, l) if x == self.tx_id => self.lookups[l as usize],
			Entity::Tx => self.tx,
			_ => panic!("{e:?} belongs to another transaction"),
		}
	}
}

/// The effect of a committed transaction, which is what the durable log (see `log`) records and replays:
/// the transaction's resolved datoms rather than its operations, so replaying it doesn't depend on resolving tempids or lookups again
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct TxRecord
{
	pub(crate) basis_t: T,
	/// The transaction's entity
	pub(crate) tx: Eid,
	/// The next entity id after the transaction (tempids that only appear in retract_entity or unified away still use up ids)
	pub(crate) next_eid: Eid,
	pub(crate) instant: i64,
	/// The attributes the transaction defined, with their ids
	pub(crate) attrs: Vec<(AttrId, Attr)>,
	/// The datoms retracted, then asserted (each in EAV order), without the transaction's instant
	pub(crate) retracted: Vec<(Eid, AttrId, Value)>,
	pub(crate) asserted: Vec<(Eid, AttrId, Value)>,
}

impl TxReport
{
	/// The record of this transaction for the durable log (the tests compare it with what the connection logged)
	#[cfg(test)]
	pub(crate) fn record(&self) -> TxRecord
	{
		let (before, after) = (&self.db_before, &self.db_after);
		// The attributes the transaction defined or altered
		let attrs = (0..after.attrs.len() as AttrId).filter(|&a| before.try_attr(a) != Some(after.attr(a))).map(|a| (a, after.attr(a).clone())).collect();
		let instant = match self.tx_data[0].v
		{
			Value::Instant(i) => i,
			_ => unreachable!("the first datom is the transaction's instant"),
		};
		let changes = |added: bool| self.tx_data[1..].iter().filter(|d| d.added == added).map(|d| (d.e, d.a, d.v.clone())).collect();
		TxRecord { basis_t: after.basis_t, tx: self.tx, next_eid: after.next_eid, instant, attrs, retracted: changes(false), asserted: changes(true) }
	}
}

/// A transaction that `Db::plan` accepted: the value with its schema, basis and entity ids updated (still sharing the indexes of the value
/// before it), and the changes that `finish` applies, which can't fail
struct Planned
{
	db: Db,
	tx: Eid,
	instant: i64,
	retracted: Vec<(Eid, AttrId, Value)>,
	asserted: Vec<(Eid, AttrId, Value)>,
	tempids: Vec<Eid>,
	lookups: Vec<Eid>,
	tx_id: u64,
}

impl Planned
{
	/// The record of the transaction for the durable log, given the value before it
	fn record(&self, before: &Db) -> TxRecord
	{
		let after = &self.db;
		// The attributes the transaction defined or altered
		let attrs = (0..after.attrs.len() as AttrId).filter(|&a| before.try_attr(a) != Some(after.attr(a))).map(|a| (a, after.attr(a).clone())).collect();
		TxRecord {
			basis_t: after.basis_t,
			tx: self.tx,
			next_eid: after.next_eid,
			instant: self.instant,
			attrs,
			retracted: self.retracted.clone(),
			asserted: self.asserted.clone(),
		}
	}

	/// Apply the changes: in place, for the indexes that no other value shares (see `pmap`)
	fn finish(self, db_before: Db) -> TxReport
	{
		let Planned { mut db, tx, instant, retracted, asserted, tempids, lookups, tx_id } = self;
		db.apply(tx, instant, &retracted, &asserted);
		let instant_attr = db.attr_id(TX_INSTANT).expect("built in");
		let datom = |(e, a, v): (Eid, AttrId, Value), added: bool| Datom { e, a, v, tx, added };
		let mut tx_data = vec![datom((tx, instant_attr, Value::Instant(instant)), true)];
		tx_data.extend(retracted.into_iter().map(|d| datom(d, false)));
		tx_data.extend(asserted.into_iter().map(|d| datom(d, true)));
		TxReport { db_before, db_after: db, tx, tempids, lookups, tx_data, tx_id }
	}
}

/// The contents of a checkpoint (see `Db::snapshot`)
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Snapshot
{
	pub(crate) basis_t: T,
	pub(crate) next_eid: Eid,
	pub(crate) instant: i64,
	/// Every definition and alteration, by (attribute, basis)
	pub(crate) schema: Vec<((AttrId, T), Attr)>,
	/// The transaction entity of each basis
	pub(crate) txs: Vec<(T, Eid)>,
	/// Every change, in log order: (tx, e, a, v, added)
	pub(crate) history: Vec<(Eid, Eid, AttrId, Value, bool)>,
}

/// An entity of a transaction before its tempids are allocated
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Res
{
	Eid(Eid),
	Temp(u32),
}

/// A value of a transaction before its tempids are allocated
#[derive(Clone, PartialEq, Eq, Hash)]
enum ResVal
{
	Val(Value),
	Temp(u32),
}

struct ResOp
{
	added: bool,
	e: Res,
	a: AttrId,
	v: ResVal,
}

/// The tempid classes of a transaction: a union-find, with the existing entity (if any) each class has upserted to
struct Temps
{
	parent: Vec<u32>,
	upserted: Vec<Option<Eid>>,
}

impl Temps
{
	fn find(&mut self, t: u32) -> u32
	{
		let p = self.parent[t as usize];
		if p == t
		{
			return t;
		}
		let r = self.find(p);
		self.parent[t as usize] = r;
		r
	}

	/// The entity a tempid stands for so far: an existing one, or its class
	fn res(&mut self, t: u32) -> Res
	{
		let r = self.find(t);
		self.upserted[r as usize].map_or(Res::Temp(r), Res::Eid)
	}

	/// Make tempid `t` the same entity as `other`; returns whether anything changed
	fn unify(&mut self, t: u32, other: Res) -> Result<bool, Error>
	{
		let r = self.find(t);
		match other
		{
			Res::Eid(x) => match self.upserted[r as usize]
			{
				None =>
				{
					self.upserted[r as usize] = Some(x);
					Ok(true)
				}
				Some(y) if y == x => Ok(false),
				Some(y) => Err(Error::UpsertConflict(y, x)),
			},
			Res::Temp(u) =>
			{
				let s = self.find(u);
				if r == s
				{
					return Ok(false);
				}
				let up = match (self.upserted[r as usize], self.upserted[s as usize])
				{
					(Some(x), Some(y)) if x != y => return Err(Error::UpsertConflict(x, y)),
					(x, y) => x.or(y),
				};
				self.parent[s as usize] = r;
				self.upserted[r as usize] = up;
				Ok(true)
			}
		}
	}
}

impl Db
{
	pub fn new() -> Self
	{
		let mut db = Db {
			basis_t: 0,
			next_eid: 1,
			instant: 0,
			attrs: PMap::new(),
			attr_ids: PMap::new(),
			stats: PMap::new(),
			eavt: PMap::new(),
			aevt: PMap::new(),
			avet: PMap::new(),
			vaet: PMap::new(),
			heavt: PMap::new(),
			haevt: PMap::new(),
			havet: PMap::new(),
			hvaet: PMap::new(),
			tlog: PMap::new(),
			schema: PMap::new(),
			hstats: PMap::new(),
			txs: PMap::new(),
			history: false,
			since: 0,
			upto: None,
		};
		db.add_attr(Attr::new(TX_INSTANT, ValueType::Instant, Cardinality::One));
		db
	}

	pub fn basis_t(&self) -> T
	{
		self.basis_t
	}

	pub fn attr_id(&self, name: &str) -> Option<AttrId>
	{
		self.attr_ids.get(&Arc::from(name)).copied()
	}

	pub fn attr(&self, a: AttrId) -> &Attr
	{
		self.attrs.get(&a).expect("attribute ids come from this database")
	}

	/// The attribute with id `a`, if this database defines it
	pub fn try_attr(&self, a: AttrId) -> Option<&Attr>
	{
		self.attrs.get(&a)
	}

	/// The transaction entity of basis `t` (None for t = 0 and for a future t)
	pub fn tx_of(&self, t: T) -> Option<Eid>
	{
		if t > self.basis_t { None } else { self.txs.get(&t).copied() }
	}

	/// The database as of basis `t` (None if `t` is in the future): the datoms, the schema and the basis it had then.
	/// A view of this value, read from the history indexes, so any past value is available without keeping it;
	/// it composes with `history` and `since`, and with itself (the earlier basis wins). The planner's statistics stay this value's (an estimate).
	/// As of this value's own basis, it is this value
	pub fn as_of(&self, t: T) -> Option<Db>
	{
		if t > self.basis_t
		{
			return None;
		}
		if t == self.basis_t
		{
			return Some(self.clone());
		}
		let upto = if t == 0 { 0 } else { self.tx_of(t).expect("every basis up to this one has a transaction") };
		// The attributes as they were at t (the schema history is in (id, basis) order, so a later change overrides an earlier one)
		let (mut attrs, mut attr_ids) = (PMap::new(), PMap::new());
		for ((a, _), def) in self.schema.iter().filter(|((_, at), _)| *at <= t)
		{
			attrs.insert_mut(*a, def.clone());
			attr_ids.insert_mut(def.name.clone(), *a);
		}
		// Transaction t + 1's entity is the first id it allocated (before its tempids)
		let next_eid = self.txs.get(&(t + 1)).copied().unwrap_or(self.next_eid);
		let instant = self.instant_of(t);
		Some(Db { basis_t: t, next_eid, instant, attrs, attr_ids, upto: Some(upto), ..self.clone() })
	}

	// The `db/txInstant` of basis t's transaction (0 for basis 0), from the history (it is asserted once and never changes)
	fn instant_of(&self, t: T) -> i64
	{
		let Some(tx) = self.tx_of(t) else { return 0 };
		let a = self.attr_id(TX_INSTANT).expect("built in");
		match self.heavt.seek(|k| (k.0, k.1) < (tx, a)).next()
		{
			Some(((e, b, Value::Instant(i), _, true), _)) if (*e, *b) == (tx, a) => *i,
			_ => unreachable!("every transaction has its instant"),
		}
	}

	/// The basis as of an instant: the last one whose transaction's `db/txInstant` is at or before `instant` (0 if none is),
	/// for `as_of` and `since` by time. Instants never decrease with the basis, so it is a binary search
	pub fn basis_of_instant(&self, instant: i64) -> T
	{
		// The first basis after the instant, in 1..=basis_t + 1
		let (mut lo, mut hi) = (1, self.basis_t + 1);
		while lo < hi
		{
			let mid = lo + (hi - lo) / 2;
			if self.instant_of(mid) <= instant { lo = mid + 1 } else { hi = mid }
		}
		lo - 1
	}

	/// The basis that transaction entity `tx` produced (None if `tx` isn't a transaction of this value's history), for `as_of` and `since`
	/// by transaction. The transactions' entities increase with the basis, so it is a binary search
	pub fn basis_of_tx(&self, tx: Eid) -> Option<T>
	{
		let (mut lo, mut hi) = (1, self.basis_t + 1);
		while lo < hi
		{
			let mid = lo + (hi - lo) / 2;
			if self.tx_of(mid).expect("every basis up to this one has a transaction") < tx { lo = mid + 1 } else { hi = mid }
		}
		(lo <= self.basis_t && self.tx_of(lo) == Some(tx)).then_some(lo)
	}

	/// The datoms current as of transaction `upto`, from history entries in index order (each datom's entries together, in transaction order):
	/// a datom is current when its last change at or before `upto` is an assertion, which gives its transaction
	fn current_as_of<'a, K>(entries: impl Iterator<Item = &'a K>, same: impl Fn(&K, &K) -> bool, change: impl Fn(&K) -> (Eid, bool), upto: Eid) -> Vec<(&'a K, Eid)>
	{
		let mut out = Vec::new();
		let mut group: Option<&K> = None;
		let mut last: Option<(&K, Eid, bool)> = None;
		for k in entries
		{
			if group.map_or(true, |g| !same(g, k))
			{
				if let Some((d, tx, true)) = last.take()
				{
					out.push((d, tx));
				}
				group = Some(k);
			}
			let (tx, added) = change(k);
			if tx <= upto
			{
				last = Some((k, tx, added));
			}
		}
		if let Some((d, tx, true)) = last
		{
			out.push((d, tx));
		}
		out
	}

	/// The log: the transactions that produced the bases `from..to` (clipped to this value's history), each with its datoms.
	/// It reads the whole history up to this value's basis, whatever the view
	pub fn tx_range(&self, from: T, to: T) -> Vec<TxLogEntry>
	{
		(from.max(1)..to.min(self.basis_t + 1))
			.map(|t| {
				let tx = self.tx_of(t).expect("every basis up to this one has a transaction");
				let data = self.tlog.seek(|k| k.0 < tx).take_while(|(k, _)| k.0 == tx).map(|(k, _)| Datom { e: k.1, a: k.2, v: k.3.clone(), tx, added: k.4 }).collect();
				TxLogEntry { t, tx, data }
			})
			.collect()
	}

	/// The history view of this database: queries (and `datoms`) see every assertion and retraction up to its basis, with their `added` flags.
	/// The current-state accessors (`values`, `entities`, `lookup`, ...) are unaffected; `pull` and `transact` refuse a history view
	pub fn history(&self) -> Db
	{
		Db { history: true, ..self.clone() }
	}

	/// The view of the datoms of the transactions after basis `t` (None if `t` is in the future). It filters every read,
	/// the current-state accessors included, and composes with `history` (in either order) and with itself (the later basis wins)
	pub fn since(&self, t: T) -> Option<Db>
	{
		let tx = if t == 0 { 0 } else { self.tx_of(t)? };
		Some(Db { since: self.since.max(tx), ..self.clone() })
	}

	pub fn is_history(&self) -> bool
	{
		self.history
	}

	/// The statistics of the attribute's datoms in this view: the history's in a history view; in a `since` or `as_of` view,
	/// those of the whole (latest) database, an estimate
	pub fn stats(&self, a: AttrId) -> AttrStats
	{
		if self.history { &self.hstats } else { &self.stats }.get(&a).copied().unwrap_or_default()
	}

	/// The datoms of attribute `a` in this view whose entity is `e` and whose value is `v` (each when given), in index order.
	/// A datom of the current view has the transaction that asserted it and `added` true; a history view has every assertion and retraction
	pub fn datoms(&self, e: Option<Eid>, a: AttrId, v: Option<&Value>) -> Vec<Datom>
	{
		let d = |e: Eid, v: &Value, tx: Eid, added: bool| Datom { e, a, v: v.clone(), tx, added };
		// An as-of view reads the history, which gives the current datoms as of its basis
		let mut out: Vec<Datom> = match (self.history || self.upto.is_some(), e, v)
		{
			(false, Some(e), Some(v)) => self.eavt.get(&(e, a, v.clone())).map(|tx| d(e, v, *tx, true)).into_iter().collect(),
			(false, Some(e), None) => self.eavt.seek(|k| (k.0, k.1) < (e, a)).take_while(|(k, _)| (k.0, k.1) == (e, a)).map(|(k, tx)| d(e, &k.2, *tx, true)).collect(),
			(false, None, Some(v)) => self.avet.seek(|k| (k.0, &k.1) < (a, v)).take_while(|(k, _)| (k.0, &k.1) == (a, v)).map(|(k, tx)| d(k.2, v, *tx, true)).collect(),
			(false, None, None) => self.aevt.seek(|k| k.0 < a).take_while(|(k, _)| k.0 == a).map(|(k, tx)| d(k.1, &k.2, *tx, true)).collect(),
			(true, Some(e), Some(v)) =>
			{
				self.heavt.seek(|k| (k.0, k.1, &k.2) < (e, a, v)).take_while(|(k, _)| (k.0, k.1, &k.2) == (e, a, v)).map(|(k, _)| d(e, v, k.3, k.4)).collect()
			}
			(true, Some(e), None) => self.heavt.seek(|k| (k.0, k.1) < (e, a)).take_while(|(k, _)| (k.0, k.1) == (e, a)).map(|(k, _)| d(e, &k.2, k.3, k.4)).collect(),
			(true, None, Some(v)) => self.havet.seek(|k| (k.0, &k.1) < (a, v)).take_while(|(k, _)| (k.0, &k.1) == (a, v)).map(|(k, _)| d(k.2, v, k.3, k.4)).collect(),
			(true, None, None) => self.haevt.seek(|k| k.0 < a).take_while(|(k, _)| k.0 == a).map(|(k, _)| d(k.1, &k.2, k.3, k.4)).collect(),
		};
		match (self.upto, self.history)
		{
			(Some(upto), true) => out.retain(|d| d.tx <= upto),
			// The history entries of each datom are together, in transaction order
			(Some(upto), false) =>
			{
				out = Self::current_as_of(out.iter(), |x, y| (x.e, &x.v) == (y.e, &y.v), |d| (d.tx, d.added), upto).into_iter().map(|(x, tx)| Datom { tx, added: true, ..x.clone() }).collect()
			}
			(None, _) => {}
		}
		out.retain(|d| d.tx > self.since);
		out
	}

	// The current-state accessors: the current datoms of the database (after the `since` basis, in a since view)

	// Each accessor reads the current indexes, or in an as-of view derives the datoms from the history indexes (`current_as_of`),
	// whose entries are in the same order with each datom's changes together

	/// The values of attribute `a` of entity `e`
	pub fn values(&self, e: Eid, a: AttrId) -> impl Iterator<Item = &Value>
	{
		let since = self.since;
		match self.upto
		{
			None => Either::Now(self.eavt.seek(move |k| (k.0, k.1) < (e, a)).take_while(move |(k, _)| k.0 == e && k.1 == a).filter(move |(_, tx)| **tx > since).map(|(k, _)| &k.2)),
			Some(upto) =>
			{
				let hist = self.heavt.seek(move |k| (k.0, k.1) < (e, a)).take_while(move |(k, _)| (k.0, k.1) == (e, a)).map(|(k, _)| k);
				Either::Then(Self::current_as_of(hist, |x, y| x.2 == y.2, |k| (k.3, k.4), upto).into_iter().filter(move |(_, tx)| *tx > since).map(|(k, _)| &k.2))
			}
		}
	}

	/// All the (attribute, value) pairs of entity `e`
	pub fn entity_datoms(&self, e: Eid) -> impl Iterator<Item = (AttrId, &Value)>
	{
		let since = self.since;
		match self.upto
		{
			None => Either::Now(self.eavt.seek(move |k| k.0 < e).take_while(move |(k, _)| k.0 == e).filter(move |(_, tx)| **tx > since).map(|(k, _)| (k.1, &k.2))),
			Some(upto) =>
			{
				let hist = self.heavt.seek(move |k| k.0 < e).take_while(move |(k, _)| k.0 == e).map(|(k, _)| k);
				Either::Then(Self::current_as_of(hist, |x, y| (x.1, &x.2) == (y.1, &y.2), |k| (k.3, k.4), upto).into_iter().filter(move |(_, tx)| *tx > since).map(|(k, _)| (k.1, &k.2)))
			}
		}
	}

	/// The entities whose attribute `a` has the value `v`
	pub fn entities<'a>(&'a self, a: AttrId, v: &'a Value) -> impl Iterator<Item = Eid> + 'a
	{
		let since = self.since;
		match self.upto
		{
			None => Either::Now(
				self.avet.seek(move |k| k.0 < a || (k.0 == a && k.1 < *v)).take_while(move |(k, _)| k.0 == a && k.1 == *v).filter(move |(_, tx)| **tx > since).map(|(k, _)| k.2),
			),
			Some(upto) =>
			{
				let hist = self.havet.seek(move |k| (k.0, &k.1) < (a, v)).take_while(move |(k, _)| (k.0, &k.1) == (a, v)).map(|(k, _)| k);
				Either::Then(Self::current_as_of(hist, |x, y| x.2 == y.2, |k| (k.3, k.4), upto).into_iter().filter(move |(_, tx)| *tx > since).map(|(k, _)| k.2))
			}
		}
	}

	/// The (attribute, entity) pairs of the Ref datoms whose value is `target` (the VAET index)
	pub fn referrers(&self, target: Eid) -> impl Iterator<Item = (AttrId, Eid)> + '_
	{
		let since = self.since;
		match self.upto
		{
			None => Either::Now(self.vaet.seek(move |k| k.0 < target).take_while(move |(k, _)| k.0 == target).filter(move |(_, tx)| **tx > since).map(|(k, _)| (k.1, k.2))),
			Some(upto) =>
			{
				let hist = self.hvaet.seek(move |k| k.0 < target).take_while(move |(k, _)| k.0 == target).map(|(k, _)| k);
				Either::Then(Self::current_as_of(hist, |x, y| (x.1, x.2) == (y.1, y.2), |k| (k.3, k.4), upto).into_iter().filter(move |(_, tx)| *tx > since).map(|(k, _)| (k.1, k.2)))
			}
		}
	}

	/// All the (entity, value) pairs of attribute `a`
	pub fn attr_datoms(&self, a: AttrId) -> impl Iterator<Item = (Eid, &Value)>
	{
		let since = self.since;
		match self.upto
		{
			None => Either::Now(self.aevt.seek(move |k| k.0 < a).take_while(move |(k, _)| k.0 == a).filter(move |(_, tx)| **tx > since).map(|(k, _)| (k.1, &k.2))),
			Some(upto) =>
			{
				let hist = self.haevt.seek(move |k| k.0 < a).take_while(move |(k, _)| k.0 == a).map(|(k, _)| k);
				Either::Then(Self::current_as_of(hist, |x, y| (x.1, &x.2) == (y.1, &y.2), |k| (k.3, k.4), upto).into_iter().filter(move |(_, tx)| *tx > since).map(|(k, _)| (k.1, &k.2)))
			}
		}
	}

	pub fn has(&self, e: Eid, a: AttrId, v: &Value) -> bool
	{
		match self.upto
		{
			None => self.eavt.get(&(e, a, v.clone())).is_some_and(|tx| *tx > self.since),
			Some(upto) =>
			{
				let hist = self.heavt.seek(|k| (k.0, k.1, &k.2) < (e, a, v)).take_while(|(k, _)| (k.0, k.1, &k.2) == (e, a, v)).map(|(k, _)| k);
				Self::current_as_of(hist, |_, _| true, |k| (k.3, k.4), upto).first().is_some_and(|(_, tx)| *tx > self.since)
			}
		}
	}

	/// Resolve the lookup ref [attr v]: the entity whose unique attribute `attr` has the value `v`, if any
	pub fn lookup(&self, attr: &str, v: &Value) -> Result<Option<Eid>, Error>
	{
		let a = self.attr_id(attr).ok_or_else(|| Error::UnknownAttr(attr.to_string()))?;
		let def = self.attr(a);
		if def.unique.is_none()
		{
			return Err(Error::NotUnique(attr.to_string()));
		}
		if v.value_type() != def.value_type
		{
			return Err(Error::TypeMismatch { attr: attr.to_string(), expected: def.value_type, got: v.value_type() });
		}
		Ok(self.entities(a, v).next())
	}

	pub fn transact(&self, tx: &Tx) -> Result<TxReport, Error>
	{
		Ok(self.plan(tx)?.finish(self.clone()))
	}

	// Phases 1 to 5 and the checks of 6 (see the module documentation): everything that can fail, without changing any index.
	// The planned value shares this one's indexes until `Planned::finish` applies the changes
	fn plan(&self, tx: &Tx) -> Result<Planned, Error>
	{
		if self.history || self.since != 0 || self.upto.is_some()
		{
			return Err(Error::FilteredDb);
		}
		let mut db = self.clone();
		db.basis_t += 1;
		for def in &tx.defs
		{
			db.define(def)?;
		}
		for def in &tx.alters
		{
			db.alter(def)?;
		}
		let instant = tx.instant.unwrap_or(self.instant);
		if instant < self.instant
		{
			return Err(Error::InstantBackwards { previous: self.instant, got: instant });
		}
		db.instant = instant;
		// The transaction's entity is the first id it allocates (before its tempids): `as_of` relies on it for the next entity id of a past basis
		let tx_eid = db.next_eid;
		db.next_eid += 1;
		db.txs.insert_mut(db.basis_t, tx_eid);

		// Phase 2: the lookup refs, against the data before the transaction (db has only its schema changes so far)
		let lookups = tx
			.lookups
			.iter()
			.map(|(attr, v)| db.lookup(attr, v)?.ok_or_else(|| Error::LookupNotFound { attr: attr.to_string(), value: v.clone() }))
			.collect::<Result<Vec<Eid>, Error>>()?;
		let ent = |e: Entity| -> Result<Res, Error>
		{
			match e
			{
				Entity::Id(x) => Ok(Res::Eid(self.check_eid(x)?)),
				Entity::Temp(x, t) if x == tx.id && t < tx.temps => Ok(Res::Temp(t)),
				Entity::Lookup(x, l) if x == tx.id && (l as usize) < lookups.len() => Ok(Res::Eid(lookups[l as usize])),
				Entity::Tx => Ok(Res::Eid(tx_eid)),
				_ => Err(Error::ForeignEntity(e)),
			}
		};
		let val = |a: AttrId, v: &TxValue| -> Result<ResVal, Error>
		{
			let def = db.attr(a);
			let mismatch = |got: ValueType| Error::TypeMismatch { attr: def.name.to_string(), expected: def.value_type, got };
			Ok(match v
			{
				TxValue::Val(v) if v.value_type() != def.value_type => return Err(mismatch(v.value_type())),
				TxValue::Val(Value::Ref(r)) => ResVal::Val(Value::Ref(self.check_eid(*r)?)),
				TxValue::Val(v) => ResVal::Val(v.clone()),
				TxValue::Entity(_) if def.value_type != ValueType::Ref => return Err(mismatch(ValueType::Ref)),
				TxValue::Entity(r) => match ent(*r)?
				{
					Res::Eid(x) => ResVal::Val(Value::Ref(x)),
					Res::Temp(t) => ResVal::Temp(t),
				},
			})
		};
		// A `db/` attribute is reserved whatever the value (checked before the value's type)
		let attr_of = |attr: &str| match db.attr_id(attr)
		{
			Some(_) if attr.starts_with("db/") => Err(Error::Reserved(attr.to_string())),
			Some(a) => Ok(a),
			None => Err(Error::UnknownAttr(attr.to_string())),
		};
		let mut ops = Vec::new();
		let mut doomed = Vec::new();
		// The compare-and-swaps' checks: (entity, attribute, expected value), checked once the tempids are allocated
		let mut checks = Vec::new();
		for op in &tx.ops
		{
			match op
			{
				Op::Datom(added, e, attr, v) =>
				{
					let a = attr_of(attr)?;
					ops.push(ResOp { added: *added, e: ent(*e)?, a, v: val(a, v)? });
				}
				Op::RetractEntity(e) => doomed.push(ent(*e)?),
				Op::Cas(e, attr, expected, new) =>
				{
					let a = attr_of(attr)?;
					if db.attr(a).cardinality != Cardinality::One
					{
						return Err(Error::InvalidSchema(format!("{attr} (compare-and-swap needs a cardinality-one attribute)")));
					}
					let e = ent(*e)?;
					checks.push((e, a, expected.as_ref().map(|x| val(a, x)).transpose()?));
					ops.push(ResOp { added: true, e, a, v: val(a, new)? });
				}
			}
		}

		// Phase 3: upserts and tempid unification, to a fixed point (every round only merges classes or upserts one)
		let mut temps = Temps { parent: (0..tx.temps).collect(), upserted: vec![None; tx.temps as usize] };
		loop
		{
			let mut changed = false;
			// The identity values asserted so far this round, each with its holder; a value that is a new entity is its class
			// (which may be stale once classes merge later in the round, but then the round changed something and there is another)
			let mut holders: HashMap<(AttrId, ResVal), Res> = HashMap::new();
			for op in ops.iter().filter(|op| op.added && db.attr(op.a).unique == Some(Unique::Identity))
			{
				let v = match &op.v
				{
					ResVal::Val(v) => ResVal::Val(v.clone()),
					ResVal::Temp(t) => match temps.res(*t)
					{
						Res::Eid(x) => ResVal::Val(Value::Ref(x)),
						Res::Temp(r) => ResVal::Temp(r),
					},
				};
				let me = match op.e
				{
					Res::Temp(t) => temps.res(t),
					e => e,
				};
				// Only an existing value can belong to an existing entity
				let existing = match &v
				{
					ResVal::Val(x) => self.entities(op.a, x).next().map(Res::Eid),
					ResVal::Temp(_) => None,
				};
				let other = existing.or_else(|| holders.get(&(op.a, v.clone())).copied());
				match (op.e, other)
				{
					(Res::Temp(t), Some(o)) => changed |= temps.unify(t, o)?,
					(Res::Eid(_), Some(Res::Temp(t))) => changed |= temps.unify(t, op.e)?,
					// Two existing entities: the uniqueness check rejects it, if both keep the value
					_ => {}
				}
				holders.entry((op.a, v)).or_insert(me);
			}
			if !changed
			{
				break;
			}
		}
		let mut tempids = Vec::with_capacity(tx.temps as usize);
		for t in 0..tx.temps
		{
			let r = temps.find(t) as usize;
			if temps.upserted[r].is_none()
			{
				temps.upserted[r] = Some(db.next_eid);
				db.next_eid += 1;
			}
			tempids.push(temps.upserted[r].expect("just allocated"));
		}
		let eid = |e: Res| match e
		{
			Res::Eid(x) => x,
			Res::Temp(t) => tempids[t as usize],
		};
		// The compare-and-swaps, against the data before the transaction (a new entity has no values)
		for (e, a, expected) in checks
		{
			let e = eid(e);
			let expected = expected.map(|v| match v
			{
				ResVal::Val(v) => v,
				ResVal::Temp(t) => Value::Ref(tempids[t as usize]),
			});
			let found = self.values(e, a).next().cloned();
			if found != expected
			{
				return Err(Error::CasFailed { e, attr: db.attr_name(a), expected, found });
			}
		}

		let mut adds = BTreeSet::new();
		let mut rets = BTreeSet::new();
		for op in ops
		{
			let v = match op.v
			{
				ResVal::Val(v) => v,
				ResVal::Temp(t) => Value::Ref(tempids[t as usize]),
			};
			if op.added { &mut adds } else { &mut rets }.insert((eid(op.e), op.a, v));
		}
		// Phase 4: retract_entity, against the data before the transaction
		let mut stack: Vec<Eid> = doomed.into_iter().map(eid).collect();
		let mut seen = BTreeSet::new();
		while let Some(e) = stack.pop()
		{
			if !seen.insert(e)
			{
				continue;
			}
			for (a, v) in self.entity_datoms(e)
			{
				if let (true, Value::Ref(c)) = (self.attr(a).component, v)
				{
					stack.push(*c);
				}
				rets.insert((e, a, v.clone()));
			}
			for (a, x) in self.referrers(e)
			{
				rets.insert((x, a, Value::Ref(e)));
			}
		}

		// Phase 5: the conflicts
		if let Some((_, a, _)) = adds.iter().chain(&rets).find(|(_, a, _)| db.attr(*a).name.starts_with("db/"))
		{
			return Err(Error::Reserved(db.attr_name(*a)));
		}
		if let Some((e, a, v)) = adds.intersection(&rets).next()
		{
			return Err(Error::AddRetractConflict { e: *e, attr: db.attr_name(*a), value: v.clone() });
		}
		let mut prev: Option<&(Eid, AttrId, Value)> = None;
		for d in &adds
		{
			if let Some(p) = prev.filter(|p| (p.0, p.1) == (d.0, d.1) && db.attr(d.1).cardinality == Cardinality::One)
			{
				return Err(Error::CardinalityConflict { e: d.0, attr: db.attr_name(d.1), values: (p.2.clone(), d.2.clone()) });
			}
			prev = Some(d);
		}

		// Phase 6: the changes, and the unique attributes checked on the result before anything is applied
		let mut retracted: BTreeSet<(Eid, AttrId, Value)> = rets.into_iter().filter(|(e, a, v)| self.has(*e, *a, v)).collect();
		for (e, a, v) in &adds
		{
			if db.attr(*a).cardinality == Cardinality::One
			{
				retracted.extend(self.values(*e, *a).filter(|o| *o != v).map(|o| (*e, *a, o.clone())));
			}
		}
		let asserted: Vec<(Eid, AttrId, Value)> = adds.into_iter().filter(|(e, a, v)| !self.has(*e, *a, v)).collect();
		// The holders of a unique value after the transaction: those before it that keep it, and those that get it
		let mut gets: HashMap<(AttrId, &Value), Vec<Eid>> = HashMap::new();
		for (e, a, v) in asserted.iter().filter(|(_, a, _)| db.attr(*a).unique.is_some())
		{
			gets.entry((*a, v)).or_default().push(*e);
		}
		for ((a, v), new) in &gets
		{
			let kept = self.entities(*a, v).filter(|x| !retracted.contains(&(*x, *a, (*v).clone()))).count();
			if kept + new.len() > 1
			{
				return Err(Error::UniqueConflict { attr: db.attr_name(*a), value: (*v).clone() });
			}
		}
		drop(gets);
		Ok(Planned { db, tx: tx_eid, instant, retracted: retracted.into_iter().collect(), asserted, tempids, lookups, tx_id: tx.id })
	}

	fn define(&mut self, def: &Attr) -> Result<(), Error>
	{
		match self.attr_id(&def.name)
		{
			Some(a) if self.attr(a) == def => Ok(()),
			Some(_) => Err(Error::AttrConflict(def.name.to_string())),
			None if def.name.starts_with("db/") || (def.component && def.value_type != ValueType::Ref) => Err(Error::InvalidSchema(def.name.to_string())),
			None =>
			{
				self.add_attr(def.clone());
				Ok(())
			}
		}
	}

	// Alter an existing attribute, validated against its current data
	fn alter(&mut self, def: &Attr) -> Result<(), Error>
	{
		let a = self.attr_id(&def.name).ok_or_else(|| Error::UnknownAttr(def.name.to_string()))?;
		let refuse = |reason: String| Err(Error::AlterRefused { attr: def.name.to_string(), reason });
		let old = self.attr(a);
		if def.name.starts_with("db/")
		{
			return Err(Error::Reserved(def.name.to_string()));
		}
		if def.value_type != old.value_type
		{
			return refuse(format!("its value type is {:?} and can't change", old.value_type));
		}
		if def.component && def.value_type != ValueType::Ref
		{
			return Err(Error::InvalidSchema(def.name.to_string()));
		}
		if def.cardinality == Cardinality::One && old.cardinality == Cardinality::Many
		{
			// The AEVT order puts an entity's values together
			let mut prev: Option<(Eid, &Value)> = None;
			for (e, v) in self.attr_datoms(a)
			{
				if let Some((p, pv)) = prev.filter(|p| p.0 == e)
				{
					return refuse(format!("entity {p} has two values, {pv:?} and {v:?}"));
				}
				prev = Some((e, v));
			}
		}
		if def.unique.is_some() && old.unique.is_none()
		{
			let mut prev: Option<(&Value, Eid)> = None;
			for ((_, v, e), _) in self.avet.seek(|k| k.0 < a).take_while(|(k, _)| k.0 == a)
			{
				if let Some((pv, p)) = prev.filter(|p| p.0 == v)
				{
					return refuse(format!("entities {p} and {e} have the same value {pv:?}"));
				}
				prev = Some((v, *e));
			}
		}
		self.schema.insert_mut((a, self.basis_t), def.clone());
		self.attrs.insert_mut(a, def.clone());
		Ok(())
	}

	fn add_attr(&mut self, def: Attr)
	{
		let a = self.attrs.len() as AttrId;
		self.attr_ids.insert_mut(def.name.clone(), a);
		self.schema.insert_mut((a, self.basis_t), def.clone());
		self.attrs.insert_mut(a, def);
	}

	fn attr_name(&self, a: AttrId) -> String
	{
		self.attr(a).name.to_string()
	}

	pub(crate) fn check_eid(&self, e: Eid) -> Result<Eid, Error>
	{
		if e == 0 || e >= self.next_eid { Err(Error::UnknownEntity(e)) } else { Ok(e) }
	}

	// The datom changes of transaction tx (shared by transact and replay): the retractions of present datoms, the assertions of absent ones, and the instant
	// The retractions go one by one; the assertions (the instant included) and every change's history go in as one batch per index
	fn apply(&mut self, tx: Eid, instant: i64, retracted: &[(Eid, AttrId, Value)], asserted: &[(Eid, AttrId, Value)])
	{
		let retracted: Vec<(Eid, AttrId, Value)> = retracted.iter().filter(|(e, a, v)| self.remove(*e, *a, v)).cloned().collect();
		let instant_attr = self.attr_id(TX_INSTANT).expect("built in");
		let mut asserted = asserted.to_vec();
		asserted.push((tx, instant_attr, Value::Instant(instant)));
		self.insert_current(&asserted, tx);
		self.log_batch(tx, &retracted, &asserted);
	}

	/// Apply a transaction read back from the durable log: the database the transaction that wrote `rec` produced, if `self` is the database before it.
	/// A record that doesn't fit (its basis, entity ids, schema, value types, a retraction of an absent datom, an assertion of a present one,
	/// or a result that breaks cardinality or uniqueness) is refused with an explanation, so a corrupt record never produces an inconsistent value.
	/// It consumes `self`, so the indexes, which no other value shares, are updated in place (log replay copies no paths)
	pub(crate) fn replay(self, rec: &TxRecord) -> Result<Db, String>
	{
		debug_assert!(!self.history && self.since == 0 && self.upto.is_none(), "the log replays unfiltered values");
		if rec.basis_t != self.basis_t + 1
		{
			return Err(format!("basis {} after basis {}", rec.basis_t, self.basis_t));
		}
		if rec.tx != self.next_eid || rec.next_eid <= rec.tx
		{
			return Err(format!("transaction entity {} and next entity {} after next entity {}", rec.tx, rec.next_eid, self.next_eid));
		}
		if rec.instant < self.instant
		{
			return Err(format!("instant {} before {}", rec.instant, self.instant));
		}
		let mut db = self;
		// The schema history records changes by basis
		db.basis_t = rec.basis_t;
		if rec.attrs.windows(2).any(|w| w[0].0 >= w[1].0)
		{
			return Err("the attributes are not in strictly increasing id order".into());
		}
		for (a, def) in &rec.attrs
		{
			// An existing id under its own name is an alteration, the next id under a new name a definition
			let altered = db.try_attr(*a).is_some_and(|old| old.name == def.name && old != def);
			if altered
			{
				db.alter(def).map_err(|e| e.to_string())?;
				continue;
			}
			if *a as usize != db.attrs.len() || db.attr_id(&def.name).is_some()
			{
				return Err(format!("attribute {} defined as id {a}, when the next id is {}", def.name, db.attrs.len()));
			}
			db.define(def).map_err(|e| e.to_string())?;
		}
		let eid = |e: Eid| (1..rec.next_eid).contains(&e);
		for (list, added) in [(&rec.retracted, false), (&rec.asserted, true)]
		{
			if list.windows(2).any(|w| w[0] >= w[1])
			{
				return Err("the datoms are not in strictly increasing EAV order".into());
			}
			for (e, a, v) in list
			{
				let def = db.try_attr(*a).ok_or_else(|| format!("unknown attribute id {a}"))?;
				let ref_ok = match v
				{
					Value::Ref(r) => eid(*r),
					_ => true,
				};
				if def.name.starts_with("db/") || def.value_type != v.value_type() || !eid(*e) || !ref_ok
				{
					return Err(format!("invalid datom [{e} {} {v:?}]", def.name));
				}
				if db.has(*e, *a, v) != !added
				{
					let what = if added { "assertion of a present" } else { "retraction of an absent" };
					return Err(format!("{what} datom [{e} {} {v:?}]", def.name));
				}
			}
		}
		db.basis_t = rec.basis_t;
		db.next_eid = rec.next_eid;
		db.instant = rec.instant;
		db.txs.insert_mut(rec.basis_t, rec.tx);
		db.apply(rec.tx, rec.instant, &rec.retracted, &rec.asserted);
		for (e, a, v) in &rec.asserted
		{
			let def = db.attr(*a);
			if (def.cardinality == Cardinality::One && db.values(*e, *a).nth(1).is_some()) || (def.unique.is_some() && db.entities(*a, v).nth(1).is_some())
			{
				return Err(format!("[{e} {} {v:?}] breaks the attribute's cardinality or uniqueness", def.name));
			}
		}
		Ok(db)
	}

	/// What a checkpoint stores of this (unfiltered) value: what can't be derived (the counters, the schema history, the transactions
	/// and the history in log order); `from_snapshot` derives everything else
	pub(crate) fn snapshot(&self) -> Snapshot
	{
		debug_assert!(!self.history && self.since == 0 && self.upto.is_none(), "a checkpoint is of an unfiltered value");
		Snapshot {
			basis_t: self.basis_t,
			next_eid: self.next_eid,
			instant: self.instant,
			schema: self.schema.iter().map(|(k, def)| (*k, def.clone())).collect(),
			txs: self.txs.iter().map(|(t, tx)| (*t, *tx)).collect(),
			history: self.tlog.iter().map(|(k, _)| k.clone()).collect(),
		}
	}

	/// The value of a snapshot, built in O(n log n) (sorting the history into each index's order, then building each map in linear time),
	/// rather than by replaying the transactions. The current indexes and the statistics are derived from the history: a datom is current
	/// when its last change is an assertion. A snapshot that isn't well-formed (out of order, or naming an unknown attribute) is refused
	pub(crate) fn from_snapshot(s: Snapshot) -> Result<Db, String>
	{
		let increasing = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("the {what} are not in strictly increasing order")) };
		increasing(s.schema.windows(2).all(|w| w[0].0 < w[1].0), "attribute definitions")?;
		increasing(s.txs.windows(2).all(|w| w[0] < w[1]), "transactions")?;
		increasing(s.history.windows(2).all(|w| w[0] < w[1]), "history entries")?;
		if s.txs.len() as T != s.basis_t || s.txs.iter().enumerate().any(|(i, (t, _))| *t != i as T + 1)
		{
			return Err(format!("{} transactions for basis {}", s.txs.len(), s.basis_t));
		}
		// The latest definition of each attribute (the schema is in (id, basis) order)
		let mut latest: Vec<(AttrId, Attr)> = Vec::new();
		for ((a, _), def) in &s.schema
		{
			match latest.last_mut()
			{
				Some((b, d)) if b == a => *d = def.clone(),
				_ => latest.push((*a, def.clone())),
			}
		}
		if latest.iter().enumerate().any(|(i, (a, _))| *a != i as AttrId)
		{
			return Err("the attribute ids are not consecutive from 0".into());
		}
		if let Some(k) = s.history.iter().find(|k| k.2 as usize >= latest.len())
		{
			return Err(format!("a history entry of unknown attribute {}", k.2));
		}
		let mut names: Vec<(Arc<str>, AttrId)> = latest.iter().map(|(a, def)| (def.name.clone(), *a)).collect();
		names.sort();
		increasing(names.windows(2).all(|w| w[0].0 < w[1].0), "attribute names")?;

		// The history in each index's order
		fn sorted<K: Ord>(mut v: Vec<K>) -> Vec<K>
		{
			v.sort_unstable();
			v
		}
		let heavt = sorted(s.history.iter().map(|(tx, e, a, v, added)| (*e, *a, v.clone(), *tx, *added)).collect());
		let haevt = sorted(s.history.iter().map(|(tx, e, a, v, added)| (*a, *e, v.clone(), *tx, *added)).collect());
		let havet = sorted(s.history.iter().map(|(tx, e, a, v, added)| (*a, v.clone(), *e, *tx, *added)).collect());
		let hvaet = sorted(s.history.iter().filter_map(|(tx, e, a, v, added)| if let Value::Ref(r) = v { Some((*r, *a, *e, *tx, *added)) } else { None }).collect());
		// The current datoms (EAVT order), each with the transaction of its last change, an assertion
		let current: Vec<(Eid, AttrId, Value, Eid)> =
			Self::current_as_of(heavt.iter(), |x, y| (x.0, x.1, &x.2) == (y.0, y.1, &y.2), |k| (k.3, k.4), Eid::MAX).into_iter().map(|(k, tx)| (k.0, k.1, k.2.clone(), tx)).collect();
		let aevt = sorted(current.iter().map(|(e, a, v, tx)| ((*a, *e, v.clone()), *tx)).collect());
		let avet = sorted(current.iter().map(|(e, a, v, tx)| ((*a, v.clone(), *e), *tx)).collect());
		let vaet = sorted(current.iter().filter_map(|(e, a, v, tx)| if let Value::Ref(r) = v { Some(((*r, *a, *e), *tx)) } else { None }).collect());

		// The statistics: the number of entries, and of distinct (attribute, entity) and (attribute, value) pairs, of each attribute
		// that ever had a datom (as transact keeps one for every attribute it ever inserted, zero counts included)
		let ever: BTreeSet<AttrId> = s.history.iter().map(|k| k.2).collect();
		let count = |ae: &mut dyn Iterator<Item = (AttrId, bool)>, av: &mut dyn Iterator<Item = (AttrId, bool)>, datoms: &mut dyn Iterator<Item = AttrId>| {
			let mut stats: BTreeMap<AttrId, AttrStats> = ever.iter().map(|a| (*a, AttrStats::default())).collect();
			for a in datoms
			{
				stats.get_mut(&a).expect("an attribute with a history").datoms += 1;
			}
			for (a, new) in ae
			{
				stats.get_mut(&a).expect("an attribute with a history").entities += new as u64;
			}
			for (a, new) in av
			{
				stats.get_mut(&a).expect("an attribute with a history").values += new as u64;
			}
			PMap::from_sorted(stats)
		};
		// Whether each entry of a sorted list starts a new (attribute, x) pair
		fn firsts<'a, K: 'a, X: PartialEq>(list: &'a [K], key: impl Fn(&'a K) -> (AttrId, X) + 'a) -> impl Iterator<Item = (AttrId, bool)> + 'a
		{
			list.iter().enumerate().map(move |(i, k)| (key(k).0, i == 0 || key(&list[i - 1]) != key(k)))
		}
		let stats = count(&mut firsts(&aevt, |k| (k.0 .0, k.0 .1)), &mut firsts(&avet, |k| (k.0 .0, &k.0 .1)), &mut aevt.iter().map(|k| k.0 .0));
		let hstats = count(&mut firsts(&haevt, |k| (k.0, k.1)), &mut firsts(&havet, |k| (k.0, &k.1)), &mut haevt.iter().map(|k| k.0));

		Ok(Db {
			basis_t: s.basis_t,
			next_eid: s.next_eid,
			instant: s.instant,
			attrs: PMap::from_sorted(latest),
			attr_ids: PMap::from_sorted(names),
			stats,
			eavt: PMap::from_sorted(current.into_iter().map(|(e, a, v, tx)| ((e, a, v), tx))),
			aevt: PMap::from_sorted(aevt),
			avet: PMap::from_sorted(avet),
			vaet: PMap::from_sorted(vaet),
			heavt: PMap::from_sorted(heavt.into_iter().map(|k| (k, ()))),
			haevt: PMap::from_sorted(haevt.into_iter().map(|k| (k, ()))),
			havet: PMap::from_sorted(havet.into_iter().map(|k| (k, ()))),
			hvaet: PMap::from_sorted(hvaet.into_iter().map(|k| (k, ()))),
			tlog: PMap::from_sorted(s.history.into_iter().map(|k| (k, ()))),
			schema: PMap::from_sorted(s.schema),
			hstats,
			txs: PMap::from_sorted(s.txs),
			history: false,
			since: 0,
			upto: None,
		})
	}

	// The statistics count distinct entities and values: whether another key of an index shares a changed key's (attribute, entity) or
	// (attribute, value) prefix is what the removal or the batch insertion itself shows from the keys around it (`PMap::remove_mut_near`,
	// `PMap::insert_batch_mut`), and only when those are in another leaf does it take a search of its own

	// Retract a present datom from the current indexes (insertions and removals only see unfiltered values, as transact refuses views);
	// returns whether it was present
	fn remove(&mut self, e: Eid, a: AttrId, v: &Value) -> bool
	{
		if self.eavt.remove_mut(&(e, a, v.clone())).is_none()
		{
			return false;
		}
		let other_e = self.aevt.remove_mut_near(&(a, e, v.clone()), |k| (k.0, k.1) == (a, e)).expect("the indexes agree").1.unwrap_or_else(|| self.values(e, a).next().is_some());
		let other_v = self.avet.remove_mut_near(&(a, v.clone(), e), |k| k.0 == a && k.1 == *v).expect("the indexes agree").1.unwrap_or_else(|| self.entities(a, v).next().is_some());
		if let Value::Ref(r) = v
		{
			self.vaet.remove_mut(&(*r, a, e));
		}
		let mut s = self.stats(a);
		s.datoms -= 1;
		s.entities -= !other_e as u64;
		s.values -= !other_v as u64;
		self.stats.insert_mut(a, s);
		true
	}

	// Assert absent datoms in the current indexes, as one batch per index (each touched node visited once)
	fn insert_current(&mut self, datoms: &[(Eid, AttrId, Value)], tx: Eid)
	{
		let mut deltas: BTreeMap<AttrId, AttrStats> = BTreeMap::new();
		for (_, a, _) in datoms
		{
			deltas.entry(*a).or_default().datoms += 1;
		}
		self.eavt.insert_batch_mut(sorted_batch(datoms.iter().map(|(e, a, v)| ((*e, *a, v.clone()), tx))), None);
		let aevt = sorted_batch(datoms.iter().map(|(e, a, v)| ((*a, *e, v.clone()), tx)));
		let keys: Vec<(AttrId, Eid)> = aevt.iter().map(|(k, _)| (k.0, k.1)).collect();
		let answers = self.aevt.insert_batch_mut(aevt, Some(&|x, y| (x.0, x.1) == (y.0, y.1)));
		for (a, _) in fresh_groups(&keys, &answers, |(a, e)| self.values(*e, *a).count())
		{
			deltas.entry(*a).or_default().entities += 1;
		}
		let avet = sorted_batch(datoms.iter().map(|(e, a, v)| ((*a, v.clone(), *e), tx)));
		let keys: Vec<(AttrId, Value)> = avet.iter().map(|(k, _)| (k.0, k.1.clone())).collect();
		let answers = self.avet.insert_batch_mut(avet, Some(&|x, y| x.0 == y.0 && x.1 == y.1));
		for (a, _) in fresh_groups(&keys, &answers, |(a, v)| self.entities(*a, v).count())
		{
			deltas.entry(*a).or_default().values += 1;
		}
		let refs = sorted_batch(datoms.iter().filter_map(|(e, a, v)| if let Value::Ref(r) = v { Some(((*r, *a, *e), tx)) } else { None }));
		self.vaet.insert_batch_mut(refs, None);
		for (a, d) in deltas
		{
			let mut s = self.stats(a);
			(s.datoms, s.entities, s.values) = (s.datoms + d.datoms, s.entities + d.entities, s.values + d.values);
			self.stats.insert_mut(a, s);
		}
	}

	// Append transaction tx's changes to the history indexes, as one batch per index (a transaction changes a datom once at most,
	// so the keys are new; the log's keys all start with tx, so they land together)
	fn log_batch(&mut self, tx: Eid, retracted: &[(Eid, AttrId, Value)], asserted: &[(Eid, AttrId, Value)])
	{
		let changes: Vec<(Eid, AttrId, &Value, bool)> = retracted.iter().map(|(e, a, v)| (*e, *a, v, false)).chain(asserted.iter().map(|(e, a, v)| (*e, *a, v, true))).collect();
		let mut deltas: BTreeMap<AttrId, AttrStats> = BTreeMap::new();
		for (_, a, _, _) in &changes
		{
			deltas.entry(*a).or_default().datoms += 1;
		}
		self.heavt.insert_batch_mut(sorted_batch(changes.iter().map(|(e, a, v, added)| ((*e, *a, (*v).clone(), tx, *added), ()))), None);
		let haevt = sorted_batch(changes.iter().map(|(e, a, v, added)| ((*a, *e, (*v).clone(), tx, *added), ())));
		let keys: Vec<(AttrId, Eid)> = haevt.iter().map(|(k, _)| (k.0, k.1)).collect();
		let answers = self.haevt.insert_batch_mut(haevt, Some(&|x, y| (x.0, x.1) == (y.0, y.1)));
		for (a, _) in fresh_groups(&keys, &answers, |(a, e)| self.haevt.seek(|k| (k.0, k.1) < (*a, *e)).take_while(|(k, _)| (k.0, k.1) == (*a, *e)).count())
		{
			deltas.entry(*a).or_default().entities += 1;
		}
		let havet = sorted_batch(changes.iter().map(|(e, a, v, added)| ((*a, (*v).clone(), *e, tx, *added), ())));
		let keys: Vec<(AttrId, Value)> = havet.iter().map(|(k, _)| (k.0, k.1.clone())).collect();
		let answers = self.havet.insert_batch_mut(havet, Some(&|x, y| x.0 == y.0 && x.1 == y.1));
		for (a, _) in fresh_groups(&keys, &answers, |(a, v)| self.havet.seek(|k| (k.0, &k.1) < (*a, v)).take_while(|(k, _)| k.0 == *a && k.1 == *v).count())
		{
			deltas.entry(*a).or_default().values += 1;
		}
		let refs = sorted_batch(changes.iter().filter_map(|(e, a, v, added)| if let Value::Ref(r) = v { Some(((*r, *a, *e, tx, *added), ())) } else { None }));
		self.hvaet.insert_batch_mut(refs, None);
		self.tlog.insert_batch_mut(sorted_batch(changes.iter().map(|(e, a, v, added)| ((tx, *e, *a, (*v).clone(), *added), ()))), None);
		for (a, d) in deltas
		{
			let mut s = self.hstats.get(&a).copied().unwrap_or_default();
			(s.datoms, s.entities, s.values) = (s.datoms + d.datoms, s.entities + d.entities, s.values + d.values);
			self.hstats.insert_mut(a, s);
		}
	}
}

// A batch for `PMap::insert_batch_mut`: the entries in key order
fn sorted_batch<K: Ord, V>(entries: impl Iterator<Item = (K, V)>) -> Vec<(K, V)>
{
	let mut v: Vec<(K, V)> = entries.collect();
	v.sort_unstable_by(|x, y| x.0.cmp(&y.0));
	v
}

// The prefixes that a batch made new: for each group of equal prefixes in the batch (`keys`, in order), the prefix if no key that was in
// the index before has it, from the batch's answers or, where its leaves couldn't tell, by counting the keys with the prefix now (`count`)
fn fresh_groups<'k, P: PartialEq>(keys: &'k [P], answers: &[Option<bool>], count: impl Fn(&P) -> usize) -> Vec<&'k P>
{
	let mut out = Vec::new();
	let mut s = 0;
	while s < keys.len()
	{
		let e = s + keys[s..].iter().take_while(|k| **k == keys[s]).count();
		let group = &answers[s..e];
		let old = if group.contains(&Some(true)) { true } else if group.contains(&None) { count(&keys[s]) > e - s } else { false };
		if !old
		{
			out.push(&keys[s]);
		}
		s = e;
	}
	out
}

/// A connection: the current database value, in memory (`Conn::new`) or backed by a durable log (`Conn::open`).
/// The past values are `as_of` views of the current one, so the connection keeps only that
pub struct Conn
{
	db: Db,
	log: Option<Log>,
}

impl Default for Conn
{
	fn default() -> Self
	{
		Conn { db: Db::new(), log: None }
	}
}

impl Conn
{
	/// A connection to a new, empty database in memory
	pub fn new() -> Self
	{
		Self::default()
	}

	/// A connection to the database of the durable log at `path`, created if it doesn't exist: every recorded transaction is replayed
	/// (so `as_of` sees them all), and every transaction committed from now on is appended. A torn final record (an append that a crash
	/// interrupted, so one never acknowledged) is truncated; any other damage is `Error::CorruptLog`, and the file is left as it is.
	/// One connection at a time can hold a log (opening it again meanwhile is an `Error::Io`). See `log` for the format
	pub fn open(path: impl AsRef<Path>) -> Result<Conn, Error>
	{
		let (log, db) = Log::open(path.as_ref())?;
		Ok(Conn { db, log: Some(log) })
	}

	#[cfg(test)]
	pub(crate) fn log(&self) -> &Log
	{
		self.log.as_ref().expect("a durable connection")
	}

	#[cfg(test)]
	pub(crate) fn log_mut(&mut self) -> &mut Log
	{
		self.log.as_mut().expect("a durable connection")
	}

	pub fn db(&self) -> Db
	{
		self.db.clone()
	}

	/// Apply a transaction; on a durable connection, it returns once the transaction's record is appended to the log and synced.
	///
	/// The history already keeps every past value (each change is stored with its transaction: the *fat-node* method of partial
	/// persistence), so the connection doesn't also keep the value before the transaction by path copying: it plans the transaction
	/// (everything that can fail), lets go of its value, and applies the changes to the indexes in place, unless something else
	/// (a value a caller holds, a report) still shares them. The report's `db_before` is an `as_of` view of the new value.
	pub fn transact(&mut self, tx: &Tx) -> Result<TxReport, Error>
	{
		let planned = self.db.plan(tx)?;
		// Durable before it is visible: a transaction whose append fails isn't committed
		if let Some(log) = &mut self.log
		{
			log.append(&planned.record(&self.db))?;
		}
		let t = self.db.basis_t;
		self.db = Db::default();
		let mut report = planned.finish(Db::default());
		report.db_before = report.db_after.as_of(t).expect("an earlier basis");
		self.db = report.db_after.clone();
		Ok(report)
	}

	/// Write a checkpoint of the current value next to the log (its file name with `.ckpt` appended), so that the next `open` rebuilds this value
	/// from the checkpoint in O(n log n) and replays only the records after it. The checkpoint is a cache: the log still holds every record,
	/// and a checkpoint that is damaged, torn, of another log or ahead of the log is ignored on open (the log is replayed in full).
	/// An `Error::Io` for an in-memory connection
	pub fn checkpoint(&mut self) -> Result<(), Error>
	{
		match &mut self.log
		{
			Some(log) => log.checkpoint(&self.db),
			None => Err(Error::Io("an in-memory connection has no log to checkpoint".into())),
		}
	}

	/// The database as of basis `t` (None if `t` is in the future): a view of the current value (see `Db::as_of`)
	pub fn as_of(&self, t: T) -> Option<Db>
	{
		self.db.as_of(t)
	}
}

#[cfg(test)]
pub(crate) mod tests
{
	use super::*;
	use crate::testutil::Rng;
	use std::collections::BTreeMap;

	type Triple = (Eid, AttrId, Value);

	fn people() -> (Conn, TxReport)
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("person/name", ValueType::Str, Cardinality::One)
			.define("person/age", ValueType::Int, Cardinality::One)
			.define("person/friend", ValueType::Ref, Cardinality::Many)
			.define_attr(Attr::new("person/email", ValueType::Str, Cardinality::One).unique(Unique::Identity))
			.define_attr(Attr::new("person/ssn", ValueType::Str, Cardinality::One).unique(Unique::Identity))
			.define_attr(Attr::new("person/badge", ValueType::Int, Cardinality::One).unique(Unique::Value));
		let (a, b) = (tx.temp(), tx.temp());
		tx.add(a, "person/name", "Alice").add(a, "person/age", 30).add(b, "person/name", "Bob").add(a, "person/friend", b);
		tx.add(a, "person/email", "alice@x").add(b, "person/email", "bob@x").add(a, "person/badge", 1);
		let r = conn.transact(&tx).unwrap();
		(conn, r)
	}

	// The user datoms (without the transactions' instants)
	fn datoms(db: &Db) -> BTreeSet<Triple>
	{
		db.eavt.iter().map(|(k, _)| k.clone()).filter(|k| !db.attr(k.1).name.starts_with("db/")).collect()
	}

	#[test]
	fn transact_indexes_and_stats()
	{
		let (conn, r) = people();
		let db = conn.db();
		let (alice, bob) = (r.tempids[0], r.tempids[1]);
		let name = db.attr_id("person/name").unwrap();
		let friend = db.attr_id("person/friend").unwrap();
		assert_eq!(db.values(alice, name).collect::<Vec<_>>(), vec![&Value::from("Alice")]);
		assert_eq!(db.entities(friend, &Value::Ref(bob)).collect::<Vec<_>>(), vec![alice]);
		assert_eq!(db.referrers(bob).collect::<Vec<_>>(), vec![(friend, alice)]);
		assert_eq!(db.attr_datoms(name).count(), 2);
		assert_eq!(db.stats(name), AttrStats { datoms: 2, entities: 2, values: 2 });
		// The instant first, then the 7 assertions, all of them in the transaction's entity
		assert_eq!(r.tx_data.len(), 8);
		assert_eq!(db.attr(r.tx_data[0].a).name.as_ref(), TX_INSTANT);
		assert!(r.tx_data.iter().all(|d| d.tx == r.tx && d.added));
		assert!(r.tx != alice && r.tx != bob);
	}

	#[test]
	fn cardinality_one_replaces_and_old_versions_survive()
	{
		let (mut conn, r) = people();
		let alice = Entity::Id(r.tempids[0]);
		let before = conn.db();
		let mut tx = Tx::new();
		tx.add(alice, "person/age", 31);
		let r2 = conn.transact(&tx).unwrap();
		let age = before.attr_id("person/age").unwrap();
		let changes: Vec<_> = r2.tx_data.iter().filter(|d| d.a == age).map(|d| (d.v.clone(), d.added, d.tx)).collect();
		assert_eq!(changes, vec![(Value::Int(30), false, r2.tx), (Value::Int(31), true, r2.tx)]);
		assert_eq!(conn.db().values(r.tempids[0], age).collect::<Vec<_>>(), vec![&Value::Int(31)]);
		// The old value is untouched, both directly and through the history
		assert_eq!(before.values(r.tempids[0], age).collect::<Vec<_>>(), vec![&Value::Int(30)]);
		assert_eq!(conn.as_of(1).unwrap().values(r.tempids[0], age).collect::<Vec<_>>(), vec![&Value::Int(30)]);
		assert_eq!(conn.db().basis_t(), 2);
		assert!(conn.as_of(3).is_none());
	}

	#[test]
	fn retraction_updates_stats()
	{
		let (mut conn, r) = people();
		let mut tx = Tx::new();
		tx.retract(Entity::Id(r.tempids[0]), "person/name", "Alice").retract(Entity::Id(r.tempids[0]), "person/name", "Nobody");
		let r2 = conn.transact(&tx).unwrap();
		assert_eq!(r2.tx_data.iter().filter(|d| !d.added).count(), 1);
		let name = conn.db().attr_id("person/name").unwrap();
		assert_eq!(conn.db().stats(name), AttrStats { datoms: 1, entities: 1, values: 1 });
	}

	// A report resolves the entities of its own transaction only, even where another transaction's tempid has an index in range
	#[test]
	#[should_panic(expected = "belongs to another transaction")]
	fn a_report_refuses_another_transactions_tempid()
	{
		let (mut conn, _) = people();
		let mut other = Tx::new();
		let theirs = other.temp();
		let mut tx = Tx::new();
		let mine = tx.temp();
		tx.add(mine, "person/name", "M");
		let r = conn.transact(&tx).unwrap();
		assert!(r.eid(mine) > r.tx);
		r.eid(theirs);
	}

	#[test]
	fn validation()
	{
		let (mut conn, r) = people();
		let mut tx = Tx::new();
		tx.add(Entity::Id(99), "person/name", "X");
		assert_eq!(conn.transact(&tx).err(), Some(Error::UnknownEntity(99)));
		let mut tx = Tx::new();
		let e = tx.temp();
		tx.add(e, "person/age", "old");
		assert!(matches!(conn.transact(&tx), Err(Error::TypeMismatch { .. })));
		let mut tx = Tx::new();
		let e = tx.temp();
		tx.add(e, "person/age", e);
		assert!(matches!(conn.transact(&tx), Err(Error::TypeMismatch { got: ValueType::Ref, .. })));
		let mut tx = Tx::new();
		tx.define("person/age", ValueType::Str, Cardinality::One);
		assert!(matches!(conn.transact(&tx), Err(Error::AttrConflict(_))));
		// Making an existing attribute unique is a redefinition
		let mut tx = Tx::new();
		tx.define_attr(Attr::new("person/name", ValueType::Str, Cardinality::One).unique(Unique::Value));
		assert!(matches!(conn.transact(&tx), Err(Error::AttrConflict(_))));
		let mut tx = Tx::new();
		let e = tx.temp();
		tx.add(e, "nope", 1);
		assert!(matches!(conn.transact(&tx), Err(Error::UnknownAttr(_))));
		// Entities of another transaction
		let mut other = Tx::new();
		let (t, l) = (other.temp(), other.lookup("person/email", "alice@x"));
		for e in [t, l]
		{
			let mut tx = Tx::new();
			tx.add(e, "person/age", 1);
			assert_eq!(conn.transact(&tx).err(), Some(Error::ForeignEntity(e)));
			// Also when this transaction has a tempid and a lookup of the same index, as an entity and as a value
			let mut tx = Tx::new();
			let (mine, _) = (tx.temp(), tx.lookup("person/email", "bob@x"));
			tx.add(mine, "person/name", "M").add(e, "person/age", 1);
			assert_eq!(conn.transact(&tx).err(), Some(Error::ForeignEntity(e)));
			let mut tx = Tx::new();
			let (mine, _) = (tx.temp(), tx.lookup("person/email", "bob@x"));
			tx.add(mine, "person/friend", e);
			assert_eq!(conn.transact(&tx).err(), Some(Error::ForeignEntity(e)));
		}
		// Schema rules
		let mut tx = Tx::new();
		tx.define("db/mine", ValueType::Int, Cardinality::One);
		assert!(matches!(conn.transact(&tx), Err(Error::InvalidSchema(_))));
		let mut tx = Tx::new();
		tx.define_attr(Attr::new("person/parts", ValueType::Int, Cardinality::Many).component());
		assert!(matches!(conn.transact(&tx), Err(Error::InvalidSchema(_))));
		// The instants are the library's, directly and through retract_entity
		let mut tx = Tx::new();
		tx.add(Entity::Tx, TX_INSTANT, 5);
		assert_eq!(conn.transact(&tx).err(), Some(Error::Reserved(TX_INSTANT.into())));
		let mut tx = Tx::new();
		tx.retract_entity(Entity::Id(r.tx));
		assert_eq!(conn.transact(&tx).err(), Some(Error::Reserved(TX_INSTANT.into())));
		// Failed transactions leave no trace
		assert_eq!(conn.db().basis_t(), 1);
	}

	#[test]
	fn reified_transactions()
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("tx/author", ValueType::Str, Cardinality::One).define("doc/title", ValueType::Str, Cardinality::One).with_instant(100);
		let d = tx.temp();
		tx.add(d, "doc/title", "a").add(Entity::Tx, "tx/author", "ann");
		let r1 = conn.transact(&tx).unwrap();
		let db = conn.db();
		let (instant, author) = (db.attr_id(TX_INSTANT).unwrap(), db.attr_id("tx/author").unwrap());
		assert_eq!(db.values(r1.tx, instant).collect::<Vec<_>>(), vec![&Value::Instant(100)]);
		assert_eq!(db.values(r1.tx, author).collect::<Vec<_>>(), vec![&Value::from("ann")]);
		assert_eq!(r1.eid(Entity::Tx), r1.tx);
		// The default instant is the previous one; an earlier one is refused
		let mut tx = Tx::new();
		tx.add(Entity::Id(r1.tempids[0]), "doc/title", "b");
		let r2 = conn.transact(&tx).unwrap();
		assert_eq!(conn.db().values(r2.tx, instant).collect::<Vec<_>>(), vec![&Value::Instant(100)]);
		tx.with_instant(99);
		assert_eq!(conn.transact(&tx).err(), Some(Error::InstantBackwards { previous: 100, got: 99 }));
		tx.with_instant(250);
		let r3 = conn.transact(&tx).unwrap();
		// Every transaction is an entity of its own, which its datoms refer to
		assert!(r1.tx < r2.tx && r2.tx < r3.tx);
		assert!(r3.tx_data.iter().all(|d| d.tx == r3.tx));
		let mut q = conn.db().query();
		let (t, i) = (q.var("?tx"), q.var("?i"));
		q.pattern(t.clone(), TX_INSTANT, i.clone()).unwrap();
		q.pred(i.clone(), crate::Cmp::Ge, 100).unwrap();
		let got = q.run(&[t, i]).unwrap();
		assert_eq!(got, vec![vec![Value::Ref(r1.tx), Value::Instant(100)], vec![Value::Ref(r2.tx), Value::Instant(100)], vec![Value::Ref(r3.tx), Value::Instant(250)]]);
	}

	#[test]
	fn identity_upserts_and_unifies_tempids()
	{
		let (mut conn, r) = people();
		let (alice, bob) = (r.tempids[0], r.tempids[1]);
		let db = conn.db();
		let (age, email, ssn) = (db.attr_id("person/age").unwrap(), db.attr_id("person/email").unwrap(), db.attr_id("person/ssn").unwrap());
		// A tempid with an existing email is Alice; two tempids with the same new email are one new entity
		let mut tx = Tx::new();
		let (a, c1, c2) = (tx.temp(), tx.temp(), tx.temp());
		tx.add(a, "person/age", 31).add(a, "person/email", "alice@x");
		tx.add(c1, "person/email", "carol@x").add(c2, "person/age", 22).add(c2, "person/email", "carol@x");
		let r2 = conn.transact(&tx).unwrap();
		assert_eq!(r2.tempids[0], alice);
		assert_eq!(r2.tempids[1], r2.tempids[2]);
		assert!(r2.tempids[1] > r2.tx);
		let db = conn.db();
		assert_eq!(db.values(alice, age).collect::<Vec<_>>(), vec![&Value::Int(31)]);
		assert_eq!(db.values(r2.tempids[1], age).collect::<Vec<_>>(), vec![&Value::Int(22)]);
		assert_eq!(db.stats(email).entities, 3);

		// Upserting through a ref-valued identity: x upserts to Alice, which makes y's value Alice, which upserts y to Alice's account.
		// y comes first, so this takes a second round
		let mut tx = Tx::new();
		tx.define_attr(Attr::new("account/holder", ValueType::Ref, Cardinality::One).unique(Unique::Identity)).define("account/balance", ValueType::Int, Cardinality::One);
		let acct = tx.temp();
		tx.add(acct, "account/holder", Entity::Id(alice)).add(acct, "account/balance", 10);
		let r3 = conn.transact(&tx).unwrap();
		let mut tx = Tx::new();
		let (y, x) = (tx.temp(), tx.temp());
		tx.add(y, "account/holder", x).add(y, "account/balance", 20).add(x, "person/email", "alice@x");
		let r4 = conn.transact(&tx).unwrap();
		assert_eq!(r4.tempids, vec![r3.tempids[0], alice]);
		let balance = conn.db().attr_id("account/balance").unwrap();
		assert_eq!(conn.db().attr_datoms(balance).collect::<Vec<_>>(), vec![(r3.tempids[0], &Value::Int(20))]);

		// One tempid claiming Alice's email and Bob's ssn is a conflict, directly and through unification
		let mut tx = Tx::new();
		tx.add(Entity::Id(bob), "person/ssn", "123");
		conn.transact(&tx).unwrap();
		let mut tx = Tx::new();
		let z = tx.temp();
		tx.add(z, "person/email", "alice@x").add(z, "person/ssn", "123");
		assert!(matches!(conn.transact(&tx), Err(Error::UpsertConflict(..))));
		let mut tx = Tx::new();
		let (z1, z2) = (tx.temp(), tx.temp());
		// A shared new ssn unifies z1 and z2, then z1 upserts to Alice and z2 to Bob
		tx.add(z1, "person/ssn", "777").add(z2, "person/ssn", "777").add(z1, "person/email", "alice@x").add(z2, "person/email", "bob@x");
		assert!(matches!(conn.transact(&tx), Err(Error::UpsertConflict(..))));
		// An explicit entity keeps its identity: asserting Bob's email on Alice is a unique conflict, not an upsert
		let mut tx = Tx::new();
		tx.add(Entity::Id(alice), "person/email", "bob@x");
		assert!(matches!(conn.transact(&tx), Err(Error::UniqueConflict { .. })));
		// A tempid asserting the same new identity value as an explicit entity is that entity
		let mut tx = Tx::new();
		let w = tx.temp();
		tx.add(Entity::Id(bob), "person/ssn", "456").add(w, "person/ssn", "456").add(w, "person/age", 40);
		let r5 = conn.transact(&tx).unwrap();
		assert_eq!(r5.tempids, vec![bob]);
		assert!(conn.db().values(bob, ssn).eq([&Value::from("456")]));
		assert!(conn.db().values(bob, age).eq([&Value::Int(40)]));
	}

	// Identity values that are refs to new entities: tempids referring to the same new entity (or to tempids that unify) unify too
	#[test]
	fn identity_refs_to_tempids_unify()
	{
		let schema = |tx: &mut Tx| {
			tx.define("org/name", ValueType::Str, Cardinality::One)
				.define_attr(Attr::new("org/id", ValueType::Str, Cardinality::One).unique(Unique::Identity))
				.define_attr(Attr::new("person/org", ValueType::Ref, Cardinality::One).unique(Unique::Identity))
				.define("person/age", ValueType::Int, Cardinality::One);
		};
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		schema(&mut tx);
		conn.transact(&tx).unwrap();
		let org = conn.db().attr_id("person/org").unwrap();
		// p1 and p2 both name the new org o
		let mut tx = Tx::new();
		let (o, p1, p2) = (tx.temp(), tx.temp(), tx.temp());
		tx.add(o, "org/name", "Acme").add(p1, "person/org", o).add(p2, "person/org", o).add(p2, "person/age", 3);
		let r = conn.transact(&tx).unwrap();
		assert_eq!(r.tempids[1], r.tempids[2]);
		assert_ne!(r.tempids[0], r.tempids[1]);
		assert_eq!(conn.db().attr_datoms(org).collect::<Vec<_>>(), vec![(r.tempids[1], &Value::Ref(r.tempids[0]))]);
		// o1 and o2 unify through their own identity first, then p1 and p2 through them (p2's ops come first: a second round)
		let mut tx = Tx::new();
		let (p2, p1, o1, o2) = (tx.temp(), tx.temp(), tx.temp(), tx.temp());
		tx.add(p2, "person/org", o2).add(p1, "person/org", o1).add(o1, "org/id", "globex").add(o2, "org/id", "globex");
		let r = conn.transact(&tx).unwrap();
		assert_eq!((r.tempids[0], r.tempids[2]), (r.tempids[1], r.tempids[3]));
		assert_ne!(r.tempids[0], r.tempids[2]);
		// Refs to different new entities don't unify
		let mut tx = Tx::new();
		let (o1, o2, p1, p2) = (tx.temp(), tx.temp(), tx.temp(), tx.temp());
		tx.add(o1, "org/name", "A").add(o2, "org/name", "B").add(p1, "person/org", o1).add(p2, "person/org", o2);
		let r = conn.transact(&tx).unwrap();
		assert_eq!(r.tempids.iter().collect::<BTreeSet<_>>().len(), 4);
		// p1 and p2 are one entity through o, but p1 upserts to ann and p2 to ben (existing entities): a conflict
		let mut tx = Tx::new();
		let (ann, ben) = (tx.temp(), tx.temp());
		tx.add(ann, "org/id", "ann").add(ben, "org/id", "ben");
		conn.transact(&tx).unwrap();
		let mut tx = Tx::new();
		let (o, p1, p2) = (tx.temp(), tx.temp(), tx.temp());
		tx.add(o, "org/name", "Initech").add(p1, "person/org", o).add(p2, "person/org", o);
		tx.add(p1, "org/id", "ann").add(p2, "org/id", "ben");
		assert!(matches!(conn.transact(&tx), Err(Error::UpsertConflict(..))));
	}

	// Random upserts and unifications through string and ref-valued identities against a naive fixpoint over every pair of assertions
	#[test]
	fn identity_unification_matches_a_naive_fixpoint()
	{
		let mut rng = Rng::new(21);
		// How many transactions conflicted, and how many unified two tempids only through a ref to a new entity
		let (mut conflicts, mut through_refs) = (0, 0);
		for _ in 0..300
		{
			let mut conn = Conn::new();
			let mut tx = Tx::new();
			tx.define_attr(Attr::new("k/id", ValueType::Str, Cardinality::Many).unique(Unique::Identity))
				.define_attr(Attr::new("k/ref", ValueType::Ref, Cardinality::Many).unique(Unique::Identity));
			// Existing entities: e0 and e1 with ids, e2 referring to e0
			let ex: Vec<Entity> = (0..3).map(|_| tx.temp()).collect();
			tx.add(ex[0], "k/id", "x0").add(ex[1], "k/id", "x1").add(ex[2], "k/ref", ex[0]);
			let eids = conn.transact(&tx).unwrap().tempids;
			// The new tempids' identity assertions: (subject, Ok(id) or Err(ref target: a tempid or an existing entity))
			let nt = 2 + (rng.next() % 5) as usize;
			let mut tx = Tx::new();
			let ts: Vec<Entity> = (0..nt).map(|_| tx.temp()).collect();
			let mut ops: Vec<(usize, Result<String, usize>)> = Vec::new();
			for _ in 0..rng.next() % 8
			{
				let s = (rng.next() % nt as u64) as usize;
				let v = match rng.next() % 4
				{
					0 => Ok(["x0", "x1", "n0", "n1"][(rng.next() % 4) as usize].to_string()),
					1 => Err(nt + (rng.next() % 3) as usize),
					_ => Err((rng.next() % nt as u64) as usize),
				};
				match &v
				{
					Ok(id) => tx.add(ts[s], "k/id", id.as_str()),
					Err(t) if *t >= nt => tx.add(ts[s], "k/ref", Entity::Id(eids[t - nt])),
					Err(t) => tx.add(ts[s], "k/ref", ts[*t]),
				};
				ops.push((s, v));
			}
			// The naive fixpoint: nodes 0..nt are the tempids, nt..nt+3 the existing entities
			let mut class: Vec<usize> = (0..nt + 3).collect();
			let existing_id = |id: &str| ["x0", "x1"].iter().position(|x| *x == id).map(|i| nt + i);
			loop
			{
				let mut merged = false;
				let mut merge = |class: &mut Vec<usize>, a: usize, b: usize| {
					let (ca, cb) = (class[a], class[b]);
					if ca != cb
					{
						class.iter_mut().filter(|c| **c == cb).for_each(|c| *c = ca);
						merged = true;
					}
				};
				for i in 0..ops.len()
				{
					// An existing holder of the value
					let holder = match &ops[i].1
					{
						Ok(id) => existing_id(id),
						Err(t) => (class[nt] == class[*t]).then_some(nt + 2),
					};
					if let Some(h) = holder
					{
						merge(&mut class, h, ops[i].0);
					}
					for j in 0..i
					{
						let same = match (&ops[i].1, &ops[j].1)
						{
							(Ok(a), Ok(b)) => a == b,
							(Err(a), Err(b)) => class[*a] == class[*b],
							_ => false,
						};
						if same
						{
							merge(&mut class, ops[i].0, ops[j].0);
						}
					}
				}
				if !merged
				{
					break;
				}
			}
			let conflict = (nt..nt + 3).any(|x| (x + 1..nt + 3).any(|y| class[x] == class[y]));
			match conn.transact(&tx)
			{
				Err(Error::UpsertConflict(..)) =>
				{
					assert!(conflict, "{ops:?}");
					conflicts += 1;
				}
				Err(e) => panic!("{e:?} for {ops:?}"),
				Ok(r) =>
				{
					assert!(!conflict, "{ops:?}");
					// Two tempids unified, neither of them with a string identity nor upserted
					let plain = |t: usize| !ops.iter().any(|(s, v)| *s == t && v.is_ok()) && r.tempids[t] > r.tx;
					through_refs += (0..nt).any(|a| (0..a).any(|b| plain(a) && plain(b) && r.tempids[a] == r.tempids[b])) as usize;
					for a in 0..nt
					{
						for b in 0..nt
						{
							assert_eq!(r.tempids[a] == r.tempids[b], class[a] == class[b], "{ops:?}");
						}
						let upserted = (nt..nt + 3).find(|&x| class[x] == class[a]).map(|x| eids[x - nt]);
						match upserted
						{
							Some(e) => assert_eq!(r.tempids[a], e, "{ops:?}"),
							None => assert!(r.tempids[a] > r.tx, "{ops:?}"),
						}
					}
				}
			}
		}
		assert!(conflicts >= 10 && through_refs >= 10, "{conflicts} conflicts, {through_refs} unifications through refs");
	}

	#[test]
	fn unique_values_conflict()
	{
		let (mut conn, r) = people();
		let (alice, bob) = (Entity::Id(r.tempids[0]), Entity::Id(r.tempids[1]));
		let badge = conn.db().attr_id("person/badge").unwrap();
		let conflict = Error::UniqueConflict { attr: "person/badge".into(), value: Value::Int(1) };
		// A new entity, an existing one, or two in the same transaction
		let mut tx = Tx::new();
		let t = tx.temp();
		tx.add(t, "person/badge", 1);
		assert_eq!(conn.transact(&tx).err(), Some(conflict.clone()));
		let mut tx = Tx::new();
		tx.add(bob, "person/badge", 1);
		assert_eq!(conn.transact(&tx).err(), Some(conflict.clone()));
		let mut tx = Tx::new();
		let t = tx.temp();
		tx.add(bob, "person/badge", 2).add(t, "person/badge", 2);
		assert!(matches!(conn.transact(&tx), Err(Error::UniqueConflict { .. })));
		// Handing the value over in one transaction is fine: uniqueness holds in the result
		let mut tx = Tx::new();
		tx.add(bob, "person/badge", 1).retract(alice, "person/badge", 1);
		conn.transact(&tx).unwrap();
		// ... and so is taking it back while the previous owner's cardinality-one value is replaced
		let mut tx = Tx::new();
		tx.add(alice, "person/badge", 1).add(bob, "person/badge", 7);
		conn.transact(&tx).unwrap();
		let db = conn.db();
		assert_eq!(db.attr_datoms(badge).collect::<Vec<_>>(), vec![(r.tempids[0], &Value::Int(1)), (r.tempids[1], &Value::Int(7))]);
		// Re-asserting one's own value changes nothing (only the instant is new)
		let mut tx = Tx::new();
		tx.add(alice, "person/badge", 1);
		assert_eq!(conn.transact(&tx).unwrap().tx_data.len(), 1);
	}

	#[test]
	fn lookup_refs()
	{
		let (mut conn, r) = people();
		let (alice, bob) = (r.tempids[0], r.tempids[1]);
		let mut tx = Tx::new();
		let (a, b) = (tx.lookup("person/email", "alice@x"), tx.lookup("person/email", "bob@x"));
		tx.add(a, "person/age", 33).retract(a, "person/friend", b).add(b, "person/friend", a);
		let r2 = conn.transact(&tx).unwrap();
		assert_eq!(r2.lookups, vec![alice, bob]);
		assert_eq!(r2.eid(b), bob);
		let db = conn.db();
		let friend = db.attr_id("person/friend").unwrap();
		assert_eq!(db.referrers(alice).collect::<Vec<_>>(), vec![(friend, bob)]);
		assert_eq!(db.referrers(bob).count(), 0);
		assert_eq!(db.lookup("person/badge", &Value::Int(1)), Ok(Some(alice)));
		assert_eq!(db.lookup("person/badge", &Value::Int(2)), Ok(None));
		assert_eq!(db.lookup("person/name", &"Alice".into()), Err(Error::NotUnique("person/name".into())));
		assert!(matches!(db.lookup("person/badge", &"1".into()), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.lookup("person/nope", &"1".into()), Err(Error::UnknownAttr(_))));
		// Lookups resolve before the transaction, so one can't find what the same transaction asserts
		let mut tx = Tx::new();
		let (t, l) = (tx.temp(), tx.lookup("person/email", "eve@x"));
		tx.add(t, "person/email", "eve@x").add(l, "person/age", 1);
		assert_eq!(conn.transact(&tx).err(), Some(Error::LookupNotFound { attr: "person/email".into(), value: "eve@x".into() }));
		// In a query, a lookup ref is a constant entity
		let mut q = db.query();
		let (f, n) = (q.var("?f"), q.var("?n"));
		let b = q.lookup("person/email", "bob@x").unwrap();
		q.pattern(b, "person/friend", f.clone()).unwrap();
		q.pattern(f, "person/name", n.clone()).unwrap();
		assert_eq!(q.run(&[n]).unwrap(), vec![vec![Value::from("Alice")]]);
		assert!(matches!(q.lookup("person/email", "nobody@x"), Err(Error::LookupNotFound { .. })));
		assert!(matches!(q.lookup("person/name", "Bob"), Err(Error::NotUnique(_))));
	}

	#[test]
	fn retract_entity_cascades_through_components()
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("order/id", ValueType::Int, Cardinality::One)
			.define_attr(Attr::new("order/lines", ValueType::Ref, Cardinality::Many).component())
			.define("line/product", ValueType::Ref, Cardinality::One)
			.define("product/name", ValueType::Str, Cardinality::One)
			.define("customer/last", ValueType::Ref, Cardinality::One);
		let (o, l1, l2, p, c) = (tx.temp(), tx.temp(), tx.temp(), tx.temp(), tx.temp());
		tx.add(o, "order/id", 1).add(o, "order/lines", l1).add(o, "order/lines", l2);
		tx.add(l1, "line/product", p).add(l2, "line/product", p).add(p, "product/name", "pen").add(c, "customer/last", o);
		let r = conn.transact(&tx).unwrap();
		let mut tx = Tx::new();
		tx.retract_entity(o);
		assert!(matches!(conn.transact(&tx), Err(Error::ForeignEntity(_))));
		let mut tx = Tx::new();
		tx.retract_entity(Entity::Id(r.eid(o)));
		let r2 = conn.transact(&tx).unwrap();
		// The order, its lines (components) and the customer's reference go; the product (not a component) stays
		let left = datoms(&conn.db());
		let pname = conn.db().attr_id("product/name").unwrap();
		assert_eq!(left, BTreeSet::from([(r.eid(p), pname, Value::from("pen"))]));
		assert_eq!(r2.tx_data.iter().filter(|d| !d.added).count(), 6);
		assert_eq!(conn.db().referrers(r.eid(p)).count(), 0);
		// Retracting an entity and asserting one of its datoms in the same transaction conflicts
		let mut tx = Tx::new();
		tx.retract_entity(Entity::Id(r.eid(p))).add(Entity::Id(r.eid(p)), "product/name", "pen");
		assert!(matches!(conn.transact(&tx), Err(Error::AddRetractConflict { .. })));
		// ... but asserting a new value of it doesn't
		let mut tx = Tx::new();
		tx.retract_entity(Entity::Id(r.eid(p))).add(Entity::Id(r.eid(p)), "product/name", "ink");
		conn.transact(&tx).unwrap();
		assert_eq!(datoms(&conn.db()), BTreeSet::from([(r.eid(p), pname, Value::from("ink"))]));
	}

	fn kind(e: Error) -> &'static str
	{
		match e
		{
			Error::AddRetractConflict { .. } => "add-retract",
			Error::CardinalityConflict { .. } => "cardinality",
			Error::UniqueConflict { .. } => "unique",
			e => panic!("unexpected error {e}"),
		}
	}

	fn shuffle<X>(v: &mut [X], rng: &mut Rng)
	{
		for i in (1..v.len()).rev()
		{
			v.swap(i, (rng.next() % (i as u64 + 1)) as usize);
		}
	}

	fn check_stats(db: &Db, attrs: &[AttrId])
	{
		let all = datoms(db);
		for &a in attrs
		{
			let of_a: Vec<&Triple> = all.iter().filter(|d| d.1 == a).collect();
			let entities = of_a.iter().map(|d| d.0).collect::<BTreeSet<_>>().len() as u64;
			let values = of_a.iter().map(|d| &d.2).collect::<BTreeSet<_>>().len() as u64;
			assert_eq!(db.stats(a), AttrStats { datoms: of_a.len() as u64, entities, values });
		}
	}

	// A transaction is a set of operations against the database before it: its outcome doesn't depend on the order of the operations,
	// and it equals a naive model of the set semantics, including which transactions conflict
	#[test]
	fn transactions_are_sets_matching_a_reference_model()
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("one", ValueType::Int, Cardinality::One)
			.define("many", ValueType::Int, Cardinality::Many)
			.define_attr(Attr::new("uniq", ValueType::Int, Cardinality::One).unique(Unique::Value));
		let es: Vec<Entity> = (0..4).map(|_| tx.temp()).collect();
		for &e in &es
		{
			tx.add(e, "many", 0);
		}
		let eids = conn.transact(&tx).unwrap().tempids;
		let names = ["one", "many", "uniq"];
		let ids: Vec<AttrId> = names.iter().map(|n| conn.db().attr_id(n).unwrap()).collect();
		let card_one = |a: AttrId| a != ids[1];
		let mut rng = Rng::new(3);
		let mut outcomes: BTreeMap<&str, u32> = BTreeMap::new();
		for _ in 0..600
		{
			let db = conn.db();
			let n = 1 + rng.next() % 5;
			let mut ops: Vec<(bool, usize, usize, i64)> =
				(0..n).map(|_| (rng.next() % 3 != 0, (rng.next() % 4) as usize, (rng.next() % 3) as usize, (rng.next() % 3) as i64)).collect();
			let triple = |&(_, e, a, v): &(bool, usize, usize, i64)| (eids[e], ids[a], Value::Int(v));
			let adds: BTreeSet<Triple> = ops.iter().filter(|o| o.0).map(triple).collect();
			let rets: BTreeSet<Triple> = ops.iter().filter(|o| !o.0).map(triple).collect();
			// A datom that a cardinality-one assertion of another value replaces
			let replaced = |d: &Triple| card_one(d.1) && adds.iter().any(|o| (o.0, o.1) == (d.0, d.1) && o.2 != d.2);
			let want = if !adds.is_disjoint(&rets)
			{
				Err("add-retract")
			}
			else if adds.iter().any(|d| replaced(d))
			{
				Err("cardinality")
			}
			else
			{
				let after: BTreeSet<Triple> = datoms(&db).into_iter().filter(|d| !rets.contains(d) && !replaced(d)).chain(adds.iter().cloned()).collect();
				let uniq: Vec<&Value> = after.iter().filter(|d| d.1 == ids[2]).map(|d| &d.2).collect();
				if uniq.iter().collect::<BTreeSet<_>>().len() < uniq.len() { Err("unique") } else { Ok(after) }
			};
			*outcomes.entry(want.as_ref().err().copied().unwrap_or("ok")).or_default() += 1;
			let mut first = None;
			for _ in 0..2
			{
				let mut tx = Tx::new();
				for &(added, e, a, v) in &ops
				{
					if added { tx.add(Entity::Id(eids[e]), names[a], v) } else { tx.retract(Entity::Id(eids[e]), names[a], v) };
				}
				let got = db.transact(&tx);
				if let Ok(r) = &got
				{
					check_stats(&r.db_after, &ids);
				}
				assert_eq!(got.map(|r| datoms(&r.db_after)).map_err(kind), want, "ops {ops:?}");
				first.get_or_insert(tx);
				shuffle(&mut ops, &mut rng);
			}
			if want.is_ok()
			{
				conn.transact(&first.expect("two attempts")).unwrap();
			}
		}
		for k in ["ok", "add-retract", "cardinality", "unique"]
		{
			assert!(outcomes.get(k).copied().unwrap_or(0) >= 30, "too few {k} outcomes: {outcomes:?}");
		}
	}

	// retract_entity against a naive closure over a scan of all datoms, and the VAET index against a scan, over random reference graphs
	#[test]
	fn retract_entity_and_vaet_match_a_scan()
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("n", ValueType::Int, Cardinality::One)
			.define("ref", ValueType::Ref, Cardinality::Many)
			.define("one", ValueType::Ref, Cardinality::One)
			.define_attr(Attr::new("part", ValueType::Ref, Cardinality::Many).component());
		let es: Vec<Entity> = (0..12).map(|_| tx.temp()).collect();
		for (i, &e) in es.iter().enumerate()
		{
			tx.add(e, "n", i as i64);
		}
		let eids = conn.transact(&tx).unwrap().tempids;
		let names = ["ref", "one", "part"];
		let db = conn.db();
		let refs: Vec<AttrId> = names.iter().map(|n| db.attr_id(n).unwrap()).collect();
		let all: Vec<AttrId> = refs.iter().copied().chain([db.attr_id("n").unwrap()]).collect();
		let mut rng = Rng::new(17);
		let mut retracted = 0;
		for round in 0..300
		{
			let db = conn.db();
			let before = datoms(&db);
			let mut tx = Tx::new();
			if round % 4 == 3
			{
				let roots: Vec<Eid> = (0..1 + rng.next() % 2).map(|_| eids[(rng.next() % 12) as usize]).collect();
				for &x in &roots
				{
					tx.retract_entity(Entity::Id(x));
				}
				// The naive closure: entities reachable from the roots through component datoms
				let mut doomed: BTreeSet<Eid> = roots.into_iter().collect();
				loop
				{
					let more: BTreeSet<Eid> = before
						.iter()
						.filter_map(|d| match d.2
						{
							Value::Ref(c) if d.1 == refs[2] && doomed.contains(&d.0) && !doomed.contains(&c) => Some(c),
							_ => None,
						})
						.collect();
					if more.is_empty()
					{
						break;
					}
					doomed.extend(more);
				}
				let want: BTreeSet<Triple> =
					before.iter().filter(|d| !doomed.contains(&d.0) && !matches!(d.2, Value::Ref(c) if doomed.contains(&c))).cloned().collect();
				retracted += before.len() - want.len();
				conn.transact(&tx).unwrap();
				assert_eq!(datoms(&conn.db()), want);
			}
			else
			{
				for _ in 0..1 + rng.next() % 3
				{
					let (e, a, v) = (eids[(rng.next() % 12) as usize], names[(rng.next() % 3) as usize], eids[(rng.next() % 12) as usize]);
					if rng.next() % 4 == 0 { tx.retract(Entity::Id(e), a, Entity::Id(v)) } else { tx.add(Entity::Id(e), a, Entity::Id(v)) };
				}
				if let Err(e) = conn.transact(&tx)
				{
					assert!(matches!(e, Error::CardinalityConflict { .. } | Error::AddRetractConflict { .. }), "{e}");
				}
			}
			let db = conn.db();
			let scan: BTreeSet<(Eid, AttrId, Eid)> = datoms(&db)
				.into_iter()
				.filter_map(|d| match d.2
				{
					Value::Ref(c) => Some((c, d.1, d.0)),
					_ => None,
				})
				.collect();
			let vaet: BTreeSet<(Eid, AttrId, Eid)> = eids.iter().flat_map(|&x| db.referrers(x).map(move |(a, e)| (x, a, e))).collect();
			assert_eq!(vaet, scan);
			check_stats(&db, &all);
		}
		assert!(retracted > 100, "too few retractions: {retracted}");
	}

	/// The attributes of `random_conn`; the Refs are "ref", "best" and "part" (a component)
	pub(crate) const RANDOM_ATTRS: [&str; 6] = ["n", "tag", "ref", "best", "part", "key"];

	/// A random workload over attributes of every kind (with conflicting transactions, which fail, upserts and retract_entity):
	/// the connection and the report of every transaction, the first one defining the schema and 10 entities. Transaction i has the instant 10 * (i + 1)
	pub(crate) fn random_conn(seed: u64, txs: usize) -> (Conn, Vec<TxReport>)
	{
		let mut conn = Conn::new();
		let reports = random_workload(&mut conn, seed, txs);
		(conn, reports)
	}

	/// The workload of `random_conn`, into an empty connection (in memory or durable)
	pub(crate) fn random_workload(conn: &mut Conn, seed: u64, txs: usize) -> Vec<TxReport>
	{
		let mut tx = Tx::new();
		tx.define("n", ValueType::Int, Cardinality::One)
			.define("tag", ValueType::Str, Cardinality::Many)
			.define("ref", ValueType::Ref, Cardinality::Many)
			.define("best", ValueType::Ref, Cardinality::One)
			.define_attr(Attr::new("part", ValueType::Ref, Cardinality::Many).component())
			.define_attr(Attr::new("key", ValueType::Int, Cardinality::One).unique(Unique::Identity))
			.with_instant(10);
		let es: Vec<Entity> = (0..10).map(|_| tx.temp()).collect();
		for (i, &e) in es.iter().enumerate()
		{
			tx.add(e, "n", i as i64);
		}
		let first = conn.transact(&tx).unwrap();
		let eids = first.tempids.clone();
		let mut reports = vec![first];
		let mut rng = Rng::new(seed);
		while reports.len() < txs
		{
			let mut tx = Tx::new();
			tx.with_instant(10 * (reports.len() as i64 + 1));
			let pick = |rng: &mut Rng| Entity::Id(eids[(rng.next() % 10) as usize]);
			if rng.next() % 8 == 0
			{
				let e = pick(&mut rng);
				tx.retract_entity(e);
			}
			else
			{
				for _ in 0..1 + rng.next() % 4
				{
					let (e, v) = (pick(&mut rng), pick(&mut rng));
					let added = rng.next() % 3 != 0;
					let k = rng.next();
					let val: TxValue = match k % 6
					{
						0 | 5 => Value::Int((k / 6 % 4) as i64).into(),
						1 => ["a", "b", "c"][(k / 6 % 3) as usize].into(),
						_ => v.into(),
					};
					// A tempid with a key upserts to its holder or becomes a new entity
					let e = if k % 6 == 5 && k / 6 % 5 == 0 { tx.temp() } else { e };
					if added { tx.add(e, RANDOM_ATTRS[(k % 6) as usize], val) } else { tx.retract(e, RANDOM_ATTRS[(k % 6) as usize], val) };
				}
			}
			// Conflicting transactions fail and leave no trace
			if let Ok(r) = conn.transact(&tx)
			{
				reports.push(r);
			}
		}
		reports
	}

	/// Assert that two database values are the same: every field, every index (with its transactions) and the view markers
	pub(crate) fn assert_same_db(x: &Db, y: &Db, what: &str)
	{
		fn all<K: Ord + std::hash::Hash + Clone, V: Clone>(m: &PMap<K, V>) -> Vec<(K, V)>
		{
			m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
		}
		assert_eq!((x.basis_t, x.next_eid, x.instant, x.history, x.since, x.upto), (y.basis_t, y.next_eid, y.instant, y.history, y.since, y.upto), "{what}");
		assert_eq!(all(&x.attrs), all(&y.attrs), "{what}");
		assert_eq!(all(&x.attr_ids), all(&y.attr_ids), "{what}");
		assert_eq!(all(&x.stats), all(&y.stats), "{what}");
		assert_eq!(all(&x.hstats), all(&y.hstats), "{what}");
		assert_eq!(all(&x.txs), all(&y.txs), "{what}");
		assert_eq!(all(&x.eavt), all(&y.eavt), "{what}");
		assert_eq!(all(&x.aevt), all(&y.aevt), "{what}");
		assert_eq!(all(&x.avet), all(&y.avet), "{what}");
		assert_eq!(all(&x.vaet), all(&y.vaet), "{what}");
		assert_eq!(all(&x.heavt), all(&y.heavt), "{what}");
		assert_eq!(all(&x.haevt), all(&y.haevt), "{what}");
		assert_eq!(all(&x.havet), all(&y.havet), "{what}");
		assert_eq!(all(&x.hvaet), all(&y.hvaet), "{what}");
		assert_eq!(all(&x.tlog), all(&y.tlog), "{what}");
		assert_eq!(all(&x.schema), all(&y.schema), "{what}");
	}

	/// Assert that two database values (either may be a view) read the same: basis, schema, the log, and every accessor
	/// over every entity, attribute and value either has ever had
	pub(crate) fn assert_same_reads(x: &Db, y: &Db, what: &str)
	{
		assert_eq!((x.basis_t(), x.next_eid, x.is_history()), (y.basis_t(), y.next_eid, y.is_history()), "{what}");
		let nattrs = x.attrs.len().max(y.attrs.len()).max(x.schema.len()) as AttrId;
		for a in 0..nattrs
		{
			assert_eq!(x.try_attr(a), y.try_attr(a), "{what}: attribute {a}");
			if let Some(def) = x.try_attr(a)
			{
				assert_eq!(x.attr_id(&def.name), y.attr_id(&def.name), "{what}");
			}
		}
		for t in 0..=x.basis_t() + 1
		{
			assert_eq!(x.tx_of(t), y.tx_of(t), "{what}: tx of {t}");
		}
		assert_eq!(x.tx_range(0, T::MAX), y.tx_range(0, T::MAX), "{what}: log");
		let attrs: Vec<AttrId> = (0..nattrs).filter(|&a| x.try_attr(a).is_some()).collect();
		let ents: Vec<Eid> = (1..x.next_eid.max(y.next_eid)).collect();
		for &a in &attrs
		{
			let w = format!("{what}: attribute {}", x.attr(a).name);
			assert_eq!(x.datoms(None, a, None), y.datoms(None, a, None), "{w}");
			assert_eq!(x.attr_datoms(a).collect::<Vec<_>>(), y.attr_datoms(a).collect::<Vec<_>>(), "{w}");
			// Every value the attribute ever had, in either
			let vals: BTreeSet<Value> = x.history().datoms(None, a, None).into_iter().chain(y.history().datoms(None, a, None)).map(|d| d.v).collect();
			for v in &vals
			{
				assert_eq!(x.datoms(None, a, Some(v)), y.datoms(None, a, Some(v)), "{w} = {v:?}");
				assert_eq!(x.entities(a, v).collect::<Vec<_>>(), y.entities(a, v).collect::<Vec<_>>(), "{w} = {v:?}");
			}
			for &e in &ents
			{
				assert_eq!(x.datoms(Some(e), a, None), y.datoms(Some(e), a, None), "{w} of {e}");
				assert_eq!(x.values(e, a).collect::<Vec<_>>(), y.values(e, a).collect::<Vec<_>>(), "{w} of {e}");
				for v in &vals
				{
					assert_eq!(x.has(e, a, v), y.has(e, a, v), "{w} of {e} = {v:?}");
					assert_eq!(x.datoms(Some(e), a, Some(v)), y.datoms(Some(e), a, Some(v)), "{w} of {e} = {v:?}");
				}
			}
		}
		for &e in &ents
		{
			assert_eq!(x.entity_datoms(e).collect::<Vec<_>>(), y.entity_datoms(e).collect::<Vec<_>>(), "{what}: entity {e}");
			assert_eq!(x.referrers(e).collect::<Vec<_>>(), y.referrers(e).collect::<Vec<_>>(), "{what}: referrers of {e}");
		}
	}

	/// The current datoms that a transaction log leaves, with the transaction that asserted each
	pub(crate) fn replay<'a>(log: impl IntoIterator<Item = &'a Datom>) -> BTreeMap<Triple, Eid>
	{
		let mut current = BTreeMap::new();
		for d in log
		{
			if d.added
			{
				assert!(current.insert((d.e, d.a, d.v.clone()), d.tx).is_none(), "asserting a present datom {d:?}");
			}
			else
			{
				assert!(current.remove(&(d.e, d.a, d.v.clone())).is_some(), "retracting an absent datom {d:?}");
			}
		}
		current
	}

	type Row = (Eid, AttrId, Value, Eid, bool);

	fn rows(ds: impl IntoIterator<Item = Datom>) -> Vec<Row>
	{
		let mut v: Vec<Row> = ds.into_iter().map(|d| (d.e, d.a, d.v, d.tx, d.added)).collect();
		v.sort();
		v
	}

	// Every past value's history, current state and since views against the concatenated tx_data of the reports (the reference log),
	// for every combination of bound positions
	#[test]
	fn history_and_since_views_match_the_log()
	{
		let (conn, reports) = random_conn(23, 50);
		let last = conn.db();
		assert_eq!(last.basis_t() as usize, reports.len());
		let attrs: Vec<AttrId> = RANDOM_ATTRS.iter().chain([&TX_INSTANT]).map(|n| last.attr_id(n).unwrap()).collect();
		let mut rng = Rng::new(4);
		for t in 0..=reports.len()
		{
			let db = conn.as_of(t as T).unwrap();
			let log: Vec<&Datom> = reports[..t].iter().flat_map(|r| &r.tx_data).collect();
			let current = replay(log.iter().copied());
			let ents: Vec<Eid> = log.iter().map(|d| d.e).collect::<BTreeSet<_>>().into_iter().collect();
			for s in 0..=t
			{
				let after = if s == 0 { 0 } else { reports[s - 1].tx };
				assert_eq!(db.tx_of(s as T), (s > 0).then_some(after));
				let since = db.since(s as T).unwrap();
				let hist_log = || log.iter().filter(|d| d.tx > after).map(|d| (*d).clone());
				let cur_log = || current.iter().filter(|(_, tx)| **tx > after).map(|((e, a, v), tx)| Datom { e: *e, a: *a, v: v.clone(), tx: *tx, added: true });
				for (view, hist) in [(since.clone(), false), (since.history(), true), (db.history().since(s as T).unwrap(), true)]
				{
					assert_eq!(view.is_history(), hist);
					// A past value is an as-of view (of the latest one)
				assert_eq!(view.transact(&Tx::new()).err(), if hist || s > 0 || t < reports.len() { Some(Error::FilteredDb) } else { None });
					for &a in &attrs
					{
						let vals: Vec<Value> = log.iter().filter(|d| d.a == a).map(|d| d.v.clone()).collect();
						for _ in 0..3
						{
							let e = (!ents.is_empty() && rng.next() % 2 == 0).then(|| ents[(rng.next() % ents.len() as u64) as usize]);
							let v = (!vals.is_empty() && rng.next() % 2 == 0).then(|| vals[(rng.next() % vals.len() as u64) as usize].clone());
							let matches = |d: &Datom| d.a == a && e.map_or(true, |e| d.e == e) && v.as_ref().map_or(true, |v| d.v == *v);
							let want = if hist { rows(hist_log().filter(matches)) } else { rows(cur_log().filter(matches)) };
							assert_eq!(rows(view.datoms(e, a, v.as_ref())), want, "t {t} since {s} history {hist} e {e:?} v {v:?}");
						}
					}
				}
				// The current-state accessors of the since view
				for &a in &attrs
				{
					let want: Vec<(Eid, Value)> = cur_log().filter(|d| d.a == a).map(|d| (d.e, d.v)).collect();
					assert_eq!(since.attr_datoms(a).map(|(e, v)| (e, v.clone())).collect::<Vec<_>>(), want);
				}
				for &x in &ents
				{
					let want: BTreeSet<(AttrId, Eid)> = cur_log().filter(|d| d.v == Value::Ref(x)).map(|d| (d.a, d.e)).collect();
					assert_eq!(since.referrers(x).collect::<BTreeSet<_>>(), want);
					let want: Vec<(AttrId, Value)> = cur_log().filter(|d| d.e == x).map(|d| (d.a, d.v)).collect();
					assert_eq!(since.entity_datoms(x).map(|(a, v)| (a, v.clone())).collect::<Vec<_>>(), want);
				}
			}
			assert!(db.since(t as T + 1).is_none());
			// The history statistics count every change (a past value, an as-of view, has the latest value's statistics, an estimate)
			let h = db.history();
			for &a in &attrs
			{
				if t < reports.len()
				{
					assert_eq!(h.stats(a), last.history().stats(a));
					continue;
				}
				let of_a: Vec<&&Datom> = log.iter().filter(|d| d.a == a).collect();
				let entities = of_a.iter().map(|d| d.e).collect::<BTreeSet<_>>().len() as u64;
				let values = of_a.iter().map(|d| &d.v).collect::<BTreeSet<_>>().len() as u64;
				assert_eq!(h.stats(a), AttrStats { datoms: of_a.len() as u64, entities, values });
			}
		}
		// The workload has retractions of every kind
		let log: Vec<&Datom> = reports.iter().flat_map(|r| &r.tx_data).collect();
		for a in &attrs[..6]
		{
			assert!(log.iter().filter(|d| d.a == *a && !d.added).count() >= 3, "too few retractions of {}", last.attr(*a).name);
		}
	}

	#[test]
	fn cas_compares_against_the_database_before_the_transaction()
	{
		let (mut conn, r) = people();
		let (alice, bob) = (Entity::Id(r.tempids[0]), Entity::Id(r.tempids[1]));
		let age = conn.db().attr_id("person/age").unwrap();
		let cas = |e: Entity, expected: Option<i64>, new: i64| {
			let mut tx = Tx::new();
			tx.cas(e, "person/age", expected.map(TxValue::from), new);
			tx
		};
		conn.transact(&cas(alice, Some(30), 31)).unwrap();
		assert_eq!(conn.db().values(r.tempids[0], age).collect::<Vec<_>>(), vec![&Value::Int(31)]);
		let failed = Error::CasFailed { e: r.tempids[0], attr: "person/age".into(), expected: Some(Value::Int(30)), found: Some(Value::Int(31)) };
		assert_eq!(conn.transact(&cas(alice, Some(30), 32)).err(), Some(failed));
		// No value expected: an entity without one, and a new entity
		conn.transact(&cas(bob, None, 20)).unwrap();
		assert!(matches!(conn.transact(&cas(bob, None, 21)), Err(Error::CasFailed { found: Some(Value::Int(20)), .. })));
		let mut tx = Tx::new();
		let t = tx.temp();
		tx.cas(t, "person/age", None, 5);
		let r2 = conn.transact(&tx).unwrap();
		assert_eq!(conn.db().values(r2.tempids[0], age).next(), Some(&Value::Int(5)));
		// Two swaps of one value in one transaction are two values of a cardinality-one attribute
		let mut tx = cas(alice, Some(31), 40);
		tx.add(alice, "person/age", 41);
		assert!(matches!(conn.transact(&tx), Err(Error::CardinalityConflict { .. })));
		// The attribute must be cardinality-one, and the values of its type
		let mut tx = Tx::new();
		tx.cas(alice, "person/friend", None, bob);
		assert!(matches!(conn.transact(&tx), Err(Error::InvalidSchema(_))));
		let mut tx = Tx::new();
		tx.cas(alice, "person/age", Some("old".into()), 1);
		assert!(matches!(conn.transact(&tx), Err(Error::TypeMismatch { .. })));
		assert_eq!(conn.db().values(r.tempids[0], age).collect::<Vec<_>>(), vec![&Value::Int(31)]);
	}

	#[test]
	fn alter_attr_validates_against_the_data()
	{
		let (mut conn, r) = people();
		let (alice, bob) = (Entity::Id(r.tempids[0]), Entity::Id(r.tempids[1]));
		let alter = |attr: Attr| {
			let mut tx = Tx::new();
			tx.alter_attr(attr);
			tx
		};
		let friend_one = Attr::new("person/friend", ValueType::Ref, Cardinality::One);
		// Cardinality one: refused while an entity has two values
		let mut tx = Tx::new();
		tx.add(alice, "person/friend", alice);
		conn.transact(&tx).unwrap();
		assert!(matches!(conn.transact(&alter(friend_one.clone())), Err(Error::AlterRefused { .. })));
		let mut tx = Tx::new();
		tx.retract(alice, "person/friend", alice);
		conn.transact(&tx).unwrap();
		let before = conn.db().basis_t();
		conn.transact(&alter(friend_one.clone())).unwrap();
		let friend = conn.db().attr_id("person/friend").unwrap();
		assert_eq!(conn.db().attr(friend).cardinality, Cardinality::One);
		assert_eq!(conn.as_of(before).unwrap().attr(friend).cardinality, Cardinality::Many);
		// ... and from then on a new value replaces the old one
		let mut tx = Tx::new();
		tx.add(alice, "person/friend", alice);
		conn.transact(&tx).unwrap();
		assert_eq!(conn.db().values(r.tempids[0], friend).collect::<Vec<_>>(), vec![&Value::Ref(r.tempids[0])]);

		// Uniqueness: refused while two entities share a value; then upserts and lookups work
		let age_unique = Attr::new("person/age", ValueType::Int, Cardinality::One).unique(Unique::Identity);
		let mut tx = Tx::new();
		tx.add(bob, "person/age", 30);
		conn.transact(&tx).unwrap();
		assert!(matches!(conn.transact(&alter(age_unique.clone())), Err(Error::AlterRefused { .. })));
		let mut tx = Tx::new();
		tx.add(bob, "person/age", 40);
		conn.transact(&tx).unwrap();
		conn.transact(&alter(age_unique.clone())).unwrap();
		let mut tx = Tx::new();
		let t = tx.temp();
		tx.add(t, "person/age", 30).add(t, "person/name", "Alice A.");
		assert_eq!(conn.transact(&tx).unwrap().tempids[0], r.tempids[0]);
		assert_eq!(conn.db().lookup("person/age", &Value::Int(40)).unwrap(), Some(r.tempids[1]));
		// An alteration and data in one transaction: the data is checked under the new schema
		let badge_plain = Attr::new("person/badge", ValueType::Int, Cardinality::One);
		let mut tx = Tx::new();
		tx.alter_attr(badge_plain).add(bob, "person/badge", 1);
		conn.transact(&tx).unwrap();
		let badge = conn.db().attr_id("person/badge").unwrap();
		assert_eq!(conn.db().entities(badge, &Value::Int(1)).count(), 2);

		// What can't change
		assert!(matches!(conn.transact(&alter(Attr::new("person/age", ValueType::Str, Cardinality::One))), Err(Error::AlterRefused { .. })));
		assert!(matches!(conn.transact(&alter(Attr::new("person/age", ValueType::Int, Cardinality::One).component())), Err(Error::InvalidSchema(_))));
		assert!(matches!(conn.transact(&alter(Attr::new("nope", ValueType::Int, Cardinality::One))), Err(Error::UnknownAttr(_))));
		assert!(matches!(conn.transact(&alter(Attr::new(TX_INSTANT, ValueType::Instant, Cardinality::Many))), Err(Error::Reserved(_))));
		// An alteration is recorded for the durable log, and replays to the same value
		let mut tx = Tx::new();
		tx.alter_attr(Attr::new("person/email", ValueType::Str, Cardinality::One));
		let rep = conn.db().transact(&tx).unwrap();
		let email = conn.db().attr_id("person/email").unwrap();
		assert_eq!(rep.record().attrs, vec![(email, Attr::new("person/email", ValueType::Str, Cardinality::One))]);
		assert_same_db(&rep.db_before.clone().replay(&rep.record()).unwrap(), &rep.db_after, "replayed alteration");
	}

	#[test]
	fn tx_range_matches_the_reports()
	{
		let (conn, reports) = random_conn(31, 40);
		let db = conn.db();
		let log = db.tx_range(0, T::MAX);
		assert_eq!(log.len(), reports.len());
		for (entry, r) in log.iter().zip(&reports)
		{
			assert_eq!((entry.t, entry.tx), (r.db_after.basis_t(), r.tx));
			let mut want = r.tx_data.clone();
			want.sort_by(|x, y| (x.e, x.a, &x.v, x.added).cmp(&(y.e, y.a, &y.v, y.added)));
			assert_eq!(entry.data, want);
		}
		// A range, a past value's log, and the views
		assert_eq!(db.tx_range(5, 8), log[4..7].to_vec());
		assert_eq!(conn.as_of(10).unwrap().tx_range(0, T::MAX), log[..10].to_vec());
		assert_eq!(db.history().tx_range(3, 4), log[2..3].to_vec());
		assert_eq!(db.since(20).unwrap().tx_range(0, T::MAX), log);
		assert!(db.tx_range(9, 9).is_empty() && db.tx_range(db.basis_t() + 1, T::MAX).is_empty());
	}

	// The order of values is total and consistent with equality and hashing, Floats included (NaN, -0.0, infinities)
	#[test]
	fn value_order_is_total()
	{
		use std::hash::{BuildHasher, BuildHasherDefault};
		let hash = |v: &Value| BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(v);
		let vals = [
			Value::Bool(false),
			Value::Int(-3),
			Value::Int(7),
			Value::Float(f64::NEG_INFINITY),
			Value::Float(-1.5),
			Value::Float(-0.0),
			Value::Float(0.0),
			Value::Float(2.0),
			Value::Float(f64::INFINITY),
			Value::Float(f64::NAN),
			Value::from("a"),
			Value::Instant(-5),
			Value::Instant(5),
			Value::Ref(1),
		];
		// Listed in order: by type, then within it
		for (i, x) in vals.iter().enumerate()
		{
			for (j, y) in vals.iter().enumerate()
			{
				assert_eq!(x.cmp(y), i.cmp(&j), "{x:?} vs {y:?}");
				assert_eq!(x == y, i == j);
				if i == j
				{
					assert_eq!(hash(x), hash(&x.clone()));
				}
			}
		}
		// In an index: a Float attribute holds NaN and both zeros as distinct values
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("x", ValueType::Float, Cardinality::Many);
		let e = tx.temp();
		for f in [f64::NAN, 0.0, -0.0, 1.0]
		{
			tx.add(e, "x", f);
		}
		let r = conn.transact(&tx).unwrap();
		let x = conn.db().attr_id("x").unwrap();
		assert_eq!(conn.db().values(r.tempids[0], x).count(), 4);
		assert!(conn.db().has(r.tempids[0], x, &Value::Float(f64::NAN)));
	}

	// basis_of_instant and basis_of_tx against a scan of the transactions
	#[test]
	fn bases_by_instant_and_transaction()
	{
		let (conn, reports) = random_conn(13, 30);
		let db = conn.db();
		let instants: Vec<i64> = reports.iter().map(|r| match r.tx_data[0].v
		{
			Value::Instant(i) => i,
			_ => unreachable!(),
		}).collect();
		for i in instants[0] - 5..instants.last().unwrap() + 5
		{
			let want = instants.iter().filter(|&&x| x <= i).count() as T;
			assert_eq!(db.basis_of_instant(i), want, "instant {i}");
		}
		for (t, r) in reports.iter().enumerate()
		{
			assert_eq!(db.basis_of_tx(r.tx), Some(t as T + 1));
			// An entity that isn't a transaction, and a past view (whose history ends at its basis)
			assert_eq!(db.basis_of_tx(r.tx + 1).filter(|_| !reports.iter().any(|q| q.tx == r.tx + 1)), None);
			let past = db.as_of(t as T + 1).unwrap();
			assert_eq!(past.basis_of_instant(i64::MAX), t as T + 1);
			assert_eq!(past.basis_of_tx(reports.last().unwrap().tx), if t + 1 == reports.len() { Some(t as T + 1) } else { None });
		}
		assert_eq!(db.basis_of_tx(0), None);
		assert_eq!(Db::new().basis_of_instant(0), 0);
	}

	// A value rebuilt from its snapshot (every index and statistic derived from the history alone) is the value transact maintained,
	// at every basis of two workloads: a check of the incremental maintenance by a second implementation
	#[test]
	fn snapshots_rebuild_the_same_value()
	{
		for seed in [3, 41]
		{
			let (mut conn, mut reports) = random_conn(seed, 40);
			let mut tx = Tx::new();
			tx.alter_attr(Attr::new("key", ValueType::Int, Cardinality::One)).define("extra", ValueType::Str, Cardinality::One);
			reports.push(conn.transact(&tx).unwrap());
			let mut values = vec![Db::new()];
			values.extend(reports.iter().map(|r| r.db_after.clone()));
			for (t, db) in values.iter().enumerate()
			{
				let rebuilt = Db::from_snapshot(db.snapshot()).unwrap();
				assert_same_db(&rebuilt, db, &format!("seed {seed}, basis {t}"));
				assert_eq!(rebuilt.snapshot(), db.snapshot());
			}
		}
		// Malformed snapshots are refused
		let (conn, _) = random_conn(5, 10);
		let good = conn.db().snapshot();
		let mut s = good.clone();
		s.history.swap(0, 1);
		assert!(Db::from_snapshot(s).is_err());
		let mut s = good.clone();
		s.history[0].2 = 99;
		assert!(Db::from_snapshot(s).is_err());
		let mut s = good.clone();
		s.txs.pop();
		assert!(Db::from_snapshot(s).is_err());
		let mut s = good;
		s.schema.remove(0);
		assert!(Db::from_snapshot(s).is_err());
	}

	// Every past value, as an as-of view of the latest one, reads the same as the value the transaction produced at the time
	// (with schema alterations, retract_entity, components, a unique attribute and refs), and so do its history and since views
	#[test]
	fn as_of_views_read_like_the_past_values()
	{
		let (mut conn, mut reports) = random_conn(37, 30);
		let eids: Vec<Eid> = reports[0].tempids.clone();
		let mut rng = Rng::new(8);
		for round in 0..12
		{
			let mut tx = Tx::new();
			match round % 4
			{
				// Alterations, some of them refused by the data (and so leaving no trace)
				0 => tx.alter_attr(Attr::new("key", ValueType::Int, Cardinality::One)),
				1 => tx.alter_attr(Attr::new("key", ValueType::Int, Cardinality::One).unique(Unique::Identity)),
				2 => tx.alter_attr(Attr::new("part", ValueType::Ref, Cardinality::Many)),
				_ => tx.alter_attr(Attr::new("tag", ValueType::Str, Cardinality::One)),
			};
			for _ in 0..3
			{
				let (e, r) = (Entity::Id(eids[(rng.next() % 10) as usize]), Entity::Id(eids[(rng.next() % 10) as usize]));
				match rng.next() % 3
				{
					0 => tx.add(e, "ref", r),
					1 => tx.retract(e, "ref", r),
					_ => tx.add(e, "n", (rng.next() % 5) as i64),
				};
			}
			if let Ok(r) = conn.transact(&tx)
			{
				reports.push(r);
			}
		}
		let latest = conn.db();
		assert!(latest.basis_t() > 32, "some alterations went through");
		for t in 0..latest.basis_t()
		{
			let view = latest.as_of(t).unwrap();
			let past = if t == 0 { Db::new() } else { reports[t as usize - 1].db_after.clone() };
			assert_same_reads(&view, &past, &format!("as of {t}"));
			assert_same_reads(&view.history(), &past.history(), &format!("history as of {t}"));
			for s in [0, t / 2, t]
			{
				assert_same_reads(&view.since(s).unwrap(), &past.since(s).unwrap(), &format!("as of {t} since {s}"));
			}
			// Views compose: as of t of a view as of a later basis, and a history view's as of
			assert_same_reads(&latest.as_of(t + 1).unwrap().as_of(t).unwrap(), &past, &format!("as of {t} twice"));
			assert_same_reads(&latest.history().as_of(t).unwrap(), &past.history(), &format!("history, then as of {t}"));
			assert!(view.as_of(t + 1).is_none());
			assert_eq!(view.transact(&Tx::new()).err(), Some(Error::FilteredDb));
		}
		assert_eq!(latest.as_of(latest.basis_t()).unwrap().upto, None);
	}

	// Throughput of bulk loads (release: cargo test --release bulk_timings -- --ignored --nocapture): large transactions of new entities,
	// 200 entities of 5 attributes each per transaction, so that each index's insertions cluster
	#[test]
	#[ignore]
	fn bulk_timings()
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		for a in ["a", "b", "c", "d"]
		{
			tx.define(a, ValueType::Int, Cardinality::One);
		}
		tx.define("name", ValueType::Str, Cardinality::One);
		conn.transact(&tx).unwrap();
		let t = std::time::Instant::now();
		for round in 0..200i64
		{
			let mut tx = Tx::new();
			for i in 0..200i64
			{
				let e = tx.temp();
				let n = round * 200 + i;
				tx.add(e, "a", n % 97).add(e, "b", n % 13).add(e, "c", n).add(e, "d", -n).add(e, "name", format!("entity {n}").as_str());
			}
			conn.transact(&tx).unwrap();
		}
		println!("bulk: 200 transactions of 1000 datoms: {:?}", t.elapsed());
	}

	// Throughput of the indexes (release: cargo test --release index_timings -- --ignored --nocapture): transactions of 20 assertions
	// (a quarter on new entities), then reads of the current value, of a past one (as_of) and a full scan
	#[test]
	#[ignore]
	fn index_timings()
	{
		for txs in [10_000, 50_000]
		{
			let mut conn = Conn::new();
			let mut tx = Tx::new();
			tx.define("n", ValueType::Int, Cardinality::One).define("tag", ValueType::Str, Cardinality::Many).define("ref", ValueType::Ref, Cardinality::Many);
			conn.transact(&tx).unwrap();
			let mut rng = Rng::new(3);
			let mut eids: Vec<Eid> = Vec::new();
			let t = std::time::Instant::now();
			for _ in 1..txs
			{
				let mut tx = Tx::new();
				for _ in 0..20
				{
					let e = if eids.is_empty() || rng.next() % 4 == 0 { tx.temp() } else { Entity::Id(eids[(rng.next() % eids.len() as u64) as usize]) };
					match rng.next() % 3
					{
						0 => tx.add(e, "n", (rng.next() % 1000) as i64),
						1 => tx.add(e, "tag", ["a", "b", "c", "d"][(rng.next() % 4) as usize]),
						_ if !eids.is_empty() => tx.add(e, "ref", Entity::Id(eids[(rng.next() % eids.len() as u64) as usize])),
						_ => tx.add(e, "n", 0),
					};
				}
				if let Ok(r) = conn.transact(&tx)
				{
					eids.extend(r.tempids.iter().copied());
				}
			}
			let transact = t.elapsed();
			let db = conn.db();
			let (n, tag) = (db.attr_id("n").unwrap(), db.attr_id("tag").unwrap());
			let t = std::time::Instant::now();
			let mut hits = 0usize;
			for (i, e) in eids.iter().enumerate().step_by(7)
			{
				hits += db.values(*e, n).count() + db.entities(tag, &Value::from(["a", "b", "c", "d"][i % 4])).take(10).count();
			}
			let reads = t.elapsed();
			let past = db.as_of(db.basis_t() / 2).unwrap();
			let t = std::time::Instant::now();
			for e in eids.iter().take(eids.len() / 2).step_by(7)
			{
				hits += past.values(*e, n).count();
			}
			let past_reads = t.elapsed();
			let t = std::time::Instant::now();
			hits += db.attr_datoms(tag).count();
			let scan = t.elapsed();
			println!("{txs} transactions ({} entities): transact {transact:?}; {} point reads {reads:?}; as-of reads {past_reads:?}; scan {scan:?} ({hits})", eids.len(), eids.len() / 7);
		}
	}
}
