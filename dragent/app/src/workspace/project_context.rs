//! Project sidebar context menu and project-level workspace actions.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;

use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Hoverable,
    MainAxisAlignment, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::r#async::Timer;
use warpui::{TypedActionView, UpdateView, ViewContext};
use warpui_core::elements::MouseStateHandle;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use std::cell::RefCell;
use std::collections::HashMap;

use crate::process_ext::NoWindow;

use super::left_panel::sidebar_mouse;
use super::panel_nav::{menu_item, menu_separator};
use super::pane_layout::PaneLayout;
use super::project_naming::{
    belongs_to_project, default_pane_cwd, linked_worktree_branch, parse_worktree_list,
    same_folder, worktree_main_checkout,
};
use super::view::{PaneTab, WorkspaceView};
use super::WorkspaceAction;
use crate::icons::AppIcon;
use crate::theme::Theme;

fn importable_worktrees(
    output: &str,
    project: &Path,
    known: &HashSet<PathBuf>,
) -> Vec<(PathBuf, String)> {
    let project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let known = known
        .iter()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    parse_worktree_list(output)
        .into_iter()
        .filter(|(path, _)| {
            let Ok(path) = path.canonicalize() else {
                return false;
            };
            path != project && !known.contains(&path) && seen.insert(path)
        })
        .collect()
}

/// A tab an earlier import left in another workspace: scoped to a linked
/// worktree that isn't this workspace, and still named after its branch.
fn is_imported_worktree_tab(tab: &PaneTab, workspace_root: &Path) -> bool {
    let (Some(label), Some(root)) = (&tab.custom_label, &tab.project_root) else {
        return false;
    };
    if root == workspace_root {
        return false;
    }
    linked_worktree_branch(root).as_ref() == Some(label)
        && root.canonicalize().ok().as_deref() != Some(workspace_root)
}

/// Remove such tabs from one surface, which keeps at least one tab and a valid
/// selection. `active_moved` reports that the selected tab was among them.
fn take_imported_tabs(
    panes: &mut Vec<PaneTab>,
    split_tree: &mut Option<PaneLayout>,
    active_tab_id: &mut usize,
    active_pane_id: &mut Option<warpui::EntityId>,
    workspace_root: &Path,
    active_moved: &mut bool,
) -> Vec<PaneTab> {
    let in_split = |id: usize, layout: &Option<PaneLayout>| {
        layout
            .as_ref()
            .is_some_and(|layout| layout.pane_count() > 1 && layout.contains(id))
    };
    let mut imported: Vec<usize> = panes
        .iter()
        .filter(|tab| !in_split(tab.id, split_tree) && is_imported_worktree_tab(tab, workspace_root))
        .map(|tab| tab.id)
        .collect();
    // An empty surface has nothing to keep (and `remove(0)` would panic).
    if !imported.is_empty() && imported.len() == panes.len() {
        imported.remove(0);
    }
    if imported.is_empty() {
        return Vec::new();
    }
    let (moved, kept): (Vec<PaneTab>, Vec<PaneTab>) = std::mem::take(panes)
        .into_iter()
        .partition(|tab| imported.contains(&tab.id));
    *panes = kept;
    if imported.contains(active_tab_id) {
        *active_moved = true;
        if let Some(first) = panes.first() {
            *active_tab_id = first.id;
            *active_pane_id = Some(first.pane.id());
        }
    }
    if split_tree
        .as_ref()
        .is_some_and(|layout| imported.iter().any(|id| layout.contains(*id)))
    {
        *split_tree = None;
    }
    moved
}

fn remove_linked_worktrees(
    project: &Path,
    mut roots: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, (Vec<PathBuf>, String)> {
    roots.sort();
    let canonical_project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let mut removed = Vec::new();
    for root in roots {
        if root.canonicalize().ok().as_deref() == Some(canonical_project.as_path())
            // Compared as written when the project's folder is gone, or its
            // worktrees could never be removed.
            || !worktree_main_checkout(&root)
                .map(|path| path.canonicalize().unwrap_or(path))
                .is_some_and(|main| same_folder(&main, &canonical_project))
        {
            return Err((
                removed,
                format!(
                    "{} is not a linked worktree of this project",
                    root.display()
                ),
            ));
        }
        if let Err(error) = crate::git_status::worktree::force_remove_worktree(project, &root) {
            return Err((removed, format!("Could not remove {}: {error}", root.display())));
        }
        removed.push(root);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_removes_linked_worktree_and_preserves_project_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let worktree = temp.path().join("worktree");
        std::fs::create_dir(&project).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&project)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "start",
        ]);
        git(&[
            "worktree",
            "add",
            "-qb",
            "workspace/test",
            worktree.to_str().unwrap(),
        ]);
        std::fs::write(worktree.join("uncommitted.txt"), "draft").unwrap();

        assert!(remove_linked_worktrees(&project, vec![project.clone()]).is_err());
        assert!(project.is_dir());
        assert_eq!(
            remove_linked_worktrees(&project, vec![worktree.clone()]).unwrap(),
            vec![worktree.clone()]
        );
        assert!(!worktree.exists());
        assert!(project.join(".git").is_dir());
    }

    #[test]
    fn import_from_linked_worktree_excludes_main_and_open_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let clicked = temp.path().join("clicked");
        let already_open = temp.path().join("open");
        let new = temp.path().join("new");
        for path in [&project, &clicked, &already_open, &new] {
            std::fs::create_dir(path).unwrap();
        }
        let output = format!("worktree {}\nbranch refs/heads/main\n\nworktree {}\nbranch refs/heads/clicked\n\nworktree {}\nbranch refs/heads/open\n\nworktree {}\nbranch refs/heads/new\n\n", project.display(), clicked.display(), already_open.display(), new.display());
        let known = HashSet::from([clicked, already_open]);
        assert_eq!(
            importable_worktrees(&output, &project, &known),
            vec![(new, "new".into())]
        );
    }
}

impl WorkspaceView {
    /// Open one queued worktree as a workspace of its own: the sidebar lists
    /// workspaces, so a tab added to the open one would never appear there.
    /// Ends on the last one opened.
    pub(super) fn import_next_project_worktree(
        &mut self,
        mut pending: VecDeque<(PathBuf, String)>,
        mut last_surface: Option<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some((path, branch)) = pending.pop_front() {
            let root = path.canonicalize().unwrap_or(path);
            if root.is_dir() && self.workspace_for(&root).is_none() {
                let tab = self.create_tab_with_cwd_focus(
                    ctx,
                    Some(branch.clone()),
                    None,
                    root.clone(),
                    Some(root.clone()),
                    false,
                );
                tab.pane.set_workspace_active(false, ctx);
                last_surface = Some(self.park_new_surface(root, branch, tab));
            }
        }
        if !pending.is_empty() {
            ctx.spawn(
                Timer::after(Duration::from_millis(16)),
                move |view, _, ctx| {
                    view.import_next_project_worktree(pending, last_surface, ctx);
                },
            );
        } else {
            self.import_project_worktrees_busy = false;
            if let Some(workspace_tab_id) = last_surface {
                self.project_start_visible = false;
                self.handle_workspace_tab_action(
                    &WorkspaceAction::SelectWorkspaceTab { workspace_tab_id },
                    ctx,
                );
                self.sync_left_panel(ctx);
                self.save_session(ctx);
            }
        }
        ctx.notify();
    }

    /// Earlier imports added each worktree as a tab of whichever workspace was
    /// open, so it never reached the sidebar. Give those tabs the workspace
    /// they were meant to have. Tabs the user arranged into a split stay put.
    pub(crate) fn rehome_imported_worktree_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        let surfaces: Vec<(usize, PathBuf)> = self
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace
                    .tab_ids
                    .iter()
                    .map(|surface| (*surface, workspace.root.clone()))
            })
            .collect();
        for (surface_id, root) in surfaces {
            let active = surface_id == self.active_workspace_tab_id;
            let mut moved = Vec::new();
            let mut active_moved = false;
            if active {
                moved = take_imported_tabs(
                    &mut self.tabs,
                    &mut self.split_tree,
                    &mut self.active_tab_id,
                    &mut self.active_pane_id,
                    &root,
                    &mut active_moved,
                );
            } else if let Some(surface) = self
                .parked_workspace_tabs
                .iter_mut()
                .find(|surface| surface.id == surface_id)
            {
                moved = take_imported_tabs(
                    &mut surface.panes,
                    &mut surface.split_tree,
                    &mut surface.active_tab_id,
                    &mut surface.active_pane_id,
                    &root,
                    &mut active_moved,
                );
            }
            for tab in moved {
                let Some(tab_root) = tab.project_root.clone() else {
                    continue;
                };
                let tab_root = tab_root.canonicalize().unwrap_or(tab_root);
                if active {
                    tab.pane.set_workspace_active(false, ctx);
                }
                match self.workspace_for(&tab_root) {
                    Some(workspace) => self.add_parked_surface(workspace, tab),
                    None => {
                        let title = tab.custom_label.clone().unwrap_or_default();
                        self.park_new_surface(tab_root, title, tab);
                    }
                }
            }
            if active && active_moved {
                self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
                self.update_active_pane(ctx);
            }
        }
    }

    pub(crate) fn handle_project_context_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ToggleProjectContextMenu { root } => {
                self.project_context_menu_root = (self.project_context_menu_root.as_ref()
                    != Some(root)
                    && self.sidebar_project_roots().contains(root))
                .then(|| root.clone());
                ctx.notify();
            }
            WorkspaceAction::CloseProjectContextMenu => {
                self.project_context_menu_root = None;
                ctx.notify();
            }
            WorkspaceAction::OpenProjectInFinder { root } => {
                self.project_context_menu_root = None;
                if !root.is_dir() {
                    return;
                }
                #[cfg(target_os = "macos")]
                let mut command = Command::new("open");
                #[cfg(target_os = "linux")]
                let mut command = Command::new("xdg-open");
                #[cfg(target_os = "windows")]
                let mut command = Command::new("explorer");
                if let Err(error) = command.arg(root).no_window().spawn() {
                    log::warn!("[project-menu] could not open {}: {error}", root.display());
                }
                ctx.notify();
            }
            WorkspaceAction::NewProjectGroup { root } => {
                self.project_context_menu_root = None;
                if !self.sidebar_project_roots().contains(root) {
                    return;
                }
                self.new_project_group_root = Some(root.clone());
                self.project_group_error = None;
                let editor = self.project_group_name_editor.clone();
                ctx.update_view(&editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                    editor_ctx.focus_self();
                });
                ctx.notify();
            }
            WorkspaceAction::CancelProjectGroup => {
                self.new_project_group_root = None;
                self.project_group_error = None;
                ctx.notify();
            }
            WorkspaceAction::RemoveProjectGroup { root, group } => {
                if self.project_removal_busy { return; }
                if !self.sidebar_project_roots().contains(root) { return; }
                self.pending_project_removal = Some((root.clone(), true));
                self.pending_project_group_removal = Some(group.clone());
                self.project_removal_error = None;
                ctx.notify();
            }
            WorkspaceAction::SubmitProjectGroup => {
                let Some(root) = self.new_project_group_root.clone() else {
                    return;
                };
                let name = ctx.update_view(&self.project_group_name_editor, |editor, _| {
                    editor.get_text().trim().to_string()
                });
                if self.workspace_trust.add_group(&root, &name) {
                    self.new_project_group_root = None;
                    self.project_group_error = None;
                    self.sync_left_panel(ctx);
                    ctx.notify();
                } else {
                    self.project_group_error = Some(if name.is_empty() {
                        "Enter a group name.".into()
                    } else {
                        "A group with this name already exists.".into()
                    });
                    ctx.notify();
                }
            }
            WorkspaceAction::MoveWorkspaceToGroup {
                tab_id,
                project_root,
                group,
            } => {
                let workspace_id = self.left_panel_view.read(ctx, |panel, _| {
                    panel
                        .tabs
                        .iter()
                        .find(|row| row.id == *tab_id)
                        .and_then(|row| row.workspace_id)
                });
                let Some(root) = self
                    .workspaces
                    .iter()
                    .find(|workspace| Some(workspace.id) == workspace_id)
                    .map(|workspace| workspace.root.clone())
                else {
                    return;
                };
                let project = self.project_root_of(&root);
                if &project != project_root {
                    log::warn!(
                        "[sidebar-group] rejected move: workspace={} shown_under={} target={}",
                        root.display(),
                        project.display(),
                        project_root.display()
                    );
                    return;
                }
                if group
                    .as_ref()
                    .is_some_and(|name| !self.workspace_trust.groups(project_root).contains(name))
                {
                    log::warn!(
                        "[sidebar-group] rejected move: group {:?} is absent from {}",
                        group,
                        project_root.display()
                    );
                    return;
                }
                self.workspace_trust
                    .set_workspace_group(&root, group.clone());
                log::info!(
                    "[sidebar-group] moved workspace={} project={} group={:?}",
                    root.display(),
                    project_root.display(),
                    group
                );
                ctx.update_view(&self.left_panel_view, |panel, panel_ctx| {
                    panel.sidebar_menu_tab_id = None;
                    panel_ctx.notify();
                });
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::ReorderWorkspaceTab {
                tab_id,
                target_tab_id,
                project_root,
                group,
            } => {
                let (source_id, target_id) = self.left_panel_view.read(ctx, |panel, _| {
                    let owner = |id| {
                        panel
                            .tabs
                            .iter()
                            .find(|row| row.id == id)
                            .and_then(|row| row.workspace_id)
                    };
                    (owner(*tab_id), owner(*target_tab_id))
                });
                let Some(source_pos) = self
                    .workspaces
                    .iter()
                    .position(|workspace| Some(workspace.id) == source_id)
                else {
                    return;
                };
                let Some(target_pos) = self
                    .workspaces
                    .iter()
                    .position(|workspace| Some(workspace.id) == target_id)
                else {
                    return;
                };
                if source_pos == target_pos {
                    return;
                }
                let source_root = self.workspaces[source_pos].root.clone();
                let target_root = self.workspaces[target_pos].root.clone();
                if &self.project_root_of(&source_root) != project_root
                    || &self.project_root_of(&target_root) != project_root
                {
                    return;
                }
                if group
                    .as_ref()
                    .is_some_and(|name| !self.workspace_trust.groups(project_root).contains(name))
                {
                    return;
                }
                self.workspace_trust
                    .set_workspace_group(&source_root, group.clone());
                let workspace = self.workspaces.remove(source_pos);
                let new_target_pos = self
                    .workspaces
                    .iter()
                    .position(|candidate| Some(candidate.id) == target_id)
                    .unwrap();
                // In the drag direction, insert after the row below or before the row above.
                let insert_at = if source_pos < target_pos {
                    new_target_pos + 1
                } else {
                    new_target_pos
                };
                self.workspaces.insert(insert_at, workspace);
                ctx.update_view(&self.left_panel_view, |panel, panel_ctx| {
                    panel.sidebar_sort = super::left_panel::SidebarSortMode::Manual;
                    panel.sidebar_menu_tab_id = None;
                    panel_ctx.notify();
                });
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::ReorderProject { root, target_root } => {
                let source = root.to_string_lossy().into_owned();
                let target = target_root.to_string_lossy().into_owned();
                let moved = ctx.update_view(&self.left_panel_view, |panel, panel_ctx| {
                    let moved = panel.move_project(&source, &target);
                    if moved {
                        panel_ctx.notify();
                    }
                    moved
                });
                if moved {
                    self.sync_left_panel(ctx);
                    self.save_session(ctx);
                    ctx.notify();
                }
            }
            WorkspaceAction::ImportProjectWorktrees { root } => {
                self.project_context_menu_root = None;
                if self.import_project_worktrees_busy
                    || !self.open_project_roots.contains(root)
                    || !root.is_dir()
                {
                    return;
                }
                self.import_project_worktrees_busy = true;
                self.import_project_worktrees_error = None;
                let root = worktree_main_checkout(root).unwrap_or_else(|| root.clone());
                let root_for_error = root.clone();
                // Only a workspace counts as imported: a tab inside another
                // workspace doesn't put the worktree in the sidebar.
                let known = self
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.root.clone())
                    .chain(self.open_project_roots.iter().cloned())
                    .collect::<HashSet<_>>();
                ctx.spawn(
                    async move {
                        let output = crate::git_cmd::GitCommand::new()
                            .args(["worktree", "list", "--porcelain"])
                            .current_dir(&root)
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .no_window()
                            .output()
                            .map_err(|error| format!("Could not list worktrees: {error}"))?;
                        if !output.status.success() {
                            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
                        }
                        let worktrees = importable_worktrees(
                            &String::from_utf8_lossy(&output.stdout),
                            &root,
                            &known,
                        );
                        Ok::<_, String>((root, worktrees))
                    },
                    move |view, result, ctx| {
                        view.import_project_worktrees_busy = false;
                        match result {
                            Ok(found) => view.importable_project_worktrees = Some(found),
                            Err(error) => {
                                view.import_project_worktrees_error = Some(error);
                                view.importable_project_worktrees =
                                    Some((root_for_error, Vec::new()));
                            }
                        }
                        ctx.notify();
                    },
                );
            }
            WorkspaceAction::CancelImportProjectWorktrees => {
                if self.import_project_worktrees_busy {
                    return;
                }
                self.importable_project_worktrees = None;
                self.import_project_worktrees_error = None;
                ctx.notify();
            }
            WorkspaceAction::ConfirmImportProjectWorktrees => {
                let Some((_project, worktrees)) = self.importable_project_worktrees.take() else {
                    return;
                };
                if worktrees.is_empty() {
                    return;
                }
                self.import_project_worktrees_busy = true;
                ctx.spawn(
                    Timer::after(Duration::from_millis(16)),
                    move |view, _, ctx| {
                        view.import_next_project_worktree(worktrees.into(), None, ctx);
                    },
                );
                ctx.notify();
            }
            WorkspaceAction::HideProjectFromSidebar { root } => {
                self.project_context_menu_root = None;
                if !self.sidebar_project_roots().contains(root) {
                    return;
                }
                self.workspace_trust.hide(root);
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::ShowProjectInSidebar { root } => {
                if !self.sidebar_project_roots().contains(root) {
                    return;
                }
                self.workspace_trust.show(root);
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::RequestArchiveProject { root }
            | WorkspaceAction::RequestDeleteProject { root } => {
                if self.project_removal_busy { return; }
                self.project_context_menu_root = None;
                if !self.sidebar_project_roots().contains(root)
                    && !self.workspace_trust.recent_roots().contains(root) {
                    return;
                }
                self.pending_project_removal = Some((
                    root.clone(),
                    matches!(action, WorkspaceAction::RequestDeleteProject { .. }),
                ));
                self.pending_project_group_removal = None;
                self.project_removal_error = None;
                ctx.notify();
            }
            WorkspaceAction::CancelProjectRemoval => {
                if self.project_removal_busy {
                    return;
                }
                self.pending_project_removal = None;
                self.pending_project_group_removal = None;
                self.project_removal_error = None;
                ctx.notify();
            }
            WorkspaceAction::ConfirmProjectRemoval => {
                if self.project_removal_busy {
                    return;
                }
                let Some((project, delete)) = self.pending_project_removal.clone() else {
                    return;
                };
                let group = self.pending_project_group_removal.clone();
                let targets = self.workspaces.iter().map(|workspace| workspace.root.clone())
                    .chain(self.tabs.iter().filter_map(|tab| tab.project_root.clone()))
                    .chain(self.parked_workspace_tabs.iter().flat_map(|surface| surface.panes.iter().filter_map(|tab| tab.project_root.clone())))
                    // Or listed under it: the heading is what is being deleted.
                    .filter(|root| belongs_to_project(root, &project) || same_folder(&self.project_root_of(root), &project))
                    .filter(|root| group.as_ref().is_none_or(|name| self.workspace_trust.workspace_group(root) == Some(name.as_str())))
                    .collect::<HashSet<_>>();
                let roots = targets.iter()
                    .filter(|root| !same_folder(root, &project) && worktree_main_checkout(root).is_some_and(|main| same_folder(&main, &project)))
                    .cloned().collect::<HashSet<_>>();
                self.project_removal_busy = true;
                self.project_removal_error = None;
                // End the terminals running in the worktrees first: on Windows
                // a folder that is a running shell's working directory cannot
                // be deleted, so removing it under a live shell always fails.
                let doomed_tabs = self
                    .tabs
                    .iter()
                    .chain(self.parked_workspace_tabs.iter().flat_map(|surface| surface.panes.iter()))
                    .filter(|tab| tab.project_root.as_ref().is_some_and(|root| roots.contains(root)))
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>();
                for tab_id in doomed_tabs {
                    self.close_host_tab_session(tab_id);
                }
                // Deleting (not archiving) forgets everything Dragent stored
                // for the project and its worktrees; located while they exist.
                let mut forgotten = Vec::new();
                if delete && group.is_none() {
                    forgotten.extend(roots.iter().map(|root| super::project_data::ProjectData::locate(root)));
                    forgotten.push(super::project_data::ProjectData::locate(&project));
                }
                ctx.notify();
                let checkout = project.clone();
                ctx.spawn(
                    async move { remove_linked_worktrees(&checkout, roots.into_iter().collect()) },
                    move |view, result, ctx| {
                        view.project_removal_busy = false;
                        let (removed, error) = match result {
                            Ok(removed) => (removed, None),
                            Err((removed, error)) => (removed, Some(error)),
                        };
                        let closed_roots = if error.is_none() { targets.clone() } else { removed.iter().cloned().collect() };
                        let workspaces = view.workspaces.iter()
                            .filter(|workspace| closed_roots.contains(&workspace.root))
                            .map(|workspace| (workspace.id, workspace.active_tab_id))
                            .collect::<Vec<_>>();
                        for (workspace_id, workspace_tab_id) in workspaces {
                            view.pending_close_project_workspace = Some(workspace_id);
                            view.handle_workspace_tab_action(
                                &WorkspaceAction::CloseWorkspaceTabConfirmed { workspace_tab_id }, ctx,
                            );
                        }
                        let ids = view
                            .tabs
                            .iter()
                            .filter(|tab| {
                                tab.project_root.as_ref().is_some_and(|root| {
                                    closed_roots.contains(root)
                                })
                            })
                            .map(|tab| tab.id)
                            .collect::<Vec<_>>();
                        for id in ids {
                            view.handle_action(
                                &WorkspaceAction::CloseTabConfirmed { tab_id: id },
                                ctx,
                            );
                        }
                        let dock_ids = view
                            .document_terminals
                            .iter()
                            .filter(|dock| {
                                closed_roots.contains(&dock.owner_root)
                            })
                            .map(|dock| dock.id)
                            .collect::<Vec<_>>();
                        for id in dock_ids {
                            view.close_document_terminal(id, ctx);
                        }
                        let owner_removed = view.document_owner_root.as_ref().is_some_and(|root| {
                            closed_roots.contains(root)
                        });
                        if owner_removed {
                            view.document_owner_root = None;
                            view.document_center_open = false;
                            view.document_open_paths.clear();
                            view.document_requested_path = None;
                        }
                        if view.tabs.is_empty() && !view.document_center_open {
                            view.project_start_visible = true;
                        }
                        for root in &closed_roots {
                            view.workspace_trust.forget_workspace(root);
                            if same_folder(root, &project) { continue; }
                            view.persistence_writer.delete_workspace_project(
                                dragent_workspace_model::Project::new_for_organisation(
                                    &view.organisation_id,
                                    root.clone(),
                                    0,
                                )
                                .id,
                            );
                        }
                        view.open_project_roots
                            .retain(|root| same_folder(root, &project) || !closed_roots.contains(root));
                        if view
                            .prompt_selected_worktree
                            .as_ref()
                            .is_some_and(|root| removed.contains(root))
                        {
                            view.prompt_selected_worktree = None;
                            view.prompt_branch = None;
                        }
                        if removed.contains(&view.document_index_root) {
                            view.document_index_root = project.clone();
                        }
                        if let Some(error) = error {
                            view.project_removal_error = Some(error);
                            view.sync_left_panel(ctx);
                            view.save_session(ctx);
                        } else {
                            view.pending_project_removal = None;
                            if let Some(group) = &group {
                                view.workspace_trust.remove_group(&project, group);
                                for root in &closed_roots {
                                    if root != &project { view.workspace_trust.forget_project(root); }
                                }
                                view.pending_project_group_removal = None;
                            } else if delete {
                                for data in forgotten {
                                    data.forget(&view.persistence_writer);
                                }
                                view.persistence_writer.delete_workspace_project(
                                    dragent_workspace_model::Project::new_for_organisation(
                                        &view.organisation_id,
                                        project.clone(),
                                        0,
                                    )
                                    .id,
                                );
                                // Every folder listed under the heading goes, however
                                // it is spelled, or the heading would stay.
                                let listed: Vec<PathBuf> = view.open_project_roots.iter()
                                    .filter(|root| belongs_to_project(root, &project) || same_folder(&view.project_root_of(root), &project))
                                    .cloned().collect();
                                view.open_project_roots.retain(|root| !listed.contains(root));
                                // The catalogue keys a project by its path as
                                // written: forget each spelling it was saved under.
                                for root in &listed {
                                    view.persistence_writer.delete_workspace_project(
                                        dragent_workspace_model::Project::new_for_organisation(
                                            &view.organisation_id,
                                            root.clone(),
                                            0,
                                        )
                                        .id,
                                    );
                                }
                                view.workspace_trust.forget_project(&project);
                                // A remote project is also forgotten in Machines;
                                // its files stay on the machine.
                                super::remote_project::forget_remote_project(&project);
                                view.project_start_recent_projects =
                                    view.workspace_trust.recent_roots();
                                if view.document_index_root == project {
                                    view.document_index_root = view
                                        .open_project_roots
                                        .last()
                                        .cloned()
                                        .unwrap_or_else(default_pane_cwd);
                                }
                                if view.prompt_selected_worktree.as_ref() == Some(&project) {
                                    view.prompt_selected_worktree = None;
                                }
                                if view.open_project_roots.is_empty() && view.tabs.is_empty() {
                                    view.project_start_visible = true;
                                }
                            }
                            view.sync_left_panel(ctx);
                            view.save_session(ctx);
                            view.persistence_writer.flush();
                        }
                        ctx.notify();
                    },
                );
            }
            _ => {}
        }
    }
}

pub(crate) fn render_project_context_menu(
    workspace_id: warpui::EntityId,
    theme: &Theme,
    ui: FamilyId,
    root: &Path,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let items = [
        (
            "Open in Finder",
            AppIcon::FolderOpen,
            false,
            WorkspaceAction::OpenProjectInFinder {
                root: root.to_path_buf(),
            },
        ),
        (
            "New Group",
            AppIcon::Plus,
            false,
            WorkspaceAction::NewProjectGroup {
                root: root.to_path_buf(),
            },
        ),
        (
            "Import untracked worktrees",
            AppIcon::GitBranch,
            false,
            WorkspaceAction::ImportProjectWorktrees {
                root: root.to_path_buf(),
            },
        ),
        (
            "Remove from sidebar",
            AppIcon::XClose,
            false,
            WorkspaceAction::HideProjectFromSidebar {
                root: root.to_path_buf(),
            },
        ),
        (
            "Archive all",
            AppIcon::Layers,
            true,
            WorkspaceAction::RequestArchiveProject {
                root: root.to_path_buf(),
            },
        ),
        (
            "Delete Project…",
            AppIcon::XClose,
            true,
            WorkspaceAction::RequestDeleteProject {
                root: root.to_path_buf(),
            },
        ),
    ];
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (index, (label, icon, danger, action)) in items.into_iter().enumerate() {
        if index == 3 || index == 5 {
            column = column.with_child(menu_separator(theme));
        }
        column = column.with_child(menu_item(
            ui,
            theme,
            workspace_id,
            label,
            icon,
            danger,
            action,
            sidebar_mouse(
                states,
                format!("project_context_{}_{}", root.display(), index),
            ),
        ));
    }
    Container::new(
        Container::new(column.finish())
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_background_color(theme.sidebar_menu_background())
    .with_border(Border::all(1.0).with_border_color(theme.border()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

impl WorkspaceView {
    fn project_dialog_button(
        &self,
        theme: &Theme,
        ui: FamilyId,
        label: &str,
        action: WorkspaceAction,
        danger: bool,
        enabled: bool,
        key: &str,
    ) -> Box<dyn Element> {
        let workspace_id = self.workspace_view_id;
        let mouse = sidebar_mouse(
            &self.new_tab_picker_mouse_states,
            format!("project_dialog_{key}"),
        );
        let label = label.to_string();
        let red = theme.ui_red();
        let hoverable = Hoverable::new(mouse, move |state| {
            let background = if danger {
                if state.is_hovered() && enabled {
                    ColorU::new(red.r, red.g, red.b, 225)
                } else {
                    red
                }
            } else if state.is_hovered() && enabled {
                theme.sidebar_menu_hover()
            } else {
                theme.surface_2()
            };
            let text_color = if danger {
                if theme.is_dark {
                    ColorU::new(0, 0, 0, 255)
                } else {
                    ColorU::white()
                }
            } else {
                theme.main_text()
            };
            Container::new(
                Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                    .with_color(text_color)
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish(),
            )
            .with_background_color(background)
            .with_border(Border::all(1.0).with_border_color(if danger {
                red
            } else {
                theme.border()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .finish()
        });
        if enabled {
            Box::new(
                hoverable
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action_for(workspace_id, action.clone())
                    }),
            )
        } else {
            Box::new(hoverable)
        }
    }

    fn project_dialog(
        &self,
        theme: &Theme,
        ui: FamilyId,
        title: &str,
        body: &str,
        middle: Option<Box<dyn Element>>,
        cancel: WorkspaceAction,
        cancel_enabled: bool,
        confirm: Option<(String, WorkspaceAction, bool, bool)>,
    ) -> Box<dyn Element> {
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(14.0)
            .with_child(
                Text::new(title.to_string(), ui, crate::theme::text_size(18.0))
                    .with_color(theme.main_text())
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            )
            .with_child(
                Text::new(body.to_string(), ui, crate::theme::text_size(12.5))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        if let Some(middle) = middle {
            column = column.with_child(middle);
        }
        let mut buttons = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(8.0)
            .with_child(self.project_dialog_button(
                theme,
                ui,
                "Cancel",
                cancel,
                false,
                cancel_enabled,
                "cancel",
            ));
        if let Some((label, action, danger, enabled)) = confirm {
            buttons = buttons.with_child(
                self.project_dialog_button(theme, ui, &label, action, danger, enabled, "confirm"),
            );
        }
        column = column.with_child(
            Container::new(buttons.finish())
                .with_padding_top(8.0)
                .finish(),
        );
        let card = Container::new(column.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.border()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
            .with_padding_left(24.0)
            .with_padding_right(24.0)
            .with_padding_top(24.0)
            .with_padding_bottom(22.0)
            .finish();
        let centered = ConstrainedBox::new(card).with_max_width(480.0).finish();
        Container::new(Align::new(centered).finish())
            .with_background_color(ColorU::new(0, 0, 0, 165))
            .finish()
    }

    pub(crate) fn render_project_group_dialog(
        &self,
        theme: &Theme,
        ui: FamilyId,
    ) -> Box<dyn Element> {
        let editor = crate::shared_input::shared_input(
            theme,
            &self.project_group_name_editor,
            crate::shared_input::SharedInputOpts {
                ..Default::default()
            },
        );
        let body = self
            .project_group_error
            .as_deref()
            .unwrap_or("Name the group for this project.");
        self.project_dialog(
            theme,
            ui,
            "New group",
            body,
            Some(editor),
            WorkspaceAction::CancelProjectGroup,
            true,
            Some((
                "Create group".into(),
                WorkspaceAction::SubmitProjectGroup,
                false,
                true,
            )),
        )
    }

    pub(crate) fn render_project_import_dialog(
        &self,
        theme: &Theme,
        ui: FamilyId,
    ) -> Box<dyn Element> {
        let (_, worktrees) = self
            .importable_project_worktrees
            .as_ref()
            .expect("import dialog state");
        let mut body = if worktrees.is_empty() {
            "No untracked worktrees found.".to_string()
        } else {
            format!("Import {} worktree(s) as workspaces:\n", worktrees.len())
        };
        for (path, branch) in worktrees.iter().take(8) {
            body.push_str(&format!("\n{branch} · {}", path.display()));
        }
        if worktrees.len() > 8 {
            body.push_str(&format!("\n…and {} more", worktrees.len() - 8));
        }

        self.project_dialog(
            theme,
            ui,
            "Import untracked worktrees",
            &body,
            None,
            WorkspaceAction::CancelImportProjectWorktrees,
            true,
            (!worktrees.is_empty()).then(|| {
                (
                    "Import worktrees".into(),
                    WorkspaceAction::ConfirmImportProjectWorktrees,
                    false,
                    true,
                )
            }),
        )
    }

    pub(crate) fn render_project_removal_dialog(
        &self,
        theme: &Theme,
        ui: FamilyId,
    ) -> Box<dyn Element> {
        let (root, delete) = self
            .pending_project_removal
            .as_ref()
            .expect("removal dialog state");
        let group = self.pending_project_group_removal.as_deref();
        let count = self
            .workspaces
            .iter()
            .filter(|workspace| belongs_to_project(&workspace.root, root))
            .filter(|workspace| group.is_none_or(|name| self.workspace_trust.workspace_group(&workspace.root) == Some(name)))
            .count();
        let project_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("this project");
        let body = if let Some(group) = group {
            format!("Delete {group} and all {count} workspace{} inside it. Running terminals and agents will be stopped.", if count == 1 { "" } else { "s" })
        } else if *delete {
            format!(
                "Remove {project_name} and close {count} workspace{} in Dragent. Running terminals and agents will be stopped.",
                if count == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "Archive {count} workspace{} for {project_name}.",
                if count == 1 { "" } else { "s" }
            )
        };
        let red = theme.ui_red();
        let warning = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(Text::new("Linked worktrees may be deleted".to_string(), ui, crate::theme::text_size(13.0))
                    .with_color(red)
                    .with_style(Properties::default().weight(Weight::Semibold)).finish())
                .with_child(Text::new("Any linked worktrees opened as workspaces will be removed, including their uncommitted changes. This cannot be undone.".to_string(), ui, crate::theme::text_size(12.5))
                    .with_color(theme.main_text()).finish())
                .with_child(Text::new("The project folder shown below stays on disk.".to_string(), ui, crate::theme::text_size(12.5))
                    .with_color(theme.sub_text()).finish())
                .finish(),
        )
        .with_background_color(ColorU::new(red.r, red.g, red.b, 18))
        .with_border(Border::all(1.0).with_border_color(ColorU::new(red.r, red.g, red.b, 90)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(14.0).with_padding_right(14.0)
        .with_padding_top(13.0).with_padding_bottom(13.0)
        .finish();
        let mut details = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(warning)
            .with_child(
                Text::new(root.display().to_string(), ui, crate::theme::text_size(11.0))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );

        self.project_dialog(
            theme,
            ui,
            if group.is_some() {
                "Delete group and its workspaces?"
            } else if *delete {
                "Delete Project from Dragent?"
            } else {
                "Archive all workspaces?"
            },
            &body,
            Some(details.finish()),
            WorkspaceAction::CancelProjectRemoval,
            !self.project_removal_busy,
            Some((
                if self.project_removal_busy {
                    "Working…"
                } else if group.is_some() {
                    "Delete group"
                } else if *delete {
                    "Delete Project"
                } else {
                    "Archive all"
                }
                .into(),
                WorkspaceAction::ConfirmProjectRemoval,
                true,
                !self.project_removal_busy,
            )),
        )
    }
}
