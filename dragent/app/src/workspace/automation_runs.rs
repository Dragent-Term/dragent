//! Automations inside a workspace window: the Automations page, and the
//! workspaces runs produce. A started run's workspace goes into the sidebar
//! right away (filed into its automation's tag folder) with a tab attached
//! to the terminal its agent runs in; Dragent's own agent starts in a pane
//! here. Project-less session runs sit under "Sessions". The header marks
//! a workspace an automation started.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dragent_automations::{Run, RunTerminal, RunWorkspace, WorkspaceMode};
use dragent_workspace_model::WorkItemId;
use warpui::color::ColorU;
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element,
    Empty, EventHandler, Flex, MainAxisSize, OffsetPositioning, ParentElement,
    PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition, Text,
};
use warpui::geometry::vector::vec2f;
use warpui::{
    AppContext, ReadView, SingletonEntity, TypedActionView, UpdateModel, UpdateView, ViewContext,
    ViewHandle,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use super::host_tabs::HostReattach;
use super::pane::WorkspacePane;
use super::view::{PaneTab, WorkspaceView};
use super::workspace_tab::WorkspaceTab;
use super::worktree_status::WorktreeAgentStatus;
use super::{PromptAgent, WorkspaceAction};
use crate::agent::TabSessionKind;
use crate::terminal::TerminalView;
use crate::automations::{executor::DRAGENT_AGENT, AutomationEvent, AutomationRuntime};
use crate::icons::{render_icon, AppIcon};
use crate::terminal::manager::pty_writer::HostPtyBinding;
use crate::theme::{scaled, Theme};

/// `now`, `5m`, `3h`, `2d`, then the date.
fn compact_age(ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - ms).max(0) / 60_000;
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes}m"),
        60..=1439 => format!("{}h", minutes / 60),
        1440..=10079 => format!("{}d", minutes / 1440),
        _ => crate::automations::format_day(ms),
    }
}

/// How a run workspace's agent is doing, for the run history.
fn activity_label(status: &WorktreeAgentStatus) -> String {
    match status {
        WorktreeAgentStatus::Idle => "Agent idle".into(),
        WorktreeAgentStatus::Review => "Agent finished — ready for review".into(),
        WorktreeAgentStatus::Working { label } if !label.trim().is_empty() => {
            format!("Agent working: {label}")
        }
        WorktreeAgentStatus::Working { .. } => "Agent working".into(),
        WorktreeAgentStatus::Permission => "Agent waiting for your input".into(),
        WorktreeAgentStatus::Failed => "Agent stopped with an error".into(),
    }
}

/// Projects named in the "Create with AI" prompt, at most.
const PROMPT_PROJECTS: usize = 20;

/// The seed for "Create with AI": the agent interviews the user and
/// creates the automation with the command line, working in `project`
/// (the one the user picked; `None`: no project). It is told what it can
/// choose from, so it offers the user's agents rather than asking for paths.
fn create_with_agent_prompt(
    cli: &str,
    project: Option<&(PathBuf, String)>,
    projects: &[(PathBuf, String)],
    agents: &[&str],
    zone: &str,
) -> String {
    let others = projects
        .iter()
        .filter(|(root, _)| project.map(|(chosen, _)| chosen) != Some(root))
        .take(PROMPT_PROJECTS)
        .map(|(root, name)| format!("{name} ({})", root.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let target = match project {
        Some((root, name)) => format!(
            "I chose my project {name} ({path}) for it: pass --project '{path}' unless I ask for \
another project or none. You're in that project now, so look around it to write a better prompt \
if that helps, but don't change anything in it.{others}",
            path = root.display(),
            others = if others.is_empty() { String::new() } else { format!(" My other projects: {others}.") },
        ),
        None => format!(
            "I chose no project for it: pass --session unless I ask for one of my projects{}.",
            if others.is_empty() { String::new() } else { format!(" ({others})") },
        ),
    };
    let agents = agents.join(", ");
    format!(
        "Help me set up a Dragent automation (an agent that starts on a schedule). \
Use the Dragent command line for it: run `{cli} help` first to see the commands and options. \
Ask me what should happen each time it runs, then agree with me on how often it should run \
and which agent should run it. \
{target} \
Agents installed on this computer, for --agent: {agents} (dragent is Dragent's own agent). \
Schedules use this computer's time zone ({zone}) unless I ask for another. \
Pass --agent and either --project or --session explicitly. \
Nobody watches a run, so write the prompt so the agent can finish without asking anything; \
if `create` says the agent may wait for approval, tell me. \
Create it with `{cli} create`, start a first run with `{cli} run <id>`, \
and then we'll look at the result together."
    )
}

/// The folder of runs whose automation has no tags.
const DEFAULT_RUN_FOLDER: &str = "automation";

impl WorkspaceView {
    /// Projects the page can target: everything in the sidebar but the
    /// Sessions lane and remote projects (runs happen on this computer).
    fn automation_projects(&self) -> Vec<(PathBuf, String)> {
        let sessions = super::project_naming::automation_sessions_root();
        self.sidebar_project_roots()
            .into_iter()
            .filter(|root| root.is_dir() && !root.starts_with(&sessions))
            .filter(|root| crate::remote_fs::resolve(root).is_none())
            .map(|root| {
                let label = root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                (root, label)
            })
            .collect()
    }

    /// Hand the page the agents found on this computer (the prompt
    /// screen's detection), whenever detection settles.
    pub(crate) fn sync_automation_agents(&self, ctx: &mut ViewContext<Self>) {
        let installed = self.prompt_installed_agents.clone();
        let checking = self.prompt_agent_checking;
        let page = self.automations_view.clone();
        ctx.update_view(&page, |page, page_ctx| page.set_agents(installed, checking, page_ctx));
    }

    /// Sidebar folders already in use, offered as automation tags.
    fn automation_tag_suggestions(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .sidebar_project_roots()
            .iter()
            .flat_map(|root| self.workspace_trust.groups(root))
            .map(|group| group.to_lowercase())
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }

    pub(crate) fn open_automations(&mut self, ctx: &mut ViewContext<Self>) {
        self.close_settings(ctx);
        self.close_org_menus();
        self.automations_open = true;
        let projects = self.automation_projects();
        let installed = self.prompt_installed_agents.clone();
        let checking = self.prompt_agent_checking;
        let tags = self.automation_tag_suggestions();
        let theme = self.theme;
        let current = self.active_project_root().map(|root| self.project_root_of(&root));
        let page = self.automations_view.clone();
        ctx.update_view(&page, |page, page_ctx| {
            page.refresh(theme, projects, installed, checking, tags, page_ctx);
            page.set_current_project(current, page_ctx);
        });
        self.push_run_activity(ctx);
        ctx.focus(&page);
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// Open the page on one automation (from a `<scheme>://automations/<id>`
    /// link).
    pub(crate) fn open_automation_link(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        self.open_automations(ctx);
        let page = self.automations_view.clone();
        let id = id.to_string();
        ctx.update_view(&page, |page, page_ctx| page.open_link(&id, page_ctx));
    }

    pub(crate) fn close_automations(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.automations_open {
            return;
        }
        self.automations_open = false;
        let page = self.automations_view.clone();
        ctx.update_view(&page, |page, _| page.hide());
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
            tab.pane.focus(ctx);
        } else {
            ctx.focus_self();
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// React to the runtime: place started runs, start Dragent-agent runs.
    pub(crate) fn on_automation_event(&mut self, event: &AutomationEvent, ctx: &mut ViewContext<Self>) {
        match event {
            AutomationEvent::RunStarted(run) => self.place_automation_run(run.clone(), ctx),
            AutomationEvent::StartInWindow(run) => self.start_window_run(run.clone(), ctx),
            AutomationEvent::Alert => {
                self.show_automation_alerts(ctx);
                return;
            }
            AutomationEvent::Changed => {}
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// Desktop alerts, with their sound, for runs that finished, failed, or
    /// need someone; also for the tab this window is showing, since its
    /// person may be busy elsewhere.
    fn show_automation_alerts(&mut self, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        let alerts = ctx.update_model(&runtime, |runtime, _| runtime.take_alerts());
        for alert in alerts {
            crate::notifications::notify(
                ctx,
                crate::notifications::Kind::Automation,
                &alert.title,
                &alert.body,
                Some(alert.data()),
            );
        }
    }

    /// A clicked alert: show the run it's about.
    pub(crate) fn open_automation_run_by_id(&mut self, run_id: &str, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        match runtime.read(ctx, |runtime, _| runtime.run(run_id)) {
            Some(run) if run.can_open() => self.open_automation_run(run, ctx),
            Some(run) => self.open_automation_link(&run.automation_id, ctx),
            None => {}
        }
    }

    /// Runs that started (or wait for a window) while no window took them.
    pub(crate) fn adopt_pending_automation_runs(&mut self, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        let (unplaced, waiting) =
            runtime.read(ctx, |runtime, _| (runtime.unplaced_runs(), runtime.waiting_window_runs()));
        for run in unplaced {
            self.place_automation_run(run, ctx);
        }
        for run in waiting {
            self.start_window_run(run, ctx);
        }
    }

    pub(crate) fn workspace_for(&self, path: &Path) -> Option<usize> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.workspaces
            .iter()
            .find(|workspace| {
                workspace.root == path
                    || workspace.root.canonicalize().unwrap_or_else(|_| workspace.root.clone()) == canonical
            })
            .map(|workspace| workspace.id)
    }

    /// File a run's workspace into a folder under its project: the folder
    /// of its first tag that already exists there, else of its first tag, or
    /// "automation" when it has none. Every workspace a run makes is in one.
    pub(crate) fn file_into_tag_folder(&mut self, root: &Path, run: &Run) {
        let snapshot = &run.snapshot;
        // The run knows its project. Asking the sidebar instead answered
        // the new worktree itself (not worked out yet), so each run made a
        // folder of its own under a heading nobody sees.
        let project = if snapshot.is_session() {
            root.to_path_buf()
        } else {
            snapshot.project_root.clone()
        };
        self.file_into_folder(root, &project, &snapshot.tags);
    }

    /// File a workspace into a folder under `project`: the first of `tags`
    /// already there, else the first of `tags`, else "automation".
    pub(crate) fn file_into_folder(&mut self, root: &Path, project: &Path, tags: &[String]) {
        let tags: Vec<String> = tags
            .iter()
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect();
        let tags = if tags.is_empty() { vec![DEFAULT_RUN_FOLDER.to_string()] } else { tags };
        let groups = self.workspace_trust.groups(project);
        let existing = tags.iter().find_map(|tag| {
            groups
                .iter()
                .find(|group| group.eq_ignore_ascii_case(tag))
                .cloned()
        });
        let group = match existing {
            Some(group) => group,
            None => {
                self.workspace_trust.add_group(project, &tags[0]);
                tags[0].clone()
            }
        };
        self.workspace_trust.set_workspace_group(root, Some(group));
    }

    /// Whether a run made its workspace (rather than reusing one).
    fn run_created_workspace(run: &Run) -> bool {
        match &run.snapshot.workspace {
            WorkspaceMode::NewWorktree { .. } | WorkspaceMode::ExistingBranch { .. } => true,
            WorkspaceMode::Pinned { path } => run.snapshot.is_session() && path.is_none(),
        }
    }

    /// Put a started run's agent in the sidebar without leaving the current
    /// workspace: a background surface whose tab attaches to the run's
    /// terminal, in the run's workspace (added to the sidebar if needed).
    pub(crate) fn place_automation_run(&mut self, run: Run, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        if !ctx.update_model(&runtime, |runtime, _| runtime.claim_placement(&run.id)) {
            return;
        }
        let Some(path) = run.workspace_path.clone().filter(|path| path.is_dir()) else {
            return;
        };
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        ctx.update_model(&runtime, |runtime, ctx| runtime.record_run_workspace(&run, &canonical, ctx));
        let Some(terminal) = run.terminal.clone() else {
            return;
        };
        // A continued run's agent is already in the earlier run's tab.
        if run.agent_pane().is_some_and(|pane| self.tab_with_pane(pane, ctx).is_some()) {
            return;
        }
        let tab = self.run_tab(&run, &canonical, &terminal, ctx);
        match self.workspace_for(&canonical) {
            // A pinned workspace (or one already open): the run gets a
            // surface of its own in it.
            Some(workspace_id) => {
                self.add_parked_surface(workspace_id, tab);
            }
            None => {
                let title = run.branch.clone().unwrap_or_else(|| run.snapshot.name.clone());
                self.park_new_surface(canonical.clone(), title, tab);
                if Self::run_created_workspace(&run) {
                    self.file_into_tag_folder(&canonical, &run);
                }
            }
        }
        self.sync_left_panel(ctx);
        self.save_session(ctx);
    }

    /// The tab (in any surface) whose terminal is pane `pane`.
    pub(super) fn tab_with_pane(&self, pane: &str, ctx: &ViewContext<Self>) -> Option<usize> {
        let pane = uuid::Uuid::parse_str(pane).ok()?;
        self.tabs
            .iter()
            .chain(self.parked_workspace_tabs.iter().flat_map(|surface| surface.panes.iter()))
            .find(|tab| matches!(tab.pane, WorkspacePane::Terminal(_)) && tab.pane.pane_uuid(ctx) == pane)
            .map(|tab| tab.id)
    }

    /// A background tab attached to `run`'s agent terminal.
    fn run_tab(&mut self, run: &Run, workspace: &Path, terminal: &RunTerminal, ctx: &mut ViewContext<Self>) -> PaneTab {
        let project = if run.snapshot.is_session() {
            workspace.to_path_buf()
        } else {
            run.snapshot.project_root.clone()
        };
        let mut tab = self.run_terminal_tab(workspace, &project, terminal, false, ctx);
        tab.custom_label = Some(run.snapshot.name.clone());
        tab.pane.set_agent_id(ctx, Some(run.snapshot.agent.clone()));
        tab
    }

    /// One more surface, holding `tab`, in sidebar workspace `workspace_id`,
    /// without switching to it.
    pub(super) fn add_parked_surface(&mut self, workspace_id: usize, tab: PaneTab) {
        let surface_id = self.next_workspace_tab_id;
        self.next_workspace_tab_id += 1;
        self.workspace_tab_order.push(surface_id);
        let mut surface = WorkspaceTab::fresh(surface_id, String::new());
        surface.active_tab_id = tab.id;
        surface.active_pane_id = Some(tab.pane.id());
        surface.panes = vec![tab];
        self.parked_workspace_tabs.push(surface);
        if let Some(workspace) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            workspace.tab_ids.push(surface_id);
        }
    }

    /// A workspace with one surface holding `tab`, without switching to it.
    /// Returns the surface.
    pub(super) fn park_new_surface(&mut self, root: PathBuf, title: String, tab: PaneTab) -> usize {
        let surface_id = self.next_workspace_tab_id;
        self.next_workspace_tab_id += 1;
        self.workspace_tab_order.push(surface_id);
        let mut surface = WorkspaceTab::fresh(surface_id, String::new());
        surface.active_tab_id = tab.id;
        surface.active_pane_id = Some(tab.pane.id());
        surface.panes = vec![tab];
        self.parked_workspace_tabs.push(surface);
        self.closed_workspace_roots.retain(|closed| closed != &root);
        let id = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .max()
            .unwrap_or(0)
            + 1;
        self.workspaces.push(dragent_persistence::SerializedWorkspace {
            organisation_id: self.organisation_id.clone(),
            id,
            root,
            title,
            tab_ids: vec![surface_id],
            active_tab_id: surface_id,
        });
        surface_id
    }

    /// Start Dragent's own agent for a run that's waiting on a window: its
    /// workspace goes in the sidebar with a pane running the agent.
    /// The Dragent tab an earlier run of this automation opened in
    /// `workspace`, if it's still open: "Continue the last session" sends
    /// the next prompt there.
    fn previous_window_run_terminal(
        &self,
        run: &Run,
        workspace: &Path,
        ctx: &ViewContext<Self>,
    ) -> Option<ViewHandle<TerminalView>> {
        self.tabs
            .iter()
            .chain(self.parked_workspace_tabs.iter().flat_map(|surface| surface.panes.iter()))
            .filter(|tab| tab.custom_label.as_deref() == Some(run.snapshot.name.as_str()))
            .find_map(|tab| {
                let WorkspacePane::Terminal(terminal) = &tab.pane else {
                    return None;
                };
                let cwd = terminal.read(ctx, |terminal, ctx| {
                    terminal.git_repo_status.read(ctx, |status, _| status.cwd.clone())
                });
                let cwd = cwd.canonicalize().unwrap_or(cwd);
                cwd.starts_with(workspace).then(|| terminal.clone())
            })
    }

    pub(crate) fn start_window_run(&mut self, run: Run, ctx: &mut ViewContext<Self>) {
        let runtime = AutomationRuntime::handle(ctx);
        let Some(run) = ctx.update_model(&runtime, |runtime, _| runtime.claim_window_run(&run.id)) else {
            return;
        };
        // `Ok((true, _))`: the prompt went to the conversation an earlier
        // run left. The pane is the one the agent works in.
        let result = (|| -> Result<(bool, uuid::Uuid), String> {
            let path = run
                .workspace_path
                .clone()
                .filter(|path| path.is_dir())
                .ok_or_else(|| "The run's workspace is gone.".to_string())?;
            let canonical = path.canonicalize().unwrap_or(path);
            if run.snapshot.continue_session {
                if let Some(terminal) = self.previous_window_run_terminal(&run, &canonical, ctx) {
                    // Queued behind the agent's current turn if it's still busy.
                    let prompt = run.snapshot.prompt.clone();
                    let writes = run.snapshot.service_writes;
                    ctx.update_view(&terminal, |terminal, terminal_ctx| {
                        terminal.submit_automation_query(prompt, writes, terminal_ctx)
                    });
                    ctx.update_model(&runtime, |runtime, ctx| {
                        runtime.record_run_workspace(&run, &canonical, ctx)
                    });
                    return Ok((true, terminal.read(ctx, |terminal, _| terminal.pane_uuid())));
                }
            }
            let project = (!run.snapshot.is_session()).then(|| run.snapshot.project_root.clone());
            let mut tab = self.create_tab_with_cwd_focus(
                ctx,
                Some(run.snapshot.name.clone()),
                None,
                canonical.clone(),
                project.or_else(|| Some(canonical.clone())),
                false,
            );
            tab.custom_label = Some(run.snapshot.name.clone());
            let WorkspacePane::Terminal(terminal) = &tab.pane else {
                return Err("Couldn't open a terminal for the agent.".into());
            };
            let terminal = terminal.clone();
            let pane = terminal.read(ctx, |terminal, _| terminal.pane_uuid());
            let prompt = run.snapshot.prompt.clone();
            let writes = run.snapshot.service_writes;
            ctx.update_view(&terminal, |terminal, terminal_ctx| {
                terminal.submit_automation_query(prompt, writes, terminal_ctx)
            });
            match self.workspace_for(&canonical) {
                // A pinned workspace already in the sidebar: add a surface.
                Some(workspace_id) => self.add_parked_surface(workspace_id, tab),
                None => {
                    let title = run.branch.clone().unwrap_or_else(|| run.snapshot.name.clone());
                    self.park_new_surface(canonical.clone(), title, tab);
                    if Self::run_created_workspace(&run) {
                        self.file_into_tag_folder(&canonical, &run);
                    }
                }
            }
            ctx.update_model(&runtime, |runtime, ctx| runtime.record_run_workspace(&run, &canonical, ctx));
            Ok((false, pane))
        })();
        ctx.update_model(&runtime, |runtime, ctx| runtime.finish_window_run(&run.id, result, ctx));
        self.sync_left_panel(ctx);
        self.save_session(ctx);
    }

    /// Show a run's agent: its own tab, else its workspace.
    pub(crate) fn open_automation_run(&mut self, run: Run, ctx: &mut ViewContext<Self>) {
        let Some(path) = run.workspace_path.clone().filter(|path| path.is_dir()) else {
            return;
        };
        self.close_automations(ctx);
        // A started run is usually in the sidebar already.
        let runtime = AutomationRuntime::handle(ctx);
        // Seen now, however it was opened (the inbox, an alert, its history).
        ctx.update_model(&runtime, |runtime, ctx| runtime.mark_read(&run.id, ctx));
        if run.status == dragent_automations::RunStatus::Started {
            self.place_automation_run(run.clone(), ctx);
        }
        if let Some(tab_id) = run.agent_pane().and_then(|pane| self.tab_with_pane(pane, ctx)) {
            self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
            return;
        }
        if let Some(workspace_id) = self.workspace_for(&path) {
            // No tab shows the agent (it was closed, or the run is from
            // before runs had tabs of their own): attach one.
            if let Some(terminal) = run.terminal.clone() {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                let tab = self.run_tab(&run, &canonical, &terminal, ctx);
                let tab_id = tab.id;
                self.add_parked_surface(workspace_id, tab);
                self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
                return;
            }
            self.handle_action(&WorkspaceAction::SelectProjectWorkspace { workspace_id }, ctx);
            return;
        }
        if self.begin_fresh_surface(ctx).is_none() {
            return;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        ctx.update_model(&runtime, |runtime, ctx| runtime.record_run_workspace(&run, &canonical, ctx));
        let title = run
            .branch
            .clone()
            .unwrap_or_else(|| run.snapshot.name.clone());
        self.assign_surface_to_workspace(path.clone(), title.clone());
        if Self::run_created_workspace(&run) {
            self.file_into_tag_folder(&canonical, &run);
        }
        let project = if run.snapshot.is_session() {
            canonical.clone()
        } else {
            run.snapshot.project_root.clone()
        };
        let mut tab = match &run.terminal {
            Some(terminal) => self.run_terminal_tab(&path, &project, terminal, true, ctx),
            None => self.create_tab_with_cwd(
                ctx,
                Some(title.clone()),
                None,
                path.clone(),
                Some(project.clone()),
            ),
        };
        tab.custom_label = Some(run.snapshot.name.clone());
        tab.pane.set_agent_id(ctx, Some(run.snapshot.agent.clone()));
        self.show_new_tab(tab, ctx);
    }

    /// Make `tab` the live surface's active tab (after `begin_fresh_surface`).
    fn show_new_tab(&mut self, tab: PaneTab, ctx: &mut ViewContext<Self>) {
        let tab_id = tab.id;
        self.tabs.push(tab);
        self.active_tab_id = tab_id;
        self.split_tree = None;
        self.document_center_open = false;
        self.document_terminal_visible = false;
        self.project_start_visible = false;
        self.right_panel_visible = true;
        self.right_panel_user_closed = false;
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// "Create with AI": a session workspace with an agent that interviews
    /// the user and creates the automation with `dragent automations`.
    pub(crate) fn create_automation_with_agent(
        &mut self,
        agent: PromptAgent,
        project: Option<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        let page = self.automations_view.clone();
        let fail = |message: String, ctx: &mut ViewContext<Self>| {
            log::error!("[automations] create with AI: {message}");
            ctx.update_view(&page, |page, page_ctx| {
                page.show_error(format!("Couldn't start Create with AI: {message}"), page_ctx)
            });
        };
        let cli = match crate::automations::cli::install_shim() {
            Ok(path) => path,
            Err(error) => return fail(error, ctx),
        };
        let cli = format!("'{}' automations", cli.display());
        if agent != PromptAgent::Dragent && super::prompt_agent_interactive_command(agent).is_none() {
            return fail(format!("{} isn't installed", agent.label()), ctx);
        }
        let installed: Vec<&str> = PromptAgent::ALL
            .into_iter()
            .filter(|agent| *agent != PromptAgent::Dragent && self.prompt_installed_agents.contains(agent))
            .map(PromptAgent::command_key)
            .chain(std::iter::once(PromptAgent::Dragent.command_key()))
            .collect();
        let projects = self.automation_projects();
        let chosen = project.and_then(|project| projects.iter().find(|(root, _)| *root == project));
        let prompt = create_with_agent_prompt(
            &cli,
            chosen,
            &projects,
            &installed,
            &dragent_automations::local_time_zone(),
        );
        // The conversation happens in the chosen project, else in a session
        // folder of its own.
        let folder = match chosen {
            Some((root, _)) => root.clone(),
            None => {
                let runtime = AutomationRuntime::handle(ctx);
                let folder = runtime
                    .read(ctx, |runtime, _| runtime.sessions_root())
                    .join(format!("create-automation-{}", &uuid::Uuid::new_v4().to_string()[..8]));
                if let Err(error) = std::fs::create_dir_all(&folder) {
                    return fail(format!("couldn't create {}: {error}", folder.display()), ctx);
                }
                folder.canonicalize().unwrap_or(folder)
            }
        };
        self.close_automations(ctx);
        if let Err(error) = self.open_agent_surface(folder, "New automation".into(), Some(agent), &prompt, None, ctx) {
            fail(error, ctx);
        }
    }

    /// How `agent` starts (`None`: Dragent's own agent, which takes the
    /// prompt as a query), with `model` when one is asked for.
    pub(crate) fn agent_launch(
        agent: PromptAgent,
        model: Option<&str>,
    ) -> Result<Option<crate::setup::settings::AgentCommandConfig>, String> {
        if agent == PromptAgent::Dragent {
            return match model {
                Some(_) => Err("Dragent's own agent uses the model chosen in Settings".into()),
                None => Ok(None),
            };
        }
        let mut config = super::prompt_agent_interactive_command(agent)
            .ok_or_else(|| format!("{} isn't installed", agent.label()))?;
        if let Some(model) = model {
            config.command = crate::cli::model::apply(&config.command, agent, model)?;
            config.prompt_command = crate::cli::model::apply(&config.prompt_command, agent, model)?;
        }
        Ok(Some(config))
    }

    /// A tab at `root` running `agent` on `prompt` (a plain terminal with no
    /// agent), not yet placed in a surface.
    pub(super) fn agent_tab(
        &mut self,
        root: PathBuf,
        title: String,
        agent: Option<PromptAgent>,
        prompt: &str,
        model: Option<&str>,
        focus: bool,
        account: &dragent_host::agent_accounts::Selection,
        ctx: &mut ViewContext<Self>,
    ) -> Result<PaneTab, String> {
        let launch = match agent {
            Some(agent) => Some((agent, Self::agent_launch(agent, model)?)),
            None => None,
        };
        let binding = if let Some(agent) = agent {
            let registry = dragent_host::agent_accounts::Registry::load()?;
            Some(registry.resolve(agent.command_key(), dragent_host::agent_accounts::project_for(&root, &registry).as_deref(), account)?)
        } else { None };
        // A shell on another machine can't read a prompt file written here.
        let local = self.machine_agents(Some(&root), ctx).is_none();
        let tab = self.create_tab_with_cwd_focus(ctx, Some(title), None, root.clone(), Some(root), focus);
        if let Some(binding) = binding { dragent_host::agent_sessions::stage_account(tab.pane.pane_uuid(ctx), &binding)?; }
        if let (WorkspacePane::Terminal(terminal), Some((agent, config))) = (&tab.pane, launch) {
            let terminal = terminal.clone();
            tab.pane.set_agent_id(ctx, Some(agent.command_key().to_string()));
            let prompt = prompt.to_string();
            match config {
                Some(config) => {
                    ctx.update_view(&terminal, |terminal, terminal_ctx| {
                        let shell = terminal.launch_shell.as_deref().unwrap_or("/bin/sh").to_string();
                        let command = if prompt.trim().is_empty() {
                            Ok(config.command.clone())
                        } else {
                            config.build_prompt_command(&prompt, &shell, local)
                        };
                        match command {
                            Ok(command) => terminal.submit_shell_command(&command, terminal_ctx),
                            Err(error) => log::error!("[workspace] agent command: {error}"),
                        }
                    });
                }
                None if !prompt.trim().is_empty() => ctx.update_view(&terminal, |terminal, terminal_ctx| {
                    terminal.submit_agent_query(prompt, terminal_ctx)
                }),
                None => {}
            }
        }
        Ok(tab)
    }

    /// A fresh workspace surface at `root` titled `title`, in front, its
    /// first tab running `agent` on `prompt`. Returns that tab's session id.
    pub(crate) fn open_agent_surface(
        &mut self,
        root: PathBuf,
        title: String,
        agent: Option<PromptAgent>,
        prompt: &str,
        model: Option<&str>,
        ctx: &mut ViewContext<Self>,
    ) -> Result<uuid::Uuid, String> {
        // Fail before anything changes on screen.
        if let Some(agent) = agent {
            Self::agent_launch(agent, model)?;
        }
        if self.begin_fresh_surface(ctx).is_none() {
            return Err("Couldn't open a new workspace".into());
        }
        self.assign_surface_to_workspace(root.clone(), title.clone());
        let tab = self.agent_tab(root, title, agent, prompt, model, true, &Default::default(), ctx)?;
        let pane = tab.pane.pane_uuid(ctx);
        self.show_new_tab(tab, ctx);
        Ok(pane)
    }

    /// One more tab-surface in sidebar workspace `workspace_id` (at
    /// `root`), running `agent`, without switching to it.
    pub(crate) fn add_agent_surface(
        &mut self,
        workspace_id: usize,
        root: PathBuf,
        agent: PromptAgent,
        prompt: &str,
        model: Option<&str>,
        account: &dragent_host::agent_accounts::Selection,
        ctx: &mut ViewContext<Self>,
    ) -> Result<uuid::Uuid, String> {
        let tab = self.agent_tab(root, agent.label().to_string(), Some(agent), prompt, model, false, account, ctx)?;
        let pane = tab.pane.pane_uuid(ctx);
        self.add_parked_surface(workspace_id, tab);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        Ok(pane)
    }

    /// A tab bound to an existing host terminal, through the same attach
    /// path restored tabs use.
    fn run_terminal_tab(
        &mut self,
        cwd: &Path,
        project_root: &Path,
        terminal: &RunTerminal,
        focus: bool,
        ctx: &mut ViewContext<Self>,
    ) -> PaneTab {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let pane_uuid = terminal
            .pane_uuid
            .as_deref()
            .and_then(|uuid| uuid::Uuid::parse_str(uuid).ok())
            .unwrap_or_else(uuid::Uuid::new_v4);
        let shell = dragent_pty::resolve_shell(
            crate::console::editor_settings::EditorSettings::global()
                .terminal
                .default_shell
                .clone()
                .or_else(|| std::env::var("SHELL").ok()),
        );
        let slot = Arc::new(HostPtyBinding::new());
        let pane = WorkspacePane::spawn_host_pending(ctx, cwd.to_path_buf(), slot.clone());
        let WorkspacePane::Terminal(handle) = &pane else {
            unreachable!("host panes are terminals");
        };
        let handle = handle.clone();
        ctx.update_view(&handle, |view, _| {
            view.launch_shell = Some(shell);
            view.pane_uuid = pane_uuid;
            view.set_tab_session_kind(TabSessionKind::Agent);
        });
        super::tabs_panes::subscribe_workspace_pane(
            tab_id,
            &pane,
            self.persistence_writer.sender(),
            ctx,
        );
        if focus {
            pane.focus(ctx);
            self.active_pane_id = Some(pane.id());
        }
        let surface_id = self.register_surface(&pane, ctx);
        self.attach_restored_host_tabs(
            ctx,
            vec![(
                tab_id,
                HostReattach {
                    pane: handle,
                    slot,
                    binding: dragent_persistence::SerializedHostTerminalBinding {
                        remote_host: None,
                        pane_uuid,
                        workspace_id: terminal.workspace_id.clone(),
                        home_id: terminal.home_id.clone(),
                        session_id: terminal.session_id.clone(),
                        agent: true,
                    },
                    cwd: cwd.to_path_buf(),
                    project_root: Some(project_root.to_path_buf()),
                    expect_agent: true,
                    restart_after_update: false,
                },
            )],
        );
        PaneTab {
            id: tab_id,
            surface_id,
            title: "Agent".to_string(),
            custom_label: None,
            color: None,
            work_item_id: Some(WorkItemId::new()),
            project_root: Some(project_root.to_path_buf()),
            hidden_from_sidebar: false,
            pane,
        }
    }

    /// Dragent's own agent in `pane` reached `state`: the automation run it
    /// works for, if any, records it.
    pub(crate) fn note_run_agent(
        &self,
        pane: uuid::Uuid,
        state: dragent_automations::AgentState,
        reply: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let runtime = AutomationRuntime::handle(ctx);
        ctx.update_model(&runtime, |runtime, ctx| {
            runtime.note_pane_progress(&pane.to_string(), state, reply, ctx)
        });
    }

    // ── Sidebar and page sync ────────────────────────────────────────────────

    /// Unseen failures for the sidebar badge.
    pub(crate) fn automation_failures(&self, app: &AppContext) -> usize {
        AutomationRuntime::handle(app).read(app, |runtime, _| runtime.unseen_failures())
    }

    /// Hand the page what each run workspace's agent is doing.
    pub(crate) fn push_run_activity(&self, ctx: &mut ViewContext<Self>) {
        if !self.automations_open {
            return;
        }
        let activity: HashMap<PathBuf, String> = self.left_panel_view.read(ctx, |panel, _| {
            panel
                .tabs
                .iter()
                .filter(|row| !row.agents.is_empty())
                .map(|row| (PathBuf::from(&row.cwd), activity_label(&row.agent_status)))
                .collect()
        });
        let page = self.automations_view.clone();
        ctx.update_view(&page, |page, page_ctx| page.set_run_activity(activity, page_ctx));
    }

    // ── Notifications ────────────────────────────────────────────────────────

    /// Unread automation results for the bell's list, newest first.
    pub(crate) fn automation_notices(
        &self,
        app: &AppContext,
    ) -> Vec<super::header_menus::AutomationNotice> {
        let now = chrono::Utc::now().timestamp_millis();
        AutomationRuntime::handle(app)
            .read(app, |runtime, _| runtime.inbox(false))
            .into_iter()
            .filter(Run::is_unread)
            .map(|run| automation_notice(&run, now))
            .collect()
    }

    pub(crate) fn handle_automation_run_menu(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::OpenAutomationNotice {
                run_id,
                automation_id,
            } => {
                self.close_attention_bell();
                let runtime = AutomationRuntime::handle(ctx);
                ctx.update_model(&runtime, |runtime, ctx| runtime.mark_read(run_id, ctx));
                self.handle_action(&WorkspaceAction::OpenAutomations, ctx);
                // Deleted since it ran: its result is still in the inbox.
                if runtime.read(ctx, |runtime, _| runtime.automation(automation_id).is_some()) {
                    let page = self.automations_view.clone();
                    ctx.update_view(&page, |page, page_ctx| page.open_link(automation_id, page_ctx));
                } else {
                    self.show_automation_inbox(ctx);
                }
            }
            WorkspaceAction::OpenAutomationInbox => {
                self.close_attention_bell();
                self.handle_action(&WorkspaceAction::OpenAutomations, ctx);
                self.show_automation_inbox(ctx);
            }
            WorkspaceAction::OpenAutomationLink(id) => {
                let id = id.clone();
                self.open_automation_link(&id, ctx);
            }
            WorkspaceAction::OpenAutomationRun(run_id) => {
                let run_id = run_id.clone();
                self.open_automation_run_by_id(&run_id, ctx);
            }
            _ => return,
        }
        ctx.notify();
    }

    fn close_attention_bell(&mut self) {
        self.attention_bell_open = false;
        self.attention_popover_item = None;
    }

    fn show_automation_inbox(&mut self, ctx: &mut ViewContext<Self>) {
        let page = self.automations_view.clone();
        ctx.update_view(&page, |page, page_ctx| page.show_inbox(page_ctx));
    }
}

/// How a run ended, in the bell's list.
fn run_outcome(
    status: dragent_automations::RunStatus,
    state: Option<dragent_automations::AgentState>,
) -> &'static str {
    use dragent_automations::{AgentState, RunStatus};
    match (status, state) {
        (RunStatus::Failed, _) | (_, Some(AgentState::Failed)) => "Failed",
        (_, Some(AgentState::NeedsInput)) => "Waiting for you",
        (_, Some(AgentState::TimedOut)) => "Stopped waiting for you",
        _ => "Finished",
    }
}

/// `run` as a row in the bell's list.
fn automation_notice(run: &Run, now_ms: i64) -> super::header_menus::AutomationNotice {
    let outcome = run_outcome(run.status, run.progress.as_ref().map(|progress| progress.state));
    super::header_menus::AutomationNotice {
        run_id: run.id.clone(),
        automation_id: run.automation_id.clone(),
        name: run.snapshot.name.clone(),
        detail: match run.result_at_ms() {
            Some(at) => format!("{outcome} · {}", compact_age(at, now_ms)),
            None => outcome.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_says_how_the_run_ended() {
        use dragent_automations::{AgentState, RunStatus};
        assert_eq!(run_outcome(RunStatus::Failed, None), "Failed");
        assert_eq!(run_outcome(RunStatus::Started, Some(AgentState::Failed)), "Failed");
        assert_eq!(run_outcome(RunStatus::Started, Some(AgentState::Done)), "Finished");
        assert_eq!(run_outcome(RunStatus::Started, Some(AgentState::Ended)), "Finished");
        assert_eq!(
            run_outcome(RunStatus::Started, Some(AgentState::NeedsInput)),
            "Waiting for you"
        );
        assert_eq!(
            run_outcome(RunStatus::Started, Some(AgentState::TimedOut)),
            "Stopped waiting for you"
        );
    }

    #[test]
    fn ages_read_compactly() {
        let now = 1_000_000_000_000;
        assert_eq!(compact_age(now - 20_000, now), "now");
        assert_eq!(compact_age(now - 5 * 60_000, now), "5m");
        assert_eq!(compact_age(now - 3 * 3_600_000, now), "3h");
        assert_eq!(compact_age(now - 2 * 86_400_000, now), "2d");
        // In the future (clock skew) reads as now, not negative.
        assert_eq!(compact_age(now + 60_000, now), "now");
    }

    #[test]
    fn the_create_prompt_names_the_command_and_the_choices() {
        let projects = vec![
            (PathBuf::from("/code/app"), "app".to_string()),
            (PathBuf::from("/code/site"), "site".to_string()),
        ];
        let prompt = create_with_agent_prompt(
            "'/x/dragent' automations",
            Some(&projects[0]),
            &projects,
            &["claude", "dragent"],
            "Africa/Lagos",
        );
        assert!(prompt.contains("'/x/dragent' automations help"));
        assert!(prompt.contains("'/x/dragent' automations create"));
        // The project the user picked, and the others in case they change their mind.
        assert!(prompt.contains("I chose my project app (/code/app)"));
        assert!(prompt.contains("--project '/code/app'"));
        assert!(prompt.contains("My other projects: site (/code/site)."));
        assert!(prompt.contains("for --agent: claude, dragent"));
        assert!(prompt.contains("(Africa/Lagos)"));
        // No newlines: the prompt travels as one shell argument.
        assert!(!prompt.contains('\n'));

        let prompt = create_with_agent_prompt("dragent automations", None, &projects, &["dragent"], "UTC");
        assert!(prompt.contains("I chose no project for it: pass --session"));
        assert!(prompt.contains("app (/code/app), site (/code/site)"));
        let prompt = create_with_agent_prompt("dragent automations", None, &[], &["dragent"], "UTC");
        assert!(prompt.contains("pass --session unless I ask for one of my projects."));
    }
}
