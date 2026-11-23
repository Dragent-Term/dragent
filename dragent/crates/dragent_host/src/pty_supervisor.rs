//! Supervised ownership of the PTY daemon child process.
//!
//! The host no longer owns [`TerminalRegistry`](super::terminals::TerminalRegistry)
//! in-process. Instead each host supervises one
//! `dragent-host --dragent-pty-service` child per org dir (the endpoint
//! file's parent). The supervisor:
//!
//! * adopts a healthy manifest on startup (a SIGKILLed host's shells keep
//!   running — surviving shells are the feature),
//! * spawns a replacement when the manifest is stale (counting each dead
//!   daemon id once toward the circuit breaker),
//! * watches its owned child (precise `try_wait`, never pid checks) plus a
//!   hello-health checks for adopted daemons (respawned after
//!   [`WATCH_STRIKES`] misses),
//! * opens a circuit past [`MAX_CRASHES`] crashes inside a sliding
//!   [`CRASH_WINDOW`] so a crash loop fails openly instead of hot-respawning.
//!   Recovery resumes after a cooldown; an explicit `restart` bypasses it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::RngCore;

use super::protocol::*;
use super::transport::{Address, Stream};

/// Crash budget: more than this many daemon crashes inside [`CRASH_WINDOW`]
/// pause automatic respawn for a cooldown.
pub const MAX_CRASHES: u32 = 3;
/// Sliding window the crash budget is counted over. Crashes older than this
/// no longer count. The same interval is the recovery cooldown.
pub const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// Watcher poll interval. Adopted daemon liveness is checked frequently so a
/// daemon that dies outside this host is recovered without waiting for a UI
/// request.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// Consecutive failed health checks before the watcher acts.
pub const WATCH_STRIKES: u32 = 3;
/// Grace period for a Shutdown RPC before portable kill escalation.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long a fresh spawn gets to write its manifest + answer hello.
pub const SPAWN_WAIT: Duration = Duration::from_secs(5);
/// Test hook: when set, respawn backoff collapses to 50 ms.
pub const TEST_FAST_ENV: &str = "DRAGENT_PTY_TEST_FAST";
/// Hello attempts in the fast path before declaring a manifest dead.
/// How long a live daemon may go without answering before it is stopped and
/// replaced. Heavy builds can starve it for many seconds at a time.
const UNRESPONSIVE_GRACE: Duration = Duration::from_secs(60);
/// Test override for [`UNRESPONSIVE_GRACE`], in milliseconds.
const UNRESPONSIVE_GRACE_ENV: &str = "DRAGENT_PTY_UNRESPONSIVE_GRACE_MS";
const TERMINAL_SERVICE_NOT_ANSWERING: &str =
    "The terminal service isn't responding yet. Your sessions are safe; retrying.";
const HELLO_RETRIES: u32 = 5;
const HELLO_RETRY_WAIT: Duration = Duration::from_millis(100);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Structured ops log: one JSON object per supervisor lifecycle event on
/// target `dragent::pty` (filterable via `RUST_LOG`, parseable via `jq`).
/// Human-readable `log::*` lines stay everywhere else; only lifecycle
/// events go through `emit()`.
fn emit(level: log::Level, event: &str, fields: serde_json::Value) {
    let mut obj = match fields {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    obj.insert(
        "event".to_owned(),
        serde_json::Value::String(event.to_owned()),
    );
    log::log!(
        target: "dragent::pty",
        level,
        "{}",
        serde_json::Value::Object(obj).to_string()
    );
}

/// Backoff before an async respawn for the `crash_n`-th crash in the
/// window: 200 ms, 1 s, 5 s, then `None` once the budget is spent (the
/// circuit is open).
pub fn respawn_delay(crash_n: u32) -> Option<Duration> {
    match crash_n {
        0 | 1 => Some(Duration::from_millis(200)),
        2 => Some(Duration::from_secs(1)),
        n if n <= MAX_CRASHES => Some(Duration::from_secs(5)),
        _ => None,
    }
}

/// Was `build` made after `other`? Build ids end in the executable's
/// modification time; anything unreadable counts as not newer.
fn build_is_newer(build: &str, other: &str) -> bool {
    let modified = |id: &str| id.rsplit('-').next()?.parse::<u128>().ok();
    matches!((modified(build), modified(other)), (Some(a), Some(b)) if a > b)
}

// ── Manifest file ──

pub fn manifest_path(org_dir: &Path) -> PathBuf {
    org_dir.join(PTY_MANIFEST_FILENAME)
}

/// Durable update receipt: both the open GUI and the agent launch helper
/// distinguish an update shutdown from a user ending a session.
const UPDATE_SESSIONS_FILE: &str = "pty-update-sessions.json";

pub fn session_interrupted_by_update(session_id: &str) -> bool {
    let path = org_dir().join(UPDATE_SESSIONS_FILE);
    std::fs::read(path)
        .ok()
        .and_then(|raw| {
            serde_json::from_slice::<std::collections::HashMap<String, String>>(&raw).ok()
        })
        .is_some_and(|ids| ids.contains_key(session_id))
}

/// Recovery must wait until the old daemon has finished session cleanup;
/// creating a shell in that daemon would immediately lose it again.
pub fn update_replacement_ready(session_id: &str) -> bool {
    let dir = org_dir();
    let previous = std::fs::read(dir.join(UPDATE_SESSIONS_FILE))
        .ok()
        .and_then(|raw| {
            serde_json::from_slice::<std::collections::HashMap<String, String>>(&raw).ok()
        })
        .and_then(|ids| ids.get(session_id).cloned());
    match (previous, read_manifest(&dir)) {
        (Some(previous), Some(manifest)) => {
            manifest.daemon_id != previous && hello(&manifest).is_ok()
        }
        _ => false,
    }
}

pub fn read_manifest(org_dir: &Path) -> Option<PtyDaemonManifest> {
    let raw = std::fs::read_to_string(manifest_path(org_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_manifest(org_dir: &Path, manifest: &PtyDaemonManifest) -> std::io::Result<()> {
    std::fs::create_dir_all(org_dir)?;
    let path = manifest_path(org_dir);
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let file = std::fs::File::create(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    serde_json::to_writer(&file, manifest)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    (&file as &std::fs::File).write_all(b"\n")?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove the manifest only when it still describes `pid` (never clobber a
/// successor that already replaced it).
pub fn remove_manifest_if_pid(org_dir: &Path, pid: u32) -> bool {
    match read_manifest(org_dir) {
        Some(manifest) if manifest.pid == pid => {
            std::fs::remove_file(manifest_path(org_dir)).is_ok()
        }
        _ => false,
    }
}

/// Remove the manifest only when it still describes `daemon_id` (the
/// daemon's own exit path — a replaced successor keeps its file).
pub fn remove_manifest_if_daemon(org_dir: &Path, daemon_id: &str) -> bool {
    match read_manifest(org_dir) {
        Some(manifest) if manifest.daemon_id == daemon_id => {
            std::fs::remove_file(manifest_path(org_dir)).is_ok()
        }
        _ => false,
    }
}

/// Org dir: the endpoint file's parent (mirrors worktrees/agents).
pub fn org_dir() -> PathBuf {
    super::discovery::endpoint_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

// ── Daemon connection ──

/// One unary RPC connection's worth of address + token.
#[derive(Debug, Clone)]
pub struct DaemonConn {
    pub port: u16,
    pub token: String,
    /// Unix domain socket path (macOS/Linux), preferred over `port`.
    pub socket: Option<String>,
}

impl From<&PtyDaemonManifest> for DaemonConn {
    fn from(manifest: &PtyDaemonManifest) -> Self {
        DaemonConn {
            port: manifest.port,
            token: manifest.token.clone(),
            socket: manifest.socket.clone(),
        }
    }
}

impl DaemonConn {
    /// Open a long-lived connection carrying `request` (e.g. an output
    /// subscription); the caller reads responses until the stream ends.
    pub fn open_stream(&self, request: DaemonRequest) -> Result<Stream, String> {
        let address = Address::from_parts(self.port, self.socket.as_deref());
        let stream = Stream::connect(&address, Duration::from_secs(2))
            .map_err(|e| format!("pty connect: {e}"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| format!("{e}"))?;
        let envelope = DaemonRequestEnvelope {
            version: DAEMON_PROTOCOL_VERSION,
            token: self.token.clone(),
            request,
        };
        super::protocol::write_message(&mut &stream, &envelope)
            .map_err(|e| format!("pty write: {e}"))?;
        Ok(stream)
    }

    pub fn rpc(&self, request: DaemonRequest) -> Result<DaemonResponse, String> {
        self.rpc_for_version(request, DAEMON_PROTOCOL_VERSION)
    }

    // Update shutdowns use the daemon's advertised version, so a protocol
    // change does not prevent an older daemon from accepting its own Shutdown.
    fn rpc_for_version(
        &self,
        request: DaemonRequest,
        version: u32,
    ) -> Result<DaemonResponse, String> {
        // Creation may wait for the spawn gate and shell integration. A hello
        // does no work, and a daemon that can't answer one holds up every caller.
        let timeout = match &request {
            DaemonRequest::CreateTerminal { .. } => Duration::from_secs(45),
            DaemonRequest::Hello { .. } => Duration::from_secs(3),
            _ => Duration::from_secs(10),
        };
        let address = Address::from_parts(self.port, self.socket.as_deref());
        let stream = Stream::connect(&address, Duration::from_secs(2))
            .map_err(|e| format!("pty connect: {e}"))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| format!("{e}"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| format!("{e}"))?;
        let envelope = DaemonRequestEnvelope {
            version,
            token: self.token.clone(),
            request,
        };
        super::protocol::write_message(&mut &stream, &envelope)
            .map_err(|e| format!("pty write: {e}"))?;
        let env: DaemonResponseEnvelope =
            super::protocol::read_message(&mut &stream).map_err(|e| format!("pty read: {e}"))?;
        if env.version != version {
            return Err(format!(
                "pty protocol mismatch (daemon {}, client {})",
                env.version, version
            ));
        }
        env.result
    }
}

/// Liveness probe: what daemon id + start time answer at this manifest's
/// address/token?
pub fn hello(manifest: &PtyDaemonManifest) -> Result<(String, u64), String> {
    match DaemonConn::from(manifest).rpc(DaemonRequest::Hello {
        version: DAEMON_PROTOCOL_VERSION,
    }) {
        Ok(DaemonResponse::HelloAck {
            daemon_id,
            started_at,
            ..
        }) if daemon_id == manifest.daemon_id && started_at == manifest.started_at_unix_ms => {
            Ok((daemon_id, started_at))
        }
        Ok(DaemonResponse::HelloAck { .. }) => {
            Err("PTY readiness answered by another daemon generation".into())
        }
        Ok(other) => Err(format!("pty unexpected hello response: {other:?}")),
        Err(error) => Err(error),
    }
}

// ── Supervisor ──

struct Inner {
    child: Option<Child>,
    /// When each counted crash happened; pruned to [`CRASH_WINDOW`].
    crash_times: std::collections::VecDeque<Instant>,
    circuit_open: bool,
    last_crash_id: Option<String>,
    /// A daemon has been up (spawned or adopted) at least once. Before that
    /// a missing manifest is the lazy initial state, not a vanished daemon.
    ever_up: bool,
    /// The daemon that stopped answering while its process stays alive, and
    /// since when.
    unresponsive: Option<(String, Instant)>,
}

impl Inner {
    fn recovery_wait(&mut self) -> Option<Duration> {
        if !self.circuit_open {
            return None;
        }
        let remaining = self
            .crash_times
            .back()
            .map(|at| CRASH_WINDOW.saturating_sub(at.elapsed()))
            .unwrap_or_default();
        if !remaining.is_zero() {
            return Some(remaining);
        }
        self.circuit_open = false;
        self.crash_times.clear();
        // Retain last_crash_id: polling the same dead manifest is not a new crash.
        None
    }

    /// Crashes inside the sliding window (prunes older ones).
    fn recent_crashes(&mut self) -> u32 {
        let now = Instant::now();
        while self
            .crash_times
            .front()
            .is_some_and(|at| now.duration_since(*at) >= CRASH_WINDOW)
        {
            self.crash_times.pop_front();
        }
        self.crash_times.len() as u32
    }
}

#[derive(Debug, Clone)]
pub struct PtyStatus {
    pub daemon_pid: Option<u32>,
    pub daemon_id: Option<String>,
    pub owned: bool,
    pub circuit_open: bool,
    pub crash_attempts: u32,
}

pub struct PtySupervisor {
    org_dir: PathBuf,
    exe: PathBuf,
    build_id: String,
    inner: Mutex<Inner>,
    spawn_lock: Mutex<()>,
    update_deferred: Mutex<Option<Instant>>,
}

impl PtySupervisor {
    /// Infallible constructor: captures executable metadata without connecting
    /// to a daemon. The host comes up even when its terminal service cannot.
    pub fn new(org_dir: PathBuf, exe: PathBuf) -> Self {
        Self {
            org_dir,
            build_id: super::build_identity::executable(&exe).unwrap_or_default(),
            exe,
            inner: Mutex::new(Inner {
                child: None,
                crash_times: std::collections::VecDeque::new(),
                circuit_open: false,
                last_crash_id: None,
                ever_up: false,
                unresponsive: None,
            }),
            spawn_lock: Mutex::new(()),
            update_deferred: Mutex::new(None),
        }
    }

    /// Fast-path health: manifest + hello (brief retries cover daemon
    /// startup / transient refused windows; a dead daemon refuses fast).
    fn healthy_conn(&self) -> Option<DaemonConn> {
        let manifest = read_manifest(&self.org_dir)?;
        // A daemon from another build is never "healthy" here, even if it
        // answers: it must be upgraded (see `spawn_locked`).
        if manifest.version != DAEMON_PROTOCOL_VERSION {
            return None;
        }
        let desired = &self.build_id;
        if !desired.is_empty() && manifest.build_id == *desired {
            *self
                .update_deferred
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
        }
        let deferred = self
            .update_deferred
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some_and(|at| at.elapsed() < Duration::from_secs(300));
        if self.should_replace_build(&manifest) && !deferred {
            return None;
        }
        for _ in 0..HELLO_RETRIES {
            if hello(&manifest).is_ok() {
                self.inner
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .unresponsive = None;
                return Some(DaemonConn::from(&manifest));
            }
            std::thread::sleep(HELLO_RETRY_WAIT);
        }
        None
    }

    /// Should this host move a same-protocol daemon onto its own build? Only
    /// forwards. Two installed builds open at once (a dev build beside the
    /// app, or a tool started from the other one) would otherwise take the
    /// daemon back and forth on every request, restarting it each time.
    fn should_replace_build(&self, manifest: &PtyDaemonManifest) -> bool {
        !self.build_id.is_empty()
            && manifest.build_id != self.build_id
            && !build_is_newer(&manifest.build_id, &self.build_id)
    }

    /// A daemon that is alive but not answering: the machine is overloaded,
    /// it has run out of file handles, or it has hung. Starting a second one
    /// beside it would leave its terminals running where nothing can reach
    /// them, so wait, and only replace it by stopping it first.
    /// `Ok` means it is gone and a new one may start.
    fn stop_if_unresponsive_too_long(&self, manifest: &PtyDaemonManifest) -> Result<(), String> {
        let grace = std::env::var(UNRESPONSIVE_GRACE_ENV)
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(UNRESPONSIVE_GRACE);
        let waited = {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            match &inner.unresponsive {
                Some((id, since)) if *id == manifest.daemon_id => since.elapsed(),
                _ => {
                    inner.unresponsive = Some((manifest.daemon_id.clone(), Instant::now()));
                    Duration::ZERO
                }
            }
        };
        if waited < grace {
            return Err(TERMINAL_SERVICE_NOT_ANSWERING.to_string());
        }
        emit(
            log::Level::Warn,
            "daemon_stopped_unresponsive",
            serde_json::json!({ "pid": manifest.pid, "waited_ms": waited.as_millis() as u64 }),
        );
        super::stale::terminate_outdated(manifest.pid, PTY_SERVICE_ARG);
        if super::stale::process_may_be_alive(manifest.pid) {
            return Err(TERMINAL_SERVICE_NOT_ANSWERING.to_string());
        }
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .unresponsive = None;
        Ok(())
    }

    /// Resolve a live daemon: circuit check, stable-reset refund, adopt a
    /// healthy manifest, else spawn under the lock.
    pub fn ensure(&self) -> Result<DaemonConn, String> {
        if let Some(conn) = self.healthy_conn() {
            self.inner.lock().unwrap_or_else(|p| p.into_inner()).ever_up = true;
            return Ok(conn);
        }
        self.spawn_locked()
    }

    /// Spawn (or adopt) under the spawn lock. Re-checks health first so two
    /// racing callers converge on one daemon; counts a dead manifest once
    /// per daemon id toward the circuit.
    pub fn spawn_locked(&self) -> Result<DaemonConn, String> {
        let _guard = self.spawn_lock.lock().unwrap_or_else(|p| p.into_inner());
        // Re-check under the lock: a racer may have brought one up.
        if let Some(conn) = self.healthy_conn() {
            return Ok(conn);
        }
        if let Some(wait) = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .recovery_wait()
        {
            return Err(format!(
                "Terminal service is recovering after repeated failures. Automatic retry in {}s",
                wait.as_secs() + 1
            ));
        }
        let dead = read_manifest(&self.org_dir);
        if let Some(ref manifest) = dead {
            let alive = super::stale::service_may_be_alive(
                manifest.pid,
                manifest.started_at_unix_ms,
                PTY_SERVICE_ARG,
            );
            if alive
                && (manifest.version != DAEMON_PROTOCOL_VERSION
                    || self.should_replace_build(manifest))
            {
                if self.upgrade(manifest) {
                    if let Some(conn) = self.healthy_conn() {
                        return Ok(conn);
                    }
                    // The Windows daemon exited for the update; start the new build.
                    if !super::stale::process_may_be_alive(manifest.pid) {
                        remove_manifest_if_pid(&self.org_dir, manifest.pid);
                        drop(_guard);
                        return self.spawn_locked();
                    }
                }
                // If the update failed, reuse compatible code and retry later.
                // Unix handoff failures preserve the original PTY owner;
                // Windows uses an explicit shutdown for successful updates.
                *self
                    .update_deferred
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
                if manifest.version == DAEMON_PROTOCOL_VERSION && hello(manifest).is_ok() {
                    log::warn!(
                        "PTY update deferred; preserving live sessions on compatible daemon"
                    );
                    return Ok(DaemonConn::from(manifest));
                }
                return Err("Terminal service update pending: safe handoff unavailable. Existing sessions were preserved; retry after the service exits.".into());
            } else if alive {
                self.stop_if_unresponsive_too_long(manifest)?;
            } else if self.account_crash(&manifest.daemon_id) {
                return Err(
                    "Terminal service is recovering after repeated failures. Automatic retry in 60s".to_string(),
                );
            }
        }
        // Reap our handle when it already exited (precise, no pid checks).
        // A still-running handle is left alone — it is stale by construction
        // (no healthy manifest describes it) and gets replaced below.
        let stale = {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            match inner.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(_)) => inner.child.take(),
                    _ => None,
                },
                None => None,
            }
        };
        drop(stale);
        // Stable token reuse: keep the manifest token across respawns.
        let token = dead
            .map(|manifest| manifest.token)
            .unwrap_or_else(generate_token);
        let mut command = Command::new(&self.exe);
        // A fresh daemon, never a hand-off successor, whatever this process
        // inherited.
        for name in dragent_paths::PTY_HANDOFF_VARS {
            command.env_remove(name);
        }
        let mut child = command
            .arg(PTY_SERVICE_ARG)
            .arg(dragent_paths::organisation_arg())
            .env(PTY_TOKEN_ENV, &token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // The host's own stderr: null normally, the log file when
            // `DRAGENT_HOST_LOG_FILE` is set, so daemon logs land with it.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("pty spawn: {e}"))?;
        let pid = child.id();
        // Wait for OUR daemon's manifest (pid + token) plus a hello.
        let deadline = Instant::now() + SPAWN_WAIT;
        let mut ready: Option<DaemonConn> = None;
        while Instant::now() < deadline {
            match read_manifest(&self.org_dir) {
                Some(manifest) if manifest.pid == pid && manifest.token == token => {
                    if hello(&manifest).is_ok() {
                        ready = Some(DaemonConn::from(&manifest));
                        break;
                    }
                }
                _ => {}
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = child.wait();
                    return Err(format!("pty daemon exited during startup: {status}"));
                }
                Err(error) => return Err(format!("pty wait: {error}")),
                Ok(None) => {}
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        match ready {
            Some(conn) => {
                let port = conn.port;
                let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
                inner.ever_up = true;
                if let Some(mut previous) = inner.child.replace(child) {
                    // A still-running previous handle is stale (no healthy
                    // manifest described it): reclaim it so it never leaks.
                    match previous.try_wait() {
                        Ok(None) => {
                            let _ = previous.kill();
                            let _ = previous.wait();
                        }
                        _ => {}
                    }
                }
                drop(inner);
                emit(
                    log::Level::Info,
                    "daemon_up",
                    serde_json::json!({ "pid": pid, "port": port }),
                );
                Ok(conn)
            }
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Err("pty daemon did not come up within 5s".to_string())
            }
        }
    }

    /// Windows updates deliberately close active sessions: persistent shells
    /// must not prevent a new build from taking effect. The existing Shutdown
    /// request also works with older daemons that cannot transfer sessions.
    #[cfg(windows)]
    fn upgrade(&self, outdated: &PtyDaemonManifest) -> bool {
        if !self.exe.is_file() || self.build_id.is_empty() {
            return false;
        }
        let conn = DaemonConn::from(outdated);
        let sessions = match conn.rpc_for_version(
            DaemonRequest::ListTerminals {
                workspace_id: String::new(),
            },
            outdated.version,
        ) {
            Ok(DaemonResponse::TerminalList { sessions }) => sessions,
            result => {
                log::warn!("PTY update could not capture sessions for recovery: {result:?}");
                return false;
            }
        };
        let receipt = self.org_dir.join(UPDATE_SESSIONS_FILE);
        let previous = std::fs::read(&receipt).ok();
        let mut ids = previous
            .as_deref()
            .and_then(|raw| {
                serde_json::from_slice::<std::collections::HashMap<String, String>>(raw).ok()
            })
            .unwrap_or_default();
        ids.extend(
            sessions
                .into_iter()
                .filter(|session| matches!(session.state, TerminalState::Running))
                .map(|session| (session.id, outdated.daemon_id.clone())),
        );
        // Publish before shutdown, so even a final exit frame triggers recovery.
        let temporary = receipt.with_extension(format!("tmp-{}", std::process::id()));
        let saved = serde_json::to_vec(&ids)
            .map_err(std::io::Error::other)
            .and_then(|raw| std::fs::write(&temporary, raw))
            .and_then(|_| std::fs::rename(&temporary, &receipt));
        if let Err(error) = saved {
            log::warn!("PTY update could not save recovery receipt: {error}");
            return false;
        }
        match conn.rpc_for_version(DaemonRequest::Shutdown, outdated.version) {
            Ok(DaemonResponse::Ack) => {}
            result => {
                log::warn!("PTY update shutdown failed: {result:?}");
                // A transport error may mean Shutdown ran but its Ack was lost.
                // Keep the receipt in that case; it only affects ended sessions.
                if matches!(result, Ok(_))
                    || matches!(&result, Err(error) if error == "unauthorized" || error.contains("protocol mismatch"))
                {
                    if let Some(raw) = previous {
                        let _ = std::fs::write(&receipt, raw);
                    } else {
                        let _ = std::fs::remove_file(&receipt);
                    }
                }
                return false;
            }
        }
        log::info!("restarting Windows PTY daemon for update; active sessions will close");
        // Allow normal session and process-tree cleanup to finish.
        let deadline = Instant::now() + Duration::from_secs(10);
        while super::stale::process_may_be_alive(outdated.pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if super::stale::service_may_be_alive(
            outdated.pid,
            outdated.started_at_unix_ms,
            PTY_SERVICE_ARG,
        ) {
            // An authenticated shutdown was accepted, but cleanup stalled.
            // Verify the service identity before stopping only its process.
            super::stale::terminate_outdated(outdated.pid, PTY_SERVICE_ARG);
        }
        if super::stale::process_may_be_alive(outdated.pid) {
            return false;
        }
        if let Some(mut old) = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .child
            .take()
        {
            std::thread::spawn(move || {
                let _ = old.wait();
            });
        }
        emit(
            log::Level::Info,
            "daemon_restarted_for_update",
            serde_json::json!({ "from_pid": outdated.pid }),
        );
        true
    }

    /// Ask an outdated daemon to hand its sessions to a successor running
    /// this build (`PTY_UPGRADE_MAGIC`). True once the successor is up.
    #[cfg(not(windows))]
    fn upgrade(&self, outdated: &PtyDaemonManifest) -> bool {
        let Ok(stream) = Stream::connect(&outdated.address(), Duration::from_secs(2)) else {
            return false;
        };
        // The predecessor waits up to 10 s for its successor to adopt.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let mut request = PTY_UPGRADE_MAGIC.to_vec();
        request.extend_from_slice(outdated.token.as_bytes());
        request.push(b'\n');
        request.extend_from_slice(self.exe.to_string_lossy().as_bytes());
        if super::protocol::write_frame(&mut &stream, &request).is_err() {
            return false;
        }
        let reply = super::protocol::read_frame(&mut &stream).unwrap_or_default();
        if reply == b"EMPTY" {
            let deadline = Instant::now() + Duration::from_secs(5);
            while super::stale::process_may_be_alive(outdated.pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            return !super::stale::process_may_be_alive(outdated.pid);
        }
        if reply != b"OK" {
            emit(
                log::Level::Warn,
                "daemon_upgrade_refused",
                serde_json::json!({ "reply": String::from_utf8_lossy(&reply) }),
            );
            return false;
        }
        // The old daemon now exits on its own. If it was our child, stop
        // tracking it so its exit isn't counted as a crash; reap it quietly.
        if let Some(mut old) = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .child
            .take()
        {
            std::thread::spawn(move || {
                let _ = old.wait();
            });
        }
        emit(
            log::Level::Info,
            "daemon_upgraded",
            serde_json::json!({ "from_pid": outdated.pid }),
        );
        true
    }

    /// Count one crash; idempotent per daemon id. Returns true when the
    /// circuit is (now) open.
    fn account_crash(&self, daemon_id: &str) -> bool {
        let (is_open, newly_open, crashes) = {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            // Repeated watcher observations must not extend the cooldown.
            if inner.circuit_open {
                return true;
            }
            if inner.last_crash_id.as_deref() == Some(daemon_id) {
                return inner.circuit_open;
            }
            inner.last_crash_id = Some(daemon_id.to_string());
            inner.recent_crashes();
            inner.crash_times.push_back(Instant::now());
            let crashes = inner.crash_times.len() as u32;
            let newly_open = crashes > MAX_CRASHES && !inner.circuit_open;
            if crashes > MAX_CRASHES {
                inner.circuit_open = true;
            }
            (inner.circuit_open, newly_open, crashes)
        };
        if newly_open {
            emit(
                log::Level::Warn,
                "circuit_open",
                serde_json::json!({ "crashes": crashes }),
            );
        }
        is_open
    }

    fn backoff(&self) -> Duration {
        if std::env::var_os(TEST_FAST_ENV).is_some() {
            return Duration::from_millis(50);
        }
        let crashes = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .recent_crashes();
        respawn_delay(crashes).unwrap_or(Duration::from_secs(20))
    }

    /// Async respawn for the watcher: sleep the backoff, re-check health
    /// (someone may have recovered), then spawn; spawn failures count
    /// distinctly so startup breakage trips the circuit instead of looping.
    pub fn respawn_later(self: &Arc<Self>) {
        let this = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(this.backoff());
            if this.healthy_conn().is_some() {
                return;
            }
            if this
                .inner
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .recovery_wait()
                .is_some()
            {
                return;
            }
            match this.spawn_locked() {
                Ok(_) => {
                    emit(log::Level::Info, "daemon_respawned", serde_json::json!({}));
                }
                // Waiting on a live daemon is not a failed start.
                Err(error) if error == TERMINAL_SERVICE_NOT_ANSWERING => {}
                Err(error) => {
                    emit(
                        log::Level::Warn,
                        "daemon_respawn_failed",
                        serde_json::json!({ "error": error }),
                    );
                    let crashes = this
                        .inner
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .recent_crashes();
                    this.account_crash(&format!("spawn-failure-{}-{crashes}", now_ms()));
                }
            }
        });
    }

    /// Clear the circuit, stop what we own (or adopted), and ensure fresh.
    pub fn restart(&self) -> Result<DaemonConn, String> {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            inner.crash_times.clear();
            inner.circuit_open = false;
            inner.last_crash_id = None;
        }
        if let Some(manifest) = read_manifest(&self.org_dir) {
            let conn = DaemonConn::from(&manifest);
            if conn.rpc(DaemonRequest::Handoff).is_ok() {
                let deadline = Instant::now() + SPAWN_WAIT;
                while Instant::now() < deadline {
                    if let Some(next) = read_manifest(&self.org_dir) {
                        if next.pid != std::process::id() && hello(&next).is_ok() {
                            return Ok(DaemonConn::from(&next));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                return Err("pty daemon handoff timed out".to_string());
            }
        }
        self.ensure()
    }

    /// Stop the daemon and drop its manifest. Takes our handle OUT first so
    /// the watcher never mistakes a deliberate stop for a crash. Shutdown
    /// RPC first (sessions die openly), 5 s grace, then portable
    /// kill-and-wait escalation (SIGHUP-free: [`Child::kill`] only).
    pub fn stop_owned(&self) {
        let child = self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .child
            .take();
        let manifest = read_manifest(&self.org_dir);
        match child {
            Some(mut child) => {
                let pid = child.id();
                if let Some(ref m) = manifest {
                    let conn = DaemonConn::from(m);
                    let _ = conn.rpc(DaemonRequest::Shutdown);
                }
                let deadline = Instant::now() + STOP_GRACE;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {
                            if Instant::now() >= deadline {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => break,
                    }
                }
                match child.try_wait() {
                    Ok(Some(_)) => {}
                    _ => {
                        emit(
                            log::Level::Warn,
                            "daemon_kill_escalation",
                            serde_json::json!({ "pid": pid }),
                        );
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                remove_manifest_if_pid(&self.org_dir, pid);
                emit(
                    log::Level::Info,
                    "daemon_stopped",
                    serde_json::json!({ "pid": pid }),
                );
            }
            None => {
                // Adopted (or already gone): Shutdown RPC when the manifest
                // answers, then drop the file once the port goes quiet.
                if let Some(manifest) = manifest {
                    let conn = DaemonConn::from(&manifest);
                    let _ = conn.rpc(DaemonRequest::Shutdown);
                    let deadline = Instant::now() + STOP_GRACE;
                    while hello(&manifest).is_ok() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    if hello(&manifest).is_err() {
                        remove_manifest_if_pid(&self.org_dir, manifest.pid);
                        emit(
                            log::Level::Info,
                            "daemon_stopped",
                            serde_json::json!({ "pid": manifest.pid }),
                        );
                    } else {
                        log::warn!(
                            "pty daemon {}:{} did not stop; leaving manifest",
                            manifest.pid,
                            manifest.port
                        );
                    }
                }
            }
        }
    }

    /// Best-effort session close for teardown hooks: ensure + CloseTerminal,
    /// swallowing every error (CloseTerminal itself is idempotent).
    pub fn close_session(&self, session_id: &str) {
        let conn = match self.ensure() {
            Ok(conn) => conn,
            Err(_) => return,
        };
        let _ = conn.rpc(DaemonRequest::CloseTerminal {
            session_id: session_id.to_string(),
        });
    }

    /// Passive snapshot: manifest file + supervisor counters, never spawns.
    pub fn status(&self) -> PtyStatus {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let manifest = read_manifest(&self.org_dir);
        PtyStatus {
            daemon_pid: manifest.as_ref().map(|m| m.pid),
            daemon_id: manifest.map(|m| m.daemon_id),
            owned: inner.child.is_some(),
            circuit_open: inner.circuit_open,
            crash_attempts: inner.crash_times.len() as u32,
        }
    }

    /// [`WATCH_INTERVAL`] watch loop: owned-handle exit (precise `try_wait`, no pid
    /// checks) counts the manifest daemon id and respawns; adopted daemons
    /// are health-checked and respawned after three misses as well. This is
    /// what lets a host recover a PTY daemon that died after the host itself
    /// was restarted.
    pub fn watch(supervisor: Arc<PtySupervisor>, shutdown: Arc<AtomicBool>) {
        let mut strikes = 0u32;
        loop {
            std::thread::sleep(WATCH_INTERVAL);
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            // Retry a deferred compatible update even if the GUI makes no
            // further terminal requests. The spawn lock keeps retries single
            // flight; update refusals never count against the crash budget.
            let retry_update = supervisor
                .update_deferred
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(300));
            if retry_update {
                if let Err(error) = supervisor.ensure() {
                    log::warn!("deferred terminal service update: {error}");
                }
            }
            let owned: Option<bool> = {
                let mut inner = supervisor.inner.lock().unwrap_or_else(|p| p.into_inner());
                match inner.child.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(_)) => {
                            inner.child.take();
                            Some(false)
                        }
                        _ => Some(true),
                    },
                    None => None,
                }
            };
            match owned {
                Some(false) => {
                    strikes = 0;
                    let id = read_manifest(&supervisor.org_dir)
                        .map(|m| m.daemon_id)
                        .unwrap_or_else(|| "watch-unknown".to_string());
                    emit(
                        log::Level::Warn,
                        "daemon_exited",
                        serde_json::json!({ "owned": true }),
                    );
                    supervisor.account_crash(&id);
                    supervisor.respawn_later();
                }
                Some(true) => {
                    strikes = 0;
                    if let Some(manifest) = read_manifest(&supervisor.org_dir) {
                        if hello(&manifest).is_err() {
                            emit(
                                log::Level::Warn,
                                "daemon_unhealthy",
                                serde_json::json!({ "owned": true }),
                            );
                        }
                    }
                }
                None => match read_manifest(&supervisor.org_dir) {
                    Some(manifest) => {
                        if manifest.version != DAEMON_PROTOCOL_VERSION
                            && super::stale::service_may_be_alive(
                                manifest.pid,
                                manifest.started_at_unix_ms,
                                PTY_SERVICE_ARG,
                            )
                        {
                            // A live incompatible service is an update pending,
                            // not a crash. Preserve it until safe handoff succeeds.
                            strikes = 0;
                            let first_attempt = supervisor
                                .update_deferred
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .is_none();
                            if first_attempt {
                                if let Err(error) = supervisor.ensure() {
                                    log::warn!("terminal service update pending: {error}");
                                }
                            }
                            continue;
                        }
                        if hello(&manifest).is_ok() {
                            strikes = 0;
                        } else {
                            strikes += 1;
                            if strikes >= WATCH_STRIKES {
                                strikes = 0;
                                emit(
                                    log::Level::Warn,
                                    "daemon_unhealthy",
                                    serde_json::json!({ "owned": false }),
                                );
                                supervisor.account_crash(&manifest.daemon_id);
                                supervisor.respawn_later();
                            }
                        }
                    }
                    None => {
                        let ever_up = supervisor
                            .inner
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .ever_up;
                        if !ever_up {
                            // Lazy state: no daemon ever ran — nothing to watch.
                            strikes = 0;
                        } else if {
                            strikes += 1;
                            strikes >= WATCH_STRIKES
                        } {
                            strikes = 0;
                            supervisor.stop_owned();
                            supervisor.account_crash(&format!("watch-vanished-{}", now_ms()));
                            supervisor.respawn_later();
                        }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_budget_is_a_sliding_window() {
        let mut inner = Inner {
            child: None,
            crash_times: std::collections::VecDeque::new(),
            circuit_open: false,
            last_crash_id: None,
            ever_up: true,
            unresponsive: None,
        };
        let now = Instant::now();
        // Two crashes long ago, one just now: only the recent one counts.
        inner
            .crash_times
            .push_back(now - CRASH_WINDOW - Duration::from_secs(5));
        inner
            .crash_times
            .push_back(now - CRASH_WINDOW - Duration::from_secs(1));
        inner.crash_times.push_back(now);
        assert_eq!(inner.recent_crashes(), 1);
    }

    #[test]
    fn only_a_later_build_counts_as_newer() {
        assert!(build_is_newer("100-2000", "100-1000"));
        assert!(!build_is_newer("100-1000", "100-2000"));
        assert!(!build_is_newer("100-1000", "200-1000"));
        assert!(!build_is_newer("", "100-1000"));
        assert!(!build_is_newer("100-1000", "not-a-time"));
    }

    #[test]
    fn backoff_table() {
        assert_eq!(respawn_delay(1), Some(Duration::from_millis(200)));
        assert_eq!(respawn_delay(2), Some(Duration::from_secs(1)));
        assert_eq!(respawn_delay(3), Some(Duration::from_secs(5)));
        assert_eq!(respawn_delay(MAX_CRASHES + 1), None);
        assert_eq!(respawn_delay(100), None);
    }

    #[test]
    fn manifest_round_trip_and_remove_if_owned() {
        let dir =
            std::env::temp_dir().join(format!("dragent-pty-manifest-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let manifest = PtyDaemonManifest {
            version: DAEMON_PROTOCOL_VERSION,
            build_id: crate::build_identity::current().to_owned(),
            pid: 12345,
            port: 47999,
            socket: Some("/tmp/pty.sock".to_string()),
            token: "tok".to_string(),
            daemon_id: "d1".to_string(),
            started_at_unix_ms: 7,
        };
        write_manifest(&dir, &manifest).expect("write");
        let back = read_manifest(&dir).expect("read");
        assert_eq!(back.daemon_id, "d1");
        assert_eq!(back.pid, 12345);
        assert_eq!(back.port, 47999);
        assert_eq!(back.token, "tok");
        // Wrong pid must not remove.
        assert!(!remove_manifest_if_pid(&dir, 999));
        assert!(read_manifest(&dir).is_some());
        // Matching pid removes.
        assert!(remove_manifest_if_pid(&dir, 12345));
        assert!(read_manifest(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
