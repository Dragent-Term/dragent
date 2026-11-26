//! Find Dragent host and terminal services that nothing can reach any more,
//! and stop them safely (`dragent doctor --stop-strays`).
//!
//! A service is *current* when the record in its data folder names it: that
//! is how every client finds it, and it is never stopped from here. Any other
//! service is a stray — its data folder is gone, its record is gone, or its
//! record names another service. A stray with anything running inside it
//! (shells, agents) is only stopped when forced, and so is a terminal service
//! that can't be asked what it holds: after an update its shells are no longer
//! its child processes. A service that can't be judged — its environment can't
//! be read, or it has only just started — is left alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::protocol::{
    DaemonRequest, DaemonResponse, ENDPOINT_FILE_ENV, ENDPOINT_FILENAME, PTY_MANIFEST_FILENAME,
    PTY_SERVICE_ARG, PTY_TOKEN_ENV, PTY_UPDATE_RETRY, SERVICE_ARG, TerminalState,
};
use crate::service_lifecycle::RecordState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    Host,
    Terminal,
}

impl ServiceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Host => "host service",
            Self::Terminal => "terminal service",
        }
    }

    fn arg(self) -> &'static str {
        match self {
            Self::Host => SERVICE_ARG,
            Self::Terminal => PTY_SERVICE_ARG,
        }
    }

    fn record_name(self) -> &'static str {
        match self {
            Self::Host => ENDPOINT_FILENAME,
            Self::Terminal => PTY_MANIFEST_FILENAME,
        }
    }
}

/// A process running inside a service (a shell, and what runs in it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inner {
    pub pid: u32,
    /// e.g. `zsh → claude`.
    pub what: String,
}

/// One data folder a service may belong to, with its record file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub data_dir: PathBuf,
    pub record: PathBuf,
}

/// A running Dragent service process, as seen from outside.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceProcess {
    pub pid: u32,
    pub kind: ServiceKind,
    pub exe: Option<PathBuf>,
    /// Seconds since the unix epoch.
    pub started_at_unix: u64,
    /// Organisation id from its command line (`local` without one).
    pub organisation: String,
    #[serde(skip)]
    pub candidates: Vec<Candidate>,
    /// The terminal token it was started with, when its environment is
    /// readable (lets a stray terminal service be asked to close politely).
    #[serde(skip)]
    pub token: Option<String>,
    /// Processes running inside it, Dragent services excluded.
    pub inside: Vec<Inner>,
    /// Running terminals a terminal service says it holds; `None` when it
    /// could not be asked.
    pub terminals: Option<usize>,
    /// The folder its own environment chose, when that is known.
    #[serde(skip)]
    pub own_dir: Option<PathBuf>,
    /// Release or Dev, as far as known.
    #[serde(skip)]
    pub edition: &'static str,
    /// Why this service can't be judged from outside, if so.
    #[serde(skip)]
    pub unjudgeable: Option<String>,
}

/// A service this young may not have written its record yet, or may be taking
/// over terminals from the one it replaces.
const SETTLING: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "standing", rename_all = "snake_case")]
pub enum Standing {
    /// Its data folder's record names it: in use, never stopped from here.
    Current,
    /// Can't be told from here: left alone.
    Unknown { reason: String },
    /// No client can find it.
    Stray { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceReport {
    #[serde(flatten)]
    pub process: ServiceProcess,
    pub data_dir: Option<PathBuf>,
    pub data_dir_exists: bool,
    /// Built by the Dev edition (by its executable's path), as far as known.
    pub edition: &'static str,
    #[serde(flatten)]
    pub standing: Standing,
    /// Whether a client can still reach it (only through its record).
    pub reachable: bool,
    #[serde(skip)]
    pub record: RecordState,
    /// The folder `record` was judged in, when that isn't its own (shown)
    /// one: such a service is never spoken to.
    #[serde(skip)]
    judged_elsewhere: bool,
}

impl ServiceReport {
    /// What runs inside, for people.
    pub fn inside_text(&self) -> String {
        match (self.process.inside.len(), self.process.terminals) {
            (0, Some(0)) => "nothing inside".into(),
            (0, Some(n)) => format!("{n} terminal(s) inside"),
            (0, None) if self.process.kind == ServiceKind::Terminal => {
                "possibly terminals inside (it can't be asked)".into()
            }
            (0, None) => "nothing inside".into(),
            (n, _) => format!("{n} process(es) inside"),
        }
    }

    pub fn is_stray(&self) -> bool {
        matches!(self.standing, Standing::Stray { .. })
    }

    /// Something runs inside it, or might: stopping it could end processes.
    pub fn busy(&self) -> bool {
        !self.process.inside.is_empty()
            || (self.process.kind == ServiceKind::Terminal && self.process.terminals != Some(0))
    }
}

/// Which data folder a service belongs to and whether its record names it.
/// A record that names it in any candidate folder makes it current; else the
/// first existing folder says why it is a stray; no existing folder means
/// its data folder is gone. A record that can't be read or parsed (it may be
/// being rewritten) counts as naming it (safe side).
pub fn classify(
    kind: ServiceKind,
    pid: u32,
    candidates: &[Candidate],
    read: impl Fn(&Path) -> std::io::Result<String>,
) -> (Option<PathBuf>, RecordState) {
    let owner = |text: &str| -> Option<u32> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        value.get("pid")?.as_u64().map(|pid| pid as u32)
    };
    let _ = kind;
    let mut first_existing: Option<(PathBuf, RecordState)> = None;
    for candidate in candidates {
        if !candidate.data_dir.is_dir() {
            continue;
        }
        let state = match read(&candidate.record) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => RecordState::RecordGone,
            Err(_) => return (Some(candidate.data_dir.clone()), RecordState::Ours),
            Ok(text) => match owner(&text) {
                Some(named) if named != pid => RecordState::Replaced { pid: named },
                _ => return (Some(candidate.data_dir.clone()), RecordState::Ours),
            },
        };
        if first_existing.is_none() {
            first_existing = Some((candidate.data_dir.clone(), state));
        }
    }
    match first_existing {
        Some((dir, state)) => (Some(dir), state),
        None => (
            candidates.first().map(|c| c.data_dir.clone()),
            RecordState::DataFolderGone,
        ),
    }
}

/// The service's own home folder and why its folders can't be told, if so.
/// This process's variables say nothing about another's.
fn home_of(env: &HashMap<String, String>) -> Result<PathBuf, String> {
    if env.is_empty() {
        return Err("its environment can't be read".into());
    }
    for name in [ENDPOINT_FILE_ENV, "DRAGENT_DEV_HOME", "DRAGENT_HOME"] {
        if env
            .get(name)
            .is_some_and(|path| !path.is_empty() && !Path::new(path).is_absolute())
        {
            return Err("its data folder is relative to where it was started".into());
        }
    }
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env.get(name)
        .filter(|path| Path::new(path).is_absolute())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .ok_or_else(|| "its home folder is unknown".into())
}

/// Data folders a service started with this environment and organisation
/// may use: an explicit record path wins; otherwise each possible Dragent
/// home (Dev and release) with the organisation's folder in it.
pub fn candidates_for(
    kind: ServiceKind,
    env: &HashMap<String, String>,
    organisation: &str,
    user_home: &Path,
) -> Vec<Candidate> {
    if let Some(endpoint) = env.get(ENDPOINT_FILE_ENV).filter(|p| !p.is_empty()) {
        let endpoint = PathBuf::from(endpoint);
        let data_dir = endpoint.parent().map(Path::to_path_buf).unwrap_or_default();
        let record = match kind {
            ServiceKind::Host => endpoint,
            ServiceKind::Terminal => data_dir.join(PTY_MANIFEST_FILENAME),
        };
        return vec![Candidate { data_dir, record }];
    }
    let mut homes: Vec<PathBuf> = Vec::new();
    for name in ["DRAGENT_DEV_HOME", "DRAGENT_HOME"] {
        if let Some(path) = env.get(name).filter(|p| !p.is_empty()) {
            homes.push(PathBuf::from(path));
        }
    }
    homes.push(user_home.join(".dragent-dev"));
    homes.push(user_home.join(".dragent"));
    let mut out: Vec<Candidate> = Vec::new();
    for home in homes {
        let data_dir = organisation_dir(home, organisation);
        if out.iter().any(|c| c.data_dir == data_dir) {
            continue;
        }
        out.push(Candidate {
            record: data_dir.join(kind.record_name()),
            data_dir,
        });
    }
    out
}

/// The edition of a service, for people. A dev build may be anywhere, in a
/// build folder by any name; the folder for dev builds in its environment
/// gives it away, unless it is an installed app that inherited it.
fn edition_of(exe: Option<&Path>, dev_home_set: bool) -> &'static str {
    let Some(exe) = exe else { return "unknown" };
    let text = exe.to_string_lossy().replace('\\', "/");
    if text.contains("Dragent Dev") {
        return "dev";
    }
    if text.contains("/Dragent.app/") {
        return "release";
    }
    if dev_home_set || text.contains("/target/") || text.contains("/debug/") {
        "dev"
    } else {
        "release"
    }
}

/// The data folder a service's own environment names, as its edition reads it
/// (see [`dragent_paths::home`]); `None` when it names none.
fn own_dir(
    kind: ServiceKind,
    env: &HashMap<String, String>,
    organisation: &str,
    user_home: &Path,
    edition: &str,
) -> Option<PathBuf> {
    let var = |name: &str| env.get(name).filter(|path| !path.is_empty());
    if var(ENDPOINT_FILE_ENV).is_some() {
        let only = candidates_for(kind, env, organisation, user_home);
        return only.into_iter().next().map(|c| c.data_dir);
    }
    let home = var("DRAGENT_HOME").map(PathBuf::from);
    let home = match edition {
        "release" => home?,
        // A dev build ignores the installed app's folder.
        "dev" => match var(dragent_paths::DEV_HOME_ENV) {
            Some(dev) => PathBuf::from(dev),
            None => home.filter(|home| !home.starts_with(user_home.join(".dragent")))?,
        },
        _ => return None,
    };
    Some(organisation_dir(home, organisation))
}

fn organisation_dir(home: PathBuf, organisation: &str) -> PathBuf {
    if organisation == "local" {
        home
    } else {
        home.join("organisations")
            .join(dragent_paths::organisation_key(organisation))
    }
}

/// Every Dragent host/terminal service process on this machine (this
/// process excluded), with what runs inside each.
pub fn scan() -> Vec<ServiceProcess> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    let mut children: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let kind_of = |process: &sysinfo::Process| -> Option<ServiceKind> {
        let cmd = process.cmd();
        if cmd.iter().any(|a| a == PTY_SERVICE_ARG) {
            Some(ServiceKind::Terminal)
        } else if cmd.iter().any(|a| a == SERVICE_ARG) {
            Some(ServiceKind::Host)
        } else {
            None
        }
    };
    let name = |pid: &sysinfo::Pid| {
        system
            .process(*pid)
            .map(|p| p.name().to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    // Console hosts Windows attaches to every pseudo console.
    let plumbing = |n: &str| {
        let n = n.to_ascii_lowercase();
        n == "conhost.exe" || n == "openconsole.exe"
    };
    let me = std::process::id();
    let mut out = Vec::new();
    for (pid, process) in system.processes() {
        if pid.as_u32() == me {
            continue;
        }
        // Threads show up as processes on Linux; only count real ones.
        if process.thread_kind().is_some() {
            continue;
        }
        let Some(kind) = kind_of(process) else {
            continue;
        };
        let organisation = process
            .cmd()
            .iter()
            .filter_map(|a| a.to_str())
            .find_map(|a| a.strip_prefix(&format!("{}=", dragent_paths::ORGANISATION_ARG)))
            .unwrap_or("local")
            .to_string();
        let env: HashMap<String, String> = process
            .environ()
            .iter()
            .filter_map(|entry| {
                let entry = entry.to_str()?;
                let (k, v) = entry.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect();
        let (user_home, unjudgeable) = match home_of(&env) {
            Ok(home) => (home, None),
            Err(why) => (
                dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
                Some(why),
            ),
        };
        let dev_home_set = env
            .get(dragent_paths::DEV_HOME_ENV)
            .is_some_and(|path| !path.is_empty());
        let edition = edition_of(process.exe(), dev_home_set);
        let inside = children
            .get(pid)
            .into_iter()
            .flatten()
            .filter(|child| {
                system
                    .process(**child)
                    .is_some_and(|p| kind_of(p).is_none() && !plumbing(&name(child)))
            })
            .map(|child| {
                let mut what = vec![name(child)];
                if let Some(grand) = children.get(child) {
                    what.extend(grand.iter().map(&name).filter(|n| !plumbing(n)));
                }
                Inner {
                    pid: child.as_u32(),
                    what: what.join(" → "),
                }
            })
            .collect();
        out.push(ServiceProcess {
            pid: pid.as_u32(),
            kind,
            exe: process.exe().map(Path::to_path_buf),
            started_at_unix: process.start_time(),
            candidates: candidates_for(kind, &env, &organisation, &user_home),
            token: env.get(PTY_TOKEN_ENV).cloned().filter(|t| !t.is_empty()),
            own_dir: own_dir(kind, &env, &organisation, &user_home, edition),
            edition,
            organisation,
            inside,
            terminals: None,
            unjudgeable,
        });
    }
    out.sort_by_key(|p| (p.started_at_unix, p.pid));
    out
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Judge one scanned service.
pub fn report(process: ServiceProcess) -> ServiceReport {
    report_at(process, unix_now())
}

fn report_at(process: ServiceProcess, now_unix: u64) -> ServiceReport {
    let (data_dir, state) = classify(process.kind, process.pid, &process.candidates, |path| {
        std::fs::read_to_string(path)
    });
    // A start in the future (the clock was set back) counts as just now.
    let settling = now_unix.saturating_sub(process.started_at_unix) < SETTLING.as_secs();
    // What may be stopped follows from every folder it might use; what is said
    // about it, from the one it was started with.
    let (data_dir, why, judged_elsewhere) = match &process.own_dir {
        Some(own) if state != RecordState::Ours && data_dir.as_ref() != Some(own) => {
            // Its own folder is a candidate, so one that exists has no record
            // naming it either.
            let its = process
                .candidates
                .iter()
                .filter(|c| c.data_dir == *own)
                .cloned()
                .collect::<Vec<_>>();
            let (_, its) = classify(process.kind, process.pid, &its, |path| {
                std::fs::read_to_string(path)
            });
            (Some(own.clone()), its, true)
        }
        _ => (data_dir, state.clone(), false),
    };
    let data_dir_exists = data_dir.as_ref().is_some_and(|d| d.is_dir());
    let standing = match (&state, &process.unjudgeable) {
        (RecordState::Ours, _) => Standing::Current,
        (_, Some(why)) => Standing::Unknown {
            reason: why.clone(),
        },
        _ if settling => Standing::Unknown {
            reason: "it has only just started".into(),
        },
        (_, None) => Standing::Stray {
            reason: why.describe(),
        },
    };
    ServiceReport {
        edition: process.edition,
        reachable: standing == Standing::Current,
        data_dir,
        data_dir_exists,
        standing,
        record: state,
        judged_elsewhere,
        process,
    }
}

/// Ask a stray terminal service what it holds.
fn with_terminals(mut report: ServiceReport) -> ServiceReport {
    if report.is_stray() && report.process.kind == ServiceKind::Terminal {
        report.process.terminals = running_terminals(&report);
    }
    report
}

/// A connection that certainly reaches this very terminal service.
///
/// Its token proves nothing: a service that replaces another keeps the token,
/// so the current one would accept it too. While a record names another
/// service, that one may answer on the shared address (an update hands the
/// listening socket over), so there is no safe way to talk to the old one.
fn connection(report: &ServiceReport) -> Option<crate::pty_supervisor::DaemonConn> {
    if report.judged_elsewhere || matches!(report.record, RecordState::Replaced { .. }) {
        return None;
    }
    let dir = report.data_dir.as_ref()?;
    let conn = crate::pty_supervisor::DaemonConn {
        port: crate::protocol::pty_stable_port(&dir.to_string_lossy()),
        token: report.process.token.clone()?,
        socket: cfg!(unix).then(|| dir.join("pty.sock").to_string_lossy().into_owned()),
    };
    let hello = conn.rpc(DaemonRequest::Hello {
        version: crate::protocol::DAEMON_PROTOCOL_VERSION,
    });
    let Ok(DaemonResponse::HelloAck { started_at, .. }) = hello else {
        return None;
    };
    same_start(started_at, report.process.started_at_unix).then_some(conn)
}

/// The service that answered started when the process did.
fn same_start(answered_ms: u64, process_secs: u64) -> bool {
    (answered_ms / 1000).abs_diff(process_secs) <= 5
}

fn running_terminals(report: &ServiceReport) -> Option<usize> {
    match connection(report)?.rpc(DaemonRequest::ListTerminals {
        workspace_id: String::new(),
    }) {
        Ok(DaemonResponse::TerminalList { sessions }) => Some(
            sessions
                .iter()
                .filter(|s| matches!(s.state, TerminalState::Running))
                .count(),
        ),
        _ => None,
    }
}

/// Every Dragent service on this machine, judged.
pub fn survey() -> Vec<ServiceReport> {
    scan()
        .into_iter()
        .map(|process| with_terminals(report(process)))
        .collect()
}

/// What `stop` may do with a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Current: never stopped from here.
    Keep,
    /// A stray with nothing inside: safe to stop.
    Stop,
    /// A stray with processes inside: only with `--force`.
    NeedsForce,
}

pub fn verdict(report: &ServiceReport, force: bool) -> Verdict {
    if !report.is_stray() {
        Verdict::Keep
    } else if report.busy() && !force {
        Verdict::NeedsForce
    } else {
        Verdict::Stop
    }
}

/// Stop one stray, re-checking it first (it may have been replaced, picked
/// up, or started a shell since the survey). A terminal service is asked to
/// close politely when its token is known (this also works on Windows, which
/// has no SIGTERM); otherwise, or if it does not close, the verified service
/// process is terminated. True when it is gone.
pub fn stop(pid: u32, force: bool) -> Result<(), String> {
    let fresh = scan()
        .into_iter()
        .find(|p| p.pid == pid)
        .map(|process| with_terminals(report(process)))
        .ok_or_else(|| format!("pid {pid} is not a Dragent service (any more)"))?;
    match verdict(&fresh, force) {
        Verdict::Keep => {
            if let Standing::Unknown { reason } = &fresh.standing {
                return Err(format!(
                    "pid {pid} can't be judged ({reason}); not stopping it"
                ));
            }
            return Err(format!(
                "pid {pid} is the current {} of {}; not stopping it",
                fresh.process.kind.label(),
                fresh
                    .data_dir
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            ));
        }
        Verdict::NeedsForce => {
            return Err(format!(
                "pid {pid} has {}; pass --force to stop it",
                fresh.inside_text()
            ));
        }
        Verdict::Stop => {}
    }
    let kind = fresh.process.kind;
    if kind == ServiceKind::Terminal {
        if ask_to_close(&fresh) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if !crate::stale::is_service_process(pid, kind.arg()) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    if crate::stale::terminate_outdated(pid, kind.arg())
        || !crate::stale::is_service_process(pid, kind.arg())
    {
        Ok(())
    } else {
        Err(format!("pid {pid} did not stop"))
    }
}

/// Ask this very terminal service, and no other, to close. False when there is
/// no safe way to.
fn ask_to_close(report: &ServiceReport) -> bool {
    connection(report).is_some_and(|conn| conn.rpc(DaemonRequest::Shutdown).is_ok_or_retry())
}

trait OkOrRetry {
    fn is_ok_or_retry(&self) -> bool;
}

impl<T> OkOrRetry for Result<T, String> {
    fn is_ok_or_retry(&self) -> bool {
        match self {
            Ok(_) => true,
            Err(error) => error == PTY_UPDATE_RETRY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-strays-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn reader(path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn sample(kind: ServiceKind, inside: usize, candidates: Vec<Candidate>) -> ServiceReport {
        report(ServiceProcess {
            pid: 4242,
            kind,
            exe: None,
            started_at_unix: 0,
            organisation: "local".into(),
            candidates,
            token: None,
            inside: (0..inside)
                .map(|i| Inner {
                    pid: 5000 + i as u32,
                    what: "zsh → claude".into(),
                })
                .collect(),
            terminals: Some(inside),
            own_dir: None,
            edition: "unknown",
            unjudgeable: None,
        })
    }

    #[test]
    fn a_service_its_record_names_is_current_and_kept() {
        let dir = scratch();
        std::fs::write(dir.join(PTY_MANIFEST_FILENAME), r#"{"pid":4242}"#).unwrap();
        let candidates = vec![Candidate {
            data_dir: dir.clone(),
            record: dir.join(PTY_MANIFEST_FILENAME),
        }];
        let report = sample(ServiceKind::Terminal, 0, candidates.clone());
        assert_eq!(report.standing, Standing::Current);
        assert!(report.reachable);
        assert_eq!(verdict(&report, false), Verdict::Keep);
        // Never, not even forced.
        assert_eq!(verdict(&report, true), Verdict::Keep);
        // Busy or not.
        assert_eq!(
            verdict(&sample(ServiceKind::Terminal, 3, candidates), true),
            Verdict::Keep
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strays_are_named_with_their_reason() {
        let gone =
            std::env::temp_dir().join(format!("dragent-strays-gone-{}", uuid::Uuid::new_v4()));
        let at = |dir: &Path| Candidate {
            data_dir: dir.to_path_buf(),
            record: dir.join(ENDPOINT_FILENAME),
        };
        let (dir, state) = classify(ServiceKind::Host, 4242, &[at(&gone)], reader);
        assert_eq!(dir.as_deref(), Some(gone.as_path()));
        assert_eq!(state, RecordState::DataFolderGone);

        let live = scratch();
        let (_, state) = classify(ServiceKind::Host, 4242, &[at(&gone), at(&live)], reader);
        assert_eq!(state, RecordState::RecordGone);

        std::fs::write(live.join(ENDPOINT_FILENAME), r#"{"pid":7}"#).unwrap();
        let (_, state) = classify(ServiceKind::Host, 4242, &[at(&live)], reader);
        assert_eq!(state, RecordState::Replaced { pid: 7 });

        // A record naming it in any candidate folder wins over the others.
        let other = scratch();
        std::fs::write(other.join(ENDPOINT_FILENAME), r#"{"pid":4242}"#).unwrap();
        let (dir, state) = classify(ServiceKind::Host, 4242, &[at(&live), at(&other)], reader);
        assert_eq!(
            (dir.as_deref(), state),
            (Some(other.as_path()), RecordState::Ours)
        );

        // A record it can't parse counts as naming it.
        std::fs::write(live.join(ENDPOINT_FILENAME), "{").unwrap();
        let (_, state) = classify(ServiceKind::Host, 4242, &[at(&live)], reader);
        assert_eq!(state, RecordState::Ours);
        let _ = std::fs::remove_dir_all(&live);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_stray_is_described_by_its_own_folder_and_judged_as_before() {
        let user = scratch();
        let default = user.join(".dragent-dev");
        std::fs::create_dir_all(&default).unwrap();
        let record = default.join(PTY_MANIFEST_FILENAME);
        std::fs::write(&record, r#"{"pid":7}"#).unwrap();
        let gone = user.join("gone");
        let env = HashMap::from([(
            dragent_paths::DEV_HOME_ENV.to_string(),
            gone.to_string_lossy().into_owned(),
        )]);
        let candidates = candidates_for(ServiceKind::Terminal, &env, "local", &user);
        let mut process = sample(ServiceKind::Terminal, 0, candidates).process;
        process.own_dir = own_dir(ServiceKind::Terminal, &env, "local", &user, "dev");
        let judged = report(process.clone());
        assert_eq!(judged.data_dir.as_ref(), Some(&gone));
        assert!(!judged.data_dir_exists);
        assert_eq!(
            judged.standing,
            Standing::Stray {
                reason: RecordState::DataFolderGone.describe()
            }
        );
        // The record in the other folder still decides what may be done.
        assert_eq!(judged.record, RecordState::Replaced { pid: 7 });
        assert!(connection(&judged).is_none());
        process.terminals = None;
        assert_eq!(verdict(&report(process.clone()), false), Verdict::NeedsForce);
        // Named there, it stays current.
        std::fs::write(&record, r#"{"pid":4242}"#).unwrap();
        let named = report(process);
        assert_eq!(named.standing, Standing::Current);
        assert_eq!(named.data_dir, Some(default));
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn a_services_own_folder_and_edition_follow_its_environment() {
        let user = Path::new("/u");
        let own = |vars: &[(&str, &str)], edition| {
            let env = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            own_dir(ServiceKind::Host, &env, "local", user, edition)
        };
        let dev = ("DRAGENT_DEV_HOME", "/d");
        let home = ("DRAGENT_HOME", "/h");
        assert_eq!(own(&[dev, home], "dev"), Some("/d".into()));
        assert_eq!(own(&[dev, home], "release"), Some("/h".into()));
        assert_eq!(own(&[home], "dev"), Some("/h".into()));
        assert_eq!(own(&[("DRAGENT_HOME", "/u/.dragent")], "dev"), None);
        assert_eq!(own(&[dev], "release"), None);
        assert_eq!(own(&[], "dev"), None);
        let endpoint = (ENDPOINT_FILE_ENV, "/t/host.json");
        assert_eq!(own(&[dev, endpoint], "dev"), Some("/t".into()));

        let edition = |exe: &str, dev| edition_of(Some(Path::new(exe)), dev);
        assert_eq!(edition("/tmp/bins/dragent_perf", true), "dev");
        assert_eq!(edition("/c/team-build/debug/dragent", false), "dev");
        assert_eq!(edition("/r/target/release/dragent", false), "dev");
        assert_eq!(edition("C:\\r\\target\\release\\dragent.exe", false), "dev");
        let app = "/Applications/Dragent.app/Contents/MacOS/dragent";
        assert_eq!(edition(app, true), "release");
        assert_eq!(edition(&app.replace("Dragent", "Dragent Dev"), false), "dev");
        assert_eq!(edition("/opt/dragent/dragent", false), "release");
        assert_eq!(edition_of(None, true), "unknown");
    }

    #[test]
    fn a_busy_stray_needs_force_and_an_empty_one_does_not() {
        let gone =
            std::env::temp_dir().join(format!("dragent-strays-gone-{}", uuid::Uuid::new_v4()));
        let candidates = vec![Candidate {
            data_dir: gone.clone(),
            record: gone.join(PTY_MANIFEST_FILENAME),
        }];
        let empty = sample(ServiceKind::Terminal, 0, candidates.clone());
        assert!(empty.is_stray() && !empty.reachable && !empty.data_dir_exists);
        assert_eq!(verdict(&empty, false), Verdict::Stop);
        let busy = sample(ServiceKind::Terminal, 2, candidates);
        assert_eq!(verdict(&busy, false), Verdict::NeedsForce);
        assert_eq!(verdict(&busy, true), Verdict::Stop);
    }

    #[test]
    fn a_terminal_service_that_cant_be_asked_needs_force() {
        // After an update a service's shells are not its children.
        let gone = std::env::temp_dir().join(format!("dragent-strays-{}", uuid::Uuid::new_v4()));
        let candidates = vec![Candidate {
            data_dir: gone.clone(),
            record: gone.join(PTY_MANIFEST_FILENAME),
        }];
        let mut adopted = sample(ServiceKind::Terminal, 0, candidates.clone());
        adopted.process.terminals = None;
        assert_eq!(verdict(&adopted, false), Verdict::NeedsForce);
        adopted.process.terminals = Some(2);
        assert_eq!(verdict(&adopted, false), Verdict::NeedsForce);
        // A host holds no terminals: its children are all there is.
        let mut host = sample(ServiceKind::Host, 0, candidates);
        host.process.terminals = None;
        assert_eq!(verdict(&host, false), Verdict::Stop);
    }

    #[test]
    fn a_record_that_cant_be_read_counts_as_naming_the_service() {
        let dir = scratch();
        let candidates = [Candidate {
            data_dir: dir.clone(),
            record: dir.join(ENDPOINT_FILENAME),
        }];
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::Other,
        ] {
            let (_, state) = classify(ServiceKind::Host, 1, &candidates, |_| Err(kind.into()));
            assert_eq!(state, RecordState::Ours, "{kind:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_service_that_cant_be_judged_or_just_started_is_left_alone() {
        let gone = std::env::temp_dir().join(format!("dragent-strays-{}", uuid::Uuid::new_v4()));
        let candidates = vec![Candidate {
            data_dir: gone.clone(),
            record: gone.join(PTY_MANIFEST_FILENAME),
        }];
        let mut process = sample(ServiceKind::Host, 0, candidates).process;
        process.started_at_unix = 10_000;
        let at = |process: &ServiceProcess, now| verdict(&report_at(process.clone(), now), true);
        assert_eq!(at(&process, 10_059), Verdict::Keep);
        // The clock was set back since it started.
        assert_eq!(at(&process, 5_000), Verdict::Keep);
        assert_eq!(at(&process, 10_060), Verdict::Stop);
        process.unjudgeable = Some("its environment can't be read".into());
        assert_eq!(at(&process, 99_999), Verdict::Keep);
    }

    #[test]
    fn the_home_folder_is_the_services_own() {
        let mut env = HashMap::new();
        assert!(home_of(&env).is_err(), "unreadable environment");
        let home = std::env::temp_dir();
        let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        env.insert(name.to_string(), home.to_string_lossy().into_owned());
        assert_eq!(home_of(&env), Ok(home));
        env.insert("DRAGENT_HOME".into(), "data".into());
        assert!(home_of(&env).is_err(), "relative to its own start folder");
    }

    /// Answers like a running terminal service that started at `started_at`
    /// and accepts any token; returns what it was asked.
    #[cfg(unix)]
    fn stand_in(
        dir: &Path,
        started_at: u64,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<DaemonRequest>>> {
        use crate::protocol::{
            DAEMON_PROTOCOL_VERSION, DaemonRequestEnvelope, DaemonResponseEnvelope, read_message,
            write_message,
        };
        let listener = std::os::unix::net::UnixListener::bind(dir.join("pty.sock")).unwrap();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = asked.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let Ok(request) = read_message::<DaemonRequestEnvelope>(&mut stream) else {
                    continue;
                };
                let response = match &request.request {
                    DaemonRequest::Hello { .. } => DaemonResponse::HelloAck {
                        version: DAEMON_PROTOCOL_VERSION,
                        daemon_id: "stand-in".into(),
                        started_at,
                    },
                    _ => DaemonResponse::Ack,
                };
                log.lock().unwrap().push(request.request);
                let _ = write_message(
                    &mut stream,
                    &DaemonResponseEnvelope {
                        version: DAEMON_PROTOCOL_VERSION,
                        result: Ok(response),
                    },
                );
            }
        });
        asked
    }

    /// A service that replaces another keeps its token and may answer on its
    /// address: stopping the old one must never close the current one.
    #[cfg(unix)]
    #[test]
    fn a_replacement_reusing_the_token_is_never_stopped() {
        let shutdowns = |asked: &std::sync::Mutex<Vec<DaemonRequest>>| {
            let asked = asked.lock().unwrap();
            asked
                .iter()
                .filter(|r| matches!(r, DaemonRequest::Shutdown))
                .count()
        };
        // Short: a Unix socket's path may not exceed about 100 characters.
        let name = uuid::Uuid::new_v4().simple().to_string();
        let dir = std::env::temp_dir().join(format!("ds-{}", &name[..8]));
        std::fs::create_dir_all(&dir).unwrap();
        let record = dir.join(PTY_MANIFEST_FILENAME);
        let stray = || {
            let mut report = sample(
                ServiceKind::Terminal,
                0,
                vec![Candidate {
                    data_dir: dir.clone(),
                    record: record.clone(),
                }],
            );
            report.process.started_at_unix = 10_000;
            report.process.token = Some("shared-by-both".into());
            report
        };
        let replacement = stand_in(&dir, 20_000_000);

        // The record names the replacement: the old one isn't spoken to.
        std::fs::write(&record, r#"{"pid":7}"#).unwrap();
        assert!(stray().is_stray());
        assert!(!ask_to_close(&stray()));
        assert!(replacement.lock().unwrap().is_empty());

        // The replacement's record is missing for a moment: it answers, but
        // as a service that started at another time.
        std::fs::remove_file(&record).unwrap();
        assert!(!ask_to_close(&stray()));
        assert_eq!(shutdowns(&replacement), 0);

        // The stray itself answers: it is asked to close.
        std::fs::remove_file(dir.join("pty.sock")).unwrap();
        let itself = stand_in(&dir, 10_001_000);
        assert!(ask_to_close(&stray()));
        assert_eq!((shutdowns(&itself), shutdowns(&replacement)), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidate_folders_follow_the_service_environment() {
        let home = Path::new("/u");
        let mut env = HashMap::new();
        // Defaults: both editions' homes.
        let found = candidates_for(ServiceKind::Host, &env, "local", home);
        assert_eq!(
            found.iter().map(|c| c.data_dir.clone()).collect::<Vec<_>>(),
            [home.join(".dragent-dev"), home.join(".dragent")]
        );
        assert_eq!(
            found[1].record,
            home.join(".dragent").join(ENDPOINT_FILENAME)
        );
        // An organisation's folder sits under its home.
        let org = candidates_for(ServiceKind::Terminal, &env, "acme", home);
        assert_eq!(
            org[0].record,
            home.join(".dragent-dev")
                .join("organisations")
                .join(dragent_paths::organisation_key("acme"))
                .join(PTY_MANIFEST_FILENAME)
        );
        // A home set in its environment comes first, without duplicates.
        env.insert("DRAGENT_HOME".into(), "/u/.dragent".into());
        let found = candidates_for(ServiceKind::Host, &env, "local", home);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].data_dir, home.join(".dragent"));
        // An explicit endpoint file (tests) is the only candidate.
        env.insert(ENDPOINT_FILE_ENV.into(), "/t/endpoint.json".into());
        let found = candidates_for(ServiceKind::Terminal, &env, "local", home);
        assert_eq!(
            found,
            [Candidate {
                data_dir: PathBuf::from("/t"),
                record: PathBuf::from("/t").join(PTY_MANIFEST_FILENAME),
            }]
        );
    }
}
