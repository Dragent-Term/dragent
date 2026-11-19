//! Query submission, approvals, cancellation, and background tasks.
//!
//! Blocks here are impl AgentController: same type, same behavior.
//!
//! Moved verbatim from `app/src/agent/controller/submit.rs` apart from the
//! engine seams: `ctx.emit` → `sink.push`, `ctx.notify` dropped,
//! `crate::bg::detach` → [`spawn_background`][crate::runtime::spawn_background],
//! `pipeline::post_tool_result` / `cancel_local_task` → `deps.pipeline` via an
//! `Arc` clone into the background closure, `local_tools::{kill_command,
//! clear_shell_state, grant_external_read_once, execute_with_call_id}` →
//! `deps.tools`, `super::block/state/types` → `crate::block/state/types`,
//! `super::session::{QueryOrigin, RuntimeEvent, resume_error_to_stream_error}`
//! → `super::session_helpers`, `super::unix_now_ms` →
//! `super::event_helpers::unix_now_ms`, checkpoint text helpers →
//! `super::checkpoint_helpers`, `BlockContext` (terminal model) →
//! [`BlockContextSnapshot`][crate::block::BlockContextSnapshot], and
//! `AgentControllerEvent`/`AgentEvent` →
//! [`CoreEvent`][crate::runtime::CoreEvent] /
//! [`AgentEvent`][crate::runtime::AgentEvent].
//!
//! Turn-launching entry points return [`LaunchedTurn`][super::run::LaunchedTurn]
//! instead of spawning WarpUI timers: the owner pumps the launched turn.
//! `start_background_watcher` is dropped (the owner reschedules
//! [`poll_background_once`][AgentController::poll_background_once] instead);
//! `approve_plan` lives in `run.rs` (via `questions.rs`) and is not
//! duplicated here. The app's `log_agent_response` file logging has no engine
//! equivalent and is dropped (the host owns logging). The mock-backend
//! approval gate reads a default autonomy profile with the host's
//! `auto_run_safe_commands` flag instead of the global `AppSettings`.

use std::path::PathBuf;

use std::sync::Arc;

use uuid::Uuid;

use super::checkpoint_helpers::{append_resume_block, push_checkpoint_section};
use super::core::AgentController;
use super::event_helpers::unix_now_ms;
use super::run::LaunchedTurn;
use super::session_helpers::{QueryOrigin, RuntimeEvent, resume_error_to_stream_error};
use super::tools::is_checkpoint_write_tool;
use crate::api::AgentError;
use crate::api_types::{AgentAction, AgentActionKind, AgentQueryRequest, AgentQueryResponse};
use crate::attachments::MessageAttachment;
use crate::autonomy::AgentAutonomySettings;
use crate::background::BackgroundCompletion;
use crate::block::{
    AgentAuthor, AgentBlock, AgentBlockKind, ApprovalStatus, BlockContextSnapshot, ErrorKind,
};
use crate::files::ToolResultKind;
use crate::runtime::{AgentEvent, CoreEvent, EventSink, spawn_background};
use crate::session::AgentSessionContext;
use crate::state::ConversationState;
use crate::types::QueuedMessage;

impl AgentController {
    /// Submit a user query. If the controller is busy, the running session is
    /// cleared and the new query starts fresh.
    pub fn submit_user_query(
        &mut self,
        request: AgentQueryRequest,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        if !matches!(self.state, ConversationState::Idle) {
            self.clear_conversation(sink);
        }

        // Reset tool round counter for each new user query.
        self.tool_round = 0;

        let query = request.query.clone();
        let cwd = request.session.cwd.clone();
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);

        self.push_block(
            AgentBlockKind::UserQuery {
                text: query.clone(),
                author: None,
                attached_blocks: request.session.attached_blocks.clone(),
                attachments: Vec::new(),
                checkpoint: None,
            },
            conv_id,
        );
        self.state = ConversationState::ProcessingQuery;
        sink.push(CoreEvent::BlocksChanged);

        self.active_session = Some(request.session.clone());
        let response = self.deps.api.submit_query(request);
        let events = self.apply_response(response, &query, &cwd);
        for ev in events {
            sink.push(ev);
        }
        None
    }

    /// Approve a legacy RunCommand action. The caller supplies its executor.
    pub fn approve_action(&mut self, action_id: Uuid, sink: &mut dyn EventSink) -> Option<String> {
        let command = self.set_approval_status(action_id, ApprovalStatus::Approved);
        if let Some(ref cmd) = command {
            self.approved_action_id = Some(action_id);
            self.approved_command = Some(cmd.clone());
            self.state = ConversationState::Idle;
            sink.push(CoreEvent::ApprovalResolved {
                action_id,
                approved: true,
            });
            sink.push(CoreEvent::BlocksChanged);
        }
        command
    }

    /// Detach a legacy approved action from shell completion notifications.
    pub fn detach_approved_command(&mut self, action_id: Uuid) {
        if self.approved_action_id == Some(action_id) {
            self.approved_action_id = None;
            self.approved_command = None;
        }
    }

    pub fn record_background_command_result(&mut self, action_id: Uuid, conversation_id: Option<Uuid>, command: String, output: String, failed: bool, sink: &mut dyn EventSink) {
        if self.active_conversation_id != conversation_id { return; }
        let conv_id = conversation_id.unwrap_or_else(Uuid::new_v4);
        self.push_block(AgentBlockKind::ActionResult {
            action_id, command, exit_code: None, output_preview: if failed { format!("Command failed.\n{output}") } else { output },
        }, conv_id);
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Reject a RunCommand action.
    pub fn reject_action(
        &mut self,
        action_id: Uuid,
        feedback: Option<String>,
        sink: &mut dyn EventSink,
    ) {
        self.set_approval_status(action_id, ApprovalStatus::Rejected);
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
        let msg = if let Some(f) = feedback {
            format!("Command rejected. Feedback: {f}")
        } else {
            "Command rejected.".to_string()
        };
        self.push_block(
            AgentBlockKind::AgentMessage {
                text: msg,
                is_streaming: false,
            },
            conv_id,
        );
        self.state = ConversationState::Idle;
        sink.push(CoreEvent::ApprovalResolved {
            action_id,
            approved: false,
        });
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Notify the controller that a shell block completed (AC-6.1/6.2).
    /// If the block followed an approved RunCommand, appends an `ActionResult` block.
    pub fn on_block_completed(&mut self, context: &BlockContextSnapshot, sink: &mut dyn EventSink) {
        let Some(action_id) = self.approved_action_id.take() else {
            return;
        };
        let Some(cmd) = self.approved_command.take() else {
            return;
        };
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);

        let output_preview: String = context
            .output_text
            .lines()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");

        self.push_block(
            AgentBlockKind::ActionResult {
                action_id,
                command: cmd,
                exit_code: context.exit_code,
                output_preview,
            },
            conv_id,
        );
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Reset the conversation, clearing all agent blocks and session history.
    /// The old JSONL file is kept on disk; a new session starts fresh.
    pub fn clear_conversation(&mut self, sink: &mut dyn EventSink) {
        self.blocks.clear();
        self.active_conversation_id = None;
        self.state = ConversationState::Idle;
        self.approved_action_id = None;
        self.approved_command = None;
        self.checkpoints.clear();
        self.current_query_block_id = None;
        self.session_history.clear();
        self.pending_plans.clear();
        self.pending_tool_calls.clear();
        self.queued_approvals.clear();
        self.pending_failed_resume = None;
        self.pending_server_resume_task_id = None;
        self.message_queue.clear();
        // Drain stale completion notifications so cleared sessions don't auto-restart.
        if let Some(ref rx) = self.bg_completion_rx {
            while rx.try_recv().is_ok() {}
        }
        self.deferred_bg_summaries.clear();
        self.streaming_task_id = None;
        self.streaming_text.clear();
        self.reset_streaming_on_next_text = false;
        self.thinking_block_id = None;
        self.loading_block_id = None;
        self.tool_call_count = 0;
        self.turn_count = 0;
        self.agent_modified_files.clear();
        crate::session_facts::clear_conversation(self.pane_uuid);
        // Open a brand-new session file so resume picks up from here next time
        self.session_store = None;
        // The old conversation's SQLite rows are deliberately left in place —
        // `next_block_id` is never reset here, so future rows can't collide
        // with them — so it stays reachable through the session history
        // browser instead of being destroyed.
        self.deps.tools.clear_shell_state(self.pane_uuid);
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Stop the currently running agent stream. Safe to call at any time.
    /// The orphaned stream is silently discarded; the session history is NOT cleared
    /// so the user can continue the conversation from where it stopped.
    /// Close the still-open TurnGroup (if any), stamping its duration and an
    /// optional end note ("Stopped", "Failed"). Every terminal path — Done,
    /// user stop, server error, stream error — must come through here so the
    /// header can never keep saying "Working…" after work has ceased.
    pub(crate) fn close_open_turn(&mut self, note: Option<&str>) -> Option<usize> {
        let now_ms = unix_now_ms();
        let mut tg_id_to_save = None;
        for b in self.blocks.iter_mut().rev() {
            if let AgentBlockKind::TurnGroup {
                duration_ms,
                started_at_ms,
                status_note,
                ..
            } = &mut b.kind
            {
                if duration_ms.is_none() {
                    *duration_ms = Some(now_ms - *started_at_ms);
                    *status_note = note.map(str::to_string);
                    tg_id_to_save = Some(b.id);
                    break;
                }
            }
        }
        if let Some(id) = tg_id_to_save {
            self.update_block_in_db(id);
        }
        tg_id_to_save
    }

    pub fn cancel_stream(&mut self, sink: &mut dyn EventSink) -> Option<LaunchedTurn> {
        if matches!(self.state, ConversationState::Idle) {
            self.remove_loading_block();
            sink.push(CoreEvent::BlocksChanged);
            return None;
        }
        // Signal any in-flight tool threads to post a cancellation result immediately
        // so the server doesn't keep waiting for a tool that may run for minutes.
        self.tool_cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // Resolve calls waiting on an approval card as cancelled. Without a
        // result the remote task can remain parked forever after Stop.
        let task_id = self.streaming_task_id.clone().unwrap_or_default();
        // Desktop-owned tasks are coordinated by the local pipeline loop. Stop
        // that loop before resolving pending calls so no further provider step
        // can begin after the user has cancelled.
        let _ = self.deps.pipeline.cancel_task(&task_id);
        let pipeline = Arc::clone(&self.deps.pipeline);
        for pending in self.pending_tool_calls.values() {
            let call_id = pending.call_id.clone();
            let task_id = task_id.clone();
            let pipeline = Arc::clone(&pipeline);
            spawn_background(Box::new(move || {
                if let Err(e) = pipeline.post_tool_result(
                    &task_id,
                    &call_id,
                    "Tool cancelled by user before approval.",
                    true,
                ) {
                    log::warn!("[agent-stream] cancellation result {call_id}: {e}");
                }
            }));
        }
        // Stop locally tracked command/process groups associated with this
        // stream. Non-command tools simply return through the cancellation
        // flag; kill_command is a no-op for unknown call IDs.
        for call_id in self.active_tool_calls.iter() {
            let _ = self.deps.tools.kill_command(call_id);
        }
        // Fresh flag for the next turn — old Arc stays with the in-flight threads.
        self.tool_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.active_tool_calls.clear();
        self.completed_tool_calls.clear();

        self.stream_generation += 1;
        self.streaming_task_id = None;
        self.state = ConversationState::Idle;
        self.remove_loading_block();
        let _ = self.close_open_turn(Some("Stopped"));
        // Any open ask_user card is now backed by a dead task — mark it dismissed
        // so it stops looking interactive.
        self.dismiss_open_question();
        // Drop any in-progress tool approval so the UI doesn't show a stale card.
        self.pending_tool_calls.clear();
        self.queued_approvals.clear();
        sink.push(CoreEvent::BlocksChanged);
        // Per the user's choice: Stop aborts the current turn and immediately
        // starts the next queued prompt (if any). The queue is NOT cleared.
        self.maybe_start_next_queued(sink)
    }

    /// Poll-once background-completion pump. Drains ready completions (skipping
    /// probe messages with an empty task_id), then resumes from any summaries
    /// that couldn't start a turn when they arrived. The owner reschedules
    /// this for the lifetime of the controller; nothing here spawns timers.
    pub fn poll_background_once(&mut self, sink: &mut dyn EventSink) -> Option<LaunchedTurn> {
        let completions: Vec<BackgroundCompletion> = match self.bg_completion_rx.as_ref() {
            Some(rx) => {
                let mut out = Vec::new();
                while let Ok(c) = rx.try_recv() {
                    if c.task_id.is_empty() {
                        continue;
                    }
                    out.push(c);
                }
                out
            }
            None => Vec::new(),
        };
        let mut launched = None;
        for c in completions {
            if let Some(turn) = self.on_background_task_finished(c, sink) {
                launched = Some(turn);
            }
        }
        // Resume from any summaries that couldn't start a turn when they arrived
        // (agent was mid-turn). This is what unsticks a killed/finished task whose
        // completion landed during the narration window.
        if let Some(turn) = self.try_resume_deferred_background(sink) {
            launched = Some(turn);
        }
        launched
    }

    /// Drain queued background summaries into a single system continuation once
    /// the agent is Idle. No-op while a turn is running (would orphan it) or
    /// when there is no active session.
    pub(crate) fn try_resume_deferred_background(
        &mut self,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        if self.deferred_bg_summaries.is_empty()
            || self.active_conversation_id.is_none()
            || !matches!(self.state, ConversationState::Idle)
        {
            return None;
        }
        let note = std::mem::take(&mut self.deferred_bg_summaries).join("\n\n");
        self.start_system_continuation(
            note,
            self.last_model_name.clone(),
            Box::new({
                let sys = self.last_system_arc.clone();
                move || sys.lock().map(|g| g.clone()).unwrap_or_default()
            }),
            sink,
        )
    }

    pub(crate) fn on_background_task_finished(
        &mut self,
        c: BackgroundCompletion,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        if self.ingest_shell_checkpoint(&c.task_id) {
            sink.push(CoreEvent::BlocksChanged);
        }
        let exit_str = c
            .exit_code
            .map(|e| e.to_string())
            .unwrap_or_else(|| "?".to_string());
        let status = if c.exit_code == Some(0) {
            "successfully"
        } else {
            &format!("with exit code {exit_str}")
        };
        // Evidence gate for backgrounded verification commands (same rules as
        // foreground: only exit 0 clears unverified edits).
        let verify_note = match crate::session_facts::record_verification_result(
            self.pane_uuid,
            &c.command,
            c.exit_code,
        ) {
            crate::session_facts::VerificationOutcome::Failed => {
                "\n[system] Verification failed (non-zero exit). Edits remain unverified — \
                 fix the failure and re-run the check before claiming the task is done."
            }
            _ => "",
        };
        // Build the runtime event that will be attached to the provider's
        // one-turn system context once the controller is idle.
        let summary = format!(
            "[Background task {task_id} finished {status}]\nCommand: {cmd}\nOutput:\n{output}{verify_note}",
            task_id = c.task_id,
            status = status,
            cmd = c.command,
            output = if c.output.trim().is_empty() {
                "(no output)"
            } else {
                &c.output
            },
            verify_note = verify_note,
        );
        // Only auto-resume if we have an active session.
        if self.active_conversation_id.is_none() {
            return None;
        }
        // Enqueue unconditionally; the watcher (`try_resume_deferred_background`)
        // drains this into a system continuation as soon as the agent is Idle.
        // Enqueuing even when currently Idle keeps a single ordered path and lets
        // the watcher batch several completions that land close together — and,
        // critically, guarantees a resume for completions (including a KILLED
        // command's) that arrive mid-turn, which previously stranded in
        // `session_history` and left the agent stuck "waiting".
        self.deferred_bg_summaries.push(summary);
        self.try_resume_deferred_background(sink)
    }

    // ── Pure helpers (testable without a sink) ─────────────────────────────────

    /// Process an API response; returns events to emit. Mutates `self`.
    pub(crate) fn apply_response(
        &mut self,
        response: Result<AgentQueryResponse, AgentError>,
        query: &str,
        cwd: &std::path::Path,
    ) -> Vec<CoreEvent> {
        // `query`/`cwd` are kept for parity with the app's turn loop (which
        // file-logs the response); the engine emits events only.
        let _ = (query, cwd);
        match response {
            Err(e) => {
                log::warn!("[agent] query failed: {e}");
                self.state = ConversationState::Idle;
                let msg = "Something went wrong. Please try again.".to_string();
                let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
                self.push_block(
                    AgentBlockKind::Error {
                        message: msg.clone(),
                        kind: ErrorKind::Internal,
                        task_id: None,
                    },
                    conv_id,
                );
                vec![CoreEvent::Error { message: msg }, CoreEvent::BlocksChanged]
            }
            Ok(resp) => {
                let new_conv_id = resp.conversation_id;
                self.active_conversation_id = Some(new_conv_id);

                let completed_response = resp.clone();
                let mut pending_action_id: Option<Uuid> = None;
                let mut extra_events: Vec<CoreEvent> = Vec::new();
                let mut next_actions = resp.actions;

                // The mock-backend approval gate reads a default autonomy
                // profile with the host's effective approval mode and auto-run
                // flag (no global settings).
                let autonomy = AgentAutonomySettings {
                    auto_run_safe_commands: self.deps.config.auto_run_safe_commands,
                    ..AgentAutonomySettings::default()
                }
                .with_approval_mode(self.deps.config.approval_mode);

                // No arbitrary tool-round quota: useful long tasks may require
                // many steps. The unified runner applies novelty controls to
                // duplicate reads instead of terminating work at a fixed count.
                loop {
                    let mut tool_actions_to_run = Vec::new();
                    let mut has_run_command = false;

                    for action in &next_actions {
                        match &action.kind {
                            AgentActionKind::Message { text } => {
                                self.push_block(
                                    AgentBlockKind::AgentMessage {
                                        text: text.clone(),
                                        is_streaming: false,
                                    },
                                    new_conv_id,
                                );
                            }
                            // Legacy (mock backend) path: commands only execute
                            // through the view's Approve button, so the autonomy
                            // policy may escalate (Manual mode / auto-run off)
                            // but never clear the backend's approval flag —
                            // clearing it here would silently drop the command.
                            AgentActionKind::RunCommand { command, reason }
                                if action.requires_approval
                                    || crate::approval_policy::approval_required(
                                        &autonomy,
                                        "run_command",
                                        Some(command),
                                        false,
                                    ) =>
                            {
                                pending_action_id = Some(action.id);
                                self.push_block(
                                    AgentBlockKind::ApprovalRequest {
                                        action_id: action.id,
                                        command: command.clone(),
                                        reason: reason.clone(),
                                        status: ApprovalStatus::Pending,
                                    },
                                    new_conv_id,
                                );
                                has_run_command = true;
                                break;
                            }
                            AgentActionKind::ReadFiles { paths } => {
                                tool_actions_to_run.push(action.clone());
                            }
                            AgentActionKind::Grep { pattern, path } => {
                                tool_actions_to_run.push(action.clone());
                            }
                            AgentActionKind::SearchCodebase { query, repo_root } => {
                                tool_actions_to_run.push(action.clone());
                            }
                            AgentActionKind::Done => {}
                            _ => {}
                        }
                    }

                    if has_run_command {
                        break;
                    }

                    if tool_actions_to_run.is_empty() {
                        break;
                    }

                    // Execute tool actions and compile results
                    self.tool_round = self.tool_round.saturating_add(1);
                    self.state = ConversationState::ExecutingTool;
                    let mut tool_results = Vec::new();

                    for action in tool_actions_to_run {
                        let res_kind = match &action.kind {
                            AgentActionKind::ReadFiles { paths } => {
                                ToolResultKind::ReadFiles(self.deps.tools.run_read_files(paths))
                            }
                            AgentActionKind::Grep { pattern, path } => ToolResultKind::Grep(
                                self.deps.tools.run_grep(pattern, path.as_deref()),
                            ),
                            AgentActionKind::SearchCodebase { query, repo_root } => {
                                ToolResultKind::SearchCodebase(
                                    self.deps.tools.run_search_codebase(query, repo_root),
                                )
                            }
                            _ => unreachable!(),
                        };

                        extra_events.push(CoreEvent::Agent(AgentEvent::ToolExecuted {
                            result: res_kind.clone(),
                        }));

                        // Convert ToolResultKind back to scripted mock types/AgentAction for continue_conversation payload
                        let result_action = match res_kind {
                            ToolResultKind::ReadFiles(r) => {
                                let paths = r.files.iter().map(|f| f.path.clone()).collect();
                                AgentAction::read_files(paths)
                            }
                            ToolResultKind::Grep(r) => {
                                let pattern = r
                                    .matches
                                    .first()
                                    .map(|m| m.line_text.clone())
                                    .unwrap_or_default();
                                AgentAction::grep(pattern, None)
                            }
                            ToolResultKind::SearchCodebase(r) => {
                                let query = r
                                    .hits
                                    .first()
                                    .map(|h| h.preview.clone())
                                    .unwrap_or_default();
                                let repo_root = match &action.kind {
                                    AgentActionKind::SearchCodebase { repo_root, .. } => {
                                        repo_root.clone()
                                    }
                                    _ => PathBuf::from(""),
                                };
                                AgentAction::search_codebase(query, repo_root)
                            }
                            ToolResultKind::WriteFile(_)
                            | ToolResultKind::EditFile(_)
                            | ToolResultKind::ListDirectory(_) => {
                                AgentAction::message("File operation completed.")
                            }
                        };
                        tool_results.push(result_action);
                    }

                    // Build a dummy/minimal session context for continue_conversation
                    let dummy_session = AgentSessionContext {
                        cwd: cwd.to_path_buf(),
                        recent_blocks: Vec::new(),
                        is_autodetected: false,
                        project: None,
                        timestamp: chrono::Utc::now(),
                        os: std::env::consts::OS.to_string(),
                        shell: std::env::var("SHELL").unwrap_or_default(),
                        codebases: Vec::new(),
                        attached_blocks: Vec::new(),
                        skills: Vec::new(),
                    };

                    let session_ctx = self.active_session.as_ref().unwrap_or(&dummy_session);

                    match self.deps.api.continue_conversation(
                        new_conv_id,
                        tool_results,
                        session_ctx,
                    ) {
                        Ok(next_resp) => {
                            next_actions = next_resp.actions;
                        }
                        Err(e) => {
                            log::warn!("[agent] continue query failed: {e}");
                            extra_events.push(CoreEvent::Error {
                                message: e.to_string(),
                            });
                            break;
                        }
                    }
                }

                self.state = match pending_action_id {
                    Some(id) => ConversationState::AwaitingApproval { action_id: id },
                    None => ConversationState::Idle,
                };

                let mut events = vec![
                    CoreEvent::Agent(AgentEvent::QueryCompleted {
                        response: completed_response,
                    }),
                    CoreEvent::BlocksChanged,
                ];
                events.extend(extra_events);
                events
            }
        }
    }

    // ── Streaming agent (natural language → SSE pipeline) ─────────────────────

    /// Submit a natural language query to the streaming agent pipeline.
    /// Write/execute tools are gated until the user approves a plan.
    pub fn start_streaming_query(
        &mut self,
        query: String,
        attachments: Vec<MessageAttachment>,
        author: Option<AgentAuthor>,
        model_name: String,
        system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        let unattended = std::mem::take(&mut self.next_query_unattended);
        let service_writes = std::mem::take(&mut self.next_query_service_writes) && unattended;
        // An open ask_user question claims typed input: the user is ANSWERING
        // the question, not starting a new prompt. Route it to the card —
        // "1"/"2"-style replies select the matching option — so the agent
        // continues with the answer instead of the text becoming a queued
        // message that later fires as an out-of-context turn. An automation's
        // prompt is never an answer: it queues behind the question.
        if let ConversationState::AwaitingUserChoice { action_id } = self.state.clone() {
            if !unattended {
                self.answer_user_question_from_input(action_id, query, sink);
                return None;
            }
        }

        // Busy already? Queue this prompt instead of wiping the running turn.
        // It shows as a "Queued" pill and starts automatically when the current
        // turn finishes (see `maybe_start_next_queued`).
        if !matches!(self.state, ConversationState::Idle) {
            return self.enqueue_message(
                query,
                attachments,
                author,
                model_name,
                system_builder,
                unattended,
                service_writes,
                sink,
            );
        }
        self.unattended = unattended;
        self.unattended_service_writes = service_writes;

        // Continue is not a new user task. Preserve its execution facts and
        // checkpoint owner, and inject the durable journal as trusted runtime
        // context rather than displaying/storing a fake user message.
        if let Some(checkpoint) = self.pending_failed_resume.take() {
            self.pending_query_attachments.clear();
            self.pending_query_author = None;
            self.evidence_nudge_count = 0;
            let gate = self.gate_tools_until_plan;
            let server_task_id = self.pending_server_resume_task_id.take();
            return self.start_streaming_query_impl(
                query,
                model_name,
                system_builder,
                gate,
                QueryOrigin::System,
                Some(RuntimeEvent::ResumeCheckpoint {
                    checkpoint,
                    server_task_id,
                }),
                sink,
            );
        }

        self.pending_query_attachments = attachments;
        self.pending_query_author = author;
        // A user-TYPED query is a new task: reset edit/verification facts.
        // Relevant conversation evidence is selected and hash-validated later
        // on the background system-prompt builder, never on this UI path.
        // This lives HERE and not in the impl on purpose — plan approval also
        // uses QueryOrigin::User but must keep the reads gathered during the
        // plan's research stage, or the write-guard would block the very first
        // implementation writes.
        crate::session_facts::reset(self.pane_uuid);
        self.evidence_nudge_count = 0;

        // gate_tools comes from the chat mode (Plan gates; others don't). The
        // plan gate is not reset between turns, so approvals made earlier in the
        // session carry through to subsequent queries. Nobody can approve a
        // plan in an unattended turn.
        let gate = self.mode_gate && !unattended;
        self.start_streaming_query_impl(
            query,
            model_name,
            system_builder,
            gate,
            QueryOrigin::User,
            None,
            sink,
        )
    }

    /// Cap on queued-but-not-started prompts, so a runaway loop can't grow the
    /// queue without bound. Beyond this, new submissions are dropped with a note.
    const MAX_QUEUED_MESSAGES: usize = 12;

    /// Append a prompt to the FIFO queue and show its "Queued" pill.
    pub(crate) fn enqueue_message(
        &mut self,
        query: String,
        attachments: Vec<MessageAttachment>,
        author: Option<AgentAuthor>,
        model_name: String,
        system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
        unattended: bool,
        service_writes: bool,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        if self.message_queue.len() >= Self::MAX_QUEUED_MESSAGES {
            log::warn!(
                "[agent] message queue full ({}) — dropping submission",
                Self::MAX_QUEUED_MESSAGES
            );
            let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
            self.push_block(
                AgentBlockKind::Error {
                    message: "Too many queued messages — wait for some to finish.".to_string(),
                    kind: ErrorKind::Busy,
                    task_id: None,
                },
                conv_id,
            );
            sink.push(CoreEvent::BlocksChanged);
            return None;
        }
        let queue_id = Uuid::new_v4();
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
        let block_id = self.push_block(
            AgentBlockKind::QueuedMessage {
                queue_id,
                text: query.clone(),
            },
            conv_id,
        );
        self.message_queue.push_back(QueuedMessage {
            queue_id,
            block_id,
            query,
            author,
            attachments,
            model_name,
            system_builder,
            unattended,
            service_writes,
        });
        sink.push(CoreEvent::BlocksChanged);
        None
    }

    /// If the agent is idle and a prompt is queued, start the next one. Called
    /// at every point a turn finishes (done, error, stop) so the queue drains.
    pub fn maybe_start_next_queued(&mut self, sink: &mut dyn EventSink) -> Option<LaunchedTurn> {
        if !matches!(self.state, ConversationState::Idle) {
            return None;
        }
        let Some(next) = self.message_queue.pop_front() else {
            return None;
        };
        // Drop its "Queued" pill; start_streaming_query_impl pushes the real
        // UserQuery block, so the message stays visible as a normal turn.
        self.remove_block_by_id(next.block_id);
        self.pending_query_attachments = next.attachments;
        self.pending_query_author = next.author;
        crate::session_facts::reset(self.pane_uuid);
        self.evidence_nudge_count = 0;
        self.unattended = next.unattended;
        self.unattended_service_writes = next.service_writes;
        let gate = self.mode_gate && !next.unattended;
        self.start_streaming_query_impl(
            next.query,
            next.model_name,
            next.system_builder,
            gate,
            QueryOrigin::User,
            None,
            sink,
        )
        // The started turn's blocks were appended at the bottom; keep any
        // still-queued pills below it (they're the future, not the present).
        // (pin_queued_blocks_to_bottom runs inside the impl's turn setup.)
    }

    /// Move all QueuedMessage pills to the end of the transcript so they always
    /// sit below the active turn.
    pub(crate) fn pin_queued_blocks_to_bottom(&mut self) {
        if !self
            .blocks
            .iter()
            .any(|b| matches!(b.kind, AgentBlockKind::QueuedMessage { .. }))
        {
            return;
        }
        let mut queued: Vec<AgentBlock> = Vec::new();
        self.blocks.retain(|b| {
            if matches!(b.kind, AgentBlockKind::QueuedMessage { .. }) {
                queued.push(b.clone());
                false
            } else {
                true
            }
        });
        self.blocks.extend(queued);
    }

    /// Remove a still-queued prompt before it runs (the × on its pill).
    pub fn cancel_queued_message(&mut self, queue_id: Uuid, sink: &mut dyn EventSink) {
        let before = self.message_queue.len();
        self.message_queue.retain(|m| m.queue_id != queue_id);
        if self.message_queue.len() == before {
            return; // already started or unknown
        }
        // QueuedMessage blocks are never persisted, so an in-memory drop is enough.
        self.blocks.retain(|b| {
            !matches!(
                &b.kind,
                AgentBlockKind::QueuedMessage { queue_id: q, .. } if *q == queue_id
            )
        });
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Number of prompts currently waiting in the queue (for the UI).
    pub fn queued_message_count(&self) -> usize {
        self.message_queue.len()
    }

    /// Select an interrupted turn for a first-class continuation. Complete chat
    /// history cannot represent a failed turn's tool evidence, so rebuild it
    /// from the richer block journal that is also persisted to SQLite.
    pub fn prepare_failed_turn_resume(&mut self, error_block_id: Option<usize>) -> bool {
        let error_pos = error_block_id
            .and_then(|id| {
                self.blocks.iter().position(|block| {
                    block.id == id && matches!(block.kind, AgentBlockKind::Error { .. })
                })
            })
            .or_else(|| {
                self.blocks
                    .iter()
                    .rposition(|block| matches!(block.kind, AgentBlockKind::Error { .. }))
            });
        let Some(error_pos) = error_pos else {
            return false;
        };

        let turn_pos = self.blocks[..error_pos].iter().rposition(|block| {
            matches!(
                &block.kind,
                AgentBlockKind::TurnGroup {
                    status_note: Some(note),
                    ..
                } if note == "Failed" || note == "Stopped"
            )
        });
        let Some(turn_pos) = turn_pos else {
            return false;
        };
        let objective_pos = self.blocks[..=turn_pos]
            .iter()
            .rposition(|block| matches!(block.kind, AgentBlockKind::UserQuery { .. }));
        let Some(objective_pos) = objective_pos else {
            return false;
        };

        // A grant may have been persisted earlier in this task or by another
        // window, so disk is authoritative at the continuation boundary.
        self.permission_store = crate::permission_store::PermissionStore::load();

        let objective = match &self.blocks[objective_pos].kind {
            AgentBlockKind::UserQuery { text, .. } => text.as_str(),
            _ => unreachable!(),
        };
        let error_message = match &self.blocks[error_pos].kind {
            AgentBlockKind::Error { message, .. } => message.as_str(),
            _ => unreachable!(),
        };
        self.pending_server_resume_task_id = match &self.blocks[error_pos].kind {
            AgentBlockKind::Error { task_id, .. } => task_id.clone(),
            _ => None,
        };

        let mut checkpoint = String::from(
            "## Interrupted task execution checkpoint\n\n\
This is authoritative runtime state recovered from Dragent's durable execution journal. \
Resume the same task from the last successful step. Do not ask the user to repeat \
permissions already listed below, do not claim completed reads/actions were unavailable, \
and do not repeat completed commands or mutations unless their recorded result proves a retry is required.\n\n",
        );
        push_checkpoint_section(&mut checkpoint, "Original user objective", objective, 6_000);

        let mut grants: Vec<String> = self
            .permission_store
            .always_allow_read_roots
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        grants.sort();
        let grants = if grants.is_empty() {
            "No permanent external read roots are currently granted.".to_string()
        } else {
            grants
                .into_iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        push_checkpoint_section(
            &mut checkpoint,
            "Currently effective permanent read grants",
            &grants,
            4_000,
        );

        checkpoint.push_str("\n### Completed work and evidence\n");
        let mut entries = 0usize;
        for block in &self.blocks[objective_pos + 1..error_pos] {
            append_resume_block(&mut checkpoint, block, &mut entries);
            if checkpoint.len() >= 24_000 || entries >= 64 {
                checkpoint.push_str(
                    "- Additional older journal entries omitted to keep the resume context bounded.\n",
                );
                break;
            }
        }
        push_checkpoint_section(&mut checkpoint, "Interruption", error_message, 1_000);
        checkpoint.push_str(
            "\nContinue toward the original objective using the recorded evidence. First decide the \
next unresolved step; do not restart discovery from the beginning. If a recorded fact may have \
changed, verify only that fact.\n",
        );

        log::info!(
            "[agent-resume] prepared checkpoint for error block {} ({} chars, {} journal entries)",
            self.blocks[error_pos].id,
            checkpoint.len(),
            entries
        );
        self.pending_failed_resume = Some(checkpoint);
        true
    }
}

impl AgentController {
    /// Action id of the approval request currently shown as Pending, if any.
    /// Used by the env-gated automation bridge to resolve approvals headlessly.
    pub fn frontmost_pending_approval(&self) -> Option<Uuid> {
        self.blocks.iter().find_map(|b| match &b.kind {
            AgentBlockKind::ApprovalRequest {
                action_id,
                status: ApprovalStatus::Pending,
                ..
            } if self.pending_tool_calls.contains_key(action_id) => Some(*action_id),
            _ => None,
        })
    }

    /// Action ids of still-open approval requests, excluding `resolved`.
    pub fn open_approval_ids(&self, resolved: &Uuid) -> Vec<Uuid> {
        let mut ids: Vec<_> = self
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                AgentBlockKind::ApprovalRequest {
                    action_id,
                    status: ApprovalStatus::Pending,
                    ..
                } if action_id != resolved && self.pending_tool_calls.contains_key(action_id) => {
                    Some(*action_id)
                }
                _ => None,
            })
            .collect();
        ids.extend(self.queued_approvals.iter().filter_map(|approval| {
            (approval.action_id != *resolved
                && self.pending_tool_calls.contains_key(&approval.action_id))
            .then_some(approval.action_id)
        }));
        ids
    }

    pub fn resolve_streaming_approval(
        &mut self,
        action_id: Uuid,
        granted: bool,
        feedback: Option<String>,
        sink: &mut dyn EventSink,
    ) {
        // Only the visible request may be resolved; queued calls have no card yet.
        // "Allow always" moves its card into the activity group just before
        // calling this method, so checking the top-level block would reject it.
        if self
            .queued_approvals
            .iter()
            .any(|approval| approval.action_id == action_id)
        {
            return;
        }
        let Some(pending) = self.pending_tool_calls.remove(&action_id) else {
            return;
        };
        let Some(task_id) = self.streaming_task_id.clone() else {
            return;
        };

        // Update the approval block in the UI immediately.
        let was_approved_always = self.blocks.iter().any(|block| {
            matches!(
                &block.kind,
                AgentBlockKind::ApprovalRequest {
                    action_id: id,
                    status: ApprovalStatus::ApprovedAlways,
                    ..
                } if *id == action_id
            )
        });
        let status = if granted && was_approved_always {
            ApprovalStatus::ApprovedAlways
        } else if granted {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Rejected
        };
        self.set_approval_status(action_id, status);
        let next_consent = self.show_next_approval();
        if next_consent.is_none() {
            self.state = ConversationState::Streaming;
        }
        sink.push(CoreEvent::ApprovalResolved {
            action_id,
            approved: granted,
        });
        sink.push(CoreEvent::BlocksChanged);
        if let Some(event) = next_consent {
            sink.push(event);
        }

        let call_id = pending.call_id;
        let tool_name = pending.tool_name;
        let args_json = pending.args_json;
        let external_read_roots = pending.external_read_roots;

        if granted && tool_name == "run_command" {
            self.register_shell_checkpoint(&call_id, &args_json);
        } else if granted && is_checkpoint_write_tool(&tool_name) {
            self.capture_write_checkpoint(&tool_name, &args_json);
        }

        if granted && !external_read_roots.is_empty() {
            self.deps.tools.grant_external_read_once(&call_id, external_read_roots);
        }
        // Approval controls permission, never presentation. Agent commands use
        // the isolated executor even for command-intent turns; borrowing the
        // user's PTY can change its cwd, consume input or enter an alternate screen.

        let pane_uuid = self.pane_uuid;
        let recovery_tx = self.recovery_tx.clone();
        let tools = Arc::clone(&self.deps.tools);
        let pipeline = Arc::clone(&self.deps.pipeline);
        spawn_background(Box::new(move || {
            let (output, is_error) = if granted {
                tools.execute_with_call_id(&call_id, &tool_name, &args_json, pane_uuid)
            } else {
                let msg = if let Some(ref f) = feedback {
                    format!("Operation rejected by user. Feedback: {f}")
                } else {
                    "Operation rejected by user.".to_string()
                };
                (msg, true)
            };
            match recovery_tx {
                // Restart-recovered run: no live stream to post into. Commit
                // the result to the run's checkpoint; the model resumes on
                // this channel once every parked call of the round resolved.
                Some(tx) => {
                    if let Err(error) = pipeline.launch_recovered_tool(
                        &task_id,
                        &call_id,
                        &output,
                        is_error,
                        tx.clone(),
                    ) {
                        let _ = tx.send(Err(resume_error_to_stream_error(error)));
                    }
                }
                None => {
                    if let Err(e) = pipeline.post_tool_result(&task_id, &call_id, &output, is_error)
                    {
                        log::warn!("[agent-stream] post_tool_result {call_id}: {e}");
                    }
                }
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::super::core::test_stubs::test_deps;
    use super::*;

    fn engine() -> AgentController {
        AgentController::new(test_deps())
    }

    #[test]
    fn command_approval_never_hands_the_visible_terminal_to_the_agent() {
        use crate::runtime::{PipelineLauncher, StreamLaunchRequest, StreamSender};
        struct Reply(std::sync::mpsc::Sender<(String, bool)>);
        impl PipelineLauncher for Reply {
            fn launch_stream(&self, _: StreamLaunchRequest, _: StreamSender) {}
            fn post_tool_result(&self, _: &str, _: &str, output: &str, error: bool) -> Result<(), crate::pipeline::StreamError> {
                self.0.send((output.into(), error)).unwrap(); Ok(())
            }
            fn cancel_task(&self, _: &str) -> bool { false }
            fn register_host_tools(&self, _: &str, _: Vec<crate::pipeline::HostToolDef>) {}
            fn launch_recovered_run(&self, _: &str, _: StreamSender) -> Result<(), crate::pipeline::ResumeTaskError> { Ok(()) }
            fn launch_recovered_tool(&self, _: &str, _: &str, _: &str, _: bool, _: StreamSender) -> Result<(), crate::pipeline::ResumeTaskError> { Ok(()) }
        }
        for granted in [true, false] {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut deps = test_deps();
            deps.pipeline = Arc::new(Reply(tx));
            let mut c = AgentController::new(deps);
            c.streaming_task_id = Some("turn".into());
            c.force_command_approval = true;
            let action_id = Uuid::new_v4();
            c.pending_tool_calls.insert(action_id, crate::types::PendingToolCall {
                call_id: "command".into(), tool_name: "run_command".into(),
                args_json: r#"{"command":"printf hello"}"#.into(),
                // Even an old pending record must use the isolated executor.
                run_in_terminal: true, allow_terminal_cwd_change: true,
                external_read_roots: Vec::new(),
            });
            let mut sink = crate::runtime::VecSink::default();
            c.resolve_streaming_approval(action_id, granted, None, &mut sink);
            let (_, error) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert_eq!(error, !granted);
            assert!(!sink.events.iter().any(|event| matches!(event, CoreEvent::RunCommandInTerminal { .. })));
            assert!(!c.pending_tool_calls.contains_key(&action_id));
        }
    }

    #[test]
    fn is_busy_covers_the_pre_task_id_window() {
        let mut c = engine();
        assert!(!c.is_busy(), "idle must not be busy");
        assert!(!c.is_streaming());
        c.state = ConversationState::ProcessingQuery;
        assert!(
            c.is_busy(),
            "a submitted turn must be stoppable before the task id arrives"
        );
        assert!(!c.is_streaming(), "no task id yet");
        for state in [
            ConversationState::Streaming,
            ConversationState::ExecutingTool,
        ] {
            c.state = state;
            assert!(c.is_busy());
        }
        c.state = ConversationState::Idle;
        assert!(!c.is_busy());
    }

    #[test]
    fn stop_button_and_cancel_stream_agree_on_which_states_are_abortable() {
        let mut c = engine();
        for state in [
            ConversationState::Idle,
            ConversationState::ProcessingQuery,
            ConversationState::Streaming,
            ConversationState::ExecutingTool,
        ] {
            let is_idle = matches!(state, ConversationState::Idle);
            c.state = state.clone();
            assert_eq!(
                c.is_busy(),
                !is_idle,
                "button state and cancel_stream's early-return guard disagree for {state:?}"
            );
        }
    }

    fn open_turns(c: &AgentController) -> Vec<&AgentBlock> {
        c.blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.kind,
                    AgentBlockKind::TurnGroup {
                        duration_ms: None,
                        ..
                    }
                )
            })
            .collect()
    }

    #[test]
    fn a_terminal_command_splits_the_turn_around_its_block() {
        let mut c = engine();
        let conv = Uuid::new_v4();
        c.active_conversation_id = Some(conv);
        c.state = ConversationState::Streaming;
        c.open_turn_blocks(conv);
        c.push_turn_block(
            AgentBlockKind::AgentMessage {
                text: "Running the tests.".into(),
                is_streaming: true,
            },
            true,
            conv,
        );
        let first = c.blocks.last().unwrap().id;
        let mut sink = crate::runtime::VecSink::default();

        c.hand_turn_to_terminal(&mut sink);
        assert!(
            open_turns(&c).is_empty(),
            "the block above the command stops working"
        );
        let AgentBlockKind::TurnGroup { children, .. } = &c.blocks.last().unwrap().kind else {
            panic!("expected the turn block");
        };
        assert!(children.iter().all(|child| match &child.kind {
            AgentBlockKind::AgentMessage { is_streaming, .. } => !is_streaming,
            AgentBlockKind::Loading { .. } => false,
            _ => true,
        }));

        // The command's shell block is created now; the agent's next block after it.
        let command_created = unix_now_ms();
        c.continue_turn_below(&mut sink);
        let open = open_turns(&c);
        assert_eq!(open.len(), 1);
        assert_ne!(open[0].id, first);
        assert!(open[0].created_at_unix_ms >= command_created);
        let AgentBlockKind::TurnGroup { children, .. } = &open[0].kind else {
            unreachable!()
        };
        assert!(matches!(
            children.as_slice(),
            [AgentBlock {
                kind: AgentBlockKind::Loading { .. },
                ..
            }]
        ));
    }

    #[test]
    fn the_turn_carries_on_under_an_answered_question() {
        let mut c = engine();
        let conv = Uuid::new_v4();
        c.active_conversation_id = Some(conv);
        c.state = ConversationState::Streaming;
        c.streaming_task_id = Some("task".into());
        c.open_turn_blocks(conv);
        let first = c.blocks.last().unwrap().id;
        c.begin_user_question(
            "call".into(),
            r#"{"questions":[{"question":"Which database?","options":["Postgres","SQLite"]}]}"#,
            conv,
        );
        assert!(
            open_turns(&c).is_empty(),
            "the block above the card stops working"
        );
        let card = c.blocks.last().unwrap();
        let AgentBlockKind::UserQuestion { action_id, .. } = card.kind else {
            panic!("expected the question card last");
        };
        let card_created = card.created_at_unix_ms;

        let mut sink = crate::runtime::VecSink::default();
        c.pick_user_question_option(action_id, "SQLite".into(), &mut sink);
        let open = open_turns(&c);
        assert_eq!(open.len(), 1);
        assert_ne!(open[0].id, first);
        assert!(open[0].created_at_unix_ms >= card_created);
        assert!(matches!(
            c.blocks.last().unwrap().kind,
            AgentBlockKind::TurnGroup {
                duration_ms: None,
                ..
            }
        ));
    }

    #[test]
    fn a_turn_stopped_during_its_terminal_command_stays_stopped() {
        let mut c = engine();
        let conv = Uuid::new_v4();
        c.active_conversation_id = Some(conv);
        c.state = ConversationState::Streaming;
        c.open_turn_blocks(conv);
        let mut sink = crate::runtime::VecSink::default();
        c.hand_turn_to_terminal(&mut sink);
        c.state = ConversationState::Idle;
        c.continue_turn_below(&mut sink);
        assert!(open_turns(&c).is_empty());
        assert!(
            !c.blocks
                .iter()
                .any(|b| matches!(b.kind, AgentBlockKind::Loading { .. }))
        );
    }

    #[test]
    fn queued_pills_pinned_below_active_turn() {
        let mut c = engine();
        let conv = Uuid::new_v4();
        c.push_block(
            AgentBlockKind::QueuedMessage {
                queue_id: Uuid::new_v4(),
                text: "first".into(),
            },
            conv,
        );
        c.push_block(
            AgentBlockKind::UserQuery {
                text: "running".into(),
                author: None,
                attached_blocks: Vec::new(),
                attachments: Vec::new(),
                checkpoint: None,
            },
            conv,
        );
        c.push_block(
            AgentBlockKind::QueuedMessage {
                queue_id: Uuid::new_v4(),
                text: "second".into(),
            },
            conv,
        );
        c.pin_queued_blocks_to_bottom();
        let kinds: Vec<&AgentBlockKind> = c.blocks.iter().map(|b| &b.kind).collect();
        let n = kinds.len();
        assert!(
            matches!(kinds[n - 2], AgentBlockKind::QueuedMessage { text, .. } if text == "first")
        );
        assert!(
            matches!(kinds[n - 1], AgentBlockKind::QueuedMessage { text, .. } if text == "second")
        );
        assert!(
            kinds[..n - 2]
                .iter()
                .any(|k| matches!(k, AgentBlockKind::UserQuery { .. }))
        );
    }

    /// Records the tool results the controller posts.
    #[derive(Default)]
    struct RecordingPipeline {
        posted: std::sync::Mutex<Vec<(String, String, bool)>>,
    }

    impl crate::runtime::PipelineLauncher for RecordingPipeline {
        fn launch_stream(
            &self,
            _request: crate::runtime::StreamLaunchRequest,
            _tx: crate::runtime::StreamSender,
        ) {
        }
        fn post_tool_result(
            &self,
            _task_id: &str,
            call_id: &str,
            output: &str,
            is_error: bool,
        ) -> Result<(), crate::pipeline::StreamError> {
            self.posted
                .lock()
                .unwrap()
                .push((call_id.to_string(), output.to_string(), is_error));
            Ok(())
        }
        fn cancel_task(&self, _task_id: &str) -> bool {
            false
        }
        fn register_host_tools(&self, _run_id: &str, _defs: Vec<crate::pipeline::HostToolDef>) {}
        fn launch_recovered_run(
            &self,
            _run_id: &str,
            _tx: crate::runtime::StreamSender,
        ) -> Result<(), crate::pipeline::ResumeTaskError> {
            Ok(())
        }
        fn launch_recovered_tool(
            &self,
            _task_id: &str,
            _call_id: &str,
            _output: &str,
            _is_error: bool,
            _tx: crate::runtime::StreamSender,
        ) -> Result<(), crate::pipeline::ResumeTaskError> {
            Ok(())
        }
    }

    /// A streaming controller whose posted tool results land in the
    /// returned pipeline.
    fn streaming_engine(unattended: bool) -> (AgentController, Arc<RecordingPipeline>, Uuid) {
        let pipeline = Arc::new(RecordingPipeline::default());
        let mut deps = test_deps();
        deps.pipeline = pipeline.clone();
        let mut c = AgentController::new(deps);
        let conv = Uuid::new_v4();
        c.active_conversation_id = Some(conv);
        c.state = ConversationState::Streaming;
        c.streaming_task_id = Some("task".into());
        c.unattended = unattended;
        c.open_turn_blocks(conv);
        (c, pipeline, conv)
    }

    fn tool_call(call_id: &str, name: &str, args: &str) -> crate::pipeline::PipelineEvent {
        serde_json::from_value(serde_json::json!({
            "kind": "agent.tool_call",
            "tool_id": call_id,
            "tool_name": name,
            "tool_args": args,
        }))
        .unwrap()
    }

    /// Posted results arrive from a background thread.
    fn posted(pipeline: &RecordingPipeline) -> Vec<(String, String, bool)> {
        for _ in 0..200 {
            let posted = pipeline.posted.lock().unwrap().clone();
            if !posted.is_empty() {
                return posted;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Vec::new()
    }

    const QUESTION: &str =
        r#"{"questions":[{"question":"Which database?","options":["Postgres","SQLite"]}]}"#;

    #[test]
    fn an_unattended_turn_answers_its_own_questions() {
        let (mut c, pipeline, conv) = streaming_engine(true);
        c.apply_tool_call_event(tool_call("ask", "ask_user", QUESTION), conv);
        assert!(!matches!(
            c.state,
            ConversationState::AwaitingUserChoice { .. }
        ));
        assert!(
            !c.blocks
                .iter()
                .any(|b| matches!(b.kind, AgentBlockKind::UserQuestion { .. })),
            "no card waits for an answer"
        );
        let posted = posted(&pipeline);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].0, "ask");
        assert!(posted[0].1.contains("Choose the most sensible option"));
        assert!(!posted[0].2, "an answer, not an error");

        // Attended, the same call opens the card.
        let (mut c, _, conv) = streaming_engine(false);
        c.apply_tool_call_event(tool_call("ask", "ask_user", QUESTION), conv);
        assert!(matches!(
            c.state,
            ConversationState::AwaitingUserChoice { .. }
        ));
    }

    #[test]
    fn an_unattended_turn_refuses_what_needs_approval() {
        let consent: crate::types::ToolPolicy =
            Arc::new(
                |_| crate::pipeline::WorkspaceToolDisposition::NeedsConsent {
                    reason: "writes outside the task".into(),
                },
            );
        let (mut c, pipeline, conv) = streaming_engine(true);
        c.set_tool_policy(Some(consent.clone()));
        c.apply_tool_call_event(
            tool_call("w", "write_file", r#"{"path":"a.txt","content":"x"}"#),
            conv,
        );
        assert!(!matches!(
            c.state,
            ConversationState::AwaitingApproval { .. }
        ));
        assert!(
            !c.blocks
                .iter()
                .any(|b| matches!(b.kind, AgentBlockKind::ApprovalRequest { .. }))
        );
        let posted = posted(&pipeline);
        assert_eq!(posted.len(), 1);
        assert!(posted[0].1.contains("writes outside the task"));
        assert!(posted[0].2, "refused as an error");

        let (mut c, _, conv) = streaming_engine(false);
        c.set_tool_policy(Some(consent));
        c.apply_tool_call_event(
            tool_call("w", "write_file", r#"{"path":"a.txt","content":"x"}"#),
            conv,
        );
        assert!(matches!(
            c.state,
            ConversationState::AwaitingApproval { .. }
        ));
    }

    #[test]
    fn an_automation_only_posts_to_services_when_allowed() {
        let comment = r#"{"service":"linear","id":"ENG-1","text":"Fixed"}"#;
        let (mut c, pipeline, conv) = streaming_engine(true);
        c.apply_tool_call_event(tool_call("c", "integrations_comment", comment), conv);
        let refused = posted(&pipeline);
        assert_eq!(refused.len(), 1);
        assert!(refused[0].1.contains("can only read from connected services"));
        assert!(refused[0].2, "refused as an error");

        // Allowed: it isn't refused (it runs through the tools instead).
        let (mut c, pipeline, conv) = streaming_engine(true);
        c.unattended_service_writes = true;
        c.apply_tool_call_event(tool_call("c", "integrations_comment", comment), conv);
        assert!(!posted(&pipeline)
            .iter()
            .any(|(_, text, _)| text.contains("can only read")));

        // A person is asked first.
        let (mut c, _, conv) = streaming_engine(false);
        c.apply_tool_call_event(tool_call("c", "integrations_comment", comment), conv);
        assert!(matches!(
            c.state,
            ConversationState::AwaitingApproval { .. }
        ));
    }

    #[test]
    fn an_automation_prompt_keeps_its_unattended_turn_through_the_queue() {
        let mut c = engine();
        c.state = ConversationState::Streaming;
        let mut sink = crate::runtime::VecSink::default();
        c.set_next_query_unattended(true);
        c.start_streaming_query(
            "nightly check".into(),
            Vec::new(),
            None,
            String::new(),
            Box::new(String::new),
            &mut sink,
        );
        c.start_streaming_query(
            "typed by the user".into(),
            Vec::new(),
            None,
            String::new(),
            Box::new(String::new),
            &mut sink,
        );
        let queued: Vec<bool> = c.message_queue.iter().map(|m| m.unattended).collect();
        assert_eq!(queued, vec![true, false]);
        assert!(c.message_queue.iter().all(|m| !m.service_writes));
        assert!(!c.next_query_unattended, "the flag is for one query");
        assert!(!c.is_unattended(), "the running turn is the user's");

        c.state = ConversationState::Idle;
        c.maybe_start_next_queued(&mut sink);
        assert!(c.is_unattended(), "the automation's turn starts unattended");
        assert!(!c.gate_tools_until_plan);
    }
}
