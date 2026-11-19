//! Engine stream-event dispatcher + pump-contract helpers.
//!
//! Moved from `app/src/agent/controller/events.rs`: only the `kind ->
//! handler` dispatcher (`apply_stream_event`) and the small sink-free /
//! sink-pushing turn-failure helpers the pump contract needs
//! (`handle_unexpected_stream_end`, `fail_stream_turn`,
//! `set_streaming_task_id`). The WarpUI pumps (`poll_stream`,
//! `poll_diagrams` — timers + viewer state) stay app-side; the owner drives
//! these through `drain_deferred` / `EventSink` instead of `ctx.emit`.

use uuid::Uuid;

use super::core::AgentController;
use crate::block::{AgentBlockKind, ErrorKind, ToolStepStatus};
use crate::pipeline::{AgentEventKind, PipelineEvent};
use crate::runtime::{CoreEvent, EventSink};
use crate::state::ConversationState;

impl AgentController {
    pub fn apply_stream_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        match ev.kind {
            // Consumed inside the desktop pipeline orchestrator. Keeping this
            // arm defensive prevents a protocol-level marker becoming visible
            // if a future transport forwards it by mistake.
            AgentEventKind::StepDone => false,
            AgentEventKind::Thinking => self.apply_thinking_event(ev, conv_id),
            AgentEventKind::ToolEnd => self.apply_tool_end_event(ev, conv_id),
            AgentEventKind::ToolCallStarted => self.apply_tool_call_started_event(ev, conv_id),
            AgentEventKind::ToolCall => self.apply_tool_call_event(ev, conv_id),
            AgentEventKind::TextDelta => self.apply_text_delta_event(ev, conv_id),
            AgentEventKind::Done => self.apply_done_event(ev, conv_id),
            AgentEventKind::Error => self.apply_error_event(ev, conv_id),
        }
    }
}

impl AgentController {
    /// Called when the SSE stream ends or times out while the agent is still active.
    /// Resets state to Idle, marks any stuck Running tool blocks as errored, removes
    /// the loading indicator, and pushes a user-visible error block.
    ///
    /// Sink-free: the `Error` event goes into `deferred_events` for the owner
    /// to drain via `drain_deferred` (the old `ctx.emit` sites in `poll_stream`).
    pub fn handle_unexpected_stream_end(&mut self, conv_id: Uuid) {
        let failed_task_id = self.streaming_task_id.clone();
        self.state = ConversationState::Idle;
        self.streaming_task_id = None;
        self.remove_loading_block();
        self.deferred_events.push(CoreEvent::Error {
            message: "The agent stream ended unexpectedly.".to_string(),
        });
        // The task is gone — an open ask_user card can no longer post its result,
        // so retire it (otherwise answering it would spin "Working…" forever).
        self.dismiss_open_question();

        // Collect IDs of all Running ToolSteps (flat list, TurnGroup children, ActivityGroup inside TurnGroup)
        let running_bids: Vec<usize> = {
            let mut bids = Vec::new();
            for b in &self.blocks {
                if let AgentBlockKind::ToolStep {
                    status: ToolStepStatus::Running,
                    ..
                } = &b.kind
                {
                    bids.push(b.id);
                }
                if let AgentBlockKind::TurnGroup { children, .. } = &b.kind {
                    for c in children {
                        if let AgentBlockKind::ToolStep {
                            status: ToolStepStatus::Running,
                            ..
                        } = &c.kind
                        {
                            bids.push(c.id);
                        }
                        if let AgentBlockKind::ActivityGroup {
                            blocks: ag_blocks, ..
                        } = &c.kind
                        {
                            for ab in ag_blocks {
                                if let AgentBlockKind::ToolStep {
                                    status: ToolStepStatus::Running,
                                    ..
                                } = &ab.kind
                                {
                                    bids.push(ab.id);
                                }
                            }
                        }
                    }
                }
                if let AgentBlockKind::ActivityGroup {
                    blocks: ag_blocks, ..
                } = &b.kind
                {
                    for ab in ag_blocks {
                        if let AgentBlockKind::ToolStep {
                            status: ToolStepStatus::Running,
                            ..
                        } = &ab.kind
                        {
                            bids.push(ab.id);
                        }
                    }
                }
            }
            bids
        };
        for bid in running_bids {
            if let Some(block) = self.find_block_mut_by_id(bid) {
                if let AgentBlockKind::ToolStep { status, .. } = &mut block.kind {
                    *status = ToolStepStatus::Error;
                }
            }
        }

        // Close active TurnGroup — this path pushes an Error block next.
        self.close_open_turn(Some("Failed"));

        self.pending_tool_calls.clear();
        self.queued_approvals.clear();
        self.reset_streaming_on_next_text = false;

        self.push_block(
            AgentBlockKind::Error {
                message: "Connection to the agent service was lost. Please try again.".to_string(),
                kind: ErrorKind::Network,
                task_id: failed_task_id,
            },
            conv_id,
        );
    }
}

impl AgentController {
    /// Fail the active turn with a user-visible error block. Idles the turn,
    /// retires any open `ask_user` card (its task is dead), closes the turn,
    /// and notifies the owner through the sink.
    pub fn fail_stream_turn(
        &mut self,
        message: String,
        kind: ErrorKind,
        conv_id: Uuid,
        sink: &mut dyn EventSink,
    ) {
        let failed_task_id = self.streaming_task_id.clone();
        self.state = ConversationState::Idle;
        self.streaming_task_id = None;
        self.remove_loading_block();
        // The task is dead — an open ask_user card can no longer post its
        // result, so retire it (otherwise answering it hangs on "Working…").
        self.dismiss_open_question();
        let _ = self.close_open_turn(Some("Failed"));
        self.push_block(
            AgentBlockKind::Error {
                message,
                kind,
                task_id: failed_task_id,
            },
            conv_id,
        );
        sink.push(CoreEvent::BlocksChanged);
    }

    /// Record the in-flight provider task id carried on the stream's
    /// `__task_id__` sentinel. The owner parses the sentinel app-side and
    /// hands the id here so error cards keep their task attribution.
    pub fn set_streaming_task_id(&mut self, task_id: String) {
        self.streaming_task_id = Some(task_id);
    }
}
