use super::pane::WorkspacePane;
use super::project_sidebar::sidebar_diff_comments::{DiffComment, DiffCommentAgent, Placement};
use super::project_sidebar::ProjectSidebarView;
use super::{view::{PaneTab, WorkspaceView}, WorkspaceAction};
use std::path::Path;
use warpui::{Entity, ReadModel, TypedActionView, UpdateView, ViewContext};

impl WorkspaceView {
    /// The terminals a comment on `root` can go to: every tab of the
    /// workspace the diff belongs to, including its tabs that aren't open
    /// right now, with whether each is in the open tab.
    pub(super) fn feedback_terminals(&self, root: &Path) -> Vec<(&PaneTab, bool)> {
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id));
        let Some(workspace) = workspace else {
            return self
                .tabs
                .iter()
                .filter(|tab| tab.project_root.as_deref() == Some(root))
                .map(|tab| (tab, true))
                .collect();
        };
        let mut terminals = Vec::new();
        for surface in &workspace.tab_ids {
            if *surface == self.active_workspace_tab_id {
                terminals.extend(self.tabs.iter().map(|tab| (tab, true)));
            } else if let Some(parked) =
                self.parked_workspace_tabs.iter().find(|parked| parked.id == *surface)
            {
                terminals.extend(parked.panes.iter().map(|tab| (tab, false)));
            }
        }
        terminals.retain(|(tab, _)| matches!(tab.pane, WorkspacePane::Terminal(_)));
        terminals
    }

    pub(super) fn feedback_agents(&self, root: &Path, ctx: &ViewContext<Self>) -> Vec<DiffCommentAgent> {
        let settings = crate::setup::UiSettings::load();
        let mut agents = Vec::new();
        let mut tabs = self.feedback_terminals(root);
        tabs.sort_by_key(|(tab, _)| {
            std::cmp::Reverse(match &tab.pane {
                WorkspacePane::Terminal(t) => {
                    t.read(ctx, |t, _| t.last_local_input_edit.max(t.side_activity_at))
                }
                _ => web_time::Instant::now(),
            })
        });
        for (tab, open) in tabs {
            let WorkspacePane::Terminal(terminal) = &tab.pane else {
                continue;
            };
            let agent = terminal.read(ctx, |t, tc| {
                let detected = t.active_ai_tool(tc);
                let detected_id = detected.and_then(|tool| {
                    super::agent_definition::all_definitions(&settings.custom_agents, false)
                        .into_iter()
                        .find(|d| d.cli.as_deref() == Some(tool.cli()))
                        .map(|d| d.id)
                });
                let id = detected_id.or_else(|| t.agent_id.clone())?;
                // Put to sleep to save memory: its terminal is closed on
                // purpose, and sending wakes it.
                let asleep = super::agent_sleep::is_pane_asleep(t.pane_uuid);
                if id != "dragent" && detected.is_none() && !asleep {
                    let definition = super::agent_definition::definition_by_id(
                        &id,
                        &settings.custom_agents,
                        false,
                    )?;
                    let command = t.current_running_command.as_deref().unwrap_or("");
                    let executable = command
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_matches(|ch| ch == '\'' || ch == '"');
                    let executable = Path::new(executable)
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or(executable);
                    if !t.is_running(tc) || definition.cli.as_deref() != Some(executable) {
                        return None;
                    }
                }
                let name =
                    super::agent_definition::definition_by_id(&id, &settings.custom_agents, false)
                        .map(|d| d.name)
                        .unwrap_or_else(|| id.clone());
                let image = super::agent_definition::definition_by_id(
                    &id,
                    &settings.custom_agents,
                    self.theme.is_dark,
                )
                .and_then(|d| d.image);
                Some(DiffCommentAgent {
                    key: format!("existing:{}", t.pane_uuid),
                    title: tab.custom_label.clone().unwrap_or_else(|| tab.title.clone()),
                    detail: if asleep {
                        format!("{name} · asleep, wakes when you send")
                    } else if open {
                        name.clone()
                    } else {
                        format!("{name} · in another tab")
                    },
                    name,
                    agent: id,
                    existing: true,
                    image,
                })
            });
            if let Some(agent) = agent {
                agents.push(agent);
            }
        }
        let machine = self.machine_agents(Some(root), ctx);
        for definition in
            super::agent_definition::all_definitions(&settings.custom_agents, self.theme.is_dark)
        {
            let installed = if definition.id == "dragent" {
                true
            } else if let Some(machine) = &machine {
                definition
                    .cli
                    .as_deref()
                    .is_some_and(|cli| machine.has(cli))
            } else if let Some(agent) = definition.builtin {
                self.prompt_installed_agents.contains(&agent)
            } else {
                self.custom_installed_agents.contains(&definition.id)
            };
            if installed {
                agents.push(DiffCommentAgent {
                    key: format!("new:{}", definition.id),
                    title: definition.name.clone(),
                    detail: "Start a new session".into(),
                    name: definition.name,
                    agent: definition.id,
                    existing: false,
                    image: definition.image,
                });
            }
        }
        agents
    }

    /// The tab behind an `existing:<pane>` target whose agent is asleep.
    pub(super) fn asleep_target(&self, root: &Path, key: &str, ctx: &ViewContext<Self>) -> Option<(usize, uuid::Uuid)> {
        self.feedback_terminals(root).into_iter().find_map(|(tab, _)| {
            let pane = tab.pane.pane_uuid(ctx);
            (format!("existing:{pane}") == key && super::agent_sleep::is_pane_asleep(pane)).then_some((tab.id, pane))
        })
    }

    /// Wake a sleeping agent, then run `then` once it's up again: awake,
    /// its agent running in the terminal and given a moment to start
    /// taking input. `failed` hears why when it can't be woken or doesn't
    /// come up within a minute and a half.
    pub(super) fn wake_then(
        &mut self,
        tab_id: usize,
        pane: uuid::Uuid,
        then: Box<dyn FnOnce(&mut Self, &mut ViewContext<Self>)>,
        failed: Box<dyn FnOnce(&mut Self, String, &mut ViewContext<Self>)>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.wake_agent(tab_id, ctx) {
            return failed(self, "That agent couldn't be woken: its conversation can't be resumed.".into(), ctx);
        }
        self.wait_awake(tab_id, pane, 0, then, failed, ctx);
    }

    fn wait_awake(
        &mut self,
        tab_id: usize,
        pane: uuid::Uuid,
        tries: u32,
        then: Box<dyn FnOnce(&mut Self, &mut ViewContext<Self>)>,
        failed: Box<dyn FnOnce(&mut Self, String, &mut ViewContext<Self>)>,
        ctx: &mut ViewContext<Self>,
    ) {
        const EVERY_MS: u64 = 500;
        const MOST: u32 = 180;
        ctx.spawn(warpui::r#async::Timer::after(std::time::Duration::from_millis(EVERY_MS)), move |view, _, ctx| {
            let up = !super::agent_sleep::is_pane_asleep(pane)
                && view.host_owned_tabs.get(&tab_id).is_some_and(|owned| {
                    owned.pane.read(ctx, |terminal, terminal_ctx| {
                        terminal.is_running(terminal_ctx) && terminal.active_ai_tool(terminal_ctx).is_some()
                    })
                });
            if up {
                // Its screen is up; give it a moment to take typing.
                ctx.spawn(warpui::r#async::Timer::after(std::time::Duration::from_millis(2500)), move |view, _, ctx| {
                    then(view, ctx)
                });
            } else if tries >= MOST {
                failed(view, "That agent didn't wake up in time. Try again, or choose another.".into(), ctx);
            } else {
                view.wait_awake(tab_id, pane, tries + 1, then, failed, ctx);
            }
        });
    }

    pub(crate) fn handle_diff_comment_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        let comment = self
            .project_sidebar_view
            .read(ctx, |view, _| view.diff_comment.clone());
        let Some(comment) = comment else {
            return;
        };
        // A comment is owned by the workspace that supplied the diff. Never
        // deliver a retained draft to a different org or a different worktree.
        if self.organisation_id != dragent_paths::organisation_id() {
            return;
        }
        let current_root = ctx.read_model(&self.project_sidebar_model, |m, _| {
            m.git_review_root()
                .unwrap_or(m.workspace_root())
                .to_path_buf()
        });
        if current_root != comment.root {
            return;
        }
        if comment.workspace.as_ref().is_some_and(|scope| {
            scope != &(self.organisation_id.clone(), self.active_workspace_tab_id)
        }) {
            return;
        }
        let agents = self.feedback_agents(&comment.root, ctx);
        if matches!(action, WorkspaceAction::RefreshDiffCommentAgents) {
            let settings = crate::setup::UiSettings::load();
            ctx.update_view(&self.project_sidebar_view, |view, vc| {
                if let Some(c) = &mut view.diff_comment {
                    c.workspace =
                        Some((self.organisation_id.clone(), self.active_workspace_tab_id));
                    c.agents = agents;
                    if !c.selection_override
                        || !c
                            .selected
                            .as_ref()
                            .is_some_and(|key| c.agents.iter().any(|a| a.key == *key))
                    {
                        c.selected = super::default_agent_target(&c.agents, &settings);
                    }
                }
                ProjectSidebarView::diff_comment_changed(vc);
            });
            return;
        }
        if comment.sending || comment.text.trim().is_empty() {
            return;
        }
        let Some(target) = comment
            .selected
            .as_ref()
            .and_then(|key| agents.iter().find(|a| a.key == *key))
            .cloned()
        else {
            self.finish_diff_comment(
                &comment,
                Err(
                    "This agent session has ended. Choose another session or start a new one."
                        .into(),
                ),
                ctx,
            );
            self.handle_diff_comment_action(&WorkspaceAction::RefreshDiffCommentAgents, ctx);
            return;
        };
        if target.existing {
            if let Some((tab_id, pane)) = self.asleep_target(&comment.root, &target.key, ctx) {
                ctx.update_view(&self.project_sidebar_view, |view, vc| {
                    if let Some(c) = &mut view.diff_comment {
                        c.sending = true;
                        c.error = None;
                    }
                    ProjectSidebarView::diff_comment_changed(vc);
                });
                let again = action.clone();
                let woken: Box<dyn FnOnce(&mut Self, &mut ViewContext<Self>)> = Box::new(move |view, ctx| {
                    ctx.update_view(&view.project_sidebar_view, |sidebar, vc| {
                        if let Some(c) = &mut sidebar.diff_comment {
                            c.sending = false;
                        }
                        ProjectSidebarView::diff_comment_changed(vc);
                    });
                    view.handle_diff_comment_action(&again, ctx);
                });
                let stuck = comment.clone();
                let failed: Box<dyn FnOnce(&mut Self, String, &mut ViewContext<Self>)> =
                    Box::new(move |view, reason, ctx| view.finish_diff_comment(&stuck, Err(reason), ctx));
                self.wake_then(tab_id, pane, woken, failed, ctx);
                return;
            }
        }
        let prompt = comment.prompt();
        if target.existing {
            let terminal = self
                .feedback_terminals(&comment.root)
                .into_iter()
                .find_map(|(tab, _)| match &tab.pane {
                    WorkspacePane::Terminal(t)
                        if format!("existing:{}", tab.pane.pane_uuid(ctx)) == target.key =>
                    {
                        Some(t.clone())
                    }
                    _ => None,
                });
            let Some(terminal) = terminal else {
                return;
            };
            if target.agent == "dragent" {
                ctx.update_view(&terminal, |t, tc| t.submit_agent_query(prompt, tc));
                self.finish_diff_comment(&comment, Ok(()), ctx);
            } else {
                let (writer, delay) = terminal.read(ctx, |t, tc| {
                    (
                        t.manager.writer().clone(),
                        t.active_ai_tool(tc)
                            .map(|tool| tool.submit_cr_delay_ms())
                            .unwrap_or(100),
                    )
                });
                ctx.update_view(&self.project_sidebar_view, |view, vc| {
                    if let Some(c) = &mut view.diff_comment {
                        c.sending = true;
                        c.error = None;
                    }
                    ProjectSidebarView::diff_comment_changed(vc);
                });
                let prompt: String = prompt
                    .chars()
                    .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
                    .collect();
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let result = (|| -> Result<(), String> {
                        let text = prompt.replace("\r\n", "\n").replace('\r', "\n");
                        writer.write_confirmed(format!("\x1b[200~{text}\x1b[201~").as_bytes())?;
                        std::thread::sleep(std::time::Duration::from_millis(delay.max(50)));
                        writer.write_confirmed(b"\r")
                    })();
                    let _ = tx.send(result);
                });
                ctx.spawn(rx, move |view, result, ctx| {
                    view.finish_diff_comment(
                        &comment,
                        result.unwrap_or_else(|_| Err("Comment delivery interrupted".into())),
                        ctx,
                    );
                    view.handle_diff_comment_action(
                        &WorkspaceAction::RefreshDiffCommentAgents,
                        ctx,
                    );
                });
            }
        } else {
            let result = self.start_diff_comment_agent(&comment, &target.agent, &prompt, ctx);
            self.finish_diff_comment(&comment, result, ctx);
        }
    }

    fn finish_diff_comment(
        &mut self,
        sent: &DiffComment,
        result: Result<(), String>,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Err(error) = &result { self.show_error_notice("Couldn't send comment", error.clone(), ctx); }
        ctx.update_view(&self.project_sidebar_view, |view, vc| {
            if view
                .diff_comment
                .as_ref()
                .is_some_and(|c| c.generation == sent.generation)
            {
                match result {
                    Ok(()) => view.diff_comment = None,
                    Err(_) => {
                        if let Some(c) = &mut view.diff_comment {
                            c.sending = false;
                            c.error = None;
                        }
                    }
                }
                ProjectSidebarView::diff_comment_changed(vc);
            }
        });
    }

    /// The command that starts `agent_id` on `prompt` in `root`, or `None`
    /// for Dragent's own agent (which takes the prompt as a query), plus the
    /// agent's display name.
    fn diff_comment_launch(
        &self,
        agent_id: &str,
        root: &Path,
        prompt: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(Option<String>, String), String> {
        let settings = crate::setup::UiSettings::load();
        let definition =
            super::agent_definition::definition_by_id(agent_id, &settings.custom_agents, false)
                .ok_or("That agent's configuration no longer exists")?;
        if agent_id == "dragent" {
            return Ok((None, definition.name));
        }
        let config = if let Some(agent) = definition.builtin {
            super::action_projects::agent_command_config(agent, &settings)
        } else {
            settings
                .custom_agents
                .iter()
                .find(|a| super::agent_definition::custom_id(&a.id) == agent_id)
                .ok_or("That agent's configuration no longer exists")?
                .launch
                .clone()
        };
        let shell = self
            .tabs
            .iter()
            .find_map(|tab| match &tab.pane {
                WorkspacePane::Terminal(t) => t.read(ctx, |t, _| t.launch_shell.clone()),
                _ => None,
            })
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    // The shell a new tab opens: Git Bash when installed.
                    dragent_pty::resolve_shell(
                        crate::console::editor_settings::EditorSettings::global()
                            .terminal
                            .default_shell
                            .clone(),
                    )
                } else {
                    "sh".into()
                }
            });
        let machine = self.machine_agents(Some(root), ctx);
        // A shell on another machine can't read a prompt file written here.
        let line = config.build_prompt_command(prompt, &shell, machine.is_none())?;
        let line = match machine {
            Some(machine) => machine.launch_line(&line),
            None => line,
        };
        Ok((Some(line), definition.name))
    }

    /// Start the chosen agent on the comment beside the diff or in a new tab
    /// of this workspace.
    fn start_diff_comment_agent(
        &mut self,
        c: &DiffComment,
        agent_id: &str,
        prompt: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        self.start_feedback_agent(&c.root, c.placement, agent_id, prompt, &[], ctx)
    }

    pub(super) fn start_feedback_agent(
        &mut self,
        root: &Path,
        placement: Placement,
        agent_id: &str,
        prompt: &str,
        files: &[std::path::PathBuf],
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let (command, name) = self.diff_comment_launch(agent_id, root, prompt, ctx)?;
        let split_target = if self.editor_pane_focused && self.editor_pane_in_layout() {
            super::pane_layout::EDITOR_PANE_ID
        } else {
            self.active_tab_id
        };
        // A new tab here is a pane tab in this workspace, never a new workspace.
        let tab = if agent_id == "dragent" {
            self.create_agent_tab_at(ctx, Some(root.to_path_buf()))
        } else {
            self.create_tab_with_cwd(ctx, Some(name), None, root.to_path_buf(), Some(root.to_path_buf()))
        };
        tab.pane.set_agent_id(ctx, Some(agent_id.to_string()));
        let WorkspacePane::Terminal(terminal) = &tab.pane else {
            return Err("Couldn't open a terminal for the agent".into());
        };
        let terminal = terminal.clone();
        let tab_id = tab.id;
        self.tabs.push(tab);
        if placement == Placement::Split {
            let layout = self
                .split_tree
                .get_or_insert_with(|| super::pane_layout::PaneLayout::new(split_target));
            layout.split_pane(split_target, tab_id, super::pane_layout::SplitSide::Right);
            layout.focus(tab_id);
        }
        self.active_tab_id = tab_id;
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.prune_split_handles();
        self.refresh_all_tab_titles(ctx);
        self.sync_window_title(ctx);
        self.update_active_pane(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.update_view(&terminal, |t, tc| match command {
            Some(command) => t.submit_shell_command(&command, tc),
            None => {
                t.receive_context_files(files.iter().take(5).cloned().collect(), tc);
                t.submit_agent_query(prompt.to_string(), tc);
            }
        });
        ctx.notify();
        Ok(())
    }
}

impl WorkspaceView {
    /// Drives a diff comment for the E2E automation bridge and replies with
    /// the card's state. `action` is `open:<path>` (working-tree diff) or
    /// one of [`ProjectSidebarView::diff_comment_automation`]'s.
    pub(crate) fn diff_comment_automation(
        &mut self,
        action: &str,
        ctx: &mut ViewContext<Self>,
    ) -> serde_json::Value {
        if let Some(path) = action.strip_prefix("open:") {
            self.handle_action(
                &WorkspaceAction::OpenGitDiff { rel_path: path.to_string(), staged: false },
                ctx,
            );
        }
        let diff = self.project_sidebar_model.read(ctx, |model, _| {
            model
                .git_file_diff
                .as_ref()
                .map(|diff| diff.relative_path.display().to_string())
        });
        let mut state = ctx.update_view(&self.project_sidebar_view, |view, vc| {
            view.diff_comment_automation(action, vc)
        });
        state["diff"] = diff.into();
        state
    }
}
