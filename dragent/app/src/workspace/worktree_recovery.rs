//! Recover navigation from surviving Git checkouts when UI metadata is lost.
//! This does not claim to recover an agent process or its old terminal identity.
use super::project_naming::{linked_worktree_branch, worktree_main_checkout, worktrees_root};
use dragent_persistence::{
    PersistedSession, SerializedDocumentSession, SerializedPaneLayout, SerializedSession,
    SerializedTab, SerializedWorkspace, SerializedWorkspaceTab,
};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(super) fn recover_workspace_checkouts(
    saved: Option<PersistedSession>,
    local_workspaces: &[SerializedWorkspace],
) -> Option<PersistedSession> {
    let candidates = discover(&worktrees_root());
    if candidates.is_empty() && discover_local_checkouts(local_workspaces).is_empty() {
        return saved;
    }
    let mut saved = saved.unwrap_or_else(|| PersistedSession {
        session: SerializedSession {
            tabs: Vec::new(),
            active_tab_id: 0,
            active_pane_uuid: None,
            chrome: Default::default(),
            document: SerializedDocumentSession {
                project_start_visible: true,
                ..Default::default()
            },
            layouts: Default::default(),
        },
        blocks: Default::default(),
        agent_blocks: Default::default(),
        working_copies: Vec::new(),
        workspace_work_items: Vec::new(),
        workspace_events: Vec::new(),
        canonical_workspace_events: Vec::new(),
    });
    recover_local_workspaces(&mut saved.session, local_workspaces);
    recover(&mut saved.session, candidates);
    Some(saved)
}

fn discover(managed: &Path) -> Vec<(PathBuf, PathBuf, String)> {
    let mut candidates = Vec::new();
    let Ok(projects) = std::fs::read_dir(managed) else {
        return candidates;
    };
    for project in projects.flatten() {
        let Ok(checkouts) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for checkout in checkouts.flatten() {
            let path = checkout.path();
            // Git's .git pointer and branch HEAD prove this is a linked
            // checkout. Detached PR previews and arbitrary folders are skipped.
            let (Some(main), Some(branch)) =
                (worktree_main_checkout(&path), linked_worktree_branch(&path))
            else {
                continue;
            };
            if !main.is_dir() || !path.is_dir() {
                continue;
            }
            candidates.push((path.canonicalize().unwrap_or(path), main, branch));
        }
    }
    candidates.sort();
    candidates
}

/// Local-branch workspaces use the main project directory, so they have no
/// separate folder beneath worktrees/. Require a durable workspace record, not
/// a recent/open project or the mere presence of a local branch. HEAD validates
/// that the checkout still exists; recovery never switches its branch.
fn discover_local_checkouts(workspaces: &[SerializedWorkspace]) -> Vec<(PathBuf, PathBuf, String)> {
    let mut candidates: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    for workspace in workspaces {
        let Ok(root) = workspace.root.canonicalize() else {
            continue;
        };
        if candidates.iter().any(|(path, _, _)| path == &root) {
            continue;
        }
        let git = root.join(".git");
        // Linked checkouts are recovered by discover(), not projected twice.
        if !git.is_dir() {
            continue;
        }
        let Ok(head) = std::fs::read_to_string(git.join("HEAD")) else {
            continue;
        };
        let Some(branch) = head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .filter(|branch| !branch.is_empty())
        else {
            continue;
        };
        let title = if workspace.title.is_empty() {
            branch.into()
        } else {
            workspace.title.clone()
        };
        candidates.push((root.clone(), root, title));
    }
    candidates
}

fn recover_local_workspaces(session: &mut SerializedSession, records: &[SerializedWorkspace]) {
    for record in records {
        let existed = session
            .document
            .workspaces
            .iter()
            .any(|workspace| same_path(&workspace.root, &record.root));
        recover(
            session,
            discover_local_checkouts(std::slice::from_ref(record)),
        );
        // Preserve the recorded workspace ID when rebuilding lost navigation,
        // but never replace an existing owner's identity or collide with it.
        if !existed
            && !session.document.workspaces.iter().any(|workspace| {
                workspace.id == record.id && !same_path(&workspace.root, &record.root)
            })
        {
            if let Some(workspace) = session
                .document
                .workspaces
                .iter_mut()
                .find(|workspace| same_path(&workspace.root, &record.root))
            {
                workspace.id = record.id;
            }
        }
    }
}

fn recover(session: &mut SerializedSession, candidates: Vec<(PathBuf, PathBuf, String)>) {
    let mut next_pane = session
        .tabs
        .iter()
        .chain(
            session
                .document
                .parked_workspace_tabs
                .iter()
                .flat_map(|surface| surface.tabs.iter()),
        )
        .map(|pane| pane.id)
        .max()
        .unwrap_or(0)
        + 1;
    let mut next_surface = session
        .document
        .parked_workspace_tabs
        .iter()
        .map(|surface| surface.id)
        .chain(session.document.active_workspace_tab_id)
        .chain(session.document.workspace_tab_order.iter().copied())
        .chain(
            session
                .document
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.tab_ids.iter().copied()),
        )
        .max()
        // The builder reserves surface 1 for a metadata-free front screen.
        .unwrap_or(1)
        .max(1)
        + 1;
    let mut next_workspace = session
        .document
        .workspaces
        .iter()
        .map(|workspace| workspace.id)
        .max()
        .unwrap_or(0)
        + 1;
    for (root, project, title) in candidates {
        let document = &mut session.document;
        if document
            .closed_workspace_roots
            .iter()
            .any(|closed| same_path(closed, &root))
            || document
                .workspaces
                .iter()
                .any(|workspace| same_path(&workspace.root, &root))
            || document.workspace_tab_work_items.iter().any(|item| {
                item.project_root
                    .as_ref()
                    .is_some_and(|path| same_path(path, &root))
            })
            || session
                .tabs
                .iter()
                .chain(
                    document
                        .parked_workspace_tabs
                        .iter()
                        .flat_map(|surface| surface.tabs.iter()),
                )
                .any(|pane| layout_has_root(&pane.root_layout, &root))
        {
            continue;
        }
        // A new recovery terminal is deliberately unbound: no lost agent
        // session is impersonated and no old session ID is invented.
        let uuid = Uuid::new_v4();
        let pane = SerializedTab {
            id: next_pane,
            title: "Terminal".into(),
            custom_label: None,
            color: None,
            hidden_from_sidebar: false,
            owner_pane_uuid: Some(uuid),
            root_layout: SerializedPaneLayout::Single {
                uuid,
                cwd: root.clone(),
                shell_title: None,
                is_active: false,
                clear_gap_height_px: 0.0,
                clear_gap_before_block_id: None,
                clear_gap_after_all: false,
                agent_id: None,
                browser_url: None,
            },
        };
        document.parked_workspace_tabs.push(SerializedWorkspaceTab {
            id: next_surface,
            name: "Terminal".into(),
            tabs: vec![pane],
            active_tab_id: next_pane,
            active_pane_uuid: Some(uuid),
        });
        document.workspaces.push(SerializedWorkspace {
            organisation_id: dragent_paths::organisation_id(),
            id: next_workspace,
            root: root.clone(),
            title,
            tab_ids: vec![next_surface],
            active_tab_id: next_surface,
        });
        document.workspace_tab_order.push(next_surface);
        if !document
            .open_project_roots
            .iter()
            .any(|path| same_path(path, &project))
        {
            document.open_project_roots.push(project);
        }
        log::warn!(
            "[workspace-recovery] recovered checkout missing from UI state: {}",
            root.display()
        );
        next_pane += 1;
        next_surface += 1;
        next_workspace += 1;
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

fn layout_has_root(layout: &SerializedPaneLayout, root: &Path) -> bool {
    match layout {
        SerializedPaneLayout::Single { cwd, .. } => same_path(cwd, root),
        SerializedPaneLayout::Split {
            left_or_top,
            right_or_bottom,
            ..
        } => layout_has_root(left_or_top, root) || layout_has_root(right_or_bottom, root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_session() -> SerializedSession {
        SerializedSession {
            tabs: Vec::new(),
            active_tab_id: 0,
            active_pane_uuid: None,
            chrome: Default::default(),
            layouts: Default::default(),
            document: SerializedDocumentSession {
                project_start_visible: true,
                ..Default::default()
            },
        }
    }

    #[test]
    fn recovered_worktrees_are_independent_and_do_not_activate_over_current_screen() {
        let mut session = empty_session();
        session.document.active_workspace_tab_id = Some(42);
        session.document.root = "/selected/project".into();
        recover(
            &mut session,
            vec![
                ("/managed/a".into(), "/project".into(), "branch-a".into()),
                ("/managed/b".into(), "/project".into(), "branch-b".into()),
            ],
        );
        assert!(session.tabs.is_empty());
        assert!(session.document.project_start_visible);
        assert_eq!(session.document.root, PathBuf::from("/selected/project"));
        assert_eq!(session.document.active_workspace_tab_id, Some(42));
        assert_eq!(
            session.document.open_project_roots,
            vec![PathBuf::from("/project")]
        );
        assert_eq!(session.document.workspaces.len(), 2);
        let a = &session.document.parked_workspace_tabs[0];
        let b = &session.document.parked_workspace_tabs[1];
        assert_ne!(a.id, b.id);
        assert_ne!(a.tabs[0].id, b.tabs[0].id);
        assert_ne!(a.active_pane_uuid, b.active_pane_uuid);
        assert!(session.document.host_terminal_bindings.is_empty());
        recover(
            &mut session,
            vec![("/managed/a".into(), "/project".into(), "branch-a".into())],
        );
        assert_eq!(session.document.workspaces.len(), 2);
    }

    #[test]
    fn legacy_terminal_metadata_prevents_duplicate_recovery() {
        let mut session = empty_session();
        let candidate = ("/managed/a".into(), "/project".into(), "branch-a".into());
        recover(&mut session, vec![candidate.clone()]);
        assert_ne!(session.document.parked_workspace_tabs[0].id, 1);
        session.document.workspaces.clear();
        recover(&mut session, vec![candidate]);
        assert_eq!(session.document.parked_workspace_tabs.len(), 1);
    }

    #[test]
    fn discovery_requires_git_worktree_identity_and_skips_detached_previews() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let managed = temp.path().join("managed");
        let checkout = managed.join("project/branch");
        let git_dir = project.join(".git/worktrees/branch");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(managed.join("project/arbitrary-directory")).unwrap();
        std::fs::write(
            checkout.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature/recovered\n").unwrap();
        let found = discover(&managed);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].2, "feature/recovered");
        std::fs::write(git_dir.join("HEAD"), "1234567890\n").unwrap();
        assert!(discover(&managed).is_empty());
    }

    #[test]
    fn recovered_catalogue_survives_database_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        let mut session = empty_session();
        recover(
            &mut session,
            vec![(checkout.clone(), project, "recovered".into())],
        );
        let path = temp.path().join("recovery.sqlite");
        {
            let mut connection = dragent_persistence::setup_connection(&path).unwrap();
            dragent_persistence::save_session_snapshot(&mut connection, &session).unwrap();
        }
        let mut connection = dragent_persistence::setup_connection(&path).unwrap();
        let restored = dragent_persistence::load_session(&mut connection)
            .unwrap()
            .unwrap()
            .session;
        assert!(restored.tabs.is_empty());
        assert!(restored.document.project_start_visible);
        assert_eq!(restored.document.workspaces[0].root, checkout);
        assert_eq!(restored.document.parked_workspace_tabs.len(), 1);
        assert_eq!(
            restored.document.parked_workspace_tabs[0].active_pane_uuid,
            session.document.parked_workspace_tabs[0].active_pane_uuid
        );
        assert!(restored.document.host_terminal_bindings.is_empty());
    }

    #[test]
    fn local_branch_checkout_recovers_without_changing_head_or_current_screen() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let head = "ref: refs/heads/feature/local-work\n";
        std::fs::write(project.join(".git/HEAD"), head).unwrap();
        std::fs::write(project.join("unsaved-work.txt"), "local changes").unwrap();
        let workspace = SerializedWorkspace {
            organisation_id: dragent_paths::organisation_id(),
            id: 7,
            root: project.clone(),
            title: "My local workspace".into(),
            tab_ids: vec![8],
            active_tab_id: 8,
        };
        let candidates = discover_local_checkouts(&[workspace.clone(), workspace.clone()]);
        assert_eq!(candidates.len(), 1);
        let mut session = empty_session();
        session.document.active_workspace_tab_id = Some(42);
        recover(&mut session, candidates.clone());
        assert_eq!(
            session.document.workspaces[0].root,
            project.canonicalize().unwrap()
        );
        assert_eq!(session.document.workspaces[0].title, "My local workspace");
        assert_eq!(session.document.active_workspace_tab_id, Some(42));
        assert!(session.document.project_start_visible);
        assert_eq!(
            std::fs::read_to_string(project.join(".git/HEAD")).unwrap(),
            head
        );
        assert_eq!(
            std::fs::read_to_string(project.join("unsaved-work.txt")).unwrap(),
            "local changes"
        );
        recover(&mut session, candidates);
        assert_eq!(session.document.workspaces.len(), 1);
        std::fs::write(project.join(".git/HEAD"), "deadbeef\n").unwrap();
        assert!(discover_local_checkouts(&[workspace]).is_empty());
    }

    #[test]
    fn open_project_and_local_branch_do_not_prove_workspace_existence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        let mut session = empty_session();
        session.document.open_project_roots.push(root);
        recover(&mut session, discover_local_checkouts(&[]));
        assert!(session.document.workspaces.is_empty());
        assert!(session.document.parked_workspace_tabs.is_empty());
    }

    #[test]
    fn local_workspace_record_recovers_after_ui_metadata_is_lost() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        let workspace = SerializedWorkspace {
            organisation_id: dragent_paths::organisation_id(),
            id: 7,
            root: root.clone(),
            title: "Local project work".into(),
            tab_ids: vec![8],
            active_tab_id: 8,
        };
        let path = temp.path().join("recovery.sqlite");
        {
            let mut conn = dragent_persistence::setup_connection(&path).unwrap();
            dragent_persistence::workspace_catalogue::record_local_workspace(&mut conn, &workspace)
                .unwrap();
            // A subsequent empty UI snapshot must not erase workspace evidence.
            dragent_persistence::save_session_snapshot(&mut conn, &empty_session()).unwrap();
        }
        let mut conn = dragent_persistence::setup_connection(&path).unwrap();
        let records =
            dragent_persistence::workspace_catalogue::load_local_workspaces(&mut conn).unwrap();
        let mut session = empty_session();
        recover_local_workspaces(&mut session, &records);
        assert_eq!(session.document.workspaces[0].id, workspace.id);
        assert_eq!(session.document.workspaces[0].title, workspace.title);
        assert_eq!(
            session.document.workspaces[0].root,
            root.canonicalize().unwrap()
        );
        assert!(session.document.project_start_visible);
        assert!(session.document.host_terminal_bindings.is_empty());
        dragent_persistence::workspace_catalogue::close_local_workspace(&mut conn, &root).unwrap();
        dragent_persistence::save_session_snapshot(&mut conn, &empty_session()).unwrap();
        drop(conn);
        let mut conn = dragent_persistence::setup_connection(&path).unwrap();
        assert!(
            dragent_persistence::workspace_catalogue::load_local_workspaces(&mut conn)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn explicitly_closed_local_workspace_stays_closed_after_database_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let mut session = empty_session();
        session.document.open_project_roots = vec![root.clone()];
        session.document.closed_workspace_roots = vec![root.clone()];
        let path = temp.path().join("closed.sqlite");
        {
            let mut connection = dragent_persistence::setup_connection(&path).unwrap();
            dragent_persistence::save_session_snapshot(&mut connection, &session).unwrap();
        }
        let mut connection = dragent_persistence::setup_connection(&path).unwrap();
        let mut restored = dragent_persistence::load_session(&mut connection)
            .unwrap()
            .unwrap()
            .session;
        recover(&mut restored, vec![(root.clone(), root, "master".into())]);
        assert!(restored.document.workspaces.is_empty());
        assert!(restored.document.parked_workspace_tabs.is_empty());
    }
}
