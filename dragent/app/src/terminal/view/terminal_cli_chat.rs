//! A terminal agent's tab as a chat (Claude Code, Codex, OpenCode, MiMo Code,
//! Gemini, Qwen Code, Copilot, Cursor). The agent keeps running in its
//! terminal exactly as before and its input panel stays: only the terminal
//! picture above the panel is replaced by the conversation, read from the
//! record the agent itself writes on this machine. Switched per tab from the
//! "CLI | Chat" switch in the input (CLI is the agent's own terminal screen).

use std::collections::HashSet;
use std::time::Duration;

use super::terminal_view::TerminalView;
use crate::chat::{layout, steps, view as chat_view};
use dragent_host::agent_transcript::{self, Row};
use uuid::Uuid;
use warpui::elements::{
    ClippedScrollable, Container, CrossAxisAlignment, Element, Flex, ParentElement, ScrollbarWidth, SelectableArea,
    Text,
};
use warpui::r#async::Timer;
use warpui::{AppContext, UpdateView, ViewContext};

/// How often the conversation is re-read while the agent works (or a sent
/// message is on its way), and while it is idle.
const READ_WORKING: Duration = Duration::from_millis(500);
const READ_IDLE: Duration = Duration::from_millis(1500);

/// Working = the terminal has been drawing without a break for at least
/// `RUN_MIN` and drew within `RUN_GAP`. Every one of these agents draws a
/// spinner or timer for as long as it works (never silent for more than
/// 3 s, even through long tool calls) and nothing once it stops; a single
/// redraw (a resize, a keypress echo) is too short to count.
const RUN_MIN: Duration = Duration::from_millis(1200);
const RUN_GAP: Duration = Duration::from_millis(3500);
/// A message just sent counts as work starting until the agent draws.
const SENT_GRACE: Duration = Duration::from_millis(4000);
/// A sent message the conversation never shows stops being drawn after this.
const SENT_GIVE_UP: Duration = Duration::from_secs(60);

/// A message sent from the chat, until the conversation has it.
#[derive(Clone, Debug)]
pub(crate) struct SentPrompt {
    pub text: String,
    pub at: web_time::Instant,
    /// How many rows the conversation had when it was sent.
    pub rows_before: usize,
}

/// Whether the agent is working, from its terminal's output: drawing for a
/// while and drawing now, or a message just went out.
pub(crate) fn busy_from_output(
    now: web_time::Instant,
    output_at: Option<web_time::Instant>,
    run_since: Option<web_time::Instant>,
    sent_at: Option<web_time::Instant>,
) -> bool {
    let drawing = match (output_at, run_since) {
        (Some(at), Some(since)) => now.duration_since(at) <= RUN_GAP && at.duration_since(since) >= RUN_MIN,
        _ => false,
    };
    drawing || sent_at.is_some_and(|sent| now.duration_since(sent) <= SENT_GRACE)
}

/// What stops `provider` mid-turn. OpenCode and MiMo ask for Esc twice;
/// Cursor stops on Ctrl-C; the rest on one Esc.
pub(crate) fn stop_keys(provider: &str) -> &'static [&'static [u8]] {
    match provider {
        "opencode" | "mimo" => &[b"\x1b", b"\x1b"],
        "cursor-agent" => &[b"\x03"],
        _ => &[b"\x1b"],
    }
}

/// What was last read of the conversation.
#[derive(Clone, Debug, Default)]
pub(crate) struct CliChat {
    /// The whole conversation: this machine reads it straight from disk.
    pub rows: Vec<Row>,
    /// The agent is on a turn.
    pub working: bool,
    /// …and is held on a permission prompt only its terminal can answer.
    pub waiting: bool,
    /// This agent's conversation can't be read here.
    pub unreadable: bool,
    /// The model the conversation's latest reply came from (Claude Code's
    /// full id, e.g. `claude-opus-5-5`), read from the transcript.
    pub model: Option<String>,
    /// The conversation file and its size/time when `rows` were read, so an
    /// unchanged file isn't read again.
    seen: Option<(agent_transcript::Source, (u64, std::time::SystemTime))>,
}

/// One read: `rows` is `None` when the conversation hasn't changed.
struct ChatRead {
    rows: Option<Vec<Row>>,
    model: Option<String>,
    waiting: bool,
    unreadable: bool,
    seen: Option<(agent_transcript::Source, (u64, std::time::SystemTime))>,
    took: Duration,
}

/// Read pane `pane`'s agent conversation in full (blocking: call off the UI
/// thread). `provider` is the agent running there; `cwd` its folder, for an
/// agent started by hand (no session record): its newest conversation there.
/// `seen` is the previous read's file stamp: unchanged, nothing is re-read.
fn read_chat(
    pane: Uuid,
    provider: String,
    cwd: std::path::PathBuf,
    seen: Option<(agent_transcript::Source, (u64, std::time::SystemTime))>,
) -> ChatRead {
    use dragent_host::agent_sessions;
    let started = std::time::Instant::now();
    let record = agent_sessions::load(pane).filter(|r| r.provider == provider);
    // Working is read from the terminal (see `busy_from_output`): only some
    // agents report their turns. A permission prompt draws nothing, so that
    // still comes from the reports of the agents that send them.
    let (source, waiting) = match &record {
        Some(record) => (
            // The record may not name the conversation (it does once the
            // agent reports it; an agent opened from Claude's background
            // sessions view, `claude agents` / `claude attach`, never does):
            // then the conversation being written in the tab's folder.
            agent_transcript::source(record).or_else(|| agent_transcript::latest_source(&provider, &cwd)),
            agent_sessions::load_activity(pane).is_some_and(|activity| activity.event == "PermissionRequest"),
        ),
        None => (agent_transcript::latest_source(&provider, &cwd), false),
    };
    let unreadable = source.is_none() && !agent_transcript::reads_latest(&provider);
    let stamp = source.as_ref().and_then(|src| agent_transcript::signature(src).map(|sig| (src.clone(), sig)));
    let rows = if stamp.is_some() && stamp == seen {
        None
    } else {
        Some(source.as_ref().and_then(|src| agent_transcript::read_source(src, agent_transcript::Extent::Full).ok()).unwrap_or_default())
    };
    let model = match (&rows, provider.as_str(), &source) {
        (Some(_), "claude", Some(src)) => last_claude_model(&src.path),
        _ => None,
    };
    ChatRead { rows, model, waiting, unreadable, seen: stamp, took: started.elapsed() }
}

impl TerminalView {
    /// The terminal agent in this tab whose conversation can be shown as a
    /// chat: the one it was started as, else the one detected running.
    pub(crate) fn cli_chat_provider<C: warpui::ReadModel>(&self, ctx: &C) -> Option<String> {
        use dragent_host::agent_transcript::supported;
        if self.is_builtin_agent_tab() {
            return None;
        }
        if let Some(id) = self.agent_id.as_deref().filter(|id| supported(id)) {
            return Some(id.to_string());
        }
        self.active_ai_tool(ctx).map(|tool| tool.cli()).filter(|cli| supported(cli)).map(str::to_string)
    }

    pub(crate) fn cli_chat_available<C: warpui::ReadModel>(&self, ctx: &C) -> bool {
        self.cli_chat_provider(ctx).is_some()
    }

    /// The chat is what this tab shows right now.
    pub(crate) fn showing_cli_chat<C: warpui::ReadModel>(&self, ctx: &C) -> bool {
        self.cli_chat_on && self.cli_chat_available(ctx)
    }

    /// The agent shown as a chat is working, read from its terminal.
    pub(crate) fn cli_chat_busy<C: warpui::ReadModel>(&self, _ctx: &C) -> bool {
        if let Some(chat) = &self.acp_chat {
            return chat.working;
        }
        let (output_at, run_since) = {
            let core = self.session_core.lock();
            (core.output_at, core.output_run_since)
        };
        let waiting = self.cli_chat.as_ref().is_some_and(|chat| chat.waiting);
        !waiting
            && busy_from_output(web_time::Instant::now(), output_at, run_since, self.cli_chat_sent.as_ref().map(|s| s.at))
    }

    /// A message went out from the chat: show it at once (the agent may
    /// write it to its conversation only seconds later) and read again now.
    pub(crate) fn note_cli_chat_sent(&mut self, text: String, ctx: &mut ViewContext<Self>) {
        let rows_before = self.cli_chat.as_ref().map_or(0, |chat| chat.rows.len());
        self.cli_chat_sent = Some(SentPrompt { text, at: web_time::Instant::now(), rows_before });
        self.agent_transcript_follow.set(true);
        self.cli_chat_generation = self.cli_chat_generation.wrapping_add(1);
        let generation = self.cli_chat_generation;
        self.read_cli_chat(generation, ctx);
        ctx.notify();
    }

    /// Stop the agent's current turn with the key it stops on.
    pub(crate) fn stop_cli_agent(&mut self, ctx: &mut ViewContext<Self>) {
        if self.acp_chat.is_some() {
            self.acp_stop(ctx);
            return;
        }
        let Some(provider) = self.cli_chat_provider(ctx) else {
            return;
        };
        let keys = stop_keys(&provider);
        self.manager.writer().write(keys[0]);
        if let Some(second) = keys.get(1).copied() {
            // Sent apart, so it reads as a second press, not one sequence.
            ctx.spawn(Timer::after(Duration::from_millis(150)), move |view, _, _| {
                view.manager.writer().write(second);
            });
        }
        self.cli_chat_sent = None;
        ctx.notify();
    }

    /// The chat's input: Dragent's agent input, the same box and the same
    /// controls in the same places. The prompt; then along the bottom + (attach), the CLI / Chat switch
    /// where the agent's mode picker sits, the model picker (the agent's own
    /// models), and on the right the mic and Send / Stop. Typing goes to the
    /// rich input editor, which the chat sends from.
    pub(crate) fn render_chat_composer(&self, app: &AppContext) -> Box<dyn Element> {
        use crate::terminal::view::actions::TerminalAction;
        use crate::terminal::view::terminal_chips::PillMenuRow;
        use warpui::elements::{ChildView, Expanded};
        let theme = self.theme;
        let mono = self.mono_family;

        let prompt_glyph = Container::new(crate::icons::render_icon(
            crate::icons::AppIcon::ChevronRight,
            theme.ansi_green(),
            crate::theme::INPUT_FONT_SIZE,
        ))
        .with_padding_right(8.0)
        .finish();
        let editor_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(prompt_glyph)
            .with_child(Box::new(Expanded::new(1.0, ChildView::new(&self.rich_input_editor).finish())))
            .finish();

        let mut controls = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
        controls = controls.with_child(super::terminal_helpers::input_icon_button(
            crate::icons::AppIcon::Plus,
            theme.sub_text(),
            TerminalAction::OpenFilePicker,
        ));
        // The same "CLI | Chat" switch as the CLI view's input: both views in
        // sight, the current one lit.
        let chrome = self.model.read(app, |m, _| {
            m.with_core(|core| crate::terminal::render::sample_ai_chrome(core.active_emulator(), &theme))
        });
        controls = controls.with_child(crate::console::ai_toolbar::view_switch_chip(
            self.cli_chat_on,
            TerminalAction::SetChatView { chat: true },
            TerminalAction::SetChatView { chat: false },
            chrome,
            &theme,
            mono,
        ));
        if self.chat_model_picker(app).is_some() {
            controls = controls.with_child(self.render_model_selector(app, &theme, mono));
        }
        controls = controls.with_child(Box::new(Expanded::new(1.0, warpui::elements::Empty::new().finish())));
        if let Some(mic) = self.render_input_mic_button() {
            controls = controls.with_child(mic);
        }
        let has_text = !self.rich_input_editor.read(app, |e, _| e.get_text().trim().is_empty());
        controls = controls.with_child(self.render_input_send_button(
            self.cli_chat_busy(app),
            has_text,
            TerminalAction::SubmitRichInput,
            TerminalAction::StopChatAgent,
        ));

        let inner = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(editor_row)
            .with_child(Container::new(controls.finish()).with_padding_top(6.0).finish())
            .finish();
        let editor_container = Container::new(inner)
            .with_background_color(theme.pane_surface())
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(4.0)
            .finish();
        // No folder chip row on top, like Dragent's agent input. A top rule
        // keeps the input apart from the conversation.
        Container::new(editor_container)
            .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
            .finish()
    }

    /// The model selector's list for the CLI's own session (Claude Code):
    /// what it runs now, and a pick here switches it with the CLI's `/model`.
    pub(crate) fn cli_model_picker<C: warpui::ReadModel>(&self, ctx: &C) -> Option<dragent_acp::Picker> {
        if self.acp_chat.is_some() || !self.showing_cli_chat(ctx) || self.cli_chat_provider(ctx).as_deref() != Some("claude") {
            return None;
        }
        let choices = claude_models()?;
        let current = self
            .cli_chat_model_pick
            .clone()
            .or_else(|| self.cli_chat.as_ref()?.model.as_deref().map(|m| claude_choice_for(&choices, m)))
            .unwrap_or_else(|| "default".into());
        Some(dragent_acp::Picker { current, choices, via: dragent_acp::SetVia::Model })
    }

    /// The effort picker for the CLI's own session (Claude Code): a pick
    /// here switches it with the CLI's `/effort`.
    pub(crate) fn cli_effort_picker<C: warpui::ReadModel>(&self, ctx: &C) -> Option<dragent_acp::Picker> {
        self.cli_model_picker(ctx)?;
        let choices = known_efforts("claude")?;
        let current = self.cli_chat_effort_pick.clone().unwrap_or_default();
        Some(dragent_acp::Picker { current, choices, via: dragent_acp::SetVia::ConfigOption("effort".into()) })
    }

    /// The effort picker in the chat, however the chat runs.
    pub(crate) fn chat_effort_picker<C: warpui::ReadModel>(&self, ctx: &C) -> Option<dragent_acp::Picker> {
        self.acp_effort_picker(ctx).or_else(|| self.cli_effort_picker(ctx))
    }

    /// Set the chat agent's effort, however the chat runs.
    pub(crate) fn set_chat_effort(&mut self, level: String, ctx: &mut ViewContext<Self>) {
        if self.chat_effort_picker(ctx).is_some_and(|picker| picker.current == level) {
            return;
        }
        if self.acp_effort_picker(ctx).is_some() {
            self.acp_choose_effort(level, ctx);
        } else if self.cli_effort_picker(ctx).is_some() {
            self.switch_cli_effort(level, ctx);
        }
    }

    /// Where the effort slider's track is, by name in the frame.
    pub(crate) fn effort_track_id(&self) -> String {
        format!("chat-effort-track-{}", self.pane_uuid)
    }

    /// The effort level nearest window x on the slider's track.
    pub(crate) fn effort_level_at(&self, x: f32, ctx: &ViewContext<Self>) -> Option<String> {
        let picker = self.chat_effort_picker(ctx)?;
        let track = ctx.element_position_by_id(self.effort_track_id())?;
        let stops = picker.choices.len();
        if stops == 0 || track.width() <= 0.0 {
            return None;
        }
        // Each stop sits in the middle of an equal share of the track.
        let share = track.width() / stops as f32;
        let index = ((x - track.min_x()) / share).floor().clamp(0.0, (stops - 1) as f32) as usize;
        Some(picker.choices[index].id.clone())
    }

    /// Switch the CLI's own session to effort `level`, as you would: type
    /// `/effort <level>` at its prompt and press Enter.
    pub(crate) fn switch_cli_effort(&mut self, level: String, ctx: &mut ViewContext<Self>) {
        self.cli_chat_effort_pick = Some(level.clone());
        self.manager.writer().write(format!("/effort {level}").as_bytes());
        let delay = self.active_ai_tool(ctx).map(|t| t.submit_cr_delay_ms()).unwrap_or(160).max(300);
        ctx.spawn(Timer::after(Duration::from_millis(delay)), |view, _, _| {
            view.manager.writer().write(b"\r");
        });
        ctx.notify();
    }

    /// The model selector's list in the chat, however the chat runs.
    pub(crate) fn chat_model_picker<C: warpui::ReadModel>(&self, ctx: &C) -> Option<dragent_acp::Picker> {
        self.acp_model_picker(ctx).or_else(|| self.cli_model_picker(ctx))
    }

    /// Switch the CLI's own session to model `id`, the way you would: type
    /// `/model <id>` at its prompt and press Enter. (Claude Code also keeps
    /// it as the default for new sessions, as when you type it yourself.)
    pub(crate) fn switch_cli_model(&mut self, id: String, ctx: &mut ViewContext<Self>) {
        self.cli_chat_model_pick = Some(id.clone());
        self.manager.writer().write(format!("/model {id}").as_bytes());
        let delay = self.active_ai_tool(ctx).map(|t| t.submit_cr_delay_ms()).unwrap_or(160).max(300);
        ctx.spawn(Timer::after(Duration::from_millis(delay)), |view, _, _| {
            view.manager.writer().write(b"\r");
        });
        ctx.notify();
    }

    /// The input panel's actions, with the CLI / Chat switch when this
    /// tab's agent can be shown as a chat.
    pub(crate) fn ai_panel_actions_with_view(&self, app: &AppContext) -> crate::console::ai_toolbar::AiPanelActions<crate::terminal::view::actions::TerminalAction> {
        use crate::terminal::view::actions::TerminalAction;
        let mut actions = Self::ai_panel_actions();
        if self.cli_chat_available(app) {
            actions.view_switch = Some((
                self.cli_chat_on,
                TerminalAction::SetChatView { chat: true },
                TerminalAction::SetChatView { chat: false },
            ));
            if self.cli_chat_on {
                let has_text = self.rich_input_editor.read(app, |editor, _| !editor.get_text().trim().is_empty());
                actions.send = Some(crate::console::ai_toolbar::SendButton {
                    busy: self.cli_chat_busy(app),
                    has_text,
                    send: TerminalAction::SubmitRichInput,
                    stop: TerminalAction::StopChatAgent,
                });
            }
        }
        actions
    }

    /// Switch this tab between its chat and its terminal.
    pub(crate) fn set_cli_chat(&mut self, on: bool, ctx: &mut ViewContext<Self>) {
        if self.cli_chat_on == on {
            return;
        }
        self.cli_chat_on = on;
        self.cli_chat_generation = self.cli_chat_generation.wrapping_add(1);
        if on && self.cli_chat_available(ctx) {
            // Typing goes through the input panel while the chat is shown
            // (the same steps as switching rich input on by hand).
            if !self.rich_input_active {
                self.rich_input_active = true;
                if self.is_remote {
                    self.remote_rich_input_closed = false;
                }
                self.set_rich_input_proxy_chars(0, ctx);
                self.last_dims = None;
                ctx.update_view(&self.rich_input_editor, |_editor, ectx| ectx.focus_self());
            }
            self.agent_transcript_follow.set(true);
            self.chat_window = CHAT_WINDOW;
            // The chat is the CLI's own session: what you send goes to the
            // agent running in this tab and the conversation shown is that
            // agent's. A second agent process (a driver) would be another
            // session, so none runs here; any from before is stopped.
            if let Some(agent) = self.acp_chat.take().and_then(|chat| chat.agent) {
                agent.shutdown();
            }
            self.read_cli_chat(self.cli_chat_generation, ctx);
        }
        ctx.notify();
    }

    fn read_cli_chat(&mut self, generation: u64, ctx: &mut ViewContext<Self>) {
        // An ACP chat hears from its agent directly: only the running clock
        // needs a tick.
        if self.acp_chat.is_some() {
            if self.acp_chat.as_ref().is_some_and(|chat| chat.working) {
                ctx.notify();
            }
            ctx.spawn(Timer::after(Duration::from_millis(1000)), move |view, _, ctx| {
                if view.cli_chat_generation == generation && view.showing_cli_chat(ctx) {
                    view.read_cli_chat(generation, ctx);
                }
            });
            return;
        }
        let pane = self.pane_uuid;
        let Some(provider) = self.cli_chat_provider(ctx) else {
            return;
        };
        let cwd = self.cwd(ctx);
        let seen = self.cli_chat.as_ref().and_then(|chat| chat.seen.clone());
        ctx.spawn(
            async move { tokio::task::spawn_blocking(move || read_chat(pane, provider, cwd, seen)).await.ok() },
            move |view, read, ctx| {
                if view.cli_chat_generation != generation || !view.showing_cli_chat(ctx) {
                    return;
                }
                let mut changed = false;
                if let Some(read) = &read {
                    let chat = view.cli_chat.get_or_insert_with(CliChat::default);
                    changed = read.rows.is_some() || chat.waiting != read.waiting || chat.unreadable != read.unreadable;
                    if let Some(rows) = read.rows.clone() {
                        chat.rows = rows;
                    }
                    chat.waiting = read.waiting;
                    chat.unreadable = read.unreadable;
                    chat.seen = read.seen.clone();
                    if read.model.is_some() && read.model != chat.model {
                        chat.model = read.model.clone();
                        // The session says which model it runs now.
                        view.cli_chat_model_pick = None;
                    }
                }
                // The sent message is in the conversation now (or never will be).
                if let Some(sent) = &view.cli_chat_sent {
                    let rows = view.cli_chat.as_ref().map_or(&[][..], |chat| &chat.rows[..]);
                    let arrived = rows.iter().skip(sent.rows_before).any(|row| row.role == "user");
                    if arrived || sent.at.elapsed() > SENT_GIVE_UP {
                        view.cli_chat_sent = None;
                        changed = true;
                    }
                }
                let working = view.cli_chat_busy(ctx);
                if let Some(chat) = view.cli_chat.as_mut() {
                    changed |= chat.working != working;
                    chat.working = working;
                }
                // The timer counts seconds while the agent works.
                if changed || working {
                    ctx.notify();
                }
                let base = if working || view.cli_chat_sent.is_some() { READ_WORKING } else { READ_IDLE };
                // A huge conversation takes a while to read: never spend more
                // than about a third of the time reading it.
                let next = read.as_ref().map_or(base, |r| base.max(r.took * 3));
                ctx.spawn(Timer::after(next), move |view, _, ctx| {
                    if view.cli_chat_generation == generation {
                        view.read_cli_chat(generation, ctx);
                    }
                });
            },
        );
    }

    /// The conversation, in place of the terminal picture.
    pub(crate) fn render_cli_chat(&self, app: &AppContext) -> Box<dyn Element> {
        // Follow the newest line unless the person scrolled up to read.
        let offset = self.agent_transcript_scroll.scroll_start().as_f32();
        let (follow, last) = super::terminal_agent_transcript::follow_transition(
            offset,
            self.agent_transcript_last_scroll.get(),
            self.agent_transcript_scroll.max_scroll().as_f32(),
            self.agent_transcript_follow.get(),
        );
        self.agent_transcript_follow.set(follow);
        self.agent_transcript_last_scroll.set(last);
        let theme = self.theme;
        let ui = crate::fonts::ui_role(self.mono_family);
        let mut context = self.block_render_context(app);
        context.content_width_px = (self.output_viewport_width_px - 2.0 * crate::theme::TERMINAL_PADDING_LEFT).max(80.0);
        let empty_open = HashSet::new();
        let view = chat_view::ChatView {
            ctx: &context,
            theme,
            ui,
            mono: self.mono_family,
            now_ms: chrono::Utc::now().timestamp_millis(),
            open: if self.chat_open.is_empty() { &empty_open } else { &self.chat_open },
            you_extras: None,
        };
        let body: Box<dyn Element> = if let Some(acp) = &self.acp_chat {
            match self.acp_note() {
                Some(text) => note(&text, &view),
                None => {
                    let (rows, hidden) = recent_rows(&acp.conversation.rows, self.chat_window);
                    let entries = steps::entries_from_rows(rows, acp.working);
                    let lines = layout::layout(&entries, acp.working, acp.permission.is_some());
                    let conversation = with_earlier(
                        hidden,
                        chat_view::render(&lines, &view, &|_: &()| warpui::elements::Empty::new().finish()),
                        &view,
                    );
                    match self.render_acp_permission(app) {
                        Some(card) => Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_child(conversation)
                            .with_child(Container::new(card).with_padding_top(10.0).finish())
                            .finish(),
                        None => conversation,
                    }
                }
            }
        } else {
            match &self.cli_chat {
            None => note("Reading the conversation…", &view),
            Some(chat) if chat.unreadable => {
                note("This agent's conversation can't be shown as a chat. Switch back to CLI below.", &view)
            }
            Some(chat) if chat.rows.is_empty() && self.cli_chat_sent.is_none() => {
                note("Nothing here yet. Send a message below to start.", &view)
            }
            Some(chat) => {
                let (rows, hidden) = recent_rows(&chat.rows, self.chat_window);
                let mut entries = steps::entries_from_rows(rows, chat.working);
                if let Some(sent) = &self.cli_chat_sent {
                    entries.push(layout::Entry::You {
                        id: "chat-sending".into(),
                        text: sent.text.clone(),
                        at: Some(chrono::Utc::now().timestamp_millis() - sent.at.elapsed().as_millis() as i64),
                        note: Some(layout::YouNote::Sending),
                    });
                }
                let lines = layout::layout(&entries, chat.working, chat.waiting);
                with_earlier(hidden, chat_view::render(&lines, &view, &|_: &()| warpui::elements::Empty::new().finish()), &view)
            }
            }
        };
        let document = Container::new(
            Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_child(body).finish(),
        )
        .with_padding_left(crate::theme::TERMINAL_PADDING_LEFT)
        .with_padding_right(crate::theme::TERMINAL_PADDING_LEFT)
        .with_padding_top(16.0)
        .with_padding_bottom(24.0)
        .finish();
        let selected = self.block_selected_text.clone();
        let selectable = SelectableArea::new(
            self.block_selectable_handle.clone(),
            move |args: warpui::elements::SelectionUpdateArgs, _: &mut warpui::EventContext, _: &AppContext| {
                *selected.write() = args.selection;
            },
            document,
        )
        .finish();
        if self.agent_transcript_follow.get() {
            self.agent_transcript_scroll.scroll_to(warpui_core::units::Pixels::new(f32::MAX));
        }
        // Scrolled up to read: a "Latest" button brings the newest line back.
        let away = !self.agent_transcript_follow.get() && self.agent_transcript_scroll.max_scroll().as_f32() > 0.0;
        let scroller = Container::new(
            ClippedScrollable::vertical(
                self.agent_transcript_scroll.clone(),
                selectable,
                ScrollbarWidth::Auto,
                theme.disabled_text().into(),
                theme.sub_text().into(),
                theme.pane_surface().into(),
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_background_color(theme.pane_surface())
        .finish();
        if !away {
            return scroller;
        }
        use warpui::elements::{ChildAnchor, OffsetPositioning, ParentAnchor, ParentOffsetBounds, Stack};
        Box::new(Stack::new().with_child(scroller).with_positioned_child(
            latest_button(&theme, ui),
            OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0, -16.0),
                ParentOffsetBounds::ParentBySize,
                ParentAnchor::BottomMiddle,
                ChildAnchor::BottomMiddle,
            ),
        ))
    }
}

/// The floating "↓ Latest" pill over a chat scrolled away from its end.
fn latest_button(theme: &crate::theme::Theme, ui: warpui::fonts::FamilyId) -> Box<dyn Element> {
    use crate::terminal::view::actions::TerminalAction;
    use warpui::elements::{DispatchEventResult, EventHandler};
    use warpui_core::scene::{Border, CornerRadius, DropShadow, Radius};
    // A little see-through, so it floats over the conversation without
    // hiding it, with a light ring and a soft shadow so it stands out on
    // every theme.
    let tint = |color: warpui::color::ColorU, alpha: u8| warpui::color::ColorU::new(color.r, color.g, color.b, alpha);
    let pill = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(crate::icons::render_icon(crate::icons::AppIcon::ChevronDown, theme.main_text(), 13.0))
            .with_child(Text::new("Latest", ui, crate::theme::text_size(12.5)).with_color(theme.main_text()).finish())
            .finish(),
    )
    .with_background_color(tint(theme.surface_3(), 225))
    .with_border(Border::all(1.0).with_border_color(tint(theme.main_text(), 46)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(999.0)))
    .with_drop_shadow(DropShadow {
        color: warpui::color::ColorU::new(0, 0, 0, 90),
        offset: warpui::geometry::vector::vec2f(0.0, 2.0),
        blur_radius: 10.0,
        spread_radius: 0.0,
    })
    .with_padding_left(12.0)
    .with_padding_right(14.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .finish();
    EventHandler::new(pill)
        .on_left_mouse_down(|event_ctx, _, _| {
            event_ctx.dispatch_typed_action(TerminalAction::JumpToLatestChat);
            DispatchEventResult::StopPropagation
        })
        .finish()
}

/// Claude Code's models, listed once per run by asking a throwaway
/// `claude` (in a scratch folder, no message sent, so no conversation is
/// made). `None` until the answer is in.
/// Claude Code's models for the chat's picker, at once: its own list when
/// known (this run or saved), else the bundled catalog, after its default.
fn claude_models() -> Option<Vec<dragent_acp::Choice>> {
    Some(instant_choices("claude"))
}

/// An agent's models as picker choices, without waiting for the agent: its
/// default first, then its own list (or the catalog).
pub(crate) fn instant_choices(key: &str) -> Vec<dragent_acp::Choice> {
    let mut choices = vec![dragent_acp::Choice { id: "default".into(), name: "Default".into(), description: None }];
    choices.extend(
        crate::teams::models::choices_for_key(key)
            .into_iter()
            .filter(|(id, _)| id != "default")
            .map(|(id, name)| dragent_acp::Choice { id, name, description: None }),
    );
    choices
}

/// The effort levels an agent takes, when known before it says so itself:
/// Claude Code's `--effort` / `/effort`, Codex's reasoning effort.
pub(crate) fn known_efforts(key: &str) -> Option<Vec<dragent_acp::Choice>> {
    matches!(key, "claude" | "codex").then(|| {
        ["low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .map(|level| dragent_acp::Choice { id: level.into(), name: level.into(), description: None })
            .collect()
    })
}

/// The choice a reply's model (`claude-opus-5-5`) belongs to: itself when
/// listed, else its family's alias (`opus`), else the id as it is.
fn claude_choice_for(choices: &[dragent_acp::Choice], model: &str) -> String {
    if choices.iter().any(|c| c.id == model) {
        return model.to_string();
    }
    ["opus", "sonnet", "haiku", "fable"]
        .into_iter()
        .find(|family| model.contains(family) && choices.iter().any(|c| c.id == *family))
        .map(str::to_string)
        .unwrap_or_else(|| model.to_string())
}

/// The model of the latest reply in a Claude Code transcript (its last part
/// is enough: replies name their model).
fn last_claude_model(path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(512 * 1024))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    let key = "\"model\":\"claude-";
    let at = tail.rfind(key)? + "\"model\":\"".len();
    let rest = &tail[at..];
    Some(rest[..rest.find('"')?].to_string())
}

/// How many rows of a conversation are drawn at first, and how many more
/// each "Show earlier messages" adds. A long session holds thousands; drawing
/// them all on every frame froze the app.
pub(crate) const CHAT_WINDOW: usize = 200;

/// The last `window` rows (moved back to where a turn starts, so a turn is
/// never cut in half) and how many earlier rows are left out.
pub(crate) fn recent_rows(rows: &[Row], window: usize) -> (&[Row], usize) {
    if rows.len() <= window {
        return (rows, 0);
    }
    let mut start = rows.len() - window;
    // Back to your message that opens this turn, within one more window.
    let floor = start.saturating_sub(window);
    while start > floor && rows[start].role != "user" {
        start -= 1;
    }
    if rows[start].role != "user" {
        start = rows.len() - window;
    }
    (&rows[start..], start)
}

/// The conversation with a "Show earlier messages" row above it when rows
/// were left out.
fn with_earlier(hidden: usize, conversation: Box<dyn Element>, view: &chat_view::ChatView<'_>) -> Box<dyn Element> {
    use crate::terminal::view::actions::TerminalAction;
    use warpui::elements::{DispatchEventResult, EventHandler};
    if hidden == 0 {
        return conversation;
    }
    let label = format!("Show {} earlier message{}", hidden.min(CHAT_WINDOW), if hidden.min(CHAT_WINDOW) == 1 { "" } else { "s" });
    let link = EventHandler::new(
        Container::new(Text::new(label, view.ui, crate::theme::text_size(12.5)).with_color(view.theme.accent).finish())
            .with_padding_top(4.0)
            .with_padding_bottom(12.0)
            .finish(),
    )
    .on_left_mouse_down(|event_ctx, _, _| {
        event_ctx.dispatch_typed_action(TerminalAction::ShowEarlierChat);
        DispatchEventResult::StopPropagation
    })
    .finish();
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(link)
        .with_child(conversation)
        .finish()
}

fn note(words: &str, view: &chat_view::ChatView<'_>) -> Box<dyn Element> {
    Container::new(
        Text::new(words.to_string(), view.ui, crate::theme::text_size(13.0)).with_color(view.theme.sub_text()).finish(),
    )
    .with_padding_top(24.0)
    .finish()
}

#[cfg(test)]
mod busy_tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_spinner_drawing_steadily_reads_as_working_and_silence_as_done() {
        let start = web_time::Instant::now();
        // Drawing for 5 s, last frame just now: working.
        assert!(busy_from_output(start + ms(5000), Some(start + ms(4950)), Some(start), None));
        // Through a quiet stretch of a long tool call (under 3.5 s): still working.
        assert!(busy_from_output(start + ms(8000), Some(start + ms(5000)), Some(start), None));
        // Silent past that: done.
        assert!(!busy_from_output(start + ms(9000), Some(start + ms(5000)), Some(start), None));
    }

    #[test]
    fn a_single_redraw_is_not_work() {
        let start = web_time::Instant::now();
        // A resize or echo: one short burst.
        assert!(!busy_from_output(start + ms(200), Some(start + ms(100)), Some(start), None));
        assert!(!busy_from_output(start, None, None, None));
    }

    #[test]
    fn a_message_just_sent_counts_as_work_starting() {
        let start = web_time::Instant::now();
        assert!(busy_from_output(start + ms(1000), None, None, Some(start)));
        // The agent never drew: not working once the grace is over.
        assert!(!busy_from_output(start + ms(5000), None, None, Some(start)));
    }

    #[test]
    fn each_agent_stops_on_its_own_key() {
        assert_eq!(stop_keys("claude"), &[b"\x1b" as &[u8]]);
        assert_eq!(stop_keys("codex"), &[b"\x1b" as &[u8]]);
        assert_eq!(stop_keys("opencode").len(), 2);
        assert_eq!(stop_keys("mimo").len(), 2);
        assert_eq!(stop_keys("cursor-agent"), &[b"\x03" as &[u8]]);
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn rows(roles: &[&'static str]) -> Vec<Row> {
        roles.iter().enumerate().map(|(i, r)| Row::new(i.to_string(), r, String::new())).collect()
    }

    #[test]
    fn a_short_conversation_is_drawn_whole() {
        let all = rows(&["user", "assistant"]);
        assert_eq!(recent_rows(&all, 200), (&all[..], 0));
    }

    #[test]
    fn a_long_one_starts_where_a_turn_starts() {
        // 10 turns of 30 rows: you, then 29 steps and replies.
        let mut roles = Vec::new();
        for _ in 0..10 {
            roles.push("user");
            roles.extend(std::iter::repeat_n("tool", 29));
        }
        let all = rows(&roles);
        let (shown, hidden) = recent_rows(&all, 100);
        assert_eq!(shown[0].role, "user", "never half a turn");
        assert_eq!(hidden + shown.len(), all.len());
        assert!(shown.len() >= 100 && shown.len() < 130);
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    fn choices(ids: &[&str]) -> Vec<dragent_acp::Choice> {
        ids.iter().map(|id| dragent_acp::Choice { id: id.to_string(), name: id.to_string(), description: None }).collect()
    }

    #[test]
    fn a_replys_model_maps_to_its_listed_choice() {
        let list = choices(&["default", "opus", "sonnet", "haiku", "claude-sonnet-5"]);
        assert_eq!(claude_choice_for(&list, "claude-opus-5-5"), "opus", "the family's alias");
        assert_eq!(claude_choice_for(&list, "claude-sonnet-5"), "claude-sonnet-5", "listed as itself");
        assert_eq!(claude_choice_for(&list, "claude-mystery-1"), "claude-mystery-1", "unknown: as it is");
    }

    #[test]
    fn the_latest_model_is_read_from_the_transcripts_end() {
        let path = std::env::temp_dir().join(format!("claude-model-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            "{\"message\":{\"model\":\"claude-opus-5-5\"}}\n{\"type\":\"user\"}\n{\"message\":{\"model\":\"claude-haiku-5-5\",\"id\":\"m\"}}\n",
        )
        .unwrap();
        assert_eq!(last_claude_model(&path).as_deref(), Some("claude-haiku-5-5"));
        std::fs::remove_file(&path).ok();
    }
}
