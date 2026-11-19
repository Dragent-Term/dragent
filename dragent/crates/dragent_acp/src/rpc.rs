//! The Agent Client Protocol over [`Peer`], and the vocabulary every chat
//! driver shares (errors, permission requests, events).

use std::io::{BufRead, Write};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::jsonrpc::{Incoming, Peer, Responder};
use crate::options::{SessionOptions, SetVia};
use crate::update::{ToolCall, Update};
use crate::{Session, StopReason};

/// An error the agent answered with, or the agent going away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub(crate) fn gone() -> Self {
        RpcError { code: -1, message: "The agent stopped running.".into() }
    }
    pub(crate) fn timed_out(method: &str) -> Self {
        RpcError { code: -2, message: format!("The agent didn't answer ({method}).") }
    }
    pub(crate) fn other(message: impl Into<String>) -> Self {
        RpcError { code: -4, message: message.into() }
    }
    /// The agent needs signing in before it can work.
    pub fn needs_login(&self) -> bool {
        let m = self.message.to_ascii_lowercase();
        self.code == -32000 || m.contains("auth") || m.contains("log in") || m.contains("login")
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RpcError {}

/// A choice offered with a permission request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub kind: String,
}

/// The agent asks before running a tool.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRequest {
    /// What the driver needs to answer it; pass back to `answer_permission`.
    pub request: Value,
    pub session_id: String,
    pub tool: ToolCall,
    pub options: Vec<PermissionOption>,
}

/// What the agent says without being asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Update { session_id: String, update: Update },
    Permission(PermissionRequest),
    /// The agent's output ended: it exited or crashed.
    Exited,
}

/// One agent spoken to in ACP.
pub struct Agent {
    peer: Peer,
    events: Mutex<Option<mpsc::Receiver<Event>>>,
}

/// Turn what an ACP agent starts into events.
fn handler(events: mpsc::Sender<Event>) -> impl FnMut(Incoming, &Responder) + Send + 'static {
    move |incoming, responder| match incoming {
        Incoming::Request { id, method, params } if method == "session/request_permission" => {
            let mut tool = params.get("toolCall").cloned().unwrap_or(Value::Null);
            if let Some(object) = tool.as_object_mut() {
                object.insert("sessionUpdate".into(), "tool_call".into());
            }
            let tool = match Update::parse(&tool) {
                Update::ToolCall(call) => call,
                _ => ToolCall::default(),
            };
            let options = params
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|o| {
                    Some(PermissionOption {
                        id: o.get("optionId")?.as_str()?.to_string(),
                        name: o.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        kind: o.get("kind").and_then(Value::as_str).unwrap_or_default().to_string(),
                    })
                })
                .collect();
            let session_id = params.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
            let _ = events.send(Event::Permission(PermissionRequest { request: id, session_id, tool, options }));
        }
        // Nothing else was offered in `initialize`.
        Incoming::Request { id, method, .. } => {
            let _ = responder.error(&id, -32601, &format!("{method} is not supported"));
        }
        Incoming::Notification { method, params } if method == "session/update" => {
            let session_id = params.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
            let update = Update::parse(params.get("update").unwrap_or(&Value::Null));
            let _ = events.send(Event::Update { session_id, update });
        }
        Incoming::Notification { .. } => {}
        Incoming::Closed => {
            let _ = events.send(Event::Exited);
        }
    }
}

impl Agent {
    /// Start `program args…` in `cwd` and talk ACP with it.
    pub fn spawn(
        program: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
    ) -> std::io::Result<Agent> {
        let (tx, rx) = mpsc::channel();
        let peer = Peer::spawn(program, args, cwd, env, true, handler(tx))?;
        Ok(Agent { peer, events: Mutex::new(Some(rx)) })
    }

    /// Talk ACP over any reader and writer (a process's pipes, or a test's).
    pub fn over(reader: impl BufRead + Send + 'static, writer: impl Write + Send + 'static) -> Agent {
        let (tx, rx) = mpsc::channel();
        let peer = Peer::over(reader, writer, true, handler(tx));
        Agent { peer, events: Mutex::new(Some(rx)) }
    }

    /// The agent's own messages. Taken once.
    pub fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        self.events.lock().ok()?.take()
    }

    pub fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, RpcError> {
        self.peer.request(method, params, timeout)
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        self.peer.notify(method, params)
    }

    /// The handshake. This client reads and writes files itself and runs no
    /// terminals for the agent, so it offers neither.
    pub fn initialize(&self) -> Result<Value, RpcError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": "dragent", "title": "Dragent", "version": env!("CARGO_PKG_VERSION")},
            }),
            Some(Duration::from_secs(60)),
        )
    }

    /// A new conversation in `cwd`.
    pub fn new_session(&self, cwd: &std::path::Path) -> Result<Session, RpcError> {
        let result = self.request(
            "session/new",
            json!({"cwd": cwd.to_string_lossy(), "mcpServers": []}),
            Some(Duration::from_secs(120)),
        )?;
        let id = result.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(Session { id, options: SessionOptions::parse(&result) })
    }

    /// Open an earlier conversation: the agent replays it as updates first.
    pub fn load_session(&self, id: &str, cwd: &std::path::Path) -> Result<Session, RpcError> {
        let result = self.request(
            "session/load",
            json!({"sessionId": id, "cwd": cwd.to_string_lossy(), "mcpServers": []}),
            Some(Duration::from_secs(120)),
        )?;
        Ok(Session { id: id.to_string(), options: SessionOptions::parse(&result) })
    }

    /// Send a message and wait for the turn to end.
    pub fn prompt(&self, session_id: &str, text: &str) -> Result<StopReason, RpcError> {
        let result = self.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
            None,
        )?;
        Ok(StopReason::parse(result.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn")))
    }

    /// Stop the current turn. Its `prompt` then returns `Cancelled`.
    pub fn cancel(&self, session_id: &str) -> Result<(), RpcError> {
        self.notify("session/cancel", json!({"sessionId": session_id}))
    }

    /// Choose `value` in the picker set `via`. Returns the agent's new
    /// options when it sends them back.
    pub fn set_option(&self, session_id: &str, via: &SetVia, value: &str) -> Result<Option<SessionOptions>, RpcError> {
        let limit = Some(Duration::from_secs(60));
        match via {
            SetVia::ConfigOption(config) => {
                let result = self.request(
                    "session/set_config_option",
                    json!({"sessionId": session_id, "configId": config, "value": value}),
                    limit,
                )?;
                Ok(result.get("configOptions").is_some().then(|| SessionOptions::parse(&result)))
            }
            SetVia::Model => {
                self.request("session/set_model", json!({"sessionId": session_id, "modelId": value}), limit)?;
                Ok(None)
            }
            SetVia::Mode => {
                self.request("session/set_mode", json!({"sessionId": session_id, "modeId": value}), limit)?;
                Ok(None)
            }
        }
    }

    /// Answer a permission request with one of its options, or `None` to
    /// decline answering (the turn was cancelled).
    pub fn answer_permission(&self, request: &Value, option_id: Option<&str>) -> Result<(), RpcError> {
        let outcome = match option_id {
            Some(id) => json!({"outcome": "selected", "optionId": id}),
            None => json!({"outcome": "cancelled"}),
        };
        self.peer.responder().result(request, json!({"outcome": outcome}))
    }

    /// Stop the agent process.
    pub fn shutdown(&self) {
        self.peer.shutdown();
    }
}

impl crate::ChatDriver for Agent {
    fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        Agent::take_events(self)
    }

    fn open(&self, cwd: &std::path::Path, resume: Option<&str>) -> Result<Session, RpcError> {
        let hello = self.initialize()?;
        let can_load = hello.pointer("/agentCapabilities/loadSession").and_then(Value::as_bool).unwrap_or(false);
        match resume.filter(|_| can_load) {
            Some(id) => self.load_session(id, cwd).or_else(|_| self.new_session(cwd)),
            None => self.new_session(cwd),
        }
    }

    fn replays_history(&self) -> bool {
        true
    }

    fn prompt(&self, session_id: &str, text: &str) -> Result<StopReason, RpcError> {
        Agent::prompt(self, session_id, text)
    }

    fn cancel(&self, session_id: &str) -> Result<(), RpcError> {
        Agent::cancel(self, session_id)
    }

    fn set_option(&self, session_id: &str, via: &SetVia, value: &str) -> Result<Option<SessionOptions>, RpcError> {
        Agent::set_option(self, session_id, via, value)
    }

    fn answer_permission(&self, request: &Value, option_id: Option<&str>) -> Result<(), RpcError> {
        Agent::answer_permission(self, request, option_id)
    }

    fn shutdown(&self) {
        Agent::shutdown(self)
    }

    fn error_output(&self) -> Option<String> {
        self.peer.error_output()
    }
}
