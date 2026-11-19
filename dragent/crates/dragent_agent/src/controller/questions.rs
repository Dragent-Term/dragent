//! Agent questions, plans, and todos.
//!
//! ask_user flows, plan approval, todo tools, and their parsers.
//! Blocks here are impl AgentController + free fns: same behavior.
//!
//! Moved verbatim from `app/src/agent/controller/questions.rs` apart from
//! the engine seams: `ctx.emit` → `sink.push`, `ctx.notify` dropped,
//! `crate::bg::detach` → `crate::runtime::spawn_background`,
//! `pipeline::post_tool_result` → `self.deps.pipeline.post_tool_result`
//! via an `Arc` clone into the background closure, and
//! `super::block/state/types` → `crate::block/state/types`.

use std::sync::Arc;

use uuid::Uuid;

use crate::block::{AgentBlock, AgentBlockKind, PlanStatus, QuestionStatus};
use crate::runtime::{CoreEvent, EventSink, spawn_background};
use crate::state::ConversationState;
use crate::types::PendingToolCall;

use super::session_helpers::QueryOrigin;

pub use super::question_parse::{
    apply_option_pick, apply_text_answer, build_question_result_json, clamp_question_cursor,
    normalize_todo_priority, normalize_todo_status, parse_todo_list, parse_user_questions,
    resolve_typed_answer, summarize_question_answers,
};

impl super::core::AgentController {
    /// Parse an `ask_user` call and show the question card, parking the stream
    /// until the user answers. Always returns true (the frame changed). A
    /// malformed payload is turned into an error tool-result so the model can
    /// recover instead of the stream hanging on the server's result gate.
    pub(crate) fn begin_user_question(
        &mut self,
        call_id: String,
        args_json: &str,
        conv_id: Uuid,
    ) -> bool {
        let task_id = self.streaming_task_id.clone().unwrap_or_default();
        let call_id_for_event = call_id.clone();
        let questions = match parse_user_questions(args_json) {
            Ok(q) if !q.is_empty() => q,
            _ => {
                let call_id_e = call_id.clone();
                let pipeline = Arc::clone(&self.deps.pipeline);
                spawn_background(Box::new(move || {
                    if let Err(e) = pipeline.post_tool_result(
                        &task_id,
                        &call_id_e,
                        "ask_user needs a non-empty `questions` array; each question needs \
                         `question` text and 2+ `options`. Fix the arguments and retry, or \
                         proceed without asking.",
                        true,
                    ) {
                        log::warn!("[agent-stream] ask_user reject {call_id_e}: {e}");
                    }
                }));
                return true;
            }
        };

        // Close any open thinking step so the card is the focused element.
        if let Some(bid) = self.thinking_block_id.take() {
            if let Some(block) = self.find_block_mut_by_id(bid) {
                if let AgentBlockKind::ThinkingStep { complete, .. } = &mut block.kind {
                    *complete = true;
                }
            }
        }
        self.remove_loading_block();
        // The turn waits on the card: stop showing it as working above the
        // card, and carry on under the card once it's answered.
        self.close_turn_block();

        let action_id = Uuid::new_v4();
        let answers = vec![Vec::new(); questions.len()];
        self.pending_tool_calls.insert(
            action_id,
            PendingToolCall {
                call_id,
                tool_name: "ask_user".to_string(),
                args_json: args_json.to_string(),
                run_in_terminal: false,
                allow_terminal_cwd_change: false,
                external_read_roots: Vec::new(),
            },
        );
        let summary = questions
            .iter()
            .map(|q| q.question.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        self.deferred_events.push(CoreEvent::QuestionOpened {
            action_id,
            call_id: call_id_for_event,
            summary,
        });
        self.push_block(
            AgentBlockKind::UserQuestion {
                action_id,
                questions,
                current: 0,
                answers,
                status: QuestionStatus::Pending,
                cursor: 0,
            },
            conv_id,
        );
        self.state = ConversationState::AwaitingUserChoice { action_id };
        true
    }

    /// A user clicked an option. In single-select this records the answer and
    /// advances (finalizing after the last question); in multi-select it just
    /// toggles the option and waits for an explicit Continue.
    pub fn pick_user_question_option(
        &mut self,
        action_id: Uuid,
        option: String,
        sink: &mut dyn EventSink,
    ) {
        let mut block_id = None;
        let mut single_select_advance = false;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                answers,
                status,
                ..
            } = &mut block.kind
            {
                if *bid != action_id || *status != QuestionStatus::Pending {
                    continue;
                }
                let Some(q) = questions.get(*current) else {
                    break;
                };
                let Some(slot) = answers.get_mut(*current) else {
                    break;
                };
                single_select_advance = apply_option_pick(q.multi_select, slot, &option);
                block_id = Some(block.id);
                break;
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
            sink.push(CoreEvent::BlocksChanged);
        }
        if single_select_advance {
            self.advance_or_finalize_question(action_id, sink);
        }
    }

    /// Multi-select "Continue": commit the current question's toggles and move
    /// on (or finalize if it was the last).
    pub fn confirm_user_question(&mut self, action_id: Uuid, sink: &mut dyn EventSink) {
        self.advance_or_finalize_question(action_id, sink);
    }

    /// The user typed a custom answer (the "Other" free-text field). Single
    /// select uses it as THE answer and advances; multi-select adds it to the
    /// current selections (deduped) and stays so they can pick more or Continue.
    ///
    /// "1"/"2." style numbers and option-label text resolve to the matching
    /// option first — users routinely type the option NUMBER here, and the raw
    /// digit is meaningless to the model (the result JSON carries labels, not
    /// numbering), so an unresolved "1" reads as a nonsense answer.
    pub fn answer_user_question_text(
        &mut self,
        action_id: Uuid,
        text: String,
        sink: &mut dyn EventSink,
    ) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let mut block_id = None;
        let mut single_select_advance = false;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                answers,
                status,
                ..
            } = &mut block.kind
            {
                if *bid != action_id || *status != QuestionStatus::Pending {
                    continue;
                }
                let Some(q) = questions.get(*current) else {
                    break;
                };
                let Some(slot) = answers.get_mut(*current) else {
                    break;
                };
                let picks = resolve_typed_answer(&text, &q.options);
                if q.multi_select {
                    for p in &picks {
                        apply_text_answer(true, slot, p);
                    }
                    single_select_advance = false;
                } else {
                    let answer = picks.into_iter().next().unwrap_or(text.clone());
                    single_select_advance = apply_text_answer(false, slot, &answer);
                }
                block_id = Some(block.id);
                break;
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
            sink.push(CoreEvent::BlocksChanged);
        }
        if single_select_advance {
            self.advance_or_finalize_question(action_id, sink);
        }
    }

    /// The user typed a reply in the MAIN input bar while a question card was
    /// open. Interpret it as the answer to the CURRENT question and commit it:
    /// "1"/"2."-style numbers (comma-separated allowed for multi-select) and
    /// exact option labels select options; anything else is a free-text answer.
    /// Unlike the card's own "Other" field, a typed submission always advances
    /// — pressing Enter in the input reads as "this is my answer, go".
    pub fn answer_user_question_from_input(
        &mut self,
        action_id: Uuid,
        text: String,
        sink: &mut dyn EventSink,
    ) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let mut block_id = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                answers,
                status,
                ..
            } = &mut block.kind
            {
                if *bid != action_id || *status != QuestionStatus::Pending {
                    continue;
                }
                let Some(q) = questions.get(*current) else {
                    break;
                };
                let Some(slot) = answers.get_mut(*current) else {
                    break;
                };
                let mut picks = resolve_typed_answer(&text, &q.options);
                if !q.multi_select && picks.len() > 1 {
                    picks.truncate(1);
                }
                *slot = picks;
                block_id = Some(block.id);
                break;
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
            sink.push(CoreEvent::BlocksChanged);
            self.advance_or_finalize_question(action_id, sink);
        }
    }

    /// Option label under the keyboard cursor, for Alt+Enter to pick.
    pub fn question_cursor_pick(&self, action_id: Uuid) -> Option<String> {
        self.blocks.iter().find_map(|b| match &b.kind {
            AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                cursor,
                status,
                ..
            } if *bid == action_id && *status == QuestionStatus::Pending => questions
                .get(*current)?
                .options
                .get(*cursor)
                .map(|o| o.label.clone()),
            _ => None,
        })
    }

    /// Option label by 0-based index, for Alt+1..9 quick-pick.
    pub fn question_digit_pick(&self, action_id: Uuid, index: usize) -> Option<String> {
        self.blocks.iter().find_map(|b| match &b.kind {
            AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                status,
                ..
            } if *bid == action_id && *status == QuestionStatus::Pending => questions
                .get(*current)?
                .options
                .get(index)
                .map(|o| o.label.clone()),
            _ => None,
        })
    }

    /// The `action_id` of the currently-open question card, if any.
    pub fn first_pending_question_id(&self) -> Option<Uuid> {
        self.blocks.iter().find_map(|b| match &b.kind {
            AgentBlockKind::UserQuestion {
                action_id,
                status: QuestionStatus::Pending,
                ..
            } => Some(*action_id),
            _ => None,
        })
    }

    /// Move the keyboard cursor over the current question's options, clamped
    /// to the option range. Render highlights the cursor row.
    pub fn move_question_cursor(&mut self, action_id: Uuid, delta: i32, sink: &mut dyn EventSink) {
        let mut block_id = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                cursor,
                status,
                ..
            } = &mut block.kind
            {
                if *bid != action_id || *status != QuestionStatus::Pending {
                    continue;
                }
                let len = questions
                    .get(*current)
                    .map(|q| q.options.len())
                    .unwrap_or(0);
                *cursor = clamp_question_cursor(*cursor, delta, len);
                block_id = Some(block.id);
                break;
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
            sink.push(CoreEvent::BlocksChanged);
        }
    }

    /// Advance to the next question, or finalize when the last is answered.
    pub(crate) fn advance_or_finalize_question(
        &mut self,
        action_id: Uuid,
        sink: &mut dyn EventSink,
    ) {
        let mut block_id = None;
        let mut finished = false;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                current,
                cursor,
                status,
                ..
            } = &mut block.kind
            {
                if *bid != action_id || *status != QuestionStatus::Pending {
                    continue;
                }
                if *current + 1 < questions.len() {
                    *current += 1;
                    // Fresh question, fresh cursor position.
                    *cursor = 0;
                } else {
                    finished = true;
                }
                block_id = Some(block.id);
                break;
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
            sink.push(CoreEvent::BlocksChanged);
        }
        if finished {
            self.finalize_user_question(action_id, sink);
        }
    }

    /// All questions answered: mark the card done, POST the combined answers
    /// back to the agent as the tool result, and resume streaming.
    pub(crate) fn finalize_user_question(&mut self, action_id: Uuid, sink: &mut dyn EventSink) {
        let Some(pending) = self.pending_tool_calls.remove(&action_id) else {
            return;
        };
        let Some(task_id) = self.streaming_task_id.clone() else {
            return;
        };

        let mut result_json = None;
        let mut block_id = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                action_id: bid,
                questions,
                answers,
                status,
                ..
            } = &mut block.kind
            {
                if *bid != action_id {
                    continue;
                }
                *status = QuestionStatus::Answered;
                result_json = Some(build_question_result_json(questions, answers));
                block_id = Some(block.id);
                break;
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
        }

        // Resume: the server's result gate is still blocking this call_id.
        self.state = ConversationState::Streaming;
        self.continue_turn_below(sink);

        let call_id = pending.call_id;
        let output = result_json.unwrap_or_else(|| "{\"answers\":[]}".to_string());
        self.deferred_events
            .push(CoreEvent::QuestionAnswered { action_id });
        // Record the decision on the turn's tool log so embedded hosts can
        // fold "asked → answered" into their own memory digest.
        if let Some(call) = self
            .turn_tool_calls
            .iter_mut()
            .rev()
            .find(|c| c.call_id == call_id)
        {
            call.result_summary = Some(summarize_question_answers(&output));
        }
        let pipeline = Arc::clone(&self.deps.pipeline);
        spawn_background(Box::new(move || {
            if let Err(e) = pipeline.post_tool_result(&task_id, &call_id, &output, false) {
                log::warn!("[agent-stream] ask_user result {call_id}: {e}");
            }
        }));
    }

    /// Apply a `todowrite` call: render the task list inline inside the
    /// active TurnGroup — just like the agent's continuous response — updating
    /// the live card in place so progress reads chronologically. Falls back
    /// to a top-level card only when no turn is active. Persists the change
    /// and posts the JSON list back as the tool result so the model
    /// continues. A malformed payload posts an error result so the model can
    /// recover. Always returns true (the frame changed).
    pub(crate) fn apply_todo_write(
        &mut self,
        call_id: String,
        args_json: &str,
        conv_id: Uuid,
    ) -> bool {
        let task_id = self.streaming_task_id.clone().unwrap_or_default();
        let todos = match parse_todo_list(args_json) {
            Ok(t) => t,
            Err(e) => {
                let call_id_e = call_id.clone();
                let pipeline = Arc::clone(&self.deps.pipeline);
                spawn_background(Box::new(move || {
                    if let Err(e) = pipeline.post_tool_result(
                        &task_id,
                        &call_id_e,
                        &format!("todowrite payload invalid ({e}); send {{\"todos\":[...]}} and retry, or proceed without a task list."),
                        true,
                    ) {
                        log::warn!("[agent-stream] todowrite reject {call_id_e}: {e}");
                    }
                }));
                return true;
            }
        };
        let output =
            serde_json::to_string_pretty(&todos).unwrap_or_else(|_| "{\"todos\":[]}".to_string());

        // Route into the active TurnGroup so the list lives with the working
        // agent's continuous response instead of sticking as a separate card
        // at the bottom. Update the live card in place — one list per turn.
        if let Some(tg_pos) = self.blocks.iter().rposition(|b| {
            matches!(
                b.kind,
                AgentBlockKind::TurnGroup {
                    duration_ms: None,
                    ..
                }
            )
        }) {
            self.remove_loading_block();
            let tg_id = self.blocks[tg_pos].id;
            let mut updated = false;
            if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[tg_pos].kind {
                if let Some(existing) = children
                    .iter_mut()
                    .rev()
                    .find(|c| matches!(c.kind, AgentBlockKind::TodoList { .. }))
                {
                    existing.kind = AgentBlockKind::TodoList {
                        todos: todos.clone(),
                    };
                    updated = true;
                }
            }
            if !updated {
                let id = self.next_block_id;
                self.next_block_id += 1;
                let block = AgentBlock::new(
                    id,
                    conv_id,
                    AgentBlockKind::TodoList {
                        todos: todos.clone(),
                    },
                );
                if let AgentBlockKind::TurnGroup { children, .. } = &mut self.blocks[tg_pos].kind {
                    children.push(block);
                }
            }
            self.update_block_in_db(tg_id);
        } else {
            self.push_block(AgentBlockKind::TodoList { todos }, conv_id);
        }

        let pipeline = Arc::clone(&self.deps.pipeline);
        spawn_background(Box::new(move || {
            if let Err(e) = pipeline.post_tool_result(&task_id, &call_id, &output, false) {
                log::warn!("[agent-stream] todowrite result {call_id}: {e}");
            }
        }));
        true
    }

    /// Mark any open question as dismissed (used when a turn is stopped/cleared
    /// before it was answered) so its card no longer looks interactive. The
    /// server task is being torn down, so no tool result is posted.
    pub fn dismiss_open_question(&mut self) {
        let mut block_id = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::UserQuestion {
                status, action_id, ..
            } = &mut block.kind
            {
                if *status == QuestionStatus::Pending {
                    *status = QuestionStatus::Dismissed;
                    self.pending_tool_calls.remove(&*action_id);
                    block_id = Some(block.id);
                    break;
                }
            }
        }
        if let Some(bid) = block_id {
            self.update_block_in_db(bid);
        }
    }

    /// User clicked "Implement" on a PlanReview block.
    /// Updates the block status and starts a new streaming task telling the LLM to proceed.
    pub fn approve_plan(
        &mut self,
        plan_id: Uuid,
        sink: &mut dyn EventSink,
    ) -> Option<super::run::LaunchedTurn> {
        // Mark the block approved in the UI.
        let mut block_to_update = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::PlanReview {
                plan_id: bid,
                status,
                ..
            } = &mut block.kind
            {
                if *bid == plan_id {
                    *status = PlanStatus::Approved;
                    block_to_update = Some(block.id);
                    break;
                }
            }
        }
        if let Some(bid) = block_to_update {
            self.update_block_in_db(bid);
        }
        self.pending_plans.remove(&plan_id);
        sink.push(CoreEvent::BlocksChanged);

        // Start implementation. Reuse the saved model + system prompt so the
        // LLM gets the same project context it had when it wrote the plan.
        // gate_tools=false: the user already approved the plan so tools should run freely.
        let model = self.last_model_name.clone();
        let system_arc = self.last_system_arc.clone();
        // The launch propagates to the owner for polling — dropping it here
        // stranded the implementation turn with nobody reading its stream.
        self.start_streaming_query_impl(
            "Approved. Please implement the plan now.".to_string(),
            model,
            // The arc is always populated by the time the user can click Implement
            // (the agent had to respond with a plan first).
            Box::new(move || system_arc.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            false,
            QueryOrigin::User,
            None,
            sink,
        )
    }

    /// User clicked "Refine" on a PlanReview block.
    /// Marks the block and returns focus to the input so the user can type a refinement.
    /// The session history already contains the plan, so the next query the user
    /// types will be seen by the LLM in context of the full plan.
    pub fn mark_plan_refining(&mut self, plan_id: Uuid, sink: &mut dyn EventSink) {
        let mut block_to_update = None;
        for block in &mut self.blocks {
            if let AgentBlockKind::PlanReview {
                plan_id: bid,
                status,
                ..
            } = &mut block.kind
            {
                if *bid == plan_id {
                    *status = PlanStatus::Refining;
                    block_to_update = Some(block.id);
                    break;
                }
            }
        }
        if let Some(bid) = block_to_update {
            self.update_block_in_db(bid);
        }
        self.pending_plans.remove(&plan_id);
        sink.push(CoreEvent::BlocksChanged);
        // No new task started — the user will type their refinement normally.
    }
}
