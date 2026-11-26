//! EnsureHome tests: plain project roots register as daemon session homes
//! without any disk mutation, so UI terminal tabs can move onto host-owned
//! PTYs that outlive the app process.
//!
//! Proves: idempotent registration, no marker/files written into the root,
//! git provenance attached read-only, the full ensure→terminal→read→close
//! chain, and — critically — teardown of a root home never touches the
//! user's directory (registration dropped, files intact).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
        let guard = lock().lock().unwrap_or_else(|p| p.into_inner());
        let base =
            std::env::temp_dir().join(format!("dragent-host-home-{name}-{}", std::process::id()));
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

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cond()
}

#[test]
fn ensure_plain_dir_idempotent_and_writes_nothing() {
    let iso = Isolated::new("plain");
    let client = iso.ensure();
    let root = iso.base.join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("sentinel.txt"), "keep\n").unwrap();
    let root_str = root.to_string_lossy().to_string();

    let ws = client.create_workspace(&root_str).expect("workspace");
    let first = client.ensure_home(&ws, &root_str).expect("ensure");
    assert!(first.id.starts_with("home-"), "id shape: {}", first.id);
    assert_eq!(first.workspace_id, ws);
    assert!(first.repo.is_none(), "plain dir carries no binding");

    let second = client.ensure_home(&ws, &root_str).expect("re-ensure");
    assert_eq!(first.id, second.id, "idempotent per workspace+path");

    // The root is untouched: no marker, no new entries besides our sentinel.
    let entries: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("sentinel.txt")]);
    assert_eq!(
        std::fs::read_to_string(root.join("sentinel.txt")).unwrap(),
        "keep\n"
    );
}

#[test]
fn ensure_git_checkout_attaches_binding_read_only() {
    let iso = Isolated::new("gitroot");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo");
    // Pre-existing worktree checkout also registers (linked, not main).
    let before = std::process::Command::new("git")
        .args(["-C", &repo, "worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    let listed_before = String::from_utf8_lossy(&before.stdout).into_owned();

    let ws = client.create_workspace(&repo).expect("workspace");
    let home = client.ensure_home(&ws, &repo).expect("ensure");
    let binding = home.repo.expect("checkout carries a binding");
    // Canonical on macOS (/var -> /private/var): compare resolved forms.
    let expected = std::path::Path::new(&repo)
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(binding.repo_root, expected);

    let after = std::process::Command::new("git")
        .args(["-C", &repo, "worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    assert_eq!(
        String::from_utf8_lossy(&after.stdout).into_owned(),
        listed_before,
        "ensure must not add/remove git worktrees"
    );
}

#[test]
fn ensure_then_terminal_chain_survives_ui_death() {
    let iso = Isolated::new("chain");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-chain");
    let ws = client.create_workspace(&repo).expect("workspace");
    let home = client.ensure_home(&ws, &repo).expect("ensure");

    let sess = client
        .create_terminal(&ws, &home.id, Some("echo CHAIN-MARKER-9"), None, 80, 24)
        .expect("create terminal");
    assert!(sess.id.starts_with("term-"));
    assert_eq!(sess.worktree_id, home.id);

    let ok = wait_for(Duration::from_secs(10), || {
        client
            .read_terminal(&ws, &sess.id, None)
            .map(|(text, _, _)| text.contains("CHAIN-MARKER-9"))
            .unwrap_or(false)
    });
    assert!(ok, "host PTY output must be readable");

    // Sessions stay listed while the (simulated-dead) UI is gone: listing
    // needs no UI-side state, so a relaunched app re-attaches by id.
    let sessions = client.list_terminals(&ws).expect("list");
    assert!(sessions.iter().any(|s| s.id == sess.id));

    let closed = client.close_terminal(&ws, &sess.id).expect("close");
    assert_eq!(closed.id, sess.id);
}

#[test]
fn teardown_root_home_deregisters_but_keeps_everything() {
    let iso = Isolated::new("teardown");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-tear");
    std::fs::write(PathBuf::from(&repo).join("precious.txt"), "data\n").unwrap();
    let ws = client.create_workspace(&repo).expect("workspace");
    let home = client.ensure_home(&ws, &repo).expect("ensure");
    let sess = client
        .create_terminal(&ws, &home.id, None, None, 80, 24)
        .expect("create terminal");

    let report = client
        .teardown_worktree(&ws, &home.id)
        .expect("teardown root home");
    assert!(
        report.warnings.is_empty(),
        "warnings: {:?}",
        report.warnings
    );

    // The user's directory is byte-identical: checkout, files, git intact.
    assert!(PathBuf::from(&repo).is_dir(), "root dir must survive");
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&repo).join("precious.txt")).unwrap(),
        "data\n"
    );
    let log = std::process::Command::new("git")
        .args(["-C", &repo, "log", "--oneline"])
        .output()
        .expect("git log");
    assert!(log.status.success(), "repo must stay valid");
    // Registration dropped; the session was disposed with it.
    let homes = client.list_worktrees(&ws).expect("list");
    assert!(!homes.iter().any(|h| h.id == home.id));
    let sessions = client.list_terminals(&ws).expect("list");
    assert!(!sessions.iter().any(|s| s.id == sess.id));
}

#[test]
fn distinct_roots_get_distinct_homes() {
    let iso = Isolated::new("distinct");
    let client = iso.ensure();
    let a = iso.base.join("a");
    let b = iso.base.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ws = client
        .create_workspace(&a.to_string_lossy())
        .expect("workspace");
    let ha = client
        .ensure_home(&ws, &a.to_string_lossy())
        .expect("ensure a");
    let hb = client
        .ensure_home(&ws, &b.to_string_lossy())
        .expect("ensure b");
    assert_ne!(ha.id, hb.id);
}

/// Core durability promise: a daemon session (and its processes) do not
/// depend on the client connection that created them. Dropping the creator
/// client simulates UI death; a fresh `connect_existing` client — what a
/// relaunched app gets — finds the session live and duplex-working.
#[test]
fn session_outlives_creator_client() {
    let iso = Isolated::new("survive");
    let repo = init_repo(&iso.base, "repo-survive");
    let (ws, home_id, sess_id) = {
        let client = iso.ensure();
        let ws = client.create_workspace(&repo).expect("workspace");
        let home = client.ensure_home(&ws, &repo).expect("ensure");
        // A plain shell: the round trip below is the shell running the
        // command, not the terminal echoing keystrokes back.
        let sess = client
            .create_terminal(&ws, &home.id, None, None, 80, 24)
            .expect("create terminal");
        // The daemon is a separate process, detached from the creator.
        assert_ne!(
            std::process::id(),
            read_daemon_pid(&client),
            "session must live in the daemon, not the client process"
        );
        (ws, home.id, sess.id)
    };
    // Creator client dropped: the UI is "dead".

    let client = HostClient::connect_existing().expect("reconnect without spawning");
    let sessions = client.list_terminals(&ws).expect("list");
    let live = sessions
        .iter()
        .find(|s| s.id == sess_id)
        .expect("session survives creator death");
    assert!(
        matches!(live.state, dragent_host::TerminalState::Running),
        "session still running: {:?}",
        live.state
    );
    // Full duplex through the fresh client.
    client
        .send_terminal_input(&ws, &sess_id, b"echo SURVIVE-DUPLEX-$((1 + 2))\n")
        .expect("send after reconnect");
    let ok = wait_for(Duration::from_secs(10), || {
        client
            .read_terminal(&ws, &sess_id, None)
            .map(|(text, _, _)| text.contains("SURVIVE-DUPLEX-3"))
            .unwrap_or(false)
    });
    assert!(ok, "round-trip through reconnected client");
    client.close_terminal(&ws, &sess_id).expect("close");
    let _ = home_id;
}

/// Daemon pid behind a client, via the endpoint file it was resolved from.
fn read_daemon_pid(_client: &HostClient) -> u32 {
    let raw = std::fs::read_to_string(
        std::env::var(dragent_host::ENDPOINT_FILE_ENV).expect("endpoint env"),
    )
    .expect("endpoint file");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("endpoint json");
    value
        .get("pid")
        .and_then(|pid| pid.as_u64())
        .expect("endpoint pid") as u32
}

#[test]
fn adopt_agent_checkout_registers_without_marker_and_tears_down() {
    let iso = Isolated::new("adopt");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-adopt");
    let checkout = iso.base.join("agent-checkout");
    let out = std::process::Command::new("git")
        .args(["-C", &repo, "worktree", "add", "--detach"])
        .arg(&checkout)
        .arg("HEAD")
        .output()
        .expect("git worktree add");
    assert!(
        out.status.success(),
        "worktree add: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ws = client.create_workspace(&repo).expect("workspace");
    let first = client
        .adopt_checkout(&ws, &repo, &checkout.to_string_lossy(), "agent")
        .expect("adopt");
    assert!(first.id.starts_with("agent-"), "id shape: {}", first.id);
    assert_eq!(
        first.kind,
        dragent_host::HomeKind::AgentCheckout,
        "adopted home is an agent checkout"
    );
    // No marker is written into the checkout (it would pollute the diff).
    assert!(
        !checkout.join(".dragent-home").exists(),
        "adopt must not write a marker"
    );

    let second = client
        .adopt_checkout(&ws, &repo, &checkout.to_string_lossy(), "agent")
        .expect("re-adopt");
    assert_eq!(first.id, second.id, "adopt is idempotent per path");

    // A plain directory is refused: only real linked worktrees adopt.
    let plain = iso.base.join("not-a-worktree");
    std::fs::create_dir_all(&plain).unwrap();
    assert!(
        client
            .adopt_checkout(&ws, &repo, &plain.to_string_lossy(), "agent")
            .is_err(),
        "non-worktree paths must be refused"
    );

    let report = client
        .teardown_worktree(&ws, &first.id)
        .expect("teardown adopted checkout");
    assert!(
        report.warnings.is_empty(),
        "warnings: {:?}",
        report.warnings
    );
    assert!(!checkout.exists(), "checkout directory removed");
    let homes = client.list_worktrees(&ws).expect("list");
    assert!(!homes.iter().any(|h| h.id == first.id));
    // The repository itself is untouched.
    let log = std::process::Command::new("git")
        .args(["-C", &repo, "log", "--oneline"])
        .output()
        .expect("git log");
    assert!(log.status.success(), "repo must stay valid");
}
