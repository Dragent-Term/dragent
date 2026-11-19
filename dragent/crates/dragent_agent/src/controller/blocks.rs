//! Block storage helpers for AgentController.
//!
//! Block allocation, lookup, persistence fan-out, loading placeholders.
//! Moved verbatim; same behavior.

use uuid::Uuid;

use super::core::{AgentController, QueuedApproval};
use crate::block::{AgentBlock, AgentBlockKind, ApprovalStatus};
use crate::runtime::{CoreEvent, EventSink};

impl AgentController {
    pub(crate) fn enqueue_approval(&mut self, approval: QueuedApproval) -> Option<CoreEvent> {
        self.queued_approvals.push_back(approval);
        if self.frontmost_pending_approval().is_none() {
            self.show_next_approval()
        } else {
            None
        }
    }

    pub(crate) fn show_next_approval(&mut self) -> Option<CoreEvent> {
        let approval = self.queued_approvals.pop_front()?;
        let pending = self.pending_tool_calls.get(&approval.action_id)?;
        let event = CoreEvent::ConsentRequested {
            action_id: approval.action_id,
            call_id: pending.call_id.clone(),
            tool_name: pending.tool_name.clone(),
            args_json: pending.args_json.clone(),
            reason: approval.reason.clone(),
        };
        self.push_block(
            AgentBlockKind::ApprovalRequest {
                action_id: approval.action_id,
                command: approval.command,
                reason: approval.reason,
                status: ApprovalStatus::Pending,
            },
            approval.conversation_id,
        );
        self.state = crate::state::ConversationState::AwaitingApproval {
            action_id: approval.action_id,
        };
        Some(event)
    }

    /// Returns the ID of the block that was created (for non-internal kinds: the top-level ID;
    /// for internal kinds grouped in ActivityGroup: the inner block's ID).
    pub(crate) fn push_block(&mut self, kind: AgentBlockKind, conversation_id: Uuid) -> usize {
        // Route Loading into the active TurnGroup (if any) so it shows inside the work wrapper.
        if matches!(kind, AgentBlockKind::Loading { .. }) {
            let id = self.next_block_id;
            self.next_block_id += 1;
            if let Some(tg_pos) = self.blocks.iter().rposition(|b| {
                matches!(
                    b.kind,
                    AgentBlockKind::TurnGroup {
                        duration_ms: None,
                        ..
                    }
                )
            }) {
                let block = AgentBlock::new(id, conversation_id, kind);
                if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[tg_pos].kind {
                    children.push(block);
                }
                return id;
            }
            self.blocks.push(AgentBlock::new(id, conversation_id, kind));
            return id;
        }

        let is_internal = matches!(
            kind,
            AgentBlockKind::ThinkingStep { .. }
                | AgentBlockKind::ToolStep { .. }
                | AgentBlockKind::ActionResult { .. }
        ) || matches!(
            kind,
            AgentBlockKind::ApprovalRequest {
                status: ApprovalStatus::Approved
                    | ApprovalStatus::ApprovedAlways
                    | ApprovalStatus::Rejected,
                ..
            }
        );

        if is_internal {
            let inner_id = self.next_block_id;
            self.next_block_id += 1;
            let inner_block = AgentBlock::new(inner_id, conversation_id, kind.clone());

            if let Some(last_block) = self.blocks.last_mut() {
                if let AgentBlockKind::ActivityGroup { blocks, .. } = &mut last_block.kind {
                    blocks.push(inner_block);
                    let ag_id = last_block.id;
                    self.update_block_in_db(ag_id);
                    return inner_id;
                }
            }

            // Create a new ActivityGroup
            let ag_id = self.next_block_id;
            self.next_block_id += 1;
            let group_kind = AgentBlockKind::ActivityGroup {
                blocks: vec![inner_block],
                expanded: false,
            };
            self.blocks
                .push(AgentBlock::new(ag_id, conversation_id, group_kind.clone()));
            self.save_block_in_db(ag_id, group_kind, conversation_id);
            return inner_id;
        }

        let id = self.next_block_id;
        self.next_block_id += 1;
        self.blocks
            .push(AgentBlock::new(id, conversation_id, kind.clone()));
        self.save_block_in_db(id, kind, conversation_id);
        id
    }

    /// Push an internal block into the active TurnGroup (if one exists), with routing control:
    /// - `direct = true`: added as its own child row in TurnGroup (for write tools, ThinkingStep)
    /// - `direct = false`: added into an ActivityGroup inside TurnGroup (for read/search tools)
    /// Falls back to the old ActivityGroup-in-flat-list behaviour if no active TurnGroup exists.
    /// Returns the inner block's ID.
    pub(crate) fn push_turn_block(
        &mut self,
        kind: AgentBlockKind,
        direct: bool,
        conversation_id: Uuid,
    ) -> usize {
        let inner_id = self.next_block_id;
        self.next_block_id += 1;

        // Locate active TurnGroup (duration_ms = None means still streaming)
        let tg_pos = self.blocks.iter().rposition(|b| {
            matches!(
                b.kind,
                AgentBlockKind::TurnGroup {
                    duration_ms: None,
                    ..
                }
            )
        });

        if let Some(tg_pos) = tg_pos {
            let inner_block = AgentBlock::new(inner_id, conversation_id, kind.clone());
            if direct {
                if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[tg_pos].kind {
                    children.push(inner_block);
                }
            } else {
                // Check if TurnGroup's last child is already an ActivityGroup
                let has_trailing_ag =
                    if let AgentBlockKind::TurnGroup { children, .. } = &self.blocks[tg_pos].kind {
                        matches!(
                            children.last().map(|c| &c.kind),
                            Some(AgentBlockKind::ActivityGroup { .. })
                        )
                    } else {
                        false
                    };

                if has_trailing_ag {
                    if let AgentBlockKind::TurnGroup { children, .. } =
                        &mut self.blocks[tg_pos].kind
                    {
                        if let Some(last_child) = children.last_mut() {
                            if let AgentBlockKind::ActivityGroup {
                                blocks: ag_blocks, ..
                            } = &mut last_child.kind
                            {
                                ag_blocks.push(inner_block);
                            }
                        }
                    }
                } else {
                    let ag_id = self.next_block_id;
                    self.next_block_id += 1;
                    let ag = AgentBlock::new(
                        ag_id,
                        conversation_id,
                        AgentBlockKind::ActivityGroup {
                            blocks: vec![inner_block],
                            expanded: false,
                        },
                    );
                    if let AgentBlockKind::TurnGroup { children, .. } =
                        &mut self.blocks[tg_pos].kind
                    {
                        children.push(ag);
                    }
                }
            }
            return inner_id;
        }

        // No active TurnGroup: fall back to old ActivityGroup-in-flat-list behaviour
        let inner_block = AgentBlock::new(inner_id, conversation_id, kind.clone());
        if let Some(last_block) = self.blocks.last_mut() {
            if let AgentBlockKind::ActivityGroup { blocks, .. } = &mut last_block.kind {
                blocks.push(inner_block);
                let ag_id = last_block.id;
                self.update_block_in_db(ag_id);
                return inner_id;
            }
        }
        let ag_id = self.next_block_id;
        self.next_block_id += 1;
        let group_kind = AgentBlockKind::ActivityGroup {
            blocks: vec![inner_block],
            expanded: false,
        };
        self.blocks
            .push(AgentBlock::new(ag_id, conversation_id, group_kind.clone()));
        self.save_block_in_db(ag_id, group_kind, conversation_id);
        inner_id
    }

    /// Find a block by ID, searching the flat list, TurnGroup children, and ActivityGroup blocks
    /// nested inside TurnGroup. Returns a mutable reference if found.
    pub(crate) fn find_block_mut_by_id(&mut self, id: usize) -> Option<&mut AgentBlock> {
        for b in self.blocks.iter_mut() {
            if b.id == id {
                return Some(b);
            }
            if let AgentBlockKind::TurnGroup { children, .. } = &mut b.kind {
                for c in children.iter_mut() {
                    if c.id == id {
                        return Some(c);
                    }
                    if let AgentBlockKind::ActivityGroup {
                        blocks: ag_blocks, ..
                    } = &mut c.kind
                    {
                        for ab in ag_blocks.iter_mut() {
                            if ab.id == id {
                                return Some(ab);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Find the block ID of a ToolStep matching `tool_id`, searching all nesting levels.
    pub(crate) fn find_tool_step_block_id(&self, tool_id: &str) -> Option<usize> {
        for b in &self.blocks {
            if let AgentBlockKind::ToolStep { tool_id: tid, .. } = &b.kind {
                if tid == tool_id {
                    return Some(b.id);
                }
            }
            if let AgentBlockKind::TurnGroup { children, .. } = &b.kind {
                for c in children {
                    if let AgentBlockKind::ToolStep { tool_id: tid, .. } = &c.kind {
                        if tid == tool_id {
                            return Some(c.id);
                        }
                    }
                    if let AgentBlockKind::ActivityGroup {
                        blocks: ag_blocks, ..
                    } = &c.kind
                    {
                        for ab in ag_blocks {
                            if let AgentBlockKind::ToolStep { tool_id: tid, .. } = &ab.kind {
                                if tid == tool_id {
                                    return Some(ab.id);
                                }
                            }
                        }
                    }
                }
            }
            // Also search flat ActivityGroup (fallback path without TurnGroup)
            if let AgentBlockKind::ActivityGroup {
                blocks: ag_blocks, ..
            } = &b.kind
            {
                for ab in ag_blocks {
                    if let AgentBlockKind::ToolStep { tool_id: tid, .. } = &ab.kind {
                        if tid == tool_id {
                            return Some(ab.id);
                        }
                    }
                }
            }
        }
        None
    }

    /// Find the ID of the outermost flat block that contains the block with `inner_id`.
    /// For blocks in the flat list, returns `inner_id`. For nested blocks, returns the
    /// TurnGroup (or ActivityGroup) ID. Used to save the right block to DB.
    pub(crate) fn containing_flat_block_id(&self, inner_id: usize) -> Option<usize> {
        for b in &self.blocks {
            if b.id == inner_id {
                return Some(inner_id);
            }
            if let AgentBlockKind::TurnGroup { children, .. } = &b.kind {
                let tg_id = b.id;
                for c in children {
                    if c.id == inner_id {
                        return Some(tg_id);
                    }
                    if let AgentBlockKind::ActivityGroup {
                        blocks: ag_blocks, ..
                    } = &c.kind
                    {
                        if ag_blocks.iter().any(|ab| ab.id == inner_id) {
                            return Some(tg_id);
                        }
                    }
                }
            }
            if let AgentBlockKind::ActivityGroup {
                blocks: ag_blocks, ..
            } = &b.kind
            {
                if ag_blocks.iter().any(|ab| ab.id == inner_id) {
                    return Some(b.id);
                }
            }
        }
        None
    }

    pub(crate) fn save_block_in_db(&self, id: usize, kind: AgentBlockKind, conversation_id: Uuid) {
        // Loading and QueuedMessage are transient: Loading is a spinner, and a
        // QueuedMessage's backing entry (its system-prompt closure) can't be
        // persisted anyway, so never write them — a reload would otherwise show
        // an orphan "Queued" pill that can never start.
        let should_persist = !matches!(
            kind,
            AgentBlockKind::Loading { .. } | AgentBlockKind::QueuedMessage { .. }
        );
        if should_persist {
            if let Some(ref sender) = self.persistence_sender {
                if let Ok(kind_json) = serde_json::to_string(&kind) {
                    let created_at_unix_ms = self
                        .blocks
                        .iter()
                        .find(|b| b.id == id)
                        .map(|b| b.created_at_unix_ms)
                        .unwrap_or(0);
                    let block = dragent_persistence::PersistedAgentBlock {
                        kind_json,
                        conversation_id,
                        created_at_unix_ms,
                    };
                    let sort_order = id as i32;
                    let _ = sender.send(dragent_persistence::ModelEvent::SaveAgentBlock {
                        pane_uuid: self.pane_uuid,
                        block,
                        sort_order,
                        project_root: Some(self.cwd.clone()),
                    });
                }
            }
        }
    }

    pub(crate) fn update_block_in_db(&self, id: usize) {
        // If `id` belongs to a nested block, save the containing flat block instead.
        let save_id = self.containing_flat_block_id(id).unwrap_or(id);
        if let Some(block) = self.blocks.iter().find(|b| b.id == save_id) {
            let should_persist = !matches!(block.kind, AgentBlockKind::Loading { .. });
            if should_persist {
                if let Some(ref sender) = self.persistence_sender {
                    if let Ok(kind_json) = serde_json::to_string(&block.kind) {
                        let persisted = dragent_persistence::PersistedAgentBlock {
                            kind_json,
                            conversation_id: block.conversation_id,
                            created_at_unix_ms: block.created_at_unix_ms,
                        };
                        let sort_order = save_id as i32;
                        let _ = sender.send(dragent_persistence::ModelEvent::SaveAgentBlock {
                            pane_uuid: self.pane_uuid,
                            block: persisted,
                            sort_order,
                            project_root: Some(self.cwd.clone()),
                        });
                    }
                }
            }
        }
    }

    pub(crate) fn remove_loading_block(&mut self) {
        self.loading_block_id = None;
        self.blocks
            .retain(|b| !matches!(b.kind, AgentBlockKind::Loading { .. }));
        // Also remove Loading from active TurnGroup children
        for b in &mut self.blocks {
            if let AgentBlockKind::TurnGroup { children, .. } = &mut b.kind {
                children.retain(|c| !matches!(c.kind, AgentBlockKind::Loading { .. }));
            }
        }
    }

    /// Re-add a Loading block for the active conversation (e.g. after a PTY
    /// run_command finishes and we're waiting for the next LLM response).
    pub fn push_loading_block(&mut self, sink: &mut dyn EventSink) {
        if let Some(conv_id) = self.active_conversation_id {
            self.remove_loading_block();
            let loading_id = self.push_block(
                AgentBlockKind::Loading {
                    message: "Working…".to_string(),
                },
                conv_id,
            );
            self.loading_block_id = Some(loading_id);
            sink.push(CoreEvent::BlocksChanged);
        }
    }

    /// The agent's command now runs in the terminal, as a shell block below
    /// this turn's block: close the turn block (see
    /// [`close_turn_block`](Self::close_turn_block)).
    pub fn hand_turn_to_terminal(&mut self, sink: &mut dyn EventSink) {
        if self.close_turn_block() {
            sink.push(CoreEvent::BlocksChanged);
        }
    }

    /// Close the open turn block before a block the turn then waits on (the
    /// agent's shell command, a question card), so it stops showing as
    /// working above it. [`continue_turn_below`](Self::continue_turn_below)
    /// opens the next turn block under it. Returns whether a block closed.
    pub(crate) fn close_turn_block(&mut self) -> bool {
        let Some(pos) = self.blocks.iter().rposition(|b| {
            matches!(
                b.kind,
                AgentBlockKind::TurnGroup {
                    duration_ms: None,
                    ..
                }
            )
        }) else {
            return false;
        };
        self.remove_loading_block();
        if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[pos].kind {
            for child in children {
                match &mut child.kind {
                    AgentBlockKind::AgentMessage { is_streaming, .. } => *is_streaming = false,
                    AgentBlockKind::ThinkingStep { complete, .. } => *complete = true,
                    _ => {}
                }
            }
        }
        // The next text starts a new message in the next turn block.
        self.thinking_block_id = None;
        self.reset_streaming_on_next_text = true;
        self.close_open_turn(None);
        true
    }

    /// The agent's terminal command finished, or its question was answered:
    /// carry on in a new turn block, so what the agent does next shows under
    /// that block.
    pub fn continue_turn_below(&mut self, sink: &mut dyn EventSink) {
        // Stopped in the meantime.
        if matches!(self.state, crate::state::ConversationState::Idle) {
            return;
        }
        let Some(conv_id) = self.active_conversation_id else {
            return;
        };
        let open = self.blocks.iter().any(|b| {
            matches!(
                b.kind,
                AgentBlockKind::TurnGroup {
                    duration_ms: None,
                    ..
                }
            )
        });
        if open {
            self.push_loading_block(sink);
            return;
        }
        self.remove_loading_block();
        self.open_turn_blocks(conv_id);
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Update the Loading block message if one is currently shown.
    /// The in-progress `AgentMessage` of the open turn (TurnGroup children
    /// first, then the flat list) — the block streamed text lands in.
    pub(crate) fn find_streaming_message_mut(&mut self) -> Option<&mut AgentBlock> {
        let tg_pos = self.blocks.iter().rposition(|b| {
            matches!(
                b.kind,
                AgentBlockKind::TurnGroup {
                    duration_ms: None,
                    ..
                }
            )
        });
        let is_streaming_msg = |b: &AgentBlock| {
            matches!(
                b.kind,
                AgentBlockKind::AgentMessage {
                    is_streaming: true,
                    ..
                }
            )
        };
        let in_group = tg_pos.is_some_and(|pos| {
            matches!(&self.blocks[pos].kind, AgentBlockKind::TurnGroup { children, .. }
                if children.iter().any(is_streaming_msg))
        });
        if in_group {
            let pos = tg_pos.unwrap();
            if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[pos].kind {
                return children.iter_mut().rev().find(|b| is_streaming_msg(b));
            }
            return None;
        }
        self.blocks.iter_mut().rev().find(|b| is_streaming_msg(b))
    }

    pub(crate) fn set_loading_message(&mut self, message: &str) {
        if let Some(id) = self.loading_block_id {
            if let Some(block) = self.find_block_mut_by_id(id) {
                if let AgentBlockKind::Loading { message: msg } = &mut block.kind {
                    *msg = message.to_string();
                }
            }
        }
    }

    pub(crate) fn set_approval_status(
        &mut self,
        action_id: Uuid,
        status: ApprovalStatus,
    ) -> Option<String> {
        let mut block_to_update = None;
        let mut result_cmd = None;
        let mut approval_kind = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::ApprovalRequest {
                action_id: bid,
                command,
                status: s,
                ..
            } = &mut block.kind
            {
                if *bid == action_id {
                    let cmd = command.clone();
                    *s = status;
                    approval_kind = Some(block.kind.clone());

                    // Convert the block to Empty
                    block.kind = AgentBlockKind::Empty;
                    block_to_update = Some(block.id);
                    result_cmd = Some(cmd);
                    break;
                }
            }
        }

        if let Some(bid) = block_to_update {
            self.update_block_in_db(bid);
        }

        // Push the resolved approval into ActivityGroup inside TurnGroup (or flat ActivityGroup)
        if let Some(kind) = approval_kind {
            if let Some(conv_id) = self.active_conversation_id {
                self.push_turn_block(kind, false, conv_id);
            }
        }

        result_cmd
    }
}
