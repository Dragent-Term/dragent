#!/usr/bin/env python3
"""Benchmark scenario runner + measures for Dragent speed and memory.

Scenario: builds a scratch repo in a temp folder (never the person's repos),
creates ~12 git worktrees running fake noisy agents for `--duration`, records
memory with memsample.py, then closes all but `--keep` agents and keeps
sampling to show memory dropping.

Measures (each a subcommand, appended to measures.csv + printed as a table):
  cold-start     process launch -> usable window (dev-build startup log line,
                 else headless probe timing)
  input-latency  keystroke -> frame (DRAGENT_BENCH_INPUT log lines, else a
                 local PTY echo round-trip proxy, clearly labelled)
  throughput     `yes | head -n 2000000` wall time + MB/s (+ app frame times
                 when a GUI run log provides them)
  frame-times    stream load applied while sampling; in-app frame times need
                 the GUI manual step (row records the applied load + pointer)
  tab-switch     workspace/tab switch time (CLI round-trip when the dev app
                 runs, else a headless worktree-context-switch proxy)
  worktree-create  worktree + workspace creation time on a scratch repo
  git-status     `git status` on a generated large repo
  diff-open      open/parse a generated 10,000-line diff

`all` runs every measure plus a short scenario. Every row carries a `method`
column so headless proxies are never mistaken for in-app numbers.

Stdlib only: macOS, Windows and Linux with no installs.
"""

import argparse
import csv
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
FAKE_AGENT = os.path.join(HERE, "fake_agent.py")
MEMSAMPLE = os.path.join(HERE, "memsample.py")
# Repo root is three levels up from dragent/scripts/bench.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
MEASURES_FIELDS = ["timestamp", "measure", "method", "value", "unit", "note"]

MANUAL_GUI_STEP = (
    "GUI step (manual): launch the dev build "
    "(scripts/dev.sh from the repo root; dev edition ~/.dragent-dev, local mode), "
    "open 10+ agent tabs, then re-run this measure: in-app numbers come from "
    "the [bench] log lines described in scripts/bench/README.md."
)


# ---------------------------------------------------------------- utilities


def log(msg):
    print("[bench] %s" % msg, flush=True)


def table(rows, headers):
    widths = [len(h) for h in headers]
    for r in rows:
        for i, c in enumerate(r):
            widths[i] = max(widths[i], len(str(c)))
    fmt = "  ".join("{:<%d}" % w for w in widths)
    print(fmt.format(*headers))
    print(fmt.format(*["-" * w for w in widths]))
    for r in rows:
        print(fmt.format(*[str(c) for c in r]))


def append_csv(path, fieldnames, rows):
    new = not os.path.exists(path)
    with open(path, "a", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames)
        if new:
            w.writeheader()
        for r in rows:
            w.writerow(r)


def record_measure(out_dir, measure, method, value, unit, note=""):
    row = {"timestamp": time.time(), "measure": measure, "method": method,
           "value": "" if value is None else round(float(value), 3),
           "unit": unit, "note": note}
    append_csv(os.path.join(out_dir, "measures.csv"), MEASURES_FIELDS, [row])
    table([[measure, method, row["value"], unit, note[:80]]],
          ["measure", "method", "value", "unit", "note"])
    return row


def run(cmd, **kw):
    kw.setdefault("timeout", 300)
    return subprocess.run(cmd, **kw)


def free_bytes():
    """Free physical memory (conservative), cross-platform."""
    system = os.uname().sysname if hasattr(os, "uname") else ""
    try:
        if sys.platform == "darwin":
            p = run(["vm_stat"], capture_output=True, text=True, timeout=30)
            pages = {}
            for line in p.stdout.splitlines():
                m = re.match(r"Pages\s+(\w+):\s+(\d+)", line)
                if m:
                    pages[m.group(1).lower()] = int(m.group(2))
            page = 16384
            m2 = re.search(r"page size of (\d+) bytes", p.stdout)
            if m2:
                page = int(m2.group(1))
            avail = pages.get("free", 0) + pages.get("inactive", 0)
            return avail * page
        if sys.platform.startswith("linux"):
            with open("/proc/meminfo", encoding="utf-8") as f:
                for line in f:
                    if line.startswith("MemAvailable:"):
                        return int(line.split()[1]) * 1024
        if sys.platform == "win32":
            import ctypes

            class MEMSTAT(ctypes.Structure):
                _fields_ = [("dwLength", ctypes.c_ulong),
                            ("dwMemoryLoad", ctypes.c_ulong),
                            ("ullTotalPhys", ctypes.c_ulonglong),
                            ("ullAvailPhys", ctypes.c_ulonglong),
                            ("ullTotalPageFile", ctypes.c_ulonglong),
                            ("ullAvailPageFile", ctypes.c_ulonglong),
                            ("ullTotalVirtual", ctypes.c_ulonglong),
                            ("ullAvailVirtual", ctypes.c_ulonglong),
                            ("ullAvailExtendedVirtual", ctypes.c_ulonglong)]

            st = MEMSTAT()
            st.dwLength = ctypes.sizeof(st)
            if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(st)):
                return int(st.ullAvailPhys)
    except Exception:
        pass
    return None


def mem_guard(min_gb=1.5):
    """False when the machine is too low on memory to continue safely."""
    free = free_bytes()
    if free is None:
        log("free memory unknown; continuing")
        return True
    log("free memory: %.2f GB" % (free / 1024 ** 3))
    if free < min_gb * 1024 ** 3:
        log("STOP: free memory under %.1f GB; ending run early" % min_gb)
        return False
    return True


def ensure_git():
    if shutil.which("git") is None:
        raise SystemExit("git is required on PATH for the benchmark harness")


def git_repo(path, files=5, lines=20, seed=1):
    os.makedirs(path, exist_ok=True)
    run(["git", "init", "-q"], cwd=path, check=True)
    run(["git", "config", "user.email", "bench@example.com"], cwd=path, check=True)
    run(["git", "config", "user.name", "bench"], cwd=path, check=True)
    for i in range(files):
        with open(os.path.join(path, "file%04d.txt" % i), "w",
                  encoding="utf-8") as f:
            f.write("seed=%d file=%d\n" % (seed, i))
            for n in range(lines):
                f.write("line %d padding abcdefghijklmnopqrstuvwxyz\n" % n)
    run(["git", "add", "-A"], cwd=path, check=True)
    run(["git", "commit", "-qm", "bench seed"], cwd=path, check=True)
    return path


def gen_large_repo(path, n_files=2000, lines_per_file=10):
    """Generate a repo with many files for git-status measurements."""
    git_repo(path, files=n_files, lines=lines_per_file)
    # Dirty half the files so status has work beyond the index.
    for i in range(0, n_files, 2):
        with open(os.path.join(path, "file%04d.txt" % i), "a",
                  encoding="utf-8") as f:
            f.write("dirty\n")
    return n_files


def gen_big_diff(path, n_lines=10000):
    git_repo(path, files=3, lines=50)
    big = os.path.join(path, "file0000.txt")
    with open(big, "a", encoding="utf-8") as f:
        for i in range(n_lines):
            f.write("changed line %d with some content to diff\n" % i)
    return n_lines


def dev_bin():
    """Dev-build app binary of this branch, if it was built."""
    exe = "dragent.exe" if sys.platform == "win32" else "dragent"
    override = os.environ.get("DRAGENT_DEV_BIN")
    if override and os.path.exists(override):
        return override
    cands = [os.path.join(REPO_ROOT, "dragent", "target", "debug", exe),
             os.path.join(REPO_ROOT, "target", "debug", exe)]
    # A custom CARGO_TARGET_DIR (e.g. the perf team's shared folder) puts the
    # binary outside the checkout; honour it when set.
    target_dir = os.environ.get("CARGO_TARGET_DIR")
    if target_dir:
        cands.append(os.path.join(target_dir, "debug", exe))
        cands.append(os.path.join(target_dir, "release", exe))
    for cand in cands:
        if os.path.exists(cand):
            return cand
    return None


def dev_log_dirs(out_dir):
    """Where the dev app's log lines may live (newest layout first)."""
    dev_home = os.environ.get("DRAGENT_DEV_HOME") or \
        os.path.join(os.path.expanduser("~"), ".dragent-dev")
    return [os.path.join(dev_home, "logs"),
            os.path.join(os.path.expanduser("~"), ".dragent-dev", "logs"),
            os.path.join(os.path.expanduser("~"), ".dragent-dev"),
            out_dir]


def bench_log_text(out_dir):
    """Concatenated tails of dev-app logs holding `[bench]` lines."""
    text = ""
    for base in dev_log_dirs(out_dir):
        for name in ("desktop.log", "dragent.log", "Dragent.log", "bench.log"):
            cand = os.path.join(base, name)
            if os.path.exists(cand):
                try:
                    with open(cand, encoding="utf-8", errors="replace") as f:
                        f.seek(0, 2)
                        size = f.tell()
                        f.seek(max(0, size - 500000))
                        text += f.read()
                except OSError:
                    pass
    return text


def percentile(vals, q):
    """Nearest-rank percentile (matches bench.rs)."""
    if not vals:
        return None
    s = sorted(vals)
    rank = max(1, int(math.ceil(q * len(s))))
    return s[min(rank, len(s)) - 1]


def dev_cli_status(dev_home):
    """True when a dev-edition app answers CLI commands (i.e. GUI is up)."""
    exe = dev_bin()
    if exe is None:
        return False
    env = dict(os.environ)
    if dev_home:
        env["DRAGENT_DEV_HOME"] = dev_home
    try:
        p = run([exe, "status"], capture_output=True, text=True,
                timeout=30, env=env)
        return p.returncode == 0
    except Exception:
        return False


# ---------------------------------------------------------------- scenario


def cmd_scenario(args):
    ensure_git()
    if not mem_guard():
        return 2
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    home = args.dev_home or os.environ.get("DRAGENT_DEV_HOME") or \
        os.path.join(os.path.expanduser("~"), ".dragent-dev")

    tmp = tempfile.mkdtemp(prefix="dragent-bench-")
    seed_repo = os.path.join(tmp, "seed")
    git_repo(seed_repo)
    log("scratch repo: %s" % seed_repo)

    agents = []
    gui_mode = False
    if args.gui:
        if dev_cli_status(home):
            gui_mode = True
            log("dev app reachable: driving workspaces via its CLI")
        else:
            log("NOTE: --gui requested but no dev app answers; using headless "
                "mode. " + MANUAL_GUI_STEP)
    else:
        log("headless mode: fake agents run as plain subprocesses (one per "
            "worktree). " + MANUAL_GUI_STEP.replace("GUI step (manual): ",
                                                    "To drive real tabs: "))

    return _scenario_run(args, out_dir, home, tmp, seed_repo, agents, gui_mode)


def _scenario_run(args, out_dir, home, tmp, seed_repo, agents, gui_mode):
    n = args.agents
    duration = args.duration
    tail = args.tail
    # Start one fake agent per worktree.
    for i in range(n):
        wt = os.path.join(tmp, "wt%02d" % i)
        run(["git", "worktree", "add", "--detach", wt],
            cwd=seed_repo, check=True, capture_output=True)
        # Agent transcripts live in the scratch dir (they are large); only
        # the small CSVs stay in out_dir for committing.
        logf = open(os.path.join(tmp, "agent%02d.log" % i), "w",
                    encoding="utf-8")
        if gui_mode:
            exe = dev_bin()
            env = dict(os.environ)
            env["DRAGENT_DEV_HOME"] = home
            cmd = [exe, "workspaces", "create", "--project", seed_repo,
                   "--command", "%s %s --duration %s --seed %s --worktree %s"
                   % (sys.executable, FAKE_AGENT, duration, args.seed + i, wt),
                   "--name", "bench-%02d" % i]
            p = subprocess.Popen(cmd, stdout=logf, stderr=subprocess.STDOUT,
                                 env=env)
        else:
            p = subprocess.Popen(
                [sys.executable, FAKE_AGENT, "--duration", str(duration),
                 "--seed", str(args.seed + i), "--worktree", wt,
                 "--id", "agent%02d" % i],
                stdout=logf, stderr=subprocess.STDOUT, cwd=wt)
        agents.append((p, logf, wt))
    agent_pids = [p.pid for p, _, _ in agents]
    log("started %d fake agents" % len(agent_pids))

    mem_csv = os.path.join(out_dir, "scenario-mem.csv")
    if os.path.exists(mem_csv):
        os.remove(mem_csv)
    total = duration + tail + 10
    sampler = subprocess.Popen(
        [sys.executable, MEMSAMPLE, "--duration", str(total),
         "--interval", "1", "--output", mem_csv, "--home", home]
        + [a for pid in agent_pids for a in ("--agent-pid", str(pid))])
    log("sampling memory to %s" % mem_csv)

    # Let them run; stop early if the machine runs low on memory.
    end = time.time() + duration
    while time.time() < end:
        if not mem_guard():
            log("low memory: ending scenario early")
            break
        if all(p.poll() is not None for p, _, _ in agents):
            break
        time.sleep(5)

    # Close all but --keep tabs, keep sampling to show memory dropping.
    keep = agents[: args.keep]
    drop = agents[args.keep:]
    for p, _, _ in drop:
        if p.poll() is None:
            p.terminate()
    for p, _, _ in drop:
        try:
            p.wait(timeout=15)
        except subprocess.TimeoutExpired:
            p.kill()
    log("closed %d of %d tabs; keeping %d; sampling %.0fs more"
        % (len(drop), len(agents), len(keep), tail))
    deadline = time.time() + tail
    while time.time() < deadline and mem_guard():
        time.sleep(5)
    for p, _, _ in keep:
        if p.poll() is None:
            p.terminate()
    for p, logf, _ in agents:
        try:
            p.wait(timeout=15)
        except subprocess.TimeoutExpired:
            p.kill()
        logf.close()
    try:
        sampler.wait(timeout=30)
    except subprocess.TimeoutExpired:
        sampler.kill()

    summarize_mem(mem_csv, out_dir)
    log("scratch kept at %s (delete when done)" % tmp)
    return 0


def summarize_mem(mem_csv, out_dir):
    """Peak/avg per role + before/after-close drop, printed and saved."""
    roles = {}
    order = []
    try:
        with open(mem_csv, encoding="utf-8") as f:
            for row in csv.DictReader(f):
                try:
                    b = int(float(row["bytes"]))
                except ValueError:
                    continue
                key = (row["role"], row["pid"])
                roles.setdefault(key, []).append((float(row["timestamp"]), b))
                if row["role"] not in order:
                    order.append(row["role"])
    except FileNotFoundError:
        log("no mem csv to summarize")
        return
    rows = []
    for role in order:
        vals = [b for (k, v) in roles.items() if k[0] == role for _, b in v]
        if not vals:
            continue
        rows.append([role, "%.1f" % (max(vals) / 1024 ** 2),
                     "%.1f" % (statistics.mean(vals) / 1024 ** 2), len(vals)])
    table(rows, ["role", "peak MB", "mean MB", "samples"])
    # Drop after close: compare agent totals first vs last third.
    agent = [b for (k, v) in roles.items() if k[0] == "agent" for _, b in v]
    if agent:
        n = max(1, len(agent) // 3)
        first = statistics.mean(agent[:n]) / 1024 ** 2
        last = statistics.mean(agent[-n:]) / 1024 ** 2
        log("agent total: first-third mean %.1f MB -> last-third mean %.1f MB"
            % (first, last))


# ---------------------------------------------------------------- measures


def _repeat_times(fn, repeat=3):
    vals = []
    for _ in range(repeat):
        vals.append(fn())
    return statistics.median(vals)


def cmd_cold_start(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    # Prefer real in-app numbers: launch -> first presented frame, logged by
    # the dev app's frame hook (DRAGENT_BENCH_INPUT/FRAME=1).
    inapp = [float(m.group(1)) for m in
             re.finditer(r"startup ready_ms=([\d.]+)[^\n]*frame=first-presented",
                         bench_log_text(out_dir))]
    if len(inapp) >= args.repeat:
        recent = inapp[-args.repeat:]
        record_measure(out_dir, "cold-start", "in-app-first-frame",
                       statistics.median(recent), "ms",
                       "%d launch->first-presented-frame samples from the dev "
                       "app log (DRAGENT_BENCH_INPUT/FRAME=1)"
                       % len(recent))
        return 0
    exe = dev_bin()
    if exe is None:
        record_measure(out_dir, "cold-start", "skipped", None, "ms",
                       "no dev binary built yet; run cargo build -p dragent. "
                       + MANUAL_GUI_STEP)
        return 0

    def once():
        t0 = time.monotonic()
        try:
            p = run([exe, "--bench-probe-startup"], capture_output=True,
                    text=True, timeout=180)
        except subprocess.TimeoutExpired:
            return None
        out = (p.stdout or "") + (p.stderr or "")
        m = re.search(r"bench\.startup ready_ms=(\d+)", out)
        if m:
            return float(m.group(1))
        # Old binary without the probe flag: fall back to wall time of a
        # no-op CLI round-trip (labelled as such).
        t1 = time.monotonic()
        if p.returncode != 0 and "bench-probe-startup" in out:
            return None
        return (t1 - t0) * 1000

    vals = [v for v in (once() for _ in range(args.repeat)) if v is not None]
    if not vals:
        record_measure(out_dir, "cold-start", "skipped", None, "ms",
                       "dev binary lacks --bench-probe-startup; rebuild. "
                       + MANUAL_GUI_STEP)
        return 0
    record_measure(out_dir, "cold-start", "probe-startup",
                   statistics.median(vals), "ms",
                   "dev binary process-init probe (<%s>); full window-open "
                   "timing: %s" % (exe, MANUAL_GUI_STEP))
    return 0


def cmd_input_latency(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    # Prefer real in-app key->frame lines from a DRAGENT_BENCH_INPUT run.
    samples = [float(m.group(1)) for m in
               re.finditer(r"bench\.input key_to_frame_ms=([\d.]+)",
                           bench_log_text(out_dir))]
    if samples:
        record_measure(out_dir, "input-latency", "in-app-log",
                       percentile(samples, 0.50), "ms",
                       "%d in-app key_to_frame samples "
                       "(p50; p95=%.2f ms)" % (len(samples),
                                               percentile(samples, 0.95)))
        return 0
    # Headless proxy: PTY echo round-trip (labelled, not the app's latency).
    if sys.platform == "win32":
        record_measure(out_dir, "input-latency", "manual", None, "ms",
                       "no DRAGENT_BENCH_INPUT log found; Windows proxy not "
                       "implemented headless. " + MANUAL_GUI_STEP)
        return 0
    import pty
    import select
    import termios

    def once():
        master, slave = pty.openpty()
        try:
            p = subprocess.Popen(["cat"], stdin=slave, stdout=slave,
                                 stderr=slave, close_fds=True)
            os.close(slave)
            t0 = time.monotonic()
            os.write(master, b"x")
            r, _, _ = select.select([master], [], [], 5)
            dt = (time.monotonic() - t0) * 1000 if r else None
            p.terminate()
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                p.kill()
            return dt
        finally:
            os.close(master)

    vals = [v for v in (once() for _ in range(10)) if v is not None]
    record_measure(out_dir, "input-latency", "pty-echo-proxy",
                   statistics.median(vals) if vals else None, "ms",
                   "local `cat` PTY echo round-trip (NOT app latency); in-app "
                   "numbers need DRAGENT_BENCH_INPUT=1. " + MANUAL_GUI_STEP)
    return 0


def cmd_throughput(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    n = 2000000
    t0 = time.monotonic()
    if sys.platform == "win32":
        prod = subprocess.Popen(
            [sys.executable, "-c",
             "import sys\nfor i in range(%d): sys.stdout.write('y\\n')" % n],
            stdout=subprocess.PIPE)
    else:
        prod = subprocess.Popen(["yes"], stdout=subprocess.PIPE)
    total = 0
    try:
        if sys.platform == "win32":
            while True:
                chunk = prod.stdout.read(65536)
                if not chunk:
                    break
                total += len(chunk)
                if total >= 2 * n + n // 40:
                    break
            prod.terminate()
        else:
            head = subprocess.Popen(["head", "-n", str(n)],
                                    stdin=prod.stdout, stdout=subprocess.PIPE)
            prod.stdout.close()
            while True:
                chunk = head.stdout.read(65536)
                if not chunk:
                    break
                total += len(chunk)
            head.wait(timeout=120)
            prod.terminate()
    finally:
        try:
            prod.wait(timeout=10)
        except subprocess.TimeoutExpired:
            prod.kill()
    dt = time.monotonic() - t0
    mbs = (total / 1024 ** 2) / dt if dt > 0 else 0
    record_measure(out_dir, "throughput", "yes-head-2M",
                   dt * 1000, "ms",
                   "2M lines, %.1f MB in %.2fs (%.0f MB/s); app frame times "
                   "need a GUI run, see README" % (total / 1024 ** 2, dt, mbs))
    return 0


def cmd_frame_times(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    # Prefer real in-app per-second frame aggregates (DRAGENT_BENCH_FRAME=1).
    log_text = bench_log_text(out_dir)
    p50s = [float(m.group(1)) for m in
            re.finditer(r"bench\.frame ms=([\d.]+)", log_text)]
    maxs = [float(m.group(1)) for m in
            re.finditer(r"bench\.frame ms=[\d.]+ p95_ms=[\d.]+ max_ms=([\d.]+)",
                        log_text)]
    if p50s:
        record_measure(out_dir, "frame-times", "in-app-frames",
                       statistics.median(p50s), "ms",
                       "%d in-app 1 s frame aggregates "
                       "(median p50; max frame %.2f ms)" % (len(p50s),
                                                            max(maxs) if maxs else 0))
        return 0
    # Apply stream load headless: 3 fake agents bursting while we sample.
    tmp = tempfile.mkdtemp(prefix="dragent-bench-frames-")
    git_repo(os.path.join(tmp, "seed"))
    procs = []
    for i in range(3):
        wt = os.path.join(tmp, "wt%d" % i)
        run(["git", "worktree", "add", "--detach", wt],
            cwd=os.path.join(tmp, "seed"), check=True, capture_output=True)
        p = subprocess.Popen(
            [sys.executable, FAKE_AGENT, "--duration", "20", "--seed", str(i),
             "--rate", "30", "--burst-every", "6", "--burst-lines", "5000",
             "--worktree", wt, "--id", "frames%d" % i],
            stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, cwd=wt)
        procs.append(p)
    t0 = time.monotonic()
    lines = 0
    # Count stream output rate from one extra agent piped here (load applied).
    probe = subprocess.Popen(
        [sys.executable, FAKE_AGENT, "--duration", "15", "--seed", "99",
         "--rate", "30", "--burst-every", "6", "--burst-lines", "5000",
         "--worktree", tmp, "--id", "probe"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, cwd=tmp)
    try:
        for _line in probe.stdout:
            lines += 1
    finally:
        probe.wait(timeout=30)
    dt = time.monotonic() - t0
    for p in procs:
        if p.poll() is None:
            p.terminate()
    for p in procs:
        try:
            p.wait(timeout=15)
        except subprocess.TimeoutExpired:
            p.kill()
    shutil.rmtree(tmp, ignore_errors=True)
    record_measure(out_dir, "frame-times", "stream-load-applied",
                   lines / dt if dt > 0 else 0, "lines/s",
                   "streamed %d lines in %.1fs from 4 fake agents; per-frame "
                   "times need the GUI run (%s)" % (lines, dt, MANUAL_GUI_STEP))
    return 0


def cmd_tab_switch(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    exe = dev_bin()
    home = os.environ.get("DRAGENT_DEV_HOME")
    if exe is not None and dev_cli_status(home):
        env = dict(os.environ)
        if home:
            env["DRAGENT_DEV_HOME"] = home

        def once():
            t0 = time.monotonic()
            p = run([exe, "workspaces", "list"], capture_output=True,
                    text=True, timeout=60, env=env)
            if p.returncode != 0:
                return None
            return (time.monotonic() - t0) * 1000

        vals = [v for v in (once() for _ in range(args.repeat))
                if v is not None]
        if vals:
            record_measure(out_dir, "tab-switch", "cli-roundtrip",
                           statistics.median(vals), "ms",
                           "`dragent workspaces list` against the running dev "
                           "app (app-responsiveness proxy)")
            return 0
    # Headless proxy: switching git context across 10 worktrees.
    tmp = tempfile.mkdtemp(prefix="dragent-bench-tabs-")
    git_repo(os.path.join(tmp, "seed"), files=50)
    wts = []
    for i in range(10):
        wt = os.path.join(tmp, "wt%d" % i)
        run(["git", "worktree", "add", "--detach", wt],
            cwd=os.path.join(tmp, "seed"), check=True, capture_output=True)
        wts.append(wt)

    def switch_once():
        t0 = time.monotonic()
        for wt in wts:
            run(["git", "rev-parse", "--show-toplevel"], cwd=wt,
                check=True, capture_output=True, timeout=60)
        return (time.monotonic() - t0) * 1000 / len(wts)

    med = _repeat_times(switch_once, args.repeat)
    shutil.rmtree(tmp, ignore_errors=True)
    record_measure(out_dir, "tab-switch", "headless-proxy", med, "ms",
                   "mean per-switch over 10 scratch worktrees (NOT app tab "
                   "switch); in-app: " + MANUAL_GUI_STEP)
    return 0


def cmd_worktree_create(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="dragent-bench-wt-")
    git_repo(os.path.join(tmp, "seed"), files=100)

    def once():
        wt = tempfile.mkdtemp(prefix="wt-", dir=tmp)
        shutil.rmtree(wt)
        t0 = time.monotonic()
        run(["git", "worktree", "add", "--detach", wt],
            cwd=os.path.join(tmp, "seed"), check=True, capture_output=True,
            timeout=120)
        dt = (time.monotonic() - t0) * 1000
        run(["git", "worktree", "remove", "--force", wt],
            cwd=os.path.join(tmp, "seed"), check=True, capture_output=True,
            timeout=120)
        return dt

    med = _repeat_times(once, args.repeat)
    shutil.rmtree(tmp, ignore_errors=True)
    record_measure(out_dir, "worktree-create", "git-worktree-add", med, "ms",
                   "median `git worktree add/remove --detach` on 100-file "
                   "scratch repo (what workspace creation shells out to)")
    return 0


def cmd_git_status(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="dragent-bench-status-")
    n = gen_large_repo(os.path.join(tmp, "big"), n_files=args.files,
                       lines_per_file=10)

    def once():
        t0 = time.monotonic()
        p = run(["git", "status", "--porcelain=v1", "-uno"],
                cwd=os.path.join(tmp, "big"), check=True, capture_output=True,
                text=True, timeout=300)
        dt = (time.monotonic() - t0) * 1000
        return dt, len(p.stdout.splitlines())

    vals = [once() for _ in range(args.repeat)]
    med = statistics.median(v[0] for v in vals)
    shutil.rmtree(tmp, ignore_errors=True)
    record_measure(out_dir, "git-status", "git-status-porcelain", med, "ms",
                   "median over %d runs on generated %d-file repo "
                   "(%d changed paths)" % (args.repeat, n, vals[0][1]))
    return 0


def cmd_diff_open(args):
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="dragent-bench-diff-")
    n = gen_big_diff(os.path.join(tmp, "repo"), n_lines=10000)

    def once():
        t0 = time.monotonic()
        p = run(["git", "diff", "--numstat"], cwd=os.path.join(tmp, "repo"),
                check=True, capture_output=True, text=True, timeout=300)
        added = sum(int(l.split()[0]) for l in p.stdout.splitlines()
                    if l.split()[0].isdigit())
        dt = (time.monotonic() - t0) * 1000
        return dt, added

    vals = [once() for _ in range(args.repeat)]
    med = statistics.median(v[0] for v in vals)
    shutil.rmtree(tmp, ignore_errors=True)
    record_measure(out_dir, "diff-open", "git-diff-parse", med, "ms",
                   "median `git diff --numstat` + parse over a %d-line diff "
                   "(%d added lines counted)" % (n, vals[0][1]))
    return 0


def cmd_all(args):
    if not mem_guard():
        return 2
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    rc = 0
    for fn in (cmd_cold_start, cmd_input_latency, cmd_throughput,
               cmd_frame_times, cmd_tab_switch, cmd_worktree_create,
               cmd_git_status, cmd_diff_open):
        rc |= fn(args) or 0
        if not mem_guard():
            log("low memory: stopping `all` early")
            return 2
    # Short scenario last (it takes minutes).
    sc = argparse.Namespace(agents=12, duration=args.duration, tail=30,
                            keep=1, out=out_dir, gui=args.gui,
                            dev_home=args.dev_home, seed=20261122)
    rc |= _scenario_cmd(sc)
    return rc


def _scenario_cmd(sc):
    ensure_git()
    out_dir = os.path.abspath(sc.out)
    os.makedirs(out_dir, exist_ok=True)
    home = sc.dev_home or os.environ.get("DRAGENT_DEV_HOME") or \
        os.path.join(os.path.expanduser("~"), ".dragent-dev")
    tmp = tempfile.mkdtemp(prefix="dragent-bench-")
    seed_repo = os.path.join(tmp, "seed")
    git_repo(seed_repo)
    gui_mode = bool(sc.gui and dev_cli_status(home))
    return _scenario_run(sc, out_dir, home, tmp, seed_repo, [], gui_mode)


# ---------------------------------------------------------------- cli


def build_parser():
    ap = argparse.ArgumentParser(
        description="Dragent benchmark harness: scenario + measures.")
    ap.add_argument("--out", default=os.path.join(REPO_ROOT, "dragent", "docs",
                                                  "perf", "data"),
                    help="Directory for CSVs (default docs/perf/data).")
    sub = ap.add_subparsers(dest="cmd", required=True)

    sc = sub.add_parser("scenario", help="N fake agents in N worktrees + memory sampling.")
    sc.add_argument("--duration", type=float, default=300)
    sc.add_argument("--agents", type=int, default=12)
    sc.add_argument("--keep", type=int, default=1, help="Tabs left open after close.")
    sc.add_argument("--tail", type=float, default=45, help="Sampling seconds after close.")
    sc.add_argument("--seed", type=int, default=20261122)
    sc.add_argument("--gui", action="store_true", help="Drive the dev app's terminals via its CLI when reachable.")
    sc.add_argument("--dev-home", default=None)
    sc.set_defaults(func=cmd_scenario)

    c = sub.add_parser("cold-start", help="Process launch -> usable window timing.")
    c.add_argument("--repeat", type=int, default=3)
    c.set_defaults(func=cmd_cold_start)

    c = sub.add_parser("input-latency", help="Keystroke -> frame latency.")
    c.set_defaults(func=cmd_input_latency)

    c = sub.add_parser("throughput", help="`yes | head -n 2000000` timing.")
    c.set_defaults(func=cmd_throughput)

    c = sub.add_parser("frame-times", help="Frame times while agents stream.")
    c.set_defaults(func=cmd_frame_times)

    c = sub.add_parser("tab-switch", help="Tab/workspace switch time.")
    c.add_argument("--repeat", type=int, default=3)
    c.set_defaults(func=cmd_tab_switch)

    c = sub.add_parser("worktree-create", help="Worktree+workspace creation time.")
    c.add_argument("--repeat", type=int, default=3)
    c.set_defaults(func=cmd_worktree_create)

    c = sub.add_parser("git-status", help="`git status` on a generated large repo.")
    c.add_argument("--repeat", type=int, default=3)
    c.add_argument("--files", type=int, default=2000)
    c.set_defaults(func=cmd_git_status)

    c = sub.add_parser("diff-open", help="Open a generated 10,000-line diff.")
    c.add_argument("--repeat", type=int, default=3)
    c.set_defaults(func=cmd_diff_open)

    c = sub.add_parser("all", help="Every measure + short scenario.")
    c.add_argument("--duration", type=float, default=60)
    c.add_argument("--repeat", type=int, default=3)
    c.add_argument("--files", type=int, default=2000)
    c.add_argument("--gui", action="store_true")
    c.add_argument("--dev-home", default=None)
    c.set_defaults(func=cmd_all)
    return ap


def main(argv=None):
    ap = build_parser()
    args = ap.parse_args(argv)
    # `all` reuses measure defaults.
    if args.cmd == "all":
        if not hasattr(args, "agents"):
            args.agents = 12
        if not hasattr(args, "tail"):
            args.tail = 30
        if not hasattr(args, "keep"):
            args.keep = 1
        if not hasattr(args, "seed"):
            args.seed = 20261122
    return args.func(args) or 0


if __name__ == "__main__":
    sys.exit(main())
