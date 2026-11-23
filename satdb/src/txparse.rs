//! A text syntax for transactions: Datomic's transaction data, written in EDN, read into the `Tx` builder.
//!
//! ```text
//! [{:db/ident :person/name    :db/valueType :db.type/string  :db/cardinality :db.cardinality/one  :db/unique :db.unique/identity}
//!  {:db/ident :person/age     :db/valueType :db.type/long    :db/cardinality :db.cardinality/one}
//!  {:db/ident :person/friend  :db/valueType :db.type/ref     :db/cardinality :db.cardinality/many}
//!
//!  {:db/id "alice"  :person/name "Alice"  :person/friend ["bob" {:person/name "Carol"}]}
//!  [:db/add "bob" :person/name "Bob"]
//!  [:db/retract [:person/name "Dave"] :person/friend 17]
//!  [:db/retractEntity [:person/name "Erin"]]
//!  [:db/cas 17 :person/age nil 30]
//!  [:db/add "datomic.tx" :db/txInstant #inst "2024-02-29"]]
//! ```
//! * A transaction is a vector of attribute maps, entity maps and operations. As in the builder it is a set, so their order doesn't
//!   matter; the attribute maps are read first, and the rest by the types they give.
//! * An attribute map is a map with `:db/ident`: `:db/valueType` (`:db.type/boolean`, `long`, `double`, `string`, `instant` or `ref`),
//!   `:db/cardinality` (`:db.cardinality/one` or `many`), `:db/unique` (`:db.unique/identity` or `value`) and `:db/isComponent`.
//!   A new attribute needs its type and cardinality (`Tx::define_attr`). For an existing attribute the map gives what changes
//!   (`Tx::alter_attr`; `:db/unique nil` makes it not unique), and an unchanged one is a no-op. `:db/doc` and `:db/index` are accepted
//!   and ignored (there is nowhere to keep a docstring, and every attribute is indexed).
//! * An entity is an id, a tempid (a string: the same string is the same new entity throughout the transaction, and `"datomic.tx"`
//!   is the transaction's own entity) or a lookup ref `[:attr value]`.
//! * An entity map asserts each of its attributes of the entity `:db/id` (without it, a new entity). A vector is the values of a
//!   cardinality-many attribute, each asserted. The value of a Ref attribute is an entity, or an entity map of its own, nested.
//! * The operations: `[:db/add e :attr v]`, `[:db/retract e :attr v]`, `[:db/retractEntity e]` and `[:db/cas e :attr old new]`
//!   (`old` nil: no value), with Datomic's older names `:db.fn/retractEntity` and `:db.fn/cas` too.
//! * `:db/txInstant` of `"datomic.tx"` (an `#inst`) is the transaction's instant (`Tx::with_instant`).
//! * Constants are those of the queries; an integer is an entity id where the attribute is a Ref.
//!
//! Syntax errors are `Error::Parse`, with the line and column; an unknown attribute is `Error::UnknownAttr`, and the errors of the
//! transaction itself (a type mismatch, a lookup ref without entity, ...) are those of `transact`.

use crate::db::{Attr, Cardinality, Conn, Db, Eid, Entity, Error, Tx, TxReport, TxValue, Unique, Value, ValueType, TX_INSTANT};
use crate::parse::{error_at, Form, Node, Reader};
use std::collections::{BTreeMap, HashMap};

/// The tempid of the transaction's own entity
const TX_TEMPID: &str = "datomic.tx";

/// A parsed text transaction
pub struct TextTx
{
	pub tx: Tx,
	/// The entity of each tempid string (not of `"datomic.tx"`, nor of the maps without `:db/id`)
	pub tempids: BTreeMap<String, Entity>,
}

/// The report of a text transaction
pub struct TextTxReport
{
	pub report: TxReport,
	/// The entity that each tempid string resolved to
	pub tempids: BTreeMap<String, Eid>,
}

impl TextTx
{
	fn report(self, report: TxReport) -> TextTxReport
	{
		let tempids = self.tempids.into_iter().map(|(name, e)| (name, report.eid(e))).collect();
		TextTxReport { report, tempids }
	}
}

impl Db
{
	/// Parse a text transaction (see the module documentation), against this value's schema
	pub fn parse_tx(&self, text: &str) -> Result<TextTx, Error>
	{
		let node = Reader::new(text, "transaction").read_all()?;
		let mut parser = TxParser { text, db: self, defs: HashMap::new(), tx: Tx::new(), tempids: BTreeMap::new() };
		parser.transaction(&node)?;
		Ok(TextTx { tx: parser.tx, tempids: parser.tempids })
	}

	/// Parse and apply a text transaction (see `Db::transact`)
	pub fn transact_str(&self, text: &str) -> Result<TextTxReport, Error>
	{
		let parsed = self.parse_tx(text)?;
		let report = self.transact(&parsed.tx)?;
		Ok(parsed.report(report))
	}
}

impl Conn
{
	/// Parse and apply a text transaction (see `Conn::transact`)
	pub fn transact_str(&mut self, text: &str) -> Result<TextTxReport, Error>
	{
		let parsed = self.db().parse_tx(text)?;
		let report = self.transact(&parsed.tx)?;
		Ok(parsed.report(report))
	}
}

fn is_keyword(node: &Node, name: &str) -> bool
{
	matches!(&node.form, Form::Keyword(k) if k == name)
}

/// The reader of a transaction's forms into the builder
struct TxParser<'a>
{
	text: &'a str,
	db: &'a Db,
	/// The attributes that the transaction defines or alters, by name: its data is read by them
	defs: HashMap<String, Attr>,
	tx: Tx,
	tempids: BTreeMap<String, Entity>,
}

impl TxParser<'_>
{
	fn err(&self, node: &Node, msg: impl Into<String>) -> Error
	{
		error_at(self.text, node.pos, msg)
	}

	fn transaction(&mut self, node: &Node) -> Result<(), Error>
	{
		let Form::Vector(items) = &node.form
		else
		{
			return Err(self.err(node, "a transaction is a vector of maps and operations [:db/add e :attr v]"));
		};
		// The attribute maps first: the data is read by the attributes' types
		let is_attr_map = |n: &&Node| matches!(&n.form, Form::Map(kvs) if kvs.iter().step_by(2).any(|k| is_keyword(k, "db/ident")));
		for item in items.iter().filter(is_attr_map)
		{
			self.attr_map(item)?;
		}
		for item in items.iter().filter(|n| !is_attr_map(n))
		{
			match &item.form
			{
				Form::Map(_) =>
				{
					self.entity_map(item)?;
				}
				Form::Vector(op) => self.operation(item, op)?,
				_ => return Err(self.err(item, "expected a map {...} or an operation [:db/add e :attr v]")),
			}
		}
		Ok(())
	}

	// The option that a keyword names
	fn choice<T: Copy>(&self, node: &Node, options: &[(&str, T)]) -> Result<T, Error>
	{
		let found = options.iter().find(|(name, _)| is_keyword(node, name));
		found.map(|(_, x)| *x).ok_or_else(|| self.err(node, format!("expected {}", options.iter().map(|(name, _)| format!(":{name}")).collect::<Vec<_>>().join(" or "))))
	}

	// An attribute map: a definition, or the alteration of an existing attribute (what the map doesn't give stays as it is)
	fn attr_map(&mut self, node: &Node) -> Result<(), Error>
	{
		let Form::Map(kvs) = &node.form
		else
		{
			unreachable!("the caller matched a map");
		};
		let (mut name, mut value_type, mut cardinality, mut unique, mut component) = (None, None, None, None, None);
		for pair in kvs.chunks(2)
		{
			let (k, v) = (&pair[0], &pair[1]);
			let Form::Keyword(key) = &k.form
			else
			{
				return Err(self.err(k, "the keys of a map are keywords"));
			};
			match (key.as_str(), &v.form)
			{
				("db/ident", Form::Keyword(n)) => name = Some(n.clone()),
				("db/ident", _) => return Err(self.err(v, ":db/ident takes the attribute's keyword")),
				("db/valueType", _) =>
				{
					let types = [("db.type/boolean", ValueType::Bool), ("db.type/long", ValueType::Int), ("db.type/double", ValueType::Float)];
					let types = [types.as_slice(), &[("db.type/string", ValueType::Str), ("db.type/instant", ValueType::Instant), ("db.type/ref", ValueType::Ref)]].concat();
					value_type = Some(self.choice(v, &types)?)
				}
				("db/cardinality", _) => cardinality = Some(self.choice(v, &[("db.cardinality/one", Cardinality::One), ("db.cardinality/many", Cardinality::Many)])?),
				("db/unique", Form::Nil) => unique = Some(None),
				("db/unique", _) => unique = Some(Some(self.choice(v, &[("db.unique/identity", Unique::Identity), ("db.unique/value", Unique::Value)])?)),
				("db/isComponent", Form::Bool(b)) => component = Some(*b),
				("db/isComponent", _) => return Err(self.err(v, ":db/isComponent takes true or false")),
				("db/doc" | "db/index", _) => {}
				_ => return Err(self.err(k, format!("unknown key :{key} of an attribute's map (the keys are :db/ident, :db/valueType, :db/cardinality, :db/unique and :db/isComponent)"))),
			}
		}
		let name = name.expect("an attribute map has :db/ident");
		let old = self.db.attr_id(&name).map(|a| self.db.attr(a).clone());
		let missing = |key: &str| self.err(node, format!("the new attribute :{name} needs {key}"));
		let attr = Attr {
			name: name.as_str().into(),
			value_type: value_type.or(old.as_ref().map(|o| o.value_type)).ok_or_else(|| missing(":db/valueType"))?,
			cardinality: cardinality.or(old.as_ref().map(|o| o.cardinality)).ok_or_else(|| missing(":db/cardinality"))?,
			unique: unique.unwrap_or(old.as_ref().and_then(|o| o.unique)),
			component: component.unwrap_or(old.as_ref().is_some_and(|o| o.component)),
		};
		match old
		{
			Some(old) if old != attr => self.tx.alter_attr(attr.clone()),
			_ => self.tx.define_attr(attr.clone()),
		};
		self.defs.insert(name, attr);
		Ok(())
	}

	// The attribute that a keyword names: one of the transaction's, or of the database
	fn attr(&self, node: &Node) -> Result<Attr, Error>
	{
		let Form::Keyword(name) = &node.form
		else
		{
			return Err(self.err(node, "expected an attribute keyword"));
		};
		match self.defs.get(name)
		{
			Some(attr) => Ok(attr.clone()),
			None => self.db.attr_id(name).map(|a| self.db.attr(a).clone()).ok_or_else(|| Error::UnknownAttr(name.clone())),
		}
	}

	// An entity: an id, a tempid string or a lookup ref [:attr value]
	fn entity(&mut self, node: &Node) -> Result<Entity, Error>
	{
		match &node.form
		{
			Form::Int(i) => Eid::try_from(*i).map(Entity::Id).map_err(|_| self.err(node, "an entity id is not negative")),
			Form::Str(name) if name == TX_TEMPID => Ok(Entity::Tx),
			Form::Str(name) => Ok(match self.tempids.get(name)
			{
				Some(e) => *e,
				None =>
				{
					let e = self.tx.temp();
					self.tempids.insert(name.clone(), e);
					e
				}
			}),
			Form::Vector(items) => match items.as_slice()
			{
				[a, v] =>
				{
					let attr = self.attr(a)?;
					let v = self.constant(v, attr.value_type)?;
					Ok(self.tx.lookup(&attr.name, v))
				}
				_ => Err(self.err(node, "a lookup ref is [:attr value]")),
			},
			_ => Err(self.err(node, "expected an entity: an id, a tempid string or a lookup ref [:attr value]")),
		}
	}

	// A constant, read for an attribute of type `ty`: an integer is an entity id for a Ref
	fn constant(&self, node: &Node, ty: ValueType) -> Result<Value, Error>
	{
		Ok(match &node.form
		{
			Form::Int(i) if ty == ValueType::Ref => Value::Ref(Eid::try_from(*i).map_err(|_| self.err(node, "an entity id is not negative"))?),
			Form::Int(i) => Value::Int(*i),
			Form::Str(s) => Value::from(s.as_str()),
			Form::Float(f) => Value::Float(*f),
			Form::Bool(b) => Value::Bool(*b),
			Form::Instant(i) => Value::Instant(*i),
			Form::Keyword(k) => return Err(self.err(node, format!("the keyword :{k} is not a value (keywords only name attributes)"))),
			_ => return Err(self.err(node, "expected a constant")),
		})
	}

	// A value of an attribute: an entity (or a nested entity map) for a Ref, else a constant
	fn value(&mut self, node: &Node, attr: &Attr) -> Result<TxValue, Error>
	{
		match &node.form
		{
			Form::Int(_) | Form::Str(_) | Form::Vector(_) if attr.value_type == ValueType::Ref => Ok(TxValue::Entity(self.entity(node)?)),
			Form::Map(_) if attr.value_type == ValueType::Ref => Ok(TxValue::Entity(self.entity_map(node)?)),
			Form::Map(_) => Err(self.err(node, format!("only a Ref attribute takes an entity map, and :{} is a {:?}", attr.name, attr.value_type))),
			_ => Ok(TxValue::Val(self.constant(node, attr.value_type)?)),
		}
	}

	// Assert a value of an attribute of an entity; the db/txInstant of the transaction's own entity is its instant
	fn add(&mut self, e: Entity, attr: &Attr, v: &Node) -> Result<(), Error>
	{
		if e == Entity::Tx && &*attr.name == TX_INSTANT
		{
			let Form::Instant(i) = &v.form
			else
			{
				return Err(self.err(v, ":db/txInstant takes an instant (#inst \"...\")"));
			};
			self.tx.with_instant(*i);
		}
		else
		{
			let v = self.value(v, attr)?;
			self.tx.add(e, &attr.name, v);
		}
		Ok(())
	}

	// An entity map: an assertion per attribute (per element of a vector, for a cardinality-many attribute) of its :db/id,
	// or of a new entity; returns the entity
	fn entity_map(&mut self, node: &Node) -> Result<Entity, Error>
	{
		let Form::Map(kvs) = &node.form
		else
		{
			unreachable!("the caller matched a map");
		};
		let e = match kvs.chunks(2).find(|pair| is_keyword(&pair[0], "db/id"))
		{
			Some(pair) => self.entity(&pair[1])?,
			None if kvs.is_empty() => return Err(self.err(node, "an entity map needs an attribute")),
			None => self.tx.temp(),
		};
		for pair in kvs.chunks(2).filter(|pair| !is_keyword(&pair[0], "db/id"))
		{
			let (attr, v) = (self.attr(&pair[0])?, &pair[1]);
			match &v.form
			{
				Form::Vector(values) if attr.cardinality == Cardinality::Many =>
				{
					for v in values
					{
						self.add(e, &attr, v)?;
					}
				}
				_ => self.add(e, &attr, v)?,
			}
		}
		Ok(e)
	}

	fn operation(&mut self, node: &Node, items: &[Node]) -> Result<(), Error>
	{
		let Some((Node { form: Form::Keyword(op), .. }, args)) = items.split_first()
		else
		{
			return Err(self.err(node, "an operation starts with its keyword: :db/add, :db/retract, :db/retractEntity or :db/cas"));
		};
		match (op.as_str(), args)
		{
			("db/add", [e, a, v]) =>
			{
				let (e, attr) = (self.entity(e)?, self.attr(a)?);
				self.add(e, &attr, v)?;
			}
			("db/retract", [e, a, v]) =>
			{
				let (e, attr) = (self.entity(e)?, self.attr(a)?);
				let v = self.value(v, &attr)?;
				self.tx.retract(e, &attr.name, v);
			}
			("db/retractEntity" | "db.fn/retractEntity", [e]) =>
			{
				let e = self.entity(e)?;
				self.tx.retract_entity(e);
			}
			("db/cas" | "db.fn/cas", [e, a, old, new]) =>
			{
				let (e, attr) = (self.entity(e)?, self.attr(a)?);
				let old = match &old.form
				{
					Form::Nil => None,
					_ => Some(self.value(old, &attr)?),
				};
				let new = self.value(new, &attr)?;
				self.tx.cas(e, &attr.name, old, new);
			}
			("db/add" | "db/retract", _) => return Err(self.err(node, format!(":{op} takes an entity, an attribute and a value"))),
			("db/retractEntity" | "db.fn/retractEntity", _) => return Err(self.err(node, format!(":{op} takes an entity"))),
			("db/cas" | "db.fn/cas", _) => return Err(self.err(node, format!(":{op} takes an entity, an attribute, the expected value (or nil) and the new value"))),
			_ => return Err(self.err(&items[0], format!("unknown operation :{op} (the operations are :db/add, :db/retract, :db/retractEntity and :db/cas)"))),
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::db::tests::assert_same_db;
	use crate::parse::parse_instant;

	const SCHEMA: &str = r#"
		{:db/ident :person/name     :db/valueType :db.type/string   :db/cardinality :db.cardinality/one  :db/unique :db.unique/identity  :db/doc "The name"}
		{:db/ident :person/age      :db/valueType :db.type/long     :db/cardinality :db.cardinality/one}
		{:db/ident :person/score    :db/valueType :db.type/double   :db/cardinality :db.cardinality/one}
		{:db/ident :person/vip      :db/valueType :db.type/boolean  :db/cardinality :db.cardinality/one  :db/index true}
		{:db/ident :person/born     :db/valueType :db.type/instant  :db/cardinality :db.cardinality/one}
		{:db/ident :person/aka      :db/valueType :db.type/string   :db/cardinality :db.cardinality/many}
		{:db/ident :person/friend   :db/valueType :db.type/ref      :db/cardinality :db.cardinality/many}
		{:db/ident :person/boss     :db/valueType :db.type/ref      :db/cardinality :db.cardinality/one}
		{:db/ident :person/address  :db/valueType :db.type/ref      :db/cardinality :db.cardinality/one  :db/isComponent true}
		{:db/ident :address/city    :db/valueType :db.type/string   :db/cardinality :db.cardinality/one}
		{:db/ident :tx/note         :db/valueType :db.type/string   :db/cardinality :db.cardinality/one}"#;

	fn schema(tx: &mut Tx)
	{
		use {Cardinality::*, ValueType::*};
		tx.define_attr(Attr::new("person/name", Str, One).unique(Unique::Identity))
			.define("person/age", Int, One)
			.define("person/score", Float, One)
			.define("person/vip", Bool, One)
			.define("person/born", Instant, One)
			.define("person/aka", Str, Many)
			.define("person/friend", Ref, Many)
			.define("person/boss", Ref, One)
			.define_attr(Attr::new("person/address", Ref, One).component())
			.define("address/city", Str, One)
			.define("tx/note", Str, One);
	}

	// Each text transaction leaves the same database as its builder transaction (entity ids, indexes and history included), and
	// reports the entities of its tempid strings: every form of entity and value, the operations, and the schema's definition and alteration
	#[test]
	fn text_transactions_match_the_builder()
	{
		let (mut text, mut built) = (Conn::new(), Conn::new());
		let same = |text: &Conn, built: &Conn, what: &str| assert_same_db(&text.db(), &built.db(), what);

		// The schema and data that it types, in one transaction; the data before the attribute maps
		let first = text
			.transact_str(&format!(
				r#"[{{:db/id "alice"  :person/name "Alice"  :person/age 30  :person/score 1.5  :person/vip true  :person/born #inst "1990-01-02"
				      :person/aka ["Al" "Ally"]  :person/friend ["bob" {{:person/name "Carol" :person/age 41}}]  :person/address {{:address/city "Paris"}}}}
				    [:db/add "bob" :person/name "Bob"]
				    [:db/add "bob" :person/boss "alice"]
				    {{:person/name "Dave", :person/boss "bob"}}  ; a new entity
				    [:db/add "datomic.tx" :tx/note "first"]
				    {{:db/id "datomic.tx" :db/txInstant #inst "2024-02-29T13:45:00Z"}}
				    {SCHEMA}]"#
			))
			.unwrap();
		let mut tx = Tx::new();
		schema(&mut tx);
		// The tempids in the order the text first names them
		let (alice, bob, carol, address, dave) = (tx.temp(), tx.temp(), tx.temp(), tx.temp(), tx.temp());
		tx.add(alice, "person/name", "Alice").add(alice, "person/age", 30).add(alice, "person/score", 1.5).add(alice, "person/vip", true);
		tx.add(alice, "person/born", Value::Instant(parse_instant("1990-01-02").unwrap())).add(alice, "person/aka", "Al").add(alice, "person/aka", "Ally");
		tx.add(alice, "person/friend", bob).add(alice, "person/friend", carol).add(carol, "person/name", "Carol").add(carol, "person/age", 41);
		tx.add(alice, "person/address", address).add(address, "address/city", "Paris");
		tx.add(bob, "person/name", "Bob").add(bob, "person/boss", alice).add(dave, "person/name", "Dave").add(dave, "person/boss", bob);
		tx.add(Entity::Tx, "tx/note", "first").with_instant(parse_instant("2024-02-29T13:45:00Z").unwrap());
		let report = built.transact(&tx).unwrap();
		same(&text, &built, "the first transaction");
		assert_eq!(first.tempids, BTreeMap::from([("alice".to_string(), report.eid(alice)), ("bob".to_string(), report.eid(bob))]));
		assert_eq!(first.report.tx_data, report.tx_data);
		let bob = report.eid(bob);

		// Lookup refs and ids as entities and as values, the operations, an upsert, and a later instant
		let second = text
			.transact_str(&format!(
				r#"[[:db/retract [:person/name "Alice"] :person/aka "Al"]
				    [:db/cas {bob} :person/age nil 50]
				    [:db.fn/cas [:person/name "Alice"] :person/age 30 31]
				    [:db.fn/retractEntity [:person/name "Dave"]]
				    {{:db/id [:person/name "Carol"]  :person/friend [[:person/name "Alice"] {bob}]  :person/boss [:person/name "Bob"]}}
				    [:db/retract [:person/name "Alice"] :person/friend [:person/name "Carol"]]
				    {{:person/name "Bob" :person/aka "Bobby"}}
				    [:db/add "erin" :person/name "Erin"]
				    [:db/add "erin" :person/boss {bob}]
				    [:db/add "datomic.tx" :db/txInstant #inst "2024-03-01"]]"#
			))
			.unwrap();
		let mut tx = Tx::new();
		let (alice, carol, dave) = (tx.lookup("person/name", "Alice"), tx.lookup("person/name", "Carol"), tx.lookup("person/name", "Dave"));
		let (upsert, erin) = (tx.temp(), tx.temp());
		tx.retract(alice, "person/aka", "Al").cas(Entity::Id(bob), "person/age", None, 50).cas(alice, "person/age", Some(30.into()), 31);
		tx.retract_entity(dave).add(carol, "person/friend", alice).add(carol, "person/friend", Entity::Id(bob));
		let boss = tx.lookup("person/name", "Bob");
		tx.add(carol, "person/boss", boss).retract(alice, "person/friend", carol);
		tx.add(upsert, "person/name", "Bob").add(upsert, "person/aka", "Bobby").add(erin, "person/name", "Erin").add(erin, "person/boss", Entity::Id(bob));
		tx.with_instant(parse_instant("2024-03-01").unwrap());
		let report = built.transact(&tx).unwrap();
		same(&text, &built, "the second transaction");
		assert_eq!(second.tempids, BTreeMap::from([("erin".to_string(), report.eid(erin))]));
		assert_eq!(report.eid(upsert), bob);

		// Alterations: only what changes is given, the data is read by the altered attribute, and an unchanged attribute is a no-op
		text.transact_str(
			r#"[{:db/id [:person/name "Carol"] :person/boss [[:person/name "Alice"]]}
			    {:db/ident :person/boss :db/cardinality :db.cardinality/many}
			    {:db/ident :address/city :db/unique :db.unique/value}
			    {:db/ident :person/address :db/isComponent false}
			    {:db/ident :person/age :db/valueType :db.type/long :db/cardinality :db.cardinality/one}]"#,
		)
		.unwrap();
		let mut tx = Tx::new();
		let (alice, carol) = (tx.lookup("person/name", "Alice"), tx.lookup("person/name", "Carol"));
		tx.alter_attr(Attr::new("person/boss", ValueType::Ref, Cardinality::Many))
			.alter_attr(Attr::new("address/city", ValueType::Str, Cardinality::One).unique(Unique::Value))
			.alter_attr(Attr::new("person/address", ValueType::Ref, Cardinality::One))
			.define("person/age", ValueType::Int, Cardinality::One)
			.add(carol, "person/boss", alice);
		built.transact(&tx).unwrap();
		same(&text, &built, "the alterations");
		let boss = text.db().attr_id("person/boss").unwrap();
		assert_eq!(text.db().values(text.db().lookup("person/name", &"Carol".into()).unwrap().unwrap(), boss).count(), 2);

		text.transact_str("[{:db/ident :address/city :db/unique nil}]").unwrap();
		built.transact(Tx::new().alter_attr(Attr::new("address/city", ValueType::Str, Cardinality::One))).unwrap();
		same(&text, &built, "no longer unique");

		// An empty transaction, and a database value without connection
		text.transact_str("[]").unwrap();
		built.transact(&Tx::new()).unwrap();
		same(&text, &built, "the empty transaction");
		let (db, mut tx) = (Db::new(), Tx::new());
		schema(&mut tx);
		assert_same_db(&db.transact_str(&format!("[{SCHEMA}]")).unwrap().report.db_after, &db.transact(&tx).unwrap().db_after, "a value's transaction");
	}

	#[test]
	fn errors()
	{
		let mut conn = Conn::new();
		conn.transact_str(&format!("[{SCHEMA} {{:person/name \"Alice\" :person/age 30}}]")).unwrap();
		let db = conn.db();
		let syntax = |text: &str, at: (usize, usize), part: &str| match db.parse_tx(text)
		{
			Err(Error::Parse { line, col, msg }) =>
			{
				assert_eq!((line, col), at, "{text}: {msg}");
				assert!(msg.contains(part), "{text}: {msg}");
			}
			other => panic!("{text}: {:?}", other.map(|_| ())),
		};
		syntax("{}", (1, 1), "a transaction is a vector");
		syntax("[7]", (1, 2), "expected a map");
		syntax("[] []", (1, 4), "unexpected text after the transaction");
		syntax("[[:db/add 1 :person/age 2]", (1, 1), "never closed");
		syntax("[[1 :person/age 2]]", (1, 2), "starts with its keyword");
		syntax("[[:db/add 1 :person/age]]", (1, 2), ":db/add takes an entity, an attribute and a value");
		syntax("[[:db/retract 1 :person/age]]", (1, 2), ":db/retract takes");
		syntax("[[:db/retractEntity 1 2]]", (1, 2), "takes an entity");
		syntax("[[:db/cas 1 :person/age 2]]", (1, 2), "the expected value (or nil)");
		syntax("[[:db/frob 1]]", (1, 3), "unknown operation :db/frob");
		syntax("[[:db/add -1 :person/age 2]]", (1, 11), "not negative");
		syntax("[[:db/add ?e :person/age 2]]", (1, 11), "expected an entity");
		syntax("[[:db/add [:person/name] :person/age 2]]", (1, 11), "a lookup ref is");
		syntax("[[:db/add 1 \"person/age\" 2]]", (1, 13), "expected an attribute keyword");
		syntax("[[:db/add 1 :person/age :old]]", (1, 25), "the keyword :old is not a value");
		syntax("[[:db/add 1 :person/age nil]]", (1, 25), "expected a constant");
		syntax("[[:db/add 1 :person/friend -2]]", (1, 28), "not negative");
		syntax("[[:db/add \"datomic.tx\" :db/txInstant 5]]", (1, 38), "takes an instant");
		syntax("[{}]", (1, 2), "needs an attribute");
		syntax("[{\"k\" 1}]", (1, 3), "expected an attribute keyword");
		syntax("[{:person/name {:person/age 1}}]", (1, 16), "only a Ref attribute takes an entity map");
		syntax("[\n {:db/ident :x/y :db/cardinality :db.cardinality/one}]", (2, 2), "the new attribute :x/y needs :db/valueType");
		syntax("[{:db/ident :x/y :db/valueType :db.type/long}]", (1, 2), "needs :db/cardinality");
		syntax("[{:db/ident :x/y :db/valueType :db.type/uuid}]", (1, 32), "expected :db.type/boolean or :db.type/long");
		syntax("[{:db/ident \"x\"}]", (1, 13), "takes the attribute's keyword");
		syntax("[{:db/ident :x/y :db/isComponent 1}]", (1, 34), "true or false");
		syntax("[{:db/ident :x/y :db/fulltext true}]", (1, 18), "unknown key :db/fulltext");

		// An unknown attribute, and the transaction's own errors; a failed transaction changes nothing
		assert_eq!(db.parse_tx("[[:db/add 1 :nope/attr 1]]").err(), Some(Error::UnknownAttr("nope/attr".into())));
		let failed = |conn: &mut Conn, text: &str| conn.transact_str(text).err().unwrap_or_else(|| panic!("{text} is applied"));
		assert!(matches!(failed(&mut conn, "[[:db/add \"x\" :person/age \"old\"]]"), Error::TypeMismatch { .. }));
		assert!(matches!(failed(&mut conn, "[[:db/add \"x\" :person/score 1]]"), Error::TypeMismatch { .. }));
		assert!(matches!(failed(&mut conn, "[[:db/cas [:person/name \"Alice\"] :person/age 99 100]]"), Error::CasFailed { .. }));
		assert!(matches!(failed(&mut conn, "[[:db/add [:person/name \"Nobody\"] :person/age 1]]"), Error::LookupNotFound { .. }));
		assert!(matches!(failed(&mut conn, "[[:db/add [:person/age 30] :person/vip true]]"), Error::NotUnique(_)));
		assert!(matches!(failed(&mut conn, "[{:db/ident :person/age :db/valueType :db.type/string}]"), Error::AlterRefused { .. }));
		assert!(matches!(failed(&mut conn, "[{:db/ident :db/mine :db/valueType :db.type/long :db/cardinality :db.cardinality/one}]"), Error::InvalidSchema(_)));
		assert!(matches!(failed(&mut conn, "[[:db/add [:person/name \"Alice\"] :db/txInstant #inst \"2024-01-01\"]]"), Error::Reserved(_)));
		assert!(matches!(conn.as_of(0).unwrap().transact_str("[]").err(), Some(Error::FilteredDb)));
		assert_same_db(&conn.db(), &db, "after the failed transactions");
	}
}
