//! Importing worktrees through the real workspace view, in an isolated process
//! with a private Dragent home and HOME.

use super::view::WorkspaceView;
use crate::realtime::manager::SessionManager;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use warpui::platform::WindowStyle;
use warpui::{App, UpdateView};

#[test]
fn imported_worktrees_become_workspaces() {
    let sandbox = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::import_worktree_tests::isolated_imported_worktrees_become_workspaces",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DRAGENT_HOME", sandbox.path())
        .env("HOME", sandbox.path())
        .env("DRAGENT_INDEX_DISABLE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn worktree(project: &Path, parent: &Path, branch: &str) -> PathBuf {
    let path = parent.join(branch);
    git(project, &["worktree", "add", "-b", branch, path.to_str().unwrap()]);
    path.canonicalize().unwrap()
}

/// Every tab scope of the workspace rooted at `root`, across its surfaces.
fn tab_roots(view: &WorkspaceView, root: &Path) -> Vec<PathBuf> {
    let Some(workspace) = view.workspaces.iter().find(|workspace| workspace.root == root) else {
        return Vec::new();
    };
    workspace
        .tab_ids
        .iter()
        .flat_map(|surface| {
            if *surface == view.active_workspace_tab_id {
                view.tabs.iter().collect::<Vec<_>>()
            } else {
                view.parked_workspace_tabs
                    .iter()
                    .filter(|parked| parked.id == *surface)
                    .flat_map(|parked| parked.panes.iter())
                    .collect()
            }
        })
        .filter_map(|tab| tab.project_root.clone())
        .collect()
}

#[test]
#[ignore = "launched by imported_worktrees_become_workspaces in a private DRAGENT_HOME"]
fn isolated_imported_worktrees_become_workspaces() {
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let home = dragent_paths::home();
        let parent = home.join("fixtures");
        let project = parent.join("project");
        let other = parent.join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        git(&project, &["init", "-q", "-b", "main"]);
        git(&project, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let project = project.canonicalize().unwrap();
        let other = other.canonicalize().unwrap();
        let one = worktree(&project, &parent, "one");
        let two = worktree(&project, &parent, "two");
        let renamed = worktree(&project, &parent, "three");
        let stray = worktree(&project, &parent, "four");

        app.update(|ctx| {
            ctx.add_singleton_model(crate::auth::AuthManager::preview);
            ctx.add_singleton_model(crate::settings::AccountStore::new);
            ctx.add_singleton_model(crate::settings::OrgStore::new);
            ctx.add_singleton_model(crate::organisation_context::OrganisationContext::preview);
            ctx.add_singleton_model(crate::settings::machines_store::MachinesStore::preview);
            ctx.add_singleton_model(crate::automations::AutomationRuntime::preview);
            ctx.add_singleton_model(crate::cli::inbox::CliInbox::preview);
            ctx.add_singleton_model(|_| SessionManager {
                session: None,
                forwards: Vec::new(),
                forward_error: None,
                forward_busy: false,
                #[cfg(not(target_family = "wasm"))]
                _forward_tunnel: None,
                idle_epoch: 0,
                share_busy: None,
                viewer_theme_id: None,
            });
        });
        let (_window, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            WorkspaceView::build_prepared(
                ctx,
                true,
                (None, dragent_persistence::WriterHandles::noop()),
            )
        });
        workspace.update(&mut app, |view, ctx| {
            view.persists_session_snapshot = false;
            // Another project's workspace is what's open during the import.
            let tab = view.create_tab_with_cwd_focus(
                ctx,
                None,
                None,
                other.clone(),
                Some(other.clone()),
                false,
            );
            view.active_tab_id = tab.id;
            view.active_pane_id = Some(tab.pane.id());
            view.tabs = vec![tab];
            view.workspaces.clear();
            view.reconcile_workspace_ownership();
            assert_eq!(tab_roots(view, &other), vec![other.clone()]);

            for (path, branch) in [(&one, "one"), (&two, "two")] {
                view.import_next_project_worktree(
                    VecDeque::from([(path.clone(), branch.to_string())]),
                    None,
                    ctx,
                );
                assert!(view.workspace_for(path).is_some(), "{branch} has no workspace");
                assert_eq!(&tab_roots(view, path), &[path.clone()]);
            }
            assert_eq!(tab_roots(view, &other), vec![other.clone()], "import touched the open workspace");
            // The last one imported is what's on screen.
            assert_eq!(view.tabs[0].project_root.as_ref(), Some(&two));

            // Importing it again changes nothing.
            let count = view.workspaces.len();
            view.import_next_project_worktree(
                VecDeque::from([(one.clone(), "one".to_string())]),
                None,
                ctx,
            );
            assert_eq!(view.workspaces.len(), count);

            // What an earlier build left behind: worktree tabs inside `other`.
            let other_surface = view
                .workspaces
                .iter()
                .find(|workspace| workspace.root == other)
                .unwrap()
                .tab_ids[0];
            let stray_tab = view.create_tab_with_cwd_focus(
                ctx,
                Some("four".into()),
                None,
                stray.clone(),
                Some(stray.clone()),
                false,
            );
            // The user named this one, so it isn't an import to move.
            let named_tab = view.create_tab_with_cwd_focus(
                ctx,
                Some("my notes".into()),
                None,
                renamed.clone(),
                Some(renamed.clone()),
                false,
            );
            let surface = view
                .parked_workspace_tabs
                .iter_mut()
                .find(|surface| surface.id == other_surface)
                .unwrap();
            surface.panes.push(stray_tab);
            surface.panes.push(named_tab);

            view.rehome_imported_worktree_tabs(ctx);
            assert_eq!(&tab_roots(view, &stray), &[stray.clone()], "stray tab wasn't moved");
            assert_eq!(
                tab_roots(view, &other),
                vec![other.clone(), renamed.clone()],
                "the workspace kept the wrong tabs"
            );
            assert!(view.workspace_for(&renamed).is_none());

            // Running it again moves nothing more.
            let count = view.workspaces.len();
            view.rehome_imported_worktree_tabs(ctx);
            assert_eq!(view.workspaces.len(), count);

            // A left-behind tab that's selected in the surface on screen: the
            // surface falls back to the tab that really belongs to it.
            let selected = worktree(&project, &parent, "five");
            let two_tab = view.tabs[0].id;
            let tab = view.create_tab_with_cwd_focus(
                ctx,
                Some("five".into()),
                None,
                selected.clone(),
                Some(selected.clone()),
                false,
            );
            view.active_tab_id = tab.id;
            view.active_pane_id = Some(tab.pane.id());
            view.tabs.push(tab);
            view.rehome_imported_worktree_tabs(ctx);
            assert_eq!(&tab_roots(view, &selected), &[selected.clone()]);
            assert_eq!(view.active_tab_id, two_tab);
            assert_eq!(view.tabs.len(), 1);
        });
    });
}
