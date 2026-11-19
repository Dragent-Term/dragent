//! PTY split tests: TerminalRegistry lives in a supervised
//! `dragent-host --dragent-pty-service` child, not in the host process.
//!
//! Each test owns an isolated subdir (the manifest is a per-org singleton),
//! with WAL sidecar cleanup like the terminal tests. Proves: a SIGKILLed
//! host's sessions survive via manifest adoption, graceful restart kills
//! sessions openly with a new daemon pid, a crashed daemon respawns
//! transparently on the next op, the crash circuit opens and `restart`
//! clears it, and the daemon handshake rejects unknown versions.

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
    base: PathBuf,
    endpoint: PathBuf,
    db: PathBuf,
}

impl Isolated {
    fn new(name: &str) -> Self {
        let guard = lock().lock().unwrap_or_else(|p| p.into_inner());
        let base = std::env::temp_dir().join(format!(
            "dragent-host-ptysplit-{name}-{}",
            std::process::id()
        ));
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
        if let Ok(client) = HostClient::connect_existing() {
            let _ = client.shutdown();
        }
        std::thread::sleep(Duration::from_millis(300));
        // Best-effort: a crashed host skips stop_owned, so reap a lingerer.
        if let Ok(raw) = std::fs::read_to_string(self.base.join("pty-daemon.json")) {
            if let Ok(manifest) = serde_json::from_str::<dragent_host::PtyDaemonManifest>(&raw) {
                kill_process(manifest.pid);
            }
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

/// Portable SIGKILL: `kill -9` on unix, `taskkill /F` on Windows.
fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
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

/// Poll `cond` until true or the deadline passes; final verdict included.
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

#[cfg(unix)]
#[test]
fn agent_launch_uses_external_worktree_home() {
    let iso = Isolated::new("external-worktree-launch");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "main checkout");
    let worktree = iso.base.join("agent worktree");
    let output = std::process::Command::new("git")
        .args(["-C", &repo, "worktree", "add", "-b", "agent"])
        .arg(&worktree)
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let ws = client.create_local_workspace(&repo).unwrap();
    let root_home = client.ensure_terminal_home(&ws, &repo, &repo).unwrap();
    let cwd = worktree.to_string_lossy();
    // Reproduce the old launcher mismatch: the host must still reject it.
    assert!(client.create_terminal_with_shell(&ws, &root_home.id, None,
        Some(&cwd), 80, 24, Some("/bin/bash")).unwrap_err()
        .contains("cwd must stay inside home checkout"));
    let home = client.ensure_terminal_home(&ws, &repo, &cwd).unwrap();
    assert_ne!(root_home.id, home.id);
    assert_eq!(std::path::Path::new(&home.path), worktree.canonicalize().unwrap());
    let terminal = client.create_terminal_with_shell(&ws, &home.id,
        Some("pwd -P > launch-cwd.txt"), Some(&cwd), 80, 24, Some("/bin/bash")).unwrap();
    assert!(wait_for(Duration::from_secs(10), || {
        std::fs::read_to_string(worktree.join("launch-cwd.txt"))
            .is_ok_and(|value| value.trim() == worktree.canonicalize().unwrap().to_string_lossy())
    }), "launch command must execute inside the selected worktree");
    assert_eq!(client.ensure_terminal_home(&ws, &repo, &cwd).unwrap().id, home.id);
    client.close_terminal(&ws, &terminal.id).unwrap();
}

fn tail(client: &HostClient, ws: &str, id: &str) -> String {
    client
        .read_terminal(ws, id, None)
        .map(|(text, _, _)| text)
        .unwrap_or_default()
}

fn daemon_pid(client: &HostClient) -> Option<u32> {
    client.pty_status().ok().and_then(|(pid, ..)| pid)
}

fn daemon_manifest(iso: &Isolated) -> dragent_host::PtyDaemonManifest {
    let raw = std::fs::read_to_string(iso.base.join(dragent_host::PTY_MANIFEST_FILENAME))
        .expect("pty manifest file");
    serde_json::from_str(&raw).expect("manifest json")
}

fn prime_terminal(
    client: &HostClient,
    base: &PathBuf,
    ws: &str,
    branch: &str,
    marker: &str,
) -> (dragent_host::WorktreeHome, dragent_host::TerminalSession) {
    let repo = init_repo(base, &format!("repo-{branch}"));
    let home = client
        .create_worktree(ws, "flow", &repo, branch)
        .expect("create home");
    let sess = client
        .create_terminal(ws, &home.id, Some(&format!("echo {marker}")), None, 80, 24)
        .expect("create terminal");
    assert!(
        wait_for(Duration::from_secs(20), || tail(client, ws, &sess.id)
            .contains(marker)),
        "marker {marker} missing: {:?}",
        tail(client, ws, &sess.id)
    );
    (home, sess)
}

#[test]
fn host_kill_leaves_sessions_alive() {
    let iso = Isolated::new("adopt");
    let client = iso.ensure();
    let (home, sess) = prime_terminal(
        &client,
        &iso.base,
        "ws-adopt",
        "adopt-branch",
        "ADOPT-MARKER-1",
    );
    let daemon = daemon_pid(&client).expect("daemon pid");
    let host_pid = client.host_info().expect("info").pid;

    // Crash the host: SIGKILL skips stop_owned, so the daemon must survive.
    kill_process(host_pid);
    assert!(
        wait_for(Duration::from_secs(10), || HostClient::connect_existing()
            .is_err()),
        "killed host must stop answering"
    );

    // A fresh host adopts the SAME daemon via the manifest.
    let again = HostClient::ensure_host_with(&host_bin()).expect("re-ensure");
    assert_eq!(
        daemon_pid(&again),
        Some(daemon),
        "re-ensured host must adopt the surviving daemon"
    );
    // Existing panes retain their original client, including its input writer.
    // It must discover the replacement rather than retry the old token forever.
    assert_eq!(
        client.host_info().expect("original client reconnects").pid,
        again.host_info().unwrap().pid,
    );
    // Changing active discovery must not redirect an existing pane's authority.
    unsafe { std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, iso.base.join("other-host.json")); }
    let stayed_with_original_authority = client.ping();
    unsafe { std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &iso.endpoint); }
    assert!(stayed_with_original_authority);
    assert!(tail(&client, "ws-adopt", &sess.id).contains("ADOPT-MARKER-1"));
    client
        .send_terminal_input("ws-adopt", &sess.id, b"echo ADOPT-MARKER-2\n")
        .expect("send through original client after adopt");
    assert!(wait_for(Duration::from_secs(20), ||
        tail(&client, "ws-adopt", &sess.id).contains("ADOPT-MARKER-2")));
    let mut checkpoint_received = false;
    client.stream_terminal_operations("ws-adopt", &sess.id, 0, |chunk| {
        checkpoint_received = chunk.resume.is_some();
        false
    }).expect("original pane output reconnects");
    assert!(checkpoint_received);

    // New terminals opened through the retained authority must work too.
    let fresh = client.create_terminal("ws-adopt", &home.id,
        Some("echo NEW-AFTER-RESTART"), None, 80, 24).expect("new terminal");
    assert!(wait_for(Duration::from_secs(20), ||
        tail(&client, "ws-adopt", &fresh.id).contains("NEW-AFTER-RESTART")));

}

/// Real PTYs and foreground children, without relying on paid CLI accounts.
/// Two instances of one provider must remain distinct across GUI disconnect
/// and host adoption. Files are written by the children, so echoed input
/// cannot produce a false positive.
#[cfg(unix)]
#[test]
fn agent_sessions_reattach_to_exact_processes_and_missing_stays_missing() {
    let iso = Isolated::new("agent-reattach");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "agents");
    let ws = "ws-agent-reattach";
    let home = client.create_worktree(ws, "flow", &repo, "agents").unwrap();
    let script = iso.base.join("agent.sh");
    std::fs::write(
        &script,
        r#"stty -echo
printf '%s' "$$" > "$1.pid"
printf '\033[?1049h\033[?2004hwaiting'
dd if=/dev/zero bs=1024 count=600 2>/dev/null
while IFS= read -r line; do
  printf '%s:%s\n' "$$" "$line" >> "$1.replies"
  printf 'received:%s\r\n' "$line"
done
"#,
    )
    .unwrap();
    let quote =
        |path: &std::path::Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let mut sessions = Vec::new();
    for label in ["claude-a", "claude-b", "opencode"] {
        let prefix = iso.base.join(label);
        let command = format!("sh {} {}", quote(&script), quote(&prefix));
        let terminal = client
            .create_terminal_with_shell(
                ws,
                &home.id,
                Some(&command),
                None,
                80,
                24,
                Some("/bin/bash"),
            )
            .unwrap();
        let pid_path = prefix.with_extension("pid");
        assert!(wait_for(Duration::from_secs(15), || pid_path.exists()));
        let pid = std::fs::read_to_string(pid_path).unwrap();
        sessions.push((terminal.id, prefix, pid));
    }
    let shell = client
        .create_terminal_with_shell(ws, &home.id, None, None, 80, 24, Some("/bin/bash"))
        .unwrap();
    let shell_pid_file = iso.base.join("shell.pid");
    client
        .send_terminal_input(
            ws,
            &shell.id,
            format!("printf '%s' \"$$\" > {}\n", quote(&shell_pid_file)).as_bytes(),
        )
        .unwrap();
    assert!(wait_for(Duration::from_secs(15), || shell_pid_file.exists()));
    let shell_pid = std::fs::read_to_string(&shell_pid_file).unwrap();
    let daemon = daemon_pid(&client);
    let host = client.host_info().unwrap().pid;
    drop(client); // GUI transport disconnect, process must keep running.

    for round in 0..3 {
        if round == 1 {
            kill_process(host); // host temporarily absent; daemon must be adopted.
            assert!(wait_for(Duration::from_secs(10), || {
                HostClient::connect_existing().is_err()
            }));
        }
        if round == 2 {
            let mut manifest = daemon_manifest(&iso);
            manifest.version = dragent_host::DAEMON_PROTOCOL_VERSION - 1;
            std::fs::write(
                iso.base.join("pty-daemon.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
        }
        let connected = iso.ensure();
        assert_eq!(connected.list_terminals(ws).unwrap().len(), 4);
        if round == 2 {
            assert_ne!(
                daemon_pid(&connected),
                daemon,
                "daemon upgrade must adopt the same child processes"
            );
        } else {
            assert_eq!(daemon_pid(&connected), daemon);
        }
        for (index, (id, prefix, pid)) in sessions.iter().enumerate() {
            let mut got_checkpoint = false;
            connected
                .stream_terminal_operations(ws, id, 0, |chunk| {
                    let resume = chunk.resume.as_ref().expect("initial atomic checkpoint");
                    assert!(resume.command.as_ref().unwrap().contains("agent.sh"));
                    let mut emulator = dragent_terminal::Emulator::new(1, 1);
                    emulator
                        .restore_display_checkpoint(&resume.display)
                        .unwrap();
                    assert!(emulator.alt_screen_active());
                    got_checkpoint = true;
                    false
                })
                .unwrap();
            assert!(got_checkpoint);
            let nonce = format!("round-{round}-pane-{index}");
            connected
                .send_terminal_input(ws, id, format!("{nonce}\n").as_bytes())
                .unwrap();
            assert!(wait_for(Duration::from_secs(10), || {
                std::fs::read_to_string(prefix.with_extension("replies"))
                    .unwrap_or_default()
                    .contains(&format!("{pid}:{nonce}"))
            }));
            for (other_id, other_prefix, _) in &sessions {
                if other_id != id {
                    assert!(
                        !std::fs::read_to_string(other_prefix.with_extension("replies"))
                            .unwrap_or_default()
                            .contains(&nonce)
                    );
                }
            }
        }
        let restored_pid_file = iso.base.join(format!("shell-{round}.pid"));
        connected
            .send_terminal_input(
                ws,
                &shell.id,
                format!("printf '%s' \"$$\" > {}\n", quote(&restored_pid_file)).as_bytes(),
            )
            .unwrap();
        assert!(wait_for(Duration::from_secs(10), || restored_pid_file.exists()));
        assert_eq!(
            std::fs::read_to_string(restored_pid_file).unwrap(),
            shell_pid
        );
    }
    let connected = iso.ensure();
    connected.close_terminal(ws, &sessions[0].0).unwrap();
    assert!(connected
        .send_terminal_input(ws, &sessions[0].0, b"never a shell\n")
        .is_err());
    assert!(connected
        .stream_terminal_operations(ws, &sessions[0].0, 0, |_| true)
        .is_err());
    let listed = connected.list_terminals(ws).unwrap();
    assert!(!listed.iter().any(|terminal| terminal.id == sessions[0].0
        && matches!(terminal.state, dragent_host::TerminalState::Running)));
    assert!(
        !listed.iter().any(|terminal| terminal.id != shell.id
            && !sessions.iter().any(|(id, _, _)| id == &terminal.id)),
        "reattach must not create replacement sessions"
    );
}

#[test]
fn graceful_restart_kills_sessions_openly() {
    let iso = Isolated::new("grace");
    let client = iso.ensure();
    let (_home, sess) = prime_terminal(
        &client,
        &iso.base,
        "ws-grace",
        "grace-branch",
        "GRACE-MARKER-1",
    );
    let old = daemon_pid(&client).expect("daemon pid");

    client.restart_pty_daemon().expect("restart");
    let new = daemon_pid(&client).expect("daemon pid after restart");
    assert_ne!(old, new, "restart must replace the daemon");

    // Old sessions are openly gone (fresh registry), not hanging.
    let err = client
        .read_terminal("ws-grace", &sess.id, None)
        .expect_err("old session must be gone");
    assert!(err.contains("unknown terminal"), "unexpected: {err}");
    assert!(client.list_terminals("ws-grace").expect("list").is_empty());

    // Recreate works on the new daemon.
    let (_home2, sess2) = prime_terminal(
        &client,
        &iso.base,
        "ws-grace",
        "grace-branch-2",
        "GRACE-MARKER-2",
    );
    assert!(tail(&client, "ws-grace", &sess2.id).contains("GRACE-MARKER-2"));
}

#[test]
fn daemon_crash_respawns_transparently() {
    let iso = Isolated::new("crash");
    let client = iso.ensure();
    let (home, sess) = prime_terminal(
        &client,
        &iso.base,
        "ws-crash",
        "crash-branch",
        "CRASH-MARKER-1",
    );
    let old = daemon_pid(&client).expect("daemon pid");

    kill_process(old);
    // The next op respawns a new daemon transparently.
    let sess2 = client
        .create_terminal(
            "ws-crash",
            &home.id,
            Some("echo CRASH-MARKER-2"),
            None,
            80,
            24,
        )
        .expect("op after daemon crash must respawn");
    let new = daemon_pid(&client).expect("daemon pid after respawn");
    assert_ne!(old, new, "crashed daemon must be replaced");

    // The old session died with the old daemon, openly.
    assert!(client.read_terminal("ws-crash", &sess.id, None).is_err());
    assert!(
        wait_for(Duration::from_secs(20), || tail(
            &client, "ws-crash", &sess2.id
        )
        .contains("CRASH-MARKER-2")),
        "new session on respawned daemon must serve output"
    );
}

#[test]
fn circuit_opens_then_restart_clears() {
    let iso = Isolated::new("circuit");
    // Removed on drop — also when an assertion below panics, so a failure
    // here can't leak fast-respawn mode into the tests that run after it.
    struct FastRespawn;
    impl Drop for FastRespawn {
        fn drop(&mut self) {
            // SAFETY: tests serialize on a global lock (held by `iso`).
            unsafe { std::env::remove_var("DRAGENT_PTY_TEST_FAST") };
        }
    }
    // SAFETY: tests serialize on a global lock, so no concurrent env access.
    unsafe {
        std::env::set_var("DRAGENT_PTY_TEST_FAST", "1");
    }
    let _fast = FastRespawn;
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-circuit");
    let home = client
        .create_worktree("ws-circuit", "flow", &repo, "circuit-branch")
        .expect("create home");
    client
        .create_terminal("ws-circuit", &home.id, None, None, 80, 24)
        .expect("prime");

    // Budget of MAX_CRASHES inside the window: that many kills respawn,
    // the next one trips the circuit.
    let trip = dragent_host::pty_supervisor::MAX_CRASHES + 1;
    for i in 1..=trip {
        let pid = daemon_pid(&client).expect("daemon pid");
        kill_process(pid);
        if i < trip {
            client
                .create_terminal("ws-circuit", &home.id, None, None, 80, 24)
                .expect("respawn before circuit opens");
        } else {
            let err = client
                .create_terminal("ws-circuit", &home.id, None, None, 80, 24)
                .expect_err("circuit must be open");
            assert!(
                err.contains("recovering after repeated failures"),
                "unexpected: {err}"
            );
        }
    }

    // Restart clears the circuit and serves again.
    client.restart_pty_daemon().expect("restart clears circuit");
    let sess = client
        .create_terminal(
            "ws-circuit",
            &home.id,
            Some("echo CIRCUIT-OK"),
            None,
            80,
            24,
        )
        .expect("serves after restart");
    assert!(
        wait_for(Duration::from_secs(20), || tail(
            &client,
            "ws-circuit",
            &sess.id
        )
        .contains("CIRCUIT-OK")),
        "post-restart session must serve output"
    );
}

#[test]
fn handshake_rejects_unknown_version() {
    let iso = Isolated::new("handshake");
    let client = iso.ensure();
    // Lazy daemon: force a spawn first.
    client.restart_pty_daemon().expect("force daemon spawn");
    let manifest = daemon_manifest(&iso);
    // macOS/Linux talk over a Unix domain socket; Windows over loopback TCP.
    #[cfg(unix)]
    assert!(
        manifest.socket.is_some(),
        "daemon should listen on a unix socket"
    );

    let stream =
        dragent_host::transport::Stream::connect(&manifest.address(), Duration::from_secs(2))
            .expect("connect to daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeouts");
    let envelope = dragent_host::DaemonRequestEnvelope {
        version: 999,
        token: manifest.token.clone(),
        request: dragent_host::DaemonRequest::Hello { version: 1 },
    };
    dragent_host::protocol::write_message(&mut &stream, &envelope).expect("write");
    let env: dragent_host::DaemonResponseEnvelope =
        dragent_host::protocol::read_message(&mut &stream).expect("read");
    match env.result {
        Ok(other) => panic!("version 999 must be rejected, got {other:?}"),
        Err(err) => assert!(err.contains("mismatch"), "unexpected: {err}"),
    }
}

// ── SIGHUP boundary: what the kernel does to PTY children when the ──
// daemon dies is a kernel property, not daemon code. Normal shells die
// with the master; HUP-armored processes survive and need explicit cleanup.

#[cfg(unix)]
fn marker_lines(marker: &std::path::Path) -> usize {
    std::fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .count()
}

#[cfg(unix)]
fn pkill_marker(marker: &std::path::Path) {
    let _ = std::process::Command::new("pkill")
        .args(["-f", &marker.to_string_lossy()])
        .status();
}

#[cfg(unix)]
#[test]
fn kernel_sighup_reaps_normal_shells() {
    let iso = Isolated::new("sighup");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-sighup");
    let ws = client.create_workspace(&repo).expect("workspace");
    let home = client
        .create_worktree(&ws, "flow", &repo, "sighup-branch")
        .expect("home");
    let marker = iso.base.join("sighup-normal.log");
    let _ = std::fs::remove_file(&marker);
    let loop_cmd = format!(
        "bash -c 'while true; do echo tick >> \"{}\"; sleep 0.2; done'",
        marker.display()
    );
    let sess = client
        .create_terminal(&ws, &home.id, Some(&loop_cmd), None, 80, 24)
        .expect("terminal");
    assert!(
        wait_for(Duration::from_secs(20), || marker_lines(&marker) >= 3),
        "marker loop never started"
    );
    let daemon = daemon_pid(&client).expect("daemon pid");

    kill_process(daemon);
    // Let SIGHUP delivery settle, then the file must go quiet for 4 s.
    std::thread::sleep(Duration::from_secs(1));
    let before = marker_lines(&marker);
    std::thread::sleep(Duration::from_secs(4));
    let after = marker_lines(&marker);
    pkill_marker(&marker);
    let _ = std::fs::remove_file(&marker);
    assert_eq!(
        before, after,
        "normal shell kept writing after daemon death ({before} -> {after})"
    );
    let _ = sess;
}

#[cfg(unix)]
#[test]
fn hup_armored_processes_survive_and_are_cleaned_up() {
    let iso = Isolated::new("huparmor");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-huparmor");
    let ws = client.create_workspace(&repo).expect("workspace");
    let home = client
        .create_worktree(&ws, "flow", &repo, "huparmor-branch")
        .expect("home");
    let marker = iso.base.join("hup-armored.log");
    let _ = std::fs::remove_file(&marker);
    let loop_cmd = format!(
        "bash -c 'trap \"\" HUP; while true; do echo tick >> \"{}\"; sleep 0.2; done'",
        marker.display()
    );
    let _sess = client
        .create_terminal(&ws, &home.id, Some(&loop_cmd), None, 80, 24)
        .expect("terminal");
    assert!(
        wait_for(Duration::from_secs(20), || marker_lines(&marker) >= 3),
        "armored loop never started"
    );
    let daemon = daemon_pid(&client).expect("daemon pid");

    kill_process(daemon);
    std::thread::sleep(Duration::from_secs(1));
    let before = marker_lines(&marker);
    std::thread::sleep(Duration::from_secs(3));
    let after = marker_lines(&marker);
    assert!(
        after > before,
        "HUP-armored loop should survive daemon death ({before} -> {after})"
    );
    // Explicit cleanup: the survivor is reparented, not reaped.
    pkill_marker(&marker);
    std::thread::sleep(Duration::from_secs(1));
    let quiet = marker_lines(&marker);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        quiet,
        marker_lines(&marker),
        "pkill cleanup must stop the armored loop"
    );
    let _ = std::fs::remove_file(&marker);
}

// ── Cold restore: disk tombstones survive the daemon; teardown purges them.

#[test]
fn session_history_tombstone_survives_daemon_crash() {
    let iso = Isolated::new("tombstone");
    let client = iso.ensure();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "tombstone-home")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, Some("echo TOMBSTONE-MARKER-9"), None, 80, 24)
        .expect("terminal");
    assert!(
        wait_for(Duration::from_secs(20), || tail(&client, &ws, &sess.id)
            .contains("TOMBSTONE-MARKER-9")),
        "marker missing"
    );
    let daemon = daemon_pid(&client).expect("daemon pid");

    // SIGKILL the daemon: no reaper runs, so the tombstone keeps
    // ended_at None while the tail stays readable from disk.
    kill_process(daemon);
    let records = wait_for_record(&client, &sess.id, Duration::from_secs(30));
    let record = records
        .iter()
        .find(|r| r.session_id == sess.id)
        .expect("tombstone must list the dead session");
    assert_eq!(record.workspace_id, ws);
    assert_eq!(record.home_id, home.id);
    assert!(
        record.ended_at_unix_ms.is_none(),
        "crashed session must have no ended_at yet: {record:?}"
    );
    let (back, tail_bytes) = client
        .read_session_history(&sess.id, 65536)
        .expect("tail readable after crash");
    assert_eq!(back.session_id, sess.id);
    assert!(
        String::from_utf8_lossy(&tail_bytes).contains("TOMBSTONE-MARKER-9"),
        "disk tail must contain the marker"
    );
    // The restore plan carries the workspace-filtered tombstone.
    let plan = client
        .restore_plan(&ws, vec![sess.id.clone()], None)
        .expect("plan");
    assert!(
        plan.history.iter().any(|r| r.session_id == sess.id),
        "plan must attach the tombstone"
    );
}

#[test]
fn close_stamps_ended_at_and_teardown_purges_home_histories() {
    let iso = Isolated::new("purge");
    let client = iso.ensure();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "purge-home")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, Some("echo PURGE-MARKER-1"), None, 80, 24)
        .expect("terminal");
    assert!(
        wait_for(Duration::from_secs(20), || tail(&client, &ws, &sess.id)
            .contains("PURGE-MARKER-1")),
        "marker missing"
    );

    client
        .close_terminal(&ws, &sess.id)
        .expect("close stamps history");
    let records = wait_for_record(&client, &sess.id, Duration::from_secs(20));
    let record = records
        .iter()
        .find(|r| r.session_id == sess.id)
        .expect("closed session must leave a tombstone");
    assert!(
        record.ended_at_unix_ms.is_some(),
        "close must stamp ended_at: {record:?}"
    );

    client
        .teardown_standalone_session(&ws, &home.id)
        .expect("teardown");
    assert!(
        wait_for(Duration::from_secs(20), || {
            client
                .list_session_history()
                .map(|rs| !rs.iter().any(|r| r.session_id == sess.id))
                .unwrap_or(false)
        }),
        "teardown must purge the home histories"
    );
}

fn wait_for_record(
    client: &HostClient,
    session_id: &str,
    timeout: Duration,
) -> Vec<dragent_host::HistoryRecord> {
    let start = Instant::now();
    loop {
        if let Ok(records) = client.list_session_history() {
            if records.iter().any(|r| r.session_id == session_id) {
                return records;
            }
        }
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for history {session_id}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The app's restore path (`host_tabs.rs`): a tab saved with
/// (workspace_id, session_id) re-attaches by looking the session up under
/// the SAVED workspace id after a host crash. Local workspace ids are minted
/// fresh per request, so a restore that minted another id would never find
/// the session again.
#[test]
fn restored_tab_reattaches_under_its_saved_workspace_id() {
    let iso = Isolated::new("reattach");
    let client = iso.ensure();
    let root = init_repo(&iso.base, "repo-reattach");
    let workspace_id = client.create_local_workspace(&root).expect("local ws");
    let home = client.ensure_home(&workspace_id, &root).expect("root home");
    let sess = client
        .create_terminal(&workspace_id, &home.id, None, Some(&root), 80, 24)
        .expect("create");

    // Crash the host; the daemon (and the session) survive.
    kill_process(client.host_info().expect("info").pid);
    assert!(
        wait_for(Duration::from_secs(10), || HostClient::connect_existing()
            .is_err()),
        "killed host must stop answering"
    );
    let again = HostClient::ensure_host_with(&host_bin()).expect("re-ensure");

    // Minting a new local workspace for the same root gives a different id
    // that owns no sessions — the bug this guards.
    let fresh = again.create_local_workspace(&root).expect("fresh ws");
    assert_ne!(fresh, workspace_id);
    assert!(again
        .list_terminals(&fresh)
        .expect("list fresh")
        .iter()
        .all(|s| s.id != sess.id));

    // The saved id still finds the running session.
    let listed = again.list_terminals(&workspace_id).expect("list saved");
    assert!(
        listed
            .iter()
            .any(|s| s.id == sess.id && matches!(s.state, dragent_host::TerminalState::Running)),
        "saved session must be listed under its saved workspace id: {listed:?}"
    );
}

/// Explicit close must not leave survivors: a HUP-trapping loop in the shell
/// and a child that detached into its own session both ignore the kernel's
/// hangup, so close escalates to SIGKILL for everything in the tree.
#[cfg(unix)]
#[test]
fn close_kills_hup_armored_and_detached_descendants() {
    let iso = Isolated::new("closekill");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-closekill");
    let ws = client.create_workspace(&repo).expect("workspace");
    let home = client
        .create_worktree(&ws, "flow", &repo, "closekill-branch")
        .expect("home");
    let armored = iso.base.join("close-armored.log");
    let detached = iso.base.join("close-detached.log");
    let loop_for = |marker: &std::path::Path| {
        format!(
            "trap \"\" HUP; while true; do echo tick >> \"{}\"; sleep 0.2; done",
            marker.display()
        )
    };
    let command = format!(
        "perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' bash -c '{}' & bash -c '{}'",
        loop_for(&detached),
        loop_for(&armored)
    );
    let sess = client
        .create_terminal(&ws, &home.id, Some(&command), None, 80, 24)
        .expect("terminal");
    assert!(
        wait_for(Duration::from_secs(20), || marker_lines(&armored) >= 3
            && marker_lines(&detached) >= 3),
        "loops never started"
    );

    client.close_terminal(&ws, &sess.id).expect("close");
    // Grace (1 s) + first SIGKILL round, with margin for a loaded machine.
    std::thread::sleep(Duration::from_secs(3));
    let (a, d) = (marker_lines(&armored), marker_lines(&detached));
    std::thread::sleep(Duration::from_secs(2));
    let (a2, d2) = (marker_lines(&armored), marker_lines(&detached));
    pkill_marker(&armored);
    pkill_marker(&detached);
    assert_eq!(a, a2, "HUP-armored loop survived close ({a} -> {a2})");
    assert_eq!(d, d2, "detached loop survived close ({d} -> {d2})");
}

/// Push delivery: replayed output, then live output, then the exit code as
/// the final message. Offsets are contiguous, and once the shell has exited
/// the session is gone from the live registry while its history remains.
#[test]
fn stream_delivers_catch_up_live_output_and_exit() {
    let iso = Isolated::new("stream");
    let client = iso.ensure();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "stream-home")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, Some("echo STREAM-BEFORE"), None, 80, 24)
        .expect("terminal");
    assert!(
        wait_for(Duration::from_secs(20), || tail(&client, &ws, &sess.id)
            .contains("STREAM-BEFORE")),
        "first marker missing"
    );

    let sender = HostClient::connect_existing().expect("second client");
    let (ws2, id2) = (ws.clone(), sess.id.clone());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let _ = sender.send_terminal_input(&ws2, &id2, b"echo STREAM-AFTER; exit 5\n");
    });

    let mut text = String::new();
    let mut next_offset = 0u64;
    let mut exit = None;
    client
        .stream_terminal(&ws, &sess.id, 0, |chunk| {
            assert_eq!(chunk.base, next_offset, "stream offsets must be contiguous");
            next_offset = chunk.base + chunk.data.len() as u64;
            text.push_str(&String::from_utf8_lossy(&chunk.data));
            exit = chunk.exit_code;
            true
        })
        .expect("stream");
    assert!(text.contains("STREAM-BEFORE"), "catch-up missing: {text:?}");
    assert!(
        text.contains("STREAM-AFTER"),
        "live output missing: {text:?}"
    );
    assert_eq!(exit, Some(5));

    // Removed from the live registry at exit; the history keeps the code.
    assert!(client.read_terminal(&ws, &sess.id, None).is_err());
    let records = wait_for_record(&client, &sess.id, Duration::from_secs(10));
    let record = records.iter().find(|r| r.session_id == sess.id).unwrap();
    assert_eq!(record.exit_code, Some(5), "{record:?}");
}

fn flood_command(marker: &std::path::Path) -> String {
    // ~50 MB of output, then a marker file: the marker can only appear once
    // every byte has been read out of the PTY.
    format!(
        "head -c 50000000 /dev/zero | tr '\\0' x; touch \"{}\"",
        marker.display()
    )
}

/// Flow control: while the only subscriber stops reading, the PTY is paused
/// and the program blocks on its writes instead of racing ahead.
#[cfg(unix)]
#[test]
fn slow_subscriber_pauses_the_program_until_it_catches_up() {
    let iso = Isolated::new("flowctl");
    let client = iso.ensure();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "flow-home")
        .expect("home");
    let marker = iso.base.join("flood-done");
    let sess = client
        .create_terminal(&ws, &home.id, None, None, 80, 24)
        .expect("terminal");

    let mut stalled_ok = None;
    let mut started = false;
    let result = client.stream_terminal(&ws, &sess.id, 0, |_chunk| {
        if !started {
            started = true;
            let sender = HostClient::connect_existing().expect("sender");
            sender
                .send_terminal_input(
                    &ws,
                    &sess.id,
                    format!("{}\n", flood_command(&marker)).as_bytes(),
                )
                .expect("start flood");
            // Stop reading for a while: without flow control the whole
            // flood would be read (and the marker written) in this window.
            std::thread::sleep(Duration::from_secs(4));
            stalled_ok = Some(!marker.exists());
        }
        // Keep reading until the program could finish.
        !marker.exists()
    });
    assert!(result.is_ok(), "stream failed: {result:?}");
    assert_eq!(
        stalled_ok,
        Some(true),
        "program finished while its reader was stalled"
    );
    assert!(
        wait_for(Duration::from_secs(60), || marker.exists()),
        "program must finish once the reader catches up"
    );
    let _ = client.close_terminal(&ws, &sess.id);
}

/// A subscriber that stays congested past the stuck limit is dropped, and
/// the program resumes (it no longer waits on that reader).
#[cfg(unix)]
#[test]
fn stuck_subscriber_is_dropped_and_the_program_resumes() {
    let iso = Isolated::new("stuck");
    // SAFETY: tests serialize on a global lock (held by `iso`).
    unsafe { std::env::set_var(dragent_host::fanout::STUCK_TIMEOUT_ENV, "1500") };
    struct Unset;
    impl Drop for Unset {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(dragent_host::fanout::STUCK_TIMEOUT_ENV) };
        }
    }
    let _unset = Unset;
    let client = iso.ensure();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "stuck-home")
        .expect("home");
    let marker = iso.base.join("stuck-done");
    let sess = client
        .create_terminal(&ws, &home.id, None, None, 80, 24)
        .expect("terminal");

    let mut first = true;
    let result = client.stream_terminal(&ws, &sess.id, 0, |_chunk| {
        if first {
            first = false;
            let sender = HostClient::connect_existing().expect("sender");
            sender
                .send_terminal_input(
                    &ws,
                    &sess.id,
                    format!("{}\n", flood_command(&marker)).as_bytes(),
                )
                .expect("start flood");
            // Never read again: this subscriber is stuck.
            std::thread::sleep(Duration::from_secs(8));
        }
        true
    });
    assert!(result.is_err(), "a stuck subscriber's stream must be ended");
    assert!(
        wait_for(Duration::from_secs(60), || marker.exists()),
        "program must resume once the stuck subscriber is dropped"
    );
    let _ = client.close_terminal(&ws, &sess.id);
}

/// Running (a killed-but-unreaped zombie child of this test counts as gone).
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|out| {
            let stat = String::from_utf8_lossy(&out.stdout);
            out.status.success() && !stat.trim().is_empty() && !stat.trim_start().starts_with('Z')
        })
}

/// Upgrade path: a host left running by an older build (its endpoint
/// records another protocol version, and it cannot answer this client)
/// holds the authority lock. The next client must replace it instead of
/// retrying forever; likewise an outdated PTY daemon is replaced by the host.
#[cfg(unix)]
#[test]
fn unauthenticated_update_preserves_existing_services() {
    let iso = Isolated::new("update-auth-failure");
    let client = iso.ensure();
    let ws = client.create_workspace(&iso.base.to_string_lossy()).unwrap();
    let home = client.create_standalone_session(&ws, "update-home").unwrap();
    let sess = client.create_terminal(&ws, &home.id, None, None, 80, 24).unwrap();
    let old_host = client.host_info().unwrap().pid;
    let original_daemon = daemon_manifest(&iso);
    let original_endpoint = std::fs::read(&iso.endpoint).unwrap();
    let mut endpoint: serde_json::Value = serde_json::from_slice(&original_endpoint).unwrap();
    endpoint["version"] = serde_json::json!(dragent_host::PROTOCOL_VERSION - 1);
    endpoint["token"] = serde_json::json!("outdated");
    std::fs::write(&iso.endpoint, serde_json::to_vec(&endpoint).unwrap()).unwrap();
    let error = HostClient::ensure_host_with(&host_bin()).unwrap_err();
    assert!(error.contains("preserved"), "{error}");
    assert!(pid_alive(old_host));
    assert!(pid_alive(original_daemon.pid));
    // Repair the deliberately corrupted endpoint; the original transport works.
    std::fs::write(&iso.endpoint, original_endpoint).unwrap();
    let mut manifest = original_daemon.clone();
    manifest.version = dragent_host::DAEMON_PROTOCOL_VERSION - 1;
    manifest.token = "outdated".into();
    std::fs::write(iso.base.join("pty-daemon.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = client.send_terminal_input(&ws, &sess.id, b"echo preserved\n").unwrap_err();
    assert!(error.contains("preserved"), "{error}");
    assert!(pid_alive(original_daemon.pid));
    std::fs::write(iso.base.join("pty-daemon.json"), serde_json::to_vec(&original_daemon).unwrap()).unwrap();
    client.close_terminal(&ws, &sess.id).unwrap();
}

/// Same-protocol application update: replace host and daemon while keeping
/// two independently running foreground processes, IDs, and input routing.
#[cfg(unix)]
#[test]
fn application_build_update_preserves_foreground_sessions() {
    let iso = Isolated::new("application-build-update");
    let client = iso.ensure();
    let ws = client.create_workspace(&iso.base.to_string_lossy()).unwrap();
    let home = client.create_standalone_session(&ws, "build-update-home").unwrap();
    let script = iso.base.join("agent.sh");
    std::fs::write(&script, r#"printf 'READY-%s:%s\n' "$1" "$$"
while IFS= read -r value; do printf 'REPLY-%s:%s:%s\n' "$1" "$$" "$value"; done
"#).unwrap();
    let mut sessions = Vec::new();
    for name in ["A", "B"] {
        let command = format!("/bin/bash '{}' {name}", script.display());
        let sess = client.create_terminal(&ws, &home.id, Some(&command), None, 80, 24).unwrap();
        let prefix = format!("READY-{name}:");
        let process = |text: &str| text.split(&prefix).skip(1).find_map(|part| {
            part.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u32>().ok()
        });
        assert!(wait_for(Duration::from_secs(15), || process(&tail(&client, &ws, &sess.id)).is_some()));
        sessions.push((sess.id.clone(), name, process(&tail(&client, &ws, &sess.id)).unwrap()));
    }
    let old_host = client.host_info().unwrap().pid;
    let old_daemon = daemon_manifest(&iso);
    let next_binary = iso.base.join("updated-dragent-host");
    std::fs::copy(host_bin(), &next_binary).unwrap();
    // macOS copyfile may preserve the source mtime. Stamp the installed
    // successor artifact as a new build without changing its wire protocol.
    std::fs::File::options().write(true).open(&next_binary).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now())).unwrap();
    let fresh = HostClient::ensure_host_with(&next_binary).expect("automatic host update");
    assert_ne!(fresh.host_info().unwrap().pid, old_host);
    assert_eq!(fresh.list_terminals(&ws).unwrap().len(), 2);
    let updated_daemon = daemon_manifest(&iso);
    assert_ne!(updated_daemon.pid, old_daemon.pid);
    assert_ne!(updated_daemon.build_id, old_daemon.build_id);
    assert_eq!(updated_daemon.version, old_daemon.version);
    for (id, name, pid) in sessions {
        assert!(pid_alive(pid));
        fresh.send_terminal_input(&ws, &id, format!("input-{name}\n").as_bytes()).unwrap();
        let expected = format!("REPLY-{name}:{pid}:input-{name}");
        assert!(wait_for(Duration::from_secs(15), || tail(&fresh, &ws, &id).contains(&expected)), "same process must receive input: {}", tail(&fresh, &ws, &id));
        assert!(fresh.read_terminal_screen(&ws, &id).unwrap().complete);
        fresh.close_terminal(&ws, &id).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn failed_daemon_successor_preserves_input_transport() {
    let iso = Isolated::new("failed-daemon-update");
    let client = iso.ensure();
    let ws = client.create_workspace(&iso.base.to_string_lossy()).unwrap();
    let home = client.create_standalone_session(&ws, "failed-update-home").unwrap();
    let sess = client.create_terminal(&ws, &home.id, Some("printf ready > before-update.txt"), None, 80, 24).unwrap();
    assert!(wait_for(Duration::from_secs(15), || std::fs::read_to_string(std::path::Path::new(&home.path).join("before-update.txt")).is_ok_and(|value| value == "ready")));
    let original = daemon_manifest(&iso);
    let bad_binary = iso.base.join("not-an-executable");
    std::fs::write(&bad_binary, "invalid successor").unwrap();
    let supervisor = dragent_host::pty_supervisor::PtySupervisor::new(iso.base.clone(), bad_binary);
    supervisor.ensure().expect("compatible original daemon should remain usable");
    assert_eq!(daemon_manifest(&iso).pid, original.pid);
    assert!(pid_alive(original.pid));
    supervisor.ensure().expect("deferred updates should reuse the healthy daemon");
    client.send_terminal_input(&ws, &sess.id, b"printf survived > after-update.txt\n").unwrap();
    assert!(wait_for(Duration::from_secs(15), || std::fs::read_to_string(std::path::Path::new(&home.path).join("after-update.txt")).is_ok_and(|value| value == "survived")), "output after failed successor: {:?}", tail(&client, &ws, &sess.id));
    client.close_terminal(&ws, &sess.id).unwrap();
}

/// Upgrade handoff: a daemon from another build hands its live shells to a
/// successor running the current binary. The SAME shell process keeps
/// running, its earlier output is still replayable, and it keeps taking
/// input — the daemon process underneath it is replaced.
#[cfg(unix)]
#[test]
fn upgrade_hands_live_shells_to_the_new_daemon() {
    let iso = Isolated::new("handoff");
    let client = iso.ensure();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "handoff-home")
        .expect("home");
    let sess = client
        .create_terminal(
            &ws,
            &home.id,
            Some("echo SHELL-PID-BEFORE=$$"),
            None,
            80,
            24,
        )
        .expect("terminal");
    // The echoed command line also contains the marker (as `$$`); take the
    // occurrence the shell expanded to a number.
    let printed_pid = |text: &str| {
        text.split("SHELL-PID-BEFORE=")
            .skip(1)
            .filter_map(|rest| {
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u32>().ok()
            })
            .find(|pid| *pid > 1)
    };
    assert!(
        wait_for(Duration::from_secs(20), || printed_pid(&tail(
            &client, &ws, &sess.id
        ))
        .is_some()),
        "shell never started"
    );
    let shell_pid = printed_pid(&tail(&client, &ws, &sess.id)).unwrap();
    let old_daemon = daemon_manifest(&iso).pid;

    // Make the live daemon look like it came from another build.
    let mut manifest = daemon_manifest(&iso);
    manifest.version = dragent_host::DAEMON_PROTOCOL_VERSION - 1;
    std::fs::write(
        iso.base.join("pty-daemon.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();

    // The next request finds it outdated and upgrades it.
    client
        .send_terminal_input(&ws, &sess.id, b"echo SHELL-PID-AFTER=$$\n")
        .expect("input after upgrade");
    assert!(
        wait_for(Duration::from_secs(20), || tail(&client, &ws, &sess.id)
            .contains(&format!("SHELL-PID-AFTER={shell_pid}"))),
        "the same shell must keep running and take input: {}",
        tail(&client, &ws, &sess.id)
    );
    let text = tail(&client, &ws, &sess.id);
    assert!(
        text.contains("SHELL-PID-BEFORE="),
        "output from before the upgrade must still be replayable: {text:?}"
    );
    let new_manifest = daemon_manifest(&iso);
    assert_ne!(new_manifest.pid, old_daemon, "daemon must be replaced");
    assert_eq!(new_manifest.version, dragent_host::DAEMON_PROTOCOL_VERSION);
    assert!(
        wait_for(Duration::from_secs(10), || !pid_alive(old_daemon)),
        "old daemon must exit after handing over"
    );
    assert!(pid_alive(shell_pid), "shell must survive its old daemon");
    let screen = client
        .read_terminal_screen(&ws, &sess.id)
        .expect("screen after handoff");
    // The handoff carries the display checkpoint, so the successor's screen
    // is exact and reattaching to this shell stays allowed.
    assert!(
        screen.complete,
        "a handoff with a display checkpoint must restore an exact screen"
    );
    let _ = client.close_terminal(&ws, &sess.id);
}

/// Step-7 transport proof: on macOS/Linux the host listens on an
/// owner-only Unix socket next to the endpoint file, and the full
/// tab flow (workspace → home → terminal → push stream) works through
/// it. The client prefers the socket whenever the endpoint carries
/// one, so a green run here means the live path rides UDS.
#[cfg(unix)]
#[test]
fn uds_serves_full_tab_flow() {
    use std::os::unix::fs::PermissionsExt;

    let iso = Isolated::new("udsflow");
    let client = iso.ensure();

    let raw = std::fs::read_to_string(
        std::env::var(dragent_host::ENDPOINT_FILE_ENV).expect("endpoint env"),
    )
    .expect("endpoint file");
    let endpoint: dragent_host::ServiceEndpoint =
        serde_json::from_str(&raw).expect("endpoint json");
    let socket = endpoint.socket.expect("host must publish a socket path");
    // Unix-bound hosts hold no TCP listener at all, so client traffic
    // (which prefers the socket whenever present) cannot ride TCP.
    assert_eq!(endpoint.port, 0, "no TCP listener when UDS-bound");
    let mode = std::fs::metadata(&socket)
        .expect("socket file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "socket must be owner-only");

    let started = Instant::now();
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "uds-home")
        .expect("home");
    let sess = client
        .create_terminal(&ws, &home.id, Some("echo UDS-LIVE-MARKER"), None, 80, 24)
        .expect("terminal");

    let mut text = String::new();
    client
        .stream_terminal(&ws, &sess.id, 0, |chunk| {
            text.push_str(&String::from_utf8_lossy(&chunk.data));
            !text.contains("UDS-LIVE-MARKER")
        })
        .expect("stream");
    assert!(
        text.contains("UDS-LIVE-MARKER"),
        "pushed output missing: {text:?}"
    );
    eprintln!(
        "[uds-live] marker streamed {:.0}ms after spawn over {}",
        started.elapsed().as_secs_f64() * 1000.0,
        socket
    );
    let _ = client.close_terminal(&ws, &sess.id);
}

/// Sets an environment variable for one test (tests hold the global lock).
#[cfg(unix)]
struct TestEnv(&'static str);

#[cfg(unix)]
impl TestEnv {
    fn set(name: &'static str, value: &str) -> Self {
        // SAFETY: tests serialize on a global lock, so no concurrent env access.
        unsafe { std::env::set_var(name, value) };
        Self(name)
    }
}

#[cfg(unix)]
impl Drop for TestEnv {
    fn drop(&mut self) {
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(self.0) };
    }
}

/// Open handles on PTY masters, or `None` where `lsof` isn't installed.
#[cfg(unix)]
fn terminal_handles(pid: u32) -> Option<usize> {
    let output = std::process::Command::new("lsof")
        .args(["-nP", "-p", &pid.to_string()])
        .output()
        .ok()?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains("ptmx"))
            .count(),
    )
}

#[cfg(unix)]
fn start_shell(
    client: &HostClient,
    iso: &Isolated,
    name: &str,
    command: &str,
) -> (String, dragent_host::TerminalSession, u32) {
    let ws = client
        .create_workspace(&iso.base.to_string_lossy())
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, name)
        .expect("home");
    let session = client
        .create_terminal(
            &ws,
            &home.id,
            Some(&format!("echo SHELL-IS=$$; {command}")),
            None,
            80,
            24,
        )
        .expect("terminal");
    let printed = |text: &str| {
        text.split("SHELL-IS=").skip(1).find_map(|rest| {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u32>().ok().filter(|pid| *pid > 1)
        })
    };
    let mut shell = None;
    assert!(
        wait_for(Duration::from_secs(20), || {
            shell = printed(&tail(client, &ws, &session.id));
            shell.is_some()
        }),
        "shell never started"
    );
    (ws, session, shell.unwrap())
}

/// A daemon that keeps its terminals across upgrades must not collect another
/// set of handles each time: 256 of them is enough to stop it accepting
/// connections on macOS, and then it looks dead.
#[cfg(unix)]
#[test]
fn upgrades_do_not_collect_terminal_handles() {
    let iso = Isolated::new("fdgrowth");
    let client = iso.ensure();
    let (ws, session, shell) = start_shell(&client, &iso, "fd-home", "true");
    let mut counts = Vec::new();
    for round in 0..3 {
        let before = daemon_manifest(&iso).pid;
        let mut manifest = daemon_manifest(&iso);
        manifest.version = dragent_host::DAEMON_PROTOCOL_VERSION - 1;
        std::fs::write(
            iso.base.join("pty-daemon.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        client
            .send_terminal_input(
                &ws,
                &session.id,
                format!("echo ROUND-$((40+{round}))\n").as_bytes(),
            )
            .expect("input after upgrade");
        assert!(
            wait_for(Duration::from_secs(20), || tail(&client, &ws, &session.id)
                .contains(&format!("ROUND-{}", 40 + round))
                && !pid_alive(before)),
            "upgrade {round} did not finish"
        );
        match terminal_handles(daemon_manifest(&iso).pid) {
            Some(count) => counts.push(count),
            None => return,
        }
    }
    assert!(pid_alive(shell), "the shell must survive every upgrade");
    assert!(counts[0] > 0, "lsof must see the terminal: {counts:?}");
    assert!(
        counts.iter().all(|count| *count == counts[0]),
        "handles grew across upgrades: {counts:?}"
    );
}

/// Being slow is not being dead: a second daemon would leave the first one's
/// terminals running with nothing able to reach them.
#[cfg(unix)]
#[test]
fn a_daemon_that_stops_answering_is_stopped_before_it_is_replaced() {
    let iso = Isolated::new("frozen");
    let _grace = TestEnv::set("DRAGENT_PTY_UNRESPONSIVE_GRACE_MS", "20000");
    let client = iso.ensure();
    let (ws, session, shell) = start_shell(&client, &iso, "frozen-home", "true");
    let frozen = daemon_manifest(&iso);
    let signal = |name: &str| {
        let _ = std::process::Command::new("kill")
            .args([name, &frozen.pid.to_string()])
            .status();
    };
    signal("-STOP");

    // Within the grace period nothing new may start.
    let _ = client.send_terminal_input(&ws, &session.id, b"echo hello\n");
    assert_eq!(daemon_manifest(&iso).pid, frozen.pid, "a second daemon started");
    assert!(pid_alive(shell), "the shell must still be there");

    // After it, the frozen daemon goes first, and its shell with it.
    let replaced = wait_for(Duration::from_secs(120), || {
        let _ = client.pty_status();
        let _ = client.send_terminal_input(&ws, &session.id, b"\n");
        daemon_manifest(&iso).pid != frozen.pid
    });
    if !replaced {
        signal("-CONT");
    }
    assert!(replaced, "the frozen daemon was never replaced");
    assert!(!pid_alive(frozen.pid), "the frozen daemon must be gone");
    assert!(
        wait_for(Duration::from_secs(15), || !pid_alive(shell)),
        "its shell must not keep running"
    );
}

/// A daemon nothing can reach closes down, unless something is still attached,
/// and doesn't take its replacement's socket with it.
#[cfg(unix)]
#[test]
fn a_replaced_daemon_closes_once_nothing_is_attached() {
    let iso = Isolated::new("replaced");
    let _retire = TestEnv::set("DRAGENT_PTY_RETIRE_AFTER_MS", "1000");
    let client = iso.ensure();
    let (ws, session, shell) = start_shell(
        &client,
        &iso,
        "replaced-home",
        "while :; do echo tick; sleep 1; done",
    );
    let original = daemon_manifest(&iso);

    let attached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stream = {
        let attached = attached.clone();
        let (ws, id) = (ws.clone(), session.id.clone());
        let client = HostClient::connect_existing().expect("second connection");
        std::thread::spawn(move || {
            let _ = client.stream_terminal(&ws, &id, 0, |_| {
                attached.load(std::sync::atomic::Ordering::SeqCst)
            });
        })
    };
    std::thread::sleep(Duration::from_secs(1));

    // Something takes over the manifest while the first daemon is still alive.
    let mut replacement = std::process::Command::new(host_bin())
        .arg(dragent_host::PTY_SERVICE_ARG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("replacement daemon");
    assert!(
        wait_for(Duration::from_secs(10), || daemon_manifest(&iso).pid
            == replacement.id()),
        "the replacement never took over"
    );

    std::thread::sleep(Duration::from_secs(4));
    assert!(pid_alive(original.pid), "a daemon in use must stay");

    attached.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = stream.join();
    assert!(
        wait_for(Duration::from_secs(30), || !pid_alive(original.pid)),
        "the replaced daemon must close down"
    );
    assert!(
        wait_for(Duration::from_secs(15), || !pid_alive(shell)),
        "its terminals must close with it"
    );

    let manifest = daemon_manifest(&iso);
    assert_eq!(manifest.pid, replacement.id(), "the replacement keeps the manifest");
    let (_, _, again) = start_shell(&client, &iso, "after-home", "true");
    assert!(pid_alive(again), "the replacement must still accept work");
    let _ = replacement.kill();
    let _ = replacement.wait();
}
