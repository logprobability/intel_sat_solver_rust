#!/bin/bash
# Deterministic performance metric: the instructions retired (and CPU time) of a solver executable per instance, as reported by /usr/bin/time -l (macOS).
# Unlike the wall/CPU time, the number of instructions is nearly independent of the load of the machine, so it can be compared between builds reliably.
# Usage: scripts/perf_instr.sh <executable> <instance.cnf>... [-- extra solver parameters]
# Output: one line per instance: <instance> <instructions retired> <cycles elapsed> <user+sys seconds> <exit code> <md5 of the output>, and a TOTAL line (instructions, cycles).
# The instructions are reproducible to within ~0.5%; the cycles depend on the load of the machine (caches), so compare the minimum of a few runs (PERF_RUNS, default 3).
EXE="$1"; shift
TMPOUT="${TMPDIR:-/tmp}/perf_instr_$$.out"
INSTS=(); PARAMS=()
while [ $# -gt 0 ]; do if [ "$1" = "--" ]; then shift; PARAMS=("$@"); break; fi; INSTS+=("$1"); shift; done
TOT=0; TOTC=0
RUNS="${PERF_RUNS:-3}"
for f in "${INSTS[@]}"; do
	INSTR=0; CYC=0; CPU=0
	for ((k = 0; k < RUNS; k++)); do
		OUT=$( { /usr/bin/time -l "$EXE" "$f" /topor_tool/print_model 0 "${PARAMS[@]}" > "$TMPOUT"; echo "exit $?" >&2; } 2>&1 )
		CODE=$(echo "$OUT" | sed -n 's/^exit //p')
		I=$(echo "$OUT" | awk '/instructions retired/ {print $1}')
		C=$(echo "$OUT" | awk '/cycles elapsed/ {print $1}')
		T=$(echo "$OUT" | awk '/ real / {print $3 + $5}')
		if [ "$CYC" -eq 0 ] || [ "$C" -lt "$CYC" ]; then CYC=$C; INSTR=$I; CPU=$T; fi
	done
	MD5=$(md5 -q "$TMPOUT")
	printf "%-18s %14s %14s %8.2f %s %s\n" "$(basename "$f")" "$INSTR" "$CYC" "$CPU" "$CODE" "$MD5"
	TOT=$((TOT + INSTR)); TOTC=$((TOTC + CYC))
done
rm -f "$TMPOUT"
echo "TOTAL $TOT $TOTC"
