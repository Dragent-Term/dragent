//! The Teams page in the window: opening and closing it, showing the agents
//! it points at, and the sidebar badge (with a desktop alert) for messages
//! a team's lead left for the person.

use std::path::PathBuf;
use std::time::Duration;

use warpui::r#async::Timer;
use warpui::{TypedActionView, UpdateView, ViewContext};

use super::view::WorkspaceView;
use super::WorkspaceAction;

/// How often the badge looks for messages left for the person.
const BADGE_POLL: Duration = Duration::from_secs(5);

impl WorkspaceView {
    pub(crate) fn open_teams(&mut self, team: Option<String>, ctx: &mut ViewContext<Self>) {
        self.close_pull_requests(ctx);
        self.close_settings(ctx);
        self.close_automations(ctx);
        self.close_tasks(ctx);
        self.close_org_menus();
        self.teams_open = true;
        let theme = self.theme;
        let page = self.teams_view.clone();
        ctx.update_view(&page, |page, page_ctx| page.show(theme, team, page_ctx));
        ctx.focus(&page);
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// Straight to a just-started team's Board, where its lead's planning
    /// shows.
    pub(crate) fn open_team_board(&mut self, team: String, ctx: &mut ViewContext<Self>) {
        self.open_teams(None, ctx);
        let theme = self.theme;
        let page = self.teams_view.clone();
        ctx.update_view(&page, |page, page_ctx| page.show_board(theme, team, page_ctx));
        ctx.notify();
    }

    pub(crate) fn close_teams(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.teams_open {
            return;
        }
        self.teams_open = false;
        let page = self.teams_view.clone();
        ctx.update_view(&page, |page, _| page.hide());
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
            tab.pane.focus(ctx);
        } else {
            ctx.focus_self();
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// Show the agent in `pane` (a team's lead or worker): its tab, else its
    /// workspace.
    pub(crate) fn open_team_pane(
        &mut self,
        pane: &str,
        workspace: Option<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(tab_id) = self.tab_with_pane(pane, ctx) {
            self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
            return;
        }
        if let Some(remote) = workspace.as_deref().and_then(crate::remote_fs::resolve) {
            let pane = pane.to_string();
            let machine = remote.machine_id;
            let project = self.teams_view.read(ctx, |page,_|page.team.as_ref().map(|team|PathBuf::from(&team.project_root)));
            ctx.spawn(async move {
                tokio::task::spawn_blocking(move || {
                    let client = crate::remote_fs::client(&machine)?;
                    let rows = client.coordination("host_sessions", serde_json::json!({"panes":[pane]}), serde_json::to_value(crate::cli::team::Caller::person()).unwrap())?;
                    let record: dragent_host::agent_sessions::AgentSession = serde_json::from_value(rows[0]["record"].clone()).map_err(|e|e.to_string())?;
                    let terminal = client.terminal(&record.workspace_id,&record.terminal_id)?;
                    Ok::<_,String>((machine,terminal,project))
                }).await.map_err(|e|e.to_string())?
            }, |view,result,ctx| match result {
                Ok((machine,terminal,project)) => { view.close_teams(ctx); view.attach_remote_session_at_project(&machine,terminal,project,ctx); }
                Err(error) => view.show_error_notice("Couldn't open remote agent", error, ctx),
            });
            return;
        }
        if let Some(workspace_id) = workspace
            .as_deref()
            .and_then(|path| self.workspace_for(path))
        {
            self.handle_action(
                &WorkspaceAction::SelectProjectWorkspace { workspace_id },
                ctx,
            );
        }
    }

    /// Start `agent` in `cwd` as the new lead of `team` (its last lead
    /// stopped), in a tab of its own, and show it.
    pub(crate) fn start_new_lead(
        &mut self,
        team: String,
        agent: &str,
        model: Option<String>,
        cwd: PathBuf,
        prompt: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(remote) = crate::remote_fs::resolve(&cwd) {
            let agent = agent.to_string();
            let machine = remote.machine_id;
            ctx.spawn(async move {
                tokio::task::spawn_blocking(move || {
                    let client = crate::remote_fs::client(&machine)?;
                    let value = client.coordination("host_start_lead", serde_json::json!({"team":crate::team_source::split(&team).1,"agent":agent,"model":model,"cwd":remote.path}), serde_json::to_value(crate::cli::team::Caller::person()).unwrap())?;
                    let response: crate::cli::requests::Response = serde_json::from_value(value).map_err(|e|e.to_string())?;
                    Ok::<_,String>((response.agent_id.ok_or("Missing lead session")?,cwd))
                }).await.map_err(|e|e.to_string())?
            }, |view,result,ctx|match result {
                Ok((pane,cwd))=>view.open_team_pane(&pane,Some(cwd),ctx),
                Err(error)=>view.show_error_notice("Couldn't restart remote lead",error,ctx),
            });
            return;
        }
        let Some(agent) = crate::automations::executor::prompt_agent(agent) else {
            return;
        };
        // Dragent's own agent leads once it's registered as the lead (its
        // instructions and tools come from that), so it's asked after.
        let dragent = agent == super::action::PromptAgent::Dragent;
        let first = if dragent { "" } else { prompt };
        let cli_model = if dragent { None } else { model.as_deref() };
        let started = match self.workspace_for(&cwd) {
            Some(workspace_id) => self.add_agent_surface(
                workspace_id,
                cwd.clone(),
                agent,
                first,
                cli_model,
                &dragent_host::agent_accounts::Selection::Inherit,
                ctx,
            ),
            None => {
                self.open_agent_surface(cwd.clone(), "Lead".into(), Some(agent), first, cli_model, ctx)
            }
        };
        match started {
            Ok(pane) => {
                let pane = pane.to_string();
                let key = agent.command_key().to_string();
                let lead_pane = pane.clone();
                if dragent {
                    let prompt = prompt.to_string();
                    let pane = pane.clone();
                    ctx.spawn(
                        async move { crate::cli::team::set_lead(&team, &lead_pane, &key, model.as_deref()) },
                        move |view, registered, ctx| {
                            if let Err(error) = registered {
                                view.show_error_notice("Couldn't start coordination lead", error, ctx);
                                return;
                            }
                            if let Some((terminal, _)) = view.terminal_with_pane(&pane, ctx) {
                                ctx.update_view(&terminal, |terminal, terminal_ctx| {
                                    terminal.submit_agent_query(prompt, terminal_ctx)
                                });
                            }
                        },
                    );
                } else {
                    std::thread::spawn(move || {
                        if let Err(error) = crate::cli::team::set_lead(&team, &lead_pane, &key, model.as_deref()) {
                            log::warn!(
                                "[team] couldn't make {lead_pane} the lead of {team}: {error}"
                            );
                        }
                    });
                }
                self.close_teams(ctx);
                self.open_team_pane(&pane, Some(cwd), ctx);
            }
            Err(error) => log::warn!("[team] couldn't start a new lead: {error}"),
        }
    }

    /// Close the workspaces of a finished team's worktrees and remove the
    /// worktrees (the Teams page checked they hold no uncommitted work;
    /// branches stay). One no longer in the sidebar is removed directly.
    pub(crate) fn clean_up_team(
        &mut self,
        project: PathBuf,
        remove: Vec<PathBuf>,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let mut outside = Vec::new();
        for path in remove {
            if self.workspace_for(&path).is_some() {
                let _ = self.delete_workspace_for_cli(&path, ctx);
            } else {
                outside.push(path);
            }
        }
        if !outside.is_empty() {
            std::thread::spawn(move || {
                for path in outside {
                    let mut command = crate::git_cmd::GitCommand::new();
                    command.arg("-C").arg(&project).args(["worktree", "remove"]);
                    if force {
                        command.arg("--force");
                    }
                    let removed = command.arg("--").arg(&path).output();
                    if !removed.is_ok_and(|output| output.status.success()) {
                        log::warn!("[team] couldn't remove the worktree {}", path.display());
                    }
                }
            });
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// A deleted coordination's work: close its workers' tabs wherever they
    /// are (never removing a worktree with a tab), then their workspaces
    /// and worktrees, uncommitted work and all (the person confirmed). The
    /// lead's tab and workspace stay and, the coordination forgotten, show
    /// as an ordinary workspace.
    pub(crate) fn delete_team_work(
        &mut self,
        project: PathBuf,
        remove: Vec<PathBuf>,
        workers: &[String],
        ctx: &mut ViewContext<Self>,
    ) {
        for pane in workers.iter().filter_map(|pane| uuid::Uuid::parse_str(pane).ok()) {
            let _ = self.close_agent_tab(pane, ctx);
        }
        self.clean_up_team(project, remove, true, ctx);
    }

    /// Dragent's own agents in this window that lead teams: note what each
    /// is doing (the page shows it), and bring an idle one its news as a
    /// message from Dragent. The person's messages also go to one that's
    /// working, which queues them. Nothing goes in while it waits for the
    /// person, or the person is writing to it.
    fn serve_dragent_leads(&mut self, leads: &[(String, String)], ctx: &mut ViewContext<Self>) {
        use crate::teams::WorkerState;
        for (team, pane) in leads {
            let Some((terminal, cwd)) = self.terminal_with_pane(pane, ctx) else {
                continue;
            };
            let (state, writing) = terminal.read(ctx, |terminal, app| {
                let controller = terminal.agent_controller.as_ref(app);
                let state = if controller.frontmost_pending_approval().is_some()
                    || controller.first_pending_question_id().is_some()
                {
                    WorkerState::NeedsApproval
                } else if controller.is_busy() {
                    WorkerState::Working
                } else {
                    WorkerState::Idle
                };
                let writing = [&terminal.rich_input_editor, &terminal.command_editor]
                    .iter()
                    .any(|editor| !editor.as_ref(app).get_text().trim().is_empty());
                (state, writing)
            });
            crate::teams::note_dragent_lead(pane, state, cwd);
            if state == WorkerState::NeedsApproval
                || writing
                || !self.lead_news_pending.insert(team.clone())
            {
                continue;
            }
            let busy = state != WorkerState::Idle;
            let team = team.clone();
            let taking = team.clone();
            ctx.spawn(
                async move { crate::teams::take_lead_news(&taking, busy) },
                move |view, news, ctx| {
                    view.lead_news_pending.remove(&team);
                    let Some(news) = news else {
                        return;
                    };
                    let from = crate::agent::controller::AgentAuthor {
                        display_name: "Dragent".into(),
                        avatar_url: String::new(),
                    };
                    ctx.update_view(&terminal, |terminal, terminal_ctx| {
                        terminal.submit_agent_query_with_system_context(
                            news,
                            None,
                            Some(from),
                            terminal_ctx,
                        )
                    });
                },
            );
        }
    }

    /// The terminal showing agent session `pane` in this window, with its
    /// folder.
    fn terminal_with_pane(
        &self,
        pane: &str,
        ctx: &ViewContext<Self>,
    ) -> Option<(
        warpui::ViewHandle<crate::terminal::view::TerminalView>,
        PathBuf,
    )> {
        let pane = uuid::Uuid::parse_str(pane).ok()?;
        self.tabs
            .iter()
            .chain(
                self.parked_workspace_tabs
                    .iter()
                    .flat_map(|surface| surface.panes.iter()),
            )
            .find_map(|tab| match &tab.pane {
                super::pane::WorkspacePane::Terminal(terminal)
                    if tab.pane.pane_uuid(ctx) == pane =>
                {
                    Some((terminal.clone(), tab.pane.cwd(ctx)))
                }
                _ => None,
            })
    }

    /// Look for messages left for the person, now and every few seconds:
    /// the sidebar shows how many, and a new one rings. Dragent's own
    /// agents that lead teams get their news on the same beat.
    pub(crate) fn poll_teams_badge(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(async { crate::teams::overview() }, |view, overview, ctx| {
            view.serve_dragent_leads(&overview.dragent_leads, ctx);
            // The sidebar groups each coordination's workspaces.
            if view.coordination_groups != overview.groups {
                view.coordination_groups = overview.groups;
                view.sync_left_panel(ctx);
            }
            for (team, due) in overview.due {
                crate::cli::team::start_due_in_background(team, due);
            }
            let waiting = overview.waiting;
            // The first look only counts: what waited before the app
            // started isn't news.
            let seeded = std::mem::replace(&mut view.teams_badge_seeded, true);
            for (team, title, count) in waiting.iter().filter(|_| seeded) {
                let before = view.teams_waiting.get(team).copied().unwrap_or(0);
                if *count > before {
                    crate::notifications::notify(
                        ctx,
                        crate::notifications::Kind::AgentNeedsYou,
                        &format!("Coordination · {title}"),
                        "The lead has a question for you",
                        Some(format!("{}{team}", crate::teams::ALERT_PREFIX)),
                    );
                }
            }
            view.teams_waiting = waiting
                .iter()
                .map(|(team, _, count)| (team.clone(), *count))
                .collect();
            let total = waiting.iter().map(|(_, _, count)| count).sum();
            if total != view.teams_needing_you {
                view.teams_needing_you = total;
                view.sync_left_panel(ctx);
            }
            ctx.spawn(Timer::after(BADGE_POLL), |view, _, ctx| {
                view.poll_teams_badge(ctx)
            });
        });
    }
}
