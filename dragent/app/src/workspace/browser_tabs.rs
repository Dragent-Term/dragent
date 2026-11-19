//! Browser panes in the workspace. They open like terminals — as a new
//! workspace tab, a sidebar tab, or a split beside the current pane — and stay
//! scoped to the organisation and project they were opened in.

use std::path::{Path, PathBuf};

use dragent_workspace_model::{BrowserOwner, BrowserPaneSession, BrowserUrl, Project};
use uuid::Uuid;
use warpui::fonts::FamilyId;
use warpui::{TypedActionView, UpdateView, ViewContext, ViewHandle};

use super::pane::WorkspacePane;
use super::pane_layout::{PaneLayout, SplitDirection, SplitSide};
use super::project_naming::default_pane_cwd;
use super::view::{PaneTab, WorkspaceView};
use super::WorkspaceAction;
use crate::browser::agent::{AgentCommand, AgentReply};
use crate::browser::{BrowserEvent, BrowserView};
use crate::cli::inbox::CliInbox;
use crate::cli::requests::Response;
use crate::theme::Theme;

impl WorkspaceView {
    /// Organisation, project, and workspace a new browser belongs to.
    fn browser_owner(&self, project_root: Option<&Path>) -> BrowserOwner {
        browser_owner(&self.organisation_id, project_root)
    }

    pub(crate) fn spawn_browser_pane(
        session: BrowserPaneSession,
        project_root: Option<PathBuf>,
        theme: Theme,
        font: FamilyId,
        ctx: &mut ViewContext<Self>,
    ) -> WorkspacePane {
        let browser: ViewHandle<BrowserView> = ctx.add_typed_action_view(move |browser_ctx| {
            BrowserView::new(session, project_root, theme, font, browser_ctx)
        });
        WorkspacePane::Browser(browser)
    }

    /// A browser pane-tab in the active project. Blank browsers open on the
    /// start page with the address bar focused.
    pub(crate) fn create_browser_tab(
        &mut self,
        url: Option<BrowserUrl>,
        project_root: Option<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) -> PaneTab {
        self.create_browser_tab_focus(url, project_root, true, ctx)
    }

    /// [`Self::create_browser_tab`], leaving focus where it is when `focus`
    /// is false (a tab opened in the background).
    pub(crate) fn create_browser_tab_focus(
        &mut self,
        url: Option<BrowserUrl>,
        project_root: Option<PathBuf>,
        focus: bool,
        ctx: &mut ViewContext<Self>,
    ) -> PaneTab {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let url = url.unwrap_or_else(|| BrowserUrl::parse("about:blank").expect("static URL"));
        let session = BrowserPaneSession::new(self.browser_owner(project_root.as_deref()), url);
        let pane = Self::spawn_browser_pane(
            session,
            project_root.clone(),
            self.workspace_theme(),
            self.ui_font,
            ctx,
        );
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
        PaneTab {
            id: tab_id,
            surface_id,
            title: "Browser".to_string(),
            custom_label: None,
            color: None,
            // Like Settings, a browser is not a unit of agent work.
            work_item_id: None,
            project_root,
            hidden_from_sidebar: false,
            pane,
        }
    }

    /// Header `+` → Browser: a new workspace tab holding one browser.
    pub(crate) fn open_browser_workspace_tab(&mut self, ctx: &mut ViewContext<Self>) {
        self.new_tab_picker_open = false;
        if self.closing_worktree_tab.is_some() {
            return;
        }
        let project_root = self.active_project_root();
        if self.begin_fresh_surface(ctx).is_none() {
            return;
        }
        let new_tab = self.create_browser_tab(None, project_root, ctx);
        self.show_new_browser_tab(new_tab, ctx);
    }

    /// Open a link the way the app opens every link: a web page in
    /// Dragent's browser beside where it was clicked (its bar can still hand
    /// the page to the system browser); anything else (`mailto:`, other
    /// apps' links), and every link while Settings → General → Open links
    /// in Dragent is off, in the system's handler. Sign-in pages that refuse
    /// embedded browsers go to the system browser too.
    pub(crate) fn open_link(&mut self, url: &str, ctx: &mut ViewContext<Self>) {
        let web = url.starts_with("http://") || url.starts_with("https://");
        let in_app = (web && !refuses_embedded_browsers(url)
            && crate::settings::git_consumers::open_links_in_app())
            .then(|| BrowserUrl::parse(url).ok())
            .flatten();
        match in_app {
            Some(url) => self.open_link_beside(url, ctx),
            None => ctx.open_url(url),
        }
    }

    /// A web page next to the pane it was clicked in: in the browser already
    /// shown beside it, else in a new browser split to its right. Dragent
    /// doesn't move the keyboard (a page that focuses itself as it loads
    /// takes it, as a click on it would). A full page (Settings, Code reviews…)
    /// gives way to the workspace first; an open document, whose layout
    /// can't be split, gets a browser tab of its own.
    fn open_link_beside(&mut self, url: BrowserUrl, ctx: &mut ViewContext<Self>) {
        self.close_settings(ctx);
        #[cfg(not(target_family = "wasm"))]
        {
            self.close_automations(ctx);
            self.close_teams(ctx);
            self.close_tasks(ctx);
            self.close_pull_requests(ctx);
        }
        let origin = self.active_tab_id;
        if self.document_center_open || !self.tabs.iter().any(|tab| tab.id == origin) {
            self.open_browser_pane_tab(Some(url), ctx);
            return;
        }
        let shown = |id: usize| {
            id == origin || self.split_tree.as_ref().is_some_and(|layout| layout.contains(id) && layout.contains(origin))
        };
        let beside = self.tabs.iter().find_map(|tab| match &tab.pane {
            WorkspacePane::Browser(browser) if shown(tab.id) => Some(browser.clone()),
            _ => None,
        });
        if let Some(browser) = beside {
            browser.update(ctx, |browser, browser_ctx| browser.navigate(url, browser_ctx));
            return;
        }
        self.split_browser(SplitSide::Right, Some(url), false, ctx);
    }

    /// Sidebar new-tab menu → Browser, or a page asking for a new window:
    /// a browser pane-tab in the current workspace tab.
    pub(crate) fn open_browser_pane_tab(
        &mut self,
        url: Option<BrowserUrl>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.new_tab_picker_open = false;
        self.document_center_open = false;
        self.document_terminal_visible = false;
        let project_root = self.active_project_root();
        let new_tab = self.create_browser_tab(url, project_root, ctx);
        self.split_tree = None;
        self.show_new_browser_tab(new_tab, ctx);
    }

    fn show_new_browser_tab(&mut self, new_tab: PaneTab, ctx: &mut ViewContext<Self>) {
        self.active_tab_id = new_tab.id;
        self.tabs.push(new_tab);
        self.project_start_visible = false;
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_window_title(ctx);
        ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
            lp.sidebar_menu_tab_id = None;
            lp_ctx.notify();
        });
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// Header `+` → Browser to the side: split the current pane and open a
    /// browser on the right, so an agent and its preview sit side by side.
    pub(crate) fn split_with_browser(&mut self, ctx: &mut ViewContext<Self>) {
        self.split_with_browser_on(SplitSide::Right, ctx);
    }

    /// Split the active pane with a browser on `side` of it.
    pub(crate) fn split_with_browser_on(&mut self, side: SplitSide, ctx: &mut ViewContext<Self>) {
        self.split_browser(side, None, true, ctx);
    }

    /// Split the active pane with a browser on `side` of it, open at `url`;
    /// with no active pane, a browser tab instead. `focus` gives the new
    /// browser the keyboard; without it the active pane keeps it and nothing
    /// has to be handed back (handing it back raced the page's own focus and
    /// froze the app). Returns the new pane's id when it is a split.
    pub(crate) fn split_browser(
        &mut self,
        side: SplitSide,
        url: Option<BrowserUrl>,
        focus: bool,
        ctx: &mut ViewContext<Self>,
    ) -> Option<usize> {
        self.new_tab_picker_open = false;
        let target = self.active_tab_id;
        let Some(project_root) = self
            .tabs
            .iter()
            .find(|tab| tab.id == target)
            .map(|tab| tab.project_root.clone())
        else {
            self.open_browser_pane_tab(url, ctx);
            return None;
        };
        let project_root = project_root.or_else(|| self.active_project_root());
        let mut new_tab = self.create_browser_tab(url, project_root, ctx);
        new_tab.hidden_from_sidebar = true;
        let new_tab_id = new_tab.id;
        self.tabs.push(new_tab);
        match &mut self.split_tree {
            Some(layout) if layout.contains(target) => {
                layout.split_pane(target, new_tab_id, side);
                if focus {
                    layout.focus(new_tab_id);
                }
            }
            _ => {
                let mut layout = PaneLayout::new(target);
                layout.split_pane(target, new_tab_id, side);
                if focus {
                    layout.focus(new_tab_id);
                }
                self.split_tree = Some(layout);
            }
        }
        self.prune_split_handles();
        if focus {
            self.active_tab_id = new_tab_id;
            self.update_active_pane(ctx);
        }
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
        Some(new_tab_id)
    }
}

impl WorkspaceView {
    pub(crate) fn navigate_active_browser(&mut self, url: BrowserUrl, ctx: &mut ViewContext<Self>) {
        let browser = self.tabs.iter().find_map(|tab| match &tab.pane {
            WorkspacePane::Browser(browser) if tab.id == self.active_tab_id => {
                Some(browser.clone())
            }
            _ => None,
        });
        if let Some(browser) = browser {
            browser.update(ctx, |browser, browser_ctx| {
                browser.navigate(url, browser_ctx)
            });
        }
    }

    pub(crate) fn dispatch_active_browser_action(
        &mut self,
        action: crate::browser::BrowserAction,
        ctx: &mut ViewContext<Self>,
    ) {
        // The active pane's browser, else one shown with it (an agent that
        // was just started beside the page takes focus).
        let browser = self
            .tabs
            .iter()
            .filter(|tab| tab.id == self.active_tab_id)
            .chain(&self.tabs)
            .find_map(|tab| match &tab.pane {
                WorkspacePane::Browser(browser) => Some(browser.clone()),
                _ => None,
            });
        if let Some(browser) = browser {
            browser.update(ctx, |browser, browser_ctx| {
                browser.handle_action(&action, browser_ctx)
            });
        }
    }

    /// Every browser pane, live and parked, for the E2E automation bridge.
    pub(crate) fn browser_automation_snapshot(&self, ctx: &ViewContext<Self>) -> serde_json::Value {
        let parked = self
            .parked_workspace_tabs
            .iter()
            .flat_map(|surface| surface.panes.iter().map(|tab| (tab, false)));
        let live = self.tabs.iter().map(|tab| (tab, true));
        serde_json::Value::Array(
            live.chain(parked)
                .filter_map(|(tab, live)| match &tab.pane {
                    WorkspacePane::Browser(browser) => {
                        let mut state = browser.read(ctx, |browser, _| browser.automation_state(ctx, ctx.window_id()));
                        state["tab_id"] = tab.id.into();
                        state["active"] = (live && tab.id == self.active_tab_id).into();
                        state["beside"] = (live
                            && self.split_tree.as_ref().is_some_and(|layout| {
                                layout.contains(tab.id) && layout.contains(self.active_tab_id)
                            }))
                        .into();
                        Some(state)
                    }
                    _ => None,
                })
                .collect(),
        )
    }
}

impl WorkspaceView {
    /// The terminal whose git status drives the sidebar while `tab` is
    /// shown: the tab itself when it is a terminal. A browser has no
    /// repository of its own, so it lends a project terminal's — one of the
    /// same project shown beside it first — and the Source Control panel
    /// (commit, push) keeps working while the page has focus.
    pub(crate) fn git_terminal_for(
        &self,
        tab: &PaneTab,
    ) -> Option<(usize, ViewHandle<crate::terminal::TerminalView>)> {
        match &tab.pane {
            WorkspacePane::Terminal(handle) => Some((tab.id, handle.clone())),
            WorkspacePane::Settings(_) => None,
            WorkspacePane::Browser(_) => {
                let terminals = self
                    .tabs
                    .iter()
                    .filter_map(|other| match &other.pane {
                        WorkspacePane::Terminal(handle) => Some((other, handle)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let candidates = terminals
                    .iter()
                    .map(|(other, _)| GitCandidate {
                        id: other.id,
                        project_root: other.project_root.as_deref(),
                        shown_beside: self.split_tree.as_ref().is_some_and(|layout| {
                            layout.contains(other.id) && layout.contains(tab.id)
                        }),
                    })
                    .collect::<Vec<_>>();
                let chosen = pick_git_terminal(tab.project_root.as_deref(), &candidates)?;
                terminals
                    .into_iter()
                    .find(|(other, _)| other.id == chosen)
                    .map(|(other, handle)| (other.id, handle.clone()))
            }
        }
    }
}

/// A terminal that could lend its repository to a browser pane.
struct GitCandidate<'a> {
    id: usize,
    project_root: Option<&'a Path>,
    shown_beside: bool,
}

/// The terminal whose repository a browser in `browser_root` shows: only
/// project terminals qualify; the same project wins, then one shown beside it.
fn pick_git_terminal(browser_root: Option<&Path>, candidates: &[GitCandidate]) -> Option<usize> {
    candidates
        .iter()
        .filter(|candidate| candidate.project_root.is_some())
        .max_by_key(|candidate| {
            let same_project = browser_root.is_none() || candidate.project_root == browser_root;
            (same_project, candidate.shown_beside)
        })
        .map(|candidate| candidate.id)
}

/// `dragent browser`: agents drive the browser beside their terminal.
impl WorkspaceView {
    /// Run a `dragent browser` command against the browser beside the
    /// calling terminal. `None`: answered once the page has done it.
    pub(crate) fn browser_tool_for_cli(
        &mut self,
        id: &str,
        pane: Option<&str>,
        terminal: Option<&str>,
        tool: &str,
        arguments: &serde_json::Value,
        ctx: &mut ViewContext<Self>,
    ) -> Option<Response> {
        let id = id.to_string();
        let reply: AgentReply = Box::new(move |result| {
            CliInbox::respond(
                &id,
                &match result {
                    Ok(output) => Response::with_data(serde_json::json!({ "output": output })),
                    Err(error) => match dragent_host::outcome::Failure::decode(&error) {
                        Some(failure) => Response::failure(failure),
                        None => Response::failed(error),
                    },
                },
            )
        });
        self.run_browser_tool(pane, terminal, tool, arguments, reply, ctx);
        None
    }

    /// Find (or, for `open`, make) the browser for a command and hand the
    /// command to it; `reply` gets the answer either way.
    pub(crate) fn run_browser_tool(
        &mut self,
        pane: Option<&str>,
        terminal: Option<&str>,
        tool: &str,
        arguments: &serde_json::Value,
        reply: AgentReply,
        ctx: &mut ViewContext<Self>,
    ) {
        let caller = self.cli_caller_tab(pane, terminal, ctx);
        if tool == "list" {
            return reply(Ok(self.browser_list(caller, ctx).to_string()));
        }
        let command = match AgentCommand::parse(tool, arguments) {
            Ok(command) => command,
            Err(error) => return reply(Err(error)),
        };
        // A browser the agent named (`--pane`, an id from `list`), else the
        // one nearest the caller; `open --new` always gets a fresh one.
        let named = match arguments["pane"].as_u64() {
            Some(id) => match self.browser_by_id(id as usize) {
                Some(found) => Some(found),
                None => {
                    return reply(Err(crate::browser::agent::failure(
                        dragent_host::outcome::Status::InvalidTarget,
                        "no_such_browser",
                        format!("There's no browser pane {id}."),
                        "`dragent browser list` shows the browser panes and their ids.",
                    )))
                }
            },
            None => None,
        };
        let fresh = matches!(command, AgentCommand::Open(_))
            && named.is_none()
            && arguments["new"].as_bool() == Some(true);
        let nearest = if fresh { None } else { named.or_else(|| self.browser_near(caller, ctx)) };
        let (surface, tab_id, browser) = match nearest {
            Some(found) => found,
            None if matches!(command, AgentCommand::Open(_)) => {
                match self.open_browser_for_agent(caller, ctx) {
                    Some(found) => found,
                    None => return reply(Err("Dragent couldn't open a browser here.".into())),
                }
            }
            None => {
                return reply(Err("No browser is open beside this terminal. Run \
                                  `dragent browser open URL` to open one."
                    .into()))
            }
        };
        // The person watches what the agent does: bring the page into view
        // (its engine only runs while it is shown).
        self.reveal_browser(surface, tab_id, ctx);
        browser.update(ctx, |view, view_ctx| view.agent(command, reply, view_ctx));
    }

    /// The workspace tab and pane-tab of the terminal a command came from:
    /// an agent Dragent started (`pane`) or any host terminal (`terminal`).
    pub(crate) fn cli_caller_tab(
        &self,
        pane: Option<&str>,
        terminal: Option<&str>,
        ctx: &ViewContext<Self>,
    ) -> Option<(usize, usize)> {
        let pane = pane.and_then(|pane| Uuid::parse_str(pane).ok());
        let terminal_tab = terminal.and_then(|terminal| {
            self.host_owned_tabs
                .iter()
                .find(|(_, owned)| owned.persisted_session_id() == Some(terminal))
                .map(|(tab_id, _)| *tab_id)
        });
        let is_caller = |tab: &PaneTab| {
            terminal_tab == Some(tab.id) || pane.is_some_and(|pane| tab.pane.pane_uuid(ctx) == pane)
        };
        self.surfaces().find_map(|(surface, panes)| {
            panes
                .iter()
                .find(|tab| is_caller(tab))
                .map(|tab| (surface, tab.id))
        })
    }

    /// Every workspace tab's pane-tabs, the shown one first.
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = (usize, &[PaneTab])> {
        std::iter::once((self.active_workspace_tab_id, self.tabs.as_slice())).chain(
            self.parked_workspace_tabs
                .iter()
                .map(|surface| (surface.id, surface.panes.as_slice())),
        )
    }

    /// Every browser pane, for `dragent browser list`: its id (also its pane
    /// id in `dragent panes list`), page, whether it's on screen and whether
    /// it's in the caller's workspace tab.
    fn browser_list(
        &self,
        caller: Option<(usize, usize)>,
        ctx: &ViewContext<Self>,
    ) -> serde_json::Value {
        let shown = |surface: usize, tab: usize| {
            surface == self.active_workspace_tab_id
                && (tab == self.active_tab_id
                    || self
                        .split_tree
                        .as_ref()
                        .is_some_and(|layout| layout.contains(tab)))
        };
        let nearest = self.browser_near(caller, ctx).map(|(_, id, _)| id);
        serde_json::Value::Array(
            self.surfaces()
                .flat_map(|(surface, panes)| panes.iter().map(move |tab| (surface, tab)))
                .filter_map(|(surface, tab)| match &tab.pane {
                    WorkspacePane::Browser(browser) => {
                        let (url, title, loading) = browser.read(ctx, |browser, _| {
                            (browser.address().to_string(), browser.title(), browser.is_loading())
                        });
                        Some(serde_json::json!({
                            "id": tab.id,
                            "url": url,
                            "title": title,
                            "loading": loading,
                            "shown": shown(surface, tab.id),
                            "workspace_tab": surface,
                            "beside_you": caller.is_some_and(|(caller_surface, _)| caller_surface == surface),
                            "default": nearest == Some(tab.id),
                        }))
                    }
                    _ => None,
                })
                .collect(),
        )
    }

    /// The browser pane with id `id`, wherever it is.
    fn browser_by_id(&self, id: usize) -> Option<(usize, usize, ViewHandle<BrowserView>)> {
        self.surfaces().find_map(|(surface, panes)| {
            panes.iter().find_map(|tab| match &tab.pane {
                WorkspacePane::Browser(browser) if tab.id == id => {
                    Some((surface, tab.id, browser.clone()))
                }
                _ => None,
            })
        })
    }

    /// The browser an agent's command is for: one in the caller's workspace
    /// tab, else one in the same project; without a caller, the shown one.
    fn browser_near(
        &self,
        caller: Option<(usize, usize)>,
        ctx: &ViewContext<Self>,
    ) -> Option<(usize, usize, ViewHandle<BrowserView>)> {
        let browser_in = |surface: usize, panes: &[PaneTab], project: Option<&Path>| {
            panes.iter().find_map(|tab| match &tab.pane {
                WorkspacePane::Browser(browser)
                    if project.is_none_or(|root| tab.project_root.as_deref() == Some(root)) =>
                {
                    Some((surface, tab.id, browser.clone()))
                }
                _ => None,
            })
        };
        let _ = ctx;
        match caller {
            Some((surface, tab_id)) => {
                let project = self
                    .surfaces()
                    .find_map(|(_, panes)| panes.iter().find(|tab| tab.id == tab_id))
                    .and_then(|tab| tab.project_root.clone());
                self.surfaces()
                    .find(|(id, _)| *id == surface)
                    .and_then(|(id, panes)| browser_in(id, panes, None))
                    .or_else(|| {
                        let project = project.as_deref()?;
                        self.surfaces()
                            .find_map(|(id, panes)| browser_in(id, panes, Some(project)))
                    })
            }
            None => self
                .surfaces()
                .find_map(|(id, panes)| browser_in(id, panes, None)),
        }
    }

    /// Open a browser beside the calling terminal, leaving the keyboard with
    /// it, so the person sees the agent's page appear next to the agent.
    fn open_browser_for_agent(
        &mut self,
        caller: Option<(usize, usize)>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<(usize, usize, ViewHandle<BrowserView>)> {
        if let Some((surface, tab_id)) = caller {
            if surface != self.active_workspace_tab_id {
                self.handle_action(
                    &WorkspaceAction::SelectWorkspaceTab {
                        workspace_tab_id: surface,
                    },
                    ctx,
                );
            }
            if self.tabs.iter().any(|tab| tab.id == tab_id) {
                self.active_tab_id = tab_id;
            }
        }
        let new_tab = if self.tabs.is_empty() {
            self.open_browser_workspace_tab(ctx);
            self.active_tab_id
        } else {
            // With a caller, the keyboard stays with it.
            let split = self.split_browser(SplitSide::Right, None, caller.is_none(), ctx);
            split.unwrap_or(self.active_tab_id)
        };
        let browser = self.tabs.iter().find_map(|tab| match &tab.pane {
            WorkspacePane::Browser(browser) if tab.id == new_tab => Some(browser.clone()),
            _ => None,
        })?;
        Some((self.active_workspace_tab_id, new_tab, browser))
    }

    /// Show a browser pane-tab without disturbing a split it is part of.
    pub(super) fn reveal_browser(&mut self, surface: usize, tab_id: usize, ctx: &mut ViewContext<Self>) {
        if surface != self.active_workspace_tab_id {
            self.handle_action(
                &WorkspaceAction::SelectWorkspaceTab {
                    workspace_tab_id: surface,
                },
                ctx,
            );
        }
        let in_shown_split = self
            .split_tree
            .as_ref()
            .is_some_and(|layout| layout.contains(tab_id) && layout.contains(self.active_tab_id));
        if self.active_tab_id != tab_id && !in_shown_split {
            self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
        }
    }
}

/// Follow a browser pane's page events: titles, focus, and new windows.
pub(crate) fn subscribe_browser_pane(
    tab_id: usize,
    browser: &ViewHandle<BrowserView>,
    ctx: &mut ViewContext<WorkspaceView>,
) {
    ctx.subscribe_to_view(browser, move |view, browser, event, ctx| match event {
        BrowserEvent::DesignTargetsRequested => view.request_browser_design_targets(tab_id, ctx),
        BrowserEvent::SendDesign { target, prompt, files } => view.send_browser_design(tab_id, target, prompt, files, ctx),
        BrowserEvent::Changed => {
            view.refresh_all_tab_titles(ctx);
            view.schedule_left_panel_sync(ctx);
            view.schedule_session_save(ctx);
            ctx.notify();
        }
        // Only while it still has the keyboard: a focus that was already
        // handed to another pane must not pull the selection back, or the
        // two panes take it from each other forever.
        BrowserEvent::Focused => {
            if view.active_tab_id != tab_id
                && view.terminal_focus_can_activate(tab_id)
                && browser.read(ctx, |browser, _| browser.has_focus())
            {
                view.select_tab_with_focus(tab_id, false, ctx);
            }
        }
        BrowserEvent::OpenInNewTab(url) => {
            let url = url.clone();
            view.open_browser_pane_tab(Some(url), ctx);
        }
        // Same behaviour as a terminal pane's header: split along the
        // opposite of the parent split, so nested layouts alternate.
        BrowserEvent::SplitPane => {
            let action = match view
                .split_tree
                .as_ref()
                .and_then(|layout| layout.parent_direction(tab_id))
            {
                Some(SplitDirection::Horizontal) => WorkspaceAction::SplitTabVertical { tab_id },
                _ => WorkspaceAction::SplitTabHorizontal { tab_id },
            };
            view.handle_action(&action, ctx);
        }
        BrowserEvent::ClosePane => {
            view.handle_action(&WorkspaceAction::CloseTab { tab_id }, ctx);
        }
    });
}

/// The owner identity for a browser opened in `project_root` (or outside any
/// project). Storage follows the organisation; agent access follows the
/// project and workspace.
pub(crate) fn browser_owner(organisation: &str, project_root: Option<&Path>) -> BrowserOwner {
    let root = project_root
        .map(Path::to_path_buf)
        .unwrap_or_else(default_pane_cwd);
    let project_id = Project::new_for_organisation(organisation, root.clone(), 0).id;
    let workspace_key = format!("{organisation}\u{0}{}", root.display());
    BrowserOwner {
        organisation: organisation.to_string(),
        project_id,
        workspace_id: Uuid::new_v5(&Uuid::NAMESPACE_OID, workspace_key.as_bytes()),
    }
}

/// Sign-in pages that block embedded browsers ("This browser or app may not
/// be secure"); a command line's login link to one would dead-end in a tab.
fn refuses_embedded_browsers(url: &str) -> bool {
    const HOSTS: [&str; 4] = [
        "accounts.google.com",
        "login.microsoftonline.com",
        "login.live.com",
        "appleid.apple.com",
    ];
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| HOSTS.contains(&host.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_pages_that_refuse_embedding_stay_outside() {
        assert!(refuses_embedded_browsers(
            "https://accounts.google.com/o/oauth2/auth?client_id=x"
        ));
        assert!(refuses_embedded_browsers("https://Login.MicrosoftOnline.com/common"));
        assert!(!refuses_embedded_browsers("https://github.com/login/device"));
        assert!(!refuses_embedded_browsers("http://localhost:3000"));
    }

    fn candidate(
        id: usize,
        root: Option<&'static str>,
        shown_beside: bool,
    ) -> GitCandidate<'static> {
        GitCandidate {
            id,
            project_root: root.map(Path::new),
            shown_beside,
        }
    }

    #[test]
    fn a_browser_shows_the_repository_of_the_terminal_beside_it() {
        let project = Some(Path::new("/work/app"));
        // The agent's terminal split beside the browser wins.
        assert_eq!(
            pick_git_terminal(
                project,
                &[
                    candidate(1, Some("/work/app"), false),
                    candidate(2, Some("/work/app"), true)
                ]
            ),
            Some(2)
        );
        // Same project beats a terminal of another project shown beside it.
        assert_eq!(
            pick_git_terminal(
                project,
                &[
                    candidate(3, Some("/work/other"), true),
                    candidate(4, Some("/work/app"), false)
                ]
            ),
            Some(4)
        );
        // A browser opened outside a project borrows any project terminal.
        assert_eq!(
            pick_git_terminal(None, &[candidate(5, Some("/work/app"), false)]),
            Some(5)
        );
        // Projectless shells have no repository to lend.
        assert_eq!(
            pick_git_terminal(project, &[candidate(6, None, true)]),
            None
        );
        assert_eq!(pick_git_terminal(project, &[]), None);
    }
}

