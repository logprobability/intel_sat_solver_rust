//! Datalog rules: named relations defined by clauses, recursion and mutual recursion included, evaluated bottom-up.
//!
//! A rule is defined on a query (`Query::rule`) by a head of distinct variables and a body of clauses; several definitions of one name
//! are a union. A call (`Query::call_clause`, `Query::call`) is a clause that, like a pattern, binds its variable arguments to the tuples
//! of the rule's relation that match its bound arguments and constants. A rule's variables are its own: its body runs in an environment
//! of its own, so the query's variables of the same name never meet it. Every variable of the head must be bound by the body.
//!
//! # Evaluation
//! When a clause that calls rules is added to the query, the rules it depends on are evaluated (materialized) right away, so that
//! the online planner estimates the calls with the relations' real sizes; the rules are fixed from then on.
//! * Strata: the rules that depend on each other (the strongly connected components of the call graph) are evaluated together,
//!   after the rules they call. A call inside a `not` must be to a lower stratum: a rule that depends on itself through `not`
//!   has no stratification and is an error.
//! * Semi-naive evaluation to a fixpoint: round 0 evaluates every definition with the stratum's relations empty. After that, each
//!   *positive occurrence* of a call to the stratum (in the body or in a branch of an `or`, at any depth) gives a variant of its definition
//!   that reads only the tuples that are new since the previous round (the delta) at that occurrence and the whole relations elsewhere:
//!   every new derivation uses a new tuple at some occurrence, so the union of the variants, minus the tuples already known, is the next delta.
//!   The stratum is done when a round finds nothing new.
//! * Plans: each definition's body is planned by the SAT planner once for round 0, and each variant once after round 0, with the
//!   relations' sizes at that point (the delta of round 0 is all of the relation); the plans are reused in every later round.
//! * The relations keep, per combination of bound argument positions that a call has used, a hash index, which grows with the relation.
//!
//! Evaluation is bottom-up: a rule's whole relation is derived, whatever constants its calls have (there is no magic-sets rewriting).

use crate::db::{AttrStats, Error, Value};
use crate::planner::VarId;
use crate::query::{body_binds, unbound_need, Clause, Query, Step, Term};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The conflict budget of planning a rule body (once per stratum and variant)
const RULE_BUDGET: u64 = 10_000;
/// The most arguments of a rule (the positions of an index key are a bit mask)
const MAX_ARITY: usize = 64;

/// One definition of a rule
pub(crate) struct Def
{
	head: Vec<VarId>,
	body: Vec<Clause>,
}

pub(crate) struct Rule
{
	pub(crate) name: String,
	pub(crate) arity: usize,
	/// Empty for a rule that is called but not (yet) defined
	defs: Vec<Def>,
}

/// What evaluating the query's rules took
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RuleStats
{
	/// The strata (groups of mutually recursive rules) evaluated
	pub strata: u64,
	/// The rounds of semi-naive evaluation, over all the strata (round 0 included)
	pub rounds: u64,
	/// The rule bodies planned (one per definition and stratum for round 0, one per variant)
	pub plans: u64,
}

type Index = HashMap<Vec<Value>, Vec<usize>>;

/// The tuples of a rule, with hash indexes by bound positions
#[derive(Default)]
pub(crate) struct Relation
{
	rows: Vec<Vec<Value>>,
	set: HashSet<Vec<Value>>,
	/// By the mask of the bound positions; built when a call first needs it (a scan holds a clone of the `Rc`, not the borrow)
	indexes: RefCell<HashMap<u64, Rc<Index>>>,
	/// Evaluated to its fixpoint
	ready: bool,
}

fn key_of(row: &[Value], mask: u64) -> Vec<Value>
{
	row.iter().enumerate().filter(|(i, _)| mask >> i & 1 == 1).map(|(_, v)| v.clone()).collect()
}

impl Relation
{
	fn insert(&mut self, row: Vec<Value>) -> bool
	{
		if !self.set.insert(row.clone())
		{
			return false;
		}
		let i = self.rows.len();
		for (&mask, index) in self.indexes.get_mut().iter_mut()
		{
			Rc::make_mut(index).entry(key_of(&row, mask)).or_default().push(i);
		}
		self.rows.push(row);
		true
	}

	pub(crate) fn len(&self) -> usize
	{
		self.rows.len()
	}

	/// The planner's statistics, as if the first two positions were a pattern's entity and value
	pub(crate) fn stats(&self) -> AttrStats
	{
		let distinct = |i: usize| self.rows.iter().filter_map(|r| r.get(i)).collect::<HashSet<_>>().len().max(1) as u64;
		AttrStats { datoms: self.rows.len() as u64, entities: distinct(0), values: distinct(1) }
	}

	/// Call `f` on the rows matching the bound positions of `bound`, until it returns false (then returns false)
	pub(crate) fn scan(&self, bound: &[Option<Value>], f: &mut dyn FnMut(&[Value]) -> bool) -> bool
	{
		let mask = bound.iter().enumerate().filter(|(_, b)| b.is_some()).fold(0u64, |m, (i, _)| m | 1 << i);
		if mask == 0
		{
			return self.rows.iter().all(|r| f(r));
		}
		let key: Vec<Value> = bound.iter().flatten().cloned().collect();
		if key.len() == bound.len()
		{
			return !self.set.contains(&key) || f(&key);
		}
		let found = self.indexes.borrow().get(&mask).cloned();
		let index = found.unwrap_or_else(|| {
			let mut index = Index::new();
			for (i, r) in self.rows.iter().enumerate()
			{
				index.entry(key_of(r, mask)).or_default().push(i);
			}
			let index = Rc::new(index);
			self.indexes.borrow_mut().insert(mask, index.clone());
			index
		});
		index.get(&key).map_or(true, |ids| ids.iter().all(|&i| f(&self.rows[i])))
	}
}

/// The relations of a query's rules, by rule id, and what evaluating them took
#[derive(Default)]
pub(crate) struct Rels
{
	pub(crate) full: Vec<Relation>,
	/// The tuples of the latest round, while a stratum is evaluated
	pub(crate) delta: Vec<Relation>,
	stats: RuleStats,
}

/// What planning and compiling see of the rules: the relations, and the one call (by address) that reads its rule's delta
pub(crate) struct Ctx<'r>
{
	pub(crate) rels: &'r Rels,
	pub(crate) delta: Option<&'r Clause>,
}

impl Ctx<'_>
{
	pub(crate) fn is_delta(&self, c: &Clause) -> bool
	{
		self.delta.is_some_and(|d| std::ptr::eq(d, c))
	}

	/// The relation that the call `c` reads
	pub(crate) fn relation(&self, c: &Clause) -> &Relation
	{
		let Clause::Rule { rule, .. } = c
		else
		{
			unreachable!("only a rule call reads a relation")
		};
		if self.is_delta(c) { &self.rels.delta[*rule] } else { &self.rels.full[*rule] }
	}
}

// Call `f` on every rule call of the clause, with whether it is inside a not
fn calls(c: &Clause, negative: bool, f: &mut dyn FnMut(usize, bool))
{
	match c
	{
		Clause::Rule { rule, .. } => f(*rule, negative),
		Clause::Not { body, .. } => body.iter().for_each(|d| calls(d, true, f)),
		Clause::Or { branches, .. } => branches.iter().flatten().for_each(|d| calls(d, negative, f)),
		Clause::Pattern { .. } | Clause::Pred { .. } => {}
	}
}

// The calls to the rules of `stratum` that aren't inside a not (where stratification forbids them)
fn positive_calls<'a>(c: &'a Clause, stratum: &[usize], out: &mut Vec<&'a Clause>)
{
	match c
	{
		Clause::Rule { rule, .. } if stratum.contains(rule) => out.push(c),
		Clause::Or { branches, .. } => branches.iter().flatten().for_each(|d| positive_calls(d, stratum, out)),
		_ => {}
	}
}

impl Query
{
	// The id of rule `name`, declared with `arity` if it is new
	fn rule_id(&mut self, name: &str, arity: usize) -> Result<usize, Error>
	{
		if name.is_empty()
		{
			return Err(Error::InvalidQuery("a rule needs a name".into()));
		}
		if arity > MAX_ARITY
		{
			return Err(Error::InvalidQuery(format!("the rule {name} has {arity} arguments, more than {MAX_ARITY}")));
		}
		match self.rules.iter().position(|r| r.name == name)
		{
			Some(i) if self.rules[i].arity != arity => Err(Error::InvalidQuery(format!("the rule {name} takes {} arguments, got {arity}", self.rules[i].arity))),
			Some(i) => Ok(i),
			None =>
			{
				self.rules.push(Rule { name: name.to_string(), arity, defs: Vec::new() });
				Ok(self.rules.len() - 1)
			}
		}
	}

	/// Define rule `name` (or add a definition to it): the tuples of the `head` variables (distinct) over the solutions of `body`,
	/// which must bind them all (see the module documentation). Rules are defined before any clause of the query calls one
	pub fn rule(&mut self, name: &str, head: &[Term], body: Vec<Clause>) -> Result<(), Error>
	{
		if self.rules_used
		{
			return Err(Error::InvalidQuery(format!("the rule {name} is defined after a clause of the query called a rule (the rules are fixed then)")));
		}
		let mut vars = Vec::with_capacity(head.len());
		for t in head
		{
			match t
			{
				Term::Var(x) if *x >= self.vars.len() => return Err(Error::InvalidQuery(format!("variable #{x} belongs to another query"))),
				Term::Var(x) if vars.contains(x) => return Err(Error::InvalidQuery(format!("the head of rule {name} has {} twice", self.vars[*x]))),
				Term::Var(x) => vars.push(*x),
				Term::Val(v) => return Err(Error::InvalidQuery(format!("the head of rule {name} has the constant {v:?} (a head has variables only)"))),
			}
		}
		if body.is_empty()
		{
			return Err(Error::InvalidQuery(format!("the rule {name} has an empty body")));
		}
		body.iter().try_for_each(|c| self.check(c))?;
		let binds = body_binds(&body);
		if let Some(x) = unbound_need(&body, &BTreeSet::new()).or_else(|| vars.iter().copied().find(|x| !binds.contains(x)))
		{
			return Err(Error::Unbound(self.vars[x].clone()));
		}
		let id = self.rule_id(name, vars.len())?;
		self.rules[id].defs.push(Def { head: vars, body });
		// Relations evaluated for a clause that then failed to be added are out of date
		self.rels = Rels::default();
		Ok(())
	}

	/// The call of rule `name` with `args` (variables or constants), validated but not added (for a rule body, a `not` or an `or`).
	/// A rule that isn't defined yet is declared with this arity (so that rules can call each other)
	pub fn call_clause(&mut self, name: &str, args: &[Term]) -> Result<Clause, Error>
	{
		let rule = self.rule_id(name, args.len())?;
		let c = Clause::Rule { rule, args: args.to_vec() };
		self.check(&c)?;
		Ok(c)
	}

	/// Add the call of rule `name` with `args`; evaluates the rules it depends on. Returns the clause's index
	pub fn call(&mut self, name: &str, args: &[Term]) -> Result<usize, Error>
	{
		let c = self.call_clause(name, args)?;
		self.add_clause(c)
	}

	/// The name of rule number `rule` of this query (as in `Clause::Rule`), if it has one
	pub fn rule_name(&self, rule: usize) -> Option<&str>
	{
		self.rules.get(rule).map(|r| r.name.as_str())
	}

	/// What evaluating the rules took so far
	pub fn rule_stats(&self) -> RuleStats
	{
		self.rels.stats
	}

	/// Evaluate every rule that the clause depends on and that isn't yet; returns whether it calls any rule
	pub(crate) fn materialize(&self, c: &Clause, rels: &mut Rels) -> Result<bool, Error>
	{
		let mut called = BTreeSet::new();
		calls(c, false, &mut |r, _| {
			called.insert(r);
		});
		if called.is_empty()
		{
			return Ok(false);
		}
		let n = self.rules.len();
		rels.full.resize_with(n, Relation::default);
		rels.delta.resize_with(n, Relation::default);
		// The call graph, with whether each call is inside a not
		let deps: Vec<Vec<(usize, bool)>> = self
			.rules
			.iter()
			.map(|r| {
				let mut d = Vec::new();
				r.defs.iter().flat_map(|def| &def.body).for_each(|c| calls(c, false, &mut |s, neg| d.push((s, neg))));
				d
			})
			.collect();
		// The rules reachable from r through one call or more
		let reachable = |r: usize| -> Vec<bool> {
			let mut seen = vec![false; n];
			let mut stack: Vec<usize> = deps[r].iter().map(|d| d.0).collect();
			while let Some(s) = stack.pop()
			{
				if !seen[s]
				{
					seen[s] = true;
					stack.extend(deps[s].iter().map(|d| d.0));
				}
			}
			seen
		};
		let reach: Vec<Vec<bool>> = (0..n).map(reachable).collect();
		let needed: Vec<usize> = (0..n).filter(|&r| called.contains(&r) || called.iter().any(|&c| reach[c][r])).collect();
		for &r in &needed
		{
			if self.rules[r].defs.is_empty()
			{
				return Err(Error::InvalidQuery(format!("the rule {} is called but never defined", self.rules[r].name)));
			}
			if let Some(&(s, _)) = deps[r].iter().find(|&&(s, neg)| neg && reach[s][r])
			{
				return Err(Error::InvalidQuery(format!("the rule {} depends on itself through not (calling {}), so the rules have no stratification", self.rules[r].name, self.rules[s].name)));
			}
		}
		for &r in &called
		{
			self.eval_stratum(r, &deps, &reach, rels)?;
		}
		Ok(true)
	}

	// Evaluate the stratum of rule r, after the strata it calls
	fn eval_stratum(&self, r: usize, deps: &[Vec<(usize, bool)>], reach: &[Vec<bool>], rels: &mut Rels) -> Result<(), Error>
	{
		if rels.full[r].ready
		{
			return Ok(());
		}
		let stratum: Vec<usize> = (0..self.rules.len()).filter(|&s| s == r || reach[r][s] && reach[s][r]).collect();
		for &m in &stratum
		{
			for &(s, _) in &deps[m]
			{
				if !stratum.contains(&s)
				{
					self.eval_stratum(s, deps, reach, rels)?;
				}
			}
		}
		self.eval_fixpoint(&stratum, rels)
	}

	// Semi-naive evaluation of a stratum whose lower strata are evaluated (see the module documentation)
	fn eval_fixpoint(&self, stratum: &[usize], rels: &mut Rels) -> Result<(), Error>
	{
		for &r in stratum
		{
			rels.full[r] = Relation::default();
			rels.delta[r] = Relation::default();
		}
		let defs: Vec<(usize, &Def)> = stratum.iter().flat_map(|&r| self.rules[r].defs.iter().map(move |d| (r, d))).collect();
		let mut found: HashMap<usize, HashSet<Vec<Value>>> = HashMap::new();

		// Round 0, with the stratum's relations empty
		for &(r, d) in &defs
		{
			let steps = self.plan_def(d, &Ctx { rels, delta: None })?;
			self.run_def(r, d, &steps, rels, &mut found);
		}
		let (mut plans, mut rounds) = (defs.len() as u64, 1);
		let mut more = Self::install(stratum, rels, &mut found);

		// One variant per positive occurrence of a call to the stratum, planned with the sizes of round 0
		let mut variants: Vec<(usize, &Def, Vec<Step>)> = Vec::new();
		for &(r, d) in &defs
		{
			let mut occurrences = Vec::new();
			d.body.iter().for_each(|c| positive_calls(c, stratum, &mut occurrences));
			for o in occurrences
			{
				variants.push((r, d, self.plan_def(d, &Ctx { rels, delta: Some(o) })?));
			}
		}
		plans += variants.len() as u64;
		while more && !variants.is_empty()
		{
			for (r, d, steps) in &variants
			{
				self.run_def(*r, d, steps, rels, &mut found);
			}
			rounds += 1;
			more = Self::install(stratum, rels, &mut found);
		}
		for &r in stratum
		{
			rels.full[r].ready = true;
			rels.delta[r] = Relation::default();
		}
		rels.stats.strata += 1;
		rels.stats.rounds += rounds;
		rels.stats.plans += plans;
		Ok(())
	}

	// Plan a definition's body with the SAT planner and compile it
	fn plan_def<'a>(&'a self, d: &'a Def, ctx: &Ctx) -> Result<Vec<Step<'a>>, Error>
	{
		self.compile_body(&d.body, vec![false; self.vars.len()], ctx, RULE_BUDGET)
	}

	// Run a definition's steps, collecting the head tuples that rule r doesn't have yet
	fn run_def(&self, r: usize, d: &Def, steps: &[Step], rels: &Rels, found: &mut HashMap<usize, HashSet<Vec<Value>>>)
	{
		let out = found.entry(r).or_default();
		let full = &rels.full[r];
		self.exec(steps, &mut vec![None; self.vars.len()], rels, &mut |env| {
			let t: Vec<Value> = d.head.iter().map(|&x| env[x].clone().expect("the body binds the head")).collect();
			if !full.set.contains(&t)
			{
				out.insert(t);
			}
			true
		});
	}

	// Make the tuples found in a round the delta, and add them to the relations; returns whether there were any
	fn install(stratum: &[usize], rels: &mut Rels, found: &mut HashMap<usize, HashSet<Vec<Value>>>) -> bool
	{
		let mut any = false;
		for &r in stratum
		{
			let mut delta = Relation::default();
			for t in found.remove(&r).unwrap_or_default()
			{
				rels.full[r].insert(t.clone());
				delta.insert(t);
			}
			any |= delta.len() > 0;
			rels.delta[r] = delta;
		}
		any
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::db::{Attr, AttrId, Cardinality, Conn, Eid, Entity, Tx, Unique, ValueType};
	use crate::query::tests::{naive_body, random_clause, sorted, Row};
	use crate::query::Cmp;
	use crate::testutil::Rng;

	// A graph of n nodes: node i has node/id i, a weight node/w and a node/edge to the target of each of its edges
	fn graph(n: usize, edges: &[(usize, usize)], rng: &mut Rng) -> (Conn, Vec<Eid>, Vec<i64>)
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define_attr(Attr::new("node/id", ValueType::Int, Cardinality::One).unique(Unique::Identity))
			.define("node/w", ValueType::Int, Cardinality::One)
			.define("node/edge", ValueType::Ref, Cardinality::Many);
		let nodes: Vec<Entity> = (0..n).map(|_| tx.temp()).collect();
		let w: Vec<i64> = (0..n).map(|_| (rng.next() % 10) as i64).collect();
		for (i, &e) in nodes.iter().enumerate()
		{
			tx.add(e, "node/id", i as i64).add(e, "node/w", w[i]);
		}
		for &(a, b) in edges
		{
			tx.add(nodes[a], "node/edge", nodes[b]);
		}
		let rep = conn.transact(&tx).unwrap();
		let ids = nodes.iter().map(|&e| rep.eid(e)).collect();
		(conn, ids, w)
	}

	// Each ordered pair (self-loops included) with probability `percent`
	fn random_edges(n: usize, percent: u64, rng: &mut Rng) -> Vec<(usize, usize)>
	{
		(0..n).flat_map(|a| (0..n).map(move |b| (a, b))).filter(|_| rng.next() % 100 < percent).collect()
	}

	// Floyd-Warshall: reach[a][b] when a path of one edge or more leads from a to b
	fn closure(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<bool>>
	{
		let mut reach = vec![vec![false; n]; n];
		for &(a, b) in edges
		{
			reach[a][b] = true;
		}
		for k in 0..n
		{
			for a in 0..n
			{
				for b in 0..n
				{
					reach[a][b] |= reach[a][k] && reach[k][b];
				}
			}
		}
		reach
	}

	fn pairs(n: usize, ids: &[Eid], keep: impl Fn(usize, usize) -> bool) -> Vec<Vec<Value>>
	{
		sorted((0..n).flat_map(|a| (0..n).map(move |b| (a, b))).filter(|&(a, b)| keep(a, b)).map(|(a, b)| vec![Value::Ref(ids[a]), Value::Ref(ids[b])]))
	}

	#[derive(Clone, Copy, Debug, PartialEq)]
	enum Form
	{
		/// reach(x, y) :- edge(x, z), reach(z, y)
		Right,
		/// reach(x, y) :- reach(x, z), edge(z, y)
		Left,
		/// reach(x, y) :- reach(x, z), reach(z, y)
		Double,
		/// One definition: reach(x, y) :- (or edge(x, y) (and edge(x, z) reach(z, y)))
		Or,
	}

	const FORMS: [Form; 4] = [Form::Right, Form::Left, Form::Double, Form::Or];

	// The transitive closure of node/edge as the rule reach, in the given form
	fn define_reach(q: &mut Query, form: Form)
	{
		let (x, y, z) = (q.var("?x"), q.var("?y"), q.var("?z"));
		let edge = |q: &Query, a: &Term, b: &Term| q.pattern_clause(a.clone(), "node/edge", b.clone()).unwrap();
		let call = |q: &mut Query, a: &Term, b: &Term| q.call_clause("reach", &[a.clone(), b.clone()]).unwrap();
		if form == Form::Or
		{
			let rec = vec![edge(q, &x, &z), call(q, &z, &y)];
			let or = q.or(Some(&[x.clone(), y.clone()]), vec![vec![edge(q, &x, &y)], rec]).unwrap();
			q.rule("reach", &[x, y], vec![or]).unwrap();
			return;
		}
		let body = match form
		{
			Form::Right => vec![edge(q, &x, &z), call(q, &z, &y)],
			Form::Left => vec![call(q, &x, &z), edge(q, &z, &y)],
			_ => vec![call(q, &x, &z), call(q, &z, &y)],
		};
		q.rule("reach", &[x.clone(), y.clone()], body).unwrap();
		let base = vec![edge(q, &x, &y)];
		q.rule("reach", &[x, y], base).unwrap();
	}

	// Transitive closure in four forms on random graphs (cycles and self-loops included), against Floyd-Warshall:
	// all pairs, from a constant source with a predicate on the target, and the nodes on a cycle
	#[test]
	fn transitive_closure_matches_floyd_warshall()
	{
		let mut rng = Rng::new(3);
		let mut max_rounds = 0;
		for k in 0..24
		{
			let n = 1 + k % 12 + if k >= 20 { 14 } else { 0 };
			let edges = random_edges(n, [4, 10, 20, 40][k % 4], &mut rng);
			let (conn, ids, w) = graph(n, &edges, &mut rng);
			let db = conn.db();
			let reach = closure(n, &edges);
			let src = (rng.next() % n as u64) as usize;
			for form in FORMS
			{
				let mut q = db.query();
				define_reach(&mut q, form);
				let (a, b) = (q.var("?a"), q.var("?b"));
				q.call("reach", &[a.clone(), b.clone()]).unwrap();
				assert_eq!(q.run(&[a.clone(), b.clone()]).unwrap(), pairs(n, &ids, |a, b| reach[a][b]), "{form:?} {edges:?}");
				let stats = q.rule_stats();
				assert_eq!(stats.strata, 1);
				max_rounds = max_rounds.max(stats.rounds);

				let mut q = db.query();
				define_reach(&mut q, form);
				let (b, v) = (q.var("?b"), q.var("?v"));
				q.pred(v.clone(), Cmp::Gt, 4).unwrap();
				q.call("reach", &[Value::Ref(ids[src]).into(), b.clone()]).unwrap();
				q.pattern(b.clone(), "node/w", v.clone()).unwrap();
				let want = sorted((0..n).filter(|&j| reach[src][j] && w[j] > 4).map(|j| vec![Value::Ref(ids[j])]));
				assert_eq!(q.run(&[b.clone()]).unwrap(), want, "{form:?} from {src} {edges:?}");

				let mut q = db.query();
				define_reach(&mut q, form);
				let a = q.var("?a");
				q.call("reach", &[a.clone(), a.clone()]).unwrap();
				assert_eq!(q.run(&[a.clone()]).unwrap(), sorted((0..n).filter(|&i| reach[i][i]).map(|i| vec![Value::Ref(ids[i])])));
			}
		}
		assert!(max_rounds >= 6, "too few rounds: {max_rounds}");
	}

	// even and odd call each other: walks of even (at least 2) and odd length, against a search over (node, parity)
	#[test]
	fn mutual_recursion_matches_a_parity_search()
	{
		let mut rng = Rng::new(7);
		for k in 0..16
		{
			let n = 2 + k % 9;
			let edges = random_edges(n, [8, 15, 30][k % 3], &mut rng);
			let (conn, ids, _) = graph(n, &edges, &mut rng);
			let db = conn.db();
			// walk[s][t][p]: a walk of one edge or more, of parity p, from s to t
			let mut walk = vec![vec![[false; 2]; n]; n];
			for s in 0..n
			{
				let mut stack: Vec<(usize, usize)> = edges.iter().filter(|e| e.0 == s).map(|e| (e.1, 1)).collect();
				while let Some((t, p)) = stack.pop()
				{
					if !walk[s][t][p]
					{
						walk[s][t][p] = true;
						stack.extend(edges.iter().filter(|e| e.0 == t).map(|e| (e.1, 1 - p)));
					}
				}
			}
			let define = |q: &mut Query, even_first: bool| {
				let (x, y, z) = (q.var("?x"), q.var("?y"), q.var("?z"));
				let edge = |q: &Query, a: &Term, b: &Term| q.pattern_clause(a.clone(), "node/edge", b.clone()).unwrap();
				let to_even = q.call_clause("even", &[z.clone(), y.clone()]).unwrap();
				let to_odd = q.call_clause("odd", &[z.clone(), y.clone()]).unwrap();
				let mut defs = vec![("odd", vec![edge(q, &x, &y)]), ("odd", vec![edge(q, &x, &z), to_even]), ("even", vec![edge(q, &x, &z), to_odd])];
				if even_first
				{
					defs.reverse();
				}
				for (name, body) in defs
				{
					q.rule(name, &[x.clone(), y.clone()], body).unwrap();
				}
			};
			for (name, p) in [("odd", 1), ("even", 0)]
			{
				let mut q = db.query();
				define(&mut q, k % 2 == 0);
				let (a, b) = (q.var("?a"), q.var("?b"));
				q.call(name, &[a.clone(), b.clone()]).unwrap();
				assert_eq!(q.run(&[a.clone(), b.clone()]).unwrap(), pairs(n, &ids, |a, b| walk[a][b][p]), "{name} {edges:?}");
				// The two rules are one stratum
				assert_eq!(q.rule_stats().strata, 1);
			}
			// Even but not odd, with a not of a rule in the query
			let mut q = db.query();
			define(&mut q, false);
			let (a, b) = (q.var("?a"), q.var("?b"));
			let odd = q.call_clause("odd", &[a.clone(), b.clone()]).unwrap();
			let not = q.not(None, vec![odd]).unwrap();
			q.add_clause(not).unwrap();
			q.call("even", &[a.clone(), b.clone()]).unwrap();
			assert_eq!(q.run(&[a.clone(), b.clone()]).unwrap(), pairs(n, &ids, |a, b| walk[a][b][0] && !walk[a][b][1]));
		}
	}

	// Predicates in rule bodies, a rule of one argument with a constant in its body, and a stratified not of a rule
	#[test]
	fn rules_with_predicates_and_negation()
	{
		let mut rng = Rng::new(13);
		for k in 0..12
		{
			let n = 3 + k % 10;
			let edges = random_edges(n, [10, 25][k % 2], &mut rng);
			let (conn, ids, w) = graph(n, &edges, &mut rng);
			let db = conn.db();
			let reach = closure(n, &edges);

			// light(x, y): a path whose every node after x weighs less than 5
			let light_edges: Vec<(usize, usize)> = edges.iter().copied().filter(|e| w[e.1] < 5).collect();
			let light = closure(n, &light_edges);
			let mut q = db.query();
			let (x, y, z, v) = (q.var("?x"), q.var("?y"), q.var("?z"), q.var("?v"));
			let step = |q: &Query, a: &Term, b: &Term| vec![q.pattern_clause(a.clone(), "node/edge", b.clone()).unwrap(), q.pattern_clause(b.clone(), "node/w", v.clone()).unwrap(), Clause::pred(v.clone(), Cmp::Lt, 5)];
			let base = step(&q, &x, &y);
			q.rule("light", &[x.clone(), y.clone()], base).unwrap();
			let mut rec = step(&q, &z, &y);
			rec.push(q.call_clause("light", &[x.clone(), z.clone()]).unwrap());
			q.rule("light", &[x.clone(), y.clone()], rec).unwrap();
			let (a, b) = (q.var("?a"), q.var("?b"));
			q.call("light", &[a.clone(), b.clone()]).unwrap();
			assert_eq!(q.run(&[a.clone(), b.clone()]).unwrap(), pairs(n, &ids, |a, b| light[a][b]), "{edges:?} {w:?}");

			// from(y): reachable from node 0, named by a lookup of its id
			let mut q = db.query();
			let (y, z) = (q.var("?y"), q.var("?z"));
			let zero = q.lookup("node/id", 0).unwrap();
			let base = vec![q.pattern_clause(zero, "node/edge", y.clone()).unwrap()];
			q.rule("from", &[y.clone()], base).unwrap();
			let rec = vec![q.call_clause("from", &[z.clone()]).unwrap(), q.pattern_clause(z.clone(), "node/edge", y.clone()).unwrap()];
			q.rule("from", &[y.clone()], rec).unwrap();
			let b = q.var("?b");
			q.call("from", &[b.clone()]).unwrap();
			assert_eq!(q.run(&[b.clone()]).unwrap(), sorted((0..n).filter(|&j| reach[0][j]).map(|j| vec![Value::Ref(ids[j])])));

			// apart(x, y): no path from x to y, a not of the lower stratum reach
			let mut q = db.query();
			define_reach(&mut q, Form::Right);
			let (x, y, i, j) = (q.var("?x"), q.var("?y"), q.var("?i"), q.var("?j"));
			let call = q.call_clause("reach", &[x.clone(), y.clone()]).unwrap();
			let body = vec![q.pattern_clause(x.clone(), "node/id", i).unwrap(), q.pattern_clause(y.clone(), "node/id", j).unwrap(), q.not(None, vec![call]).unwrap()];
			q.rule("apart", &[x.clone(), y.clone()], body).unwrap();
			let (a, b) = (q.var("?a"), q.var("?b"));
			q.call("apart", &[a.clone(), b.clone()]).unwrap();
			assert_eq!(q.run(&[a.clone(), b.clone()]).unwrap(), pairs(n, &ids, |a, b| !reach[a][b]));
			assert_eq!(q.rule_stats().strata, 2);
		}
	}

	// A rule that depends on itself through not, directly or through another rule, has no stratification
	#[test]
	fn unstratified_rules_are_refused()
	{
		let mut rng = Rng::new(1);
		let (conn, _, _) = graph(3, &[(0, 1)], &mut rng);
		let db = conn.db();
		let stratification = |r: Result<usize, Error>| matches!(r, Err(Error::InvalidQuery(m)) if m.contains("stratification"));

		let mut q = db.query();
		let (x, i, a) = (q.var("?x"), q.var("?i"), q.var("?a"));
		let node = q.pattern_clause(x.clone(), "node/id", i.clone()).unwrap();
		let me = q.call_clause("p", &[x.clone()]).unwrap();
		let body = vec![node.clone(), q.not(None, vec![me]).unwrap()];
		q.rule("p", &[x.clone()], body).unwrap();
		assert!(stratification(q.call("p", &[a.clone()])));
		assert!(q.clauses().is_empty());

		// p :- not q, q :- p; and the same through an or inside the not
		for in_or in [false, true]
		{
			let mut q = db.query();
			let (x, i, a) = (q.var("?x"), q.var("?i"), q.var("?a"));
			let node = q.pattern_clause(x.clone(), "node/id", i.clone()).unwrap();
			let mut neg = q.call_clause("q", &[x.clone()]).unwrap();
			if in_or
			{
				let w = q.pattern_clause(x.clone(), "node/w", 3).unwrap();
				neg = q.or(None, vec![vec![neg], vec![w]]).unwrap();
			}
			let body = vec![node.clone(), q.not(None, vec![neg]).unwrap()];
			q.rule("p", &[x.clone()], body).unwrap();
			let body = vec![q.call_clause("p", &[x.clone()]).unwrap()];
			q.rule("q", &[x.clone()], body).unwrap();
			assert!(stratification(q.call("q", &[a.clone()])), "in or: {in_or}");
		}

		// A recursive rule may be negated by a higher stratum
		let mut q = db.query();
		define_reach(&mut q, Form::Left);
		let (x, y, i) = (q.var("?x"), q.var("?y"), q.var("?i"));
		let call = q.call_clause("reach", &[x.clone(), y.clone()]).unwrap();
		let body = vec![q.pattern_clause(x.clone(), "node/id", i.clone()).unwrap(), q.pattern_clause(y.clone(), "node/id", 1).unwrap(), q.not(None, vec![call]).unwrap()];
		q.rule("far", &[x.clone()], body).unwrap();
		let a = q.var("?a");
		q.call("far", &[a.clone()]).unwrap();
		assert_eq!(q.run(&[a]).unwrap().len(), 2);
	}

	#[test]
	fn rule_validation()
	{
		let mut rng = Rng::new(1);
		let (conn, _, _) = graph(3, &[(0, 1), (1, 2)], &mut rng);
		let db = conn.db();
		let invalid = |r: Result<(), Error>| matches!(r, Err(Error::InvalidQuery(_)));
		let mut q = db.query();
		let (x, y, z, v) = (q.var("?x"), q.var("?y"), q.var("?z"), q.var("?v"));
		let edge = q.pattern_clause(x.clone(), "node/edge", y.clone()).unwrap();
		// Heads: variables only, distinct, of this query; the body binds them all and isn't empty
		assert!(invalid(q.rule("r", &[x.clone(), Term::from(1)], vec![edge.clone()])));
		assert!(invalid(q.rule("r", &[x.clone(), x.clone()], vec![edge.clone()])));
		assert!(invalid(q.rule("r", &[x.clone(), Term::Var(99)], vec![edge.clone()])));
		assert!(invalid(q.rule("r", &[x.clone()], vec![])));
		assert!(invalid(q.rule("", &[x.clone()], vec![edge.clone()])));
		assert_eq!(q.rule("r", &[x.clone(), z.clone()], vec![edge.clone()]), Err(Error::Unbound("?z".into())));
		assert_eq!(q.rule("r", &[x.clone()], vec![edge.clone(), Clause::pred(v.clone(), Cmp::Gt, 1)]), Err(Error::Unbound("?v".into())));
		// Arities agree between definitions and calls
		q.rule("r", &[x.clone(), y.clone()], vec![edge.clone()]).unwrap();
		assert!(matches!(q.call_clause("r", &[x.clone()]), Err(Error::InvalidQuery(_))));
		assert!(invalid(q.rule("r", &[x.clone()], vec![edge.clone()])));
		assert!(matches!(q.add_clause(Clause::Rule { rule: 0, args: vec![x.clone()] }), Err(Error::InvalidQuery(_))));
		assert!(matches!(q.add_clause(Clause::Rule { rule: 7, args: vec![x.clone()] }), Err(Error::InvalidQuery(_))));
		assert_eq!((q.rule_name(0), q.rule_name(7)), (Some("r"), None));
		// A call of a rule that is never defined fails when it is added, and the rule can still be defined then
		let s = q.call_clause("s", &[x.clone()]).unwrap();
		let t = q.call_clause("t", &[x.clone()]).unwrap();
		q.rule("t", &[x.clone()], vec![s]).unwrap();
		assert!(matches!(q.add_clause(t.clone()), Err(Error::InvalidQuery(m)) if m.contains("never defined")));
		assert!(q.clauses().is_empty());
		let base = vec![q.pattern_clause(x.clone(), "node/id", 2).unwrap()];
		q.rule("s", &[x.clone()], base).unwrap();
		q.add_clause(t).unwrap();
		assert_eq!(q.run(&[x.clone()]).unwrap().len(), 1);
		// The rules are fixed once a clause calls one
		assert!(invalid(q.rule("u", &[x.clone(), y.clone()], vec![edge.clone()])));

		// A rule's variables are its own: the ?y of r is not the query's ?y
		let mut q = db.query();
		let (x, y, a) = (q.var("?x"), q.var("?y"), q.var("?a"));
		let edge = q.pattern_clause(x.clone(), "node/edge", y.clone()).unwrap();
		q.rule("r", &[x.clone(), y.clone()], vec![edge]).unwrap();
		q.pattern(y.clone(), "node/id", 0).unwrap();
		q.call("r", &[a.clone(), x.clone()]).unwrap();
		// The edges (a, x) with any ?y (node 0): 0 -> 1 and 1 -> 2
		assert_eq!(q.run(&[a, x, y]).unwrap().len(), 2);
	}

	// The plans are made once per stratum and variant, whatever the number of rounds; a later call reuses the relation
	#[test]
	fn plans_are_reused_across_rounds()
	{
		let mut rng = Rng::new(1);
		let len = 40;
		let edges: Vec<(usize, usize)> = (1..len).map(|i| (i - 1, i)).collect();
		let (conn, ids, _) = graph(len, &edges, &mut rng);
		let db = conn.db();
		for (form, plans) in [(Form::Right, 3), (Form::Left, 3), (Form::Double, 4), (Form::Or, 2)]
		{
			let mut q = db.query();
			define_reach(&mut q, form);
			let (a, b) = (q.var("?a"), q.var("?b"));
			q.call("reach", &[a.clone(), b.clone()]).unwrap();
			let stats = q.rule_stats();
			assert_eq!((stats.strata, stats.plans), (1, plans), "{form:?}");
			// One round per path length (or per doubling), and the one that finds nothing new
			if form == Form::Double
			{
				assert!((6..=9).contains(&stats.rounds), "{form:?} {stats:?}");
			}
			else
			{
				assert_eq!(stats.rounds, len as u64, "{form:?}");
			}
			q.call("reach", &[b.clone(), Value::Ref(ids[len - 1]).into()]).unwrap();
			assert_eq!(q.rule_stats(), stats);
			// The nodes before the last but one
			assert_eq!(q.run(&[a.clone()]).unwrap().len(), len - 2);
		}
	}

	// Rules over a since view see only the later datoms
	#[test]
	fn rules_over_a_since_view()
	{
		let mut rng = Rng::new(5);
		let n = 8;
		let first = random_edges(n, 15, &mut rng);
		let (mut conn, ids, _) = graph(n, &first, &mut rng);
		let later = random_edges(n, 15, &mut rng);
		let mut tx = Tx::new();
		for &(a, b) in &later
		{
			tx.add(Entity::Id(ids[a]), "node/edge", Entity::Id(ids[b]));
		}
		conn.transact(&tx).unwrap();
		let new: Vec<(usize, usize)> = later.iter().copied().filter(|e| !first.contains(e)).collect();
		let all: Vec<(usize, usize)> = first.iter().chain(&later).copied().collect();
		assert!(!new.is_empty());
		for (db, edges) in [(conn.db().since(1).unwrap(), new), (conn.db(), all)]
		{
			let reach = closure(n, &edges);
			let mut q = db.query();
			define_reach(&mut q, Form::Double);
			let (a, b) = (q.var("?a"), q.var("?b"));
			q.call("reach", &[a.clone(), b.clone()]).unwrap();
			assert_eq!(q.run(&[a, b]).unwrap(), pairs(n, &ids, |a, b| reach[a][b]));
		}
	}

	// Random positive programs (recursion, mutual recursion, calls inside or, predicates, nots of patterns) over the attributes of
	// `random_conn`, against a naive simultaneous fixpoint of the reference evaluator: every evaluated relation, and the query's answer
	#[test]
	fn random_programs_match_a_naive_fixpoint()
	{
		use crate::db::tests::{random_conn, replay};
		let (conn, reports) = random_conn(83, 60);
		let db = conn.db();
		let log: Vec<_> = reports.iter().flat_map(|r| &r.tx_data).collect();
		let facts: BTreeSet<(Eid, AttrId, Value)> = replay(log.iter().copied()).into_keys().collect();
		let ids: Vec<Eid> = facts.iter().map(|f| f.0).collect::<BTreeSet<_>>().into_iter().collect();
		let mut rng = Rng::new(29);
		let rules = [("p", 2), ("q", 1), ("r", 2)];
		let (mut tested, mut recursive, mut mutual, mut in_or) = (0, 0, 0, 0);
		for _ in 0..1500
		{
			let mut q = db.query();
			let mut or_calls = false;
			for (name, arity) in rules
			{
				// A first definition without calls, so that the relations tend not to be empty
				for def in 0..2 + rng.next() % 2
				{
					let mut body = Vec::new();
					for _ in 0..1 + rng.next() % 3
					{
						let c = if def > 0 && rng.next() % 5 < 3
						{
							let (callee, k) = rules[(rng.next() % 3) as usize];
							let args: Vec<Term> = (0..k)
								.map(|_| if rng.next() % 6 == 0 { Value::Ref(ids[(rng.next() % ids.len() as u64) as usize]).into() } else { q.var(["?a", "?b", "?c"][(rng.next() % 3) as usize]) })
								.collect();
							let call = q.call_clause(callee, &args).unwrap();
							if rng.next() % 3 == 0
							{
								let join: Vec<Term> = args.iter().filter(|t| matches!(t, Term::Var(_))).cloned().collect();
								let other = random_clause(&mut q, &mut rng, 0, &ids, true);
								other.and_then(|o| q.or(Some(&join), vec![vec![call], vec![o]]).ok()).inspect(|_| or_calls = true)
							}
							else
							{
								Some(call)
							}
						}
						else
						{
							random_clause(&mut q, &mut rng, 1, &ids, false)
						};
						body.extend(c);
					}
					let mut binds: Vec<VarId> = body_binds(&body).into_iter().collect();
					if binds.len() < arity
					{
						continue;
					}
					for i in (1..binds.len()).rev()
					{
						binds.swap(i, (rng.next() % (i as u64 + 1)) as usize);
					}
					// Mostly the entity variables, which the calls' arguments are
					if rng.next() % 4 != 0
					{
						binds.sort_by_key(|&x| !["?a", "?b", "?c"].contains(&q.var_name(x)));
					}
					let head: Vec<Term> = binds[..arity].iter().map(|&x| Term::Var(x)).collect();
					let _ = q.rule(name, &head, body);
				}
			}
			let (a, b) = (q.var("?qa"), q.var("?qb"));
			if q.call("p", &[a.clone(), b.clone()]).is_err()
			{
				continue;
			}
			let got = q.run(&[a, b]).unwrap();

			// The naive fixpoint: every definition over all the tuples so far, until nothing changes
			let mut derived: Vec<BTreeSet<Vec<Value>>> = vec![BTreeSet::new(); q.rules.len()];
			loop
			{
				let mut found = Vec::new();
				for (r, rule) in q.rules.iter().enumerate()
				{
					for d in &rule.defs
					{
						for row in naive_body(&facts, &derived, &d.body, [Row::new()].into())
						{
							found.push((r, d.head.iter().map(|x| row[x].clone()).collect::<Vec<_>>()));
						}
					}
				}
				let mut changed = false;
				for (r, t) in found
				{
					changed |= derived[r].insert(t);
				}
				if !changed
				{
					break;
				}
			}
			for (r, rel) in q.rels.full.iter().enumerate().filter(|(_, rel)| rel.ready)
			{
				assert_eq!(rel.set.iter().cloned().collect::<BTreeSet<_>>(), derived[r], "rule {}", q.rules[r].name);
			}
			let p = q.rules.iter().position(|r| r.name == "p").unwrap();
			assert_eq!(got, derived[p].iter().cloned().collect::<Vec<_>>());
			if got.is_empty()
			{
				continue;
			}
			let stats = q.rule_stats();
			let evaluated = q.rels.full.iter().filter(|rel| rel.ready).count() as u64;
			tested += 1;
			recursive += (stats.rounds > stats.strata + 1) as usize;
			mutual += (evaluated > stats.strata) as usize;
			in_or += or_calls as usize;
		}
		assert!(tested >= 250 && recursive >= 20 && mutual >= 25 && in_or >= 200, "too few: {tested} {recursive} {mutual} {in_or}");
	}
}
