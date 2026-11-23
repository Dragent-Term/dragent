//! Terminal sessions owned per home (Phase 6.2).
//!
//! Each [`TerminalRegistry`] entry is one interactive PTY shell whose output
//! feeds a bounded in-memory replay ring ([`ReplayBuffer`]) and an optional
//! asynchronous history writer. A per-session reaper watches the PTY event stream
//! and flips the record to [`TerminalState::Exited`] when the child dies.
//! Spawns are gated by [`SpawnGate`] (max 3 concurrent, 30 s acquire).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::fanout::{Delivery, Fanout, Subscriber};
use super::history::HistoryStore;
use super::protocol::{
    HandoffSession, HistoryRecord, TerminalSession, TerminalState, WorktreeHome, WorktreeState,
};

/// Byte cap of one session's replay ring.
pub const REPLAY_CAP: usize = 512 * 1024;
/// Max lines any single tail read returns (cap from the bottom).
pub const READ_LINE_CAP: usize = 1000;
/// Max concurrent PTY spawns.
pub const MAX_SPAWNS: usize = 3;
/// How long [`SpawnGate::acquire`] waits for a spawn slot.
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
/// Default dimensions (`cols`, `rows`) when the caller passes zero.
pub const DEFAULT_DIMS: (u16, u16) = (80, 24);
/// Clamp for either dimension.
pub const MAX_DIM: u16 = 1024;

/// Resolve on the machine that owns the PTY. SSH services commonly inherit
/// SHELL=/bin/sh, which has no command lifecycle hooks. Use an installed
/// integrated shell for that generic default instead of creating a terminal
/// whose command blocks can never finish.
pub(crate) fn integrated_shell(preferred: Option<&str>) -> Result<String, String> {
    use dragent_shell_hooks::ShellKind;
    let shell = dragent_pty::resolve_shell(preferred.map(str::to_owned));
    if ShellKind::from_program(&shell) != ShellKind::Other {
        return Ok(shell);
    }
    let name = std::path::Path::new(&shell).file_name().and_then(|s| s.to_str());
    if matches!(name, Some("sh" | "dash" | "ash")) {
        if let Some(choice) = dragent_pty::available_shells().iter()
            .find(|choice| ShellKind::from_program(&choice.path) != ShellKind::Other)
        {
            return Ok(choice.path.clone());
        }
    }
    Err(format!("Shell {shell} cannot report command completion. Install or select Bash, Zsh, Fish, PowerShell, or Nushell on this machine."))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Zero means "default"; anything above [`MAX_DIM`] clamps.
pub fn normalize_dims(cols: u16, rows: u16) -> (u16, u16) {
    let cols = if cols == 0 {
        DEFAULT_DIMS.0
    } else {
        cols.min(MAX_DIM)
    };
    let rows = if rows == 0 {
        DEFAULT_DIMS.1
    } else {
        rows.min(MAX_DIM)
    };
    (cols, rows)
}

// ── ReplayBuffer ──

/// Bounded FIFO of raw PTY bytes with absolute offsets. `start` is the
/// absolute offset of `buf[0]`; `total() = start + len()` is the absolute
/// offset one past the end. Turnover drops from the front and advances
/// `start`, so stale `read_since` offsets demonstrably jump forward.
#[derive(Debug, Default)]
pub struct ReplayBuffer {
    buf: VecDeque<u8>,
    start: u64,
}

impl ReplayBuffer {
    /// A ring resumed from a predecessor's snapshot: `bytes` start at
    /// absolute offset `start`, so offsets stay continuous across a handoff.
    pub fn resumed(start: u64, bytes: &[u8]) -> Self {
        let mut ring = ReplayBuffer {
            buf: VecDeque::new(),
            start,
        };
        ring.push(bytes);
        ring
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend(bytes.iter().copied());
        let overflow = self.buf.len().saturating_sub(REPLAY_CAP);
        if overflow > 0 {
            self.buf.drain(..overflow);
            self.start += overflow as u64;
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Absolute offset of `buf[0]` (bytes dropped to turnover so far).
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Absolute offset one past the newest byte.
    pub fn total(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// Last `max_lines` (default: all, capped at [`READ_LINE_CAP`]) lines of
    /// output. Truncation happens on raw bytes first, so UTF-8 lossiness can
    /// only affect the cut boundary — interior text stays byte-faithful.
    pub fn tail_text(&self, max_lines: Option<usize>) -> String {
        let want = max_lines.unwrap_or(READ_LINE_CAP).min(READ_LINE_CAP);
        if want == 0 || self.buf.is_empty() {
            return String::new();
        }
        let bytes = self.snapshot();
        // Start offset of every line; a trailing '\n' opens a phantom empty
        // tail entry that `tail -n` semantics ignore.
        let mut starts = vec![0usize];
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                starts.push(i + 1);
            }
        }
        let n_lines = if bytes.ends_with(b"\n") {
            starts.len() - 1
        } else {
            starts.len()
        };
        let cut = if n_lines <= want {
            0
        } else {
            starts[n_lines - want]
        };
        String::from_utf8_lossy(&bytes[cut..]).into_owned()
    }

    /// Absolute-offset slice: returns `(data, base, total)` where `base` is
    /// the absolute offset of `data[0]`. When turnover has dropped the
    /// requested `offset`, `base` jumps forward past the request (`base ==
    /// start > offset`) instead of erroring.
    pub fn read_since(&self, offset: u64, max_bytes: usize) -> (Vec<u8>, u64, u64) {
        let total = self.total();
        let base = offset.max(self.start).min(total);
        let available = (total - base) as usize;
        let n = available.min(max_bytes);
        let skip = (base - self.start) as usize;
        let data = self.buf.iter().skip(skip).take(n).copied().collect();
        (data, base, total)
    }
}

// ── SpawnGate ──

/// Admission control for PTY spawns: at most [`MAX_SPAWNS`] concurrent.
pub struct SpawnGate {
    active: Mutex<usize>,
    cvar: Condvar,
}

impl Default for SpawnGate {
    fn default() -> Self {
        Self {
            active: Mutex::new(0),
            cvar: Condvar::new(),
        }
    }
}

/// Held while a PTY spawn is in flight; releases the slot on drop.
pub struct SpawnPermit<'a> {
    gate: &'a SpawnGate,
}

impl Drop for SpawnPermit<'_> {
    fn drop(&mut self) {
        let mut active = self.gate.active.lock().unwrap_or_else(|p| p.into_inner());
        *active = active.saturating_sub(1);
        self.gate.cvar.notify_one();
    }
}

impl SpawnGate {
    pub fn acquire(&self) -> Result<SpawnPermit<'_>, String> {
        self.acquire_with(ACQUIRE_TIMEOUT)
    }

    pub fn acquire_with(&self, timeout: Duration) -> Result<SpawnPermit<'_>, String> {
        let mut active = self.active.lock().unwrap_or_else(|p| p.into_inner());
        if *active >= MAX_SPAWNS {
            let (guard, res) = self
                .cvar
                .wait_timeout(active, timeout)
                .unwrap_or_else(|p| p.into_inner());
            active = guard;
            if *active >= MAX_SPAWNS {
                if res.timed_out() {
                    return Err(format!(
                        "terminal spawn gate timeout ({} in flight)",
                        *active
                    ));
                }
                return Err(format!("terminal spawn gate busy ({} in flight)", *active));
            }
        }
        *active += 1;
        Ok(SpawnPermit { gate: self })
    }

    #[cfg(test)]
    fn in_flight(&self) -> usize {
        *self.active.lock().unwrap_or_else(|p| p.into_inner())
    }
}

// ── LiveSession + TerminalRegistry ──

/// Replies can arrive before spawn returns the writer. Keep that short startup
/// queue bounded; the feed holds only a Weak reference to avoid a PTY cycle.
#[derive(Default)]
struct QueryReplies {
    writer: Option<dragent_pty::PtySessionHandle>,
    pending: Vec<u8>,
}

impl QueryReplies {
    fn send(&mut self, replies: Vec<Vec<u8>>) {
        for bytes in replies {
            if let Some(writer) = &self.writer {
                if let Err(error) = writer.write(&bytes) {
                    log::debug!("terminal query reply failed: {error:#}");
                }
            } else if self.pending.len() + bytes.len() <= 64 * 1024 {
                self.pending.extend_from_slice(&bytes);
            } else {
                log::warn!("terminal startup query reply buffer exhausted");
            }
        }
    }

    fn bind(&mut self, writer: dragent_pty::PtySessionHandle) {
        if !self.pending.is_empty() {
            if let Err(error) = writer.write(&self.pending) {
                log::debug!("terminal startup query reply failed: {error:#}");
            }
            self.pending.clear();
        }
        self.writer = Some(writer);
    }
}

/// Input for a shell that is still starting. A slow shell (Git Bash under
/// emulation can take most of a minute) must not have its initial command or
/// the user's input written before its integration has loaded, nor be killed
/// for being slow: input waits here until the first prompt, then is written
/// in order.
struct Startup {
    ready: bool,
    pending: Vec<u8>,
}

impl Startup {
    #[cfg(unix)]
    fn ready() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { ready: true, pending: Vec::new() }))
    }
}

/// The most input held for one starting shell.
const STARTUP_INPUT_LIMIT: usize = 256 * 1024;

/// Mutable half of a session: the PTY handle (until exit) plus the replay ring.
/// A session's output plumbing before its PTY exists.
struct Plumbing {
    replies: Arc<Mutex<QueryReplies>>,
    ring: Arc<Mutex<ReplayBuffer>>,
    screen: Arc<Mutex<super::terminal_screen::SessionScreen>>,
    fanout: Arc<Fanout>,
    feed: dragent_pty::FeedCallback,
    sync_len: dragent_pty::SyncBufferLenCallback,
    wakeup: dragent_pty::WakeupCallback,
}

pub struct LiveSession {
    _replies: Arc<Mutex<QueryReplies>>,
    startup: Arc<Mutex<Startup>>,
    handle: Option<dragent_pty::PtySessionHandle>,
    buffer: Arc<Mutex<ReplayBuffer>>,
    screen: Arc<Mutex<super::terminal_screen::SessionScreen>>,
    /// Push delivery to streaming subscribers (with flow control).
    fanout: Arc<Fanout>,
}

/// Host-owned interactive PTY sessions, keyed by `term-<uuid>` id.
/// Interior-mutability lets the host server share one registry behind a mutex.
pub struct TerminalRegistry {
    sessions: Arc<Mutex<HashMap<String, (TerminalSession, Arc<Mutex<LiveSession>>)>>>,
    spawn_gate: SpawnGate,
    history: Option<HistoryStore>,
}

impl Default for TerminalRegistry {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            spawn_gate: SpawnGate::default(),
            history: None,
        }
    }
}

impl TerminalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with per-session disk history (cold restore). Plain [`new`]
    /// keeps working without history.
    pub fn new_with_history(history: HistoryStore) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            spawn_gate: SpawnGate::default(),
            history: Some(history),
        }
    }

    /// Disk tail + meta for one dead-or-live session. `None` without history
    /// or when the session never wrote meta.
    pub fn read_history(
        &self,
        session_id: &str,
        max_bytes: usize,
    ) -> Option<(Vec<u8>, HistoryRecord)> {
        self.history.as_ref()?.read_tail(session_id, max_bytes)
    }

    /// All disk tombstones, oldest-first. Empty without history.
    pub fn list_history(&self) -> Vec<HistoryRecord> {
        self.history.as_ref().map(|h| h.list()).unwrap_or_default()
    }

    /// Purge every tombstone of one home. Returns how many were removed.
    pub fn purge_home_history(&self, home_id: &str) -> usize {
        self.history
            .as_ref()
            .map(|h| h.purge_home(home_id))
            .unwrap_or(0)
    }

    /// Spawn a shell in `home`'s checkout. `cwd` defaults to the home dir and
    /// must resolve inside it; `command` (when given) is written with a
    /// trailing newline once the shell is up.
    pub fn create(
        &self,
        home: &WorktreeHome,
        workspace_id: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalSession, String> {
        self.create_with_shell(home, workspace_id, command, cwd, cols, rows, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_with_shell(
        &self,
        home: &WorktreeHome,
        workspace_id: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        shell: Option<&str>,
    ) -> Result<TerminalSession, String> {
        if !matches!(home.state, WorktreeState::Live) || home.workspace_id != workspace_id {
            return Err(format!("unknown worktree {}", home.id));
        }
        let home_path = PathBuf::from(&home.path)
            .canonicalize()
            .map_err(|e| format!("home checkout unavailable: {e}"))?;
        let requested = match cwd {
            Some(dir) => {
                let raw = PathBuf::from(dir);
                if raw.is_absolute() {
                    raw
                } else {
                    home_path.join(raw)
                }
            }
            None => home_path.clone(),
        };
        let cwd_path = requested
            .canonicalize()
            .map_err(|e| format!("cwd unavailable: {e}"))?;
        if !cwd_path.starts_with(&home_path) {
            return Err(format!(
                "cwd must stay inside home checkout ({} is outside {})",
                cwd_path.display(),
                home_path.display()
            ));
        }
        let (cols, rows) = normalize_dims(cols, rows);

        let _permit = self.spawn_gate.acquire()?;
        // Mint the id before spawn so the feed closure attributes bytes.
        let id = format!("term-{}", uuid::Uuid::new_v4());
        let plumbing = self.plumbing(
            &id,
            ReplayBuffer::default(),
            rows,
            cols,
            cwd_path.to_string_lossy().into_owned(),
        );
        // Integration belongs to the shell's lifetime, not to an attached UI.
        // Resolve/install before spawning so failures cannot leave a live PTY
        // that never reports command completion.
        let resolved_shell = integrated_shell(shell)?;
        let shell_kind = dragent_shell_hooks::ShellKind::from_program(&resolved_shell);
        let bootstrap = dragent_shell_hooks::install_bootstrap(shell_kind)
            .map_err(|e| format!("shell integration installation failed: {e:#}"))?
            .map(|path| dragent_shell_hooks::source_command(shell_kind, &path));
        let has_bootstrap = bootstrap.is_some();
        let integration = {
            let mut bootstrap = bootstrap.unwrap_or_default();
            // Agent helpers inherit the exact daemon transport identity. This
            // lets crash recovery adopt a PTY created during an interrupted
            // recovery, even before the GUI saved its replacement binding.
            let quote = |s: &str| crate::agent_sessions::quote(s, &resolved_shell);
            let vars = match shell_kind {
                dragent_shell_hooks::ShellKind::PowerShell => format!(
                    "$env:DRAGENT_HOST_WORKSPACE_ID={}; $env:DRAGENT_HOST_TERMINAL_ID={}; ", quote(workspace_id), quote(&id)),
                dragent_shell_hooks::ShellKind::Fish => format!(
                    "set -gx DRAGENT_HOST_WORKSPACE_ID {}; set -gx DRAGENT_HOST_TERMINAL_ID {}; ", quote(workspace_id), quote(&id)),
                dragent_shell_hooks::ShellKind::Nu => format!(
                    "$env.DRAGENT_HOST_WORKSPACE_ID = {}; $env.DRAGENT_HOST_TERMINAL_ID = {}; ", quote(workspace_id), quote(&id)),
                _ => format!("export DRAGENT_HOST_WORKSPACE_ID={} DRAGENT_HOST_TERMINAL_ID={}; ", quote(workspace_id), quote(&id)),
            };
            bootstrap.insert_str(0, &vars);
            // The organisation's `dragent` launcher first on PATH, so
            // `dragent …` in this terminal acts in this organisation.
            if let Some(path) = launcher_path_entry(shell_kind, &quote) {
                bootstrap.insert_str(0, &path);
            }
            // Programs that open a page through $BROWSER open it in
            // Dragent's browser (or the system one, per the user's
            // setting), unless the user chose a browser themselves.
            if let Some(browser) = browser_entry(shell_kind, &quote) {
                bootstrap.insert_str(0, &browser);
            }
            bootstrap
        };
        // PowerShell runs the integration as its startup command instead of
        // typed input: PSReadLine redraws the whole line for each typed
        // character, which for this line costs seconds of startup on a busy
        // machine and shows the line being typed.
        let as_startup_command = has_bootstrap
            && matches!(shell_kind, dragent_shell_hooks::ShellKind::PowerShell);
        let args: Vec<String> = if as_startup_command {
            vec!["-NoExit".into(), "-Command".into(), integration.trim().to_string()]
        } else {
            Vec::new()
        };
        if has_bootstrap {
            // Before the shell exists, so no prompt can beat it.
            plumbing
                .screen
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .mark_starting(resolved_shell.clone());
        }
        let config = dragent_pty::PtyConfig {
            shell: Some(resolved_shell.clone()),
            cwd: Some(cwd_path.clone()),
            rows,
            cols,
        };
        let (handle, event_rx) = dragent_pty::spawn_interactive_with_args(
            config,
            &args,
            plumbing.feed,
            plumbing.sync_len,
            plumbing.wakeup,
        )
        .map_err(|e| format!("pty spawn failed: {e:#}"))?;
        plumbing.replies.lock().unwrap_or_else(|p| p.into_inner()).bind(handle.clone());
        if !as_startup_command {
            if let Err(error) = handle.write(format!("{integration}\r").as_bytes()) {
                let _ = handle.shutdown();
                return Err(format!("shell integration bootstrap failed: {error:#}"));
            }
        }
        drop(_permit);

        // A successful bootstrap write does not prove the shell sourced its
        // hooks, so the initial command (and any input) waits for the first
        // prompt marker instead of being written into a half-started shell.
        let startup = Arc::new(Mutex::new(Startup {
            ready: !has_bootstrap,
            pending: Vec::new(),
        }));
        if let Some(cmd) = command {
            let mut line = cmd.to_string();
            line.push('\n');
            if has_bootstrap {
                startup
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .pending
                    .extend_from_slice(line.as_bytes());
            } else {
                handle
                    .write(line.as_bytes())
                    .map_err(|e| format!("initial command write failed: {e:#}"))?;
            }
        }
        let watcher = has_bootstrap.then(|| (plumbing.screen.clone(), handle.clone()));

        let record = TerminalSession {
            id: id.clone(),
            workspace_id: workspace_id.to_string(),
            worktree_id: home.id.clone(),
            cwd: cwd_path.to_string_lossy().to_string(),
            cols,
            rows,
            state: TerminalState::Running,
            created_at_unix_ms: now_ms(),
        };
        if let Some(store) = self.history.as_ref() {
            store.write_meta(&HistoryRecord {
                session_id: record.id.clone(),
                workspace_id: record.workspace_id.clone(),
                home_id: record.worktree_id.clone(),
                cols: record.cols,
                rows: record.rows,
                cwd: record.cwd.clone(),
                started_at_unix_ms: record.created_at_unix_ms,
                ended_at_unix_ms: None,
                exit_code: None,
            });
        }
        self.install(
            record.clone(),
            plumbing.ring,
            plumbing.fanout,
            plumbing.screen,
            plumbing.replies,
            startup.clone(),
            handle,
            event_rx,
        );
        if let Some((screen, handle)) = watcher {
            self.release_when_ready(id, resolved_shell, screen, startup, handle);
        }
        Ok(record)
    }

    /// Deliver a starting shell's held input once its first prompt arrives.
    /// There is no deadline: a slow shell is still a working shell, and the
    /// user can close the terminal. The watcher ends with the session.
    fn release_when_ready(
        &self,
        id: String,
        shell: String,
        screen: Arc<Mutex<super::terminal_screen::SessionScreen>>,
        startup: Arc<Mutex<Startup>>,
        handle: dragent_pty::PtySessionHandle,
    ) {
        let sessions = self.sessions.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut reported_slow = false;
            loop {
                if screen.lock().unwrap_or_else(|p| p.into_inner()).shell_ready() {
                    let mut startup = startup.lock().unwrap_or_else(|p| p.into_inner());
                    if !startup.pending.is_empty() {
                        if let Err(error) = handle.write(&startup.pending) {
                            log::warn!("[terminal] {id}: delivering held input failed: {error:#}");
                        }
                        startup.pending.clear();
                    }
                    startup.ready = true;
                    if reported_slow {
                        log::info!("[terminal] {id}: {shell} ready after {:?}", started.elapsed());
                    }
                    return;
                }
                if !sessions.lock().unwrap_or_else(|p| p.into_inner()).contains_key(&id) {
                    return;
                }
                if !reported_slow && started.elapsed() >= Duration::from_secs(10) {
                    reported_slow = true;
                    log::info!("[terminal] {id}: {shell} is still starting after 10s");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
    }

    /// Ring, fan-out and the PTY callbacks that feed them, for session `id`.
    fn plumbing(
        &self,
        id: &str,
        ring: ReplayBuffer,
        rows: u16,
        cols: u16,
        cwd: String,
    ) -> Plumbing {
        let mut screen = super::terminal_screen::SessionScreen::new(rows, cols, cwd, ring.start());
        for chunk in ring.snapshot().chunks(4096) {
            screen.feed(chunk);
            // Historical output rebuilds state, never sends old replies.
            screen.take_replies();
        }
        let replies = Arc::new(Mutex::new(QueryReplies::default()));
        let feed_replies = Arc::downgrade(&replies);
        let screen = Arc::new(Mutex::new(screen));
        let feed_screen = screen.clone();
        let ring: Arc<Mutex<ReplayBuffer>> = Arc::new(Mutex::new(ring));
        let fanout = Arc::new(Fanout::default());
        let feed_ring = ring.clone();
        let feed_fanout = fanout.clone();
        let feed_history = self
            .history
            .as_ref()
            .and_then(|store| match store.writer(id) {
                Ok(writer) => Some(writer),
                Err(error) => {
                    log::warn!("terminal {id} history unavailable: {error}");
                    None
                }
            });
        let feed: dragent_pty::FeedCallback = Box::new(move |bytes: &[u8]| {
            // Release the parser lock between bounded chunks so snapshots and
            // resizes do not wait behind an arbitrarily large output batch.
            for chunk in bytes.chunks(4096) {
                // Update the screen and publish bytes under the replay lock.
                // A snapshot's offset must never run ahead of published output.
                let mut ring = feed_ring.lock().unwrap_or_else(|p| p.into_inner());
                let (frame, replies) = {
                    let mut screen = feed_screen.lock().unwrap_or_else(|p| p.into_inner());
                    let frame = screen.feed(chunk);
                    (frame, screen.take_replies())
                };
                // This only enqueues writes onto the PTY event loop; it never
                // waits for a UI or a host RPC to answer the foreground app.
                if let Some(sink) = feed_replies.upgrade() {
                    sink.lock().unwrap_or_else(|p| p.into_inner()).send(replies);
                }
                ring.push(chunk);
                feed_fanout.publish_operations(&frame);
            }
            if let Some(writer) = feed_history.as_ref() {
                writer.append(bytes);
            }
            dragent_pty::PtyFeedStats {
                bytes_processed: bytes.len(),
            }
        });
        let len_ring = ring.clone();
        let sync_len: dragent_pty::SyncBufferLenCallback =
            Box::new(move || Some(len_ring.lock().unwrap_or_else(|p| p.into_inner()).len()));
        Plumbing {
            replies,
            screen,
            ring,
            fanout,
            feed,
            sync_len,
            wakeup: Box::new(|| {}),
        }
    }

    /// Register a running session and start its exit reaper.
    fn install(
        &self,
        record: TerminalSession,
        ring: Arc<Mutex<ReplayBuffer>>,
        fanout: Arc<Fanout>,
        screen: Arc<Mutex<super::terminal_screen::SessionScreen>>,
        replies: Arc<Mutex<QueryReplies>>,
        startup: Arc<Mutex<Startup>>,
        handle: dragent_pty::PtySessionHandle,
        event_rx: dragent_pty::Receiver<dragent_pty::PtyEvent>,
    ) {
        let pause_handle = handle.clone();
        fanout.set_pause_hook(Box::new(move |paused| {
            let _ = pause_handle.set_read_paused(paused);
        }));
        let id = record.id.clone();
        let live = Arc::new(Mutex::new(LiveSession {
            _replies: replies,
            startup,
            handle: Some(handle),
            buffer: ring,
            screen,
            fanout,
        }));
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), (record, live));
        // Per-session reaper: owns only the Arc map + id (no raw pointers).
        // On ProcessExited it stamps the disk tombstone, tells subscribers
        // the exit code (their final message), and removes the session from
        // the live registry right away — the history still describes it.
        let owned = self.sessions.clone();
        let reaper_history = self.history.clone();
        std::thread::spawn(move || {
            let mark_exited = |code: i32| {
                let removed = owned.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                if let Some(store) = reaper_history.as_ref() {
                    store.close_session(&id, Some(code));
                }
                if let Some((_, live)) = removed {
                    let mut live = live.lock().unwrap_or_else(|p| p.into_inner());
                    live.handle = None;
                    // Under the ring lock, after any final publish.
                    let _ring = live.buffer.lock().unwrap_or_else(|p| p.into_inner());
                    live.fanout.finish(Delivery::Exit(code));
                }
            };
            loop {
                match event_rx.recv_blocking() {
                    Ok(dragent_pty::PtyEvent::ProcessExited(code)) => {
                        mark_exited(code);
                        break;
                    }
                    Ok(dragent_pty::PtyEvent::Error(_)) => continue,
                    Err(_) => {
                        // Event loop died without an exit report: record a
                        // sentinel so readers stop blocking on a dead shell.
                        mark_exited(-1);
                        break;
                    }
                }
            }
        });
    }

    /// Upgrade handoff, sender side: stop reading every live session (so no
    /// output is consumed here after the snapshot) and describe each one plus
    /// the PTY fd its successor will inherit. Unix only.
    #[cfg(unix)]
    pub fn prepare_handoff(&self) -> Vec<HandoffSession> {
        let lives: Vec<(TerminalSession, Arc<Mutex<LiveSession>>)> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|(record, _)| matches!(record.state, TerminalState::Running))
            .map(|(record, live)| (record.clone(), live.clone()))
            .collect();
        for (_, live) in &lives {
            if let Some(handle) = live
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .handle
                .as_ref()
            {
                let _ = handle.set_read_paused(true);
            }
        }
        // Let a read already in flight on each event loop land in its ring
        // before the rings are copied.
        std::thread::sleep(Duration::from_millis(100));
        lives
            .into_iter()
            .filter_map(|(record, live)| {
                let live = live.lock().unwrap_or_else(|p| p.into_inner());
                let (master_fd, shell_pid) = live.handle.as_ref()?.handoff_parts()?;
                let ring = live.buffer.lock().unwrap_or_else(|p| p.into_inner());
                Some(HandoffSession {
                    id: record.id,
                    workspace_id: record.workspace_id,
                    worktree_id: record.worktree_id,
                    cwd: record.cwd,
                    cols: record.cols,
                    rows: record.rows,
                    created_at_unix_ms: record.created_at_unix_ms,
                    shell_pid,
                    master_fd,
                    ring_start: ring.start(),
                    ring: ring.snapshot(),
                    operations: Some(
                        live.screen
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .operation_checkpoint(),
                    ),
                })
            })
            .collect()
    }

    /// A handoff was abandoned: resume reading every session here.
    pub fn cancel_handoff(&self) {
        for (_, live) in self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
        {
            let live = live.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(handle) = live.handle.as_ref() {
                let _ = handle.set_read_paused(false);
            }
            // Re-apply flow control a subscriber may still need.
            live.fanout.reapply_pause();
        }
    }

    /// Upgrade handoff, receiver side: resume a predecessor's session on
    /// its inherited PTY fd, with the same id, record and output offsets.
    #[cfg(unix)]
    pub fn adopt_session(&self, session: HandoffSession) -> Result<TerminalSession, String> {
        let plumbing = self.plumbing(
            &session.id,
            ReplayBuffer::resumed(session.ring_start, &session.ring),
            session.rows,
            session.cols,
            session.cwd.clone(),
        );
        {
            let mut screen = plumbing.screen.lock().unwrap_or_else(|p| p.into_inner());
            // Historical resize/grid state is still absent from legacy handoff.
            if session.ring_start != 0 { screen.mark_incomplete(); }
            if let Some(checkpoint) = session.operations {
                screen.restore_operations(checkpoint)?;
            }
        }
        // SAFETY: the predecessor passed this fd for this session only.
        let (handle, event_rx) = unsafe {
            dragent_pty::adopt_interactive_paused(
                session.master_fd,
                session.shell_pid,
                plumbing.feed,
                plumbing.sync_len,
                plumbing.wakeup,
            )
        }
        .map_err(|e| format!("adopt {}: {e:#}", session.id))?;
        plumbing.replies.lock().unwrap_or_else(|p| p.into_inner()).bind(handle.clone());
        let record = TerminalSession {
            id: session.id,
            workspace_id: session.workspace_id,
            worktree_id: session.worktree_id,
            cwd: session.cwd,
            cols: session.cols,
            rows: session.rows,
            state: TerminalState::Running,
            created_at_unix_ms: session.created_at_unix_ms,
        };
        self.install(
            record.clone(),
            plumbing.ring,
            plumbing.fanout,
            plumbing.screen,
            plumbing.replies,
            Startup::ready(),
            handle,
            event_rx,
        );
        Ok(record)
    }

    /// Stream a session's output from absolute `offset`: whatever of it is
    /// still in the replay ring first, then live output as it arrives, then
    /// a final exit/close delivery. Workspace-gated like every other access.
    pub fn subscribe(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
    ) -> Result<(Arc<Fanout>, Arc<Subscriber>), String> {
        let (_, live) = self.with_gated(workspace_id, session_id)?;
        let live = live.lock().unwrap_or_else(|p| p.into_inner());
        let ring = live.buffer.lock().unwrap_or_else(|p| p.into_inner());
        let (bytes, base, _) = ring.read_since(offset, REPLAY_CAP);
        let catch_up = (!bytes.is_empty()).then_some(Delivery::Data { base, bytes });
        let subscriber = live.fanout.subscribe(catch_up);
        drop(ring);
        Ok((live.fanout.clone(), subscriber))
    }

    pub fn subscribe_operations(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
    ) -> Result<(Arc<Fanout>, Arc<Subscriber>), String> {
        let (_, live) = self.with_gated(workspace_id, session_id)?;
        let live = live.lock().unwrap_or_else(|p| p.into_inner());
        let _ring = live.buffer.lock().unwrap_or_else(|p| p.into_inner());
        let screen = live.screen.lock().unwrap_or_else(|p| p.into_inner());
        let frames = screen.replay_since(offset)?;
        let subscriber = live.fanout.subscribe_frames(frames, true);
        Ok((live.fanout.clone(), subscriber))
    }

    fn with_gated(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<(TerminalSession, Arc<Mutex<LiveSession>>), String> {
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        match sessions.get(session_id) {
            Some((record, live)) if record.workspace_id == workspace_id => {
                Ok((record.clone(), live.clone()))
            }
            _ => Err(format!("unknown terminal {session_id}")),
        }
    }

    /// Workspace-gated snapshot (unknown ids and foreign-workspace ids both
    /// report "unknown terminal").
    pub fn get(&self, workspace_id: &str, session_id: &str) -> Result<TerminalSession, String> {
        self.with_gated(workspace_id, session_id)
            .map(|(record, _)| record)
    }

    /// Workspace-gated list of live sessions, sorted by id. Exited sessions
    /// remain readable through their tombstone until explicitly closed, but
    /// are no longer advertised as attachable terminals.
    pub fn list(&self, workspace_id: &str) -> Vec<TerminalSession> {
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<TerminalSession> = sessions
            .values()
            .filter(|(record, _)| {
                record.workspace_id == workspace_id
                    && matches!(record.state, TerminalState::Running)
            })
            .map(|(record, _)| record.clone())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Sorted session ids attached to one home (for teardown fan-out).
    pub fn sessions_for_home(&self, home_id: &str) -> Vec<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let mut ids: Vec<String> = sessions
            .values()
            .filter(|(record, _)| record.worktree_id == home_id)
            .map(|(record, _)| record.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Write input bytes; errors when the shell is dead.
    pub fn send(&self, workspace_id: &str, session_id: &str, data: &[u8]) -> Result<(), String> {
        let (_, live) = self.with_gated(workspace_id, session_id)?;
        let live = live.lock().unwrap_or_else(|p| p.into_inner());
        let Some(handle) = live.handle.as_ref() else {
            return Err(format!("terminal {session_id} is not running"));
        };
        // Held across the write so input cannot overtake the startup queue.
        let mut startup = live.startup.lock().unwrap_or_else(|p| p.into_inner());
        if !startup.ready {
            if startup.pending.len() + data.len() > STARTUP_INPUT_LIMIT {
                return Err(format!("terminal {session_id} is still starting"));
            }
            startup.pending.extend_from_slice(data);
            return Ok(());
        }
        handle
            .write(data)
            .map_err(|e| format!("terminal {session_id} write failed: {e:#}"))
    }

    /// Tail text plus liveness: `(text, alive, exit_code)`.
    pub fn read(
        &self,
        workspace_id: &str,
        session_id: &str,
        max_lines: Option<usize>,
    ) -> Result<(String, bool, Option<i32>), String> {
        let (record, live) = self.with_gated(workspace_id, session_id)?;
        let live = live.lock().unwrap_or_else(|p| p.into_inner());
        let text = live
            .buffer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tail_text(max_lines);
        let (alive, exit) = match &record.state {
            TerminalState::Running => (true, None),
            TerminalState::Exited { code } => (false, Some(*code)),
        };
        Ok((text, alive, exit))
    }

    /// Absolute-offset delta: `(data, base, total)`.
    pub fn read_since(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, u64, u64, Option<i32>), String> {
        let (record, live) = self.with_gated(workspace_id, session_id)?;
        let live = live.lock().unwrap_or_else(|p| p.into_inner());
        let (data, base, total) = live
            .buffer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .read_since(offset, max_bytes);
        let exit_code = match record.state {
            TerminalState::Exited { code } => Some(code),
            TerminalState::Running => None,
        };
        Ok((data, base, total, exit_code))
    }

    /// Read a display projection from the daemon, with the same ownership gate
    /// as raw output. Serialization happens after releasing the parser lock.
    pub fn screen(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<super::screen_protocol::TerminalScreen, String> {
        let (_, live) = self.with_gated(workspace_id, session_id)?;
        let (screen, buffer) = {
            let live = live.lock().unwrap_or_else(|p| p.into_inner());
            (live.screen.clone(), live.buffer.clone())
        };
        let _published = buffer.lock().unwrap_or_else(|p| p.into_inner());
        let snapshot = screen.lock().unwrap_or_else(|p| p.into_inner()).snapshot();
        snapshot
    }

    /// Set the colours this terminal answers queries with, telling the running
    /// app if its dark/light scheme flipped.
    pub fn set_colors(
        &self,
        workspace_id: &str,
        session_id: &str,
        colors: crate::screen_protocol::TerminalColors,
    ) -> Result<(), String> {
        let (_, live) = self.with_gated(workspace_id, session_id)?;
        let live = live.lock().unwrap_or_else(|p| p.into_inner());
        let report = live
            .screen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .set_colors(colors);
        if let (Some(report), Some(handle)) = (report, live.handle.as_ref()) {
            handle
                .write(&report)
                .map_err(|e| format!("terminal {session_id} colour report failed: {e:#}"))?;
        }
        Ok(())
    }

    /// Validate dims, resize the live PTY when still running, update the record.
    pub fn resize(
        &self,
        workspace_id: &str,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalSession, String> {
        let (mut record, live) = self.with_gated(workspace_id, session_id)?;
        let (cols, rows) = normalize_dims(cols, rows);
        {
            let live = live.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(handle) = live.handle.as_ref() {
                let mut screen = live.screen.lock().unwrap_or_else(|p| p.into_inner());
                handle
                    .resize(rows, cols)
                    .map_err(|e| format!("terminal {session_id} resize failed: {e:#}"))?;
                screen.resize(rows, cols);
            }
        }
        record.cols = cols;
        record.rows = rows;
        {
            let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((stored, _)) = sessions.get_mut(session_id) {
                stored.cols = cols;
                stored.rows = rows;
            }
        }
        Ok(record)
    }

    /// Remove and shut down; returns the record (None when unknown).
    /// Ungated by workspace: teardown fan-out only has session ids.
    /// Stamps the disk tombstone with the live exit code when known.
    pub fn close(&self, session_id: &str) -> Option<TerminalSession> {
        let removed = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
            sessions.remove(session_id)
        };
        if let Some((record, live)) = removed {
            let exit_code = match &record.state {
                TerminalState::Exited { code } => Some(*code),
                TerminalState::Running => None,
            };
            let live = live.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(handle) = live.handle.as_ref() {
                let _ = handle.shutdown();
            }
            live.fanout.finish(Delivery::Closed);
            if let Some(store) = self.history.as_ref() {
                store.close_session(session_id, exit_code);
            }
            Some(record)
        } else {
            None
        }
    }

    /// Close every session of one home; returns the sorted ids closed.
    pub fn close_for_home(&self, home_id: &str) -> Vec<String> {
        let ids = self.sessions_for_home(home_id);
        for id in &ids {
            self.close(id);
        }
        ids
    }

    pub fn running_count(&self) -> usize {
        self.sessions.lock().unwrap_or_else(|p| p.into_inner()).values()
            .filter(|(record, _)| matches!(record.state, TerminalState::Running)).count()
    }

    /// Close everything; returns how many sessions were dropped.
    pub fn shutdown_all(&self) -> usize {
        let ids: Vec<String> = {
            let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
            sessions.keys().cloned().collect()
        };
        let count = ids.len();
        for id in &ids {
            self.close(id);
        }
        count
    }

    pub fn len(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
}

/// The shell statement putting the organisation's `dragent` launcher first
/// on PATH, when the app has written one (see [`launcher`]).
fn launcher_path_entry(
    shell_kind: dragent_shell_hooks::ShellKind,
    quote: &dyn Fn(&str) -> String,
) -> Option<String> {
    use dragent_shell_hooks::ShellKind;
    let (dir, _) = launcher(shell_kind, "dragent")?;
    let dir = quote(&dir);
    Some(match shell_kind {
        ShellKind::PowerShell => format!("$env:PATH = {dir} + [IO.Path]::PathSeparator + $env:PATH; "),
        ShellKind::Nu => format!("$env.PATH = ($env.PATH | prepend {dir}); "),
        ShellKind::Fish => format!("set -gx PATH {dir} $PATH; "),
        ShellKind::Zsh | ShellKind::Bash => format!("export PATH={dir}:\"$PATH\"; "),
        ShellKind::Other => return None,
    })
}

/// The shell statement pointing `BROWSER` at the organisation's
/// `dragent-open` launcher, when the app has written one and the user's own
/// startup files left `BROWSER` unset.
fn browser_entry(
    shell_kind: dragent_shell_hooks::ShellKind,
    quote: &dyn Fn(&str) -> String,
) -> Option<String> {
    use dragent_shell_hooks::ShellKind;
    let (_, file) = launcher(shell_kind, "dragent-open")?;
    let file = quote(&file);
    Some(match shell_kind {
        ShellKind::PowerShell => format!("if (-not $env:BROWSER) {{ $env:BROWSER = {file} }}; "),
        ShellKind::Nu => format!("$env.BROWSER = ($env.BROWSER? | default {file}); "),
        ShellKind::Fish => format!("set -q BROWSER; or set -gx BROWSER {file}; "),
        ShellKind::Zsh | ShellKind::Bash => format!("[ -n \"$BROWSER\" ] || export BROWSER={file}; "),
        ShellKind::Other => return None,
    })
}

/// The shell statement that makes programs in this shell open pages in the
/// system's own browser instead of Dragent's: `open` on macOS, `xdg-open`
/// on Linux and other Unixes; on Windows `BROWSER` is removed, so programs
/// use the default browser. For the pop-up terminal shown over a page
/// (sign-ins, installers), whose shell keeps it for every later command.
/// `os` is a [`std::env::consts::OS`] value. `None` for a shell Dragent
/// can't write statements for.
pub fn system_browser_entry(
    shell_kind: dragent_shell_hooks::ShellKind,
    os: &str,
) -> Option<String> {
    use dragent_shell_hooks::ShellKind;
    if os == "windows" {
        return Some(match shell_kind {
            ShellKind::PowerShell => {
                "Remove-Item Env:BROWSER -ErrorAction SilentlyContinue; ".into()
            }
            ShellKind::Nu => "hide-env -i BROWSER; ".into(),
            ShellKind::Fish => "set -e BROWSER; ".into(),
            ShellKind::Zsh | ShellKind::Bash => "unset BROWSER; ".into(),
            ShellKind::Other => return None,
        });
    }
    let opener = if os == "macos" { "open" } else { "xdg-open" };
    Some(match shell_kind {
        ShellKind::PowerShell => format!("$env:BROWSER = '{opener}'; "),
        ShellKind::Nu => format!("$env.BROWSER = '{opener}'; "),
        ShellKind::Fish => format!("set -gx BROWSER {opener}; "),
        ShellKind::Zsh | ShellKind::Bash => format!("export BROWSER={opener}; "),
        ShellKind::Other => return None,
    })
}

/// Where the app wrote the launcher `name` for this shell, as the shell
/// writes paths: its folder and the file. Windows shells that run `.cmd`
/// files use `bin`; POSIX shells there (Git Bash) use `bin/posix`, where a
/// `sh` launcher lives. `None` until the app has written it.
fn launcher(shell_kind: dragent_shell_hooks::ShellKind, name: &str) -> Option<(String, String)> {
    use dragent_shell_hooks::ShellKind;
    let bin = dragent_paths::organisation_dir().join("bin");
    let windows_native = cfg!(windows) && matches!(shell_kind, ShellKind::PowerShell | ShellKind::Nu);
    let (dir, file) = if !cfg!(windows) {
        (bin, name.to_string())
    } else if windows_native {
        (bin, format!("{name}.cmd"))
    } else {
        (bin.join("posix"), name.to_string())
    };
    let path = dir.join(&file);
    if !path.is_file() {
        return None;
    }
    let shown = |path: &std::path::Path| {
        if cfg!(windows) && !windows_native {
            dragent_shell_hooks::posix_path(path)
        } else {
            path.to_string_lossy().into_owned()
        }
    };
    Some((shown(&dir), shown(&path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_browser_entry_names_the_platforms_opener_for_each_shell() {
        use dragent_shell_hooks::ShellKind;
        let entry = |kind, os| system_browser_entry(kind, os);
        assert_eq!(entry(ShellKind::Zsh, "macos").as_deref(), Some("export BROWSER=open; "));
        assert_eq!(entry(ShellKind::Bash, "linux").as_deref(), Some("export BROWSER=xdg-open; "));
        assert_eq!(entry(ShellKind::Fish, "macos").as_deref(), Some("set -gx BROWSER open; "));
        assert_eq!(entry(ShellKind::Nu, "linux").as_deref(), Some("$env.BROWSER = 'xdg-open'; "));
        assert_eq!(entry(ShellKind::PowerShell, "macos").as_deref(), Some("$env:BROWSER = 'open'; "));
        // Windows: no BROWSER, so programs open the default browser.
        assert_eq!(
            entry(ShellKind::PowerShell, "windows").as_deref(),
            Some("Remove-Item Env:BROWSER -ErrorAction SilentlyContinue; ")
        );
        assert_eq!(entry(ShellKind::Bash, "windows").as_deref(), Some("unset BROWSER; "));
        assert_eq!(entry(ShellKind::Nu, "windows").as_deref(), Some("hide-env -i BROWSER; "));
        assert_eq!(entry(ShellKind::Other, "macos"), None);
        // Never Dragent's own launcher.
        for os in ["macos", "linux", "windows"] {
            for kind in [ShellKind::Zsh, ShellKind::Bash, ShellKind::Fish, ShellKind::PowerShell, ShellKind::Nu] {
                assert!(!entry(kind, os).unwrap().contains("dragent"));
            }
        }
    }


    #[test]
    fn ring_push_snapshot_fidelity_and_cap() {
        let mut ring = ReplayBuffer::default();
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.snapshot(), b"hello world");
        assert_eq!(ring.len(), 11);
        assert_eq!(ring.total(), 11);
        // Overflow evicts oldest first, total keeps counting.
        let big = vec![b'x'; REPLAY_CAP + 1000];
        ring.push(&big);
        assert_eq!(ring.len(), REPLAY_CAP);
        assert_eq!(ring.start(), 1011);
        assert_eq!(ring.total(), 11 + REPLAY_CAP as u64 + 1000);
        assert!(ring.snapshot().iter().all(|b| *b == b'x'));
    }

    #[test]
    fn tail_text_takes_bottom_lines_capped() {
        let mut ring = ReplayBuffer::default();
        for i in 0..10 {
            ring.push(format!("line-{i:02}\n").as_bytes());
        }
        let tail = ring.tail_text(Some(3));
        assert_eq!(tail, "line-07\nline-08\nline-09\n");
        // None returns everything (capped at READ_LINE_CAP).
        assert_eq!(ring.tail_text(None).lines().count(), 10);
        // Over-large requests clamp, never exceed the cap.
        let mut many = ReplayBuffer::default();
        for i in 0..(READ_LINE_CAP + 50) {
            many.push(format!("l{i}\n").as_bytes());
        }
        assert_eq!(many.tail_text(None).lines().count(), READ_LINE_CAP);
        assert_eq!(
            many.tail_text(Some(usize::MAX)).lines().count(),
            READ_LINE_CAP
        );
        assert_eq!(ring.tail_text(Some(0)), "");
    }

    #[test]
    fn read_since_slices_and_turnover_gap() {
        let mut ring = ReplayBuffer::default();
        ring.push(b"0123456789");
        let (data, base, total) = ring.read_since(0, 4);
        assert_eq!((data.as_slice(), base, total), (b"0123".as_slice(), 0, 10));
        let (data, base, total) = ring.read_since(8, 100);
        assert_eq!((data.as_slice(), base, total), (b"89".as_slice(), 8, 10));
        // Turnover: base jumps past a stale request.
        let big = vec![b'y'; REPLAY_CAP];
        ring.push(&big);
        let start = ring.start();
        assert!(start > 0);
        let (data, base, total) = ring.read_since(0, 10);
        assert_eq!(base, start);
        assert_eq!(data, vec![b'y'; 10]);
        assert_eq!(total, ring.total());
        // Offset past total clamps to total with empty data.
        let (data, base, total) = ring.read_since(u64::MAX, 10);
        assert!(data.is_empty());
        assert_eq!(base, total);
    }

    #[test]
    fn dims_validate_default_and_clamp() {
        assert_eq!(normalize_dims(0, 0), (80, 24));
        assert_eq!(normalize_dims(0, 40), (80, 40));
        assert_eq!(normalize_dims(100, 0), (100, 24));
        assert_eq!(normalize_dims(80, 24), (80, 24));
        assert_eq!(normalize_dims(5000, 9000), (1024, 1024));
        assert_eq!(normalize_dims(1024, 1024), (1024, 1024));
    }

    #[test]
    fn spawn_gate_enforces_max_and_times_out() {
        let gate = SpawnGate::default();
        let _a = gate.acquire().expect("slot 1");
        let _b = gate.acquire().expect("slot 2");
        let _c = gate.acquire().expect("slot 3");
        assert_eq!(gate.in_flight(), 3);
        assert!(gate.acquire_with(Duration::from_millis(50)).is_err());
        drop(_c);
        let _d = gate
            .acquire_with(Duration::from_secs(1))
            .expect("slot freed");
        assert_eq!(gate.in_flight(), 3);
    }
}
