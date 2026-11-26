//! Services close by themselves when nothing can reach them, when idle (host)
//! or empty (terminal service), and when the test that started them is gone —
//! never while a terminal is running in them. Intervals are shortened through
//! the services' environment. Each test owns a private data folder.

use dragent_host::HostClient;
use dragent_host::protocol::{PTY_SERVICE_ARG, SERVICE_ARG};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(unix)]
const SHELL: Option<&str> = Some("/bin/sh");
#[cfg(not(unix))]
const SHELL: Option<&str> = None;

/// Every knob a test may set; cleared before each test so none leaks.
const KNOBS: [&str; 4] = [
    "DRAGENT_TEST_MEMORY_PRESSURE",
    "DRAGENT_SERVICE_CHECK_MS",
    "DRAGENT_HOST_IDLE_EXIT_MS",
    "DRAGENT_PTY_EMPTY_EXIT_MS",
];

struct Isolated {
    _lock: std::sync::MutexGuard<'static, ()>,
    services: dragent_host::testing::ServiceGuard,
    base: PathBuf,
}

impl Isolated {
    fn new(name: &str, knobs: &[(&str, &str)]) -> Self {
        let lock = lock().lock().unwrap_or_else(|p| p.into_inner());
        // Short: a Unix socket's path may not exceed about 100 characters.
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let base = std::env::temp_dir().join(format!("dl-{name}-{}", &unique[..8]));
        std::fs::create_dir_all(base.join("home")).unwrap();
        // SAFETY: tests in this executable serialize on `lock`.
        unsafe {
            for knob in KNOBS {
                std::env::remove_var(knob);
            }
            for (name, value) in knobs {
                std::env::set_var(name, value);
            }
            std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, base.join("host.json"));
            std::env::set_var("DRAGENT_WORKSPACE_DB", base.join("db.sqlite"));
            std::env::set_var("DRAGENT_HOME", base.join("data"));
            std::env::set_var("HOME", base.join("home"));
            std::env::set_var("ZDOTDIR", base.join("home"));
        }
        Self {
            _lock: lock,
            services: dragent_host::testing::ServiceGuard::new(),
            base,
        }
    }

    fn ensure(&self) -> HostClient {
        HostClient::ensure_host_with(&host_bin()).expect("host should come up")
    }

    fn endpoint(&self) -> PathBuf {
        self.base.join("host.json")
    }

    fn pty_pid(&self) -> Option<u32> {
        dragent_host::pty_supervisor::read_manifest(&self.base).map(|m| m.pid)
    }

    /// A workspace with one running shell; returns (workspace, home, terminal id).
    fn terminal(&self, client: &HostClient) -> (String, String, String) {
        let root = self.base.join("project");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.to_str().unwrap();
        let workspace = client.create_local_workspace(root).unwrap();
        let home = client
            .ensure_terminal_home(&workspace, root, root)
            .unwrap()
            .id;
        let id = self.another(client, &workspace, &home);
        (workspace, home, id)
    }

    fn another(&self, client: &HostClient, workspace: &str, home: &str) -> String {
        let root = self.base.join("project");
        client
            .create_terminal_with_shell(
                workspace,
                home,
                None,
                Some(root.to_str().unwrap()),
                80,
                24,
                SHELL,
            )
            .expect("terminal")
            .id
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        self.services.stop();
        let _ = std::fs::remove_dir_all(&self.base);
        unsafe {
            for knob in KNOBS {
                std::env::remove_var(knob);
            }
        }
    }
}

fn alive(pid: u32, arg: &str) -> bool {
    dragent_host::stale::is_service_process(pid, arg)
}

/// Wait up to `within` for `done`.
fn eventually(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

fn rewrite_token(path: &Path) {
    let mut endpoint: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    endpoint["token"] = "someone-else".into();
    endpoint["pid"] = 999_999.into();
    std::fs::write(path, endpoint.to_string()).unwrap();
}

#[test]
fn a_host_whose_record_is_gone_or_names_another_closes_and_keeps_the_shells() {
    let iso = Isolated::new("record", &[("DRAGENT_SERVICE_CHECK_MS", "200")]);
    let client = iso.ensure();
    let host = client.endpoint().pid;
    iso.terminal(&client);
    let pty = iso.pty_pid().expect("terminal service up");
    std::fs::remove_file(iso.endpoint()).unwrap();
    assert!(
        eventually(Duration::from_secs(10), || !alive(host, SERVICE_ARG)),
        "a host nobody can find any more must close"
    );
    // Its terminals live on in the terminal service.
    assert!(
        alive(pty, PTY_SERVICE_ARG),
        "the shells must outlast the host"
    );

    // A record naming another host: same.
    let client = iso.ensure();
    let second = client.endpoint().pid;
    assert_ne!(second, host);
    rewrite_token(&iso.endpoint());
    assert!(
        eventually(Duration::from_secs(10), || !alive(second, SERVICE_ARG)),
        "a host whose record names another must close"
    );
    assert!(alive(pty, PTY_SERVICE_ARG));
}

// Windows can't delete a folder whose files are open.
#[cfg(unix)]
#[test]
fn without_their_data_folder_the_host_closes_and_the_terminal_service_waits_for_its_shells() {
    let iso = Isolated::new("folder", &[("DRAGENT_SERVICE_CHECK_MS", "200")]);
    let client = iso.ensure();
    let host = client.endpoint().pid;
    iso.terminal(&client);
    let pty = iso.pty_pid().expect("terminal service up");
    let shells = children(pty);
    assert!(!shells.is_empty());
    std::fs::remove_dir_all(&iso.base).unwrap();
    assert!(
        eventually(Duration::from_secs(15), || !alive(host, SERVICE_ARG)),
        "the host must close once its data folder is gone"
    );
    // Many checks later: a folder that is gone may only be unmounted, and the
    // shells exist nowhere else.
    std::thread::sleep(Duration::from_secs(2));
    assert!(alive(pty, PTY_SERVICE_ARG), "it holds a terminal: it stays");
    assert_eq!(children(pty), shells);
    for shell in shells {
        // SAFETY: this test's own shell.
        unsafe { libc::kill(shell, libc::SIGKILL) };
    }
    assert!(
        eventually(Duration::from_secs(15), || !alive(pty, PTY_SERVICE_ARG)),
        "with its last terminal gone nothing is left to wait for"
    );
}

#[cfg(unix)]
fn children(pid: u32) -> Vec<i32> {
    let out = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// The terminal service's record is deleted while it runs a shell: it writes
/// the record again, and the shell is still found through it.
#[test]
fn a_terminal_service_puts_its_missing_record_back() {
    let iso = Isolated::new("record-back", &[("DRAGENT_SERVICE_CHECK_MS", "200")]);
    let client = iso.ensure();
    let (workspace, _, terminal) = iso.terminal(&client);
    let before = dragent_host::pty_supervisor::read_manifest(&iso.base).unwrap();
    std::fs::remove_file(dragent_host::pty_supervisor::manifest_path(&iso.base)).unwrap();
    assert!(
        eventually(Duration::from_secs(10), || {
            dragent_host::pty_supervisor::read_manifest(&iso.base).is_some()
        }),
        "a service holding terminals must stay findable"
    );
    let after = dragent_host::pty_supervisor::read_manifest(&iso.base).unwrap();
    assert_eq!(
        (after.pid, &after.daemon_id, &after.token),
        (before.pid, &before.daemon_id, &before.token)
    );
    assert!(client
        .list_terminals(&workspace)
        .unwrap()
        .iter()
        .any(|t| t.id == terminal));
}

/// A closing host owns its endpoint a moment longer and turns a new one away;
/// asking for a host right then must wait for the new one, not fail.
#[test]
fn a_host_asked_for_while_another_is_closing_comes_up() {
    let iso = Isolated::new("closing", &[]);
    let closing = dragent_host::discovery::acquire_host_authority()
        .unwrap()
        .expect("nobody owns this endpoint yet");
    let closed = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        drop(closing);
    });
    let client = iso.ensure();
    closed.join().unwrap();
    assert!(client.ping());
}

/// A host whose record names another may share its socket path with it: the
/// file belongs to the other one.
#[cfg(unix)]
#[test]
fn a_replaced_host_leaves_its_replacements_socket_alone() {
    let iso = Isolated::new("socket", &[("DRAGENT_SERVICE_CHECK_MS", "200")]);
    let client = iso.ensure();
    let host = client.endpoint().pid;
    let socket = PathBuf::from(client.endpoint().socket.clone().expect("unix socket"));
    // The replacement's listener takes the path over.
    std::fs::remove_file(&socket).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    rewrite_token(&iso.endpoint());
    assert!(eventually(Duration::from_secs(10), || !alive(
        host,
        SERVICE_ARG
    )));
    assert!(
        std::os::unix::net::UnixStream::connect(&socket).is_ok(),
        "the replacement became unreachable"
    );
    drop(listener);
}

#[test]
fn an_idle_host_closes_and_comes_back_at_once_with_its_terminals() {
    let iso = Isolated::new(
        "idle",
        &[
            ("DRAGENT_SERVICE_CHECK_MS", "200"),
            ("DRAGENT_HOST_IDLE_EXIT_MS", "1500"),
        ],
    );
    let client = iso.ensure();
    let host = client.endpoint().pid;
    let (workspace, _, terminal) = iso.terminal(&client);
    let pty = iso.pty_pid().expect("terminal service up");
    assert!(
        eventually(Duration::from_secs(15), || !alive(host, SERVICE_ARG)),
        "a host with no client must close after the idle period"
    );
    // No app connected never closes the terminal service: its shell runs on.
    assert!(alive(pty, PTY_SERVICE_ARG));
    // The app's next request brings a host back within a second, and the
    // terminal is still there, still running.
    let started = Instant::now();
    let client = iso.ensure();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(1), "host restart took {took:?}");
    assert_ne!(client.endpoint().pid, host);
    let terminals = client.list_terminals(&workspace).expect("list");
    let session = terminals
        .iter()
        .find(|t| t.id == terminal)
        .expect("the terminal survived the host's idle exit");
    assert!(matches!(
        session.state,
        dragent_host::protocol::TerminalState::Running
    ));
    assert_eq!(iso.pty_pid(), Some(pty), "same terminal service, adopted");
}

/// Agents that wait to start or are paused are only known to a host: it
/// stays up for them however long nobody asks it anything.
#[cfg(unix)]
#[test]
fn an_idle_host_stays_up_for_waiting_and_paused_agents() {
    use dragent_host::agent_memory::{PAUSED_FILENAME, PausedAgent, Proc};
    let knobs = [
        ("DRAGENT_SERVICE_CHECK_MS", "200"),
        ("DRAGENT_HOST_IDLE_EXIT_MS", "1500"),
        ("DRAGENT_TEST_MEMORY_PRESSURE", "critical"),
    ];
    let iso = Isolated::new("held", &knobs);
    // A start that waits.
    let client = iso.ensure();
    let host = client.endpoint().pid;
    let ask = dragent_host::protocol::StartAsk {
        id: "pane".into(),
        terminal_id: "term".into(),
        label: "claude".into(),
        since_ms: 1,
    };
    let answer = client.agent_start(ask, Duration::from_secs(5)).unwrap();
    assert!(matches!(answer, dragent_host::protocol::StartAnswer::Wait { .. }));
    drop(client);
    std::thread::sleep(Duration::from_secs(5));
    assert!(alive(host, SERVICE_ARG), "a start waits");
    // It goes away: the host closes.
    assert!(eventually(Duration::from_secs(30), || !alive(host, SERVICE_ARG)));

    // A paused agent, as an earlier host left it.
    let mut agent = std::process::Command::new("sleep").arg("600").spawn().unwrap();
    let paused = serde_json::json!({ "paused": [PausedAgent {
        pane: uuid::Uuid::new_v4(),
        terminal_id: "term".into(),
        label: "claude".into(),
        bytes: 1 << 30,
        root: Proc::of_pid(agent.id()).unwrap(),
        stopped: Vec::new(),
    }]});
    std::fs::write(iso.base.join(PAUSED_FILENAME), paused.to_string()).unwrap();
    let client = iso.ensure();
    let host = client.endpoint().pid;
    drop(client);
    std::thread::sleep(Duration::from_secs(5));
    assert!(alive(host, SERVICE_ARG), "an agent is paused");
    iso.ensure().resume_agent("term").unwrap();
    assert!(
        eventually(Duration::from_secs(15), || !alive(host, SERVICE_ARG)),
        "resumed: nothing keeps it up any more"
    );
    agent.kill().unwrap();
    agent.wait().unwrap();
}

#[test]
fn a_connected_app_keeps_the_host_up() {
    let iso = Isolated::new(
        "presence",
        &[
            ("DRAGENT_SERVICE_CHECK_MS", "200"),
            ("DRAGENT_HOST_IDLE_EXIT_MS", "800"),
        ],
    );
    let client = iso.ensure();
    let host = client.endpoint().pid;
    // What the app holds for its lifetime (`hold_presence`).
    let presence = client.clone();
    std::thread::spawn(move || presence.hold_open());
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        alive(host, SERVICE_ARG),
        "an app is connected: the host stays"
    );
}

#[test]
fn the_terminal_service_closes_only_once_it_holds_no_terminals() {
    let iso = Isolated::new(
        "empty",
        &[
            ("DRAGENT_SERVICE_CHECK_MS", "200"),
            ("DRAGENT_PTY_EMPTY_EXIT_MS", "1000"),
        ],
    );
    let client = iso.ensure();
    let (workspace, home, terminal) = iso.terminal(&client);
    let pty = iso.pty_pid().expect("terminal service up");
    // A running terminal (an agent, maybe) keeps it up, with no app attached.
    std::thread::sleep(Duration::from_secs(3));
    assert!(alive(pty, PTY_SERVICE_ARG), "it holds a terminal: it stays");
    client.close_terminal(&workspace, &terminal).expect("close");
    assert!(
        eventually(Duration::from_secs(10), || !alive(pty, PTY_SERVICE_ARG)),
        "an empty terminal service must close after its grace period"
    );
    assert!(iso.pty_pid().is_none(), "its record goes with it");
    // The host doesn't count that as a crash and start an empty one again…
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        iso.pty_pid().is_none(),
        "nothing restarted an empty service"
    );
    // …and the next terminal gets a fresh one.
    let again = iso.another(&client, &workspace, &home);
    assert!(!again.is_empty());
    let fresh = iso.pty_pid().expect("fresh terminal service");
    assert_ne!(fresh, pty);
}

/// Run as a child process by the test below: start services under the
/// guard, report their pids, then die without running any destructor.
#[test]
#[ignore = "helper process for services_close_when_the_test_that_started_them_dies"]
fn guard_child() {
    let Ok(out) = std::env::var("LIFECYCLE_GUARD_CHILD_OUT") else {
        return;
    };
    let iso = Isolated::new("child", &[("DRAGENT_SERVICE_CHECK_MS", "200")]);
    let client = iso.ensure();
    iso.terminal(&client);
    let pids = format!("{} {}", client.endpoint().pid, iso.pty_pid().unwrap());
    std::fs::write(out, pids).unwrap();
    std::process::abort();
}

#[test]
fn services_close_when_the_test_that_started_them_dies() {
    let _lock = lock().lock().unwrap_or_else(|p| p.into_inner());
    let out = std::env::temp_dir().join(format!("dragent-guard-child-{}", uuid::Uuid::new_v4()));
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "guard_child", "--ignored", "--nocapture"])
        .env("LIFECYCLE_GUARD_CHILD_OUT", &out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "the child aborts on purpose");
    let pids = std::fs::read_to_string(&out).expect("child reported its services");
    let _ = std::fs::remove_file(&out);
    let mut pids = pids.split(' ').map(|p| p.parse::<u32>().unwrap());
    let (host, pty) = (pids.next().unwrap(), pids.next().unwrap());
    assert!(
        eventually(Duration::from_secs(15), || {
            !alive(host, SERVICE_ARG) && !alive(pty, PTY_SERVICE_ARG)
        }),
        "services must close once the test that started them is gone"
    );
}

#[test]
fn the_guard_stops_services_when_a_test_panics() {
    let (host, pty) = {
        let outcome = std::panic::catch_unwind(|| {
            let iso = Isolated::new("panic", &[]);
            let client = iso.ensure();
            iso.terminal(&client);
            let pids = (client.endpoint().pid, iso.pty_pid().unwrap());
            std::panic::panic_any(pids);
        });
        *outcome
            .unwrap_err()
            .downcast::<(u32, u32)>()
            .expect("pids carried by the panic")
    };
    // The guard ran during unwinding and waited: both are gone already.
    assert!(!alive(host, SERVICE_ARG), "host left running after a panic");
    assert!(
        !alive(pty, PTY_SERVICE_ARG),
        "terminal service left running"
    );
}
