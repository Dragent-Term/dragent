//! Phase-6.2 terminal session tests: real host processes + isolated org dirs.
//!
//! Proves terminals owned per home end to end: lifecycle round-trip (spawn,
//! echo markers, tail cap, resize, list gate, close), cwd containment +
//! unknown-home rejection + initial command, exit-keeps-tail (exit code,
//! dead-send rejection, close drops), absolute-offset delta reads, and
//! teardown fan-out over attached sessions.

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
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Isolated {
    fn new(name: &str) -> Self {
        let guard = lock().lock().unwrap_or_else(|p| p.into_inner());
        let base =
            std::env::temp_dir().join(format!("dragent-host-term-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("isolated subdir");
        let endpoint = base.join("host.json");
        let db = base.join("db.sqlite");
        for sidecar in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{sidecar}", db.display()));
        }
        let mut previous = Vec::new();
        for (name, path) in [
            (dragent_host::ENDPOINT_FILE_ENV, endpoint.clone()),
            ("DRAGENT_WORKSPACE_DB", db.clone()),
            ("DRAGENT_HOME", base.join("data")),
            ("HOME", base.join("home")),
            ("ZDOTDIR", base.join("home")),
            ("XDG_CONFIG_HOME", base.join("home/.config")),
        ] {
            if name != dragent_host::ENDPOINT_FILE_ENV && name != "DRAGENT_WORKSPACE_DB" {
                std::fs::create_dir_all(&path).expect("isolated home");
            }
            previous.push((name, std::env::var_os(name)));
            // SAFETY: tests serialize on a global lock.
            unsafe {
                std::env::set_var(name, path);
            }
        }
        Self {
            services: dragent_host::testing::ServiceGuard::new(),
            _guard: guard,
            base,
            endpoint,
            db,
            previous,
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
        for (name, previous) in &self.previous {
            unsafe {
                if let Some(value) = previous {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
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

fn tail(client: &HostClient, ws: &str, id: &str) -> String {
    client
        .read_terminal(ws, id, None)
        .map(|(text, _, _)| text)
        .unwrap_or_default()
}

/// Apps send a colour query followed by a device query as a marker, and treat
/// an answer arriving after the marker's as no answer. So the daemon must answer
/// both, in order, and tell the app when its dark/light scheme flips.
#[cfg(unix)]
#[test]
fn colour_queries_answer_in_order_and_scheme_flips_reach_the_app() {
    use dragent_host::screen_protocol::TerminalColors;
    let iso = Isolated::new("colours");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-colours");
    let home = client
        .create_worktree("ws-colours", "flow", &repo, "colours-branch")
        .unwrap();
    let script = r#"bash -c 'stty raw -echo; read -r -t 5 go; printf "\033[?2031h\033]11;?\007\033[c"; IFS= read -r -t 5 -d c reply; IFS= read -r -t 20 -d n flip; stty sane; printf "GOT[%q]FLIP[%q]END\n" "$reply" "$flip"; sleep 5'"#;
    let session = client
        .create_terminal("ws-colours", &home.id, Some(script), None, 80, 24)
        .unwrap();
    let light = TerminalColors {
        foreground: (0, 0, 0),
        background: (250, 250, 250),
        is_dark: false,
    };
    client
        .set_terminal_colors("ws-colours", &session.id, light)
        .unwrap();
    // Start it only once the script runs (the shell reports it), or the
    // shell takes "go" itself and every read in the script times out.
    assert!(wait_for(Duration::from_secs(20), || tail(
        &client,
        "ws-colours",
        &session.id
    )
    .contains("7979;preexec;bash -c")));
    std::thread::sleep(Duration::from_millis(500));
    client
        .send_terminal_input("ws-colours", &session.id, b"go\n")
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    client
        .set_terminal_colors("ws-colours", &session.id, TerminalColors::default())
        .unwrap();
    // The script's own report, not the command line echoing its format
    // string (which also contains "GOT[%q]…END").
    let reported = |output: &str| {
        output
            .match_indices("GOT[")
            .any(|(at, _)| !output[at..].starts_with("GOT[%q]") && output[at..].contains("END"))
    };
    assert!(
        wait_for(Duration::from_secs(20), || reported(&tail(
            &client,
            "ws-colours",
            &session.id
        ))),
        "output: {:?}",
        tail(&client, "ws-colours", &session.id)
    );
    let output = tail(&client, "ws-colours", &session.id);
    let colour = output
        .find("rgb:fafa/fafa/fafa")
        .unwrap_or_else(|| panic!("colour answer missing: {output:?}"));
    let marker = output[colour..].find("[?6").expect("marker after colour");
    assert!(marker > 0, "{output:?}");
    assert!(output.contains("?997;1"), "{output:?}");
}

/// A daemon started from an agent's tool shell inherits `NO_COLOR=1`. Terminals
/// it opens must not: agents told that draw without any colour of their own.
#[cfg(unix)]
#[test]
fn terminals_do_not_inherit_a_no_colour_environment() {
    let iso = Isolated::new("nocolor");
    // SAFETY: tests serialize on a global lock.
    unsafe {
        std::env::set_var("NO_COLOR", "1");
        std::env::set_var("COLORTERM", "");
    }
    let client = iso.ensure();
    // SAFETY: as above; the daemon has already inherited them.
    unsafe {
        std::env::remove_var("NO_COLOR");
        std::env::remove_var("COLORTERM");
    }
    let repo = init_repo(&iso.base, "repo-nocolor");
    let home = client
        .create_worktree("ws-nocolor", "flow", &repo, "nocolor-branch")
        .unwrap();
    let script = r#"printf 'NC[%s]CT[%s]\n' "${NO_COLOR-unset}" "${COLORTERM-unset}""#;
    let session = client
        .create_terminal("ws-nocolor", &home.id, Some(script), None, 80, 24)
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || tail(
            &client,
            "ws-nocolor",
            &session.id
        )
        .contains("NC[unset]CT[truecolor]")),
        "output: {:?}",
        tail(&client, "ws-nocolor", &session.id)
    );
}

#[test]
fn lifecycle_round_trip() {
    let iso = Isolated::new("lifecycle");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-life");
    let home = client
        .create_worktree("ws-life", "flow", &repo, "life-branch")
        .expect("create home");

    let sess = client
        .create_terminal(
            "ws-life",
            &home.id,
            Some("echo LIFE-MARKER-1"),
            None,
            80,
            24,
        )
        .expect("create terminal");
    assert!(sess.id.starts_with("term-"));
    assert_eq!(sess.workspace_id, "ws-life");
    assert_eq!(sess.worktree_id, home.id);
    assert!(matches!(sess.state, dragent_host::TerminalState::Running));

    // Echo marker lands in the tail.
    assert!(
        wait_for(Duration::from_secs(20), || tail(
            &client, "ws-life", &sess.id
        )
        .contains("LIFE-MARKER-1")),
        "tail: {:?}",
        tail(&client, "ws-life", &sess.id)
    );
    // Tail cap still carries the marker; interactive input works.
    let (capped, alive, exit) = client
        .read_terminal("ws-life", &sess.id, Some(10))
        .expect("capped read");
    assert!(alive);
    assert_eq!(exit, None);
    assert!(
        capped.contains("LIFE-MARKER-1"),
        "capped tail must keep the marker, got: {capped:?}"
    );
    client
        .send_terminal_input("ws-life", &sess.id, b"echo LIFE-MARKER-2\n")
        .expect("send input");
    assert!(
        wait_for(Duration::from_secs(20), || tail(
            &client, "ws-life", &sess.id
        )
        .contains("LIFE-MARKER-2")),
        "second marker missing"
    );

    // Resize updates the record.
    let resized = client
        .resize_terminal("ws-life", &sess.id, 100, 40)
        .expect("resize");
    assert_eq!((resized.cols, resized.rows), (100, 40));
    assert_eq!(client.terminal("ws-life", &sess.id).expect("get").cols, 100);

    // List gate: visible to the owner, invisible + unreadable elsewhere.
    let listed = client.list_terminals("ws-life").expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, sess.id);
    assert!(client
        .list_terminals("ws-foreign")
        .expect("list")
        .is_empty());
    assert!(client.terminal("ws-foreign", &sess.id).is_err());
    assert!(client.read_terminal("ws-foreign", &sess.id, None).is_err());

    // Close drops the session.
    client.close_terminal("ws-life", &sess.id).expect("close");
    assert!(client.list_terminals("ws-life").expect("list").is_empty());
    assert!(client.read_terminal("ws-life", &sess.id, None).is_err());
}

#[test]
fn cwd_containment_unknown_home_initial_command() {
    let iso = Isolated::new("cwdgate");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-cwd");
    let home = client
        .create_worktree("ws-cwd", "flow", &repo, "cwd-branch")
        .expect("create home");

    // cwd outside the checkout is rejected.
    let err = client
        .create_terminal("ws-cwd", &home.id, None, Some("/tmp"), 80, 24)
        .expect_err("outside cwd must be rejected");
    assert!(err.contains("inside"), "unexpected: {err}");
    // Unknown and foreign homes are gated as unknown worktrees.
    assert!(client
        .create_terminal("ws-cwd", "wt-does-not-exist", None, None, 80, 24)
        .is_err());
    assert!(client
        .create_terminal("ws-foreign", &home.id, None, None, 80, 24)
        .expect_err("foreign home must be gated")
        .contains("unknown worktree"));

    // Initial command runs without a separate send.
    let sess = client
        .create_terminal("ws-cwd", &home.id, Some("echo INIT-CMD-OK"), None, 80, 24)
        .expect("create with initial command");
    assert!(
        wait_for(Duration::from_secs(20), || tail(
            &client, "ws-cwd", &sess.id
        )
        .contains("INIT-CMD-OK")),
        "initial command output missing"
    );
    client.close_terminal("ws-cwd", &sess.id).expect("close");
}

#[test]
fn exit_leaves_history_and_rejects_input() {
    let iso = Isolated::new("exitcode");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-exit");
    let home = client
        .create_worktree("ws-exit", "flow", &repo, "exit-branch")
        .expect("create home");

    let sess = client
        .create_terminal("ws-exit", &home.id, Some("exit 42"), None, 80, 24)
        .expect("create");
    // The shell exits 42: the session leaves the live registry at once
    // (streaming subscribers got the exit code as their final message) ...
    assert!(
        wait_for(Duration::from_secs(20), || client
            .terminal("ws-exit", &sess.id)
            .is_err()),
        "exited session should leave the live registry"
    );
    assert!(client.list_terminals("ws-exit").expect("list").is_empty());
    // ... dead shells reject input ...
    assert!(client
        .send_terminal_input("ws-exit", &sess.id, b"echo nope\n")
        .is_err());
    // ... and the history keeps the exit code and the final output.
    let (record, tail) = client
        .read_session_history(&sess.id, 64 * 1024)
        .expect("history after exit");
    assert_eq!(record.exit_code, Some(42), "{record:?}");
    assert!(record.ended_at_unix_ms.is_some(), "{record:?}");
    let text = String::from_utf8_lossy(&tail);
    assert!(
        text.contains("exit 42"),
        "echo of the command should persist: {text:?}"
    );
}

#[test]
fn delta_read_since_zero_then_since_total() {
    let iso = Isolated::new("delta");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-delta");
    let home = client
        .create_worktree("ws-delta", "flow", &repo, "delta-branch")
        .expect("create home");

    let sess = client
        .create_terminal(
            "ws-delta",
            &home.id,
            Some("echo DELTA-MARKER"),
            None,
            80,
            24,
        )
        .expect("create");
    // Since-zero eventually carries the marker; offsets are absolute.
    let (mut base, mut total) = (0u64, 0u64);
    assert!(
        wait_for(Duration::from_secs(20), || {
            match client.read_terminal_since("ws-delta", &sess.id, 0, 1024 * 1024) {
                Ok((data, b, t)) => {
                    base = b;
                    total = t;
                    String::from_utf8_lossy(&data).contains("DELTA-MARKER")
                }
                Err(_) => false,
            }
        }),
        "delta since-zero missing marker"
    );
    assert_eq!(base, 0);
    assert!(total > 0);
    // The shell may still be drawing its next prompt: wait until the output
    // stops growing, so `total` is the end of the stream.
    let mut settled = 0;
    while settled < 3 {
        std::thread::sleep(Duration::from_millis(150));
        let (_, _, now) = client
            .read_terminal_since("ws-delta", &sess.id, 0, 1024 * 1024)
            .expect("read");
        if now == total {
            settled += 1;
        } else {
            total = now;
            settled = 0;
        }
    }
    // Since-total returns an empty delta anchored at total.
    let (data, b, t) = client
        .read_terminal_since("ws-delta", &sess.id, total, 1024 * 1024)
        .expect("since-total");
    assert!(data.is_empty());
    assert_eq!((b, t), (total, total));
    // More output advances total; the delta is exactly the new bytes.
    client
        .send_terminal_input("ws-delta", &sess.id, b"echo DELTA-2\n")
        .expect("send");
    assert!(
        wait_for(Duration::from_secs(20), || {
            client
                .read_terminal_since("ws-delta", &sess.id, total, 1024 * 1024)
                .map(|(d, _, _)| String::from_utf8_lossy(&d).contains("DELTA-2"))
                .unwrap_or(false)
        }),
        "delta-2 missing"
    );
    // Unknown sessions and foreign workspaces error.
    assert!(client
        .read_terminal_since("ws-delta", "term-nope", 0, 100)
        .is_err());
    assert!(client
        .read_terminal_since("ws-foreign", &sess.id, 0, 100)
        .is_err());
    client.close_terminal("ws-delta", &sess.id).expect("close");
}

#[test]
fn teardown_fan_out_closes_attached_sessions() {
    let iso = Isolated::new("fanout");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-fanout");
    let home = client
        .create_worktree("ws-fan", "flow", &repo, "fan-branch")
        .expect("create home");

    let first = client
        .create_terminal("ws-fan", &home.id, None, None, 80, 24)
        .expect("term 1");
    let second = client
        .create_terminal("ws-fan", &home.id, None, None, 80, 24)
        .expect("term 2");
    assert_eq!(client.list_terminals("ws-fan").expect("list").len(), 2);

    // Teardown fans out to both attached terminals (counts) and removes the
    // checkout; the terminals are gone with it.
    let report = client
        .teardown_worktree("ws-fan", &home.id)
        .expect("teardown");
    assert_eq!(report.sessions_detached, 2);
    assert!(
        !PathBuf::from(&home.path).exists(),
        "checkout dir must be gone"
    );
    assert!(client.list_terminals("ws-fan").expect("list").is_empty());
    assert!(client.read_terminal("ws-fan", &first.id, None).is_err());
    assert!(client.read_terminal("ws-fan", &second.id, None).is_err());
}

#[test]
fn daemon_screen_without_ui_tracks_alternate_screen_and_resize() {
    let iso = Isolated::new("screen");
    let client = iso.ensure();
    let repo = init_repo(&iso.base, "repo-screen");
    let home = client
        .create_worktree("ws-screen", "flow", &repo, "screen-branch")
        .unwrap();
    let session = client
        .create_terminal(
            "ws-screen",
            &home.id,
            // Keep the foreground command alive. Returning to the shell
            // prompt intentionally restores the primary screen.
            Some("printf '\\033[?1049h\\033[2J\\033[HDAEMON-SCREEN'; read -r reply; printf '\\033[?1049l'"),
            None,
            80,
            24,
        )
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || {
            client
                .read_terminal_screen("ws-screen", &session.id)
                .is_ok_and(|screen| {
                    screen.alternate_screen
                        && screen
                            .lines
                            .iter()
                            .flat_map(|r| &r.spans)
                            .any(|span| span.text.contains("DAEMON-SCREEN"))
                })
        }),
        "screen: {:?}; output: {:?}",
        client.read_terminal_screen("ws-screen", &session.id),
        tail(&client, "ws-screen", &session.id)
    );
    let screen = client
        .read_terminal_screen("ws-screen", &session.id)
        .unwrap();
    assert!(screen.complete);
    assert_eq!(screen.lines.len(), 24);
    assert!(screen.output_offset > 0);
    let raw = client
        .read_terminal_chunk("ws-screen", &session.id, screen.output_offset, 1024)
        .unwrap();
    assert!(
        raw.total >= screen.output_offset,
        "snapshot must not lead published output"
    );
    assert!(client
        .read_terminal_screen("foreign-workspace", &session.id)
        .is_err());
    assert!(client.read_terminal_screen("ws-screen", "missing").is_err());
    client
        .resize_terminal("ws-screen", &session.id, 100, 30)
        .unwrap();
    let screen = client
        .read_terminal_screen("ws-screen", &session.id)
        .unwrap();
    assert_eq!(
        (screen.cols, screen.rows, screen.lines.len()),
        (100, 30, 30)
    );
    // A newly attached client sees the same daemon state without replaying bytes.
    let attached = HostClient::connect_existing().unwrap();
    assert!(
        attached
            .read_terminal_screen("ws-screen", &session.id)
            .unwrap()
            .alternate_screen
    );
    client
        .send_terminal_input("ws-screen", &session.id, b"\n")
        .unwrap();
    assert!(wait_for(Duration::from_secs(10), || {
        attached
            .read_terminal_screen("ws-screen", &session.id)
            .is_ok_and(|screen| !screen.alternate_screen)
    }));
    client.close_terminal("ws-screen", &session.id).unwrap();
}
