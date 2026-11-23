// Fuzz tests: random incremental scenarios over the public API, verified against an exact oracle (see fuzzing.rs)
// The number of scenarios can be changed with the environment variable TOPOR_FUZZ_ITERS

use crate::fuzzing::*;
use crate::topor_dyn_array::alloc_fault;

fn Iters(default: u64) -> u64
{
	std::env::var("TOPOR_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn ScenarioBytes(seed: u64) -> Vec<u8>
{
	RandomBytes(seed, 200 + (seed as usize * 37) % 3000)
}

// The random scenarios are split to several tests, so that they would run in parallel
fn FuzzRange(part: u64)
{
	let iters = Iters(2000);
	for seed in (0..iters).filter(|s| s % 8 == part)
	{
		FuzzOne(&ScenarioBytes(seed), false);
	}
}

// The big scenarios (see GenScenario)
fn FuzzBigRange(part: u64)
{
	for seed in (0..Iters(2000) / 10).filter(|s| s % 4 == part)
	{
		FuzzOne(&RandomBytes(seed, 20000), true);
	}
}

#[test]
fn fuzz_big_scenarios_0()
{
	FuzzBigRange(0);
}

#[test]
fn fuzz_big_scenarios_1()
{
	FuzzBigRange(1);
}

#[test]
fn fuzz_big_scenarios_2()
{
	FuzzBigRange(2);
}

#[test]
fn fuzz_big_scenarios_3()
{
	FuzzBigRange(3);
}

#[test]
fn fuzz_random_scenarios_0()
{
	FuzzRange(0);
}

#[test]
fn fuzz_random_scenarios_1()
{
	FuzzRange(1);
}

#[test]
fn fuzz_random_scenarios_2()
{
	FuzzRange(2);
}

#[test]
fn fuzz_random_scenarios_3()
{
	FuzzRange(3);
}

#[test]
fn fuzz_random_scenarios_4()
{
	FuzzRange(4);
}

#[test]
fn fuzz_random_scenarios_5()
{
	FuzzRange(5);
}

#[test]
fn fuzz_random_scenarios_6()
{
	FuzzRange(6);
}

#[test]
fn fuzz_random_scenarios_7()
{
	FuzzRange(7);
}

// Memory-out fuzzing: every allocation of a scenario is made to fail in turn (and all the allocations after it)
// The solver tests most of its allocations and moves to the unrecoverable memory-out status, when one fails: this is what the test exercises.
// However, precisely as in the C++ version, not every allocation is tested (e.g., the ones inside the vectors, which silently drop the pushed element),
// so after an untested allocation fails the solver may proceed to hit an assertion (the C++ version would crash) or to provide an answer for a different formula.
// Hence, whatever happens after the injected failure is tolerated here, as long as the process survives.
fn FuzzAllocFailures(solverMode: u8)
{
	for seed in 0..Iters(2000) / 100
	{
		let s = GenScenario(&ScenarioBytes(1000003 * (seed + 1) + solverMode as u64), seed % 10 == 9);
		let before = alloc_fault::allocs_so_far();
		RunScenario(&s, solverMode, None);
		let allocs = alloc_fault::allocs_so_far() - before;
		let step = 1 + allocs / 150;
		let mut failAt = seed % step;
		while failAt < allocs
		{
			let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RunScenario(&s, solverMode, Some(failAt))));
			alloc_fault::never_fail();
			failAt += step;
		}
	}
}

#[test]
fn fuzz_alloc_failures_mode_0()
{
	FuzzAllocFailures(0);
}

#[test]
fn fuzz_alloc_failures_mode_1()
{
	FuzzAllocFailures(1);
}

#[test]
fn fuzz_alloc_failures_mode_2()
{
	FuzzAllocFailures(2);
}

#[test]
fn fuzz_input_and_oracle()
{
	let s = TFuzzScenario { m_VarsNumHint: 3, m_Verbosity: 1, m_Ops: vec![TFuzzOp::AddClause(vec![1, -2, 0, 3]), TFuzzOp::Solve(vec![2], 5), TFuzzOp::Solve(vec![], u64::MAX), TFuzzOp::SetParam("/a/b".to_string(), 0.5),
		TFuzzOp::FixPolarity(-1, true), TFuzzOp::BoostScore(2, 10.), TFuzzOp::ClearUserPolarityInfo(1), TFuzzOp::CreateInternalLit(-3), TFuzzOp::Backtrack(true), TFuzzOp::Backtrack(false), TFuzzOp::ChangeConfigToGiven(7), TFuzzOp::Stats], ..Default::default() };
	assert_eq!(ScenarioToCnf(&s), "p cnf 3 0\nr /verbosity/level 1\n1 -2 0\noc 5\ns 2 0\ns 0\nr /a/b 0.5\nlf -1 1\nlb 2 10\nlc 1\nll -3\nb 0\nc b <half of the highest decision level>\nn 7\n");

	let mut inp = CFuzzInput::new(&[7, 200, 1, 2]);
	assert_eq!(inp.Byte(), 7);
	assert_eq!(inp.Below(100), 0);
	assert_eq!(inp.Below(1), 0);
	assert_eq!(inp.Below(1000), 258);
	assert!(inp.Exhausted());
	assert_eq!(inp.Byte(), 0);
	assert!(inp.Chance(1));
	assert_eq!(RandomBytes(5, 13).len(), 13);
	assert_ne!(RandomBytes(5, 64), RandomBytes(6, 64));
	assert!(FuzzParams().len() > 100);

	for vars in [1, 3, 7, 9]
	{
		let mut o = CFuzzOracle::new(vars);
		let last = vars as i32;
		assert!(o.IsSatUnder(&[]) && o.IsSatUnder(&[last]) && !o.IsSatUnder(&[last, -last]));
		assert!(!o.IsImplied(&[last]) && o.IsImplied(&[1, -1]));
		o.AddClause(&[1, last]);
		o.AddClause(&[-1, last]);
		assert!(o.IsImplied(&[last]) && !o.IsSatUnder(&[-last]) && o.IsSatUnder(&[last]));
		o.AddClause(&[]);
		assert!(!o.IsSatUnder(&[]));
	}
}

// Delta-debugging (the counterpart of scripts/delta_debug_intel_sat.csh): finds the first failing scenario and shrinks it to a locally minimal one
// Usage: TOPOR_FUZZ_ITERS=<scenarios to try> cargo test fuzz_delta_debug -- --ignored --nocapture
#[test]
#[ignore]
fn fuzz_delta_debug()
{
	let Fails = |s: &TFuzzScenario, solverMode: u8| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RunScenario(s, solverMode, None))).is_err();
	std::panic::set_hook(Box::new(|_| ()));
	let firstSeed = std::env::var("TOPOR_FUZZ_FIRST_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
	for seed in firstSeed..firstSeed + Iters(2000)
	{
		let isBig = std::env::var("TOPOR_FUZZ_BIG").is_ok();
		let mut s = GenScenario(&if isBig { RandomBytes(seed, 20000) } else { ScenarioBytes(seed) }, isBig);
		let solverMode = match (0..3).find(|m| Fails(&s, *m))
		{
			Some(m) => m,
			None => continue,
		};
		// Remove chunks of operations as long as the scenario still fails
		let mut chunk = s.m_Ops.len() / 2;
		while chunk >= 1
		{
			let mut i = 0;
			while i < s.m_Ops.len()
			{
				let mut cand = s.clone();
				cand.m_Ops.drain(i..(i + chunk).min(cand.m_Ops.len()));
				if Fails(&cand, solverMode) { s = cand } else { i += chunk }
			}
			chunk /= 2;
		}
		let _ = std::panic::take_hook();
		println!("seed {} fails in solver mode {}; the minimized scenario:\n{:#?}", seed, solverMode, s);
		println!("In the format of the solver executable (/topor_tool/solver_mode {}):\n{}", solverMode, ScenarioToCnf(&s));
		if let Ok(f) = std::env::var("TOPOR_FUZZ_CNF_FILE")
		{
			std::fs::write(f, ScenarioToCnf(&s)).unwrap();
		}
		RunScenario(&s, solverMode, None);
	}
}
