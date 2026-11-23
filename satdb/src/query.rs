//! Datalog-style queries over a database value.
//!
//! A `Query` is assembled clause by clause, and every clause is handed to the query's planner as soon as it is added:
//! the planner keeps one incremental SAT solver for the whole query and refines the join order within a small conflict budget per clause,
//! so a good (often optimal) plan already exists when the query is complete. `run` then finishes the optimization and executes the plan as nested loops,
//! each clause using the index that its bound positions select (EAVT, AVET or AEVT; their history counterparts in a history view,
//! where a pattern can also match the datom's transaction and added flag, see `Query::pattern_tx`).
//! The `parse` module reads the same queries from a Datalog text (`Db::q`).
//!
//! # Clauses
//! * A pattern `[e attr v]` (or `[e attr v tx added]`) matches datoms and binds its variables.
//! * A predicate `l op r` compares two bound terms.
//! * `not` keeps the rows for which its body has no solution. Its *join* variables (every variable of its body, or the list of `not-join`)
//!   must be bound when it runs; the body's other variables are its own.
//! * `or` is the union of its branches' solutions, projected on its join variables (the variables of its branches, which must all use
//!   the same ones, or the list of `or-join`); the branches' other variables are their own. An `or` whose every branch binds every join
//!   variable by itself is a *binder*, like a pattern; any other `or` is a *filter*, which needs its join variables bound, like a `not`.
//!
//! The SAT planner doesn't look inside `not` and `or`: to it, a filter is a clause that needs its join variables (like a predicate,
//! so it is pending until they have binders), and a binder is a clause that binds them, at a cost estimated from its branches.
//! A clause never both needs and binds variables, which keeps the planner's encoding sound (a binder never has requirements of its own).
//! Each run plans the bodies of `not` and `or` once, with the join variables bound where the clause runs counted as constants,
//! and runs them in an environment of their own (the join variables only, so a body's own variables never meet the query's).
//!
//! # Find specs
//! `Query::find` takes plain terms and aggregates (`count`, `count-distinct`, `sum`, `min`, `max`, `avg`). As in Datomic, the query
//! first finds the distinct tuples of every plain, aggregated and `with` term, then groups them by the plain terms and aggregates each
//! group's column (so `with` variables keep tuples that would otherwise be equal apart). With no matching tuples, there are no rows.
//! There is no floating-point value type, so `avg` is the integer mean rounded down; `sum` and `avg` take Int values only.
//!
//! # Pull
//! `Db::pull` reads an entity as a tree, by a pattern of attributes: forward ones (a cardinality-one attribute gives one value,
//! a cardinality-many one the list of its values), reverse ones (the entities referring to it through a Ref attribute: one entity
//! for a component attribute, as a component has one owner, otherwise a list), nested patterns for the entities of a Ref attribute,
//! and the wildcard (every attribute of the entity, pulling the entities of its component attributes with the wildcard, recursively).
//! As in Datomic, absent attributes are left out, explicit entries override the wildcard's entry for the same attribute,
//! and a reverse attribute's key is its name with `_` before the local part (`person/_friend`).

use crate::db::{AttrId, AttrStats, Cardinality, Db, Eid, Error, Value, ValueType};
use crate::planner::{fanout_of_weight, greedy_order, monotone, weight_of_fanout, ClauseShape, Plan, Planner, VarId};
use std::collections::{BTreeMap, BTreeSet};

/// The conflict budget of the refinement that follows every added clause
const ONLINE_BUDGET: u64 = 1000;
/// The conflict budget of planning the body of a `not` or the branch of an `or` (once per run)
const BODY_BUDGET: u64 = 1000;
/// The estimated fraction of the rows that a `not` keeps
const NOT_SELECTIVITY: f64 = 0.5;

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

fn var_of(t: &Term) -> Option<VarId>
{
	match t
	{
		Term::Var(x) => Some(*x),
		Term::Val(_) => None,
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
	/// The rows for which the body has no solution (see the module documentation); built by `Query::not`
	Not
	{
		join: Vec<VarId>, body: Vec<Clause>
	},
	/// The union of the branches' solutions on the join variables (see the module documentation); built by `Query::or`
	Or
	{
		join: Vec<VarId>, branches: Vec<Vec<Clause>>
	},
}

impl Clause
{
	pub fn pred(l: impl Into<Term>, op: Cmp, r: impl Into<Term>) -> Clause
	{
		Clause::Pred { op, l: l.into(), r: r.into() }
	}

	// The variables the clause shares with the clauses around it: all of a pattern's or a predicate's, the join variables of not and or
	fn free_vars(&self, out: &mut BTreeSet<VarId>)
	{
		match self
		{
			Clause::Pattern { e, v, tx, added, .. } => out.extend([Some(e), Some(v), tx.as_ref(), added.as_ref()].into_iter().flatten().filter_map(var_of)),
			Clause::Pred { l, r, .. } => out.extend([l, r].into_iter().filter_map(var_of)),
			Clause::Not { join, .. } | Clause::Or { join, .. } => out.extend(join),
		}
	}

	// Every variable the clause mentions, its bodies included
	fn all_vars(&self, out: &mut BTreeSet<VarId>)
	{
		self.free_vars(out);
		match self
		{
			Clause::Not { body, .. } => body.iter().for_each(|c| c.all_vars(out)),
			Clause::Or { branches, .. } => branches.iter().flatten().for_each(|c| c.all_vars(out)),
			_ => {}
		}
	}

	/// The variables the clause needs bound before it runs, and those it binds (one of them is empty)
	fn needs_binds(&self) -> (Vec<VarId>, Vec<VarId>)
	{
		match self
		{
			Clause::Pattern { .. } =>
			{
				let mut vs = BTreeSet::new();
				self.free_vars(&mut vs);
				(vec![], vs.into_iter().collect())
			}
			Clause::Pred { l, r, .. } => ([l, r].into_iter().filter_map(var_of).collect(), vec![]),
			Clause::Not { join, .. } => (join.clone(), vec![]),
			Clause::Or { join, branches } =>
			{
				let binder = branches.iter().all(|b| unbound_need(b, &BTreeSet::new()).is_none() && join.iter().all(|x| body_binds(b).contains(x)));
				if binder { (vec![], join.clone()) } else { (join.clone(), vec![]) }
			}
		}
	}
}

// The variables that the clauses of a body bind
fn body_binds(body: &[Clause]) -> BTreeSet<VarId>
{
	body.iter().flat_map(|c| c.needs_binds().1).collect()
}

// A variable that a clause of the body needs, but that neither `pre` nor the body binds
fn unbound_need(body: &[Clause], pre: &BTreeSet<VarId>) -> Option<VarId>
{
	let binds = body_binds(body);
	body.iter().flat_map(|c| c.needs_binds().0).find(|x| !pre.contains(x) && !binds.contains(x))
}

/// The planner's weights of a pattern, from the attribute's statistics: the fanout is the number of datoms matching per input row
fn pattern_weights(s: AttrStats) -> [u32; 4]
{
	let d = s.datoms as f64;
	let (e, v) = (s.entities.max(1) as f64, s.values.max(1) as f64);
	monotone([d, d / e, d / v, d / (e * v)].map(weight_of_fanout))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agg
{
	Count,
	CountDistinct,
	Sum,
	Min,
	Max,
	/// The integer mean, rounded down
	Avg,
}

impl Agg
{
	/// The aggregate's name in the text syntax
	pub fn name(self) -> &'static str
	{
		match self
		{
			Agg::Count => "count",
			Agg::CountDistinct => "count-distinct",
			Agg::Sum => "sum",
			Agg::Min => "min",
			Agg::Max => "max",
			Agg::Avg => "avg",
		}
	}

	pub fn from_name(name: &str) -> Option<Agg>
	{
		[Agg::Count, Agg::CountDistinct, Agg::Sum, Agg::Min, Agg::Max, Agg::Avg].into_iter().find(|a| a.name() == name)
	}

	// The aggregate of a group's column (never empty: a group has at least one tuple)
	fn apply(self, vals: &[&Value]) -> Result<Value, Error>
	{
		let ints = || -> Result<i128, Error> {
			let mut s: i128 = 0;
			for v in vals
			{
				match v
				{
					Value::Int(i) => s += *i as i128,
					v => return Err(Error::InvalidQuery(format!("{} of the non-integer value {v:?}", self.name()))),
				}
			}
			Ok(s)
		};
		Ok(match self
		{
			Agg::Count => Value::Int(vals.len() as i64),
			Agg::CountDistinct => Value::Int(vals.iter().collect::<BTreeSet<_>>().len() as i64),
			Agg::Min => (*vals.iter().min().expect("a group is not empty")).clone(),
			Agg::Max => (*vals.iter().max().expect("a group is not empty")).clone(),
			Agg::Sum => Value::Int(i64::try_from(ints()?).map_err(|_| Error::InvalidQuery("the sum overflows an Int".into()))?),
			// The mean of Ints lies between them, so it fits
			Agg::Avg => Value::Int(ints()?.div_euclid(vals.len() as i128) as i64),
		})
	}
}

/// An element of a find spec
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Find
{
	Term(Term),
	Agg(Agg, Term),
}

/// A clause of a run, with the bodies of `not` and `or` ordered for the variables bound where they run
enum Step<'a>
{
	/// A pattern or a predicate
	Clause(&'a Clause),
	Not
	{
		join: &'a [VarId], body: Vec<Step<'a>>
	},
	Or
	{
		join: &'a [VarId], branches: Vec<Vec<Step<'a>>>
	},
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

	/// The name of variable `x` of this query
	pub fn var_name(&self, x: VarId) -> &str
	{
		&self.vars[x]
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
		let c = self.pattern_clause(e, attr, v)?;
		self.add_clause(c)
	}

	/// Add the pattern [e attr v tx added], which also matches the datom's transaction entity and its added flag
	/// (false for a retraction, which only a history view has); a variable used nowhere else ignores a position.
	/// Returns the clause's index
	pub fn pattern_tx(&mut self, e: impl Into<Term>, attr: &str, v: impl Into<Term>, tx: impl Into<Term>, added: impl Into<Term>) -> Result<usize, Error>
	{
		let c = self.pattern_tx_clause(e, attr, v, tx, added)?;
		self.add_clause(c)
	}

	/// The pattern [e attr v], validated but not added (for the body of a `not` or an `or`)
	pub fn pattern_clause(&self, e: impl Into<Term>, attr: &str, v: impl Into<Term>) -> Result<Clause, Error>
	{
		self.make_pattern(e.into(), attr, v.into(), None, None)
	}

	/// The pattern [e attr v tx added], validated but not added
	pub fn pattern_tx_clause(&self, e: impl Into<Term>, attr: &str, v: impl Into<Term>, tx: impl Into<Term>, added: impl Into<Term>) -> Result<Clause, Error>
	{
		self.make_pattern(e.into(), attr, v.into(), Some(tx.into()), Some(added.into()))
	}

	fn make_pattern(&self, e: Term, attr: &str, v: Term, tx: Option<Term>, added: Option<Term>) -> Result<Clause, Error>
	{
		let a = self.db.attr_id(attr).ok_or_else(|| Error::UnknownAttr(attr.to_string()))?;
		let c = Clause::Pattern { e, a, v, tx, added };
		self.check(&c)?;
		Ok(c)
	}

	/// Add the predicate `l op r`; its variables must be bound by other clauses. Returns the clause's index
	pub fn pred(&mut self, l: impl Into<Term>, op: Cmp, r: impl Into<Term>) -> Result<usize, Error>
	{
		self.add_clause(Clause::pred(l, op, r))
	}

	fn join_vars(join: &[Term]) -> Result<Vec<VarId>, Error>
	{
		let mut out: Vec<VarId> = join.iter().map(|t| var_of(t).ok_or_else(|| Error::InvalidQuery(format!("the join list has the constant {t:?}")))).collect::<Result<_, _>>()?;
		out.sort_unstable();
		out.dedup();
		Ok(out)
	}

	/// The clause `not` (`join` None: every variable of the body is a join variable) or `not-join` (the variables of `join`), validated but not added
	pub fn not(&self, join: Option<&[Term]>, body: Vec<Clause>) -> Result<Clause, Error>
	{
		let join = match join
		{
			Some(j) => Self::join_vars(j)?,
			None =>
			{
				let mut vs = BTreeSet::new();
				body.iter().for_each(|c| c.free_vars(&mut vs));
				vs.into_iter().collect()
			}
		};
		let c = Clause::Not { join, body };
		self.check(&c)?;
		Ok(c)
	}

	/// The clause `or` (`join` None: the join variables are the branches' variables, which must be the same in every branch)
	/// or `or-join` (the variables of `join`), validated but not added
	pub fn or(&self, join: Option<&[Term]>, branches: Vec<Vec<Clause>>) -> Result<Clause, Error>
	{
		let join = match join
		{
			Some(j) => Self::join_vars(j)?,
			None =>
			{
				let sets: Vec<BTreeSet<VarId>> = branches
					.iter()
					.map(|b| {
						let mut vs = BTreeSet::new();
						b.iter().for_each(|c| c.free_vars(&mut vs));
						vs
					})
					.collect();
				if let Some(i) = sets.iter().position(|s| *s != sets[0])
				{
					let names = |s: &BTreeSet<VarId>| s.iter().map(|&x| self.vars.get(x).map_or("?", |n| n.as_str())).collect::<Vec<_>>().join(" ");
					return Err(Error::InvalidQuery(format!("the branches of or use different variables: [{}] and [{}] (or-join names the shared ones)", names(&sets[0]), names(&sets[i]))));
				}
				sets.into_iter().next().unwrap_or_default().into_iter().collect()
			}
		};
		let c = Clause::Or { join, branches };
		self.check(&c)?;
		Ok(c)
	}

	// Validate a clause, its bodies included: its variables are this query's, its patterns are well-typed, its bodies aren't empty,
	// and a body's needs are bound by its join variables or its own clauses
	fn check(&self, c: &Clause) -> Result<(), Error>
	{
		let mut vs = BTreeSet::new();
		c.all_vars(&mut vs);
		if let Some(x) = vs.into_iter().find(|&x| x >= self.vars.len())
		{
			return Err(Error::InvalidQuery(format!("variable #{x} belongs to another query")));
		}
		self.check_clause(c)
	}

	fn check_clause(&self, c: &Clause) -> Result<(), Error>
	{
		match c
		{
			Clause::Pattern { e, a, v, tx, added } =>
			{
				let def = self.db.try_attr(*a).ok_or_else(|| Error::UnknownAttr(format!("#{a}")))?;
				let typed = [(Some(e), ValueType::Ref), (Some(v), def.value_type), (tx.as_ref(), ValueType::Ref), (added.as_ref(), ValueType::Bool)];
				for (t, expected) in typed
				{
					if let Some(Term::Val(x)) = t
					{
						if x.value_type() != expected
						{
							return Err(Error::TypeMismatch { attr: def.name.to_string(), expected, got: x.value_type() });
						}
					}
				}
				Ok(())
			}
			Clause::Pred { .. } => Ok(()),
			Clause::Not { join, body } => self.check_body(body, join, "not"),
			Clause::Or { branches, .. } =>
			{
				if branches.is_empty()
				{
					return Err(Error::InvalidQuery("an or without branches".into()));
				}
				// A binder's branches bind everything by themselves; a filter's have the join variables bound
				let (needs, _) = c.needs_binds();
				branches.iter().try_for_each(|b| self.check_body(b, &needs, "or branch"))
			}
		}
	}

	fn check_body(&self, body: &[Clause], pre: &[VarId], what: &str) -> Result<(), Error>
	{
		if body.is_empty()
		{
			return Err(Error::InvalidQuery(format!("an empty {what}")));
		}
		body.iter().try_for_each(|c| self.check_clause(c))?;
		match unbound_need(body, &pre.iter().copied().collect())
		{
			Some(x) => Err(Error::Unbound(self.vars[x].clone())),
			None => Ok(()),
		}
	}

	/// Validate and add a clause (built by `pattern_clause`, `not`, `or`, `Clause::pred`, ...); returns its index
	pub fn add_clause(&mut self, c: Clause) -> Result<usize, Error>
	{
		self.check(&c)?;
		let shape = self.shape(&c, &vec![false; self.vars.len()]);
		self.clauses.push(c);
		self.planner.add_clause(shape, self.vars.len());
		self.planner.refine(ONLINE_BUDGET);
		Ok(self.clauses.len() - 1)
	}

	// The planner's view of a clause, with the variables of `pre` bound beforehand (they count as constants)
	fn shape(&self, c: &Clause, pre: &[bool]) -> ClauseShape
	{
		let free = |vs: Vec<VarId>| -> Vec<VarId> { vs.into_iter().filter(|&x| !pre[x]).collect() };
		let (needs, binds) = c.needs_binds();
		let (needs, binds) = (free(needs), free(binds));
		match c
		{
			Clause::Pattern { e, a, v, .. } =>
			{
				// The tx and added positions bind their variables, but the cost model only distinguishes the entity and value positions
				let slot = |t: &Term| var_of(t).filter(|&x| !pre[x]);
				ClauseShape { slots: [slot(e), slot(v)], needs, binds, weights: pattern_weights(self.db.stats(*a)) }
			}
			Clause::Pred { op, .. } => ClauseShape { slots: [None, None], needs, binds, weights: [weight_of_fanout(op.selectivity()); 4] },
			Clause::Not { .. } => ClauseShape { slots: [None, None], needs, binds, weights: [weight_of_fanout(NOT_SELECTIVITY); 4] },
			Clause::Or { branches, .. } =>
			{
				// The sum of the branches' estimated sizes, each that of its final result in the greedy order (at most one row per input row for a filter)
				let mut inner = pre.to_vec();
				for &x in &needs
				{
					inner[x] = true;
				}
				let mut rows = 0.0;
				for b in branches
				{
					let shapes: Vec<ClauseShape> = b.iter().map(|c| self.shape(c, &inner)).collect();
					let mut bound = vec![false; pre.len()];
					let mut log2 = 0.0;
					for k in greedy_order(&shapes, pre.len())
					{
						log2 += fanout_of_weight(shapes[k].weights[shapes[k].mode(&bound)]).log2();
						for &x in &shapes[k].binds
						{
							bound[x] = true;
						}
					}
					rows += log2.exp2();
				}
				if binds.is_empty()
				{
					rows = rows.min(1.0);
				}
				ClauseShape { slots: [None, None], needs, binds, weights: [weight_of_fanout(rows); 4] }
			}
		}
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

	/// Optimize the plan and run it with a find spec of plain terms and aggregates (see the module documentation);
	/// returns the rows (one per group), sorted
	pub fn find(&mut self, spec: &[Find], with: &[Term]) -> Result<Vec<Vec<Value>>, Error>
	{
		let keys: Vec<Term> = spec.iter().filter_map(|f| if let Find::Term(t) = f { Some(t.clone()) } else { None }).collect();
		let aggs: Vec<(Agg, Term)> = spec.iter().filter_map(|f| if let Find::Agg(a, t) = f { Some((*a, t.clone())) } else { None }).collect();
		let cols: Vec<Term> = keys.iter().cloned().chain(aggs.iter().map(|a| a.1.clone())).chain(with.iter().cloned()).collect();
		let mut groups: BTreeMap<Vec<Value>, Vec<Vec<Value>>> = BTreeMap::new();
		for mut row in self.run(&cols)?
		{
			let rest = row.split_off(keys.len());
			groups.entry(row).or_default().push(rest);
		}
		let mut out = BTreeSet::new();
		for (key, tuples) in groups
		{
			let (mut key, mut col) = (key.into_iter(), 0);
			let mut row = Vec::with_capacity(spec.len());
			for f in spec
			{
				row.push(match f
				{
					Find::Term(_) => key.next().expect("a key per plain term"),
					Find::Agg(a, _) =>
					{
						let vals: Vec<&Value> = tuples.iter().map(|t| &t[col]).collect();
						col += 1;
						a.apply(&vals)?
					}
				});
			}
			out.insert(row);
		}
		Ok(out.into_iter().collect())
	}

	pub(crate) fn run_in_order(&self, order: &[usize], find: &[Term]) -> Result<Vec<Vec<Value>>, Error>
	{
		let n = self.vars.len();
		if let Some(x) = find.iter().filter_map(var_of).find(|&x| x >= n)
		{
			return Err(Error::InvalidQuery(format!("variable #{x} belongs to another query")));
		}
		let mut bindable = vec![false; n];
		for x in self.clauses.iter().flat_map(|c| c.needs_binds().1)
		{
			bindable[x] = true;
		}
		let needed = find.iter().filter_map(var_of).chain(self.clauses.iter().flat_map(|c| c.needs_binds().0));
		if let Some(x) = needed.into_iter().find(|&x| !bindable[x])
		{
			return Err(Error::Unbound(self.vars[x].clone()));
		}
		let steps = self.compile(&self.clauses, order, &mut vec![false; n])?;
		let mut out = BTreeSet::new();
		self.exec(&steps, &mut vec![None; n], &mut |env| {
			out.insert(find.iter().map(|t| Self::eval(t, env).expect("the find variables are bound by the clauses")).collect());
			true
		});
		Ok(out.into_iter().collect())
	}

	// The steps of running `clauses` in `order`, from the variables of `bound` (which it updates); a clause whose needs aren't bound
	// where it runs is an error
	fn compile<'a>(&'a self, clauses: &'a [Clause], order: &[usize], bound: &mut [bool]) -> Result<Vec<Step<'a>>, Error>
	{
		let mut steps = Vec::with_capacity(order.len());
		for &c in order
		{
			let clause = &clauses[c];
			let (needs, binds) = clause.needs_binds();
			if let Some(&x) = needs.iter().find(|&&x| !bound[x])
			{
				return Err(Error::Unbound(self.vars[x].clone()));
			}
			// A body runs in an environment of its own, from the join variables that are bound here
			let inner = |join: &[VarId]| {
				let mut b = vec![false; bound.len()];
				for &x in join
				{
					b[x] = bound[x];
				}
				b
			};
			steps.push(match clause
			{
				Clause::Pattern { .. } | Clause::Pred { .. } => Step::Clause(clause),
				Clause::Not { join, body } => Step::Not { join, body: self.compile_body(body, inner(join))? },
				Clause::Or { join, branches } =>
				{
					let pre = inner(join);
					Step::Or { join, branches: branches.iter().map(|b| self.compile_body(b, pre.clone())).collect::<Result<_, _>>()? }
				}
			});
			for x in binds
			{
				bound[x] = true;
			}
		}
		Ok(steps)
	}

	// Plan a body with the variables of `bound` as constants (with the SAT planner, within a budget), and compile it
	fn compile_body<'a>(&'a self, body: &'a [Clause], mut bound: Vec<bool>) -> Result<Vec<Step<'a>>, Error>
	{
		let mut planner = Planner::new();
		for c in body
		{
			planner.add_clause(self.shape(c, &bound), bound.len());
		}
		let order = planner.refine(BODY_BUDGET).order.clone();
		self.compile(body, &order, &mut bound)
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

	// The environment of a body: the join variables' values only
	fn inner_env(env: &[Option<Value>], join: &[VarId]) -> Vec<Option<Value>>
	{
		let mut inner = vec![None; env.len()];
		for &x in join
		{
			inner[x] = env[x].clone();
		}
		inner
	}

	// Run the steps from `env`, calling `emit` on every solution; stops as soon as `emit` returns false, and returns false then
	fn exec(&self, steps: &[Step], env: &mut Vec<Option<Value>>, emit: &mut dyn FnMut(&[Option<Value>]) -> bool) -> bool
	{
		let Some((step, rest)) = steps.split_first()
		else
		{
			return emit(env);
		};
		match step
		{
			Step::Clause(Clause::Pattern { e, a, v, tx, added }) =>
			{
				let eid = match Self::eval(e, env)
				{
					Some(Value::Ref(x)) => Some(x),
					// A non-entity value in the entity position matches nothing
					Some(_) => return true,
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
					let go = !ok || self.exec(rest, env, emit);
					for x in newly
					{
						env[x] = None;
					}
					if !go
					{
						return false;
					}
				}
				true
			}
			Step::Clause(Clause::Pred { op, l, r }) =>
			{
				let lv = Self::eval(l, env).expect("compile checked that the predicate's variables are bound");
				let rv = Self::eval(r, env).expect("compile checked that the predicate's variables are bound");
				!op.holds(&lv, &rv) || self.exec(rest, env, emit)
			}
			Step::Clause(_) => unreachable!("compile turns not and or into their own steps"),
			Step::Not { join, body } =>
			{
				// The body has a solution exactly when its run stops at the first one
				let found = !self.exec(body, &mut Self::inner_env(env, join), &mut |_| false);
				found || self.exec(rest, env, emit)
			}
			Step::Or { join, branches } =>
			{
				// With every join variable bound, one solution of one branch decides
				let decided = join.iter().all(|&x| env[x].is_some());
				let mut tuples = BTreeSet::new();
				for b in branches
				{
					self.exec(b, &mut Self::inner_env(env, join), &mut |inner| {
						tuples.insert(join.iter().map(|&x| inner[x].clone().expect("every branch binds the join variables that aren't bound")).collect::<Vec<_>>());
						!decided
					});
					if decided && !tuples.is_empty()
					{
						break;
					}
				}
				for t in tuples
				{
					let mut newly = Vec::new();
					for (&x, val) in join.iter().zip(t)
					{
						let ok = Self::unify(&Term::Var(x), val, env, &mut newly);
						debug_assert!(ok, "a branch keeps the join variables' values");
					}
					let go = self.exec(rest, env, emit);
					for x in newly
					{
						env[x] = None;
					}
					if !go
					{
						return false;
					}
				}
				true
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
		q.pred(age.clone(), Cmp::Gt, 40).unwrap();
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
		q.pred(age.clone(), Cmp::Lt, 10).unwrap();
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

	type Row = BTreeMap<VarId, Value>;

	// The reference semantics, bottom-up over a set of (e, a, v) facts: a body is the natural join of its binders' relations
	// (patterns and binding ors) with the seed, then filtered by its predicates, nots and filtering ors
	fn naive_body(facts: &BTreeSet<(Eid, AttrId, Value)>, body: &[Clause], seed: BTreeSet<Row>) -> BTreeSet<Row>
	{
		let mut cur = seed;
		let binder = |c: &&Clause| matches!(c, Clause::Pattern { .. } | Clause::Or { .. }) && c.needs_binds().0.is_empty();
		for c in body.iter().filter(binder)
		{
			let rel = naive_rel(facts, c);
			let mut next = BTreeSet::new();
			for a in &cur
			{
				for b in &rel
				{
					if b.iter().all(|(k, v)| a.get(k).map_or(true, |w| w == v))
					{
						next.insert(a.iter().chain(b).map(|(k, v)| (*k, v.clone())).collect());
					}
				}
			}
			cur = next;
		}
		let project = |row: &Row, join: &[VarId]| -> BTreeSet<Row> { [join.iter().map(|x| (*x, row[x].clone())).collect()].into() };
		for c in body.iter().filter(|c| !binder(c))
		{
			cur.retain(|row| match c
			{
				Clause::Pred { op, l, r } =>
				{
					let ev = |t: &Term| match t
					{
						Term::Val(v) => v.clone(),
						Term::Var(x) => row[x].clone(),
					};
					op.holds(&ev(l), &ev(r))
				}
				Clause::Not { join, body } => naive_body(facts, body, project(row, join)).is_empty(),
				Clause::Or { join, branches } => branches.iter().any(|b| !naive_body(facts, b, project(row, join)).is_empty()),
				Clause::Pattern { .. } => unreachable!(),
			});
		}
		cur
	}

	// The relation of a binder over its variables
	fn naive_rel(facts: &BTreeSet<(Eid, AttrId, Value)>, c: &Clause) -> BTreeSet<Row>
	{
		match c
		{
			Clause::Pattern { e, a, v, tx: None, added: None } =>
			{
				let mut out = BTreeSet::new();
				for (fe, _, fv) in facts.iter().filter(|f| f.1 == *a)
				{
					let mut row = Row::new();
					let mut ok = true;
					for (t, val) in [(e, Value::Ref(*fe)), (v, fv.clone())]
					{
						match t
						{
							Term::Val(c) => ok &= *c == val,
							Term::Var(x) => ok &= row.insert(*x, val.clone()).map_or(true, |old| old == val),
						}
					}
					if ok
					{
						out.insert(row);
					}
				}
				out
			}
			Clause::Or { join, branches } => branches
				.iter()
				.flat_map(|b| naive_body(facts, b, [Row::new()].into()))
				.map(|row| join.iter().map(|x| (*x, row[x].clone())).collect())
				.collect(),
			_ => unreachable!("a binder is a pattern (without tx and added) or a binding or"),
		}
	}

	// A random clause over the attributes of `random_conn`, with entity variables ?a ?b ?c, Int variables ?x ?y and the tag variable ?t
	fn random_clause(q: &mut Query, rng: &mut Rng, depth: u32, ids: &[Eid], pattern_only: bool) -> Option<Clause>
	{
		let pick = |q: &mut Query, names: &[&str], rng: &mut Rng| q.var(names[(rng.next() % names.len() as u64) as usize]);
		let kind = if pattern_only || depth == 0 && rng.next() % 4 != 0 { 0 } else { rng.next() % if depth == 0 { 2 } else { 6 } };
		match kind
		{
			// Patterns and ors, twice as likely as predicates and nots
			0 | 4 =>
			{
				let e = pick(q, &["?a", "?b", "?c"], rng);
				let attr = ["n", "tag", "ref", "best", "n", "ref"][(rng.next() % 6) as usize];
				let constant = rng.next() % 3 == 0;
				let v = match attr
				{
					"n" if constant => Term::Val(Value::Int((rng.next() % 4) as i64)),
					"n" => pick(q, &["?x", "?y"], rng),
					"tag" if constant => Term::Val(["a", "b", "c"][(rng.next() % 3) as usize].into()),
					"tag" => q.var("?t"),
					_ if constant => Term::Val(Value::Ref(ids[(rng.next() % ids.len() as u64) as usize])),
					_ => pick(q, &["?a", "?b", "?c"], rng),
				};
				q.pattern_clause(e, attr, v).ok()
			}
			1 if rng.next() % 3 == 0 =>
			{
				let (l, r) = (pick(q, &["?a", "?b", "?c"], rng), pick(q, &["?a", "?b", "?c"], rng));
				Some(Clause::pred(l, Cmp::Ne, r))
			}
			1 =>
			{
				let op = [Cmp::Eq, Cmp::Ne, Cmp::Lt, Cmp::Le, Cmp::Gt, Cmp::Ge][(rng.next() % 6) as usize];
				Some(Clause::pred(pick(q, &["?x", "?y"], rng), op, (rng.next() % 4) as i64))
			}
			_ =>
			{
				// A body or branch starts with a pattern
				let body = |q: &mut Query, rng: &mut Rng| -> Option<Vec<Clause>> {
					let mut b = vec![random_clause(q, rng, depth - 1, ids, true)?];
					for _ in 0..rng.next() % 2
					{
						b.push(random_clause(q, rng, depth - 1, ids, false)?);
					}
					Some(b)
				};
				let first = body(q, rng)?;
				let mut vs = BTreeSet::new();
				first.iter().for_each(|c| c.free_vars(&mut vs));
				let join: Option<Vec<Term>> = (rng.next() % 4 != 0).then(|| vs.iter().filter(|_| rng.next() % 2 == 0).map(|&x| Term::Var(x)).collect());
				if kind == 2
				{
					q.not(join.as_deref(), first).ok()
				}
				else
				{
					let second = body(q, rng)?;
					q.or(join.as_deref(), vec![first, second]).ok()
				}
			}
		}
	}

	// Random queries with not, not-join, filtering and binding or (nested too), in every clause order, against the bottom-up reference
	#[test]
	fn not_and_or_match_a_naive_evaluator()
	{
		use crate::db::tests::{random_conn, replay};
		let (conn, reports) = random_conn(53, 60);
		let db = conn.db();
		let log: Vec<_> = reports.iter().flat_map(|r| &r.tx_data).collect();
		let facts: BTreeSet<(Eid, AttrId, Value)> = replay(log.iter().copied()).into_keys().collect();
		let ids: Vec<Eid> = facts.iter().map(|f| f.0).collect::<BTreeSet<_>>().into_iter().collect();
		let mut rng = Rng::new(17);
		// Queries run, by the kind of clause they contain, with a non-empty answer
		let (mut tested, mut nots, mut filters, mut binders, mut nested) = (0, 0, 0, 0, 0);
		for _ in 0..2000
		{
			let mut q = db.query();
			let mut clauses = Vec::new();
			for _ in 0..2 + rng.next() % 3
			{
				match random_clause(&mut q, &mut rng, 2, &ids, false)
				{
					Some(c) => clauses.push(c),
					None => break,
				}
			}
			if clauses.iter().any(|c| q.add_clause(c.clone()).is_err()) || clauses.len() < 2
			{
				continue;
			}
			let mut find: Vec<VarId> = clauses.iter().flat_map(|c| c.needs_binds().1).collect::<BTreeSet<_>>().into_iter().collect();
			if find.is_empty()
			{
				continue;
			}
			find.truncate(3);
			let terms: Vec<Term> = find.iter().map(|&x| Term::Var(x)).collect();
			let Ok(got) = q.run(&terms)
			else
			{
				continue;
			};
			let want = sorted(naive_body(&facts, &clauses, [Row::new()].into()).iter().map(|row| find.iter().map(|x| row[x].clone()).collect()));
			assert_eq!(got, want, "clauses {clauses:?}");
			let mut valid = 0;
			for order in permutations(clauses.len())
			{
				match q.run_in_order(&order, &terms)
				{
					Ok(r) =>
					{
						assert_eq!(r, got, "order {order:?} of {clauses:?}");
						valid += 1;
					}
					Err(Error::Unbound(_)) => {}
					Err(e) => panic!("{e:?}"),
				}
			}
			assert!(valid > 0);
			if got.is_empty()
			{
				continue;
			}
			tested += 1;
			let mut kinds = Vec::new();
			fn walk(c: &Clause, depth: u32, kinds: &mut Vec<(u32, &'static str)>)
			{
				let (kind, inner): (&'static str, Vec<&Clause>) = match c
				{
					Clause::Not { body, .. } => ("not", body.iter().collect()),
					Clause::Or { branches, .. } => (if c.needs_binds().1.is_empty() { "filter" } else { "binder" }, branches.iter().flatten().collect()),
					_ => return,
				};
				kinds.push((depth, kind));
				inner.into_iter().for_each(|d| walk(d, depth + 1, kinds));
			}
			clauses.iter().for_each(|c| walk(c, 0, &mut kinds));
			nots += kinds.iter().any(|k| k.1 == "not") as usize;
			filters += kinds.iter().any(|k| k.1 == "filter") as usize;
			binders += kinds.iter().any(|k| k.1 == "binder") as usize;
			nested += kinds.iter().any(|k| k.0 > 0) as usize;
		}
		assert!(tested >= 150 && nots >= 30 && filters >= 30 && binders >= 25 && nested >= 25, "too few: {tested} {nots} {filters} {binders} {nested}");
	}

	#[test]
	fn not_and_or_examples()
	{
		let conn = social(30);
		let db = conn.db();
		let id = |n: &str| db.attr_id(n).unwrap();
		let people: Vec<Eid> = db.attr_datoms(id("person/name")).map(|(e, _)| e).collect();
		let city = |e: Eid| db.values(e, id("person/city")).next().unwrap().clone();
		let age = |e: Eid| match db.values(e, id("person/age")).next() { Some(Value::Int(a)) => *a, _ => unreachable!() };
		let follows = |e: Eid| db.values(e, id("person/follows")).map(|v| if let Value::Ref(f) = v { *f } else { unreachable!() }).collect::<Vec<_>>();
		let r = |e: Eid| vec![Value::Ref(e)];

		// A filter that arrives before its binder is pending, like a predicate, and is placed after the binder once there is one
		let mut q = db.query();
		let (p, f) = (q.var("?p"), q.var("?f"));
		let body = vec![q.pattern_clause(f.clone(), "person/city", "Haifa").unwrap(), q.pattern_clause(p.clone(), "person/follows", f.clone()).unwrap()];
		let not = q.not(Some(&[p.clone()]), body).unwrap();
		assert!(matches!(&not, Clause::Not { join, .. } if *join == vec![0]));
		q.add_clause(not).unwrap();
		assert_eq!(q.plan().pending, vec![0]);
		assert_eq!(q.run(&[p.clone()]).err(), Some(Error::Unbound("?p".into())));
		q.pattern(p.clone(), "person/city", "Austin").unwrap();
		assert!(q.plan().pending.is_empty());
		assert_eq!(q.plan().order, vec![1, 0]);
		// The people of Austin who follow nobody in Haifa (the body's ?f is its own)
		let want = sorted(people.iter().filter(|&&e| city(e) == Value::from("Austin") && follows(e).iter().all(|&g| city(g) != Value::from("Haifa"))).map(|&e| r(e)));
		assert!(!want.is_empty() && want.len() < people.len() / 3);
		assert_eq!(q.run(&[p.clone()]).unwrap(), want);
		assert_eq!(q.run_in_order(&[0, 1], &[p.clone()]).err(), Some(Error::Unbound("?p".into())));

		// A binding or as the only binder, and a filtering or (one branch doesn't bind ?p)
		let mut q = db.query();
		let (p, a) = (q.var("?p"), q.var("?a"));
		let young = vec![q.pattern_clause(p.clone(), "person/age", a.clone()).unwrap(), Clause::pred(a.clone(), Cmp::Lt, 20)];
		let haifa = vec![q.pattern_clause(p.clone(), "person/city", "Haifa").unwrap()];
		let or = q.or(Some(&[p.clone()]), vec![young.clone(), haifa.clone()]).unwrap();
		assert_eq!(or.needs_binds(), (vec![], vec![0]));
		q.add_clause(or).unwrap();
		let want = sorted(people.iter().filter(|&&e| age(e) < 20 || city(e) == Value::from("Haifa")).map(|&e| r(e)));
		assert_eq!(q.run(&[p.clone()]).unwrap(), want);
		let mut q = db.query();
		let (p, a) = (q.var("?p"), q.var("?a"));
		let filter = q.or(Some(&[p.clone()]), vec![vec![Clause::pred(p.clone(), Cmp::Eq, Value::Ref(people[3]))], haifa.clone()]).unwrap();
		assert_eq!(filter.needs_binds(), (vec![0], vec![]));
		q.add_clause(filter).unwrap();
		q.pattern(p.clone(), "person/age", a.clone()).unwrap();
		assert_eq!(q.plan().order, vec![1, 0]);
		let want = sorted(people.iter().filter(|&&e| e == people[3] || city(e) == Value::from("Haifa")).map(|&e| r(e)));
		assert_eq!(q.run(&[p.clone()]).unwrap(), want);

		// Validation
		let mut q = db.query();
		let (p, a, n, other) = (q.var("?p"), q.var("?a"), q.var("?n"), Term::Var(7));
		let name = q.pattern_clause(p.clone(), "person/name", n.clone()).unwrap();
		let aged = q.pattern_clause(p.clone(), "person/age", a.clone()).unwrap();
		assert!(matches!(q.or(None, vec![vec![name.clone()], vec![aged.clone()]]), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.not(None, vec![]), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.or(None, vec![]), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.or(Some(&[p.clone()]), vec![vec![]]), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.not(Some(&[Term::from(1)]), vec![aged.clone()]), Err(Error::InvalidQuery(_))));
		// A body's predicate on a variable that only the query outside binds is unbound in the body
		assert_eq!(q.not(Some(&[p.clone()]), vec![name.clone(), Clause::pred(a.clone(), Cmp::Gt, 3)]).err(), Some(Error::Unbound("?a".into())));
		assert!(matches!(q.pred(other.clone(), Cmp::Gt, 3), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.pattern(other.clone(), "person/age", 3), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.run(&[Term::Var(99)]), Err(Error::InvalidQuery(_))));
		assert!(q.clauses().is_empty());
	}

	// Aggregates against a hand grouping of the plain answer
	#[test]
	fn aggregates_match_a_grouping()
	{
		use crate::db::tests::random_conn;
		// The ages of the people followed from each city, apart by the followed person (?f) or not
		let conn = social(60);
		let db = conn.db();
		let mut q = db.query();
		let (c, p, f, v) = (q.var("?c"), q.var("?p"), q.var("?f"), q.var("?v"));
		q.pattern(p.clone(), "person/city", c.clone()).unwrap();
		q.pattern(p.clone(), "person/follows", f.clone()).unwrap();
		q.pattern(f.clone(), "person/age", v.clone()).unwrap();
		let e = c;
		let rows = q.run(&[e.clone(), v.clone(), f.clone()]).unwrap();
		let aggs = [Agg::Count, Agg::CountDistinct, Agg::Sum, Agg::Min, Agg::Max, Agg::Avg];
		let spec: Vec<Find> = [Find::Term(e.clone())].into_iter().chain(aggs.iter().map(|&a| Find::Agg(a, v.clone()))).collect();
		let mut distinct_count_differs = false;
		for with in [vec![f.clone()], vec![]]
		{
			// The tuples are distinct over [?e ?v] plus the with variables
			let tuples: BTreeSet<Vec<Value>> = rows.iter().map(|r| r[..2 + with.len()].to_vec()).collect();
			let mut groups: BTreeMap<Value, Vec<i64>> = BTreeMap::new();
			for t in &tuples
			{
				let Value::Int(n) = t[1] else { unreachable!() };
				groups.entry(t[0].clone()).or_default().push(n);
			}
			let want: Vec<Vec<Value>> = groups
				.iter()
				.map(|(k, ns)| {
					let distinct = ns.iter().collect::<BTreeSet<_>>().len() as i64;
					distinct_count_differs |= distinct != ns.len() as i64;
					let sum: i64 = ns.iter().sum();
					[k.clone()].into_iter().chain([ns.len() as i64, distinct, sum, *ns.iter().min().unwrap(), *ns.iter().max().unwrap(), sum.div_euclid(ns.len() as i64)].map(Value::Int)).collect()
				})
				.collect();
			assert_eq!(want.len(), 3);
			assert_eq!(q.find(&spec, &with).unwrap(), want, "with {with:?}");
		}
		assert!(distinct_count_differs, "the with variable should keep equal values apart");
		// Without a plain term: one group, or no row at all
		assert_eq!(q.find(&[Find::Agg(Agg::Count, f.clone())], &[]).unwrap(), vec![vec![Value::Int(rows.iter().map(|r| &r[2]).collect::<BTreeSet<_>>().len() as i64)]]);
		let (conn, _) = random_conn(61, 60);
		let db = conn.db();
		let mut empty = db.query();
		let x = empty.var("?x");
		empty.pattern(x.clone(), "n", 999).unwrap();
		assert!(empty.find(&[Find::Agg(Agg::Count, x.clone())], &[]).unwrap().is_empty());
		// Aggregates in any position, of any type for min and max; sum and avg take Ints only
		let mut q = db.query();
		let (e, t) = (q.var("?e"), q.var("?t"));
		q.pattern(e.clone(), "tag", t.clone()).unwrap();
		let pairs = q.run(&[e.clone(), t.clone()]).unwrap();
		let mut want: BTreeMap<Value, (Value, i64)> = BTreeMap::new();
		for p in &pairs
		{
			let w = want.entry(p[0].clone()).or_insert((p[1].clone(), 0));
			w.0 = w.0.clone().max(p[1].clone());
			w.1 += 1;
		}
		let want: Vec<Vec<Value>> = sorted(want.into_iter().map(|(k, (m, n))| vec![m, k, Value::Int(n)]));
		assert_eq!(q.find(&[Find::Agg(Agg::Max, t.clone()), Find::Term(e.clone()), Find::Agg(Agg::Count, t.clone())], &[]).unwrap(), want);
		assert!(matches!(q.find(&[Find::Agg(Agg::Sum, t.clone())], &[]), Err(Error::InvalidQuery(_))));
		assert_eq!(Agg::Avg.apply(&[&Value::Int(-1), &Value::Int(0)]), Ok(Value::Int(-1)));
		assert!(matches!(Agg::Sum.apply(&[&Value::Int(i64::MAX), &Value::Int(1)]), Err(Error::InvalidQuery(_))));
		assert_eq!(Agg::Sum.apply(&[&Value::Int(i64::MAX), &Value::Int(1), &Value::Int(-2)]), Ok(Value::Int(i64::MAX - 1)));
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
