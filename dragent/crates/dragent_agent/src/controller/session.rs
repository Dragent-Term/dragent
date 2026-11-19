//! AgentController session helpers.
//!
//! Session restore, SQLite block restore, host setters, and read accessors.
//! Moved verbatim from `app/src/agent/controller/session.rs`; same behavior.
//! Seam rewrites only: constructors (`new`/`with_allowed_roots`),
//! `display_text`, and `set_allowed_roots` stay app-side; `set_persistence`
//! already lives in `super::core`; transcripts go through
//! `deps.transcripts` (`TranscriptStore`/`TranscriptSession`); `ctx.emit` /
//! `ctx.notify` become `sink.push` (notifies dropped); warpui imports gone.

use std::sync::Arc;

use uuid::Uuid;

use super::AgentRole;
use super::core::AgentController;
use crate::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionStatus,
    ToolStepStatus,
};
use crate::pipeline::HostToolDef;
use crate::runtime::{CoreEvent, EventSink};
use crate::state::ConversationState;
use crate::types::{
    DisplayTextFilter, ResponseStyle, SessionMessage, ToolPolicy, TurnToolCall, truncate_chars,
};

impl AgentController {
    /// Try to restore the last session for `cwd`, scoped to this pane's UUID.
    /// Returns true if a session was found and loaded.
    pub fn restore_last_session(&mut self) -> bool {
        let Some(loaded) = self.deps.transcripts.load_last(&self.cwd, self.pane_uuid) else {
            return false;
        };

        self.session_history = loaded.history;

        // Reconstruct a synthetic conv_id so all resumed blocks belong together
        if !loaded.blocks.is_empty() {
            let conv_id = loaded.blocks[0].conversation_id;
            self.active_conversation_id = Some(conv_id);
            for rb in loaded.blocks {
                let id = self.next_block_id;
                self.next_block_id += 1;
                self.blocks
                    .push(AgentBlock::new(id, rb.conversation_id, rb.kind));
            }
        }

        // Continue appending to the same file
        self.session_store = Some(
            self.deps
                .transcripts
                .open_existing(loaded.path, loaded.session_id),
        );
        true
    }

    /// Restore agent blocks from the SQLite store.
    /// Called by `TerminalView::new_with_restore` when SQLite has finer-grained
    /// data than the JSONL transcript. Replaces any blocks already loaded by
    /// `restore_last_session` with the richer per-kind representation.
    ///
    /// `true_max_sort_order` is the pane's highest `agent_blocks.sort_order`
    /// across EVERY conversation in SQLite (from
    /// `dragent_persistence::max_sort_order_for_pane`), not just the ones in
    /// `persisted` — since `load_session` now only restores a pane's latest
    /// conversation, older ones can exist unloaded in the DB. Without this,
    /// renumbering IDs from 0 below could produce `sort_order`s that collide
    /// with — and silently overwrite — an unloaded older conversation's rows
    /// the next time a block is saved.
    pub fn restore_blocks_from_sql(
        &mut self,
        persisted: Vec<dragent_persistence::PersistedAgentBlock>,
        true_max_sort_order: Option<i32>,
    ) {
        self.load_persisted_blocks(persisted, true_max_sort_order);
    }

    /// Swap the active conversation to `conversation_id`, loading `persisted`
    /// (from `dragent_persistence::load_conversation`) as the new `self.blocks`
    /// — the session-history-browser "resume" action. Same
    /// `true_max_sort_order` requirement as `restore_blocks_from_sql`, for the
    /// same reason: this can run mid-session with other conversations sitting
    /// unloaded in the DB.
    pub fn switch_to_conversation(
        &mut self,
        persisted: Vec<dragent_persistence::PersistedAgentBlock>,
        conversation_id: Uuid,
        true_max_sort_order: Option<i32>,
        sink: &mut dyn EventSink,
    ) {
        self.load_persisted_blocks(persisted, true_max_sort_order);
        // Loaded blocks already carry this conversation_id (set below), but
        // force it explicitly too in case `persisted` was somehow empty —
        // a follow-up message must still continue the RIGHT conversation.
        self.active_conversation_id = Some(conversation_id);
        self.bind_checkpoints_for_active_conversation();
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Shared by `restore_blocks_from_sql` and `switch_to_conversation`:
    /// clears `self.blocks`, renumbers `persisted` into fresh sequential IDs
    /// starting at 0, then bumps `next_block_id` above both the nested
    /// TurnGroup/ActivityGroup IDs embedded in what was just loaded AND
    /// `true_max_sort_order` (see the doc comments above for why the latter
    /// matters).
    fn load_persisted_blocks(
        &mut self,
        persisted: Vec<dragent_persistence::PersistedAgentBlock>,
        true_max_sort_order: Option<i32>,
    ) {
        self.blocks.clear();
        self.next_block_id = 0;
        self.active_conversation_id = None;

        for p in persisted {
            match serde_json::from_str::<AgentBlockKind>(&p.kind_json) {
                Ok(kind) => {
                    let id = self.next_block_id;
                    self.next_block_id += 1;
                    if self.active_conversation_id.is_none() {
                        self.active_conversation_id = Some(p.conversation_id);
                    }
                    self.blocks.push(AgentBlock::new_with_timestamp(
                        id,
                        p.conversation_id,
                        kind,
                        p.created_at_unix_ms,
                    ));
                }
                Err(e) => {
                    log::warn!("[agent] failed to deserialize persisted block: {e}");
                }
            }
        }
        // Ensure next_block_id is above any IDs embedded in nested TurnGroup/ActivityGroup blocks.
        let mut max_nested_id = self.next_block_id;
        for b in &self.blocks {
            if let AgentBlockKind::TurnGroup { children, .. } = &b.kind {
                for c in children {
                    max_nested_id = max_nested_id.max(c.id + 1);
                    if let AgentBlockKind::ActivityGroup {
                        blocks: ag_blocks, ..
                    } = &c.kind
                    {
                        for ab in ag_blocks {
                            max_nested_id = max_nested_id.max(ab.id + 1);
                        }
                    }
                }
            }
            if let AgentBlockKind::ActivityGroup {
                blocks: ag_blocks, ..
            } = &b.kind
            {
                for ab in ag_blocks {
                    max_nested_id = max_nested_id.max(ab.id + 1);
                }
            }
        }
        // And above every sort_order already used anywhere in SQLite for this
        // pane, including conversations not in `persisted` at all.
        let floor = true_max_sort_order.map(|v| v + 1).unwrap_or(0).max(0) as usize;
        self.next_block_id = max_nested_id.max(floor);
        if self.persistence_sender.is_some() {
            self.bind_checkpoints_for_active_conversation();
        }
    }
}

impl AgentController {
    /// Refresh tool permissions when the terminal cwd changes.
    /// Restrict the provider-visible tool set for every following turn (an
    /// embedded host such as the Work Item Principal). `None` restores the
    /// terminal's unrestricted inventory. Interactive tools the controller
    /// intercepts itself (`ask_user`, `todowrite`) must be listed too — the
    /// model can only call what the offer contains.
    pub fn set_allowed_tools(&mut self, tools: Option<Vec<String>>) {
        self.allowed_tools = tools;
    }

    /// Per-turn cap on EXECUTED research tools (ask_user / todowrite never
    /// count). Past the cap each further call gets a "synthesize now" tool
    /// error instead of running. `None` = unlimited.
    pub fn set_tool_call_budget(&mut self, budget: Option<u32>) {
        self.tool_call_budget = budget;
    }

    /// Presentation-only filter over the assistant's streamed text — see
    /// `display_text_filter`. The raw text still reaches `session_history`
    /// and `TurnCompleted`, so a structured-output host can parse the full
    /// payload while the block shows only the conversational part.
    pub fn set_display_text_filter(&mut self, filter: Option<DisplayTextFilter>) {
        self.display_text_filter = filter;
    }

    /// Install a per-call tool policy — the Work Contract's enforcement for a
    /// worker/reviewer attempt. `Execute` runs without asking (the contract
    /// pre-authorized it), `NeedsConsent` shows the approval card (and the
    /// host raises Attention), `Denied` posts the reason back as a tool error.
    pub fn set_tool_policy(&mut self, policy: Option<ToolPolicy>) {
        self.tool_policy = policy;
    }

    /// Use a stable run id for scoped streams (see `run_id_override`).
    pub fn set_run_id(&mut self, run_id: Option<String>) {
        self.run_id_override = run_id;
    }

    /// Tools executed by the embedding host (see `host_tools`). Their names
    /// are added to the provider allowlist automatically; the host answers a
    /// `HostToolCall` event with `pipeline::post_tool_result`.
    pub fn set_host_tools(&mut self, tools: Vec<HostToolDef>) {
        self.host_tools = tools;
    }

    /// Replace the provider-facing transcript. Embedded hosts that carry the
    /// conversation in their own canonical store (and rebuild it into the
    /// prompt) pass an empty Vec before each turn so the model never sees the
    /// history twice. The JSONL transcript store is opened here too, so the
    /// first turn's "load the last transcript as invisible context" step is
    /// skipped instead of silently re-adding what was just cleared.
    pub fn set_session_history(&mut self, history: Vec<SessionMessage>) {
        self.session_history = history;
        let pane_uuid = self.pane_uuid;
        let cwd = self.cwd.clone();
        let transcripts = Arc::clone(&self.deps.transcripts);
        self.session_store
            .get_or_insert_with(|| transcripts.open_new(&cwd, pane_uuid));
    }

    /// Tool calls the model issued during the current (or, right after
    /// `TurnCompleted`, the just-finished) turn.
    pub fn turn_tool_calls(&self) -> &[TurnToolCall] {
        &self.turn_tool_calls
    }

    /// Surface a host-side failure (post-turn validation, authority rejection)
    /// as the same Error card a stream failure would produce, so it keeps its
    /// place in the transcript and persists like every other block.
    pub fn push_host_error(&mut self, message: String, sink: &mut dyn EventSink) {
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
        self.push_block(
            AgentBlockKind::Error {
                message,
                kind: ErrorKind::Internal,
                task_id: None,
            },
            conv_id,
        );
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Record an exchange that happened without a provider pass (a host's
    /// canned reply to a greeting): the same user + agent blocks a streamed
    /// turn would leave, persisted the same way.
    pub fn push_local_exchange(
        &mut self,
        user_text: String,
        assistant_text: String,
        sink: &mut dyn EventSink,
    ) {
        let conv_id = self.active_conversation_id.unwrap_or_else(Uuid::new_v4);
        self.active_conversation_id = Some(conv_id);
        self.push_block(
            AgentBlockKind::UserQuery {
                text: user_text,
                author: None,
                attached_blocks: Vec::new(),
                attachments: Vec::new(),
                checkpoint: None,
            },
            conv_id,
        );
        self.push_block(
            AgentBlockKind::AgentMessage {
                text: assistant_text,
                is_streaming: false,
            },
            conv_id,
        );
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Plain-text digest of what this agent is doing / has done — what a
    /// Principal reads when asked "what is the frontend agent working on?".
    /// Walks the block list in order (TurnGroup / ActivityGroup flattened):
    /// live state, the last `max_steps` tool steps with their results, the
    /// latest task list, open approval/question cards, errors, the last
    /// reply, and files touched. Never includes thinking content.
    pub fn activity_digest(&self, max_steps: usize) -> String {
        fn walk<'a>(blocks: &'a [AgentBlock], out: &mut Vec<&'a AgentBlock>) {
            for block in blocks {
                match &block.kind {
                    AgentBlockKind::TurnGroup { children, .. } => walk(children, out),
                    AgentBlockKind::ActivityGroup { blocks, .. } => walk(blocks, out),
                    _ => out.push(block),
                }
            }
        }
        let mut flat = Vec::new();
        walk(&self.blocks, &mut flat);

        let state = match self.state {
            ConversationState::Idle => "idle (no turn running)",
            ConversationState::ProcessingQuery => "starting a turn",
            ConversationState::Streaming => "working (streaming)",
            ConversationState::ExecutingTool => "executing a tool",
            ConversationState::AwaitingApproval { .. } => {
                "BLOCKED: waiting for a permission decision"
            }
            ConversationState::AwaitingUserChoice { .. } => {
                "BLOCKED: waiting for an answer to its question"
            }
        };
        let mut lines = vec![format!("state: {state}")];

        let steps: Vec<String> = flat
            .iter()
            .filter_map(|block| match &block.kind {
                AgentBlockKind::ToolStep {
                    name,
                    args_summary,
                    status,
                    result_summary,
                    ..
                } => Some(format!(
                    "- {} {} [{}]{}",
                    name,
                    args_summary,
                    match status {
                        ToolStepStatus::Running => "running",
                        ToolStepStatus::Done => "done",
                        ToolStepStatus::Error => "error",
                    },
                    result_summary
                        .as_deref()
                        .filter(|r| !r.is_empty())
                        .map(|r| format!(" → {}", truncate_chars(r, 160)))
                        .unwrap_or_default()
                )),
                _ => None,
            })
            .collect();
        if !steps.is_empty() {
            let skip = steps.len().saturating_sub(max_steps);
            lines.push(format!(
                "recent tool steps ({} of {}):",
                steps.len() - skip,
                steps.len()
            ));
            lines.extend(steps.into_iter().skip(skip));
        }
        if let Some(todos) = flat.iter().rev().find_map(|block| match &block.kind {
            AgentBlockKind::TodoList { todos } => Some(todos),
            _ => None,
        }) {
            lines.push("task list:".into());
            for todo in todos {
                lines.push(format!("- [{}] {}", todo.status, todo.content));
            }
        }
        for block in &flat {
            match &block.kind {
                AgentBlockKind::ApprovalRequest {
                    command,
                    reason,
                    status: ApprovalStatus::Pending,
                    ..
                } => lines.push(format!("OPEN PERMISSION REQUEST: {reason} ({command})")),
                AgentBlockKind::UserQuestion {
                    questions,
                    status: QuestionStatus::Pending,
                    ..
                } => lines.push(format!(
                    "OPEN QUESTION: {}",
                    questions
                        .iter()
                        .map(|q| q.question.as_str())
                        .collect::<Vec<_>>()
                        .join(" · ")
                )),
                AgentBlockKind::Error { message, .. } => {
                    lines.push(format!("ERROR: {}", truncate_chars(message, 240)))
                }
                _ => {}
            }
        }
        if let Some(text) = flat.iter().rev().find_map(|block| match &block.kind {
            AgentBlockKind::AgentMessage { text, .. } if !text.trim().is_empty() => Some(text),
            _ => None,
        }) {
            lines.push(format!("last reply: {}", truncate_chars(text, 600)));
        }
        if !self.agent_modified_files.is_empty() {
            lines.push(format!(
                "files written: {}",
                self.agent_modified_files.join(", ")
            ));
        }
        lines.join("\n")
    }

    /// Change the response verbosity style injected into the system prompt.
    pub fn set_response_style(&mut self, style: ResponseStyle) {
        self.response_style = style;
    }

    /// Apply the input-bar chat mode for upcoming turns: the agent role (Multitask
    /// → Coordinator), whether write/exec tools are gated behind a plan (Plan),
    /// and whether mutating tools are hard-refused (Ask). The mode's
    /// system-prompt section is applied view-side at submit time.
    pub fn set_chat_mode(&mut self, role: AgentRole, gate_tools: bool, read_only: bool) {
        self.role = role;
        self.mode_gate = gate_tools;
        self.read_only_mode = read_only;
    }

    /// Keep the current command-intent request visible until the user approves
    /// it. This remains active for follow-up turns generated by that request.
    pub fn set_force_command_approval(&mut self, force: bool, allow_cwd_change: bool) {
        self.force_command_approval = force;
        self.allow_terminal_cwd_change = force && allow_cwd_change;
    }

    /// The next query comes from an automation: its turn (and the work that
    /// follows from it) never waits for an approval or an answer.
    pub fn set_next_query_unattended(&mut self, unattended: bool) {
        self.next_query_unattended = unattended;
    }

    /// Whether the next query's automation lets its turn comment, change
    /// statuses and create items in connected services.
    pub fn set_next_query_service_writes(&mut self, allowed: bool) {
        self.next_query_service_writes = allowed;
    }

    pub fn is_unattended(&self) -> bool {
        self.unattended
    }

    /// Mark a tool as always-allowed and execute the pending approval immediately.
    pub fn allow_tool_always(&mut self, action_id: Uuid, sink: &mut dyn EventSink) {
        if self.frontmost_pending_approval() != Some(action_id) {
            return;
        }
        // External reads are persisted by directory, never by tool name. A
        // blanket `read_file` grant would authorize that tool against every
        // file on the machine instead of the location the user reviewed.
        if let Some(pending) = self.pending_tool_calls.get(&action_id) {
            if pending.external_read_roots.is_empty() {
                self.permission_store.allow_always(&pending.tool_name);
            } else {
                for root in &pending.external_read_roots {
                    self.permission_store.allow_read_root_always(root);
                }
            }
        }
        self.set_approval_status(action_id, ApprovalStatus::ApprovedAlways);
        self.resolve_streaming_approval(action_id, true, None, sink);
    }

    /// Budget accessors for the UI.
    pub fn turn_count(&self) -> u32 {
        self.turn_count
    }
    pub fn session_input_tokens(&self) -> u64 {
        self.session_input_tokens
    }
    pub fn session_cached_input_tokens(&self) -> u64 {
        self.session_cached_input_tokens
    }
    pub fn session_output_tokens(&self) -> u64 {
        self.session_output_tokens
    }
    /// Cumulative tokens across the session — input + cached + output — the
    /// true total the provider billed (matches the server's rolling window).
    pub fn session_total_tokens(&self) -> u64 {
        self.session_input_tokens + self.session_cached_input_tokens + self.session_output_tokens
    }
    /// Current context size (input + cached prompt tokens of the last turn),
    /// for the context-window meter. 0 until the first turn completes.
    pub fn current_context_tokens(&self) -> u64 {
        self.current_context_tokens
    }
    /// Model that produced the current context — the meter divides by this
    /// model's real context window.
    pub fn last_model_name(&self) -> &str {
        &self.last_model_name
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn agent_blocks(&self) -> &[AgentBlock] {
        &self.blocks
    }

    /// Replace this controller's display state with the authoritative snapshot
    /// produced by a shared-session host. Remote mirrors never execute these
    /// blocks or persist them; they only use the normal agent renderer.
    pub fn replace_remote_blocks(&mut self, blocks: Vec<AgentBlock>, sink: &mut dyn EventSink) {
        fn visit(blocks: &[AgentBlock], max_id: &mut usize, busy: &mut bool) {
            for block in blocks {
                *max_id = (*max_id).max(block.id.saturating_add(1));
                match &block.kind {
                    AgentBlockKind::AgentMessage {
                        is_streaming: true, ..
                    }
                    | AgentBlockKind::Loading { .. }
                    | AgentBlockKind::ToolStep {
                        status: ToolStepStatus::Running,
                        ..
                    }
                    | AgentBlockKind::ApprovalRequest {
                        status: ApprovalStatus::Pending,
                        ..
                    }
                    | AgentBlockKind::UserQuestion {
                        status: QuestionStatus::Pending,
                        ..
                    }
                    | AgentBlockKind::PlanReview {
                        status: PlanStatus::Pending,
                        ..
                    } => *busy = true,
                    AgentBlockKind::TurnGroup {
                        children,
                        duration_ms,
                        ..
                    } => {
                        if duration_ms.is_none() {
                            *busy = true;
                        }
                        visit(children, max_id, busy)
                    }
                    AgentBlockKind::ActivityGroup { blocks, .. } => visit(blocks, max_id, busy),
                    _ => {}
                }
            }
        }

        let mut next_id = 0;
        let mut busy = false;
        visit(&blocks, &mut next_id, &mut busy);
        self.blocks = blocks;
        self.next_block_id = next_id;
        self.state = if busy {
            ConversationState::Streaming
        } else {
            ConversationState::Idle
        };
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Returns the first pending approval action_id, if any.
    pub fn first_pending_action_id(&self) -> Option<Uuid> {
        self.frontmost_pending_approval()
    }

    pub fn state(&self) -> &ConversationState {
        &self.state
    }

    pub fn active_conversation_id(&self) -> Option<Uuid> {
        self.active_conversation_id
    }
}
