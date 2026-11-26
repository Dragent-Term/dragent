//! How much memory each agent holds, and pausing one that runs away.
//!
//! An agent is its launch monitor (`dragent --dragent-agent-run <pane>`) and
//! everything below it. Its memory is what that tree holds, whether resident,
//! compressed or swapped out (on Windows, resident only).
//!
//! Pausing stops everything *below* the monitor. The monitor itself keeps
//! running: it is the job the terminal's shell waits for, and a shell whose
//! job stops takes its terminal back, after which a resumed agent could no
//! longer read from it. Parents stop before their children and children
//! resume before their parents, so a shell inside the tree never sees its
//! own job stopped either.
//!
//! What is paused is written down before anything stops, with the processes'
//! start times, so a later host can always resume it and never signals a
//! process that merely got a dead one's id.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use uuid::Uuid;

use crate::agent_sessions::AgentSession;

/// How often agents are measured.
pub const WATCH_EVERY: Duration = Duration::from_secs(10);
pub const PAUSED_FILENAME: &str = "paused-agents.json";
/// Processes started while a tree is being stopped are stopped in turn, this
/// many times over at most.
const STOP_ROUNDS: usize = 8;

/// Every process, with its memory and command line.
pub fn processes() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_cmd(UpdateKind::Always),
    );
    system
}

fn process_tree() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    system
}

/// The recorded agent sessions that are running.
pub fn active_sessions(sessions_dir: &Path) -> Vec<AgentSession> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let bytes = std::fs::read(entry.path().join("session.json")).ok()?;
            let record: AgentSession = serde_json::from_slice(&bytes).ok()?;
            let own = entry.file_name().to_str() == Some(record.pane_uuid.to_string().as_str());
            (record.active && own).then_some(record)
        })
        .collect()
}

/// The pane a launch monitor serves, from its command line. The program path
/// may be missing from it, and macOS can glue the environment onto a later
/// word, so the pane is the start of the word after the marker.
pub fn wrapper_pane<S: AsRef<std::ffi::OsStr>>(cmd: &[S]) -> Option<Uuid> {
    let marker = std::ffi::OsStr::new(crate::agent_sessions::RUN_ARG);
    let at = cmd.iter().take(2).position(|arg| arg.as_ref() == marker)?;
    let word = cmd.get(at + 1)?.as_ref().to_str()?;
    word.get(..36)?.parse().ok()
}

/// Every pane's launch monitor.
pub fn wrappers(system: &System) -> HashMap<Uuid, Pid> {
    let mut found: HashMap<Uuid, (u64, Pid)> = HashMap::new();
    for (pid, process) in system.processes() {
        if process.thread_kind().is_some() {
            continue;
        }
        if let Some(pane) = wrapper_pane(process.cmd()) {
            let entry = found.entry(pane).or_insert((process.start_time(), *pid));
            if process.start_time() < entry.0 {
                *entry = (process.start_time(), *pid);
            }
        }
    }
    found
        .into_iter()
        .map(|(pane, (_, pid))| (pane, pid))
        .collect()
}

/// `root` and everything below it, parents before their children.
pub fn tree(system: &System, root: Pid) -> Vec<Pid> {
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, process) in system.processes() {
        if process.thread_kind().is_some() {
            continue;
        }
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let mut tree = vec![root];
    let mut seen = HashSet::from([root]);
    let mut at = 0;
    while at < tree.len() {
        for child in children.get(&tree[at]).into_iter().flatten() {
            if seen.insert(*child) {
                tree.push(*child);
            }
        }
        at += 1;
    }
    tree
}

pub fn tree_bytes(system: &System, root: Pid) -> u64 {
    tree(system, root)
        .iter()
        .filter_map(|pid| {
            let resident = system.process(*pid)?.memory();
            Some(footprint(pid.as_u32(), resident))
        })
        .sum()
}

/// What a process holds, including what the system compressed or swapped out
/// (a runaway's memory is the first to go there). `resident` where that
/// can't be read.
fn footprint(pid: u32, resident: u64) -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer is the version asked for.
        let status = unsafe {
            libc::proc_pid_rusage(
                pid as libc::c_int,
                libc::RUSAGE_INFO_V4,
                (&mut usage as *mut libc::rusage_info_v4).cast(),
            )
        };
        if status == 0 { usage.ri_phys_footprint } else { resident }
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        resident + swapped(&status)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        resident
    }
}

/// Bytes swapped out, from `/proc/<pid>/status`.
#[cfg(any(target_os = "linux", test))]
fn swapped(status: &str) -> u64 {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmSwap:"))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024)
}

/// One agent's memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMemory {
    pub pane: Uuid,
    pub terminal_id: String,
    pub label: String,
    pub root: Proc,
    pub bytes: u64,
}

/// Every running agent's memory, from one look at the processes.
pub fn measure(sessions_dir: &Path) -> Vec<AgentMemory> {
    let sessions = active_sessions(sessions_dir);
    if sessions.is_empty() {
        return Vec::new();
    }
    let system = processes();
    let wrappers = wrappers(&system);
    sessions
        .into_iter()
        .filter_map(|record| {
            let root = *wrappers.get(&record.pane_uuid)?;
            Some(AgentMemory {
                pane: record.pane_uuid,
                terminal_id: record.terminal_id,
                label: record.provider,
                root: Proc::of(&system, root)?,
                bytes: tree_bytes(&system, root),
            })
        })
        .collect()
}

/// A process told apart from a later one with the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Proc {
    pub pid: u32,
    /// When it started, in Unix seconds.
    pub started: u64,
}

impl Proc {
    fn of(system: &System, pid: Pid) -> Option<Self> {
        system.process(pid).map(|process| Self {
            pid: pid.as_u32(),
            started: process.start_time(),
        })
    }

    /// The process this runs in.
    pub fn this() -> Option<Self> {
        Self::of_pid(std::process::id())
    }

    /// The process that has the id `pid` now.
    pub fn of_pid(pid: u32) -> Option<Self> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        Self::of(&system, pid)
    }

    pub fn is_running(&self) -> bool {
        Self::of_pid(self.pid).is_some_and(|now| now.started.abs_diff(self.started) <= 1)
    }

    /// Whether it is still that process. Linux derives start times from the
    /// boot time, which a clock correction moves by a second.
    fn is_in(&self, system: &System) -> bool {
        Self::of(system, Pid::from_u32(self.pid))
            .is_some_and(|now| now.started.abs_diff(self.started) <= 1)
    }
}

/// An agent that was paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedAgent {
    pub pane: Uuid,
    pub terminal_id: String,
    pub label: String,
    /// What it held when it was paused.
    pub bytes: u64,
    pub root: Proc,
    /// What was stopped, parents first.
    #[serde(default)]
    pub stopped: Vec<Proc>,
}

/// What the person let a resumed agent have.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Allowance {
    root: Proc,
    bytes: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    paused: Vec<PausedAgent>,
    #[serde(default)]
    allowed: HashMap<Uuid, Allowance>,
}

/// The agents that are paused, kept in `<org>/paused-agents.json`.
pub struct PausedAgents {
    org_dir: PathBuf,
    state: Mutex<State>,
}

impl PausedAgents {
    pub fn load(org_dir: PathBuf) -> Self {
        let state = std::fs::read(org_dir.join(PAUSED_FILENAME))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        Self {
            org_dir,
            state: Mutex::new(state),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn save(&self, state: &State) -> Result<(), String> {
        let path = self.org_dir.join(PAUSED_FILENAME);
        if state.paused.is_empty() && state.allowed.is_empty() {
            return match std::fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(error.to_string())
                }
                _ => Ok(()),
            };
        }
        let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        crate::agent_sessions::write_atomic(&self.org_dir, PAUSED_FILENAME, &bytes)
    }

    pub fn list(&self) -> Vec<PausedAgent> {
        self.lock().paused.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().paused.is_empty()
    }

    /// What the agent paused in `terminal_id` held, if one is.
    pub fn held_by(&self, terminal_id: &str) -> Option<u64> {
        self.lock()
            .paused
            .iter()
            .find(|p| p.terminal_id == terminal_id)
            .map(|p| p.bytes)
    }

    pub fn is_paused(&self, terminal_id: &str) -> bool {
        self.lock()
            .paused
            .iter()
            .any(|p| p.terminal_id == terminal_id)
    }

    /// What the person allowed this agent by resuming it.
    pub fn allowed(&self, agent: &AgentMemory) -> Option<u64> {
        self.lock()
            .allowed
            .get(&agent.pane)
            .filter(|allowance| allowance.root == agent.root)
            .map(|allowance| allowance.bytes)
    }

    /// Pause an agent. Nothing is stopped unless it could be written down.
    pub fn pause(&self, agent: &AgentMemory) -> Result<(), String> {
        let mut state = self.lock();
        if state.paused.iter().any(|p| p.pane == agent.pane) {
            return Ok(());
        }
        state.paused.push(PausedAgent {
            pane: agent.pane,
            terminal_id: agent.terminal_id.clone(),
            label: agent.label.clone(),
            bytes: agent.bytes,
            root: agent.root,
            stopped: Vec::new(),
        });
        if let Err(error) = self.save(&state) {
            state.paused.pop();
            return Err(error);
        }
        let stopped = stop_below(agent.root);
        if stopped.is_empty() {
            state.paused.pop();
            let _ = self.save(&state);
            return Err("nothing of it could be paused".into());
        }
        if let Some(entry) = state.paused.last_mut() {
            entry.stopped = stopped;
        }
        // Already safe: a resume looks at the whole tree too.
        let _ = self.save(&state);
        Ok(())
    }

    /// Resume the paused agent of `id` (a pane or terminal). It may then
    /// grow half as much again before it is paused once more.
    pub fn resume(&self, id: &str) -> Result<PausedAgent, String> {
        let mut state = self.lock();
        let at = state
            .paused
            .iter()
            .position(|p| p.terminal_id == id || p.pane.to_string() == id)
            .ok_or("That agent isn't paused.")?;
        let agent = state.paused[at].clone();
        // Forgotten only after it runs again: an interrupted resume can be
        // repeated, and continuing a running process does nothing.
        let alive = thaw(&agent);
        state.paused.remove(at);
        if alive {
            state.allowed.insert(
                agent.pane,
                Allowance {
                    root: agent.root,
                    bytes: agent.bytes.saturating_add(agent.bytes / 2),
                },
            );
        }
        let _ = self.save(&state);
        if alive {
            Ok(agent)
        } else {
            Err("That agent has already exited.".into())
        }
    }

    /// Forget agents that ended, setting free anything of theirs that is
    /// still stopped.
    pub fn forget_ended(&self) {
        let mut state = self.lock();
        if state.paused.is_empty() && state.allowed.is_empty() {
            return;
        }
        let system = process_tree();
        let before = (state.paused.len(), state.allowed.len());
        state.paused.retain(|agent| {
            let alive = agent.root.is_in(&system);
            if !alive {
                thaw(agent);
            }
            alive
        });
        state.allowed.retain(|_, a| a.root.is_in(&system));
        if (state.paused.len(), state.allowed.len()) != before {
            let _ = self.save(&state);
        }
    }
}

/// Stop everything below `root`, itself left running. What was stopped,
/// parents first.
fn stop_below(root: Proc) -> Vec<Proc> {
    let mut stopped: Vec<Proc> = Vec::new();
    for _ in 0..STOP_ROUNDS {
        let system = process_tree();
        if !root.is_in(&system) {
            break;
        }
        let fresh: Vec<Proc> = tree(&system, Pid::from_u32(root.pid))
            .into_iter()
            .skip(1)
            .filter_map(|pid| Proc::of(&system, pid))
            .filter(|process| !stopped.contains(process))
            .collect();
        if fresh.is_empty() {
            break;
        }
        let before = stopped.len();
        stopped.extend(fresh.into_iter().filter(|p| platform::stop(p.pid)));
        if stopped.len() == before {
            break;
        }
    }
    stopped
}

/// Let a paused agent run again, children before their parents: what was
/// recorded as stopped, and anything below its monitor. Whether the agent is
/// still there.
fn thaw(agent: &PausedAgent) -> bool {
    let system = process_tree();
    let alive = agent.root.is_in(&system);
    let mut order: Vec<u32> = agent
        .stopped
        .iter()
        .filter(|process| process.is_in(&system))
        .map(|process| process.pid)
        .collect();
    // Unix only: on Windows a process that wasn't suspended would be woken
    // out of a suspension of its own.
    if cfg!(unix) && alive {
        for pid in tree(&system, Pid::from_u32(agent.root.pid))
            .into_iter()
            .skip(1)
        {
            if !order.contains(&pid.as_u32()) {
                order.push(pid.as_u32());
            }
        }
    }
    for pid in order.iter().rev() {
        platform::cont(*pid);
    }
    alive
}

#[cfg(unix)]
mod platform {
    pub fn stop(pid: u32) -> bool {
        // SAFETY: plain signals to a process found in the agent's tree.
        pid > 1 && unsafe { libc::kill(pid as i32, libc::SIGSTOP) } == 0
    }

    pub fn cont(pid: u32) {
        if pid > 1 {
            // SAFETY: as above.
            unsafe { libc::kill(pid as i32, libc::SIGCONT) };
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSuspendProcess(process: HANDLE) -> i32;
        fn NtResumeProcess(process: HANDLE) -> i32;
    }

    fn with(pid: u32, act: unsafe extern "system" fn(HANDLE) -> i32) -> bool {
        // SAFETY: the handle is checked, used and closed here.
        unsafe {
            let handle = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
            if handle.is_null() {
                return false;
            }
            let status = act(handle);
            CloseHandle(handle);
            status >= 0
        }
    }

    pub fn stop(pid: u32) -> bool {
        with(pid, NtSuspendProcess)
    }

    pub fn cont(pid: u32) {
        with(pid, NtResumeProcess);
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    pub fn stop(_pid: u32) -> bool {
        false
    }

    pub fn cont(_pid: u32) {}
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A session record for `pane` under `sessions_dir`.
    pub(crate) fn write_session(sessions_dir: &Path, pane: Uuid, active: bool) {
        let record = serde_json::json!({
            "organisation_id": "", "pane_uuid": pane, "generation": Uuid::new_v4(),
            "provider": "claude", "cwd": "/", "executable": "claude",
            "resume_options": null, "conversation_id": null, "active": active,
            "workspace_id": "ws", "terminal_id": format!("term-{pane}"),
        });
        let dir = sessions_dir.join(pane.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), record.to_string()).unwrap();
    }

    #[test]
    fn a_monitor_is_known_by_its_marker_and_pane_only() {
        let pane = Uuid::new_v4();
        let id = pane.to_string();
        let run = crate::agent_sessions::RUN_ARG;
        assert_eq!(
            wrapper_pane(&["dragent", run, &id, "--", "claude"]),
            Some(pane)
        );
        assert_eq!(wrapper_pane(&[run, &id, "--", "claude"]), Some(pane));
        assert_eq!(
            wrapper_pane(&["dragent", run, &format!("{id}\u{1}TERM=x")]),
            Some(pane)
        );
        // A shell running that line, or an agent whose prompt mentions it.
        assert_eq!(
            wrapper_pane(&["zsh", "-c", &format!("dragent {run} {id}")]),
            None
        );
        assert_eq!(wrapper_pane(&["claude", "-p", run, &id]), None);
        assert_eq!(wrapper_pane(&["dragent", run]), None);
    }

    #[test]
    fn swapped_memory_is_read_from_a_process_status() {
        let status = "VmRSS:\t 10 kB\nVmSwap:\t    2048 kB\nThreads: 1\n";
        assert_eq!(swapped(status), 2 << 20);
        assert_eq!(swapped("VmRSS: 10 kB"), 0);
    }

    #[cfg(unix)]
    struct Kill(std::process::Child);
    #[cfg(unix)]
    impl Drop for Kill {
        fn drop(&mut self) {
            let root = Pid::from_u32(self.0.id());
            for pid in tree(&process_tree(), root).into_iter().rev() {
                unsafe { libc::kill(pid.as_u32() as i32, libc::SIGKILL) };
            }
            let _ = self.0.wait();
        }
    }

    /// `sh` standing in for a launch monitor, with a child and a grandchild.
    #[cfg(unix)]
    fn agent() -> (Kill, AgentMemory, Vec<u32>) {
        let child = std::process::Command::new("sh")
            .args(["-c", "sh -c 'sleep 600 & wait' & wait"])
            .spawn()
            .unwrap();
        let pid = Pid::from_u32(child.id());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let system = processes();
            let below: Vec<u32> = tree(&system, pid)
                .into_iter()
                .skip(1)
                .map(|p| p.as_u32())
                .collect();
            if below.len() == 2 {
                let memory = AgentMemory {
                    pane: Uuid::new_v4(),
                    terminal_id: "term-1".into(),
                    label: "claude".into(),
                    root: Proc::of(&system, pid).unwrap(),
                    bytes: tree_bytes(&system, pid),
                };
                assert!(memory.bytes > 0);
                return (Kill(child), memory, below);
            }
            assert!(std::time::Instant::now() < deadline, "tree never came up");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn stopped(pid: u32) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().starts_with('T')
    }

    #[cfg(unix)]
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-paused-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn pausing_stops_everything_below_the_monitor_and_resuming_frees_it() {
        let org = scratch();
        let (_kill, memory, below) = agent();
        let paused = PausedAgents::load(org.clone());
        paused.pause(&memory).unwrap();
        assert!(
            below.iter().all(|pid| stopped(*pid)),
            "all below it stopped"
        );
        assert!(!stopped(memory.root.pid), "its monitor keeps running");
        assert!(paused.is_paused("term-1"));
        paused.resume("term-1").unwrap();
        assert!(below.iter().all(|pid| !stopped(*pid)));
        assert!(!paused.is_paused("term-1"));
        assert_eq!(
            paused.allowed(&memory),
            Some(memory.bytes + memory.bytes / 2)
        );
        assert!(paused.resume("term-1").is_err(), "not paused any more");
        let _ = std::fs::remove_dir_all(&org);
    }

    #[cfg(unix)]
    #[test]
    fn a_paused_agent_is_resumed_by_a_later_host() {
        let org = scratch();
        let (_kill, memory, below) = agent();
        PausedAgents::load(org.clone()).pause(&memory).unwrap();
        // The host that paused it is gone; this one read what it left.
        let next = PausedAgents::load(org.clone());
        next.forget_ended();
        assert_eq!(next.list().len(), 1, "still there to resume");
        next.resume(&memory.pane.to_string()).unwrap();
        assert!(below.iter().all(|pid| !stopped(*pid)));
        let _ = std::fs::remove_dir_all(&org);
    }

    #[cfg(unix)]
    #[test]
    fn a_host_killed_halfway_through_pausing_leaves_it_resumable() {
        let org = scratch();
        let (_kill, memory, below) = agent();
        // Written down and stopped, but the list of what was stopped wasn't.
        let mut state = State::default();
        state.paused.push(PausedAgent {
            pane: memory.pane,
            terminal_id: memory.terminal_id.clone(),
            label: memory.label.clone(),
            bytes: memory.bytes,
            root: memory.root,
            stopped: Vec::new(),
        });
        PausedAgents::load(org.clone()).save(&state).unwrap();
        assert!(!stop_below(memory.root).is_empty());
        PausedAgents::load(org.clone()).resume("term-1").unwrap();
        assert!(below.iter().all(|pid| !stopped(*pid)));
        let _ = std::fs::remove_dir_all(&org);
    }

    #[cfg(unix)]
    #[test]
    fn a_process_with_a_reused_id_is_never_signalled() {
        let org = scratch();
        let (_kill, memory, below) = agent();
        assert!(!stop_below(memory.root).is_empty());
        // The record names the same ids, started long ago.
        let old = |pid: u32| Proc {
            pid,
            started: 1_000,
        };
        let mut state = State::default();
        state.paused.push(PausedAgent {
            pane: memory.pane,
            terminal_id: memory.terminal_id.clone(),
            label: memory.label.clone(),
            bytes: 1,
            root: old(memory.root.pid),
            stopped: below.iter().map(|pid| old(*pid)).collect(),
        });
        let paused = PausedAgents::load(org.clone());
        paused.save(&state).unwrap();
        let paused = PausedAgents::load(org.clone());
        assert!(paused.resume("term-1").is_err());
        assert!(below.iter().all(|pid| stopped(*pid)), "left alone");
        assert!(!org.join(PAUSED_FILENAME).exists(), "and forgotten");
        let _ = std::fs::remove_dir_all(&org);
    }

    #[cfg(unix)]
    #[test]
    fn what_a_dead_agent_left_stopped_is_set_free() {
        let org = scratch();
        let (mut kill, memory, below) = agent();
        let paused = PausedAgents::load(org.clone());
        paused.pause(&memory).unwrap();
        kill.0.kill().unwrap();
        kill.0.wait().unwrap();
        paused.forget_ended();
        assert!(paused.is_empty());
        std::thread::sleep(Duration::from_millis(100));
        assert!(below.iter().all(|pid| !stopped(*pid)));
        for pid in below {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(&org);
    }
}
