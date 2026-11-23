// Targeted unit tests: the construction-time error statuses of the solver

use crate::topi::*;
use crate::topi_params::param_def_fault;
use crate::topor_dyn_array::alloc_fault;

// Every allocation of the constructor fails in turn (only that one): each array, which the constructor tests, gets its own status explanation
#[test]
fn constructor_single_alloc_failures()
{
	fn Check<Ind: TUInd, const Compress: bool>()
	{
		let before = alloc_fault::allocs_so_far();
		let t = CTopi::<Ind, Compress>::new(10);
		assert!(!t.IsError());
		let allocs = alloc_fault::allocs_so_far() - before;
		let mut explanations = std::collections::BTreeSet::new();
		for failAt in 0..allocs
		{
			alloc_fault::fail_only(failAt);
			let t = CTopi::<Ind, Compress>::new(10);
			alloc_fault::never_fail();
			if t.IsError()
			{
				assert_eq!(t.m_Status.get(), TToporStatus::STATUS_ALLOC_FAILED);
				explanations.insert(t.GetStatusExplanation());
			}
		}
		for arr in ["the main buffer", "m_E2IVarMap", "m_Watches", "m_AssignmentInfo", "m_VarInfo", "m_TrailLastVarPerDecLevel"]
		{
			assert!(explanations.iter().any(|e| e.ends_with(arr)), "{} : {:?}", arr, explanations);
		}
	}
	Check::<u32, false>();
	Check::<u64, true>();
}

#[test]
fn constructor_param_definition_error()
{
	// The first parameter definition
	param_def_fault::Set(Some("/mode/value"));
	let p = CTopiParams::new(2);
	let mut t = CTopi::<u32, false>::new(3);
	param_def_fault::Set(None);
	assert!(p.IsError());
	assert_eq!(p.GetErrorDescr(), ERR_VAL_BELOW_MIN);
	// No parameter is registered after the erroneous definition
	assert!(!p.GetAllParamsDescr().contains("/mode/value"));
	param_def_fault::Set(Some("/verbosity/print_out_confs"));
	let p = CTopiParams::new(2);
	param_def_fault::Set(None);
	assert!(p.IsError() && p.GetAllParamsDescr().contains("/verbosity/level") && !p.GetAllParamsDescr().contains("/verbosity/print_out_confs"));
	assert!(t.IsError());
	assert_eq!(t.m_Status.get(), TToporStatus::STATUS_PARAM_ERROR);
	assert_eq!(t.GetStatusExplanation(), ERR_VAL_BELOW_MIN);
	assert_eq!(t.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_PARAM_ERROR);
}

// The dump file cannot be created: the solver works without dumping
// The test runs in a child process, since the environment variable would affect the solvers of the tests running in parallel otherwise
#[test]
fn constructor_dump_file_cannot_be_opened()
{
	if std::env::var("TOPOR_TEST_DUMP_CHILD").is_ok()
	{
		let mut t = CTopi::<u32, false>::new(3);
		assert!(t.m_DumpFile.is_none() && !t.IsError());
		t.AddUserClause(&[1, 2]);
		assert_eq!(t.Solve(&[], (f64::MAX, true), u64::MAX), TToporReturnVal::RET_SAT);
		return;
	}
	let out = std::process::Command::new(std::env::current_exe().unwrap())
		.args(["tests::cov_misc::constructor_dump_file_cannot_be_opened", "--exact", "--test-threads=1"])
		.env("TOPOR_TEST_DUMP_CHILD", "1")
		.env("TOPOR_DUMP_NAME", "/nonexistent_directory_of_intel_sat_solver_tests/dump")
		.output()
		.unwrap();
	assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}
