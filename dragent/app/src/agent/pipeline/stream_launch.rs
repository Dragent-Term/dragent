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
use crate::agent::pipeline::stream_local::normalize_desktop_request;
use crate::agent::pipeline::stream_local::run_local_checkpoint;
use crate::agent::pipeline::stream_registry::build_task_request;
use crate::agent::pipeline::stream_registry::take_host_tools;
use crate::agent::pipeline::StreamError;
use crate::api::API_CLIENT;

/// Start a task and consume its initial SSE stream on the same POST response.
/// If that response drops after headers arrive, reconnect through the ordinary
/// GET endpoint from the last observed sequence number.
#[cfg(not(target_family = "wasm"))]
pub fn start_task_stream(
    model_name: &str,
    system: &str,
    query: &str,
    prior_messages: &[crate::agent::controller::SessionMessage],
    agent_mode: &str,
    // Tools this agent gets beyond everyone's (a coordination lead's).
    turn_tools: Vec<serde_json::Value>,
    tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> Result<(), StreamError> {
    let (mut body, extra_headers) =
        build_task_request(model_name, system, query, prior_messages, agent_mode, None)?;
    if !turn_tools.is_empty() {
        if let Some(extra) = body["extra_tools"].as_array_mut() {
            extra.extend(turn_tools);
        }
    }
    normalize_desktop_request(&mut body);
    let run_id = format!("local-{}", uuid::Uuid::new_v4().simple());
    let checkpoint = super::local_run::LocalRunCheckpoint::new(run_id, body);
    checkpoint.install().map_err(StreamError::Network)?;
    run_local_checkpoint(checkpoint, extra_headers, tx)
}

/// Start a desktop-owned task whose provider-visible tools are restricted by a
/// durable Work Contract. An empty allowlist intentionally gives the model no
/// tools; it is not equivalent to the unrestricted legacy entry point.
#[cfg(not(target_family = "wasm"))]
pub fn start_scoped_task_stream(
    model_name: &str,
    system: &str,
    query: &str,
    prior_messages: &[crate::agent::controller::SessionMessage],
    agent_mode: &str,
    allowed_tools: &[String],
    tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> Result<(), StreamError> {
    let run_id = format!("local-{}", uuid::Uuid::new_v4().simple());
    start_scoped_task_stream_with_run_id(
        &run_id,
        model_name,
        system,
        query,
        prior_messages,
        agent_mode,
        allowed_tools,
        tx,
    )
}

#[cfg(target_family = "wasm")]
#[allow(clippy::too_many_arguments)]
pub fn start_scoped_task_stream_with_run_id(
    _run_id: &str,
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

/// Start a scoped desktop-owned task with a caller-provided stable run ID.
/// Workspace execution uses an ID derived from its canonical attempt so an
/// interrupted `awaiting_tools` checkpoint can be found after restart.
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::too_many_arguments)]
pub fn start_scoped_task_stream_with_run_id(
    run_id: &str,
    model_name: &str,
    system: &str,
    query: &str,
    prior_messages: &[crate::agent::controller::SessionMessage],
    agent_mode: &str,
    allowed_tools: &[String],
    tx: mpsc::Sender<Result<PipelineEvent, StreamError>>,
) -> Result<(), StreamError> {
    let (mut body, extra_headers) = build_task_request(
        model_name,
        system,
        query,
        prior_messages,
        agent_mode,
        Some(allowed_tools),
    )?;
    // Host-executed tools registered for this run ride alongside the MCP /
    // local-fs definitions (already allowlist-filtered above; the host lists
    // its tools in `allowed_tools` too, so the server offers them).
    let host_tools = take_host_tools(run_id);
    if !host_tools.is_empty() {
        if let Some(extra) = body["extra_tools"].as_array_mut() {
            extra.extend(host_tools);
        }
    }
    normalize_desktop_request(&mut body);
    let checkpoint = super::local_run::LocalRunCheckpoint::new(run_id.to_string(), body);
    checkpoint.install().map_err(StreamError::Network)?;
    run_local_checkpoint(checkpoint, extra_headers, tx)
}
