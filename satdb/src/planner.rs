//! The join-order planner: an incremental SAT encoding of "the cheapest order of the query's clauses", kept alive in one solver
//! while the query is being assembled.
//!
//! # The cost model
//! A plan runs the clauses as nested loops in its order. A clause's *access mode* is which of its (at most two) variable positions
//! are already bound when it runs, and its *fanout* is the estimated number of rows it produces per input row in that mode.
//! With `log f` the scaled log-fanout, the cost of an order is `sum_c w(c, mode(c)) * (1 + #clauses after c)`, that is,
//! the sum of the log-sizes of all the intermediate results. (A proxy: it underweights a single huge intermediate result.)
//! The weights are shifted by the constant `SHIFT` to be non-negative; for a fixed number of clauses that adds the same constant to every order.
//!
//! # The encoding (everything is added monotonically, so that nothing has to be retracted when a clause is added)
//! * Order: a tournament over the clauses, one variable per pair, with both 3-cycles of every triple forbidden (an acyclic tournament is a total order).
//! * Boundness: for each variable `v` at a position of clause `c`, a literal `b(v,c)` with `b(v,c) -> OR_{d binds v} before(d,c) OR tail(v,c)`.
//!   The tail stands for "binders yet to be added": each solve assumes `!tail`, and a binder added later extends the disjunction
//!   with a fresh tail (`tail_old -> before(d_new,c) OR tail_new`).
//! * Predicates require their variables to be bound: `req(c) -> b(v,c)`, under the assumption `req(c)`. When a predicate cannot be satisfied yet
//!   (one of its variables has no binder), the solver's UNSAT core names it, and the planner plans the rest, reporting the predicate as pending.
//! * Cost: a mode literal `m(c,k) <- conditions of mode k` and pairwise literals `z <- before(c,d) AND m(c,k)`, all weighted by `w(c,k)`.
//!   The weighted sum is a binary adder network; each new clause adds its terms (and the old clauses' terms against it) to the previous sum.
//!   Only lower bounds are encoded, both on the indicators and in the adders (which guarantee `encoded sum >= true sum`, see `full_adder`):
//!   the comparators bound the sum from above only, so the exact values always remain a solution, and minimizing pushes everything down to them.
//! * Optimization: `sum <= K` comparators, each guarded by its own activation literal (assumed by one solve only). The bound starts below the greedy plan's cost
//!   and tightens after every model, until UNSAT (optimal) or the conflict budget runs out (the best plan so far).

use intel_sat_solver::{CTopor, TLit, TToporLitVal, TToporReturnVal};
use std::collections::HashMap;

pub type VarId = usize;

/// The scaled log2 is in [-16, 40] * SCALE, so the shifted weights are in [0, 56 * SCALE]
const SCALE: f64 = 4.0;
const SHIFT: i64 = 16 * SCALE as i64;
const MIN_LOG2: f64 = -16.0;
const MAX_LOG2: f64 = 40.0;

pub(crate) fn weight_of_fanout(f: f64) -> u32
{
	let l = if f > 0.0 { f.log2().clamp(MIN_LOG2, MAX_LOG2) } else { MIN_LOG2 };
	((l * SCALE).round() as i64 + SHIFT) as u32
}

/// Make mode weights non-increasing as slots become bound
pub(crate) fn monotone(mut w: [u32; 4]) -> [u32; 4]
{
	w[1] = w[1].min(w[0]);
	w[2] = w[2].min(w[0]);
	w[3] = w[3].min(w[1]).min(w[2]);
	w
}

/// What the planner needs to know about a clause
#[derive(Clone, Debug)]
pub(crate) struct ClauseShape
{
	/// The variables at the two mode-determining positions (entity, value); None for a constant position, which counts as bound
	pub slots: [Option<VarId>; 2],
	/// The variables that must be bound before the clause runs
	pub needs: Vec<VarId>,
	/// The variables the clause binds
	pub binds: Vec<VarId>,
	/// The weight of each mode, indexed by (slot 0 bound) | (slot 1 bound) << 1; must not increase when a slot becomes bound
	pub weights: [u32; 4],
}

impl ClauseShape
{
	fn mode(&self, bound: &[bool]) -> usize
	{
		let b = |s: Option<VarId>| s.map_or(true, |v| bound[v]);
		b(self.slots[0]) as usize | (b(self.slots[1]) as usize) << 1
	}
}

/// The cost of running the clauses in `order` (in the shifted units of the objective)
pub(crate) fn cost_of_order(shapes: &[ClauseShape], order: &[usize], nvars: usize) -> u64
{
	let mut bound = vec![false; nvars];
	let mut cost = 0;
	for (p, &c) in order.iter().enumerate()
	{
		cost += shapes[c].weights[shapes[c].mode(&bound)] as u64 * (order.len() - p) as u64;
		for &v in &shapes[c].binds
		{
			bound[v] = true;
		}
	}
	cost
}

/// The predicates (by clause index) that one of whose needed variables no clause binds
pub(crate) fn unbindable(shapes: &[ClauseShape]) -> Vec<usize>
{
	let bindable: std::collections::HashSet<VarId> = shapes.iter().flat_map(|s| s.binds.iter().copied()).collect();
	(0..shapes.len()).filter(|&c| shapes[c].needs.iter().any(|v| !bindable.contains(v))).collect()
}

/// Run the cheapest ready clause next; predicates are ready once their variables are bound, unbindable ones go last
pub(crate) fn greedy_order(shapes: &[ClauseShape], nvars: usize) -> Vec<usize>
{
	let pending = unbindable(shapes);
	let mut bound = vec![false; nvars];
	let mut left: Vec<usize> = (0..shapes.len()).filter(|c| !pending.contains(c)).collect();
	let mut order = Vec::new();
	while !left.is_empty()
	{
		let ready = |&&c: &&usize| shapes[c].needs.iter().all(|&v| bound[v]);
		let &best = left.iter().filter(ready).min_by_key(|&&c| shapes[c].weights[shapes[c].mode(&bound)]).expect("a pattern is always ready, and so is a predicate whose binders already ran");
		left.retain(|&c| c != best);
		order.push(best);
		for &v in &shapes[best].binds
		{
			bound[v] = true;
		}
	}
	order.extend(pending);
	order
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanSource
{
	Sat,
	/// The greedy heuristic: before the first solve, or after a solver error disabled the SAT planner
	Greedy,
}

#[derive(Clone, Debug)]
pub struct Plan
{
	/// Clause indices in execution order
	pub order: Vec<usize>,
	/// The cost of the order (see the module documentation)
	pub cost: u64,
	/// Proved optimal under the cost model
	pub optimal: bool,
	/// Predicates that can't be satisfied yet, since one of their variables has no binder (they are last in the order, and the query can't run)
	pub pending: Vec<usize>,
	pub source: PlanSource,
}

pub(crate) struct Planner
{
	solver: CTopor,
	max_var: TLit,
	t: TLit,
	shapes: Vec<ClauseShape>,
	nvars: usize,
	// prec[j][i], i < j: "clause i runs before clause j"
	prec: Vec<Vec<TLit>>,
	// Per clause: the boundness literal and the current tail of each of its variables
	bound: Vec<Vec<(VarId, TLit, TLit)>>,
	// Per clause: the mode literals with their weights
	modes: Vec<Vec<(TLit, u32)>>,
	binders: HashMap<VarId, Vec<usize>>,
	// Per clause: the requirement assumption of a predicate
	req: Vec<Option<TLit>>,
	// The objective in binary, LSB first
	sum: Vec<TLit>,
	plan: Plan,
	// The solver's explanation, once a solver error has disabled the SAT planner
	error: Option<String>,
	// The activation literal of the latest sum <= K bound
	act: Option<TLit>,
	// Decision hints for the literals added since the latest solve (see `solve`)
	hints: Vec<TLit>,
	pub(crate) solves: u64,
}

impl Planner
{
	pub(crate) fn new() -> Self
	{
		let mut p = Planner {
			solver: CTopor::new(0),
			max_var: 0,
			t: 0,
			shapes: Vec::new(),
			nvars: 0,
			prec: Vec::new(),
			bound: Vec::new(),
			modes: Vec::new(),
			binders: HashMap::new(),
			req: Vec::new(),
			sum: Vec::new(),
			plan: Plan { order: Vec::new(), cost: 0, optimal: true, pending: Vec::new(), source: PlanSource::Sat },
			error: None,
			act: None,
			hints: Vec::new(),
			solves: 0,
		};
		p.t = p.new_var();
		p.solver.AddClause(&[p.t]);
		p
	}

	pub(crate) fn plan(&self) -> &Plan
	{
		&self.plan
	}

	pub(crate) fn error(&self) -> Option<&str>
	{
		self.error.as_deref()
	}

	fn new_var(&mut self) -> TLit
	{
		self.max_var += 1;
		self.max_var
	}

	fn before(&self, i: usize, j: usize) -> TLit
	{
		if i < j { self.prec[j][i] } else { -self.prec[i][j] }
	}

	/// Add a clause (to be numbered `self.shapes.len()`); `nvars` is the number of query variables so far.
	/// The plan becomes the greedy one until the next `refine`.
	pub(crate) fn add_clause(&mut self, shape: ClauseShape, nvars: usize)
	{
		let w = shape.weights;
		assert!(w[1] <= w[0] && w[2] <= w[0] && w[3] <= w[1] && w[3] <= w[2], "the weights must not increase when a slot becomes bound");
		self.retire_bound();
		let c = self.shapes.len();
		self.nvars = nvars;

		// The order
		let row: Vec<TLit> = (0..c).map(|_| self.new_var()).collect();
		self.prec.push(row);
		for i in 0..c
		{
			for j in i + 1..c
			{
				self.solver.AddClause(&[-self.before(i, j), -self.before(j, c), -self.before(c, i)]);
				self.solver.AddClause(&[-self.before(i, c), -self.before(c, j), -self.before(j, i)]);
			}
		}

		// The boundness of the clause's variables, justified by the existing binders (or by binders yet to be added)
		let mut vars: Vec<VarId> = shape.slots.iter().flatten().chain(&shape.needs).copied().collect();
		vars.sort_unstable();
		vars.dedup();
		let mut bound = Vec::new();
		for v in vars
		{
			let (b, tail) = (self.new_var(), self.new_var());
			let mut cls = vec![-b, tail];
			cls.extend(self.binders.get(&v).into_iter().flatten().map(|&d| self.before(d, c)));
			self.solver.AddClause(&cls);
			self.hints.push(b);
			bound.push((v, b, tail));
		}
		self.bound.push(bound);

		// The clause justifies the boundness of its variables in the existing clauses
		for &v in &shape.binds
		{
			for d in 0..c
			{
				for k in 0..self.bound[d].len()
				{
					if self.bound[d][k].0 == v
					{
						let tail = self.new_var();
						self.solver.AddClause(&[-self.bound[d][k].2, self.before(c, d), tail]);
						self.bound[d][k].2 = tail;
					}
				}
			}
			self.binders.entry(v).or_default().push(c);
		}

		// The requirements of a predicate
		let req = if shape.needs.is_empty()
		{
			None
		}
		else
		{
			let r = self.new_var();
			for &v in &shape.needs
			{
				let b = self.bound_lit(c, v);
				self.solver.AddClause(&[-r, b]);
			}
			Some(r)
		};
		self.req.push(req);

		// The cost terms: the clause's modes, and every pair of the clause with an existing one
		let mut modes = Vec::new();
		for k in 0..4
		{
			let mut conds = Vec::new();
			let mut possible = true;
			for s in 0..2
			{
				let on = k >> s & 1 == 1;
				match shape.slots[s]
				{
					Some(v) =>
					{
						let b = self.bound_lit(c, v);
						conds.push(if on { b } else { -b });
					}
					None => possible &= on,
				}
			}
			if !possible
			{
				continue;
			}
			let m = if conds.is_empty() { self.t } else { self.new_var() };
			if m != self.t
			{
				let mut cls: Vec<TLit> = conds.iter().map(|&l| -l).collect();
				cls.push(m);
				self.solver.AddClause(&cls);
			}
			if m != self.t
			{
				self.hints.push(-m);
			}
			modes.push((m, shape.weights[k]));
		}
		self.modes.push(modes);
		self.shapes.push(shape);

		let mut terms: Vec<(TLit, u32)> = self.modes[c].clone();
		for d in 0..c
		{
			for (x, y) in [(c, d), (d, c)]
			{
				for k in 0..self.modes[x].len()
				{
					let (m, w) = self.modes[x][k];
					let z = self.new_var();
					self.solver.AddClause(&[-self.before(x, y), -m, z]);
					self.hints.push(-z);
					terms.push((z, w));
				}
			}
		}
		self.add_to_sum(&terms);

		self.set_greedy();
	}

	fn bound_lit(&self, c: usize, v: VarId) -> TLit
	{
		self.bound[c].iter().find(|e| e.0 == v).expect("every slot and need of a clause has a boundness literal").1
	}

	// sum += sum of the weights of the true literals of terms
	fn add_to_sum(&mut self, terms: &[(TLit, u32)])
	{
		let mut cols: Vec<Vec<TLit>> = self.sum.iter().map(|&b| vec![b]).collect();
		for &(l, w) in terms
		{
			for j in 0..32
			{
				if w >> j & 1 == 1
				{
					if cols.len() <= j
					{
						cols.resize(j + 1, Vec::new());
					}
					cols[j].push(l);
				}
			}
		}
		// Each column is reduced as a chain (each adder's sum goes into the next adder of the column); measured, this propagates much better
		// than reducing the column in rounds of disjoint triples, despite the depth
		let mut sum = Vec::new();
		let mut j = 0;
		while j < cols.len()
		{
			while cols[j].len() > 1
			{
				let (a, b) = (cols[j].pop().unwrap(), cols[j].pop().unwrap());
				let c = if cols[j].is_empty() { -self.t } else { cols[j].pop().unwrap() };
				let (s, carry) = self.full_adder(a, b, c);
				cols[j].push(s);
				if cols.len() <= j + 1
				{
					cols.push(Vec::new());
				}
				cols[j + 1].push(carry);
			}
			sum.push(cols[j].pop().unwrap_or(-self.t));
			j += 1;
		}
		self.sum = sum;
	}

	// A full adder that only bounds its value from below: s + 2 * carry >= a + b + c.
	// That suffices, since the comparators only bound the sum from above, and the exact value always satisfies it.
	// (The parity equivalences of an exact adder propagate poorly and make the solver search even when the sum is unconstrained.)
	fn full_adder(&mut self, a: TLit, b: TLit, c: TLit) -> (TLit, TLit)
	{
		let (s, carry) = (self.new_var(), self.new_var());
		for x in [a, b, c]
		{
			self.solver.AddClause(&[-x, s, carry]);
		}
		for (x, y) in [(a, b), (a, c), (b, c)]
		{
			self.solver.AddClause(&[-x, -y, carry]);
		}
		self.solver.AddClause(&[-a, -b, -c, s]);
		self.hints.extend([-s, -carry]);
		(s, carry)
	}

	// Turn off the latest bound for good, so that the solver can't set its activation literal by a decision in a later solve
	fn retire_bound(&mut self)
	{
		if let Some(act) = self.act.take()
		{
			self.solver.AddClause(&[-act]);
		}
	}

	// A fresh activation literal, which implies sum <= k
	fn at_most(&mut self, k: u64) -> TLit
	{
		self.retire_bound();
		let act = self.new_var();
		self.act = Some(act);
		let n = self.sum.len();
		if n >= 64 || k < 1 << n
		{
			for j in 0..n
			{
				if k >> j & 1 == 0
				{
					let mut cls = vec![-act, -self.sum[j]];
					cls.extend((j + 1..n).filter(|&i| k >> i & 1 == 1).map(|i| -self.sum[i]));
					self.solver.AddClause(&cls);
				}
			}
		}
		act
	}

	fn lit_true(&self, l: TLit) -> bool
	{
		self.solver.GetLitValue(l) == TToporLitVal::VAL_SATISFIED
	}

	fn decode_order(&self) -> Option<Vec<usize>>
	{
		let n = self.shapes.len();
		let mut order = vec![usize::MAX; n];
		for c in 0..n
		{
			let p = (0..n).filter(|&d| d != c && self.lit_true(self.before(d, c))).count();
			if order[p] != usize::MAX
			{
				return None;
			}
			order[p] = c;
		}
		Some(order)
	}

	// Decision hints (each measured to speed up the optimization): boundness prefers true, the cost indicators and the adder outputs prefer false
	// (they are only ever implied), and the order starts from the best plan so far.
	// The upstream solver requires that these calls be followed by Solve (and not by AddClause), hence they are made here.
	fn solve(&mut self, assumps: &[TLit], budget: &mut u64) -> TToporReturnVal
	{
		for l in std::mem::take(&mut self.hints)
		{
			self.solver.FixPolarity(l, false);
		}
		let mut pos = vec![0; self.shapes.len()];
		for (p, &c) in self.plan.order.iter().enumerate()
		{
			pos[c] = p;
		}
		for j in 0..self.prec.len()
		{
			for i in 0..j
			{
				let l = self.prec[j][i];
				self.solver.FixPolarity(if pos[i] < pos[j] { l } else { -l }, true);
			}
		}
		let before = self.solver.GetConflictsNumber();
		let r = self.solver.Solve(assumps, (f64::MAX, true), *budget);
		self.solves += 1;
		// u64::MAX means unlimited to the solver, while any other threshold is added to its conflict count, so u64::MAX must stay as it is
		if *budget != u64::MAX
		{
			*budget = budget.saturating_sub(self.solver.GetConflictsNumber() - before);
		}
		r
	}

	fn fail(&mut self, what: &str)
	{
		let why = if self.solver.IsError() { self.solver.GetStatusExplanation() } else { what.to_string() };
		self.error = Some(why);
		self.set_greedy();
	}

	fn set_greedy(&mut self)
	{
		let order = greedy_order(&self.shapes, self.nvars);
		let cost = cost_of_order(&self.shapes, &order, self.nvars);
		self.plan = Plan { order, cost, optimal: false, pending: unbindable(&self.shapes), source: PlanSource::Greedy };
	}

	/// Improve the plan within `budget` conflicts (u64::MAX: until optimal)
	pub(crate) fn refine(&mut self, mut budget: u64) -> &Plan
	{
		if self.error.is_some()
		{
			self.set_greedy();
			return &self.plan;
		}
		if self.plan.optimal
		{
			return &self.plan;
		}
		self.retire_bound();
		let tails: Vec<TLit> = self.bound.iter().flatten().map(|e| -e.2).collect();

		// Find the predicates that can't be satisfied: drop the requirements in each UNSAT core, then re-admit those which hold on their own
		let mut reqs: Vec<(usize, TLit)> = self.req.iter().enumerate().filter_map(|(c, r)| r.map(|r| (c, r))).collect();
		let mut dropped = Vec::new();
		loop
		{
			let assumps: Vec<TLit> = tails.iter().copied().chain(reqs.iter().map(|r| r.1)).collect();
			match self.solve(&assumps, &mut budget)
			{
				TToporReturnVal::RET_SAT => break,
				TToporReturnVal::RET_UNSAT =>
				{
					let core: Vec<usize> = (0..reqs.len()).filter(|&i| self.solver.IsAssumptionRequired(tails.len() + i)).collect();
					if core.is_empty()
					{
						self.fail("the order constraints are unsatisfiable without any predicate requirement");
						return &self.plan;
					}
					for &i in core.iter().rev()
					{
						dropped.push(reqs.remove(i));
					}
				}
				TToporReturnVal::RET_CONFLICT_OUT => return &self.plan,
				_ =>
				{
					self.fail("unexpected solver result");
					return &self.plan;
				}
			}
		}
		let mut pending = Vec::new();
		for r in dropped
		{
			let assumps: Vec<TLit> = tails.iter().copied().chain(reqs.iter().map(|r| r.1)).chain([r.1]).collect();
			match self.solve(&assumps, &mut budget)
			{
				TToporReturnVal::RET_SAT => reqs.push(r),
				TToporReturnVal::RET_UNSAT => pending.push(r.0),
				TToporReturnVal::RET_CONFLICT_OUT => return &self.plan,
				_ =>
				{
					self.fail("unexpected solver result");
					return &self.plan;
				}
			}
		}
		pending.sort_unstable();
		debug_assert_eq!(pending, self.plan.pending, "a predicate is unsatisfiable exactly when one of its variables has no binder");
		self.plan.pending = pending;

		// Minimize, starting from the best plan so far (the greedy one, or the result of an earlier refine, which ran out of budget)
		let base: Vec<TLit> = tails.iter().copied().chain(reqs.iter().map(|r| r.1)).collect();
		while self.plan.cost > 0
		{
			let act = self.at_most(self.plan.cost - 1);
			let assumps: Vec<TLit> = base.iter().copied().chain([act]).collect();
			match self.solve(&assumps, &mut budget)
			{
				TToporReturnVal::RET_SAT =>
				{
					let Some(order) = self.decode_order()
					else
					{
						self.fail("the model doesn't encode a total order");
						return &self.plan;
					};
					let cost = cost_of_order(&self.shapes, &order, self.nvars);
					debug_assert!(cost < self.plan.cost, "the true cost of a model is at most its objective value");
					self.plan.order = order;
					self.plan.cost = cost;
					self.plan.source = PlanSource::Sat;
				}
				TToporReturnVal::RET_UNSAT => break,
				TToporReturnVal::RET_CONFLICT_OUT => return &self.plan,
				_ =>
				{
					self.fail("unexpected solver result");
					return &self.plan;
				}
			}
		}
		self.plan.optimal = true;
		&self.plan
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::testutil::Rng;

	fn permutations(n: usize) -> Vec<Vec<usize>>
	{
		if n == 0
		{
			return vec![vec![]];
		}
		let mut out = Vec::new();
		for p in permutations(n - 1)
		{
			for i in 0..=p.len()
			{
				let mut q = p.clone();
				q.insert(i, n - 1);
				out.push(q);
			}
		}
		out
	}

	// Predicates that have binders must come after one binder of each of their variables
	fn valid(shapes: &[ClauseShape], order: &[usize], nvars: usize) -> bool
	{
		let pending = unbindable(shapes);
		let mut bound = vec![false; nvars];
		for &c in order
		{
			if !pending.contains(&c) && !shapes[c].needs.iter().all(|&v| bound[v])
			{
				return false;
			}
			for &v in &shapes[c].binds
			{
				bound[v] = true;
			}
		}
		true
	}

	fn random_query(rng: &mut Rng, n: usize, nvars: usize) -> Vec<ClauseShape>
	{
		let mut shapes = Vec::new();
		for _ in 0..n
		{
			let slot = |rng: &mut Rng| if rng.next() % 4 == 0 { None } else { Some((rng.next() % nvars as u64) as usize) };
			if rng.next() % 4 == 0
			{
				let (a, b) = (slot(rng), slot(rng));
				let needs: Vec<VarId> = [a, b].into_iter().flatten().collect();
				let w = weight_of_fanout(1.0 / (1 + rng.next() % 8) as f64);
				shapes.push(ClauseShape { slots: [None, None], needs, binds: vec![], weights: [w; 4] });
			}
			else
			{
				let slots = [slot(rng), slot(rng)];
				let binds: Vec<VarId> = slots.iter().flatten().copied().collect();
				let w = [0; 4].map(|_| weight_of_fanout((rng.next() % 5000) as f64 / 16.0));
				shapes.push(ClauseShape { slots, needs: vec![], binds, weights: monotone(w) });
			}
		}
		shapes
	}

	#[test]
	fn optimum_matches_brute_force_and_incremental_matches_batch()
	{
		let mut rng = Rng::new(7);
		for round in 0..60
		{
			let n = 1 + round % 6;
			let nvars = 1 + (rng.next() % 4) as usize;
			let shapes = random_query(&mut rng, n, nvars);
			let best = permutations(n).into_iter().filter(|o| valid(&shapes, o, nvars)).map(|o| cost_of_order(&shapes, &o, nvars)).min().unwrap();

			// Online: refine after every clause
			let mut online = Planner::new();
			for s in &shapes
			{
				online.add_clause(s.clone(), nvars);
				online.refine(u64::MAX);
			}
			// Batch: one refine at the end
			let mut batch = Planner::new();
			for s in &shapes
			{
				batch.add_clause(s.clone(), nvars);
			}
			for p in [&mut online, &mut batch]
			{
				let plan = p.refine(u64::MAX).clone();
				assert!(plan.optimal && p.error().is_none(), "round {round}: {:?} {plan:?}", p.error());
				assert!(valid(&shapes, &plan.order, nvars), "round {round}: invalid order {:?}", plan.order);
				assert_eq!(plan.cost, best, "round {round}: shapes {shapes:?}");
				assert_eq!(plan.cost, cost_of_order(&shapes, &plan.order, nvars));
				assert_eq!(plan.pending, unbindable(&shapes));
			}
		}
	}

	#[test]
	fn pending_predicate_becomes_plannable()
	{
		let pred = ClauseShape { slots: [None, None], needs: vec![1], binds: vec![], weights: [weight_of_fanout(0.3); 4] };
		let pat = |slots: [Option<VarId>; 2], w: f64| ClauseShape { slots, needs: vec![], binds: slots.iter().flatten().copied().collect(), weights: [weight_of_fanout(w); 4] };
		let mut p = Planner::new();
		p.add_clause(pat([Some(0), None], 100.0), 2);
		p.add_clause(pred, 2);
		assert_eq!(p.refine(u64::MAX).pending, vec![1]);
		p.add_clause(pat([Some(0), Some(1)], 10.0), 2);
		let plan = p.refine(u64::MAX).clone();
		assert!(plan.pending.is_empty() && plan.optimal);
		assert!(plan.order.iter().position(|&c| c == 2) < plan.order.iter().position(|&c| c == 1));
	}

	#[test]
	fn budget_gives_a_valid_anytime_plan()
	{
		let mut rng = Rng::new(3);
		let shapes = random_query(&mut rng, 12, 6);
		let mut p = Planner::new();
		for s in &shapes
		{
			p.add_clause(s.clone(), 6);
		}
		let quick = p.refine(1).clone();
		assert!(quick.cost <= cost_of_order(&shapes, &greedy_order(&shapes, 6), 6));
		let full = p.refine(u64::MAX).clone();
		assert!(full.optimal && full.cost <= quick.cost);
		assert!(valid(&shapes, &full.order, 6));
	}
}
