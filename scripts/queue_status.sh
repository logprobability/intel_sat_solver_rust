#!/bin/sh
# Colorized view of the shared build/test/solver-run queue (see queue.sh).
#
#     scripts/queue_status.sh              # one-shot
#     watch -n1 scripts/queue_status.sh    # live-refreshing dashboard
#
# The columns are laid out here rather than passed through: task-spooler pads
# its own Output column to whatever the temporary file path happens to be, so
# the command starts seventy columns in and wraps off the side of the terminal.
# The path is of no use while watching -- queue.sh reproduces a job's output to
# whoever is waiting on it -- so the command takes the width instead.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

width=${COLUMNS:-0}
[ "$width" -gt 0 ] 2>/dev/null || width=$(tput cols 2>/dev/null || echo 100)

"$script_dir/queue.sh" --status | awk -v width="$width" '
    function paint(color, text) { return color text "\033[0m" }

    # A finished row carries an exit status and its timings before the command;
    # a queued or running one has neither, so the command starts earlier.
    NR == 1 {
        header = $0
        next
    }
    {
        id = $1
        state = $2
        if (state == "finished") {
            level = $4
            split($5, t, "/")
            elapsed = sprintf("%.1fs", t[1])
            first = 6
        } else {
            level = ""
            elapsed = "-"
            first = 4
        }

        command = ""
        for (i = first; i <= NF; i++)
            command = command (command == "" ? "" : " ") $i

        if (state == "running")      { color = "\033[32m"; running++ }
        else if (state == "queued")  { color = "\033[33m"; queued++ }
        else if (state == "finished"){ color = (level == "0") ? "\033[2m" : "\033[31m" }
        else                          { color = "\033[0m" }

        # Three columns of 21 characters plus a space leave the rest for the
        # command; an ellipsis marks where a long one was cut.
        room = width - 22
        if (room < 20) room = 20
        if (length(command) > room)
            command = substr(command, 1, room - 3) "..."

        rows[++n] = paint(color, sprintf("%-4s %-9s %-6s %s", id, state, elapsed, command))
    }
    END {
        slots = ""
        if (match(header, /\[run=[0-9]+\/[0-9]+\]/))
            slots = substr(header, RSTART + 5, RLENGTH - 6)

        printf "%s", paint("\033[1m", sprintf("%-4s %-9s %-6s %s", "ID", "STATE", "TIME", "COMMAND"))
        printf "   %s\n", paint("\033[2m", (slots == "" ? "" : "slots " slots))
        for (i = 1; i <= n; i++)
            print rows[i]
        if (n == 0)
            print paint("\033[2m", "the queue is empty")
        else
            printf "%s\n", paint("\033[2m", sprintf("%d running, %d queued", running + 0, queued + 0))
    }
'
