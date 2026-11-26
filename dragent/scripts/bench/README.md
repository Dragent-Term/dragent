# Dragent benchmark harness

Cross-platform (macOS, Windows, Linux) speed + memory benchmarks. Python 3
standard library only — no installs. Never touches the installed Dragent
(`~/​.dragent`, `/Applications/Dragent.app`); it only measures dev-edition
processes (`~/.dragent-dev` / `DRAGENT_DEV_HOME`) and scratch repos in temp
folders, and never stops or kills anything it did not start.

Layout (`dragent/scripts/bench/`):

- `fake_agent.py` — deterministic noisy agent: ANSI-coloured output, file
  edits, periodic `git status`, compiler-sized log bursts. `--seed` fixes it.
- `gui_app.py` — GUI-driven measures against the REAL dev app: launches the
  dev binary with a scratch `DRAGENT_DEV_HOME` (created and deleted here),
  drives it via the automation server, and records `in-app-*` rows: cold
  start (launch → first presented frame), input latency (≥60 automation
  `run` submissions), frame times (1 s aggregates under a 10-agent stream),
  plus a 12-agent memory scenario (`gui-scenario-mem.csv`). Needs a desktop
  session and a binary built with `--features performance-harness`, run with
  `DRAGENT_DEV_TERMINAL=1` (a dev window with a single local terminal:
  production tabs are host-owned and never register with the automation
  server, and the unsigned dev CLI cannot open tabs). Tab switch and
  worktree creation stay labelled proxies (same sign-in reason).
- `memsample.py` — per-second sampler → CSV (`timestamp,role,pid,bytes`).
  Roles: `gui`, `host`, `pty`, `index`, `computer`, `cli`, `other-own` for
  Dragent's own dev-edition processes; `agent` is one summed row (`pid=-1`).
- `run.py` — scenario runner + one subcommand per measure; appends to
  `measures.csv` and prints a table.

## Run on each OS

From the repo root (or from `dragent/` with `scripts/bench/…` adjusted):

```sh
# macOS / Linux
python3 scripts/bench/run.py --help
python3 scripts/bench/run.py --out dragent/docs/perf/data all --duration 60

# Windows (PowerShell)
py scripts\bench\run.py --help
py scripts\bench\run.py --out dragent\docs\perf\data all --duration 60
```

From `dragent/` directly, use `scripts/bench/run.py` under it
(`python3 scripts/bench/run.py …`) — same files.

Prerequisites: `git` on PATH. Building the dev app needs Rust
(`cargo build -p dragent` from `dragent/`); measures that need the binary
record `skipped` rows until it exists.

## Subcommands

| command | what it does |
|---|---|
| `scenario --duration S --agents N --keep K --tail T [--gui]` | scratch repo + N worktrees running fake agents, memory sampled every second, then all but K closed while sampling continues |
| `cold-start` | process launch → usable window: parses the dev build's `[bench] startup ready_ms=` line (`--bench-probe-startup` headless probe; full window timing needs the GUI step below) |
| `input-latency` | parses `bench.input key_to_frame_ms=` from a `DRAGENT_BENCH_INPUT=1` app log; otherwise a labelled local PTY echo proxy |
| `throughput` | `yes \| head -n 2000000` wall time + MB/s |
| `frame-times` | applies 4-agent stream load, counts delivered lines/s; per-frame times need the GUI step |
| `tab-switch` | `dragent workspaces list` round-trip when a dev app runs, else a labelled headless worktree-context-switch proxy |
| `worktree-create` | median `git worktree add/remove` on a scratch repo |
| `git-status` | `git status` on a generated large repo (`--files`, default 2000) |
| `diff-open` | `git diff --numstat` + parse over a generated 10,000-line diff |
| `all` | every measure + a short scenario |

Every `measures.csv` row carries a `method` column (`probe-startup`,
`pty-echo-proxy`, `headless-proxy`, `cli-roundtrip`, `in-app-log`, …) plus a
`note`, so headless proxies are never mistaken for in-app numbers.

The run stops early (exit 2) if free memory drops under 1.5 GB.

## Driving the real GUI (optional)

Headless mode (default) runs fake agents as plain subprocesses — no window,
nothing to disturb. To drive real dev-app terminals instead:

1. Build and start the dev edition: `scripts/dev.sh` (repo root; dev data in
   `~/.dragent-dev`, local mode, never the installed app).
2. `run.py scenario --gui …` uses that build's `dragent` CLI
   (`workspaces create --command …`) to open one workspace per fake agent.

Manual steps the harness cannot do headlessly (documented in each row's
`note`): opening a 10,000-line diff in the UI, switching visible tabs while
10 agents stream, and reading frame times — run them in the dev app with
`DRAGENT_BENCH_INPUT=1` and re-run the matching subcommand to pick up the
`[bench]` log lines.

## Memory: what is counted

- Scope: only processes belonging to the dev edition (`DRAGENT_DEV_HOME` or
  `~/.dragent-dev`). Release markers (`/Applications/Dragent.app`,
  `*.app/Contents` bundles without "dev", `~/.dragent`, `Program Files`
  installs) always lose. On shared machines pass `--root-pid` (the dev app
  PID the harness started) so only that tree is counted.
- macOS: `phys_footprint` via `/usr/bin/footprint -p PID`, `ps` RSS fallback.
- Linux: `/proc/PID/smaps_rollup` Pss, `VmRSS` fallback.
- Windows: `GetProcessMemoryInfo` PrivateUsage via ctypes, tasklist fallback.

Role detection is by executable name/arguments: `--dragent-host-service`
(host), `--dragent-pty-service` (terminal/PTY), `--dragent-index-service`
(code index), `--dragent-computer-service`, `--dragent-cli`, else the `dragent`
GUI binary. See `dragent/app/src/main.rs` and `dragent/crates/dragent_host`
for how services are launched and named.

## Dev-build hooks (`--bench-mem` etc.)

Small dev-only additions (`bench.rs` in `dragent/app/src`, fenced on
`dragent_paths::is_dev() || cfg!(debug_assertions)`, native only; plus one
dev-gated frame callback in `warpui_core::bench_frame`, a no-op — one cached
bool load per frame — unless a `DRAGENT_BENCH_*` var below is set):

- `--bench-mem` (or `DRAGENT_BENCH_MEM=1`): own memory (footprint on Mac)
  plus sibling dev-service sum, every 5 s, to the app log
  (`<dev-home>/logs/desktop.log`) and stderr.
- `[bench] startup ready_ms=… dev=… probe=…`: `probe=true` from
  `--bench-probe-startup` (headless process-init timing, what `cold-start`
  parses without a window); with a window, `note_window_open()` logs it and
  the first presented frame logs it again with `frame=first-presented` —
  `ready` is the first presented frame.
- `DRAGENT_BENCH_INPUT=1`: key→frame hook logging
  `bench.input key_to_frame_ms=…` (what `input-latency` parses). Key side is
  wired (automation `run` submissions); the frame side is the warpui
  `frame_drawn` hook calling `bench::note_frame_presented()`.
- `DRAGENT_BENCH_FRAME=1`: per-frame times. One aggregate line per second
  with frames: `bench.frame ms=<p50> p95_ms=<p95> max_ms=<max> n=<n>`,
  where each sample is the gap since the previous presented frame
  (inter-frame interval; render cost itself is in automation `perf_stats`).
  Either var arms the warpui hook.

Log contract (shared with the perf overlay):
`bench.input key_to_frame_ms=<float>`,
`[bench] startup ready_ms=<ms> … frame=first-presented`,
`bench.frame ms=<p50> p95_ms=<p95> max_ms=<max> n=<n>`.

Rerun the baseline: `python3 scripts/bench/run.py --out
dragent/docs/perf/data all --duration 180` (3-minute scenario) after
`cargo build -p dragent` from `dragent/`, then the GUI measures
(`python3 dragent/scripts/bench/gui_app.py --out dragent/docs/perf/data`
needs a desktop session), then compare with
`dragent/docs/perf/baseline-2026-11-22.md`.
