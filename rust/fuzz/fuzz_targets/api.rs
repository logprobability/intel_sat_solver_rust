// libFuzzer target: incremental scenarios over the public API, verified against an exact oracle (see src/tests/fuzzing.rs)
// Run with: cargo +nightly fuzz run api
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
	intel_sat_solver::fuzzing::FuzzOne(data, false);
});
