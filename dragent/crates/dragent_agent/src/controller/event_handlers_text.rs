//! Stream event handlers (text/done/error).
//!
//! One method per PipelineEvent kind, moved verbatim from the app's
//! `apply_stream_event`; same behavior. Seam rewrites only:
//! `AgentControllerEvent` -> [`CoreEvent`][crate::runtime::CoreEvent],
//! memory extraction via `deps.hooks`, background spawn via
//! [`spawn_background`][crate::runtime::spawn_background].
//! (`display_text` / `close_open_turn` live in the session/submit home
//! modules, not here.)

use std::sync::Arc;

use uuid::Uuid;

use super::event_helpers::sanitize_server_error;
use crate::block::{AgentBlock, AgentBlockKind, PlanStatus};
use crate::pipeline::PipelineEvent;
use crate::runtime::CoreEvent;
use crate::state::ConversationState;
use crate::types::SessionMessage;

impl super::core::AgentController {
    pub(crate) fn apply_thinking_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        let text = ev.text.unwrap_or_default();
        if text.is_empty() {
            return false;
        }
        self.set_loading_message("Thinking…");
        self.remove_loading_block();
        let mut updated = false;
        if let Some(bid) = self.thinking_block_id {
            // Update in-place (search flat list + TurnGroup children)
            if let Some(block) = self.find_block_mut_by_id(bid) {
                if let AgentBlockKind::ThinkingStep { content, .. } = &mut block.kind {
                    content.push_str(&text);
                    updated = true;
                }
            }
            if updated {
                return true;
            }
        }
        // First thinking delta: create new ThinkingStep directly in TurnGroup
        let id = self.push_turn_block(
            AgentBlockKind::ThinkingStep {
                content: text,
                complete: false,
                expanded: false,
            },
            true,
            conv_id,
        );
        self.thinking_block_id = Some(id);
        true
    }
    pub(crate) fn apply_text_delta_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        let text = ev.text.unwrap_or_default();
        if text.is_empty() {
            return false;
        }
        self.remove_loading_block();

        // If a tool round just completed, finalize the previous message block
        // and start accumulating fresh text for the next round.
        if self.reset_streaming_on_next_text {
            self.reset_streaming_on_next_text = false;
            self.streaming_text.clear();
            // Mark any in-progress message block as complete so the shimmer clears.
            // Messages now live in TurnGroup children; search there first.
            let tg_pos = self.blocks.iter().rposition(|b| {
                matches!(
                    b.kind,
                    AgentBlockKind::TurnGroup {
                        duration_ms: None,
                        ..
                    }
                )
            });
            let mut cleared = false;
            if let Some(pos) = tg_pos {
                if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[pos].kind {
                    if let Some(block) = children.iter_mut().rev().find(|c| {
                        matches!(
                            c.kind,
                            AgentBlockKind::AgentMessage {
                                is_streaming: true,
                                ..
                            }
                        )
                    }) {
                        if let AgentBlockKind::AgentMessage { is_streaming, .. } = &mut block.kind {
                            *is_streaming = false;
                            cleared = true;
                        }
                    }
                }
            }
            if !cleared {
                if let Some(block) = self.blocks.iter_mut().rev().find(|b| {
                    matches!(
                        b.kind,
                        AgentBlockKind::AgentMessage {
                            is_streaming: true,
                            ..
                        }
                    )
                }) {
                    if let AgentBlockKind::AgentMessage { is_streaming, .. } = &mut block.kind {
                        *is_streaming = false;
                    }
                }
            }
        }

        self.streaming_text.push_str(&text);
        let text_clone = self.display_text(&self.streaming_text, true);

        // Find or create the streaming AgentMessage.
        // While a TurnGroup is active, messages live in its children so they stay
        // inside the collapsible "Worked for X" block. The final summary is extracted
        // and promoted to top-level when the turn completes (Done handler below).
        let tg_pos = self.blocks.iter().rposition(|b| {
            matches!(
                b.kind,
                AgentBlockKind::TurnGroup {
                    duration_ms: None,
                    ..
                }
            )
        });

        let last_is_message = if let Some(pos) = tg_pos {
            if let AgentBlockKind::TurnGroup { children, .. } = &self.blocks[pos].kind {
                children
                    .last()
                    .map(|c| {
                        matches!(
                            c.kind,
                            AgentBlockKind::AgentMessage {
                                is_streaming: true,
                                ..
                            }
                        )
                    })
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            self.blocks
                .last()
                .map(|b| {
                    matches!(
                        b.kind,
                        AgentBlockKind::AgentMessage {
                            is_streaming: true,
                            ..
                        }
                    )
                })
                .unwrap_or(false)
        };

        let mut bid_to_update = None;
        if last_is_message {
            if let Some(pos) = tg_pos {
                if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[pos].kind {
                    if let Some(block) = children.last_mut() {
                        let bid = block.id;
                        if let AgentBlockKind::AgentMessage { text, .. } = &mut block.kind {
                            *text = text_clone;
                        }
                        bid_to_update = Some(bid);
                    }
                }
            } else if let Some(block) = self.blocks.last_mut() {
                let bid = block.id;
                if let AgentBlockKind::AgentMessage { text, .. } = &mut block.kind {
                    *text = text_clone;
                }
                bid_to_update = Some(bid);
            }
        } else if let Some(pos) = tg_pos {
            // Route into TurnGroup children — hidden until the turn collapses.
            let id = self.next_block_id;
            self.next_block_id += 1;
            let block = AgentBlock::new(
                id,
                conv_id,
                AgentBlockKind::AgentMessage {
                    text: text_clone,
                    is_streaming: true,
                },
            );
            if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[pos].kind {
                children.push(block);
            }
        } else {
            self.push_block(
                AgentBlockKind::AgentMessage {
                    text: text_clone,
                    is_streaming: true,
                },
                conv_id,
            );
        }
        if let Some(bid) = bid_to_update {
            self.update_block_in_db(bid);
        }
        true
    }
    pub(crate) fn apply_done_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        self.remove_loading_block();
        const PLAN_MARKER: &str = "<!-- plan_ready -->";

        // Hosts that parse structured output get the raw text; the
        // block shows the filter's final (non-streaming) rendering.
        if self.display_text_filter.is_some() && !self.streaming_text.is_empty() {
            let final_text = self.display_text(&self.streaming_text, false);
            if let Some(block) = self.find_streaming_message_mut() {
                if let AgentBlockKind::AgentMessage { text, .. } = &mut block.kind {
                    *text = final_text;
                }
            }
        }
        self.deferred_events.push(CoreEvent::TurnCompleted {
            conversation_id: conv_id,
            assistant_text: self.streaming_text.clone(),
            model_name: self.last_model_name.clone(),
            tool_calls: self.turn_tool_calls.clone(),
        });

        // Track token usage. Session totals ACCUMULATE across turns;
        // current context is a SNAPSHOT of the last turn's prompt size.
        if let Some(n) = ev.total_input_tokens {
            self.session_input_tokens += n.max(0) as u64;
        }
        if let Some(n) = ev.total_cached_input_tokens {
            self.session_cached_input_tokens += n.max(0) as u64;
        }
        if let Some(n) = ev.total_output_tokens {
            self.session_output_tokens += n.max(0) as u64;
        }
        if let Some(n) = ev.context_tokens {
            self.current_context_tokens = n.max(0) as u64;
        }

        // Check if the LLM ended its response with the plan marker,
        // meaning it wants approval before executing any tools.
        let has_plan = self.streaming_text.contains(PLAN_MARKER);
        let plan_content = if has_plan {
            // Strip the marker from what gets stored/shown.
            self.streaming_text
                .replace(PLAN_MARKER, "")
                .trim()
                .to_string()
        } else {
            self.streaming_text.clone()
        };

        // Append to session history (without the marker).
        if !plan_content.is_empty() {
            self.session_history.push(SessionMessage {
                role: "assistant".to_string(),
                content: plan_content.clone(),
                attachments: Vec::new(),
            });
            // Write assistant reply to transcript AFTER Done so the entry
            // is complete rather than partial.
            if let Some(store) = &self.session_store {
                store.append(&crate::session_store::TranscriptEntry::assistant(
                    &plan_content,
                ));
            }
        }

        // Mark the streaming message as complete so the shimmer clears.
        // The streaming AgentMessage may live in TurnGroup children.
        {
            let mut bid_done = None;
            let tg_pos = self.blocks.iter().rposition(|b| {
                matches!(
                    b.kind,
                    AgentBlockKind::TurnGroup {
                        duration_ms: None,
                        ..
                    }
                )
            });
            if let Some(pos) = tg_pos {
                if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[pos].kind {
                    if let Some(block) = children.iter_mut().rev().find(|c| {
                        matches!(
                            c.kind,
                            AgentBlockKind::AgentMessage {
                                is_streaming: true,
                                ..
                            }
                        )
                    }) {
                        if let AgentBlockKind::AgentMessage { is_streaming, .. } = &mut block.kind {
                            *is_streaming = false;
                        }
                        bid_done = Some(block.id);
                    }
                }
            }
            if bid_done.is_none() {
                if let Some(block) = self.blocks.iter_mut().rev().find(|b| {
                    matches!(
                        b.kind,
                        AgentBlockKind::AgentMessage {
                            is_streaming: true,
                            ..
                        }
                    )
                }) {
                    if let AgentBlockKind::AgentMessage { is_streaming, .. } = &mut block.kind {
                        *is_streaming = false;
                    }
                    bid_done = Some(block.id);
                }
            }
            if let Some(bid) = bid_done {
                self.update_block_in_db(bid);
            }
        }

        if has_plan {
            // Convert the last AgentMessage block to a PlanReview block
            // so the user sees Implement / Refine buttons.
            // The message may live in TurnGroup children — extract it to top-level so it's visible.
            let plan_id = Uuid::new_v4();
            self.pending_plans.insert(plan_id, plan_content.clone());
            let mut bid_to_update = None;
            let tg_pos_plan = self.blocks.iter().rposition(|b| {
                matches!(
                    b.kind,
                    AgentBlockKind::TurnGroup {
                        duration_ms: None,
                        ..
                    }
                )
            });
            if let Some(pos) = tg_pos_plan {
                let tg_block_id = self.blocks[pos].id;
                let msg_idx =
                    if let AgentBlockKind::TurnGroup { children, .. } = &self.blocks[pos].kind {
                        children
                            .iter()
                            .rposition(|c| matches!(c.kind, AgentBlockKind::AgentMessage { .. }))
                    } else {
                        None
                    };
                if let Some(idx) = msg_idx {
                    let extracted = if let AgentBlockKind::TurnGroup { children, .. } =
                        &mut self.blocks[pos].kind
                    {
                        Some(children.remove(idx))
                    } else {
                        None
                    };
                    if extracted.is_some() {
                        let pr_id = self.next_block_id;
                        self.next_block_id += 1;
                        let pr_kind = AgentBlockKind::PlanReview {
                            plan_id,
                            content: plan_content.clone(),
                            status: PlanStatus::Pending,
                        };
                        self.blocks
                            .push(AgentBlock::new(pr_id, conv_id, pr_kind.clone()));
                        self.save_block_in_db(pr_id, pr_kind, conv_id);
                        self.update_block_in_db(tg_block_id); // TurnGroup lost a child
                    }
                }
            } else if let Some(block) = self
                .blocks
                .iter_mut()
                .rev()
                .find(|b| matches!(b.kind, AgentBlockKind::AgentMessage { .. }))
            {
                let bid = block.id;
                block.kind = AgentBlockKind::PlanReview {
                    plan_id,
                    content: plan_content,
                    status: PlanStatus::Pending,
                };
                bid_to_update = Some(bid);
            }
            if let Some(bid) = bid_to_update {
                self.update_block_in_db(bid);
            }
            // Stay Idle — the user must click Implement before any tools run.
            self.state = ConversationState::Idle;
        } else {
            self.state = ConversationState::Idle;
        }

        // Close the active TurnGroup: set duration; completed normally.
        let tg_id_to_save = self.close_open_turn(None);
        if let Some(tg_id) = tg_id_to_save {
            // Extract the final AgentMessage from TurnGroup children and promote it
            // to a top-level block so it appears below the collapsed "Worked for X" header.
            let msg_idx = self
                .blocks
                .iter()
                .position(|b| b.id == tg_id)
                .and_then(|pos| {
                    if let AgentBlockKind::TurnGroup { children, .. } = &self.blocks[pos].kind {
                        children
                            .iter()
                            .rposition(|c| matches!(c.kind, AgentBlockKind::AgentMessage { .. }))
                            .map(|i| (pos, i))
                    } else {
                        None
                    }
                });
            if let Some((tg_pos, child_idx)) = msg_idx {
                let extracted = if let AgentBlockKind::TurnGroup { children, .. } =
                    &mut self.blocks[tg_pos].kind
                {
                    Some(children.remove(child_idx))
                } else {
                    None
                };
                if let Some(msg) = extracted {
                    let new_id = self.next_block_id;
                    self.next_block_id += 1;
                    let new_kind = msg.kind.clone();
                    self.blocks
                        .push(AgentBlock::new(new_id, conv_id, new_kind.clone()));
                    self.save_block_in_db(new_id, new_kind, conv_id);
                }
            }
            self.update_block_in_db(tg_id);
        }

        self.streaming_task_id = None;
        self.streaming_text.clear();
        self.reset_streaming_on_next_text = false;
        self.thinking_block_id = None;
        self.pending_tool_calls.clear();
        self.queued_approvals.clear();
        self.gate_tools_until_plan = false;

        // Auto-memory extraction. Fires when the session is deep enough
        // (the original heuristic) OR when the user's latest message was
        // an explicit instruction/correction — so a "how I want this done"
        // rule stated in a SHORT session is captured immediately instead
        // of being lost before the 20-message threshold is ever reached.
        let extract_tool_count = self.tool_call_count;
        self.tool_call_count = 0;
        let deep_enough = self.session_history.len() >= 20 && extract_tool_count >= 3;
        let latest_user_directive = self
            .session_history
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| self.deps.hooks.looks_like_directive(&m.content))
            .unwrap_or(false);
        let capture_directive = latest_user_directive && self.session_history.len() >= 2;
        if deep_enough || capture_directive {
            let model = self.last_model_name.clone();
            let history = self.session_history.clone();
            let cwd = self.cwd.clone();
            let hooks = Arc::clone(&self.deps.hooks);
            log::info!(
                "[memory-extract] triggering (deep={deep_enough}, directive={capture_directive})"
            );
            crate::runtime::spawn_background(Box::new(move || {
                hooks.try_extract_and_save(&model, &history, &cwd);
            }));
        }

        // ── Evidence gate ─────────────────────────────────────────────
        // The agent finished a real turn (not a plan) but edited files
        // without a *successful* build/test/lint/typecheck afterward —
        // the single most common "I fixed it" hallucination. Auto-
        // continue up to twice (so a failed check can re-nudge once)
        // with a system note to verify or explicitly admit it didn't.
        const MAX_EVIDENCE_NUDGES: u8 = 2;
        if !has_plan
            && self.evidence_nudge_count < MAX_EVIDENCE_NUDGES
            && crate::session_facts::edited_anything(self.pane_uuid)
            && !crate::session_facts::unverified_edits(self.pane_uuid).is_empty()
            && !crate::session_facts::verification_attempted(self.pane_uuid)
        {
            self.evidence_nudge_count += 1;
            let files = crate::session_facts::unverified_edits(self.pane_uuid);
            let names: Vec<String> = files
                .iter()
                .take(8)
                .map(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string_lossy().into_owned())
                })
                .collect();
            let note = format!(
                "[system] You edited {} file(s) ({}) but have no successful verification \
                     after the last edit (build/test/lint/typecheck with exit 0). Do not report \
                     the task as done on faith. Run the project-appropriate check now and show \
                     its actual output + exit code. If you cannot verify, say so plainly and why. \
                     Never claim something works without evidence.",
                files.len(),
                names.join(", "),
            );
            // apply_stream_event has no ModelContext; stash the note and
            // let poll_stream (which does) fire the continuation.
            log::info!(
                "[evidence-gate] nudging ({}/{}): {} unverified edit(s)",
                self.evidence_nudge_count,
                MAX_EVIDENCE_NUDGES,
                files.len()
            );
            self.pending_evidence_nudge = Some(note);
        }

        true
    }
    pub(crate) fn apply_error_event(&mut self, ev: PipelineEvent, conv_id: Uuid) -> bool {
        self.remove_loading_block();
        let _ = self.close_open_turn(Some("Failed"));
        let raw = ev.error.unwrap_or_else(|| "stream error".to_string());
        log::warn!("[agent-stream] server error: {raw}");
        let (message, kind) = sanitize_server_error(&raw);
        let failed_task_id = self.streaming_task_id.clone();
        self.state = ConversationState::Idle;
        self.streaming_task_id = None;
        self.pending_tool_calls.clear();
        self.queued_approvals.clear();
        self.push_block(
            AgentBlockKind::Error {
                message: message.clone(),
                kind,
                task_id: failed_task_id,
            },
            conv_id,
        );
        // Embedded hosts wait on a turn outcome: a server-side failure
        // ends the turn just like a transport error does.
        self.deferred_events.push(CoreEvent::Error { message });
        true
    }

    /// Presentation filter for streamed text (session-owned). Lives here
    /// ahead of the `session` module move.
    pub fn display_text(&self, raw: &str, is_streaming: bool) -> String {
        match &self.display_text_filter {
            Some(filter) => filter(raw, is_streaming),
            None => raw.to_string(),
        }
    }
}
