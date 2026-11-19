//! Persistent host runtime (Phase 1: foundation only).
//!
//! Owns no agents/workspaces/PTYs yet. Serves identity, a shaped runtime
//! snapshot, heartbeats, and an event stream — enough to prove UI-independent
//! lifetime, discovery, and reconnect.

use super::transport::{Listener, Stream};
use std::collections::HashMap;

/// Host socket file, next to the endpoint file.
pub const HOST_SOCKET_FILENAME: &str = "host.sock";
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;

use super::discovery;
use super::protocol::*;

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

fn generate_host_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Shared {
    token: String,
    info: HostInfo,
    shutdown: Arc<AtomicBool>,
    preserve_pty_on_exit: AtomicBool,
    /// client_id -> last-seen unix_ms. Informational in Phase 1 (the host
    /// deliberately does NOT exit when clients go away — persistence without
    /// UI is the point).
    clients: Mutex<HashMap<String, u64>>,
    /// Subscriber senders for the event stream.
    subscribers: Mutex<Vec<std::sync::mpsc::Sender<Vec<u8>>>>,
    /// Canonical workspace authority (Phase 2). `None` when the persistence
    /// backend failed to open — identity RPCs still serve degraded.
    #[cfg(not(target_family = "wasm"))]
    workspaces: Mutex<Option<super::workspaces::WorkspaceRegistry>>,
    /// Worktree-backed workspace + session authority (Phase 6.1). `None`
    /// when the ledger failed to open — identity RPCs still serve degraded.
    #[cfg(not(target_family = "wasm"))]
    worktrees: Mutex<Option<super::worktrees::WorktreeManager>>,
    /// Agent configs + run records (Phase 6.3). `None` when the store
    /// failed to open — identity RPCs still serve degraded.
    #[cfg(not(target_family = "wasm"))]
    agents: Mutex<Option<super::agents::AgentRegistry>>,
    /// Supervised PTY daemon owning the terminal sessions (PTY split).
    /// Infallible by construction: spawn failures surface per-RPC, never as
    /// a missing authority.
    #[cfg(not(target_family = "wasm"))]
    pty: Arc<super::pty_supervisor::PtySupervisor>,
    /// Project file watcher (git-porcelain polling). Infallible by
    /// construction; per-watch errors surface per-RPC.
    #[cfg(not(target_family = "wasm"))]
    watch: Arc<super::watch::WatchRegistry>,
    /// Headless scheduler driver state (Phase 4).
    scheduler: crate::scheduler::SchedulerDriver,
}

fn broadcast(shared: &Shared, event: &Event) {
    let payload = match super::protocol::encode_message(&EventEnvelope {
        event: event.clone(),
    }) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let mut subs = shared.subscribers.lock().unwrap_or_else(|p| p.into_inner());
    subs.retain(|tx| tx.send(payload.clone()).is_ok());
}

fn reply(stream: &Stream, result: Result<Response, String>) {
    let env = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        result,
    };
    let mut out = stream;
    let _ = super::protocol::write_message(&mut out, &env);
}

/// Live home scoped to one workspace (ownership gate): unknown ids and
/// foreign-workspace ids both report "unknown worktree", never existence.
#[cfg(not(target_family = "wasm"))]
fn owned_home(
    shared: &Shared,
    workspace_id: &str,
    worktree_id: &str,
) -> Result<super::protocol::WorktreeHome, String> {
    let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
    match worktrees.as_ref() {
        Some(manager) => match manager.get(worktree_id) {
            Some(home) if home.workspace_id == workspace_id => Ok(home),
            _ => Err(format!("unknown worktree {worktree_id}")),
        },
        None => Err("worktree authority unavailable".to_string()),
    }
}

/// Drop a deleted checkout's code index, on a detached thread: a down index
/// service must never fail workspace operations. Nothing here adds a checkout
/// to the index; embedding starts when something first searches it.
#[cfg(not(target_family = "wasm"))]
fn forget_index(root: std::path::PathBuf) {
    std::thread::Builder::new()
        .name("dragent-index-hook".to_string())
        .spawn(move || dragent_index::get_manager().remove_index(&root))
        .ok();
}

/// Forward one unary RPC to the supervised PTY daemon (ensuring it is up
/// first). Daemon transport errors surface as-is; shape mismatches are the
/// caller's to report.
#[cfg(not(target_family = "wasm"))]
fn daemon_call(shared: &Shared, request: DaemonRequest) -> Result<DaemonResponse, String> {
    let conn = shared
        .pty
        .ensure()
        .map_err(|e| format!("pty daemon unavailable: {e}"))?;
    match conn.rpc(request.clone()) {
        Err(error) if error == super::protocol::PTY_UPDATE_RETRY => {
            // The predecessor explicitly rejected this request before running
            // it. Retrying here cannot duplicate input or terminal creation.
            shared.pty.ensure()?.rpc(request)
        }
        result => result,
    }
}

/// Agents' sessions as the team courier sees them: their records and hook
/// reports, the PTY daemon for their terminals.
#[cfg(not(target_family = "wasm"))]
struct TeamSessions {
    shared: Arc<Shared>,
}

#[cfg(not(target_family = "wasm"))]
impl super::team_courier::Sessions for TeamSessions {
    fn session(&self, pane: &str) -> Option<super::team_courier::SessionView> {
        let pane = uuid::Uuid::parse_str(pane).ok()?;
        let record = super::agent_sessions::load(pane)?;
        let terminal = daemon_call(
            &self.shared,
            DaemonRequest::GetTerminal {
                session_id: record.terminal_id.clone(),
            },
        );
        let (reachable, exited) = match terminal {
            Ok(DaemonResponse::TerminalSnapshot(session)) => (
                matches!(session.state, super::protocol::TerminalState::Running),
                matches!(session.state, super::protocol::TerminalState::Exited { .. }),
            ),
            _ => (false, false),
        };
        Some(super::team_courier::SessionView {
            workspace_id: record.workspace_id.clone(),
            terminal_id: record.terminal_id.clone(),
            active: record.active && !exited,
            reachable,
            lost: false,
            activity: super::agent_sessions::load_activity(pane).map(|activity| (activity.event, activity.at_ms)),
            last_input_ms: super::team_courier::last_input_ms(&record.terminal_id),
            asleep: super::agent_sessions::is_asleep(pane),
            output_quiet_ms: None,
        })
    }

    fn pane_of_terminal(&self, terminal: &str) -> Option<String> {
        super::agent_sessions::pane_for_terminal(terminal).map(|pane| pane.to_string())
    }

    fn output_total(&self, terminal: &str) -> Option<u64> {
        <Self as super::agent_quiet::Terminals>::output_total(self, terminal)
    }

    fn wake(&self, pane: &str, reason: &str) {
        let Ok(pane) = uuid::Uuid::parse_str(pane) else {
            return;
        };
        if let Err(error) = super::agent_sessions::request_wake(pane, reason) {
            log::warn!("[team] couldn't ask for {pane} to wake: {error}");
        }
    }

    /// A paste, then Enter in a write of its own once it landed (sent
    /// together, some agents keep the text in their input box).
    fn type_text(&self, session: &super::team_courier::SessionView, text: &str) -> Result<(), String> {
        let text: String = text
            .chars()
            .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
            .collect();
        let mut paste = b"\x1b[200~".to_vec();
        paste.extend_from_slice(text.as_bytes());
        paste.extend_from_slice(b"\x1b[201~");
        for data in [paste, b"\r".to_vec()] {
            match daemon_call(
                &self.shared,
                DaemonRequest::SendTerminalInput {
                    session_id: session.terminal_id.clone(),
                    data,
                },
            )? {
                DaemonResponse::Ack => {}
                other => return Err(format!("unexpected response: {other:?}")),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }
}

#[cfg(not(target_family = "wasm"))]
impl super::agent_quiet::Terminals for TeamSessions {
    fn output_total(&self, terminal: &str) -> Option<u64> {
        // Asking for no bytes still says how many there have been.
        match daemon_call(
            &self.shared,
            DaemonRequest::ReadTerminalSince {
                session_id: terminal.to_string(),
                offset: u64::MAX,
                max_bytes: 0,
            },
        ) {
            Ok(DaemonResponse::TerminalData {
                total_bytes,
                exit_code: None,
                ..
            }) => Some(total_bytes),
            _ => None,
        }
    }

    fn last_interrupt_ms(&self, terminal: &str) -> Option<i64> {
        super::agent_quiet::last_interrupt_ms(terminal)
    }
}

/// Daemon GetTerminal plus the host workspace gate (unknown ids and
/// foreign-workspace ids both report "unknown terminal", never existence).
#[cfg(not(target_family = "wasm"))]
fn daemon_session(
    shared: &Shared,
    workspace_id: &str,
    session_id: &str,
) -> Result<super::protocol::TerminalSession, String> {
    match daemon_call(
        shared,
        DaemonRequest::GetTerminal {
            session_id: session_id.to_string(),
        },
    ) {
        Ok(DaemonResponse::TerminalSnapshot(session)) if session.workspace_id == workspace_id => {
            Ok(session)
        }
        Ok(_) => Err(format!("unknown terminal {session_id}")),
        Err(error) => Err(error),
    }
}

/// Fill a run record with live terminal state + screen output (the
/// GetTerminal pattern: liveness resolves through the PTY daemon, never
/// from the agents record). `None` when the terminal is gone.
#[cfg(not(target_family = "wasm"))]
fn agent_snapshot(
    shared: &Shared,
    base: &super::protocol::AgentRun,
) -> Option<super::protocol::AgentRun> {
    let session = match daemon_call(
        shared,
        DaemonRequest::GetTerminal {
            session_id: base.id.clone(),
        },
    ) {
        Ok(DaemonResponse::TerminalSnapshot(session)) => session,
        _ => return None,
    };
    if session.workspace_id != base.workspace_id {
        return None;
    }
    let state = match session.state {
        super::protocol::TerminalState::Running => super::protocol::AgentRunState::Running,
        super::protocol::TerminalState::Exited { code } => {
            super::protocol::AgentRunState::Exited { code }
        }
    };
    let output = match daemon_call(
        shared,
        DaemonRequest::ReadTerminal {
            session_id: base.id.clone(),
            max_lines: None,
        },
    ) {
        Ok(DaemonResponse::TerminalOutput { text, .. }) => text,
        _ => String::new(),
    };
    Some(super::protocol::AgentRun {
        state,
        output,
        ..base.clone()
    })
}

/// Phase 6.3 launch: resolve home → config → overrides → stage → context →
/// assemble → launch through the SAME [`TerminalRegistry::create`] path
/// `CreateTerminal` uses (no duplicated spawn) → attach → record.
#[cfg(not(target_family = "wasm"))]
fn run_agent_inner(
    shared: &Shared,
    home: &super::protocol::WorktreeHome,
    agent_ref: &str,
    prompt: &str,
    dispatch: &super::protocol::AgentDispatch,
) -> Result<super::protocol::AgentRun, String> {
    use super::agents as phases;
    let config = {
        let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
        match agents.as_ref() {
            Some(registry) => registry
                .resolve(agent_ref)
                .ok_or_else(|| format!("unknown agent {agent_ref}"))?,
            None => return Err("agent authority unavailable".to_string()),
        }
    };
    let mut effective = phases::apply_overrides(&config, &dispatch.overrides);
    // The copy of the agent chosen for this machine (Settings → Machines).
    if let Some(words) = super::host_tools::launch_words(&effective.command) {
        effective.args = words[1..].iter().cloned().chain(effective.args).collect();
        effective.command = words[0].clone();
    }
    // Capacity gate before anything with side effects: live runs only
    // (exited sessions already released their slots).
    {
        let tracked: Vec<(String, String)> = {
            let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
            match agents.as_ref() {
                Some(registry) => registry.all_tracked_runs(),
                None => return Err("agent authority unavailable".to_string()),
            }
        };
        let mut workspace_live = 0usize;
        let mut host_live = 0usize;
        for (run_workspace, run_id) in &tracked {
            let alive = matches!(
                daemon_call(
                    shared,
                    DaemonRequest::GetTerminal {
                        session_id: run_id.clone(),
                    },
                ),
                Ok(DaemonResponse::TerminalSnapshot(session))
                    if matches!(session.state, super::protocol::TerminalState::Running)
            );
            if alive {
                host_live += 1;
                if run_workspace == &home.workspace_id {
                    workspace_live += 1;
                }
            }
        }
        super::resources::check_capacity(
            workspace_live,
            host_live,
            super::resources::CapacityLimits::default(),
        )
        .map_err(|e| e)?;
    }
    let home_dir = std::path::Path::new(&home.path);
    let staged =
        phases::stage_attachments(home_dir, &dispatch.attachments).map_err(|e| format!("{e:#}"))?;
    let budget = phases::clamp_context_chars(dispatch.context_chars);
    let context = phases::read_context(home_dir, &dispatch.context_from, budget);
    let final_prompt = phases::assemble_prompt(&context, &staged, prompt);
    let shell = phases::build_launch(&effective, &final_prompt);
    // SAME path CreateTerminal uses: interactive shell in the home with the
    // launch line as the initial command (argv payload or stdin heredoc both
    // ride inside `shell`), via the supervised PTY daemon.
    let created = daemon_call(
        shared,
        DaemonRequest::CreateTerminal {
            home: home.clone(),
            workspace_id: home.workspace_id.clone(),
            command: Some(shell),
            shell: None,
            cwd: None,
            cols: 0,
            rows: 0,
        },
    );
    match created {
        Ok(DaemonResponse::TerminalOpened { session }) => {
            let attached = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.attach_session(&home.id, &session.id),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match attached {
                Ok(_) => {
                    let meta = phases::RunMeta {
                        agent: effective.id.clone(),
                        label: effective.label.clone(),
                        workspace_id: home.workspace_id.clone(),
                        started_at: now_ms(),
                        instance_id: dispatch.instance_id.clone(),
                        lease_id: dispatch.lease_id.clone(),
                    };
                    let base = {
                        let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                        match agents.as_ref() {
                            Some(registry) => registry.record_run(&session, meta),
                            None => {
                                return Err("agent authority unavailable".to_string());
                            }
                        }
                    };
                    Ok(agent_snapshot(shared, &base).unwrap_or(base))
                }
                Err(error) => {
                    // Attach failed: dispose the orphaned PTY (mirrors
                    // CreateTerminal).
                    shared.pty.close_session(&session.id);
                    Err(format!("{error:#}"))
                }
            }
        }
        Ok(other) => Err(format!("unexpected response: {other:?}")),
        Err(error) => Err(error),
    }
}

fn handle_connection(stream: Stream, shared: Arc<Shared>) {
    // Accepted sockets inherit the listener's non-blocking mode on
    // macOS/BSD; streams need blocking writes with timeouts.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let payload = match super::protocol::read_frame(&mut &stream) {
        Ok(payload) => payload,
        Err(_) => return,
    };
    if let Some(token) = payload.strip_prefix(super::protocol::HOST_UPDATE_MAGIC) {
        if !super::protocol::tokens_equal(&String::from_utf8_lossy(token), &shared.token) {
            let _ = super::protocol::write_frame(&mut &stream, b"ERR unauthorized");
            return;
        }
        // Preserve the separate PTY owner; ordinary Shutdown remains explicit
        // and destructive. This handshake is independent of RPC versions.
        shared.preserve_pty_on_exit.store(true, Ordering::SeqCst);
        shared.shutdown.store(true, Ordering::SeqCst);
        let _ = super::protocol::write_frame(&mut &stream, b"OK");
        broadcast(&shared, &Event::ShutdownNotice);
        return;
    }
    let envelope: RequestEnvelope = match super::protocol::decode_message(&payload) {
        Ok(envelope) => envelope,
        Err(_) => return,
    };
    if !super::protocol::tokens_equal(&envelope.token, &shared.token) {
        reply(&stream, Err("unauthorized".to_string()));
        return;
    }
    // Shutdown is honored across versions when the token matches (lets a new
    // client evict a stale host), like the index service.
    if matches!(envelope.request, Request::Shutdown) {
        reply(&stream, Ok(Response::Ack));
        broadcast(&shared, &Event::ShutdownNotice);
        shared.shutdown.store(true, Ordering::SeqCst);
        return;
    }
    if envelope.version != PROTOCOL_VERSION {
        reply(
            &stream,
            Err(format!(
                "protocol mismatch (host {}, client {})",
                PROTOCOL_VERSION, envelope.version
            )),
        );
        return;
    }
    {
        let mut clients = shared.clients.lock().unwrap_or_else(|p| p.into_inner());
        clients.insert(envelope.client_id.clone(), now_ms());
    }
    match envelope.request {
        #[cfg(not(target_family = "wasm"))]
        Request::Development(request) => reply(&stream, super::development::execute(request).map(Response::Development)),
        #[cfg(target_family = "wasm")]
        Request::Development(_) => reply(&stream, Err("development host operations unavailable on wasm".into())),
        #[cfg(not(target_family = "wasm"))]
        Request::Team(request) => {
            let changes = request.changes();
            let result = super::team::execute(request);
            if let (Some(team_id), Ok(response)) = (changes, &result) {
                let team_id = match response {
                    super::team::TeamResponse::Team(team) => team.id.clone(),
                    _ => team_id,
                };
                broadcast(&shared, &Event::TeamChanged { team_id });
            }
            reply(&stream, result.map(Response::Team))
        }
        #[cfg(target_family = "wasm")]
        Request::Team(_) => reply(&stream, Err("teams unavailable on wasm".into())),
        Request::Coordination { tool, arguments, caller } => {
            #[cfg(not(target_family = "wasm"))]
            let result = serde_json::from_value::<super::team_commands::Caller>(caller)
                .map_err(|e| e.to_string()).and_then(|caller| super::service_relay::dispatch(&tool, &arguments, &caller).unwrap_or_else(||super::team_runtime::call(&tool, &arguments, &caller)));
            #[cfg(target_family = "wasm")]
            let result: Result<serde_json::Value, String> = Err("Coordination unavailable on wasm".into());
            reply(&stream, result.map(Response::Coordination));
        }
        Request::Ping => reply(&stream, Ok(Response::Pong)),
        Request::GetHostInfo => reply(&stream, Ok(Response::HostInfo(shared.info.clone()))),
        Request::GetRuntimeState => {
            #[cfg(not(target_family = "wasm"))]
            let state = {
                let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
                let count = workspaces.as_ref().map(|w| w.list().len()).unwrap_or(0) as u32;
                // Daemon census (empty workspace lists all); degraded to
                // zero when the daemon cannot serve.
                let terminals = match daemon_call(
                    &shared,
                    DaemonRequest::ListTerminals {
                        workspace_id: String::new(),
                    },
                ) {
                    Ok(DaemonResponse::TerminalList { sessions }) => sessions.len() as u32,
                    _ => 0,
                };
                let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                let agent_runs = agents.as_ref().map(|r| r.run_count()).unwrap_or(0) as u32;
                crate::protocol::RuntimeState {
                    workspaces: count,
                    agents: agent_runs,
                    terminals,
                }
            };
            #[cfg(target_family = "wasm")]
            let state = crate::protocol::RuntimeState::default();
            reply(&stream, Ok(Response::RuntimeState(state)))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::RunGit {
            root,
            args,
            stdin,
            env,
            env_remove,
        } => match run_git_process(&root, &args, &stdin, &env, &env_remove) {
            Ok(output) => reply(
                &stream,
                Ok(Response::GitOutput {
                    status: output.status.code().unwrap_or(-1),
                    stdout: output.stdout,
                    stderr: output.stderr,
                }),
            ),
            Err(error) => reply(&stream, Err(format!("git execution failed: {error}"))),
        },
        Request::Heartbeat => {
            broadcast(
                &shared,
                &Event::HeartbeatAck {
                    client_id: envelope.client_id.clone(),
                },
            );
            reply(&stream, Ok(Response::Ack))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ListWorkspaces => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => reply(
                    &stream,
                    Ok(Response::WorkspaceList {
                        workspace_ids: registry.list(),
                    }),
                ),
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CreateWorkspace { root } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.create(&root) {
                    Ok(id) => {
                        drop(workspaces);
                        broadcast(
                            &shared,
                            &Event::WorkspaceCreated {
                                workspace_id: id.clone(),
                            },
                        );
                        reply(&stream, Ok(Response::WorkspaceCreated { workspace_id: id }));
                    }
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CreateLocalWorkspace { root } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.create_local(&root) {
                    Ok(id) => {
                        drop(workspaces);
                        broadcast(
                            &shared,
                            &Event::WorkspaceCreated {
                                workspace_id: id.clone(),
                            },
                        );
                        reply(&stream, Ok(Response::WorkspaceCreated { workspace_id: id }));
                    }
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::GetWorkspaceState { workspace_id } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref().and_then(|r| r.snapshot(&workspace_id)) {
                Some(snapshot) => reply(&stream, Ok(Response::WorkspaceStateSnapshot(snapshot))),
                None => reply(&stream, Err(format!("unknown workspace {workspace_id}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::AcceptWorkspaceCommand {
            workspace_id,
            command,
        } => {
            let accepted = {
                let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
                match workspaces.as_ref() {
                    Some(registry) => registry.accept(&workspace_id, &command),
                    None => Err(anyhow::anyhow!("workspace authority unavailable")),
                }
            };
            match accepted {
                Ok(envelopes) => {
                    broadcast(
                        &shared,
                        &Event::WorkspaceUpdated {
                            workspace_id: workspace_id.clone(),
                        },
                    );
                    // Immediate scheduler pass on state change (the tick
                    // covers the rest). Decisions broadcast only on change.
                    let decision = {
                        let workspaces =
                            shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
                        workspaces.as_ref().and_then(|registry| {
                            registry
                                .state_cloned(&workspace_id)
                                .and_then(|state| shared.scheduler.evaluate(&workspace_id, &state))
                        })
                    };
                    if let Some(projection) = decision {
                        broadcast(
                            &shared,
                            &Event::SchedulerEvaluated {
                                workspace_id: workspace_id.clone(),
                                ready_tasks: projection.ready_task_ids.len(),
                                launchable_tasks: projection.launchable_task_ids.len(),
                                work_complete: projection.work_complete,
                            },
                        );
                    }
                    reply(
                        &stream,
                        Ok(Response::WorkspaceEventsAccepted {
                            count: envelopes.len(),
                        }),
                    );
                }
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::SaveWorkspaceProject { project } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.save_project(&project) {
                    Ok(()) => reply(&stream, Ok(Response::Ack)),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::SaveWorkspaceWorkItem { work_item } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.save_work_item(&work_item) {
                    Ok(()) => reply(&stream, Ok(Response::Ack)),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::SaveWorkspaceEvent { event } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.save_event(&event) {
                    Ok(()) => reply(&stream, Ok(Response::Ack)),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::LoadWorkspaceProject { project_id } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.load_project(&project_id) {
                    Ok(project) => reply(&stream, Ok(Response::WorkspaceProjectLoaded { project })),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::LoadRecentWorkspaceEvents { limit } => {
            let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
            match workspaces.as_ref() {
                Some(registry) => match registry.load_recent_events(limit) {
                    Ok(events) => reply(&stream, Ok(Response::WorkspaceEventList { events })),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("workspace authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::GetSchedulerStatus { workspace_id } => {
            // Live evaluation against current canonical state (no UI needed).
            let status = {
                let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
                workspaces.as_ref().and_then(|registry| {
                    registry.state_cloned(&workspace_id).and_then(|state| {
                        crate::scheduler::evaluate_workspace(&state).map(|projection| {
                            crate::protocol::SchedulerStatus::from_projection(
                                &workspace_id,
                                &projection,
                            )
                        })
                    })
                })
            };
            // Distinguish "unknown workspace" (None) from "no plan yet" by
            // re-checking membership when there is no projection.
            let status = match status {
                Some(status) => Some(status),
                None => {
                    let known = shared
                        .workspaces
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                        .map(|r| r.list().contains(&workspace_id))
                        .unwrap_or(false);
                    known.then(|| crate::protocol::SchedulerStatus::no_plan(&workspace_id))
                }
            };
            reply(&stream, Ok(Response::SchedulerStatusReport { status }))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CreateWorktree {
            workspace_id,
            flow,
            repo_root,
            branch,
        } => {
            let created = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => {
                        manager.create_worktree(&workspace_id, &flow, &repo_root, &branch)
                    }
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match created {
                Ok(home) => {
                    broadcast(
                        &shared,
                        &Event::WorktreeCreated {
                            workspace_id: home.workspace_id.clone(),
                            worktree_id: home.id.clone(),
                        },
                    );
                    reply(&stream, Ok(Response::WorktreeCreated { home }));
                }
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::EnsureHome { workspace_id, root } => {
            let ensured = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.ensure_root_home(&workspace_id, &root),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match ensured {
                Ok(home) => {
                    reply(&stream, Ok(Response::HomeEnsured { home }));
                }
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(target_family = "wasm")]
        Request::AdoptCheckout { .. } => {
            reply(&stream, Err("worktrees unavailable on wasm".to_string()))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::AdoptCheckout {
            workspace_id,
            repo_root,
            path,
            flow,
        } => {
            let adopted = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => {
                        manager.adopt_checkout(&workspace_id, &repo_root, &path, &flow)
                    }
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match adopted {
                Ok(home) => reply(&stream, Ok(Response::HomeEnsured { home })),
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ListWorktrees { workspace_id } => {
            let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
            match worktrees.as_ref() {
                Some(manager) => reply(
                    &stream,
                    Ok(Response::WorktreeList {
                        homes: manager.list(&workspace_id),
                    }),
                ),
                None => reply(&stream, Err("worktree authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::GetWorktree {
            workspace_id,
            worktree_id,
        } => match owned_home(&shared, &workspace_id, &worktree_id) {
            Ok(home) => reply(&stream, Ok(Response::WorktreeSnapshot(home))),
            Err(error) => reply(&stream, Err(error)),
        },
        #[cfg(not(target_family = "wasm"))]
        Request::TeardownWorktree {
            workspace_id,
            worktree_id,
        } => {
            let home = match owned_home(&shared, &workspace_id, &worktree_id) {
                Ok(home) => home,
                Err(error) => {
                    reply(&stream, Err(error));
                    return;
                }
            };
            let torn_down = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.teardown(&worktree_id, "rpc-teardown", |sessions| {
                        // PTY split: fan out to attached terminal sessions
                        // through the daemon (idempotent, errors swallowed).
                        for id in sessions {
                            shared.pty.close_session(id);
                        }
                        // Cold restore must not resurrect torn-down work:
                        // purge this home's disk histories (best-effort).
                        let _ = daemon_call(
                            &shared,
                            DaemonRequest::PurgeHomeHistory {
                                home_id: worktree_id.clone(),
                            },
                        );
                        // Phase 6.3: agent runs die with their home.
                        let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(registry) = agents.as_ref() {
                            registry.drop_runs_for_sessions(sessions);
                        }
                    }),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match torn_down {
                Ok(report) => {
                    forget_index(std::path::PathBuf::from(&home.path));
                    broadcast(
                        &shared,
                        &Event::WorktreeTornDown {
                            workspace_id: home.workspace_id,
                            worktree_id: worktree_id.clone(),
                        },
                    );
                    reply(&stream, Ok(Response::WorktreeTornDown { report }));
                }
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CreateStandaloneSession {
            workspace_id,
            label,
        } => {
            let created = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.create_standalone_session(&workspace_id, &label),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match created {
                Ok(home) => {
                    broadcast(
                        &shared,
                        &Event::WorktreeCreated {
                            workspace_id: home.workspace_id.clone(),
                            worktree_id: home.id.clone(),
                        },
                    );
                    reply(&stream, Ok(Response::StandaloneSessionCreated { home }));
                }
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::TeardownStandaloneSession {
            workspace_id,
            session_id,
        } => {
            let home = match owned_home(&shared, &workspace_id, &session_id) {
                Ok(home) => home,
                Err(error) => {
                    reply(&stream, Err(error));
                    return;
                }
            };
            let torn_down = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.teardown(&session_id, "rpc-teardown", |sessions| {
                        // PTY split: fan out to attached terminal sessions
                        // through the daemon (idempotent, errors swallowed).
                        for id in sessions {
                            shared.pty.close_session(id);
                        }
                        // Cold restore must not resurrect torn-down work:
                        // purge this home's disk histories (best-effort).
                        let _ = daemon_call(
                            &shared,
                            DaemonRequest::PurgeHomeHistory {
                                home_id: session_id.clone(),
                            },
                        );
                        // Phase 6.3: agent runs die with their home.
                        let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(registry) = agents.as_ref() {
                            registry.drop_runs_for_sessions(sessions);
                        }
                    }),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match torn_down {
                Ok(report) => {
                    forget_index(std::path::PathBuf::from(&home.path));
                    broadcast(
                        &shared,
                        &Event::WorktreeTornDown {
                            workspace_id: home.workspace_id,
                            worktree_id: report.worktree_id.clone(),
                        },
                    );
                    reply(
                        &stream,
                        Ok(Response::StandaloneSessionTornDown {
                            worktree_id: report.worktree_id,
                        }),
                    );
                }
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::AttachSession {
            workspace_id,
            worktree_id,
            session_id,
        } => {
            if let Err(error) = owned_home(&shared, &workspace_id, &worktree_id) {
                reply(&stream, Err(error));
                return;
            }
            let attached = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.attach_session(&worktree_id, &session_id),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match attached {
                Ok(home) => reply(&stream, Ok(Response::WorktreeSnapshot(home))),
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::DetachSession {
            workspace_id,
            worktree_id,
            session_id,
        } => {
            if let Err(error) = owned_home(&shared, &workspace_id, &worktree_id) {
                reply(&stream, Err(error));
                return;
            }
            let detached = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.detach_session(&worktree_id, &session_id),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match detached {
                Ok(home) => reply(&stream, Ok(Response::WorktreeSnapshot(home))),
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::AllocatePort {
            workspace_id,
            worktree_id,
        } => {
            if let Err(error) = owned_home(&shared, &workspace_id, &worktree_id) {
                reply(&stream, Err(error));
                return;
            }
            let allocated = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.allocate_port(&worktree_id),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match allocated {
                Ok(port) => reply(&stream, Ok(Response::PortAllocated { port })),
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ReleasePort {
            workspace_id,
            worktree_id,
            port,
        } => {
            if let Err(error) = owned_home(&shared, &workspace_id, &worktree_id) {
                reply(&stream, Err(error));
                return;
            }
            let released = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.release_port(&worktree_id, port),
                    None => Err(anyhow::anyhow!("worktree authority unavailable")),
                }
            };
            match released {
                Ok(()) => reply(&stream, Ok(Response::Ack)),
                Err(error) => reply(&stream, Err(format!("{error:#}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CreateTerminal {
            workspace_id,
            worktree_id,
            command,
            shell,
            cwd,
            cols,
            rows,
        } => {
            // Resolve + validate the home first, then drop the worktree lock
            // before the (slow, gated) daemon spawn.
            let home = match owned_home(&shared, &workspace_id, &worktree_id) {
                Ok(home) => home,
                Err(error) => {
                    reply(&stream, Err(error));
                    return;
                }
            };
            let created = daemon_call(
                &shared,
                DaemonRequest::CreateTerminal {
                    home,
                    workspace_id: workspace_id.clone(),
                    command,
                    shell,
                    cwd,
                    cols,
                    rows,
                },
            );
            match created {
                Ok(DaemonResponse::TerminalOpened { session }) => {
                    let attached = {
                        let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                        match worktrees.as_ref() {
                            Some(manager) => manager.attach_session(&worktree_id, &session.id),
                            None => Err(anyhow::anyhow!("worktree authority unavailable")),
                        }
                    };
                    match attached {
                        Ok(_) => {
                            broadcast(
                                &shared,
                                &Event::TerminalOpened {
                                    workspace_id: workspace_id.clone(),
                                    session_id: session.id.clone(),
                                },
                            );
                            reply(&stream, Ok(Response::TerminalOpened { session }));
                        }
                        Err(error) => {
                            // Attach failed: dispose the orphaned PTY.
                            shared.pty.close_session(&session.id);
                            reply(&stream, Err(format!("{error:#}")));
                        }
                    }
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ListTerminals { workspace_id } => {
            match daemon_call(
                &shared,
                DaemonRequest::ListTerminals {
                    workspace_id: workspace_id.clone(),
                },
            ) {
                Ok(DaemonResponse::TerminalList { sessions }) => {
                    reply(&stream, Ok(Response::TerminalList { sessions }))
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::GetTerminal {
            workspace_id,
            session_id,
        } => match daemon_session(&shared, &workspace_id, &session_id) {
            Ok(session) => reply(&stream, Ok(Response::TerminalSnapshot(session))),
            Err(error) => reply(&stream, Err(error)),
        },
        #[cfg(not(target_family = "wasm"))]
        Request::SendTerminalInput {
            workspace_id,
            session_id,
            data,
        } => {
            // Ownership gate first, then the write.
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            // Someone may be typing: the team courier waits for a pause.
            super::team_courier::note_input(&session_id);
            super::agent_quiet::note_input(
                &session_id,
                &data,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as i64)
                    .unwrap_or(0),
            );
            // A submitted line means an agent running there got a prompt.
            let submitted = data.iter().any(|b| matches!(b, b'\r' | b'\n'));
            match daemon_call(
                &shared,
                DaemonRequest::SendTerminalInput {
                    session_id: session_id.clone(),
                    data,
                },
            ) {
                Ok(DaemonResponse::Ack) => {
                    reply(&stream, Ok(Response::Ack));
                    if submitted {
                        std::thread::spawn(move || {
                            crate::agent_sessions::note_submitted_input(&session_id)
                        });
                    }
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ReadTerminal {
            workspace_id,
            session_id,
            max_lines,
        } => {
            // Ownership gate first, then the tail read.
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            match daemon_call(
                &shared,
                DaemonRequest::ReadTerminal {
                    session_id: session_id.clone(),
                    max_lines,
                },
            ) {
                Ok(DaemonResponse::TerminalOutput {
                    text,
                    alive,
                    exit_code,
                    ..
                }) => reply(
                    &stream,
                    Ok(Response::TerminalOutput {
                        text,
                        alive,
                        exit_code,
                    }),
                ),
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ReadTerminalScreen {
            workspace_id,
            session_id,
        } => {
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            match daemon_call(&shared, DaemonRequest::ReadTerminalScreen { session_id }) {
                Ok(DaemonResponse::TerminalScreen(screen)) => {
                    reply(&stream, Ok(Response::TerminalScreen(screen)))
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ReadTerminalSince {
            workspace_id,
            session_id,
            offset,
            max_bytes,
        } => {
            // Ownership gate first, then the delta read.
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            match daemon_call(
                &shared,
                DaemonRequest::ReadTerminalSince {
                    session_id: session_id.clone(),
                    offset,
                    max_bytes,
                },
            ) {
                Ok(DaemonResponse::TerminalData {
                    data,
                    base_offset,
                    total_bytes,
                    exit_code,
                    ..
                }) => reply(
                    &stream,
                    Ok(Response::TerminalData {
                        data,
                        base: base_offset,
                        total: total_bytes,
                        exit_code,
                    }),
                ),
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ResizeTerminal {
            workspace_id,
            session_id,
            cols,
            rows,
        } => {
            // Ownership gate first, then the resize.
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            match daemon_call(
                &shared,
                DaemonRequest::ResizeTerminal {
                    session_id: session_id.clone(),
                    cols,
                    rows,
                },
            ) {
                Ok(DaemonResponse::TerminalSnapshot(session)) => {
                    reply(&stream, Ok(Response::TerminalSnapshot(session)))
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::SetTerminalColors {
            workspace_id,
            session_id,
            colors,
        } => {
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            match daemon_call(
                &shared,
                DaemonRequest::SetTerminalColors { session_id, colors },
            ) {
                Ok(DaemonResponse::Ack) => reply(&stream, Ok(Response::Ack)),
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CloseTerminal {
            workspace_id,
            session_id,
        } => {
            // Fetch metadata first (ownership gate) so we can detach after.
            let meta = match daemon_session(&shared, &workspace_id, &session_id) {
                Ok(meta) => meta,
                Err(error) => {
                    reply(&stream, Err(error));
                    return;
                }
            };
            // Daemon close is idempotent (Ack even when unknown); teardown
            // races stay silent.
            if let Err(error) = daemon_call(
                &shared,
                DaemonRequest::CloseTerminal {
                    session_id: session_id.clone(),
                },
            ) {
                reply(&stream, Err(error));
                return;
            }
            // Best-effort detach from the home (already gone after teardown).
            {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(manager) = worktrees.as_ref() {
                    let _ = manager.detach_session(&meta.worktree_id, &session_id);
                }
            }
            // Phase 6.3: a closed terminal also drops its agent run
            // record (best-effort — plain terminals simply miss).
            {
                let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(registry) = agents.as_ref() {
                    registry.drop_run(&session_id);
                }
            }
            broadcast(
                &shared,
                &Event::TerminalClosed {
                    workspace_id: workspace_id.clone(),
                    session_id: session_id.clone(),
                },
            );
            reply(&stream, Ok(Response::TerminalSnapshot(meta)));
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ReadSessionHistory {
            session_id,
            max_bytes,
        } => {
            match daemon_call(
                &shared,
                DaemonRequest::ReadSessionHistory {
                    session_id: session_id.clone(),
                    max_bytes,
                },
            ) {
                Ok(DaemonResponse::SessionHistory { record, tail }) => {
                    reply(&stream, Ok(Response::SessionHistory { record, tail }))
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ListSessionHistory => {
            match daemon_call(&shared, DaemonRequest::ListSessionHistory) {
                Ok(DaemonResponse::SessionHistoryList { records }) => {
                    reply(&stream, Ok(Response::SessionHistoryList { records }))
                }
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::PurgeHomeHistory { home_id } => {
            match daemon_call(
                &shared,
                DaemonRequest::PurgeHomeHistory {
                    home_id: home_id.clone(),
                },
            ) {
                Ok(DaemonResponse::Ack) => reply(&stream, Ok(Response::Ack)),
                Ok(other) => reply(&stream, Err(format!("unexpected response: {other:?}"))),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(target_family = "wasm")]
        Request::CreateTerminal { .. }
        | Request::ListTerminals { .. }
        | Request::GetTerminal { .. }
        | Request::SendTerminalInput { .. }
        | Request::ReadTerminal { .. }
        | Request::ReadTerminalScreen { .. }
        | Request::ReadTerminalSince { .. }
        | Request::SubscribeTerminal { .. }
        | Request::SubscribeTerminalOperations { .. }
        | Request::ResizeTerminal { .. }
        | Request::SetTerminalColors { .. }
        | Request::CloseTerminal { .. }
        | Request::ReadSessionHistory { .. }
        | Request::ListSessionHistory
        | Request::PurgeHomeHistory { .. } => {
            reply(&stream, Err("terminals unavailable on wasm".to_string()))
        }
        // ── Phase 6.3: agent runs in terminal sessions ──
        #[cfg(not(target_family = "wasm"))]
        Request::ListAgentConfigs => {
            let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
            match agents.as_ref() {
                Some(registry) => reply(
                    &stream,
                    Ok(Response::AgentConfigList {
                        configs: registry.list_configs(),
                    }),
                ),
                None => reply(&stream, Err("agent authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::AddAgentConfig {
            label,
            command,
            args,
            prompt_transport,
            prompt_args,
            env,
        } => {
            let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
            match agents.as_ref() {
                Some(registry) => match registry.add_config(
                    &label,
                    &command,
                    args,
                    prompt_transport,
                    prompt_args,
                    env,
                ) {
                    Ok(config) => reply(&stream, Ok(Response::AgentConfigAdded { config })),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err("agent authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::RunAgent {
            workspace_id,
            worktree_id,
            agent,
            prompt,
            dispatch,
        } => {
            // Resolve + validate the home first (ownership gate), then drop
            // locks before the (slow, gated) PTY spawn — same order as
            // CreateTerminal.
            let home = match owned_home(&shared, &workspace_id, &worktree_id) {
                Ok(home) => home,
                Err(error) => {
                    reply(&stream, Err(error));
                    return;
                }
            };
            match run_agent_inner(&shared, &home, &agent, &prompt, &dispatch) {
                Ok(run) => {
                    broadcast(
                        &shared,
                        &Event::AgentRunStarted {
                            workspace_id: workspace_id.clone(),
                            worktree_id: worktree_id.clone(),
                            run_id: run.id.clone(),
                        },
                    );
                    reply(&stream, Ok(Response::AgentRunStarted { run }));
                }
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::GetAgentRun { run_id } => {
            let base = {
                let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                match agents.as_ref() {
                    Some(registry) => registry.get_record(&run_id),
                    None => {
                        reply(&stream, Err("agent authority unavailable".to_string()));
                        return;
                    }
                }
            };
            match base {
                Some(base) => match agent_snapshot(&shared, &base) {
                    Some(run) => reply(&stream, Ok(Response::AgentRunSnapshot { run })),
                    None => {
                        // Terminal is gone (closed out of band): drop the
                        // stale record and report unknown.
                        let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(registry) = agents.as_ref() {
                            registry.drop_run(&run_id);
                        }
                        reply(&stream, Err(format!("unknown agent run {run_id}")));
                    }
                },
                None => reply(&stream, Err(format!("unknown agent run {run_id}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ListAgentRuns { workspace_id } => {
            let bases = {
                let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                match agents.as_ref() {
                    Some(registry) => registry.list_records(&workspace_id),
                    None => {
                        reply(&stream, Err("agent authority unavailable".to_string()));
                        return;
                    }
                }
            };
            let mut runs = Vec::new();
            let mut stale = Vec::new();
            for base in bases {
                match agent_snapshot(&shared, &base) {
                    Some(run) => runs.push(run),
                    None => stale.push(base.id.clone()),
                }
            }
            if !stale.is_empty() {
                let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(registry) = agents.as_ref() {
                    registry.drop_runs_for_sessions(&stale);
                }
            }
            reply(&stream, Ok(Response::AgentRunList { runs }))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::CheckAgentReadiness { agent } => {
            let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
            match agents.as_ref() {
                Some(registry) => reply(
                    &stream,
                    Ok(Response::AgentReadinessReport {
                        readiness: super::agents::check_readiness(registry, &agent),
                    }),
                ),
                None => reply(&stream, Err("agent authority unavailable".to_string())),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ProposeAgentRuns { workspace_id } => {
            // Unknown workspaces propose nothing (not an error — the UI
            // polls this for workspaces that may not exist yet).
            let proposals = {
                let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
                match workspaces
                    .as_ref()
                    .and_then(|r| r.state_cloned(&workspace_id))
                {
                    Some(state) => super::router::route_launchable(&state),
                    None => Vec::new(),
                }
            };
            reply(&stream, Ok(Response::AgentRouteList { proposals }));
        }
        #[cfg(not(target_family = "wasm"))]
        Request::WatchWorkspace { workspace_id } => {
            // Resolve the project root from canonical state; watching is
            // meaningless (and refused) for unknown workspaces.
            let root = {
                let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
                workspaces
                    .as_ref()
                    .and_then(|r| r.state_cloned(&workspace_id))
                    .map(|state| state.project().root.clone())
            };
            match root {
                Some(root) => match shared.watch.watch(&workspace_id, &root) {
                    Ok(()) => reply(&stream, Ok(Response::Ack)),
                    Err(error) => reply(&stream, Err(format!("{error:#}"))),
                },
                None => reply(&stream, Err(format!("unknown workspace {workspace_id}"))),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::UnwatchWorkspace { workspace_id } => {
            shared.watch.unwatch(&workspace_id);
            reply(&stream, Ok(Response::Ack));
        }
        #[cfg(not(target_family = "wasm"))]
        Request::ListWatches => {
            reply(
                &stream,
                Ok(Response::WatchList {
                    workspace_ids: shared.watch.watched_workspaces(),
                }),
            );
        }
        #[cfg(target_family = "wasm")]
        Request::ListAgentConfigs
        | Request::AddAgentConfig { .. }
        | Request::RunAgent { .. }
        | Request::GetAgentRun { .. }
        | Request::ListAgentRuns { .. }
        | Request::CheckAgentReadiness { .. }
        | Request::ProposeAgentRuns { .. }
        | Request::WatchWorkspace { .. }
        | Request::UnwatchWorkspace { .. }
        | Request::ListWatches => reply(&stream, Err("agents unavailable on wasm".to_string())),
        #[cfg(not(target_family = "wasm"))]
        Request::GetRestorePlan {
            workspace_id,
            known_session_ids,
            max_tail_lines,
        } => {
            // One call re-attaches a whole workspace: live homes, live
            // sessions with bounded tails, and the known ids that are gone.
            let tail_lines = max_tail_lines.unwrap_or(200).clamp(1, 2000);
            let homes = {
                let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
                match worktrees.as_ref() {
                    Some(manager) => manager.list(&workspace_id),
                    None => {
                        reply(&stream, Err("worktree authority unavailable".to_string()));
                        return;
                    }
                }
            };
            let listed = match daemon_call(
                &shared,
                DaemonRequest::ListTerminals {
                    workspace_id: workspace_id.clone(),
                },
            ) {
                Ok(DaemonResponse::TerminalList { sessions }) => sessions,
                Ok(other) => {
                    reply(&stream, Err(format!("unexpected response: {other:?}")));
                    return;
                }
                Err(error) => {
                    reply(&stream, Err(error));
                    return;
                }
            };
            let mut sessions = Vec::new();
            let mut gone: Vec<String> = Vec::new();
            for record in listed {
                match daemon_call(
                    &shared,
                    DaemonRequest::ReadTerminal {
                        session_id: record.id.clone(),
                        max_lines: Some(tail_lines),
                    },
                ) {
                    Ok(DaemonResponse::TerminalOutput {
                        text: tail,
                        alive,
                        exit_code,
                        ..
                    }) => {
                        sessions.push(crate::protocol::RestoreSession {
                            session: record,
                            tail,
                            alive,
                            exit_code,
                        });
                    }
                    _ => gone.push(record.id),
                }
            }
            let live: std::collections::HashSet<&str> =
                sessions.iter().map(|s| s.session.id.as_str()).collect();
            for known in &known_session_ids {
                if !live.contains(known.as_str()) && !gone.iter().any(|g| g == known) {
                    gone.push(known.clone());
                }
            }
            gone.sort();
            // Cold restore: workspace-filtered disk tombstones. Best-effort
            // — history failures never fail the plan.
            let history = match daemon_call(&shared, DaemonRequest::ListSessionHistory) {
                Ok(DaemonResponse::SessionHistoryList { records }) => records
                    .into_iter()
                    .filter(|r| r.workspace_id == workspace_id)
                    .collect(),
                _ => Vec::new(),
            };
            reply(
                &stream,
                Ok(Response::RestorePlanSnapshot(
                    crate::protocol::RestorePlan {
                        workspace_id,
                        homes,
                        sessions,
                        gone_session_ids: gone,
                        history,
                    },
                )),
            );
        }
        #[cfg(target_family = "wasm")]
        Request::GetRestorePlan { .. } => {
            reply(&stream, Err("restore unavailable on wasm".to_string()))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::RestartPtyDaemon => match shared.pty.restart() {
            Ok(_) => reply(&stream, Ok(Response::Ack)),
            Err(error) => reply(&stream, Err(error)),
        },
        #[cfg(not(target_family = "wasm"))]
        Request::GetPtyStatus => {
            let status = shared.pty.status();
            reply(
                &stream,
                Ok(Response::PtyStatusReport {
                    daemon_pid: status.daemon_pid,
                    daemon_id: status.daemon_id,
                    owned: status.owned,
                    circuit_open: status.circuit_open,
                    crash_attempts: status.crash_attempts,
                }),
            )
        }
        #[cfg(target_family = "wasm")]
        Request::RestartPtyDaemon | Request::GetPtyStatus => {
            reply(&stream, Err("pty unavailable on wasm".to_string()))
        }
        #[cfg(not(target_family = "wasm"))]
        Request::SubscribeTerminal {
            workspace_id,
            session_id,
            offset,
        } => {
            // Ownership gate first, then relay the daemon's stream.
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            let daemon = shared
                .pty
                .ensure()
                .and_then(|conn| {
                    conn.open_stream(DaemonRequest::SubscribeTerminal {
                        session_id: session_id.clone(),
                        offset,
                    })
                })
                .map_err(|e| format!("pty daemon unavailable: {e}"));
            match daemon {
                Ok(daemon) => relay_terminal_stream(&stream, &daemon),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Request::SubscribeTerminalOperations {
            workspace_id,
            session_id,
            offset,
        } => {
            // Ownership gate first, then relay the daemon's stream.
            if let Err(error) = daemon_session(&shared, &workspace_id, &session_id) {
                reply(&stream, Err(error));
                return;
            }
            let daemon = shared
                .pty
                .ensure()
                .and_then(|conn| {
                    conn.open_stream(DaemonRequest::SubscribeTerminalOperations {
                        session_id: session_id.clone(),
                        offset,
                    })
                })
                .map_err(|e| format!("pty daemon unavailable: {e}"));
            match daemon {
                Ok(daemon) => relay_terminal_stream(&stream, &daemon),
                Err(error) => reply(&stream, Err(error)),
            }
        }
        Request::Subscribe => {
            // Long-lived stream: ack is implicit — the client reads event
            // lines until EOF. Send HostReady first so the client can confirm
            // the subscription without a second round trip.
            let first = super::protocol::encode_message(&EventEnvelope {
                event: Event::HostReady {
                    host_id: shared.info.host_id.clone(),
                },
            })
            .unwrap_or_default();
            let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
            {
                let mut subs = shared.subscribers.lock().unwrap_or_else(|p| p.into_inner());
                subs.push(tx);
            }
            if super::protocol::write_frame(&mut &stream, &first).is_err() {
                return;
            }
            // Keep the stream alive during quiet periods and detect clients
            // that disconnected without another event to broadcast.
            loop {
                let next = match rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(queued) => queued,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        super::protocol::encode_message(&EventEnvelope {
                            event: Event::HeartbeatAck {
                                client_id: envelope.client_id.clone(),
                            },
                        })
                        .unwrap_or_default()
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if super::protocol::write_frame(&mut &stream, &next).is_err()
                    || shared.shutdown.load(Ordering::SeqCst)
                {
                    break;
                }
            }
        }
        Request::Shutdown => unreachable!(),
    }
}

/// Run one git command for a client (all of the app's git goes through the
/// host). Stdin is fed from a separate thread so a large patch can't
/// deadlock against git's own output.
fn run_git_process(
    root: &str,
    args: &[String],
    stdin: &[u8],
    env: &[(String, String)],
    env_remove: &[String],
) -> std::io::Result<std::process::Output> {
    use std::process::Stdio;
    let mut command = crate::git_command();
    command
        .args(args)
        .current_dir(root)
        .stdin(if stdin.is_empty() {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    for key in env_remove {
        command.env_remove(key);
    }
    // No console window flashing up for each git call on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let writer = child.stdin.take().map(|mut pipe| {
        let input = stdin.to_vec();
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut pipe, &input);
        })
    });
    let output = child.wait_with_output();
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    output
}

/// Relay one daemon output stream to a client until either side ends.
///
/// Backpressure passes straight through: while the client is slow this
/// thread blocks writing to it and stops reading the daemon, so the daemon's
/// queue for this subscriber grows and it pauses the PTY. The write timeout
/// sits just above the daemon's stuck-subscriber limit so that limit, not
/// this relay, decides when a slow client is dropped.
#[cfg(not(target_family = "wasm"))]
fn relay_terminal_stream(client: &Stream, daemon: &Stream) {
    let _ = client.set_read_timeout(None);
    let _ = client.set_write_timeout(Some(
        crate::fanout::STUCK_SUBSCRIBER_TIMEOUT + Duration::from_secs(5),
    ));
    loop {
        let env: DaemonResponseEnvelope = match super::protocol::read_message(&mut &*daemon) {
            Ok(env) => env,
            Err(error) => {
                log::info!("terminal relay: daemon stream ended: {error}");
                break;
            }
        };
        let (result, last) = match env.result {
            Ok(DaemonResponse::TerminalData {
                data,
                base_offset,
                total_bytes,
                exit_code,
                ..
            }) => (
                Ok(Response::TerminalData {
                    data,
                    base: base_offset,
                    total: total_bytes,
                    exit_code,
                }),
                exit_code.is_some(),
            ),
            Ok(DaemonResponse::TerminalOperations { frame }) => {
                (Ok(Response::TerminalOperations { frame }), false)
            }
            Ok(other) => (Err(format!("unexpected daemon response: {other:?}")), true),
            Err(error) => (Err(error), true),
        };
        let envelope = ResponseEnvelope {
            version: PROTOCOL_VERSION,
            result,
        };
        let mut out = client;
        if let Err(error) = super::protocol::write_message(&mut out, &envelope) {
            log::info!("terminal relay: client write failed: {error}");
            break;
        }
        if last {
            break;
        }
    }
    // Dropping the daemon connection ends the subscription there.
    daemon.shutdown();
}

/// One headless scheduler pass over every managed workspace. Pure
/// evaluations; broadcasts fire only on decision change.
#[cfg(not(target_family = "wasm"))]
fn drive_all_workspaces(shared: &Shared) {
    let snapshots: Vec<(String, dragent_workspace_model::WorkspaceState)> = {
        let workspaces = shared.workspaces.lock().unwrap_or_else(|p| p.into_inner());
        match workspaces.as_ref() {
            Some(registry) => registry.snapshots(),
            None => return,
        }
    };
    for (id, projection) in crate::scheduler::drive_once(&shared.scheduler, &snapshots) {
        broadcast(
            shared,
            &Event::SchedulerEvaluated {
                workspace_id: id,
                ready_tasks: projection.ready_task_ids.len(),
                launchable_tasks: projection.launchable_task_ids.len(),
                work_complete: projection.work_complete,
            },
        );
    }
    enforce_lease_expiry(shared, &snapshots);
}

/// Close runs whose execution lease is spent. Only runs carrying a
/// lease id are ever touched (unmanaged runs are invisible here); a
/// lease that cannot be parsed or resolved in canonical state is left
/// alone — absence of evidence is not evidence of expiry, and the
/// capacity gate already bounds the unmanaged.
#[cfg(not(target_family = "wasm"))]
fn enforce_lease_expiry(
    shared: &Shared,
    snapshots: &[(String, dragent_workspace_model::WorkspaceState)],
) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let tracked: Vec<(String, String, Option<String>)> = {
        let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
        match agents.as_ref() {
            Some(registry) => registry
                .all_tracked_runs()
                .into_iter()
                .map(|(workspace_id, session_id)| {
                    let lease_id = registry
                        .run_meta(&session_id)
                        .and_then(|meta| meta.lease_id.clone());
                    (workspace_id, session_id, lease_id)
                })
                .collect(),
            None => return,
        }
    };
    let by_workspace: std::collections::HashMap<&str, &dragent_workspace_model::WorkspaceState> =
        snapshots
            .iter()
            .map(|(id, state)| (id.as_str(), state))
            .collect();
    for (workspace_id, session_id, lease_id) in tracked {
        let Some(lease_id) = lease_id else {
            continue;
        };
        let Ok(lease_uuid) = lease_id.parse::<uuid::Uuid>() else {
            continue;
        };
        let lease_id = dragent_workspace_model::ExecutionLeaseId(lease_uuid);
        let spent = match by_workspace.get(workspace_id.as_str()) {
            Some(state) => match state.execution_lease(lease_id) {
                Some(lease) => crate::scheduler::lease_is_spent(lease, now),
                // Workspace exists but the lease is gone (revoked/
                // never committed): the run has no authority.
                None => true,
            },
            // Workspace itself gone: nothing to judge by — leave it.
            None => false,
        };
        if spent {
            log::info!("lease {lease_id:?} spent — closing run {session_id}");
            shared.pty.close_session(&session_id);
            let agents = shared.agents.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(registry) = agents.as_ref() {
                registry.drop_run(&session_id);
            }
        }
    }
}

/// Run the host service loop on an ephemeral loopback port. Blocks until a
/// token-gated `Shutdown` arrives.
pub fn run_service() -> anyhow::Result<()> {
    let _authority = discovery::acquire_host_authority()?
        .ok_or_else(|| anyhow::anyhow!("another dragent host owns this endpoint"))?;
    // Unix domain socket next to the endpoint file (macOS/Linux; the
    // authority lock above makes this host its only owner), loopback TCP on
    // Windows or if the socket can't be created.
    let listener = Listener::bind_local(
        Some(&super::pty_supervisor::org_dir().join(HOST_SOCKET_FILENAME)),
        &[],
    )
    .map_err(|e| anyhow::anyhow!("host bind: {e}"))?;
    let port = listener.port();
    let token = generate_token();
    let host_id = generate_host_id();
    let info = HostInfo {
        host_id: host_id.clone(),
        version: PROTOCOL_VERSION,
        pid: std::process::id(),
        started_at_unix_ms: now_ms(),
    };
    let endpoint = ServiceEndpoint {
        version: PROTOCOL_VERSION,
        build_id: super::build_identity::current().to_owned(),
        port,
        socket: listener.socket_path(),
        token: token.clone(),
        pid: std::process::id(),
        host_id: host_id.clone(),
        started_at_unix_ms: info.started_at_unix_ms,
    };
    discovery::write_endpoint(&endpoint).map_err(|e| anyhow::anyhow!("{e}"))?;
    log::info!(
        "dragent-host up id={host_id} pid={} port={port}",
        std::process::id()
    );
    #[cfg(not(target_family = "wasm"))]
    let workspaces = match super::workspaces::WorkspaceRegistry::open() {
        Ok(registry) => Some(registry),
        Err(error) => {
            log::warn!("workspace authority unavailable: {error:#}");
            None
        }
    };
    #[cfg(not(target_family = "wasm"))]
    let worktrees = match super::discovery::endpoint_path()
        .parent()
        .map(|p| p.to_path_buf())
    {
        Some(org_dir) => match super::worktrees::WorktreeManager::open_deferred_reconcile(&org_dir)
        {
            Ok(manager) => Some(manager),
            Err(error) => {
                log::warn!("worktree authority unavailable: {error:#}");
                None
            }
        },
        None => {
            log::warn!("worktree authority unavailable: endpoint path has no parent");
            None
        }
    };
    #[cfg(not(target_family = "wasm"))]
    let agents = match super::discovery::endpoint_path()
        .parent()
        .map(|p| p.to_path_buf())
    {
        Some(org_dir) => match super::agents::AgentRegistry::open(&org_dir) {
            Ok(registry) => Some(registry),
            Err(error) => {
                log::warn!("agent authority unavailable: {error:#}");
                None
            }
        },
        None => {
            log::warn!("agent authority unavailable: endpoint path has no parent");
            None
        }
    };
    #[cfg(not(target_family = "wasm"))]
    let pty = {
        let org_dir = super::pty_supervisor::org_dir();
        // Both service entry points support PTY_SERVICE_ARG. Use this exact
        // build: a sibling dragent-host may be left over from an older cargo
        // build and speak a different daemon protocol, causing a crash loop.
        let exe =
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("dragent-host"));
        Arc::new(super::pty_supervisor::PtySupervisor::new(org_dir, exe))
    };
    let shared = Arc::new(Shared {
        token,
        info,
        shutdown: Arc::new(AtomicBool::new(false)),
        preserve_pty_on_exit: AtomicBool::new(false),
        clients: Mutex::new(HashMap::new()),
        subscribers: Mutex::new(Vec::new()),
        #[cfg(not(target_family = "wasm"))]
        workspaces: Mutex::new(workspaces),
        #[cfg(not(target_family = "wasm"))]
        worktrees: Mutex::new(worktrees),
        #[cfg(not(target_family = "wasm"))]
        agents: Mutex::new(agents),
        #[cfg(not(target_family = "wasm"))]
        pty,
        #[cfg(not(target_family = "wasm"))]
        watch: Arc::new(super::watch::WatchRegistry::new()),
        scheduler: crate::scheduler::SchedulerDriver::default(),
    });
    #[cfg(not(target_family = "wasm"))]
    {
        let worktrees = shared.worktrees.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(manager) = worktrees.as_ref() {
            let report = manager.reconcile_with(|sessions, home_id| {
                for session_id in sessions {
                    shared.pty.close_session(session_id);
                }
                if let Ok(conn) = shared.pty.ensure() {
                    let _ = conn.rpc(DaemonRequest::PurgeHomeHistory {
                        home_id: home_id.to_string(),
                    });
                }
            });
            if report.resumed > 0 || report.stranded > 0 || !report.warnings.is_empty() {
                log::info!(
                    "worktree reconcile: resumed={} stranded={} warnings={:?}",
                    report.resumed,
                    report.stranded,
                    report.warnings
                );
            }
        }
    }
    // Project file watcher: one git-poll loop for all watched roots.
    // Changes fan out as WorkspaceFilesChanged events. Stops with the host.
    #[cfg(not(target_family = "wasm"))]
    {
        let watch_registry = shared.watch.clone();
        let watch_down = shared.shutdown.clone();
        let watch_shared = shared.clone();
        std::thread::spawn(move || {
            super::watch::drive(watch_registry, watch_down, move |workspace_id, paths| {
                broadcast(
                    &watch_shared,
                    &Event::WorkspaceFilesChanged {
                        workspace_id,
                        paths,
                    },
                );
            });
        });
    }
    // PTY daemon watcher (owns no sessions itself — adopt/respawn only).
    // Stops with the host.
    #[cfg(not(target_family = "wasm"))]
    {
        let watch_sup = shared.pty.clone();
        let watch_down = shared.shutdown.clone();
        std::thread::spawn(move || {
            super::pty_supervisor::PtySupervisor::watch(watch_sup, watch_down)
        });
    }
    // Team courier: carries a team's news and messages into its agents'
    // sessions while they sit idle. Stops with the host.
    #[cfg(not(target_family = "wasm"))]
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let sessions = TeamSessions { shared: shared.clone() };
            super::team_courier::run(&sessions, &|| shared.shutdown.load(Ordering::SeqCst));
        });
    }
    // Agents that stopped without reporting it: their terminals fall silent.
    // Stops with the host.
    #[cfg(not(target_family = "wasm"))]
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let terminals = TeamSessions { shared: shared.clone() };
            let mut watch = super::agent_quiet::Watch::default();
            while !shared.shutdown.load(Ordering::SeqCst) {
                let agents = super::agent_sessions::unsettled();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as i64)
                    .unwrap_or(0);
                let stopped = watch.stopped(now, &agents, &terminals);
                for agent in agents.iter().filter(|agent| stopped.contains(&agent.pane)) {
                    super::agent_quiet::settle(agent);
                }
                std::thread::sleep(Duration::from_millis(super::agent_quiet::POLL_MS));
            }
        });
    }
    // Headless driver loop (Phase 4): evaluates every managed workspace on a
    // tick with zero UI required. Stops with the host.
    #[cfg(not(target_family = "wasm"))]
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            while !shared.shutdown.load(Ordering::SeqCst) {
                drive_all_workspaces(&shared);
                std::thread::sleep(crate::scheduler::DRIVER_TICK);
            }
        });
    }
    broadcast(&shared, &Event::HostReady { host_id });
    listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                // Local-only enforcement (always true for a Unix socket).
                if !stream.is_local_peer() {
                    continue;
                }
                let shared = shared.clone();
                std::thread::spawn(move || handle_connection(stream, shared));
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
                if shared.shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
    }
    // Drain: give in-flight handlers a moment, then exit.
    std::thread::sleep(Duration::from_millis(250));
    #[cfg(not(target_family = "wasm"))]
    if !shared.preserve_pty_on_exit.load(Ordering::SeqCst) {
        shared.pty.stop_owned();
    }
    listener.remove_socket_file();
    discovery::remove_owned_endpoint(&shared.token);
    log::info!("dragent-host exiting id={}", shared.info.host_id);
    Ok(())
}
