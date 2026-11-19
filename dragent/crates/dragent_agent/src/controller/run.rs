//! Streaming runs: continuations, recovery, status, and checkpoints.
//!
//! Blocks here are impl AgentController: same type, same behavior.
//!
//! Moved verbatim from `app/src/agent/controller/run.rs` apart from the
//! engine seams: `ctx.emit` → `sink.push`, `ctx.notify` dropped,
//! `crate::bg::detach` → `crate::runtime::spawn_background`,
//! `pipeline::register_host_tools` → `deps.pipeline`, local-tools calls →
//! `deps.tools`, `dream_consolidator::on_session_start` → `deps.hooks`,
//! `SessionStore` → `deps.transcripts`, and WarpUI (`ModelContext`, `Timer`)
//! dropped. Launch methods return `Option<LaunchedTurn>` instead of
//! spawning a poll timer: the owner pumps the returned receiver. The
//! background thread makes a single `deps.pipeline.launch_stream` call;
//! fresh and resume turns differ only in `request.resume`.

use std::sync::{Arc, mpsc};

use uuid::Uuid;

use super::AgentRole;
use super::event_helpers::unix_now_ms;
use super::session_helpers::{
    MAX_SESSION_TURNS, QueryOrigin, RuntimeEvent, append_runtime_event, estimate_tokens,
    resume_error_to_stream_error, tool_summary_for_approval,
};
use crate::block::{AgentAuthor, AgentBlock, AgentBlockKind};
use crate::pipeline::{PipelineEvent, StreamError};
use crate::runtime::{CoreEvent, EventSink, StreamLaunchRequest, spawn_background};
use crate::state::ConversationState;
use crate::types::{PendingToolCall, RecoveredParkedCall, SessionMessage};

/// An opened provider-stream turn: the pump receiver plus the identity the
/// owner needs to drive it (`poll_stream` equivalent).
pub struct LaunchedTurn {
    pub conv_id: Uuid,
    pub rx: mpsc::Receiver<Result<PipelineEvent, StreamError>>,
    pub generation: u64,
    pub started_at: web_time::Instant,
}

impl super::core::AgentController {
    /// Remove a top-level block by id. QueuedMessage/Loading are never persisted,
    /// so this is a memory-only drop.
    pub(crate) fn remove_block_by_id(&mut self, block_id: usize) {
        self.blocks.retain(|b| b.id != block_id);
    }

    /// Resume the agent with a SYSTEM continuation (a finished background
    /// command's output, an auto-resume note). The output is supplied to the
    /// provider as one-turn system context, never as user-authored content.
    pub fn start_system_continuation(
        &mut self,
        note: String,
        model_name: String,
        system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        // A forced approval belongs only to the explicit command request that
        // started the turn. The agent's follow-up work must use the selected
        // autonomy mode normally (including Auto).
        self.force_command_approval = false;
        self.allow_terminal_cwd_change = false;
        self.start_streaming_query_impl(
            "A background task completed. Continue the existing task using the runtime event in \
             the system instructions."
                .to_string(),
            model_name,
            system_builder,
            false,
            QueryOrigin::System,
            Some(RuntimeEvent::BackgroundTask(note)),
            sink,
        )
    }

    /// `start_streaming_query` for hosts whose wire query is a prompt scaffold
    /// around the user's words: `display_text` is what the user block (and
    /// the JSONL transcript) shows; `query` is what the model receives.
    #[allow(clippy::too_many_arguments)]
    pub fn start_streaming_query_with_display(
        &mut self,
        query: String,
        display_text: String,
        attachments: Vec<crate::attachments::MessageAttachment>,
        author: Option<AgentAuthor>,
        model_name: String,
        system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        self.pending_query_display = Some(display_text);
        self.start_streaming_query(query, attachments, author, model_name, system_builder, sink)
    }

    /// A host-driven follow-up pass in the SAME conversation whose prompt is
    /// internal plumbing, not user intent: a structured-output repair
    /// instruction, a cut-off-JSON continuation, a model-route fallback. It
    /// is sent as this turn's query but — like a background-task
    /// continuation — never shown as a user block nor recorded as a user
    /// transcript entry, so the visible conversation stays what the user
    /// actually typed. `note`, when present, is the status text shown in the
    /// turn's activity indicator.
    pub fn start_internal_continuation(
        &mut self,
        query: String,
        note: Option<String>,
        model_name: String,
        system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        self.force_command_approval = false;
        self.allow_terminal_cwd_change = false;
        let launched = self.start_streaming_query_impl(
            query,
            model_name,
            system_builder,
            false,
            QueryOrigin::System,
            None,
            sink,
        );
        if let Some(note) = note {
            self.set_loading_message(&note);
            sink.push(CoreEvent::BlocksChanged);
        }
        launched
    }

    /// Push the TurnGroup wrapper for all agent work this turn, plus the
    /// transient Loading block so the user sees immediate feedback.
    pub(crate) fn open_turn_blocks(&mut self, conv_id: Uuid) {
        let tg_id = self.next_block_id;
        self.next_block_id += 1;
        let turn_started_ms = unix_now_ms();
        self.blocks.push(AgentBlock::new(
            tg_id,
            conv_id,
            AgentBlockKind::TurnGroup {
                children: Vec::new(),
                expanded: true,
                started_at_ms: turn_started_ms,
                duration_ms: None,
                status_note: None,
            },
        ));
        self.save_block_in_db(
            tg_id,
            AgentBlockKind::TurnGroup {
                children: Vec::new(),
                expanded: true,
                started_at_ms: turn_started_ms,
                duration_ms: None,
                status_note: None,
            },
            conv_id,
        );
        let loading_id = self.push_block(
            AgentBlockKind::Loading {
                message: "Working…".to_string(),
            },
            conv_id,
        );
        self.loading_block_id = Some(loading_id);
    }

    /// Continue a local run this controller (or its predecessor before an app
    /// restart) was driving, from its durable checkpoint. Used by embedded
    /// hosts after `set_session_history`/`restore_blocks_from_sql` so the
    /// transcript is already on screen: no new user block is shown.
    ///
    /// `parked` are the tool calls the run was waiting on for consent when
    /// it stopped; each gets its approval card back (and a `ConsentRequested`
    /// event for the host's Attention mirror). With none, the model resumes
    /// immediately (returned as a `LaunchedTurn` for the owner to pump);
    /// parked approvals resolve through the normal approval flow in `submit`,
    /// so this returns `None` and keeps the channel in `recovery_tx`.
    pub fn recover_local_run(
        &mut self,
        run_id: String,
        parked: Vec<RecoveredParkedCall>,
        model_name: String,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        self.active_tool_calls.clear();
        self.completed_tool_calls.clear();
        self.turn_executed_tool_calls = 0;
        self.turn_tool_calls.clear();
        if !matches!(self.state, ConversationState::Idle) {
            self.stream_generation += 1;
        }
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
        self.active_conversation_id = Some(conv_id);
        self.checkpoints.bind(self.pane_uuid, conv_id);
        self.streaming_text.clear();
        self.reset_streaming_on_next_text = false;
        self.thinking_block_id = None;
        self.turn_count += 1;
        self.last_model_name = model_name;
        self.run_id_override = Some(run_id.clone());
        self.streaming_task_id = Some(run_id.clone());
        if !self.host_tools.is_empty() {
            self.deps
                .pipeline
                .register_host_tools(&run_id, self.host_tools.clone());
        }
        self.deps.tools.set_project_cwd(&self.cwd, self.pane_uuid);
        self.open_turn_blocks(conv_id);

        let (tx, rx) = mpsc::channel::<Result<PipelineEvent, StreamError>>();
        if parked.is_empty() {
            self.state = ConversationState::Streaming;
            self.recovery_tx = None;
            let run_id = run_id.clone();
            let pipeline = Arc::clone(&self.deps.pipeline);
            spawn_background(Box::new(move || {
                if let Err(error) = pipeline.launch_recovered_run(&run_id, tx.clone()) {
                    let _ = tx.send(Err(resume_error_to_stream_error(error)));
                }
            }));
        } else {
            self.remove_loading_block();
            for call in parked {
                let action_id = Uuid::new_v4();
                let reason = if call.reason.trim().is_empty() {
                    format!("Agent wants to {}", call.tool_name.replace('_', " "))
                } else {
                    call.reason.clone()
                };
                let command = tool_summary_for_approval(&call.tool_name, &call.args_json);
                self.pending_tool_calls.insert(
                    action_id,
                    PendingToolCall {
                        call_id: call.call_id.clone(),
                        tool_name: call.tool_name.clone(),
                        args_json: call.args_json.clone(),
                        run_in_terminal: false,
                        allow_terminal_cwd_change: false,
                        external_read_roots: Vec::new(),
                    },
                );
                if let Some(event) = self.enqueue_approval(super::core::QueuedApproval {
                    action_id,
                    command,
                    reason,
                    conversation_id: conv_id,
                }) {
                    self.deferred_events.push(event);
                }
            }
            // Keep the channel open until an approval resumes the run.
            self.recovery_tx = Some(tx);
            sink.push(CoreEvent::BlocksChanged);
            for event in std::mem::take(&mut self.deferred_events) {
                sink.push(event);
            }
            return None;
        }
        sink.push(CoreEvent::BlocksChanged);
        for event in std::mem::take(&mut self.deferred_events) {
            sink.push(event);
        }

        let generation = self.stream_generation;
        Some(LaunchedTurn {
            conv_id,
            rx,
            generation,
            started_at: web_time::Instant::now(),
        })
    }

    pub(crate) fn start_streaming_query_impl(
        &mut self,
        query: String,
        model_name: String,
        system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
        gate_tools: bool,
        origin: QueryOrigin,
        runtime_event: Option<RuntimeEvent>,
        sink: &mut dyn EventSink,
    ) -> Option<LaunchedTurn> {
        self.active_tool_calls.clear();
        self.completed_tool_calls.clear();
        self.turn_executed_tool_calls = 0;
        self.turn_tool_calls.clear();
        if !matches!(self.state, ConversationState::Idle) {
            // Orphan the running stream, then wipe the session so the new query
            // starts from a clean slate without the previous context.
            self.stream_generation += 1;
            self.clear_conversation(sink);
        }

        if self.turn_count >= MAX_SESSION_TURNS {
            let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
            self.push_block(
                AgentBlockKind::AgentMessage {
                    text: format!(
                        "Session limit reached ({MAX_SESSION_TURNS} turns). \
                         Use /clear to start a new conversation."
                    ),
                    is_streaming: false,
                },
                conv_id,
            );
            sink.push(CoreEvent::BlocksChanged);
            return None;
        }

        self.gate_tools_until_plan = gate_tools;

        // On first turn of a session, kick off dream consolidation in background.
        if self.turn_count == 0 {
            let model = model_name.clone();
            let cwd = self.cwd.clone();
            let hooks = Arc::clone(&self.deps.hooks);
            spawn_background(Box::new(move || {
                hooks.on_session_start(&model, &cwd);
            }));
        }

        // Keep the project CWD in sync so local tools can resolve relative paths.
        self.deps.tools.set_project_cwd(&self.cwd, self.pane_uuid);

        // NOTE: the session_facts ledger reset intentionally does NOT live here
        // — only in `start_streaming_query` (user-typed turns). Plan approval
        // and system continuations reach this impl too, and both must KEEP the
        // ledger (research-phase reads / mid-task progress respectively).

        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
        self.active_conversation_id = Some(conv_id);
        self.checkpoints.bind(self.pane_uuid, conv_id);
        self.streaming_text.clear();
        self.reset_streaming_on_next_text = false;
        self.thinking_block_id = None;
        self.turn_count += 1;

        match origin {
            QueryOrigin::User => {
                // Snapshot the pending attachments for DISPLAY on the block (the
                // base64 payload is taken separately below for the wire). This
                // keeps the image thumbnail / file chip visible on the sent
                // prompt so the user can confirm what they attached.
                let block_attachments = self
                    .pending_query_attachments
                    .iter()
                    .map(|a| a.to_block_display())
                    .collect();
                let query_author = self.pending_query_author.take();
                let query_block_id = self.push_block(
                    AgentBlockKind::UserQuery {
                        text: self
                            .pending_query_display
                            .clone()
                            .unwrap_or_else(|| query.clone()),
                        author: query_author,
                        attached_blocks: Vec::new(),
                        attachments: block_attachments,
                        checkpoint: None,
                    },
                    conv_id,
                );
                // A NEW user prompt owns subsequent checkpoints.
                self.current_query_block_id = Some(query_block_id);
            }
            QueryOrigin::System => {
                // Continuation input (a background-task result, an auto-resume
                // note) is fed to the LLM as this turn's `query`, but is NOT
                // shown as its own block — it's internal plumbing, and the user
                // sees the agent's human summary of it (an AgentMessage) instead
                // of the raw "[system] …/[Background task …]" text. Deliberately
                // no checkpoint-owner change, so edits it drives still attach to
                // the originating user prompt.
            }
        }
        self.open_turn_blocks(conv_id);
        self.state = ConversationState::Streaming;
        sink.push(CoreEvent::BlocksChanged);

        // On the very first query of a fresh session, load the most recent JSONL
        // transcript scoped to THIS pane's UUID as invisible LLM context. Each
        // pane has its own session subdirectory so tabs with the same CWD never
        // share history with each other.
        if self.session_history.is_empty() && self.session_store.is_none() {
            if let Some(loaded) = self.deps.transcripts.load_last(&self.cwd, self.pane_uuid) {
                self.session_history = loaded.history;
            }
        }

        // Initialize the transcript before the API call. Only an actual user
        // turn is recorded as role=user; recovery and background continuations
        // are controller runtime events and must not become fabricated user
        // instructions in future history.
        let pane_uuid_for_store = self.pane_uuid;
        let cwd_for_store = self.cwd.clone();
        let transcripts = Arc::clone(&self.deps.transcripts);
        let store = self
            .session_store
            .get_or_insert_with(|| transcripts.open_new(&cwd_for_store, pane_uuid_for_store));
        if origin == QueryOrigin::User {
            self.session_history.push(SessionMessage {
                role: "user".to_string(),
                content: query.clone(),
                attachments: std::mem::take(&mut self.pending_query_attachments),
            });
            // Write the real user request before the API call so a crash does
            // not lose it.
            let transcript_text = self
                .pending_query_display
                .take()
                .unwrap_or_else(|| query.clone());
            store.append(&crate::session_store::TranscriptEntry::user(
                &transcript_text,
            ));
        } else {
            self.pending_query_attachments.clear();
            self.pending_query_display = None;
        }

        // Merge-back for the previous turn's client-side compaction (item 2):
        // adopt the compacted snapshot, preserving entries appended during
        // that turn. A shrunk history (clear / truncate / revert since the
        // snapshot) discards the stale slot instead of resurrecting blocks.
        if let Some((compacted, base_len)) = self
            .compacted_history_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            if self.session_history.len() >= base_len {
                let appended = self.session_history.len() - base_len;
                let mut merged = compacted;
                merged.extend(self.session_history[base_len..].iter().cloned());
                log::info!(
                    "[agent] merged compacted history ({} snapshot + {appended} appended)",
                    merged.len() - appended,
                );
                self.session_history = merged;
            } else {
                log::info!("[agent] discarded stale compacted snapshot after history shrink");
            }
        }

        // Save model name for approve_plan reuse.
        self.last_model_name = model_name.clone();
        // last_system_arc will be written by the background thread once built.
        let last_system_arc = self.last_system_arc.clone();

        let history = self.session_history.clone();
        let is_coordinator = self.role == AgentRole::Coordinator;
        let mut allowed_tools = self.allowed_tools.clone();
        let mut run_id_override = self.run_id_override.clone();
        if !self.host_tools.is_empty() {
            // Host tools need a scoped stream with a known run id so their
            // definitions can be attached to that exact request.
            let run_id = run_id_override
                .get_or_insert_with(|| format!("local-{}", uuid::Uuid::new_v4().simple()))
                .clone();
            let allowed = allowed_tools.get_or_insert_with(Vec::new);
            for tool in &self.host_tools {
                if !allowed.iter().any(|name| name == &tool.name) {
                    allowed.push(tool.name.clone());
                }
            }
            self.deps
                .pipeline
                .register_host_tools(&run_id, self.host_tools.clone());
        }
        let (tx, rx) = mpsc::channel::<Result<PipelineEvent, StreamError>>();

        log::info!(
            "[agent] spawning pipeline thread — history={} messages, ~{} tokens, mode={}",
            history.len(),
            estimate_tokens(&history),
            if is_coordinator {
                "coordinator"
            } else {
                "normal"
            }
        );
        let thread_start = web_time::Instant::now();

        // Structured server resume, when recovering (`(task_id, context)`).
        // The pipeline owns the resume-versus-fresh branch; the core only
        // forwards it on the request.
        let resume = runtime_event.as_ref().and_then(|event| match event {
            RuntimeEvent::ResumeCheckpoint {
                checkpoint,
                server_task_id: Some(server_task_id),
            } => Some((server_task_id.clone(), checkpoint.clone())),
            _ => None,
        });
        let mode = if is_coordinator { "coordinator" } else { "" };
        let pipeline = Arc::clone(&self.deps.pipeline);
        spawn_background(Box::new(move || {
            // Build the system prompt (ProjectContext + MemoryStore + formatting).
            // This is the previously-blocking work that was on the UI thread.
            let base_system = system_builder();
            let system = append_runtime_event(base_system.clone(), runtime_event.as_ref());
            log::info!(
                "[agent] system prompt ready ({}ms since query, {} chars)",
                thread_start.elapsed().as_millis(),
                system.len()
            );
            // Persist for approve_plan, which reads this after the agent responds.
            if let Ok(mut guard) = last_system_arc.lock() {
                *guard = base_system;
            }
            log::info!(
                "[agent] starting unified task stream ({}ms since query submitted)",
                thread_start.elapsed().as_millis()
            );
            let request = StreamLaunchRequest {
                model_name,
                system,
                query,
                history,
                mode: mode.to_string(),
                allowed_tools,
                run_id: run_id_override,
                resume,
            };
            pipeline.launch_stream(request, tx);
        }));

        let generation = self.stream_generation;
        Some(LaunchedTurn {
            conv_id,
            rx,
            generation,
            started_at: web_time::Instant::now(),
        })
    }

    /// True while the streaming SSE pipeline is active.
    ///
    /// This is specifically "the server has given us a task id and events are
    /// flowing" — approval resolution needs that, because it posts back against
    /// the task id. For "is a turn in flight at all", use [`is_busy`].
    pub fn is_streaming(&self) -> bool {
        self.streaming_task_id.is_some()
    }

    /// True from the moment a turn is submitted until it finishes — including
    /// the `ProcessingQuery` window, before the server has returned a task id.
    ///
    /// `is_streaming()` is false during that window, so keying the Send/Stop
    /// button off it left a gap where the request was already in flight but the
    /// button still said Send and the user could not abort. `cancel_stream`
    /// works throughout: it aborts by bumping `stream_generation`, which
    /// orphans the poll loop and needs no task id.
    pub fn is_busy(&self) -> bool {
        !matches!(self.state, ConversationState::Idle)
    }

    /// Toggle whether a thinking step block is expanded or collapsed.
    pub fn toggle_thinking_expanded(&mut self, block_id: usize, sink: &mut dyn EventSink) {
        if let Some(block) = self.find_block_mut_by_id(block_id) {
            if let AgentBlockKind::ThinkingStep { expanded, .. } = &mut block.kind {
                *expanded = !*expanded;
                sink.push(CoreEvent::BlocksChanged);
            }
        }
    }

    /// Toggle whether a tool step block is expanded or collapsed.
    pub fn toggle_tool_step_expanded(&mut self, block_id: usize, sink: &mut dyn EventSink) {
        if let Some(block) = self.find_block_mut_by_id(block_id) {
            if let AgentBlockKind::ToolStep { expanded, .. } = &mut block.kind {
                *expanded = !*expanded;
                sink.push(CoreEvent::BlocksChanged);
            }
        }
    }

    /// Toggle whether an activity group or turn group block is expanded or collapsed.
    /// Returns true if the block was found and toggled.
    pub fn toggle_activity_group_expanded(
        &mut self,
        block_id: usize,
        sink: &mut dyn EventSink,
    ) -> bool {
        if let Some(block) = self.find_block_mut_by_id(block_id) {
            match &mut block.kind {
                AgentBlockKind::ActivityGroup { expanded, .. } => {
                    *expanded = !*expanded;
                    sink.push(CoreEvent::BlocksChanged);
                    return true;
                }
                AgentBlockKind::TurnGroup { expanded, .. } => {
                    *expanded = !*expanded;
                    sink.push(CoreEvent::BlocksChanged);
                    return true;
                }
                _ => {}
            }
        }
        false
    }
}
