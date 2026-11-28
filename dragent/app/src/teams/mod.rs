//! The Teams page: this organisation's teams, and one team's plan, its
//! workers and its messages, kept up to date while the page is open. It
//! reads the background service's team store (`dragent_host::team`) and
//! acts through the same operations the `dragent` command runs, as the
//! person (`cli::team`).

mod board;
mod page;
mod summary;
pub(crate) mod sidebar;
pub(crate) mod live_models;
pub(crate) mod models;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use dragent_host::team::{Party, TaskStatus, Team, TeamRequest, TeamResponse, TeamSummary};
use dragent_host::HostClient;
use serde_json::json;
use warpui::elements::ClippedScrollStateHandle;
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Element, Entity, EntityId, TypedActionView, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::elements::MouseStateHandle;

use crate::theme::Theme;
use crate::workspace::{InputEditor, InputMode};

/// How often an open page looks again.
const REFRESH: Duration = Duration::from_secs(2);

/// What a running worker's agent is doing, from its hook reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerState {
    Working,
    /// Waiting at an approval prompt: the person's to answer.
    NeedsApproval,
    /// Its turn ended.
    Idle,
    /// Its agent doesn't report, or isn't running.
    Unknown,
}

#[derive(Clone, Debug)]
pub enum TeamsAction {
    Select(String),
    /// Which coordinations the list shows.
    Filter(ListFilter),
    /// The selected one's Overview or Activity.
    Tab(DetailTab),
    /// Show the lead's terminal.
    OpenLead,
    /// Show the terminal of the worker on this task.
    OpenWorker(String),
    /// Wake a sleeping agent (its session's pane) through the wake request
    /// the sleep sweep answers.
    Wake(String),
    /// Open or close a task's details, or the lead's message.
    ToggleDetails(String),
    AskStop,
    CancelStop,
    ConfirmStop,
    CancelTask(String),
    /// Send what's in the message box to the lead.
    Send,
    /// Start or stop dictating into the message box.
    ToggleVoice,
    /// Fold the coordinations list away, or bring it back.
    ToggleList,
    /// A one-click answer to the lead that isn't a reply to a message
    /// (its turn ended asking, in its terminal).
    Say(String),
    /// The person clicked one of the answers a question to them offers:
    /// it goes to the lead as their answer.
    Choose { message: u64, choice: String },
    /// Answer an agent's permission prompt from the page: allow (Enter)
    /// or deny (Esc). `task` is the worker's; none is the lead's.
    Approve { task: Option<String>, allow: bool },
    /// How many workers of one agent may run at once.
    SetParallel(u32),
    /// Whether tasks start on their own once what they wait for is done.
    SetAutoStart(bool),
    SetLeadAnswers(bool),
    ChooseTaskModel(String),
    SetTaskModel { task: String, model: Option<String> },
    CloseModelPicker,
    ClearModelSearch,
    BrowseModelTab(models::ModelTab),
    ToggleModelFavorite(crate::settings::AgentModelFavorite),
    /// Start another agent as lead, the last one having stopped.
    StartNewLead,
    /// "Carry on" on the lead's waiting card: close it, sending nothing.
    DismissLeadWaiting,
    AskCleanUp,
    CancelCleanUp,
    ConfirmCleanUp,
    /// Delete a coordination that's over: ask first, then close its
    /// workers' tabs and workspaces (their worktrees removed) and take it
    /// off the list. The lead's tab and workspace stay, as ordinary ones.
    AskDelete,
    CancelDelete,
    ConfirmDelete,
    /// Board: open the new-task form, close it, add the task.
    StartAddTask,
    CancelAddTask,
    AddTask,
    ChooseNewTaskAgent(String),
    ToggleNewTaskAfter(String),
    /// Start a ready task now (when tasks don't start on their own).
    StartTask(String),
    /// Show a task's details on the Overview.
    ShowTask(String),
}

#[derive(Clone, Debug)]
pub enum TeamsEvent {
    /// Show the tab holding this agent session (else its workspace).
    OpenPane {
        pane: String,
        workspace: Option<PathBuf>,
    },
    /// Start `agent` in `cwd` on `prompt`, as the new lead of `team`.
    NewLead {
        team: String,
        agent: String,
        model: Option<String>,
        cwd: PathBuf,
        prompt: String,
    },
    /// Close these worktrees' workspaces and remove the worktrees (their
    /// branches stay); `project` is the repository they belong to.
    CleanUp {
        project: PathBuf,
        remove: Vec<PathBuf>,
    },
    /// A coordination is being deleted: close its workers' tabs (sessions
    /// `workers`) and these worktrees' workspaces, removing the worktrees
    /// even with uncommitted work (the person confirmed). The lead's tab
    /// and workspace are never among them.
    Delete {
        project: PathBuf,
        remove: Vec<PathBuf>,
        workers: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListFilter {
    #[default]
    All,
    /// Planning or at work.
    Active,
    /// Done or stopped.
    Finished,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailTab {
    #[default]
    Overview,
    /// The tasks as cards in columns by where they are.
    Board,
    Activity,
}

/// The lead's agent, as the page sees it.
#[derive(Clone, Debug, Default)]
pub(crate) struct LeadState {
    /// What it's doing; `None` when it isn't running.
    pub(crate) state: Option<WorkerState>,
    /// Put to sleep to save memory: it isn't running, and wakes on request.
    /// The pane it sleeps in (the lead's recorded pane, else the one found
    /// from its terminal), which the page's Wake button wakes.
    pub(crate) asleep: Option<String>,
    /// Where it ran: a new lead starts there.
    pub(crate) cwd: Option<PathBuf>,
}

/// What cleaning up a finished team would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Cleanup {
    /// Worktrees that go, by task; their branches stay.
    pub(crate) remove: Vec<(String, PathBuf)>,
    /// Worktrees with uncommitted changes: left alone.
    pub(crate) dirty: Vec<(String, PathBuf)>,
    /// The final tasks' worktrees, kept to look at the result.
    pub(crate) keep: Vec<(String, PathBuf)>,
}

/// What one look at the store found.
struct Loaded {
    teams: Vec<TeamSummary>,
    team: Option<Team>,
    workers: HashMap<String, WorkerState>,
    asleep_workers: std::collections::HashSet<String>,
    lead: LeadState,
    cleanup: Option<Cleanup>,
    replies: AgentReplies,
}

/// The last thing each agent said, as a person would read it.
#[derive(Clone, Debug, Default)]
pub(crate) struct AgentReplies {
    pub(crate) lead: Option<String>,
    /// When the lead said it (its turn's end).
    pub(crate) lead_at: Option<u64>,
    pub(crate) tasks: HashMap<String, String>,
}

pub struct TeamsView {
    pub(crate) view_id: EntityId,
    pub(crate) ui_font: FamilyId,
    pub(crate) mono_font: FamilyId,
    pub(crate) theme: Theme,
    visible: bool,
    pub(crate) teams: Vec<TeamSummary>,
    pub(crate) selected: Option<String>,
    pub(crate) team: Option<Team>,
    pub(crate) workers: HashMap<String, WorkerState>,
    /// Tasks whose worker agent is asleep (local teams only).
    pub(crate) asleep_workers: std::collections::HashSet<String>,
    pub(crate) lead: LeadState,
    pub(crate) cleanup: Option<Cleanup>,
    pub(crate) replies: AgentReplies,
    /// Tasks showing their details, and the lead's full message under `LEAD`.
    pub(crate) expanded: std::collections::HashSet<String>,
    pub(crate) confirm_cleanup: bool,
    /// What deleting the coordination would remove, while the person decides.
    pub(crate) confirm_delete: Option<DeletePlan>,
    /// The first look hasn't come back yet.
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) confirm_stop: bool,
    pub(crate) message_editor: ViewHandle<InputEditor>,
    pub(crate) message: String,
    pub(crate) model_task: Option<String>,
    pub(crate) model_tab: models::ModelTab,
    pub(crate) model_editor: ViewHandle<InputEditor>,
    pub(crate) model_query: String,
    pub(crate) model_scroll: ClippedScrollStateHandle,
    pub(crate) sending: bool,
    /// Dictation into the message box, as the new workspace's prompt has.
    voice_session: Option<crate::voice::VoiceSession>,
    pub(crate) voice_ui: crate::voice::VoiceUiState,
    voice_gen: u64,
    pub(crate) voice_spin: usize,
    /// "Start a new lead" was pressed: when, while the new lead comes up.
    pub(crate) starting_lead: Option<std::time::Instant>,
    /// The lead's waiting card closed with "Carry on": the team, and when the
    /// lead said what it showed (a newer word from the lead shows again).
    pub(crate) dismissed_waiting: Option<(String, u64)>,
    pub(crate) lead_spin: usize,
    voice_fold: crate::voice::fold::VoiceFold,
    voice_pending: Option<String>,
    /// What dictation has to say (heard words awaiting a confirm, a failure).
    pub(crate) voice_notice: Option<String>,
    generation: u64,
    refresh_scheduled: bool,
    pub(crate) filter: ListFilter,
    /// The list is folded to a narrow rail, leaving the room to the chosen one.
    pub(crate) list_collapsed: bool,
    pub(crate) tab: DetailTab,
    /// The page's width, as the workspace last laid it out: a narrow page
    /// folds the info column under the header.
    pub(crate) width: std::cell::Cell<f32>,
    pub(crate) mouse: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) list_scroll: ClippedScrollStateHandle,
    pub(crate) detail_scroll: ClippedScrollStateHandle,
    /// A long message from the lead scrolls inside its card.
    pub(crate) card_scroll: ClippedScrollStateHandle,
    pub(crate) rail_scroll: ClippedScrollStateHandle,
    /// The Board's new-task form.
    pub(crate) adding_task: bool,
    pub(crate) adding_busy: bool,
    pub(crate) task_title_editor: ViewHandle<InputEditor>,
    pub(crate) task_prompt_editor: ViewHandle<InputEditor>,
    pub(crate) new_task_agent: Option<String>,
    pub(crate) new_task_after: Vec<String>,
    /// The Board scrolls sideways when its columns don't fit.
    pub(crate) board_scroll: ClippedScrollStateHandle,
    /// Each Board column scrolls on its own.
    pub(crate) column_scrolls: [ClippedScrollStateHandle; 5],
}

impl Entity for TeamsView {
    type Event = TeamsEvent;
}

impl TeamsView {
    pub fn new(
        ctx: &mut ViewContext<Self>,
        ui_font: FamilyId,
        mono_font: FamilyId,
        theme: Theme,
    ) -> Self {
        let message_editor = ctx.add_typed_action_view(move |inner| {
            // As the new workspace's prompt: the same face, size and height.
            InputEditor::new(
                "Message the lead…",
                mono_font,
                crate::theme::text_size(crate::theme::INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                // Grows with the text, up to three lines, then scrolls.
                InputMode::MultiLine {
                    key: "team_message".into(),
                    max_height: (crate::console::input_layout::line_height_with_spacing(
                        crate::theme::text_size(crate::theme::INPUT_FONT_SIZE),
                    ) * 3.0)
                        .ceil() as u32,
                },
                inner.focus_handle(),
            )
        });
        ctx.subscribe_to_view(&message_editor, |view, editor, _event, ctx| {
            let text = ctx.update_view(&editor, |editor, _| editor.get_text().to_string());
            if text != view.message {
                view.message = text;
                ctx.notify();
            }
        });
        let model_editor = ctx.add_typed_action_view(move |inner| InputEditor::new(
            "Search models", ui_font, crate::theme::text_size(13.0),
            theme.main_text(), theme.disabled_text(), theme.accent, theme,
            InputMode::Plain { key: "team_model".into() }, inner.focus_handle()));
        ctx.subscribe_to_view(&model_editor, |view, editor, _, ctx| {
            view.model_query = ctx.update_view(&editor, |editor, _| editor.get_text().to_string());
            ctx.notify();
        });
        let task_title_editor = ctx.add_typed_action_view(move |inner| InputEditor::new(
            "Title, e.g. Add a dark mode toggle", ui_font, crate::theme::text_size(13.0),
            theme.main_text(), theme.disabled_text(), theme.accent, theme,
            InputMode::Plain { key: "team_task_title".into() }, inner.focus_handle()));
        let task_prompt_editor = ctx.add_typed_action_view(move |inner| InputEditor::new(
            "What should be done, and how you'll know it's done", ui_font, crate::theme::text_size(13.0),
            theme.main_text(), theme.disabled_text(), theme.accent, theme,
            InputMode::MultiLine { key: "team_task_prompt".into(), max_height: 160 }, inner.focus_handle()));
        Self {
            adding_task: false,
            adding_busy: false,
            task_title_editor,
            task_prompt_editor,
            new_task_agent: None,
            new_task_after: Vec::new(),
            board_scroll: ClippedScrollStateHandle::new(),
            column_scrolls: std::array::from_fn(|_| ClippedScrollStateHandle::new()),
            model_task: None,
            model_tab: models::ModelTab::Agent(crate::workspace::PromptAgent::Dragent),
            model_editor,
            model_query: String::new(),
            model_scroll: ClippedScrollStateHandle::new(),
            view_id: ctx.view_id(),
            ui_font,
            mono_font,
            theme,
            visible: false,
            teams: Vec::new(),
            selected: None,
            team: None,
            workers: HashMap::new(),
            asleep_workers: Default::default(),
            lead: LeadState::default(),
            replies: AgentReplies::default(),
            expanded: Default::default(),
            cleanup: None,
            confirm_cleanup: false,
            confirm_delete: None,
            loading: true,
            error: None,
            confirm_stop: false,
            message_editor,
            message: String::new(),
            sending: false,
            voice_session: None,
            voice_ui: crate::voice::VoiceUiState::Idle,
            voice_gen: 0,
            voice_spin: 0,
            starting_lead: None,
            dismissed_waiting: None,
            lead_spin: 0,
            voice_fold: Default::default(),
            voice_pending: None,
            voice_notice: None,
            generation: 0,
            refresh_scheduled: false,
            filter: ListFilter::All,
            list_collapsed: false,
            tab: DetailTab::Overview,
            width: std::cell::Cell::new(1100.0),
            mouse: RefCell::new(HashMap::new()),
            list_scroll: ClippedScrollStateHandle::new(),
            detail_scroll: ClippedScrollStateHandle::new(),
            card_scroll: ClippedScrollStateHandle::new(),
            rail_scroll: ClippedScrollStateHandle::new(),
        }
    }

    pub(crate) fn mouse(&self, key: &str) -> MouseStateHandle {
        self.mouse
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// The page came on screen (`team`: open on that one).
    pub fn show(&mut self, theme: Theme, team: Option<String>, ctx: &mut ViewContext<Self>) {
        self.set_theme(theme, ctx);
        self.visible = true;
        if team.is_some() {
            if self.selected != team { self.sending = false; }
            self.selected = team;
            self.confirm_stop = false;
        }
        self.refresh(ctx);
    }

    /// The page and its message box follow the app's theme.
    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        ctx.update_view(&self.model_editor, |editor, ctx| editor.set_theme(theme, ctx));
        let editor = self.message_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx)
        });
    }

    /// Open on this coordination's Board: a team just started there, and
    /// its lead's planning shows first.
    pub fn show_board(&mut self, theme: Theme, team: String, ctx: &mut ViewContext<Self>) {
        self.tab = DetailTab::Board;
        self.show(theme, Some(team), ctx);
    }

    pub fn hide(&mut self) {
        if let Some(session) = self.voice_session.take() {
            session.stop();
        }
        self.clear_voice();
        self.visible = false;
        self.confirm_stop = false;
    }

    /// Look at the store again, off the main thread.
    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let selected = self.selected.clone();
        ctx.spawn(async move { tokio::task::spawn_blocking(move || load(selected)).await.map_err(|e| e.to_string())? }, move |view, loaded, ctx| {
            if view.generation != generation {
                return;
            }
            view.loading = false;
            match loaded {
                Ok(loaded) => {
                    view.error = None;
                    view.selected = loaded.team.as_ref().map(|team| team.id.clone());
                    view.teams = loaded.teams;
                    view.team = loaded.team;
                    view.workers = loaded.workers;
                    view.asleep_workers = loaded.asleep_workers;
                    view.lead = loaded.lead;
                    view.cleanup = loaded.cleanup;
                    view.replies = loaded.replies;
                }
                Err(error) => view.error = Some(error),
            }
            ctx.notify();
            view.schedule_refresh(ctx);
        });
    }

    fn schedule_refresh(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.visible || self.refresh_scheduled {
            return;
        }
        self.refresh_scheduled = true;
        ctx.spawn(Timer::after(REFRESH), |view, _, ctx| {
            view.refresh_scheduled = false;
            if view.visible {
                view.refresh(ctx);
            }
        });
    }

    /// Run team operation `tool` as the person, then look again.
    fn act(&mut self, tool: &'static str, args: serde_json::Value, ctx: &mut ViewContext<Self>) {
        let selected = self.selected.clone();
        let sent_message = self.message.clone();
        ctx.spawn(
            async move { tokio::task::spawn_blocking(move || crate::team_source::call(tool, &args, &crate::cli::team::Caller::person())).await.map_err(|e|e.to_string())? },
            move |view, result, ctx| {
                if view.selected != selected { return; }
                view.sending = false;
                match result {
                    Ok(_) if tool == "msg_send" && view.message == sent_message => {
                        view.message.clear();
                        let editor = view.message_editor.clone();
                        ctx.update_view(&editor, |editor, ectx| editor.set_text("", ectx));
                    }
                    Ok(_) => {}
                    Err(error) => view.error = Some(error),
                }
                view.refresh(ctx);
            },
        );
    }

    // ── Dictation, as the new workspace's prompt does it ──

    fn clear_voice(&mut self) {
        self.voice_ui = crate::voice::VoiceUiState::Idle;
        self.voice_gen = self.voice_gen.wrapping_add(1);
        self.voice_session = None;
        self.voice_fold = Default::default();
        self.voice_pending = None;
    }

    fn apply_voice(&mut self, text: &str, is_final: bool, ctx: &mut ViewContext<Self>) {
        let current = self.message_editor.read(ctx, |editor, _| editor.get_text().to_string());
        let updated = if is_final {
            Some(self.voice_fold.apply(&current, text, false))
        } else {
            self.voice_fold.apply_partial(&current, text, false)
        };
        let Some(updated) = updated else {
            self.voice_notice = Some(format!("Dictating: {text}"));
            return;
        };
        self.message = updated.clone();
        let editor = self.message_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| editor.set_text(&updated, editor_ctx));
    }

    fn on_voice(&mut self, event: crate::voice::VoiceEvent, generation: u64, ctx: &mut ViewContext<Self>) {
        if generation != self.voice_gen {
            return;
        }
        match event {
            crate::voice::VoiceEvent::Live => self.voice_ui = crate::voice::VoiceUiState::Recording,
            crate::voice::VoiceEvent::Partial(text) => self.apply_voice(&text, false, ctx),
            crate::voice::VoiceEvent::Final(text) => match self.voice_fold.final_disposition(text) {
                crate::voice::FinalDisposition::Insert(text) => {
                    self.apply_voice(&text, true, ctx);
                    self.clear_voice();
                }
                crate::voice::FinalDisposition::AwaitingConfirm(text) => {
                    self.voice_session = None;
                    self.voice_gen = self.voice_gen.wrapping_add(1);
                    self.voice_ui = crate::voice::VoiceUiState::Idle;
                    self.voice_notice = Some(crate::voice::review_notice(&text));
                    self.voice_pending = Some(text);
                }
            },
            crate::voice::VoiceEvent::Error(message) => {
                self.voice_notice = Some(message);
                self.clear_voice();
            }
        }
        ctx.notify();
    }

    /// Turn the new lead's loader until the lead runs, or give the button
    /// back after a while (a start that failed says so in its own notice).
    fn tick_lead_spinner(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(std::time::Duration::from_millis(80)), move |view, _, ctx| {
            let Some(started) = view.starting_lead else { return };
            let running = view.lead.state.is_some() || view.lead.asleep.is_some();
            if running || started.elapsed() > std::time::Duration::from_secs(30) {
                view.starting_lead = None;
            } else {
                view.lead_spin = (view.lead_spin + 1) % crate::icons::SPINNER_FRAMES;
                view.tick_lead_spinner(ctx);
            }
            ctx.notify();
        });
    }

    fn tick_voice_spinner(&mut self, generation: u64, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(std::time::Duration::from_millis(80)), move |view, _, ctx| {
            if view.voice_gen != generation || view.voice_ui != crate::voice::VoiceUiState::Connecting {
                return;
            }
            view.voice_spin = (view.voice_spin + 1) % crate::icons::SPINNER_FRAMES;
            view.tick_voice_spinner(generation, ctx);
            ctx.notify();
        });
    }

    fn toggle_voice(&mut self, ctx: &mut ViewContext<Self>) {
        use crate::voice::VoiceUiState;
        // A held dictation is confirmed by the next press: put it in now.
        if self.voice_session.is_none() && self.voice_ui == VoiceUiState::Idle {
            if let Some(pending) = self.voice_pending.take() {
                self.apply_voice(&pending, true, ctx);
                self.voice_fold = Default::default();
                self.voice_notice = None;
                ctx.notify();
                return;
            }
        }
        if let Some(session) = self.voice_session.take() {
            session.stop();
            if self.voice_ui == VoiceUiState::Connecting {
                self.clear_voice();
            } else {
                let generation = self.voice_gen;
                ctx.spawn(Timer::after(std::time::Duration::from_secs(6)), move |view, _, ctx| {
                    if view.voice_gen == generation && view.voice_ui != VoiceUiState::Idle {
                        view.clear_voice();
                        ctx.notify();
                    }
                });
            }
            ctx.notify();
            return;
        }
        if self.voice_ui != VoiceUiState::Idle {
            return;
        }
        let (tx, rx) = async_channel::unbounded();
        match crate::voice::start(tx) {
            Ok(session) => {
                self.voice_gen = self.voice_gen.wrapping_add(1);
                let generation = self.voice_gen;
                self.voice_session = Some(session);
                self.voice_ui = VoiceUiState::Connecting;
                self.voice_spin = 0;
                self.voice_notice = None;
                let existing = self.message_editor.read(ctx, |editor, _| editor.get_text().to_string());
                self.voice_fold = crate::voice::fold::VoiceFold::start_with_confirmation(
                    existing,
                    crate::settings::AppSettings::global().voice.confirm_before_insert,
                );
                self.voice_pending = None;
                self.tick_voice_spinner(generation, ctx);
                ctx.spawn_stream_local(rx, move |view, event, ctx| view.on_voice(event, generation, ctx), |_, _| {});
                ctx.spawn(Timer::after(std::time::Duration::from_secs(20)), move |view, _, ctx| {
                    if view.voice_gen == generation && view.voice_ui == VoiceUiState::Connecting {
                        if let Some(session) = view.voice_session.take() {
                            session.stop();
                        }
                        view.voice_notice = Some("The dictation engine did not connect.".into());
                        view.clear_voice();
                        ctx.notify();
                    }
                });
            }
            Err(message) => self.voice_notice = Some(message),
        }
        ctx.notify();
    }

    /// A one-click answer (a choice, a permission prompt) as the person.
    /// Unlike typing a message, it leaves the message box's draft alone.
    fn answer(&mut self, tool: &'static str, args: serde_json::Value, ctx: &mut ViewContext<Self>) {
        let selected = self.selected.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::team_source::call(tool, &args, &crate::cli::team::Caller::person())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |view, result, ctx| {
                if view.selected != selected {
                    return;
                }
                if let Err(error) = result {
                    view.error = Some(error);
                }
                view.refresh(ctx);
            },
        );
    }

    /// Press Enter (allow) or Esc (deny) at the lead's permission prompt,
    /// in its terminal, as the phone's answer does.
    fn answer_lead(&mut self, team: String, allow: bool, ctx: &mut ViewContext<Self>) {
        let Some(pane) = self
            .team
            .as_ref()
            .and_then(|team| team.lead.as_ref())
            .and_then(|lead| lead.pane.clone())
        else {
            return;
        };
        let selected = self.selected.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let session = pane
                        .parse()
                        .ok()
                        .and_then(dragent_host::agent_sessions::load)
                        .filter(|record| record.active)
                        .ok_or("The lead isn't running any more.")?;
                    let client = crate::team_source::client(&team)?;
                    client.send_terminal_input(
                        &session.workspace_id,
                        &session.terminal_id,
                        if allow { b"\r" } else { b"\x1b" },
                    )
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |view, result, ctx| {
                if view.selected != selected {
                    return;
                }
                if let Err(error) = result {
                    view.error = Some(error);
                }
                view.refresh(ctx);
            },
        );
    }

    /// Ask for a sleeping agent to be woken. The app's sleep sweep answers
    /// the request within a few seconds, the same way the team's courier does.
    fn wake(&mut self, pane: String, ctx: &mut ViewContext<Self>) {
        let Ok(uuid) = uuid::Uuid::parse_str(&pane) else {
            return;
        };
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    if !dragent_host::agent_sessions::is_asleep(uuid) {
                        return Err("That agent isn't asleep any more.".to_string());
                    }
                    dragent_host::agent_sessions::request_wake(uuid, "Woken from the Teams page")
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            |view, result, ctx| {
                if let Err(error) = result {
                    view.error = Some(error);
                }
                view.refresh(ctx);
            },
        );
    }

    /// Add the person's task to the plan, then tell the lead about it.
    fn add_task(&mut self, team: String, ctx: &mut ViewContext<Self>) {
        let title = self.task_title_editor.read(ctx, |e, _| e.get_text().trim().to_string());
        let prompt = self.task_prompt_editor.read(ctx, |e, _| e.get_text().trim().to_string());
        if title.is_empty() || prompt.is_empty() {
            self.error = Some("Give the task a title and say what should be done.".into());
            ctx.notify();
            return;
        }
        self.adding_busy = true;
        self.error = None;
        let mut args = json!({"team": team, "title": title, "prompt": prompt, "after": self.new_task_after});
        if let Some(agent) = &self.new_task_agent {
            args["agent"] = json!(agent);
        }
        let selected = self.selected.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    let person = crate::cli::team::Caller::person();
                    let added = crate::team_source::call("task_add", &args, &person)?;
                    let id = added.get("id").or_else(|| added.pointer("/task/id")).and_then(|v| v.as_str()).unwrap_or("the new task").to_string();
                    // The lead plans and integrates: it needs to know.
                    let note = format!(
                        "I added {id} “{title}” to the plan from the board. It starts once what it waits for is done; check and integrate it like the others. What it asks: {prompt}"
                    );
                    let _ = crate::team_source::call("msg_send", &json!({"team": team, "to": "lead", "text": note, "kind": "note"}), &person);
                    Ok::<_, String>(())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |view, result, ctx| {
                view.adding_busy = false;
                if view.selected != selected {
                    return;
                }
                match result {
                    Ok(()) => view.adding_task = false,
                    Err(error) => view.error = Some(error),
                }
                view.refresh(ctx);
            },
        );
    }

    /// The message to the person a reply answers: the latest one.
    pub(crate) fn answering(&self) -> Option<&dragent_host::team::Message> {
        self.team
            .as_ref()?
            .messages
            .iter()
            .rev()
            .find(|message| message.to == Party::User)
    }
}

/// Every team, the chosen one (else the most recently active) in full, and
/// what its workers' agents are doing. The person's messages count as read
/// once they're on screen.
fn load(selected: Option<String>) -> Result<Loaded, String> {
    let teams = crate::team_source::list()?;
    if let Some(id) = selected.as_deref().filter(|id| crate::team_source::split(id).0.is_some()) {
        // A disconnected host must never select or act on a local team.
        crate::team_source::client(id)?;
        if !teams.iter().any(|team|team.id == id) {
            return Err("Couldn't refresh the selected SSH team. Reconnect its machine to continue.".into());
        }
    }
    let chosen = selected
        .filter(|id| teams.iter().any(|team| &team.id == id))
        .or_else(|| teams.first().map(|team| team.id.clone()));
    let client = crate::team_source::client(chosen.as_deref().unwrap_or_default())?;
    let machine = chosen.as_deref().and_then(|id| crate::team_source::split(id).0).map(str::to_owned);
    let team = match chosen {
        Some(id) => match client.team(TeamRequest::Get { team: crate::team_source::split(&id).1.to_string() })? {
            TeamResponse::Team(team) => {
                if team.unread(&Party::User) > 0 {
                    let _ = client.team(TeamRequest::Inbox {
                        team: crate::team_source::split(&id).1.to_string(),
                        party: Party::User,
                        all: false,
                        peek: false,
                    });
                }
                Some(match &machine { Some(machine) => crate::team_source::display_team(*team, machine), None => *team })
            }
            _ => None,
        },
        None => None,
    };
    let (workers, lead) = if let (Some(machine), Some(team)) = (&machine, &team) {
        remote_states(&client, team, machine)?
    } else {
        let workers = team.as_ref().map(|team| team.tasks.iter()
            .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Blocked))
            .filter_map(|task| Some((task.id.clone(), worker_state(task.worker.as_ref()?.pane.as_deref()?))))
            .collect()).unwrap_or_default();
        let lead = team.as_ref().map(|team| lead_state(&client, team)).unwrap_or_default();
        (workers, lead)
    };
    let cleanup = team.as_ref().and_then(cleanup);
    // Session files only describe agents on this computer.
    let replies = team.as_ref().filter(|_| machine.is_none()).map(agent_replies).unwrap_or_default();
    // Asleep workers are read from their session files, so only on this computer too.
    let asleep_workers = team
        .as_ref()
        .filter(|_| machine.is_none())
        .map(|team| {
            team.tasks
                .iter()
                // Only workers kept on a task: a finished one's may sleep.
                .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Blocked))
                .filter(|task| {
                    task.worker
                        .as_ref()
                        .and_then(|worker| worker.pane.as_deref())
                        .and_then(|pane| uuid::Uuid::parse_str(pane).ok())
                        .is_some_and(dragent_host::agent_sessions::is_asleep)
                })
                .map(|task| task.id.clone())
                .collect()
        })
        .unwrap_or_default();
    Ok(Loaded {
        teams,
        team,
        workers,
        asleep_workers,
        lead,
        cleanup,
        replies,
    })
}

fn agent_replies(team: &Team) -> AgentReplies {
    let full = |member: Option<&dragent_host::team::Member>| {
        let pane = uuid::Uuid::parse_str(member?.pane.as_deref()?).ok()?;
        dragent_host::agent_sessions::load_reply(pane)
    };
    let reply = |member: Option<&dragent_host::team::Member>| full(member).map(|reply| reply.text);
    let lead = full(team.lead.as_ref());
    AgentReplies {
        lead_at: lead.as_ref().map(|reply| reply.at_ms),
        lead: lead.map(|reply| reply.text),
        tasks: team
            .tasks
            .iter()
            .filter(|task| task.status.is_finished())
            .filter_map(|task| Some((task.id.clone(), reply(task.worker.as_ref())?)))
            .collect(),
    }
}

/// Read session state from its owner; local session files never describe SSH agents.
fn remote_states(client: &HostClient, team: &Team, machine: &str) -> Result<(HashMap<String, WorkerState>, LeadState), String> {
    let panes: Vec<_> = team.lead.iter().filter_map(|lead| lead.pane.clone())
        .chain(team.tasks.iter().filter_map(|task| task.worker.as_ref()?.pane.clone())).collect();
    let snapshots = client.coordination("host_sessions", json!({"panes":panes}), serde_json::to_value(crate::cli::team::Caller::person()).unwrap())?;
    let mut states = HashMap::new();
    let mut cwd = None;
    for row in snapshots.as_array().into_iter().flatten() {
        let Some(pane) = row["pane"].as_str() else { continue };
        let record: dragent_host::agent_sessions::AgentSession = serde_json::from_value(row["record"].clone()).map_err(|e|e.to_string())?;
        if team.lead.as_ref().and_then(|lead|lead.pane.as_deref()) == Some(pane) {
            cwd = Some(crate::remote_fs::alias(machine, &record.cwd.to_string_lossy()));
        }
        if !record.active || !client.terminal(&record.workspace_id,&record.terminal_id).is_ok_and(|t|t.state == dragent_host::protocol::TerminalState::Running) { continue; }
        let state=match row["activity"]["event"].as_str() {
            Some("Start")=>WorkerState::Working, Some("PermissionRequest")=>WorkerState::NeedsApproval,
            Some(_)=>WorkerState::Idle,None=>WorkerState::Unknown,
        };
        states.insert(pane.to_string(),state);
    }
    let lead=LeadState {state:team.lead.as_ref().and_then(|lead|states.get(lead.pane.as_deref()?).copied()),cwd,asleep:None};
    let workers=team.tasks.iter().filter_map(|task|Some((task.id.clone(),states.get(task.worker.as_ref()?.pane.as_deref()?).copied().unwrap_or(WorkerState::Unknown)))).collect();
    Ok((workers,lead))
}

/// Whether the lead's agent still runs (its session active, its terminal
/// alive), and what it's doing.
fn lead_state(client: &HostClient, team: &Team) -> LeadState {
    let Some(lead) = &team.lead else {
        return LeadState::default();
    };
    // Dragent's own agent: its window says what it's doing.
    if lead.agent.as_deref() == Some(dragent_host::team::DRAGENT_AGENT) {
        let seen = lead.pane.as_deref().and_then(dragent_lead_state);
        return LeadState {
            state: seen.as_ref().map(|(state, _)| *state),
            cwd: Some(
                seen.map(|(_, cwd)| cwd)
                    .unwrap_or_else(|| PathBuf::from(&team.project_root)),
            ),
            asleep: None,
        };
    }
    let pane = lead.pane.clone().or_else(|| {
        lead.terminal
            .as_deref()
            .and_then(dragent_host::agent_sessions::pane_for_terminal)
            .map(|pane| pane.to_string())
    });
    let record = pane
        .as_deref()
        .and_then(|pane| uuid::Uuid::parse_str(pane).ok())
        .and_then(dragent_host::agent_sessions::load);
    let running = record.as_ref().is_some_and(|record| {
        record.active
            && client
                .terminal(&record.workspace_id, &record.terminal_id)
                .is_ok_and(|terminal| {
                    terminal.state == dragent_host::protocol::TerminalState::Running
                })
    });
    // A sleeping lead's terminal is closed on purpose: not gone, asleep.
    let asleep = pane.clone().filter(|pane| {
        !running
            && uuid::Uuid::parse_str(pane).is_ok_and(dragent_host::agent_sessions::is_asleep)
    });
    LeadState {
        state: running.then(|| worker_state(pane.as_deref().unwrap_or_default())),
        cwd: record.map(|record| record.cwd),
        asleep,
    }
}

/// How a new lead takes over `team` after its last lead stopped: the same
/// agent (else the team's first), on the lead's model, where the last one
/// ran (`lead_cwd`, else the project), told to pick up from the plan.
pub(crate) struct NewLeadPlan {
    pub(crate) agent: String,
    pub(crate) model: Option<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) prompt: String,
}

pub(crate) fn new_lead_plan(team: &Team, lead_cwd: Option<PathBuf>) -> NewLeadPlan {
    let agent = team
        .lead
        .as_ref()
        .and_then(|lead| lead.agent.clone())
        .or_else(|| team.roster.first().map(|entry| entry.agent.clone()))
        .unwrap_or_else(|| "claude".into());
    let prompt = if agent == dragent_host::team::DRAGENT_AGENT {
        crate::cli::team::dragent_takeover_prompt(team)
    } else {
        crate::cli::team::takeover_prompt(&crate::team_source::native_team(team))
    };
    NewLeadPlan {
        agent,
        model: team.lead_model.clone(),
        cwd: lead_cwd.unwrap_or_else(|| PathBuf::from(&team.project_root)),
        prompt,
    }
}

/// The team is at work but its lead isn't running: nobody follows the
/// workers (the page's "Start a new lead"). Also where the last lead ran.
pub(crate) fn lead_gone(client: &HostClient, team: &Team) -> (bool, Option<PathBuf>) {
    let lead = lead_state(client, team);
    let gone = matches!(
        team.status,
        dragent_host::team::TeamStatus::Planning | dragent_host::team::TeamStatus::Running
    ) && team.lead.is_some()
        && lead.state.is_none()
        && lead.asleep.is_none();
    (gone, lead.cwd)
}

/// What deleting a coordination removes: every worker checkout (never the
/// project itself, nor the lead's), with which hold uncommitted changes, and
/// every worker's session, so their tabs close. The lead stays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeletePlan {
    /// Worktrees that go, by the last task in each.
    pub(crate) remove: Vec<(String, PathBuf)>,
    /// Of those, the ones with changes not committed (lost).
    pub(crate) dirty: Vec<String>,
    /// Worker sessions (pane ids) whose tabs close.
    pub(crate) workers: Vec<String>,
}

pub(crate) fn delete_plan(team: &Team) -> DeletePlan {
    let project = std::path::Path::new(&team.project_root);
    let lead = team.lead.as_ref().and_then(|lead| lead.pane.clone());
    let mut plan = DeletePlan::default();
    for task in &team.tasks {
        if let Some(pane) = task.worker.as_ref().and_then(|worker| worker.pane.clone()) {
            if Some(&pane) != lead.as_ref() && !plan.workers.contains(&pane) {
                plan.workers.push(pane);
            }
        }
        let Some(path) = task.workspace.as_deref().map(PathBuf::from) else {
            continue;
        };
        if path == project {
            continue;
        }
        match plan.remove.iter_mut().find(|(_, known)| *known == path) {
            Some(entry) => entry.0 = task.id.clone(),
            None => plan.remove.push((task.id.clone(), path)),
        }
    }
    plan.remove.retain(|(_, path)| {
        if crate::remote_fs::is_remote(path) { crate::remote_fs::is_dir(path) } else { path.is_dir() }
    });
    plan.dirty = plan
        .remove
        .iter()
        .filter(|(_, path)| !crate::remote_fs::is_remote(path) && uncommitted(path))
        .map(|(id, _)| id.clone())
        .collect();
    plan
}

/// For a team that's over: which of its worktrees can go. Those holding a
/// final task (nothing builds on it) stay so the result can be looked at,
/// and so does any with uncommitted changes. `None` when there's nothing
/// to do.
pub(crate) fn cleanup(team: &Team) -> Option<Cleanup> {
    use dragent_host::team::TaskKind;
    if !matches!(
        team.status,
        dragent_host::team::TeamStatus::Done | dragent_host::team::TeamStatus::Stopped
    ) {
        return None;
    }
    let project = std::path::Path::new(&team.project_root);
    let built_on = |id: &str| {
        team.tasks.iter().any(|other| {
            other.kind == TaskKind::Work
                && other.depends_on.iter().any(|dependency| dependency == id)
        })
    };
    // Work that followed on shares a checkout: a checkout goes as one,
    // named after the last task in it, and stays if any of them is final.
    let mut checkouts: Vec<(PathBuf, Vec<&str>)> = Vec::new();
    for task in team.tasks.iter().filter(|task| task.kind == TaskKind::Work) {
        let Some(path) = task.workspace.as_deref().map(PathBuf::from) else {
            continue;
        };
        match checkouts.iter_mut().find(|(known, _)| *known == path) {
            Some((_, ids)) => ids.push(&task.id),
            None => checkouts.push((path, vec![&task.id])),
        }
    }
    let mut plan = Cleanup::default();
    for (path, ids) in checkouts {
        if path == project || !(if crate::remote_fs::is_remote(&path) { crate::remote_fs::is_dir(&path) } else { path.is_dir() }) {
            continue;
        }
        let last = ids.last().copied().unwrap_or_default().to_string();
        if let Some(result) = ids.iter().rev().find(|id| !built_on(id)) {
            plan.keep.push((result.to_string(), path));
        } else if uncommitted(&path) {
            plan.dirty.push((last, path));
        } else {
            plan.remove.push((last, path));
        }
    }
    (!plan.remove.is_empty()).then_some(plan)
}

/// Changes in the worktree at `path` that aren't committed (or no answer).
fn uncommitted(path: &std::path::Path) -> bool {
    crate::git_cmd::GitCommand::new()
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .map(|output| !output.status.success() || !output.stdout.is_empty())
        .unwrap_or(true)
}

fn worker_state(pane: &str) -> WorkerState {
    let activity = uuid::Uuid::parse_str(pane)
        .ok()
        .and_then(dragent_host::agent_sessions::load_activity);
    match activity.as_ref().map(|activity| activity.event.as_str()) {
        Some("Start") => WorkerState::Working,
        Some("PermissionRequest") => WorkerState::NeedsApproval,
        Some(_) => WorkerState::Idle,
        None => WorkerState::Unknown,
    }
}

/// Teams whose messages for the person wait unread: (id, title, how
/// many), for the sidebar badge and alerts (off the main thread). Also
/// notes which sessions are teams' workers (see [`is_worker`]).
pub fn overview() -> Overview {
    match crate::team_source::list() {
        Ok(teams) => {
            if let Ok(mut workers) = worker_panes().write() {
                *workers = teams
                    .iter()
                    .flat_map(|team| team.worker_panes.iter().cloned())
                    .collect();
            }
            // Work on this computer whose next tasks are due: the window
            // starts them (a remote machine starts its own).
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or(0);
            let due: Vec<(String, Vec<String>)> = HostClient::connect_existing()
                .map(|client| {
                    teams
                        .iter()
                        .filter(|team| team.status == dragent_host::team::TeamStatus::Running)
                        .filter(|team| crate::team_source::split(&team.id).0.is_none())
                        .filter_map(|summary| {
                            let TeamResponse::Team(team) = client
                                .team(TeamRequest::Get {
                                    team: summary.id.clone(),
                                })
                                .ok()?
                            else {
                                return None;
                            };
                            let due = dragent_host::team_commands::due_to_start(
                                &team,
                                now,
                                &dragent_host::team_commands::session_activity,
                            );
                            (!due.is_empty()).then(|| (team.id.clone(), due))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Overview {
                groups: sidebar::groups(&teams),
                due,
                dragent_leads: teams
                    .iter()
                    .filter_map(|team| Some((team.id.clone(), team.dragent_lead.clone()?)))
                    .collect(),
                waiting: teams
                    .into_iter()
                    .filter(|team| team.needs_user > 0)
                    .map(|team| (team.id, team.title, team.needs_user))
                    .collect(),
            }
        }
        _ => Overview::default(),
    }
}

/// One look at every team, for a window.
#[derive(Debug, Default)]
pub struct Overview {
    /// Teams whose messages for the person wait unread: (id, title, how
    /// many).
    pub waiting: Vec<(String, String, usize)>,
    /// Teams Dragent's own agent leads: (id, its session). The window
    /// showing that agent brings it its news.
    pub dragent_leads: Vec<(String, String)>,
    /// Teams on this computer whose next tasks start now: (id, tasks).
    pub due: Vec<(String, Vec<String>)>,
    /// Each coordination's lead and task checkouts, for the sidebar groups.
    pub groups: Vec<sidebar::CoordinationGroup>,
}

/// How long what a window saw a lead doing counts: it looks every few
/// seconds while the agent's tab is open.
const LEAD_SEEN_FOR: Duration = Duration::from_secs(20);

#[allow(clippy::type_complexity)]
fn dragent_leads(
) -> &'static std::sync::RwLock<HashMap<String, (WorkerState, PathBuf, web_time::Instant)>> {
    static LEADS: std::sync::OnceLock<
        std::sync::RwLock<HashMap<String, (WorkerState, PathBuf, web_time::Instant)>>,
    > = std::sync::OnceLock::new();
    LEADS.get_or_init(Default::default)
}

/// A window saw Dragent's own agent in `pane`, which leads a team, doing
/// `state` in `cwd`.
pub(crate) fn note_dragent_lead(pane: &str, state: WorkerState, cwd: PathBuf) {
    if let Ok(mut leads) = dragent_leads().write() {
        leads.insert(pane.to_string(), (state, cwd, web_time::Instant::now()));
    }
}

/// What Dragent's own agent in `pane` was last seen doing, while a window
/// still shows it.
fn dragent_lead_state(pane: &str) -> Option<(WorkerState, PathBuf)> {
    let leads = dragent_leads().read().ok()?;
    let (state, cwd, at) = leads.get(pane)?;
    (at.elapsed() < LEAD_SEEN_FOR).then(|| (*state, cwd.clone()))
}

/// The news Dragent's own agent leading `team` hasn't had yet, as one
/// message (taking it counts as delivered).
pub(crate) fn take_lead_news(team: &str, busy: bool) -> Option<String> {
    let client = HostClient::connect_existing().ok()?;
    match client.team(TeamRequest::TakeLeadNews {
        team: team.to_string(),
        busy,
    }) {
        Ok(TeamResponse::News(news)) => news,
        _ => None,
    }
}

fn worker_panes() -> &'static std::sync::RwLock<std::collections::HashSet<String>> {
    static PANES: std::sync::OnceLock<std::sync::RwLock<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    PANES.get_or_init(Default::default)
}

/// Whether the agent session `pane` works on a task of a team at work:
/// its turns ending isn't news for the person (the lead reports on it),
/// though its asking for an approval is.
pub fn is_worker(pane: &str) -> bool {
    worker_panes()
        .read()
        .is_ok_and(|workers| workers.contains(pane))
}

/// Data a team alert carries back when it's clicked.
pub const ALERT_PREFIX: &str = "dragent-team:";

/// The team a clicked alert is about.
pub fn team_from_alert(data: &str) -> Option<&str> {
    data.strip_prefix(ALERT_PREFIX)
        .filter(|team| !team.is_empty())
}

impl View for TeamsView {
    fn ui_name() -> &'static str {
        "TeamsView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        page::render(self, app)
    }
}

impl TypedActionView for TeamsView {
    type Action = TeamsAction;

    fn handle_action(&mut self, action: &TeamsAction, ctx: &mut ViewContext<Self>) {
        if !matches!(action, TeamsAction::AskStop | TeamsAction::ConfirmStop) {
            self.confirm_stop = false;
        }
        if !matches!(
            action,
            TeamsAction::AskCleanUp | TeamsAction::ConfirmCleanUp
        ) {
            self.confirm_cleanup = false;
        }
        if !matches!(action, TeamsAction::AskDelete | TeamsAction::ConfirmDelete) {
            self.confirm_delete = None;
        }
        let team = self.team.as_ref().map(|team| team.id.clone());
        match action {
            TeamsAction::BrowseModelTab(tab) => {
                self.model_tab = *tab;
                self.model_query.clear();
                ctx.update_view(&self.model_editor, |editor, ctx| { editor.set_text("", ctx); ctx.focus_self(); });
            }
            TeamsAction::ToggleModelFavorite(favorite) => {
                models::toggle_favorite(favorite.clone());
                ctx.update_view(&self.model_editor, |_, ctx| ctx.focus_self());
            }
            TeamsAction::ChooseTaskModel(task) => {
                if let Some(agent) = self.team.as_ref().and_then(|team| team.task(task))
                    .and_then(|task| task.agent.as_deref()).and_then(crate::automations::executor::prompt_agent) {
                    self.model_tab = models::ModelTab::Agent(agent);
                }
                self.model_task = Some(task.clone());
                self.model_query.clear();
                self.model_scroll = ClippedScrollStateHandle::new();
                ctx.update_view(&self.model_editor, |editor, ctx| { editor.set_text("", ctx); ctx.focus_self(); });
            }
            TeamsAction::CloseModelPicker => self.model_task = None,
            TeamsAction::ClearModelSearch => {
                self.model_query.clear();
                ctx.update_view(&self.model_editor, |editor, ctx| { editor.set_text("", ctx); ctx.focus_self(); });
            }
            TeamsAction::SetTaskModel { task, model } => {
                if let Some(team) = team {
                    self.act("task_model", json!({"team": team, "task": task, "model": model.as_deref().unwrap_or("none")}), ctx);
                }
                self.model_task = None;
            }
            TeamsAction::Select(id) => {
                self.expanded.clear();
                self.model_task = None;
                if self.selected.as_ref() != Some(id) {
                    self.selected = Some(id.clone());
                    self.sending = false;
                    // Another team's loader isn't this one's.
                    self.starting_lead = None;
                    self.team = None;
                    self.workers.clear();
                    self.detail_scroll = ClippedScrollStateHandle::new();
                    self.rail_scroll = ClippedScrollStateHandle::new();
                    self.refresh(ctx);
                }
            }
            TeamsAction::Filter(filter) => self.filter = *filter,
            TeamsAction::Tab(tab) => self.tab = *tab,
            TeamsAction::ToggleDetails(key) => {
                if !self.expanded.remove(key) {
                    self.expanded.insert(key.clone());
                }
            }
            TeamsAction::OpenLead => {
                if let Some(pane) = self
                    .team
                    .as_ref()
                    .and_then(|team| team.lead.as_ref())
                    .and_then(|lead| lead.pane.clone())
                {
                    ctx.emit(TeamsEvent::OpenPane {
                        pane,
                        workspace: self.lead.cwd.clone().or_else(|| self.team.as_ref().map(|team|PathBuf::from(&team.project_root))),
                    });
                }
            }
            TeamsAction::Wake(pane) => self.wake(pane.clone(), ctx),
            TeamsAction::OpenWorker(id) => {
                let task = self.team.as_ref().and_then(|team| team.task(id));
                if let Some(pane) = task
                    .and_then(|task| task.worker.as_ref())
                    .and_then(|worker| worker.pane.clone())
                {
                    ctx.emit(TeamsEvent::OpenPane {
                        pane,
                        workspace: task
                            .and_then(|task| task.workspace.clone())
                            .map(PathBuf::from),
                    });
                }
            }
            TeamsAction::AskStop => self.confirm_stop = true,
            TeamsAction::CancelStop => {}
            TeamsAction::ConfirmStop => {
                self.confirm_stop = false;
                if let Some(team) = team {
                    self.act("team_stop", json!({"team": team}), ctx);
                }
            }
            TeamsAction::StartAddTask => {
                self.adding_task = true;
                self.new_task_agent = None;
                self.new_task_after.clear();
                for editor in [self.task_title_editor.clone(), self.task_prompt_editor.clone()] {
                    ctx.update_view(&editor, |editor, ectx| editor.set_text("", ectx));
                }
                ctx.focus(&self.task_title_editor);
            }
            TeamsAction::CancelAddTask => self.adding_task = false,
            TeamsAction::ChooseNewTaskAgent(agent) => self.new_task_agent = Some(agent.clone()),
            TeamsAction::ToggleNewTaskAfter(id) => {
                if let Some(at) = self.new_task_after.iter().position(|t| t == id) {
                    self.new_task_after.remove(at);
                } else {
                    self.new_task_after.push(id.clone());
                }
            }
            TeamsAction::AddTask => {
                if let Some(team) = team {
                    self.add_task(team, ctx);
                }
            }
            TeamsAction::StartTask(id) => {
                if let Some(team) = team {
                    self.act("task_start", json!({"team": team, "tasks": [id]}), ctx);
                }
            }
            TeamsAction::ShowTask(id) => {
                self.tab = DetailTab::Overview;
                self.expanded.insert(id.clone());
            }
            TeamsAction::CancelTask(id) => {
                if let Some(team) = team {
                    self.act("task_cancel", json!({"team": team, "task": id}), ctx);
                }
            }
            TeamsAction::ToggleVoice => self.toggle_voice(ctx),
            TeamsAction::Say(text) => {
                if let Some(team) = team {
                    self.answer("msg_send", json!({"team": team, "to": "lead", "text": text, "kind": "answer"}), ctx);
                }
            }
            TeamsAction::Choose { message, choice } => {
                if let Some(team) = team {
                    self.answer(
                        "msg_send",
                        json!({"team": team, "to": "lead", "text": choice, "kind": "answer", "reply_to": message}),
                        ctx,
                    );
                }
            }
            TeamsAction::Approve { task, allow } => {
                let key = if *allow { "enter" } else { "esc" };
                match (team, task) {
                    (Some(team), Some(task)) => {
                        self.answer("task_keys", json!({"team": team, "task": task, "keys": [key]}), ctx);
                    }
                    (Some(team), None) => self.answer_lead(team, *allow, ctx),
                    _ => {}
                }
            }
            TeamsAction::ToggleList => {
                self.list_collapsed = !self.list_collapsed;
                ctx.notify();
            }
            TeamsAction::Send => {
                let text = self.message.trim().to_string();
                if text.is_empty() || self.sending {
                    return;
                }
                if let Some(team) = team {
                    self.sending = true;
                    let reply_to = self
                        .answering()
                        .filter(|message| message.from == Party::Lead)
                        .map(|message| message.id);
                    self.act(
                        "msg_send",
                        json!({
                            "team": team,
                            "to": "lead",
                            "text": text,
                            "kind": if reply_to.is_some() { "answer" } else { "note" },
                            "reply_to": reply_to,
                        }),
                        ctx,
                    );
                }
            }
            TeamsAction::DismissLeadWaiting => {
                if let (Some(team), Some(at)) = (&self.team, self.replies.lead_at) {
                    self.dismissed_waiting = Some((team.id.clone(), at));
                }
            }
            TeamsAction::StartNewLead => {
                if self.starting_lead.is_some() {
                    return;
                }
                if let Some(team) = &self.team {
                    let plan = new_lead_plan(team, self.lead.cwd.clone());
                    let event = TeamsEvent::NewLead {
                        team: team.id.clone(),
                        agent: plan.agent,
                        model: plan.model,
                        cwd: plan.cwd,
                        prompt: plan.prompt,
                    };
                    // Show the loader first: starting the lead's terminal can
                    // hold the window for a moment, so it starts a frame later.
                    self.starting_lead = Some(std::time::Instant::now());
                    self.lead_spin = 0;
                    self.tick_lead_spinner(ctx);
                    ctx.spawn(Timer::after(std::time::Duration::from_millis(40)), move |_, _, ctx| {
                        ctx.emit(event);
                    });
                }
            }
            TeamsAction::AskCleanUp => self.confirm_cleanup = true,
            TeamsAction::CancelCleanUp => {}
            TeamsAction::ConfirmCleanUp => {
                self.confirm_cleanup = false;
                if let (Some(team), Some(plan)) = (&self.team, self.cleanup.take()) {
                    ctx.emit(TeamsEvent::CleanUp {
                        project: PathBuf::from(&team.project_root),
                        remove: plan.remove.into_iter().map(|(_, path)| path).collect(),
                    });
                }
            }
            TeamsAction::AskDelete => {
                self.confirm_delete = self.team.as_ref().map(delete_plan);
            }
            TeamsAction::CancelDelete => {}
            TeamsAction::ConfirmDelete => {
                let plan = self.confirm_delete.take();
                if let (Some(full), Some(plan)) = (self.team.as_ref(), plan) {
                    ctx.emit(TeamsEvent::Delete {
                        project: PathBuf::from(&full.project_root),
                        remove: plan.remove.into_iter().map(|(_, path)| path).collect(),
                        workers: plan.workers,
                    });
                }
                if let Some(team) = team {
                    self.selected = None;
                    self.team = None;
                    self.act("team_forget", json!({"team": team}), ctx);
                }
            }
            TeamsAction::SetParallel(limit) => {
                if let Some(team) = team {
                    self.act(
                        "team_configure",
                        json!({"team": team, "max_parallel": limit}),
                        ctx,
                    );
                }
            }
            TeamsAction::SetAutoStart(on) => {
                if let Some(team) = team {
                    self.act(
                        "team_configure",
                        json!({"team": team, "auto_start": on}),
                        ctx,
                    );
                }
            }
            TeamsAction::SetLeadAnswers(on) => {
                if let Some(team) = team {
                    self.act(
                        "team_configure",
                        json!({"team": team, "lead_answers_prompts": on}),
                        ctx,
                    );
                }
            }
        }
        ctx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn cleanup_keeps_the_result_unsaved_work_and_the_project_itself() {
        let root = std::env::temp_dir().join(format!(
            "dragent-team-cleanup-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "-q", "-b", "main"]);
        git(&project, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let tree = |name: &str| {
            let path = root.join(name);
            git(
                &project,
                &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
            );
            path
        };
        let (api, docs, result) = (tree("api"), tree("docs"), tree("result"));
        std::fs::write(docs.join("draft.md"), "not committed").unwrap();
        let task = |id: &str, path: &std::path::Path, after: &[&str]| {
            serde_json::json!({
                "id": id, "title": id, "prompt": "", "kind": "work", "status": "done",
                "workspace": path, "depends_on": after, "created_at_ms": 0, "updated_at_ms": 0,
            })
        };
        let mut team: Team = serde_json::from_value(serde_json::json!({
            "id": "team-1", "title": "T", "project_root": project, "status": "running",
            "created_at_ms": 0, "updated_at_ms": 0,
            "tasks": [
                // The page followed on in the API's checkout, and t7 in the result's.
                task("t1", &api, &[]),
                task("t2", &api, &["t1"]),
                task("t3", &docs, &[]),
                task("t4", &result, &["t2", "t3"]),
                task("t5", &project, &[]),
                {"id": "t6", "title": "Review", "prompt": "", "kind": "review", "status": "done",
                 "review_of": "t1", "depends_on": ["t1"], "workspace": api, "created_at_ms": 0, "updated_at_ms": 0},
                task("t7", &result, &["t4"]),
            ],
        }))
        .unwrap();
        assert_eq!(
            cleanup(&team),
            None,
            "nothing goes while the team is at work"
        );
        team.status = dragent_host::team::TeamStatus::Done;
        let plan = cleanup(&team).unwrap();
        let ids =
            |list: &[(String, PathBuf)]| list.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(&plan.remove),
            ["t2"],
            "one checkout, named after its last task"
        );
        assert_eq!(ids(&plan.dirty), ["t3"]);
        assert_eq!(
            ids(&plan.keep),
            ["t7"],
            "the result's checkout stays though t4 in it was built on; the project itself is never touched"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_takes_every_worker_checkout_and_session_but_keeps_project_and_lead() {
        let root = std::env::temp_dir().join(format!("dragent-team-delete-{}", uuid::Uuid::new_v4().simple()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "-q", "-b", "main"]);
        git(&project, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let tree = |name: &str| {
            let path = root.join(name);
            git(&project, &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
            path
        };
        let (api, docs) = (tree("api"), tree("docs"));
        std::fs::write(docs.join("draft.md"), "not committed").unwrap();
        let gone = root.join("gone");
        let task = |id: &str, path: &std::path::Path, pane: &str| {
            serde_json::json!({
                "id": id, "title": id, "prompt": "", "kind": "work", "status": "done",
                "workspace": path, "worker": {"pane": pane}, "created_at_ms": 0, "updated_at_ms": 0,
            })
        };
        let team: Team = serde_json::from_value(serde_json::json!({
            "id": "team-1", "title": "T", "project_root": project, "status": "done",
            "lead": {"pane": "lead-pane"}, "created_at_ms": 0, "updated_at_ms": 0,
            "tasks": [
                task("t1", &api, "w1"),
                task("t2", &api, "w2"),
                task("t3", &docs, "w3"),
                task("t4", &project, "w4"),
                task("t5", &gone, "lead-pane"),
            ],
        }))
        .unwrap();
        let plan = delete_plan(&team);
        let ids: Vec<_> = plan.remove.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["t2", "t3"], "each checkout once, by its last task; the project and missing folders never");
        assert_eq!(plan.dirty, ["t3"], "uncommitted work is named, not spared");
        assert_eq!(plan.workers, ["w1", "w2", "w3", "w4"], "the lead's session stays");
        let _ = std::fs::remove_dir_all(&root);
    }
}
