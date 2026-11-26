#!/usr/bin/env python3
"""Fake noisy agent for Dragent benchmark scenarios.

Emits ANSI-coloured terminal output at a configurable rate, edits files in its
worktree, runs `git status` periodically, and periodically prints cargo/npm-sized
log bursts (thousands of lines quickly). Deterministic with `--seed`.

Stdlib only: runs on macOS, Windows and Linux with no installs.
"""

import argparse
import os
import random
import subprocess
import sys
import time

COLORS = ["31", "32", "33", "34", "35", "36", "37", "90", "91", "92", "93", "94"]
SPINNERS = ["|", "/", "-", "\\"]
FILES = ["notes.txt", "src/todo.md", "scratch/log.txt"]


def emit_line(text, color=None, flush=True):
    if color:
        sys.stdout.write("\x1b[%sm%s\x1b[0m\n" % (color, text))
    else:
        sys.stdout.write(text + "\n")
    if flush:
        sys.stdout.flush()


def burst(rng, lines):
    """A compiler-sized log burst: thousands of lines as fast as possible."""
    crates = ["dragent", "warpui", "tokio", "wry", "serde", "ignore", "rmcp"]
    chunk = []
    for i in range(lines):
        crate = crates[rng.randrange(len(crates))]
        ms = rng.randrange(5, 4000)
        chunk.append("   Compiling %-12s %4d/%4d (%dms)" % (crate, i + 1, lines, ms))
        if rng.random() < 0.02:
            chunk.append("\x1b[33mwarning\x1b[0m: unused variable `x%d` in %s" % (i, crate))
    sys.stdout.write("\n".join(chunk) + "\n")
    sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser(description="Fake noisy agent for benchmarks.")
    ap.add_argument("--duration", type=float, default=120,
                    help="Seconds to run (default 120).")
    ap.add_argument("--rate", type=float, default=8,
                    help="Average output lines per second outside bursts (default 8).")
    ap.add_argument("--seed", type=int, default=1, help="Deterministic seed.")
    ap.add_argument("--worktree", default=".",
                    help="Worktree directory to edit files in.")
    ap.add_argument("--burst-every", type=float, default=25,
                    help="Seconds between log bursts (0 to disable).")
    ap.add_argument("--burst-lines", type=int, default=3000,
                    help="Lines per burst.")
    ap.add_argument("--git-status-every", type=float, default=15,
                    help="Seconds between `git status` runs (0 to disable).")
    ap.add_argument("--id", default="agent", help="Label shown in output lines.")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    worktree = os.path.abspath(args.worktree)
    for sub in ("src", "scratch"):
        try:
            os.makedirs(os.path.join(worktree, sub), exist_ok=True)
        except OSError:
            pass

    deadline = time.time() + args.duration
    next_burst = time.time() + args.burst_every if args.burst_every > 0 else None
    next_status = time.time() + args.git_status_every if args.git_status_every > 0 else None
    i = 0
    interval = 1.0 / args.rate if args.rate > 0 else 0.1
    while time.time() < deadline:
        now = time.time()
        if next_burst is not None and now >= next_burst:
            emit_line("[%s] build started (burst of %d lines)" % (args.id, args.burst_lines),
                      color="36")
            burst(rng, args.burst_lines)
            emit_line("[%s] build finished" % args.id, color="32")
            next_burst = now + args.burst_every
        if next_status is not None and now >= next_status:
            try:
                p = subprocess.run(["git", "status", "--porcelain=v1", "-uno"],
                                   cwd=worktree, capture_output=True,
                                   text=True, timeout=60)
                n = len(p.stdout.splitlines())
                emit_line("[%s] git status: %d changed paths (rc=%d)"
                          % (args.id, n, p.returncode), color="34")
            except Exception as e:  # noqa: BLE001 - keep the agent running
                emit_line("[%s] git status failed: %s" % (args.id, e), color="31")
            next_status = now + args.git_status_every
        # Edit a file in the worktree every few lines (deterministic content).
        if i % 7 == 0:
            name = FILES[rng.randrange(len(FILES))]
            path = os.path.join(worktree, name)
            try:
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "a", encoding="utf-8") as f:
                    f.write("seed=%d line=%d tick=%.2f spinner=%s\n"
                            % (args.seed, i, now, SPINNERS[i % len(SPINNERS)]))
            except OSError as e:
                emit_line("[%s] edit failed: %s" % (args.id, e), color="31")
        color = COLORS[rng.randrange(len(COLORS))]
        emit_line("[%s] step %d %s %s" % (args.id, i, SPINNERS[i % len(SPINNERS)],
                                          rng.randrange(10 ** 6)), color=color)
        i += 1
        time.sleep(interval * (0.5 + rng.random()))

    emit_line("[%s] done after %d steps" % (args.id, i), color="32")


if __name__ == "__main__":
    main()
