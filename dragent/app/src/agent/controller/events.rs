//! UI-side agent events + the WarpUI stream pumps.
//!
//! [`AgentControllerEvent`] mirrors the engine's
//! [`CoreEvent`][dragent_agent::runtime::CoreEvent] 1:1 (including payloads);
//! translation is mechanical (see `host_impls::translate_event`). The stream
//! dispatcher itself (`apply_stream_event`, `handle_unexpected_stream_end`)
//! lives in the engine core — `poll_stream` drives it through the pump
//! contract (`drain_deferred`, generation checks, sink pushes) and owns the
//! WarpUI timers.

use uuid::Uuid;

use crate::agent::pipeline::{AgentEventKind, PipelineEvent};
use crate::agent::AgentEvent;

use super::block::ErrorKind;
use super::state::ConversationState;
use super::AgentController;
use std::sync::mpsc;
use std::time::Duration;
use warpui::r#async::Timer;
use warpui::ModelContext;

use dragent_agent::runtime::VecSink;

use crate::agent::host_impls::{emit_deferred, pump, translate_event};

#[derive(Clone, Debug)]
pub enum AgentControllerEvent {
    BlocksChanged,
    ApprovalResolved {
        action_id: Uuid,
        approved: bool,
    },
    Error {
        message: String,
    },
    /// The user's refresh token was revoked or expired — the session is gone.
    /// Subscribers should log the user out and transition to the signed-out UI.
    SessionExpired,
    /// Fired after each Done event with cumulative token and turn counts.
    BudgetUpdated {
        turns: u32,
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Cross-layer bus event (e.g. `AgentEvent::QueryCompleted` for Layer 5+ subscribers).
    Agent(AgentEvent),
    /// A turn (or an internal continuation pass) reached `Done`. Carries the
    /// RAW assistant text of that pass — before any display filter — plus every
    /// tool call it made, so an embedded host can validate structured output
    /// and derive observed files without re-implementing the stream loop.
    TurnCompleted {
        conversation_id: Uuid,
        assistant_text: String,
        model_name: String,
        tool_calls: Vec<super::TurnToolCall>,
    },
    /// The turn is parked on an approval card. Contract-governed hosts mirror
    /// this as a durable Permission attention item so the scheduler blocks
    /// and Coordinate can route the user to the agent's pane.
    ConsentRequested {
        action_id: Uuid,
        call_id: String,
        tool_name: String,
        args_json: String,
        reason: String,
    },
    /// The turn is parked on an `ask_user` card.
    QuestionOpened {
        action_id: Uuid,
        call_id: String,
        summary: String,
    },
    /// The `ask_user` card was answered and the stream resumed.
    QuestionAnswered {
        action_id: Uuid,
    },
    /// The model called a tool the embedding host owns (see
    /// `AgentController::set_host_tools`). The host executes it and answers
    /// with `pipeline::post_tool_result(task_id, call_id, …)`; the server's
    /// ToolEnd then completes the card like any other tool.
    HostToolCall {
        task_id: String,
        call_id: String,
        name: String,
        args_json: String,
    },
    /// Agent approved a command-intent `run_command` tool call. The terminal view
    /// decides whether it is safe to use the visible PTY by comparing the tool's
    /// requested cwd with the terminal's cwd. Mismatched commands use the normal
    /// isolated executor instead of moving or borrowing the user's shell.
    RunCommandInTerminal {
        task_id: String,
        call_id: String,
        command: String,
        args_json: String,
        allow_terminal_cwd_change: bool,
    },
}

use crate::agent::pipeline::StreamError;
pub use dragent_agent::controller::event_helpers::{
    friendly_stream_error, sanitize_server_error, unix_now_ms,
};

impl AgentController {
    /// Start the next queued prompt, if any, after a turn ended. Emits the
    /// engine's queued-turn events and pumps the launched turn.
    fn pump_maybe_start_next(&mut self, ctx: &mut ModelContext<Self>) {
        let mut sink = VecSink::default();
        let launched = self.engine.maybe_start_next_queued(&mut sink);
        let mut changed = false;
        for event in sink.events {
            let event = translate_event(event);
            changed |= matches!(event, AgentControllerEvent::BlocksChanged);
            ctx.emit(event);
        }
        if changed {
            ctx.notify();
        }
        self.pump_launch(ctx, launched);
    }

    pub(crate) fn poll_stream(
        &mut self,
        conv_id: Uuid,
        rx: mpsc::Receiver<Result<PipelineEvent, StreamError>>,
        gen: u64,
        last_event_at: web_time::Instant,
        ctx: &mut ModelContext<Self>,
    ) {
        // Silently drop events from an orphaned stream (superseded by a new query).
        if gen != self.engine.stream_generation() {
            return;
        }

        // Drain all available events without blocking
        let mut changed = false;
        let mut done = false;
        let mut new_last_event = last_event_at;

        loop {
            match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Stream channel closed — tx was dropped when read_stream returned.
                    // If the Done event was already processed, state is Idle and this is
                    // a normal clean shutdown. If state is still Streaming/AwaitingApproval,
                    // the connection was lost while work was in progress; clean up so the
                    // agent doesn't freeze indefinitely in a non-Idle state.
                    if !matches!(self.engine.state(), ConversationState::Idle) {
                        log::warn!("[agent-stream] SSE stream closed unexpectedly while agent was still active");
                        pump(ctx, &mut self.engine, |e, _| {
                            e.handle_unexpected_stream_end(conv_id);
                        });
                        emit_deferred(ctx, &mut self.engine);
                        changed = true;
                    }
                    done = true;
                    break;
                }
                Ok(Err(e)) => {
                    // Log the raw internal error for diagnostics but never show it to
                    // the user — it may contain provider URLs or internal stack traces.
                    log::warn!("[agent-stream] error: {e}");
                    let (message, kind) = friendly_stream_error(&e);

                    // If the session was revoked or expired, signal the terminal view
                    // to log the user out. We can't call AuthManager directly from a
                    // ModelContext, so we emit SessionExpired and let the subscriber
                    // (TerminalView) trigger the logout from its ViewContext.
                    if matches!(kind, ErrorKind::Auth) {
                        log::warn!("[agent-stream] auth failure — emitting SessionExpired");
                        ctx.emit(AgentControllerEvent::SessionExpired);
                    }

                    // Fail the turn through the engine (pushes the Error block
                    // and a BlocksChanged into the sink, retires any open
                    // ask_user card), then surface the user-facing error.
                    pump(ctx, &mut self.engine, |engine, sink| {
                        engine.fail_stream_turn(message.clone(), kind, conv_id, sink);
                    });
                    ctx.emit(AgentControllerEvent::Error { message });
                    // A hung/errored turn shouldn't strand the queue — start the
                    // next queued prompt now that this turn is over.
                    self.pump_maybe_start_next(ctx);
                    return;
                }
                Ok(Ok(ev)) => {
                    // Any byte from the server (including keepalive pings) counts
                    // as liveness and resets the inactivity timer below.
                    new_last_event = web_time::Instant::now();
                    // A recovered run is live again: results post normally now.
                    self.engine.clear_recovery_tx();
                    // Intercept sentinels carried on TextDelta.error.
                    if matches!(ev.kind, AgentEventKind::TextDelta) {
                        if let Some(ref err) = ev.error {
                            if let Some(task_id) = err.strip_prefix("__task_id__") {
                                self.engine.set_streaming_task_id(task_id.to_string());
                                continue;
                            }
                            // Server keepalive ping — liveness only, no UI change.
                            if err == "__ping__" {
                                continue;
                            }
                        }
                    }
                    changed |= self.engine.apply_stream_event(ev, conv_id);
                    emit_deferred(ctx, &mut self.engine);
                    if matches!(self.engine.state(), ConversationState::Idle) {
                        done = true;
                    }
                }
            }
        }

        // Inactivity timeout: if no SSE events have arrived for 120 seconds while the
        // agent is still active, the server connection is likely silently dead. Terminate
        // with a clear error rather than leaving the agent frozen indefinitely.
        //
        // Excluded: states where we are DELIBERATELY waiting on the human
        // (an approval or an ask_user question). There the server is idle by
        // design — it's blocked on the tool-result gate — so "no events" is
        // expected, not a dead connection. Its 20s pings still arrive and reset
        // the timer, but a user can out-wait 120s, so don't gamble on that here.
        const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
        let awaiting_user = matches!(
            self.engine.state(),
            ConversationState::AwaitingApproval { .. }
                | ConversationState::AwaitingUserChoice { .. }
        );
        if !done && !awaiting_user && !matches!(self.engine.state(), ConversationState::Idle) {
            if new_last_event.elapsed() > INACTIVITY_TIMEOUT {
                log::warn!(
                    "[agent-stream] inactivity timeout after {}s — no SSE events received",
                    new_last_event.elapsed().as_secs()
                );
                pump(ctx, &mut self.engine, |e, _| {
                    e.handle_unexpected_stream_end(conv_id);
                });
                emit_deferred(ctx, &mut self.engine);
                changed = true;
                done = true;
            }
        }

        if changed {
            ctx.emit(AgentControllerEvent::BlocksChanged);
        }
        // Turn-ending events raised outside the per-event loop (unexpected
        // stream end, inactivity timeout).
        emit_deferred(ctx, &mut self.engine);

        if done {
            ctx.emit(AgentControllerEvent::BudgetUpdated {
                turns: self.engine.turn_count(),
                input_tokens: self.engine.session_input_tokens(),
                output_tokens: self.engine.session_output_tokens(),
            });
            // Evidence gate (stashed by the Done handler): auto-continue with a
            // system nudge to verify unverified edits. Fires here because this
            // scope has the ModelContext the continuation needs.
            if let Some(note) = self.engine.take_pending_evidence_nudge() {
                let model = self.engine.last_model_name().to_string();
                let system_arc = self.engine.last_system_arc();
                self.start_system_continuation(
                    note,
                    model,
                    Box::new(move || system_arc.lock().unwrap_or_else(|p| p.into_inner()).clone()),
                    ctx,
                );
                return;
            }
            // Turn finished cleanly — start the next queued prompt, if any.
            self.pump_maybe_start_next(ctx);
            // The message is now complete, so its mermaid diagrams become "live"
            // and the view kicks off background SVG renders. Poll for a short grace
            // window (and while any render is in flight) so the view repaints when
            // each rendered diagram lands.
            self.poll_diagrams(6, ctx);
        } else {
            ctx.spawn(
                Timer::after(Duration::from_millis(50)),
                move |this, _, mctx| {
                    this.poll_stream(conv_id, rx, gen, new_last_event, mctx);
                },
            );
        }
    }

    /// Repaint driver for async mermaid diagram rendering. Emits `BlocksChanged`
    /// (which re-renders the terminal view) while any diagram render is in flight,
    /// plus a short grace window so it reliably catches the view's initial render
    /// kick. Self-terminates once nothing is pending and the grace is exhausted.
    pub(crate) fn poll_diagrams(&mut self, grace: u8, ctx: &mut ModelContext<Self>) {
        let pending = crate::terminal::render::mermaid_render::PENDING_COUNT
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0;
        if pending || grace > 0 {
            ctx.emit(AgentControllerEvent::BlocksChanged);
            // Only spend down the settle-grace tail once rendering has actually
            // finished. Otherwise a diagram that takes longer than the initial
            // grace window to render exhausts it while still pending, leaving
            // zero grace ticks left to catch the pending->Ready transition —
            // polling would then stop on the very next tick, right when the
            // diagram's real (often much taller) size first becomes visible.
            let next = if pending {
                grace
            } else {
                grace.saturating_sub(1)
            };
            ctx.spawn(
                Timer::after(Duration::from_millis(220)),
                move |this, _, ctx| {
                    this.poll_diagrams(next, ctx);
                },
            );
        }
    }
}
