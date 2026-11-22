//! Datalog-style conjunctive queries over a database value.
//!
//! A `Query` is assembled clause by clause, and every clause is handed to the query's planner as soon as it is added:
//! the planner keeps one incremental SAT solver for the whole query and refines the join order within a small conflict budget per clause,
//! so a good (often optimal) plan already exists when the query is complete. `run` then finishes the optimization and executes the plan as nested loops,
//! each clause using the index that its bound positions select (EAVT, AVET or AEVT).

use crate::db::{AttrId, AttrStats, Db, Eid, Error, Value, ValueType};
use crate::planner::{monotone, weight_of_fanout, ClauseShape, Plan, Planner, VarId};
use std::collections::BTreeSet;

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
	/// [e a v]: the entity term must be a variable or a `Value::Ref`
	Pattern
	{
		e: Term, a: AttrId, v: Term
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

	pub fn clauses(&self) -> &[Clause]
	{
		&self.clauses
	}

	/// Add the pattern [e attr v]; returns the clause's index
	pub fn pattern(&mut self, e: impl Into<Term>, attr: &str, v: impl Into<Term>) -> Result<usize, Error>
	{
		let (e, v) = (e.into(), v.into());
		let a = self.db.attr_id(attr).ok_or_else(|| Error::UnknownAttr(attr.to_string()))?;
		let def = self.db.attr(a);
		let mismatch = |expected: ValueType, got: &Value| Error::TypeMismatch { attr: attr.to_string(), expected, got: got.value_type() };
		if let Term::Val(x) = &e
		{
			if x.value_type() != ValueType::Ref
			{
				return Err(mismatch(ValueType::Ref, x));
			}
		}
		if let Term::Val(x) = &v
		{
			if x.value_type() != def.value_type
			{
				return Err(mismatch(def.value_type, x));
			}
		}
		let slots = [&e, &v].map(|t| match t
		{
			Term::Var(x) => Some(*x),
			Term::Val(_) => None,
		});
		let shape = ClauseShape { slots, needs: vec![], binds: slots.iter().flatten().copied().collect(), weights: pattern_weights(self.db.stats(a)) };
		Ok(self.add(Clause::Pattern { e, a, v }, shape))
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
			if let Clause::Pattern { e, v, .. } = c
			{
				for t in [e, v]
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
			Clause::Pattern { e, a, v } =>
			{
				let eid = match Self::eval(e, env)
				{
					Some(Value::Ref(x)) => Some(x),
					// A non-entity value in the entity position matches nothing
					Some(_) => return Ok(()),
					None => None,
				};
				let db = &self.db;
				let rows: Vec<(Eid, Value)> = match (eid, Self::eval(v, env))
				{
					(Some(x), Some(val)) => if db.has(x, *a, &val) { vec![(x, val)] } else { vec![] },
					(Some(x), None) => db.values(x, *a).map(|val| (x, val.clone())).collect(),
					(None, Some(val)) => db.entities(*a, &val).map(|x| (x, val.clone())).collect(),
					(None, None) => db.attr_datoms(*a).map(|(x, val)| (x, val.clone())).collect(),
				};
				for (x, val) in rows
				{
					let mut newly = Vec::new();
					if Self::unify(e, Value::Ref(x), env, &mut newly) && Self::unify(v, val, env, &mut newly)
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
}
