//! The database value: datoms in three persistent covering indexes, the schema and the per-attribute statistics the planner estimates with.
//!
//! A `Db` is immutable. `transact` returns a new `Db` that shares all the unchanged index nodes with the old one,
//! so keeping every past version (see `Conn::as_of`) costs only the changed paths.

use crate::pmap::PMap;
use std::fmt;
use std::sync::Arc;

pub type Eid = u64;
pub type AttrId = u32;
/// The basis of a database value: the number of transactions applied to it
pub type T = u64;

/// Values of different types order by their type (in the declaration order below)
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Value
{
	Bool(bool),
	Int(i64),
	Str(Arc<str>),
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
			Value::Str(_) => ValueType::Str,
			Value::Ref(_) => ValueType::Ref,
		}
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValueType
{
	Bool,
	Int,
	Str,
	Ref,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cardinality
{
	/// Asserting a new value retracts the entity's previous value of the attribute
	One,
	Many,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attr
{
	pub name: Arc<str>,
	pub value_type: ValueType,
	pub cardinality: Cardinality,
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
	pub tx: T,
	/// false for a retraction
	pub added: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Error
{
	UnknownAttr(String),
	/// An attribute redefined with a different type or cardinality
	AttrConflict(String),
	UnknownEntity(Eid),
	TypeMismatch { attr: String, expected: ValueType, got: ValueType },
	/// A variable used by a predicate or in the find list, but bound by no pattern of the query
	Unbound(String),
	/// The SAT planner was disabled after a solver error; the message is the solver's explanation
	Planner(String),
}

impl fmt::Display for Error
{
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result
	{
		match self
		{
			Error::UnknownAttr(a) => write!(f, "unknown attribute {a}"),
			Error::AttrConflict(a) => write!(f, "attribute {a} is already defined differently"),
			Error::UnknownEntity(e) => write!(f, "entity {e} was never allocated"),
			Error::TypeMismatch { attr, expected, got } => write!(f, "attribute {attr} expects {expected:?}, got {got:?}"),
			Error::Unbound(v) => write!(f, "variable {v} is bound by no pattern"),
			Error::Planner(m) => write!(f, "planner error: {m}"),
		}
	}
}

impl std::error::Error for Error {}

type PSet<K> = PMap<K, ()>;

#[derive(Clone, Default)]
pub struct Db
{
	basis_t: T,
	next_eid: Eid,
	attrs: PMap<AttrId, Attr>,
	attr_ids: PMap<Arc<str>, AttrId>,
	stats: PMap<AttrId, AttrStats>,
	eavt: PSet<(Eid, AttrId, Value)>,
	aevt: PSet<(AttrId, Eid, Value)>,
	avet: PSet<(AttrId, Value, Eid)>,
}

/// An entity in a transaction: an existing one, or a new one, which the transaction allocates
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entity
{
	Id(Eid),
	Temp(u32),
}

/// A value in a transaction: either a value, or a reference to a new entity of the same transaction
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TxValue
{
	Val(Value),
	Temp(u32),
}

impl From<Entity> for TxValue
{
	fn from(e: Entity) -> Self
	{
		match e
		{
			Entity::Id(e) => TxValue::Val(Value::Ref(e)),
			Entity::Temp(t) => TxValue::Temp(t),
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

/// A transaction: attribute definitions (applied first) and assertions/retractions (applied in order)
#[derive(Default)]
pub struct Tx
{
	temps: u32,
	defs: Vec<Attr>,
	ops: Vec<(bool, Entity, Arc<str>, TxValue)>,
}

impl Tx
{
	pub fn new() -> Self
	{
		Self::default()
	}

	/// A new entity, allocated when the transaction is applied
	pub fn temp(&mut self) -> Entity
	{
		self.temps += 1;
		Entity::Temp(self.temps - 1)
	}

	/// Define an attribute; redefining an existing attribute identically is a no-op
	pub fn define(&mut self, name: &str, value_type: ValueType, cardinality: Cardinality) -> &mut Self
	{
		self.defs.push(Attr { name: name.into(), value_type, cardinality });
		self
	}

	pub fn add(&mut self, e: Entity, attr: &str, v: impl Into<TxValue>) -> &mut Self
	{
		self.ops.push((true, e, attr.into(), v.into()));
		self
	}

	/// Retracting an absent datom is a no-op
	pub fn retract(&mut self, e: Entity, attr: &str, v: impl Into<TxValue>) -> &mut Self
	{
		self.ops.push((false, e, attr.into(), v.into()));
		self
	}
}

pub struct TxReport
{
	pub db_before: Db,
	pub db_after: Db,
	/// The entity allocated for each `Tx::temp`, in order
	pub tempids: Vec<Eid>,
	pub tx_data: Vec<Datom>,
}

impl TxReport
{
	pub fn eid(&self, e: Entity) -> Eid
	{
		match e
		{
			Entity::Id(e) => e,
			Entity::Temp(t) => self.tempids[t as usize],
		}
	}
}

impl Db
{
	pub fn new() -> Self
	{
		Db { next_eid: 1, ..Self::default() }
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

	pub fn stats(&self, a: AttrId) -> AttrStats
	{
		self.stats.get(&a).copied().unwrap_or_default()
	}

	/// The values of attribute `a` of entity `e`
	pub fn values(&self, e: Eid, a: AttrId) -> impl Iterator<Item = &Value>
	{
		self.eavt.seek(move |k| (k.0, k.1) < (e, a)).take_while(move |(k, _)| k.0 == e && k.1 == a).map(|(k, _)| &k.2)
	}

	/// The entities whose attribute `a` has the value `v`
	pub fn entities<'a>(&'a self, a: AttrId, v: &'a Value) -> impl Iterator<Item = Eid> + 'a
	{
		self.avet.seek(move |k| k.0 < a || (k.0 == a && k.1 < *v)).take_while(move |(k, _)| k.0 == a && k.1 == *v).map(|(k, _)| k.2)
	}

	/// All the (entity, value) pairs of attribute `a`
	pub fn attr_datoms(&self, a: AttrId) -> impl Iterator<Item = (Eid, &Value)>
	{
		self.aevt.seek(move |k| k.0 < a).take_while(move |(k, _)| k.0 == a).map(|(k, _)| (k.1, &k.2))
	}

	pub fn has(&self, e: Eid, a: AttrId, v: &Value) -> bool
	{
		self.eavt.contains_key(&(e, a, v.clone()))
	}

	pub fn transact(&self, tx: &Tx) -> Result<TxReport, Error>
	{
		let mut db = self.clone();
		db.basis_t += 1;
		for def in &tx.defs
		{
			match db.attr_id(&def.name)
			{
				Some(a) if db.attr(a) == def => {}
				Some(_) => return Err(Error::AttrConflict(def.name.to_string())),
				None =>
				{
					let a = db.attrs.len() as AttrId;
					db.attrs = db.attrs.insert(a, def.clone());
					db.attr_ids = db.attr_ids.insert(def.name.clone(), a);
				}
			}
		}
		let tempids: Vec<Eid> = (0..tx.temps as u64).map(|t| db.next_eid + t).collect();
		db.next_eid += tx.temps as u64;

		let mut tx_data = Vec::new();
		for (added, e, attr, v) in &tx.ops
		{
			let a = db.attr_id(attr).ok_or_else(|| Error::UnknownAttr(attr.to_string()))?;
			let e = match *e
			{
				Entity::Id(e) => db.check_eid(e)?,
				Entity::Temp(t) => tempids[t as usize],
			};
			let v = match v
			{
				TxValue::Val(v) => v.clone(),
				TxValue::Temp(t) => Value::Ref(tempids[*t as usize]),
			};
			let def = db.attr(a).clone();
			if v.value_type() != def.value_type
			{
				return Err(Error::TypeMismatch { attr: attr.to_string(), expected: def.value_type, got: v.value_type() });
			}
			if let Value::Ref(r) = v
			{
				db.check_eid(r)?;
			}
			if !*added
			{
				if db.remove(e, a, &v)
				{
					tx_data.push(Datom { e, a, v, tx: db.basis_t, added: false });
				}
				continue;
			}
			if db.has(e, a, &v)
			{
				continue;
			}
			if def.cardinality == Cardinality::One
			{
				let old: Vec<Value> = db.values(e, a).cloned().collect();
				for o in old
				{
					db.remove(e, a, &o);
					tx_data.push(Datom { e, a, v: o, tx: db.basis_t, added: false });
				}
			}
			db.insert(e, a, &v);
			tx_data.push(Datom { e, a, v, tx: db.basis_t, added: true });
		}
		Ok(TxReport { db_before: self.clone(), db_after: db, tempids, tx_data })
	}

	fn check_eid(&self, e: Eid) -> Result<Eid, Error>
	{
		if e == 0 || e >= self.next_eid { Err(Error::UnknownEntity(e)) } else { Ok(e) }
	}

	fn insert(&mut self, e: Eid, a: AttrId, v: &Value)
	{
		let mut s = self.stats(a);
		s.datoms += 1;
		s.entities += self.values(e, a).next().is_none() as u64;
		s.values += self.entities(a, v).next().is_none() as u64;
		self.stats = self.stats.insert(a, s);
		self.eavt = self.eavt.insert((e, a, v.clone()), ());
		self.aevt = self.aevt.insert((a, e, v.clone()), ());
		self.avet = self.avet.insert((a, v.clone(), e), ());
	}

	// Returns whether the datom was present
	fn remove(&mut self, e: Eid, a: AttrId, v: &Value) -> bool
	{
		if !self.has(e, a, v)
		{
			return false;
		}
		self.eavt = self.eavt.remove(&(e, a, v.clone()));
		self.aevt = self.aevt.remove(&(a, e, v.clone()));
		self.avet = self.avet.remove(&(a, v.clone(), e));
		let mut s = self.stats(a);
		s.datoms -= 1;
		s.entities -= self.values(e, a).next().is_none() as u64;
		s.values -= self.entities(a, v).next().is_none() as u64;
		self.stats = self.stats.insert(a, s);
		true
	}
}

/// A connection: the current database value and every past one
pub struct Conn
{
	history: Vec<Db>,
}

impl Default for Conn
{
	fn default() -> Self
	{
		Conn { history: vec![Db::new()] }
	}
}

impl Conn
{
	pub fn new() -> Self
	{
		Self::default()
	}

	pub fn db(&self) -> Db
	{
		self.history.last().expect("the history starts with the empty database").clone()
	}

	pub fn transact(&mut self, tx: &Tx) -> Result<TxReport, Error>
	{
		let report = self.db().transact(tx)?;
		self.history.push(report.db_after.clone());
		Ok(report)
	}

	/// The database as of basis `t` (None if `t` is in the future)
	pub fn as_of(&self, t: T) -> Option<Db>
	{
		self.history.get(t as usize).cloned()
	}
}

#[cfg(test)]
mod tests
{
	use super::*;

	fn people() -> (Conn, TxReport)
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("person/name", ValueType::Str, Cardinality::One)
			.define("person/age", ValueType::Int, Cardinality::One)
			.define("person/friend", ValueType::Ref, Cardinality::Many);
		let (a, b) = (tx.temp(), tx.temp());
		tx.add(a, "person/name", "Alice").add(a, "person/age", 30).add(b, "person/name", "Bob").add(a, "person/friend", b);
		let r = conn.transact(&tx).unwrap();
		(conn, r)
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
		assert_eq!(db.attr_datoms(name).count(), 2);
		assert_eq!(db.stats(name), AttrStats { datoms: 2, entities: 2, values: 2 });
		assert_eq!(r.tx_data.len(), 4);
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
		assert_eq!(r2.tx_data.iter().map(|d| (d.v.clone(), d.added)).collect::<Vec<_>>(), vec![(Value::Int(30), false), (Value::Int(31), true)]);
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
		assert_eq!(r2.tx_data.len(), 1);
		let name = conn.db().attr_id("person/name").unwrap();
		assert_eq!(conn.db().stats(name), AttrStats { datoms: 1, entities: 1, values: 1 });
	}

	#[test]
	fn validation()
	{
		let (mut conn, _) = people();
		let mut tx = Tx::new();
		tx.add(Entity::Id(99), "person/name", "X");
		assert_eq!(conn.transact(&tx).err(), Some(Error::UnknownEntity(99)));
		let mut tx = Tx::new();
		let e = tx.temp();
		tx.add(e, "person/age", "old");
		assert!(matches!(conn.transact(&tx), Err(Error::TypeMismatch { .. })));
		let mut tx = Tx::new();
		tx.define("person/age", ValueType::Str, Cardinality::One);
		assert!(matches!(conn.transact(&tx), Err(Error::AttrConflict(_))));
		let mut tx = Tx::new();
		let e = tx.temp();
		tx.add(e, "nope", 1);
		assert!(matches!(conn.transact(&tx), Err(Error::UnknownAttr(_))));
		// Failed transactions leave no trace
		assert_eq!(conn.db().basis_t(), 1);
	}
}
