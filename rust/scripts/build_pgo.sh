#!/bin/bash
# Optional: builds the release executable with profile-guided optimization (PGO). The default build (cargo build --release) isn't affected.
# PGO changes only the inlining decisions and the code layout, so the solver's output and proofs stay byte-identical to those of the default build (and of the C++ solver).
# Usage: scripts/build_pgo.sh [training .cnf files...]   (default: ../regression_instances/*.cnf); the executable is $PGO_DIR/release/intel_sat_solver
# Requirements: llvm-profdata of the LLVM version used by rustc; set LLVM_BIN_DIR if it isn't on the PATH.
# Environment: PGO_DIR (default target/pgo): the output directory; PGO_MODES (default "0 1 2"): the solver modes (/topor_tool/solver_mode) to train on.
# The profile depends on the toolchain and on the sources, so the script regenerates it each time.
set -e
set -o pipefail
export LC_ALL=C
cd "$(dirname "$0")/.."

if [ -z "$LLVM_BIN_DIR" ]; then
	for d in "$(dirname "$(command -v llvm-profdata 2>/dev/null)" 2>/dev/null)" /opt/homebrew/opt/llvm@22/bin /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin"; do
		if [ -x "$d/llvm-profdata" ]; then LLVM_BIN_DIR="$d"; break; fi
	done
fi
if [ -z "$LLVM_BIN_DIR" ]; then echo "llvm-profdata not found; set LLVM_BIN_DIR"; exit 2; fi

if [ $# -gt 0 ]; then TRAIN=("$@"); else TRAIN=(../regression_instances/*.cnf); fi
PGO_MODES="${PGO_MODES:-0 1 2}"
PGO_DIR="${PGO_DIR:-target/pgo}"
mkdir -p "$PGO_DIR"
PGO_DIR="$(cd "$PGO_DIR" && pwd)"
PROF_DIR="$PGO_DIR/prof"
rm -rf "$PROF_DIR"
mkdir -p "$PROF_DIR"

# Step 1: the instrumented build (the same profile and flags as the final build, but for the instrumentation)
RUSTFLAGS="$RUSTFLAGS -C profile-generate=$PROF_DIR" CARGO_TARGET_DIR="$PGO_DIR/gen" cargo build --release --bin intel_sat_solver 2>&1 | tail -1

# Step 2: the training runs (the exit code of the solver is the SAT/UNSAT/... status, not an error)
if [ ! -x "$PGO_DIR/gen/release/intel_sat_solver" ]; then echo "the instrumented build didn't produce $PGO_DIR/gen/release/intel_sat_solver"; exit 1; fi
N=0; BAD=0
for f in "${TRAIN[@]}"; do
	for m in $PGO_MODES; do
		CODE=0
		LLVM_PROFILE_FILE="$PROF_DIR/r-%m.profraw" "$PGO_DIR/gen/release/intel_sat_solver" "$f" /topor_tool/print_model 0 /topor_tool/solver_mode "$m" > /dev/null 2>&1 || CODE=$?
		# The solver exits with 10 (SAT), 20 (UNSAT) or another multiple of 10 (a stopped run); anything else is a failed run (e.g., a missing file or a crash)
		if [ $((CODE % 10)) -ne 0 ] || [ "$CODE" -gt 100 ]; then BAD=$((BAD + 1)); echo "warning: exit code $CODE on $f in solver mode $m"; fi
		N=$((N + 1))
	done
done
echo "$N training runs, $BAD failed"
if [ "$BAD" -eq "$N" ] || ! ls "$PROF_DIR"/*.profraw > /dev/null 2>&1; then echo "no training run produced a profile"; exit 1; fi

# Step 3: merge the profiles
"$LLVM_BIN_DIR/llvm-profdata" merge -o "$PGO_DIR/merged.profdata" "$PROF_DIR"/*.profraw

# Step 4: the optimized build
RUSTFLAGS="$RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata" CARGO_TARGET_DIR="$PGO_DIR" cargo build --release --bin intel_sat_solver 2>&1 | tail -1
echo "$PGO_DIR/release/intel_sat_solver"
