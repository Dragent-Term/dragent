//! A tab's chat driven straight through the agent.
//!
//! The chat doesn't read a transcript and type into a terminal: Dragent starts
//! the agent in its machine-readable mode and talks to it directly — ACP for
//! OpenCode, Gemini, Qwen, Cursor and Copilot, the app-server for Codex,
//! streaming JSON for Claude Code (see `dragent_acp`). That gives the chat the
//! agent's own model list (switched from the same model selector Dragent's
//! agent uses), a reply that streams as it is written, exact working state,
//! permission questions to answer in place, and a Stop that cancels the turn.
//!
//! The chat continues this tab's conversation when the agent can load it,
//! otherwise it starts a new one.

use std::sync::Arc;

use dragent_acp::{ChatDriver, Event, PermissionRequest, SetVia, StopReason};

use super::acp_conversation::AcpConversation;
use super::terminal_view::TerminalView;
use crate::terminal::view::actions::TerminalAction;
use warpui::elements::{
    Container, CrossAxisAlignment, DispatchEventResult, Element, EventHandler, Flex, ParentElement, Text,
};
use warpui::{AppContext, ViewContext};
use warpui_core::scene::{Border, CornerRadius, Radius};

/// Where the agent is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AcpStatus {
    Starting,
    Ready,
    Failed(String),
}

/// A tab's ACP chat.
pub(crate) struct AcpChat {
    pub agent: Option<Arc<dyn ChatDriver>>,
    pub session: Option<String>,
    pub conversation: AcpConversation,
    pub status: AcpStatus,
    /// A turn is running.
    pub working: bool,
    /// The agent asks before running a tool.
    pub permission: Option<PermissionRequest>,
    /// Sent before the agent was ready: goes out once it is.
    pending: Option<String>,
    /// The agent's command (`codex`, `opencode`…), for its models before it
    /// has started.
    provider: String,
    /// A model or effort picked before the agent was ready: set once it is.
    pending_model: Option<String>,
    pending_effort: Option<String>,
    agent_name: String,
    /// When the agent was restarted after stopping on its own (recent ones).
    restarts: Vec<web_time::Instant>,
}

/// Restarts allowed within [`RESTART_WINDOW`] before the chat gives up.
const RESTARTS: usize = 2;
const RESTART_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// What a start produced.
struct Started {
    agent: Arc<dyn ChatDriver>,
    session: dragent_acp::Session,
    events: std::sync::mpsc::Receiver<Event>,
    /// The conversation so far, when the agent doesn't replay it itself.
    history: Vec<dragent_host::agent_transcript::Row>,
}

/// Start `provider`'s chat driver in `cwd` and open a session: `resume`
/// when the agent can continue it, a new one otherwise. Blocking.
fn start(
    pane: uuid::Uuid,
    provider: String,
    program: String,
    cwd: std::path::PathBuf,
    resume: Option<String>,
) -> Result<Started, String> {
    // The same copy of the agent the terminal runs, found the same way (an
    // app opened from the Finder has a narrow PATH).
    let command = dragent_host::host_tools::command(&program);
    let path = std::path::PathBuf::from(command.get_program());
    let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    let env: Vec<(String, String)> = command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
        .collect();
    // Claude Code continues a conversation only from the folder spelling it
    // was filed under, and one never saved can't be continued at all.
    let (resume, run_in) = match (provider.as_str(), resume) {
        ("claude", Some(id)) => match dragent_host::agent_transcript::claude_conversation_file(&id) {
            Some(file) => {
                let dir = dragent_host::agent_transcript::claude_resume_dir(&file, &cwd).unwrap_or_else(|| cwd.clone());
                (Some(id), dir)
            }
            None => (None, cwd.clone()),
        },
        (_, resume) => (resume, cwd.clone()),
    };
    let agent = dragent_acp::start(&provider, &path, &args, &run_in, &env).map_err(|e| format!("Couldn't start {program}: {e}"))?;
    let events = agent.take_events().ok_or("no events")?;
    let session = agent.open(&run_in, resume.as_deref()).map_err(|e| {
        if e.needs_login() {
            format!("Sign in to {program} first (run it in a terminal once), then switch to Chat again. ({})", e.message)
        } else {
            // Say why when the agent said why (its error output).
            match agent.error_output() {
                Some(why) => format!("{program} stopped while starting: {why}"),
                None => e.message,
            }
        }
    })?;
    // Continuing a conversation the agent doesn't replay: show it from the
    // agent's own transcript.
    // Only when that conversation really was continued: a new one (the
    // agent couldn't reopen it) starts empty rather than looking continued.
    let history = match (&resume, agent.replays_history()) {
        (Some(id), false) if *id == session.id => conversation_so_far(pane, &provider, &cwd),
        _ => Vec::new(),
    };
    Ok(Started { agent, session, events, history })
}

/// The tab's conversation so far as transcript rows: the one its session
/// record names, else `provider`'s newest in `cwd` (empty when unreadable).
fn conversation_so_far(pane: uuid::Uuid, provider: &str, cwd: &std::path::Path) -> Vec<dragent_host::agent_transcript::Row> {
    use dragent_host::agent_transcript::{self, Extent};
    dragent_host::agent_sessions::load(pane)
        .filter(|record| record.provider == provider)
        .and_then(|record| agent_transcript::source(&record))
        .or_else(|| agent_transcript::latest_source(provider, cwd))
        .and_then(|source| agent_transcript::read_source(&source, Extent::Full).ok())
        .unwrap_or_default()
}

impl TerminalView {
    /// This tab's agent can run its chat over ACP.
    pub(crate) fn acp_launch<C: warpui::ReadModel>(&self, ctx: &C) -> Option<(String, Vec<String>)> {
        let provider = self.cli_chat_provider(ctx)?;
        let (_, program, args) = dragent_acp::driver_for(&provider)?;
        Some((program.to_string(), args.iter().map(|a| a.to_string()).collect()))
    }

    /// The chat on show is an ACP chat.
    pub(crate) fn acp_chat_shown<C: warpui::ReadModel>(&self, ctx: &C) -> bool {
        self.acp_chat.is_some() && self.showing_cli_chat(ctx)
    }

    /// Start the agent for this tab's chat (once).
    pub(crate) fn start_acp_chat(&mut self, ctx: &mut ViewContext<Self>) {
        if self.acp_chat.as_ref().is_some_and(|chat| !matches!(chat.status, AcpStatus::Failed(_))) {
            return;
        }
        let Some((program, _)) = self.acp_launch(ctx) else { return };
        let provider = self.cli_chat_provider(ctx).unwrap_or_default();
        let cwd = self.cwd(ctx);
        // The conversation this tab's agent is in, to continue it.
        let resume = dragent_host::agent_sessions::load(self.pane_uuid)
            .filter(|record| record.provider == provider)
            .and_then(|record| record.conversation_id)
            .or_else(|| dragent_host::agent_transcript::latest_source(&provider, &cwd).and_then(|s| s.session));
        let agent_name = self.active_ai_tool(ctx).map(|t| t.label().to_string()).unwrap_or_else(|| program.clone());
        self.acp_chat = Some(AcpChat {
            agent: None,
            session: None,
            conversation: AcpConversation::default(),
            status: AcpStatus::Starting,
            working: false,
            permission: None,
            pending: None,
            provider: provider.clone(),
            pending_model: None,
            pending_effort: None,
            agent_name,
            restarts: Vec::new(),
        });
        self.launch_acp(program, provider, cwd, resume, ctx);
        ctx.notify();
    }

    /// The agent stopped on its own: start it again on the same
    /// conversation, keeping what the chat shows. Gives up after a few quick
    /// failures, saying why.
    fn restart_acp_chat(&mut self, ctx: &mut ViewContext<Self>) {
        let Some((program, _)) = self.acp_launch(ctx) else { return };
        let provider = self.cli_chat_provider(ctx).unwrap_or_default();
        let cwd = self.cwd(ctx);
        let Some(chat) = self.acp_chat.as_mut() else { return };
        let why = chat.agent.as_ref().and_then(|agent| agent.error_output());
        chat.agent = None;
        chat.working = false;
        chat.permission = None;
        chat.conversation.end_turn();
        let now = web_time::Instant::now();
        chat.restarts.retain(|at| now.duration_since(*at) < RESTART_WINDOW);
        if chat.restarts.len() >= RESTARTS || chat.session.is_none() {
            let reason = why.map(|w| format!(" ({w})")).unwrap_or_default();
            chat.status = AcpStatus::Failed(format!("The agent keeps stopping{reason}. Switch to CLI and back to try again."));
            chat.conversation.note(&format!("⚠ The agent stopped{reason}."));
            return;
        }
        chat.restarts.push(now);
        chat.status = AcpStatus::Starting;
        let resume = chat.session.clone();
        self.launch_acp(program, provider, cwd, resume, ctx);
    }

    /// Start the driver off the UI thread and connect the chat to it.
    fn launch_acp(
        &mut self,
        program: String,
        provider: String,
        cwd: std::path::PathBuf,
        resume: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let pane = self.pane_uuid;
        ctx.spawn(
            async move { tokio::task::spawn_blocking(move || start(pane, provider, program, cwd, resume)).await.ok() },
            |view, started, ctx| {
                let Some(chat) = view.acp_chat.as_mut() else { return };
                match started {
                    Some(Ok(started)) => {
                        if chat.conversation.rows.is_empty() {
                            chat.conversation.preload(started.history);
                        } else if started.agent.replays_history() {
                            // A restart: the agent replays the conversation, so
                            // the chat rebuilds from that instead of doubling it.
                            chat.conversation = AcpConversation::default();
                        }
                        chat.conversation.take_options(started.session.options.clone());
                        chat.session = Some(started.session.id.clone());
                        chat.agent = Some(started.agent);
                        chat.status = AcpStatus::Ready;
                        view.pump_acp_events(started.events, ctx);
                        view.apply_pending_picks(ctx);
                        if let Some(text) = view.acp_chat.as_mut().and_then(|c| c.pending.take()) {
                            view.acp_send(text, ctx);
                        }
                    }
                    Some(Err(error)) => {
                        chat.working = false;
                        chat.pending = None;
                        if !chat.conversation.rows.is_empty() {
                            chat.conversation.note(&format!("⚠ {error}"));
                        }
                        chat.status = AcpStatus::Failed(error);
                    }
                    None => chat.status = AcpStatus::Failed("The agent couldn't start.".into()),
                }
                ctx.notify();
            },
        );
    }

    /// Bring the agent's own messages to the UI thread.
    fn pump_acp_events(&mut self, events: std::sync::mpsc::Receiver<Event>, ctx: &mut ViewContext<Self>) {
        let (tx, rx) = async_channel::unbounded::<Event>();
        std::thread::Builder::new()
            .name("acp-events".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if tx.send_blocking(event).is_err() {
                        break;
                    }
                }
            })
            .ok();
        ctx.spawn_stream_local(
            rx,
            |view, event, ctx| {
                let Some(chat) = view.acp_chat.as_mut() else { return };
                match event {
                    Event::Update { session_id, update } => {
                        if chat.session.as_deref() == Some(session_id.as_str()) || session_id.is_empty() {
                            chat.conversation.apply(update);
                        }
                    }
                    Event::Permission(request) => chat.permission = Some(request),
                    Event::Exited => {
                        // Only the connected agent's ending counts (a replaced
                        // one's is old news).
                        if chat.agent.is_some() && chat.status == AcpStatus::Ready {
                            view.restart_acp_chat(ctx);
                        }
                    }
                }
                ctx.notify();
            },
            |_, _| {},
        );
    }

    /// Send a message from the chat.
    pub(crate) fn acp_send(&mut self, text: String, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        self.agent_transcript_follow.set(true);
        match (&chat.status, chat.agent.clone(), chat.session.clone()) {
            (AcpStatus::Ready, Some(agent), Some(session)) => {
                chat.conversation.you(&text);
                chat.working = true;
                ctx.spawn(
                    async move { tokio::task::spawn_blocking(move || agent.prompt(&session, &text)).await.ok() },
                    |view, ended, ctx| {
                        let Some(chat) = view.acp_chat.as_mut() else { return };
                        chat.working = false;
                        chat.permission = None;
                        chat.conversation.end_turn();
                        match ended {
                            Some(Ok(StopReason::EndTurn | StopReason::Cancelled)) => {}
                            Some(Ok(StopReason::Refusal)) => chat.conversation.note("The agent declined to continue."),
                            Some(Ok(StopReason::MaxTokens)) => chat.conversation.note("Stopped: the reply hit the model's length limit."),
                            Some(Ok(StopReason::MaxTurnRequests)) => chat.conversation.note("Stopped: the agent reached its step limit for one turn."),
                            Some(Ok(StopReason::Other(_))) => {}
                            Some(Err(error)) => chat.conversation.note(&format!("⚠ {}", error.message)),
                            None => chat.conversation.note("⚠ The message couldn't be sent."),
                        }
                        ctx.notify();
                    },
                );
            }
            (AcpStatus::Starting, _, _) => {
                chat.conversation.you(&text);
                chat.pending = Some(text);
                chat.working = true;
            }
            (AcpStatus::Failed(_), _, _) => {
                // The agent is down: start it again (fresh tries) and send
                // once it's up.
                chat.conversation.you(&text);
                chat.pending = Some(text);
                chat.working = true;
                chat.restarts.clear();
                chat.status = AcpStatus::Starting;
                chat.agent = None;
                let resume = chat.session.clone();
                if let Some((program, _)) = self.acp_launch(ctx) {
                    let provider = self.cli_chat_provider(ctx).unwrap_or_default();
                    let cwd = self.cwd(ctx);
                    self.launch_acp(program, provider, cwd, resume, ctx);
                }
            }
            _ => {}
        }
        ctx.notify();
    }

    /// Stop the running turn.
    pub(crate) fn acp_stop(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        if let Some(request) = chat.permission.take() {
            if let Some(agent) = &chat.agent {
                let _ = agent.answer_permission(&request.request, None);
            }
        }
        if let (Some(agent), Some(session)) = (&chat.agent, &chat.session) {
            let _ = agent.cancel(session);
        }
        if chat.pending.take().is_some() {
            chat.working = false;
        }
        ctx.notify();
    }

    /// Choose `value` in the picker set `via` (the model, from the selector).
    pub(crate) fn acp_set_option(&mut self, via: SetVia, value: String, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        if let Some(picker) = chat.conversation.options.picker_mut(&via) {
            picker.current = value.clone();
        }
        let (Some(agent), Some(session)) = (chat.agent.clone(), chat.session.clone()) else { return };
        ctx.spawn(
            async move { tokio::task::spawn_blocking(move || agent.set_option(&session, &via, &value)).await.ok() },
            |view, answer, ctx| {
                let Some(chat) = view.acp_chat.as_mut() else { return };
                match answer {
                    Some(Ok(Some(options))) => chat.conversation.take_options(options),
                    Some(Err(error)) => chat.conversation.note(&format!("⚠ Couldn't switch: {}", error.message)),
                    _ => {}
                }
                ctx.notify();
            },
        );
        ctx.notify();
    }

    /// Answer the agent's permission question.
    pub(crate) fn acp_answer_permission(&mut self, option: Option<String>, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        if let (Some(request), Some(agent)) = (chat.permission.take(), &chat.agent) {
            let _ = agent.answer_permission(&request.request, option.as_deref());
        }
        ctx.notify();
    }

    /// The agent's model list, for the model selector. Until the agent has
    /// started (seconds), its known list, so the picker is there at once.
    pub(crate) fn acp_model_picker<C: warpui::ReadModel>(&self, ctx: &C) -> Option<dragent_acp::Picker> {
        if !self.acp_chat_shown(ctx) {
            return None;
        }
        let chat = self.acp_chat.as_ref()?;
        if let Some(picker) = chat.conversation.options.model.clone() {
            return Some(picker);
        }
        if chat.session.is_some() {
            return None;
        }
        let choices = super::terminal_cli_chat::instant_choices(&chat.provider);
        (choices.len() > 1).then(|| dragent_acp::Picker {
            current: chat.pending_model.clone().unwrap_or_else(|| "default".into()),
            choices,
            via: SetVia::Model,
        })
    }

    /// The agent's effort levels, when it has them; before it has started,
    /// the levels it's known to take.
    pub(crate) fn acp_effort_picker<C: warpui::ReadModel>(&self, ctx: &C) -> Option<dragent_acp::Picker> {
        if !self.acp_chat_shown(ctx) {
            return None;
        }
        let chat = self.acp_chat.as_ref()?;
        if let Some(picker) = chat.conversation.options.effort.clone() {
            return Some(picker);
        }
        if chat.session.is_some() {
            return None;
        }
        Some(dragent_acp::Picker {
            current: chat.pending_effort.clone().unwrap_or_default(),
            choices: super::terminal_cli_chat::known_efforts(&chat.provider)?,
            via: SetVia::ConfigOption("effort".into()),
        })
    }

    /// Pick a model: now when the agent is ready, else once it is.
    pub(crate) fn acp_choose_model(&mut self, value: String, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        match chat.conversation.options.model.as_ref().map(|p| p.via.clone()) {
            Some(via) if chat.session.is_some() => self.acp_set_option(via, value, ctx),
            _ => {
                chat.pending_model = Some(value);
                ctx.notify();
            }
        }
    }

    /// Pick an effort level: now when the agent is ready, else once it is.
    pub(crate) fn acp_choose_effort(&mut self, value: String, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        match chat.conversation.options.effort.as_ref().map(|p| p.via.clone()) {
            Some(via) if chat.session.is_some() => self.acp_set_option(via, value, ctx),
            _ => {
                chat.pending_effort = Some(value);
                ctx.notify();
            }
        }
    }

    /// What was picked while the agent started, set now it's ready.
    fn apply_pending_picks(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(chat) = self.acp_chat.as_mut() else { return };
        let model = chat.pending_model.take().filter(|m| m != "default");
        let effort = chat.pending_effort.take();
        if let Some(model) = model {
            self.acp_choose_model(model, ctx);
        }
        if let Some(effort) = effort {
            // An agent that turned out not to have efforts drops the pick.
            if self.acp_chat.as_ref().is_some_and(|c| c.conversation.options.effort.is_some()) {
                self.acp_choose_effort(effort, ctx);
            }
        }
    }

    /// What the chat says in place of the conversation while there is none.
    pub(crate) fn acp_note(&self) -> Option<String> {
        let chat = self.acp_chat.as_ref()?;
        match &chat.status {
            AcpStatus::Starting if chat.conversation.rows.is_empty() => Some(format!("Starting {}…", chat.agent_name)),
            AcpStatus::Failed(error) if chat.conversation.rows.is_empty() => Some(error.clone()),
            AcpStatus::Ready if chat.conversation.rows.is_empty() => {
                Some(format!("Chatting with {}. Pick a model below and send a message to start.", chat.agent_name))
            }
            _ => None,
        }
    }

    /// The agent's permission question with its answers, under the chat.
    pub(crate) fn render_acp_permission(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let request = self.acp_chat.as_ref()?.permission.as_ref()?;
        let theme = self.theme;
        let mono = self.mono_family;
        let _ = app;
        let title = if request.tool.title.is_empty() { "run a tool".to_string() } else { request.tool.title.clone() };
        let mut buttons = Flex::row().with_spacing(6.0);
        for option in &request.options {
            let allow = option.kind.starts_with("allow");
            let color = if allow { theme.accent } else { theme.sub_text() };
            let id = option.id.clone();
            let button = Container::new(Text::new(option.name.clone(), mono, crate::theme::text_size(11.5)).with_color(color).finish())
                .with_padding_left(9.0)
                .with_padding_right(9.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_border(Border::all(1.0).with_border_color(color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish();
            buttons = buttons.with_child(
                EventHandler::new(button)
                    .on_left_mouse_down(move |event_ctx, _, _| {
                        event_ctx.dispatch_typed_action(TerminalAction::AnswerAcpPermission { option: Some(id.clone()) });
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
        }
        Some(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(8.0)
                    .with_child(
                        Text::new(format!("Allow the agent to {title}?"), mono, crate::theme::text_size(12.5))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .with_child(buttons.finish())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_border(Border::all(1.0).with_border_color(theme.ui_yellow()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish(),
        )
    }
}
