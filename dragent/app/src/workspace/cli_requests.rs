//! Requests from the `dragent` command line, done in this window.

use std::path::{Path, PathBuf};

use warpui::{SingletonEntity, TypedActionView, UpdateModel, UpdateView, ViewContext};

use super::pane::WorkspacePane;
use super::project_workspace_creation::{self, WorkspaceNames};
use super::view::WorkspaceView;
use super::{ProjectFolderIntent, PromptAgent, WorkspaceAction};
use crate::cli::inbox::CliInbox;
use crate::cli::requests::{Request, Response};

/// `dragent workspaces create`, as the window gets it.
struct NewWorkspace {
    project: PathBuf,
    agent: Option<PromptAgent>,
    prompt: String,
    branch: Option<String>,
    base_branch: Option<String>,
    name: Option<String>,
    pull_request: Option<u32>,
    local: bool,
    command: Option<String>,
    tags: Vec<String>,
    model: Option<String>,
    background: bool,
}

impl WorkspaceView {
    /// A command-line request is waiting; the first window to take it does it.
    pub(crate) fn on_cli_request(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        let inbox = CliInbox::handle(ctx);
        let Some(request) = ctx.update_model(&inbox, |inbox, _| inbox.take(id)) else {
            return;
        };
        // The command line only works signed in. `whoami` answers either way.
        if !matches!(request, Request::Whoami)
            && !crate::auth::AuthManager::as_ref(ctx).is_logged_in()
        {
            return CliInbox::respond(
                id,
                &Response::failure(
                    dragent_host::outcome::Failure::new(
                        dragent_host::outcome::Status::NeedsAuth,
                        "signed_out",
                        crate::cli::SIGN_IN_FIRST,
                    )
                    .next("Ask the person to sign in to Dragent, then run the command again."),
                ),
            );
        }
        let answer = match request {
            Request::Open { path } => self.open_for_cli(path, ctx),
            Request::CreateWorkspace {
                project,
                agent,
                prompt,
                branch,
                base_branch,
                name,
                pull_request,
                local,
                command,
                tags,
                model,
                background,
            } => {
                match self.cli_agent(agent.as_deref(), &prompt) {
                    Ok(agent) => self.create_workspace_for_cli(
                        id.to_string(),
                        NewWorkspace {
                            project,
                            agent,
                            prompt,
                            branch,
                            base_branch,
                            name,
                            pull_request,
                            local,
                            command,
                            tags,
                            model,
                            background,
                        },
                        ctx,
                    ),
                    Err(error) => CliInbox::respond(id, &Response::failed(error)),
                }
                // Answered when the worktree is ready.
                return;
            }
            Request::StartAgent {
                account,
                workspace,
                agent,
                prompt,
                model,
            } => self.start_agent_for_cli(
                &workspace,
                agent.as_deref(),
                &prompt,
                model.as_deref(),
                &account,
                ctx,
            ),
            Request::DeleteWorkspace { path } => self.delete_workspace_for_cli(&path, ctx),
            Request::Organisations => self.organisations_for_cli(ctx),
            Request::SwitchOrganisation { organisation } => {
                self.switch_organisation_for_cli(&organisation, ctx)
            }
            Request::Whoami => self.whoami_for_cli(ctx),
            Request::Machines => machines_for_cli(ctx),
            Request::PluginServers {
                upsert,
                remove,
                revoke,
            } => {
                // Through the settings view, which owns the server list, so
                // an open Settings page never writes an older one back.
                let action = crate::settings::SettingsAction::ApplyPluginServers {
                    upsert,
                    remove,
                    revoke,
                };
                let settings = self.settings_view.clone();
                ctx.update_view(&settings, |view, vctx| view.handle_action(&action, vctx));
                Response::done()
            }
            Request::PluginServerStatus { names } => plugin_server_status(&names),
            Request::PluginCheck {
                server,
                tool,
                arguments,
            } => {
                // Answered when the server replies, off the main thread.
                let id = id.to_string();
                std::thread::spawn(move || {
                    let name = format!(
                        "mcp__{}__{}",
                        server.replace('-', "_"),
                        tool.replace('-', "_")
                    );
                    let (output, failed) =
                        crate::agent::mcp::call_tool(&name, &arguments.to_string());
                    let output: String = output.chars().take(4_000).collect();
                    CliInbox::respond(
                        &id,
                        &Response::with_data(
                            serde_json::json!({ "output": output, "failed": failed }),
                        ),
                    );
                });
                return;
            }
            Request::Setting { key, value } => self.setting_for_cli(&key, value.as_deref(), ctx),
            Request::Feedback { text } => {
                // Answered when the server replies.
                self.feedback_for_cli(id.to_string(), text, ctx);
                return;
            }
            Request::IntegrationTool {
                tool,
                arguments,
                root,
            } => {
                // Answered when the service replies, off the main thread.
                let id = id.to_string();
                std::thread::spawn(move || {
                    let answer = match crate::integrations::tools::call(&tool, &arguments, &root) {
                        Ok(output) => Response::with_data(serde_json::json!({ "output": output })),
                        Err(error) => Response::failed(error),
                    };
                    CliInbox::respond(&id, &answer);
                });
                return;
            }
            Request::WorkTool {
                pane,
                terminal,
                tool,
                arguments,
            } => match self.work_tool_for_cli(
                id,
                pane.as_deref(),
                terminal.as_deref(),
                &tool,
                &arguments,
                ctx,
            ) {
                Some(answer) => answer,
                // A question for the lead: answered when it replies.
                None => return,
            },
            Request::ComputerTool {
                pane,
                terminal,
                tool,
                arguments,
            } => {
                match self.computer_tool_for_cli(
                    id,
                    pane.as_deref(),
                    terminal.as_deref(),
                    &tool,
                    &arguments,
                    ctx,
                ) {
                    Some(answer) => answer,
                    None => return,
                }
            }
            Request::BrowserTool {
                pane,
                terminal,
                tool,
                arguments,
            } => match self.browser_tool_for_cli(
                id,
                pane.as_deref(),
                terminal.as_deref(),
                &tool,
                &arguments,
                ctx,
            ) {
                Some(answer) => answer,
                // Answered once the page has done it.
                None => return,
            },
            Request::OpenLink { url } => {
                self.open_link(&url, ctx);
                Response::done()
            }
            Request::PaneTool {
                pane,
                terminal,
                tool,
                arguments,
            } => {
                self.pane_tool_for_cli(pane.as_deref(), terminal.as_deref(), &tool, &arguments, ctx)
            }
            Request::StopAgent { pane } => {
                let closed =
                    uuid::Uuid::parse_str(&pane).is_ok_and(|pane| self.close_agent_tab(pane, ctx));
                Response::with_data(serde_json::json!({ "tab_closed": closed }))
            }
        };
        CliInbox::respond(id, &answer);
    }

    /// The agent a request names: `--agent`, else the prompt screen's
    /// choice when there's a prompt, else none (a plain terminal).
    fn cli_agent(&self, key: Option<&str>, prompt: &str) -> Result<Option<PromptAgent>, String> {
        match key {
            Some(key) => crate::automations::executor::prompt_agent(key)
                .map(Some)
                .ok_or_else(|| format!("Unknown agent “{key}”.")),
            None if prompt.trim().is_empty() => Ok(None),
            None => Ok(Some(self.prompt_agent)),
        }
    }

    /// `dragent open`: the folder's workspace if the sidebar has one, else
    /// the folder as a project (through the usual trust prompt), in front.
    fn open_for_cli(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) -> Response {
        if let Some(workspace_id) = self.workspace_for(&path) {
            self.handle_action(
                &WorkspaceAction::SelectProjectWorkspace { workspace_id },
                ctx,
            );
        } else if path.is_dir() {
            self.receive_project_folder(path.clone(), ProjectFolderIntent::OpenFolder, ctx);
        } else {
            return Response::failed(format!("{} isn't a folder.", path.display()));
        }
        let window_id = ctx.window_id();
        ctx.windows().show_window_and_focus_app(window_id);
        Response {
            ok: true,
            workspace: Some(path),
            ..Response::default()
        }
    }

    /// `dragent workspaces create`: a worktree (from a branch, a new branch
    /// or a pull request), or the project's own checkout with `local`.
    fn create_workspace_for_cli(
        &mut self,
        id: String,
        new: NewWorkspace,
        ctx: &mut ViewContext<Self>,
    ) {
        let fail = |id: &str, error: String| CliInbox::respond(id, &Response::failed(error));
        if new.background && new.command.is_some() {
            return fail(
                &id,
                "A workspace opened in the background can't run a command yet.".into(),
            );
        }
        if let Some(agent) = new.agent {
            if let Err(error) = Self::agent_launch(agent, new.model.as_deref()) {
                return fail(&id, error);
            }
        }
        if crate::remote_fs::resolve(&new.project).is_some() {
            return fail(
                &id,
                "Projects on other machines can't get workspaces from the command line yet.".into(),
            );
        }
        if new.local {
            let root = new
                .project
                .canonicalize()
                .unwrap_or_else(|_| new.project.clone());
            let branch = super::project_naming::local_branch_name(&root);
            let title = new
                .name
                .clone()
                .or_else(|| branch.clone())
                .unwrap_or_else(|| super::project_naming::worktree_project_name(&root));
            self.finish_cli_workspace(id, root.clone(), root, branch, title, new, ctx);
            return;
        }
        let Some(repo) = super::project_sidebar::detect::find_git_root(&new.project) else {
            return fail(
                &id,
                format!("{} isn't a git repository.", new.project.display()),
            );
        };
        let naming_prompt = new.prompt.clone();
        let naming_agent = new.agent.unwrap_or(self.prompt_agent);
        let branch = new.branch.clone();
        let base_branch = new.base_branch.clone();
        let pull_request = new.pull_request;
        let source = repo.clone();
        ctx.spawn(
            async move {
                let names = match (pull_request, branch) {
                    (_, Some(branch)) => WorkspaceNames {
                        title: branch.clone(),
                        branch,
                    },
                    (Some(number), None) => WorkspaceNames {
                        title: format!("PR #{number}"),
                        branch: format!("pr-{number}"),
                    },
                    (None, None) => {
                        project_workspace_creation::generate_names(&naming_prompt, naming_agent)
                            .await
                    }
                };
                let base = source.clone();
                let start = tokio::task::spawn_blocking(move || match pull_request {
                    Some(number) => fetch_pull_request(&base, number),
                    // A commit id is an exact start point (a team worker's
                    // snapshot): no base override or upstream applies.
                    None => match base_branch.as_deref() {
                        Some(commit) if is_commit_id(commit) => Ok(commit.to_string()),
                        selected => project_workspace_creation::latest_start_point(&base, selected),
                    },
                })
                .await
                .map_err(|error| format!("Could not prepare worktree: {error}"))??;
                let branch = names.branch.clone();
                let (path, branch) = tokio::task::spawn_blocking(move || {
                    project_workspace_creation::create_worktree(&source, &branch, &start)
                })
                .await
                .map_err(|error| format!("Could not create worktree: {error}"))??;
                Ok::<_, String>((path, branch, names.title))
            },
            move |view, result, ctx| match result {
                Ok((path, branch, title)) => {
                    let title = new.name.clone().unwrap_or(title);
                    super::workspace_setup::maybe_init_worktree(&path);
                    view.finish_cli_workspace(id, repo, path, Some(branch), title, new, ctx);
                }
                Err(error) => CliInbox::respond(&id, &Response::failed(error)),
            },
        );
    }

    /// Open the new workspace with its agent (or terminal), the extra
    /// command and the sidebar folders, then answer.
    #[allow(clippy::too_many_arguments)]
    fn finish_cli_workspace(
        &mut self,
        id: String,
        project: PathBuf,
        root: PathBuf,
        branch: Option<String>,
        title: String,
        new: NewWorkspace,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.open_project_roots.contains(&project) {
            self.open_project_roots.push(project.clone());
        }
        let opened = if new.background {
            // In the sidebar, its agent at work, without leaving what's in front.
            self.agent_tab(
                root.clone(),
                title.clone(),
                new.agent,
                &new.prompt,
                new.model.as_deref(),
                false,
                &dragent_host::agent_accounts::Selection::Inherit,
                ctx,
            )
            .map(|tab| {
                let pane = tab.pane.pane_uuid(ctx);
                self.park_new_surface(root.clone(), title.clone(), tab);
                pane
            })
        } else {
            self.open_agent_surface(
                root.clone(),
                title.clone(),
                new.agent,
                &new.prompt,
                new.model.as_deref(),
                ctx,
            )
        };
        let pane = match opened {
            Ok(pane) => pane,
            Err(error) => return CliInbox::respond(&id, &Response::failed(error)),
        };
        if let Some(command) = new
            .command
            .as_deref()
            .filter(|command| !command.trim().is_empty())
        {
            match new.agent {
                // Its own tab, so the agent's stays in front.
                Some(_) => {
                    let tab = self.create_tab_with_cwd_focus(
                        ctx,
                        Some(command.to_string()),
                        None,
                        root.clone(),
                        Some(root.clone()),
                        false,
                    );
                    if let WorkspacePane::Terminal(terminal) = &tab.pane {
                        let command = command.to_string();
                        ctx.update_view(terminal, |terminal, tctx| {
                            terminal.submit_shell_command(&command, tctx)
                        });
                    }
                    self.tabs.push(tab);
                }
                // The workspace's only terminal runs it.
                None => {
                    let terminal = self.tabs.iter().find_map(|tab| match &tab.pane {
                        WorkspacePane::Terminal(terminal) if tab.pane.pane_uuid(ctx) == pane => {
                            Some(terminal.clone())
                        }
                        _ => None,
                    });
                    if let Some(terminal) = terminal {
                        let command = command.to_string();
                        ctx.update_view(&terminal, |terminal, tctx| {
                            terminal.submit_shell_command(&command, tctx)
                        });
                    }
                }
            }
        }
        if !new.tags.is_empty() {
            self.file_into_folder(&root, &project, &new.tags);
        }
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        CliInbox::respond(
            &id,
            &Response {
                ok: true,
                workspace: Some(root),
                branch,
                title: Some(title),
                agent: new.agent.map(|agent| agent.command_key().to_string()),
                agent_id: new.agent.map(|_| pane.to_string()),
                ..Response::default()
            },
        );
    }

    /// `dragent agents start`: another agent in a workspace already in the
    /// sidebar, in a tab of its own, without switching to it.
    fn start_agent_for_cli(
        &mut self,
        workspace: &Path,
        agent: Option<&str>,
        prompt: &str,
        model: Option<&str>,
        account: &dragent_host::agent_accounts::Selection,
        ctx: &mut ViewContext<Self>,
    ) -> Response {
        let Some(workspace_id) = self.workspace_for(workspace) else {
            return Response::failed(format!(
                "{} isn't a workspace in Dragent.",
                workspace.display()
            ));
        };
        let agent = match self.cli_agent(agent, prompt) {
            Ok(Some(agent)) => agent,
            Ok(None) => return Response::failed("What should the agent do? Pass a prompt."),
            Err(error) => return Response::failed(error),
        };
        let root = self
            .workspaces
            .iter()
            .find(|known| known.id == workspace_id)
            .map(|known| (known.root.clone(), known.title.clone()));
        let Some((root, title)) = root else {
            return Response::failed("That workspace closed.");
        };
        match self.add_agent_surface(
            workspace_id,
            root.clone(),
            agent,
            prompt,
            model,
            account,
            ctx,
        ) {
            Ok(pane) => Response {
                ok: true,
                workspace: Some(root),
                title: Some(title),
                agent: Some(agent.command_key().to_string()),
                agent_id: Some(pane.to_string()),
                ..Response::default()
            },
            Err(error) => Response::failed(error),
        }
    }

    /// `dragent agents stop`: close the tab showing pane `pane` (ending its
    /// terminal) without switching surfaces. Never removes a worktree.
    /// `false` when no tab in this window shows it.
    pub(crate) fn close_agent_tab(
        &mut self,
        pane: uuid::Uuid,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let shows = |tab: &super::view::PaneTab, ctx: &ViewContext<Self>| {
            !tab.pane.is_settings() && tab.pane.pane_uuid(ctx) == pane
        };
        if let Some(tab_id) = self
            .tabs
            .iter()
            .find(|tab| shows(tab, ctx))
            .map(|tab| tab.id)
        {
            // A pending close is what removes a worktree with its last tab.
            if self
                .pending_close_tab
                .as_ref()
                .is_some_and(|(id, _)| *id == tab_id)
            {
                self.pending_close_tab = None;
            }
            self.handle_action(&WorkspaceAction::CloseTabConfirmed { tab_id }, ctx);
            return true;
        }
        let Some((position, index)) =
            self.parked_workspace_tabs
                .iter()
                .enumerate()
                .find_map(|(position, surface)| {
                    surface
                        .panes
                        .iter()
                        .position(|tab| shows(tab, ctx))
                        .map(|index| (position, index))
                })
        else {
            return false;
        };
        let surface = &mut self.parked_workspace_tabs[position];
        let tab = surface.panes.remove(index);
        if surface.panes.is_empty() {
            // Its only tab: the surface goes, and a workspace left without
            // surfaces leaves the sidebar.
            let surface_id = surface.id;
            self.parked_workspace_tabs.remove(position);
            self.workspace_tab_order.retain(|id| *id != surface_id);
            for workspace in &mut self.workspaces {
                workspace.tab_ids.retain(|id| *id != surface_id);
                if workspace.active_tab_id == surface_id {
                    if let Some(&last) = workspace.tab_ids.last() {
                        workspace.active_tab_id = last;
                    }
                }
            }
            self.workspaces
                .retain(|workspace| !workspace.tab_ids.is_empty());
        } else {
            if let Some(tree) = surface.split_tree.as_mut() {
                tree.close_pane(tab.id);
                if tree.is_single() {
                    surface.split_tree = None;
                }
            }
            if surface.active_tab_id == tab.id {
                surface.active_tab_id = surface.panes[0].id;
                surface.active_pane_id = Some(surface.panes[0].pane.id());
            }
        }
        self.dispose_surface_panes(std::slice::from_ref(&tab), ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
        true
    }

    /// `dragent workspaces delete`: what confirming the close dialog does
    /// (the command line already checked for unsaved work and agents).
    pub(super) fn delete_workspace_for_cli(
        &mut self,
        path: &Path,
        ctx: &mut ViewContext<Self>,
    ) -> Response {
        let Some(workspace_id) = self.workspace_for(path) else {
            return Response::failed(format!("{} isn't a workspace in Dragent.", path.display()));
        };
        let Some(active_tab_id) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.active_tab_id)
        else {
            return Response::failed("That workspace closed.");
        };
        self.pending_close_project_workspace = Some(workspace_id);
        self.handle_action(
            &WorkspaceAction::CloseWorkspaceTabConfirmed {
                workspace_tab_id: active_tab_id,
            },
            ctx,
        );
        Response {
            ok: true,
            workspace: Some(path.to_path_buf()),
            ..Response::default()
        }
    }

    fn organisations_for_cli(&mut self, ctx: &mut ViewContext<Self>) -> Response {
        let store = crate::settings::OrgStore::as_ref(ctx);
        let Some(organisations) = store.organizations.clone() else {
            return Response::failed(
                "Dragent hasn't loaded your organisations yet. Try again in a moment.",
            );
        };
        let active = store
            .active
            .as_ref()
            .map(|organisation| organisation.id.clone());
        Response::with_data(serde_json::Value::Array(
            organisations
                .iter()
                .map(|organisation| {
                    serde_json::json!({
                        "id": organisation.id,
                        "name": organisation.name,
                        "slug": organisation.slug,
                        "role": organisation.role,
                        "active": active.as_deref() == Some(organisation.id.as_str()),
                    })
                })
                .collect(),
        ))
    }

    fn switch_organisation_for_cli(
        &mut self,
        wanted: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Response {
        let store = crate::settings::OrgStore::as_ref(ctx);
        let Some(organisations) = store.organizations.clone() else {
            return Response::failed(
                "Dragent hasn't loaded your organisations yet. Try again in a moment.",
            );
        };
        let matches: Vec<_> = organisations
            .iter()
            .filter(|organisation| {
                organisation.id == wanted
                    || organisation.slug.eq_ignore_ascii_case(wanted)
                    || organisation.name.eq_ignore_ascii_case(wanted)
            })
            .collect();
        let organisation = match matches.as_slice() {
            [one] => (*one).clone(),
            [] => {
                return Response::failed(format!(
                    "You aren't in an organisation called “{wanted}”."
                ))
            }
            _ => {
                return Response::failed(format!(
                    "Several organisations are called “{wanted}”; use its id."
                ))
            }
        };
        let already = store
            .active
            .as_ref()
            .is_some_and(|active| active.id == organisation.id);
        if !already {
            self.handle_action(
                &WorkspaceAction::SwitchOrganisation {
                    id: organisation.id.clone(),
                },
                ctx,
            );
        }
        Response::with_data(serde_json::json!({ "id": organisation.id, "name": organisation.name }))
    }

    fn whoami_for_cli(&mut self, ctx: &mut ViewContext<Self>) -> Response {
        if crate::api::is_local_mode() {
            return Response::with_data(crate::api::local_mode::identity_json());
        }
        let auth = crate::auth::AuthManager::as_ref(ctx);
        if !auth.is_logged_in() {
            return Response::with_data(serde_json::json!({ "signed_in": false }));
        }
        let Some(data) = crate::settings::AccountStore::as_ref(ctx).data.clone() else {
            // Account details load separately from authentication. Missing
            // profile data does not mean that the user has signed out.
            let user_id = match auth.status() {
                crate::auth::AuthStatus::LoggedIn(user) => user.user_id.clone(),
                _ => None,
            };
            let organisation = crate::settings::OrgStore::as_ref(ctx).active.as_ref();
            return Response::with_data(serde_json::json!({
                "signed_in": true,
                "user_id": user_id,
                "organisation": organisation.map(|org| &org.name),
                "organisation_id": organisation.map(|org| &org.id),
            }));
        };
        let organisation = crate::settings::OrgStore::as_ref(ctx).active.clone();
        let user = data.user;
        let name = if user.display_name.is_empty() {
            format!("{} {}", user.first_name, user.last_name)
                .trim()
                .to_string()
        } else {
            user.display_name.clone()
        };
        Response::with_data(serde_json::json!({
            "signed_in": true,
            "user_id": user.id,
            "name": name,
            "email": user.email,
            "plan": user.tier,
            "organisation": organisation.as_ref().map(|organisation| organisation.name.clone()),
            "organisation_id": organisation.as_ref().map(|organisation| organisation.id.clone()),
        }))
    }

    /// `dragent settings set`: the Settings page's own action, so the
    /// change applies everywhere and is saved the usual way.
    fn setting_for_cli(
        &mut self,
        key: &str,
        value: Option<&str>,
        ctx: &mut ViewContext<Self>,
    ) -> Response {
        let editor = crate::console::editor_settings::EditorSettings::global();
        match crate::cli::settings::action_for(key, value, &editor) {
            Ok(Some(action)) => {
                let settings = self.settings_view.clone();
                ctx.update_view(&settings, |view, vctx| view.handle_action(&action, vctx));
                Response::done()
            }
            Ok(None) => Response::done(),
            Err(error) => Response::failed(error),
        }
    }

    /// `dragent feedback`: posted as the signed-in user, like the Feedback
    /// window; answered when the server replies.
    fn feedback_for_cli(&mut self, id: String, text: String, ctx: &mut ViewContext<Self>) {
        if !crate::auth::AuthManager::as_ref(ctx).is_logged_in() {
            return CliInbox::respond(
                &id,
                &Response::failed("Sign in to Dragent to send feedback."),
            );
        }
        let account = crate::settings::AccountStore::as_ref(ctx)
            .data
            .as_ref()
            .map(|data| data.user.email.clone())
            .unwrap_or_default();
        let organisation = crate::settings::OrgStore::as_ref(ctx)
            .active
            .as_ref()
            .map(|organisation| format!("{} ({})", organisation.name, organisation.id))
            .unwrap_or_default();
        let content = super::feedback::build_content(
            &format!("{text}\n\n(Sent with `dragent feedback`.)"),
            "",
            &account,
            &organisation,
            env!("CARGO_PKG_VERSION"),
            &super::feedback::platform_name(),
            false,
        );
        crate::bg::detach(move || {
            let parts = vec![crate::api::MultipartPart::Field {
                name: "content".into(),
                value: content,
            }];
            let response =
                match crate::api::API_CLIENT.post_multipart("/api/v1/feedback", &parts, 60) {
                    Ok(_) => Response::done(),
                    Err(error) => Response::failed(error.user_message()),
                };
            CliInbox::respond(&id, &response);
        });
    }
}

/// Fetch pull request `number`'s head from `origin`; returns the ref to
/// start a worktree from.
fn fetch_pull_request(repo: &Path, number: u32) -> Result<String, String> {
    use crate::process_ext::NoWindow;
    let target = format!("refs/remotes/origin/pr/{number}");
    let output = crate::git_cmd::GitCommand::new()
        .args([
            "fetch",
            "--quiet",
            "origin",
            &format!("+pull/{number}/head:{target}"),
        ])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .map_err(|error| format!("Could not fetch pull request #{number}: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "Could not fetch pull request #{number} from origin{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(format!("origin/pr/{number}"))
}

/// A full commit id (SHA-1 or SHA-256), as opposed to a branch name.
fn is_commit_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `dragent machines`: each saved machine as the app sees it now. Reads
/// state only: it never connects, or reconnects one the person disconnected.
fn machines_for_cli(ctx: &mut ViewContext<WorkspaceView>) -> Response {
    use crate::settings::machines_store::{MachineStatus, MachinesStore};
    let rows: Vec<serde_json::Value> = MachinesStore::handle(ctx).read(ctx, |store, _| {
        store
            .registry
            .machines
            .iter()
            .map(|machine| {
                let connection = store.connection(&machine.id);
                serde_json::json!({
                    "id": machine.id,
                    "status": match store.status(&machine.id) {
                        MachineStatus::Connected => "connected",
                        MachineStatus::Connecting => "connecting",
                        MachineStatus::Error => "error",
                        MachineStatus::Idle => "not_connected",
                    },
                    "error": connection.and_then(|c| c.error.clone()),
                    "os": connection.and_then(|c| c.environment.as_ref()).map(|e| format!("{} {}", e.os, e.arch)),
                    "host_version": connection.and_then(|c| c.version),
                    "agents": connection.map(|c| c.agents.len()),
                })
            })
            .collect()
    });
    Response::with_data(serde_json::json!({ "machines": rows }))
}

/// Each named MCP server as the app runs it: configured, switched on,
/// approved, connected, and how many tools it lists.
fn plugin_server_status(names: &[String]) -> Response {
    let settings = crate::settings::AppSettings::global();
    let rows: Vec<serde_json::Value> = names
        .iter()
        .map(|name| {
            let entry = settings.mcp_servers.iter().find(|server| &server.name == name);
            let tools = crate::agent::mcp::tool_count_for(name);
            let error = crate::agent::mcp::connection_error_for(name);
            serde_json::json!({
                "name": name,
                "configured": entry.is_some(),
                "on": entry.is_some_and(|server| server.enabled),
                "approved": settings.plugin.has_consent(name),
                "connected": entry.is_some_and(|server| server.enabled) && error.is_none() && tools > 0,
                "tools": tools,
                "error": error,
                "missing_settings": entry
                    .map(|server| server.env.iter().filter(|(_, value)| value.is_empty()).map(|(key, _)| key.clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
            })
        })
        .collect();
    Response::with_data(serde_json::json!({ "servers": rows }))
}

#[cfg(test)]
mod tests {
    use super::is_commit_id;

    #[test]
    fn only_full_commit_ids_are_exact_start_points() {
        assert!(is_commit_id("028c9dd7543da033ddcc92af95910e168720af48"));
        assert!(is_commit_id(&"a".repeat(64)));
        assert!(!is_commit_id("main"));
        assert!(!is_commit_id("028c9dd"));
        assert!(!is_commit_id("feature-branch-name-that-is-forty-chars!"));
    }
}
