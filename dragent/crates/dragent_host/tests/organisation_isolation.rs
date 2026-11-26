//! Separate host authorities and durable UI/catalogues, with one shared checkout.

use dragent_host::HostClient;
use dragent_persistence::{
    self as storage, SerializedDocumentSession, SerializedSession, SerializedWorkspace,
};
use dragent_workspace_model::Project;
use std::path::PathBuf;

#[cfg(unix)]
const TERMINAL_SHELL: Option<&str> = Some("/bin/bash");
#[cfg(not(unix))]
const TERMINAL_SHELL: Option<&str> = None;

struct Harness {
    home: PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    hosts: Vec<HostClient>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for host in &self.hosts {
            if let Ok(workspaces) = host.list_workspaces() {
                for workspace in workspaces {
                    if let Ok(terminals) = host.list_terminals(&workspace) {
                        for terminal in terminals {
                            let _ = host.close_terminal(&workspace, &terminal.id);
                        }
                    }
                }
            }
            let _ = host.shutdown();
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        for org in ["local", "Acme!", "Acme?"] {
            let dir = if org == "local" {
                self.home.clone()
            } else {
                self.home
                    .join("organisations")
                    .join(dragent_paths::organisation_key(org))
            };
            // Wait for (or kill) whatever is still running there.
            dragent_host::testing::stop_services(
                &dir.join(dragent_host::protocol::ENDPOINT_FILENAME),
                &dir,
            );
            if let Some(manifest) = dragent_host::pty_supervisor::read_manifest(&dir) {
                let conn = dragent_host::pty_supervisor::DaemonConn::from(&manifest);
                let _ = conn.rpc(dragent_host::DaemonRequest::Shutdown);
            }
        }
        for (name, value) in &self.previous {
            unsafe {
                if let Some(value) = value {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn organisations_restore_only_their_own_workspaces_and_keep_other_hosts_alive() {
    let home = std::env::temp_dir().join(format!("dragent-org-test-{}", uuid::Uuid::new_v4()));
    let names = [
        "DRAGENT_HOME",
        "DRAGENT_HOST_ENDPOINT_FILE",
        "DRAGENT_WORKSPACE_DB",
    ];
    let mut h = Harness {
        home: home.clone(),
        previous: names
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect(),
        hosts: Vec::new(),
    };
    // This executable contains one test; no user host/database is touched.
    unsafe {
        std::env::set_var("DRAGENT_HOME", &home);
        std::env::remove_var("DRAGENT_HOST_ENDPOINT_FILE");
        std::env::remove_var("DRAGENT_WORKSPACE_DB");
    }
    dragent_host::testing::mark_test_services();
    let root = home.join("shared-checkout");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let mut identities = Vec::new();
    let mut paths = Vec::new();
    let mut sessions = Vec::new();
    for (index, org) in ["local", "Acme!", "Acme?"].into_iter().enumerate() {
        dragent_paths::select_organisation(org);
        let path = storage::database_path().unwrap();
        assert!(!paths.contains(&path));
        paths.push(path.clone());
        let mut conn = storage::setup_connection(&path).unwrap();
        assert!(storage::load_session(&mut conn).unwrap().is_none());
        assert!(
            storage::workspace_catalogue::load_local_workspaces(&mut conn)
                .unwrap()
                .is_empty()
        );
        let project = Project::new_for_organisation(org, root.clone(), 1);
        assert!(!identities.contains(&project.id));
        identities.push(project.id);
        storage::save_workspace_project(&mut conn, &project).unwrap();
        let workspace = SerializedWorkspace {
            organisation_id: org.into(),
            id: index + 1,
            root: root.clone(),
            title: format!("{org} workspace"),
            tab_ids: vec![index + 1],
            active_tab_id: index + 1,
        };
        storage::workspace_catalogue::record_local_workspace(&mut conn, &workspace).unwrap();
        let document = SerializedDocumentSession {
            organisation_id: org.into(),
            root: root.clone(),
            open_project_roots: vec![root.clone()],
            workspaces: vec![workspace],
            ..Default::default()
        };
        storage::save_session_snapshot(
            &mut conn,
            &SerializedSession {
                tabs: Vec::new(),
                active_tab_id: 0,
                active_pane_uuid: None,
                chrome: Default::default(),
                document,
                layouts: Default::default(),
            },
        )
        .unwrap();
        let host =
            HostClient::ensure_host_with(std::path::Path::new(env!("CARGO_BIN_EXE_dragent-host")))
                .unwrap();
        assert!(host.list_workspaces().unwrap().is_empty());
        let workspace_id = host.create_local_workspace(root.to_str().unwrap()).unwrap();
        let terminal_home = host
            .ensure_terminal_home(
                &workspace_id,
                root.to_str().unwrap(),
                root.to_str().unwrap(),
            )
            .unwrap();
        let terminal = host
            .create_terminal_with_shell(
                &workspace_id,
                &terminal_home.id,
                None,
                Some(root.to_str().unwrap()),
                80,
                24,
                TERMINAL_SHELL,
            )
            .unwrap();
        sessions.push((workspace_id, terminal.id));
        h.hosts.push(host);
    }
    // Selection changes only which saved context is read. All hosts stay alive.
    let mut local_snapshot = None;
    let mut local_writer = None;
    for (index, org) in ["local", "Acme!", "Acme?", "local"].into_iter().enumerate() {
        let slot = index % 3;
        dragent_paths::select_organisation(org);
        let mut conn = storage::setup_connection(&storage::database_path().unwrap()).unwrap();
        let restored = storage::load_session(&mut conn).unwrap().unwrap();
        if index == 0 {
            local_snapshot = Some(restored.session.clone());
            local_writer = Some(storage::shared_writer());
        } else if index == 1 {
            // Old panes can finish saving after the active context changes.
            let mut snapshot = local_snapshot.clone().unwrap();
            snapshot.document.sidebar_ui = "saved-in-background".into();
            local_writer.as_ref().unwrap().snapshot(snapshot);
            local_writer.as_ref().unwrap().flush();
            let mut old_connections = storage::WriterConns::new(paths[0].clone()).unwrap();
            let local_project = Project::new(root.clone(), 1);
            storage::save_workspace_project(
                old_connections.conn_for_root(Some(&root)),
                &local_project,
            )
            .unwrap();
            assert!(storage::db_file_for_root(&root, "local").is_file());
        } else if index == 3 {
            assert_eq!(restored.session.document.sidebar_ui, "saved-in-background");
        }
        assert_eq!(restored.session.document.organisation_id, org);
        assert_eq!(restored.session.document.workspaces.len(), 1);
        assert_eq!(
            restored.session.document.workspaces[0].title,
            format!("{org} workspace")
        );
        assert_eq!(
            storage::loaded_session()
                .unwrap()
                .session
                .document
                .organisation_id,
            org
        );
        assert_eq!(storage::loaded_local_workspaces()[0].organisation_id, org);
        assert_eq!(
            storage::workspace_catalogue::load_local_workspaces(&mut conn)
                .unwrap()
                .len(),
            1
        );
        let connected = HostClient::connect_existing().unwrap();
        assert_eq!(
            connected.list_workspaces().unwrap(),
            vec![sessions[slot].0.clone()]
        );
        for (host, (workspace, terminal)) in h.hosts.iter().zip(&sessions) {
            host.send_terminal_input(workspace, terminal, b"echo still-alive\n")
                .unwrap();
            assert!(host
                .list_terminals(workspace)
                .unwrap()
                .iter()
                .any(|t| t.id == *terminal
                    && matches!(t.state, dragent_host::TerminalState::Running)));
        }
        // An explicitly wrong-org snapshot is rejected before deleting tabs.
        let mut wrong = restored.session.clone();
        wrong.document.organisation_id = "another-org".into();
        assert!(storage::save_session_snapshot(&mut conn, &wrong).is_err());
        assert_eq!(
            storage::load_session(&mut conn)
                .unwrap()
                .unwrap()
                .session
                .document
                .organisation_id,
            org
        );
    }
    assert_eq!(paths[0], home.join("dragent.sqlite"));
}
