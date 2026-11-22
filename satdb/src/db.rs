//! The database value: datoms in four persistent indexes, the schema and the per-attribute statistics the planner estimates with.
//!
//! A `Db` is immutable. `transact` returns a new `Db` that shares all the unchanged index nodes with the old one,
//! so keeping every past version (see `Conn::as_of`) costs only the changed paths.
//!
//! # Transactions
//! A transaction is a *set* of assertions and retractions, all of them against the database before it (as in Datomic):
//! their order doesn't matter, and operations that contradict each other are an error instead of "the last one wins".
//! `transact` runs in phases:
//! 1. The attribute definitions are applied.
//! 2. Lookup refs (`[attr value]` of a unique attribute) resolve against the database before the transaction;
//!    one that doesn't resolve is an error, even if the transaction itself asserts the value.
//! 3. Tempids resolve: a tempid asserting a `Unique::Identity` value that some entity already has *upserts* to that entity,
//!    and tempids asserting the same new identity value unify into one new entity (to a fixed point, as identity values may be refs to tempids).
//!    The tempids that remain are allocated, after the transaction's own entity.
//! 4. `retract_entity` expands into the entity's datoms, the datoms referring to it (through the VAET index) and, recursively,
//!    the entities it owns through component attributes.
//! 5. The conflicts are checked: a datom both asserted and retracted, two values of a cardinality-one attribute of an entity,
//!    and any operation on a `db/` attribute (which only the library writes).
//! 6. The changes are applied (a cardinality-one assertion retracts the entity's previous value), the transaction entity gets its `db/txInstant`,
//!    and the unique attributes are checked on the result: a value may belong to one entity at most.

use crate::pmap::PMap;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

pub type Eid = u64;
pub type AttrId = u32;
/// The basis of a database value: the number of transactions applied to it
pub type T = u64;

/// The built-in attribute holding the instant of every transaction entity (an Int, in the caller's units)
pub const TX_INSTANT: &str = "db/txInstant";

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
			Error::Unbound(v) => write!(f, "variable {v} is bound by no pattern"),
			Error::Planner(m) => write!(f, "planner error: {m}"),
		}
	}
}

impl std::error::Error for Error {}

type PSet<K> = PMap<K, ()>;

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
	eavt: PSet<(Eid, AttrId, Value)>,
	aevt: PSet<(AttrId, Eid, Value)>,
	avet: PSet<(AttrId, Value, Eid)>,
	/// The reverse index of the Ref datoms: (referenced entity, attribute, referring entity)
	vaet: PSet<(Eid, AttrId, Eid)>,
}

impl Default for Db
{
	fn default() -> Self
	{
		Self::new()
	}
}

/// An entity in a transaction: an existing one, a new one (which the transaction allocates or upserts),
/// a lookup ref (`Tx::lookup`) or the transaction's own entity
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entity
{
	Id(Eid),
	Temp(u32),
	Lookup(u32),
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
}

/// A transaction: attribute definitions (applied first), then a set of assertions and retractions (see the module documentation)
#[derive(Default)]
pub struct Tx
{
	temps: u32,
	lookups: Vec<(Arc<str>, Value)>,
	defs: Vec<Attr>,
	ops: Vec<Op>,
	instant: Option<i64>,
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
		Entity::Temp(self.temps - 1)
	}

	/// The lookup ref [attr v]: the entity whose unique attribute `attr` has the value `v` before the transaction
	pub fn lookup(&mut self, attr: &str, v: impl Into<Value>) -> Entity
	{
		self.lookups.push((attr.into(), v.into()));
		Entity::Lookup(self.lookups.len() as u32 - 1)
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
}

impl TxReport
{
	/// The entity that `e` of this report's transaction resolved to (panics for an entity of another transaction)
	pub fn eid(&self, e: Entity) -> Eid
	{
		match e
		{
			Entity::Id(e) => e,
			Entity::Temp(t) => self.tempids[t as usize],
			Entity::Lookup(l) => self.lookups[l as usize],
			Entity::Tx => self.tx,
		}
	}
}

/// An entity of a transaction before its tempids are allocated
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Res
{
	Eid(Eid),
	Temp(u32),
}

/// A value of a transaction before its tempids are allocated
#[derive(Clone)]
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
		};
		db.add_attr(Attr::new(TX_INSTANT, ValueType::Int, Cardinality::One));
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

	pub fn stats(&self, a: AttrId) -> AttrStats
	{
		self.stats.get(&a).copied().unwrap_or_default()
	}

	/// The values of attribute `a` of entity `e`
	pub fn values(&self, e: Eid, a: AttrId) -> impl Iterator<Item = &Value>
	{
		self.eavt.seek(move |k| (k.0, k.1) < (e, a)).take_while(move |(k, _)| k.0 == e && k.1 == a).map(|(k, _)| &k.2)
	}

	/// All the (attribute, value) pairs of entity `e`
	pub fn entity_datoms(&self, e: Eid) -> impl Iterator<Item = (AttrId, &Value)>
	{
		self.eavt.seek(move |k| k.0 < e).take_while(move |(k, _)| k.0 == e).map(|(k, _)| (k.1, &k.2))
	}

	/// The entities whose attribute `a` has the value `v`
	pub fn entities<'a>(&'a self, a: AttrId, v: &'a Value) -> impl Iterator<Item = Eid> + 'a
	{
		self.avet.seek(move |k| k.0 < a || (k.0 == a && k.1 < *v)).take_while(move |(k, _)| k.0 == a && k.1 == *v).map(|(k, _)| k.2)
	}

	/// The (attribute, entity) pairs of the Ref datoms whose value is `target` (the VAET index)
	pub fn referrers(&self, target: Eid) -> impl Iterator<Item = (AttrId, Eid)> + '_
	{
		self.vaet.seek(move |k| k.0 < target).take_while(move |(k, _)| k.0 == target).map(|(k, _)| (k.1, k.2))
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
		let mut db = self.clone();
		db.basis_t += 1;
		for def in &tx.defs
		{
			db.define(def)?;
		}
		let instant = tx.instant.unwrap_or(self.instant);
		if instant < self.instant
		{
			return Err(Error::InstantBackwards { previous: self.instant, got: instant });
		}
		db.instant = instant;
		let tx_eid = db.next_eid;
		db.next_eid += 1;

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
				Entity::Temp(t) if t < tx.temps => Ok(Res::Temp(t)),
				Entity::Lookup(l) if (l as usize) < lookups.len() => Ok(Res::Eid(lookups[l as usize])),
				Entity::Tx => Ok(Res::Eid(tx_eid)),
				_ => Err(Error::ForeignEntity(e)),
			}
		};
		let mut ops = Vec::new();
		let mut doomed = Vec::new();
		for op in &tx.ops
		{
			let (added, e, attr, v) = match op
			{
				Op::Datom(added, e, attr, v) => (*added, e, attr, v),
				Op::RetractEntity(e) =>
				{
					doomed.push(ent(*e)?);
					continue;
				}
			};
			let a = db.attr_id(attr).ok_or_else(|| Error::UnknownAttr(attr.to_string()))?;
			let def = db.attr(a);
			let mismatch = |got: ValueType| Error::TypeMismatch { attr: attr.to_string(), expected: def.value_type, got };
			let v = match v
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
			};
			ops.push(ResOp { added, e: ent(*e)?, a, v });
		}

		// Phase 3: upserts and tempid unification, to a fixed point (every round only merges classes or upserts one)
		let mut temps = Temps { parent: (0..tx.temps).collect(), upserted: vec![None; tx.temps as usize] };
		loop
		{
			let mut changed = false;
			let mut holders: HashMap<(AttrId, Value), Res> = HashMap::new();
			for op in ops.iter().filter(|op| op.added && db.attr(op.a).unique == Some(Unique::Identity))
			{
				let v = match &op.v
				{
					ResVal::Val(v) => v.clone(),
					ResVal::Temp(t) => match temps.res(*t)
					{
						Res::Eid(x) => Value::Ref(x),
						// An identity value that is a new entity can't match an existing one; it may next round
						Res::Temp(_) => continue,
					},
				};
				let me = match op.e
				{
					Res::Temp(t) => temps.res(t),
					e => e,
				};
				let other = self.entities(op.a, &v).next().map(Res::Eid).or_else(|| holders.get(&(op.a, v.clone())).copied());
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

		// Phase 6: apply, then check the unique attributes
		let mut retracted: BTreeSet<(Eid, AttrId, Value)> = rets.into_iter().filter(|(e, a, v)| self.has(*e, *a, v)).collect();
		for (e, a, v) in &adds
		{
			if db.attr(*a).cardinality == Cardinality::One
			{
				retracted.extend(self.values(*e, *a).filter(|o| *o != v).map(|o| (*e, *a, o.clone())));
			}
		}
		let asserted: Vec<(Eid, AttrId, Value)> = adds.into_iter().filter(|(e, a, v)| !self.has(*e, *a, v)).collect();
		for (e, a, v) in &retracted
		{
			db.remove(*e, *a, v);
		}
		for (e, a, v) in &asserted
		{
			db.insert(*e, *a, v);
		}
		for (_, a, v) in &asserted
		{
			if db.attr(*a).unique.is_some() && db.entities(*a, v).nth(1).is_some()
			{
				return Err(Error::UniqueConflict { attr: db.attr_name(*a), value: v.clone() });
			}
		}
		let instant_attr = db.attr_id(TX_INSTANT).expect("built in");
		db.insert(tx_eid, instant_attr, &Value::Int(instant));
		let datom = |(e, a, v): (Eid, AttrId, Value), added: bool| Datom { e, a, v, tx: tx_eid, added };
		let mut tx_data = vec![datom((tx_eid, instant_attr, Value::Int(instant)), true)];
		tx_data.extend(retracted.into_iter().map(|d| datom(d, false)));
		tx_data.extend(asserted.into_iter().map(|d| datom(d, true)));
		Ok(TxReport { db_before: self.clone(), db_after: db, tx: tx_eid, tempids, lookups, tx_data })
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

	fn add_attr(&mut self, def: Attr)
	{
		let a = self.attrs.len() as AttrId;
		self.attr_ids = self.attr_ids.insert(def.name.clone(), a);
		self.attrs = self.attrs.insert(a, def);
	}

	fn attr_name(&self, a: AttrId) -> String
	{
		self.attr(a).name.to_string()
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
		if let Value::Ref(r) = v
		{
			self.vaet = self.vaet.insert((*r, a, e), ());
		}
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
		if let Value::Ref(r) = v
		{
			self.vaet = self.vaet.remove(&(*r, a, e));
		}
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
		assert_eq!(db.values(r1.tx, instant).collect::<Vec<_>>(), vec![&Value::Int(100)]);
		assert_eq!(db.values(r1.tx, author).collect::<Vec<_>>(), vec![&Value::from("ann")]);
		assert_eq!(r1.eid(Entity::Tx), r1.tx);
		// The default instant is the previous one; an earlier one is refused
		let mut tx = Tx::new();
		tx.add(Entity::Id(r1.tempids[0]), "doc/title", "b");
		let r2 = conn.transact(&tx).unwrap();
		assert_eq!(conn.db().values(r2.tx, instant).collect::<Vec<_>>(), vec![&Value::Int(100)]);
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
		q.pred(i.clone(), crate::Cmp::Ge, 100);
		let got = q.run(&[t, i]).unwrap();
		assert_eq!(got, vec![vec![Value::Ref(r1.tx), Value::Int(100)], vec![Value::Ref(r2.tx), Value::Int(100)], vec![Value::Ref(r3.tx), Value::Int(250)]]);
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
}
