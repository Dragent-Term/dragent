//! JSON-RPC between Dragent and an agent process, one message per line.
//!
//! [`Peer`] sends requests and matches their answers, sends notifications,
//! and hands everything the agent starts (its requests and notifications) to
//! a handler running on the reader thread. ACP and Codex's app-server both
//! talk this way; ACP stamps `"jsonrpc": "2.0"` on every message, Codex
//! leaves it out.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::rpc::RpcError;

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, RpcError>>>>>;
type Output = Arc<Mutex<Box<dyn Write + Send>>>;

/// Something the agent started.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// It asks; answer with [`Responder::result`] or [`Responder::error`].
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
    /// Its output ended.
    Closed,
}

/// Writes answers to the agent's requests.
#[derive(Clone)]
pub(crate) struct Responder {
    out: Output,
    stamp: bool,
}

impl Responder {
    pub fn send(&self, mut message: Value) -> Result<(), RpcError> {
        if self.stamp {
            if let Some(object) = message.as_object_mut() {
                object.insert("jsonrpc".into(), "2.0".into());
            }
        }
        let mut line = serde_json::to_vec(&message).map_err(|e| RpcError { code: -3, message: e.to_string() })?;
        line.push(b'\n');
        let mut out = self.out.lock().map_err(|_| RpcError::gone())?;
        out.write_all(&line).and_then(|_| out.flush()).map_err(|_| RpcError::gone())
    }

    pub fn result(&self, id: &Value, result: Value) -> Result<(), RpcError> {
        self.send(json!({"id": id, "result": result}))
    }

    pub fn error(&self, id: &Value, code: i64, message: &str) -> Result<(), RpcError> {
        self.send(json!({"id": id, "error": {"code": code, "message": message}}))
    }
}

/// One agent process (or any reader and writer) spoken to in JSON-RPC.
pub(crate) struct Peer {
    errors: ErrorTail,
    responder: Responder,
    pending: Pending,
    next_id: AtomicU64,
    child: Mutex<Option<Child>>,
}

impl Peer {
    /// Start `program args…` in `cwd`; its error output is dropped.
    pub fn spawn(
        program: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
        stamp: bool,
        handler: impl FnMut(Incoming, &Responder) + Send + 'static,
    ) -> std::io::Result<Peer> {
        let errors = ErrorTail::default();
        let mut child = spawn_piped(program, args, cwd, env, &errors)?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut peer = Peer::over(BufReader::new(stdout), stdin, stamp, handler);
        peer.errors = errors;
        *peer.child.lock().unwrap() = Some(child);
        Ok(peer)
    }

    pub fn over(
        reader: impl BufRead + Send + 'static,
        writer: impl Write + Send + 'static,
        stamp: bool,
        mut handler: impl FnMut(Incoming, &Responder) + Send + 'static,
    ) -> Peer {
        let responder = Responder { out: Arc::new(Mutex::new(Box::new(writer))), stamp };
        let pending: Pending = Arc::default();
        {
            let responder = responder.clone();
            let pending = pending.clone();
            std::thread::Builder::new()
                .name("agent-rpc-reader".into())
                .spawn(move || {
                    for line in reader.lines() {
                        let Ok(line) = line else { break };
                        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
                        let method = message.get("method").and_then(Value::as_str).map(str::to_string);
                        let id = message.get("id").filter(|id| !id.is_null()).cloned();
                        let params = message.get("params").cloned().unwrap_or(Value::Null);
                        match (method, id) {
                            (None, Some(id)) => {
                                let Some(id) = id.as_u64() else { continue };
                                let Some(waiter) = pending.lock().ok().and_then(|mut p| p.remove(&id)) else { continue };
                                let answer = match message.get("error") {
                                    Some(error) => Err(RpcError {
                                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                                        message: error
                                            .get("message")
                                            .and_then(Value::as_str)
                                            .unwrap_or("The agent failed.")
                                            .to_string(),
                                    }),
                                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                                };
                                let _ = waiter.send(answer);
                            }
                            (Some(method), Some(id)) => handler(Incoming::Request { id, method, params }, &responder),
                            (Some(method), None) => handler(Incoming::Notification { method, params }, &responder),
                            (None, None) => {}
                        }
                    }
                    if let Ok(mut waiting) = pending.lock() {
                        for (_, waiter) in waiting.drain() {
                            let _ = waiter.send(Err(RpcError::gone()));
                        }
                    }
                    handler(Incoming::Closed, &responder);
                })
                .expect("spawn agent reader");
        }
        Peer { errors: ErrorTail::default(), responder, pending, next_id: AtomicU64::new(0), child: Mutex::new(None) }
    }

    /// Ask and wait for the answer (forever when `timeout` is `None`).
    pub fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().map_err(|_| RpcError::gone())?.insert(id, tx);
        if let Err(error) = self.responder.send(json!({"id": id, "method": method, "params": params})) {
            self.pending.lock().ok().map(|mut p| p.remove(&id));
            return Err(error);
        }
        let answer = match timeout {
            Some(limit) => rx.recv_timeout(limit).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => RpcError::timed_out(method),
                mpsc::RecvTimeoutError::Disconnected => RpcError::gone(),
            }),
            None => rx.recv().map_err(|_| RpcError::gone()),
        };
        if answer.is_err() {
            self.pending.lock().ok().map(|mut p| p.remove(&id));
        }
        answer?
    }

    /// Ask without waiting: the answer is dropped when it comes.
    pub fn request_detached(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.responder.send(json!({"id": id, "method": method, "params": params}))
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        self.responder.send(json!({"method": method, "params": params}))
    }

    /// What the agent last wrote to its error output.
    pub fn error_output(&self) -> Option<String> {
        error_text(&self.errors)
    }

    pub fn responder(&self) -> &Responder {
        &self.responder
    }

    pub fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().ok().and_then(|mut c| c.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The last lines an agent wrote to its error output: why it stopped, when
/// it stops on its own.
pub(crate) type ErrorTail = Arc<Mutex<std::collections::VecDeque<String>>>;

/// How many error lines are kept.
const ERROR_LINES: usize = 20;

/// The kept error lines worth showing (the last few that say something).
pub(crate) fn error_text(tail: &ErrorTail) -> Option<String> {
    let lines = tail.lock().ok()?;
    let useful: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let start = useful.len().saturating_sub(3);
    let text = useful[start..].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then(|| text.chars().take(400).collect())
}

/// `program args…` in `cwd` with stdin and stdout piped; its error output
/// is read into `tail` (the last [`ERROR_LINES`] lines).
pub(crate) fn spawn_piped(
    program: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
    env: &[(String, String)],
    tail: &ErrorTail,
) -> std::io::Result<Child> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn()?;
    if let Some(stderr) = child.stderr.take() {
        let tail = tail.clone();
        let _ = std::thread::Builder::new().name("agent-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut lines) = tail.lock() {
                    if lines.len() == ERROR_LINES {
                        lines.pop_front();
                    }
                    lines.push_back(line);
                }
            }
        });
    }
    Ok(child)
}
