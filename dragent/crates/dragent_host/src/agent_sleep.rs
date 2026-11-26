//! What the app needs to decide which agents may sleep: how short of memory
//! the machine is, how much memory each agent's processes hold, whether an
//! agent left something serving (a dev server it started), and whether
//! anything it started in the background still runs (a build, a test run).
//!
//! An agent's processes are the `dragent --dragent-agent-run <pane>` wrapper
//! and everything below it. Read-only: nothing here stops a process.

use std::collections::{HashMap, HashSet};
use std::path::Path;
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

/// Total and available memory; 0 for what the system wouldn't say.
fn read_totals() -> (u64, u64) {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    (system.total_memory(), system.available_memory())
}

/// The pressure verdict from totals plus the system's own level: the
/// stricter of the two. No total, or nothing at all available, is a failed
/// reading (a running machine always has some), which leaves the system's
/// own level, or `None`.
fn pressure_of(total: u64, available: u64, system: Option<Pressure>) -> Option<Pressure> {
    let share =
        (total > 0 && available > 0).then(|| pressure_from_available(available, total));
    share.max(system)
}

/// The machine's memory right now. Pressure that can't be read counts as
/// Normal.
pub fn memory() -> Memory {
    let (total, available) = read_totals();
    let pressure = test_pressure_override()
        .or_else(|| pressure_of(total, available, system_pressure()))
        .unwrap_or(Pressure::Normal);
    Memory { total, available, pressure }
}

/// The pressure reading for the agent-start gate, or `None` when it can't
/// be told, which never holds a start.
pub fn gate_pressure() -> Option<Pressure> {
    if let Some(forced) = test_pressure_override() {
        return Some(forced);
    }
    let (total, available) = read_totals();
    pressure_of(total, available, system_pressure())
}

/// Forces the pressure in tests and dev runs, so waiting can be exercised
/// without exhausting memory: the variable holds `critical`, `elevated` or
/// `normal`, or the path of a file that does. Release builds ignore it.
pub const TEST_PRESSURE_ENV: &str = "DRAGENT_TEST_MEMORY_PRESSURE";

fn test_pressure_override() -> Option<Pressure> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let value = std::env::var(TEST_PRESSURE_ENV).ok()?;
    if Path::new(&value).is_absolute() {
        return parse_pressure(&std::fs::read_to_string(value).ok()?);
    }
    parse_pressure(&value)
}

fn parse_pressure(text: &str) -> Option<Pressure> {
    match text.trim().to_ascii_lowercase().as_str() {
        "critical" => Some(Pressure::Critical),
        "elevated" => Some(Pressure::Elevated),
        "normal" => Some(Pressure::Normal),
        _ => None,
    }
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
    /// Something it started still runs (a build, tests, a script), listening
    /// or not. Stopping the agent would interrupt it.
    pub background: bool,
}

/// Tools an agent starts for itself, at launch, within this many seconds.
const LAUNCH_HELPERS_SECS: u64 = 30;

/// A process below an agent, as [`runs_in_background`] sees it.
struct Below<'a> {
    pid: u32,
    parent: Option<u32>,
    name: &'a str,
    command: &'a str,
    /// Seconds after the agent program started.
    after: u64,
    /// The agent program itself.
    own: bool,
}

fn is_shell(name: &str) -> bool {
    let name = name.trim_start_matches('-').to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh" | "pwsh" | "powershell" | "cmd")
}

/// A tool the agent keeps for its own use whenever it starts: MCP servers,
/// language servers, and the one that keeps the machine awake.
fn is_helper(name: &str, command: &str) -> bool {
    let text = format!("{name} {command}").to_ascii_lowercase();
    ["mcp", "language-server", "languageserver", "lsp", "rust-analyzer", "gopls", "tsserver", "pyright", "clangd", "caffeinate"]
        .iter()
        .any(|word| text.contains(word))
}

/// Whether anything below an agent is work it started rather than its own
/// machinery. When it can't be told apart it counts as work: an agent kept
/// awake costs memory, one stopped mid-build costs the build.
///
/// `below` lists parents before their children. A shell the agent program
/// runs is a command (tools run in shells), and so is all it started.
/// Anything else is the agent's own when it is a known kind of helper, or
/// started along with the agent; what a helper starts is part of it.
fn runs_in_background(below: &[Below]) -> bool {
    let mut own: HashSet<u32> = HashSet::new();
    let mut helpers: HashSet<u32> = HashSet::new();
    for process in below {
        let parent = process.parent.unwrap_or(0);
        if process.own {
            own.insert(process.pid);
        } else if helpers.contains(&parent) {
            helpers.insert(process.pid);
        } else if own.contains(&parent)
            && (is_helper(process.name, process.command)
                || (!is_shell(process.name) && process.after <= LAUNCH_HELPERS_SECS))
        {
            helpers.insert(process.pid);
        } else {
            return true;
        }
    }
    false
}

/// How many agents run on this machine in the current organisation.
pub fn agents_awake() -> usize {
    let system = crate::agent_memory::processes();
    crate::agent_memory::wrappers(&system)
        .into_keys()
        .filter(|pane| crate::agent_sessions::load(*pane).is_some_and(|record| record.active))
        .count()
}

/// Find each pane's agent processes. One process scan for all panes.
pub fn agent_processes(panes: &[Uuid]) -> HashMap<Uuid, AgentProcesses> {
    let system = crate::agent_memory::processes();
    let wrappers = crate::agent_memory::wrappers(&system);
    let listeners = listening_pids();
    let mut out = HashMap::new();
    for pane in panes {
        let Some(root) = wrappers.get(pane).copied() else {
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
        let tree = crate::agent_memory::tree(&system, root);
        let memory = crate::agent_memory::tree_bytes(&system, root);
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
        let alive = |pid: &sysinfo::Pid| {
            system.process(*pid).is_some_and(|process| {
                !matches!(process.status(), sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead)
            })
        };
        let started = tree
            .iter()
            .filter(|pid| **pid != root && own(pid))
            .filter_map(|pid| Some(system.process(*pid)?.start_time()))
            .min()
            .or_else(|| Some(system.process(root)?.start_time()))
            .unwrap_or(0);
        let commands: Vec<(sysinfo::Pid, String, String)> = tree
            .iter()
            .filter(|pid| alive(pid))
            .filter_map(|pid| {
                let process = system.process(*pid)?;
                let command: Vec<_> = process.cmd().iter().map(|arg| arg.to_string_lossy()).collect();
                Some((*pid, process.name().to_string_lossy().into_owned(), command.join(" ")))
            })
            .collect();
        let below: Vec<Below> = commands
            .iter()
            .filter_map(|(pid, name, command)| {
                let process = system.process(*pid)?;
                Some(Below {
                    pid: pid.as_u32(),
                    parent: process.parent().map(|parent| parent.as_u32()),
                    name,
                    command,
                    after: process.start_time().saturating_sub(started),
                    own: own(pid),
                })
            })
            .collect();
        let background = runs_in_background(&below) || left_behind(&system, root, &tree);
        out.insert(*pane, AgentProcesses { found: true, memory, listening, background });
    }
    out
}

/// Whether something in the agent's terminal outlived the command that
/// started it (`npm run build &`): no longer below the agent, but stopped
/// along with its terminal.
#[cfg(unix)]
fn left_behind(system: &sysinfo::System, root: sysinfo::Pid, tree: &[sysinfo::Pid]) -> bool {
    let session = |pid: sysinfo::Pid| system.process(pid).and_then(|process| process.session_id());
    let Some(own) = session(root) else {
        return false;
    };
    // The shell the agent was started from, and what is above it.
    let mut above = HashSet::new();
    let mut at = root;
    while let Some(parent) = system.process(at).and_then(|process| process.parent()) {
        if !above.insert(parent) {
            break;
        }
        at = parent;
    }
    system.processes().iter().any(|(pid, process)| {
        process.thread_kind().is_none()
            && process.parent().is_none_or(|parent| parent.as_u32() <= 1)
            && !tree.contains(pid)
            && !above.contains(pid)
            && process.session_id() == Some(own)
            && !is_helper(&process.name().to_string_lossy(), "")
    })
}

#[cfg(not(unix))]
fn left_behind(_: &sysinfo::System, _: sysinfo::Pid, _: &[sysinfo::Pid]) -> bool {
    false
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
    fn pressure_combines_the_system_level_with_the_share() {
        let gb = 1u64 << 30;
        // Neither reads: no verdict.
        assert_eq!(pressure_of(0, 0, None), None);
        assert_eq!(pressure_of(16 * gb, 0, None), None, "a failed reading isn't full memory");
        assert_eq!(pressure_of(0, 0, Some(Pressure::Elevated)), Some(Pressure::Elevated));
        // Otherwise the stricter of the two.
        assert_eq!(pressure_of(16 * gb, 8 * gb, None), Some(Pressure::Normal));
        assert_eq!(pressure_of(16 * gb, 8 * gb, Some(Pressure::Critical)), Some(Pressure::Critical));
        assert_eq!(pressure_of(16 * gb, gb, Some(Pressure::Normal)), Some(Pressure::Critical));
        assert_eq!(pressure_of(16 * gb, 2 * gb, Some(Pressure::Normal)), Some(Pressure::Elevated));
    }

    #[test]
    fn only_known_words_force_the_pressure() {
        assert_eq!(parse_pressure(" CRITICAL\n"), Some(Pressure::Critical));
        assert_eq!(parse_pressure("elevated"), Some(Pressure::Elevated));
        assert_eq!(parse_pressure("normal"), Some(Pressure::Normal));
        assert_eq!(parse_pressure("1"), None);
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

    fn below(pid: u32, parent: u32, name: &'static str, command: &'static str, after: u64) -> Below<'static> {
        Below { pid, parent: Some(parent), name, command, after, own: pid <= 2 }
    }

    #[test]
    fn a_command_still_running_counts_as_background_work() {
        let agent = || vec![below(1, 0, "dragent", "", 0), below(2, 1, "claude", "claude", 0)];
        let with = |more: Vec<Below<'static>>| {
            let mut all = agent();
            all.extend(more);
            runs_in_background(&all)
        };
        assert!(!with(vec![]), "an agent on its own");
        assert!(with(vec![below(3, 2, "zsh", "zsh -c cargo test", 900), below(4, 3, "cargo", "cargo test", 900)]));
        assert!(with(vec![below(3, 2, "zsh", "zsh -c sleep 600", 5)]), "even one started at once");
        assert!(with(vec![below(3, 2, "python3", "python3 train.py", 900)]), "or without a shell");
        assert!(with(vec![below(3, 2, "cmd.exe", "cmd /c npm run build", 900)]), "on Windows too");
    }

    #[test]
    fn an_agents_own_helpers_are_not_background_work() {
        let all = vec![
            below(1, 0, "dragent", "", 0),
            below(2, 1, "claude", "claude", 0),
            below(3, 2, "npm", "npm exec some-server", 2),
            below(4, 3, "sh", "sh -c some-server", 3),
            below(5, 4, "node", "node some-server", 4),
            below(6, 2, "dragent", "dragent --dragent-cli mcp", 700),
            below(7, 2, "rust-analyzer", "rust-analyzer", 800),
            below(8, 2, "caffeinate", "caffeinate -i -t 300", 900),
        ];
        assert!(!runs_in_background(&all));
    }

    /// The real scan, on a real process tree shaped like an agent's: the
    /// launch monitor, the agent program, and a command it runs.
    #[cfg(unix)]
    #[test]
    fn a_running_command_is_found_below_a_real_agent() {
        use std::os::unix::process::CommandExt;
        let pane = Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("dragent-background-{pane}"));
        std::fs::create_dir_all(&dir).unwrap();
        // The command runs until the test says so: a slow machine takes
        // seconds over each look, which a fixed-length command could outlast.
        let script = "/bin/bash -c '/bin/sh -c \"while [ ! -e finish ]; do sleep 0.1; done; true\"; read -r _; true'; true";
        std::fs::write(dir.join(pane.to_string()), script).unwrap();
        let mut child = std::process::Command::new("/bin/bash")
            .arg0(crate::agent_sessions::RUN_ARG)
            .args([pane.to_string().as_str(), "--", "some-agent"])
            .current_dir(&dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let wait_for = |expected: bool| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                let found = agent_processes(&[pane])[&pane];
                if found.found && found.background == expected {
                    return true;
                }
                if std::time::Instant::now() > until {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        };
        let busy = wait_for(true);
        std::fs::write(dir.join("finish"), b"").unwrap();
        let quiet = wait_for(false);
        drop(child.stdin.take());
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(busy, "the command counts while it runs");
        assert!(quiet, "and not once it has finished");
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
