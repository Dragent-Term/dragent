use std::io::{BufRead, BufReader};
use std::sync::mpsc;

#[cfg(not(target_family = "wasm"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_family = "wasm"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;

use super::events::{AgentEventKind, CreateTaskResponse, PipelineEvent};
use crate::agent::pipeline::start_recovered_local_tool_stream;
use crate::agent::pipeline::start_task_stream;
use crate::agent::pipeline::stream_recovery::checkpoint_tool_calls;
use crate::agent::pipeline::stream_types::local_runs;
use crate::agent::pipeline::stream_types::LocalToolResult;
use crate::agent::pipeline::ResumeTaskError;
use crate::agent::pipeline::StreamError;
use crate::api::API_CLIENT;

#[cfg(target_family = "wasm")]
pub fn start_task_stream(
    _model_name: &str,
    _system: &str,
    _query: &str,
    _prior_messages: &[crate::agent::controller::SessionMessage],
    _agent_mode: &str,
    _turn_tools: Vec<serde_json::Value>,
    _tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> Result<(), StreamError> {
    Err(StreamError::Network(
        "agent streaming unavailable in the web viewer".to_string(),
    ))
}

#[cfg(target_family = "wasm")]
pub fn start_scoped_task_stream(
    _model_name: &str,
    _system: &str,
    _query: &str,
    _prior_messages: &[crate::agent::controller::SessionMessage],
    _agent_mode: &str,
    _allowed_tools: &[String],
    _tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> Result<(), StreamError> {
    Err(StreamError::Network(
        "agent streaming unavailable in the web viewer".to_string(),
    ))
}

/// Request LLM-based compaction of conversation history from the server.
/// The server uses the configured LLM provider to summarize middle messages,
/// replacing them with a dense summary while preserving the first user message
/// and the most recent turns. Falls back gracefully on any error.
pub fn compact_history(
    model_name: &str,
    system: &str,
    messages: &[crate::agent::controller::SessionMessage],
) -> Result<Vec<crate::agent::controller::SessionMessage>, StreamError> {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();

    let body = serde_json::json!({
        "model_name": model_name,
        "system":     system,
        "messages":   msgs,
    });

    let resp = API_CLIENT
        .post("/api/v1/agent/compact", &body, 60)
        .map_err(|e| StreamError::Network(e.to_string()))?;

    #[derive(serde::Deserialize)]
    struct CompactResp {
        messages: Vec<CompactMsg>,
    }
    #[derive(serde::Deserialize)]
    struct CompactMsg {
        role: String,
        content: String,
    }

    let compact: CompactResp =
        serde_json::from_value(resp).map_err(|e| StreamError::Parse(e.to_string()))?;

    Ok(compact
        .messages
        .into_iter()
        .map(|m| crate::agent::controller::SessionMessage {
            role: m.role,
            content: m.content,
            attachments: Vec::new(),
        })
        .collect())
}

/// POST a tool execution result back to the server so the LLM loop can continue.
/// call_id must match the tool_id from the KindToolCall SSE event.
pub fn post_tool_result(
    task_id: &str,
    call_id: &str,
    output: &str,
    is_error: bool,
) -> Result<(), StreamError> {
    #[cfg(not(target_family = "wasm"))]
    if task_id.starts_with("local-") {
        let sender = local_runs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(task_id)
            .map(|control| control.results.clone())
            .ok_or_else(|| {
                StreamError::Network("the local agent run is no longer active".to_string())
            })?;
        sender
            .send(LocalToolResult {
                call_id: call_id.to_string(),
                output: output.to_string(),
                is_error,
            })
            .map_err(|_| StreamError::Network("local tool-result channel closed".to_string()))?;
        log::info!(
            "[local-agent] tool result delivered locally (call_id={call_id}, is_error={is_error})"
        );
        return Ok(());
    }

    let path = format!("/api/v1/agent/tasks/{task_id}/tool_results");
    let result = if is_error {
        crate::agent::api::types::ToolResultEnvelope::error(
            call_id,
            output,
            output.contains("cancelled"),
        )
    } else {
        crate::agent::api::types::ToolResultEnvelope::success(call_id, output)
    };
    let body = serde_json::json!({
        "call_id": result.call_id,
        "output": result.output,
        "is_error": result.is_error,
    });
    let t = web_time::Instant::now();
    API_CLIENT
        .post(&path, &body, 30)
        .map_err(|e| StreamError::Network(e.to_string()))?;
    log::info!(
        "[agent-stream] tool result posted in {}ms (call_id={call_id}, is_error={is_error})",
        t.elapsed().as_millis()
    );
    Ok(())
}

/// Deliver a tool result only to the exact local execution/round that created
/// it. For remote runs the backend owns the corresponding durable identity.
/// Repeating an already incorporated result is a successful no-op.
pub fn post_tool_result_for_execution(
    task_id: &str,
    execution_id: &str,
    round: u32,
    call_id: &str,
    output: &str,
    is_error: bool,
) -> Result<(), StreamError> {
    #[cfg(not(target_family = "wasm"))]
    if task_id.starts_with("local-") {
        let checkpoint =
            super::local_run::LocalRunCheckpoint::load(task_id).map_err(StreamError::Network)?;
        if checkpoint.execution_id != execution_id {
            return Err(StreamError::Network(
                "acknowledgement belongs to an expired local execution".into(),
            ));
        }
        let already_incorporated =
            checkpoint.request["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages.iter().any(|message| {
                        message["role"].as_str() == Some("tool")
                            && message["tool_call_id"].as_str() == Some(call_id)
                    })
                });
        if already_incorporated {
            return Ok(());
        }
        if checkpoint.round != round || checkpoint.phase != "awaiting_tools" {
            return Err(StreamError::Network(
                "acknowledgement no longer matches the pending provider round".into(),
            ));
        }
        let calls = checkpoint_tool_calls(&checkpoint).map_err(StreamError::Network)?;
        if !calls.iter().any(|call| call.call_id == call_id) {
            return Err(StreamError::Network(
                "acknowledgement does not match a pending provider tool call".into(),
            ));
        }
        let saved = checkpoint
            .pending_tool_results
            .get(call_id)
            .ok_or_else(|| {
                StreamError::Network("tool result must be checkpointed before delivery".into())
            })?;
        if saved.output != output || saved.is_error != is_error {
            return Err(StreamError::Network(
                "checkpointed tool result differs from the delivery attempt".into(),
            ));
        }
        let mut runs = local_runs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let control = runs
            .get_mut(task_id)
            .filter(|control| control.execution_id == execution_id)
            .ok_or_else(|| {
                StreamError::Network("the matching local execution is not active".into())
            })?;
        if !control.delivered_tool_results.insert(call_id.to_string()) {
            return Ok(());
        }
        if control
            .results
            .send(LocalToolResult {
                call_id: call_id.to_string(),
                output: output.to_string(),
                is_error,
            })
            .is_err()
        {
            control.delivered_tool_results.remove(call_id);
            return Err(StreamError::Network(
                "local tool-result channel closed".into(),
            ));
        }
        return Ok(());
    }

    post_tool_result(task_id, call_id, output, is_error)
}

/// Deliver to a live desktop run, or—after process restart—commit the result
/// into its private `awaiting_tools` checkpoint and resume from the next safe
/// model boundary once all parked calls are resolved.
pub fn deliver_tool_result_with_recovery(
    task_id: &str,
    call_id: &str,
    output: &str,
    is_error: bool,
    tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> Result<(), StreamError> {
    match post_tool_result(task_id, call_id, output, is_error) {
        Ok(()) => Ok(()),
        Err(live_error) => {
            #[cfg(not(target_family = "wasm"))]
            if task_id.starts_with("local-") {
                return start_recovered_local_tool_stream(task_id, call_id, output, is_error, tx)
                    .map(|_| ())
                    .map_err(|error| match error {
                        ResumeTaskError::Unavailable => StreamError::Network(
                            "local recovery is unavailable for this run".into(),
                        ),
                        ResumeTaskError::Rejected(message) => StreamError::Network(message),
                        ResumeTaskError::Stream(error) => error,
                    });
            }
            Err(live_error)
        }
    }
}

/// Signal a desktop-owned run to stop. The provider connection and any local
/// tool wait observe the same flag; the checkpoint is removed by the run loop.
#[cfg(not(target_family = "wasm"))]
pub fn cancel_local_task(task_id: &str) -> bool {
    let runs = local_runs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(control) = runs.get(task_id) else {
        return false;
    };
    control.cancelled.store(true, Ordering::Relaxed);
    let run_id = task_id.to_string();
    let execution_id = control.execution_id.clone();
    crate::bg::detach(move || {
        let writer = dragent_persistence::shared_writer();
        if let Err(error) = writer.park_workspace_tool_outcomes_for_run(
            run_id,
            execution_id,
            "local provider run was cancelled before acknowledgement delivery".into(),
        ) {
            log::warn!("[local-agent] park cancelled workspace outcomes: {error:#}");
        }
    });
    true
}

#[cfg(target_family = "wasm")]
pub fn cancel_local_task(_task_id: &str) -> bool {
    false
}

use dragent_agent::controller::subagent::{
    role_allows_tool, role_deny_message, system_prompt_for_role, WorkerRole,
};

/// Maximum time a Worker sub-task may go with NO traffic at all — no events
/// and no server keepalives (which arrive every 20s from a healthy backend) —
/// before the calling thread declares the connection dead.
///
/// Deliberately an IDLE gate, not a wall-clock cap: a worker that is actively
/// streaming (or whose server is alive and pinging) must never be killed, no
/// matter how long the task runs. The old fixed 10-minute wall killed
/// productive long tasks (builds, test suites) mid-flight and mislabeled the
/// result a "network error". Time spent executing a tool locally does not
/// count either — the clock only runs while waiting on the channel. Matches
/// the parent controller's 120s INACTIVITY_TIMEOUT.
const WORKER_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Run a sub-task synchronously (blocking the calling thread until done).
/// Creates a new pipeline task, streams it, and auto-executes every tool call
/// locally — the same loop the parent agent runs, but collapsed into one call.
///
/// `role` selects the worker shape: `Worker` has full tool access, `Explore`
/// is read-only (mutating calls are refused with a guidance error, never
/// executed).
///
/// Returns the accumulated assistant text from the sub-task.
/// Safe to call from any background thread (must NOT be called from the UI thread).
pub fn run_subtask_sync(
    model_name: &str,
    query: &str,
    pane_id: uuid::Uuid,
    role: WorkerRole,
) -> Result<String, StreamError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let model_name = model_name.to_string();
    let query = query.to_string();
    let system = system_prompt_for_role(role).to_string();
    let mode = role.as_str().to_string();
    crate::bg::detach(move || {
        if let Err(e) = start_stream_with_retry("worker-task-stream", || {
            start_task_stream(&model_name, &system, &query, &[], &mode, Vec::new(), tx.clone())
        }) {
            let _ = tx.send(Err(e));
        }
    });

    let mut text = String::new();
    let mut task_id = String::new();

    loop {
        // Idle gate, reset by every received event — including the keepalive
        // sentinel read_stream forwards for each 20s server ping. While the
        // agent works (or the server is alive), this never fires.
        match rx.recv_timeout(WORKER_IDLE_TIMEOUT) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(StreamError::Timeout(format!(
                    "worker stream went silent for {}s (no events or keepalives) — backend connection lost",
                    WORKER_IDLE_TIMEOUT.as_secs()
                )));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Err(e)) => return Err(e),
            Ok(Ok(ev)) => {
                // Capture and skip the task-id sentinel emitted as soon as the
                // unified POST response headers arrive.
                if matches!(ev.kind, AgentEventKind::TextDelta) {
                    if let Some(id) = ev
                        .error
                        .as_deref()
                        .and_then(|e| e.strip_prefix("__task_id__"))
                    {
                        task_id = id.to_string();
                        continue;
                    }
                }
                match ev.kind {
                    AgentEventKind::TextDelta => {
                        if let Some(t) = ev.text {
                            text.push_str(&t);
                        }
                    }
                    AgentEventKind::ToolCall => {
                        if task_id.is_empty() {
                            return Err(StreamError::Parse(
                                "worker received a tool call before its task id".to_string(),
                            ));
                        }
                        let call_id = ev.tool_id.unwrap_or_default();
                        let tool = ev.tool_name.unwrap_or_default();
                        let args = ev.tool_args.unwrap_or_default();
                        // Role gate (item 1): explore workers are read-only.
                        // Refused calls never execute — the guidance error
                        // tells the worker to report back instead.
                        if !role_allows_tool(role, &tool) {
                            let refusal = role_deny_message(role, &tool);
                            if let Err(e) = post_tool_result(&task_id, &call_id, &refusal, true) {
                                log::warn!("[subagent] post_tool_result {call_id}: {e}");
                            }
                            continue;
                        }
                        let (out, err_flag) =
                            crate::agent::local_tools::execute(&tool, &args, pane_id);
                        if let Err(e) = post_tool_result(&task_id, &call_id, &out, err_flag) {
                            log::warn!("[subagent] post_tool_result {call_id}: {e}");
                        }
                    }
                    AgentEventKind::Done => break,
                    AgentEventKind::Error => {
                        return Err(StreamError::Network(
                            ev.error.unwrap_or_else(|| "subtask error".to_string()),
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(text)
}

/// Open a provider stream with semantic retry (item 3): transient
/// establishment failures (HTTP 429 quota, 5xx/network, timeouts) are
/// retried with exponential backoff honoring the server's wait hint before
/// surfacing. Auth, credits, parse, and definitive failures return
/// immediately — retrying those burns quota or loops forever.
///
/// Runs on a background thread; the bounded sleeps never block the UI.
/// Mid-stream failures are NOT retried here (the turn already exists — those
/// flow through the error card / recovery paths instead).
pub fn start_stream_with_retry<F>(
    label: &str,
    #[allow(unused_mut)] mut open: F,
) -> Result<(), StreamError>
where
    F: FnMut() -> Result<(), StreamError>,
{
    // No threads on wasm: single attempt, same as before.
    #[cfg(target_family = "wasm")]
    {
        let _ = label;
        return open();
    }
    #[cfg(not(target_family = "wasm"))]
    {
        use dragent_agent::controller::retry::{delay_for_attempt, is_retryable, RetryPolicy};
        let policy = RetryPolicy::default();
        let mut attempt: u32 = 0;
        loop {
            match open() {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if !is_retryable(&e) || attempt + 1 >= policy.max_attempts {
                        return Err(e);
                    }
                    let delay = delay_for_attempt(&policy, attempt, &e);
                    log::warn!(
                        "[agent] {label} open attempt {} failed ({e}); retrying in {}ms",
                        attempt + 1,
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                    attempt += 1;
                }
            }
        }
    }
}

/// Liveness marker forwarded for each server keepalive ping. Carried as a
/// `TextDelta` with the `__ping__` error sentinel (same idiom as `__task_id__`)
/// so no new event kind is needed; poll_stream recognizes and drops it.
pub(crate) fn keepalive_event() -> PipelineEvent {
    PipelineEvent {
        kind: AgentEventKind::TextDelta,
        text: None,
        tool_id: None,
        tool_name: None,
        tool_icon: None,
        args_summary: None,
        tool_args: None,
        requires_approval: None,
        server_executed: None,
        result_summary: None,
        duration_ms: None,
        tool_output: None,
        tool_is_error: None,
        assistant_content: None,
        compacted_messages: None,
        total_input_tokens: None,
        total_cached_input_tokens: None,
        total_output_tokens: None,
        rounds: None,
        context_tokens: None,
        error: Some("__ping__".to_string()),
    }
}

pub(crate) fn send_task_id_event(
    tx: &mpsc::Sender<Result<PipelineEvent, StreamError>>,
    task_id: &str,
) -> Result<(), StreamError> {
    tx.send(Ok(PipelineEvent {
        kind: AgentEventKind::TextDelta,
        text: None,
        tool_id: None,
        tool_name: None,
        tool_icon: None,
        args_summary: None,
        tool_args: None,
        requires_approval: None,
        server_executed: None,
        result_summary: None,
        duration_ms: None,
        tool_output: None,
        tool_is_error: None,
        assistant_content: None,
        compacted_messages: None,
        total_input_tokens: None,
        total_cached_input_tokens: None,
        total_output_tokens: None,
        rounds: None,
        context_tokens: None,
        error: Some(format!("__task_id__{task_id}")),
    }))
    .map_err(|_| StreamError::Network("agent stream receiver closed".to_string()))
}

/// Open the SSE stream for a task and send parsed events on `tx`.
/// This function blocks the calling thread until the stream ends or `tx` is dropped.
/// Run this on a dedicated background thread.
/// Outcome of one SSE connection attempt, so the reconnect loop knows whether
/// to retry.
enum ConnOutcome {
    /// The turn finished ([DONE] / a terminal event) — stop, success.
    Done,
    /// The socket dropped mid-turn. App Runner caps a request at 120s, so this
    /// is expected on any long turn and is NOT a real failure — reconnect from
    /// `last_seq` and the server replays what was missed.
    Dropped,
    /// A definitive failure the client must surface (auth, task gone, a resume
    /// the server refused). No point retrying.
    Fatal(StreamError),
}

/// Read the task's SSE stream, transparently reconnecting across the App Runner
/// 120s request cap (and any other mid-turn transport drop).
///
/// The server assigns each event a sequence number (SSE `id:`) and buffers
/// recent events, so a reconnect carrying `Last-Event-ID` resumes exactly where
/// the drop happened — no lost tool calls, no duplicated output, no visible
/// error. Only a genuinely unrecoverable condition reaches the caller as an
/// error.
pub fn read_stream(task_id: String, tx: mpsc::Sender<Result<PipelineEvent, StreamError>>) {
    read_stream_from(task_id, 0, tx);
}

fn read_stream_from(
    task_id: String,
    mut last_seq: u64,
    tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) {
    let base_url = crate::api::backend_url();
    let url = format!("{}/api/v1/agent/tasks/{}/stream", base_url, task_id);

    let mut stalled: u32 = 0;
    // Cap CONSECUTIVE reconnects that make no progress (last_seq unchanged), so
    // a genuinely dead task can't loop forever. Any progress resets it, so a
    // long turn crossing many 120s boundaries reconnects indefinitely.
    const MAX_STALLED_RECONNECTS: u32 = 5;

    loop {
        let before = last_seq;
        match read_stream_once(&url, &mut last_seq, &tx) {
            ConnOutcome::Done => return,
            ConnOutcome::Fatal(e) => {
                let _ = tx.send(Err(e));
                return;
            }
            ConnOutcome::Dropped => {
                stalled = if last_seq > before { 0 } else { stalled + 1 };
                if stalled > MAX_STALLED_RECONNECTS {
                    let _ = tx.send(Err(StreamError::Network(
                        "stream dropped and could not be resumed".to_string(),
                    )));
                    return;
                }
                // Keep the controller's inactivity timer alive across the gap.
                if tx.send(Ok(keepalive_event())).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(
                    300 * u64::from(stalled.min(4)),
                ));
            }
        }
    }
}

/// One connection attempt. Advances `*last_seq` as events arrive so the caller
/// can resume from it, and returns how the connection ended.
#[cfg(target_family = "wasm")]
fn read_stream_once(
    _url: &str,
    _last_seq: &mut u64,
    _tx: &mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> ConnOutcome {
    ConnOutcome::Fatal(StreamError::Network(
        "agent streaming unavailable in the web viewer".to_string(),
    ))
}

#[cfg(not(target_family = "wasm"))]
fn read_stream_once(
    url: &str,
    last_seq: &mut u64,
    tx: &mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> ConnOutcome {
    // Only a session the client has given up on is a sign-in problem. Not
    // reaching the server for a token (asleep, offline, a blip) is a dropped
    // connection, retried like any other: treating it as auth signed people
    // out whenever the network hiccuped mid-turn.
    let token = match API_CLIENT.get_access_token() {
        Ok(t) => t,
        Err(crate::api::ApiError::NotAuthenticated) => {
            return ConnOutcome::Fatal(StreamError::Auth("session expired".to_string()))
        }
        Err(_) => return ConnOutcome::Dropped,
    };

    // No OVERALL timeout — an SSE body never ends, and ureq's `timeout()` spans
    // the whole body, so it killed every healthy long turn at that mark. A
    // connect timeout plus a per-read timeout instead: the server pings every
    // 20s, so any healthy stream has bytes within 20s; 90s of socket silence
    // means the connection is genuinely dead and we reconnect.
    let agent = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(90))
        .build();

    let mut req = agent
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "text/event-stream")
        .set("Cache-Control", "no-cache");
    if *last_seq > 0 {
        // SSE-standard resume cursor: replay everything after this seq.
        req = req.set("Last-Event-ID", &last_seq.to_string());
    }

    let resp = match req.call() {
        Ok(r) => r,
        // 409 = the server can no longer resume from our position (gap older
        // than its replay buffer). 404 = the task is gone. Both are terminal.
        Err(ureq::Error::Status(409, _)) => {
            return ConnOutcome::Fatal(StreamError::Network(
                "the agent turn could not be resumed — please try again".to_string(),
            ));
        }
        Err(ureq::Error::Status(404, _)) => {
            return ConnOutcome::Fatal(StreamError::Network(
                "the agent turn is no longer available — please try again".to_string(),
            ));
        }
        Err(ureq::Error::Status(401, _)) => {
            // The cached access token may have lapsed early. Drop it and get a
            // fresh one: the client signs out only if the refresh itself is
            // refused; otherwise reconnect (bounded) with the new token.
            API_CLIENT.forget_access_token();
            return match API_CLIENT.get_access_token() {
                Err(crate::api::ApiError::NotAuthenticated) => {
                    ConnOutcome::Fatal(StreamError::Auth("session expired".to_string()))
                }
                _ => ConnOutcome::Dropped,
            };
        }
        // Any other connect error is treated as a transient drop, worth a retry.
        Err(_) => return ConnOutcome::Dropped,
    };

    consume_sse_response(resp, last_seq, tx)
}

#[cfg(not(target_family = "wasm"))]
fn consume_sse_response(
    resp: ureq::Response,
    last_seq: &mut u64,
    tx: &mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> ConnOutcome {
    let reader = BufReader::new(resp.into_reader());
    let mut current_event_type = String::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            // Read error mid-stream (incl. the 120s cut) — reconnect.
            Err(_) => return ConnOutcome::Dropped,
        };

        if line.starts_with("id: ") {
            // Advance the resume cursor. The server emits `id:` before each
            // event's `data:`, so by the time we forward the event below the
            // cursor already covers it.
            if let Ok(n) = line[4..].trim().parse::<u64>() {
                *last_seq = n;
            }
        } else if line.starts_with(':') {
            // Keepalive ping — liveness only.
            if tx.send(Ok(keepalive_event())).is_err() {
                return ConnOutcome::Done;
            }
        } else if line.starts_with("event: ") {
            current_event_type = line[7..].trim().to_string();
        } else if line.starts_with("data: ") {
            let data = &line[6..];
            if data == "[DONE]" {
                return ConnOutcome::Done;
            }
            match serde_json::from_str::<PipelineEvent>(data) {
                Ok(ev) => {
                    let terminal = matches!(
                        ev.kind,
                        AgentEventKind::Done | AgentEventKind::StepDone | AgentEventKind::Error
                    );
                    if tx.send(Ok(ev)).is_err() {
                        return ConnOutcome::Done;
                    }
                    if terminal {
                        return ConnOutcome::Done;
                    }
                }
                Err(e) => {
                    log::warn!(
                        "[pipeline] parse SSE event (type={current_event_type:?}): {e}: {data}"
                    );
                }
            }
        }
    }
    // Reader ended without a terminal event — the socket closed under us.
    ConnOutcome::Dropped
}
