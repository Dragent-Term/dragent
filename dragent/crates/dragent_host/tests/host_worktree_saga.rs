//! Phase-6.1 worktree saga tests: real host processes + isolated org dirs.
//!
//! Proves worktree-backed workspaces end to end: create/list/get with an
//! ownership gate, branch dedup, ordered teardown (dir gone, branch kept,
//! unregistered), non-repo rejection, standalone session lifecycle,
//! session/port ownership, and restart reconcile of out-of-band damage.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use dragent_host::HostClient;

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

struct Isolated {
    _guard: std::sync::MutexGuard<'static, ()>,
    /// Stops the services this test started, pass or fail.
    services: dragent_host::testing::ServiceGuard,
    base: PathBuf,
    endpoint: PathBuf,
    db: PathBuf,
}

impl Isolated {
    fn new(name: &str) -> Self {
        // Tolerate a poisoned lock so one failing test does not cascade-mask
        // the rest (mirrors the server's poison-tolerant locking).
        let guard = lock().lock().unwrap_or_else(|p| p.into_inner());
        // Each test gets its own subdir: the worktree ledger is a singleton
        // at <endpoint-parent>/worktrees.json, so bare temp_dir paths would
        // collide across tests.
        let base =
            std::env::temp_dir().join(format!("dragent-host-wt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("isolated subdir");
        let endpoint = base.join("host.json");
        let db = base.join("db.sqlite");
        for sidecar in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{sidecar}", db.display()));
        }
        // SAFETY: tests serialize on a global lock, so no concurrent env access.
        unsafe {
            std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &endpoint);
            std::env::set_var("DRAGENT_WORKSPACE_DB", &db);
        }
        Self {
            services: dragent_host::testing::ServiceGuard::new(),
            _guard: guard,
            base,
            endpoint,
            db,
        }
    }

    fn ensure(&self) -> HostClient {
        HostClient::ensure_host_with(&host_bin()).expect("host should come up")
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        self.services.stop();
        if let Ok(client) = HostClient::connect_existing() {
            let _ = client.shutdown();
        }
        std::thread::sleep(Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.base);
        let _ = std::fs::remove_file(&self.endpoint);
        let _ = std::fs::remove_file(&self.db);
        for sidecar in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{sidecar}", self.db.display()));
        }
        unsafe {
            std::env::remove_var(dragent_host::ENDPOINT_FILE_ENV);
            std::env::remove_var("DRAGENT_WORKSPACE_DB");
        }
    }
}

/// Fresh git repo with one commit under the isolated base dir.
fn init_repo(base: &PathBuf, name: &str) -> String {
    let dir = base.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("repo dir");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    git(&["init"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "init"]);
    dir.to_string_lossy().to_string()
}

fn branches_in(repo: &str) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["branch", "--list"])
        .output()
        .expect("git branch");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn worktree_paths_in(repo: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    text.lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn crud_list_get_ownership_gate_unknown() {
    let iso = Isolated::new("crud");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-crud");
    assert!(client.list_worktrees("ws-crud").expect("list").is_empty());
    let home = client
        .create_worktree("ws-crud", "flow-a", &repo, "feature-x")
        .expect("create");
    assert!(home.id.starts_with("wt-"));
    assert_eq!(home.workspace_id, "ws-crud");
    assert!(home.repo.is_some());
    assert!(PathBuf::from(&home.path).exists());
    let listed = client.list_worktrees("ws-crud").expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, home.id);
    let got = client.worktree("ws-crud", &home.id).expect("get");
    assert_eq!(got.id, home.id);
    // Ownership gate: foreign workspace sees nothing, unknown ids error.
    assert!(client.list_worktrees("ws-other").expect("list").is_empty());
    assert!(client.worktree("ws-other", &home.id).is_err());
    assert!(client.worktree("ws-crud", "wt-does-not-exist").is_err());
}

#[test]
fn duplicate_branch_dedups_path_and_branch() {
    let iso = Isolated::new("dedup");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-dedup");
    let first = client
        .create_worktree("ws-dedup", "flow", &repo, "shared")
        .expect("first");
    let second = client
        .create_worktree("ws-dedup", "flow", &repo, "shared")
        .expect("second");
    assert_ne!(first.id, second.id);
    assert_ne!(first.path, second.path);
    let b1 = first.repo.as_ref().expect("repo").branch.clone();
    let b2 = second.repo.as_ref().expect("repo").branch.clone();
    assert_ne!(b1, b2);
    assert!(PathBuf::from(&second.path).exists());
    let listed = client.list_worktrees("ws-dedup").expect("list");
    assert_eq!(listed.len(), 2);
}

#[test]
fn teardown_saga_dir_gone_branch_kept_unregistered() {
    let iso = Isolated::new("saga");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-saga");
    let home = client
        .create_worktree("ws-saga", "flow", &repo, "saga-branch")
        .expect("create");
    let branch = home.repo.as_ref().expect("repo").branch.clone();
    assert!(worktree_paths_in(&repo).iter().any(|p| *p == home.path));
    let report = client
        .teardown_worktree("ws-saga", &home.id)
        .expect("teardown");
    assert_eq!(report.worktree_id, home.id);
    assert_eq!(report.branch_kept.as_deref(), Some(branch.as_str()));
    assert!(
        report.warnings.is_empty(),
        "warnings: {:?}",
        report.warnings
    );
    // Saga order: dir gone + branch kept + unregistered.
    assert!(
        !PathBuf::from(&home.path).exists(),
        "checkout dir must be gone"
    );
    assert!(
        branches_in(&repo)
            .lines()
            .any(|l| l.trim().trim_start_matches("* ").trim() == branch),
        "branch {branch} must survive teardown"
    );
    assert!(
        !worktree_paths_in(&repo).iter().any(|p| *p == home.path),
        "worktree must be unregistered"
    );
    assert!(client.list_worktrees("ws-saga").expect("list").is_empty());
    assert!(client.worktree("ws-saga", &home.id).is_err());
}

#[test]
fn create_in_non_repo_rejected() {
    let iso = Isolated::new("nonrepo");
    let client = iso.ensure();
    let plain = iso.base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let err = client
        .create_worktree("ws-nonrepo", "flow", &plain.to_string_lossy(), "branch")
        .expect_err("non-repo must be rejected");
    assert!(err.contains("not a git repository"), "unexpected: {err}");
    assert!(
        client
            .create_worktree("", "flow", &plain.to_string_lossy(), "b")
            .is_err()
    );
}

#[test]
fn standalone_session_lifecycle() {
    let iso = Isolated::new("standalone");
    let client = iso.ensure();
    let sess = client
        .create_standalone_session("ws-alone", "notebook")
        .expect("create");
    assert!(sess.id.starts_with("sess-"));
    assert!(sess.repo.is_none());
    assert!(PathBuf::from(&sess.path).exists());
    let listed = client.list_worktrees("ws-alone").expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, sess.id);
    let torn = client
        .teardown_standalone_session("ws-alone", &sess.id)
        .expect("teardown");
    assert_eq!(torn, sess.id);
    assert!(
        !PathBuf::from(&sess.path).exists(),
        "session dir must be gone"
    );
    assert!(client.list_worktrees("ws-alone").expect("list").is_empty());
}

#[test]
fn sessions_and_ports_ownership_and_teardown_counts() {
    let iso = Isolated::new("sessports");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-sessports");
    let wt1 = client
        .create_worktree("ws-sp", "flow", &repo, "wt-one")
        .expect("wt1");
    let wt2 = client
        .create_worktree("ws-sp", "flow", &repo, "wt-two")
        .expect("wt2");
    let sess = client
        .create_standalone_session("ws-sp", "term")
        .expect("sess");

    // Attach with move semantics: second attach moves the session.
    let updated = client
        .attach_session("ws-sp", &wt1.id, &sess.id)
        .expect("attach");
    assert!(updated.sessions.contains(&sess.id));
    let moved = client
        .attach_session("ws-sp", &wt2.id, &sess.id)
        .expect("move");
    assert!(moved.sessions.contains(&sess.id));
    assert!(
        !client
            .worktree("ws-sp", &wt1.id)
            .expect("get")
            .sessions
            .contains(&sess.id)
    );
    // Detach removes; foreign workspace is gated.
    let detached = client
        .detach_session("ws-sp", &wt2.id, &sess.id)
        .expect("detach");
    assert!(!detached.sessions.contains(&sess.id));
    assert!(
        client
            .attach_session("ws-foreign", &wt1.id, &sess.id)
            .is_err()
    );
    assert!(
        client
            .attach_session("ws-sp", "wt-missing", &sess.id)
            .is_err()
    );

    // Ports: allocate then release; foreign workspace gated.
    client
        .attach_session("ws-sp", &wt1.id, &sess.id)
        .expect("reattach");
    let port = client.allocate_port("ws-sp", &wt1.id).expect("alloc");
    assert!(port > 0);
    assert!(
        client
            .worktree("ws-sp", &wt1.id)
            .expect("get")
            .ports
            .contains(&port)
    );
    assert!(client.allocate_port("ws-foreign", &wt1.id).is_err());
    client
        .release_port("ws-sp", &wt1.id, port)
        .expect("release");
    assert!(
        !client
            .worktree("ws-sp", &wt1.id)
            .expect("get")
            .ports
            .contains(&port)
    );

    // Teardown counts attached sessions and reserved ports.
    let port2 = client.allocate_port("ws-sp", &wt1.id).expect("alloc2");
    let report = client
        .teardown_worktree("ws-sp", &wt1.id)
        .expect("teardown");
    assert_eq!(report.sessions_detached, 1);
    assert_eq!(report.ports_released, 1);
    assert_eq!(report.warnings.len(), 0, "warnings: {:?}", report.warnings);
    let _ = port2;
}

#[test]
fn restart_reconciles_out_of_band_checkout_loss() {
    let iso = Isolated::new("reconcile");
    let repo = init_repo(&iso.base, "repo-reconcile");
    let home = iso
        .ensure()
        .create_worktree("ws-rec", "flow", &repo, "doomed")
        .expect("create");
    assert!(PathBuf::from(&home.path).exists());
    // Crash the host, then destroy the checkout out of band (ledger still
    // references it and git still has a stale registration).
    HostClient::connect_existing()
        .expect("connect")
        .shutdown()
        .expect("shutdown");
    std::thread::sleep(Duration::from_millis(400));
    std::fs::remove_dir_all(&home.path).expect("out-of-band delete");
    assert!(worktree_paths_in(&repo).iter().any(|p| *p == home.path));
    // Restart on the same org dir: reconcile prunes the stranded home.
    let revived = iso.ensure();
    assert!(revived.list_worktrees("ws-rec").expect("list").is_empty());
    assert!(
        !worktree_paths_in(&repo).iter().any(|p| *p == home.path),
        "stale registration must be pruned"
    );
}
