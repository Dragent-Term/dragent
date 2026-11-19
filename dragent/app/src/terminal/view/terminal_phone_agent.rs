//! The built-in agent's pane, as a phone sees and drives it.
//!
//! Blocks here are impl TerminalView: same type, same behavior. See
//! `realtime::phone_agent` for the shape of what is published.

use super::terminal_view::TerminalView;
use crate::realtime::phone_agent::{self, Command};
use crate::terminal::view::actions::TerminalAction;
use std::time::Duration;
use warpui::{r#async::Timer, TypedActionView, UpdateModel, ViewContext};
use web_time::Instant;

/// The least time between two projections of a streaming conversation.
const PUBLISH_EVERY: Duration = Duration::from_millis(120);

impl TerminalView {
    /// The conversation changed. Projects it for a watching phone, at most
    /// once per [`PUBLISH_EVERY`]; a trailing timer carries the final state.
    pub(crate) fn phone_agent_changed(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.is_builtin_agent_tab() || !phone_agent::watched(self.pane_uuid) {
            return;
        }
        let wait = self
            .phone_agent_published_at
            .map(|at| PUBLISH_EVERY.saturating_sub(at.elapsed()))
            .unwrap_or_default();
        if wait.is_zero() {
            self.phone_agent_publish(ctx);
        } else if !self.phone_agent_timer {
            self.phone_agent_timer = true;
            ctx.spawn(Timer::after(wait), |view, _, ctx| {
                view.phone_agent_timer = false;
                view.phone_agent_publish(ctx);
            });
        }
    }

    fn phone_agent_publish(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.is_builtin_agent_tab() {
            return;
        }
        self.phone_agent_published_at = Some(Instant::now());
        let snapshot = self
            .agent_controller
            .read(ctx, |c, _| phone_agent::project(c.agent_blocks(), c.is_busy()));
        phone_agent::publish(self.pane_uuid, snapshot);
    }

    /// Carry out what a phone asked, exactly as the same click or prompt on
    /// the desktop would. Only the built-in agent's pane takes these.
    pub(crate) fn handle_phone_agent_command(&mut self, command: Command, ctx: &mut ViewContext<Self>) {
        // The model picker works on any tab with one: Dragent's agent, or an
        // agent CLI shown as a chat.
        let command = match command {
            Command::Models(reply) => {
                reply.send(self.phone_model_menu(ctx));
                return;
            }
            Command::SetModel { id, effort, reply } => {
                let picked = (self.phone_model_available(ctx) && !self.phone_model_locked(ctx)).then(|| {
                    let (rows, _, _) = self.model_menu(ctx);
                    rows.into_iter()
                        .find(|(row, _, level)| *row == id && level.wire_value() == effort.as_deref())
                })
                .flatten();
                if let Some((model_id, _, effort)) = picked {
                    self.handle_action(&TerminalAction::SetAgentModel { model_id, effort }, ctx);
                    ctx.notify();
                }
                reply.send(self.phone_model_menu(ctx));
                return;
            }
            other => other,
        };
        if !self.is_builtin_agent_tab() {
            return;
        }
        match command {
            Command::Refresh => {}
            Command::Submit(text) => self.submit_agent_query(text, ctx),
            Command::Stop => self.handle_action(&TerminalAction::StopAgentStream, ctx),
            Command::Approve(action_id) => {
                self.handle_action(&TerminalAction::ApproveAgentAction { action_id }, ctx)
            }
            Command::ApproveAlways(action_id) => {
                self.handle_action(&TerminalAction::AllowToolAlways { action_id }, ctx)
            }
            Command::Reject(action_id) => self.handle_action(
                &TerminalAction::RejectAgentAction { action_id, feedback: None },
                ctx,
            ),
            Command::Answer { id, option } => self.handle_action(
                &TerminalAction::PickAgentQuestionOption { action_id: id, option },
                ctx,
            ),
            Command::ConfirmAnswer(action_id) => {
                self.handle_action(&TerminalAction::ConfirmAgentQuestion { action_id }, ctx)
            }
            Command::AnswerText { id, text } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.answer_user_question_text(id, text, mctx);
                });
            }
            Command::ApprovePlan(plan_id) => {
                self.handle_action(&TerminalAction::ApprovePlan { plan_id }, ctx)
            }
            Command::RefinePlan(plan_id) => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.mark_plan_refining(plan_id, mctx);
                });
            }
            Command::CancelQueued(queue_id) => {
                self.handle_action(&TerminalAction::CancelQueuedMessage { queue_id }, ctx)
            }
            Command::Models(_) | Command::SetModel { .. } => {}
        }
        ctx.notify();
        // The phone sees the result at once, not on the next throttle tick.
        self.phone_agent_publish(ctx);
    }

    /// Whether this tab has a model picker: Dragent's own agent, or a chat
    /// whose agent lists its models (the desktop shows the picker then too).
    fn phone_model_available(&self, ctx: &ViewContext<Self>) -> bool {
        self.is_builtin_agent_tab() || self.chat_model_picker(ctx).is_some()
    }

    /// `{available, current:{id, effort}, choices:[{id, label, effort}]}`.
    fn phone_model_menu(&self, ctx: &ViewContext<Self>) -> serde_json::Value {
        use serde_json::json;
        if !self.phone_model_available(ctx) {
            return json!({"available": false, "choices": []});
        }
        if self.phone_model_locked(ctx) {
            return json!({"available": false, "locked": true, "choices": []});
        }
        let (rows, current, effort) = self.model_menu(ctx);
        json!({
            "available": true,
            "current": {"id": current, "effort": effort.wire_value()},
            "choices": rows
                .iter()
                .take(300)
                .map(|(id, label, effort)| json!({"id": id, "label": label, "effort": effort.wire_value()}))
                .collect::<Vec<_>>(),
        })
    }

    /// The desktop locks Dragent's model picker when the account may only use
    /// the free model (and no own key is set); a phone follows the same rule.
    fn phone_model_locked(&self, ctx: &warpui::AppContext) -> bool {
        use warpui::{ReadModel as _, SingletonEntity};
        if self.chat_model_picker(ctx).is_some() {
            return false;
        }
        if crate::settings::AppSettings::global().has_any_user_credential() {
            return false;
        }
        let account = crate::settings::AccountStore::handle(ctx);
        ctx.read_model(&account, |store, _| {
            store.data.as_ref().is_some_and(|d| {
                d.usage.model_locked_to_free || d.user.tier.eq_ignore_ascii_case("free")
            })
        })
    }
}
