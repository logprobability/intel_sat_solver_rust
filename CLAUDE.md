# intel_sat_solver_rust

## Build, test and solver runs

Several sessions share this machine, and solver runs, builds and test runs
started side by side slow each other down: every session then waits longer for
its result than if they had taken turns, and a timing taken meanwhile means
nothing. Put anything that competes for the cores behind `scripts/queue.sh`
(at the root of the repository), which runs it as the only such job on the
machine and returns its output and its exit status unchanged:

```sh
scripts/queue.sh cargo build --release
scripts/queue.sh cargo test
scripts/queue.sh rust/scripts/perf_instr.sh rust/target/release/intel_sat_solver regression_instances/*.cnf
scripts/queue.sh rust/scripts/diff_vs_cpp.sh
```

Queue a whole script or loop as one job rather than each solver run in it, so
the runs of one measurement stay together.

Jobs run one at a time in the order they arrived, so a call blocks for as long
as the ones ahead of it take: run it in the background rather than under a short
timeout. `scripts/queue.sh --status` shows what is running and what is waiting,
and `scripts/queue_status.sh` is the same in color. The queue is one
task-spooler daemon per repository, shared by every worktree of it, and
`brew install task-spooler` is what provides it. Running a queued job through
the queue a second time would wait on itself, so a command that is already the
running job is run where it stands rather than queued again.

Forgetting the prefix costs a retry rather than a slow machine: the
`PreToolUse` hook in `.claude/settings.json` rejects an unqueued build, test or
solver run and says what to run instead. It reads only the command, so a line
that merely names one of them and does not run it can be caught as well; the
answer then is the same, to put it behind the queue.
