//! What the app needs to decide which agents may sleep: how short of memory
//! the machine is, how much memory each agent's processes hold, and whether
//! an agent left something serving (a dev server it started).
//!
//! An agent's processes are the `dragent --dragent-agent-run <pane>` wrapper
//! and everything below it. Read-only: nothing here stops a process.

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// How short of memory the machine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    Normal,
    /// Getting short: idle agents are put to sleep sooner.
    Elevated,
    /// Close to stalling: starting another agent asks first.
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub pressure: Pressure,
}

/// Pressure from the share of memory still available, where the system
/// doesn't report its own pressure level.
pub fn pressure_from_available(available: u64, total: u64) -> Pressure {
    if total == 0 {
        return Pressure::Normal;
    }
    let share = available as f64 / total as f64;
    if share < 0.08 {
        Pressure::Critical
    } else if share < 0.15 {
        Pressure::Elevated
    } else {
        Pressure::Normal
    }
}

/// macOS's own verdict (`kern.memorystatus_vm_pressure_level`): 1 normal,
/// 2 warning, 4 critical. It accounts for compression and swap, which a
/// free-memory share can't.
#[cfg(target_os = "macos")]
fn system_pressure() -> Option<Pressure> {
    let mut level: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let name = c"kern.memorystatus_vm_pressure_level";
    // SAFETY: the out-pointer and size describe one `c_int`.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut level as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    Some(match level {
        4 => Pressure::Critical,
        2 => Pressure::Elevated,
        _ => Pressure::Normal,
    })
}

#[cfg(not(target_os = "macos"))]
fn system_pressure() -> Option<Pressure> {
    None
}

/// The machine's memory right now.
pub fn memory() -> Memory {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let (total, available) = (system.total_memory(), system.available_memory());
    let from_share = pressure_from_available(available, total);
    // The stricter of the system's verdict and the share still available.
    let pressure = system_pressure().map_or(from_share, |level| level.max(from_share));
    Memory { total, available, pressure }
}

/// One agent's processes, as found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentProcesses {
    /// The wrapper process was found.
    pub found: bool,
    /// Resident memory of the wrapper and everything below it, in bytes.
    pub memory: u64,
    /// Something it started listens for TCP connections (a dev server).
    /// Ports the agent program itself opens for its own use don't count.
    /// `None`: couldn't tell (no way to list listeners here), which callers
    /// treat as "yes".
    pub listening: Option<bool>,
}

/// Find each pane's agent processes. One process scan for all panes.
pub fn agent_processes(panes: &[Uuid]) -> HashMap<Uuid, AgentProcesses> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_memory().with_cmd(UpdateKind::Always),
    );
    let mut children: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let listeners = listening_pids();
    let mut out = HashMap::new();
    for pane in panes {
        let wanted = pane.to_string();
        let root = system.processes().iter().find_map(|(pid, process)| {
            let cmd = process.cmd();
            let is_wrapper = cmd.iter().any(|arg| arg == crate::agent_sessions::RUN_ARG)
                && cmd.iter().any(|arg| arg.to_str() == Some(wanted.as_str()));
            is_wrapper.then_some(*pid)
        });
        let Some(root) = root else {
            out.insert(*pane, AgentProcesses::default());
            continue;
        };
        // The agent program: the word after `--` in the wrapper's command.
        let agent_name = system.process(root).and_then(|process| {
            let cmd = process.cmd();
            let at = cmd.iter().position(|arg| arg == "--")?;
            let program = cmd.get(at + 1)?.to_str()?;
            Some(
                std::path::Path::new(program)
                    .file_name()?
                    .to_string_lossy()
                    .into_owned(),
            )
        });
        let mut tree = Vec::new();
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            tree.push(pid);
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids.iter().copied());
            }
        }
        let memory = tree
            .iter()
            .filter_map(|pid| system.process(*pid))
            .map(|process| process.memory())
            .sum();
        // Codex and Claude Code open a loopback port in their own process; a
        // dev server is another program the agent started.
        let own = |pid: &sysinfo::Pid| {
            *pid == root
                || system.process(*pid).is_some_and(|process| {
                    process.parent() == Some(root)
                        || agent_name.as_deref().is_some_and(|name| {
                            process.name().to_string_lossy() == name
                        })
                })
        };
        let listening = listeners.as_ref().map(|set| {
            tree.iter()
                .filter(|pid| !own(pid))
                .any(|pid| set.contains(&pid.as_u32()))
        });
        out.insert(*pane, AgentProcesses { found: true, memory, listening });
    }
    out
}

/// Every process listening for TCP connections, or `None` when the system
/// offers no way to tell.
fn listening_pids() -> Option<HashSet<u32>> {
    #[cfg(unix)]
    {
        // `-F p` prints one `p<pid>` line per process; exit 1 with no output
        // means nothing listens at all.
        let output = std::process::Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fp"])
            .output()
            .ok()?;
        if !output.status.success() && !output.stdout.is_empty() {
            return None;
        }
        Some(parse_lsof_pids(&String::from_utf8_lossy(&output.stdout)))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(parse_netstat_pids(&String::from_utf8_lossy(&output.stdout)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

fn parse_lsof_pids(text: &str) -> HashSet<u32> {
    text.lines()
        .filter_map(|line| line.strip_prefix('p'))
        .filter_map(|pid| pid.trim().parse().ok())
        .collect()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn parse_netstat_pids(text: &str) -> HashSet<u32> {
    text.lines()
        .filter(|line| line.contains("LISTENING"))
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(|pid| pid.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_follows_the_share_of_memory_left() {
        let gb = 1u64 << 30;
        assert_eq!(pressure_from_available(8 * gb, 16 * gb), Pressure::Normal);
        assert_eq!(pressure_from_available(2 * gb, 16 * gb), Pressure::Elevated);
        assert_eq!(pressure_from_available(gb, 16 * gb), Pressure::Critical);
        assert_eq!(pressure_from_available(0, 0), Pressure::Normal);
        assert!(Pressure::Critical > Pressure::Elevated && Pressure::Elevated > Pressure::Normal);
    }

    #[test]
    fn listeners_are_read_from_the_system_tools() {
        assert_eq!(parse_lsof_pids("p123\np456\n"), HashSet::from([123, 456]));
        assert!(parse_lsof_pids("").is_empty());
        let netstat = "  Proto  Local Address   Foreign Address  State       PID\n  \
            TCP    0.0.0.0:3000    0.0.0.0:0        LISTENING   4321\n  \
            TCP    10.0.0.2:5000   1.2.3.4:443      ESTABLISHED 99\n";
        assert_eq!(parse_netstat_pids(netstat), HashSet::from([4321]));
    }

    #[test]
    fn a_pane_without_an_agent_process_is_reported_as_not_found() {
        let pane = Uuid::new_v4();
        let found = agent_processes(&[pane]);
        assert_eq!(found[&pane], AgentProcesses::default());
        let now = memory();
        assert!(now.total > 0 && now.available <= now.total);
    }
}
