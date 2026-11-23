#!/bin/bash
# Measures the test coverage of the crate (unit + fuzz tests) with LLVM source-based coverage.
# Usage: scripts/coverage.sh [extra cargo test arguments]
# Requirements: llvm-profdata and llvm-cov of the LLVM version used by rustc; set LLVM_BIN_DIR if they aren't on the PATH.
# The script prints the per-file summary, and returns non-zero unless every line is either covered or explicitly marked in the sources as unreachable by construction (see below).
set -e
export LC_ALL=C
cd "$(dirname "$0")/.."

if [ -z "$LLVM_BIN_DIR" ]; then
	for d in "$(dirname "$(command -v llvm-profdata 2>/dev/null)" 2>/dev/null)" /opt/homebrew/opt/llvm@22/bin /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin"; do
		if [ -x "$d/llvm-profdata" ] && [ -x "$d/llvm-cov" ]; then LLVM_BIN_DIR="$d"; break; fi
	done
fi
if [ -z "$LLVM_BIN_DIR" ]; then echo "llvm-profdata/llvm-cov not found; set LLVM_BIN_DIR"; exit 2; fi

COV_DIR="${COV_DIR:-target/coverage}"
rm -rf "$COV_DIR"
mkdir -p "$COV_DIR"
COV_DIR="$(cd "$COV_DIR" && pwd)"
export CARGO_TARGET_DIR="$COV_DIR/target"
export RUSTFLAGS="-C instrument-coverage"
export LLVM_PROFILE_FILE="$COV_DIR/prof/%p-%m.profraw"

# Pass 1: all the tests, built as usual (debug assertions on)
cargo test --no-run "$@" 2>&1 | tail -3
BINS=$(cargo test --no-run --message-format=json "$@" 2>/dev/null | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p')
cargo test --no-fail-fast "$@" 2>&1 | grep -E "^(test result|running|error|failures:|test .* FAILED)" || true

# Pass 2: the statements, which directly follow a failing debug_assert! (the C++ "assert(0); return false;" idiom), can only execute when the assertions are compiled out,
# so the tests named nda_* (and only them) are also run in a build with the debug assertions off
export CARGO_TARGET_DIR="$COV_DIR/target_nda"
export RUSTFLAGS="-C instrument-coverage -C debug-assertions=off"
export LLVM_PROFILE_FILE="$COV_DIR/prof_nda/%p-%m.profraw"
BINS_NDA=$(cargo test --lib --no-run --message-format=json 2>/dev/null | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p')
cargo test --lib --no-fail-fast nda_ 2>&1 | grep -E "^(test result|error|failures:|test .* FAILED)" || true

"$LLVM_BIN_DIR/llvm-profdata" merge -sparse "$COV_DIR"/prof/*.profraw -o "$COV_DIR/all.profdata"
"$LLVM_BIN_DIR/llvm-profdata" merge -sparse "$COV_DIR"/prof_nda/*.profraw -o "$COV_DIR/nda.profdata"
OBJS=""
for b in $BINS; do OBJS="$OBJS -object $b"; done
OBJS_NDA=""
for b in $BINS_NDA; do OBJS_NDA="$OBJS_NDA -object $b"; done
# The tests themselves aren't part of the measured code
IGNORE='(/tests/|\.cargo/|/rustc/|library/)'
"$LLVM_BIN_DIR/llvm-cov" report $OBJS -instr-profile="$COV_DIR/all.profdata" -ignore-filename-regex="$IGNORE" > "$COV_DIR/report.txt" 2>/dev/null
"$LLVM_BIN_DIR/llvm-cov" show $OBJS -instr-profile="$COV_DIR/all.profdata" -ignore-filename-regex="$IGNORE" -show-line-counts-or-regions -show-instantiations=false > "$COV_DIR/show.txt" 2>/dev/null
"$LLVM_BIN_DIR/llvm-cov" export $OBJS -instr-profile="$COV_DIR/all.profdata" -ignore-filename-regex="$IGNORE" -format=lcov > "$COV_DIR/lcov.info" 2>/dev/null
"$LLVM_BIN_DIR/llvm-cov" export $OBJS_NDA -instr-profile="$COV_DIR/nda.profdata" -ignore-filename-regex="$IGNORE" -format=lcov > "$COV_DIR/lcov_nda.info" 2>/dev/null
# The lines are those of pass 1; a line is covered, if it is executed in any of the two passes
awk -F'[:,]' '/^SF:/ {f=$2} /^DA:/ { k = f":"$2; if (FNR == NR) { if (!(k in c) || $3 > c[k]) c[k] = $3 } else if ((k in c) && $3 > c[k]) c[k] = $3 } END { for (k in c) if (c[k] == 0) print k }' "$COV_DIR/lcov.info" "$COV_DIR/lcov_nda.info" | sort > "$COV_DIR/uncovered_all.txt"

# The lines, which are unreachable by construction, are marked in the sources (with the reason):
#   <statement> // COVERAGE-UNREACHABLE: <reason>          -- one line
#   // COVERAGE-UNREACHABLE-BEGIN: <reason> ... // COVERAGE-UNREACHABLE-END   -- a block
# They are faithful translations of C++ code, which is dead or purely defensive in the C++ solver too. An uncovered line without the mark fails the script; so does a marked line, which is executed.
# COVERAGE-NOT-REACHED marks the (very few) lines of rare corner cases, which are reachable in principle, but for which no triggering input is known
# (neither the tests nor millions of fuzzed scenarios reach them); they are reported separately.
: > "$COV_DIR/unreachable_marked.txt"
: > "$COV_DIR/not_reached_marked.txt"
for f in src/*.rs; do
	awk -v f="$PWD/$f" '/COVERAGE-UNREACHABLE-BEGIN/ { inb = 1; next } /COVERAGE-UNREACHABLE-END/ { inb = 0; next } inb || /COVERAGE-UNREACHABLE:/ { print f":"NR }' "$f" >> "$COV_DIR/unreachable_marked.txt"
	awk -v f="$PWD/$f" '/COVERAGE-NOT-REACHED:/ { print f":"NR }' "$f" >> "$COV_DIR/not_reached_marked.txt"
done
sort -o "$COV_DIR/not_reached_marked.txt" "$COV_DIR/not_reached_marked.txt"
sort -o "$COV_DIR/unreachable_marked.txt" "$COV_DIR/unreachable_marked.txt"
cat "$COV_DIR/unreachable_marked.txt" "$COV_DIR/not_reached_marked.txt" | sort > "$COV_DIR/marked.txt"
comm -23 "$COV_DIR/uncovered_all.txt" "$COV_DIR/marked.txt" > "$COV_DIR/uncovered.txt"
comm -12 "$COV_DIR/uncovered_all.txt" "$COV_DIR/unreachable_marked.txt" > "$COV_DIR/unreachable_uncovered.txt"
comm -12 "$COV_DIR/uncovered_all.txt" "$COV_DIR/not_reached_marked.txt" > "$COV_DIR/not_reached_uncovered.txt"
awk -F'[:,]' '/^SF:/ {f=$2} /^DA:/ && $3 > 0 { print f":"$2 }' "$COV_DIR/lcov.info" "$COV_DIR/lcov_nda.info" | sort -u > "$COV_DIR/covered.txt"
comm -12 "$COV_DIR/covered.txt" "$COV_DIR/marked.txt" > "$COV_DIR/marked_but_covered.txt"
# The per-file summary
cat "$COV_DIR/uncovered_all.txt" > "$COV_DIR/uncovered_all_tmp.txt"
awk -F'[:,]' 'FNR == NR { u[$1]++; next } /^SF:/ { f = $2 } /^DA:/ { k = f":"$2; if (!(k in s)) { s[k] = 1; n[f]++ } } END { for (f in n) { b = f; sub(".*/", "", b); printf "%-28s %6d lines %5d not executed\n", b, n[f], u[f] } }' "$COV_DIR/uncovered_all_tmp.txt" "$COV_DIR/lcov.info" | sort
rm -f "$COV_DIR/uncovered_all_tmp.txt"
TOTAL=$(awk -F'[:,]' '/^SF:/ {f=$2} /^DA:/ { print f":"$2 }' "$COV_DIR/lcov.info" | sort -u | wc -l)
N=$(wc -l < "$COV_DIR/uncovered.txt")
U=$(wc -l < "$COV_DIR/unreachable_uncovered.txt")
R=$(wc -l < "$COV_DIR/not_reached_uncovered.txt")
S=$(wc -l < "$COV_DIR/marked_but_covered.txt")
echo "Lines: $TOTAL"
echo "Uncovered and unmarked: $N (see $COV_DIR/uncovered.txt)"
echo "Marked unreachable by construction: $U"
echo "Marked as not reached (rare corner cases): $R"
echo "Marked, but executed (stale marks): $S (see $COV_DIR/marked_but_covered.txt)"
[ "$N" -eq 0 ] && [ "$S" -eq 0 ]
