//! Agents panel actions: toggle/refresh/focus/stop/close.
//!
//! Handles the corresponding WorkspaceAction variants. Called from the
//! dispatcher (view_actions); row resolution runs off-thread against the
//! host (`HostClient::ensure_host` + per-root workspace + agent runs +
//! worktree paths).

use std::path::PathBuf;

use warpui::ViewContext;

#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;

use super::agents_panel::{run_state_text, AgentRunRow};
use super::project_naming::default_pane_cwd;
use super::view::WorkspaceView;
use super::WorkspaceAction;

/// Resolve every host agent run across `roots` into panel rows.
fn collect_agent_rows(
    client: &dragent_host::HostClient,
    roots: &[PathBuf],
) -> (Vec<AgentRunRow>, Option<String>) {
    let mut rows = Vec::new();
    let mut error = None;
    for root in roots {
        let root_str = root.to_string_lossy().into_owned();
        let workspace_id = match client.create_workspace(&root_str) {
            Ok(workspace_id) => workspace_id,
            Err(err) => {
                error = Some(err);
                continue;
            }
        };
        let runs = match client.list_agent_runs(&workspace_id) {
            Ok(runs) => runs,
            Err(err) => {
                error = Some(err);
                continue;
            }
        };
        let homes = client.list_worktrees(&workspace_id).unwrap_or_default();
        for run in runs {
            let (branch, path) = homes
                .iter()
                .find(|home| home.id == run.worktree_id)
                .map(|home| {
                    (
                        home.repo
                            .as_ref()
                            .map(|binding| binding.branch.clone())
                            .unwrap_or_default(),
                        home.path.clone(),
                    )
                })
                .unwrap_or_default();
            rows.push(AgentRunRow {
                run_id: run.id,
                label: run.label,
                state: run_state_text(&run.state, run.memory_bytes),
                branch,
                home_path: path,
            });
        }
    }
    (rows, error)
}

impl WorkspaceView {
    pub(crate) fn handle_agents_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ToggleAgentsPanel => {
                self.agents_panel_open = !self.agents_panel_open;
                if self.agents_panel_open && self.agent_runs.is_empty() && !self.agents_busy {
                    self.refresh_agent_runs(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::RefreshAgentRuns => {
                self.refresh_agent_runs(ctx);
            }
            WorkspaceAction::FocusAgentRun { run_id } => {
                let row = self
                    .agent_runs
                    .iter()
                    .find(|row| row.run_id == *run_id)
                    .cloned();
                self.agents_panel_open = false;
                if let Some(row) = row {
                    let home = if row.home_path.is_empty() {
                        self.selected_project_root()
                            .unwrap_or_else(default_pane_cwd)
                    } else {
                        PathBuf::from(row.home_path)
                    };
                    self.open_host_session_mirror(row.run_id, row.label, home, ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::StopAgentRun { run_id } => {
                let run_id = run_id.clone();
                self.agents_busy = true;
                self.agents_error = None;
                ctx.notify();
                let roots = self.open_project_roots.clone();
                let (tx, rx) = async_channel::bounded(1);
                crate::bg::detach(move || {
                    let result = match dragent_host::HostClient::ensure_host() {
                        Ok(client) => {
                            // Stop the run on whichever workspace owns it, then
                            // re-collect so the panel reflects the new state.
                            for root in &roots {
                                let root_str = root.to_string_lossy().into_owned();
                                let Ok(workspace_id) = client.create_workspace(&root_str) else {
                                    continue;
                                };
                                let owns = client
                                    .list_agent_runs(&workspace_id)
                                    .map(|runs| runs.iter().any(|run| run.id == run_id))
                                    .unwrap_or(false);
                                if owns {
                                    let _ = client.close_terminal(&workspace_id, &run_id);
                                    break;
                                }
                            }
                            collect_agent_rows(&client, &roots)
                        }
                        Err(err) => (Vec::new(), Some(err)),
                    };
                    let _ = tx.send_blocking(result);
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, (rows, error): (Vec<AgentRunRow>, Option<String>), ctx| {
                        view.agents_busy = false;
                        view.agent_runs = rows;
                        view.agents_error = error;
                        ctx.notify();
                    },
                    |_, _| {},
                );
            }
            WorkspaceAction::CloseAgentsPanel => {
                self.agents_panel_open = false;
                ctx.notify();
            }
            _ => {}
        }
    }

    /// Re-resolve host agent runs off-thread and deliver rows to the panel.
    pub(crate) fn refresh_agent_runs(&mut self, ctx: &mut ViewContext<Self>) {
        self.agents_busy = true;
        self.agents_error = None;
        ctx.notify();
        let roots = self.open_project_roots.clone();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = match dragent_host::HostClient::ensure_host() {
                Ok(client) => collect_agent_rows(&client, &roots),
                Err(err) => (Vec::new(), Some(err)),
            };
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, (rows, error): (Vec<AgentRunRow>, Option<String>), ctx| {
                view.agents_busy = false;
                view.agent_runs = rows;
                view.agents_error = error;
                ctx.notify();
            },
            |_, _| {},
        );
    }
}
