#!/usr/bin/env python3
"""Memory sampler for Dragent benchmark runs.

Every `--interval` seconds records Dragent's own processes separately
(GUI app, host service, terminal/PTY service, code index service) and the
agents' processes (summed) into CSV (`timestamp,role,pid,bytes`).

Only processes belonging to the dev edition are counted: `DRAGENT_HOME` is
`~/.dragent-dev` (or `DRAGENT_DEV_HOME` / a scratch home the harness made).
The installed release app (`/Applications/Dragent.app`, `~/.dragent`) is
never touched or counted.

Memory metric per OS (best available, stdlib only):
- macOS: `phys_footprint` via `/usr/bin/footprint -p PID`, else `ps` RSS.
- Linux: `/proc/PID/smaps_rollup` Pss, else `/proc/PID/status` VmRSS.
- Windows: `GetProcessMemoryInfo` PrivateUsage via ctypes, else tasklist RSS.

Stdlib only: runs on macOS, Windows and Linux with no installs.
"""

import argparse
import csv
import ctypes
import os
import platform
import re
import subprocess
import sys
import time

ROLES = ("gui", "host", "pty", "index", "computer", "cli", "other-own", "agent")

HOST_ARG = "--dragent-host-service"
PTY_ARG = "--dragent-pty-service"
INDEX_ARG = "--dragent-index-service"
COMPUTER_ARG = "--dragent-computer-service"
CLI_ARG = "--dragent-cli"

_FOOTPRINT_OK = {"enabled": True}


def dev_home():
    override = os.environ.get("DRAGENT_DEV_HOME")
    if override:
        return os.path.abspath(os.path.expanduser(override))
    return os.path.abspath(os.path.join(os.path.expanduser("~"), ".dragent-dev"))


def release_home():
    return os.path.abspath(os.path.join(os.path.expanduser("~"), ".dragent"))


# ---------------------------------------------------------------- processes


def _basename_of(cmd):
    first = cmd.split()[0] if cmd.strip() else ""
    first = first.strip("\"'")
    return os.path.basename(first).lower()


def list_processes_posix():
    """[(pid, ppid, rss_bytes, cmd)] via ps."""
    try:
        p = subprocess.run(["ps", "-ax", "-o", "pid=,ppid=,rss=,command="],
                           capture_output=True, text=True, timeout=30)
    except Exception:
        return []
    if p.returncode != 0:
        return []
    out = []
    for line in p.stdout.splitlines():
        parts = line.split(None, 3)
        if len(parts) < 4:
            continue
        try:
            pid, ppid, rss_kb = int(parts[0]), int(parts[1]), int(parts[2])
        except ValueError:
            continue
        out.append({"pid": pid, "ppid": ppid,
                    "rss_bytes": rss_kb * 1024, "cmd": parts[3]})
    return out


def list_processes_windows():
    """[(pid, ppid, private_bytes|None, rss_bytes|None, cmd)] via ctypes.

    Falls back to tasklist RSS keyed by pid when the snapshot fails.
    """
    procs = []
    try:
        kernel32 = ctypes.windll.kernel32
        psapi = ctypes.windll.psapi
        TH32CS_SNAPPROCESS = 0x00000002

        class PROCESSENTRY32(ctypes.Structure):
            _fields_ = [("dwSize", ctypes.c_ulong),
                        ("cntUsage", ctypes.c_ulong),
                        ("th32ProcessID", ctypes.c_ulong),
                        ("th32DefaultHeapID", ctypes.c_ulong),
                        ("th32ModuleID", ctypes.c_ulong),
                        ("cntThreads", ctypes.c_ulong),
                        ("th32ParentProcessID", ctypes.c_ulong),
                        ("pcPriClassBase", ctypes.c_long),
                        ("dwFlags", ctypes.c_ulong),
                        ("szExeFile", ctypes.c_char * 260)]

        class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
            _fields_ = [("cb", ctypes.c_ulong),
                        ("PageFaultCount", ctypes.c_ulong),
                        ("PeakWorkingSetSize", ctypes.c_size_t),
                        ("WorkingSetSize", ctypes.c_size_t),
                        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                        ("PagefileUsage", ctypes.c_size_t),
                        ("PeakPagefileUsage", ctypes.c_size_t)]

        # Extended counters with PrivateUsage (PROCESS_MEMORY_COUNTERS_EX).
        class PROCESS_MEMORY_COUNTERS_EX(PROCESS_MEMORY_COUNTERS):
            _fields_ = [("PrivateUsage", ctypes.c_size_t)]

        INVALID = ctypes.c_void_p(-1).value
        snap = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
        if snap == INVALID:
            raise OSError("snapshot failed")
        try:
            entry = PROCESSENTRY32()
            entry.dwSize = ctypes.sizeof(PROCESSENTRY32)
            ok = kernel32.Process32First(snap, ctypes.byref(entry))
            while ok:
                pid = int(entry.th32ProcessID)
                ppid = int(entry.th32ParentProcessID)
                exe = entry.szExeFile.decode("utf-8", "replace")
                private = None
                # PROCESS_QUERY_INFORMATION (0x0400) | PROCESS_VM_READ (0x0010)
                handle = kernel32.OpenProcess(0x0400 | 0x0010, False, pid)
                path = exe
                if handle:
                    try:
                        counters = PROCESS_MEMORY_COUNTERS_EX()
                        counters.cb = ctypes.sizeof(counters)
                        if psapi.GetProcessMemoryInfo(handle,
                                                      ctypes.byref(counters),
                                                      counters.cb):
                            private = int(counters.PrivateUsage)
                        buf = ctypes.create_string_buffer(1024)
                        if psapi.GetModuleFileNameExA(handle, None, buf, 1024):
                            path = buf.value.decode("utf-8", "replace")
                    finally:
                        kernel32.CloseHandle(handle)
                procs.append({"pid": pid, "ppid": ppid,
                              "private_bytes": private,
                              "rss_bytes": None, "cmd": path,
                              "exe": exe})
                ok = kernel32.Process32Next(snap, ctypes.byref(entry))
        finally:
            kernel32.CloseHandle(snap)
    except Exception:
        procs = []
    if not procs:
        # Fallback: tasklist RSS (no ppid/cmdline; role-matched later by pid).
        try:
            p = subprocess.run(["tasklist", "/FO", "CSV", "/NH"], capture_output=True,
                               text=True, timeout=60)
            import csv as _csv
            for row in _csv.reader(p.stdout.splitlines()):
                if len(row) < 5:
                    continue
                try:
                    mem = int(row[4].replace(",", "").replace(" K", "").strip()) * 1024
                    procs.append({"pid": int(row[1]), "ppid": 0,
                                  "private_bytes": None, "rss_bytes": mem,
                                  "cmd": row[0], "exe": row[0]})
                except ValueError:
                    continue
        except Exception:
            pass
    return procs


def list_processes():
    if platform.system() == "Windows":
        return list_processes_windows()
    return list_processes_posix()


# ---------------------------------------------------------------- memory


def footprint_map_mac(pids):
    """{pid: phys_footprint bytes} for many PIDs with ONE footprint call.

    `footprint -f bytes PID…` prints one `… [PID]: … Footprint: N B` header
    per process; PIDs that need root or already exited are simply absent.
    """
    if not _FOOTPRINT_OK["enabled"] or not pids:
        return {}
    try:
        p = subprocess.run(["footprint", "-f", "bytes"]
                           + [str(i) for i in pids],
                           capture_output=True, text=True, timeout=120)
    except Exception:
        _FOOTPRINT_OK["enabled"] = False
        return {}
    if "Footprint:" not in p.stdout:
        # Binary missing or unusable; fall back to ps RSS from here on.
        if p.returncode != 0 and not p.stdout.strip():
            _FOOTPRINT_OK["enabled"] = False
        return {}
    out = {}
    for m in re.finditer(r"\[(\d+)\][^\n]*?Footprint:\s*([\d,]+)\s*B", p.stdout):
        try:
            out[int(m.group(1))] = int(m.group(2).replace(",", ""))
        except ValueError:
            continue
    return out


def footprint_bytes_mac(pid):
    """Single-PID phys_footprint; the sampler batches via footprint_map_mac."""
    return footprint_map_mac([pid]).get(pid)


def linux_pss_bytes(pid):
    """Pss from smaps_rollup, else VmRSS from status, else None."""
    try:
        with open("/proc/%d/smaps_rollup" % pid, encoding="utf-8") as f:
            for line in f:
                if line.startswith("Pss:"):
                    return int(line.split()[1]) * 1024
    except Exception:
        pass
    try:
        with open("/proc/%d/status" % pid, encoding="utf-8") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
    except Exception:
        pass
    return None


def process_bytes(proc, fp_map=None):
    """Best-available byte count for one process dict.

    `fp_map` (macOS) is the batch footprint lookup from `collect`; when the
    PID is absent, fall back to a single query, then to ps RSS.
    """
    system = platform.system()
    if system == "Darwin":
        if fp_map is not None:
            if proc["pid"] in fp_map:
                return fp_map[proc["pid"]]
        else:
            fp = footprint_bytes_mac(proc["pid"])
            if fp is not None:
                return fp
        return proc.get("rss_bytes")
    if system == "Linux":
        pss = linux_pss_bytes(proc["pid"])
        if pss is not None:
            return pss
        return proc.get("rss_bytes")
    # Windows
    if proc.get("private_bytes") is not None:
        return proc["private_bytes"]
    return proc.get("rss_bytes")


# ---------------------------------------------------------------- classify


def _argv0(cmd):
    """Executable path of a command line (prose in later args is ignored)."""
    cmd = (cmd or "").lstrip()
    if not cmd:
        return ""
    if cmd[0] in "\"'":
        end = cmd.find(cmd[0], 1)
        return cmd[1:end] if end > 0 else cmd[1:]
    return cmd.split()[0]


def _in_own_tree(proc, tree):
    if tree is None:
        return True
    return proc["pid"] in tree


def is_own(proc, home, tree=None):
    """True when the process belongs to the dev edition (never release).

    Only the executable path (argv0) and service flags count as evidence;
    prose inside other processes' arguments (e.g. another agent's `--prompt`
    quoting this task) must never match. When `tree` (a set of pids: roots
    the harness started plus their descendants) is given, the process must
    also be inside it, so other checkouts' dev builds are never counted.
    """
    cmd = proc.get("cmd", "") or ""
    exe = proc.get("exe", "") or ""
    argv0 = _argv0(cmd)
    low_exe = (argv0 + "\x00" + exe).lower()
    low_all = (cmd + "\x00" + exe).lower()
    # Any bundled release/beta app (e.g. /Applications/Dragent.app or a
    # mounted /Volumes/Dragent/Dragent.app) is never ours. A dev bundle would
    # carry "dev" in its name.
    if ".app/contents/" in low_exe:
        head = low_exe.split(".app/contents/")[0]
        bundle = head.rsplit("/", 1)[-1].rsplit("\\", 1)[-1]
        if "dev" not in bundle:
            return False
    # Installed release on Windows (outside a cargo target dir) is never ours.
    if "program files" in low_exe and "dragent" in low_exe:
        if "\\target\\" not in low_exe and "dev" not in low_exe:
            return False
    rel = release_home()
    if rel.lower() in low_exe and not (home and home.lower() in low_exe):
        return False
    owned = False
    if home and home.lower() in low_exe:
        owned = True
    elif ".dragent-dev" in low_exe or "dragent-dev" in low_exe:
        owned = True
    elif "/target/debug/" in low_exe or "/target/release/" in low_exe \
            or "\\target\\" in low_exe:
        owned = "dragent" in low_exe
    else:
        base = os.path.basename(argv0).lower()
        exe_base = os.path.basename(exe).lower() if exe else ""
        if base in ("dragent", "dragent.exe", "dragent-host", "dragent-host.exe"):
            # Bare name (PATH lookup): trust only with a dev marker or a
            # service/CLI flag in the arguments.
            if home and home.lower() in low_all:
                owned = True
            elif ".dragent-dev" in low_all or "dragent-dev://" in low_all:
                owned = True
            elif any(a in cmd for a in (HOST_ARG, PTY_ARG, INDEX_ARG,
                                        COMPUTER_ARG, CLI_ARG)):
                owned = True
        elif exe_base in ("dragent", "dragent.exe"):
            owned = True
    if not owned:
        return False
    return _in_own_tree(proc, tree)


def classify(proc):
    cmd = proc.get("cmd", "") or ""
    if HOST_ARG in cmd:
        return "host"
    if PTY_ARG in cmd:
        return "pty"
    if INDEX_ARG in cmd:
        return "index"
    if COMPUTER_ARG in cmd:
        return "computer"
    if CLI_ARG in cmd:
        return "cli"
    base = _basename_of(cmd)
    if "dragent" in base:
        return "gui"
    return "other-own"


def descendants(procs, roots):
    children = {}
    for p in procs:
        children.setdefault(p["ppid"], []).append(p["pid"])
    seen = set(r for r in roots if r)
    stack = list(seen)
    while stack:
        pid = stack.pop()
        for child in children.get(pid, []):
            if child not in seen:
                seen.add(child)
                stack.append(child)
    return seen


def collect(home=None, agent_pids=(), root_pids=()):
    """One snapshot: list of (timestamp, role, pid, bytes) row dicts."""
    home = home or dev_home()
    procs = list_processes()
    agent_set = descendants(procs, set(agent_pids))
    tree = descendants(procs, set(root_pids)) if root_pids else None
    # macOS: one batched footprint call per sample instead of one fork per
    # PID (12 agents x fork was stretching the 1 s interval to ~10 s).
    fp_map = None
    if platform.system() == "Darwin":
        want = [p["pid"] for p in procs
                if p["pid"] in agent_set or is_own(p, home, tree)]
        fp_map = footprint_map_mac(want)
    now = time.time()
    rows = []
    agent_total = 0
    for proc in procs:
        pid = proc["pid"]
        if pid in agent_set:
            b = process_bytes(proc, fp_map)
            if b is not None:
                agent_total += b
            continue
        if not is_own(proc, home, tree):
            continue
        role = classify(proc)
        b = process_bytes(proc, fp_map)
        if b is None:
            continue
        rows.append({"timestamp": now, "role": role, "pid": pid, "bytes": b})
    rows.append({"timestamp": now, "role": "agent", "pid": -1, "bytes": agent_total})
    return rows


# ---------------------------------------------------------------- main loop


def write_csv(path, rows):
    new = not os.path.exists(path)
    with open(path, "a", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=["timestamp", "role", "pid", "bytes"])
        if new:
            w.writeheader()
        for r in rows:
            w.writerow(r)


def main():
    ap = argparse.ArgumentParser(description="Sample Dragent dev-edition memory to CSV.")
    ap.add_argument("--duration", type=float, default=60, help="Seconds to sample.")
    ap.add_argument("--interval", type=float, default=1.0, help="Seconds between samples.")
    ap.add_argument("--output", default="mem.csv", help="CSV path (appended).")
    ap.add_argument("--home", default=None,
                    help="Dev home to scope to (default DRAGENT_DEV_HOME or ~/.dragent-dev).")
    ap.add_argument("--agent-pid", type=int, action="append", default=[],
                    help="PID of an agent process to count (summed); repeatable.")
    ap.add_argument("--root-pid", type=int, action="append", default=[],
                    help="PID of a dev-edition process the harness started; only it "
                         "and its descendants are counted as Dragent's own. "
                         "Repeatable. Strongly recommended on shared machines.")
    ap.add_argument("--once", action="store_true", help="Take one sample and exit.")
    args = ap.parse_args()

    home = args.home or dev_home()
    if args.once:
        rows = collect(home, args.agent_pid, args.root_pid)
        write_csv(args.output, rows)
        print("wrote %d rows to %s" % (len(rows), args.output))
        return

    interval = max(0.2, args.interval)
    deadline = time.time() + args.duration
    while time.time() < deadline:
        tick = time.monotonic()
        try:
            rows = collect(home, args.agent_pid, args.root_pid)
            write_csv(args.output, rows)
        except Exception as e:  # noqa: BLE001 - sampler must not die
            sys.stderr.write("memsample: %s\n" % e)
        wait = interval - (time.monotonic() - tick)
        if wait > 0:
            time.sleep(wait)
    print("sampling done: %s" % args.output)


if __name__ == "__main__":
    main()
