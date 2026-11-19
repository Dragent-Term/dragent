//! Principal streaming passes and response finishing.
//!
//! Moved verbatim from shape_principal; same behavior.

//! Principal shaping turn for the Shape workspace.
//!
//! The model proposes structured work and may research the project with
//! read-only tools (same terminal-agent rhythm: Working → Reading/Searching →
//! Streaming); it still cannot execute commands, edit files, create runtime
//! agents, or grant authority. Parsing and normalization happen off the UI
//! thread before the result reaches workspace state.

use super::principal_normalize::*;
use super::principal_text::*;
use super::principal_types::*;
use super::principal_validate::*;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize};

use dragent_workspace_model::{AgentRoleKind, Plan, ShapingProposal};

#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_streaming(
    model: &str,
    work_title: &str,
    user_message: &str,
    current: Option<&ShapingProposal>,
    current_plan: Option<&Plan>,
    current_roles: &[PrincipalRoleDraft],
    available_model_routes: &[PrincipalModelRoute],
    conversation_history: &[(String, String)],
    repository_context: Option<&str>,
    run_pass: &mut dyn FnMut(PrincipalPassRequest) -> Result<PrincipalPassOutcome, String>,
    mode: PrincipalTurnMode,
    turn_memory: &[String],
    approved_roles: &[String],
    resolved_decisions: &[String],
    shaping_memory: &[String],
) -> Result<PrincipalShapeResult, String> {
    let (system, prompt) = build_request(
        work_title,
        user_message,
        current,
        current_plan,
        current_roles,
        available_model_routes,
        conversation_history,
        repository_context,
        turn_memory,
        approved_roles,
        resolved_decisions,
        shaping_memory,
    );
    // Work Item Flow stage gate: appended last so it wins over the generic
    // turn-kind classification inside the shared prompt.
    let system = format!("{system}\n\n{}", turn_mode_block(mode));
    let allowed_tools = shaping_allowed_tools();
    let first = run_pass(PrincipalPassRequest {
        model: model.to_string(),
        system: system.clone(),
        prompt,
        allowed_tools: allowed_tools.clone(),
        note: None,
    })?;
    let mut content = first.content;
    let mut observed_files: Vec<String> = Vec::new();
    for call in &first.tool_calls {
        for observed in observed_files_from_tool_args(&call.args_json) {
            if !observed_files.contains(&observed) {
                observed_files.push(observed);
            }
        }
    }
    // Truncation safety net: no output cap is set client-side and the server
    // leaves the provider default in place, but a provider can still cut a
    // long proposal off mid-JSON. Continuing the SAME response (instead of
    // failing into the user-facing "inconsistent proposal" error) keeps the
    // turn alive and streams as one continuous agent response.
    if is_truncated_json(&content) {
        log::warn!("[workspace-shape] Principal response was cut off mid-JSON; continuing it");
        let continuation = run_pass(PrincipalPassRequest {
            model: model.to_string(),
            system,
            prompt: shaping_continuation_prompt(&content),
            allowed_tools,
            note: Some("Continuing the interrupted response…".into()),
        })?;
        content.push_str(&continuation.content);
        for call in &continuation.tool_calls {
            for observed in observed_files_from_tool_args(&call.args_json) {
                if !observed_files.contains(&observed) {
                    observed_files.push(observed);
                }
            }
        }
    }
    if content.trim().is_empty() {
        return Err("Principal stream ended without a response".into());
    }
    finish_response_with_observed(
        &content,
        available_model_routes,
        repository_context,
        &observed_files,
    )
}

/// What the conversation shows for a Principal turn's streamed text. The
/// model answers with one JSON object; the user should only ever see its
/// `assistant_message`. While streaming, the partially received string is
/// extracted (`conversational_preview`); once complete, the parsed field is
/// used so escapes and the final sentence render exactly.
pub(crate) fn display_text(raw: &str, is_streaming: bool) -> String {
    if !is_streaming {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(strip_code_fence(raw.trim())) {
            // Shaping turns answer in `assistant_message`; coordination turns
            // (the same conversation, mid-execution) answer in `reply_body`,
            // optionally with an escalation the human must see.
            for key in ["assistant_message", "reply_body"] {
                if let Some(message) = value.get(key).and_then(|m| m.as_str()) {
                    if !message.trim().is_empty() {
                        let escalation = value
                            .get("escalation_summary")
                            .and_then(|m| m.as_str())
                            .filter(|m| !m.trim().is_empty());
                        return match escalation {
                            Some(escalation) => format!("{message}\n\n**Needs you:** {escalation}"),
                            None => message.to_string(),
                        };
                    }
                }
            }
            if let Some(escalation) = value
                .get("escalation_summary")
                .and_then(|m| m.as_str())
                .filter(|m| !m.trim().is_empty())
            {
                return format!("**Needs you:** {escalation}");
            }
        }
    }
    conversational_preview(raw)
}

/// Streaming preview: the `assistant_message` string as far as it has
/// arrived, with plain prose passed through untouched.
pub(crate) fn conversational_preview(raw: &str) -> String {
    const WAITING: &str = "Structuring the work…";
    if raw.trim().is_empty() {
        return WAITING.into();
    }
    // If the stream is already plain prose, show it directly.
    if !raw.trim_start().starts_with('{') && !raw.trim_start().starts_with("```") {
        return raw.to_string();
    }
    let Some((key, key_len)) = raw
        .find("\"assistant_message\"")
        .map(|at| (at, "\"assistant_message\"".len()))
        .or_else(|| {
            raw.find("\"reply_body\"")
                .map(|at| (at, "\"reply_body\"".len()))
        })
    else {
        return WAITING.into();
    };
    let tail = &raw[key + key_len..];
    let Some(colon) = tail.find(':') else {
        return WAITING.into();
    };
    let value = tail[colon + 1..].trim_start();
    let Some(value) = value.strip_prefix('"') else {
        return WAITING.into();
    };
    let mut escaped = false;
    let mut end = None;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            end = Some(index);
            break;
        }
    }
    let fragment = &value[..end.unwrap_or(value.len())];
    if fragment.is_empty() {
        return WAITING.into();
    }
    if end.is_some() {
        serde_json::from_str::<String>(&format!("\"{fragment}\""))
            .unwrap_or_else(|_| fragment.to_string())
    } else {
        fragment
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    }
}

/// Cut-off detector for the truncation safety net: true only when the stream
/// ended mid-JSON (EOF / unbalanced braces). A complete object that merely
/// fails schema validation is NOT truncation — the repair pass owns that.
pub(crate) fn is_truncated_json(content: &str) -> bool {
    let body = strip_code_fence(content.trim());
    let Some(start) = body.find('{') else {
        return false;
    };
    let span = match body.rfind('}') {
        Some(end) if end > start => &body[start..=end],
        _ => &body[start..],
    };
    if span.trim().is_empty() {
        return false;
    }
    match serde_json::from_str::<serde_json::Value>(span) {
        Ok(_) => false,
        Err(error) => {
            error.is_eof()
                || span.chars().filter(|c| *c == '{').count()
                    != span.chars().filter(|c| *c == '}').count()
        }
    }
}

/// Continuation instruction for a cut-off response: resume the exact byte
/// stream, never restart or re-explain, so the combined text parses as one
/// complete proposal object.
pub(crate) fn shaping_continuation_prompt(partial: &str) -> String {
    format!(
        "Your previous response was cut off before the JSON object was complete. Continue EXACTLY where it stopped: output ONLY the remaining characters that complete the JSON object — no preamble, no explanation, no repetition of what was already sent, no closing commentary. The partial response so far is:\n```json\n{}\n```",
        partial.trim()
    )
}

/// Headless read-only provider pass — the Principal's COORDINATION turns,
/// which fire with no tab open and can only read and reply/escalate. The
/// interactive shaping turns don't come here: they run on the work item's
/// `AgentController` (see `generate_streaming` / `PrincipalPassRequest`).
pub(crate) fn stream_headless_pass(
    model: &str,
    system: &str,
    prompt: &str,
    pane_id: uuid::Uuid,
    allowed_tools: &[String],
) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let model = model.to_string();
    let system = system.to_string();
    let prompt = prompt.to_string();
    let allowed_tools_bg = allowed_tools.to_vec();
    crate::bg::detach(move || {
        if let Err(error) = crate::agent::pipeline::start_scoped_task_stream(
            &model,
            &system,
            &prompt,
            &[],
            "principal_coordination",
            &allowed_tools_bg,
            tx.clone(),
        ) {
            let _ = tx.send(Err(error));
        }
    });
    let mut content = String::new();
    let mut task_id = String::new();
    let mut findings_count = 0u32;
    while let Ok(event) = rx.recv() {
        let event = event.map_err(|error| format!("Principal stream failed: {error}"))?;
        use crate::agent::pipeline::AgentEventKind;
        match event.kind {
            AgentEventKind::TextDelta => {
                // Task-id + keepalive sentinels ride as TextDelta with error markers.
                if let Some(ref err) = event.error {
                    if let Some(id) = err.strip_prefix("__task_id__") {
                        task_id = id.to_string();
                        continue;
                    }
                    if err == "__ping__" {
                        continue;
                    }
                }
                if let Some(delta) = event.text.filter(|text| !text.is_empty()) {
                    content.push_str(&delta);
                }
            }
            AgentEventKind::StepDone => {
                if content.is_empty() {
                    if let Some(complete) = event.assistant_content.filter(|text| !text.is_empty())
                    {
                        content.push_str(&complete);
                    }
                }
            }
            AgentEventKind::Done => break,
            AgentEventKind::ToolCall => {
                if event.server_executed.unwrap_or(false) {
                    continue;
                }
                let call_id = event.tool_id.clone().unwrap_or_default();
                let name = event.tool_name.clone().unwrap_or_default();
                let args = event.tool_args.clone().unwrap_or_default();
                let (output, is_error) = if !is_principal_read_tool(&name) {
                    (
                        "Tool not permitted for a Principal coordination turn: read-only research tools only."
                            .to_string(),
                        true,
                    )
                } else {
                    findings_count += 1;
                    if findings_count > MAX_PRINCIPAL_FINDINGS {
                        (
                            "Findings budget reached: synthesize the evidence gathered so far into the response now."
                                .to_string(),
                            true,
                        )
                    } else {
                        let (output, is_error) = crate::agent::local_tools::execute_with_call_id(
                            &call_id, &name, &args, pane_id,
                        );
                        let excerpt = output.chars().take(8000).collect::<String>();
                        let bounded = if excerpt.len() < output.len() {
                            format!("{excerpt}…\n[truncated to 8000 chars]")
                        } else {
                            output
                        };
                        (bounded, is_error)
                    }
                };
                let _ =
                    crate::agent::pipeline::post_tool_result(&task_id, &call_id, &output, is_error);
            }
            AgentEventKind::Error => {
                return Err(event
                    .error
                    .unwrap_or_else(|| "Principal stream failed".into()));
            }
            AgentEventKind::ToolCallStarted
            | AgentEventKind::ToolEnd
            | AgentEventKind::Thinking => {}
        }
    }
    Ok(content)
}

pub(crate) fn build_request(
    work_title: &str,
    user_message: &str,
    current: Option<&ShapingProposal>,
    current_plan: Option<&Plan>,
    current_roles: &[PrincipalRoleDraft],
    available_model_routes: &[PrincipalModelRoute],
    conversation_history: &[(String, String)],
    repository_context: Option<&str>,
    turn_memory: &[String],
    approved_roles: &[String],
    resolved_decisions: &[String],
    shaping_memory: &[String],
) -> (String, String) {
    let current_plan_tasks = current_plan.map(|plan| {
        plan.tasks
            .iter()
            .map(|task| {
                serde_json::json!({
                    "key": task.id.0.to_string(),
                    "title": task.title,
                    "description": task.description,
                    "depends_on": task.depends_on.iter().map(|id| id.0.to_string()).collect::<Vec<_>>(),
                    "review_target": task.review_target_task_id.map(|id| id.0.to_string()),
                    "assigned_contract_id": task.assigned_contract_id,
                    "acceptance_criteria": task.acceptance_criteria,
                })
            })
            .collect::<Vec<_>>()
    });
    let has_current = current.is_some();
    let current = serde_json::to_string_pretty(&serde_json::json!({
        "proposal": current,
        "dependency_plan": current_plan_tasks,
        "proposed_roles": current_roles,
        "available_model_routes": available_model_routes,
    }))
    .unwrap_or_else(|_| "null".to_string());
    let conversation_history = conversation_history
        .iter()
        .rev()
        .take(16)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|(actor, message)| {
            serde_json::json!({
                "actor": normalize_text(actor.clone()),
                "message": normalize_text(message.clone()),
            })
        })
        .collect::<Vec<_>>();
    let conversation_history =
        serde_json::to_string_pretty(&conversation_history).unwrap_or_else(|_| "[]".to_string());
    let turn_memory = if turn_memory.is_empty() {
        "(none yet — this is the first research turn)".to_string()
    } else {
        turn_memory.join("\n")
    };
    // The brain: durable understanding of THIS work item across turns and
    // restarts — decisions with reasons, approvals, preferences, past
    // changes, and rejected options. This is why the current shape looks
    // the way it does; consult it before disturbing anything.
    let shaping_memory = if shaping_memory.is_empty() {
        "(no recorded memory yet)".to_string()
    } else {
        shaping_memory.join("\n")
    };
    // Revision guard rails: what the user already approved or resolved is
    // sacrosanct. Resolved questions stay listed (identity) but must never
    // be reopened — reproduce them verbatim in open_decisions.
    let sacrosanct = if has_current
        && (!approved_roles.is_empty() || !resolved_decisions.is_empty())
    {
        let mut lines = vec![
            "Already approved or resolved by the user — keep byte-identical unless the new message directly contradicts them (then reconcile minimally and explain the change in assistant_message):"
                .to_string(),
        ];
        if !approved_roles.is_empty() {
            lines.push(format!("Approved roles: {}", approved_roles.join("; ")));
        }
        if !resolved_decisions.is_empty() {
            lines.push(format!(
                "Resolved decisions (reproduce verbatim in open_decisions, never reopen): {}",
                resolved_decisions.join("; ")
            ));
        }
        lines.join("\n")
    } else {
        String::new()
    };
    let prompt = format!(
        "Work item: {work_title}\n\nPrior Principal conversation (oldest to newest):\n{conversation_history}\n\nPrior turn findings and decisions in THIS work item (evidence already gathered — do not re-read files listed here, do not re-ask answered questions; continue from where the last turn stopped):\n{turn_memory}\n\nWork-item memory (durable understanding of this work item — decisions with reasons, approvals, preferences, past changes, rejected options; this is why the current shape looks the way it does):\n{shaping_memory}\n\nCurrent structured shaping state:\n{current}\n\n{sacrosanct}\n\nNew user message:\n{user_message}\n\nClassify this turn first. Return turn_kind=conversation unless the user is explicitly asking to begin concrete work or explicitly asking for a complete plan/work item and enough information exists to produce one. A conversation response may explain what could be built, explore tradeoffs, gather requirements, or describe the work item you would create, then ask whether the user wants it shaped. Only turn_kind=proposal may return executable tasks and roles. When revising an existing proposal, preserve still-valid context and proposed roles, incorporate the new message, and keep unresolved questions explicit. Existing dependency-plan task keys are stable identities: preserve a task's exact key when revising that same task, including when its title or description changes. Role names are stable identities too: keep every proposed role name byte-identical unless that role itself is what changes."
    );
    (system_prompt(repository_context), prompt)
}

pub(crate) fn finish_response(
    content: &str,
    available_model_routes: &[PrincipalModelRoute],
    repository_context: Option<&str>,
) -> Result<PrincipalShapeResult, String> {
    finish_response_with_observed(content, available_model_routes, repository_context, &[])
}
