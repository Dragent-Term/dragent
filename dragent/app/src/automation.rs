//! Env-gated E2E automation server.
//!
//! When `DRAGENT_AUTOMATION_PORT` is set, the app exposes a
//! JSON-lines protocol on 127.0.0.1:<port> that drives the REAL app end to
//! end — real PTY, real shell, real OSC hooks, real session core — bypassing
//! only the GUI input widget. This is what lets CI assert "run `npm install`,
//! the block completes" against the shipping binary instead of replayed bytes
//! (see tests/e2e_automation.rs).
//!
//! Off unless the env var is present; binds loopback only. Do not set the
//! variable in production environments.
//!
//! Protocol (one JSON object per line, one JSON reply per line):
//!   {"cmd":"ping"}                     -> {"ok":true}
//!   {"cmd":"run","text":"echo hi"}    -> {"ok":true}         (submits a command)
//!   {"cmd":"blocks"}                  -> {"ok":true,"running":bool,"blocks":[...]}
//!   {"cmd":"index_status"}            -> {"ok":true,"repos":[...]}
//!   {"cmd":"perf_stats"}              -> process/render/content measurements
//!   {"cmd":"workspace_shape","text":"..."} -> submit real Shape input
//!   {"cmd":"workspace_approve"}        -> approve the current shaped plan
//!   {"cmd":"workspace_layout","layout":"coordinate"} -> switch presentation
//!   {"cmd":"workspace_state"}          -> canonical work/layout snapshot
//!   {"cmd":"workspace_browser_open","target":"tab|pane|side","url":"..."}
//!   {"cmd":"workspace_browser_navigate","url":"..."} -> active browser pane
//!   {"cmd":"workspace_open_link","url":"..."} -> as if a link was clicked in the active pane
//!   {"cmd":"workspace_browser_action","action":"back|forward|reload|devtools|split|close|menu"}
//!   {"cmd":"workspace_new_tab_menu"}    -> toggle the header `+` menu
//!   {"cmd":"workspace_open_project","path":"..."} -> open a folder with a terminal
//!   {"cmd":"workspace_open_agent","agent":"codex"} -> open an agent in a new tab
//!   {"cmd":"workspace_select_theme","theme":"..."} -> as if picked in Settings
//!   {"cmd":"workspace_start_prompt","agent":"codex","prompt":"...","project":"..."} -> as if sent on the start screen
//!   {"cmd":"workspace_agent_panes"}    -> each host tab's pane, command and colours
//!   {"cmd":"workspace_commit","message":"...","push":false} -> Source Control commit
//!   {"cmd":"workspace_git_panel"}      -> what Source Control acts on
//!   {"cmd":"workspace_git_sync","pull":true} -> Source Control pull (or push)
//!   {"cmd":"workspace_browsers"}       -> live state of every browser pane
//!   {"cmd":"workspace_diff_comment","action":"open:<path>|start:<path>:<line>|text:..|agents|choose:<key>|send|state"}
//!   {"cmd":"workspace_browser_tool","tool":"snapshot","arguments":{},"pane":null}
//!                                      -> what `dragent browser` would print
//!   {"cmd":"quit"}                    -> {"ok":true} then process exit

#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Once, OnceLock};

use parking_lot::FairMutex;
use serde_json::json;

use crate::terminal::manager::pty_writer::PtyWriter;
use crate::terminal::model::TerminalSessionCore;

mod performance;

/// The first terminal session, once there is one. Terminal commands report an
/// error until then; workspace and agent commands work from the start.
static TERMINAL: OnceLock<std::sync::Mutex<(Arc<FairMutex<TerminalSessionCore>>, PtyWriter)>> =
    OnceLock::new();

fn port() -> Option<u16> {
    std::env::var("DRAGENT_AUTOMATION_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
}

/// Register the first terminal session with the automation server (if
/// `DRAGENT_AUTOMATION_PORT` is set) and start it. Later sessions are no-ops so
/// the (core, writer) pair always refers to the same session.
pub fn maybe_start(core: Arc<FairMutex<TerminalSessionCore>>, writer: PtyWriter) {
    if port().is_none() {
        return;
    }
    let _ = TERMINAL.set(std::sync::Mutex::new((core, writer)));
    start_server();
}

/// Bind the server once, from the first terminal session or the workspace
/// bridge, whichever comes first.
fn start_server() {
    let Some(port) = port() else {
        return;
    };
    static STARTED: Once = Once::new();
    let mut claimed = false;
    STARTED.call_once(|| claimed = true);
    if !claimed {
        return;
    }

    std::thread::Builder::new()
        .name("dragent-automation".into())
        .spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", port)) {
                Ok(l) => l,
                Err(e) => {
                    log::error!("[automation] bind 127.0.0.1:{port} failed: {e}");
                    return;
                }
            };
            log::info!("[automation] listening on 127.0.0.1:{port}");
            for stream in listener.incoming().flatten() {
                if let Err(e) = serve(stream) {
                    log::warn!("[automation] connection ended: {e}");
                }
            }
        })
        .expect("spawn automation server");
}

fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match TERMINAL.get() {
            Some(terminal) => {
                let terminal = terminal.lock().unwrap_or_else(|p| p.into_inner());
                handle(&line, &terminal.0, &terminal.1)
            }
            None => handle_without_terminal(&line),
        };
        out.write_all(reply.to_string().as_bytes())?;
        out.write_all(b"\n")?;
        out.flush()?;
        if reply.get("quit").is_some() {
            // Reply has been flushed; tear the process down like a user quit.
            log::info!("[automation] quit requested");
            std::process::exit(0);
        }
    }
    Ok(())
}

/// Commands available before any terminal session exists.
fn handle_without_terminal(line: &str) -> serde_json::Value {
    let req: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"ok": false, "error": format!("bad json: {e}")}),
    };
    if let Some(reply) = handle_agent_cmd(&req) {
        return reply;
    }
    if let Some(reply) = handle_workspace_cmd(&req) {
        return reply;
    }
    match req.get("cmd").and_then(|c| c.as_str()) {
        Some("ping") => json!({"ok": true}),
        Some("quit") => json!({"ok": true, "quit": true}),
        other => json!({"ok": false, "error": format!("{other:?} needs a terminal session")}),
    }
}

fn handle(
    line: &str,
    core: &Arc<FairMutex<TerminalSessionCore>>,
    writer: &PtyWriter,
) -> serde_json::Value {
    let req: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"ok": false, "error": format!("bad json: {e}")}),
    };
    if let Some(reply) = handle_agent_cmd(&req) {
        return reply;
    }
    if let Some(reply) = handle_workspace_cmd(&req) {
        return reply;
    }
    match req.get("cmd").and_then(|c| c.as_str()) {
        Some("ping") => json!({"ok": true}),
        Some("run") => {
            let Some(text) = req.get("text").and_then(|t| t.as_str()) else {
                return json!({"ok": false, "error": "run requires text"});
            };
            // Same submission path as the input bar on Windows: Ctrl-U clears
            // any stray line state, then the command and CR.
            writer.write(b"\x15");
            writer.write(text.as_bytes());
            writer.write(b"\r");
            json!({"ok": true})
        }
        Some("blocks") => {
            let core = core.lock();
            let blocks: Vec<serde_json::Value> = core
                .blocks()
                .blocks()
                .iter()
                .map(|b| {
                    let text: String = b
                        .lines
                        .iter()
                        .map(|l| l.plain_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    json!({
                        "id": b.id,
                        "command": b.command,
                        "completed": b.is_completed(),
                        "live": b.emulator.is_some(),
                        "text": text,
                    })
                })
                .collect();
            json!({"ok": true, "running": core.is_command_running(), "blocks": blocks})
        }
        Some("index_status") => {
            let repos: Vec<serde_json::Value> = dragent_index::get_manager()
                .index_status()
                .into_iter()
                .map(|s| {
                    json!({
                        "root": s.root.to_string_lossy(),
                        "state": format!("{:?}", s.state),
                        "pending": s.pending,
                        "files": s.files,
                        "chunks": s.chunks,
                        "capped": s.capped,
                    })
                })
                .collect();
            json!({"ok": true, "repos": repos})
        }
        Some("scrollback_stats") => {
            // Live scrollback numbers from the first session's emulator, for
            // verifying the settings-driven limit against a real PTY flood.
            let core = core.lock();
            let grid = core.emulator().grid();
            // Per-block numbers are where command output actually lives.
            let blocks: Vec<serde_json::Value> = core
                .blocks()
                .blocks()
                .iter()
                .map(|b| {
                    let emu = b.emulator.as_ref().map(|e| {
                        let g = e.grid();
                        json!({"scrollback_len": g.scrollback_len(), "evicted": g.evicted()})
                    });
                    json!({
                        "id": b.id,
                        "command": b.command,
                        "lines": b.lines.len(),
                        "emulator": emu,
                    })
                })
                .collect();
            json!({
                "ok": true,
                "scrollback_len": grid.scrollback_len(),
                "evicted": grid.evicted(),
                "blocks": blocks,
            })
        }
        Some("perf_stats") => performance::snapshot(core),
        Some("context_probe") => {
            // Runs the REAL context builder (semantic-index pre-fetch with the
            // user's context-scope setting applied) and reports which fragments
            // it would inject into the agent's system prompt.
            let Some(query) = req.get("query").and_then(|q| q.as_str()) else {
                return json!({"ok": false, "error": "context_probe requires query"});
            };
            let cwd = req
                .get("cwd")
                .and_then(|c| c.as_str())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| core.lock().cwd().clone());
            let scope = crate::settings::AppSettings::global().agent.context_scope;
            let project = crate::agent::context_builder::ProjectContext::build(&cwd, query);
            let chunks: Vec<String> = project
                .relevant_code
                .iter()
                .map(|c| c.path.clone())
                .collect();
            json!({
                "ok": true,
                "scope": format!("{scope:?}"),
                "cwd": cwd.to_string_lossy(),
                "chunks": chunks,
            })
        }
        Some("quit") => json!({"ok": true, "quit": true}),
        other => json!({"ok": false, "error": format!("unknown cmd: {other:?}")}),
    }
}

#[derive(Clone, Debug)]
pub enum WorkspaceAutomationCommand {
    Shape(String),
    Approve,
    SelectLayout(String),
    OpenBrowser { target: String, url: Option<String> },
    NavigateBrowser(String),
    /// Open a link the way a click on it does.
    OpenLink(String),
    BrowserAction(String),
    ToggleNewTabMenu,
    OpenProject(std::path::PathBuf),
    OpenAgent(String),
    StartPrompt { agent: String, prompt: String, project: std::path::PathBuf },
    SelectTheme(String),
    AgentPanes(std::sync::mpsc::Sender<serde_json::Value>),
    Commit { message: String, push: bool },
    GitSync { pull: bool },
    GitPanel(std::sync::mpsc::Sender<serde_json::Value>),
    /// Reply with each browser pane's live state (read on the UI thread).
    Browsers(std::sync::mpsc::Sender<serde_json::Value>),
    /// Drive the diff comment card, then reply with its state.
    DiffComment {
        action: String,
        reply: std::sync::mpsc::Sender<serde_json::Value>,
    },
    /// A `dragent browser` command, without the command line's transport.
    BrowserTool {
        pane: Option<String>,
        tool: String,
        arguments: serde_json::Value,
        reply: std::sync::mpsc::Sender<Result<String, String>>,
    },
}

struct WorkspaceBridgeTx {
    command_tx: async_channel::Sender<WorkspaceAutomationCommand>,
    state: Arc<std::sync::Mutex<serde_json::Value>>,
}

static WORKSPACE_BRIDGE: std::sync::OnceLock<WorkspaceBridgeTx> = std::sync::OnceLock::new();

pub struct WorkspaceBridgeRx {
    pub command_rx: async_channel::Receiver<WorkspaceAutomationCommand>,
    pub state: Arc<std::sync::Mutex<serde_json::Value>>,
}

/// Claim the env-gated bridge from the first workspace window.
pub fn claim_workspace_bridge() -> Option<WorkspaceBridgeRx> {
    std::env::var("DRAGENT_AUTOMATION_PORT").ok()?;
    let (command_tx, command_rx) = async_channel::unbounded();
    let state = Arc::new(std::sync::Mutex::new(json!({
        "ready": false,
        "layout": "develop",
    })));
    WORKSPACE_BRIDGE
        .set(WorkspaceBridgeTx {
            command_tx,
            state: state.clone(),
        })
        .ok()?;
    start_server();
    Some(WorkspaceBridgeRx { command_rx, state })
}

fn handle_workspace_cmd(req: &serde_json::Value) -> Option<serde_json::Value> {
    let cmd = req.get("cmd").and_then(|value| value.as_str())?;
    if !matches!(
        cmd,
        "workspace_shape"
            | "workspace_approve"
            | "workspace_layout"
            | "workspace_state"
            | "workspace_browser_open"
            | "workspace_browser_navigate"
            | "workspace_open_link"
            | "workspace_browser_action"
            | "workspace_new_tab_menu"
            | "workspace_browsers"
            | "workspace_diff_comment"
            | "workspace_browser_tool"
            | "workspace_open_project"
            | "workspace_open_agent"
            | "workspace_start_prompt"
            | "workspace_select_theme"
            | "workspace_agent_panes"
            | "workspace_commit"
            | "workspace_git_panel"
            | "workspace_git_sync"
    ) {
        return None;
    }
    let Some(bridge) = WORKSPACE_BRIDGE.get() else {
        return Some(json!({"ok": false, "error": "workspace bridge not claimed yet"}));
    };
    Some(match cmd {
        "workspace_state" => {
            let state = bridge
                .state
                .lock()
                .map(|state| state.clone())
                .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
            json!({"ok": true, "state": state})
        }
        "workspace_shape" => {
            let Some(text) = req.get("text").and_then(|value| value.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_shape requires text"}));
            };
            match bridge
                .command_tx
                .send_blocking(WorkspaceAutomationCommand::Shape(text.to_string()))
            {
                Ok(()) => json!({"ok": true}),
                Err(error) => json!({"ok": false, "error": format!("send: {error}")}),
            }
        }
        "workspace_approve" => match bridge
            .command_tx
            .send_blocking(WorkspaceAutomationCommand::Approve)
        {
            Ok(()) => json!({"ok": true}),
            Err(error) => json!({"ok": false, "error": format!("send: {error}")}),
        },
        "workspace_layout" => {
            let Some(layout) = req.get("layout").and_then(|value| value.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_layout requires layout"}));
            };
            match bridge
                .command_tx
                .send_blocking(WorkspaceAutomationCommand::SelectLayout(layout.to_string()))
            {
                Ok(()) => json!({"ok": true}),
                Err(error) => json!({"ok": false, "error": format!("send: {error}")}),
            }
        }
        "workspace_browser_open" => {
            let target = req
                .get("target")
                .and_then(|value| value.as_str())
                .unwrap_or("tab")
                .to_string();
            let url = req
                .get("url")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            send_workspace(bridge, WorkspaceAutomationCommand::OpenBrowser { target, url })
        }
        "workspace_browser_navigate" => {
            let Some(url) = req.get("url").and_then(|value| value.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_browser_navigate requires url"}));
            };
            send_workspace(
                bridge,
                WorkspaceAutomationCommand::NavigateBrowser(url.to_string()),
            )
        }
        "workspace_open_link" => {
            let Some(url) = req.get("url").and_then(|value| value.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_open_link requires url"}));
            };
            send_workspace(bridge, WorkspaceAutomationCommand::OpenLink(url.to_string()))
        }
        "workspace_browser_action" => {
            let Some(action) = req.get("action").and_then(|value| value.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_browser_action requires action"}));
            };
            send_workspace(
                bridge,
                WorkspaceAutomationCommand::BrowserAction(action.to_string()),
            )
        }
        "workspace_open_agent" => {
            let Some(agent) = req.get("agent").and_then(|v| v.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_open_agent requires agent"}));
            };
            send_workspace(bridge, WorkspaceAutomationCommand::OpenAgent(agent.to_string()))
        }
        "workspace_start_prompt" => {
            let (Some(agent), Some(prompt), Some(project)) = (
                req.get("agent").and_then(|v| v.as_str()),
                req.get("prompt").and_then(|v| v.as_str()),
                req.get("project").and_then(|v| v.as_str()),
            ) else {
                return Some(json!({"ok": false, "error": "workspace_start_prompt requires agent, prompt and project"}));
            };
            send_workspace(
                bridge,
                WorkspaceAutomationCommand::StartPrompt {
                    agent: agent.to_string(),
                    prompt: prompt.to_string(),
                    project: project.into(),
                },
            )
        }
        "workspace_select_theme" => {
            let Some(theme) = req.get("theme").and_then(|v| v.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_select_theme requires theme"}));
            };
            send_workspace(bridge, WorkspaceAutomationCommand::SelectTheme(theme.to_string()))
        }
        "workspace_agent_panes" => {
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            match bridge
                .command_tx
                .send_blocking(WorkspaceAutomationCommand::AgentPanes(reply_tx))
            {
                Ok(()) => match reply_rx.recv_timeout(std::time::Duration::from_secs(10)) {
                    Ok(panes) => json!({"ok": true, "panes": panes}),
                    Err(error) => json!({"ok": false, "error": error.to_string()}),
                },
                Err(error) => json!({"ok": false, "error": error.to_string()}),
            }
        }
        "workspace_new_tab_menu" => {
            send_workspace(bridge, WorkspaceAutomationCommand::ToggleNewTabMenu)
        }
        "workspace_browsers" => {
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            if let Err(error) = bridge
                .command_tx
                .send_blocking(WorkspaceAutomationCommand::Browsers(reply_tx))
            {
                return Some(json!({"ok": false, "error": format!("send: {error}")}));
            }
            match reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(browsers) => json!({"ok": true, "browsers": browsers}),
                Err(error) => json!({"ok": false, "error": format!("no reply: {error}")}),
            }
        }
        "workspace_diff_comment" => {
            let action = req.get("action").and_then(|value| value.as_str()).unwrap_or("state");
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            if let Err(error) = bridge.command_tx.send_blocking(WorkspaceAutomationCommand::DiffComment {
                action: action.to_string(),
                reply: reply_tx,
            }) {
                return Some(json!({"ok": false, "error": format!("send: {error}")}));
            }
            match reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(state) => state,
                Err(error) => json!({"ok": false, "error": format!("no reply: {error}")}),
            }
        }
        "workspace_open_project" => {
            let Some(path) = req.get("path").and_then(|value| value.as_str()) else {
                return Some(json!({"ok": false, "error": "workspace_open_project requires path"}));
            };
            send_workspace(bridge, WorkspaceAutomationCommand::OpenProject(path.into()))
        }
        "workspace_commit" => send_workspace(
            bridge,
            WorkspaceAutomationCommand::Commit {
                message: req.get("message").and_then(|value| value.as_str()).unwrap_or("Commit").into(),
                push: req.get("push").and_then(|value| value.as_bool()).unwrap_or(false),
            },
        ),
        "workspace_git_sync" => send_workspace(
            bridge,
            WorkspaceAutomationCommand::GitSync {
                pull: req.get("pull").and_then(|value| value.as_bool()).unwrap_or(false),
            },
        ),
        "workspace_git_panel" => {
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            if let Err(error) = bridge.command_tx.send_blocking(WorkspaceAutomationCommand::GitPanel(reply_tx)) {
                return Some(json!({"ok": false, "error": format!("send: {error}")}));
            }
            match reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(panel) => json!({"ok": true, "panel": panel}),
                Err(error) => json!({"ok": false, "error": format!("no reply: {error}")}),
            }
        }
        "workspace_browser_tool" => {
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let command = WorkspaceAutomationCommand::BrowserTool {
                pane: req.get("pane").and_then(|value| value.as_str()).map(str::to_string),
                tool: req.get("tool").and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                arguments: req.get("arguments").cloned().unwrap_or_else(|| json!({})),
                reply: reply_tx,
            };
            if let Err(error) = bridge.command_tx.send_blocking(command) {
                return Some(json!({"ok": false, "error": format!("send: {error}")}));
            }
            match reply_rx.recv_timeout(std::time::Duration::from_secs(45)) {
                Ok(Ok(output)) => json!({"ok": true, "output": output}),
                Ok(Err(error)) => json!({"ok": false, "error": error}),
                Err(error) => json!({"ok": false, "error": format!("no reply: {error}")}),
            }
        }
        _ => unreachable!(),
    })
}

fn send_workspace(
    bridge: &WorkspaceBridgeTx,
    command: WorkspaceAutomationCommand,
) -> serde_json::Value {
    match bridge.command_tx.send_blocking(command) {
        Ok(()) => json!({"ok": true}),
        Err(error) => json!({"ok": false, "error": format!("send: {error}")}),
    }
}

// ── Agent automation bridge ───────────────────────────────────────────────────
//
// Extends the JSON-lines protocol with agent-loop commands so E2E runs can
// drive the REAL agent pipeline (LLM stream, approval policy, tool execution)
// without the GUI:
//   {"cmd":"agent","text":"..."}       -> {"ok":true}   (submits an agent query)
//   {"cmd":"agent_state"}              -> {"ok":true,"streaming":bool,
//                                          "pending_approval":str|null,"blocks":[...]}
//   {"cmd":"resolve_approval","approve":bool} -> {"ok":true} (frontmost pending)
//
// The TCP thread cannot touch warpui models, so commands flow through channels
// consumed by the first TerminalView (spawn_stream_local), and the view pushes
// a serialized block snapshot back through a shared mutex after every
// controller event.

/// Sender half + snapshot store, owned by the automation server.
struct AgentBridgeTx {
    query_tx: async_channel::Sender<String>,
    resolve_tx: async_channel::Sender<bool>,
    sync_tx: async_channel::Sender<()>,
    auto_fix_tx: async_channel::Sender<()>,
    /// Named UI actions ("agent_input", "toggle_mode_menu", …) for E2E driving
    /// the input bar without synthetic mouse/keyboard events.
    ui_tx: async_channel::Sender<String>,
    state: Arc<std::sync::Mutex<serde_json::Value>>,
}

static AGENT_BRIDGE: std::sync::OnceLock<AgentBridgeTx> = std::sync::OnceLock::new();

/// Receiver half, claimed once by the first terminal pane.
pub struct AgentBridgeRx {
    pub query_rx: async_channel::Receiver<String>,
    pub resolve_rx: async_channel::Receiver<bool>,
    pub sync_rx: async_channel::Receiver<()>,
    pub auto_fix_rx: async_channel::Receiver<()>,
    pub ui_rx: async_channel::Receiver<String>,
    pub state: Arc<std::sync::Mutex<serde_json::Value>>,
}

/// Create and claim the agent bridge for the first pane when automation is
/// enabled. Later panes (and non-automation runs) get None.
pub fn claim_agent_bridge() -> Option<AgentBridgeRx> {
    std::env::var("DRAGENT_AUTOMATION_PORT").ok()?;
    static CLAIMED: Once = Once::new();
    let mut claimed = false;
    CLAIMED.call_once(|| claimed = true);
    if !claimed {
        return None;
    }
    let (query_tx, query_rx) = async_channel::unbounded();
    let (resolve_tx, resolve_rx) = async_channel::unbounded();
    let (sync_tx, sync_rx) = async_channel::unbounded();
    let (auto_fix_tx, auto_fix_rx) = async_channel::unbounded();
    let (ui_tx, ui_rx) = async_channel::unbounded();
    let state = Arc::new(std::sync::Mutex::new(json!({})));
    let _ = AGENT_BRIDGE.set(AgentBridgeTx {
        query_tx,
        resolve_tx,
        sync_tx,
        auto_fix_tx,
        ui_tx,
        state: state.clone(),
    });
    Some(AgentBridgeRx {
        query_rx,
        resolve_rx,
        sync_rx,
        auto_fix_rx,
        ui_rx,
        state,
    })
}

/// Handle agent-bridge commands; None = not an agent command.
fn handle_agent_cmd(req: &serde_json::Value) -> Option<serde_json::Value> {
    let cmd = req.get("cmd").and_then(|c| c.as_str())?;
    let bridge = match cmd {
        "agent" | "agent_state" | "resolve_approval" | "sync_settings" | "auto_fix_last" | "ui" => {
            match AGENT_BRIDGE.get() {
                Some(b) => b,
                None => return Some(json!({"ok": false, "error": "agent bridge not claimed yet"})),
            }
        }
        _ => return None,
    };
    Some(match cmd {
        "agent" => {
            let Some(text) = req.get("text").and_then(|t| t.as_str()) else {
                return Some(json!({"ok": false, "error": "agent requires text"}));
            };
            match bridge.query_tx.send_blocking(text.to_string()) {
                Ok(()) => json!({"ok": true}),
                Err(e) => json!({"ok": false, "error": format!("send: {e}")}),
            }
        }
        "agent_state" => {
            let snap = bridge
                .state
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|p| p.into_inner().clone());
            json!({"ok": true, "state": snap})
        }
        "auto_fix_last" => match bridge.auto_fix_tx.send_blocking(()) {
            Ok(()) => json!({"ok": true}),
            Err(e) => json!({"ok": false, "error": format!("send: {e}")}),
        },
        "sync_settings" => match bridge.sync_tx.send_blocking(()) {
            Ok(()) => json!({"ok": true}),
            Err(e) => json!({"ok": false, "error": format!("send: {e}")}),
        },
        "ui" => {
            // Named input-bar action for E2E: {"cmd":"ui","action":"toggle_model_menu"}.
            let Some(action) = req.get("action").and_then(|a| a.as_str()) else {
                return Some(json!({"ok": false, "error": "ui requires action"}));
            };
            match bridge.ui_tx.send_blocking(action.to_string()) {
                Ok(()) => json!({"ok": true}),
                Err(e) => json!({"ok": false, "error": format!("send: {e}")}),
            }
        }
        "resolve_approval" => {
            let approve = req.get("approve").and_then(|a| a.as_bool()).unwrap_or(true);
            match bridge.resolve_tx.send_blocking(approve) {
                Ok(()) => json!({"ok": true}),
                Err(e) => json!({"ok": false, "error": format!("send: {e}")}),
            }
        }
        _ => unreachable!(),
    })
}
