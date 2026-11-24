// Rust-only extension (no C++ counterpart): vivification of learnt clauses as an inprocessing step (the TODO at the end of Topi.cc: "vivification: see Kissat paper")
//
// A round runs inside InprocessIfRequired (so, on the schedule of /inprocessing/...: at the level of the assumptions, that is, at the start of a query and right after a restart),
// if /inprocessing/vivify/on is set. The solver backtracks to decision level 0 (the assumptions are taken off the trail and re-assigned after the round, precisely as Solve does
// after conflict analysis has backtracked below them), and for every candidate clause C = (l1 ... ln), the long learnt clauses in the order of increasing glue and decreasing activity:
// (1) C is detached from its watches, so BCP ignores it: C can neither be a reason nor a contradiction, while it is tested;
// (2) the negations of its literals are decided one by one, each followed by BCP, until
//     (a) BCP finds a contradiction: the decisions in the implication cone of the contradicting clause are negations of literals of C, and those literals form a clause, implied by the formula; or
//     (b) the next literal is already satisfied: likewise for the decisions in its implication cone, together with the literal itself; or
//     (c) all the literals are falsified (some of them by BCP rather than by a decision): C itself is the contradiction, and the literals falsified by BCP (or at level 0) can be removed;
// (3) the solver backtracks to level 0; if the new clause is shorter than C, it replaces C: the new clause is added (and reported to the DRAT file and the new-learnt-clause callback)
//     before C is deleted (so the new clause is RUP in the proof, even when C itself is the base of its derivation), with the smaller of the two glues and the activity of C.
//     A unit clause is assigned and propagated at level 0, and a contradiction then is a global one.
//
// The effort is bounded by /inprocessing/vivify/effort: a round may propagate that fraction of the literals, which the search has propagated since the previous round.
// A clause, which wasn't shortened, isn't tried again (the hashes of such clauses are held; a stale or a colliding hash can only cost a missed or a repeated attempt).
//
// Only BCP, Assign and Backtrack touch the trail, so their invariants hold throughout. What they change besides the trail is restored: the saved phases (backtracking saves
// the phase of every variable it unassigns), and the record of the earliest falsified assumption (Assign updates it, when a decision or an implication of a round contradicts an assumption,
// which is not on the trail during the round). Neither conflicts nor decisions are counted, and the decision heap is not touched (the round doesn't use Decide).
// The answers stay correct, since every clause added is implied by the formula; the search changes, but only through the clauses.
//
// Off by default: without /inprocessing/vivify/on the hook of InprocessIfRequired returns after testing the parameter
// (the other hook, in Solve right after InprocessIfRequired, returns from the query, if the parameter is on and the round has decided it).

use crate::topi::*;
use std::collections::HashSet;
use std::io::Write;

// The state of the extension (the counters are in m_Stat)
#[derive(Default)]
pub(crate) struct CExtVivify
{
	// The number of literals propagated by the search (that is, not by vivification) till the latest round
	pub(crate) m_SearchPropsAtLastRound: u64,
	// The hashes of the clauses, which were tried and not shortened
	pub(crate) m_Tried: HashSet<u64>,
	// The buffers of a round: the literals of the candidate clause, the new clause and the stack of the implication-cone traversal
	m_Lits: Vec<TULit>,
	m_NewLits: Vec<TULit>,
	m_Stack: Vec<TUVar>,
}

// A round isn't worth its fixed costs (collecting and sorting the candidates), unless it may propagate at least that many literals; the budget is carried over otherwise
const ExtVivifyMinBudget: u64 = 2000;
// The hashes of the tried clauses are dropped, once there are that many
pub(crate) const ExtVivifyMaxTried: usize = 1 << 22;

// An order-independent hash of a clause
fn ExtVivifyHash(lits: &[TULit]) -> u64
{
	lits.iter().fold(0u64, |h, l|
	{
		// The finalizer of splitmix64
		let mut x = (*l as u64).wrapping_add(0x9E3779B97F4A7C15);
		x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
		x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
		h.wrapping_add(x ^ (x >> 31))
	})
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	// The hook of InprocessIfRequired: a vivification round, if the parameter is on and there is a budget for it
	pub(crate) fn ExtVivifyIfRequired(&mut self)
	{
		if !self.m_Params.m_ParamVivifyOn.get() || self.m_Status.get() != TToporStatus::STATUS_UNDECIDED || self.m_EarliestFalsifiedAssump != BadULit
		{
			return;
		}
		// InprocessIfRequired runs at the level of the assumptions, after BCP has completed
		debug_assert!(self.m_DecLevel == self.m_DecLevelOfLastAssignedAssumption && self.m_ToPropagate.empty());

		let searchProps = self.m_Stat.m_Implications - self.m_Stat.m_ExtVivifyPropagations;
		// (a float-to-integer cast saturates)
		let budget = (self.m_Params.m_ParamVivifyEffort.get() * (searchProps - self.m_ExtVivify.m_SearchPropsAtLastRound) as f64) as u64;
		if budget < ExtVivifyMinBudget
		{
			return;
		}
		self.m_ExtVivify.m_SearchPropsAtLastRound = searchProps;

		// The candidates: the long learnt clauses; low glue first, then high activity (the recently used ones), then the order of the buffer
		let maxGlue = self.m_Params.m_ParamVivifyMaxGlue.get();
		let isActivityKept = self.m_Params.m_ParamClsDelStrategy.get() > 0;
		let mut cands: Vec<(TUV, f32, Ind)> = Vec::new();
		let mut clsInd = self.ClsLoopFirst(true);
		while !self.ClsLoopCompleted()
		{
			if !self.ClsChunkDeleted(clsInd) && self.ClsGetIsLearnt(clsInd)
			{
				let glue = self.ClsGetGlue(clsInd);
				if glue <= maxGlue
				{
					cands.push((glue, if isActivityKept { self.ClsGetActivity(clsInd) } else { 0. }, clsInd));
				}
			}
			clsInd = self.ClsLoopNext();
		}
		if cands.is_empty()
		{
			return;
		}
		cands.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));

		if self.m_ExtVivify.m_Tried.len() > ExtVivifyMaxTried
		{
			self.m_ExtVivify.m_Tried.clear();
		}

		// To level 0: the assumptions leave the trail for the duration of the round
		let assumpDecLevel = self.m_DecLevelOfLastAssignedAssumption;
		self.Backtrack(0, false, false);
		self.m_DecLevelOfLastAssignedAssumption = 0;
		// Backtracking saves the phases; those of the search are kept
		let savedPhases: Vec<bool> = (0..self.GetNextVar() as usize).map(|v| self.m_AssignmentInfo[v].m_IsNegated()).collect();

		self.m_Stat.m_ExtVivifyRounds += 1;
		let propsAtStart = self.m_Stat.m_Implications;
		for cand in &cands
		{
			if self.m_Stat.m_Implications - propsAtStart >= budget || self.m_Status.get() != TToporStatus::STATUS_UNDECIDED || self.m_EarliestFalsifiedAssump != BadULit
			{
				break;
			}
			self.ExtVivifyCls(cand.2);
		}
		self.m_Stat.m_ExtVivifyPropagations += self.m_Stat.m_Implications - propsAtStart;

		debug_assert!(self.m_DecLevel == 0 || self.IsUnrecoverable());
		for (v, isNegated) in savedPhases.iter().enumerate().skip(1)
		{
			if !self.m_AssignmentInfo[v].m_IsAssigned()
			{
				self.m_AssignmentInfo[v].set_m_IsNegated(*isNegated);
			}
		}

		self.m_DecLevelOfLastAssignedAssumption = assumpDecLevel;
		if self.m_Status.get() != TToporStatus::STATUS_UNDECIDED
		{
			// A global contradiction, an error or a stop request of the new-learnt-clause callback: Solve returns right after the hook
			return;
		}

		if assumpDecLevel > 0
		{
			// The assumptions are back to the trail, precisely as in Solve, when conflict analysis has backtracked below them
			debug_assert!(self.m_Assumps.cap() != 0);
			self.m_FlippedLit = BadULit;
			self.m_EarliestFalsifiedAssump = BadULit;
			// A unit clause of the round might have falsified an assumption at level 0
			for l in self.m_Assumps.get_span_cap(0)
			{
				let v = GetVar(l);
				if self.IsAssigned(l) && self.IsAssumpFalsifiedGivenVar(v) && (self.m_EarliestFalsifiedAssump == BadULit || self.GetAssignedDecLevel(l) <= self.GetAssignedDecLevel(self.m_EarliestFalsifiedAssump))
				{
					self.m_EarliestFalsifiedAssump = l;
					self.m_LatestEarliestFalsifiedAssump = l;
					self.m_LatestEarliestFalsifiedAssumpSolveInv = self.m_Stat.m_SolveInvs;
				}
			}

			if self.m_EarliestFalsifiedAssump == BadULit
			{
				self.HandleAssumptionsIfBacktrackedBeyondThem();
				let assumpsSpan = self.m_Assumps.get_span_cap(0);
				self.m_DecLevelOfLastAssignedAssumption = self.GetDecLevel0ForUnassigned(assumpsSpan.get(self.GetLitsHighestDecLevel0ForUnassignedIt(assumpsSpan, 0)));
			}

			if self.m_EarliestFalsifiedAssump != BadULit && !self.IsUnrecoverable()
			{
				debug_assert!(self.IsAssigned(self.m_EarliestFalsifiedAssump));
				self.SetStatus(TToporStatus::STATUS_UNSAT, "Assumption flipped after vivification!".to_string());
			}
		}

		if self.m_Status.get() == TToporStatus::STATUS_UNDECIDED && self.m_AssignedVarsNum == self.m_LastExistingVar
		{
			// The unit clauses of the round (and the assumptions) have assigned all the variables: there is nothing left to decide
			self.SetStatus(TToporStatus::STATUS_SAT, "A model: discovered after vivification".to_string());
		}
	}

	// Vivify one clause; the solver is at decision level 0 before and after
	fn ExtVivifyCls(&mut self, clsInd: Ind)
	{
		debug_assert!(self.m_DecLevel == 0 && self.m_ToPropagate.empty());
		debug_assert!(!self.ClsChunkDeleted(clsInd) && self.ClsGetIsLearnt(clsInd));

		let mut lits = std::mem::take(&mut self.m_ExtVivify.m_Lits);
		let mut newLits = std::mem::take(&mut self.m_ExtVivify.m_NewLits);
		lits.clear();
		newLits.clear();
		let cls = self.Cls(clsInd);
		lits.extend((0..cls.size()).map(|i| cls.get(i)));

		// A clause, which is satisfied at level 0, is left to SimplifyIfRequired; the watches of any other clause are unassigned at level 0, once BCP has completed
		if !lits.iter().any(|l| self.IsSatisfied(*l)) && !self.IsAssigned(lits[0]) && !self.IsAssigned(lits[1]) && !self.m_ExtVivify.m_Tried.contains(&ExtVivifyHash(&lits))
		{
			self.m_Stat.m_ExtVivifyTried += 1;
			// Assign records the earliest falsified assumption: a decision or an implication of the test may contradict an assumption (none is on the trail now)
			let assumpInfo = (self.m_EarliestFalsifiedAssump, self.m_LatestEarliestFalsifiedAssump, self.m_LatestEarliestFalsifiedAssumpSolveInv);

			self.ExtVivifyProbe(clsInd, &lits, &mut newLits);
			let isShortened = !newLits.is_empty() && newLits.len() < lits.len() && !self.IsUnrecoverable();

			if isShortened
			{
				// The new clause is added, while its literals are still assigned (their decision levels make its glue), and before the old one is deleted
				// (AddClsToBufferAndWatch serves the DRAT file and the new-learnt-clause callback; a unit clause isn't stored)
				if newLits.len() == 1
				{
					if self.IsCbLearntOrDrat()
					{
						self.NewLearntClsApplyCbLearntDrat(Span::from_slice(&mut newLits));
					}
				}
				else
				{
					let newClsInd = self.AddClsToBufferAndWatch(Span::from_slice(&mut newLits), true, true);
					if newLits.len() > 2 && !self.IsUnrecoverable()
					{
						// The glue of the new clause is that of the artificial decision levels of the test; the glue of the old clause bounds the glue of its subset too
						let oldGlue = self.ClsGetGlue(clsInd);
						if oldGlue < self.ClsGetGlue(newClsInd)
						{
							self.ClsSetGlue(newClsInd, oldGlue);
						}
						if self.m_Params.m_ParamClsDelStrategy.get() > 0
						{
							let (oldActivity, oldSkipdel) = (self.ClsGetActivity(clsInd), self.ClsGetSkipdel(clsInd));
							self.ClsSetActivity(newClsInd, oldActivity);
							self.ClsSetSkipdel(newClsInd, oldSkipdel);
						}
					}
				}
				if !self.IsUnrecoverable()
				{
					self.ExtVivifyDratDelete(&lits);
				}
			}

			self.Backtrack(0, false, false);
			(self.m_EarliestFalsifiedAssump, self.m_LatestEarliestFalsifiedAssump, self.m_LatestEarliestFalsifiedAssumpSolveInv) = assumpInfo;

			// Re-attach the old clause: its two first literals are still its watches, and they are unassigned again
			self.WLAddLongWatch(lits[0], lits[1], clsInd);
			self.WLAddLongWatch(lits[1], lits[0], clsInd);
			self.m_Stat.m_ActiveLongClss += 1;

			if isShortened && !self.IsUnrecoverable()
			{
				self.DeleteCls(clsInd, None);
				self.m_Stat.m_ExtVivifyShortened += 1;
				self.m_Stat.m_ExtVivifyLitsRemoved += (lits.len() - newLits.len()) as u64;
				if newLits.len() == 1
				{
					// A new global unit clause
					self.m_Stat.m_ExtVivifyUnits += 1;
					let isContraditory = self.Assign(newLits[0], Self::BadClsInd, BadULit, 0, true, false);
					debug_assert!(!isContraditory);
					let _ = isContraditory;
					let contradictionInfo = self.BCP();
					if contradictionInfo.IsContradiction() && !self.IsUnrecoverable()
					{
						self.SetStatus(TToporStatus::STATUS_CONTRADICTORY, "Global contradiction: discovered by vivification".to_string());
					}
				}
			}
			else
			{
				self.m_ExtVivify.m_Tried.insert(ExtVivifyHash(&lits));
			}
		}

		self.m_ExtVivify.m_Lits = lits;
		self.m_ExtVivify.m_NewLits = newLits;
	}

	// Test the clause clsInd, whose literals are lits, at decision level 0: detach it, decide the negations of its literals and propagate.
	// If a clause over a subset of the literals is derived, it is returned in newLits (empty otherwise). The solver is left at the decision levels of the test,
	// where every literal of the new clause is assigned, and the clause is left detached.
	fn ExtVivifyProbe(&mut self, clsInd: Ind, lits: &[TULit], newLits: &mut Vec<TULit>)
	{
		// BCP must neither propagate through the clause nor find it contradicting, while its literals are being falsified, so its watches are removed
		// (and it isn't counted for the duration, so that the consistency assertions of BCP see as many long clauses as there are watched ones)
		for watchI in 0..=1
		{
			let wlInd = self.WLGetLongWatchInd(lits[watchI], clsInd);
			debug_assert!(wlInd != usize::MAX);
			self.WLRemoveLongWatch(lits[watchI], wlInd);
		}
		self.m_Stat.m_ActiveLongClss -= 1;

		// The variables, whose implication cones the new clause is derived from
		let mut stack = std::mem::take(&mut self.m_ExtVivify.m_Stack);
		stack.clear();
		let mut impliedLit = BadULit;
		let mut isContradiction = false;
		for &l in lits
		{
			if self.IsAssigned(l)
			{
				if self.IsFalsified(l)
				{
					// Falsified at level 0 or implied by the previous decisions: the literal is redundant, if the rest of the literals get falsified too
					continue;
				}
				// Implied by the previous decisions
				impliedLit = l;
				break;
			}

			self.NewDecLevel();
			let isContraditory = self.Assign(Negate(l), Self::BadClsInd, BadULit, self.m_DecLevel, true, false);
			debug_assert!(!isContraditory);
			let _ = isContraditory;
			let mut contradictionInfo = self.BCP();
			if contradictionInfo.IsContradiction() || self.IsUnrecoverable()
			{
				isContradiction = true;
				if !self.IsUnrecoverable()
				{
					let ciSpan = self.CiGetSpan(&mut contradictionInfo, TUV::MAX);
					stack.extend(ciSpan.iter().map(GetVar));
				}
				break;
			}
		}

		if impliedLit != BadULit
		{
			// The literal is implied by a clause other than ours: all the other literals of its parent are falsified
			debug_assert!(self.GetAssignedDecLevel(impliedLit) > 0 && !self.IsAssignedDec(impliedLit));
			let parent = self.GetAssignedNonDecParentSpan(impliedLit);
			stack.extend(parent.iter().filter(|l| *l != impliedLit).map(GetVar));
			newLits.push(impliedLit);
		}
		else if !isContradiction
		{
			// Every literal is falsified, so our clause is the contradiction (BCP is complete before every decision, so it never backtracks during the test)
			debug_assert!(lits.iter().all(|l| self.IsFalsified(*l)));
			stack.extend(lits.iter().filter(|l| self.IsFalsified(**l)).map(|l| GetVar(*l)));
		}

		// The decisions in the implication cones: every decision above level 0 is the negation of a literal of our clause
		while let Some(v) = stack.pop()
		{
			if self.IsVisitedVar(v) || self.GetAssignedDecLevelVar(v) == 0
			{
				continue;
			}
			self.MarkVisitedVar(v);
			if !self.IsAssignedDecVar(v)
			{
				// (in the compressed mode the span is a temporary copy of the parent, which is valid until the next clause is fetched)
				let parent = self.GetAssignedNonDecParentSpanVar(v);
				stack.extend(parent.iter().map(GetVar).filter(|u| *u != v));
			}
		}
		newLits.extend(lits.iter().filter(|l| self.IsVisited(**l) && self.IsAssignedDec(**l)));
		debug_assert!(newLits.iter().skip((impliedLit != BadULit) as usize).all(|l| self.IsFalsified(*l)));
		self.CleanVisited();
		self.m_ExtVivify.m_Stack = stack;
	}

	// The deletion of a clause in the DRAT file (the solver doesn't report its other deletions; this one follows the addition of the clause, which replaces the deleted one)
	fn ExtVivifyDratDelete(&mut self, lits: &[TULit])
	{
		if self.m_OpenedDratFile.is_none()
		{
			return;
		}

		let mut userLits: Vec<TLit> = lits.iter().map(|l| self.GetExternalLit(*l)).collect();
		if self.m_DratSortEveryClause
		{
			userLits.sort_unstable_by_key(|l| l.unsigned_abs());
		}

		// The encodings are those of NewLearntClsApplyCbLearntDrat, with d instead of a
		let mut buf = std::mem::take(&mut self.m_DratBuf);
		buf.clear();
		if self.m_IsDratBinary
		{
			buf.push(b'd');
			for l in userLits
			{
				let mut ul64: u64 = ((l.unsigned_abs() as u64) << 1) + (l < 0) as u64;
				while ul64 & !0x7f != 0
				{
					buf.push(((ul64 & 0x7f) | 0x80) as u8);
					ul64 >>= 7;
				}
				buf.push(ul64 as u8);
			}
			buf.push(0);
		}
		else
		{
			buf.extend_from_slice(b"d ");
			for l in userLits
			{
				// Writing into a vector can't fail
				let _ = write!(buf, "{} ", l);
			}
			buf.extend_from_slice(b"0\n");
		}

		let isDratBinary = self.m_IsDratBinary;
		let o = self.m_OpenedDratFile.as_mut().unwrap();
		let isGood = o.write_all(&buf).is_ok() && (isDratBinary || o.flush().is_ok());
		self.m_DratBuf = buf;
		if !isGood
		{
			self.OnBadDratFile();
		}
	}

	// The extra line of the short statistics, if the extension is on
	pub(crate) fn ExtVivifyStatStr(&self) -> String
	{
		if !self.m_Params.m_ParamVivifyOn.get()
		{
			return String::new();
		}
		format!("c ToporExt VivifyRounds {} Tried {} Shortened {} LitsRemoved {} Units {} Props {} SearchProps {}\n", self.m_Stat.m_ExtVivifyRounds, self.m_Stat.m_ExtVivifyTried, self.m_Stat.m_ExtVivifyShortened, self.m_Stat.m_ExtVivifyLitsRemoved, self.m_Stat.m_ExtVivifyUnits, self.m_Stat.m_ExtVivifyPropagations, self.m_Stat.m_Implications - self.m_Stat.m_ExtVivifyPropagations)
	}
}
