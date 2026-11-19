use super::{view::WorkspaceView, WorkspaceAction};
use warpui::{TypedActionView, UpdateView, ViewContext};
impl WorkspaceView {
    pub(crate) fn open_pull_requests(&mut self, ctx: &mut ViewContext<Self>) {
        self.close_settings(ctx);
        self.close_automations(ctx);
        self.close_teams(ctx);
        self.close_tasks(ctx);
        self.close_org_menus();
        self.pull_requests_open = true;
        let mut roots = self.sidebar_project_roots();
        if let Some(active) = self.active_project_root() {
            roots.retain(|r| *r != active);
            roots.insert(0, active);
        }
        let targets = roots
            .into_iter()
            .filter(|root| root.join(".git").exists() || crate::remote_fs::is_remote(root))
            .map(|root| crate::pull_requests::data::Target {
                label: root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string()),
                root,
            })
            .collect();
        let page = self.pull_requests_view.clone();
        let theme = self.theme;
        ctx.update_view(&page, |p, c| p.show(theme, targets, c));
        ctx.focus(&page);
        self.sync_left_panel(ctx);
        ctx.notify();
    }
    pub(crate) fn close_pull_requests(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.pull_requests_open {
            return;
        }
        self.pull_requests_open = false;
        ctx.update_view(&self.pull_requests_view, |p, c| p.hide(c));
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
            tab.pane.focus(ctx);
        } else {
            ctx.focus_self();
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }
    pub(crate) fn on_pull_request_event(
        &mut self,
        event: &crate::pull_requests::Event,
        ctx: &mut ViewContext<Self>,
    ) {
        use crate::pull_requests::Event;
        match event {
            Event::Error(e) => self.show_error_notice("Pull requests", e.clone(), ctx),
            Event::OpenUrl(url) => {
                self.handle_action(&WorkspaceAction::OpenExternalUrl { url: url.clone() }, ctx)
            }
            Event::Copy(text) => {
                ctx.clipboard()
                    .write(warpui_core::clipboard::ClipboardContent::plain_text(
                        text.clone(),
                    ));
            }
            Event::Close => self.close_pull_requests(ctx),
            Event::ConnectProvider { root, provider } => {
                let label = format!("Sign in to {}", provider.label());
                let command = provider.login_command().to_string();
                let root = root.clone();
                crate::remote_fs::ensure_stand_in(&root);
                self.open_page_command_terminal(&label, Some(root), &command, ctx);
            }
            Event::InstallProviderTools { root } => {
                let machine =
                    crate::remote_fs::resolve(root).map(|remote| remote.machine_id.clone());
                self.close_pull_requests(ctx);
                self.show_machines(machine, ctx);
            }
            Event::ReviewSetup(review) => {
                if self.project_start_busy {
                    self.show_error_notice(
                        "Review setup",
                        "Wait for the current workspace to finish starting.",
                        ctx,
                    );
                    return;
                }
                self.review_setup = Some(review.clone());
                self.review_instructions = review.instructions.clone();
                self.project_start_error = None;
                self.project_start_busy = false;
                self.prompt_menu = None;
                if review.launch_agent { self.prompt_team = false; }
                let editor = self.review_instructions_editor.clone();
                ctx.update_view(&editor, |e, c| {
                    e.set_text(&review.instructions, c);
                    e.schedule_width_reflow(650.0, c);
                });
                if review.launch_agent {
                    ctx.focus(&editor);
                } else {
                    ctx.focus_self();
                }
                ctx.notify();
            }
        }
    }
    pub(crate) fn cancel_review_setup(&mut self, ctx: &mut ViewContext<Self>) {
        if self.project_start_busy {
            return;
        }
        self.review_setup = None;
        self.review_instructions.clear();
        self.prompt_menu = None;
        self.project_start_error = None;
        if self.pull_requests_open {
            ctx.focus(&self.pull_requests_view);
        } else {
            ctx.focus_self();
        }
        ctx.notify();
    }

    pub(crate) fn start_review_setup(&mut self, ctx: &mut ViewContext<Self>) {
        if self.project_start_busy {
            return;
        }
        let Some(review) = self.review_setup.clone() else {
            return;
        };
        if !self.workspace_trust.is_trusted(&review.target.root)
            && super::action_projects::workspace_trust_enforced()
        {
            self.pending_workspace_trust = Some(review.target.root.clone());
            self.pending_workspace_trust_document = None;
            self.pending_project_folder_intent = None;
            ctx.notify();
            return;
        }
        if review.launch_agent && self.review_instructions.trim().is_empty() {
            self.project_start_error = Some("Enter review instructions before starting.".into());
            ctx.notify();
            return;
        }
        let machine = self.machine_agents(Some(&review.target.root), ctx);
        if review.launch_agent && self.prompt_agent != super::PromptAgent::Dragent {
            let problem = if let Some(machine) = &machine {
                let agent = self.prompt_agent;
                self.machine_agent_problem(
                    machine,
                    agent.cli().unwrap_or_default(),
                    agent.label(),
                    ctx,
                )
            } else if super::action_projects::prompt_agent_interactive_command(self.prompt_agent)
                .is_none()
            {
                Some(format!(
                    "{} is not installed. Select it to install it first.",
                    self.prompt_agent.label()
                ))
            } else {
                None
            };
            if let Some(problem) = problem {
                self.project_start_error = Some(problem);
                ctx.notify();
                return;
            }
        }
        let instructions = self.review_instructions.clone();
        let scope = self.organisation_id.clone();
        self.project_start_busy = true;
        self.project_start_error = None;
        self.prompt_menu = None;
        ctx.notify();
        let preparing = review.clone();
        ctx.spawn(
            crate::pull_requests::data::background(move || {
                let branch = crate::pull_requests::data::run(
                    &preparing.target,
                    dragent_host::pull_requests::Operation::PrepareWorkspace {
                        number: preparing.number,
                        head: preparing.head.clone(),
                    },
                )?;
                if preparing.launch_agent {
                    super::project_workspace_creation::create_review_worktree(
                        &preparing.target.root,
                        branch.trim(),
                        &preparing.head,
                    )
                } else {
                    super::project_workspace_creation::open_review_worktree(
                        &preparing.target.root,
                        branch.trim(),
                        &preparing.head,
                    )
                }
            }),
            move |view, result, ctx| {
                if view.organisation_id != scope
                    || !view.review_setup.as_ref().is_some_and(|r| {
                        r.head == review.head
                            && r.number == review.number
                            && r.target.root == review.target.root
                    })
                {
                    return;
                }
                view.project_start_busy = false;
                match result {
                    Ok((root, branch)) => {
                        let before = view.active_workspace_tab_id;
                        let title = format!(
                            "Review {}{} · {}",
                            if review.gitlab { "!" } else { "#" },
                            review.number,
                            review.title
                        );
                        let opened = if review.launch_agent {
                            view.finish_project_prompt_workspace(
                                root,
                                branch,
                                instructions,
                                Some(title),
                                None,
                                ctx,
                            );
                            view.active_workspace_tab_id != before
                        } else {
                            view.open_review_checkout(root, title, ctx)
                        };
                        if opened {
                            view.review_setup = None;
                            view.review_instructions.clear();
                            view.close_pull_requests(ctx);
                        }
                        ctx.notify();
                    }
                    Err(error) => {
                        view.project_start_error = Some(error);
                        ctx.notify();
                    }
                }
            },
        );
    }
    fn open_review_checkout(
        &mut self,
        root: std::path::PathBuf,
        title: String,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if let Some(id) = self
            .workspaces
            .iter()
            .find(|w| w.root == root)
            .map(|w| w.id)
        {
            self.handle_action(
                &WorkspaceAction::SelectProjectWorkspace { workspace_id: id },
                ctx,
            );
            return true;
        }
        if self.begin_fresh_surface(ctx).is_none() {
            return false;
        }
        self.assign_surface_to_workspace(root.clone(), title);
        let tab =
            self.create_tab_with_cwd(ctx, Some("Terminal".into()), None, root.clone(), Some(root));
        let id = tab.id;
        self.tabs.push(tab);
        self.active_tab_id = id;
        self.split_tree = None;
        self.document_center_open = false;
        self.document_terminal_visible = false;
        self.project_start_visible = false;
        self.project_start_error = None;
        self.right_panel_visible = true;
        self.right_panel_user_closed = false;
        self.switch_workspace_work_for_tab(id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_window_title(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        self.persistence_writer.flush();
        ctx.notify();
        true
    }
}
