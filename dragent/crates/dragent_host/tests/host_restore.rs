//! UI restore protocol tests: one call re-attaches a workspace by id.

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
    dir: PathBuf,
    endpoint: PathBuf,
    db: PathBuf,
}

impl Isolated {
    fn new(name: &str) -> Self {
        let guard = lock().lock().unwrap();
        let tag = format!("dragent-host-restore-{name}-{}", std::process::id());
        let dir = std::env::temp_dir().join(&tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("isolated dir");
        let endpoint = dir.join(format!("{tag}.json"));
        let db = dir.join(format!("{tag}.sqlite"));
        // SAFETY: tests serialize on a global lock, so no concurrent env access.
        unsafe {
            std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &endpoint);
            std::env::set_var("DRAGENT_WORKSPACE_DB", &db);
        }
        Self {
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

fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git spawn failed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(name: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "dragent-restore-repo-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&repo).unwrap();
    if !std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-b", "main", "-q"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        git(&repo, &["init", "-q"]);
        git(&repo, &["checkout", "-qb", "main"]);
    }
    std::fs::write(repo.join("README.md"), "# test\n").unwrap();
    git(&repo, &["add", "."]);
    std::process::Command::new("git")
        .current_dir(&repo)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ])
        .output()
        .expect("commit failed");
    repo
}

fn wait_for<T>(label: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(start.elapsed() < timeout, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn restore_plan_reattaches_by_id() {
    let iso = Isolated::new("plan");
    let client = iso.ensure();
    let repo = init_repo("plan");
    let repo_s = repo.to_string_lossy().into_owned();
    let ws = client.create_workspace(&repo_s).expect("workspace");
    let home = client
        .create_worktree(&ws, "terminal", &repo_s, "restore-me")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, None, None, 80, 24)
        .expect("session");
    client
        .send_terminal_input(&ws, &sess.id, b"echo restore-marker-3\n")
        .expect("send");
    wait_for("marker", Duration::from_secs(10), || {
        let (text, _, _) = client.read_terminal(&ws, &sess.id, None).expect("read");
        text.contains("restore-marker-3").then_some(())
    });

    let plan = client
        .restore_plan(&ws, vec![sess.id.clone(), "term-bogus".to_string()], None)
        .expect("plan");
    assert_eq!(plan.workspace_id, ws);
    assert_eq!(plan.homes.len(), 1);
    assert_eq!(plan.homes[0].id, home.id);
    assert_eq!(plan.sessions.len(), 1);
    assert_eq!(plan.sessions[0].session.id, sess.id);
    assert!(plan.sessions[0].tail.contains("restore-marker-3"));
    assert!(plan.sessions[0].alive);
    assert_eq!(plan.gone_session_ids, vec!["term-bogus".to_string()]);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn restore_plan_reports_closed_and_torn_down() {
    let iso = Isolated::new("gone");
    let client = iso.ensure();
    let repo = init_repo("gone");
    let repo_s = repo.to_string_lossy().into_owned();
    let ws = client.create_workspace(&repo_s).expect("workspace");
    let home = client
        .create_worktree(&ws, "agent", &repo_s, "restore-gone")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, None, None, 80, 24)
        .expect("session");
    client.close_terminal(&ws, &sess.id).expect("close");

    let plan = client
        .restore_plan(&ws, vec![sess.id.clone()], None)
        .expect("plan");
    assert!(plan.sessions.is_empty());
    assert_eq!(plan.gone_session_ids, vec![sess.id.clone()]);
    assert_eq!(plan.homes.len(), 1);

    client.teardown_worktree(&ws, &home.id).expect("teardown");
    let plan = client
        .restore_plan(&ws, vec![sess.id.clone()], None)
        .expect("plan");
    assert!(plan.homes.is_empty());
    assert_eq!(plan.gone_session_ids, vec![sess.id]);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn restore_plan_gates_by_workspace_and_caps_tails() {
    let iso = Isolated::new("gates");
    let client = iso.ensure();
    let repo = init_repo("gates");
    let repo_s = repo.to_string_lossy().into_owned();
    let ws = client.create_workspace(&repo_s).expect("workspace");
    let home = client
        .create_worktree(&ws, "terminal", &repo_s, "restore-gates")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, None, None, 80, 24)
        .expect("session");
    for i in 0..10 {
        client
            .send_terminal_input(&ws, &sess.id, format!("echo gate-line-{i}\n").as_bytes())
            .expect("send");
    }
    wait_for("lines", Duration::from_secs(10), || {
        let (text, _, _) = client.read_terminal(&ws, &sess.id, None).expect("read");
        text.contains("gate-line-9").then_some(())
    });

    let plan = client.restore_plan(&ws, vec![], Some(2)).expect("plan");
    assert_eq!(plan.sessions.len(), 1);
    assert!(plan.sessions[0].tail.lines().count() <= 2);

    let elsewhere =
        std::env::temp_dir().join(format!("dragent-restore-else-{}", std::process::id()));
    let other = client
        .create_workspace(&elsewhere.to_string_lossy())
        .expect("ws2");
    let plan = client.restore_plan(&other, vec![], None).expect("plan");
    assert!(plan.homes.is_empty() && plan.sessions.is_empty() && plan.gone_session_ids.is_empty());
    let _ = std::fs::remove_dir_all(&elsewhere);
    let _ = std::fs::remove_dir_all(&repo);
}
