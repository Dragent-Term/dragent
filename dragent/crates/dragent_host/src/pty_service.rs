//! The supervised PTY daemon child process.
//!
//! Runs as `dragent-host --dragent-pty-service` and owns the
//! [`TerminalRegistry`](super::terminals::TerminalRegistry) the host used to
//! hold in-process. One daemon per org dir (stable loopback port with an
//! ephemeral fallback, 0600 manifest via tmp + rename). The host adopts a
//! surviving daemon after its own crash — shells outlive the host by design;
//! a graceful host shutdown stops the daemon via the Shutdown RPC.
//!
//! Framing mirrors the host service: length-prefixed JSON frames over loopback
//! TCP, one connection per unary RPC. Version is checked before the token so stale
//! peers get a mismatch (not an auth) error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;

use super::fanout::Delivery;
use super::protocol::*;
use super::pty_supervisor as sup;
use super::transport::{Listener, Stream};

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

fn generate_daemon_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Daemon {
    token: String,
    daemon_id: String,
    started_at: u64,
    org_dir: PathBuf,
    registry: super::terminals::TerminalRegistry,
    /// Daemon-scoped workspace lookup: daemon RPCs carry session ids only,
    /// while the registry gates by workspace. Filled on create, pruned on
    /// close; the host re-checks snapshots for its own gating.
    workspaces: Mutex<HashMap<String, String>>,
    shutdown: AtomicBool,
    handoff: AtomicBool,
    upgrade_lock: RwLock<()>,
    /// Terminal streams attached right now.
    streams: AtomicUsize,
    /// When a client last asked for anything, in unix ms.
    last_request_ms: AtomicU64,
    listener_fd: i32,
    /// Socket path of a Unix listener (passed to a handoff successor).
    listener_socket: Option<String>,
}

fn reply(stream: &Stream, result: Result<DaemonResponse, String>) {
    let env = DaemonResponseEnvelope {
        version: DAEMON_PROTOCOL_VERSION,
        result,
    };
    let mut out = stream;
    let _ = super::protocol::write_message(&mut out, &env);
}

fn workspace_of(daemon: &Daemon, session_id: &str) -> Result<String, String> {
    daemon
        .workspaces
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(session_id)
        .cloned()
        .ok_or_else(|| format!("unknown terminal {session_id}"))
}

fn handle_request(daemon: &Daemon, request: DaemonRequest) -> Result<DaemonResponse, String> {
    match request {
        DaemonRequest::SubscribeTerminal { .. }
        | DaemonRequest::SubscribeTerminalOperations { .. } => {
            Err("SubscribeTerminal needs its own connection".to_string())
        }
        DaemonRequest::Hello { version } => {
            if !DAEMON_SUPPORTED_VERSIONS.contains(&version) {
                return Err(format!(
                    "daemon protocol mismatch (daemon {}, client {})",
                    DAEMON_PROTOCOL_VERSION, version
                ));
            }
            Ok(DaemonResponse::HelloAck {
                version: DAEMON_PROTOCOL_VERSION,
                daemon_id: daemon.daemon_id.clone(),
                started_at: daemon.started_at,
            })
        }
        DaemonRequest::CreateTerminal {
            home,
            workspace_id,
            command,
            shell,
            cwd,
            cols,
            rows,
        } => match daemon.registry.create_with_shell(
            &home,
            &workspace_id,
            command.as_deref(),
            cwd.as_deref(),
            cols,
            rows,
            shell.as_deref(),
        ) {
            Ok(session) => {
                daemon
                    .workspaces
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(session.id.clone(), session.workspace_id.clone());
                Ok(DaemonResponse::TerminalOpened { session })
            }
            Err(error) => Err(error),
        },
        DaemonRequest::GetTerminal { session_id } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            match daemon.registry.get(&workspace_id, &session_id) {
                Ok(session) => Ok(DaemonResponse::TerminalSnapshot(session)),
                Err(error) => Err(error),
            }
        }
        DaemonRequest::ListTerminals { workspace_id } => {
            if workspace_id.is_empty() {
                // Internal census (host RuntimeState): every live session.
                let ids: Vec<(String, String)> = daemon
                    .workspaces
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .iter()
                    .map(|(id, ws)| (id.clone(), ws.clone()))
                    .collect();
                let mut sessions = Vec::new();
                for (id, ws) in ids {
                    match daemon.registry.get(&ws, &id) {
                        Ok(session) => sessions.push(session),
                        Err(_) => {
                            daemon
                                .workspaces
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .remove(&id);
                        }
                    }
                }
                sessions.sort_by(|a, b| a.id.cmp(&b.id));
                Ok(DaemonResponse::TerminalList { sessions })
            } else {
                Ok(DaemonResponse::TerminalList {
                    sessions: daemon.registry.list(&workspace_id),
                })
            }
        }
        DaemonRequest::ListSessionsForHome { home_id } => Ok(DaemonResponse::SessionIds {
            ids: daemon.registry.sessions_for_home(&home_id),
        }),
        DaemonRequest::SendTerminalInput { session_id, data } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            match daemon.registry.send(&workspace_id, &session_id, &data) {
                Ok(()) => Ok(DaemonResponse::Ack),
                Err(error) => Err(error),
            }
        }
        DaemonRequest::ReadTerminal {
            session_id,
            max_lines,
        } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            match daemon.registry.read(&workspace_id, &session_id, max_lines) {
                Ok((text, alive, exit_code)) => Ok(DaemonResponse::TerminalOutput {
                    session_id,
                    text,
                    alive,
                    exit_code,
                }),
                Err(error) => Err(error),
            }
        }
        DaemonRequest::ReadTerminalScreen { session_id } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            daemon
                .registry
                .screen(&workspace_id, &session_id)
                .map(DaemonResponse::TerminalScreen)
        }
        DaemonRequest::ReadTerminalSince {
            session_id,
            offset,
            max_bytes,
        } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            match daemon
                .registry
                .read_since(&workspace_id, &session_id, offset, max_bytes)
            {
                Ok((data, base, total, exit_code)) => Ok(DaemonResponse::TerminalData {
                    session_id,
                    base_offset: base,
                    data,
                    total_bytes: total,
                    exit_code,
                }),
                Err(error) => Err(error),
            }
        }
        DaemonRequest::ResizeTerminal {
            session_id,
            cols,
            rows,
        } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            match daemon
                .registry
                .resize(&workspace_id, &session_id, cols, rows)
            {
                Ok(session) => Ok(DaemonResponse::TerminalSnapshot(session)),
                Err(error) => Err(error),
            }
        }
        DaemonRequest::SetTerminalColors { session_id, colors } => {
            let workspace_id = workspace_of(daemon, &session_id)?;
            daemon
                .registry
                .set_colors(&workspace_id, &session_id, colors)?;
            Ok(DaemonResponse::Ack)
        }
        DaemonRequest::CloseTerminal { session_id } => {
            // Idempotent by design: teardown races close what is already
            // gone, and that must still Ack.
            daemon.registry.close(&session_id);
            daemon
                .workspaces
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&session_id);
            Ok(DaemonResponse::Ack)
        }
        DaemonRequest::ReadSessionHistory {
            session_id,
            max_bytes,
        } => match daemon.registry.read_history(&session_id, max_bytes) {
            Some((tail, record)) => Ok(DaemonResponse::SessionHistory { record, tail }),
            None => Err(format!("unknown session history {session_id}")),
        },
        DaemonRequest::ListSessionHistory => Ok(DaemonResponse::SessionHistoryList {
            records: daemon.registry.list_history(),
        }),
        DaemonRequest::PurgeHomeHistory { home_id } => {
            daemon.registry.purge_home_history(&home_id);
            Ok(DaemonResponse::Ack)
        }
        DaemonRequest::Shutdown => {
            daemon.shutdown.store(true, Ordering::SeqCst);
            Ok(DaemonResponse::Ack)
        }
        DaemonRequest::Handoff => {
            #[cfg(unix)]
            {
                let fd = daemon.listener_fd;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let token = daemon.token.clone();
                let fd_text = fd.to_string();
                use std::os::unix::process::CommandExt;
                let mut command = std::process::Command::new(exe);
                command
                    .arg(PTY_SERVICE_ARG)
                    .env(PTY_TOKEN_ENV, token)
                    .env(HANDOFF_FD_ENV, &fd_text);
                match daemon.listener_socket.as_deref() {
                    Some(path) => command.env(HANDOFF_SOCKET_ENV, path),
                    None => command.env_remove(HANDOFF_SOCKET_ENV),
                };
                unsafe {
                    command.pre_exec(move || {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags >= 0 {
                            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                        }
                        Ok(())
                    });
                }
                command.spawn().map_err(|e| format!("handoff spawn: {e}"))?;
                daemon.handoff.store(true, Ordering::SeqCst);
                daemon.shutdown.store(true, Ordering::SeqCst);
                Ok(DaemonResponse::Ack)
            }
            #[cfg(not(unix))]
            {
                Err("listener handoff is unavailable on this platform".to_string())
            }
        }
    }
}

fn handle_connection(stream: Stream, daemon: Arc<Daemon>) {
    // The listener is non-blocking and macOS/BSD accepted sockets inherit
    // that: writes would fail with EAGAIN instead of honouring the timeouts
    // below as soon as a peer's buffer fills (long streams).
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let payload = match super::protocol::read_frame(&mut &stream) {
        Ok(payload) => payload,
        Err(_) => return,
    };
    // The upgrade request predates version negotiation on purpose: a daemon
    // from an older build must still understand it.
    if let Some(rest) = payload.strip_prefix(PTY_UPGRADE_MAGIC) {
        handle_upgrade(&stream, &daemon, rest);
        return;
    }
    let envelope: DaemonRequestEnvelope = match super::protocol::decode_message(&payload) {
        Ok(envelope) => envelope,
        Err(_) => return,
    };
    // Version before token: stale peers deserve a mismatch, not an auth error.
    if !DAEMON_SUPPORTED_VERSIONS.contains(&envelope.version) {
        reply(
            &stream,
            Err(format!(
                "daemon protocol mismatch (daemon {}, client {})",
                DAEMON_PROTOCOL_VERSION, envelope.version
            )),
        );
        return;
    }
    if !super::protocol::tokens_equal(&envelope.token, &daemon.token) {
        reply(&stream, Err("unauthorized".to_string()));
        return;
    }
    daemon.last_request_ms.store(now_ms(), Ordering::Relaxed);
    if let DaemonRequest::SubscribeTerminal { session_id, offset } = envelope.request {
        let _attached = AttachedStream::new(&daemon.streams);
        stream_terminal(&stream, &daemon, &session_id, offset, false);
        return;
    }
    if let DaemonRequest::SubscribeTerminalOperations { session_id, offset } = envelope.request {
        let _attached = AttachedStream::new(&daemon.streams);
        stream_terminal(&stream, &daemon, &session_id, offset, true);
        return;
    }
    // Readiness probes must remain available while the predecessor waits for
    // its successor: both briefly share the inherited listening descriptor.
    if matches!(envelope.request, DaemonRequest::Hello { .. }) {
        reply(&stream, handle_request(&daemon, envelope.request));
        return;
    }
    // Unary operations share the gate; only an upgrade takes it exclusively.
    // Creates and input writes cannot cross the ownership transfer boundary.
    let _operation = daemon
        .upgrade_lock
        .read()
        .unwrap_or_else(|p| p.into_inner());
    if daemon.handoff.load(Ordering::SeqCst) {
        reply(&stream, Err(PTY_UPDATE_RETRY.to_owned()));
        return;
    }
    reply(&stream, handle_request(&daemon, envelope.request));
}

fn reply_raw(stream: &Stream, text: &str) {
    let mut out = stream;
    let _ = super::protocol::write_frame(&mut out, text.as_bytes());
}

/// `PTY_UPGRADE_MAGIC` request: hand every live session and the listener to
/// a successor running `exe`, then stop without closing the shells.
fn handle_upgrade(stream: &Stream, daemon: &Daemon, rest: &[u8]) {
    let _upgrade = daemon
        .upgrade_lock
        .write()
        .unwrap_or_else(|p| p.into_inner());
    if daemon.shutdown.load(Ordering::SeqCst) {
        reply_raw(stream, "ERR update already in progress");
        return;
    }
    let text = String::from_utf8_lossy(rest);
    let (token, exe) = text.split_once('\n').unwrap_or((&text, ""));
    if !super::protocol::tokens_equal(token, &daemon.token) {
        reply_raw(stream, "ERR unauthorized");
        return;
    }
    let exe = exe.trim_end_matches('\n');
    if exe.is_empty() {
        reply_raw(stream, "ERR no successor executable");
        return;
    }
    #[cfg(not(unix))]
    {
        // ConPTY handles cannot move to another process. A daemon with no
        // running sessions can safely exit and let the supervisor replace it.
        let sessions = daemon.workspaces.lock().unwrap_or_else(|p| p.into_inner());
        let running = sessions.iter().any(|(id, ws)| {
            daemon
                .registry
                .get(ws, id)
                .is_ok_and(|s| matches!(s.state, super::protocol::TerminalState::Running))
        });
        if !running {
            daemon.shutdown.store(true, Ordering::SeqCst);
            reply_raw(stream, "EMPTY");
            return;
        }
    }
    match upgrade_to(daemon, std::path::Path::new(exe)) {
        Ok(adopted) => {
            log::info!("dragent-pty handed {adopted} session(s) to its successor");
            reply_raw(stream, "OK");
            daemon.handoff.store(true, Ordering::SeqCst);
            daemon.shutdown.store(true, Ordering::SeqCst);
        }
        Err(error) => {
            log::warn!("dragent-pty upgrade handoff failed: {error}");
            reply_raw(stream, &format!("ERR {error}"));
        }
    }
}

/// Snapshot file handed to an upgrade successor (inherited by path).
const HANDOFF_SNAPSHOT_ENV: &str = "DRAGENT_PTY_HANDOFF_SNAPSHOT";

#[cfg(unix)]
fn upgrade_to(daemon: &Daemon, exe: &std::path::Path) -> Result<usize, String> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;

    let original_manifest = sup::read_manifest(&daemon.org_dir);
    let expected = daemon.registry.running_count();
    let sessions = daemon.registry.prepare_handoff();
    let count = sessions.len();
    if count != expected {
        daemon.registry.cancel_handoff();
        return Err("could not snapshot every running session".into());
    }
    let mut fds: Vec<i32> = sessions.iter().map(|s| s.master_fd).collect();
    fds.push(daemon.listener_fd);
    let snapshot = super::protocol::HandoffSnapshot { sessions };
    let path = daemon
        .org_dir
        .join(format!("pty-handoff-{}.snap", std::process::id()));
    let abandon = |why: String| {
        daemon.registry.cancel_handoff();
        // A failed successor may have published a manifest before failing
        // its readiness probe. Restore the still-running authority as well.
        if let Some(original) = &original_manifest {
            let _ = sup::write_manifest(&daemon.org_dir, original);
        }
        let _ = std::fs::remove_file(&path);
        Err(why)
    };
    let bytes = match super::protocol::encode_message(&snapshot) {
        Ok(bytes) => bytes,
        Err(error) => return abandon(format!("encode snapshot: {error}")),
    };
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, &bytes));
    if let Err(error) = written {
        return abandon(format!("write snapshot: {error}"));
    }

    let mut command = std::process::Command::new(exe);
    command
        .arg(PTY_SERVICE_ARG)
        .arg(dragent_paths::organisation_arg())
        .env(PTY_TOKEN_ENV, &daemon.token)
        .env(HANDOFF_FD_ENV, daemon.listener_fd.to_string())
        .env(HANDOFF_SNAPSHOT_ENV, &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    match daemon.listener_socket.as_deref() {
        Some(socket) => command.env(HANDOFF_SOCKET_ENV, socket),
        None => command.env_remove(HANDOFF_SOCKET_ENV),
    };
    // Only the successor inherits the listener and the PTY masters.
    unsafe {
        command.pre_exec(move || {
            for &fd in &fds {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return abandon(format!("spawn successor: {error}")),
    };
    // Ready = its manifest is written (it writes it after adopting every
    // session) and it answers hello.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(manifest) = sup::read_manifest(&daemon.org_dir) {
            if manifest.pid == child.id() && sup::hello(&manifest).is_ok() {
                // Not ours to reap: it outlives this process.
                std::mem::forget(child);
                return Ok(count);
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return abandon(format!("successor exited during startup: {status}"));
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return abandon("successor did not come up within 10s".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(unix))]
fn upgrade_to(_daemon: &Daemon, _exe: &std::path::Path) -> Result<usize, String> {
    // A ConPTY cannot be passed to another process: Windows upgrades restart
    // the daemon instead.
    Err("session handoff is not supported on this platform".to_string())
}

/// Successor side: adopt the sessions a predecessor handed over.
#[cfg(unix)]
fn adopt_handoff(daemon: &Daemon) -> Result<(), String> {
    let Some(path) = std::env::var_os(HANDOFF_SNAPSHOT_ENV) else {
        return Ok(());
    };
    let snapshot = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            super::protocol::decode_message::<super::protocol::HandoffSnapshot>(&bytes)
                .map_err(|e| e.to_string())
        });
    let _ = std::fs::remove_file(&path);
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Err(format!("handoff snapshot unreadable: {error}"));
        }
    };
    let keep: Vec<i32> = snapshot.sessions.iter().map(|s| s.master_fd).collect();
    let closed = close_inherited_terminals(&keep);
    if closed > 0 {
        log::info!("closed {closed} terminal handle(s) left over from the previous daemon");
    }
    for session in snapshot.sessions {
        let (id, workspace_id) = (session.id.clone(), session.workspace_id.clone());
        match daemon.registry.adopt_session(session) {
            Ok(_) => {
                daemon
                    .workspaces
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(id, workspace_id);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Close every terminal handle this process inherited other than `keep`.
/// A successor needs one handle per session; a predecessor from an older build
/// also passes on its private copies, and nothing here would ever close them.
#[cfg(unix)]
fn close_inherited_terminals(keep: &[i32]) -> usize {
    let open: Vec<i32> = match std::fs::read_dir("/dev/fd") {
        Ok(entries) => entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .collect(),
        Err(_) => return 0,
    };
    let mut closed = 0;
    for fd in open {
        // Only terminals: logging and the listener are not ours to touch.
        if fd > 2 && !keep.contains(&fd) && unsafe { libc::isatty(fd) } == 1 {
            unsafe { libc::close(fd) };
            closed += 1;
        }
    }
    closed
}

/// The default on macOS is 256, which is about thirty terminals: each one
/// holds several handles plus its recording. Running out looks like a dead
/// daemon to everyone trying to connect.
#[cfg(unix)]
fn raise_open_file_limit() {
    const WANTED: libc::rlim_t = 10_240;
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 || limit.rlim_cur >= WANTED {
        return;
    }
    limit.rlim_cur = WANTED.min(limit.rlim_max);
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        log::warn!("could not raise the open file limit: {}", std::io::Error::last_os_error());
    }
}

/// How long a daemon must stay replaced and unused before it closes down.
/// Long enough to outlast a failed upgrade, whose successor may briefly own
/// the manifest before the original restores it.
const RETIRE_AFTER: Duration = Duration::from_secs(300);
const RETIRE_CHECK_EVERY: Duration = Duration::from_secs(30);
/// Test override for [`RETIRE_AFTER`], in milliseconds.
const RETIRE_AFTER_ENV: &str = "DRAGENT_PTY_RETIRE_AFTER_MS";

/// Is another live daemon now the one this org's manifest points clients at?
/// If so, no client that starts from here on can ever find this daemon.
fn replaced_by_another(daemon: &Daemon) -> bool {
    sup::read_manifest(&daemon.org_dir).is_some_and(|manifest| {
        manifest.daemon_id != daemon.daemon_id
            && super::stale::service_may_be_alive(
                manifest.pid,
                manifest.started_at_unix_ms,
                PTY_SERVICE_ARG,
            )
    })
}

/// Close down once nothing can reach this daemon. Its terminals would run on
/// for good otherwise, usually beside a second copy of the same agent that
/// recovery started in the daemon that replaced this one. Anything still
/// attached keeps it alive: an older app can be connected and in use.
fn retire_when_replaced(daemon: Arc<Daemon>) {
    let after = std::env::var(RETIRE_AFTER_ENV)
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(RETIRE_AFTER);
    let every = RETIRE_CHECK_EVERY.min(after / 4).max(Duration::from_millis(50));
    let mut unused_since: Option<std::time::Instant> = None;
    while !daemon.shutdown.load(Ordering::SeqCst) {
        std::thread::sleep(every);
        let last_request = daemon.last_request_ms.load(Ordering::Relaxed);
        let attached = daemon.streams.load(Ordering::Relaxed) > 0
            || now_ms().saturating_sub(last_request) < after.as_millis() as u64;
        if attached || !replaced_by_another(&daemon) {
            unused_since = None;
            continue;
        }
        let since = *unused_since.get_or_insert_with(std::time::Instant::now);
        if since.elapsed() >= after {
            log::warn!(
                "dragent-pty id={} was replaced and has no clients; closing its terminals",
                daemon.daemon_id
            );
            daemon.shutdown.store(true, Ordering::SeqCst);
        }
    }
}

/// Counts one attached terminal stream for as long as it lives.
struct AttachedStream<'a>(&'a AtomicUsize);

impl<'a> AttachedStream<'a> {
    fn new(streams: &'a AtomicUsize) -> Self {
        streams.fetch_add(1, Ordering::Relaxed);
        Self(streams)
    }
}

impl Drop for AttachedStream<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Quiet-stream keepalive interval: an empty frame, so a peer that vanished
/// is noticed (the write fails) even when the shell prints nothing.
const STREAM_KEEPALIVE: Duration = Duration::from_secs(15);

/// Serve one `SubscribeTerminal` connection until the session ends, the peer
/// goes away, or the subscriber is dropped for being stuck.
fn stream_terminal(
    stream: &Stream,
    daemon: &Daemon,
    session_id: &str,
    offset: u64,
    operations: bool,
) {
    // Requests are done; only writes from here on.
    let _ = stream.set_read_timeout(None);
    let subscribed = workspace_of(daemon, session_id).and_then(|workspace_id| {
        if operations {
            daemon
                .registry
                .subscribe_operations(&workspace_id, session_id, offset)
        } else {
            daemon.registry.subscribe(&workspace_id, session_id, offset)
        }
    });
    let (fanout, subscriber) = match subscribed {
        Ok(pair) => pair,
        Err(error) => {
            reply(stream, Err(error));
            return;
        }
    };
    let send = |base: u64, data: Vec<u8>, total: u64, exit_code: Option<i32>| {
        let env = DaemonResponseEnvelope {
            version: DAEMON_PROTOCOL_VERSION,
            result: Ok(DaemonResponse::TerminalData {
                session_id: session_id.to_string(),
                base_offset: base,
                data,
                total_bytes: total,
                exit_code,
            }),
        };
        let mut out = stream;
        match super::protocol::write_message(&mut out, &env) {
            Ok(()) => true,
            Err(error) => {
                log::info!("terminal stream {session_id}: subscriber write failed: {error}");
                false
            }
        }
    };
    let mut total = offset;
    loop {
        let alive = match subscriber.next(STREAM_KEEPALIVE) {
            None => send(total, Vec::new(), total, None),
            Some(Delivery::Data { base, bytes }) => {
                total = base + bytes.len() as u64;
                let sent = send(base, bytes, total, None);
                fanout.on_drained(&subscriber);
                sent
            }
            Some(Delivery::Operations(frame)) => {
                total = frame.end();
                let env = DaemonResponseEnvelope {
                    version: DAEMON_PROTOCOL_VERSION,
                    result: Ok(DaemonResponse::TerminalOperations { frame }),
                };
                let sent = super::protocol::write_message(&mut &*stream, &env).is_ok();
                fanout.on_drained(&subscriber);
                sent
            }
            Some(Delivery::Exit(code)) => {
                let _ = send(total, Vec::new(), total, Some(code));
                false
            }
            Some(Delivery::Closed) => {
                reply(stream, Err(format!("terminal {session_id} closed")));
                false
            }
        };
        if !alive {
            break;
        }
    }
    fanout.unsubscribe(&subscriber);
}

/// Run the PTY daemon: stable-port bind (ephemeral fallback), 0600
/// manifest, loopback-only accept loop until the Shutdown RPC. On exit all
/// sessions shut down and our own manifest is removed (daemon-id match, so
/// a successor's file is never clobbered).
pub fn run_service() -> anyhow::Result<()> {
    #[cfg(unix)]
    raise_open_file_limit();
    // Dev path: no token in env (direct binary run) mints a fresh one.
    let token = std::env::var(PTY_TOKEN_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(generate_token);
    let org_dir = sup::org_dir();
    let stable = pty_stable_port(&org_dir.to_string_lossy());
    // Unix domain socket in the org dir (macOS/Linux), loopback TCP on
    // Windows; a successor started by `Handoff` inherits its predecessor's.
    let listener = match listener_from_handoff() {
        Some(listener) => listener,
        None => Listener::bind_local(Some(&org_dir.join(PTY_SOCKET_FILENAME)), &[stable])
            .map_err(|e| anyhow::anyhow!("pty bind: {e}"))?,
    };
    let port = listener.port();
    let daemon_id = generate_daemon_id();
    let started_at = now_ms();
    let manifest = PtyDaemonManifest {
        version: DAEMON_PROTOCOL_VERSION,
        build_id: super::build_identity::current().to_owned(),
        pid: std::process::id(),
        port,
        socket: listener.socket_path(),
        token: token.clone(),
        daemon_id: daemon_id.clone(),
        started_at_unix_ms: started_at,
    };
    let registry = match super::history::HistoryStore::open(&org_dir) {
        Ok(store) => {
            let purged = store.purge_missing_cwds();
            if purged > 0 {
                log::info!("removed {purged} recordings of terminals in deleted folders");
            }
            super::terminals::TerminalRegistry::new_with_history(store)
        }
        Err(error) => {
            log::warn!("session history unavailable: {error:#}");
            super::terminals::TerminalRegistry::new()
        }
    };
    let daemon = Arc::new(Daemon {
        token,
        daemon_id,
        started_at,
        org_dir: org_dir.clone(),
        registry,
        workspaces: Mutex::new(HashMap::new()),
        shutdown: AtomicBool::new(false),
        handoff: AtomicBool::new(false),
        upgrade_lock: RwLock::new(()),
        streams: AtomicUsize::new(0),
        last_request_ms: AtomicU64::new(started_at),
        listener_fd: {
            #[cfg(unix)]
            {
                listener.raw_fd()
            }
            #[cfg(not(unix))]
            {
                -1
            }
        },
        listener_socket: listener.socket_path(),
    });
    // An upgrade successor adopts its predecessor's sessions first, and only
    // then announces itself: the predecessor treats the manifest + hello as
    // "every shell is safe here".
    #[cfg(unix)]
    if let Err(error) = adopt_handoff(&daemon) {
        // Do not publish readiness or close adopted shells after partial
        // failure: the predecessor still owns every original PTY master.
        log::error!("PTY handoff adoption failed: {error}");
        unsafe { libc::_exit(1) };
    }
    sup::write_manifest(&org_dir, &manifest).map_err(|e| anyhow::anyhow!("pty manifest: {e}"))?;
    // Every inherited reader remained paused during adoption. Only commit
    // reading after all sessions and their screen checkpoints are installed.
    if std::env::var_os(HANDOFF_SNAPSHOT_ENV).is_some() {
        daemon.registry.cancel_handoff();
    }
    log::info!(
        "dragent-pty up id={} pid={} port={port}",
        manifest.daemon_id,
        std::process::id()
    );
    listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    {
        let daemon = daemon.clone();
        std::thread::spawn(move || retire_when_replaced(daemon));
    }
    loop {
        if daemon.shutdown.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                if !stream.is_local_peer() {
                    continue;
                }
                let daemon = daemon.clone();
                std::thread::spawn(move || handle_connection(stream, daemon));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                #[cfg(unix)]
                if listener.wait_readable(Duration::from_millis(100)).is_err() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                #[cfg(not(unix))]
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                if daemon.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                // Out of file handles fails at once on every attempt.
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    // Drain: give in-flight handlers a moment, then dispose everything.
    std::thread::sleep(Duration::from_millis(250));
    let dropped = if daemon.handoff.load(Ordering::SeqCst) {
        0
    } else {
        let dropped = daemon.registry.shutdown_all();
        sup::remove_manifest_if_daemon(&daemon.org_dir, &daemon.daemon_id);
        // The socket path is per org: a replaced daemon would remove the
        // file its replacement is listening on.
        if !sup::read_manifest(&daemon.org_dir).is_some_and(|m| m.daemon_id != daemon.daemon_id) {
            listener.remove_socket_file();
        }
        // Each close hangs up its shell tree on the session's own event-loop
        // thread and escalates to SIGKILL for survivors in the background;
        // let those loops pick up the close, then let the chains finish
        // before this process exits and abandons them.
        if dropped > 0 {
            std::thread::sleep(Duration::from_millis(150));
            if !dragent_pty::wait_for_pending_kills(Duration::from_secs(8)) {
                log::warn!("dragent-pty exiting with kill escalations still running");
            }
        }
        dropped
    };
    log::info!(
        "dragent-pty exiting id={} sessions_dropped={dropped}",
        daemon.daemon_id
    );
    Ok(())
}

const HANDOFF_FD_ENV: &str = "DRAGENT_PTY_LISTENER_FD";
/// Set alongside [`HANDOFF_FD_ENV`] when the inherited listener is a Unix
/// socket (its path); absent for TCP.
const HANDOFF_SOCKET_ENV: &str = "DRAGENT_PTY_LISTENER_SOCKET";
/// Daemon socket file in the org dir.
pub const PTY_SOCKET_FILENAME: &str = "pty.sock";

fn listener_from_handoff() -> Option<Listener> {
    #[cfg(unix)]
    {
        let fd = std::env::var(HANDOFF_FD_ENV).ok()?.parse::<i32>().ok()?;
        let socket = std::env::var(HANDOFF_SOCKET_ENV).ok();
        // SAFETY: the predecessor passed us this listening fd and exits
        // without using it again.
        Some(unsafe { Listener::from_raw_fd(fd, socket.as_deref()) })
    }
    #[cfg(not(unix))]
    {
        None
    }
}
