//! Remote projects open as ordinary projects. A saved remote project is
//! rooted at its machine's local stand-in folder (see [`crate::remote_fs`]),
//! so the sidebar, project screen, workspace tabs, panes, Files, Source
//! Control and editor are exactly the components a local project uses; only
//! their I/O is served by the machine.
use super::view::WorkspaceView;
use super::{ProjectFolderIntent, WorkspaceAction};
use crate::settings::machines_store::{MachineStatus, MachinesStore};
use crate::settings::tool_catalog::{self, JobKind, ToolJob};
use crate::terminal::TerminalEvent;
use dragent_host::development::{DevelopmentRequest as Request, DevelopmentResponse as Response};
use std::path::PathBuf;
use warpui::{ReadView, SingletonEntity, TypedActionView, UpdateModel, UpdateView, ViewContext};

/// An install, update or uninstall running in the install sheet on a
/// machine, from Settings → Machines.
#[derive(Clone, Debug)]
pub struct RemoteToolJob {
    pub machine_id: String,
    /// The machine's display name.
    pub machine: String,
    pub job: ToolJob,
    /// It failed for lack of permission and may be retried with sudo.
    pub can_retry: bool,
}

/// The agents on a remote project's machine, from Settings → Machines'
/// tool check: agents there start and install there, never by this
/// computer's PATH.
pub(crate) struct MachineAgents {
    pub machine_id: String,
    pub machine: String,
    /// The machine is connected and its tools were checked.
    pub checked: bool,
    /// Executables installed and working there.
    installed: std::collections::HashSet<String>,
    /// Executable → what runs instead (the copy chosen there).
    chosen: std::collections::HashMap<String, String>,
}

impl MachineAgents {
    pub fn has(&self, executable: &str) -> bool {
        self.installed.contains(executable)
    }

    /// Built-in agents installed there.
    pub fn agents(&self) -> std::collections::HashSet<super::PromptAgent> {
        super::PromptAgent::ALL
            .into_iter()
            .filter(|agent| *agent == super::PromptAgent::Dragent || agent.cli().is_some_and(|cli| self.has(cli)))
            .collect()
    }

    /// `line` (an agent launch) running the copy chosen on the machine.
    pub fn launch_line(&self, line: &str) -> String {
        let trimmed = line.trim_start();
        let executable = trimmed.split_whitespace().next().unwrap_or_default();
        match self.chosen.get(executable) {
            Some(replacement) => format!("{replacement}{}", &trimmed[executable.len()..]),
            None => line.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RemoteAction {
    Open(String),
    /// Connect (or retry) a machine from a remote project's screen.
    Connect(String),
    Attach {
        machine_id: String,
        session: dragent_host::TerminalSession,
    },
    RunToolJob {
        machine_id: String,
        job: ToolJob,
    },
}

impl WorkspaceView {
    /// Settings → Machines, on one machine's page when `machine` is given.
    pub(crate) fn show_machines(&mut self, machine: Option<String>, ctx: &mut ViewContext<Self>) {
        self.review_menu_open = false;
        self.add_project = None;
        self.open_settings(ctx);
        let settings = self.settings_view.clone();
        ctx.update_view(&settings, |view, ctx| {
            view.show_section(crate::settings::SettingsSection::Machines, ctx);
            match machine {
                Some(id) => view.open_machine(&id, ctx),
                None => view.handle_machine_action(
                    &crate::settings::machines::MachineAction::ShowList,
                    ctx,
                ),
            }
        });
    }

    pub(crate) fn handle_remote_action(&mut self, action: &RemoteAction, ctx: &mut ViewContext<Self>) {
        match action {
            RemoteAction::Open(project_id) => self.open_remote_project(project_id, ctx),
            RemoteAction::Connect(machine_id) => {
                let store = MachinesStore::handle(ctx);
                let machine_id = machine_id.clone();
                ctx.update_model(&store, |s, c| s.recover(&machine_id, c));
            }
            RemoteAction::Attach {
                machine_id,
                session,
            } => self.attach_remote_session(machine_id, session.clone(), ctx),
            RemoteAction::RunToolJob { machine_id, job } => {
                self.run_remote_tool_job(machine_id, job.clone(), ctx)
            }
        }
    }

    /// The agents on `root`'s machine, when `root` is a remote project.
    pub(crate) fn machine_agents(&self, root: Option<&std::path::Path>, app: &warpui::AppContext) -> Option<MachineAgents> {
        let remote = crate::remote_fs::resolve(root?)?;
        let store = MachinesStore::handle(app);
        Some(store.read(app, |s, _| {
            let machine = s
                .machine(&remote.machine_id)
                .map(|m| m.name().to_string())
                .unwrap_or_else(|| remote.machine_id.clone());
            let connection = s.connection(&remote.machine_id);
            let tools = connection.and_then(|c| c.environment.as_ref()).map(|e| e.tools.clone());
            let reports = connection.map(|c| c.reports.clone()).unwrap_or_default();
            let installed = tools
                .iter()
                .flatten()
                .filter(|(name, found)| {
                    *found && reports.get(name).is_none_or(|r| r.status != dragent_host::development::ToolStatus::Broken)
                })
                .map(|(name, _)| name.clone())
                .collect();
            let chosen = reports
                .iter()
                .filter_map(|(name, report)| {
                    let words = match report.pin.as_ref()? {
                        dragent_host::development::ToolPin::Path(path) => dragent_host::remote::quote(path),
                        dragent_host::development::ToolPin::Command(command) => command.clone(),
                    };
                    Some((name.clone(), words))
                })
                .collect();
            MachineAgents {
                machine_id: remote.machine_id.clone(),
                machine,
                checked: tools.is_some() && s.status(&remote.machine_id) == MachineStatus::Connected,
                installed,
                chosen,
            }
        }))
    }

    /// Why `executable` can't start on the project's machine, if it can't.
    pub(crate) fn machine_agent_problem(&mut self, machine: &MachineAgents, executable: &str, label: &str, ctx: &mut ViewContext<Self>) -> Option<String> {
        if !machine.checked {
            let id = machine.machine_id.clone();
            self.ensure_machine(&id, ctx);
            return Some(format!("{} isn't connected. Dragent is connecting it; try again in a moment.", machine.machine));
        }
        (!machine.has(executable)).then(|| {
            format!("{label} isn't installed on {}. Choose it in the agent menu to install it there.", machine.machine)
        })
    }

    /// Install an agent on a remote project's machine (the agent menu's
    /// Install on a remote project).
    pub(crate) fn install_agent_on_machine(&mut self, machine: &MachineAgents, agent: super::PromptAgent, ctx: &mut ViewContext<Self>) {
        let Some(cli) = agent.cli() else {
            return;
        };
        let store = MachinesStore::handle(ctx);
        let os = store.read(ctx, |s, _| s.connection(&machine.machine_id).map(|c| c.os()));
        let Some(method) = os.and_then(|os| tool_catalog::recommended(cli, os)) else {
            self.project_start_error = Some(format!(
                "Dragent can't install {} on {} automatically.",
                agent.label(),
                machine.machine
            ));
            ctx.notify();
            return;
        };
        let job = ToolJob::new(JobKind::Install, vec![cli.to_string()], method.install, method.may_elevate);
        let id = machine.machine_id.clone();
        self.run_remote_tool_job(&id, job, ctx);
    }

    /// The local root of a saved remote project.
    fn remote_project_root(&self, project_id: &str, app: &warpui::AppContext) -> Option<(String, PathBuf)> {
        let store = MachinesStore::handle(app);
        store.read(app, |s, _| {
            let project = s.project(project_id)?;
            Some((
                project.machine_id.clone(),
                crate::remote_fs::alias(&project.machine_id, &project.root),
            ))
        })
    }

    /// Connect a machine unless it is connected or connecting already.
    fn ensure_machine(&mut self, machine_id: &str, ctx: &mut ViewContext<Self>) {
        let store = MachinesStore::handle(ctx);
        let status = store.read(ctx, |s, _| s.status(machine_id));
        if matches!(status, MachineStatus::Idle | MachineStatus::Error) {
            let machine_id = machine_id.to_string();
            ctx.update_model(&store, |s, c| s.connect(&machine_id, c));
        }
    }

    /// Open a saved remote project exactly like a local folder.
    pub(crate) fn open_remote_project(&mut self, project_id: &str, ctx: &mut ViewContext<Self>) {
        let store = MachinesStore::handle(ctx);
        ctx.update_model(&store, |s, c| s.reload(c));
        let Some((machine_id, root)) = self.remote_project_root(project_id, ctx) else {
            return;
        };
        self.ensure_machine(&machine_id, ctx);
        self.add_project = None;
        self.close_settings(ctx);
        self.receive_project_folder(root, ProjectFolderIntent::OpenFolder, ctx);
    }

    /// Settings → Machines → Sessions → Open: show a running remote session
    /// as an ordinary tab of its project.
    pub(crate) fn attach_remote_session(
        &mut self,
        machine_id: &str,
        session: dragent_host::TerminalSession,
        ctx: &mut ViewContext<Self>,
    ) {
        self.attach_remote_session_at_project(machine_id, session, None, ctx);
    }

    pub(crate) fn attach_remote_session_at_project(
        &mut self, machine_id: &str, session: dragent_host::TerminalSession,
        project: Option<PathBuf>, ctx: &mut ViewContext<Self>,
    ) {
        let store = MachinesStore::handle(ctx);
        let project_root = store.read(ctx, |s, _| {
            s.projects_for(machine_id)
                .into_iter()
                .filter(|p| crate::settings::machines::path_within(&session.cwd, &p.root))
                .max_by_key(|p| p.root.len())
                .map(|p| crate::remote_fs::alias(machine_id, &p.root))
        });
        let Ok(client) = crate::remote_fs::client(machine_id) else {
            self.ensure_machine(machine_id, ctx);
            return;
        };
        let cwd = crate::remote_fs::alias(machine_id, &session.cwd);
        let root = project.or(project_root).unwrap_or_else(|| cwd.clone());
        crate::remote_fs::ensure_stand_in(&root);
        self.close_settings(ctx);
        // Already shown: bring that tab forward.
        let existing = self.host_owned_tabs.iter().find_map(|(id, owned)| {
            (owned.session_id.as_deref() == Some(session.id.as_str())).then_some(*id)
        });
        if let Some(tab_id) = existing {
            self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
            return;
        }
        if !self.open_project_roots.contains(&root) {
            self.open_project_roots.push(root.clone());
        }
        let title = super::project_naming::worktree_project_name(&root);
        let tab = self.build_host_tab(
            ctx,
            None,
            None,
            cwd,
            Some(root),
            true,
            title,
            None,
            crate::agent::TabSessionKind::Shell,
            true,
            Some(client),
            Some(session),
        );
        self.adopt_new_tab(tab, ctx);
    }

    /// Settings → Machines: install, update or remove tools in the install
    /// sheet, a terminal on the machine shown over Settings, so prompts such
    /// as a sudo password are answered in place.
    fn run_remote_tool_job(&mut self, machine_id: &str, job: ToolJob, ctx: &mut ViewContext<Self>) {
        if crate::remote_fs::client(machine_id).is_err() {
            self.ensure_machine(machine_id, ctx);
            return;
        }
        let Some(home) = crate::remote_fs::home(machine_id) else {
            return;
        };
        let store = MachinesStore::handle(ctx);
        let machine = store
            .read(ctx, |s, _| s.machine(machine_id).map(|m| m.name().to_string()))
            .unwrap_or_else(|| machine_id.to_string());
        let job = RemoteToolJob {
            machine_id: machine_id.to_string(),
            machine,
            job,
            can_retry: false,
        };
        self.start_remote_tool_job(job, &home, ctx);
    }

    fn start_remote_tool_job(&mut self, job: RemoteToolJob, home: &str, ctx: &mut ViewContext<Self>) {
        let home = crate::remote_fs::alias(&job.machine_id, home);
        crate::remote_fs::ensure_stand_in(&home);
        let (terminal, generation) = self.create_installer_terminal(Some(home), &job.job.command, ctx);
        // Set after the helper: it closes any previous installer, which
        // clears the subject being replaced.
        self.remote_tool_install = Some(job);
        let model = terminal.read(ctx, |view, _| view.terminal_model());
        ctx.subscribe_to_model(&model, move |view, _, event, ctx| {
            if view.prompt_install_generation != generation {
                return;
            }
            let TerminalEvent::BlockCompleted { context } = event else {
                return;
            };
            // No exit status means it ended while the machine was out of
            // reach; the machine's tools say whether it worked.
            if context.success() || context.exit_code.is_none() {
                view.check_remote_tool_install(ctx);
                return;
            }
            let denied = tool_catalog::permission_denied(&context.output_text);
            let Some(job) = view.remote_tool_install.as_mut() else {
                return;
            };
            job.can_retry = denied && job.job.may_elevate && !job.job.command.starts_with("sudo ");
            view.prompt_install_error = Some(if job.can_retry {
                "It stopped for lack of permission. Retry with administrator rights, or fix the problem and choose Check again.".into()
            } else {
                "It stopped with an error. The output is above; fix the problem, then choose Check again.".into()
            });
            ctx.notify();
        });
        // Installers ask questions (sudo, confirmations): type straight in.
        ctx.focus(&terminal);
        ctx.notify();
    }

    /// The install sheet's "Retry with administrator rights".
    pub(super) fn retry_remote_tool_job_elevated(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(mut job) = self.remote_tool_install.clone().filter(|job| job.can_retry) else {
            return;
        };
        let Some(home) = crate::remote_fs::home(&job.machine_id) else {
            return;
        };
        job.job.command = tool_catalog::elevated(&job.job.command);
        job.can_retry = false;
        self.start_remote_tool_job(job, &home, ctx);
    }

    /// The job finished, or "Check again": ask the machine about its tools
    /// now. Settings → Machines follows, and the sheet closes once the job
    /// shows its effect.
    pub(super) fn check_remote_tool_install(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(job) = self.remote_tool_install.clone() else {
            return;
        };
        let Ok(client) = crate::remote_fs::client(&job.machine_id) else {
            self.prompt_install_error = Some(format!(
                "{} is disconnected. Connect it, then choose Check again.",
                job.machine
            ));
            ctx.notify();
            return;
        };
        let generation = self.prompt_install_generation;
        self.remote_tool_checking = true;
        self.prompt_install_error = None;
        ctx.notify();
        let names = job.job.tools.clone();
        ctx.spawn(
            async move {
                let (tools, report) = crate::settings::machines_store::probe_tools(&client, &names, Vec::new());
                if tools.is_empty() && report.is_none() {
                    // A runtime that predates `Which`: its Environment's tools.
                    return match client.development(Request::Environment)? {
                        Response::Environment { tools, .. } => Ok((tools, None)),
                        _ => Err("The machine returned an invalid environment.".to_string()),
                    };
                }
                Ok((tools, report))
            },
            move |view, result, ctx| {
                if view.prompt_install_generation != generation {
                    return;
                }
                view.remote_tool_checking = false;
                let (tools, report) = match result {
                    Ok(found) => found,
                    Err(error) => {
                        view.prompt_install_error = Some(error);
                        ctx.notify();
                        return;
                    }
                };
                let installed = |tool: &String| tools.iter().any(|(name, found)| name == tool && *found);
                let store = MachinesStore::handle(ctx);
                let id = job.machine_id.clone();
                match report.clone() {
                    Some(report) => ctx.update_model(&store, |s, c| s.apply_tools_report(&id, report, c)),
                    None => ctx.update_model(&store, |s, c| s.set_tools(&id, tools.clone(), c)),
                }
                let pending: Vec<String> = match job.job.kind {
                    JobKind::Install | JobKind::Update => {
                        job.job.tools.iter().filter(|tool| !installed(tool)).cloned().collect()
                    }
                    JobKind::Uninstall => job.job.tools.iter().filter(|tool| installed(tool)).cloned().collect(),
                };
                if pending.is_empty() {
                    view.close_prompt_agent_install(ctx);
                    return;
                }
                let names = pending.iter().map(|tool| tool_catalog::label(tool)).collect::<Vec<_>>().join(", ");
                view.prompt_install_error = Some(match job.job.kind {
                    JobKind::Uninstall => {
                        let other = report
                            .as_ref()
                            .and_then(|report| report.tools.iter().find(|tool| pending.contains(&tool.name)))
                            .and_then(|tool| tool.resolved.clone());
                        match other {
                            Some(path) => format!("{names} is still installed at {path}. That copy was installed another way; remove it the way it was installed."),
                            None => format!("{names} is still installed. If it's still being removed, wait for it to finish, then choose Check again."),
                        }
                    }
                    _ => format!(
                        "{names} isn't on {}'s PATH yet. If it's still installing, wait for it to finish, then choose Check again.",
                        job.machine
                    ),
                });
                ctx.notify();
            },
        );
    }

    /// Make a freshly built tab the active one, the way launches do.
    fn adopt_new_tab(&mut self, tab: super::view::PaneTab, ctx: &mut ViewContext<Self>) {
        let tab_id = tab.id;
        self.tabs.push(tab);
        self.active_tab_id = tab_id;
        self.split_tree = None;
        self.project_start_visible = false;
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// Settings → Machines → Workspaces → remove: the standard "Delete
    /// project" confirmation; confirming also forgets it in Machines.
    pub(crate) fn remove_remote_project(&mut self, project_id: &str, ctx: &mut ViewContext<Self>) {
        let Some((_, root)) = self.remote_project_root(project_id, ctx) else {
            return;
        };
        if !self.open_project_roots.contains(&root) {
            // Not in the sidebar: nothing to close, forget it right away.
            forget_remote_project(&root);
            let store = MachinesStore::handle(ctx);
            ctx.update_model(&store, |s, c| s.reload(c));
            return;
        }
        self.close_settings(ctx);
        self.handle_action(&WorkspaceAction::RequestDeleteProject { root }, ctx);
    }

    /// Machine connections changed: refresh whatever shows remote data.
    pub(crate) fn remote_machines_changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.add_project_machines_changed(ctx);
        // The store also reports every health poll; remote files and git only
        // need reloading when a machine became reachable or unreachable.
        let connected = crate::remote_fs::connected_machines();
        if connected == self.remote_connected {
            self.sync_left_panel(ctx);
            ctx.notify();
            return;
        }
        self.remote_connected = connected;
        // Project cards, search and Git scope resolve their repository root
        // through the machine: re-resolve now that it can answer.
        if let Some(context) = self
            .sidebar_context_root(ctx)
            .filter(|root| crate::remote_fs::is_remote(root))
        {
            self.sync_folder_git_model(ctx);
            let sidebar_view = self.project_sidebar_view.clone();
            let context_root = Some(context.clone());
            ctx.update_view(&sidebar_view, |sidebar, sidebar_ctx| {
                sidebar.refresh_workspace_context(context_root, sidebar_ctx);
            });
            let sidebar_model = self.project_sidebar_model.clone();
            ctx.update_model(&sidebar_model, |model, model_ctx| {
                model.refresh_all_cwds(&[context.clone()], Some(context), model_ctx);
            });
        }
        if crate::remote_fs::is_remote(&self.document_index_root) {
            self.refresh_document_tree(ctx);
            // A file restored before its machine connected failed to load;
            // open it again now that it can be read.
            let unloaded = self
                .project_sidebar_model
                .read(ctx, |model, _| model.buffer.is_none());
            if let Some(path) = self.document_requested_path.clone().filter(|_| {
                unloaded
                    && self.document_center_open
                    && self.document_tab_id == Some(self.active_tab_id)
            }) {
                let root = self.document_index_root.clone();
                self.open_document_path_at_root(root, path, false, ctx);
            }
        }
        // The project screen's branch and worktree chips read git, which
        // only answers once the machine is connected.
        if self
            .selected_project_root()
            .is_some_and(|root| crate::remote_fs::is_remote(&root))
        {
            self.refresh_prompt_branches(ctx);
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }
}

/// Drop a remote project from the saved registry (its files stay on the
/// machine).
pub(crate) fn forget_remote_project(root: &std::path::Path) {
    let Some(remote) = crate::remote_fs::resolve(root) else {
        return;
    };
    let _ = dragent_host::machine_registry::MachineRegistry::update_at(
        &dragent_host::machine_registry::MachineRegistry::path(),
        |registry| {
            registry
                .projects
                .retain(|p| !(p.machine_id == remote.machine_id && p.root == remote.path));
            Ok(())
        },
    );
}

#[cfg(test)]
mod machine_agent_tests {
    use super::MachineAgents;

    #[test]
    fn launches_run_the_copy_chosen_on_the_machine() {
        let machine = MachineAgents {
            machine_id: "m".into(),
            machine: "Build box".into(),
            checked: true,
            installed: ["claude".to_string(), "codex".to_string()].into_iter().collect(),
            chosen: [
                ("claude".to_string(), "'/opt/claude beta/bin/claude'".to_string()),
                ("codex".to_string(), "npx -y @openai/codex".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(
            machine.launch_line("claude --dangerously-skip-permissions"),
            "'/opt/claude beta/bin/claude' --dangerously-skip-permissions"
        );
        assert_eq!(machine.launch_line("codex"), "npx -y @openai/codex");
        // Only the executable word is replaced, never a longer name.
        assert_eq!(machine.launch_line("claudex --flag"), "claudex --flag");
        assert_eq!(machine.launch_line("gemini -p"), "gemini -p");
        assert!(machine.has("codex") && !machine.has("gemini"));
        let agents = machine.agents();
        assert!(agents.contains(&crate::workspace::PromptAgent::Claude));
        assert!(agents.contains(&crate::workspace::PromptAgent::Dragent));
        assert!(!agents.contains(&crate::workspace::PromptAgent::Gemini));
    }
}
