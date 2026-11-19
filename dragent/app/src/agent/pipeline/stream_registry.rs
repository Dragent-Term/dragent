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
use crate::agent::pipeline::stream_types::parse_wait_to_retry_label;
use crate::agent::pipeline::stream_types::HOST_TOOL_DEFS;
use crate::agent::pipeline::HostToolDef;
use crate::agent::pipeline::StreamError;
use crate::api::API_CLIENT;

/// Serialize image attachments to the server wire format.
fn attachments_json(atts: &[crate::agent::attachments::MessageAttachment]) -> serde_json::Value {
    serde_json::Value::Array(
        atts.iter()
            .map(|a| {
                serde_json::json!({
                    "type": "image",
                    "media_type": a.media_type,
                    "data": a.data_b64,
                })
            })
            .collect(),
    )
}

/// Register the host tools a run may call. Consumed by
/// `start_scoped_task_stream_with_run_id` for that run id.
pub fn register_host_tools(run_id: &str, defs: Vec<HostToolDef>) {
    if let Ok(mut map) = HOST_TOOL_DEFS.lock() {
        map.insert(run_id.to_string(), defs);
    }
}

pub(crate) fn take_host_tools(run_id: &str) -> Vec<serde_json::Value> {
    HOST_TOOL_DEFS
        .lock()
        .ok()
        .and_then(|mut map| map.remove(run_id))
        .unwrap_or_default()
        .into_iter()
        .map(|def| {
            serde_json::json!({
                "name": def.name,
                "description": def.description,
                "parameters": def.parameters,
            })
        })
        .collect()
}

pub(crate) fn restrict_extra_tools(
    tools: &mut Vec<serde_json::Value>,
    allowed_tools: Option<&[String]>,
) {
    let Some(allowed_tools) = allowed_tools else {
        return;
    };
    let allowed = allowed_tools
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    tools.retain(|tool| {
        tool["name"]
            .as_str()
            .is_some_and(|name| allowed.contains(name))
    });
}

/// Build the common task-start payload and credential-routing headers. Both the
/// legacy JSON create endpoint and the preferred streaming POST use this exact
/// representation so switching transports cannot change model context or tools.
pub(crate) fn build_task_request(
    model_name: &str,
    system: &str,
    query: &str,
    prior_messages: &[crate::agent::controller::SessionMessage],
    agent_mode: &str,
    allowed_tools: Option<&[String]>,
) -> Result<(serde_json::Value, Vec<(String, String)>), StreamError> {
    // Build the messages array: prior history + current user query.
    // The server appends the current query itself, so we only send prior turns.
    let history: Vec<serde_json::Value> = prior_messages
        .iter()
        // Exclude the last message (current user query) — server adds it from `query`
        .take(prior_messages.len().saturating_sub(1))
        .map(|m| {
            let mut v = serde_json::json!({ "role": m.role, "content": m.content });
            if !m.attachments.is_empty() {
                v["attachments"] = attachments_json(&m.attachments);
            }
            v
        })
        .collect();
    // Image attachments for the CURRENT query ride at the top level — the
    // server appends the query itself as the final user message.
    let query_attachments = prior_messages
        .last()
        .filter(|m| m.role == "user" && !m.attachments.is_empty())
        .map(|m| attachments_json(&m.attachments));

    let to_tool_json = |(name, desc, schema_json): (String, String, String)| {
        let params: serde_json::Value = serde_json::from_str(&schema_json)
            .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
        serde_json::json!({ "name": name, "description": desc, "parameters": params })
    };
    let tool_defs = crate::agent::mcp::all_tool_defs();
    let prompt_defs = crate::agent::mcp::all_prompt_defs();
    let resource_defs = crate::agent::mcp::all_resource_defs();
    // Local filesystem suite (Filesystem-MCP parity) is advertised through the
    // same channel so the model can call it — executed locally, no server process.
    #[allow(unused_mut)]
    let mut fs_defs = crate::agent::local_tools::filesystem_tool_defs();
    // The services connected in Settings → Integrations, while there are any.
    #[cfg(not(target_family = "wasm"))]
    fs_defs.extend(crate::integrations::tools::agent_tool_defs());
    #[cfg(not(target_family = "wasm"))]
    fs_defs.extend(crate::cli::computer::agent_tool_defs());
    log::info!(
        "[mcp] extra_tools being sent: {} tools, {} prompts, {} resources, {} local-fs — mcp: {:?} — local-fs: {:?}",
        tool_defs.len(),
        prompt_defs.len(),
        resource_defs.len(),
        fs_defs.len(),
        tool_defs.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>(),
        fs_defs.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>(),
    );
    let mut extra_tools: Vec<serde_json::Value> = tool_defs
        .into_iter()
        .chain(prompt_defs)
        .chain(resource_defs)
        .chain(fs_defs)
        .map(to_tool_json)
        .collect();
    restrict_extra_tools(&mut extra_tools, allowed_tools);

    let mut body = serde_json::json!({
        "model_name": model_name,
        "system": system,
        "query": query,
        "messages": history,
        "agent_mode": agent_mode,
        "extra_tools": extra_tools,
    });
    if let Some(atts) = query_attachments {
        body["attachments"] = atts;
    }
    if let Some(allowed_tools) = allowed_tools {
        body["allowed_tools"] = serde_json::json!(allowed_tools);
    }

    let settings = crate::settings::AppSettings::load();
    // Reasoning depth. Omitted entirely on Default so the provider picks; the
    // server also drops it for models that reject the parameter, so this stays
    // safe when the user switches models without touching the control.
    if let Some(effort) = settings.selected_effort.wire_value() {
        body["effort"] = serde_json::Value::String(effort.to_string());
    }
    // Single source of truth for credential routing: attaches the user's own
    // key when configured (so THEIR key serves every request) and enforces
    // the fallback toggle — an Err here means fallback is off and no user key
    // exists, so the request must not run on Dragent's platform keys.
    let extra_headers = settings
        .request_key_headers(model_name)
        .map_err(StreamError::Network)?;

    let owned_headers = extra_headers
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();

    Ok((body, owned_headers))
}

pub(crate) fn map_task_start_error(e: crate::api::ApiError) -> StreamError {
    if matches!(e, crate::api::ApiError::NotAuthenticated) {
        return StreamError::Auth("session expired".to_string());
    }
    if let crate::api::ApiError::Server { status, ref body } = e {
        let server_msg = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from));
        if status == 401 {
            return StreamError::Auth(server_msg.unwrap_or_default());
        }
        if status == 402 {
            return StreamError::InsufficientCredits(server_msg.unwrap_or_else(|| {
                "Insufficient credits. Please top up your balance.".to_string()
            }));
        }
        if status == 429 {
            let raw = server_msg.unwrap_or_default();
            let wait_str = raw
                .split("please wait")
                .nth(1)
                .map(|s| s.trim())
                .unwrap_or("");
            return StreamError::RateLimited(parse_wait_to_retry_label(wait_str));
        }
    }
    StreamError::Network(e.to_string())
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn map_desktop_step_start_error(error: crate::api::ApiError) -> StreamError {
    if matches!(
        error,
        crate::api::ApiError::Server {
            status: 404 | 405,
            ..
        }
    ) {
        return StreamError::Network(
            "the server does not support private desktop-owned agent runs yet; deploy the updated server before this app build"
                .to_string(),
        );
    }
    map_task_start_error(error)
}
