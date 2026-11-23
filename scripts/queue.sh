#!/bin/sh
# Run a command as the only heavy job on this machine, waiting its turn first.
# Usage: scripts/queue.sh <command> [args...]
#        scripts/queue.sh --status
#
# Several sessions share this machine. Solver runs, builds and test runs started
# side by side slow each other down, so every session waits longer than if they
# had taken turns, and a timing taken meanwhile means nothing. So every session
# funnels its builds, its test runs and its solver runs through one queue rather
# than starting them whenever it happens to be ready. The queue is a
# task-spooler daemon (brew install task-spooler) holding a single slot: jobs
# run one at a time, in the order they arrived. This wrapper is a transparent
# prefix around that -- it blocks until its job has run, reproduces the output
# and exits with the same status:
#
#     scripts/queue.sh cargo build --release
#     scripts/queue.sh cargo test
#     scripts/queue.sh rust/scripts/perf_instr.sh rust/target/release/intel_sat_solver regression_instances/*.cnf
#
# A job waits for as long as the ones ahead of it take, so a caller that is
# itself under a timeout should run this in the background. `--status` lists
# what is running and what is waiting.
set -u

if ! command -v ts >/dev/null 2>&1; then
    echo "queue.sh: task-spooler is not installed (brew install task-spooler)" >&2
    exit 127
fi

# One queue per repository, shared by every worktree of it: --git-common-dir is
# the primary .git seen from a worktree, where --git-dir would be that
# worktree's own directory and would give each session a private queue.
if ! common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
    echo "queue.sh: not inside a git repository" >&2
    exit 1
fi
TS_SOCKET="$common_dir/queue.socket"
TS_SLOTS=1
export TS_SOCKET TS_SLOTS

case ${1-} in
    --status) exec ts -l ;;
    '') echo "usage: scripts/queue.sh <command> [args...] | --status" >&2; exit 2 ;;
esac

# A job that is already the one running holds the only slot, so queueing from
# inside it would wait on itself for ever: run the command where it stands.
# --status is answered above, since asking what is in the queue is always safe.
if [ -n "${TOPOR_QUEUE_JOB-}" ]; then
    exec "$@"
fi

# TS_SLOTS is only read when the daemon starts, so a daemon left running from
# before is told again: one job at a time is the whole point of the queue.
ts -S 1 >/dev/null 2>&1

# The daemon runs the job with this caller's environment and working directory,
# so a worktree path and variables such as CARGO_TARGET_DIR both survive -- and
# so does the mark that stops the job queueing itself a second time.
TOPOR_QUEUE_JOB=1
export TOPOR_QUEUE_JOB
id=$(ts "$@") || exit $?
echo "queue.sh: job $id $(ts -s "$id" 2>/dev/null)" >&2

# Wait in the background and `wait` for it rather than waiting in the
# foreground, because a shell defers a trap until the foreground command
# returns -- which here is the job ending, long after the signal that was meant
# to cancel it.
ts -w "$id" >/dev/null 2>&1 &
waiter=$!

# Give up the place in the queue if this caller is killed while waiting, so the
# job neither starts unwatched nor holds the queue: -r drops one still queued,
# -k signals one already running. Killed with SIGKILL instead there is no
# chance to do either, and the job keeps the slot until it ends -- late, but
# still alone, which is the property worth protecting.
trap 'kill "$waiter" 2>/dev/null; ts -k "$id" >/dev/null 2>&1;
      out=$(ts -o "$id" 2>/dev/null) && rm -f "$out"
      ts -r "$id" >/dev/null 2>&1; exit 130' INT TERM HUP

wait "$waiter"
status=$?
trap - INT TERM HUP

# The output file exists once the job has started; both streams are in it.
if out=$(ts -o "$id" 2>/dev/null); then
    cat "$out"
    rm -f "$out"
fi
ts -r "$id" >/dev/null 2>&1
exit $status
