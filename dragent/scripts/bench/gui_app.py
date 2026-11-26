#!/usr/bin/env python3
"""GUI-driven benchmark measures against the REAL dev app (in-app numbers).

Launches the dev-edition binary with a scratch DRAGENT_DEV_HOME (created here
and deleted afterwards; never ~/.dragent or the installed app), drives it via
the env-gated automation server (DRAGENT_AUTOMATION_PORT/TOKEN), and records
measures with method=in-app-* into measures.csv:

  cold-start     3 launches; launch -> first presented frame (in-app ready_ms
                 from the frame hook + wall time to the log line)
  input-latency  >=60 automation `run` submissions; key->frame p50/p95
  frame-times    p50/p95/max of the 1 s bench.frame aggregates while 10 fake
                 agents stream in a real app terminal
  tab-switch     stays a proxy (unsigned CLI cannot reach panes; automation
                 has no tab-switch)
  worktree-create  stays a proxy (`workspaces create` needs sign-in)
  scenario       12 fake agents in the app terminal with memsample roles
                 (gui/host/pty/index/computer vs agent sum), then 9 stopped

Requires a binary built with --features performance-harness and a desktop
session to open windows. The app runs with DRAGENT_DEV_TERMINAL=1 (a dev
window holding a single LOCAL terminal): production tabs are host-owned and
never register with the automation server, and the unsigned dev CLI cannot
open tabs (signing into a dev build is forbidden).

Stdlib only: macOS, Windows and Linux with no installs.
"""

import argparse
import csv
import json
import os
import random
import re
import secrets
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
FAKE_AGENT = os.path.join(HERE, "fake_agent.py")
MEMSAMPLE = os.path.join(HERE, "memsample.py")
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
MEASURES_FIELDS = ["timestamp", "measure", "method", "value", "unit", "note"]


def log(msg):
    print("[gui-bench] %s" % msg, flush=True)


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


def free_gb():
    try:
        if sys.platform == "darwin":
            p = subprocess.run(["vm_stat"], capture_output=True, text=True,
                               timeout=30)
            pages = {}
            for line in p.stdout.splitlines():
                m = re.match(r"Pages\s+(\w+):\s+(\d+)", line)
                if m:
                    pages[m.group(1).lower()] = int(m.group(2))
            return ((pages.get("free", 0) + pages.get("inactive", 0))
                    * 16384 / 1024 ** 3)
        if sys.platform.startswith("linux"):
            with open("/proc/meminfo", encoding="utf-8") as f:
                for line in f:
                    if line.startswith("MemAvailable:"):
                        return int(line.split()[1]) / 1024 ** 2
    except Exception:
        pass
    return None


def mem_ok():
    free = free_gb()
    if free is None:
        log("free memory unknown; continuing")
        return True
    log("free memory: %.2f GB" % free)
    if free < 1.5:
        log("STOP: free memory under 1.5 GB")
        return False
    return True


def dev_bin(args):
    exe = "dragent.exe" if sys.platform == "win32" else "dragent"
    override = args.dev_bin or os.environ.get("DRAGENT_DEV_BIN")
    if override and os.path.exists(override):
        return override
    target_dir = os.environ.get("CARGO_TARGET_DIR")
    cands = []
    if target_dir:
        cands += [os.path.join(target_dir, "debug", exe)]
    cands += [os.path.join(REPO_ROOT, "dragent", "target", "debug", exe)]
    for cand in cands:
        if os.path.exists(cand):
            return cand
    return None


def clean_env(home, port, token):
    env = dict(os.environ)
    for key in ("DRAGENT_HOST_WORKSPACE_ID", "DRAGENT_HOST_TERMINAL_ID",
                "DRAGENT_PTY_LISTENER_FD", "DRAGENT_PTY_HANDOFF_SNAPSHOT",
                "DRAGENT_PTY_TOKEN", "DRAGENT_PTY_HANDOFF_FD",
                "DRAGENT_HOST_WORKSPACE_ID", "DRAGENT_DEV_SERVER",
                "DRAGENT_AUTOMATION_PORT", "DRAGENT_AUTOMATION_TOKEN"):
        env.pop(key, None)
    env["DRAGENT_DEV_HOME"] = home
    env["DRAGENT_BENCH_INPUT"] = "1"
    env["DRAGENT_BENCH_FRAME"] = "1"
    env["DRAGENT_BENCH_MEM"] = "1"
    env["DRAGENT_INDEX_DISABLE"] = "1"
    # Dev-terminal window (single LOCAL terminal; needs the
    # performance-harness build): production tabs are host-owned and never
    # register with the automation server, so without this `run`/`blocks`
    # always answer "needs a terminal session".
    env["DRAGENT_DEV_TERMINAL"] = "1"
    env["DRAGENT_AUTOMATION_PORT"] = str(port)
    env["DRAGENT_AUTOMATION_TOKEN"] = token
    return env


def pick_port():
    return 21000 + (os.getpid() % 8000) + random.randint(0, 500)


def dev_log_path(home):
    return os.path.join(home, "logs", "desktop.log")


def read_log_new(home, offset):
    """(text appended since offset, new offset) of the dev app log."""
    path = dev_log_path(home)
    try:
        size = os.path.getsize(path)
    except OSError:
        return "", offset
    if size < offset:
        offset = 0
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            f.seek(offset)
            text = f.read()
        return text, size
    except OSError:
        return "", offset


class AutoConn:
    """One JSON-lines automation connection (auth first, then requests)."""

    def __init__(self, port, token):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=30)
        self.file = self.sock.makefile("r", encoding="utf-8", errors="replace")
        reply = self.request({"cmd": "auth", "token": token}, raw=True)
        if not reply.get("ok"):
            raise RuntimeError("automation auth refused: %r" % (reply,))

    def request(self, req, raw=False, timeout=60):
        self.sock.settimeout(timeout)
        self.sock.sendall((json.dumps(req) + "\n").encode())
        line = self.file.readline()
        if not line:
            raise RuntimeError("automation connection closed")
        return json.loads(line)

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


def wait_automation(port, token, deadline_s=150):
    """Connect+auth loop; returns AutoConn once the app's server is up."""
    end = time.time() + deadline_s
    last = None
    while time.time() < end:
        try:
            conn = AutoConn(port, token)
            ping = conn.request({"cmd": "ping"}, timeout=10)
            if ping.get("ok"):
                return conn
            conn.close()
        except Exception as e:  # noqa: BLE001 - retry until deadline
            last = e
        time.sleep(2)
    raise RuntimeError("automation port never came up: %r" % (last,))


def spawn_app(exe, env):
    """Start the dev app; returns the Popen (caller stops only this)."""
    proc = subprocess.Popen([exe], env=env,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    return proc


def stop_app(proc, conn=None):
    if conn is not None:
        try:
            conn.request({"cmd": "quit"}, timeout=10)
        except Exception:
            pass
        conn.close()
    try:
        proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass


def cli(exe, home, *argv, timeout=120):
    env = dict(os.environ)
    env["DRAGENT_DEV_HOME"] = home
    # Deterministic machine-readable output (see cli/mod.rs Flags::parse).
    env["DRAGENT_JSON"] = "1"
    for key in ("DRAGENT_HOST_TERMINAL_ID", "DRAGENT_HOST_WORKSPACE_ID"):
        env.pop(key, None)
    p = subprocess.run([exe] + list(argv), capture_output=True, text=True,
                       timeout=timeout, env=env)
    return p


def cli_json(exe, home, *argv, timeout=120):
    p = cli(exe, home, *argv, timeout=timeout)
    try:
        return json.loads(p.stdout or "{}"), p
    except ValueError:
        return {"_raw": p.stdout, "_stderr": p.stderr,
                "_rc": p.returncode}, p


def wait_log_pattern(home, offset, pattern, timeout, poll=1.0):
    """Wait for a regex in the dev log; returns (match, new_offset) or (None, off)."""
    end = time.time() + timeout
    while time.time() < end:
        text, offset = read_log_new(home, offset)
        m = re.search(pattern, text)
        if m:
            return m, offset
        time.sleep(poll)
    text, offset = read_log_new(home, offset)
    return re.search(pattern, text), offset


def log_samples_since(home, offset, pattern):
    text, offset = read_log_new(home, offset)
    return [float(m.group(1)) for m in re.finditer(pattern, text)], offset


# ---------------------------------------------------------------- measures


def measure_cold_start(exe, home, out_dir, repeat=3):
    ready, walls = [], []
    for i in range(repeat):
        if not mem_ok():
            break
        port = pick_port()
        token = secrets.token_hex(24)
        env = clean_env(home, port, token)
        try:
            os.remove(dev_log_path(home))
        except OSError:
            pass
        t0 = time.monotonic()
        proc = spawn_app(exe, env)
        try:
            m, _ = wait_log_pattern(
                home, 0, r"startup ready_ms=([\d.]+)[^\n]*frame=first-presented",
                timeout=240)
            wall = (time.monotonic() - t0) * 1000
            if m:
                ready.append(float(m.group(1)))
                walls.append(wall)
                log("cold start run %d: ready_ms=%s wall=%.0f ms"
                    % (i + 1, m.group(1), wall))
            else:
                log("cold start run %d: no first-frame line in 240 s" % (i + 1,))
        finally:
            try:
                conn = wait_automation(port, token, deadline_s=20)
            except RuntimeError:
                conn = None
            stop_app(proc, conn)
            # Let the window server release the app before the next launch.
            time.sleep(5)
    if ready:
        record_measure(out_dir, "cold-start", "in-app-first-frame",
                       statistics.median(ready), "ms",
                       "launch->first presented frame, in-app ready_ms "
                       "(dev-terminal window, performance-harness build; "
                       "wall median %.0f ms, %d runs)" % (statistics.median(walls),
                                                          len(ready)))
        return True
    record_measure(out_dir, "cold-start", "in-app-first-frame", None, "ms",
                   "no first-presented-frame line; app may not have opened")
    return False


def ensure_first_terminal(conn, seed_dir):
    """Wait for the first terminal session (automation `run` needs it).

    The app runs with DRAGENT_DEV_TERMINAL=1 (a dev window holding a single
    LOCAL terminal; production tabs are host-owned and never register, and
    the unsigned dev CLI cannot open tabs). Just poll until `blocks`
    answers.
    """
    del seed_dir
    # The automation server appears once the first terminal session exists.
    end = time.time() + 120
    while time.time() < end:
        try:
            reply = conn.request({"cmd": "blocks"}, timeout=15)
            if reply.get("ok"):
                return True
        except Exception:
            pass
        time.sleep(2)
    return False


def wait_block_done(conn, marker, timeout=30):
    """True once the automation `run` command's block completes."""
    end = time.time() + timeout
    while time.time() < end:
        try:
            reply = conn.request({"cmd": "blocks"}, timeout=15)
        except Exception:
            time.sleep(0.5)
            continue
        if reply.get("ok"):
            for b in reply.get("blocks", []):
                if marker in (b.get("command") or "") and b.get("completed"):
                    return True
        time.sleep(0.3)
    return False


def measure_input_latency(exe, home, conn, out_dir, n_keys=60):
    try:
        off = os.path.getsize(dev_log_path(home))
    except OSError:
        off = 0
    sent = 0
    for i in range(n_keys):
        if not mem_ok():
            break
        marker = "lat-%04d" % i
        try:
            reply = conn.request({"cmd": "run", "text": "echo %s" % marker},
                                 timeout=30)
        except Exception as e:  # noqa: BLE001 - keep the samples we have
            log("run %d failed: %s" % (i, e))
            break
        if not reply.get("ok"):
            log("run %d refused: %s" % (i, reply))
            break
        # Space keys by completion so each key gets its own presented frame.
        wait_block_done(conn, marker, timeout=30)
        sent += 1
    # Frames flush within a second or two; wait for the tail.
    time.sleep(5)
    samples, _ = log_samples_since(
        home, off, r"bench\.input key_to_frame_ms=([\d.]+)")
    log("input latency: %d runs, %d key_to_frame samples" % (sent, len(samples)))
    if len(samples) >= 50:
        p50 = statistics.median(samples)
        p95 = statistics.quantiles(samples, n=100)[94]
        record_measure(out_dir, "input-latency", "in-app-log", p50, "ms",
                       "%d in-app key_to_frame samples (p50; p95=%.2f ms)"
                       % (len(samples), p95))
        return True
    record_measure(out_dir, "input-latency", "in-app-log", None, "ms",
                   "only %d/%d key_to_frame samples; need >=50" % (len(samples),
                                                                   sent))
    return False


def open_stream_tabs(exe, home, tmp, count, duration, seed_base, tag="x"):
    """Start `count` fake agents streaming through real app PTYs.

    Unsigned dev CLI cannot open N app tabs (`workspaces create` answers
    signed_out and signing into a dev build is forbidden), and automation
    `run` addresses the first terminal session only — so all agents run as
    background jobs in the first real app terminal. Same real PTY, grid,
    frames and pty-service memory; tabs stay at 1 (noted on every row).
    Returns [(worktree path, pid|None)].
    """
    del exe, home
    seed_repo = os.path.join(tmp, "seed-%s" % tag)
    subprocess.run(["git", "init", "-q", seed_repo], check=True)
    subprocess.run(["git", "config", "user.email", "bench@example.com"],
                   cwd=seed_repo, check=True)
    subprocess.run(["git", "config", "user.name", "bench"], cwd=seed_repo,
                   check=True)
    with open(os.path.join(seed_repo, "f.txt"), "w", encoding="utf-8") as f:
        f.write("seed\n")
    subprocess.run(["git", "add", "-A"], cwd=seed_repo, check=True)
    subprocess.run(["git", "commit", "-qm", "bench seed"], cwd=seed_repo,
                   check=True)
    wts = []
    for i in range(count):
        wt = os.path.join(tmp, "wt-%s%02d" % (tag, i))
        subprocess.run(["git", "worktree", "add", "--detach", wt],
                       cwd=seed_repo, check=True, capture_output=True)
        wts.append(wt)
    return [(wt, None) for wt in wts]


def start_stream_in_session(conn, wts, duration, seed_base, tag):
    """Background one fake agent per worktree in the first app terminal."""
    import shlex
    parts = []
    for i, wt in enumerate(wts):
        parts.append(
            "nohup %s %s --duration %s --seed %s --worktree %s --id %s%d "
            "> /dev/null 2>&1 &"
            % (shlex.quote(sys.executable), shlex.quote(FAKE_AGENT),
               duration, seed_base + i, shlex.quote(wt), tag, i))
    # One line: disown so the shell prompt returns at once.
    cmd = "{ sleep 1; } ; " + " ".join(parts) + " echo STREAM_STARTED_%s" % tag
    try:
        reply = conn.request({"cmd": "run", "text": cmd}, timeout=30)
    except Exception as e:  # noqa: BLE001
        log("stream run failed: %s" % e)
        return False
    if not reply.get("ok"):
        log("stream run refused: %s" % reply)
        return False
    return wait_block_done(conn, "STREAM_STARTED_%s" % tag, timeout=60)


def wait_agent_pids(marker, count, timeout=60):
    """Poll until `count` fake-agent PIDs carrying our marker exist."""
    agent_pids = []
    end = time.time() + timeout
    while time.time() < end:
        agent_pids = find_agent_pids(marker)
        if len(agent_pids) >= count:
            break
        time.sleep(3)
    return agent_pids


def kill_pids(pids):
    """SIGTERM only PIDs we discovered (our agents); never anything else."""
    for pid in pids:
        try:
            subprocess.run(["kill", str(pid)], timeout=10)
        except Exception:
            pass


def perf_snapshot(conn):
    """Render/process counters from the automation server (or {})."""
    try:
        reply = conn.request({"cmd": "perf_stats"}, timeout=30)
    except Exception:
        return {}
    if not reply.get("ok"):
        return {}
    return reply.get("render", {})


def measure_frame_times(conn, home, out_dir, tmp, duration=75, count=10):
    try:
        off = os.path.getsize(dev_log_path(home))
    except OSError:
        off = 0
    before = perf_snapshot(conn)
    wts = [wt for wt, _ in open_stream_tabs(None, None, tmp, count,
                                            duration + 20, 7100, "frames")]
    if not start_stream_in_session(conn, wts, duration + 20, 7100, "frames"):
        record_measure(out_dir, "frame-times", "in-app-frames", None, "ms",
                       "stream agents never started in the app terminal")
        return False
    marker = os.path.basename(tmp)
    if not wait_agent_pids(marker, len(wts), timeout=45):
        record_measure(out_dir, "frame-times", "in-app-frames", None, "ms",
                       "fake agents not found after stream start; not measured")
        return False
    log("streaming %d agents in 1 real app terminal for %ds"
        % (len(wts), duration))
    end = time.time() + duration
    while time.time() < end and mem_ok():
        time.sleep(5)
    text, _ = read_log_new(home, off)
    p50s = [float(m.group(1)) for m in
            re.finditer(r"bench\.frame ms=([\d.]+)", text)]
    p95s = [float(m.group(1)) for m in
            re.finditer(r"bench\.frame ms=[\d.]+ p95_ms=([\d.]+)", text)]
    maxs = [float(m.group(1)) for m in
            re.finditer(r"bench\.frame ms=[\d.]+ p95_ms=[\d.]+ max_ms=([\d.]+)",
                        text)]
    log("frame aggregates: %d one-second lines" % len(p50s))
    kill_pids(find_agent_pids(marker))
    after = perf_snapshot(conn)
    render_note = ""
    try:
        frames = after.get("frames", 0) - before.get("frames", 0)
        # average/max are cumulative since launch (no window mean available);
        # frames-in-window gives the presented rate over the stream.
        render_note = ("render: %d frames presented in window, cumulative "
                       "render avg %.2f ms/frame, max %.2f ms/frame"
                       % (frames, after.get("average_frame_ms", 0),
                          after.get("max_frame_ms", 0)))
    except Exception:
        pass
    if p50s:
        record_measure(out_dir, "frame-times", "in-app-frames",
                       statistics.median(p50s), "ms",
                       "%d in-app 1 s frame-interval aggregates; %d agents in "
                       "1 real app terminal (median interval p50; interval "
                       "p95 max %.2f ms; interval max %.2f ms). %s"
                       % (len(p50s), len(wts), max(p95s), max(maxs),
                          render_note))
        return True
    record_measure(out_dir, "frame-times", "in-app-frames", None, "ms",
                   "no bench.frame lines while %d agents streamed"
                   % len(wts))
    return False


def measure_tab_switch(exe, home, out_dir, repeat=20):
    # The dev CLI needs a signed-in user (`workspaces`/`panes` answer
    # signed_out unsigned) and `panes list` from a plain shell hangs without
    # a terminal context, so in-app tab switching stays a proxy: the
    # automation server has no tab-switch command. One attempt with a short
    # timeout to confirm, then label it honestly.
    try:
        p = cli(exe, home, "panes", "list", timeout=25)
        rc, out = p.returncode, p.stdout or ""
    except subprocess.TimeoutExpired:
        rc, out = -1, ""
    if rc != 0 or '"panes"' not in out:
        record_measure(out_dir, "tab-switch", "headless-proxy", None, "ms",
                       "stayed proxy: unsigned CLI cannot reach panes "
                       "(rc=%s); automation has no tab-switch command" % rc)
        return False
    record_measure(out_dir, "tab-switch", "headless-proxy", None, "ms",
                   "stayed proxy: panes reachable but focus needs a signed-in "
                   "terminal context; run.py headless-proxy row applies")
    return False


def measure_worktree_create(exe, home, out_dir, tmp, repeat=3):
    # `workspaces create` needs a signed-in user and signing into a dev build
    # is forbidden, so workspace creation stays the git proxy (run.py). One
    # attempt to confirm the refusal, then label it honestly.
    seed_repo = os.path.join(tmp, "wtseed")
    os.makedirs(seed_repo, exist_ok=True)
    try:
        p = cli_json(exe, home, "workspaces", "create", "--project",
                     seed_repo, "--command", "sleep 5",
                     "--name", "bench-wt-probe", timeout=60)[1]
        rc = p.returncode
    except subprocess.TimeoutExpired:
        rc = -1
    record_measure(out_dir, "worktree-create", "git-worktree-add", None, "ms",
                   "stayed proxy: workspaces create needs sign-in (rc=%s); "
                   "run.py git-worktree-add row applies" % rc)
    return False


def find_agent_pids(marker):
    """PIDs of fake_agent.py processes carrying our tmp marker."""
    pids = []
    try:
        p = subprocess.run(["ps", "-ax", "-o", "pid=,command="],
                           capture_output=True, text=True, timeout=30)
    except Exception:
        return pids
    for line in p.stdout.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) != 2:
            continue
        if "fake_agent.py" in parts[1] and marker in parts[1]:
            try:
                pids.append(int(parts[0]))
            except ValueError:
                continue
    return pids


def scenario_memory(conn, home, app_pid, out_dir, tmp, duration=150, agents=12,
                    keep=3, tail=30):
    mem_csv = os.path.join(out_dir, "gui-scenario-mem.csv")
    if os.path.exists(mem_csv):
        os.remove(mem_csv)
    marker = os.path.basename(tmp)
    wts = [wt for wt, _ in open_stream_tabs(None, None, tmp, agents,
                                            duration + tail + 30,
                                            seed_base=20261122, tag="scn")]
    if not start_stream_in_session(conn, wts, duration + tail + 30,
                                   20261122, "scn"):
        log("scenario: stream agents never started")
        return 1
    # Fake agents are children of the app tree; find their real PIDs by the
    # scratch marker so memsample sums exactly them as `agent`.
    agent_pids = wait_agent_pids(marker, len(wts), timeout=60)
    if not agent_pids:
        log("scenario: no agent PIDs found; not measured")
        return 1
    log("scenario: %d agents in 1 real app terminal, %d PIDs found"
        % (len(wts), len(agent_pids)))
    sampler = subprocess.Popen(
        [sys.executable, MEMSAMPLE, "--duration",
         str(duration + tail + 15), "--interval", "1", "--output", mem_csv,
         "--home", home, "--root-pid", str(app_pid)]
        + [a for pid in agent_pids for a in ("--agent-pid", str(pid))])
    log("sampling memory to %s" % mem_csv)
    end = time.time() + duration
    while time.time() < end and mem_ok():
        time.sleep(5)
    # Close agents down to `keep` (kill, the unsigned CLI cannot close tabs)
    # and keep sampling to show Dragent's own memory dropping.
    drop_pids, keep_pids = agent_pids[keep:], agent_pids[:keep]
    kill_pids(drop_pids)
    log("stopped %d of %d agents; keeping %d; sampling %.0fs more"
        % (len(drop_pids), len(agent_pids), len(keep_pids), tail))
    deadline = time.time() + tail
    while time.time() < deadline and mem_ok():
        time.sleep(5)
    kill_pids(keep_pids)
    # Belt and braces: only PIDs discovered above, only while sampling.
    kill_pids(find_agent_pids(marker))
    try:
        sampler.wait(timeout=60)
    except subprocess.TimeoutExpired:
        sampler.kill()
    summarize_mem(mem_csv, out_dir)
    return 0


def summarize_mem(mem_csv, out_dir):
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
                     "%.1f" % (statistics.mean(vals) / 1024 ** 2),
                     len(vals)])
    table(rows, ["role", "peak MB", "mean MB", "samples"])
    agent = [b for (k, v) in roles.items() if k[0] == "agent" for _, b in v]
    if agent:
        n = max(1, len(agent) // 3)
        first = statistics.mean(agent[:n]) / 1024 ** 2
        last = statistics.mean(agent[-n:]) / 1024 ** 2
        log("agent total: first-third mean %.1f MB -> last-third mean %.1f MB"
            % (first, last))


# ---------------------------------------------------------------- cli


def build_parser():
    ap = argparse.ArgumentParser(
        description="GUI-driven in-app benchmark measures.")
    ap.add_argument("--out", default=os.path.join(REPO_ROOT, "dragent", "docs",
                                                  "perf", "data"),
                    help="Directory for CSVs.")
    ap.add_argument("--dev-bin", default=None)
    ap.add_argument("--duration", type=float, default=150,
                    help="Scenario streaming seconds.")
    ap.add_argument("--agents", type=int, default=12)
    ap.add_argument("--keep", type=int, default=3)
    ap.add_argument("--tail", type=float, default=30)
    ap.add_argument("--frame-secs", type=float, default=75)
    ap.add_argument("--keys", type=int, default=60)
    ap.add_argument("--skip-scenario", action="store_true")
    ap.add_argument("--only", default=None,
                    help="Comma list: cold-start,input-latency,frame-times,"
                         "tab-switch,worktree-create,scenario")
    return ap


def cmd_all(args):
    if not mem_ok():
        return 2
    exe = dev_bin(args)
    if exe is None:
        print("no dev binary built yet; run cargo build -p dragent from dragent/",
              flush=True)
        return 1
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="dragent-gui-bench-")
    home = tempfile.mkdtemp(prefix="dragent-bench-home-")
    only = (args.only.split(",") if args.only
            else ["cold-start", "input-latency", "frame-times", "tab-switch",
                  "worktree-create", "scenario"])
    log("dev binary: %s" % exe)
    log("scratch home: %s (deleted afterwards)" % home)
    rc = 0
    conn = None
    proc = None
    try:
        if "cold-start" in only:
            if not measure_cold_start(exe, home, out_dir):
                rc |= 1
        # Fresh app for the interactive measures.
        port = pick_port()
        token = secrets.token_hex(24)
        env = clean_env(home, port, token)
        proc = spawn_app(exe, env)
        conn = wait_automation(port, token)
        log("automation server up; driving the real app")
        seed_dir = os.path.join(tmp, "keys-seed")
        os.makedirs(seed_dir, exist_ok=True)
        has_session = ensure_first_terminal(conn, seed_dir)
        log("first terminal session: %s" % has_session)
        if "input-latency" in only:
            if not has_session or not measure_input_latency(
                    exe, home, conn, out_dir, n_keys=args.keys):
                if has_session:
                    rc |= 1
                else:
                    record_measure(out_dir, "input-latency", "in-app-log",
                                   None, "ms", "no terminal session: automation "
                                   "`run` needs the performance-harness build")
        if "frame-times" in only:
            if has_session:
                measure_frame_times(conn, home, out_dir, tmp,
                                    duration=args.frame_secs)
            else:
                record_measure(out_dir, "frame-times", "in-app-frames", None,
                               "ms", "no terminal session to stream in")
        if "tab-switch" in only:
            measure_tab_switch(exe, home, out_dir)
        if "worktree-create" in only:
            measure_worktree_create(exe, home, out_dir, tmp)
        if "scenario" in only and not args.skip_scenario:
            if not has_session:
                record_measure(out_dir, "scenario", "in-app", None, "",
                               "no terminal session to stream in")
            elif scenario_memory(conn, home, proc.pid, out_dir, tmp,
                                 duration=args.duration, agents=args.agents,
                                 keep=args.keep, tail=args.tail):
                rc |= 1
    finally:
        if proc is not None:
            stop_app(proc, conn)
        shutil.rmtree(tmp, ignore_errors=True)
        shutil.rmtree(home, ignore_errors=True)
        log("scratch dirs deleted")
    return rc


def main(argv=None):
    args = build_parser().parse_args(argv)
    return cmd_all(args)


if __name__ == "__main__":
    sys.exit(main())

