//! Starts a paired phone asks this window for (see
//! `realtime::companion_starts`): work on a task, or a new coordination.
//! Each runs through the window's new-workspace prompt exactly as a start
//! from the desktop does: open the prompt on the project, fill it in,
//! send it.

use std::path::{Path, PathBuf};

use warpui::{TypedActionView, UpdateView, ViewContext};

use super::view::WorkspaceView;
use super::WorkspaceAction;
use crate::realtime::companion_starts::{Refusal, StartKind, StartRequest};

impl WorkspaceView {
    /// Start taking this window's phone starts (once, when it's built).
    pub(super) fn listen_for_companion_starts(&mut self, ctx: &mut ViewContext<Self>) {
        let requests = crate::realtime::companion_starts::start_requests(
            super::desktop_catalog::contributor_key(self),
        );
        ctx.spawn_stream_local(
            requests,
            |view, request: StartRequest, ctx| {
                let result = view.start_for_companion(&request, ctx);
                request.answer(result);
            },
            |_, _| {},
        );
    }

    fn start_for_companion(&mut self, request: &StartRequest, ctx: &mut ViewContext<Self>) -> Result<(), Refusal> {
        if self.current_account(ctx).as_ref() != Some(&request.account) {
            return Err(Refusal::NotHere);
        }
        let projects = self.sidebar_project_roots();
        let in_sidebar = |path: &str| projects.iter().find(|root| root.as_path() == Path::new(path)).cloned();
        // A team's lead or clean-up: the window showing its project does
        // what its Coordination page does.
        match &request.kind {
            StartKind::NewLead { project, team, agent, model, cwd, prompt } => {
                in_sidebar(project).ok_or(Refusal::NotHere)?;
                self.start_new_lead(team.clone(), agent, model.clone(), cwd.clone(), prompt, ctx);
                return Ok(());
            }
            StartKind::Resume { project, session } => {
                in_sidebar(project).ok_or(Refusal::NotHere)?;
                return self.resume_session((**session).clone(), ctx).map_err(Refusal::Reason);
            }
            StartKind::CleanUp { project, remove } => {
                let root = in_sidebar(project).ok_or(Refusal::NotHere)?;
                self.clean_up_team(root, remove.clone(), false, ctx);
                return Ok(());
            }
            StartKind::Item { item, root, agent } => {
                let root = match root {
                    Some(root) => Some(in_sidebar(&root.to_string_lossy()).ok_or(Refusal::NotHere)?),
                    None => None,
                };
                if self.project_start_busy {
                    return Err(Refusal::Reason(
                        "The desktop is already starting a workspace. Try again in a moment.".into(),
                    ));
                }
                // As the Tasks page's Start on a tracker row: the prompt
                // opens on the item's repository (or the project named)
                // with the item attached.
                self.pending_task_start = None;
                self.start_tracker_item((**item).clone(), root, ctx);
                let opened = self.selected_project_root().is_some()
                    && self.prompt_links.iter().any(|link| link.identifier == item.item.key);
                if !opened {
                    return Err(Refusal::Reason(format!(
                        "{} needs its project open on the desktop first. Open it there, or choose the project, then start again.",
                        item.key
                    )));
                }
                self.prompt_selected_worktree = None;
                self.prompt_agent = *agent;
                self.prompt_team = false;
                self.handle_action(&WorkspaceAction::SubmitPrompt, ctx);
                if let Some(problem) = self.project_start_error.clone() {
                    return Err(Refusal::Reason(problem));
                }
                ctx.notify();
                return Ok(());
            }
            _ => {}
        }
        let root: PathBuf = match &request.kind {
            StartKind::Task { project: Some(project), .. }
            | StartKind::Coordination { project, .. }
            | StartKind::Session { project, .. } => in_sidebar(project).ok_or(Refusal::NotHere)?,
            StartKind::NewLead { .. } | StartKind::CleanUp { .. } | StartKind::Item { .. } | StartKind::Resume { .. } => return Err(Refusal::NotHere),
            StartKind::Task { task, project: None, .. } => match &task.project {
                // The checkout of the task's repository in this window.
                Some(_) => self.project_for_task(task).ok_or(Refusal::NotHere)?,
                None => {
                    return Err(Refusal::Reason(format!(
                        "{} isn't in a project. Choose the project to work in.",
                        task.key
                    )))
                }
            },
        };
        if self.project_start_busy {
            return Err(Refusal::Reason(
                "The desktop is already starting a workspace. Try again in a moment.".into(),
            ));
        }
        self.handle_action(&WorkspaceAction::OpenProjectWorkspacePrompt { root: root.clone() }, ctx);
        // A folder that needs the person first (trust, git init) stops here.
        if self.selected_project_root().as_deref() != Some(root.as_path()) {
            return Err(Refusal::Reason(
                "The desktop needs you to confirm this project first. Open it on the desktop.".into(),
            ));
        }
        self.prompt_selected_worktree = None;
        // A session from the phone: its workspace (an existing one of this
        // project, or a new one from a branch) and the agent's model.
        if let StartKind::Session { workspace, branch, model, .. } = &request.kind {
            if let Some(path) = workspace {
                // Checked against git before it got here; it must still be there.
                if !path.is_dir() {
                    return Err(Refusal::Reason(
                        "That workspace isn't part of this project on the desktop any more. Choose another, or a new workspace.".into(),
                    ));
                }
                if path.as_path() == root.as_path() {
                    self.handle_action(&WorkspaceAction::SelectPromptCurrentCheckout, ctx);
                } else {
                    self.handle_action(&WorkspaceAction::OpenPromptWorktree { path: path.clone() }, ctx);
                }
            } else if let Some(branch) = branch {
                self.handle_action(&WorkspaceAction::CreatePromptWorktreeFromBranch { branch: branch.clone() }, ctx);
            }
            self.prompt_lead_model = model.clone();
        }
        let prompt = match &request.kind {
            StartKind::Task { task, agent, .. } => {
                self.prompt_agent = *agent;
                self.prompt_team = false;
                self.prompt_task = Some((**task).clone());
                dragent_tasks::task_prompt(task)
            }
            StartKind::Coordination { goal, lead, workers, cost, .. } => {
                self.prompt_agent = *lead;
                self.prompt_team = true;
                self.prompt_team_agents = workers.clone();
                self.prompt_team_cost = *cost;
                goal.clone()
            }
            StartKind::Session { agent, prompt, .. } => {
                self.prompt_agent = *agent;
                self.prompt_team = false;
                self.prompt_task = None;
                prompt.clone()
            }
            StartKind::NewLead { .. } | StartKind::CleanUp { .. } | StartKind::Item { .. } | StartKind::Resume { .. } => return Err(Refusal::NotHere),
        };
        self.prompt_text = prompt.clone();
        let editor = self.prompt_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| editor.set_text(&prompt, editor_ctx));
        self.handle_action(&WorkspaceAction::SubmitPrompt, ctx);
        if let Some(problem) = self.project_start_error.clone() {
            return Err(Refusal::Reason(problem));
        }
        if !self.project_start_busy {
            return Err(Refusal::Reason("The desktop didn't start it. Open the desktop to see why.".into()));
        }
        ctx.notify();
        Ok(())
    }
}
