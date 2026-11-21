//! Stream event handlers (tool calls).
//!
//! One method per PipelineEvent kind, moved verbatim from
//! `app/src/agent/controller/event_handlers_tools.rs` apart from the engine
//! seams: `super::block/state/types` → `crate::block/state/types`,
//! `AgentControllerEvent` → `crate::runtime::CoreEvent`, local tools →
//! `self.deps.tools` (`ToolHost`), pipeline posts → `self.deps.pipeline`
//! via `Arc` clones into `spawn_background` closures, autonomy globals →
//! `self.deps.config`, `host_verification::is_scoped_check` →
//! `self.deps.hooks.is_scoped_check`, and `crate::bg::detach` →
//! `crate::runtime::spawn_background`. No `ctx`/warpui here: results fan out
//! through `deferred_events`, and tool dispatch never takes a sink across
//! threads.

use std::sync::Arc;

use std::time::Duration;

use uuid::Uuid;

use crate::approval_policy;
use crate::autonomy::AgentAutonomySettings;
use crate::block::AgentBlockKind;
use crate::files::recommended_read_grant_root;
use crate::permissions::{AllowedRoots, DenyReason};
use crate::pipeline::{PipelineEvent, WorkspaceToolDisposition};
use crate::runtime::{spawn_background, CoreEvent};
use crate::state::ConversationState;
use crate::types::{PendingToolCall, TurnToolCall};

use super::loop_guard;
use super::session_helpers::tool_timeout_secs;
use super::tools::tool_display_name;
use super::tools::{is_checkpoint_write_tool, is_read_only_tool_op, tool_activity_label};

/// The answer to `ask_user` in an unattended turn.
pub(crate) const UNATTENDED_QUESTION: &str = "Nobody can answer: this turn was started by a \
     scheduled automation and nobody is watching. Choose the most sensible option yourself, carry \
     on, and say what you assumed in your final reply.";

/// Why an unattended turn doesn't post to a connected service.
pub(crate) const UNATTENDED_SERVICE_READ_ONLY: &str = "Not done: this turn was started by an \
     automation that can only read from connected services. Say in your final reply what you would \
     have posted or changed. (The automation's owner can allow changes under \"Connected \
     services\" in its settings.)";

/// Why an unattended turn's tool call that needs approval doesn't run.
pub(crate) const UNATTENDED_REFUSAL: &str = "Nobody is there to give it: this turn was started by \
     a scheduled automation. Find another way, or leave it and say so in your final reply.";

impl super::core::AgentController {
    /// Answer a tool call of an unattended turn right away instead of
    /// waiting for a person.
    fn answer_unattended(&mut self, call_id: &str, answer: String, is_error: bool) -> bool {
        log::info!("[agent] unattended turn answered {call_id} without a person");
        if let Some(call) = self
            .turn_tool_calls
            .iter_mut()
            .rev()
            .find(|c| c.call_id == call_id)
        {
            call.result_summary = Some(answer.clone());
            call.is_error = is_error;
        }
        let task_id = self.streaming_task_id.clone().unwrap_or_default();
        let call_id = call_id.to_string();
        let pipeline = Arc::clone(&self.deps.pipeline);
        spawn_background(Box::new(move || {
            if let Err(e) = pipeline.post_tool_result(&task_id, &call_id, &answer, is_error) {
                log::warn!("[agent-stream] unattended answer {call_id}: {e}");
            }
        }));
        // What the model says next is a fresh block, as after any tool call.
        self.reset_streaming_on_next_text = true;
        true
    }

    pub(crate) fn apply_tool_end_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        let tool_id = ev.tool_id.unwrap_or_default();
        self.ingest_shell_checkpoint(&tool_id);
        self.active_tool_calls.remove(&tool_id);
        self.completed_tool_calls.insert(tool_id.clone());
        if let Some(call) = self
            .turn_tool_calls
            .iter_mut()
            .rev()
            .find(|c| c.call_id == tool_id)
        {
            call.result_summary = ev.result_summary.clone();
            call.is_error = ev.tool_is_error.unwrap_or(false);
        }
        if let Some(bid) = self.find_tool_step_block_id(&tool_id) {
            // Pull any diff the edit tools captured during execution and
            // ATTACH it to the block, so the diff card persists to SQLite
            // and survives app restarts (the registry is memory-only).
            let captured_diff = crate::edit_diff::get(&tool_id);
            if let Some(block) = self.find_block_mut_by_id(bid) {
                if let AgentBlockKind::ToolStep {
                    status,
                    result_summary,
                    duration_ms,
                    diff,
                    ..
                } = &mut block.kind
                {
                    *status = crate::block::ToolStepStatus::Done;
                    *result_summary = ev.result_summary;
                    *duration_ms = ev.duration_ms;
                    if captured_diff.is_some() {
                        *diff = captured_diff;
                    }
                }
            }
            // update_block_in_db will resolve to the containing TurnGroup
            self.update_block_in_db(bid);
        }
        // Re-add the Working… indicator so there is no blank gap while the
        // LLM processes the tool result and generates its next response.
        self.remove_loading_block();
        let loading_id = self.push_block(
            AgentBlockKind::Loading {
                message: "Working…".to_string(),
            },
            conv_id,
        );
        self.loading_block_id = Some(loading_id);
        true
    }
    pub(crate) fn apply_tool_call_started_event(
        &mut self,
        ev: PipelineEvent,
        conv_id: Uuid,
    ) -> bool {
        // Early notification — args are still streaming. Update the loading indicator
        // so the user sees which tool is being prepared before it executes.
        let tool_name = ev.tool_name.as_deref().unwrap_or("tool");
        let label = tool_activity_label(tool_name);
        self.set_loading_message(label);
        true
    }
    pub(crate) fn apply_tool_call_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        let policy_event = self.tool_policy.as_ref().map(|_| ev.clone());
        let call_id = ev.tool_id.unwrap_or_default();
        let tool_name = ev.tool_name.clone().unwrap_or_default();
        let args_json = ev.tool_args.clone().unwrap_or_default();

        // Providers may retry or replay an SSE event. Call IDs are the
        // idempotency key: accept each ID once and return a tool error
        // for replays instead of running a command/write twice.
        if self.active_tool_calls.contains(&call_id) || self.completed_tool_calls.contains(&call_id)
        {
            let task_id = self.streaming_task_id.clone().unwrap_or_default();
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(
                    &task_id,
                    &call_id,
                    "Duplicate tool call ignored: this call ID was already processed.",
                    true,
                ) {
                    log::warn!("[agent-stream] duplicate tool result {call_id}: {e}");
                }
            }));
            return true;
        }
        self.active_tool_calls.insert(call_id.clone());
        self.turn_tool_calls.push(TurnToolCall {
            call_id: call_id.clone(),
            name: tool_name.clone(),
            args_json: args_json.clone(),
            result_summary: None,
            is_error: false,
        });

        // ask_user is an interactive prompt, not an executed tool: render
        // the question card and park the stream until the user answers.
        // Intercepted here so it never counts toward the step budget and
        // never routes through approval/execution.
        if tool_name == "ask_user" {
            if self.unattended {
                return self.answer_unattended(&call_id, UNATTENDED_QUESTION.to_string(), false);
            }
            return self.begin_user_question(call_id, &args_json, conv_id);
        }

        // todowrite is local UI state, not an executed tool: replace
        // the live task list, render it, and post the JSON straight
        // back so the model continues. No parking, no approval — the
        // list itself is harmless; only the work it tracks matters.
        if tool_name == "todowrite" {
            return self.apply_todo_write(call_id, &args_json, conv_id);
        }

        // Host-owned tool: render its step card and hand the call to
        // the host. The server's ToolEnd completes the card once the
        // host posts the result.
        if let Some(def) = self.host_tools.iter().find(|def| def.name == tool_name) {
            let mutating = def.mutating;
            let task_id = self.streaming_task_id.clone().unwrap_or_default();
            self.set_loading_message(&format!("{}…", tool_display_name(&tool_name)));
            self.remove_loading_block();
            if let Some(bid) = self.thinking_block_id.take() {
                if let Some(block) = self.find_block_mut_by_id(bid) {
                    if let AgentBlockKind::ThinkingStep { complete, .. } = &mut block.kind {
                        *complete = true;
                    }
                }
            }
            self.push_turn_block(
                AgentBlockKind::ToolStep {
                    tool_id: call_id.clone(),
                    name: tool_display_name(&tool_name),
                    icon: ev.tool_icon.unwrap_or_default(),
                    args_summary: ev.args_summary.unwrap_or_default(),
                    status: crate::block::ToolStepStatus::Running,
                    result_summary: None,
                    duration_ms: None,
                    expanded: false,
                    diff: None,
                },
                mutating,
                conv_id,
            );
            self.reset_streaming_on_next_text = true;
            self.deferred_events.push(CoreEvent::HostToolCall {
                task_id,
                call_id,
                name: tool_name,
                args_json,
            });
            return true;
        }

        // Embedded-host findings budget: past the cap the model is told
        // to synthesize instead of researching further. Counted here so
        // the interactive tools above never eat into it.
        if let Some(budget) = self.tool_call_budget {
            self.turn_executed_tool_calls += 1;
            if self.turn_executed_tool_calls > budget {
                let task_id_b = self.streaming_task_id.clone().unwrap_or_default();
                let call_id_b = call_id.clone();
                if let Some(call) = self
                    .turn_tool_calls
                    .iter_mut()
                    .rev()
                    .find(|c| c.call_id == call_id)
                {
                    call.result_summary = Some("budget reached".into());
                    call.is_error = true;
                }
                let pipeline = Arc::clone(&self.deps.pipeline);
                spawn_background(Box::new(move || {
                    if let Err(e) = pipeline.post_tool_result(
                            &task_id_b,
                            &call_id_b,
                            "Findings budget reached: synthesize the evidence gathered so far into the final response now.",
                            true,
                        ) {
                            log::warn!("[agent-stream] budget reject {call_id_b}: {e}");
                        }
                }));
                return true;
            }
        }

        // Server-executed tools (web_search / fetch_url run on the
        // backend through the provider's native web tooling): this
        // event is display-only. Render the ToolStep card in Running
        // state and stop — the server posts its own result and the
        // following ToolEnd completes the card. No approval, no local
        // execution, no step budget.
        if ev.server_executed.unwrap_or(false) {
            let tool_label = tool_activity_label(&tool_name);
            self.set_loading_message(tool_label);
            self.remove_loading_block();
            if let Some(bid) = self.thinking_block_id.take() {
                if let Some(block) = self.find_block_mut_by_id(bid) {
                    if let AgentBlockKind::ThinkingStep { complete, .. } = &mut block.kind {
                        *complete = true;
                    }
                }
            }
            let is_read = is_read_only_tool_op(&tool_name);
            self.push_turn_block(
                AgentBlockKind::ToolStep {
                    tool_id: call_id,
                    name: tool_display_name(&tool_name),
                    icon: ev.tool_icon.unwrap_or_default(),
                    args_summary: ev.args_summary.unwrap_or_default(),
                    status: crate::block::ToolStepStatus::Running,
                    result_summary: None,
                    duration_ms: None,
                    expanded: false,
                    diff: None,
                },
                !is_read,
                conv_id,
            );
            self.reset_streaming_on_next_text = true;
            return true;
        }

        // Doom-loop guard (item 4): the model re-issuing the identical
        // call (name + args) makes no progress while burning quota. The
        // third consecutive identical call is answered with guidance
        // instead of executing. Placed after the non-executed intercepts
        // (ask_user/todowrite/host/budget/server-executed return above),
        // so only real execution candidates are guarded; `turn_tool_calls`
        // already holds this candidate as its last entry.
        if loop_guard::doom_loop_tripped(&self.turn_tool_calls) {
            if let Some(call) = self
                .turn_tool_calls
                .iter_mut()
                .rev()
                .find(|c| c.call_id == call_id)
            {
                call.is_error = true;
            }
            let task_id_d = self.streaming_task_id.clone().unwrap_or_default();
            let call_id_d = call_id.clone();
            let message =
                loop_guard::doom_loop_message(&tool_name, loop_guard::DOOM_LOOP_THRESHOLD);
            log::info!("[loop-guard] tripped on identical {tool_name}");
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(&task_id_d, &call_id_d, &message, true) {
                    log::warn!("[agent-stream] loop-guard reject {call_id_d}: {e}");
                }
            }));
            return true;
        }

        let command = if tool_name == "run_command" {
            approval_policy::command_from_args(&args_json)
        } else if tool_name == "computer" {
            serde_json::from_str::<serde_json::Value>(&args_json)
                .ok()
                .and_then(|args| args["action"].as_str().map(str::to_string))
        } else {
            None
        };
        if let Some(reason) = approval_policy::capability_denial(
            &tool_name,
            command.as_deref(),
            self.read_only_mode
                && !(tool_name == "run_command"
                    && self.tool_policy.is_some()
                    && self.deps.hooks.is_scoped_check(self.pane_uuid, &args_json)),
            self.gate_tools_until_plan,
        ) {
            log::info!("[capabilities] refused {tool_name}: {reason}");
            let task_id_r = self.streaming_task_id.clone().unwrap_or_default();
            let call_id_r = call_id.clone();
            let reason = reason.to_string();
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(&task_id_r, &call_id_r, &reason, true) {
                    log::warn!("[agent-stream] capability reject {call_id_r}: {e}");
                }
            }));
            return true;
        }

        // Every filesystem read tool shares one path boundary. Earlier
        // only `read_file` prompted, so grep/list/search/multi-read (and
        // worker execution) could bypass the user's decision.
        let all_read_paths =
            self.deps
                .tools
                .read_paths_for_tool(&tool_name, &args_json, self.pane_uuid);
        let external_read_paths =
            self.deps
                .tools
                .external_read_paths_for_tool(&tool_name, &args_json, self.pane_uuid);
        let external_write_paths =
            self.deps
                .tools
                .external_write_paths_for_tool(&tool_name, &args_json, self.pane_uuid);
        let out_of_cwd_write = !external_write_paths.is_empty();
        // A change outside the project asks like a read outside it, in every
        // mode ("Accept edits" covers the project, not `~/.zshrc`).
        let out_of_cwd_read = !external_read_paths.is_empty() || out_of_cwd_write;

        // Sensitive locations are a hard denial, not an approval choice.
        // Keep this aligned with the executor-side check so even an
        // alternate execution path cannot bypass it.
        let sensitive = all_read_paths.iter().find_map(|path| {
            AllowedRoots::new([path.clone()])
                .check(path)
                .err()
                .filter(|reason| matches!(reason, DenyReason::SensitivePath))
                .map(|reason| (path.clone(), reason))
        });
        if let Some((path, reason)) = sensitive {
            let task_id_r = self.streaming_task_id.clone().unwrap_or_default();
            let call_id_r = call_id.clone();
            let message = format!("Permission denied for {}: {reason}", path.display());
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(&task_id_r, &call_id_r, &message, true) {
                    log::warn!("[agent-stream] protected-path reject {call_id_r}: {e}");
                }
            }));
            return true;
        }
        // Approving grants this one call the folders involved; for writes,
        // the folder the file goes in.
        let mut external_read_roots = external_read_paths
            .iter()
            .map(|path| recommended_read_grant_root(path))
            .chain(
                external_write_paths
                    .iter()
                    .map(|path| path.parent().unwrap_or(path).to_path_buf()),
            )
            .collect::<Vec<_>>();
        external_read_roots.sort();
        external_read_roots.dedup();
        // The user's approval mode and what the call does decide whether
        // it asks, the same on every route (the server's own approval flag
        // is not consulted, so the user's own key behaves alike). The
        // out-of-project read guard is passed separately: it is the one
        // prompt no mode waives.
        // Engine seam: the app reads the approval mode from the
        // `AppSettings` global into `AgentHostConfig::approval_mode`; the
        // engine config carries only that and the safe-commands toggle.
        // An unattended turn doesn't ask: nobody is there to answer.
        let autonomy = AgentAutonomySettings {
            auto_run_safe_commands: self.deps.config.auto_run_safe_commands,
            ..AgentAutonomySettings::default()
        }
        .with_approval_mode(if self.unattended {
            crate::autonomy::ApprovalMode::Auto
        } else {
            self.deps.config.approval_mode
        });
        let run_command_arg = if tool_name == "run_command" {
            approval_policy::command_from_args(&args_json)
        } else {
            None
        };
        // requires_approval/auto_deny/auto_allow are folded into the
        // declarative decide() below (with rule attribution), computed
        // after the persisted store reload so cross-tab grants apply.

        let tool_label = tool_activity_label(&tool_name);
        self.set_loading_message(tool_label);
        self.remove_loading_block();
        self.tool_call_count += 1;

        // Close any open ThinkingStep
        if let Some(bid) = self.thinking_block_id.take() {
            if let Some(block) = self.find_block_mut_by_id(bid) {
                if let AgentBlockKind::ThinkingStep { complete, .. } = &mut block.kind {
                    *complete = true;
                }
            }
        }

        // Plan gate: block file-write tools until user approves the plan.
        // run_command is excluded — it goes through per-command ApprovalRequest instead.
        let is_write_tool = is_checkpoint_write_tool(&tool_name);
        if self.gate_tools_until_plan && is_write_tool {
            let task_id_g = self.streaming_task_id.clone().unwrap_or_default();
            let call_id_g = call_id.clone();
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(
                    &task_id_g,
                    &call_id_g,
                    "Tool call blocked: present a plan first. \
                         The user must approve the plan before any write or execute tools run.",
                    true,
                ) {
                    log::warn!("[agent-stream] plan-gate reject {call_id_g}: {e}");
                }
            }));
            return true;
        }

        // Another tab may have granted a directory since this controller
        // was created. Reload the tiny JSON store at the approval boundary
        // so “Always allow” takes effect app-wide immediately.
        self.permission_store = crate::permission_store::PermissionStore::load();

        // Declarative evaluation with attribution: first matching rule
        // wins, and its name lands in the log so a surprising prompt,
        // denial, or auto-run is traceable to its cause.
        let decision = approval_policy::decide(&approval_policy::ApprovalQuery {
            settings: &autonomy,
            tool_name: &tool_name,
            command: run_command_arg.as_deref(),
            out_of_project_read: out_of_cwd_read,
            // A remembered read grant never waives asking to write.
            external_read_allowed: out_of_cwd_read
                && !out_of_cwd_write
                && external_read_paths
                    .iter()
                    .all(|path| self.permission_store.is_read_path_allowed(path)),
            persisted_deny: self.permission_store.is_always_denied(&tool_name),
            persisted_allow: self.permission_store.is_always_allowed(&tool_name),
            force_command_approval: self.force_command_approval,
        });
        // Contract-governed attempt: the Work Contract, not the user's
        // autonomy setting, decides. Denials are answered as tool
        // errors; consent requests reuse the approval card below.
        let mut policy_reason: Option<String> = None;
        let decision = match policy_event
            .as_ref()
            .zip(self.tool_policy.as_ref())
            .map(|(event, policy)| policy(event))
        {
            None => decision,
            Some(WorkspaceToolDisposition::Execute)
            | Some(WorkspaceToolDisposition::ObserveServerResult) => {
                approval_policy::ApprovalDecision::Allow {
                    rule: "work-contract",
                }
            }
            Some(WorkspaceToolDisposition::NeedsConsent { reason }) => {
                policy_reason = Some(reason);
                approval_policy::ApprovalDecision::Ask {
                    rule: "work-contract",
                }
            }
            Some(WorkspaceToolDisposition::Denied { reason }) => {
                log::info!("[work-contract] refused {tool_name}: {reason}");
                let task_id_r = self.streaming_task_id.clone().unwrap_or_default();
                let call_id_r = call_id.clone();
                let refusal = reason.clone();
                if let Some(call) = self
                    .turn_tool_calls
                    .iter_mut()
                    .rev()
                    .find(|c| c.call_id == call_id)
                {
                    call.result_summary = Some(reason);
                    call.is_error = true;
                }
                let pipeline = Arc::clone(&self.deps.pipeline);
                spawn_background(Box::new(move || {
                    if let Err(e) =
                        pipeline.post_tool_result(&task_id_r, &call_id_r, &refusal, true)
                    {
                        log::warn!("[agent-stream] contract reject {call_id_r}: {e}");
                    }
                }));
                return true;
            }
        };
        log::info!(
            "[approval] tool={} mode={:?} auto_run_safe={} effect={:?} out_of_cwd={} cmd_safe={:?} -> {:?} ({})",
            tool_name,
            autonomy.approval_mode(),
            autonomy.auto_run_safe_commands,
            approval_policy::effect(&tool_name, run_command_arg.as_deref()),
            out_of_cwd_read,
            run_command_arg
                .as_deref()
                .map(approval_policy::is_safe_readonly_command),
            match &decision {
                approval_policy::ApprovalDecision::Allow { .. } => "AUTO-RUN",
                approval_policy::ApprovalDecision::Ask { .. } => "PROMPT",
                approval_policy::ApprovalDecision::Deny { .. } => "DENY",
            },
            decision.rule(),
        );

        // An automation posts to connected services only when it's allowed
        // to: nobody sees what goes out under the user's name.
        if self.unattended
            && approval_policy::is_service_write(&tool_name)
            && !self.unattended_service_writes
        {
            return self.answer_unattended(
                &call_id,
                UNATTENDED_SERVICE_READ_ONLY.to_string(),
                true,
            );
        }

        // What still asks in Auto (a read outside the project, a contract's
        // consent) is refused instead of waiting for nobody.
        if self.unattended && decision.asked() {
            let reason = policy_reason
                .clone()
                .unwrap_or_else(|| decision.rule().to_string());
            return self.answer_unattended(
                &call_id,
                format!("Not run: this needs someone's approval ({reason}). {UNATTENDED_REFUSAL}"),
                true,
            );
        }

        if matches!(decision, approval_policy::ApprovalDecision::Deny { .. }) {
            // Auto-reject without showing UI
            let task_id_c = self.streaming_task_id.clone().unwrap_or_default();
            let call_id_c = call_id.clone();
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(
                    &task_id_c,
                    &call_id_c,
                    "Operation permanently denied.",
                    true,
                ) {
                    log::warn!("[agent-stream] auto-deny {call_id_c}: {e}");
                }
            }));
        } else if decision.asked() {
            // Pause and ask the user — tool executes only after they approve.
            let action_id = Uuid::new_v4();
            let summary = ev.args_summary.unwrap_or_default();
            let external_scope_label = external_read_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let (display_command, display_reason) = if let Some(reason) = policy_reason.clone() {
                (summary.clone(), reason)
            } else if out_of_cwd_write {
                let targets = external_write_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    summary.clone(),
                    format!(
                        "Agent wants to change files outside your project: {targets}. \
                             Approve only if you expected this."
                    ),
                )
            } else if out_of_cwd_read {
                (
                    summary.clone(),
                    format!(
                        "Agent wants to read outside your project directory. \
                             “Always allow” remembers only: {external_scope_label}"
                    ),
                )
            } else if tool_name == "run_command" {
                (summary, "Agent wants to run a shell command".to_string())
            } else {
                (
                    summary,
                    format!("Agent wants to {}", tool_name.replace('_', " ")),
                )
            };
            self.pending_tool_calls.insert(
                action_id,
                PendingToolCall {
                    call_id,
                    tool_name: tool_name.clone(),
                    args_json,
                    run_in_terminal: false,
                    allow_terminal_cwd_change: self.allow_terminal_cwd_change,
                    external_read_roots,
                },
            );
            if let Some(event) = self.enqueue_approval(super::core::QueuedApproval {
                action_id,
                command: display_command,
                reason: display_reason,
                conversation_id: conv_id,
            }) {
                self.deferred_events.push(event);
            }
        } else {
            if tool_name == "run_command" {
                self.register_shell_checkpoint(&call_id, &args_json);
            } else if is_write_tool {
                self.capture_write_checkpoint(&tool_name, &args_json);
            }
            // Record files written/edited by the agent for the diff bar UI.
            if is_write_tool {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&args_json) {
                    // write_file / edit_file: { "path": "..." }
                    if let Some(p) = v.get("path").and_then(|x| x.as_str()) {
                        let rel = p.trim_start_matches('/');
                        if !self.agent_modified_files.iter().any(|f| f == rel) {
                            self.agent_modified_files.push(rel.to_string());
                        }
                    }
                    // batch_edit: { "edits": [{ "path": "..." }, ...] }
                    if let Some(edits) = v.get("edits").and_then(|e| e.as_array()) {
                        for edit in edits {
                            if let Some(p) = edit.get("path").and_then(|x| x.as_str()) {
                                let rel = p.trim_start_matches('/');
                                if !self.agent_modified_files.iter().any(|f| f == rel) {
                                    self.agent_modified_files.push(rel.to_string());
                                }
                            }
                        }
                    }
                }
            }

            // Safe tool — execute in a background thread with a timeout and
            // cancellation so Stop never leaves the server waiting indefinitely.
            let task_id = self.streaming_task_id.clone().unwrap_or_default();
            let tool_name_c = tool_name.clone();
            let args_json_c = args_json.clone();
            let call_id_c = call_id.clone();
            let cancelled = Arc::clone(&self.tool_cancelled);
            let pane_uuid = self.pane_uuid;
            let tools = Arc::clone(&self.deps.tools);
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                let timeout_secs = tool_timeout_secs(&tool_name_c);
                let (tx, rx) = std::sync::mpsc::channel::<(String, bool)>();
                let call_id_exec = call_id_c.clone();
                let tool_name_exec = tool_name_c.clone();
                spawn_background(Box::new(move || {
                    // Tag the execution with the pipeline call id so
                    // run_command registers under it — the ToolStep
                    // block with the same id renders the live process.
                    let result = tools.execute_with_call_id(
                        &call_id_exec,
                        &tool_name_exec,
                        &args_json_c,
                        pane_uuid,
                    );
                    let _ = tx.send(result);
                }));
                let deadline = web_time::Instant::now() + Duration::from_secs(timeout_secs);
                let (output, is_error) = loop {
                    let remaining = deadline.saturating_duration_since(web_time::Instant::now());
                    if remaining.is_zero() {
                        break (
                            format!(
                                "Tool timed out after {timeout_secs}s. Try a narrower query, a more specific path, or a different approach."
                            ),
                            true,
                        );
                    }
                    match rx.recv_timeout(remaining.min(Duration::from_secs(1))) {
                        Ok(result) => break result,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            break ("Tool execution failed unexpectedly.".to_string(), true);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                                break ("Tool cancelled by user.".to_string(), true);
                            }
                        }
                    }
                };
                // Bound oversized results before POST (item 4): spill +
                // truncate like the local-run path, with a delegate hint
                // steering the model toward explore workers instead of
                // re-reading truncated sources itself.
                let output = bounded_tool_result(&tool_name_c, output);
                if let Err(e) = pipeline.post_tool_result(&task_id, &call_id_c, &output, is_error) {
                    log::warn!("[agent-stream] post_tool_result {call_id_c}: {e}");
                }
            }));
            // Show a ToolStep block in Running state — KindToolEnd will update it to Done.
            // Route read-only tools into ActivityGroup (grouped), write/exec tools directly.
            let is_read = is_read_only_tool_op(&tool_name);
            self.push_turn_block(
                AgentBlockKind::ToolStep {
                    tool_id: call_id,
                    name: tool_display_name(&tool_name),
                    icon: ev.tool_icon.unwrap_or_default(),
                    args_summary: ev.args_summary.unwrap_or_default(),
                    status: crate::block::ToolStepStatus::Running,
                    result_summary: None,
                    duration_ms: None,
                    expanded: false,
                    diff: None,
                },
                !is_read,
                conv_id,
            );
        }
        // The LLM's next text response (after tool results are returned)
        // must appear as a fresh block, not appended to the previous round's text.
        self.reset_streaming_on_next_text = true;
        true
    }
}

/// Bound oversized tool output before POST, with a delegation steer: when the
/// result was truncated, tell the model to fan research out to an explore
/// worker instead of re-reading the truncated source itself. Untruncated
/// results pass through byte-identical.
///
/// Engine-local copy of the app pipeline's
/// `bounded_tool_output_with_delegate_hint` fallback path: the engine owns no
/// spill dir, so the note carries no spill path — exactly what the app posts
/// when its spill fails.
fn bounded_tool_output_with_delegate_hint(output: String) -> String {
    const MAX_TOOL_RESULT_BYTES: usize = 48_000;
    if output.len() <= MAX_TOOL_RESULT_BYTES {
        return output;
    }
    let mut bounded = output;
    let mut end = MAX_TOOL_RESULT_BYTES;
    while !bounded.is_char_boundary(end) {
        end -= 1;
    }
    bounded.truncate(end);
    bounded.push_str("\n… [tool output truncated to fit context window]");
    format!(
        "{bounded}\nIf you need more of this output, delegate a focused research \
         subtask via spawn_subagent (role explore) instead of re-reading the full \
         source yourself — the worker returns a digest that fits context."
    )
}

fn bounded_tool_result(tool: &str, output: String) -> String {
    // Screenshots travel as a bounded image envelope until the desktop run
    // converts them into a multimodal message; text truncation would corrupt it.
    if tool == "computer" && output.len() <= 8 * 1024 * 1024 {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&output) {
            if value["computer_snapshot"] == true && value["attachment"]["type"] == "image" {
                return output;
            }
        }
    }
    bounded_tool_output_with_delegate_hint(output)
}

#[cfg(test)]
mod computer_image_tests {
    use super::*;
    #[test]
    fn screenshot_envelopes_survive_text_output_limits() {
        let output = serde_json::json!({"computer_snapshot":true,"output":"window tree", "attachment":{"type":"image","data":"a".repeat(100_000)}}).to_string();
        assert_eq!(bounded_tool_result("computer", output.clone()), output);
        assert!(bounded_tool_result("read_file", output.clone()).len() < output.len());
        assert!(bounded_tool_result("computer", "plain text ".repeat(100_000)).len() < 50_000);
    }
}
