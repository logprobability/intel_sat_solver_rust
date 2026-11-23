// libFuzzer target: long incremental scenarios over the public API; the models are verified and the verdicts of the solver modes are compared (see src/tests/fuzzing.rs)
// Run with: cargo +nightly fuzz run api_big -- -max_len=20000
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
	intel_sat_solver::fuzzing::FuzzOne(data, true);
});
