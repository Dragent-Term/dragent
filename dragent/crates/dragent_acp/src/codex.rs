//! Codex as a chat, through `codex app-server`.
//!
//! The app-server is Codex's own JSON-RPC interface (no `jsonrpc` field), the
//! one its IDE extension uses; `codex app-server generate-json-schema` prints
//! it. A conversation is a thread, a message starts a turn, and the turn's
//! work arrives as items (`item/started`, `item/completed`, streamed text in
//! `item/agentMessage/delta`) until `turn/completed` says how it ended.
//! Model and reasoning effort travel with each turn, so choosing them only
//! changes what the next turn asks for.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::jsonrpc::{Incoming, Peer, Responder};
use crate::options::{Choice, Picker, SessionOptions, SetVia};
use crate::rpc::{Event, PermissionOption, PermissionRequest, RpcError};
use crate::update::{ToolCall, ToolContent, ToolPatch, Update};
use crate::{ChatDriver, Session, StopReason};

/// The effort picker's config id.
const EFFORT: &str = "effort";

#[derive(Clone, Debug)]
struct Model {
    id: String,
    name: String,
    description: String,
    efforts: Vec<String>,
    default_effort: Option<String>,
}

/// How a turn ended: its status and, when it failed, why.
type TurnEnd = (String, Option<String>);

#[derive(Default)]
struct State {
    models: Vec<Model>,
    model: String,
    effort: Option<String>,
    /// The running turn of each thread.
    turns: HashMap<String, String>,
    /// Who waits for each thread's turn to end.
    waiting: HashMap<String, mpsc::Sender<TurnEnd>>,
    /// Messages whose text arrived in pieces (their completed copy is skipped).
    streamed: HashSet<String>,
}

/// Codex driven through its app-server.
pub struct Codex {
    peer: Peer,
    events: Mutex<Option<mpsc::Receiver<Event>>>,
    state: Arc<Mutex<State>>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The command as typed: `/bin/zsh -lc ls` is `ls`.
fn command_of(item: &Value) -> String {
    let command = text(item, "command").unwrap_or_default();
    for shell in ["-lc ", "-c "] {
        if let Some((_, rest)) = command.split_once(shell) {
            return rest.trim_matches(|c| c == '\'' || c == '"').to_string();
        }
    }
    command
}

fn status_of(item: &Value) -> String {
    match item.get("status").and_then(Value::as_str) {
        Some("inProgress") | None => "in_progress",
        Some("completed") => "completed",
        Some(_) => "failed",
    }
    .to_string()
}

/// A Codex item that is a step, as a tool call.
fn tool_call(item: &Value) -> Option<ToolCall> {
    let id = text(item, "id")?;
    let status = status_of(item);
    Some(match item.get("type")?.as_str()? {
        "commandExecution" => {
            let command = command_of(item);
            ToolCall {
                id,
                title: command.clone(),
                kind: "execute".into(),
                status,
                locations: Vec::new(),
                content: text(item, "aggregatedOutput").filter(|o| !o.is_empty()).map(ToolContent::Text).into_iter().collect(),
                raw_input: Some(json!({"command": command})),
            }
        }
        "fileChange" => {
            let changes: Vec<&Value> = item.get("changes").and_then(Value::as_array).into_iter().flatten().collect();
            let paths: Vec<String> = changes.iter().filter_map(|c| text(c, "path")).collect();
            let deleting = !changes.is_empty()
                && changes.iter().all(|c| c.pointer("/kind/type").and_then(Value::as_str) == Some("delete"));
            ToolCall {
                id,
                title: paths.first().cloned().unwrap_or_else(|| "Edit files".into()),
                kind: if deleting { "delete" } else { "edit" }.into(),
                status,
                locations: paths,
                content: changes
                    .iter()
                    .filter_map(|c| Some(ToolContent::Patch { path: text(c, "path")?, unified: text(c, "diff").unwrap_or_default() }))
                    .collect(),
                raw_input: None,
            }
        }
        "mcpToolCall" => ToolCall {
            id,
            title: format!("{} · {}", text(item, "server").unwrap_or_default(), text(item, "tool").unwrap_or_default()),
            kind: "other".into(),
            status,
            ..ToolCall::default()
        },
        "dynamicToolCall" => {
            ToolCall { id, title: text(item, "tool").unwrap_or_default(), kind: "other".into(), status, ..ToolCall::default() }
        }
        "webSearch" => ToolCall {
            id,
            title: text(item, "query").unwrap_or_else(|| "Web search".into()),
            kind: "fetch".into(),
            // A search item is reported once it is done.
            status: "completed".into(),
            raw_input: Some(json!({"query": text(item, "query")})),
            ..ToolCall::default()
        },
        _ => return None,
    })
}

fn handler(events: mpsc::Sender<Event>, state: Arc<Mutex<State>>) -> impl FnMut(Incoming, &Responder) + Send + 'static {
    move |incoming, responder| {
        let send = |session_id: String, update: Update| {
            let _ = events.send(Event::Update { session_id, update });
        };
        match incoming {
            Incoming::Notification { method, params } => {
                let thread = text(&params, "threadId").unwrap_or_default();
                match method.as_str() {
                    "item/agentMessage/delta" => {
                        let item = text(&params, "itemId");
                        if let (Some(item), Ok(mut state)) = (&item, state.lock()) {
                            state.streamed.insert(item.clone());
                        }
                        send(thread, Update::AgentText { message_id: item, text: text(&params, "delta").unwrap_or_default() });
                    }
                    "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                        send(thread, Update::Thought { text: text(&params, "delta").unwrap_or_default() });
                    }
                    "item/started" => {
                        let item = params.get("item").cloned().unwrap_or(Value::Null);
                        if let Some(call) = tool_call(&item) {
                            send(thread, Update::ToolCall(call));
                        }
                    }
                    "item/completed" => {
                        let item = params.get("item").cloned().unwrap_or(Value::Null);
                        match item.get("type").and_then(Value::as_str) {
                            Some("agentMessage") => {
                                let id = text(&item, "id");
                                let streamed = id.as_ref().is_some_and(|id| state.lock().is_ok_and(|s| s.streamed.contains(id)));
                                if !streamed {
                                    send(thread, Update::AgentText { message_id: id, text: text(&item, "text").unwrap_or_default() });
                                }
                            }
                            Some("webSearch") => {
                                if let Some(call) = tool_call(&item) {
                                    send(thread, Update::ToolCall(call));
                                }
                            }
                            _ => {
                                if let Some(call) = tool_call(&item) {
                                    send(
                                        thread,
                                        Update::ToolPatch(ToolPatch {
                                            id: call.id,
                                            status: Some(call.status),
                                            title: Some(call.title),
                                            locations: (!call.locations.is_empty()).then_some(call.locations),
                                            content: (!call.content.is_empty()).then_some(call.content),
                                            ..ToolPatch::default()
                                        }),
                                    );
                                }
                            }
                        }
                    }
                    "turn/started" => {
                        if let (Some(turn), Ok(mut state)) = (params.pointer("/turn/id").and_then(Value::as_str), state.lock()) {
                            state.turns.insert(thread, turn.to_string());
                        }
                    }
                    "turn/completed" => {
                        let status = params.pointer("/turn/status").and_then(Value::as_str).unwrap_or("completed").to_string();
                        let error = params.pointer("/turn/error/message").and_then(Value::as_str).map(str::to_string);
                        if let Ok(mut state) = state.lock() {
                            state.turns.remove(&thread);
                            if let Some(waiter) = state.waiting.remove(&thread) {
                                let _ = waiter.send((status, error));
                            }
                        }
                    }
                    "thread/tokenUsage/updated" => {
                        let used = params.pointer("/tokenUsage/last/totalTokens").and_then(Value::as_u64).unwrap_or(0);
                        let size = params.pointer("/tokenUsage/modelContextWindow").and_then(Value::as_u64).unwrap_or(0);
                        send(thread, Update::Usage { used, size });
                    }
                    _ => {}
                }
            }
            Incoming::Request { id, method, params } => {
                let thread = text(&params, "threadId").unwrap_or_default();
                let item = text(&params, "itemId").unwrap_or_default();
                let reason = text(&params, "reason");
                let tool = match method.as_str() {
                    "item/commandExecution/requestApproval" => Some(ToolCall {
                        id: item,
                        title: text(&params, "command").or(reason).unwrap_or_else(|| "run a command".into()),
                        kind: "execute".into(),
                        status: "pending".into(),
                        ..ToolCall::default()
                    }),
                    "item/fileChange/requestApproval" => Some(ToolCall {
                        id: item,
                        title: reason.unwrap_or_else(|| "change files".into()),
                        kind: "edit".into(),
                        status: "pending".into(),
                        ..ToolCall::default()
                    }),
                    _ => None,
                };
                match tool {
                    Some(tool) => {
                        let option = |id: &str, name: &str, kind: &str| PermissionOption { id: id.into(), name: name.into(), kind: kind.into() };
                        let _ = events.send(Event::Permission(PermissionRequest {
                            request: id,
                            session_id: thread,
                            tool,
                            options: vec![
                                option("accept", "Allow", "allow_once"),
                                option("acceptForSession", "Always allow", "allow_always"),
                                option("decline", "Reject", "reject_once"),
                            ],
                        }));
                    }
                    None => {
                        let _ = responder.error(&id, -32601, &format!("{method} is not supported by Dragent"));
                    }
                }
            }
            Incoming::Closed => {
                if let Ok(mut state) = state.lock() {
                    for (_, waiter) in state.waiting.drain() {
                        let _ = waiter.send(("failed".into(), Some(RpcError::gone().message)));
                    }
                }
                let _ = events.send(Event::Exited);
            }
        }
    }
}

impl Codex {
    /// Start `program args…` (`codex app-server`) in `cwd`.
    pub fn spawn(
        program: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
    ) -> std::io::Result<Codex> {
        let (tx, rx) = mpsc::channel();
        let state: Arc<Mutex<State>> = Arc::default();
        let peer = Peer::spawn(program, args, cwd, env, false, handler(tx, state.clone()))?;
        Ok(Codex { peer, events: Mutex::new(Some(rx)), state })
    }

    /// Over any reader and writer (a test's).
    pub fn over(reader: impl BufRead + Send + 'static, writer: impl Write + Send + 'static) -> Codex {
        let (tx, rx) = mpsc::channel();
        let state: Arc<Mutex<State>> = Arc::default();
        let peer = Peer::over(reader, writer, false, handler(tx, state.clone()));
        Codex { peer, events: Mutex::new(Some(rx)), state }
    }

    /// The pickers for what is chosen now.
    fn options(&self) -> SessionOptions {
        let Ok(state) = self.state.lock() else { return SessionOptions::default() };
        let model = (!state.models.is_empty()).then(|| Picker {
            current: state.model.clone(),
            choices: state
                .models
                .iter()
                .map(|m| Choice { id: m.id.clone(), name: m.name.clone(), description: Some(m.description.clone()).filter(|d| !d.is_empty()) })
                .collect(),
            via: SetVia::Model,
        });
        let effort = state.models.iter().find(|m| m.id == state.model).filter(|m| !m.efforts.is_empty()).map(|m| Picker {
            current: state.effort.clone().or_else(|| m.default_effort.clone()).unwrap_or_default(),
            choices: m.efforts.iter().map(|e| Choice { id: e.clone(), name: e.clone(), description: None }).collect(),
            via: SetVia::ConfigOption(EFFORT.into()),
        });
        SessionOptions { model, mode: None, effort }
    }
}

impl ChatDriver for Codex {
    fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        self.events.lock().ok()?.take()
    }

    fn open(&self, cwd: &std::path::Path, resume: Option<&str>) -> Result<Session, RpcError> {
        let limit = Some(Duration::from_secs(60));
        self.peer.request(
            "initialize",
            json!({"clientInfo": {"name": "dragent", "title": "Dragent", "version": env!("CARGO_PKG_VERSION")}}),
            limit,
        )?;
        self.peer.notify("initialized", json!({}))?;
        let listed = self.peer.request("model/list", json!({}), limit).unwrap_or(Value::Null);
        let models: Vec<Model> = listed
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| !m.get("hidden").and_then(Value::as_bool).unwrap_or(false))
            .filter_map(|m| {
                Some(Model {
                    id: text(m, "id")?,
                    name: text(m, "displayName").or_else(|| text(m, "id"))?,
                    description: text(m, "description").unwrap_or_default(),
                    efforts: m
                        .get("supportedReasoningEfforts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| text(e, "reasoningEffort"))
                        .collect(),
                    default_effort: text(m, "defaultReasoningEffort"),
                })
            })
            .collect();
        let cwd_text = cwd.to_string_lossy();
        let opened = match resume {
            Some(thread) => self
                .peer
                .request("thread/resume", json!({"threadId": thread, "cwd": cwd_text, "excludeTurns": true}), Some(Duration::from_secs(120)))
                .or_else(|_| self.peer.request("thread/start", json!({"cwd": cwd_text}), Some(Duration::from_secs(120))))?,
            None => self.peer.request("thread/start", json!({"cwd": cwd_text}), Some(Duration::from_secs(120)))?,
        };
        let id = opened.pointer("/thread/id").and_then(Value::as_str).ok_or_else(|| RpcError::other("Codex didn't open a conversation."))?;
        if let Ok(mut state) = self.state.lock() {
            state.models = models;
            state.model = text(&opened, "model").unwrap_or_default();
            state.effort = text(&opened, "reasoningEffort");
        }
        Ok(Session { id: id.to_string(), options: self.options() })
    }

    fn replays_history(&self) -> bool {
        false
    }

    fn prompt(&self, session_id: &str, message: &str) -> Result<StopReason, RpcError> {
        let (tx, rx) = mpsc::channel();
        let (model, effort) = {
            let mut state = self.state.lock().map_err(|_| RpcError::gone())?;
            state.waiting.insert(session_id.to_string(), tx);
            (state.model.clone(), state.effort.clone())
        };
        let mut params = json!({
            "threadId": session_id,
            "input": [{"type": "text", "text": message, "text_elements": []}],
        });
        if !model.is_empty() {
            params["model"] = json!(model);
        }
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        let started = self.peer.request("turn/start", params, Some(Duration::from_secs(120)));
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                self.state.lock().ok().map(|mut s| s.waiting.remove(session_id));
                return Err(error);
            }
        };
        if let (Some(turn), Ok(mut state)) = (started.pointer("/turn/id").and_then(Value::as_str), self.state.lock()) {
            state.turns.entry(session_id.to_string()).or_insert_with(|| turn.to_string());
        }
        let (status, error) = rx.recv().map_err(|_| RpcError::gone())?;
        match status.as_str() {
            "interrupted" => Ok(StopReason::Cancelled),
            "failed" => Err(RpcError::other(error.unwrap_or_else(|| "The turn failed.".into()))),
            _ => Ok(StopReason::EndTurn),
        }
    }

    fn cancel(&self, session_id: &str) -> Result<(), RpcError> {
        let turn = self.state.lock().ok().and_then(|s| s.turns.get(session_id).cloned());
        match turn {
            Some(turn) => self.peer.request_detached("turn/interrupt", json!({"threadId": session_id, "turnId": turn})),
            None => Ok(()),
        }
    }

    fn set_option(&self, _session_id: &str, via: &SetVia, value: &str) -> Result<Option<SessionOptions>, RpcError> {
        {
            let mut state = self.state.lock().map_err(|_| RpcError::gone())?;
            match via {
                SetVia::Model => {
                    state.model = value.to_string();
                    // An effort the new model doesn't offer falls back to its default.
                    let supported = state.models.iter().find(|m| m.id == value).map(|m| m.efforts.clone()).unwrap_or_default();
                    if state.effort.as_ref().is_some_and(|e| !supported.contains(e)) {
                        state.effort = None;
                    }
                }
                SetVia::ConfigOption(id) if id == EFFORT => state.effort = Some(value.to_string()),
                _ => return Ok(None),
            }
        }
        Ok(Some(self.options()))
    }

    fn answer_permission(&self, request: &Value, option_id: Option<&str>) -> Result<(), RpcError> {
        // No answer means the turn is being stopped: decline and stop it.
        self.peer.responder().result(request, json!({"decision": option_id.unwrap_or("cancel")}))
    }

    fn shutdown(&self) {
        self.peer.shutdown();
    }

    fn error_output(&self) -> Option<String> {
        self.peer.error_output()
    }
}
