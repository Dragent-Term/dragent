//! Claude Code as a chat, through its streaming JSON mode.
//!
//! `claude -p --input-format stream-json --output-format stream-json` reads
//! your messages as JSON lines and writes everything it does as JSON lines:
//! `stream_event` pieces of the reply while it is written, whole `assistant`
//! messages (with their tool calls), `user` messages carrying tool results,
//! and a `result` when the turn ends. Next to that runs a small control
//! channel, `control_request` / `control_response` matched by `request_id`,
//! both ways: Dragent asks it to `initialize` (which lists the models),
//! `interrupt` and `set_model`; it asks Dragent `can_use_tool` before a tool
//! runs (`--permission-prompt-tool stdio`).
//!
//! The process is started per conversation, so [`Claude::spawn`] only keeps
//! how to start it and [`ChatDriver::open`] starts it.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::options::{Choice, Picker, SessionOptions, SetVia};
use crate::rpc::{Event, PermissionOption, PermissionRequest, RpcError};
use crate::update::{ToolCall, ToolContent, ToolPatch, Update};
use crate::{ChatDriver, Session, StopReason};

type Output = Arc<Mutex<Box<dyn Write + Send>>>;
type Waiters = Arc<Mutex<HashMap<String, mpsc::Sender<Result<Value, RpcError>>>>>;

/// How to start the process.
struct Launch {
    program: std::path::PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// What the reader shares with the driver.
#[derive(Default)]
struct Shared {
    session: String,
    /// The reply message being streamed, and messages whose text streamed.
    message: Option<String>,
    streamed: HashSet<String>,
    /// Who waits for the running turn to end.
    turn: Option<mpsc::Sender<Result<StopReason, RpcError>>>,
}

/// Claude Code driven through its streaming JSON mode.
pub struct Claude {
    launch: Option<Launch>,
    pending_io: Mutex<Option<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)>>,
    out: Mutex<Option<Output>>,
    child: Mutex<Option<Child>>,
    waiters: Waiters,
    shared: Arc<Mutex<Shared>>,
    interrupted: Arc<AtomicBool>,
    events_tx: mpsc::Sender<Event>,
    events: Mutex<Option<mpsc::Receiver<Event>>>,
    next_request: AtomicU64,
    models: Mutex<Option<Picker>>,
    errors: crate::jsonrpc::ErrorTail,
    connection: Arc<AtomicU64>,
    opened: Arc<AtomicBool>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn write_line(out: &Output, message: &Value) -> Result<(), RpcError> {
    let mut line = serde_json::to_vec(message).map_err(|e| RpcError::other(e.to_string()))?;
    line.push(b'\n');
    let mut out = out.lock().map_err(|_| RpcError::gone())?;
    out.write_all(&line).and_then(|_| out.flush()).map_err(|_| RpcError::gone())
}

/// A tool call as Claude Code names and describes it.
fn tool_call(id: String, name: &str, input: &Value) -> ToolCall {
    let field = |key: &str| text(input, key);
    let path = field("file_path").or_else(|| field("notebook_path")).or_else(|| field("path"));
    let (kind, title) = match name {
        "Bash" => ("execute", field("command").unwrap_or_default()),
        "Read" => ("read", path.clone().unwrap_or_default()),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ("edit", path.clone().unwrap_or_default()),
        "Grep" | "Glob" => ("search", field("pattern").unwrap_or_default()),
        "WebFetch" => ("fetch", field("url").unwrap_or_default()),
        "WebSearch" => ("fetch", field("query").unwrap_or_default()),
        "Task" | "Agent" => ("other", field("description").unwrap_or_else(|| name.to_string())),
        _ => ("other", name.to_string()),
    };
    let mut content = Vec::new();
    if let Some(path) = &path {
        match name {
            "Edit" => content.push(ToolContent::Diff {
                path: path.clone(),
                old: field("old_string"),
                new: field("new_string").unwrap_or_default(),
            }),
            "Write" => content.push(ToolContent::Diff { path: path.clone(), old: None, new: field("content").unwrap_or_default() }),
            "MultiEdit" => {
                for edit in input.get("edits").and_then(Value::as_array).into_iter().flatten() {
                    content.push(ToolContent::Diff {
                        path: path.clone(),
                        old: text(edit, "old_string"),
                        new: text(edit, "new_string").unwrap_or_default(),
                    });
                }
            }
            _ => {}
        }
    }
    ToolCall {
        id,
        title: if title.is_empty() { name.to_string() } else { title },
        kind: kind.into(),
        status: "in_progress".into(),
        locations: path.into_iter().collect(),
        content,
        raw_input: Some(input.clone()),
    }
}

/// A tool result's text (a string, or text blocks).
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| text(b, "text")).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

struct Reader {
    /// This connection, and the newest one: a replaced connection's ending
    /// isn't reported.
    connection: u64,
    newest: Arc<AtomicU64>,
    /// A session is open: until then a start that fails is `open`'s error,
    /// not the agent stopping.
    opened: Arc<AtomicBool>,
    out: Output,
    waiters: Waiters,
    shared: Arc<Mutex<Shared>>,
    interrupted: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
}

impl Reader {
    fn update(&self, update: Update) {
        let session = self.shared.lock().map(|s| s.session.clone()).unwrap_or_default();
        let _ = self.events.send(Event::Update { session_id: session, update });
    }

    fn end_turn(&self, end: Result<StopReason, RpcError>) {
        if let Some(waiter) = self.shared.lock().ok().and_then(|mut s| s.turn.take()) {
            let _ = waiter.send(end);
        }
    }

    fn handle(&self, message: Value) {
        match message.get("type").and_then(Value::as_str).unwrap_or_default() {
            "control_response" => {
                let response = message.get("response").cloned().unwrap_or(Value::Null);
                let Some(id) = text(&response, "request_id") else { return };
                let Some(waiter) = self.waiters.lock().ok().and_then(|mut w| w.remove(&id)) else { return };
                let answer = if response.get("subtype").and_then(Value::as_str) == Some("error") {
                    Err(RpcError::other(text(&response, "error").unwrap_or_else(|| "Claude Code refused.".into())))
                } else {
                    Ok(response.get("response").cloned().unwrap_or(Value::Null))
                };
                let _ = waiter.send(answer);
            }
            "control_request" => {
                let id = text(&message, "request_id").unwrap_or_default();
                let request = message.get("request").cloned().unwrap_or(Value::Null);
                if request.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                    let name = text(&request, "tool_name").unwrap_or_default();
                    let input = request.get("input").cloned().unwrap_or(Value::Null);
                    let mut tool = tool_call(text(&request, "tool_use_id").unwrap_or_default(), &name, &input);
                    tool.status = "pending".into();
                    let option = |id: &str, name: &str, kind: &str| PermissionOption { id: id.into(), name: name.into(), kind: kind.into() };
                    let session = self.shared.lock().map(|s| s.session.clone()).unwrap_or_default();
                    let _ = self.events.send(Event::Permission(PermissionRequest {
                        request: json!({"request_id": id, "input": input}),
                        session_id: session,
                        tool,
                        options: vec![option("allow", "Allow", "allow_once"), option("deny", "Reject", "reject_once")],
                    }));
                } else {
                    let _ = write_line(
                        &self.out,
                        &json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "Not supported by Dragent"}}),
                    );
                }
            }
            "stream_event" => {
                let event = message.get("event").cloned().unwrap_or(Value::Null);
                match event.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "message_start" => {
                        if let Ok(mut shared) = self.shared.lock() {
                            shared.message = event.pointer("/message/id").and_then(Value::as_str).map(str::to_string);
                        }
                    }
                    "content_block_delta" => {
                        let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                        match delta.get("type").and_then(Value::as_str) {
                            Some("text_delta") => {
                                let id = self.shared.lock().ok().and_then(|mut s| {
                                    let id = s.message.clone();
                                    if let Some(id) = &id {
                                        s.streamed.insert(id.clone());
                                    }
                                    id
                                });
                                self.update(Update::AgentText { message_id: id, text: text(&delta, "text").unwrap_or_default() });
                            }
                            Some("thinking_delta") => self.update(Update::Thought { text: text(&delta, "thinking").unwrap_or_default() }),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                let id = message.pointer("/message/id").and_then(Value::as_str).map(str::to_string);
                let streamed = id.as_ref().is_some_and(|id| self.shared.lock().is_ok_and(|s| s.streamed.contains(id)));
                for block in message.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") if !streamed => {
                            self.update(Update::AgentText { message_id: id.clone(), text: text(block, "text").unwrap_or_default() })
                        }
                        Some("tool_use") => {
                            let name = text(block, "name").unwrap_or_default();
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            self.update(Update::ToolCall(tool_call(text(block, "id").unwrap_or_default(), &name, &input)));
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in message.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let output = result_text(block.get("content").unwrap_or(&Value::Null));
                    self.update(Update::ToolPatch(ToolPatch {
                        id: text(block, "tool_use_id").unwrap_or_default(),
                        status: Some(if failed { "failed" } else { "completed" }.into()),
                        content: (!output.is_empty() && failed).then(|| vec![ToolContent::Text(output)]),
                        ..ToolPatch::default()
                    }));
                }
            }
            "result" => {
                let interrupted = self.interrupted.swap(false, Ordering::SeqCst);
                let end = if interrupted {
                    Ok(StopReason::Cancelled)
                } else if message.get("subtype").and_then(Value::as_str) == Some("success")
                    && !message.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                {
                    Ok(StopReason::EndTurn)
                } else {
                    let why = text(&message, "result")
                        .or_else(|| message.get("errors").and_then(Value::as_array).and_then(|e| e.first()).and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| "Claude Code stopped with an error.".into());
                    Err(RpcError::other(why))
                };
                if let Ok(mut shared) = self.shared.lock() {
                    shared.message = None;
                }
                self.end_turn(end);
            }
            _ => {}
        }
    }

    fn run(self, reader: Box<dyn BufRead + Send>) {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                self.handle(message);
            }
        }
        if let Ok(mut waiters) = self.waiters.lock() {
            for (_, waiter) in waiters.drain() {
                let _ = waiter.send(Err(RpcError::gone()));
            }
        }
        if self.newest.load(Ordering::SeqCst) == self.connection {
            self.end_turn(Err(RpcError::gone()));
            if self.opened.load(Ordering::SeqCst) {
                let _ = self.events.send(Event::Exited);
            }
        }
    }
}

impl Claude {
    /// Keep how to start `program args…`; [`ChatDriver::open`] starts it.
    pub fn spawn(program: &std::path::Path, args: &[String], env: &[(String, String)]) -> Claude {
        Claude::new(Some(Launch { program: program.to_path_buf(), args: args.to_vec(), env: env.to_vec() }), None)
    }

    /// Over any reader and writer (a test's).
    pub fn over(reader: impl BufRead + Send + 'static, writer: impl Write + Send + 'static) -> Claude {
        Claude::new(None, Some((Box::new(reader), Box::new(writer))))
    }

    fn new(launch: Option<Launch>, io: Option<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)>) -> Claude {
        let (tx, rx) = mpsc::channel();
        Claude {
            launch,
            pending_io: Mutex::new(io),
            out: Mutex::new(None),
            child: Mutex::new(None),
            waiters: Arc::default(),
            shared: Arc::default(),
            interrupted: Arc::default(),
            events_tx: tx,
            events: Mutex::new(Some(rx)),
            next_request: AtomicU64::new(0),
            models: Mutex::new(None),
            errors: Default::default(),
            connection: Arc::default(),
            opened: Arc::default(),
        }
    }

    /// One attempt at opening: start the process and ask what it offers.
    fn open_once(&self, cwd: &std::path::Path, resume: Option<&str>) -> Result<Session, RpcError> {
        let session = resume.map(str::to_string).unwrap_or_else(new_session_id);
        if let Ok(mut shared) = self.shared.lock() {
            shared.session = session.clone();
        }
        self.connect(cwd, resume, &session)?;
        let hello = self.control(json!({"subtype": "initialize"}), Duration::from_secs(60))?;
        let choices: Vec<Choice> = hello
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some(Choice {
                    id: text(m, "value")?,
                    name: text(m, "displayName").or_else(|| text(m, "value"))?,
                    description: text(m, "description"),
                })
            })
            .collect();
        let model = (!choices.is_empty()).then(|| Picker {
            current: choices.iter().find(|c| c.id == "default").unwrap_or(&choices[0]).id.clone(),
            choices,
            via: SetVia::Model,
        });
        *self.models.lock().map_err(|_| RpcError::gone())? = model.clone();
        Ok(Session { id: session, options: SessionOptions { model, mode: None, effort: None } })
    }

    /// Send a message and wait for the turn to end.
    fn prompt_once(&self, message: &str) -> Result<StopReason, RpcError> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut shared) = self.shared.lock() {
            shared.turn = Some(tx);
        }
        self.interrupted.store(false, Ordering::SeqCst);
        write_line(
            &self.output()?,
            &json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": message}]}}),
        )?;
        rx.recv().map_err(|_| RpcError::gone())?
    }

    fn output(&self) -> Result<Output, RpcError> {
        self.out.lock().ok().and_then(|o| o.clone()).ok_or_else(RpcError::gone)
    }

    /// Ask on the control channel and wait for the answer.
    fn control(&self, request: Value, timeout: Duration) -> Result<Value, RpcError> {
        let id = format!("dragent-{}", self.next_request.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        self.waiters.lock().map_err(|_| RpcError::gone())?.insert(id.clone(), tx);
        write_line(&self.output()?, &json!({"type": "control_request", "request_id": id, "request": request}))?;
        rx.recv_timeout(timeout).map_err(|_| RpcError::timed_out("control request"))?
    }

    /// Start the process (or take the test's pipes) and its reader.
    fn connect(&self, cwd: &std::path::Path, resume: Option<&str>, session: &str) -> Result<(), RpcError> {
        let (reader, writer): (Box<dyn BufRead + Send>, Box<dyn Write + Send>) =
            match self.pending_io.lock().ok().and_then(|mut io| io.take()) {
                Some(io) => io,
                None => {
                    let launch = self.launch.as_ref().ok_or_else(RpcError::gone)?;
                    let mut args = launch.args.clone();
                    args.extend(
                        [
                            "-p",
                            "--input-format",
                            "stream-json",
                            "--output-format",
                            "stream-json",
                            "--verbose",
                            "--include-partial-messages",
                            "--permission-prompt-tool",
                            "stdio",
                        ]
                        .map(str::to_string),
                    );
                    match resume {
                        Some(id) => args.extend(["--resume".to_string(), id.to_string()]),
                        None => args.extend(["--session-id".to_string(), session.to_string()]),
                    }
                    let mut child = crate::jsonrpc::spawn_piped(&launch.program, &args, cwd, &launch.env, &self.errors)
                        .map_err(|e| RpcError::other(format!("Couldn't start Claude Code: {e}")))?;
                    let stdin = child.stdin.take().ok_or_else(RpcError::gone)?;
                    let stdout = child.stdout.take().ok_or_else(RpcError::gone)?;
                    *self.child.lock().map_err(|_| RpcError::gone())? = Some(child);
                    (Box::new(BufReader::new(stdout)), Box::new(stdin))
                }
            };
        let out: Output = Arc::new(Mutex::new(writer));
        *self.out.lock().map_err(|_| RpcError::gone())? = Some(out.clone());
        let reader_state = Reader {
            connection: self.connection.fetch_add(1, Ordering::SeqCst) + 1,
            newest: self.connection.clone(),
            opened: self.opened.clone(),
            out,
            waiters: self.waiters.clone(),
            shared: self.shared.clone(),
            interrupted: self.interrupted.clone(),
            events: self.events_tx.clone(),
        };
        std::thread::Builder::new()
            .name("claude-stream-reader".into())
            .spawn(move || reader_state.run(reader))
            .map_err(|e| RpcError::other(e.to_string()))?;
        Ok(())
    }
}

/// A fresh conversation id, in the form Claude Code takes (a UUID).
fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl ChatDriver for Claude {
    fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        self.events.lock().ok()?.take()
    }

    fn open(&self, cwd: &std::path::Path, resume: Option<&str>) -> Result<Session, RpcError> {
        let opened = match self.open_once(cwd, resume) {
            // The conversation couldn't be reopened (moved, from another
            // version, still open elsewhere): start a new one instead.
            Err(error) if resume.is_some() && self.launch.is_some() => {
                log::warn!("[chat] Claude Code couldn't reopen the conversation ({error}); starting a new one");
                // Make the failed attempt stale before stopping it: its
                // ending must not read as the new agent stopping (which
                // restarted and killed the healthy one).
                self.connection.fetch_add(1, Ordering::SeqCst);
                self.shutdown();
                self.open_once(cwd, None).map_err(|_| error)
            }
            opened => opened,
        };
        if opened.is_ok() {
            self.opened.store(true, Ordering::SeqCst);
        }
        opened
    }

    fn replays_history(&self) -> bool {
        false
    }

    fn prompt(&self, _session_id: &str, message: &str) -> Result<StopReason, RpcError> {
        self.prompt_once(message)
    }

    fn cancel(&self, _session_id: &str) -> Result<(), RpcError> {
        self.interrupted.store(true, Ordering::SeqCst);
        let id = format!("dragent-{}", self.next_request.fetch_add(1, Ordering::Relaxed));
        write_line(&self.output()?, &json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}}))
    }

    fn set_option(&self, _session_id: &str, via: &SetVia, value: &str) -> Result<Option<SessionOptions>, RpcError> {
        if *via != SetVia::Model {
            return Ok(None);
        }
        self.control(json!({"subtype": "set_model", "model": value}), Duration::from_secs(30))?;
        let mut models = self.models.lock().map_err(|_| RpcError::gone())?;
        if let Some(picker) = models.as_mut() {
            picker.current = value.to_string();
        }
        Ok(Some(SessionOptions { model: models.clone(), mode: None, effort: None }))
    }

    fn answer_permission(&self, request: &Value, option_id: Option<&str>) -> Result<(), RpcError> {
        let id = text(request, "request_id").unwrap_or_default();
        let answer = match option_id {
            Some("allow") => json!({"behavior": "allow", "updatedInput": request.get("input").cloned().unwrap_or(json!({}))}),
            Some(_) => json!({"behavior": "deny", "message": "The person declined this."}),
            None => json!({"behavior": "deny", "message": "The person stopped the turn.", "interrupt": true}),
        };
        write_line(&self.output()?, &json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": answer}}))
    }

    fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn error_output(&self) -> Option<String> {
        crate::jsonrpc::error_text(&self.errors)
    }
}

impl Drop for Claude {
    fn drop(&mut self) {
        self.shutdown();
    }
}
