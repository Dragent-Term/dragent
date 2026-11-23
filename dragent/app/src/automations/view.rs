//! The Automations page: your automations and your team's, with summary
//! cards, grouped and sortable lists, and a side panel to create one or
//! inspect it (runs and settings). New automations come from a form (with
//! templates) or from an agent ("Create with AI"). Opening a run hands it
//! to the workspace, which shows it as a workspace with the run's agent.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use dragent_automations::{
    local_time_zone, normalize_tags, Automation, Period, Repeat, Run, RunFilter, Schedule,
    WorkspaceMode, DEFAULT_TAG,
};
use warpui::elements::ClippedScrollStateHandle;
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Element, Entity, EntityId, SingletonEntity, TypedActionView, UpdateModel,
    UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::elements::MouseStateHandle;

use super::templates::TEMPLATES;
use super::{AutomationEvent, AutomationRuntime, LIMIT_MESSAGE};
use crate::theme::{scaled, Theme};
use crate::workspace::{InputEditor, InputEditorEvent, InputMode, PromptAgent};

/// History rows shown at first and added per “Show more”.
pub(crate) const RUN_PAGE: usize = 25;

/// Meta key: the command-line hint was dismissed.
const CLI_HINT_DISMISSED: &str = "ui.cli_hint_dismissed";
/// Meta key: the agent last chosen for "Create with AI".
const CREATE_WITH_AI_AGENT: &str = "ui.create_with_ai_agent";

#[derive(Clone, Debug)]
pub enum AutomationsEvent {
    /// Show this run as a workspace.
    OpenRun(Run),
    /// Start a session with this agent that helps the user create an
    /// automation working in this project (`None`: no project).
    CreateWithAgent(PromptAgent, Option<PathBuf>),
    /// Show the plans (the free allowance is used up).
    OpenBilling,
    /// Install an agent from the catalog (the same sheet the prompt screen uses).
    InstallAgent(PromptAgent),
    /// Check again which agents are installed (the agent menu opened).
    RefreshAgents,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    Project,
    Agent,
    Period,
    Weekday,
    Day,
    Month,
    Hour,
    Minute,
    TimeZone,
    Pinned,
    BaseBranch,
    ExistingBranch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Runs,
    Settings,
}

/// Mine, the rest of the organisation's, or what my runs came back with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Mine,
    Team,
    Inbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortField {
    Name,
    Owner,
    Schedule,
    Status,
}

/// How a run gets its workspace, as the form offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceKind {
    /// A new worktree (or, for a session, a new folder) each run.
    New,
    /// A worktree on an existing branch.
    Branch,
    /// The same workspace every run.
    Pinned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    Create,
    Detail(String),
    /// A teammate's automation, read-only.
    Team(String),
}

#[derive(Clone, Debug)]
pub enum AutomationsAction {
    // Page.
    CloseMenus,
    ToggleNewMenu,
    /// "Create with AI" from the control with this position tag: choose
    /// where it works, then the agent, in menus under it.
    CreateWithAgent(&'static str),
    /// Where the "Create with AI" automation works: a project by its index,
    /// or `None` for no project.
    CreateWithAiIn(Option<usize>),
    CreateWithAgentUsing(PromptAgent),
    NewAutomation,
    UseTemplate(usize),
    SetScope(Scope),
    Sort(SortField),
    ToggleFailedFilter,
    RetryAll,
    // Inbox.
    /// Open a run's result (it counts as read).
    OpenInboxRun(String),
    ArchiveRun(String, bool),
    MarkAllRead,
    ToggleInboxArchived,
    DismissCliHint,
    Upgrade,
    RunAutomation(String),
    ToggleRowMenu(String),
    CopyLink(String),
    /// Copy a run's agent's last reply.
    CopyRunReply(String),
    ShowHistory(String),
    RequestDeleteOf(String),
    OpenLastRun(String),
    OpenTeam(String),
    // Panel.
    Open(String),
    ClosePanel,
    ToggleEnabled(String),
    SetTab(PanelTab),
    RunNow,
    OpenRun(String),
    CancelRun(String),
    SetFilter(RunFilter),
    ShowMoreRuns,
    ToggleMenu,
    RequestDelete,
    ConfirmDelete,
    CancelDelete,
    ShowPromptHistory,
    ClosePromptHistory,
    RestorePrompt(i64),
    // Form.
    TogglePicker(Picker),
    /// Close floating layers (open picker dropdown, editor mention popup or
    /// context menu) when the panel scrolls beneath them. Floating layers
    /// are unclipped by design, so without this they paint over the sticky
    /// header and footer while the body moves.
    DismissFloatingUi,
    /// `None` makes it a session automation (no project).
    SetProject(Option<usize>),
    SetAgent(PromptAgent),
    InstallAgent(PromptAgent),
    ClearAgentSearch,
    ClosePicker,
    SetPeriod(Period),
    SetWeekday(u32),
    SetDay(u32),
    SetMonth(u32),
    SetHour(u32),
    SetMinute(u32),
    SetTimeZone(String),
    SetWorkspaceKind(WorkspaceKind),
    SelectBranch {
        base: bool,
        branch: String,
    },
    SetPinned(Option<usize>),
    SetPushBranch(bool),
    SetContinueSession(bool),
    SetServiceWrites(bool),
    /// Start runs when something new shows up in this service (its key),
    /// or (`None`) on the schedule.
    SetWatch(Option<String>),
    SetWatchInterval(u32),
    AddTag(String),
    AddTagFromField,
    RemoveTag(String),
    ToggleTemplates,
    Save,
    DismissMessage,
}

/// Form selections, including branches chosen from the shared picker.
/// Names and prompts live in their editors.
#[derive(Clone, Debug)]
pub(crate) struct Form {
    pub project: Option<PathBuf>,
    pub session: bool,
    pub agent: Option<String>,
    pub repeat: Repeat,
    /// An expression the picker can't show, kept until a picker edit.
    pub custom_schedule: Option<String>,
    pub timezone: String,
    pub kind: WorkspaceKind,
    /// The pinned workspace (`None` = the project's own checkout).
    pub pinned: Option<PathBuf>,
    pub base_branch: String,
    pub existing_branch: String,
    pub push_branch: bool,
    pub continue_session: bool,
    /// Its agent may comment, change statuses and create items in
    /// connected services.
    pub service_writes: bool,
    /// Runs start when something new shows up in this service's search
    /// (its key). `None`: on the schedule.
    pub watch_service: Option<String>,
    /// Minutes between a watching automation's checks.
    pub watch_minutes: u32,
    pub tags: Vec<String>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            project: None,
            session: false,
            agent: None,
            repeat: Repeat::default(),
            custom_schedule: None,
            timezone: local_time_zone(),
            kind: WorkspaceKind::New,
            pinned: None,
            base_branch: String::new(),
            existing_branch: String::new(),
            push_branch: false,
            continue_session: false,
            service_writes: false,
            watch_service: None,
            watch_minutes: 15,
            tags: vec![DEFAULT_TAG.to_string()],
        }
    }
}

impl Form {
    pub fn schedule(&self) -> String {
        self.custom_schedule
            .clone()
            .unwrap_or_else(|| self.repeat.expression())
    }

    fn load_schedule(&mut self, expression: &str) {
        match Repeat::from_expression(expression) {
            Some(repeat) => {
                self.repeat = repeat;
                self.custom_schedule = None;
            }
            None => self.custom_schedule = Some(expression.to_string()),
        }
    }

    /// Continuing a session needs one workspace runs share.
    pub fn can_continue(&self) -> bool {
        self.kind == WorkspaceKind::Pinned
    }
}

pub struct AutomationsView {
    pub(crate) view_id: EntityId,
    pub(crate) ui_font: FamilyId,
    pub(crate) theme: Theme,
    /// Projects the workspace knows: (root, label).
    pub(crate) projects: Vec<(PathBuf, String)>,
    pub(crate) mono_font: FamilyId,
    /// Agents found on this computer; Dragent's own is always available.
    pub(crate) installed_agents: HashSet<PromptAgent>,
    /// Agent detection is still running.
    pub(crate) agents_checking: bool,
    /// Installed agents whose command doesn't skip approvals: a run with
    /// one may stop and wait for someone.
    pub(crate) asking_agents: HashSet<PromptAgent>,
    /// Folders already used in the sidebar, offered as tags.
    pub(crate) tag_suggestions: Vec<String>,
    /// Agent activity in the workspaces runs put in the sidebar.
    pub(crate) run_activity: HashMap<PathBuf, String>,
    /// The page is on screen.
    pub(crate) visible: bool,
    /// The `dragent` launcher agents can call.
    pub(crate) cli_path: Option<PathBuf>,
    /// Workspaces of the form's project: (path, label).
    pub(crate) pinned_options: Vec<(PathBuf, String)>,
    pub(crate) search_editor: ViewHandle<InputEditor>,
    pub(crate) name_editor: ViewHandle<InputEditor>,
    pub(crate) prompt_editor: ViewHandle<InputEditor>,
    pub(crate) branch_search_editor: ViewHandle<InputEditor>,
    pub(crate) branch_search: String,
    pub(crate) branches: Vec<String>,
    pub(crate) branches_loading: bool,
    branch_generation: u64,
    pub(crate) tag_editor: ViewHandle<InputEditor>,
    pub(crate) copy_editor: ViewHandle<InputEditor>,
    /// What a watching automation searches for.
    pub(crate) watch_query_editor: ViewHandle<InputEditor>,
    pub(crate) agent_search_editor: ViewHandle<InputEditor>,
    pub(crate) agent_search: String,
    pub(crate) search: String,
    pub(crate) scope: Scope,
    pub(crate) sort: Option<(SortField, bool)>,
    pub(crate) failed_only: bool,
    /// The inbox shows what was filed away instead.
    pub(crate) inbox_archived: bool,
    pub(crate) new_menu_open: bool,
    /// The "Create with AI" agent menu is open under this position tag.
    pub(crate) ai_agent_menu: Option<&'static str>,
    /// The "Create with AI" project menu is open under this position tag.
    pub(crate) ai_project_menu: Option<&'static str>,
    /// Where the "Create with AI" automation works (`None`: no project).
    pub(crate) ai_project: Option<PathBuf>,
    /// The project the user was in when the page opened, offered first.
    pub(crate) current_project: Option<PathBuf>,
    pub(crate) row_menu: Option<String>,
    pub(crate) cli_hint_dismissed: bool,
    pub(crate) panel: Option<Panel>,
    pub(crate) tab: PanelTab,
    pub(crate) filter: RunFilter,
    pub(crate) runs_limit: usize,
    pub(crate) form: Form,
    pub(crate) open_picker: Option<Picker>,
    pub(crate) picker_scroll: ClippedScrollStateHandle,
    pub(crate) list_scroll: ClippedScrollStateHandle,
    pub(crate) panel_scroll: ClippedScrollStateHandle,
    pub(crate) history_scroll: ClippedScrollStateHandle,
    pub(crate) templates_open: bool,
    pub(crate) menu_open: bool,
    pub(crate) history_open: bool,
    pub(crate) pending_delete: Option<String>,
    pub(crate) error: Option<String>,
    /// The last error was the plan's allowance: offer the upgrade.
    pub(crate) error_is_limit: bool,
    pub(crate) notice: Option<String>,
    /// A picker changed an existing automation; save after the action.
    autosave_pending: bool,
    mouse: RefCell<HashMap<String, MouseStateHandle>>,
}

impl Entity for AutomationsView {
    type Event = AutomationsEvent;
}

fn editor(
    ctx: &mut ViewContext<AutomationsView>,
    placeholder: &'static str,
    ui_font: FamilyId,
    theme: Theme,
    mode: InputMode,
) -> ViewHandle<InputEditor> {
    ctx.add_typed_action_view(move |inner| {
        InputEditor::new(
            placeholder,
            ui_font,
            crate::theme::text_size(13.0),
            theme.main_text(),
            theme.disabled_text(),
            theme.accent,
            theme,
            mode,
            inner.focus_handle(),
        )
    })
}

fn plain(key: &str) -> InputMode {
    InputMode::Plain { key: key.into() }
}

impl AutomationsView {
    pub fn new(
        ctx: &mut ViewContext<Self>,
        ui_font: FamilyId,
        mono_font: FamilyId,
        theme: Theme,
    ) -> Self {
        let search_editor = editor(ctx, "Search", ui_font, theme, plain("automations_search"));
        let name_editor = editor(
            ctx,
            "Weekly dependency check",
            ui_font,
            theme,
            plain("automation_name"),
        );
        let prompt_editor = editor(
            ctx,
            "What should the agent do each time it runs?",
            ui_font,
            theme,
            InputMode::MultiLine {
                key: "automation_prompt".into(),
                max_height: 220,
            },
        );
        let branch_search_editor = editor(
            ctx,
            "Search branches…",
            ui_font,
            theme,
            plain("automation_branch_search"),
        );
        ctx.subscribe_to_view(&branch_search_editor, |view, editor, _event, ctx| {
            let text = ctx.update_view(&editor, |e, _| e.get_text().to_string());
            if text != view.branch_search {
                view.branch_search = text;
                view.picker_scroll = ClippedScrollStateHandle::default();
                ctx.notify();
            }
        });
        let tag_editor = editor(
            ctx,
            "Add a sidebar group…",
            ui_font,
            theme,
            plain("automation_tag"),
        );
        let copy_editor = editor(
            ctx,
            ".env, .env.local",
            ui_font,
            theme,
            plain("automation_copy_files"),
        );
        let watch_query_editor = editor(
            ctx,
            "What to search for (optional)",
            ui_font,
            theme,
            plain("automation_watch_query"),
        );
        // Same search field as the prompt screen's agent menu.
        let agent_search_editor = ctx.add_typed_action_view(|inner| {
            let mut editor = InputEditor::new(
                "Search agents…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                plain("automation_agent_search"),
                inner.focus_handle(),
            );
            editor.cursor_style_override = Some(crate::console::editor_settings::CursorStyle::Bar);
            editor
        });
        ctx.subscribe_to_view(&agent_search_editor, |view, editor, _event, ctx| {
            let text = ctx.update_view(&editor, |e, _| e.get_text().to_string());
            if text != view.agent_search {
                view.agent_search = text;
                ctx.notify();
            }
        });
        ctx.subscribe_to_view(&search_editor, |view, editor, _event, ctx| {
            let text = ctx.update_view(&editor, |e, _| e.get_text().to_string());
            if text != view.search {
                view.search = text;
                ctx.notify();
            }
        });
        // Typing a comma adds what came before it as a tag.
        ctx.subscribe_to_view(&tag_editor, |view, editor, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                if text.contains(',') {
                    let text = text.clone();
                    ctx.update_view(&editor, |e, ectx| e.set_text("", ectx));
                    for tag in text.split(',') {
                        view.handle_action(&AutomationsAction::AddTag(tag.to_string()), ctx);
                    }
                }
            }
        });
        let runtime = AutomationRuntime::handle(ctx);
        ctx.subscribe_to_model(&runtime, |view, runtime, event, ctx| {
            if matches!(event, AutomationEvent::Changed) {
                // Failures that arrive while the page is open are seen.
                if view.is_visible() {
                    ctx.update_model(&runtime, |runtime, ctx| runtime.mark_failures_seen(ctx));
                }
                ctx.notify();
            }
        });
        let cli_hint_dismissed = AutomationRuntime::as_ref(ctx)
            .meta(CLI_HINT_DISMISSED)
            .is_some();
        Self {
            view_id: ctx.view_id(),
            ui_font,
            theme,
            projects: Vec::new(),
            mono_font,
            installed_agents: HashSet::new(),
            agents_checking: false,
            asking_agents: HashSet::new(),
            tag_suggestions: Vec::new(),
            run_activity: HashMap::new(),
            visible: false,
            cli_path: None,
            pinned_options: Vec::new(),
            search_editor,
            name_editor,
            prompt_editor,
            branch_search_editor,
            branch_search: String::new(),
            branches: Vec::new(),
            branches_loading: false,
            branch_generation: 0,
            tag_editor,
            copy_editor,
            watch_query_editor,
            agent_search_editor,
            agent_search: String::new(),
            search: String::new(),
            scope: Scope::Mine,
            sort: None,
            failed_only: false,
            inbox_archived: false,
            new_menu_open: false,
            ai_agent_menu: None,
            ai_project_menu: None,
            ai_project: None,
            current_project: None,
            row_menu: None,
            cli_hint_dismissed,
            panel: None,
            tab: PanelTab::Runs,
            filter: RunFilter::All,
            runs_limit: RUN_PAGE,
            form: Form::default(),
            open_picker: None,
            picker_scroll: ClippedScrollStateHandle::default(),
            list_scroll: ClippedScrollStateHandle::default(),
            panel_scroll: ClippedScrollStateHandle::default(),
            history_scroll: ClippedScrollStateHandle::default(),
            templates_open: true,
            menu_open: false,
            history_open: false,
            pending_delete: None,
            error: None,
            error_is_limit: false,
            notice: None,
            autosave_pending: false,
            mouse: RefCell::default(),
        }
    }

    /// Agent detection finished (or changed) while the page exists.
    pub fn set_agents(
        &mut self,
        installed_agents: HashSet<PromptAgent>,
        agents_checking: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.installed_agents = installed_agents;
        self.agents_checking = agents_checking;
        self.sync_asking_agents();
        ctx.notify();
    }

    fn sync_asking_agents(&mut self) {
        let settings = crate::setup::UiSettings::load();
        self.asking_agents = self
            .installed_agents
            .iter()
            .copied()
            .filter(|agent| !super::executor::runs_unattended(*agent, &settings))
            .collect();
    }

    /// The agent "Create with AI" offers first: the last one chosen while
    /// it's still installed (and enabled), else the default.
    pub(crate) fn create_with_ai_agent(&self, app: &AppContext) -> PromptAgent {
        let autonomy = crate::settings::AppSettings::global().agent.clone();
        AutomationRuntime::as_ref(app)
            .meta(CREATE_WITH_AI_AGENT)
            .and_then(|key| super::executor::prompt_agent(&key))
            .filter(|agent| {
                (*agent == PromptAgent::Dragent || self.installed_agents.contains(agent))
                    && !autonomy.is_agent_disabled(agent.command_key())
            })
            .unwrap_or_else(|| self.default_agent())
    }

    /// The "Create with AI" agent menu, under the control tagged `anchor`.
    fn open_ai_agent_menu(&mut self, anchor: &'static str, ctx: &mut ViewContext<Self>) {
        self.ai_agent_menu = Some(anchor);
        self.agent_search.clear();
        ctx.update_view(&self.agent_search_editor, |editor, ectx| {
            editor.set_text("", ectx);
            ectx.focus_self();
        });
        ctx.emit(AutomationsEvent::RefreshAgents);
    }

    /// The project the user was in when the page opened (it's listed first).
    pub fn set_current_project(&mut self, project: Option<PathBuf>, ctx: &mut ViewContext<Self>) {
        self.current_project = project;
        ctx.notify();
    }

    /// A new automation's agent: the configured default when installed and
    /// enabled, else the first installed and enabled terminal agent, else
    /// Dragent's own when nothing else qualifies.
    fn default_agent(&self) -> PromptAgent {
        let autonomy = crate::settings::AppSettings::global().agent.clone();
        let enabled = |agent: &PromptAgent| {
            (*agent == PromptAgent::Dragent || self.installed_agents.contains(agent))
                && !autonomy.is_agent_disabled(agent.command_key())
        };
        if let Some(key) = autonomy.default_agent.as_deref() {
            if let Some(agent) = super::executor::prompt_agent(key) {
                if enabled(&agent) {
                    return agent;
                }
            }
        }
        PromptAgent::ALL
            .into_iter()
            .find(|agent| *agent != PromptAgent::Dragent && enabled(agent))
            .unwrap_or(PromptAgent::Dragent)
    }

    pub(crate) fn mouse_states(&self) -> &RefCell<HashMap<String, MouseStateHandle>> {
        &self.mouse
    }

    pub(crate) fn mouse(&self, key: impl Into<String>) -> MouseStateHandle {
        self.mouse
            .borrow_mut()
            .entry(key.into())
            .or_default()
            .clone()
    }

    /// Whether the page is on screen (the workspace sets this).
    fn is_visible(&self) -> bool {
        self.visible
    }

    /// Called whenever the page opens: what to offer in the pickers.
    pub fn refresh(
        &mut self,
        theme: Theme,
        projects: Vec<(PathBuf, String)>,
        installed_agents: HashSet<PromptAgent>,
        agents_checking: bool,
        tag_suggestions: Vec<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.theme = theme;
        self.projects = projects;
        self.installed_agents = installed_agents;
        self.agents_checking = agents_checking;
        self.sync_asking_agents();
        self.tag_suggestions = tag_suggestions;
        self.visible = true;
        self.cli_path = super::cli::install_shim()
            .map_err(|error| log::warn!("[automations] command-line launcher: {error}"))
            .ok();
        let runtime = AutomationRuntime::handle(ctx);
        ctx.update_model(&runtime, |runtime, ctx| {
            runtime.reconcile(ctx);
            runtime.mark_failures_seen(ctx);
            runtime.refresh_team(false, ctx);
        });
        ctx.notify();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.new_menu_open = false;
        self.row_menu = None;
    }

    /// Agent activity per run workspace, pushed by the workspace.
    pub fn set_run_activity(
        &mut self,
        activity: HashMap<PathBuf, String>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.run_activity != activity {
            self.run_activity = activity;
            ctx.notify();
        }
    }

    /// Open an automation from a link.
    /// Show what runs came back with.
    pub fn show_inbox(&mut self, ctx: &mut ViewContext<Self>) {
        self.scope = Scope::Inbox;
        self.failed_only = false;
        self.panel = None;
        ctx.notify();
    }

    pub fn open_link(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        if AutomationRuntime::as_ref(ctx).automation(id).is_some() {
            self.open_detail(id, ctx);
        } else {
            self.scope = Scope::Team;
            self.panel = Some(Panel::Team(id.to_string()));
        }
        ctx.notify();
    }

    fn set_editor_text(
        &self,
        editor: &ViewHandle<InputEditor>,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let text = text.to_string();
        ctx.update_view(editor, |e, ectx| e.set_text(&text, ectx));
    }

    fn editor_text(&self, editor: &ViewHandle<InputEditor>, ctx: &mut ViewContext<Self>) -> String {
        ctx.update_view(editor, |e, _| e.get_text().to_string())
    }

    fn reset_panel_state(&mut self) {
        self.open_picker = None;
        self.menu_open = false;
        self.history_open = false;
        self.error = None;
        self.error_is_limit = false;
        self.panel_scroll = ClippedScrollStateHandle::default();
    }

    fn fail(&mut self, error: String) {
        self.error_is_limit = error == LIMIT_MESSAGE;
        self.error = Some(error);
    }

    /// An error from work the page handed off (Create with AI).
    pub(crate) fn show_error(&mut self, error: String, ctx: &mut ViewContext<Self>) {
        self.fail(error);
        ctx.notify();
    }

    /// Use workspace creation's branch discovery, off the UI thread. A
    /// project switch or newer refresh invalidates a delayed result.
    fn load_branches(&mut self, ctx: &mut ViewContext<Self>) {
        self.branch_generation = self.branch_generation.wrapping_add(1);
        let generation = self.branch_generation;
        self.branches.clear();
        let Some(project) = self.form.project.clone().filter(|_| !self.form.session) else {
            self.branches_loading = false;
            return;
        };
        self.branches_loading = true;
        ctx.spawn(
            async move {
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let branches =
                        crate::terminal::view::terminal_helpers::list_local_branches(&project);
                    let _ = tx.send((project, branches));
                });
                rx.await
            },
            move |view, result, ctx| {
                if generation != view.branch_generation {
                    return;
                }
                view.branches_loading = false;
                if let Ok((project, branches)) = result {
                    if view.form.project.as_ref() == Some(&project) && !view.form.session {
                        view.branches = branches;
                    }
                }
                ctx.notify();
            },
        );
    }

    /// The project's workspaces a run can be pinned to.
    fn load_pinned_options(&mut self) {
        self.pinned_options = match &self.form.project {
            Some(project) if !self.form.session => {
                crate::workspace::project_workspace_creation::project_workspaces(project)
            }
            _ => Vec::new(),
        };
    }

    fn load_copy_patterns(&mut self, ctx: &mut ViewContext<Self>) {
        let patterns = match (&self.form.project, self.form.session) {
            (Some(project), false) => AutomationRuntime::as_ref(ctx).copy_patterns(project),
            _ => Vec::new(),
        };
        self.set_editor_text(&self.copy_editor.clone(), &patterns.join(", "), ctx);
    }

    fn start_create(&mut self, template: Option<usize>, ctx: &mut ViewContext<Self>) {
        if AutomationRuntime::as_ref(ctx).at_limit() {
            self.fail(LIMIT_MESSAGE.into());
            return;
        }
        self.reset_panel_state();
        self.panel = Some(Panel::Create);
        // The project the user was in, else the first.
        let project = self
            .current_project
            .clone()
            .filter(|current| self.projects.iter().any(|(root, _)| root == current))
            .or_else(|| self.projects.first().map(|(root, _)| root.clone()));
        self.form = Form {
            project,
            session: self.projects.is_empty(),
            agent: Some(self.default_agent().command_key().to_string()),
            ..Form::default()
        };
        let (name, prompt) = match template.and_then(|index| TEMPLATES.get(index)) {
            Some(template) => {
                self.form.load_schedule(template.schedule);
                (template.name, template.prompt)
            }
            None => ("", ""),
        };
        self.set_editor_text(&self.name_editor.clone(), name, ctx);
        self.set_editor_text(&self.prompt_editor.clone(), prompt, ctx);
        self.set_editor_text(&self.watch_query_editor.clone(), "", ctx);
        self.load_pinned_options();
        self.load_copy_patterns(ctx);
        let focus = if name.is_empty() {
            self.name_editor.clone()
        } else {
            self.prompt_editor.clone()
        };
        ctx.focus(&focus);
    }

    fn open_detail(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        let Some(automation) = AutomationRuntime::as_ref(ctx).automation(id) else {
            return;
        };
        self.reset_panel_state();
        self.panel = Some(Panel::Detail(id.to_string()));
        self.tab = PanelTab::Runs;
        self.filter = RunFilter::All;
        self.runs_limit = RUN_PAGE;
        self.load_form(&automation, ctx);
    }

    fn load_form(&mut self, automation: &Automation, ctx: &mut ViewContext<Self>) {
        let session = automation.is_session();
        let mut form = Form {
            project: (!session).then(|| automation.project_root.clone()),
            session,
            agent: Some(automation.agent.clone()),
            timezone: automation.timezone.clone(),
            continue_session: automation.continue_session,
            service_writes: automation.service_writes,
            watch_service: automation.watch.as_ref().map(|watch| watch.service.clone()),
            watch_minutes: super::watch::interval_minutes(&automation.schedule).unwrap_or(15),
            tags: automation.tags.clone(),
            ..Form::default()
        };
        let (base, existing) = match &automation.workspace {
            WorkspaceMode::NewWorktree {
                base_branch,
                push_branch,
            } => {
                form.kind = WorkspaceKind::New;
                form.push_branch = *push_branch;
                (base_branch.clone().unwrap_or_default(), String::new())
            }
            WorkspaceMode::ExistingBranch { branch } => {
                form.kind = WorkspaceKind::Branch;
                (String::new(), branch.clone())
            }
            WorkspaceMode::Pinned { path } => {
                // A session without a pinned folder gets a new one each run.
                form.kind = if session && path.is_none() {
                    WorkspaceKind::New
                } else {
                    WorkspaceKind::Pinned
                };
                form.pinned = path.clone();
                (String::new(), String::new())
            }
        };
        form.load_schedule(&automation.schedule);
        form.base_branch = base;
        form.existing_branch = existing;
        self.form = form;
        self.set_editor_text(&self.name_editor.clone(), &automation.name, ctx);
        self.set_editor_text(&self.prompt_editor.clone(), &automation.prompt, ctx);
        let query = automation
            .watch
            .as_ref()
            .map(|watch| watch.query.clone())
            .unwrap_or_default();
        self.set_editor_text(&self.watch_query_editor.clone(), &query, ctx);
        self.load_pinned_options();
        self.load_copy_patterns(ctx);
    }

    fn detail_id(&self) -> Option<String> {
        match &self.panel {
            Some(Panel::Detail(id)) => Some(id.clone()),
            _ => None,
        }
    }

    fn workspace_mode(&mut self, ctx: &mut ViewContext<Self>) -> Result<WorkspaceMode, String> {
        let base = self.form.base_branch.trim().to_string();
        let existing = self.form.existing_branch.trim().to_string();
        let name = self.editor_text(&self.name_editor.clone(), ctx);
        Ok(match (self.form.session, self.form.kind) {
            (true, WorkspaceKind::Pinned) => {
                // One folder every run shares, named after the automation.
                let folder = self.form.pinned.clone().unwrap_or_else(|| {
                    let id = self
                        .detail_id()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    AutomationRuntime::as_ref(ctx).sessions_root().join(format!(
                        "{}-{}",
                        dragent_automations::model::slug(&name, 32),
                        &id[..8.min(id.len())]
                    ))
                });
                WorkspaceMode::Pinned { path: Some(folder) }
            }
            (true, _) => WorkspaceMode::Pinned { path: None },
            (false, WorkspaceKind::New) => WorkspaceMode::NewWorktree {
                base_branch: (!base.is_empty()).then_some(base),
                push_branch: self.form.push_branch,
            },
            (false, WorkspaceKind::Branch) => {
                if existing.is_empty() {
                    return Err("Enter the branch runs should use.".into());
                }
                WorkspaceMode::ExistingBranch { branch: existing }
            }
            (false, WorkspaceKind::Pinned) => WorkspaceMode::Pinned {
                path: self.form.pinned.clone(),
            },
        })
    }

    /// The automation the form describes, ready to save.
    fn form_automation(&mut self, ctx: &mut ViewContext<Self>) -> Result<Automation, String> {
        let name = self
            .editor_text(&self.name_editor.clone(), ctx)
            .trim()
            .to_string();
        let prompt = self
            .editor_text(&self.prompt_editor.clone(), ctx)
            .trim()
            .to_string();
        let project = if self.form.session {
            PathBuf::new()
        } else {
            self.form
                .project
                .clone()
                .ok_or_else(|| "Choose a project, or run it as a session.".to_string())?
        };
        let agent = self
            .form
            .agent
            .clone()
            .ok_or_else(|| "Choose an agent.".to_string())?;
        let workspace = self.workspace_mode(ctx)?;
        // A watching automation's schedule is how often it checks.
        let watch = match self.form.watch_service.clone() {
            Some(service) => Some(dragent_automations::Watch {
                service,
                query: self
                    .editor_text(&self.watch_query_editor.clone(), ctx)
                    .trim()
                    .to_string(),
            }),
            None => None,
        };
        let schedule = if watch.is_some() {
            super::watch::interval_schedule(self.form.watch_minutes)
        } else {
            self.form.schedule()
        };
        Schedule::parse(&schedule)
            .map_err(|reason| format!("The schedule isn't valid: {reason}"))?;
        let now = chrono::Utc::now().timestamp_millis();
        let runtime = AutomationRuntime::as_ref(ctx);
        let mut automation = match self.detail_id().and_then(|id| runtime.automation(&id)) {
            Some(mut existing) => {
                existing.name = name;
                existing.prompt = prompt;
                existing.project_root = project;
                existing.agent = agent;
                existing.workspace = workspace;
                existing.schedule = schedule;
                existing
            }
            None => Automation::new(name, project, schedule, agent, prompt, workspace, now),
        };
        automation.timezone = self.form.timezone.clone();
        automation.tags = self.form.tags.clone();
        automation.continue_session = self.form.continue_session && self.form.can_continue();
        automation.service_writes = self.form.service_writes;
        automation.watch = watch;
        Ok(automation)
    }

    /// Save the project's copy-into-worktree patterns when they changed.
    fn save_copy_patterns(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(project) = self.form.project.clone().filter(|_| !self.form.session) else {
            return;
        };
        let text = self.editor_text(&self.copy_editor.clone(), ctx);
        let patterns: Vec<String> = text
            .split(|c| c == ',' || c == '\n')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        let runtime = AutomationRuntime::handle(ctx);
        if runtime.read(ctx, |runtime, _| runtime.copy_patterns(&project)) != patterns {
            ctx.update_model(&runtime, |runtime, ctx| {
                runtime.set_copy_patterns(&project, patterns, ctx)
            });
        }
    }

    fn save(&mut self, ctx: &mut ViewContext<Self>) {
        let automation = match self.form_automation(ctx) {
            Ok(automation) => automation,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        let creating = self.panel == Some(Panel::Create);
        let runtime = AutomationRuntime::handle(ctx);
        match ctx.update_model(&runtime, |runtime, ctx| runtime.save(automation, ctx)) {
            Ok(saved) => {
                self.error = None;
                self.error_is_limit = false;
                self.save_copy_patterns(ctx);
                if creating {
                    self.panel = None;
                    self.notice = Some(format!("Created “{}”.", saved.name));
                } else {
                    self.notice = Some("Changes saved.".into());
                }
            }
            Err(error) => self.fail(error),
        }
    }

    /// Picker edits: the structured schedule replaces any custom expression.
    fn edit_repeat(&mut self, change: impl FnOnce(&mut Repeat)) {
        change(&mut self.form.repeat);
        self.form.custom_schedule = None;
        self.open_picker = None;
        self.autosave_pending = true;
    }

    fn toggle_enabled(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        let enabled = runtime.read(ctx, |runtime, _| {
            runtime
                .automation(id)
                .is_some_and(|automation| automation.enabled)
        });
        let id = id.to_string();
        let result = ctx.update_model(&runtime, |runtime, ctx| {
            runtime.set_enabled(&id, !enabled, ctx)
        });
        match result {
            Ok(()) => {
                let name = runtime
                    .read(ctx, |runtime, _| runtime.automation(&id).map(|a| a.name))
                    .unwrap_or_default();
                self.notice = Some(if enabled {
                    format!("“{name}” paused.")
                } else {
                    format!("“{name}” resumed.")
                });
            }
            Err(error) => self.fail(error),
        }
    }

    fn run_automation(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        let id = id.to_string();
        match ctx.update_model(&runtime, |runtime, ctx| runtime.run_now(&id, ctx)) {
            Ok(()) => {
                let name = runtime
                    .read(ctx, |runtime, _| runtime.automation(&id).map(|a| a.name))
                    .unwrap_or_default();
                self.notice = Some(format!("Running “{name}” now."));
            }
            Err(error) => self.fail(error),
        }
    }
}

impl View for AutomationsView {
    fn ui_name() -> &'static str {
        "AutomationsView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        super::page::render(self, app)
    }
}

impl TypedActionView for AutomationsView {
    type Action = AutomationsAction;

    fn handle_action(&mut self, action: &AutomationsAction, ctx: &mut ViewContext<Self>) {
        use AutomationsAction as A;
        let runtime = AutomationRuntime::handle(ctx);
        if !matches!(action, A::ToggleRowMenu(_)) {
            self.row_menu = None;
        }
        if !matches!(action, A::ToggleNewMenu) {
            self.new_menu_open = false;
        }
        match action {
            A::CloseMenus => {}
            A::ToggleNewMenu => self.new_menu_open = !self.new_menu_open,
            A::CreateWithAgent(anchor) => {
                if AutomationRuntime::as_ref(ctx).at_limit() {
                    self.fail(LIMIT_MESSAGE.into());
                } else if self.projects.is_empty() {
                    // Nothing to choose from: it works without a project.
                    self.ai_project = None;
                    self.open_ai_agent_menu(anchor, ctx);
                } else {
                    self.ai_project_menu = Some(anchor);
                }
            }
            A::CreateWithAiIn(index) => {
                let Some(anchor) = self.ai_project_menu.take() else {
                    return;
                };
                self.ai_project = index
                    .and_then(|index| self.projects.get(index))
                    .map(|(root, _)| root.clone());
                self.open_ai_agent_menu(anchor, ctx);
            }
            A::CreateWithAgentUsing(agent) => {
                self.ai_agent_menu = None;
                ctx.focus_self();
                ctx.update_model(&runtime, |runtime, _| {
                    runtime.set_meta(CREATE_WITH_AI_AGENT, agent.command_key())
                });
                ctx.emit(AutomationsEvent::CreateWithAgent(
                    *agent,
                    self.ai_project.take(),
                ));
            }
            A::NewAutomation => self.start_create(None, ctx),
            A::UseTemplate(index) => {
                if self.panel == Some(Panel::Create) {
                    // Applying a template keeps the chosen project and agent.
                    let template = &TEMPLATES[*index];
                    self.form.load_schedule(template.schedule);
                    self.set_editor_text(&self.name_editor.clone(), template.name, ctx);
                    self.set_editor_text(&self.prompt_editor.clone(), template.prompt, ctx);
                } else {
                    self.start_create(Some(*index), ctx);
                }
            }
            A::SetScope(scope) => {
                self.scope = *scope;
                self.failed_only = false;
                if self
                    .sort
                    .is_some_and(|(field, _)| field == SortField::Owner)
                    && *scope == Scope::Mine
                {
                    self.sort = None;
                }
                if *scope == Scope::Team {
                    ctx.update_model(&runtime, |runtime, ctx| runtime.refresh_team(false, ctx));
                }
            }
            A::Sort(field) => {
                self.sort = match self.sort {
                    Some((current, ascending)) if current == *field => Some((current, !ascending)),
                    _ => Some((*field, true)),
                };
            }
            A::ToggleFailedFilter => self.failed_only = !self.failed_only,
            A::OpenInboxRun(run_id) => {
                let run = ctx.update_model(&runtime, |runtime, ctx| runtime.mark_read(run_id, ctx));
                if let Some(run) = run.filter(|run| run.can_open()) {
                    ctx.emit(AutomationsEvent::OpenRun(run));
                }
            }
            A::ArchiveRun(run_id, archived) => {
                ctx.update_model(&runtime, |runtime, ctx| {
                    runtime.set_archived(run_id, *archived, ctx)
                });
            }
            A::MarkAllRead => {
                ctx.update_model(&runtime, |runtime, ctx| runtime.mark_all_read(ctx));
            }
            A::ToggleInboxArchived => self.inbox_archived = !self.inbox_archived,
            A::RetryAll => {
                let queued = ctx.update_model(&runtime, |runtime, ctx| runtime.retry_failed(ctx));
                self.notice = Some(match queued {
                    0 => "Nothing could be retried.".into(),
                    1 => "Retrying 1 automation.".into(),
                    n => format!("Retrying {n} automations."),
                });
            }
            A::DismissCliHint => {
                self.cli_hint_dismissed = true;
                ctx.update_model(&runtime, |runtime, _| {
                    runtime.set_meta(CLI_HINT_DISMISSED, "1")
                });
            }
            A::Upgrade => ctx.emit(AutomationsEvent::OpenBilling),
            A::RunAutomation(id) => {
                let id = id.clone();
                self.run_automation(&id, ctx);
            }
            A::ToggleRowMenu(id) => {
                self.row_menu = (self.row_menu.as_deref() != Some(id.as_str())).then(|| id.clone());
            }
            A::CopyLink(id) => {
                let scheme = dragent_paths::url_scheme();
                ctx.clipboard()
                    .write(warpui_core::clipboard::ClipboardContent::plain_text(
                        format!("{scheme}://automations/{id}"),
                    ));
                self.notice = Some("Link copied.".into());
            }
            A::CopyRunReply(run_id) => {
                let reply = AutomationRuntime::as_ref(ctx)
                    .run(run_id)
                    .and_then(|run| run.progress)
                    .and_then(|progress| progress.reply);
                if let Some(reply) = reply {
                    ctx.clipboard()
                        .write(warpui_core::clipboard::ClipboardContent::plain_text(reply));
                    self.notice = Some("Reply copied.".into());
                }
            }
            A::ShowHistory(id) => {
                let id = id.clone();
                self.open_detail(&id, ctx);
                self.tab = PanelTab::Runs;
            }
            A::RequestDeleteOf(id) => self.pending_delete = Some(id.clone()),
            A::OpenLastRun(automation_id) => {
                let latest = AutomationRuntime::as_ref(ctx).latest_run(automation_id);
                if let Some(run) = latest.filter(|run| run.can_open()) {
                    ctx.emit(AutomationsEvent::OpenRun(run));
                }
            }
            A::OpenTeam(id) => {
                self.reset_panel_state();
                self.panel = Some(Panel::Team(id.clone()));
            }
            A::Open(id) => {
                let id = id.clone();
                self.open_detail(&id, ctx);
            }
            A::ClosePanel => {
                self.panel = None;
                self.reset_panel_state();
            }
            A::ToggleEnabled(id) => {
                let id = id.clone();
                self.toggle_enabled(&id, ctx);
            }
            A::SetTab(tab) => {
                self.tab = *tab;
                self.open_picker = None;
                self.panel_scroll = ClippedScrollStateHandle::default();
            }
            A::RunNow => {
                if let Some(id) = self.detail_id() {
                    self.run_automation(&id, ctx);
                    if self.error.is_none() {
                        self.tab = PanelTab::Runs;
                        self.filter = RunFilter::All;
                    }
                }
            }
            A::OpenRun(run_id) => {
                if let Some(run) = AutomationRuntime::as_ref(ctx).run(run_id) {
                    if run.can_open() {
                        ctx.emit(AutomationsEvent::OpenRun(run));
                    }
                }
            }
            A::CancelRun(run_id) => {
                let run_id = run_id.clone();
                ctx.update_model(&runtime, |runtime, ctx| runtime.cancel_run(&run_id, ctx));
            }
            A::SetFilter(filter) => {
                self.filter = *filter;
                self.runs_limit = RUN_PAGE;
            }
            A::ShowMoreRuns => self.runs_limit += RUN_PAGE,
            A::ToggleMenu => {
                self.menu_open = !self.menu_open;
                self.open_picker = None;
            }
            A::RequestDelete => {
                self.menu_open = false;
                self.pending_delete = self.detail_id();
            }
            A::ConfirmDelete => {
                if let Some(id) = self.pending_delete.take() {
                    let name = AutomationRuntime::as_ref(ctx)
                        .automation(&id)
                        .map(|automation| automation.name)
                        .unwrap_or_default();
                    ctx.update_model(&runtime, |runtime, ctx| runtime.delete(&id, ctx));
                    if self.detail_id().as_deref() == Some(id.as_str()) {
                        self.panel = None;
                        self.reset_panel_state();
                    }
                    self.notice = Some(format!("Deleted “{name}”."));
                }
            }
            A::CancelDelete => self.pending_delete = None,
            A::ShowPromptHistory => {
                self.menu_open = false;
                self.history_open = true;
                self.history_scroll = ClippedScrollStateHandle::default();
            }
            A::ClosePromptHistory => self.history_open = false,
            A::RestorePrompt(version) => {
                if let Some(id) = self.detail_id() {
                    let version = *version;
                    let result = ctx.update_model(&runtime, |runtime, ctx| {
                        runtime.restore_prompt(&id, version, ctx)
                    });
                    match result {
                        Ok(()) => {
                            if let Some(automation) = AutomationRuntime::as_ref(ctx).automation(&id)
                            {
                                let prompt = automation.prompt.clone();
                                self.set_editor_text(&self.prompt_editor.clone(), &prompt, ctx);
                            }
                            self.history_open = false;
                            self.notice = Some("Prompt restored.".into());
                        }
                        Err(error) => self.fail(error),
                    }
                }
            }
            A::TogglePicker(picker) => {
                self.open_picker = (self.open_picker != Some(*picker)).then_some(*picker);
                self.menu_open = false;
                self.picker_scroll = ClippedScrollStateHandle::default();
                // Like the prompt screen: a fresh search, focused, and a
                // new look at what is installed.
                if matches!(
                    self.open_picker,
                    Some(Picker::BaseBranch | Picker::ExistingBranch)
                ) {
                    self.branch_search.clear();
                    ctx.update_view(&self.branch_search_editor, |editor, ectx| {
                        editor.set_text("", ectx);
                        ectx.focus_self();
                    });
                    self.load_branches(ctx);
                }
                if self.open_picker == Some(Picker::Agent) {
                    self.agent_search.clear();
                    ctx.update_view(&self.agent_search_editor, |editor, ectx| {
                        editor.set_text("", ectx);
                        ectx.focus_self();
                    });
                    ctx.emit(AutomationsEvent::RefreshAgents);
                }
            }
            A::DismissFloatingUi => {
                self.open_picker = None;
                for editor in [
                    &self.name_editor,
                    &self.prompt_editor,
                    &self.branch_search_editor,
                    &self.tag_editor,
                    &self.copy_editor,
                ] {
                    ctx.update_view(editor, |view, ectx| {
                        view.close_mention(ectx);
                        // Same as InputEditorAction::CloseContextMenu.
                        if view.context_menu_position.take().is_some() {
                            view.context_menu_cursor = None;
                            view.context_menu_selected = 0;
                            ectx.notify();
                        }
                    });
                }
                ctx.notify();
            }
            A::SetProject(index) => {
                match index.and_then(|index| self.projects.get(index)) {
                    Some((root, _)) => {
                        self.form.project = Some(root.clone());
                        self.form.session = false;
                    }
                    None => {
                        self.form.session = true;
                        if self.form.kind == WorkspaceKind::Branch {
                            self.form.kind = WorkspaceKind::New;
                        }
                    }
                }
                // Branches and pinned workspaces belong to the old project.
                self.form.pinned = None;
                self.form.base_branch.clear();
                self.form.existing_branch.clear();
                self.branch_generation = self.branch_generation.wrapping_add(1);
                self.branches.clear();
                self.branches_loading = false;
                self.open_picker = None;
                self.load_pinned_options();
                self.load_copy_patterns(ctx);
                self.autosave_pending = true;
            }
            // Closing the agent menu takes focus back from its search field.
            A::SetAgent(agent) => {
                self.form.agent = Some(agent.command_key().to_string());
                self.open_picker = None;
                self.autosave_pending = true;
                ctx.focus_self();
            }
            A::InstallAgent(agent) => {
                self.open_picker = None;
                self.ai_agent_menu = None;
                ctx.focus_self();
                ctx.emit(AutomationsEvent::InstallAgent(*agent));
            }
            A::ClearAgentSearch => {
                self.agent_search.clear();
                ctx.update_view(&self.agent_search_editor, |editor, ectx| {
                    editor.set_text("", ectx)
                });
            }
            A::ClosePicker => {
                self.open_picker = None;
                self.ai_agent_menu = None;
                self.ai_project_menu = None;
                ctx.focus_self();
            }
            A::SetPeriod(period) => {
                let period = *period;
                self.edit_repeat(|repeat| repeat.period = period);
            }
            A::SetWeekday(day) => {
                let day = *day;
                self.edit_repeat(|repeat| repeat.weekday = day);
            }
            A::SetDay(day) => {
                let day = *day;
                self.edit_repeat(|repeat| repeat.day = day);
            }
            A::SetMonth(month) => {
                let month = *month;
                self.edit_repeat(|repeat| repeat.month = month);
            }
            A::SetHour(hour) => {
                let hour = *hour;
                self.edit_repeat(|repeat| repeat.hour = hour);
            }
            A::SetMinute(minute) => {
                let minute = *minute;
                self.edit_repeat(|repeat| repeat.minute = minute);
            }
            A::SetTimeZone(zone) => {
                self.form.timezone = zone.clone();
                self.open_picker = None;
                self.autosave_pending = true;
            }
            A::SetWorkspaceKind(kind) => {
                self.form.kind = *kind;
                if !self.form.can_continue() {
                    self.form.continue_session = false;
                }
                // Existing-branch mode needs a branch selection before saving.
                self.autosave_pending = *kind != WorkspaceKind::Branch;
            }
            A::SelectBranch { base, branch } => {
                if *base {
                    self.form.base_branch = branch.clone();
                } else {
                    self.form.existing_branch = branch.clone();
                }
                self.open_picker = None;
                self.autosave_pending = true;
            }
            A::SetPinned(index) => {
                self.form.pinned = index
                    .and_then(|index| self.pinned_options.get(index))
                    .map(|(path, _)| path.clone())
                    // The project's own checkout is stored as "no path".
                    .filter(|path| Some(path) != self.form.project.as_ref());
                self.open_picker = None;
                self.autosave_pending = true;
            }
            A::SetPushBranch(on) => {
                self.form.push_branch = *on;
                self.autosave_pending = true;
            }
            A::SetContinueSession(on) => {
                self.form.continue_session = *on && self.form.can_continue();
                self.autosave_pending = true;
            }
            A::SetServiceWrites(on) => {
                self.form.service_writes = *on;
                self.autosave_pending = true;
            }
            A::SetWatch(service) => {
                self.form.watch_service = service.clone();
                self.autosave_pending = true;
            }
            A::SetWatchInterval(minutes) => {
                self.form.watch_minutes = *minutes;
                self.autosave_pending = true;
            }
            A::AddTagFromField => {
                let text = self.editor_text(&self.tag_editor.clone(), ctx);
                self.set_editor_text(&self.tag_editor.clone(), "", ctx);
                self.handle_action(&A::AddTag(text), ctx);
                return;
            }
            A::AddTag(tag) => {
                let mut tags = self.form.tags.clone();
                tags.push(tag.clone());
                let tags = normalize_tags(tags);
                if tags != self.form.tags {
                    self.form.tags = tags;
                    self.autosave_pending = true;
                }
            }
            A::RemoveTag(tag) => {
                self.form.tags.retain(|existing| existing != tag);
                self.autosave_pending = true;
            }
            A::ToggleTemplates => self.templates_open = !self.templates_open,
            A::Save => self.save(ctx),
            A::DismissMessage => {
                self.error = None;
                self.error_is_limit = false;
                self.notice = None;
            }
        }
        // In an existing automation, picker changes apply at once; text
        // fields apply with “Save changes”.
        if std::mem::take(&mut self.autosave_pending) && self.detail_id().is_some() {
            self.save(ctx);
        }
        ctx.notify();
    }
}
