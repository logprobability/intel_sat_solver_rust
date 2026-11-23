//! Datalog-style conjunctive queries over a database value.
//!
//! A `Query` is assembled clause by clause, and every clause is handed to the query's planner as soon as it is added:
//! the planner keeps one incremental SAT solver for the whole query and refines the join order within a small conflict budget per clause,
//! so a good (often optimal) plan already exists when the query is complete. `run` then finishes the optimization and executes the plan as nested loops,
//! each clause using the index that its bound positions select (EAVT, AVET or AEVT; their history counterparts in a history view,
//! where a pattern can also match the datom's transaction and added flag, see `Query::pattern_tx`).
//!
//! # Pull
//! `Db::pull` reads an entity as a tree, by a pattern of attributes: forward ones (a cardinality-one attribute gives one value,
//! a cardinality-many one the list of its values), reverse ones (the entities referring to it through a Ref attribute: one entity
//! for a component attribute, as a component has one owner, otherwise a list), nested patterns for the entities of a Ref attribute,
//! and the wildcard (every attribute of the entity, pulling the entities of its component attributes with the wildcard, recursively).
//! As in Datomic, absent attributes are left out, explicit entries override the wildcard's entry for the same attribute,
//! and a reverse attribute's key is its name with `_` before the local part (`person/_friend`).

use crate::db::{AttrId, AttrStats, Cardinality, Db, Eid, Error, Value, ValueType};
use crate::planner::{monotone, weight_of_fanout, ClauseShape, Plan, Planner, VarId};
use std::collections::{BTreeMap, BTreeSet};

/// The conflict budget of the refinement that follows every added clause
const ONLINE_BUDGET: u64 = 1000;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Term
{
	Var(VarId),
	Val(Value),
}

impl From<Value> for Term
{
	fn from(v: Value) -> Self
	{
		Term::Val(v)
	}
}

impl From<bool> for Term
{
	fn from(b: bool) -> Self
	{
		Term::Val(b.into())
	}
}

impl From<i64> for Term
{
	fn from(i: i64) -> Self
	{
		Term::Val(i.into())
	}
}

/// A string constant (variables come from `Query::var`)
impl From<&str> for Term
{
	fn from(s: &str) -> Self
	{
		Term::Val(s.into())
	}
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmp
{
	Eq,
	Ne,
	Lt,
	Le,
	Gt,
	Ge,
}

impl Cmp
{
	fn holds(self, l: &Value, r: &Value) -> bool
	{
		match self
		{
			Cmp::Eq => l == r,
			Cmp::Ne => l != r,
			Cmp::Lt => l < r,
			Cmp::Le => l <= r,
			Cmp::Gt => l > r,
			Cmp::Ge => l >= r,
		}
	}

	// The estimated fraction of the rows that pass
	fn selectivity(self) -> f64
	{
		match self
		{
			Cmp::Eq => 0.1,
			Cmp::Ne => 0.9,
			_ => 1.0 / 3.0,
		}
	}
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Clause
{
	/// [e a v tx added]: the entity term must be a variable or a `Value::Ref`; tx (a Ref) and added (a Bool), when given,
	/// match the datom's transaction entity and flag (in the current view, every datom has added true)
	Pattern
	{
		e: Term, a: AttrId, v: Term, tx: Option<Term>, added: Option<Term>
	},
	Pred
	{
		op: Cmp, l: Term, r: Term
	},
}

/// The planner's weights of a pattern, from the attribute's statistics: the fanout is the number of datoms matching per input row
fn pattern_weights(s: AttrStats) -> [u32; 4]
{
	let d = s.datoms as f64;
	let (e, v) = (s.entities.max(1) as f64, s.values.max(1) as f64);
	monotone([d, d / e, d / v, d / (e * v)].map(weight_of_fanout))
}

pub struct Query
{
	db: Db,
	vars: Vec<String>,
	clauses: Vec<Clause>,
	planner: Planner,
}

impl Db
{
	pub fn query(&self) -> Query
	{
		Query { db: self.clone(), vars: Vec::new(), clauses: Vec::new(), planner: Planner::new() }
	}
}

impl Query
{
	/// The variable named `name` (the same variable for the same name)
	pub fn var(&mut self, name: &str) -> Term
	{
		let i = self.vars.iter().position(|v| v == name).unwrap_or_else(|| {
			self.vars.push(name.to_string());
			self.vars.len() - 1
		});
		Term::Var(i)
	}

	/// The lookup ref [attr v] as a constant entity term (resolved now, as the query's database is fixed); unknown values are an error
	pub fn lookup(&self, attr: &str, v: impl Into<Value>) -> Result<Term, Error>
	{
		let v = v.into();
		let e = self.db.lookup(attr, &v)?.ok_or_else(|| Error::LookupNotFound { attr: attr.to_string(), value: v })?;
		Ok(Term::Val(Value::Ref(e)))
	}

	pub fn clauses(&self) -> &[Clause]
	{
		&self.clauses
	}

	/// Add the pattern [e attr v]; returns the clause's index
	pub fn pattern(&mut self, e: impl Into<Term>, attr: &str, v: impl Into<Term>) -> Result<usize, Error>
	{
		self.add_pattern(e.into(), attr, v.into(), None, None)
	}

	/// Add the pattern [e attr v tx added], which also matches the datom's transaction entity and its added flag
	/// (false for a retraction, which only a history view has); a variable used nowhere else ignores a position.
	/// Returns the clause's index
	pub fn pattern_tx(&mut self, e: impl Into<Term>, attr: &str, v: impl Into<Term>, tx: impl Into<Term>, added: impl Into<Term>) -> Result<usize, Error>
	{
		self.add_pattern(e.into(), attr, v.into(), Some(tx.into()), Some(added.into()))
	}

	fn add_pattern(&mut self, e: Term, attr: &str, v: Term, tx: Option<Term>, added: Option<Term>) -> Result<usize, Error>
	{
		let a = self.db.attr_id(attr).ok_or_else(|| Error::UnknownAttr(attr.to_string()))?;
		let def = self.db.attr(a);
		let typed = [(Some(&e), ValueType::Ref), (Some(&v), def.value_type), (tx.as_ref(), ValueType::Ref), (added.as_ref(), ValueType::Bool)];
		for (t, expected) in typed
		{
			if let Some(Term::Val(x)) = t
			{
				if x.value_type() != expected
				{
					return Err(Error::TypeMismatch { attr: attr.to_string(), expected, got: x.value_type() });
				}
			}
		}
		let var = |t: &Term| match t
		{
			Term::Var(x) => Some(*x),
			Term::Val(_) => None,
		};
		// The tx and added positions bind their variables, but the cost model only distinguishes the entity and value positions
		let slots = [var(&e), var(&v)];
		let binds = [Some(&e), Some(&v), tx.as_ref(), added.as_ref()].into_iter().flatten().filter_map(var).collect();
		let shape = ClauseShape { slots, needs: vec![], binds, weights: pattern_weights(self.db.stats(a)) };
		Ok(self.add(Clause::Pattern { e, a, v, tx, added }, shape))
	}

	/// Add the predicate `l op r`; its variables must be bound by patterns. Returns the clause's index
	pub fn pred(&mut self, l: impl Into<Term>, op: Cmp, r: impl Into<Term>) -> usize
	{
		let (l, r) = (l.into(), r.into());
		let needs = [&l, &r]
			.iter()
			.filter_map(|t| match t
			{
				Term::Var(x) => Some(*x),
				Term::Val(_) => None,
			})
			.collect();
		let shape = ClauseShape { slots: [None, None], needs, binds: vec![], weights: [weight_of_fanout(op.selectivity()); 4] };
		self.add(Clause::Pred { op, l, r }, shape)
	}

	fn add(&mut self, clause: Clause, shape: ClauseShape) -> usize
	{
		self.clauses.push(clause);
		self.planner.add_clause(shape, self.vars.len());
		self.planner.refine(ONLINE_BUDGET);
		self.clauses.len() - 1
	}

	/// The best plan found so far
	pub fn plan(&self) -> &Plan
	{
		self.planner.plan()
	}

	/// Improve the plan within `budget` conflicts (u64::MAX: until optimal)
	pub fn optimize(&mut self, budget: u64) -> &Plan
	{
		self.planner.refine(budget)
	}

	/// The planner's error, if a solver error disabled the SAT planner (the plans are greedy then)
	pub fn planner_error(&self) -> Option<&str>
	{
		self.planner.error()
	}

	/// The number of SAT solver invocations so far
	pub fn planner_solves(&self) -> u64
	{
		self.planner.solves
	}

	/// Optimize the plan and run it; returns the distinct tuples of the `find` terms, sorted
	pub fn run(&mut self, find: &[Term]) -> Result<Vec<Vec<Value>>, Error>
	{
		let order = self.optimize(u64::MAX).order.clone();
		self.run_in_order(&order, find)
	}

	pub(crate) fn run_in_order(&self, order: &[usize], find: &[Term]) -> Result<Vec<Vec<Value>>, Error>
	{
		let mut bindable = vec![false; self.vars.len()];
		for c in &self.clauses
		{
			if let Clause::Pattern { e, v, tx, added, .. } = c
			{
				for t in [Some(e), Some(v), tx.as_ref(), added.as_ref()].into_iter().flatten()
				{
					if let Term::Var(x) = t
					{
						bindable[*x] = true;
					}
				}
			}
		}
		let used = self.clauses.iter().flat_map(|c| match c
		{
			Clause::Pred { l, r, .. } => vec![l, r],
			Clause::Pattern { .. } => vec![],
		});
		if let Some(Term::Var(x)) = find.iter().chain(used).find(|t| matches!(t, Term::Var(x) if !bindable[*x]))
		{
			return Err(Error::Unbound(self.vars[*x].clone()));
		}
		let mut out = BTreeSet::new();
		self.step(order, &mut vec![None; self.vars.len()], find, &mut out)?;
		Ok(out.into_iter().collect())
	}

	fn eval(t: &Term, env: &[Option<Value>]) -> Option<Value>
	{
		match t
		{
			Term::Val(v) => Some(v.clone()),
			Term::Var(x) => env[*x].clone(),
		}
	}

	// Match t against val, binding t if it is an unbound variable (recorded in newly)
	fn unify(t: &Term, val: Value, env: &mut [Option<Value>], newly: &mut Vec<VarId>) -> bool
	{
		match t
		{
			Term::Val(c) => *c == val,
			Term::Var(x) => match &env[*x]
			{
				Some(b) => *b == val,
				None =>
				{
					env[*x] = Some(val);
					newly.push(*x);
					true
				}
			},
		}
	}

	fn step(&self, order: &[usize], env: &mut Vec<Option<Value>>, find: &[Term], out: &mut BTreeSet<Vec<Value>>) -> Result<(), Error>
	{
		let Some((&c, rest)) = order.split_first()
		else
		{
			out.insert(find.iter().map(|t| Self::eval(t, env).expect("the find variables are bound by the patterns")).collect());
			return Ok(());
		};
		match &self.clauses[c]
		{
			Clause::Pattern { e, a, v, tx, added } =>
			{
				let eid = match Self::eval(e, env)
				{
					Some(Value::Ref(x)) => Some(x),
					// A non-entity value in the entity position matches nothing
					Some(_) => return Ok(()),
					None => None,
				};
				// The index scan of the view (the bound tx and added terms filter its rows)
				let rows = self.db.datoms(eid, *a, Self::eval(v, env).as_ref());
				for d in rows
				{
					let mut newly = Vec::new();
					let ok = Self::unify(e, Value::Ref(d.e), env, &mut newly)
						&& Self::unify(v, d.v, env, &mut newly)
						&& tx.as_ref().map_or(true, |t| Self::unify(t, Value::Ref(d.tx), env, &mut newly))
						&& added.as_ref().map_or(true, |t| Self::unify(t, Value::Bool(d.added), env, &mut newly));
					if ok
					{
						self.step(rest, env, find, out)?;
					}
					for x in newly
					{
						env[x] = None;
					}
				}
				Ok(())
			}
			Clause::Pred { op, l, r } =>
			{
				let unbound = |t: &Term| match t
				{
					Term::Var(x) => Error::Unbound(self.vars[*x].clone()),
					Term::Val(_) => unreachable!("a constant is always bound"),
				};
				let lv = Self::eval(l, env).ok_or_else(|| unbound(l))?;
				let rv = Self::eval(r, env).ok_or_else(|| unbound(r))?;
				if op.holds(&lv, &rv)
				{
					self.step(rest, env, find, out)?;
				}
				Ok(())
			}
		}
	}
}

/// An element of a pull pattern (see the module documentation)
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pull
{
	/// Every attribute of the entity, with the entities of its component attributes pulled by the wildcard
	/// (an entity already being pulled further up stays a Ref, so a component cycle ends)
	Wildcard,
	/// The attribute `name`, or its reverse (the entities referring to this one through it); `nested` pulls the referenced entities
	Attr
	{
		name: String, reverse: bool, nested: Option<Vec<Pull>>
	},
}

impl Pull
{
	pub fn attr(name: &str) -> Self
	{
		Pull::Attr { name: name.to_string(), reverse: false, nested: None }
	}

	pub fn reverse(name: &str) -> Self
	{
		Pull::Attr { name: name.to_string(), reverse: true, nested: None }
	}

	/// Pull the attribute's entities by `pattern` (the attribute must be a Ref, which `Db::pull` checks); panics on the wildcard
	pub fn nest(self, pattern: Vec<Pull>) -> Self
	{
		match self
		{
			Pull::Attr { name, reverse, .. } => Pull::Attr { name, reverse, nested: Some(pattern) },
			Pull::Wildcard => panic!("the wildcard has no entities to nest a pattern in"),
		}
	}
}

/// A value of a pulled attribute
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PullValue
{
	/// A value (a Ref, when the pattern doesn't pull the entity)
	Val(Value),
	/// A referenced entity, pulled by a nested pattern or the wildcard
	Entity(Pulled),
	/// The values of a cardinality-many attribute, or of a reverse attribute that isn't a component, in index order
	Many(Vec<PullValue>),
}

/// A pulled entity: its id and the attributes the pattern selected that it has
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pulled
{
	pub id: Eid,
	pub attrs: BTreeMap<String, PullValue>,
}

impl Pulled
{
	pub fn get(&self, key: &str) -> Option<&PullValue>
	{
		self.attrs.get(key)
	}
}

/// A pull pattern with its attributes resolved
struct PullSpec
{
	wildcard: bool,
	/// (attribute, reverse, nested pattern)
	attrs: Vec<(AttrId, bool, Option<PullSpec>)>,
}

impl PullSpec
{
	const WILDCARD: PullSpec = PullSpec { wildcard: true, attrs: Vec::new() };

	// Resolved before pulling anything, so that the errors don't depend on the data
	fn compile(db: &Db, pattern: &[Pull]) -> Result<PullSpec, Error>
	{
		let mut spec = PullSpec { wildcard: false, attrs: Vec::new() };
		for p in pattern
		{
			match p
			{
				Pull::Wildcard => spec.wildcard = true,
				Pull::Attr { name, reverse, nested } =>
				{
					let a = db.attr_id(name).ok_or_else(|| Error::UnknownAttr(name.clone()))?;
					let t = db.attr(a).value_type;
					if (*reverse || nested.is_some()) && t != ValueType::Ref
					{
						return Err(Error::TypeMismatch { attr: name.clone(), expected: ValueType::Ref, got: t });
					}
					let nested = nested.as_ref().map(|n| Self::compile(db, n)).transpose()?;
					spec.attrs.push((a, *reverse, nested));
				}
			}
		}
		Ok(spec)
	}
}

/// The result key of a reverse attribute: `ns/_name` for `ns/name`
fn reverse_key(name: &str) -> String
{
	match name.rsplit_once('/')
	{
		Some((ns, local)) => format!("{ns}/_{local}"),
		None => format!("_{name}"),
	}
}

// One value for a cardinality-one attribute (the first, if a reverse component has several owners), a list otherwise; items is not empty
fn shape(one: bool, items: Vec<PullValue>) -> PullValue
{
	if one { items.into_iter().next().expect("not empty") } else { PullValue::Many(items) }
}

impl Db
{
	/// Pull entity `e` by `pattern` (see the module documentation); an entity without any of the attributes gives an empty map.
	/// Reads the current-state accessors, so a `since` view filters the result; a history view is refused (`FilteredDb`)
	pub fn pull(&self, pattern: &[Pull], e: Eid) -> Result<Pulled, Error>
	{
		if self.is_history()
		{
			return Err(Error::FilteredDb);
		}
		self.check_eid(e)?;
		let spec = PullSpec::compile(self, pattern)?;
		Ok(self.pull_spec(&spec, e, &mut Vec::new()))
	}

	// path: the entities being pulled above this one, where the wildcard stops following components
	fn pull_spec(&self, spec: &PullSpec, e: Eid, path: &mut Vec<Eid>) -> Pulled
	{
		path.push(e);
		let mut attrs = BTreeMap::new();
		if spec.wildcard
		{
			let mut by_attr: BTreeMap<AttrId, Vec<Value>> = BTreeMap::new();
			for (a, v) in self.entity_datoms(e)
			{
				by_attr.entry(a).or_default().push(v.clone());
			}
			for (a, vals) in by_attr
			{
				let def = self.attr(a);
				let mut items = Vec::with_capacity(vals.len());
				for v in vals
				{
					items.push(match v
					{
						Value::Ref(c) if def.component && !path.contains(&c) => PullValue::Entity(self.pull_spec(&PullSpec::WILDCARD, c, path)),
						v => PullValue::Val(v),
					});
				}
				attrs.insert(def.name.to_string(), shape(def.cardinality == Cardinality::One, items));
			}
		}
		for (a, reverse, nested) in &spec.attrs
		{
			let def = self.attr(*a);
			let (key, one, vals): (String, bool, Vec<Value>) = if *reverse
			{
				(reverse_key(&def.name), def.component, self.referrers(e).filter(|(b, _)| b == a).map(|(_, x)| Value::Ref(x)).collect())
			}
			else
			{
				(def.name.to_string(), def.cardinality == Cardinality::One, self.values(e, *a).cloned().collect())
			};
			if vals.is_empty()
			{
				continue;
			}
			let mut items = Vec::with_capacity(vals.len());
			for v in vals
			{
				items.push(match (nested, v)
				{
					(Some(n), Value::Ref(c)) => PullValue::Entity(self.pull_spec(n, c, path)),
					(_, v) => PullValue::Val(v),
				});
			}
			attrs.insert(key, shape(one, items));
		}
		path.pop();
		Pulled { id: e, attrs }
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::db::{Cardinality, Conn, Entity, Tx};
	use crate::testutil::Rng;

	// People with ages and cities, a few of whom follow each other
	fn social(n: u64) -> Conn
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("person/name", ValueType::Str, Cardinality::One)
			.define("person/age", ValueType::Int, Cardinality::One)
			.define("person/city", ValueType::Str, Cardinality::One)
			.define("person/follows", ValueType::Ref, Cardinality::Many);
		let people: Vec<Entity> = (0..n).map(|_| tx.temp()).collect();
		let mut rng = Rng::new(11);
		for (i, &p) in people.iter().enumerate()
		{
			tx.add(p, "person/name", format!("p{i}").as_str()).add(p, "person/age", (rng.next() % 80) as i64).add(p, "person/city", ["Haifa", "Austin", "Lisbon"][i % 3]);
			for _ in 0..2
			{
				tx.add(p, "person/follows", people[(rng.next() % n) as usize]);
			}
		}
		conn.transact(&tx).unwrap();
		conn
	}

	#[test]
	fn friends_of_friends_in_a_city()
	{
		let conn = social(60);
		let db = conn.db();
		let mut q = db.query();
		let (p, f, ff, name, age, city) = (q.var("?p"), q.var("?f"), q.var("?ff"), q.var("?name"), q.var("?age"), q.var("?city"));
		q.pattern(p.clone(), "person/follows", f.clone()).unwrap();
		q.pattern(f.clone(), "person/follows", ff.clone()).unwrap();
		q.pattern(ff.clone(), "person/name", name.clone()).unwrap();
		q.pattern(ff.clone(), "person/age", age.clone()).unwrap();
		q.pred(age.clone(), Cmp::Gt, 40);
		q.pattern(p.clone(), "person/city", city.clone()).unwrap();
		q.pattern(p.clone(), "person/name", "p7").unwrap();
		let got = q.run(&[name.clone(), age.clone(), city.clone()]).unwrap();
		let plan = q.plan().clone();
		assert!(plan.optimal);
		// The most selective pattern (a constant name: one datom) goes first
		assert_eq!(plan.order[0], 6);

		// A hand-computed answer
		let name_a = db.attr_id("person/name").unwrap();
		let follows = db.attr_id("person/follows").unwrap();
		let age_a = db.attr_id("person/age").unwrap();
		let p7 = db.entities(name_a, &Value::from("p7")).next().unwrap();
		let mut want = BTreeSet::new();
		for f in db.values(p7, follows)
		{
			let Value::Ref(f) = f else { unreachable!() };
			for ff in db.values(*f, follows)
			{
				let Value::Ref(ff) = ff else { unreachable!() };
				let age = db.values(*ff, age_a).next().unwrap().clone();
				if age > Value::Int(40)
				{
					// p7 lives in the city 7 % 3
					want.insert(vec![db.values(*ff, name_a).next().unwrap().clone(), age, Value::from("Austin")]);
				}
			}
		}
		assert!(!want.is_empty());
		assert_eq!(got, want.into_iter().collect::<Vec<_>>());

		// Every valid order gives the same answer
		let mut rng = Rng::new(5);
		for _ in 0..30
		{
			let mut order: Vec<usize> = (0..q.clauses().len()).collect();
			for i in (1..order.len()).rev()
			{
				order.swap(i, (rng.next() % (i as u64 + 1)) as usize);
			}
			// Move the predicate right after the pattern binding ?age
			order.retain(|&c| c != 4);
			let at = order.iter().position(|&c| c == 3).unwrap();
			order.insert(at + 1, 4);
			assert_eq!(q.run_in_order(&order, &[name.clone(), age.clone(), city.clone()]).unwrap(), got);
		}
	}

	#[test]
	fn online_plan_tracks_the_query_as_it_grows()
	{
		let conn = social(40);
		let db = conn.db();
		let mut q = db.query();
		let (p, age) = (q.var("?p"), q.var("?age"));
		// The predicate arrives before its binder: it is pending, and the query can't run yet
		q.pred(age.clone(), Cmp::Lt, 10);
		assert_eq!(q.plan().pending, vec![0]);
		assert_eq!(q.run(&[p.clone()]).err(), Some(Error::Unbound("?p".into())));
		q.pattern(p.clone(), "person/age", age.clone()).unwrap();
		assert!(q.plan().pending.is_empty());
		assert_eq!(q.plan().order, vec![1, 0]);
		let young = q.run(&[p.clone()]).unwrap();
		let age_a = db.attr_id("person/age").unwrap();
		assert_eq!(young.len(), db.attr_datoms(age_a).filter(|(_, v)| **v < Value::Int(10)).count());
		assert!(q.planner_solves() > 0);
	}

	#[test]
	fn query_validation()
	{
		let conn = social(3);
		let mut q = conn.db().query();
		let x = q.var("?x");
		assert!(matches!(q.pattern(x.clone(), "nope", 1), Err(Error::UnknownAttr(_))));
		assert!(matches!(q.pattern(x.clone(), "person/age", "old"), Err(Error::TypeMismatch { .. })));
		assert!(matches!(q.pattern(5, "person/age", x.clone()), Err(Error::TypeMismatch { .. })));
		assert!(q.clauses().is_empty());
	}

	fn permutations(n: usize) -> Vec<Vec<usize>>
	{
		if n == 0
		{
			return vec![vec![]];
		}
		let mut out = Vec::new();
		for p in permutations(n - 1)
		{
			for i in 0..n
			{
				let mut q = p.clone();
				q.insert(i, n - 1);
				out.push(q);
			}
		}
		out
	}

	// The query's answer, which every order of its clauses (patterns only) must give too
	fn run_every_order(q: &mut Query, find: &[Term]) -> Vec<Vec<Value>>
	{
		let got = q.run(find).unwrap();
		for order in permutations(q.clauses().len())
		{
			assert_eq!(q.run_in_order(&order, find).unwrap(), got, "order {order:?}");
		}
		got
	}

	fn sorted(rows: impl IntoIterator<Item = Vec<Value>>) -> Vec<Vec<Value>>
	{
		rows.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
	}

	// Patterns with transaction and added terms over the history, since and current views, against brute force over the transaction log
	#[test]
	fn queries_over_history_match_the_log()
	{
		use crate::db::tests::{random_conn, replay};
		use crate::db::{Datom, TX_INSTANT};
		let (conn, reports) = random_conn(31, 80);
		let db = conn.db();
		let log: Vec<&Datom> = reports.iter().flat_map(|r| &r.tx_data).collect();
		let id = |n: &str| db.attr_id(n).unwrap();
		let (n_a, tag_a, ref_a) = (id("n"), id("tag"), id("ref"));
		let instant: BTreeMap<Eid, Value> = log.iter().filter(|d| d.a == id(TX_INSTANT)).map(|d| (d.tx, d.v.clone())).collect();
		let r = |e: Eid| Value::Ref(e);

		// The retractions of tags, with their transactions' instants
		let mut q = db.history().query();
		let (e, v, tx, i) = (q.var("?e"), q.var("?v"), q.var("?tx"), q.var("?i"));
		q.pattern_tx(e.clone(), "tag", v.clone(), tx.clone(), false).unwrap();
		q.pattern(tx.clone(), TX_INSTANT, i.clone()).unwrap();
		let want = sorted(log.iter().filter(|d| d.a == tag_a && !d.added).map(|d| vec![r(d.e), d.v.clone(), instant[&d.tx].clone()]));
		assert!(want.len() >= 3);
		assert_eq!(run_every_order(&mut q, &[e, v, i]), want);

		// An assertion of n and a change of a tag (of any entity) in the same transaction: a join on ?tx alone
		let mut q = db.history().query();
		let (e, f, nv, g, tx, ad) = (q.var("?e"), q.var("?f"), q.var("?n"), q.var("?g"), q.var("?tx"), q.var("?added"));
		q.pattern_tx(e.clone(), "n", nv.clone(), tx.clone(), true).unwrap();
		q.pattern_tx(f.clone(), "tag", g.clone(), tx.clone(), ad.clone()).unwrap();
		let mut want = Vec::new();
		for x in log.iter().filter(|d| d.a == n_a && d.added)
		{
			for y in log.iter().filter(|d| d.a == tag_a && d.tx == x.tx)
			{
				want.push(vec![r(x.e), r(y.e), x.v.clone(), y.v.clone(), Value::Bool(y.added), r(x.tx)]);
			}
		}
		let want = sorted(want);
		assert!(!want.is_empty());
		assert_eq!(run_every_order(&mut q, &[e, f, nv, g, ad, tx]), want);

		// The current refs asserted after basis 15
		let mut q = db.since(15).unwrap().query();
		let (e, x, tx, ad) = (q.var("?e"), q.var("?x"), q.var("?tx"), q.var("?added"));
		q.pattern_tx(e.clone(), "ref", x.clone(), tx.clone(), ad.clone()).unwrap();
		let after = reports[14].tx;
		let current = replay(log.iter().copied());
		let want = sorted(current.iter().filter(|((_, a, _), t)| *a == ref_a && **t > after).map(|((e, _, v), t)| vec![r(*e), v.clone(), r(*t), Value::Bool(true)]));
		assert!(!want.is_empty());
		assert_eq!(run_every_order(&mut q, &[e, x, tx, ad]), want);
		// A lookup ref in a since view sees only the later datoms too
		let key_a = id("key");
		let ((holder, _, k), t) = current.iter().find(|((_, a, _), _)| *a == key_a).unwrap();
		let s = 1 + reports.iter().position(|rep| rep.tx == *t).unwrap() as u64;
		assert_eq!(db.since(s - 1).unwrap().query().lookup("key", k.clone()), Ok(Term::Val(r(*holder))));
		assert!(matches!(db.since(s).unwrap().query().lookup("key", k.clone()), Err(Error::LookupNotFound { .. })));

		// A constant transaction in the history after basis 1
		let k = (1..reports.len()).find(|&k| reports[k].tx_data.iter().any(|d| d.a == n_a)).unwrap();
		let mut q = db.history().since(1).unwrap().query();
		let (e, v, ad) = (q.var("?e"), q.var("?v"), q.var("?added"));
		q.pattern_tx(e.clone(), "n", v.clone(), r(reports[k].tx), ad.clone()).unwrap();
		let want = sorted(reports[k].tx_data.iter().filter(|d| d.a == n_a).map(|d| vec![r(d.e), d.v.clone(), Value::Bool(d.added)]));
		assert_eq!(run_every_order(&mut q, &[e, v, ad.clone()]), want);

		// The current view: the asserting transaction of each datom, and no retractions
		let mut q = db.query();
		let (e, v, tx, i) = (q.var("?e"), q.var("?v"), q.var("?tx"), q.var("?i"));
		q.pattern_tx(e.clone(), "n", v.clone(), tx.clone(), true).unwrap();
		q.pattern(tx.clone(), TX_INSTANT, i.clone()).unwrap();
		let want = sorted(current.iter().filter(|((_, a, _), _)| *a == n_a).map(|((e, _, v), t)| vec![r(*e), v.clone(), instant[t].clone()]));
		assert_eq!(run_every_order(&mut q, &[e.clone(), v.clone(), i]), want);
		let mut q = db.query();
		let (e, v, tx) = (q.var("?e"), q.var("?v"), q.var("?tx"));
		q.pattern_tx(e.clone(), "n", v.clone(), tx.clone(), false).unwrap();
		assert!(q.run(&[e.clone()]).unwrap().is_empty());

		// Validation of the new positions
		let mut q = db.history().query();
		let (e, v, tx, ad) = (q.var("?e"), q.var("?v"), q.var("?tx"), q.var("?added"));
		assert!(matches!(q.pattern_tx(e.clone(), "n", v.clone(), 5, ad.clone()), Err(Error::TypeMismatch { expected: ValueType::Ref, .. })));
		assert!(matches!(q.pattern_tx(e.clone(), "n", v.clone(), tx.clone(), 1), Err(Error::TypeMismatch { expected: ValueType::Bool, .. })));
		assert!(q.clauses().is_empty());
		// The planner estimates a history view with the history's statistics
		assert!(db.history().stats(tag_a).datoms > db.stats(tag_a).datoms);
	}

	fn val(v: impl Into<Value>) -> PullValue
	{
		PullValue::Val(v.into())
	}

	fn ent(id: Eid, attrs: Vec<(&str, PullValue)>) -> PullValue
	{
		PullValue::Entity(Pulled { id, attrs: attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect() })
	}

	#[test]
	fn pull_examples()
	{
		use crate::db::Attr;
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("person/name", ValueType::Str, Cardinality::One)
			.define("person/aka", ValueType::Str, Cardinality::Many)
			.define("person/friend", ValueType::Ref, Cardinality::Many)
			.define_attr(Attr::new("person/address", ValueType::Ref, Cardinality::One).component())
			.define("address/city", ValueType::Str, Cardinality::One)
			.define_attr(Attr::new("node/child", ValueType::Ref, Cardinality::Many).component());
		let (a, b, c, addr, n1, n2, empty) = (tx.temp(), tx.temp(), tx.temp(), tx.temp(), tx.temp(), tx.temp(), tx.temp());
		tx.add(a, "person/name", "Alice").add(a, "person/aka", "Al").add(a, "person/aka", "Ali").add(a, "person/friend", b).add(a, "person/friend", c);
		tx.add(b, "person/name", "Bob").add(b, "person/friend", a).add(c, "person/name", "Carol");
		tx.add(a, "person/address", addr).add(addr, "address/city", "Haifa");
		// A component cycle
		tx.add(n1, "node/child", n2).add(n2, "node/child", n1);
		let rep = conn.transact(&tx).unwrap();
		let [a, b, c, addr, n1, n2, empty] = [a, b, c, addr, n1, n2, empty].map(|e| rep.eid(e));
		let db = conn.db();
		let name = || Pull::attr("person/name");

		// Cardinality one and many, refs as values
		let p = db.pull(&[name(), Pull::attr("person/aka"), Pull::attr("person/friend")], a).unwrap();
		assert_eq!(PullValue::Entity(p), ent(a, vec![("person/aka", PullValue::Many(vec![val("Al"), val("Ali")])), ("person/friend", PullValue::Many(vec![val(Value::Ref(b)), val(Value::Ref(c))])), ("person/name", val("Alice"))]));
		// Nested, and reverse (a list, or one entity for a component)
		let p = db.pull(&[name(), Pull::attr("person/friend").nest(vec![name()])], a).unwrap();
		assert_eq!(p.get("person/friend"), Some(&PullValue::Many(vec![ent(b, vec![("person/name", val("Bob"))]), ent(c, vec![("person/name", val("Carol"))])])));
		let p = db.pull(&[Pull::reverse("person/friend").nest(vec![name()])], c).unwrap();
		assert_eq!(p.get("person/_friend"), Some(&PullValue::Many(vec![ent(a, vec![("person/name", val("Alice"))])])));
		let p = db.pull(&[Pull::reverse("person/address")], addr).unwrap();
		assert_eq!(p.get("person/_address"), Some(&val(Value::Ref(a))));
		// The wildcard pulls components, and an explicit entry overrides it
		let p = db.pull(&[Pull::Wildcard], a).unwrap();
		assert_eq!(p.get("person/address"), Some(&ent(addr, vec![("address/city", val("Haifa"))])));
		assert_eq!(p.get("person/friend"), Some(&PullValue::Many(vec![val(Value::Ref(b)), val(Value::Ref(c))])));
		assert_eq!(p.attrs.len(), 4);
		let p = db.pull(&[Pull::Wildcard, Pull::attr("person/friend").nest(vec![name()])], a).unwrap();
		assert_eq!(p.get("person/friend"), Some(&PullValue::Many(vec![ent(b, vec![("person/name", val("Bob"))]), ent(c, vec![("person/name", val("Carol"))])])));
		assert_eq!(p.attrs.len(), 4);
		// A component cycle ends at the entity being pulled
		let p = db.pull(&[Pull::Wildcard], n1).unwrap();
		assert_eq!(p.get("node/child"), Some(&PullValue::Many(vec![ent(n2, vec![("node/child", PullValue::Many(vec![val(Value::Ref(n1))]))])])));
		// Absent attributes are left out
		assert!(db.pull(&[Pull::Wildcard, name()], empty).unwrap().attrs.is_empty());
		assert!(db.pull(&[Pull::attr("person/aka")], b).unwrap().attrs.is_empty());

		// Errors, independent of the data
		assert_eq!(db.pull(&[Pull::attr("nope")], empty).err(), Some(Error::UnknownAttr("nope".into())));
		assert!(matches!(db.pull(&[Pull::reverse("person/name")], empty), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.pull(&[Pull::attr("person/aka").nest(vec![])], empty), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.pull(&[Pull::attr("person/friend").nest(vec![Pull::attr("nope")])], empty), Err(Error::UnknownAttr(_))));
		assert_eq!(db.pull(&[name()], 9999).err(), Some(Error::UnknownEntity(9999)));
		assert_eq!(db.history().pull(&[name()], a).err(), Some(Error::FilteredDb));

		// A since view filters the pull, reverse refs included
		let mut tx = Tx::new();
		let d = tx.temp();
		tx.add(d, "person/name", "Dan").add(Entity::Id(a), "person/friend", d).add(d, "person/friend", Entity::Id(c));
		let rep = conn.transact(&tx).unwrap();
		let d = rep.eid(d);
		let since = conn.db().since(1).unwrap();
		let p = since.pull(&[name(), Pull::attr("person/friend")], a).unwrap();
		assert_eq!(PullValue::Entity(p), ent(a, vec![("person/friend", PullValue::Many(vec![val(Value::Ref(d))]))]));
		assert_eq!(since.pull(&[Pull::reverse("person/friend")], c).unwrap().get("person/_friend"), Some(&PullValue::Many(vec![val(Value::Ref(d))])));
		// ... and the past value is unaffected
		assert_eq!(conn.as_of(1).unwrap().pull(&[Pull::reverse("person/friend")], c).unwrap().get("person/_friend"), Some(&PullValue::Many(vec![val(Value::Ref(a))])));
	}

	// A naive pull over a set of (e, a, v) triples, by scanning it
	fn naive_pull(db: &Db, facts: &BTreeSet<(Eid, AttrId, Value)>, pattern: &[Pull], e: Eid, path: &mut Vec<Eid>) -> Pulled
	{
		path.push(e);
		let mut attrs = BTreeMap::new();
		let mut put = |key: String, one: bool, items: Vec<PullValue>| {
			if !items.is_empty()
			{
				attrs.insert(key, if one { items.into_iter().next().unwrap() } else { PullValue::Many(items) });
			}
		};
		if pattern.contains(&Pull::Wildcard)
		{
			let mine: BTreeSet<AttrId> = facts.iter().filter(|f| f.0 == e).map(|f| f.1).collect();
			for a in mine
			{
				let def = db.attr(a).clone();
				let mut items = Vec::new();
				for f in facts.iter().filter(|f| f.0 == e && f.1 == a)
				{
					items.push(match f.2
					{
						Value::Ref(x) if def.component && !path.contains(&x) => PullValue::Entity(naive_pull(db, facts, &[Pull::Wildcard], x, path)),
						_ => PullValue::Val(f.2.clone()),
					});
				}
				put(def.name.to_string(), def.cardinality == Cardinality::One, items);
			}
		}
		for p in pattern
		{
			let Pull::Attr { name, reverse, nested } = p else { continue };
			let a = db.attr_id(name).unwrap();
			let def = db.attr(a).clone();
			let vals: Vec<Value> = if *reverse
			{
				facts.iter().filter(|f| f.1 == a && f.2 == Value::Ref(e)).map(|f| Value::Ref(f.0)).collect()
			}
			else
			{
				facts.iter().filter(|f| f.0 == e && f.1 == a).map(|f| f.2.clone()).collect()
			};
			let mut items = Vec::new();
			for v in vals
			{
				items.push(match (nested, v)
				{
					(Some(n), Value::Ref(x)) => PullValue::Entity(naive_pull(db, facts, n, x, path)),
					(_, v) => PullValue::Val(v),
				});
			}
			let key = if *reverse { format!("_{name}") } else { name.clone() };
			put(key, if *reverse { def.component } else { def.cardinality == Cardinality::One }, items);
		}
		path.pop();
		Pulled { id: e, attrs }
	}

	fn random_pull(rng: &mut Rng, depth: u32) -> Vec<Pull>
	{
		use crate::db::tests::RANDOM_ATTRS;
		let refs = ["ref", "best", "part"];
		let mut p = Vec::new();
		if rng.next() % 4 == 0
		{
			p.push(Pull::Wildcard);
		}
		for _ in 0..1 + rng.next() % 3
		{
			if rng.next() % 2 == 0
			{
				let name = refs[(rng.next() % 3) as usize];
				let x = if rng.next() % 2 == 0 { Pull::reverse(name) } else { Pull::attr(name) };
				p.push(if depth > 0 && rng.next() % 2 == 0 { x.nest(random_pull(rng, depth - 1)) } else { x });
			}
			else
			{
				p.push(Pull::attr(RANDOM_ATTRS[(rng.next() % 6) as usize]));
			}
		}
		p
	}

	fn count_entities(v: &PullValue) -> usize
	{
		match v
		{
			PullValue::Val(_) => 0,
			PullValue::Entity(p) => 1 + p.attrs.values().map(count_entities).sum::<usize>(),
			PullValue::Many(vs) => vs.iter().map(count_entities).sum(),
		}
	}

	// Pulls of random patterns (forward, reverse, nested, wildcard) of every entity, in past values and since views, against a naive scan
	#[test]
	fn pull_matches_a_naive_scan()
	{
		use crate::db::tests::{random_conn, replay};
		let (conn, reports) = random_conn(41, 40);
		let mut rng = Rng::new(9);
		let mut nested = 0;
		for t in [10, 25, 40]
		{
			let db = conn.as_of(t).unwrap();
			let log: Vec<_> = reports[..t as usize].iter().flat_map(|r| &r.tx_data).collect();
			let ents: BTreeSet<Eid> = log.iter().map(|d| d.e).collect();
			let current = replay(log.iter().copied());
			for s in [0, 1, t / 2]
			{
				let view = db.since(s).unwrap();
				let after = db.tx_of(s).unwrap_or(0);
				// The naive reference keeps the user datoms, so that the wildcard of a transaction entity compares too
				let facts: BTreeSet<(Eid, AttrId, Value)> = current.iter().filter(|(_, tx)| **tx > after).map(|(k, _)| k.clone()).collect();
				for _ in 0..40
				{
					let pattern = random_pull(&mut rng, 2);
					for &e in &ents
					{
						let got = view.pull(&pattern, e).unwrap();
						// The naive keys of reverse attributes are _name, the library's ns/_name (these attributes have no namespace)
						assert_eq!(got, naive_pull(&view, &facts, &pattern, e, &mut Vec::new()), "pattern {pattern:?} e {e} t {t} since {s}");
						nested += count_entities(&PullValue::Entity(got)) - 1;
					}
				}
			}
		}
		assert!(nested > 500, "too few nested entities: {nested}");
	}
}
