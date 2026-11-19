//! The terminal's agent half as a standalone, embeddable view.
//!
//! `TerminalView` renders its `AgentController`'s blocks with
//! `build_agent_block_element` and forwards the agent `TerminalAction`s
//! (approve / pick option / toggle …) back into the controller. Any other
//! surface that wants the SAME agent UI — streaming text, thinking, tool
//! steps, ask_user cards, TodoList, receipts, error cards, SQLite-backed
//! persistence — embeds this view instead of re-drawing those cards.
//!
//! What differs per host is injected, not re-implemented: the system prompt
//! (`start_streaming_query`'s builder), the tool allowlist
//! (`AgentController::set_allowed_tools`), the findings budget, an optional
//! display filter for structured output, and what the host does with
//! [`AgentConversationEvent::TurnCompleted`].
//!
//! No PTY: an approved `run_command` here runs through the controller's own
//! background executor (`local_tools::exec_run_command`), never a shell block.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, Container, CrossAxisAlignment, Element, Flex,
    ParentElement, ScrollbarWidth,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::WindowId;
use warpui::{
    AppContext, Entity, ModelHandle, ReadModel, TypedActionView, UpdateModel, UpdateView, View,
    ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::shimmering_text::ShimmeringTextStateHandle;
use warpui_core::elements::MouseStateHandle;
use warpui_core::elements::{DispatchEventResult, EventHandler};

use crate::agent::controller::TurnToolCall;
use crate::agent::{AgentController, AgentControllerEvent};
use crate::console::dir_listing::{DirSort, DirView};
use crate::terminal::render::{build_agent_block_element, BlockRenderContext, DiagramCanvasState};
use crate::terminal::view::actions::TerminalAction;
use crate::terminal::view::copy_confirmation::{CopyConfirmation, COPY_CONFIRMATION};
use crate::theme::{scaled, Theme, INPUT_FONT_SIZE};
use crate::workspace::{InputEditor, InputMode};
use uuid::Uuid;

/// What the embedding host hears from the conversation.
#[derive(Clone, Debug)]
pub enum AgentConversationEvent {
    /// Blocks changed (stream delta, tool step, user action). Hosts usually
    /// just repaint.
    BlocksChanged,
    /// A pass reached `Done` — the raw assistant text and this pass's tool
    /// calls. See `AgentControllerEvent::TurnCompleted`.
    TurnCompleted {
        assistant_text: String,
        model_name: String,
        tool_calls: Vec<TurnToolCall>,
    },
    /// The stream failed. The controller already pushed the Error card.
    Error {
        message: String,
    },
    /// Parked on an approval card (see `AgentControllerEvent::ConsentRequested`).
    ConsentRequested {
        action_id: Uuid,
        call_id: String,
        tool_name: String,
        args_json: String,
        reason: String,
    },
    /// An approval card was answered.
    ApprovalResolved {
        action_id: Uuid,
        approved: bool,
    },
    /// Parked on an `ask_user` card.
    QuestionOpened {
        action_id: Uuid,
        call_id: String,
        summary: String,
    },
    QuestionAnswered {
        action_id: Uuid,
    },
    /// The model called a host-owned tool; answer with
    /// `pipeline::post_tool_result(task_id, call_id, …)`.
    HostToolCall {
        task_id: String,
        call_id: String,
        name: String,
        args_json: String,
    },
}

pub struct AgentConversationView {
    controller: ModelHandle<AgentController>,
    view_id: warpui::EntityId,
    window_id: WindowId,
    theme: Theme,
    mono: FamilyId,
    /// Inline editor for the ask_user card's custom answer — same editor
    /// the terminal mounts.
    question_editor: ViewHandle<InputEditor>,
    /// Inline editor for the approval card's rejection feedback.
    feedback_editor: ViewHandle<InputEditor>,
    mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    /// The copy button currently showing its checkmark.
    copy_confirmation: CopyConfirmation,
    shimmer_states: RefCell<HashMap<String, ShimmeringTextStateHandle>>,
    diagram_states: RefCell<HashMap<String, DiagramCanvasState>>,
    scroll: ClippedScrollStateHandle,
    /// Follow-bottom, like the terminal block list's FollowBottom mode: pin
    /// to the tail while the agent produces output; a manual scroll-up
    /// pauses it, scrolling back down (or a new turn) re-arms it. Cells
    /// because the bookkeeping runs in `render`, the one place every wheel
    /// movement is observed.
    follow: Cell<bool>,
    last_scroll: Cell<f32>,
    /// Available width for content (dir listings, wrapped text). Set by the
    /// host from its layout; falls back to a sensible pane width.
    content_width_px: f32,
    ticker_armed: bool,
}

/// Reads at or above this are the f32::MAX pin sentinel written by
/// `scroll_to_bottom`, never a genuine scroll position (no conversation is
/// 1e30px tall); layout clamps it to the real tail only after the read.
const FOLLOW_SENTINEL_THRESHOLD: f32 = 1e30;

/// Follow-bottom transition: a manual scroll-up pauses tail-pinning,
/// scrolling back down re-arms it. Sentinel reads preserve both values.
fn follow_transition(scroll: f32, last_scroll: f32, following: bool) -> (bool, f32) {
    if scroll >= FOLLOW_SENTINEL_THRESHOLD {
        return (following, last_scroll);
    }
    if scroll < last_scroll - 2.0 {
        (false, scroll)
    } else if scroll > last_scroll + 2.0 {
        (true, scroll)
    } else {
        (following, scroll)
    }
}

impl AgentConversationView {
    pub fn new(
        controller: ModelHandle<AgentController>,
        theme: Theme,
        mono: FamilyId,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let question_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Type your own answer…",
                mono,
                crate::theme::text_size(INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "conversation-question-answer".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let feedback_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Tell the agent what to do instead…",
                mono,
                crate::theme::text_size(INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "conversation-approval-feedback".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });

        ctx.subscribe_to_model(&controller, |view, _handle, event, ctx| {
            view.arm_ticker(ctx);
            match event {
                AgentControllerEvent::BlocksChanged => {
                    view.follow_if_pinned();
                    ctx.emit(AgentConversationEvent::BlocksChanged);
                    ctx.notify();
                }
                AgentControllerEvent::ApprovalResolved {
                    action_id,
                    approved,
                } => {
                    view.follow_if_pinned();
                    ctx.emit(AgentConversationEvent::ApprovalResolved {
                        action_id: *action_id,
                        approved: *approved,
                    });
                    ctx.notify();
                }
                AgentControllerEvent::ConsentRequested {
                    action_id,
                    call_id,
                    tool_name,
                    args_json,
                    reason,
                } => {
                    view.follow_if_pinned();
                    ctx.emit(AgentConversationEvent::ConsentRequested {
                        action_id: *action_id,
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        args_json: args_json.clone(),
                        reason: reason.clone(),
                    });
                    ctx.notify();
                }
                AgentControllerEvent::QuestionOpened {
                    action_id,
                    call_id,
                    summary,
                } => {
                    view.follow_if_pinned();
                    ctx.emit(AgentConversationEvent::QuestionOpened {
                        action_id: *action_id,
                        call_id: call_id.clone(),
                        summary: summary.clone(),
                    });
                    ctx.notify();
                }
                AgentControllerEvent::QuestionAnswered { action_id } => {
                    ctx.emit(AgentConversationEvent::QuestionAnswered {
                        action_id: *action_id,
                    });
                    ctx.notify();
                }
                AgentControllerEvent::HostToolCall {
                    task_id,
                    call_id,
                    name,
                    args_json,
                } => {
                    view.follow_if_pinned();
                    ctx.emit(AgentConversationEvent::HostToolCall {
                        task_id: task_id.clone(),
                        call_id: call_id.clone(),
                        name: name.clone(),
                        args_json: args_json.clone(),
                    });
                    ctx.notify();
                }
                AgentControllerEvent::TurnCompleted {
                    assistant_text,
                    model_name,
                    tool_calls,
                    ..
                } => {
                    ctx.emit(AgentConversationEvent::TurnCompleted {
                        assistant_text: assistant_text.clone(),
                        model_name: model_name.clone(),
                        tool_calls: tool_calls.clone(),
                    });
                    ctx.notify();
                }
                AgentControllerEvent::Error { message } => {
                    log::warn!("[agent-conversation] {message}");
                    ctx.emit(AgentConversationEvent::Error {
                        message: message.clone(),
                    });
                    ctx.notify();
                }
                AgentControllerEvent::BudgetUpdated { .. } => {
                    // Keep the account page's usage current, like the terminal.
                    use warpui::SingletonEntity;
                    let account = crate::settings::AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.fetch(mctx));
                    ctx.notify();
                }
                AgentControllerEvent::SessionExpired => {
                    log::warn!("[agent-conversation] session expired — logging out");
                    use warpui::SingletonEntity;
                    let auth = crate::auth::AuthManager::handle(ctx);
                    ctx.update_model(&auth, |a: &mut crate::auth::AuthManager, auth_ctx| {
                        a.log_out(auth_ctx)
                    });
                    ctx.notify();
                }
                // No PTY here; a command-intent turn is never forced by an
                // embedded host, so this cannot fire. Log if it ever does.
                AgentControllerEvent::RunCommandInTerminal { command, .. } => {
                    log::warn!(
                        "[agent-conversation] RunCommandInTerminal without a terminal: {command}"
                    );
                }
                AgentControllerEvent::Agent(_) => {}
            }
        });

        Self {
            controller,
            view_id: ctx.view_id(),
            window_id: ctx.window_id(),
            theme,
            mono,
            question_editor,
            feedback_editor,
            mouse_states: RefCell::new(HashMap::new()),
            copy_confirmation: CopyConfirmation::default(),
            shimmer_states: RefCell::new(HashMap::new()),
            diagram_states: RefCell::new(HashMap::new()),
            scroll: ClippedScrollStateHandle::new(),
            follow: Cell::new(true),
            last_scroll: Cell::new(0.0),
            content_width_px: 760.0,
            ticker_armed: false,
        }
    }

    pub fn controller(&self) -> &ModelHandle<AgentController> {
        &self.controller
    }

    pub fn scroll_handle(&self) -> ClippedScrollStateHandle {
        self.scroll.clone()
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        ctx.notify();
    }

    pub fn set_content_width(&mut self, width_px: f32) {
        if width_px > 1.0 {
            self.content_width_px = width_px;
        }
    }

    /// Pin the conversation to its newest block (a new turn started).
    pub fn pin_to_bottom(&mut self) {
        self.follow.set(true);
        self.scroll_to_bottom();
    }

    pub fn is_busy<C: ReadModel>(&self, ctx: &C) -> bool {
        self.controller.read(ctx, |c, _| c.is_busy())
    }

    fn follow_if_pinned(&self) {
        if self.follow.get() {
            self.scroll_to_bottom();
        }
    }

    fn scroll_to_bottom(&self) {
        // Same bottom pin the existing work-item conversation used.
        self.scroll
            .scroll_to(warpui_core::units::Pixels::new(f32::MAX));
    }

    /// Repaint once a second while the agent is busy so elapsed counters and
    /// shimmers tick — mirrors `TerminalView::arm_agent_ticker`.
    fn arm_ticker(&mut self, ctx: &mut ViewContext<Self>) {
        if self.ticker_armed {
            return;
        }
        self.ticker_armed = true;
        Self::spawn_ticker(ctx);
    }

    fn spawn_ticker(ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            Timer::after(Duration::from_secs(1)),
            |view: &mut Self, _, ctx| {
                let busy = view.controller.read(ctx, |c, _| c.is_busy());
                ctx.notify();
                if busy {
                    Self::spawn_ticker(ctx);
                } else {
                    view.ticker_armed = false;
                }
            },
        );
    }

    fn block_render_context(&self) -> BlockRenderContext<'_> {
        BlockRenderContext {
            entry_hint: None,
            link_hint: None,
            hovered_link: None,
            dir_hover: Default::default(),
            mouse_states: &self.mouse_states,
            shimmer_states: &self.shimmer_states,
            diagram_states: &self.diagram_states,
            content_width_px: self.content_width_px,
            theme: self.theme,
            mono: self.mono,
            dir_sort: DirSort::default(),
            dir_view: DirView::default(),
            feedback_editor: Some(&self.feedback_editor),
            question_editor: Some(&self.question_editor),
            block_filter_editor: None,
            block_filter_match_index: 0,
            copied_key: self.copy_confirmation.key(),
        }
    }
}

impl Entity for AgentConversationView {
    type Event = AgentConversationEvent;
}

impl View for AgentConversationView {
    fn ui_name() -> &'static str {
        "AgentConversationView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        // Observe wheel movement here (every render), like the terminal's
        // block list leaving/re-entering FollowBottom.
        let (following, last) = follow_transition(
            self.scroll.scroll_start().as_f32(),
            self.last_scroll.get(),
            self.follow.get(),
        );
        self.follow.set(following);
        self.last_scroll.set(last);

        let (blocks, live_checkpoint_ids) = self.controller.read(app, |c, _| {
            let blocks = c.agent_blocks().to_vec();
            let live: HashSet<usize> = blocks
                .iter()
                .filter_map(|b| {
                    matches!(
                        b.kind,
                        crate::agent::controller::block::AgentBlockKind::UserQuery { .. }
                    )
                    .then(|| b.id)
                })
                .filter(|id| c.has_live_checkpoint(*id))
                .collect();
            (blocks, live)
        });
        let render_ctx = self.block_render_context();
        let theme = self.theme;

        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for block in &blocks {
            column = column.with_child(build_agent_block_element(
                block,
                &render_ctx,
                theme,
                self.mono,
                &live_checkpoint_ids,
            ));
        }

        // Keep the conversation inset from pane edges on every host.
        let list = ClippedScrollable::vertical(
            self.scroll.clone(),
            Container::new(column.finish())
                .with_padding_left(24.0)
                .with_padding_right(24.0)
                .with_padding_top(22.0)
                .with_padding_bottom(22.0)
                .finish(),
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.pane_surface().into(),
        )
        .with_overlayed_scrollbar()
        .finish();

        // Enter inside the ask_user "Other" field / the rejection-feedback
        // field submits it — the terminal routes this through
        // `TerminalAction::Submit`; here the view raises it itself.
        let view_id = self.view_id;
        let window_id = self.window_id;
        let question_id = self.question_editor.id();
        let feedback_id = self.feedback_editor.id();
        EventHandler::new(list)
            .with_always_handle()
            .on_keydown(move |event_ctx, app, keystroke| {
                if keystroke.key != "enter" || keystroke.shift {
                    return DispatchEventResult::PropagateToParent;
                }
                let focused = app.focused_view_id(window_id);
                if focused.is_some_and(|id| id == question_id || id == feedback_id) {
                    event_ctx.dispatch_typed_action_for(view_id, TerminalAction::Submit);
                    return DispatchEventResult::StopPropagation;
                }
                DispatchEventResult::PropagateToParent
            })
            .finish()
    }
}

impl TypedActionView for AgentConversationView {
    type Action = TerminalAction;

    /// The agent arms of `TerminalView::handle_action`, minus everything that
    /// needs a PTY. Non-agent actions are ignored (they have no meaning here).
    fn handle_action(&mut self, action: &TerminalAction, ctx: &mut ViewContext<Self>) {
        let controller = self.controller.clone();
        match action {
            TerminalAction::ToggleBlockExpansion { block_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.toggle_activity_group_expanded(*block_id, mctx);
                });
                ctx.notify();
            }
            TerminalAction::ToggleAgentThinking { block_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.toggle_thinking_expanded(*block_id, mctx);
                });
                ctx.notify();
            }
            TerminalAction::ToggleToolStep { block_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.toggle_tool_step_expanded(*block_id, mctx);
                });
                ctx.notify();
            }
            TerminalAction::ToggleDiffExpand { tool_id } => {
                crate::agent::edit_diff::toggle_expanded(tool_id);
                ctx.update_model(&controller, |_c, mctx| mctx.notify());
                ctx.notify();
            }
            TerminalAction::CopyText { text } => {
                if !text.is_empty() {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(text.clone()));
                }
            }
            TerminalAction::CopyWithConfirmation { key, text } => {
                if !text.is_empty() {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(text.clone()));
                    let generation = self.copy_confirmation.show(key.clone());
                    ctx.notify();
                    ctx.spawn(Timer::after(COPY_CONFIRMATION), move |view, _, ctx| {
                        if view.copy_confirmation.expire(generation) {
                            ctx.notify();
                        }
                    });
                }
            }
            TerminalAction::ApproveAgentAction { action_id } => {
                let is_streaming = ctx.read_model(&controller, |c, _| c.is_streaming());
                if is_streaming {
                    ctx.update_model(&controller, |c, mctx| {
                        c.resolve_streaming_approval(*action_id, true, None, mctx);
                    });
                } else {
                    ctx.update_model(&controller, |c, mctx| {
                        c.approve_action_background(*action_id, mctx);
                    });
                }
            }
            TerminalAction::AllowToolAlways { action_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.allow_tool_always(*action_id, mctx);
                });
            }
            TerminalAction::RejectAgentAction {
                action_id,
                feedback,
            } => {
                let is_streaming = ctx.read_model(&controller, |c, _| c.is_streaming());
                if is_streaming {
                    ctx.update_model(&controller, |c, mctx| {
                        c.resolve_streaming_approval(*action_id, false, feedback.clone(), mctx);
                    });
                } else {
                    ctx.update_model(&controller, |c, mctx| {
                        c.reject_action(*action_id, feedback.clone(), mctx);
                    });
                }
                ctx.update_view(&self.feedback_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::PickAgentQuestionOption { action_id, option } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.pick_user_question_option(*action_id, option.clone(), mctx);
                });
                ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::ConfirmAgentQuestion { action_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.confirm_user_question(*action_id, mctx);
                });
                ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::SubmitAgentQuestionText { action_id } => {
                let text = self
                    .question_editor
                    .read(ctx, |e, _| e.get_text().to_string());
                if !text.trim().is_empty() {
                    ctx.update_model(&controller, |c, mctx| {
                        c.answer_user_question_text(*action_id, text, mctx);
                    });
                    ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                }
            }
            TerminalAction::MoveAgentQuestionCursor { action_id, delta } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.move_question_cursor(*action_id, *delta, mctx);
                });
            }
            TerminalAction::PickAgentQuestionCursor { action_id } => {
                let picked = ctx.read_model(&controller, |c, _| c.question_cursor_pick(*action_id));
                if let Some(option) = picked {
                    ctx.update_model(&controller, |c, mctx| {
                        c.pick_user_question_option(*action_id, option, mctx);
                    });
                    ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                }
            }
            TerminalAction::PickAgentQuestionDigit { action_id, index } => {
                let picked = ctx.read_model(&controller, |c, _| {
                    c.question_digit_pick(*action_id, *index)
                });
                if let Some(option) = picked {
                    ctx.update_model(&controller, |c, mctx| {
                        c.pick_user_question_option(*action_id, option, mctx);
                    });
                    ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                }
            }
            TerminalAction::CancelQueuedMessage { queue_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.cancel_queued_message(*queue_id, mctx);
                });
            }
            TerminalAction::ApprovePlan { plan_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.approve_plan(*plan_id, mctx);
                });
            }
            TerminalAction::RefinePlan { plan_id } => {
                ctx.update_model(&controller, |c, mctx| {
                    c.mark_plan_refining(*plan_id, mctx);
                });
            }
            TerminalAction::Submit => {
                let question_focused = self.question_editor.read(ctx, |e, cx| e.is_focused(cx));
                let feedback_focused = self.feedback_editor.read(ctx, |e, cx| e.is_focused(cx));
                if question_focused {
                    let text = self
                        .question_editor
                        .read(ctx, |e, _| e.get_text().to_string());
                    let pending = ctx.read_model(&controller, |c, _| c.first_pending_question_id());
                    if let (Some(action_id), false) = (pending, text.trim().is_empty()) {
                        ctx.update_model(&controller, |c, mctx| {
                            c.answer_user_question_text(action_id, text, mctx);
                        });
                        ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                    }
                } else if feedback_focused {
                    let feedback = self
                        .feedback_editor
                        .read(ctx, |e, _| e.get_text().to_string())
                        .trim()
                        .to_string();
                    let pending =
                        ctx.read_model(&controller, |c, _| c.frontmost_pending_approval());
                    if let Some(action_id) = pending {
                        let feedback = (!feedback.is_empty()).then_some(feedback);
                        let is_streaming = ctx.read_model(&controller, |c, _| c.is_streaming());
                        ctx.update_model(&controller, |c, mctx| {
                            if is_streaming {
                                c.resolve_streaming_approval(action_id, false, feedback, mctx);
                            } else {
                                c.reject_action(action_id, feedback, mctx);
                            }
                        });
                        ctx.update_view(&self.feedback_editor, |e, ectx| e.set_text("", ectx));
                    }
                }
            }
            TerminalAction::StopAgentStream => {
                ctx.update_model(&controller, |c, mctx| {
                    c.cancel_stream(mctx);
                });
                ctx.notify();
            }
            TerminalAction::ClearAgentConversation => {
                ctx.update_model(&controller, |c, mctx| {
                    c.clear_conversation(mctx);
                });
                ctx.notify();
            }
            TerminalAction::BlockScroll { delta_y } => {
                self.scroll
                    .scroll_by(warpui_core::units::Pixels::new(*delta_y));
                ctx.notify();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod follow_tests {
    use super::follow_transition;

    #[test]
    fn pin_sentinel_never_kills_follow() {
        // Regression: the pin writes f32::MAX and layout clamps it afterwards.
        // The clamped read must not look like a scroll-up.
        assert_eq!(follow_transition(f32::MAX, 0.0, true), (true, 0.0));
        assert_eq!(follow_transition(800.0, 0.0, true), (true, 800.0));
        assert_eq!(follow_transition(f32::MAX, 800.0, true), (true, 800.0));
        assert_eq!(follow_transition(950.0, 800.0, true), (true, 950.0));
    }

    #[test]
    fn manual_scroll_up_pauses_and_down_rearms() {
        assert_eq!(follow_transition(400.0, 950.0, true), (false, 400.0));
        assert_eq!(follow_transition(401.0, 400.0, false), (false, 401.0));
        assert_eq!(follow_transition(600.0, 401.0, false), (true, 600.0));
    }
}
