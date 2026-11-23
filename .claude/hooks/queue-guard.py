#!/usr/bin/env python3
"""Refuse a shell command that would compete with the solver runs in progress.

Several sessions share this machine, so a build or a solver run started beside
another one slows both down and spoils whatever is being timed.
scripts/queue.sh exists to run such a command as the only one of its kind, and
CLAUDE.md says to use it; this hook is what makes forgetting cost a retry
rather than a slow machine and a wrong number. It reads the PreToolUse payload
on stdin and exits 2 -- which blocks the command and hands the message back --
when an expensive command is not already going through the queue.

The settings entry that runs this checks the file exists first: a missing
script would exit 2 as well, which is the code for a refusal, and every command
in a worktree branched from before this hook landed would be blocked -- the one
that would fix it included.
"""
import json
import re
import sys

# Anchored at the start of a command rather than searched for anywhere in the
# line, so a path or a message that merely mentions one of these is left alone.
EXPENSIVE = re.compile(
    r"""(?:^|[;&|(]\s*|\bthen\s+|\bdo\s+)   # start of the line or of a segment
        (?:(?:\S+=\S+|env|nice|(?:\S*/)?time(?:\s+-\w+)*|timeout\s+\S+
             |python3?|bash|sh|zsh|csh|tcsh)\s+)*
                                             # launchers that carry the command
        (?:\S*/)?                            # an optional leading path
        (?: cargo\s+(?:\+\S+\s+)?(?:build|test|bench|run|check|clippy|fuzz)\b
          | make\b                           # the C++ solver
          | intel_sat_solver\w*(?=\s|$)      # the solver, Rust or C++
          | topor_\w+(?=\s|$)                # diff_vs_cpp.sh's reference builds
          | (?:bench_vs_cpp\.py|perf_instr\.sh|build_pgo\.sh|coverage\.sh
              |diff_vs_cpp\.sh)(?=\s|$)
          | (?:run_and_verify|fuzz_and_verify|delta_debug)\w*\.csh(?=\s|$)
          | target/(?:\S+/)?(?:release|debug)/\S+
                                             # anything else cargo built: satdb's
                                             # and the test executables
          )""",
    re.VERBOSE,
)

REASON = (
    "This command competes for the machine with the solver runs of other "
    "sessions. Run it through the queue instead:\n"
    "    scripts/queue.sh <the same command>\n"
    "(scripts/ is at the root of the repository.) It waits its turn, so run it "
    "in the background rather than under a short timeout. "
    "`scripts/queue.sh --status` shows the queue."
)


def blocked(command):
    """Whether this command is expensive and is not already queued."""
    return "queue.sh" not in command and bool(EXPENSIVE.search(command))


def main():
    try:
        command = json.load(sys.stdin)["tool_input"]["command"]
    except Exception:
        # Never block on a payload this hook failed to understand.
        return 0
    if not blocked(command):
        return 0
    print(REASON, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
