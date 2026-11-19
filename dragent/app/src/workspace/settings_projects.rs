//! Projects settings uses the same roots and actions as project navigation.
use super::{ProjectFolderIntent, WorkspaceAction, WorkspaceView};
use crate::settings::{ProjectSettingsAction, ProjectSettingsEntry};
use std::path::PathBuf;
use warpui::{TypedActionView, UpdateView, ViewContext};

impl WorkspaceView {
    pub(crate) fn refresh_projects_settings(&self, ctx: &mut ViewContext<Self>) {
        if self.organisation_id != dragent_paths::organisation_id() {
            return;
        }
        // Read this organisation's registry rather than a process-wide store
        // that may still be transitioning from the previously selected org.
        let registry = dragent_host::machine_registry::MachineRegistry::load().unwrap_or_default();
        let sidebar_roots = self.sidebar_project_roots();
        let mut roots = sidebar_roots.clone();
        for root in self.workspace_trust.recent_roots().into_iter().chain(
            registry
                .projects
                .iter()
                .map(|project| crate::remote_fs::alias(&project.machine_id, &project.root)),
        ) {
            let root = self.project_root_of(&root);
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        let mut projects = roots
            .into_iter()
            .map(|root| {
                let remote = crate::remote_fs::resolve(&root);
                let saved = registry.projects.iter().find(|project| {
                    crate::remote_fs::alias(&project.machine_id, &project.root) == root
                });
                let machine = remote
                    .as_ref()
                    .and_then(|remote| {
                        registry
                            .machines
                            .iter()
                            .find(|machine| machine.id == remote.machine_id)
                    })
                    .map(|machine| machine.name())
                    .unwrap_or_else(|| {
                        if remote.is_some() {
                            "Unknown machine".into()
                        } else {
                            "This computer".into()
                        }
                    });
                ProjectSettingsEntry {
                    name: saved
                        .map(|project| project.name())
                        .unwrap_or_else(|| super::project_naming::sidebar_project_label(&root)),
                    path: remote
                        .as_ref()
                        .map(|remote| remote.path.clone())
                        .unwrap_or_else(|| root.display().to_string()),
                    machine,
                    remote: remote.is_some(),
                    workspaces: self
                        .workspaces
                        .iter()
                        .filter(|workspace| {
                            super::project_naming::belongs_to_project(&workspace.root, &root)
                        })
                        .count(),
                    groups: self.workspace_trust.groups(&root).len(),
                    hidden: self.workspace_trust.is_hidden(&root),
                    in_sidebar: sidebar_roots.contains(&root),
                    root,
                }
            })
            .collect::<Vec<_>>();
        // Per-project order/group overrides sort before name.
        projects.sort_by(|a, b| {
            let order_of = |root: &std::path::PathBuf| {
                crate::settings::RepoSettingsStore::get(root).map(|r| r.order).unwrap_or(0)
            };
            order_of(&a.root)
                .cmp(&order_of(&b.root))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.machine.to_lowercase().cmp(&b.machine.to_lowercase()))
                .then_with(|| a.root.cmp(&b.root))
        });
        let org = self.organisation_id.clone();
        ctx.update_view(&self.settings_view, |settings, settings_ctx| {
            if settings.projects_org != org || settings.projects != projects {
                settings.projects = projects;
                settings.projects_org = org;
                settings.keep_project_selected(settings_ctx);
                settings_ctx.notify();
            }
        });
    }

    pub(crate) fn handle_projects_settings(
        &mut self,
        action: &ProjectSettingsAction,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.organisation_id != dragent_paths::organisation_id() {
            return;
        }
        let remote_id = |root: &PathBuf| {
            dragent_host::machine_registry::MachineRegistry::load()
                .ok()?
                .projects
                .into_iter()
                .find(|project| {
                    crate::remote_fs::alias(&project.machine_id, &project.root) == *root
                })
                .map(|project| project.id)
        };
        match action {
            ProjectSettingsAction::Refresh => self.refresh_projects_settings(ctx),
            ProjectSettingsAction::Add => {
                self.close_settings(ctx);
                self.handle_action(&WorkspaceAction::ToggleAddProjectMenu, ctx);
            }
            ProjectSettingsAction::Open(root) => {
                self.close_settings(ctx);
                self.workspace_trust.show(root);
                if let Some(id) = remote_id(root) {
                    self.open_remote_project(&id, ctx);
                } else if let Some(id) = self
                    .workspaces
                    .iter()
                    .find(|workspace| {
                        super::project_naming::belongs_to_project(&workspace.root, root)
                    })
                    .map(|workspace| workspace.id)
                {
                    self.handle_action(
                        &WorkspaceAction::SelectProjectWorkspace { workspace_id: id },
                        ctx,
                    );
                } else if root == &super::project_naming::automation_sessions_root() {
                    self.handle_action(&WorkspaceAction::OpenAutomations, ctx);
                } else {
                    self.handle_action(
                        &WorkspaceAction::ProjectFolderSelected {
                            root: root.clone(),
                            intent: ProjectFolderIntent::OpenFolder,
                        },
                        ctx,
                    );
                }
            }
            ProjectSettingsAction::ToggleVisibility(root) => {
                if self.workspace_trust.is_hidden(root) {
                    self.workspace_trust.show(root);
                } else {
                    self.workspace_trust.hide(root);
                }
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                self.refresh_projects_settings(ctx);
            }
            ProjectSettingsAction::Remove(root) => {
                self.close_settings(ctx);
                if let Some(id) = remote_id(root) {
                    self.remove_remote_project(&id, ctx);
                } else {
                    self.handle_action(
                        &WorkspaceAction::RequestDeleteProject { root: root.clone() },
                        ctx,
                    );
                }
            }
        }
    }
}
