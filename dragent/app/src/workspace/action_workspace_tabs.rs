//! Top-level workspace-tab actions (Worktree = workspace > Tab > Panes).
//!
//! Handles `NewWorkspaceTab`, `SelectWorkspaceTab`, `CloseWorkspaceTab` (+
//! confirmed), rename, and strip reorder. Called from the dispatcher
//! (view_actions); same patterns as the pane-tab flow in action_tabs.
//!
//! The ACTIVE surface's state always lives in the `WorkspaceView` live
//! fields, so every pane helper below keeps working untouched — switching
//! only parks the live fields and restores the target's.

use super::persistence;
use super::project_naming::{default_pane_cwd, worktree_main_checkout};
use super::view::{MachineAccountCache, PaneTab, WorkspaceView};
use super::view_tabs::{apply_sidebar_visibility, refresh_sidebar_projects};
use super::pane::WorkspacePane;
use super::workspace_tab::WorkspaceTab;
use super::{SidebarFocus, WorkspaceAction};
use crate::process_ext::NoWindow;
use warpui::{TypedActionView, UpdateView, ViewContext};

impl WorkspaceView {
    pub(crate) fn handle_workspace_tab_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::SleepProjectWorkspace { workspace_id } => {
                ctx.update_view(&self.left_panel_view, |panel, panel_ctx| {
                    panel.sidebar_menu_tab_id = None;
                    panel_ctx.notify();
                });
                self.sleep_workspace(*workspace_id, ctx);
            }
            WorkspaceAction::CloseProjectWorkspace { workspace_id } => {
                let Some(workspace) = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == *workspace_id)
                    .cloned()
                else {
                    return;
                };
                let mut running = Vec::new();
                for surface in &workspace.tab_ids {
                    let panes = if *surface == self.active_workspace_tab_id {
                        Some(&self.tabs)
                    } else {
                        self.parked_workspace_tabs
                            .iter()
                            .find(|tab| tab.id == *surface)
                            .map(|tab| &tab.panes)
                    };
                    if let Some(panes) = panes {
                        running.extend(panes.iter().filter_map(|pane| {
                            crate::confirm::foreground_command_for(pane.pane.pane_uuid(ctx))
                        }));
                    }
                }
                if self.closing_worktree_tab.is_some() {
                    return;
                }
                running.sort();
                running.dedup();
                self.pending_close_project_workspace = Some(*workspace_id);
                self.pending_close_workspace_tab =
                    Some((workspace.active_tab_id, running.join(", ")));
                self.close_workspace_tab_error = None;
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                ctx.notify();
            }
            WorkspaceAction::SelectProjectWorkspace { workspace_id } => {
                let Some(tab_id) = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == *workspace_id)
                    .map(|workspace| workspace.active_tab_id)
                else {
                    return;
                };
                if tab_id == self.active_workspace_tab_id {
                    // Already looking at this workspace: only the active pane
                    // counts as seen. Other tabs keep their Review dot until
                    // their own tab is opened.
                    let active_pane = self
                        .tabs
                        .iter()
                        .find(|tab| tab.id == self.active_tab_id)
                        .map(|tab| tab.pane.pane_uuid(ctx));
                    ctx.update_view(&self.left_panel_view, |panel, panel_ctx| {
                        if let Some(pane) = active_pane {
                            panel.acknowledge_pane_activity(pane);
                        }
                        panel_ctx.notify();
                    });
                    self.project_start_visible = false;
                    self.update_active_pane(ctx);
                    self.sync_left_panel(ctx);
                    self.save_session(ctx);
                    ctx.notify();
                } else {
                    self.handle_workspace_tab_action(
                        &WorkspaceAction::SelectWorkspaceTab {
                            workspace_tab_id: tab_id,
                        },
                        ctx,
                    );
                }
            }

            WorkspaceAction::NewWorkspaceTab => self.open_new_workspace_tab(None, None, ctx),
            WorkspaceAction::NewShellWorkspaceTab { shell } => {
                self.open_new_workspace_tab_in(None, None, Some(shell.clone()), ctx)
            }
            WorkspaceAction::NewDragentCliTab => {
                self.open_new_workspace_tab(Some("Dragent CLI"), Some("dragent --help"), ctx)
            }
            WorkspaceAction::NewWorkItemTab => self.open_new_work_item_tab(ctx),
            WorkspaceAction::OpenTeamPrompt => self.open_team_prompt(ctx),
            WorkspaceAction::SelectWorkspaceTab { workspace_tab_id } => {
                let workspace_tab_id = *workspace_tab_id;
                if workspace_tab_id == self.active_workspace_tab_id {
                    return;
                }
                if self.closing_worktree_tab.is_some() {
                    return;
                }
                let Some(pos) = self
                    .parked_workspace_tabs
                    .iter()
                    .position(|tab| tab.id == workspace_tab_id)
                else {
                    return;
                };
                self.renaming_workspace_tab = None;
                self.pending_close_workspace_tab = None;
                self.close_workspace_tab_error = None;
                self.store_active_workspace_tab();
                let target = self.parked_workspace_tabs.remove(pos);
                self.restore_workspace_tab(target);
                // An emptied surface shows the project start screen; any
                // surface with panes shows the workspace.
                self.project_start_visible = self.tabs.is_empty();
                self.new_tab_picker_open = false;
                self.prompt_menu = None;
                self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
                self.restore_document_owner_tab(self.active_tab_id, ctx);
                self.set_active_terminal_sticky_headers(true, ctx);
                self.update_active_pane(ctx);
                self.refresh_all_tab_titles(ctx);
                self.sync_window_title(ctx);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp.sidebar_focus = SidebarFocus::None;
                    lp_ctx.notify();
                });
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                self.refresh_prompt_agents(ctx);
                ctx.notify();
            }
            WorkspaceAction::CloseWorkspaceTab { workspace_tab_id } => {
                self.pending_close_project_workspace = None;
                let workspace_tab_id = *workspace_tab_id;
                if self.closing_worktree_tab.is_some() {
                    return;
                }
                let panes: Vec<PaneTab> = if workspace_tab_id == self.active_workspace_tab_id {
                    self.tabs.clone()
                } else if let Some(tab) = self
                    .parked_workspace_tabs
                    .iter()
                    .find(|tab| tab.id == workspace_tab_id)
                {
                    tab.panes.clone()
                } else {
                    return;
                };
                let running: Vec<String> = panes
                    .iter()
                    .filter(|tab| !tab.pane.is_settings())
                    .filter_map(|tab| {
                        crate::confirm::foreground_command_for(tab.pane.pane_uuid(ctx))
                    })
                    .filter(|command| !command.is_empty())
                    .collect();
                // Skip the close confirmation when disabled.
                if running.is_empty()
                    || !crate::settings::git_consumers::confirm_close_terminal()
                {
                    self.handle_action(
                        &WorkspaceAction::CloseWorkspaceTabConfirmed { workspace_tab_id },
                        ctx,
                    );
                    return;
                }
                let mut summary = running;
                summary.sort();
                summary.dedup();
                let summary = summary.join(", ");
                self.pending_close_workspace_tab = Some((workspace_tab_id, summary));
                self.close_workspace_tab_error = None;
                ctx.notify();
            }
            WorkspaceAction::CloseWorkspaceTabConfirmed { workspace_tab_id } => {
                if let Some(id) = self.pending_close_project_workspace.take() {
                    self.pending_close_workspace_tab = None;
                    self.close_workspace_tab_error = None;
                    if let Some(workspace) = self
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id == id)
                        .cloned()
                    {
                        if !self.closed_workspace_roots.contains(&workspace.root) {
                            self.closed_workspace_roots.push(workspace.root.clone());
                        }
                        self.persistence_writer.send(
                            dragent_persistence::ModelEvent::CloseLocalWorkspace(
                                workspace.root.clone(),
                            ),
                        );
                        for tab_id in workspace.tab_ids {
                            self.handle_workspace_tab_action(
                                &WorkspaceAction::CloseWorkspaceTabConfirmed {
                                    workspace_tab_id: tab_id,
                                },
                                ctx,
                            );
                        }
                        self.workspaces.retain(|workspace| workspace.id != id);
                        self.sync_left_panel(ctx);
                        self.save_session(ctx);
                    }
                    return;
                }
                let workspace_tab_id = *workspace_tab_id;
                if self.closing_worktree_tab.is_some() {
                    return;
                }
                if let Some(workspace) = self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.tab_ids.as_slice() == [workspace_tab_id])
                {
                    if !self.closed_workspace_roots.contains(&workspace.root) {
                        self.closed_workspace_roots.push(workspace.root.clone());
                    }
                    self.persistence_writer.send(
                        dragent_persistence::ModelEvent::CloseLocalWorkspace(
                            workspace.root.clone(),
                        ),
                    );
                }
                self.pending_close_workspace_tab = None;
                self.close_workspace_tab_error = None;
                if workspace_tab_id == self.active_workspace_tab_id {
                    let panes = std::mem::take(&mut self.tabs);
                    let roots = self.dispose_surface_panes(&panes, ctx);
                    self.split_tree = None;
                    self.cleanup_surface_worktrees(&roots, ctx);
                    // Last tab never disappears — it resets to a fresh
                    // surface, mirroring the empty pane-tab path.
                    if self.workspace_tab_order.len() <= 1 {
                        self.active_tab_id = 0;
                        self.active_pane_id = None;
                        self.editor_pane_focused = false;
                        self.pending_close_tab = None;
                        self.close_tab_error = None;
                        self.project_start_visible = true;
                        // Keep the emptied workspace's projects offered in the
                        // start screen, mirroring the empty pane-tab path.
                        for root in &roots {
                            if !self.open_project_roots.contains(root) {
                                self.open_project_roots.push(root.clone());
                            }
                        }
                        if let Some(root) = roots.first() {
                            self.document_index_root = root.clone();
                        }
                        // The start screen keeps the sidebar, projects or not.
                        self.left_panel_visible = true;
                    } else {
                        // Prefer the tab after the closed one, else the one before.
                        let closed_pos = self
                            .workspace_tab_order
                            .iter()
                            .position(|id| *id == workspace_tab_id)
                            .unwrap_or(0);
                        self.workspace_tab_order
                            .retain(|id| *id != workspace_tab_id);
                        let owned_neighbor = self
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.tab_ids.contains(&workspace_tab_id))
                            .and_then(|workspace| {
                                workspace.tab_ids.iter().find(|id| **id != workspace_tab_id)
                            })
                            .copied();
                        let neighbor_id = owned_neighbor.or_else(|| {
                            self.workspace_tab_order
                                .get(
                                    closed_pos
                                        .min(self.workspace_tab_order.len().saturating_sub(1)),
                                )
                                .copied()
                        });
                        if let Some(neighbor_id) = neighbor_id {
                            let neighbor_pos = self
                                .parked_workspace_tabs
                                .iter()
                                .position(|tab| tab.id == neighbor_id);
                            if let Some(neighbor_pos) = neighbor_pos {
                                let neighbor = self.parked_workspace_tabs.remove(neighbor_pos);
                                self.restore_workspace_tab(neighbor);
                            } else {
                                self.restore_workspace_tab(WorkspaceTab::fresh(
                                    neighbor_id,
                                    String::new(),
                                ));
                            }
                        } else {
                            // Defensive: order bookkeeping lagged — stay on a
                            // fresh surface rather than a half-closed one.
                            self.workspace_tab_order.push(workspace_tab_id);
                            self.restore_workspace_tab(WorkspaceTab::fresh(
                                workspace_tab_id,
                                String::new(),
                            ));
                        }
                        self.project_start_visible = self.tabs.is_empty();
                    }
                    self.after_surface_layout_changed(ctx);
                } else {
                    let Some(pos) = self
                        .parked_workspace_tabs
                        .iter()
                        .position(|tab| tab.id == workspace_tab_id)
                    else {
                        return;
                    };
                    let parked = self.parked_workspace_tabs.remove(pos);
                    self.workspace_tab_order
                        .retain(|id| *id != workspace_tab_id);
                    let roots = self.dispose_surface_panes(&parked.panes, ctx);
                    self.cleanup_surface_worktrees(&roots, ctx);
                    self.save_session(ctx);
                    ctx.notify();
                }
            }
            WorkspaceAction::CancelCloseWorkspaceTab => {
                self.pending_close_project_workspace = None;
                self.pending_close_workspace_tab = None;
                self.close_workspace_tab_error = None;
                ctx.notify();
            }
            WorkspaceAction::StartRenameWorkspaceTab { workspace_tab_id } => {
                let workspace_tab_id = *workspace_tab_id;
                let stored = if workspace_tab_id == self.active_workspace_tab_id {
                    Some(self.active_workspace_tab_name.clone())
                } else {
                    self.parked_workspace_tabs
                        .iter()
                        .find(|tab| tab.id == workspace_tab_id)
                        .map(|tab| tab.name.clone())
                };
                let Some(stored) = stored else { return };
                // Prefill with the live context label when nothing custom
                // was stored, so the user edits what they currently see.
                let name = if stored.trim().is_empty() {
                    self.surface_chip(ctx, workspace_tab_id).label
                } else {
                    stored
                };
                self.renaming_workspace_tab = Some(workspace_tab_id);
                ctx.update_view(
                    &self.workspace_tab_rename_editor,
                    |editor: &mut crate::workspace::InputEditor, editor_ctx| {
                        editor.set_text(&name, editor_ctx);
                        editor.set_mode(crate::workspace::InputMode::RenameWorkspaceTab(
                            workspace_tab_id,
                        ));
                        editor_ctx.focus_self();
                    },
                );
                ctx.notify();
            }
            WorkspaceAction::CommitRenameWorkspaceTab { workspace_tab_id } => {
                let workspace_tab_id = *workspace_tab_id;
                let label = ctx.update_view(
                    &self.workspace_tab_rename_editor,
                    |editor: &mut crate::workspace::InputEditor, editor_ctx| {
                        let text = editor.get_text().trim().to_string();
                        editor.set_text("", editor_ctx);
                        text
                    },
                );
                self.renaming_workspace_tab = None;
                // Clearing the name returns the chip to its live context
                // label; a stored name always wins while set.
                if workspace_tab_id == self.active_workspace_tab_id {
                    self.active_workspace_tab_name = label;
                } else if let Some(tab) = self
                    .parked_workspace_tabs
                    .iter_mut()
                    .find(|tab| tab.id == workspace_tab_id)
                {
                    tab.name = label;
                }
                self.sync_window_title(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::CancelRenameWorkspaceTab => {
                ctx.update_view(
                    &self.workspace_tab_rename_editor,
                    |editor: &mut crate::workspace::InputEditor, editor_ctx| {
                        editor.set_text("", editor_ctx);
                    },
                );
                self.renaming_workspace_tab = None;
                ctx.notify();
            }
            WorkspaceAction::MoveWorkspaceTabTo {
                workspace_tab_id,
                target_workspace_tab_id,
            } => {
                if self.move_workspace_tab_onto(*workspace_tab_id, *target_workspace_tab_id) {
                    self.save_session(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::MoveWorkspaceTabLeft { workspace_tab_id } => {
                self.move_workspace_tab_order(*workspace_tab_id, -1);
                ctx.notify();
            }
            WorkspaceAction::MoveWorkspaceTabRight { workspace_tab_id } => {
                self.move_workspace_tab_order(*workspace_tab_id, 1);
                ctx.notify();
            }
            WorkspaceAction::SetAgentRailExpanded { expanded } => {
                self.agent_rail_expanded = *expanded;
                if *expanded {
                    self.refresh_prompt_agents(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::OpenAgentWithAccount { agent_id, account } => {
                self.agent_account_picker = None;
                self.open_agent_in_new_surface(agent_id.clone(), account, ctx);
            }
            WorkspaceAction::OpenAgentAccountPicker { agent_id } => {
                // Single-identity (System only) and non-isolated agents never
                // see the modal — open directly like opencode and the others
                // without multi-identity support. Safety net for any direct
                // dispatch; the rail already routes these straight through.
                if !super::agent_rail::should_show_account_picker(agent_id) {
                    self.agent_account_picker = None;
                    self.open_agent_in_new_surface(agent_id.clone(), &Default::default(), ctx);
                    return;
                }
                // Toggle: same agent re-click closes. Launch clears on success.
                if self.agent_account_picker.as_ref() == Some(agent_id) {
                    self.agent_account_picker = None;
                } else {
                    self.agent_account_picker = Some(agent_id.clone());
                    // Remote projects list the machine's accounts: refresh
                    // its cache so the picker shows them (loading state
                    // while the first fetch is in flight).
                    if let Some(machine) =
                        self.machine_agents(self.active_project_root().as_deref(), ctx)
                    {
                        self.refresh_machine_accounts(
                            machine.machine_id.clone(),
                            machine.machine.clone(),
                            ctx,
                        );
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::CloseAgentAccountPicker => {
                self.agent_account_picker = None;
                ctx.notify();
            }
            WorkspaceAction::OpenAgentInNewWorkspaceTab { agent_id } => {
                self.agent_account_picker = None;
                self.open_agent_in_new_surface(agent_id.clone(), &Default::default(), ctx);
            }
            _ => {}
        }
    }

    /// Park the live surface and open a fresh one (no panes yet), returning
    /// a cwd hint that follows the previously visible pane. Shared by blank
    /// surfaces and agent launches so both start from the same state.
    pub(crate) fn begin_fresh_surface(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> Option<std::path::PathBuf> {
        if self.closing_worktree_tab.is_some() {
            return None;
        }
        let cwd = (!self.project_start_visible)
            .then(|| {
                self.tabs
                    .iter()
                    .find(|tab| tab.id == self.active_tab_id)
                    .filter(|tab| !tab.pane.is_settings())
                    .map(|tab| tab.pane.cwd(ctx))
                    .filter(|cwd| cwd.is_dir())
            })
            .flatten()
            .unwrap_or_else(default_pane_cwd);
        let owner = self
            .workspaces
            .iter()
            .position(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id));
        self.store_active_workspace_tab();
        let surface_id = self.next_workspace_tab_id;
        self.next_workspace_tab_id += 1;
        self.workspace_tab_order.push(surface_id);
        if let Some(owner) = owner {
            self.workspaces[owner].tab_ids.push(surface_id);
        }
        // No stored name: the chip resolves live (active pane) until
        // the user renames it.
        self.restore_workspace_tab(WorkspaceTab::fresh(surface_id, String::new()));
        self.renaming_workspace_tab = None;
        self.pending_close_workspace_tab = None;
        self.close_workspace_tab_error = None;
        self.new_tab_picker_open = false;
        self.document_center_open = false;
        self.document_terminal_visible = false;
        self.project_start_visible = false;
        if owner.is_none() {
            self.assign_surface_to_workspace(cwd.clone(), "Workspace".into());
        }
        self.refresh_prompt_agents(ctx);
        Some(cwd)
    }

    /// Agent-rail launch: open an installed agent on a brand-new workspace
    /// tab and activate it. The `agent_id` resolves through the agent
    /// registry, so built-ins and user-provided customs share this path:
    /// Dragent uses the built-in agent tab, everything else resolves its
    /// launch command exactly like the project prompt and submits it into a
    /// fresh terminal. Never called for uninstalled agents — the rail
    /// expands instead. The new session is bound to the `agent_id`, which
    /// persists across restarts.
    fn open_agent_in_new_surface(&mut self, agent_id: String, account: &dragent_host::agent_accounts::Selection, ctx: &mut ViewContext<Self>) {
        use super::action::PromptAgent;
        use super::agent_definition::definition_by_id;
        if self.closing_worktree_tab.is_some() {
            return;
        }
        let settings = crate::setup::UiSettings::load();
        let definition = definition_by_id(&agent_id, &settings.custom_agents, false);
        let Some(definition) = definition else {
            return;
        };
        // A remote project starts agents installed on its machine.
        let machine = self.machine_agents(self.active_project_root().as_deref(), ctx);
        let remote_executable = agent_launch_line(&definition, &settings, false)
            .and_then(|line| line.split_whitespace().next().map(str::to_string))
            .unwrap_or_default();
        let installed = match (&machine, definition.builtin) {
            (_, Some(PromptAgent::Dragent)) => true,
            (Some(machine), _) => machine.has(&remote_executable),
            (None, Some(agent)) => self.prompt_installed_agents.contains(&agent),
            (None, None) => self.custom_installed_agents.contains(&agent_id),
        };
        if !installed {
            if let Some(machine) = &machine {
                if let Some(problem) = self.machine_agent_problem(machine, &remote_executable, &definition.name, ctx) {
                    self.project_start_error = Some(problem);
                }
                ctx.notify();
                return;
            }
            // Stale cache race: land on the install action instead of a
            // dead launch.
            self.agent_rail_expanded = true;
            self.refresh_prompt_agents(ctx);
            ctx.notify();
            return;
        }
        let project_root = self.active_project_root();
        if let Some(machine) = machine {
            self.open_agent_on_machine(agent_id, account.clone(), machine, project_root, ctx);
            return;
        }
        let binding = match dragent_host::agent_accounts::Registry::load().and_then(|r| {
            let project = project_root.as_deref().and_then(|p| dragent_host::agent_accounts::project_for(p, &r).or_else(|| Some(p.to_path_buf())));
            r.resolve(&agent_id, project.as_deref(), account)
        }) {
            Ok(binding) => Some(binding),
            Err(error) => { self.show_error_notice("Agent account", error, ctx); return; }
        };
        if definition.builtin == Some(PromptAgent::Dragent) {
            if self.begin_fresh_surface(ctx).is_none() {
                return;
            }
            let new_tab = self.create_agent_tab_at(ctx, project_root);
            new_tab.pane.set_agent_id(ctx, Some(definition.id));
            self.active_tab_id = new_tab.id;
            self.tabs.push(new_tab);
            self.after_agent_surface_created(ctx);
            return;
        }
        // The install check starts a login shell and npm (~300 ms) on this UI
        // thread; an agent the background detection pass already found
        // installed skips it. Anything else is still checked before launch.
        let detected = definition
            .builtin
            .is_some_and(|agent| self.prompt_installed_agents.contains(&agent));
        // Remote machines returned above: this is always a local launch.
        let command = if detected {
            agent_launch_line(&definition, &settings, false)
        } else {
            agent_launch_command(&definition, &settings)
        };
        let label = definition.name.clone();
        let Some(command) = command else {
            self.agent_rail_expanded = true;
            self.refresh_prompt_agents(ctx);
            ctx.notify();
            return;
        };
        let Some(cwd) = self.begin_fresh_surface(ctx) else {
            return;
        };
        let new_tab = self.create_tab_with_cwd(ctx, Some(label), None, cwd, project_root);
        let new_tab_id = new_tab.id;
        if let Some(binding) = binding {
            if let Err(error) = dragent_host::agent_sessions::stage_account(new_tab.pane.pane_uuid(ctx), &binding) {
                self.show_error_notice("Agent account", error, ctx); return;
            }
        }
        new_tab.pane.set_agent_id(ctx, Some(definition.id));
        let terminal = match &new_tab.pane {
            super::pane::WorkspacePane::Terminal(terminal) => Some(terminal.clone()),
            super::pane::WorkspacePane::Settings(_) | super::pane::WorkspacePane::Browser(_) => None,
        };
        self.tabs.push(new_tab);
        self.active_tab_id = new_tab_id;
        if let Some(terminal) = terminal {
            ctx.update_view(&terminal, |terminal, terminal_ctx| {
                terminal.submit_shell_command(&command, terminal_ctx);
            });
        }
        self.after_agent_surface_created(ctx);
    }

    /// Remote launch for the agent rail: the session runs on the active
    /// project's machine, using an account stored on that machine. `System`
    /// launches with the machine's login directly; any other selection asks
    /// the machine for its `LaunchEnv` off the UI thread and prefixes the
    /// remote launch line with it (`Inherit` resolves the machine's own
    /// default there). Failures show an error notice and launch nothing.
    /// No local staging: the env prefix carries the account, and the
    /// conversation lands in that profile's home on the machine, which is
    /// what a later resume reads back.
    fn open_agent_on_machine(
        &mut self,
        agent_id: String,
        account: dragent_host::agent_accounts::Selection,
        machine: super::remote_project::MachineAgents,
        project_root: Option<std::path::PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        use super::action::PromptAgent;
        use super::agent_definition::definition_by_id;
        use dragent_host::development::{
            AccountsRequest, AccountsResponse, DevelopmentRequest, DevelopmentResponse,
        };
        let settings = crate::setup::UiSettings::load();
        let definition = definition_by_id(&agent_id, &settings.custom_agents, false);
        let Some(definition) = definition else {
            return;
        };
        if definition.builtin == Some(PromptAgent::Dragent) {
            if self.begin_fresh_surface(ctx).is_none() {
                return;
            }
            let new_tab = self.create_agent_tab_at(ctx, project_root);
            new_tab.pane.set_agent_id(ctx, Some(definition.id));
            self.active_tab_id = new_tab.id;
            self.tabs.push(new_tab);
            self.after_agent_surface_created(ctx);
            return;
        }
        let Some(base_line) = agent_launch_line(&definition, &settings, false) else {
            self.agent_rail_expanded = true;
            self.refresh_prompt_agents(ctx);
            ctx.notify();
            return;
        };
        let remote_line = machine.launch_line(&base_line);
        let label = definition.name.clone();
        let definition_id = definition.id.clone();
        if matches!(account, dragent_host::agent_accounts::Selection::System) {
            self.submit_remote_agent_command(label, remote_line, project_root, definition_id, ctx);
            return;
        }
        let client = match crate::remote_fs::client(&machine.machine_id) {
            Ok(client) => client,
            Err(error) => {
                self.show_error_notice("Agent account", error, ctx);
                return;
            }
        };
        let provider = agent_id.clone();
        let picker_agent = agent_id.clone();
        ctx.spawn(
            async move {
                // Saved accounts need POSIX shells; Windows reports its OS
                // here, before any launch line is built.
                match client.development(DevelopmentRequest::Environment) {
                    Ok(DevelopmentResponse::Environment { os, .. }) => {
                        let lower = os.to_lowercase();
                        if lower.contains("windows") || lower == "win32" {
                            return Err((
                                true,
                                "Saved accounts aren't supported on Windows machines yet".to_string(),
                            ));
                        }
                    }
                    Err(error) => return Err((false, error)),
                    _ => {}
                }
                match client.development(DevelopmentRequest::Accounts(
                    AccountsRequest::LaunchEnv { provider, selection: account },
                )) {
                    Ok(DevelopmentResponse::Accounts(AccountsResponse::LaunchEnv(env))) => Ok(env),
                    Ok(other) => Err((false, format!("unexpected development response: {other:?}"))),
                    Err(error) => Err((false, error)),
                }
            },
            move |view, result, ctx| {
                match result {
                    Ok(env) => {
                        let command = prefix_remote_launch_line(&remote_line, &env);
                        view.submit_remote_agent_command(
                            label,
                            command,
                            project_root,
                            definition_id,
                            ctx,
                        );
                    }
                    Err((windows, message)) => {
                        if windows {
                            // Fall back to asking for System login.
                            view.agent_account_picker = Some(picker_agent);
                        }
                        view.show_error_notice("Agent account", message, ctx);
                    }
                }
                ctx.notify();
            },
        );
    }

    /// Create the tab for a remote agent launch and submit its (already
    /// account-prefixed) command. Remote accounts are carried by the env
    /// prefix, never by local staging.
    fn submit_remote_agent_command(
        &mut self,
        label: String,
        command: String,
        project_root: Option<std::path::PathBuf>,
        definition_id: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(cwd) = self.begin_fresh_surface(ctx) else {
            return;
        };
        let new_tab = self.create_tab_with_cwd(ctx, Some(label), None, cwd, project_root);
        let new_tab_id = new_tab.id;
        new_tab.pane.set_agent_id(ctx, Some(definition_id));
        let terminal = match &new_tab.pane {
            super::pane::WorkspacePane::Terminal(terminal) => Some(terminal.clone()),
            super::pane::WorkspacePane::Settings(_) | super::pane::WorkspacePane::Browser(_) => None,
        };
        self.tabs.push(new_tab);
        self.active_tab_id = new_tab_id;
        if let Some(terminal) = terminal {
            ctx.update_view(&terminal, |terminal, terminal_ctx| {
                terminal.submit_shell_command(&command, terminal_ctx);
            });
        }
        self.after_agent_surface_created(ctx);
    }

    /// Refresh the cached account list for one remote machine, for the rail
    /// picker. The last good list stays visible while a refresh is in
    /// flight; the picker shows "Loading accounts…" only when empty and
    /// "Couldn't read accounts on {machine}" on error.
    pub(crate) fn refresh_machine_accounts(
        &mut self,
        machine_id: String,
        machine_name: String,
        ctx: &mut ViewContext<Self>,
    ) {
        use dragent_host::development::{
            AccountsRequest, AccountsResponse, DevelopmentRequest, DevelopmentResponse,
        };
        let entry = self
            .machine_account_caches
            .entry(machine_id.clone())
            .or_insert_with(MachineAccountCache::default);
        if entry.loading {
            return;
        }
        entry.loading = true;
        entry.error = None;
        entry.machine_name = machine_name.clone();
        let Ok(client) = crate::remote_fs::client(&machine_id) else {
            let entry = self
                .machine_account_caches
                .entry(machine_id)
                .or_insert_with(MachineAccountCache::default);
            entry.loading = false;
            entry.error = Some("not connected".to_string());
            entry.machine_name = machine_name;
            ctx.notify();
            return;
        };
        ctx.spawn(
            async move {
                match client.development(DevelopmentRequest::Accounts(AccountsRequest::List)) {
                    Ok(DevelopmentResponse::Accounts(AccountsResponse::List {
                        profiles,
                        defaults,
                        ..
                    })) => Ok((profiles, defaults)),
                    Ok(other) => Err(format!("unexpected development response: {other:?}")),
                    Err(error) => Err(error),
                }
            },
            move |view, result, ctx| {
                let entry = view
                    .machine_account_caches
                    .entry(machine_id)
                    .or_insert_with(MachineAccountCache::default);
                entry.loading = false;
                match result {
                    Ok((profiles, defaults)) => {
                        entry.profiles = profiles;
                        entry.defaults = defaults;
                        entry.error = None;
                    }
                    Err(error) => {
                        entry.error = Some(error);
                    }
                }
                entry.machine_name = machine_name;
                ctx.notify();
            },
        );
    }

    /// Shared post-launch sync for agent surfaces. Mirrors the new-tab flow.
    pub(crate) fn after_agent_surface_created(&mut self, ctx: &mut ViewContext<Self>) {
        self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
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

    /// Move a tab onto `target`'s slot in the strip of the workspace that
    /// holds both: after it when moving right, before it when moving left.
    fn move_workspace_tab_onto(&mut self, workspace_tab_id: usize, target: usize) -> bool {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.tab_ids.contains(&workspace_tab_id))
            .is_some_and(|workspace| {
                super::left_panel::move_onto(&mut workspace.tab_ids, &workspace_tab_id, &target)
            })
    }

    fn move_workspace_tab_order(&mut self, workspace_tab_id: usize, delta: isize) {
        let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.tab_ids.contains(&workspace_tab_id))
        else {
            return;
        };
        let Some(pos) = workspace
            .tab_ids
            .iter()
            .position(|id| *id == workspace_tab_id)
        else {
            return;
        };
        let next = if delta < 0 {
            pos.checked_sub(1)
        } else {
            (pos + 1 < workspace.tab_ids.len()).then_some(pos + 1)
        };
        if let Some(next) = next {
            workspace.tab_ids.swap(pos, next);
        }
    }

    /// Shared post-switch/post-close sync: work state, focus, titles,
    /// sidebar, persistence. Mirrors the pane-tab selection flow.
    fn after_surface_layout_changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.reconcile_workspace_ownership();
        self.renaming_workspace_tab = None;
        self.pending_close_workspace_tab = None;
        self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_window_title(ctx);
        refresh_sidebar_projects(self, ctx);
        apply_sidebar_visibility(self, ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// Dispose every pane-tab of a closing surface: host sessions, tracked
    /// foreground commands, agent/pane persistence, surface registration,
    /// parked work. Mirrors `CloseTabConfirmed`'s per-tab disposal without
    /// its worktree removal (roots are cleaned once via
    /// `cleanup_surface_worktrees`). Returns the panes' project roots.
    pub(crate) fn dispose_surface_panes(
        &mut self,
        panes: &[PaneTab],
        ctx: &mut ViewContext<Self>,
    ) -> Vec<std::path::PathBuf> {
        let mut roots = Vec::new();
        for tab in panes {
            if self.host_mirror_tab_id == Some(tab.id) {
                self.host_mirror_pane = None;
                self.host_mirror_tab_id = None;
                self.host_mirror = None;
            }
            if !tab.pane.is_settings() {
                let pane_uuid = tab.pane.pane_uuid(ctx);
                if let Some(root) = &tab.project_root {
                    roots.push(root.clone());
                }
                crate::confirm::set_foreground_command(pane_uuid, None);
                self.close_host_tab_session(tab.id);
                crate::agent::session_facts::clear_conversation(pane_uuid);
                if tab.project_root.as_ref().is_none_or(|root| root.is_dir()) {
                    persistence::clear_pane_data(
                        &self.persistence_writer,
                        pane_uuid,
                        tab.project_root.clone(),
                    );
                    persistence::clear_agent_blocks(
                        &self.persistence_writer,
                        pane_uuid,
                        tab.project_root.clone(),
                    );
                }
            }
            self.remove_tab_from_split(tab.id);
            self.release_surface_if_unplaced(tab.surface_id);
            if let Some(work_item_id) = tab.work_item_id {
                self.parked_workspace_work.remove(&work_item_id);
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }

    /// Remove managed worktree checkouts no other surface references anymore.
    /// Same `git worktree remove` as the pane-tab flow; failures surface on
    /// the header strip instead of being lost.
    fn cleanup_surface_worktrees(
        &mut self,
        roots: &[std::path::PathBuf],
        ctx: &mut ViewContext<Self>,
    ) {
        for root in roots {
            // Owned: the async removal below must be 'static.
            let root = root.clone();
            let Some(main) = worktree_main_checkout(&root) else {
                continue;
            };
            let still_used = self
                .tabs
                .iter()
                .any(|tab| tab.project_root.as_deref() == Some(&root))
                || self.parked_workspace_tabs.iter().any(|surface| {
                    surface
                        .panes
                        .iter()
                        .any(|tab| tab.project_root.as_deref() == Some(&root))
                });
            if still_used {
                continue;
            }
            let checkout = main.clone();
            let removed_project = root.clone();
            let data = super::project_data::ProjectData::locate(&root);
            ctx.spawn(
                async move { crate::git_status::worktree::force_remove_worktree(&main, &root) },
                move |view, result: Result<(), String>, ctx| match result {
                    Ok(()) => {
                        data.forget(&view.persistence_writer);
                        view.persistence_writer.delete_workspace_project(
                            dragent_workspace_model::Project::new_for_organisation(
                                &view.organisation_id,
                                removed_project.clone(),
                                0,
                            )
                            .id,
                        );
                        view.open_project_roots
                            .retain(|open| open != &removed_project);
                        if view.prompt_selected_worktree.as_ref() == Some(&removed_project) {
                            view.prompt_selected_worktree = None;
                            view.prompt_branch = None;
                        }
                        view.save_session(ctx);
                        ctx.notify();
                    }
                    Err(error) => {
                        // The workspace has already left the window, so
                        // nothing else would say its folder is still there.
                        view.show_error_notice(
                            "Couldn’t delete workspace folder",
                            format!("{}\n{error}", removed_project.display()),
                            ctx,
                        );
                        view.close_workspace_tab_error = Some(error);
                        ctx.notify();
                    }
                },
            );
        }
    }
}

/// Launch command for an agent definition, resolved exactly like the rail
/// and the project prompt resolve it. `None` when it is not configured.
pub(crate) fn agent_launch_command(
    definition: &super::agent_definition::AgentDefinition,
    settings: &crate::setup::UiSettings,
) -> Option<String> {
    agent_launch_line(definition, settings, true)
}

/// The launch line for an agent definition. `local`: a built-in must be
/// installed on this computer (not checked for a remote project, whose
/// machine decides).
pub(crate) fn agent_launch_line(
    definition: &super::agent_definition::AgentDefinition,
    settings: &crate::setup::UiSettings,
    local: bool,
) -> Option<String> {
    match definition.builtin {
        Some(agent) => {
            let configured = settings
                .agent_command_configs
                .contains_key(agent.command_key())
                || settings.agent_commands.contains_key(agent.command_key());
            if local && !configured && super::action_projects::prompt_agent_launch_command(agent).is_none() {
                return None;
            }
            let config = super::action_projects::agent_command_config(agent, settings);
            config
                .validate()
                .ok()
                .map(|_| config.command.trim().to_string())
        }
        None => settings
            .custom_agents
            .iter()
            .find(|custom| super::agent_definition::custom_id(&custom.id) == definition.id)
            .filter(|custom| custom.launch.validate().is_ok())
            .map(|custom| custom.launch.command.trim().to_string())
            .filter(|command| !command.is_empty()),
    }
}

/// Prefix a machine-resolved remote launch line with an account's
/// environment. `System` resolves to an empty env, so the line is unchanged;
/// `Inherit` and `Profile` carry the machine's `LaunchEnv` for that
/// selection (the machine resolves `Inherit` to its own default).
pub(crate) fn prefix_remote_launch_line(
    remote_line: &str,
    env: &dragent_host::agent_accounts::LaunchEnv,
) -> String {
    format!("{}{}", dragent_host::agent_accounts::posix_prefix(env), remote_line)
}

impl WorkspaceView {
    /// `+` → Team: the prompt screen of the project in front, Team switched
    /// on; without a project, choose one first.
    pub(crate) fn open_team_prompt(&mut self, ctx: &mut ViewContext<Self>) {
        self.new_tab_picker_open = false;
        let root = self
            .active_project_root()
            .map(|root| self.project_root_of(&root))
            .or_else(|| self.selected_project_root())
            .filter(|root| root.is_dir());
        match root {
            Some(root) => self.handle_action(&WorkspaceAction::OpenProjectWorkspacePrompt { root }, ctx),
            None => self.choose_project_folder(super::ProjectFolderIntent::OpenFolder, ctx),
        }
        self.prompt_team = true;
        ctx.notify();
    }

    /// Open a new top-level workspace tab (the current surface parks) with
    /// one terminal; with `command`, it runs there once the shell is up.
    /// `+` → Work item: a new top-level tab holding a fresh work item, the
    /// same way Terminal and Dragent CLI open theirs. The tab you were in is
    /// parked as it was: its panes, splits and editor are left alone.
    pub(crate) fn open_new_work_item_tab(&mut self, ctx: &mut ViewContext<Self>) {
        self.new_tab_picker_open = false;
        if self.closing_worktree_tab.is_some() {
            return;
        }
        let project_root = self
            .active_project_root()
            .or_else(|| self.selected_project_root())
            .filter(|root| root.is_dir());
        let Some(cwd) = self.begin_fresh_surface(ctx) else {
            return;
        };
        let cwd = project_root.clone().unwrap_or(cwd);
        let new_tab = self.create_tab_with_cwd(
            ctx,
            Some(super::view::NEW_WORK_LABEL.to_string()),
            None,
            cwd,
            project_root,
        );
        self.active_tab_id = new_tab.id;
        self.tabs.push(new_tab);
        self.project_start_visible = false;
        self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        self.open_work_item_tab(super::WorkItemView::Conversation, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_window_title(ctx);
        ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
            lp.sidebar_menu_tab_id = None;
            lp_ctx.notify();
        });
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
            editor.request_focus(editor_ctx);
        });
        ctx.notify();
    }

    pub(crate) fn open_new_workspace_tab(
        &mut self,
        label: Option<&str>,
        command: Option<&str>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.open_new_workspace_tab_in(label, command, None, ctx);
    }

    /// [`Self::open_new_workspace_tab`] in `shell`, or the default shell.
    pub(crate) fn open_new_workspace_tab_in(
        &mut self,
        label: Option<&str>,
        command: Option<&str>,
        shell: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.new_tab_picker_open = false;
        if self.closing_worktree_tab.is_some() {
            return;
        }
        // Follow the live pane's directory like a fresh pane-tab does,
        // before the current surface parks.
        let project_root = self.active_project_root();
        let Some(cwd) = self.begin_fresh_surface(ctx) else {
            return;
        };
        if let Some(label) = label {
            // Pane labels and header surface names are stored separately.
            self.active_workspace_tab_name = label.to_string();
        }
        let new_tab =
            self.create_tab_with_cwd(ctx, label.map(str::to_string), shell, cwd.clone(), project_root);
        if let (Some(command), WorkspacePane::Terminal(terminal)) = (command, &new_tab.pane) {
            // Local backend selection (Hosts settings): WSL wraps the shell
            // command so local agent runs execute in the chosen distro.
            let remote = crate::remote_fs::resolve(&cwd).is_some();
            let command = if remote {
                command.to_string()
            } else {
                crate::settings::settings_tasks_hosts::wrap_local_command(command)
            };
            ctx.update_view(terminal, |terminal, terminal_ctx| {
                terminal.submit_shell_command(&command, terminal_ctx)
            });
        }
        self.active_tab_id = new_tab.id;
        self.tabs.push(new_tab);
        self.project_start_visible = false;
        self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
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
}

#[cfg(test)]
mod remote_account_launch_tests {
    use super::prefix_remote_launch_line;
    use dragent_host::agent_accounts::LaunchEnv;

    #[test]
    fn system_selection_leaves_the_remote_line_unchanged() {
        // `System` resolves to an empty env: no prefix.
        assert_eq!(
            prefix_remote_launch_line("claude --dangerously-skip-permissions", &LaunchEnv::default()),
            "claude --dangerously-skip-permissions"
        );
        assert_eq!(
            prefix_remote_launch_line("'/opt/claude beta/bin/claude' --flag", &LaunchEnv::default()),
            "'/opt/claude beta/bin/claude' --flag"
        );
    }

    #[test]
    fn inherit_selection_prefixes_the_machine_default_env() {
        // `Inherit` asks the machine for `LaunchEnv` with `Inherit`, which
        // resolves its own default there; the returned env prefixes the
        // machine-resolved line (chosen copy included).
        let env = LaunchEnv {
            set: vec![("CLAUDE_CONFIG_DIR".into(), "/home/dev/.p/a'b".into())],
            unset: vec!["ANTHROPIC_API_KEY".into()],
        };
        assert_eq!(
            prefix_remote_launch_line("'/opt/claude' --flag", &env),
            "env -u ANTHROPIC_API_KEY CLAUDE_CONFIG_DIR='/home/dev/.p/a'\\''b' '/opt/claude' --flag"
        );
    }

    #[test]
    fn profile_selection_prefixes_set_and_unset_together() {
        let env = LaunchEnv {
            set: vec![("CODEX_HOME".into(), "/home/dev/.p/codex".into())],
            unset: vec!["OPENAI_API_KEY".into(), "CODEX_API_KEY".into()],
        };
        assert_eq!(
            prefix_remote_launch_line("npx -y @openai/codex", &env),
            "env -u OPENAI_API_KEY -u CODEX_API_KEY CODEX_HOME='/home/dev/.p/codex' npx -y @openai/codex"
        );
        // Empty env (System) never prefixes, even with replacements.
        assert_eq!(
            prefix_remote_launch_line("npx -y @openai/codex", &LaunchEnv::default()),
            "npx -y @openai/codex"
        );
    }
}
