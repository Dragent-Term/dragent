//! Watch capability tests: git-poll registry + RPCs + event fan-out.
//!
//! Proves unknown/non-git roots are refused, list/unwatch round-trip,
//! and a real file change surfaces as a WorkspaceFilesChanged event
//! through a subscription.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use dragent_host::{Event, HostClient};

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct Isolated {
    _guard: std::sync::MutexGuard<'static, ()>,
    /// Stops the services this test started, pass or fail.
    services: dragent_host::testing::ServiceGuard,
    dir: PathBuf,
    endpoint: PathBuf,
    db: PathBuf,
}

impl Isolated {
    fn new(name: &str) -> Self {
        let guard = lock().lock().unwrap();
        // Own subdir: the daemon manifest is a singleton path, so bare
        // temp files collide across tests and binaries.
        let dir = std::env::temp_dir().join(format!(
            "dragent-host-watch-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("isolated dir");
        let endpoint = dir.join("host.json");
        let db = dir.join("db.sqlite");
        // SAFETY: tests serialize on a global lock, so no concurrent env access.
        unsafe {
            std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &endpoint);
            std::env::set_var("DRAGENT_WORKSPACE_DB", &db);
        }
        Self {
            services: dragent_host::testing::ServiceGuard::new(),
            _guard: guard,
            dir,
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
        std::thread::sleep(Duration::from_millis(150));
        let _ = std::fs::remove_dir_all(&self.dir);
        unsafe {
            std::env::remove_var(dragent_host::ENDPOINT_FILE_ENV);
            std::env::remove_var("DRAGENT_WORKSPACE_DB");
        }
    }
}

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

#[test]
fn watch_refuses_unknown_and_non_git_roots() {
    let iso = Isolated::new("refuse");
    let client = iso.ensure();
    // Unknown workspace: no canonical root to resolve.
    assert!(client.watch_workspace("ws-nope").is_err());
    // Known workspace on a non-git dir: refused, never watched.
    let plain = iso.dir.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let ws = client
        .create_workspace(&plain.to_string_lossy())
        .expect("workspace");
    assert!(client.watch_workspace(&ws).is_err());
    assert!(client.list_watches().expect("list").is_empty());
}

#[test]
fn watch_list_unwatch_round_trip() {
    let iso = Isolated::new("roundtrip");
    let client = iso.ensure();
    let repo = init_repo(&iso.dir, "repo");
    let ws = client.create_workspace(&repo).expect("workspace");
    client.watch_workspace(&ws).expect("watch");
    // Idempotent re-watch replaces (no duplicate).
    client.watch_workspace(&ws).expect("re-watch");
    assert_eq!(client.list_watches().expect("list"), vec![ws.clone()]);
    client.unwatch_workspace(&ws).expect("unwatch");
    assert!(client.list_watches().expect("list").is_empty());
    // Unwatching nothing is still Ack.
    client.unwatch_workspace(&ws).expect("second unwatch");
}

#[test]
fn change_surfaces_as_event() {
    let iso = Isolated::new("event");
    let client = iso.ensure();
    let repo = init_repo(&iso.dir, "repo");
    let ws = client.create_workspace(&repo).expect("workspace");
    let (tx, rx) = mpsc::channel();
    let _handle = client
        .subscribe(move |event| {
            if matches!(event, Event::WorkspaceFilesChanged { .. }) {
                let _ = tx.send(event);
            }
        })
        .expect("subscribe");
    client.watch_workspace(&ws).expect("watch");
    // Settle: the first poll after watch baselines (no event expected).
    std::thread::sleep(Duration::from_secs(7));
    let _ = rx.try_iter().count();
    // Real change: modify a tracked file.
    std::fs::write(PathBuf::from(&repo).join("README.md"), "hello changed\n").unwrap();
    let start = Instant::now();
    let event = loop {
        if start.elapsed() > Duration::from_secs(25) {
            panic!("no WorkspaceFilesChanged within 25s");
        }
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(event) => break event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("subscription dropped")
            }
        }
    };
    match event {
        Event::WorkspaceFilesChanged {
            workspace_id,
            paths,
        } => {
            assert_eq!(workspace_id, ws);
            assert!(
                paths.iter().any(|p| p.ends_with("README.md")),
                "paths: {paths:?}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}
