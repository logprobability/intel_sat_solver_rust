//! The join-order planner: an incremental SAT encoding of "the cheapest order of the query's clauses", kept alive in one solver
//! while the query is being assembled, with an exact dynamic program for small queries.
//!
//! # The cost model
//! A plan runs the clauses as nested loops in its order. A clause's *access mode* is which of its (at most two) variable positions
//! are already bound when it runs, and its *fanout* is the estimated number of rows it produces per input row in that mode.
//! With `log f` the scaled log-fanout, the cost of an order is `sum_c w(c, mode(c)) * (1 + #clauses after c)`, that is,
//! the sum of the log-sizes of all the intermediate results. (A proxy: it underweights a single huge intermediate result.)
//! The weights are shifted by the constant `SHIFT` to be non-negative; for a fixed number of clauses that adds the same constant to every order.
//!
//! # Valid orders
//! A clause can run once its needed variables are bound (by a binder before it) and the clauses that user constraints
//! (`require_before`) put before it have run. A *pending* predicate, one with a variable that no clause binds yet, waits for
//! the binders of its other variables only (it can't run, but its place in the plan respects whatever is known).
//! Being able to run is monotone in the set of clauses that ran, so the greedy order (`greedy_order`) gets stuck exactly when
//! no order is valid: it is an exact feasibility test, which `respects`, the planner and the tests share.
//! Feasibility survives adding a clause (a new binder can run first, a new predicate last), so constraints that admit an order
//! when they are accepted admit one from then on; a constraint that doesn't is refused, with a minimal conflicting subset of the constraints.
//!
//! # Dynamic programming
//! The mode of a clause depends only on the set of clauses before it, and its multiplier only on that set's size, so the optimum
//! is a shortest path over the subsets (`dp_order`, `O(2^n n)`; a constraint only restricts which clause may come next).
//! The planner uses it for queries of up to `DP_MAX` clauses, constrained or not: measured (release), it takes 10 us to 2.5 ms up to 16 clauses,
//! where SAT takes 0.1 to 0.9 s for 8 clauses and rarely reaches the optimum of 12 or 16 within 200k conflicts (3 to 5 s).
//! SAT orders the larger queries, incrementally, and explains refused constraints; the tests use the DP as a cross-check of SAT.
//!
//! # The encoding (everything is added monotonically, so that nothing has to be retracted when a clause is added)
//! The one change that isn't an addition is `reweigh`, a clause whose estimate a later clause refined: the DP reads the new weights,
//! and the SAT planner, if it had encoded the clause, starts over with a new solver.
//! The clauses are encoded when the SAT planner is first needed (a query larger than `DP_MAX`, or a refused constraint), in the order they were added.
//! * Order: a tournament over the clauses, one variable per pair, with both 3-cycles of every triple forbidden (an acyclic tournament is a total order).
//! * Boundness: for each variable `v` at a position of clause `c`, a literal `b(v,c)` with `b(v,c) -> OR_{d binds v} before(d,c) OR tail(v,c)`.
//!   The tail stands for "binders yet to be added": each solve assumes `!tail`, and a binder added later extends the disjunction
//!   with a fresh tail (`tail_old -> before(d_new,c) OR tail_new`).
//! * Predicates require their variables to be bound: `req(c) -> b(v,c)`, under the assumption `req(c)`. A predicate that cannot be satisfied yet
//!   (one of its variables has no binder, `unbindable`) is pending: its requirement isn't assumed, the boundness of its variables that have
//!   binders is assumed directly, and the planner plans the rest. (Finding the pending predicates by UNSAT cores instead cost a solve
//!   of every refinement, which past `DP_MAX` clauses took the refinement's whole budget.)
//! * User constraints: `r -> before(a,b)`, under the assumption `r`. A refused constraint's UNSAT core over the constraints' assumptions
//!   (with every requirement held) is shrunk by deletion, a solve per deletion, then re-confirmed by the exact feasibility test before it is reported.
//! * Cost: a mode literal `m(c,k) <- conditions of mode k` and pairwise literals `z <- before(c,d) AND m(c,k)`, all weighted by `w(c,k)`.
//!   The weighted sum is a binary adder network; each new clause adds its terms (and the old clauses' terms against it) to the previous sum.
//!   Only lower bounds are encoded, both on the indicators and in the adders (which guarantee `encoded sum >= true sum`, see `full_adder`):
//!   the comparators bound the sum from above only, so the exact values always remain a solution, and minimizing pushes everything down to them.
//! * Optimization: `sum <= K` comparators, each guarded by its own activation literal (assumed by one solve only). The bound starts below the greedy plan's cost
//!   and tightens after every model, until UNSAT (optimal) or until the conflict budget runs out (the best plan so far).
//!   Every solve is charged at least one conflict, so a budget of B conflicts allows at most B solves, which use about B conflicts:
//!   the solver checks its threshold between rounds of conflict analysis, so a solve can overshoot by the few conflicts of its last round.
//!   (The solver's threshold is its conflict count plus the budget; a finite budget is clamped so that the sum doesn't wrap.)

use intel_sat_solver::{CTopor, TLit, TToporLitVal, TToporReturnVal};
use std::collections::HashMap;

pub type VarId = usize;

/// The scaled log2 is in [-16, 40] * SCALE, so the shifted weights are in [0, 56 * SCALE]
const SCALE: f64 = 4.0;
const SHIFT: i64 = 16 * SCALE as i64;
const MIN_LOG2: f64 = -16.0;
const MAX_LOG2: f64 = 40.0;

/// The largest query that the planner orders by dynamic programming instead of SAT (the DP has 2^n states)
pub(crate) const DP_MAX: usize = 16;
/// The conflict budget of finding the core of a refused constraint (its solves have no cost bound, so they are easy)
const CORE_BUDGET: u64 = 100_000;

pub(crate) fn weight_of_fanout(f: f64) -> u32
{
	let l = if f > 0.0 { f.log2().clamp(MIN_LOG2, MAX_LOG2) } else { MIN_LOG2 };
	((l * SCALE).round() as i64 + SHIFT) as u32
}

/// The fanout that a weight stands for (the inverse of `weight_of_fanout`, up to its rounding and clamping)
pub(crate) fn fanout_of_weight(w: u32) -> f64
{
	((w as i64 - SHIFT) as f64 / SCALE).exp2()
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
	pub(crate) fn mode(&self, bound: &[bool]) -> usize
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

/// Which variables some clause binds
fn bindable(shapes: &[ClauseShape], nvars: usize) -> Vec<bool>
{
	let mut out = vec![false; nvars];
	for &v in shapes.iter().flat_map(|s| &s.binds)
	{
		out[v] = true;
	}
	out
}

/// The predicates (by clause index) that one of whose needed variables no clause binds
pub(crate) fn unbindable(shapes: &[ClauseShape]) -> Vec<usize>
{
	let bindable: std::collections::HashSet<VarId> = shapes.iter().flat_map(|s| s.binds.iter().copied()).collect();
	(0..shapes.len()).filter(|&c| shapes[c].needs.iter().any(|v| !bindable.contains(v))).collect()
}

/// Run the cheapest clause that can run next (see "Valid orders" in the module documentation), and pending predicates as late as
/// the constraints `before` (pairs `(a, b)`: a runs before b) allow. None when no order is valid (exact, since being able to run is monotone)
pub(crate) fn greedy_order(shapes: &[ClauseShape], nvars: usize, before: &[(usize, usize)]) -> Option<Vec<usize>>
{
	let n = shapes.len();
	let (bindable, pending) = (bindable(shapes, nvars), unbindable(shapes));
	let mut bound = vec![false; nvars];
	let mut done = vec![false; n];
	let mut order = Vec::with_capacity(n);
	while order.len() < n
	{
		let ready = |c: &usize| !done[*c] && before.iter().all(|&(a, b)| b != *c || done[a]) && shapes[*c].needs.iter().all(|&v| bound[v] || !bindable[v]);
		let best = (0..n).filter(|c| ready(c) && !pending.contains(c)).min_by_key(|&c| shapes[c].weights[shapes[c].mode(&bound)]).or_else(|| (0..n).find(ready))?;
		done[best] = true;
		order.push(best);
		for &v in &shapes[best].binds
		{
			bound[v] = true;
		}
	}
	Some(order)
}

/// Whether `order` is a valid order of all the clauses (see "Valid orders" in the module documentation) under the constraints `before`
pub(crate) fn respects(shapes: &[ClauseShape], nvars: usize, before: &[(usize, usize)], order: &[usize]) -> bool
{
	let n = shapes.len();
	let mut pos = vec![usize::MAX; n];
	for (p, &c) in order.iter().enumerate()
	{
		if c >= n || pos[c] != usize::MAX
		{
			return false;
		}
		pos[c] = p;
	}
	if order.len() != n || before.iter().any(|&(a, b)| pos[a] > pos[b])
	{
		return false;
	}
	let bindable = bindable(shapes, nvars);
	let mut bound = vec![false; nvars];
	for &c in order
	{
		if !shapes[c].needs.iter().all(|&v| bound[v] || !bindable[v])
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

/// The cheapest valid order under the constraints `before`, with its cost, by dynamic programming over the sets of clauses that run first
/// (see the module documentation). None when no order is valid, or when the clauses bind more than 64 variables
pub(crate) fn dp_order(shapes: &[ClauseShape], nvars: usize, before: &[(usize, usize)]) -> Option<(Vec<usize>, u64)>
{
	let n = shapes.len();
	assert!(n <= 24, "the DP is exponential in the number of clauses");
	// A bit per bindable variable (a variable that no clause binds is never bound)
	let mut bit: Vec<Option<u32>> = vec![None; nvars];
	let mut nbits = 0;
	for &v in shapes.iter().flat_map(|s| &s.binds)
	{
		if bit[v].is_none()
		{
			bit[v] = Some(nbits);
			nbits += 1;
		}
	}
	if nbits > 64
	{
		return None;
	}
	let mask = |vs: &[VarId]| vs.iter().filter_map(|&v| bit[v]).fold(0u64, |m, b| m | 1 << b);
	let binds: Vec<u64> = shapes.iter().map(|s| mask(&s.binds)).collect();
	let needs: Vec<u64> = shapes.iter().map(|s| mask(&s.needs)).collect();
	// Per slot: the bit that binds it (0 for a constant), or None for a variable that is never bound
	let slots: Vec<[Option<u64>; 2]> = shapes.iter().map(|s| s.slots.map(|x| x.map_or(Some(0), |v| bit[v].map(|b| 1u64 << b)))).collect();
	let mut preds = vec![0u32; n];
	for &(a, b) in before
	{
		preds[b] |= 1 << a;
	}

	// cost[s]: the cheapest way to run the clauses of s first; bound[s]: the variables they bind; last[s]: the last of them in that way
	let full = (1usize << n) - 1;
	let mut cost = vec![u64::MAX; full + 1];
	let mut bound = vec![0u64; full + 1];
	let mut last = vec![0u8; full + 1];
	cost[0] = 0;
	for s in 0..full
	{
		if cost[s] == u64::MAX
		{
			continue;
		}
		let (b, mult) = (bound[s], (n - s.count_ones() as usize) as u64);
		let on = |x: Option<u64>| x.map_or(false, |m| m & !b == 0);
		for c in (0..n).filter(|&c| s >> c & 1 == 0 && preds[c] & !(s as u32) == 0 && needs[c] & !b == 0)
		{
			let w = shapes[c].weights[on(slots[c][0]) as usize | (on(slots[c][1]) as usize) << 1] as u64;
			let t = s | 1 << c;
			bound[t] = b | binds[c];
			if cost[s] + w * mult < cost[t]
			{
				cost[t] = cost[s] + w * mult;
				last[t] = c as u8;
			}
		}
	}
	if cost[full] == u64::MAX
	{
		return None;
	}
	let mut order = Vec::with_capacity(n);
	let mut s = full;
	while s != 0
	{
		order.push(last[s] as usize);
		s &= !(1 << last[s]);
	}
	order.reverse();
	Some((order, cost[full]))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanSource
{
	Sat,
	/// The dynamic program over subsets (a query of at most `DP_MAX` clauses)
	Dp,
	/// The greedy heuristic: before the first solve, or after a solver error disabled the SAT planner
	Greedy,
	/// The previous plan, carried over a change of the query: with an added clause at its cheapest valid position, or as it was after
	/// an accepted constraint that it respects (whichever is cheaper than the greedy plan)
	Carried,
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
	/// Predicates that can't be satisfied yet, since one of their variables has no binder (the query can't run)
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
	// The number of clauses encoded in the solver, the first ones (see `sync`)
	encoded: usize,
	// The user constraints (a, b), clause a before clause b, and the assumption literals of those encoded so far
	before: Vec<(usize, usize)>,
	before_lits: Vec<TLit>,
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
	/// The largest query that the DP orders (the tests lower it to exercise SAT)
	pub(crate) dp_max: usize,
	pub(crate) solves: u64,
	pub(crate) conflicts: u64,
	/// The refused constraints whose core the SAT solver didn't give (out of budget, or not confirmed), found by deletion with the feasibility test
	pub(crate) fallback_cores: u64,
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
			encoded: 0,
			before: Vec::new(),
			before_lits: Vec::new(),
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
			dp_max: DP_MAX,
			solves: 0,
			conflicts: 0,
			fallback_cores: 0,
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

	/// The accepted user constraints (a, b): clause a before clause b
	pub(crate) fn constraints(&self) -> &[(usize, usize)]
	{
		&self.before
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
	/// Until the next `refine`, the plan is the cheaper of the greedy one and the previous plan with the clause inserted at its cheapest valid position
	/// (a new binder may have to run before predicates that were pending), so what earlier refinements found carries over.
	pub(crate) fn add_clause(&mut self, shape: ClauseShape, nvars: usize)
	{
		let w = shape.weights;
		assert!(w[1] <= w[0] && w[2] <= w[0] && w[3] <= w[1] && w[3] <= w[2], "the weights must not increase when a slot becomes bound");
		self.shapes.push(shape);
		self.nvars = nvars;
		let (prev, c) = (std::mem::take(&mut self.plan.order), self.shapes.len() - 1);
		self.set_carried((0..=prev.len()).map(|p| [&prev[..p], &[c], &prev[p..]].concat()));
	}

	/// Change the weights of clause `c`, whose estimate a clause added later refined (a comparison bounding a pattern's range).
	/// The encoding only grows, so if the clause is encoded already, the solver starts over, and the next refinement encodes the query again.
	/// Until then, the plan is the cheaper of the greedy one and the previous one
	pub(crate) fn reweigh(&mut self, c: usize, weights: [u32; 4])
	{
		assert!(weights == monotone(weights), "the weights must not increase when a slot becomes bound");
		if self.shapes[c].weights == weights
		{
			return;
		}
		self.shapes[c].weights = weights;
		if c < self.encoded
		{
			let old = std::mem::replace(self, Planner::new());
			(self.shapes, self.nvars, self.before, self.plan, self.error, self.dp_max) = (old.shapes, old.nvars, old.before, old.plan, old.error, old.dp_max);
			(self.solves, self.conflicts, self.fallback_cores) = (old.solves, old.conflicts, old.fallback_cores);
		}
		let prev = std::mem::take(&mut self.plan.order);
		self.set_carried(std::iter::once(prev));
	}

	/// Require clause `a` to run before clause `b`; returns the constraint's index. A constraint under which no order is valid
	/// (with the other constraints and the clauses' needs) is refused, with a minimal conflicting subset of the constraints, the new one included
	pub(crate) fn require_before(&mut self, a: usize, b: usize) -> Result<usize, Vec<(usize, usize)>>
	{
		assert!(a != b && a < self.shapes.len() && b < self.shapes.len(), "the query validates the constraint's clauses");
		let mut all = self.before.clone();
		all.push((a, b));
		if greedy_order(&self.shapes, self.nvars, &all).is_some()
		{
			self.before.push((a, b));
			let prev = std::mem::take(&mut self.plan.order);
			self.set_carried(std::iter::once(prev));
			return Ok(self.before.len() - 1);
		}
		let core = self.sat_core(a, b);
		let feasible = |set: &[usize]| greedy_order(&self.shapes, self.nvars, &set.iter().map(|&i| all[i]).collect::<Vec<_>>()).is_some();
		let minimal = |core: &[usize]| !feasible(core) && (0..core.len()).all(|k| feasible(&[&core[..k], &core[k + 1..]].concat()));
		let core = match core
		{
			Some(core) if minimal(&core) => core,
			_ =>
			{
				// Deletion with the feasibility test
				self.fallback_cores += 1;
				let mut core: Vec<usize> = (0..all.len()).collect();
				let mut k = 0;
				while k < core.len()
				{
					let rest = [&core[..k], &core[k + 1..]].concat();
					if feasible(&rest)
					{
						k += 1;
					}
					else
					{
						core = rest;
					}
				}
				core
			}
		};
		debug_assert!(core.contains(&self.before.len()), "the accepted constraints admit an order, so the new one is in every core");
		Err(core.into_iter().map(|i| all[i]).collect())
	}

	// The UNSAT core of the constraints plus (a, b), shrunk by deletion: indices into the constraints, (a, b) being the last.
	// None when the budget runs out or the solver disagrees with the feasibility test
	fn sat_core(&mut self, a: usize, b: usize) -> Option<Vec<usize>>
	{
		if self.error.is_some()
		{
			return None;
		}
		self.sync();
		let r = self.new_var();
		self.solver.AddClause(&[-r, self.before(a, b)]);
		let lits: Vec<TLit> = self.before_lits.iter().copied().chain([r]).collect();
		let hard = self.requirements();

		let mut budget = CORE_BUDGET;
		let mut core: Vec<usize> = (0..lits.len()).collect();
		// The position in the core of the constraint that the next solve leaves out (none in the first solve)
		let mut k = usize::MAX;
		let found = loop
		{
			let trial: Vec<usize> = core.iter().enumerate().filter(|&(i, _)| i != k).map(|(_, &x)| x).collect();
			let assumps: Vec<TLit> = hard.iter().copied().chain(trial.iter().map(|&i| lits[i])).collect();
			match self.solve(&assumps, &mut budget)
			{
				TToporReturnVal::RET_UNSAT =>
				{
					// The new core is a subset of the trial, which keeps the constraints found necessary (those before k) in place
					core = (0..trial.len()).filter(|&i| self.solver.IsAssumptionRequired(hard.len() + i)).map(|i| trial[i]).collect();
					k = if k == usize::MAX { 0 } else { k };
				}
				// The constraint left out is necessary
				TToporReturnVal::RET_SAT if k != usize::MAX => k += 1,
				_ => break None,
			}
			if k >= core.len()
			{
				break Some(core);
			}
		};
		// The refused constraint must not constrain a later solve
		self.solver.AddClause(&[-r]);
		found
	}

	// The assumptions that hold every requirement: no binder yet to be added (the tails), the predicates' requirements,
	// and for a pending predicate (one of whose variables has no binder, see `unbindable`) the boundness of its variables that have binders
	fn requirements(&self) -> Vec<TLit>
	{
		let pending = unbindable(&self.shapes);
		let mut out: Vec<TLit> = self.bound.iter().flatten().map(|e| -e.2).collect();
		out.extend((0..self.shapes.len()).filter(|c| !pending.contains(c)).filter_map(|c| self.req[c]));
		out.extend(pending.iter().flat_map(|&c| self.shapes[c].needs.iter().filter(|v| self.binders.contains_key(v)).map(move |&v| self.bound_lit(c, v))));
		out
	}

	// Encode the clauses and the constraints added since the latest call, in order (a clause's encoding refers to the earlier ones only)
	fn sync(&mut self)
	{
		self.retire_bound();
		while self.encoded < self.shapes.len()
		{
			self.encode(self.encoded);
			self.encoded += 1;
		}
		while self.before_lits.len() < self.before.len()
		{
			let (a, b) = self.before[self.before_lits.len()];
			let r = self.new_var();
			self.solver.AddClause(&[-r, self.before(a, b)]);
			self.before_lits.push(r);
		}
	}

	fn encode(&mut self, c: usize)
	{
		let shape = self.shapes[c].clone();

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

	// Each pair's variable is read once, so that a don't-care value (either value satisfies the formula) decides both directions consistently
	fn decode_order(&self) -> Option<Vec<usize>>
	{
		let n = self.shapes.len();
		let mut preds = vec![0; n];
		for j in 0..n
		{
			for i in 0..j
			{
				preds[if self.lit_true(self.prec[j][i]) { j } else { i }] += 1;
			}
		}
		let mut order = vec![usize::MAX; n];
		for c in 0..n
		{
			let p = preds[c];
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
	// With the budget spent, nothing is solved (RET_CONFLICT_OUT): a solve that needs no conflict would succeed even with a zero threshold
	fn solve(&mut self, assumps: &[TLit], budget: &mut u64) -> TToporReturnVal
	{
		if *budget == 0
		{
			return TToporReturnVal::RET_CONFLICT_OUT;
		}
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
		// The solver's threshold is its conflict count plus the budget (wrapping), so a finite budget is clamped below u64::MAX - count
		let thr = if *budget == u64::MAX { u64::MAX } else { (*budget).min(u64::MAX - 1 - before) };
		let r = self.solver.Solve(assumps, (f64::MAX, true), thr);
		let used = self.solver.GetConflictsNumber() - before;
		self.solves += 1;
		self.conflicts += used;
		// u64::MAX means unlimited to the solver, while any other threshold is added to its conflict count, so u64::MAX must stay as it is;
		// every solve is charged at least one conflict, so that a budget bounds the number of solves too
		if *budget != u64::MAX
		{
			*budget = budget.saturating_sub(used.max(1));
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
		let order = greedy_order(&self.shapes, self.nvars, &self.before).expect("the accepted constraints admit an order");
		let cost = cost_of_order(&self.shapes, &order, self.nvars);
		self.plan = Plan { order, cost, optimal: false, pending: unbindable(&self.shapes), source: PlanSource::Greedy };
	}

	// The greedy plan, unless one of the valid orders among `carried` (adaptations of the previous plan to a changed query) is cheaper
	fn set_carried(&mut self, carried: impl Iterator<Item = Vec<usize>>)
	{
		let best = carried
			.filter(|o| respects(&self.shapes, self.nvars, &self.before, o))
			.map(|o| (cost_of_order(&self.shapes, &o, self.nvars), o))
			.min_by_key(|e| e.0);
		self.set_greedy();
		if let Some((cost, order)) = best.filter(|e| e.0 < self.plan.cost)
		{
			(self.plan.order, self.plan.cost, self.plan.source) = (order, cost, PlanSource::Carried);
		}
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

		// A small query: the DP's optimum (it can only fail on more than 64 bindable variables, as the accepted constraints admit an order)
		if self.shapes.len() <= self.dp_max
		{
			if let Some((order, cost)) = dp_order(&self.shapes, self.nvars, &self.before)
			{
				self.plan = Plan { order, cost, optimal: true, pending: unbindable(&self.shapes), source: PlanSource::Dp };
				return &self.plan;
			}
		}

		// Which predicates are pending is known in closed form (`unbindable`, which the plan has), so every solve goes to the minimization
		self.sync();
		let base: Vec<TLit> = self.requirements().into_iter().chain(self.before_lits.iter().copied()).collect();
		// Minimize, starting from the best plan so far (the greedy one, one carried over a change of the query, or the result of an earlier refine, which ran out of budget)
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
					debug_assert!(respects(&self.shapes, self.nvars, &self.before, &order), "a model is a valid order");
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

	// The cost of the cheapest valid order under the constraints, by enumeration; None when no order is valid
	fn brute_force(shapes: &[ClauseShape], nvars: usize, before: &[(usize, usize)]) -> Option<u64>
	{
		permutations(shapes.len()).into_iter().filter(|o| respects(shapes, nvars, before, o)).map(|o| cost_of_order(shapes, &o, nvars)).min()
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

	// Feasible random constraints: up to `tries` random pairs, each kept if the constraints still admit an order
	fn random_constraints(rng: &mut Rng, shapes: &[ClauseShape], nvars: usize, tries: usize) -> Vec<(usize, usize)>
	{
		let n = shapes.len() as u64;
		let mut before = Vec::new();
		for _ in 0..tries
		{
			let (a, b) = ((rng.next() % n) as usize, (rng.next() % n) as usize);
			let mut all = before.clone();
			all.push((a, b));
			if a != b && greedy_order(shapes, nvars, &all).is_some()
			{
				before = all;
			}
		}
		before
	}

	// A planner that never uses the DP
	fn sat_planner() -> Planner
	{
		let mut p = Planner::new();
		p.dp_max = 0;
		p
	}

	fn pattern(slots: [Option<VarId>; 2], fanout: f64) -> ClauseShape
	{
		ClauseShape { slots, needs: vec![], binds: slots.iter().flatten().copied().collect(), weights: [weight_of_fanout(fanout); 4] }
	}

	fn predicate(needs: Vec<VarId>) -> ClauseShape
	{
		ClauseShape { slots: [None, None], needs, binds: vec![], weights: [weight_of_fanout(0.3); 4] }
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
			let best = brute_force(&shapes, nvars, &[]).unwrap();
			let (order, cost) = dp_order(&shapes, nvars, &[]).unwrap();
			assert!(cost == best && cost_of_order(&shapes, &order, nvars) == cost && respects(&shapes, nvars, &[], &order), "round {round}: DP {order:?} {cost}");

			// Online: refine after every clause
			let mut online = sat_planner();
			for s in &shapes
			{
				online.add_clause(s.clone(), nvars);
				online.refine(u64::MAX);
			}
			// Batch: one refine at the end
			let mut batch = sat_planner();
			for s in &shapes
			{
				batch.add_clause(s.clone(), nvars);
			}
			// The default planner: the DP
			let mut dp = Planner::new();
			for s in &shapes
			{
				dp.add_clause(s.clone(), nvars);
			}
			for p in [&mut online, &mut batch, &mut dp]
			{
				let plan = p.refine(u64::MAX).clone();
				assert!(plan.optimal && p.error().is_none(), "round {round}: {:?} {plan:?}", p.error());
				assert!(respects(&shapes, nvars, &[], &plan.order), "round {round}: invalid order {:?}", plan.order);
				assert_eq!(plan.cost, best, "round {round}: shapes {shapes:?}");
				assert_eq!(plan.cost, cost_of_order(&shapes, &plan.order, nvars));
				assert_eq!(plan.pending, unbindable(&shapes));
			}
			assert_eq!(dp.plan().source, PlanSource::Dp);
			assert_eq!(dp.solves, 0);
		}
	}

	// Re-weighing clauses while the query is assembled, between refinements and constraints, ends at the optimum of the final weights:
	// by the DP, and by SAT, whose solver starts over when the clause is already encoded
	#[test]
	fn reweighing_matches_brute_force()
	{
		let mut rng = Rng::new(21);
		let mut restarts = 0;
		for round in 0..40
		{
			let n = 2 + round % 5;
			let nvars = 1 + (rng.next() % 4) as usize;
			let mut shapes = random_query(&mut rng, n, nvars);
			let (mut dp, mut sat) = (Planner::new(), sat_planner());
			let mut before = Vec::new();
			for i in 0..n
			{
				dp.add_clause(shapes[i].clone(), nvars);
				sat.add_clause(shapes[i].clone(), nvars);
				if rng.next() % 3 != 0
				{
					sat.refine(if rng.next() % 2 == 0 { u64::MAX } else { 1 });
					dp.refine(u64::MAX);
				}
				let c = (rng.next() % (i as u64 + 1)) as usize;
				let weights = if shapes[c].binds.is_empty()
				{
					[weight_of_fanout(1.0 / (1 + rng.next() % 8) as f64); 4]
				}
				else
				{
					monotone([0; 4].map(|_| weight_of_fanout((rng.next() % 5000) as f64 / 16.0)))
				};
				restarts += usize::from(c < sat.encoded && shapes[c].weights != weights);
				shapes[c].weights = weights;
				dp.reweigh(c, weights);
				sat.reweigh(c, weights);
				assert!(!sat.plan().optimal || sat.plan().cost == cost_of_order(&shapes[..=i], &sat.plan().order, nvars), "round {round}: a stale plan");
				let (a, b) = ((rng.next() % (i as u64 + 1)) as usize, (rng.next() % (i as u64 + 1)) as usize);
				if a != b && rng.next() % 3 == 0
				{
					let accepted = dp.require_before(a, b).is_ok();
					assert_eq!(sat.require_before(a, b).is_ok(), accepted, "round {round}: {a} before {b}");
					if accepted
					{
						before.push((a, b));
					}
				}
			}
			let best = brute_force(&shapes, nvars, &before).unwrap();
			for p in [&mut dp, &mut sat]
			{
				let plan = p.refine(u64::MAX).clone();
				assert!(plan.optimal && p.error().is_none(), "round {round}: {:?} {plan:?}", p.error());
				assert!(respects(&shapes, nvars, &before, &plan.order), "round {round}: invalid order {:?}", plan.order);
				assert_eq!(plan.cost, best, "round {round}: shapes {shapes:?} constraints {before:?}");
				assert_eq!(plan.cost, cost_of_order(&shapes, &plan.order, nvars));
			}
		}
		assert!(restarts > 20, "{restarts} restarts of the solver");
	}

	#[test]
	fn pending_predicate_becomes_plannable()
	{
		for mut p in [Planner::new(), sat_planner()]
		{
			p.add_clause(pattern([Some(0), None], 100.0), 2);
			p.add_clause(predicate(vec![1]), 2);
			assert_eq!(p.refine(u64::MAX).pending, vec![1]);
			p.add_clause(pattern([Some(0), Some(1)], 10.0), 2);
			let plan = p.refine(u64::MAX).clone();
			assert!(plan.pending.is_empty() && plan.optimal);
			assert!(plan.order.iter().position(|&c| c == 2) < plan.order.iter().position(|&c| c == 1));
		}
	}

	#[test]
	fn budget_gives_a_valid_anytime_plan()
	{
		let mut rng = Rng::new(3);
		let shapes = random_query(&mut rng, 12, 6);
		let mut p = sat_planner();
		for s in &shapes
		{
			p.add_clause(s.clone(), 6);
		}
		let quick = p.refine(1).clone();
		assert!(quick.cost <= cost_of_order(&shapes, &greedy_order(&shapes, 6, &[]).unwrap(), 6));
		let full = p.refine(u64::MAX).clone();
		assert!(full.optimal && full.cost <= quick.cost);
		assert!(respects(&shapes, 6, &[], &full.order));
		assert_eq!(full.cost, dp_order(&shapes, 6, &[]).unwrap().1);
	}

	#[test]
	fn constraint_examples()
	{
		// A pending predicate waits for the binders that exist: P (1) needs x and y, and only x has a binder (A, 0)
		let mut p = Planner::new();
		p.add_clause(pattern([Some(0), None], 100.0), 2);
		p.add_clause(predicate(vec![0, 1]), 2);
		assert_eq!(p.require_before(1, 0), Err(vec![(1, 0)]));
		assert_eq!(p.require_before(0, 1), Ok(0));
		// A binder of y, cheap enough to go first; P still follows A
		p.add_clause(pattern([Some(1), None], 1.0), 2);
		let plan = p.refine(u64::MAX).clone();
		assert!(plan.optimal && plan.pending.is_empty());
		assert_eq!(plan.order, vec![2, 0, 1]);
		// P needs y, whose only binder is B (2)
		assert_eq!(p.require_before(1, 2), Err(vec![(1, 2)]));

		// A cycle: every constraint of it is needed, and the others aren't
		let mut p = Planner::new();
		for _ in 0..4
		{
			p.add_clause(pattern([Some(0), None], 10.0), 1);
		}
		assert_eq!(p.require_before(0, 1), Ok(0));
		assert_eq!(p.require_before(3, 2), Ok(1));
		assert_eq!(p.require_before(1, 2), Ok(2));
		assert_eq!(p.require_before(2, 0), Err(vec![(0, 1), (1, 2), (2, 0)]));
		let plan = p.refine(u64::MAX).clone();
		assert!(plan.optimal && respects(&p.shapes, 1, &[(0, 1), (3, 2), (1, 2)], &plan.order));

		// Not syntactic: a predicate (2) may follow either binder of its variable (0 and 1), but the constraints put it before both
		let mut p = Planner::new();
		p.add_clause(pattern([Some(0), None], 10.0), 1);
		p.add_clause(pattern([Some(0), None], 20.0), 1);
		p.add_clause(predicate(vec![0]), 1);
		p.add_clause(pattern([None, None], 5.0), 1);
		assert_eq!(p.require_before(2, 0), Ok(0));
		assert_eq!(p.require_before(3, 1), Ok(1));
		assert_eq!(p.require_before(2, 3), Err(vec![(2, 0), (3, 1), (2, 3)]));
		assert_eq!(p.refine(u64::MAX).order, vec![3, 1, 2, 0]);
		assert_eq!(p.fallback_cores, 0);
	}

	#[test]
	fn constraints_and_cores_match_brute_force()
	{
		let mut rng = Rng::new(11);
		let (mut refused, mut largest) = (0, 0);
		for round in 0..150
		{
			let n = 2 + round % 5;
			let nvars = 1 + (rng.next() % 4) as usize;
			let shapes = random_query(&mut rng, n, nvars);
			// Constraints arrive between the clauses, so that clauses also arrive after constraints (pending predicates included)
			let mut planners = [Planner::new(), sat_planner()];
			let mut accepted: Vec<(usize, usize)> = Vec::new();
			for c in 0..n
			{
				for p in &mut planners
				{
					p.add_clause(shapes[c].clone(), nvars);
					p.refine(100);
				}
				for _ in 0..if c == 0 { 0 } else { rng.next() % 3 }
				{
					let a = (rng.next() % (c as u64 + 1)) as usize;
					let b = (a + 1 + (rng.next() % c as u64) as usize) % (c + 1);
					let mut all = accepted.clone();
					all.push((a, b));
					let feasible = brute_force(&shapes[..=c], nvars, &all).is_some();
					for p in &mut planners
					{
						match p.require_before(a, b)
						{
							Ok(i) => assert!(feasible && i == accepted.len(), "round {round}: accepted {all:?}"),
							Err(core) =>
							{
								assert!(!feasible, "round {round}: refused {all:?}");
								assert!(core.contains(&(a, b)) && core.iter().all(|x| all.contains(x)), "round {round}: core {core:?} of {all:?}");
								assert!(brute_force(&shapes[..=c], nvars, &core).is_none(), "round {round}: core {core:?} admits an order");
								for k in 0..core.len()
								{
									let rest = [&core[..k], &core[k + 1..]].concat();
									assert!(brute_force(&shapes[..=c], nvars, &rest).is_some(), "round {round}: core {core:?} isn't minimal");
								}
								largest = largest.max(core.len());
							}
						}
					}
					if feasible
					{
						accepted.push((a, b));
					}
					else
					{
						refused += 1;
					}
				}
			}
			let best = brute_force(&shapes, nvars, &accepted).expect("accepted constraints keep admitting an order");
			assert_eq!(dp_order(&shapes, nvars, &accepted).unwrap().1, best);
			for p in &mut planners
			{
				let plan = p.refine(u64::MAX).clone();
				assert!(plan.optimal && p.error().is_none(), "round {round}: {:?}", p.error());
				assert!(respects(&shapes, nvars, &accepted, &plan.order), "round {round}: {:?} breaks {accepted:?}", plan.order);
				assert_eq!(plan.cost, best, "round {round}: {shapes:?} {accepted:?}");
				assert_eq!(plan.pending, unbindable(&shapes));
				assert_eq!(p.fallback_cores, 0, "round {round}: the SAT core wasn't confirmed");
			}
		}
		assert!(refused > 50 && largest >= 3, "refused {refused}, largest core {largest}");
	}

	#[test]
	fn dp_matches_sat_beyond_brute_force()
	{
		let mut rng = Rng::new(23);
		for round in 0..4
		{
			let (n, nvars) = (8, 4);
			let shapes = random_query(&mut rng, n, nvars);
			let before = if round < 2 { Vec::new() } else { random_constraints(&mut rng, &shapes, nvars, 4) };
			let (order, best) = dp_order(&shapes, nvars, &before).unwrap();
			assert!(respects(&shapes, nvars, &before, &order) && cost_of_order(&shapes, &order, nvars) == best);
			let mut p = sat_planner();
			for s in &shapes
			{
				p.add_clause(s.clone(), nvars);
			}
			for &(a, b) in &before
			{
				p.require_before(a, b).unwrap();
			}
			let plan = p.refine(u64::MAX).clone();
			assert!(plan.optimal && respects(&shapes, nvars, &before, &plan.order));
			assert_eq!(plan.cost, best, "round {round}: {shapes:?} {before:?}");
		}
	}

	#[test]
	fn online_refinement_is_bounded()
	{
		let mut rng = Rng::new(5);
		let (n, nvars) = (24, 10);
		let shapes = random_query(&mut rng, n, nvars);
		let mut p = Planner::new();
		let mut before = Vec::new();
		for (c, s) in shapes.iter().enumerate()
		{
			if c == 18
			{
				// A constraint on the way
				let cons = random_constraints(&mut rng, &shapes[..c], nvars, 3);
				assert!(!cons.is_empty());
				for &(a, b) in &cons
				{
					p.require_before(a, b).unwrap();
				}
				before.extend(cons);
			}
			p.add_clause(s.clone(), nvars);
			let (solves, conflicts) = (p.solves, p.conflicts);
			let plan = p.refine(1000).clone();
			// The solver checks its threshold between rounds of conflict analysis, so a solve can overshoot by a few conflicts (3 measured)
			let (solves, conflicts) = (p.solves - solves, p.conflicts - conflicts);
			assert!(solves <= 1000 && conflicts <= 1000 + 8 * solves, "clause {c}: {solves} solves, {conflicts} conflicts");
			assert!(respects(&shapes[..=c], nvars, &before, &plan.order));
			assert!(plan.cost <= cost_of_order(&shapes[..=c], &greedy_order(&shapes[..=c], nvars, &before).unwrap(), nvars));
			if c < DP_MAX
			{
				assert!(plan.optimal && plan.source == PlanSource::Dp && p.solves == 0);
			}
			// Without a budget, nothing is solved
			let solves = p.solves;
			p.refine(0);
			assert_eq!(p.solves, solves);
		}
	}

	// A change of the query carries the previous plan over: after every added clause and accepted constraint, the plan is valid,
	// its cost is its order's, and it is never worse than the greedy plan; past DP_MAX, the DP's optimum is carried into the larger query
	#[test]
	fn plans_carry_over_changes()
	{
		let mut rng = Rng::new(17);
		let mut carried = 0;
		for _ in 0..4
		{
			let (n, nvars) = (22, 8);
			let shapes = random_query(&mut rng, n, nvars);
			let mut p = Planner::new();
			let mut before = Vec::new();
			let check = |p: &Planner, c: usize, before: &[(usize, usize)]|
			{
				let plan = p.plan();
				assert!(respects(&shapes[..=c], nvars, before, &plan.order), "clause {c}: {:?}", plan.order);
				assert_eq!(plan.cost, cost_of_order(&shapes[..=c], &plan.order, nvars));
				assert!(plan.cost <= cost_of_order(&shapes[..=c], &greedy_order(&shapes[..=c], nvars, before).unwrap(), nvars));
				assert_eq!(plan.pending, unbindable(&shapes[..=c]));
			};
			for (c, s) in shapes.iter().enumerate()
			{
				p.add_clause(s.clone(), nvars);
				check(&p, c, &before);
				carried += (c >= DP_MAX && p.plan().source == PlanSource::Carried) as usize;
				if c % 5 == 4
				{
					for (a, b) in random_constraints(&mut rng, &shapes[..=c], nvars, 2)
					{
						if !before.contains(&(a, b)) && p.require_before(a, b).is_ok()
						{
							before.push((a, b));
							check(&p, c, &before);
						}
					}
				}
				p.refine(if c < DP_MAX { u64::MAX } else { 20 });
				check(&p, c, &before);
			}
		}
		assert!(carried > 0, "no plan past DP_MAX was carried over");
	}

	// Refinements in small budgets add up: every solve goes to the minimization (what is pending is known in closed form),
	// and what the solver learns stays, so repeated refine(1) reaches the optimum
	#[test]
	fn small_refinements_make_progress()
	{
		let mut rng = Rng::new(11);
		for _ in 0..10
		{
			let (n, nvars) = (5, 4);
			let shapes = random_query(&mut rng, n, nvars);
			let mut p = sat_planner();
			for s in &shapes
			{
				p.add_clause(s.clone(), nvars);
			}
			for _ in 0..5000
			{
				if p.refine(1).optimal
				{
					break;
				}
			}
			assert!(p.plan().optimal, "5000 refinements of one conflict didn't reach the optimum");
			assert_eq!(Some(p.plan().cost), brute_force(&shapes, nvars, &[]));
		}
		// On a large query, each refine(1) searches (a conflict each)
		let shapes = random_query(&mut rng, 20, 10);
		let mut p = Planner::new();
		for s in &shapes
		{
			p.add_clause(s.clone(), 10);
		}
		for _ in 0..100
		{
			p.refine(1);
		}
		assert!(p.solver.GetConflictsNumber() >= 50, "{} conflicts in 100 refinements", p.solver.GetConflictsNumber());
	}

	// The plans of an online refinement after each added clause past DP_MAX, by budget: their mean cost relative to the greedy plan's,
	// and how often SAT improved the plan (release: cargo test --release online_refinement_budgets -- --ignored --nocapture)
	#[test]
	#[ignore]
	fn online_refinement_budgets()
	{
		for budget in [1000, 10_000, 100_000]
		{
			let (mut improved, mut total, mut ratio, t) = (0, 0, 0.0, std::time::Instant::now());
			for seed in 0..5
			{
				let mut rng = Rng::new(50 + seed);
				let (n, nvars) = (24, 10);
				let shapes = random_query(&mut rng, n, nvars);
				let mut p = Planner::new();
				for (c, s) in shapes.iter().enumerate()
				{
					p.add_clause(s.clone(), nvars);
					let plan = p.refine(budget).clone();
					if c >= DP_MAX
					{
						let greedy = cost_of_order(&shapes[..=c], &greedy_order(&shapes[..=c], nvars, &[]).unwrap(), nvars);
						total += 1;
						improved += (plan.source == PlanSource::Sat) as usize;
						ratio += plan.cost as f64 / greedy as f64;
					}
				}
			}
			println!("budget {budget}: mean cost {:.3} of the greedy plan's; SAT improved {improved} of {total} plans; {:?}", ratio / total as f64, t.elapsed());
		}
	}

	// A budget just below u64::MAX is (nearly) unlimited: the solver's threshold is its conflict count plus the budget, which must not wrap
	#[test]
	fn huge_budget_is_not_zero()
	{
		let mut rng = Rng::new(7);
		let (n, nvars) = (6, 4);
		for _ in 0..20
		{
			let shapes = random_query(&mut rng, n, nvars);
			let mut p = sat_planner();
			for s in &shapes
			{
				p.add_clause(s.clone(), nvars);
			}
			// Spend a few conflicts first, so that the solver's count is past 0
			for _ in 0..100
			{
				if p.plan().optimal || p.solver.GetConflictsNumber() >= 2
				{
					break;
				}
				p.refine(1);
			}
			let plan = p.refine(u64::MAX - 1).clone();
			assert!(plan.optimal);
			assert_eq!(Some(plan.cost), brute_force(&shapes, nvars, &[]));
		}
	}

	// Timings of the DP and of SAT (release: cargo test --release planner_timings -- --ignored --nocapture)
	#[test]
	#[ignore]
	fn planner_timings()
	{
		for n in [8, 12, 16, 20]
		{
			for seed in 0..3
			{
				let mut rng = Rng::new(100 + seed);
				let nvars = n / 2;
				let shapes = random_query(&mut rng, n, nvars);
				let t = std::time::Instant::now();
				let (_, best) = dp_order(&shapes, nvars, &[]).unwrap();
				let dp = t.elapsed();
				let before = random_constraints(&mut rng, &shapes, nvars, 3);
				let t = std::time::Instant::now();
				let constrained = dp_order(&shapes, nvars, &before).unwrap().1;
				let dpc = t.elapsed();
				let mut line = format!("n={n} seed={seed} dp={dp:?} best={best} | {} constraints: dp={dpc:?} best={constrained}", before.len());
				// SAT (without the DP), unconstrained and constrained
				for (what, cons) in [("sat", &[][..]), ("sat+constraints", &before[..])]
				{
					let mut p = sat_planner();
					let t = std::time::Instant::now();
					for s in &shapes
					{
						p.add_clause(s.clone(), nvars);
					}
					for &(a, b) in cons
					{
						p.require_before(a, b).unwrap();
					}
					let plan = p.refine(if n <= 8 { u64::MAX } else { 200_000 }).clone();
					line += &format!(" | {what}: {:?} optimal={} cost={}", t.elapsed(), plan.optimal, plan.cost);
				}
				println!("{line}");
			}
		}
	}
}
