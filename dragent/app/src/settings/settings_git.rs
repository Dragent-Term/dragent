//! Git / General / per-project Repo settings handlers.

use std::path::PathBuf;

use warpui::{UpdateView, ViewContext};

use super::action::SettingsAction;
use super::git_settings::{RepoSettingsStore, setup_guide_steps};
use super::settings_view::SettingsView;
use super::SettingsEvent;
use crate::workspace::InputEditor;
use crate::workspace::InputMode;

impl SettingsView {
    // ── persistence ──────────────────────────────────────────────────────────

    fn save_git(&mut self, ctx: &mut ViewContext<Self>) {
        self.app_settings.save();
        ctx.emit(SettingsEvent::AppSettingsChanged);
    }

    // ── editors (lazy, created on first visit like ignore_patterns_editor) ──

    fn make_text_editor(
        &mut self,
        ctx: &mut ViewContext<Self>,
        placeholder: &str,
        key: &str,
        initial: &str,
        secure: bool,
    ) -> warpui::ViewHandle<InputEditor> {
        let ui = self.ui_font;
        let theme = self.theme();
        let key = key.to_string();
        let initial = initial.to_string();
        ctx.add_typed_action_view(move |inner| {
            InputEditor::new_secure(
                placeholder,
                ui,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings(key),
                inner.focus_handle(),
                secure,
                &initial,
            )
        })
    }

    pub(crate) fn ensure_git_editors(&mut self, ctx: &mut ViewContext<Self>) {
        if self.git_prefix_custom_editor.is_none() {
            let initial = self.app_settings.git.branch_prefix_custom.clone();
            let editor =
                self.make_text_editor(ctx, "team-prefix", "git_branch_prefix_custom", &initial, false);
            self.git_prefix_custom_editor = Some(editor);
        }
        if self.workspace_dir_editor.is_none() {
            let initial = self
                .app_settings
                .general
                .workspace_dir
                .clone()
                .unwrap_or_default();
            let editor =
                self.make_text_editor(ctx, "~/.dragent/worktrees", "general_workspace_dir", &initial, false);
            self.workspace_dir_editor = Some(editor);
        }
        if self.commit_ai_agent_editor.is_none() {
            let initial = self
                .app_settings
                .git
                .commit_ai_agent
                .clone()
                .unwrap_or_default();
            let editor =
                self.make_text_editor(ctx, "Same as the agent", "git_commit_ai_agent", &initial, false);
            self.commit_ai_agent_editor = Some(editor);
        }
        if self.pr_bot_draft_editor.is_none() {
            let initial = self.pr_bot_draft.clone();
            let editor =
                self.make_text_editor(ctx, "renovate[bot]", "git_pr_bot_draft", &initial, false);
            self.pr_bot_draft_editor = Some(editor);
        }
        if self.open_in_draft_editor.is_none() {
            let initial = self.open_in_draft.clone();
            let editor = self.make_text_editor(
                ctx,
                "Cursor, VS Code, …",
                "git_open_in_draft",
                &initial,
                false,
            );
            self.open_in_draft_editor = Some(editor);
        }
    }

    /// Keep a project picked on the Projects page: the first one when none is,
    /// or the picked one is gone.
    pub(crate) fn keep_project_selected(&mut self, ctx: &mut ViewContext<Self>) {
        let picked = self
            .selected_repo_project
            .as_ref()
            .is_some_and(|root| self.projects.iter().any(|project| &project.root == root));
        if picked {
            return;
        }
        if let Some(root) = self.projects.first().map(|project| project.root.clone()) {
            self.selected_repo_project = Some(root.clone());
            self.ensure_repo_editors(ctx, &root);
        }
    }

    pub(crate) fn ensure_repo_editors(&mut self, ctx: &mut ViewContext<Self>, root: &PathBuf) {
        let key = root.display().to_string();
        let fallback = self.app_settings.repo.clone();
        let stored = RepoSettingsStore::get(root).unwrap_or(fallback);
        // The overrides the Projects page shows: field, example, saved value.
        let defs: [(&str, &str, String); 3] = [
            ("base_ref", "main", stored.base_ref.clone().unwrap_or_default()),
            ("base_path", "~/.dragent/worktrees", stored.base_path.clone().unwrap_or_default()),
            ("identity", "Name <email@example.com>", stored.identity.clone().unwrap_or_default()),
        ];
        for (field, example, initial) in &defs {
            let map_key = format!("repo_{field}:{key}");
            if !self.repo_editors.contains_key(&map_key) {
                let editor = self.make_text_editor(ctx, example, &map_key, initial, false);
                self.repo_editors.insert(map_key, editor);
            }
        }
    }

    /// Returns true when the action was a git/general/repo text keystroke.
    pub(crate) fn handle_git_text(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) -> bool {
        let SettingsAction::UpdateProviderKey(key, value) = action else {
            return false;
        };
        match key.as_str() {
            "git_branch_prefix_custom" => {
                self.app_settings.git.branch_prefix_custom = value.clone();
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                return true;
            }
            "general_workspace_dir" => {
                let trimmed = value.trim();
                self.app_settings.general.workspace_dir = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                return true;
            }
            "git_commit_ai_agent" => {
                let trimmed = value.trim();
                self.app_settings.git.commit_ai_agent = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                return true;
            }
            "git_pr_bot_draft" => {
                self.pr_bot_draft = value.clone();
                ctx.notify();
                return true;
            }
            "git_open_in_draft" => {
                self.open_in_draft = value.clone();
                ctx.notify();
                return true;
            }
            "repo_hook_draft" => {
                self.repo_hook_draft = value.clone();
                ctx.notify();
                return true;
            }
            "repo_symlink_draft" => {
                self.repo_symlink_draft = value.clone();
                ctx.notify();
                return true;
            }
            _ => {}
        }
        if let Some(rest) = key.strip_prefix("repo_base_ref:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.base_ref = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_base_path:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.base_path = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_hook_mode:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.hook_mode = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_issue_source:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.issue_source = if v.is_empty() { "auto".to_string() } else { v.to_string() };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_gh_account:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.gh_account = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_fork_sync:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.fork_sync = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_identity:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.identity = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_visibility:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.visibility = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_group_id:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.group_id = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_host_setup:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.host_setup_method = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_windows_runtime:") {
            let root = PathBuf::from(rest);
            let v = value.trim();
            RepoSettingsStore::mutate(&root, |r| {
                r.windows_runtime = if v.is_empty() { None } else { Some(v.to_string()) };
            });
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_hook_draft:") {
            let _ = rest;
            self.repo_hook_draft = value.clone();
            ctx.notify();
            return true;
        }
        if let Some(rest) = key.strip_prefix("repo_symlink_draft:") {
            let _ = rest;
            self.repo_symlink_draft = value.clone();
            ctx.notify();
            return true;
        }
        false
    }

    pub(crate) fn handle_git(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::ToggleAutoRenameBranch => {
                self.app_settings.git.auto_rename_branch =
                    !self.app_settings.git.auto_rename_branch;
                self.save_git(ctx);
            }
            SettingsAction::SelectBranchPrefix(prefix) => {
                self.app_settings.git.branch_prefix = *prefix;
                self.open_picker = None;
                self.save_git(ctx);
            }
            SettingsAction::SelectJiraCasing(casing) => {
                self.app_settings.git.jira_casing = *casing;
                self.open_picker = None;
                self.save_git(ctx);
            }
            SettingsAction::SelectDiffView(view) => {
                self.app_settings.git.diff_view = *view;
                self.save_git(ctx);
            }
            SettingsAction::ToggleDiffWordWrap => {
                self.app_settings.git.diff_word_wrap = !self.app_settings.git.diff_word_wrap;
                self.save_git(ctx);
            }
            SettingsAction::ToggleDiffShowWhitespace => {
                self.app_settings.git.diff_show_whitespace =
                    !self.app_settings.git.diff_show_whitespace;
                self.save_git(ctx);
            }
            SettingsAction::ToggleDiffCollapseUnchanged => {
                self.app_settings.git.diff_collapse_unchanged =
                    !self.app_settings.git.diff_collapse_unchanged;
                self.save_git(ctx);
            }
            SettingsAction::SelectScViewMode(mode) => {
                self.app_settings.git.sc_view_mode = *mode;
                self.save_git(ctx);
            }
            SettingsAction::SelectScGroupOrder(order) => {
                self.app_settings.git.sc_group_order = *order;
                self.open_picker = None;
                self.save_git(ctx);
            }
            SettingsAction::ToggleScCompareUpstream => {
                self.app_settings.git.sc_compare_upstream =
                    !self.app_settings.git.sc_compare_upstream;
                self.save_git(ctx);
            }
            SettingsAction::ToggleFollowSymlinks => {
                self.app_settings.git.follow_symlinks = !self.app_settings.git.follow_symlinks;
                self.save_git(ctx);
            }
            SettingsAction::ToggleShowIgnored => {
                self.app_settings.git.show_ignored = !self.app_settings.git.show_ignored;
                self.save_git(ctx);
            }
            SettingsAction::ToggleCommitAi => {
                self.app_settings.git.commit_ai_enabled = !self.app_settings.git.commit_ai_enabled;
                self.save_git(ctx);
            }
            SettingsAction::SelectCommitAiAgent(agent) => {
                self.app_settings.git.commit_ai_agent = agent.clone();
                self.open_picker = None;
                self.save_git(ctx);
            }
            SettingsAction::ClearCommitAiAgent => {
                self.app_settings.git.commit_ai_agent = None;
                if let Some(editor) = &self.commit_ai_agent_editor {
                    ctx.update_view(editor, |e, ectx| e.set_text("", ectx));
                }
                self.save_git(ctx);
            }
            SettingsAction::AddPrBotOverride => {
                let value = self.pr_bot_draft.trim().to_string();
                if !value.is_empty()
                    && !self.app_settings.git.pr_bot_overrides.iter().any(|b| b == &value)
                {
                    self.app_settings.git.pr_bot_overrides.push(value);
                    self.save_git(ctx);
                }
                self.pr_bot_draft.clear();
                if let Some(editor) = &self.pr_bot_draft_editor {
                    ctx.update_view(editor, |e, ectx| e.set_text("", ectx));
                }
            }
            SettingsAction::RemovePrBotOverride(index) => {
                if *index < self.app_settings.git.pr_bot_overrides.len() {
                    self.app_settings.git.pr_bot_overrides.remove(*index);
                    self.save_git(ctx);
                }
            }
            SettingsAction::AddOpenInApp => {
                let value = self.open_in_draft.trim().to_string();
                if !value.is_empty() && !self.app_settings.git.open_in_apps.iter().any(|a| a == &value)
                {
                    self.app_settings.git.open_in_apps.push(value);
                    self.save_git(ctx);
                }
                self.open_in_draft.clear();
                if let Some(editor) = &self.open_in_draft_editor {
                    ctx.update_view(editor, |e, ectx| e.set_text("", ectx));
                }
            }
            SettingsAction::RemoveOpenInApp(index) => {
                if *index < self.app_settings.git.open_in_apps.len() {
                    self.app_settings.git.open_in_apps.remove(*index);
                    self.save_git(ctx);
                }
            }
            SettingsAction::OpenInApp { app, path } => {
                open_path_in_app(path, app);
            }
            SettingsAction::SetWorkspaceDir(value) => {
                self.app_settings.general.workspace_dir = value.clone();
                self.save_git(ctx);
            }
            SettingsAction::ClearWorkspaceDir => {
                self.app_settings.general.workspace_dir = None;
                if let Some(editor) = &self.workspace_dir_editor {
                    ctx.update_view(editor, |e, ectx| e.set_text("", ectx));
                }
                self.save_git(ctx);
            }
            SettingsAction::ToggleNestWorkspaces => {
                self.app_settings.general.nest_workspaces = !self.app_settings.general.nest_workspaces;
                self.save_git(ctx);
            }
            SettingsAction::ToggleRefreshBaseRef => {
                self.app_settings.general.refresh_base_ref_on_create =
                    !self.app_settings.general.refresh_base_ref_on_create;
                self.save_git(ctx);
            }
            SettingsAction::ToggleConfirmCloseTerminal => {
                self.app_settings.general.confirm_close_terminal_with_process =
                    !self.app_settings.general.confirm_close_terminal_with_process;
                self.save_git(ctx);
            }
            SettingsAction::ToggleConfirmDeleteWorktree => {
                self.app_settings.general.confirm_delete_worktree =
                    !self.app_settings.general.confirm_delete_worktree;
                self.save_git(ctx);
            }
            SettingsAction::ToggleOpenLinksInApp => {
                self.app_settings.general.open_links_in_app =
                    !self.app_settings.general.open_links_in_app;
                self.save_git(ctx);
            }
            SettingsAction::ToggleRestoreSessions => {
                self.app_settings.general.restore_sessions_on_startup =
                    !self.app_settings.general.restore_sessions_on_startup;
                self.save_git(ctx);
            }
            SettingsAction::CompleteSetupGuide => {
                self.app_settings.general.setup_guide_completed = true;
                self.save_git(ctx);
            }
            SettingsAction::ResetSetupGuide => {
                self.app_settings.general.setup_guide_completed = false;
                self.app_settings.general.setup_guide_dismissed_items.clear();
                self.save_git(ctx);
            }
            SettingsAction::DismissSetupGuideItem(id) => {
                self.app_settings.general.dismiss_setup_item(id);
                // Dismissing every step completes the guide.
                if setup_guide_steps()
                    .iter()
                    .all(|(step, _, _)| self.app_settings.general.setup_item_dismissed(step))
                {
                    self.app_settings.general.setup_guide_completed = true;
                }
                self.save_git(ctx);
            }
            SettingsAction::UndismissSetupGuideItem(id) => {
                self.app_settings.general.undismiss_setup_item(id);
                self.save_git(ctx);
            }
            SettingsAction::SelectRepoProject(project) => {
                self.selected_repo_project = Some(project.clone());
                self.ensure_repo_editors(ctx, project);
                ctx.notify();
            }
            SettingsAction::ClearRepoOverride(project) => {
                RepoSettingsStore::clear(project);
                ctx.notify();
            }
            SettingsAction::SetRepoBaseRef { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.base_ref = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoBasePath { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.base_path = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoHookMode { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.hook_mode = value.clone());
                ctx.notify();
            }
            SettingsAction::AddRepoHookScript { project } => {
                let value = self.repo_hook_draft.trim().to_string();
                if !value.is_empty() {
                    RepoSettingsStore::mutate(project, |r| {
                        if !r.hook_scripts.contains(&value) {
                            r.hook_scripts.push(value.clone());
                        }
                    });
                }
                self.repo_hook_draft.clear();
                if let Some(editor) = &self.repo_hook_draft_editor {
                    ctx.update_view(editor, |e, ectx| e.set_text("", ectx));
                }
                ctx.notify();
            }
            SettingsAction::RemoveRepoHookScript { project, index } => {
                RepoSettingsStore::mutate(project, |r| {
                    if *index < r.hook_scripts.len() {
                        r.hook_scripts.remove(*index);
                    }
                });
                ctx.notify();
            }
            SettingsAction::SetRepoIssueSource { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.issue_source = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoGhAccount { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.gh_account = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoForkSync { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.fork_sync = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoIdentity { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.identity = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoVisibility { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.visibility = value.clone());
                ctx.notify();
            }
            SettingsAction::AddRepoSymlink { project } => {
                let value = self.repo_symlink_draft.trim().to_string();
                if !value.is_empty() {
                    RepoSettingsStore::mutate(project, |r| {
                        if !r.symlink_paths.contains(&value) {
                            r.symlink_paths.push(value.clone());
                        }
                    });
                }
                self.repo_symlink_draft.clear();
                if let Some(editor) = &self.repo_symlink_draft_editor {
                    ctx.update_view(editor, |e, ectx| e.set_text("", ectx));
                }
                ctx.notify();
            }
            SettingsAction::RemoveRepoSymlink { project, index } => {
                RepoSettingsStore::mutate(project, |r| {
                    if *index < r.symlink_paths.len() {
                        r.symlink_paths.remove(*index);
                    }
                });
                ctx.notify();
            }
            SettingsAction::SetRepoGroupId { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.group_id = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoOrder { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.order = *value);
                ctx.notify();
            }
            SettingsAction::SetRepoHostSetup { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.host_setup_method = value.clone());
                ctx.notify();
            }
            SettingsAction::SetRepoWindowsRuntime { project, value } => {
                RepoSettingsStore::mutate(project, |r| r.windows_runtime = value.clone());
                ctx.notify();
            }
            _ => {}
        }
    }
}

/// Open `path` in `app` (or the default handler when `app` is empty).
/// At minimum the configured apps are persisted and the first app is used
/// for the Projects "Open in" menu; full custom commands run through the
/// shell so `app` may be a binary name or an `open -a` style command.
pub fn open_path_in_app(path: &str, app: &str) {
    let app = app.trim();
    #[cfg(not(target_family = "wasm"))]
    {
        use crate::process_ext::NoWindow;
        let mut command = std::process::Command::new("open");
        command.no_window();
        if !app.is_empty() {
            command.args(["-a", app]);
        }
        command.arg(path);
        let _ = command.output();
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = (path, app);
    }
}
