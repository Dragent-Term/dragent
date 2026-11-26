//! UI-side host connector: lazy spawn-or-connect, RPC, event subscription.
//!
//! `ensure_host` is idempotent and single-flight per process: concurrent
//! callers share one in-flight attempt, and an already-running host is
//! reused (never duplicated).

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::discovery;
use super::protocol::*;

fn client_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{n:x}", std::process::id())
}

static START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static DEFERRED_UPDATE: OnceLock<Mutex<Option<(String, String, Instant)>>> = OnceLock::new();
const UPDATE_RETRY_DELAY: Duration = Duration::from_secs(300);
/// A host this process found healthy (and current) this recently is reused
/// without another ping. Restoring many terminals at launch asks for the host
/// once per terminal; each used to ping it again, one after another.
const HEALTHY_REUSE: Duration = Duration::from_secs(2);
/// Endpoint token of the host last found healthy, the desired build it was
/// healthy for, and when. The shortcut below may only skip the ping when the
/// caller wants that same build: the executable on disk can change under the
/// same path (a rebuild), so the identity — not the path — is remembered.
static LAST_HEALTHY: Mutex<Option<(String, String, Instant)>> = Mutex::new(None);
/// Called once, the first time this process reaches its local host.
static CONNECTED_HOOK: OnceLock<fn()> = OnceLock::new();

/// Register a function to call the first time this process reaches its
/// local host (the app's startup timing uses it). Only the first
/// registration counts.
pub fn on_first_connect(hook: fn()) {
    let _ = CONNECTED_HOOK.set(hook);
}

fn note_healthy(token: &str, desired: &str) {
    if let Ok(mut last) = LAST_HEALTHY.lock() {
        *last = Some((token.to_string(), desired.to_string(), Instant::now()));
    }
    if !CONNECTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        if let Some(hook) = CONNECTED_HOOK.get() {
            hook();
        }
    }
    start_presence_if_due();
}

/// Set once this process reached its host.
static CONNECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start the presence loop once it is both wanted and possible, whichever
/// comes second.
fn start_presence_if_due() {
    #[cfg(not(target_family = "wasm"))]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static STARTED: AtomicBool = AtomicBool::new(false);
        if PRESENCE_WANTED.load(Ordering::SeqCst)
            && CONNECTED.load(Ordering::SeqCst)
            && !STARTED.swap(true, Ordering::SeqCst)
        {
            start_presence();
        }
    }
}

/// Set by the app: hold a presence connection to the host for as long as
/// this process lives (see [`hold_presence`]).
static PRESENCE_WANTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// For the app: keep the local host up for as long as this process runs.
///
/// The host closes after a stretch with no client. Once this process first
/// reaches its host, it holds one event stream open to it; when the stream
/// ends (the host closed, crashed or was replaced by an update) it finds or
/// starts the host again straight away, so an app never waits on a host that
/// went away while it sat idle. Starts nothing before the app's own first
/// contact with the host (launch never waits on it).
pub fn hold_presence() {
    PRESENCE_WANTED.store(true, std::sync::atomic::Ordering::SeqCst);
    start_presence_if_due();
}

/// The presence loop: connect, hold, and on loss reconnect at once (backing
/// off only while the host can't be started).
#[cfg(not(target_family = "wasm"))]
fn start_presence() {
    let _ = std::thread::Builder::new()
        .name("dragent-host-presence".into())
        .spawn(|| {
            let mut backoff = Duration::from_millis(100);
            loop {
                let started = Instant::now();
                let held = HostClient::ensure_host().and_then(|client| client.hold_open());
                match held {
                    // Held until the host went away: look for it again now.
                    Ok(()) if held_for_a_while(started.elapsed()) => {
                        backoff = Duration::from_millis(100)
                    }
                    // A stream that ends as soon as it opens (a host that is
                    // closing, or refuses it) must not be retried flat out.
                    ended => {
                        if let Err(error) = ended {
                            log::debug!("host presence: {error}");
                        }
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(5));
                    }
                }
            }
        });
}

/// A presence stream that lasted this long had really been open.
#[cfg(not(target_family = "wasm"))]
fn held_for_a_while(held: Duration) -> bool {
    held >= Duration::from_secs(1)
}

/// Whether the host with this endpoint token was found healthy just now for
/// this same desired build. A rebuild on disk changes the desired identity,
/// so a cached healthy check for another build must not skip the update.
fn recently_healthy(token: &str, desired: &str, now: Instant) -> bool {
    LAST_HEALTHY.lock().ok().is_some_and(|last| {
        last.as_ref().is_some_and(|(seen, want, at)| {
            seen == token && want == desired && now.saturating_duration_since(*at) < HEALTHY_REUSE
        })
    })
}

/// Forget the cached health of the host with this token (a request to it
/// failed), so the next `ensure_host` pings it again.
pub fn forget_healthy(token: &str) {
    if let Ok(mut last) = LAST_HEALTHY.lock() {
        if last.as_ref().is_some_and(|(seen, _, _)| seen == token) {
            *last = None;
        }
    }
}

fn start_lock() -> &'static Mutex<()> {
    START_LOCK.get_or_init(|| Mutex::new(()))
}

/// A connected UI handle to one host authority.
#[derive(Debug, Clone)]
pub struct HostClient {
    endpoint: ServiceEndpoint,
    client_id: String,
    /// Reconnection must stay in the authority this client was created for,
    /// even if the app has selected another organisation in the meantime.
    local_endpoint: Option<std::path::PathBuf>,
    #[cfg(not(target_family = "wasm"))]
    remote: Option<std::sync::Arc<super::remote::RemoteConnection>>,
}

/// Whether a running host of build `running` should make way for `on_disk`
/// (the executable a new host would start from). A host of the build this
/// process runs (`own`) stays even once the file on disk was rebuilt or
/// replaced: restarting it drops every terminal's stream for a build this
/// process doesn't run. The next launch, running the new build, updates it.
pub(crate) fn host_outdated(running: &str, on_disk: &str, own: &str) -> bool {
    !on_disk.is_empty() && running != on_disk && (own.is_empty() || running != own)
}

impl HostClient {
    #[cfg(not(target_family = "wasm"))]
    pub fn connect_remote(host: super::remote::SshHost) -> Result<Self, String> {
        let remote = super::remote::RemoteConnection::shared(host, None, false)?;
        Ok(Self {
            endpoint: remote.endpoint()?,
            client_id: client_id(),
            local_endpoint: None,
            remote: Some(remote),
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn connect_remote_with_secret(
        host: super::remote::SshHost,
        secret: Option<String>,
        remember: bool,
    ) -> Result<Self, String> {
        let remote = super::remote::RemoteConnection::shared(host, secret, remember)?;
        Ok(Self {
            endpoint: remote.endpoint()?,
            client_id: client_id(),
            local_endpoint: None,
            remote: Some(remote),
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn connect_remote_explicit(
        host: super::remote::SshHost,
        secret: Option<String>,
    ) -> Result<Self, String> {
        let remote = super::remote::RemoteConnection::shared(host, secret, false)?;
        remote.resume();
        Ok(Self {
            endpoint: remote.endpoint()?,
            client_id: client_id(),
            local_endpoint: None,
            remote: Some(remote),
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn remote_connection(&self) -> Option<&std::sync::Arc<super::remote::RemoteConnection>> {
        self.remote.as_ref()
    }

    fn current_endpoint(&self) -> Result<ServiceEndpoint, String> {
        #[cfg(not(target_family = "wasm"))]
        if let Some(remote) = &self.remote {
            return remote.endpoint();
        }
        // Long-lived tabs survive host replacement. Reusing the captured token
        // would retry the predecessor forever, including for input and resize.
        if let Some(path) = &self.local_endpoint {
            let raw = std::fs::read(path).map_err(|e| format!("host endpoint unavailable: {e}"))?;
            return serde_json::from_slice(&raw).map_err(|e| format!("invalid host endpoint: {e}"));
        }
        Ok(self.endpoint.clone())
    }

    pub fn rpc(&self, request: Request) -> Result<Response, String> {
        self.rpc_with_timeout(request, Duration::from_secs(10))
    }

    fn rpc_with_timeout(&self, request: Request, timeout: Duration) -> Result<Response, String> {
        let endpoint = self.current_endpoint()?;
        let result = discovery::rpc_with_timeout(&endpoint, &self.client_id, request, timeout);
        if result.as_ref().is_err_and(|e| e.starts_with("connect:")) {
            // The host is gone: the next `ensure_host` must look again.
            forget_healthy(&endpoint.token);
        }
        result
    }

    pub fn development(
        &self,
        request: super::development::DevelopmentRequest,
    ) -> Result<super::development::DevelopmentResponse, String> {
        match self.rpc_with_timeout(Request::Development(request), Duration::from_secs(15 * 60))? {
            Response::Development(result) => Ok(result),
            other => Err(format!("unexpected development response: {other:?}")),
        }
    }

    pub fn accounts(
        &self,
        request: super::development::AccountsRequest,
    ) -> Result<super::development::AccountsResponse, String> {
        match self.development(super::development::DevelopmentRequest::Accounts(request))? {
            super::development::DevelopmentResponse::Accounts(response) => Ok(response),
            other => Err(format!("unexpected development response: {other:?}")),
        }
    }

    /// A team request; a `Wait` gets its own timeout plus a margin.
    pub fn coordination(
        &self,
        tool: &str,
        arguments: serde_json::Value,
        caller: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match self.rpc_with_timeout(
            Request::Coordination {
                tool: tool.into(),
                arguments,
                caller,
            },
            Duration::from_secs(120),
        )? {
            Response::Coordination(value) => Ok(value),
            other => Err(format!("Unexpected coordination response: {other:?}")),
        }
    }

    pub fn team(
        &self,
        request: super::team::TeamRequest,
    ) -> Result<super::team::TeamResponse, String> {
        let timeout = match &request {
            super::team::TeamRequest::Wait { timeout_ms, .. } => {
                Duration::from_millis((*timeout_ms).min(super::team::MAX_WAIT_MS))
                    + Duration::from_secs(30)
            }
            _ => Duration::from_secs(30),
        };
        self.team_with_timeout(request, timeout)
    }

    /// A team call within the caller's remaining request budget. Companion
    /// reads share one deadline across projects instead of stacking timeouts.
    pub fn team_with_timeout(
        &self,
        request: super::team::TeamRequest,
        timeout: Duration,
    ) -> Result<super::team::TeamResponse, String> {
        match self.rpc_with_timeout(Request::Team(request), timeout)? {
            Response::Team(result) => Ok(result),
            other => Err(format!("unexpected team response: {other:?}")),
        }
    }

    /// A client for this client's host as it is now. A local host may have
    /// been replaced (an update, a crash) under a new endpoint and token the
    /// bound client still holds: this finds the running one. A remote client
    /// follows its machine already.
    pub fn reconnected(&self) -> Result<Self, String> {
        #[cfg(not(target_family = "wasm"))]
        if self.remote.is_some() {
            return Ok(self.clone());
        }
        if self.local_endpoint.as_ref() != Some(&discovery::endpoint_path()) {
            return Err("The terminal's host belongs to another organisation; refusing to reconnect to a different authority.".into());
        }
        let fresh = Self::ensure_host()?;
        if fresh.local_endpoint != self.local_endpoint {
            return Err(
                "The selected organisation changed while reconnecting the terminal.".into(),
            );
        }
        Ok(fresh)
    }

    /// Find the running host or spawn it (single-flight). Returns a client
    /// bound to exactly one host authority.
    pub fn ensure_host() -> Result<Self, String> {
        // The app is normally its own host service. A test harness running
        // app code points `DRAGENT_HOST_EXE` at the app binary instead, so
        // the host isn't judged outdated against the harness executable.
        let exe = match std::env::var_os("DRAGENT_HOST_EXE").filter(|path| !path.is_empty()) {
            Some(path) => std::path::PathBuf::from(path),
            None => std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?,
        };
        Self::ensure_host_with(&exe)
    }

    /// Same as [`ensure_host`], but spawning this executable (integration
    /// tests point at the real `dragent-host` binary, since under
    /// `cargo test` the current exe is the test harness).
    pub fn ensure_host_with(exe: &std::path::Path) -> Result<Self, String> {
        let _guard = start_lock()
            .lock()
            .map_err(|e| format!("spawn lock: {e}"))?;
        let client_id = client_id();
        // One metadata stat: cheap next to a ping, and it changes when the
        // file on disk is rebuilt under the same path.
        let desired = super::build_identity::executable(exe).unwrap_or_default();
        if let Some(endpoint) = discovery::read_endpoint() {
            if endpoint.version == PROTOCOL_VERSION
                && recently_healthy(&endpoint.token, &desired, Instant::now())
            {
                return Ok(Self {
                    endpoint,
                    client_id,
                    local_endpoint: Some(discovery::endpoint_path()),
                    #[cfg(not(target_family = "wasm"))]
                    remote: None,
                });
            }
            let compatible = endpoint.version == PROTOCOL_VERSION;
            let healthy = compatible && discovery::ping(&endpoint, &client_id);
            let outdated = host_outdated(
                &endpoint.build_id,
                &desired,
                super::build_identity::current(),
            );
            let deferred_update = DEFERRED_UPDATE.get_or_init(|| Mutex::new(None));
            let update_deferred = deferred_update
                .lock()
                .map_err(|e| format!("update lock: {e}"))?
                .as_ref()
                .is_some_and(|(token, build, since)| {
                    token == &endpoint.token
                        && build == &desired
                        && since.elapsed() < UPDATE_RETRY_DELAY
                });
            if healthy && (!outdated || update_deferred) {
                note_healthy(&endpoint.token, &desired);
                return Ok(Self {
                    endpoint,
                    client_id,
                    local_endpoint: Some(discovery::endpoint_path()),
                    #[cfg(not(target_family = "wasm"))]
                    remote: None,
                });
            }
            if (outdated || !compatible)
                && super::stale::service_may_be_alive(
                    endpoint.pid,
                    endpoint.started_at_unix_ms,
                    SERVICE_ARG,
                )
            {
                // Never use Shutdown here: it also closes the PTY daemon.
                // Older builds without this request remain usable if compatible.
                let safe_restart = discovery::restart_for_update(&endpoint);
                let mut restart_requested = safe_restart.is_ok();
                if safe_restart.is_err()
                    && healthy
                    && endpoint.build_id.is_empty()
                    && endpoint.version == 14
                {
                    // Migration from the audited v14 host: stop only this
                    // verified host PID, leaving its separate PTY daemon alive.
                    // Never stop a process tree or the daemon itself.
                    restart_requested = super::stale::terminate_outdated(endpoint.pid, SERVICE_ARG);
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while restart_requested && std::time::Instant::now() < deadline {
                    if discovery::read_endpoint().is_none_or(|next| next.token != endpoint.token)
                        || !super::stale::service_may_be_alive(
                            endpoint.pid,
                            endpoint.started_at_unix_ms,
                            SERVICE_ARG,
                        )
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if discovery::read_endpoint().is_some_and(|next| next.token == endpoint.token)
                    && super::stale::service_may_be_alive(
                        endpoint.pid,
                        endpoint.started_at_unix_ms,
                        SERVICE_ARG,
                    )
                {
                    if healthy {
                        *deferred_update
                            .lock()
                            .map_err(|e| format!("update lock: {e}"))? =
                            Some((endpoint.token.clone(), desired.clone(), Instant::now()));
                        let reason = safe_restart.err().unwrap_or_else(|| {
                            "host did not exit after accepting the update".into()
                        });
                        log::warn!(
                            "host update deferred for 5 minutes: {reason}; sessions preserved"
                        );
                        return Ok(Self {
                            endpoint,
                            client_id,
                            local_endpoint: Some(discovery::endpoint_path()),
                            #[cfg(not(target_family = "wasm"))]
                            remote: None,
                        });
                    }
                    return Err("Host update pending: existing service could not safely restart. Running sessions were preserved.".into());
                }
            }
        }
        let mut spawned = spawn_host_process(exe)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(endpoint) = discovery::read_endpoint() {
                if discovery::ping(&endpoint, &client_id) {
                    note_healthy(&endpoint.token, &desired);
                    return Ok(Self {
                        endpoint,
                        client_id,
                        local_endpoint: Some(discovery::endpoint_path()),
                        #[cfg(not(target_family = "wasm"))]
                        remote: None,
                    });
                }
            }
            if std::time::Instant::now() > deadline {
                return Err("host did not come up within 5s".to_string());
            }
            // A host that is just closing (idle, or for an update) still owns
            // the endpoint for a moment and turns a new one away: try again.
            if matches!(spawned.try_wait(), Ok(Some(_))) {
                std::thread::sleep(Duration::from_millis(150));
                spawned = spawn_host_process(exe)?;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Re-resolve the current authority without spawning (for reconnect paths
    /// that must never create a host).
    pub fn connect_existing() -> Result<Self, String> {
        let client_id = client_id();
        let endpoint =
            discovery::read_endpoint().ok_or_else(|| "no host endpoint file".to_string())?;
        if discovery::ping(&endpoint, &client_id) {
            Ok(Self {
                endpoint,
                client_id,
                local_endpoint: Some(discovery::endpoint_path()),
                #[cfg(not(target_family = "wasm"))]
                remote: None,
            })
        } else {
            // Found gone: the next `ensure_host` must not reuse it.
            forget_healthy(&endpoint.token);
            Err("host endpoint is stale".to_string())
        }
    }

    pub fn ping(&self) -> bool {
        self.current_endpoint().is_ok_and(|endpoint| {
            let alive = discovery::ping(&endpoint, &self.client_id);
            if !alive {
                forget_healthy(&endpoint.token);
            }
            alive
        })
    }

    pub fn host_info(&self) -> Result<HostInfo, String> {
        match self.rpc(Request::GetHostInfo)? {
            Response::HostInfo(info) => Ok(info),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn runtime_state(&self) -> Result<RuntimeState, String> {
        match self.rpc(Request::GetRuntimeState)? {
            Response::RuntimeState(state) => Ok(state),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn run_git(&self, root: &str, args: &[&str]) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.run_git_with(root, &args, &[], &[], &[])
    }

    /// Run git in `root` on the host, with optional stdin and environment
    /// changes. Returns (exit code, stdout, stderr). Network operations can
    /// take minutes, so this waits much longer than an ordinary RPC.
    pub fn run_git_with(
        &self,
        root: &str,
        args: &[String],
        stdin: &[u8],
        env: &[(String, String)],
        env_remove: &[String],
    ) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
        match self.rpc_with_timeout(
            Request::RunGit {
                root: root.to_string(),
                args: args.to_vec(),
                stdin: stdin.to_vec(),
                env: env.to_vec(),
                env_remove: env_remove.to_vec(),
            },
            Duration::from_secs(15 * 60),
        )? {
            Response::GitOutput {
                status,
                stdout,
                stderr,
            } => Ok((status, stdout, stderr)),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn heartbeat(&self) -> Result<(), String> {
        match self.rpc(Request::Heartbeat)? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn shutdown(&self) -> Result<(), String> {
        match self.rpc(Request::Shutdown)? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn list_workspaces(&self) -> Result<Vec<String>, String> {
        match self.rpc(Request::ListWorkspaces)? {
            Response::WorkspaceList { workspace_ids } => Ok(workspace_ids),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn create_workspace(&self, root: &str) -> Result<String, String> {
        match self.rpc(Request::CreateWorkspace {
            root: root.to_string(),
        })? {
            Response::WorkspaceCreated { workspace_id } => Ok(workspace_id),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn create_local_workspace(&self, root: &str) -> Result<String, String> {
        match self.rpc(Request::CreateLocalWorkspace {
            root: root.to_string(),
        })? {
            Response::WorkspaceCreated { workspace_id } => Ok(workspace_id),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn workspace_state(
        &self,
        workspace_id: &str,
    ) -> Result<super::protocol::WorkspaceSnapshot, String> {
        match self.rpc(Request::GetWorkspaceState {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::WorkspaceStateSnapshot(snapshot) => Ok(snapshot),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn accept_workspace_command(
        &self,
        workspace_id: &str,
        command: dragent_session_protocol::workspace::WorkspaceCommandEnvelope,
    ) -> Result<usize, String> {
        match self.rpc(Request::AcceptWorkspaceCommand {
            workspace_id: workspace_id.to_string(),
            command,
        })? {
            Response::WorkspaceEventsAccepted { count } => Ok(count),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn save_workspace_project(
        &self,
        project: dragent_workspace_model::Project,
    ) -> Result<(), String> {
        match self.rpc(Request::SaveWorkspaceProject { project })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn save_workspace_work_item(
        &self,
        work_item: dragent_workspace_model::WorkItem,
    ) -> Result<(), String> {
        match self.rpc(Request::SaveWorkspaceWorkItem { work_item })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn save_workspace_event(
        &self,
        event: dragent_workspace_model::WorkspaceEvent,
    ) -> Result<(), String> {
        match self.rpc(Request::SaveWorkspaceEvent { event })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn load_workspace_project(
        &self,
        project_id: &str,
    ) -> Result<Option<dragent_workspace_model::Project>, String> {
        match self.rpc(Request::LoadWorkspaceProject {
            project_id: project_id.to_string(),
        })? {
            Response::WorkspaceProjectLoaded { project } => Ok(project),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn load_recent_workspace_events(
        &self,
        limit: usize,
    ) -> Result<Vec<dragent_workspace_model::WorkspaceEvent>, String> {
        match self.rpc(Request::LoadRecentWorkspaceEvents { limit })? {
            Response::WorkspaceEventList { events } => Ok(events),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn scheduler_status(
        &self,
        workspace_id: &str,
    ) -> Result<Option<super::protocol::SchedulerStatus>, String> {
        match self.rpc(Request::GetSchedulerStatus {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::SchedulerStatusReport { status } => Ok(status),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    // ── Phase 6.1: worktree-backed workspaces + teardown saga ──

    pub fn create_worktree(
        &self,
        workspace_id: &str,
        flow: &str,
        repo_root: &str,
        branch: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::CreateWorktree {
            workspace_id: workspace_id.to_string(),
            flow: flow.to_string(),
            repo_root: repo_root.to_string(),
            branch: branch.to_string(),
        })? {
            Response::WorktreeCreated { home } => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    /// Register an existing directory as a session home (no disk changes).
    /// Idempotent per workspace + canonical path.
    /// Put an existing agent checkout (a linked worktree of `repo_root`)
    /// under the host's ledger. Idempotent per path.
    pub fn adopt_checkout(
        &self,
        workspace_id: &str,
        repo_root: &str,
        path: &str,
        flow: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::AdoptCheckout {
            workspace_id: workspace_id.to_string(),
            repo_root: repo_root.to_string(),
            path: path.to_string(),
            flow: flow.to_string(),
        })? {
            Response::HomeEnsured { home } => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    /// Register the directory authority for a new terminal. A project's
    /// linked worktree can live outside its main checkout; workspace identity
    /// still belongs to the project, but the PTY must belong to that worktree.
    pub fn ensure_terminal_home(
        &self,
        workspace_id: &str,
        project_root: &str,
        cwd: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        let root = std::path::Path::new(project_root)
            .canonicalize()
            .map_err(|e| format!("project root unavailable: {e}"))?;
        let requested = std::path::Path::new(cwd);
        let directory = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            root.join(requested)
        }
        .canonicalize()
        .map_err(|e| format!("cwd unavailable: {e}"))?;
        let home = if directory.starts_with(&root) {
            &root
        } else {
            &directory
        };
        self.ensure_home(workspace_id, &home.to_string_lossy())
    }

    pub fn ensure_home(
        &self,
        workspace_id: &str,
        root: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::EnsureHome {
            workspace_id: workspace_id.to_string(),
            root: root.to_string(),
        })? {
            Response::HomeEnsured { home } => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn list_worktrees(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<super::protocol::WorktreeHome>, String> {
        match self.rpc(Request::ListWorktrees {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::WorktreeList { homes } => Ok(homes),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn worktree(
        &self,
        workspace_id: &str,
        worktree_id: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::GetWorktree {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
        })? {
            Response::WorktreeSnapshot(home) => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn teardown_worktree(
        &self,
        workspace_id: &str,
        worktree_id: &str,
    ) -> Result<super::protocol::TeardownReport, String> {
        match self.rpc(Request::TeardownWorktree {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
        })? {
            Response::WorktreeTornDown { report } => Ok(report),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn create_standalone_session(
        &self,
        workspace_id: &str,
        label: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::CreateStandaloneSession {
            workspace_id: workspace_id.to_string(),
            label: label.to_string(),
        })? {
            Response::StandaloneSessionCreated { home } => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn teardown_standalone_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<String, String> {
        match self.rpc(Request::TeardownStandaloneSession {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
        })? {
            Response::StandaloneSessionTornDown { worktree_id } => Ok(worktree_id),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn attach_session(
        &self,
        workspace_id: &str,
        worktree_id: &str,
        session_id: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::AttachSession {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
            session_id: session_id.to_string(),
        })? {
            Response::WorktreeSnapshot(home) => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn detach_session(
        &self,
        workspace_id: &str,
        worktree_id: &str,
        session_id: &str,
    ) -> Result<super::protocol::WorktreeHome, String> {
        match self.rpc(Request::DetachSession {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
            session_id: session_id.to_string(),
        })? {
            Response::WorktreeSnapshot(home) => Ok(home),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn allocate_port(&self, workspace_id: &str, worktree_id: &str) -> Result<u16, String> {
        match self.rpc(Request::AllocatePort {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
        })? {
            Response::PortAllocated { port } => Ok(port),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn release_port(
        &self,
        workspace_id: &str,
        worktree_id: &str,
        port: u16,
    ) -> Result<(), String> {
        match self.rpc(Request::ReleasePort {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
            port,
        })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    // ── Phase 6.2: terminal sessions owned per home ──

    pub fn create_terminal(
        &self,
        workspace_id: &str,
        worktree_id: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<super::protocol::TerminalSession, String> {
        self.create_terminal_with_shell(workspace_id, worktree_id, command, cwd, cols, rows, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_terminal_with_shell(
        &self,
        workspace_id: &str,
        worktree_id: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        shell: Option<&str>,
    ) -> Result<super::protocol::TerminalSession, String> {
        match self.rpc_with_timeout(
            Request::CreateTerminal {
                workspace_id: workspace_id.to_string(),
                worktree_id: worktree_id.to_string(),
                command: command.map(|s| s.to_string()),
                shell: shell.map(str::to_string),
                cwd: cwd.map(|s| s.to_string()),
                cols,
                rows,
            },
            Duration::from_secs(60),
        )? {
            Response::TerminalOpened { session } => Ok(session),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn list_terminals(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<super::protocol::TerminalSession>, String> {
        match self.rpc(Request::ListTerminals {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::TerminalList { sessions } => Ok(sessions),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn terminal(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<super::protocol::TerminalSession, String> {
        match self.rpc(Request::GetTerminal {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
        })? {
            Response::TerminalSnapshot(session) => Ok(session),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn send_terminal_input(
        &self,
        workspace_id: &str,
        session_id: &str,
        data: &[u8],
    ) -> Result<(), String> {
        match self.rpc(Request::SendTerminalInput {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            data: data.to_vec(),
        })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn read_terminal(
        &self,
        workspace_id: &str,
        session_id: &str,
        max_lines: Option<usize>,
    ) -> Result<(String, bool, Option<i32>), String> {
        match self.rpc(Request::ReadTerminal {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            max_lines,
        })? {
            Response::TerminalOutput {
                text,
                alive,
                exit_code,
            } => Ok((text, alive, exit_code)),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn read_terminal_screen(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<super::screen_protocol::TerminalScreen, String> {
        match self.rpc(Request::ReadTerminalScreen {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
        })? {
            Response::TerminalScreen(screen) => Ok(screen),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    /// Like [`Self::read_terminal_screen`], but gives up after `timeout`:
    /// for callers that serve others on the same thread (a phone's requests).
    pub fn read_terminal_screen_within(
        &self,
        workspace_id: &str,
        session_id: &str,
        timeout: Duration,
    ) -> Result<super::screen_protocol::TerminalScreen, String> {
        match self.rpc_with_timeout(
            Request::ReadTerminalScreen {
                workspace_id: workspace_id.to_string(),
                session_id: session_id.to_string(),
            },
            timeout,
        )? {
            Response::TerminalScreen(screen) => Ok(screen),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn read_terminal_since(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, u64, u64), String> {
        self.read_terminal_chunk(workspace_id, session_id, offset, max_bytes)
            .map(|chunk| (chunk.data, chunk.base, chunk.total))
    }

    /// Delta read plus the shell's exit status (`None` while running).
    pub fn read_terminal_chunk(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<TerminalChunk, String> {
        match self.rpc(Request::ReadTerminalSince {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            offset,
            max_bytes,
        })? {
            Response::TerminalData {
                data,
                base,
                total,
                exit_code,
            } => Ok(TerminalChunk {
                resume: None,
                operations: None,
                data,
                base,
                total,
                exit_code,
            }),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn resize_terminal(
        &self,
        workspace_id: &str,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<super::protocol::TerminalSession, String> {
        match self.rpc(Request::ResizeTerminal {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            cols,
            rows,
        })? {
            Response::TerminalSnapshot(session) => Ok(session),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn set_terminal_colors(
        &self,
        workspace_id: &str,
        session_id: &str,
        colors: super::screen_protocol::TerminalColors,
    ) -> Result<(), String> {
        match self.rpc(Request::SetTerminalColors {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            colors,
        })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn close_terminal(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<super::protocol::TerminalSession, String> {
        match self.rpc(Request::CloseTerminal {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
        })? {
            Response::TerminalSnapshot(session) => Ok(session),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    // ── Phase 6.3: agent runs in terminal sessions ──

    pub fn list_agent_configs(&self) -> Result<Vec<super::protocol::AgentConfig>, String> {
        match self.rpc(Request::ListAgentConfigs)? {
            Response::AgentConfigList { configs } => Ok(configs),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn add_agent_config(
        &self,
        label: &str,
        command: &str,
        args: Vec<String>,
        prompt_transport: super::protocol::PromptTransport,
        prompt_args: Vec<String>,
        env: std::collections::BTreeMap<String, String>,
    ) -> Result<super::protocol::AgentConfig, String> {
        match self.rpc(Request::AddAgentConfig {
            label: label.to_string(),
            command: command.to_string(),
            args,
            prompt_transport,
            prompt_args,
            env,
        })? {
            Response::AgentConfigAdded { config } => Ok(config),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn run_agent(
        &self,
        workspace_id: &str,
        worktree_id: &str,
        agent: &str,
        prompt: &str,
        dispatch: super::protocol::AgentDispatch,
    ) -> Result<super::protocol::AgentRun, String> {
        match self.rpc(Request::RunAgent {
            workspace_id: workspace_id.to_string(),
            worktree_id: worktree_id.to_string(),
            agent: agent.to_string(),
            prompt: prompt.to_string(),
            dispatch,
        })? {
            Response::AgentRunStarted { run } => Ok(run),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn agent_run(&self, run_id: &str) -> Result<super::protocol::AgentRun, String> {
        match self.rpc(Request::GetAgentRun {
            run_id: run_id.to_string(),
        })? {
            Response::AgentRunSnapshot { run } => Ok(run),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn list_agent_runs(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<super::protocol::AgentRun>, String> {
        match self.rpc(Request::ListAgentRuns {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::AgentRunList { runs } => Ok(runs),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    /// Whether an agent may start now. The caller asks again while it waits.
    pub fn agent_start(
        &self,
        ask: super::protocol::StartAsk,
        timeout: Duration,
    ) -> Result<super::protocol::StartAnswer, String> {
        match self.rpc_with_timeout(Request::AskToStart(ask), timeout)? {
            Response::StartAnswer(answer) => Ok(answer),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    /// Start a waiting agent (its pane or terminal) now, whatever memory says.
    pub fn start_anyway(&self, id: &str) -> Result<(), String> {
        self.acked(Request::StartAnyway { id: id.to_string() })
    }

    /// A waiting start was called off.
    pub fn leave_start_line(&self, id: &str) -> Result<(), String> {
        self.acked(Request::LeaveStartLine { id: id.to_string() })
    }

    /// Let a paused agent (its pane or terminal) run again.
    pub fn resume_agent(&self, id: &str) -> Result<(), String> {
        self.acked(Request::ResumeAgent { id: id.to_string() })
    }

    /// Which agents wait or are paused, and what each one holds.
    pub fn agent_resources(&self) -> Result<super::protocol::AgentResources, String> {
        match self.rpc_with_timeout(Request::AgentResources, Duration::from_secs(3))? {
            Response::AgentResources(resources) => Ok(resources),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    fn acked(&self, request: Request) -> Result<(), String> {
        match self.rpc(request)? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn restore_plan(
        &self,
        workspace_id: &str,
        known_session_ids: Vec<String>,
        max_tail_lines: Option<usize>,
    ) -> Result<super::protocol::RestorePlan, String> {
        match self.rpc(Request::GetRestorePlan {
            workspace_id: workspace_id.to_string(),
            known_session_ids,
            max_tail_lines,
        })? {
            Response::RestorePlanSnapshot(plan) => Ok(plan),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    // ── Phase 1 cold restore: per-session disk history ──

    pub fn read_session_history(
        &self,
        session_id: &str,
        max_bytes: usize,
    ) -> Result<(super::protocol::HistoryRecord, Vec<u8>), String> {
        match self.rpc(Request::ReadSessionHistory {
            session_id: session_id.to_string(),
            max_bytes,
        })? {
            Response::SessionHistory { record, tail } => Ok((record, tail)),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn list_session_history(&self) -> Result<Vec<super::protocol::HistoryRecord>, String> {
        match self.rpc(Request::ListSessionHistory)? {
            Response::SessionHistoryList { records } => Ok(records),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn purge_home_history(&self, home_id: &str) -> Result<(), String> {
        match self.rpc(Request::PurgeHomeHistory {
            home_id: home_id.to_string(),
        })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    // ── PTY split: supervised daemon health ──

    pub fn restart_pty_daemon(&self) -> Result<(), String> {
        match self.rpc(Request::RestartPtyDaemon)? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn pty_status(&self) -> Result<(Option<u32>, Option<String>, bool, bool, u32), String> {
        match self.rpc(Request::GetPtyStatus)? {
            Response::PtyStatusReport {
                daemon_pid,
                daemon_id,
                owned,
                circuit_open,
                crash_attempts,
            } => Ok((daemon_pid, daemon_id, owned, circuit_open, crash_attempts)),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn watch_workspace(&self, workspace_id: &str) -> Result<(), String> {
        match self.rpc(Request::WatchWorkspace {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn unwatch_workspace(&self, workspace_id: &str) -> Result<(), String> {
        match self.rpc(Request::UnwatchWorkspace {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::Ack => Ok(()),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn list_watches(&self) -> Result<Vec<String>, String> {
        match self.rpc(Request::ListWatches)? {
            Response::WatchList { workspace_ids } => Ok(workspace_ids),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn propose_agent_runs(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<super::router::RouteProposal>, String> {
        match self.rpc(Request::ProposeAgentRuns {
            workspace_id: workspace_id.to_string(),
        })? {
            Response::AgentRouteList { proposals } => Ok(proposals),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    pub fn check_agent_readiness(
        &self,
        agent: &str,
    ) -> Result<super::protocol::AgentReadiness, String> {
        match self.rpc(Request::CheckAgentReadiness {
            agent: agent.to_string(),
        })? {
            Response::AgentReadinessReport { readiness } => Ok(readiness),
            other => Err(format!("unexpected response: {other:?}")),
        }
    }

    /// Open the event stream. `on_event` runs on a background thread until    /// the host closes the connection. Returns the reader handle.
    /// Stream a terminal's output from absolute `offset`: replayed output
    /// still in the host's ring, then live output as it arrives. Blocks the
    /// calling thread, handing each chunk (including idle keepalives) to
    /// `on_chunk`. Returns `Ok(())` after the chunk carrying the exit code, or
    /// as soon as `on_chunk` returns `false`; `Err` when the stream fails or
    /// the session is unknown/closed (the caller decides whether to retry).
    pub fn stream_terminal(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
        on_chunk: impl FnMut(TerminalChunk) -> bool,
    ) -> Result<(), String> {
        self.stream_terminal_inner(workspace_id, session_id, offset, false, on_chunk)
    }

    pub fn stream_terminal_operations(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
        on_chunk: impl FnMut(TerminalChunk) -> bool,
    ) -> Result<(), String> {
        self.stream_terminal_inner(workspace_id, session_id, offset, true, on_chunk)
    }

    fn stream_terminal_inner(
        &self,
        workspace_id: &str,
        session_id: &str,
        offset: u64,
        operations: bool,
        mut on_chunk: impl FnMut(TerminalChunk) -> bool,
    ) -> Result<(), String> {
        let endpoint = self.current_endpoint()?;
        let stream =
            super::transport::Stream::connect(&endpoint.address(), Duration::from_millis(350))
                .map_err(|e| format!("connect: {e}"))?;
        // Keepalives arrive every 15 s; three missed means the host is gone.
        stream
            .set_read_timeout(Some(Duration::from_secs(45)))
            .map_err(|e| format!("{e}"))?;
        let envelope = RequestEnvelope {
            version: PROTOCOL_VERSION,
            token: endpoint.token,
            client_id: self.client_id.clone(),
            request: if operations {
                Request::SubscribeTerminalOperations {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                    offset,
                }
            } else {
                Request::SubscribeTerminal {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                    offset,
                }
            },
        };
        super::protocol::write_message(&mut &stream, &envelope)
            .map_err(|e| format!("write: {e}"))?;
        loop {
            let env: ResponseEnvelope =
                super::protocol::read_message(&mut &stream).map_err(|e| format!("read: {e}"))?;
            if env.version != PROTOCOL_VERSION {
                return Err(format!(
                    "protocol mismatch (host {}, client {})",
                    env.version, PROTOCOL_VERSION
                ));
            }
            match env.result? {
                Response::TerminalData {
                    data,
                    base,
                    total,
                    exit_code,
                } => {
                    // Raw and parsed consumers see keepalives to observe cancellation
                    // when a quiet session produces no output.
                    let exited = exit_code.is_some();
                    let keep_going = on_chunk(TerminalChunk {
                        resume: None,
                        operations: None,
                        data,
                        base,
                        total,
                        exit_code,
                    });
                    if exited || !keep_going {
                        return Ok(());
                    }
                }
                Response::TerminalOperations { frame } => {
                    let total = frame.end();
                    if !on_chunk(TerminalChunk {
                        resume: frame.resume,
                        base: frame.base,
                        total,
                        data: frame.data,
                        operations: Some(frame.operations),
                        exit_code: None,
                    }) {
                        return Ok(());
                    }
                }
                other => return Err(format!("unexpected response: {other:?}")),
            }
        }
    }

    pub fn subscribe<F>(&self, on_event: F) -> Result<std::thread::JoinHandle<()>, String>
    where
        F: Fn(Event) + Send + 'static,
    {
        let endpoint = self.current_endpoint()?;
        let stream =
            super::transport::Stream::connect(&endpoint.address(), Duration::from_millis(350))
                .map_err(|e| format!("connect: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| format!("{e}"))?;
        let envelope = RequestEnvelope {
            version: PROTOCOL_VERSION,
            token: endpoint.token,
            client_id: self.client_id.clone(),
            request: Request::Subscribe,
        };
        super::protocol::write_message(&mut &stream, &envelope)
            .map_err(|e| format!("write: {e}"))?;
        Ok(std::thread::spawn(move || {
            loop {
                match super::protocol::read_frame(&mut &stream) {
                    Ok(bytes) => {
                        if let Ok(env) = super::protocol::decode_message::<EventEnvelope>(&bytes) {
                            on_event(env.event);
                        }
                    }
                    Err(_) => break,
                }
            }
        }))
    }

    pub fn endpoint(&self) -> &ServiceEndpoint {
        &self.endpoint
    }

    /// Hold an event stream open to the host until it ends. The host counts
    /// it as a connected app and never closes for being idle meanwhile.
    pub fn hold_open(&self) -> Result<(), String> {
        let token = self.current_endpoint()?.token;
        let held = self
            .subscribe(|_| {})
            .and_then(|handle| {
                handle
                    .join()
                    .map_err(|_| "presence stream panicked".to_string())
            });
        // The stream ended or never opened: the host may be gone, so the
        // reconnect must look again rather than reuse it.
        forget_healthy(&token);
        held
    }
}

/// One delta read of a terminal's output ring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalChunk {
    pub resume: Option<crate::screen_protocol::TerminalResume>,
    pub operations: Option<std::sync::Arc<Vec<dragent_terminal::operations::Operation>>>,
    pub data: Vec<u8>,
    /// Offset of `data[0]` in the session's output stream.
    pub base: u64,
    /// Total bytes the session has produced so far.
    pub total: u64,
    /// The shell's exit code once it has exited.
    pub exit_code: Option<i32>,
}

/// Opt-in host log capture: when set, the spawned host appends its stderr
/// (the `log` output, incl. structured `dragent::pty` events) to this file.
/// The host is otherwise detached with no terminal to write to.
pub const HOST_LOG_FILE_ENV: &str = "DRAGENT_HOST_LOG_FILE";

fn spawn_host_process(exe: &std::path::Path) -> Result<std::process::Child, String> {
    let stderr = std::env::var_os(HOST_LOG_FILE_ENV)
        .and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        })
        .map(std::process::Stdio::from)
        .unwrap_or_else(std::process::Stdio::null);
    std::process::Command::new(exe)
        .arg(SERVICE_ARG)
        .arg(dragent_paths::organisation_arg())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("spawn: {e}"))
}

#[cfg(test)]
mod update_tests {
    use super::host_outdated;

    #[test]
    fn a_rebuild_on_disk_doesnt_restart_the_host_this_process_runs_with() {
        // Started together: same build everywhere.
        assert!(!host_outdated("A", "A", "A"));
        // Rebuilt on disk while running: the host matches this process; it stays.
        assert!(!host_outdated("A", "B", "A"));
        // This process is the new build (relaunched after an update): update.
        assert!(host_outdated("A", "B", "B"));
        // A host left from some other build: update.
        assert!(host_outdated("Z", "A", "A"));
        // Nothing known on disk: leave it.
        assert!(!host_outdated("A", "", "B"));
        // Unknown identities must not accidentally exempt a legacy service.
        assert!(host_outdated("", "B", ""));
    }
}

#[cfg(test)]
mod health_cache_tests {
    use super::*;

    #[test]
    fn a_host_found_healthy_is_reused_briefly_then_pinged_again() {
        let token = "health-cache-test-token";
        let build = "health-cache-test-build";
        let now = Instant::now();
        assert!(!recently_healthy(token, build, now));
        note_healthy(token, build);
        assert!(recently_healthy(token, build, Instant::now()));
        assert!(
            !recently_healthy("another-host", build, Instant::now()),
            "only that host"
        );
        assert!(
            !recently_healthy(token, "another-build", Instant::now()),
            "only that build: a rebuild must not reuse the shortcut"
        );
        assert!(
            !recently_healthy(
                token,
                build,
                Instant::now() + HEALTHY_REUSE + Duration::from_millis(1)
            ),
            "stale after the reuse window"
        );
        forget_healthy(token);
        assert!(
            !recently_healthy(token, build, Instant::now()),
            "a failed request forgets it"
        );
    }
}
