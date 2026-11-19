//! Phase-6.3 agent run tests: real host processes + isolated org dirs.
//!
//! Proves agent runs in terminal sessions end to end: catalog seeding,
//! argv echo runs (screen shows prompt, tracked, workspace-gated, close
//! drops), stdin heredoc runs, unknown agent/home rejection, teardown ending
//! runs, command-swap overrides, context tails + staged attachments, and the
//! honest readiness shape.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use dragent_host::{AgentDispatch, AgentOverrides, HostClient, PromptTransport};

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
        let base =
            std::env::temp_dir().join(format!("dragent-host-ag-{name}-{}", std::process::id()));
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

fn screen(client: &HostClient, run_id: &str) -> String {
    client
        .agent_run(run_id)
        .map(|r| r.output)
        .unwrap_or_default()
}

fn add_echo(client: &HostClient, label: &str, transport: PromptTransport) -> String {
    client
        .add_agent_config(label, "echo", vec![], transport, vec![], BTreeMap::new())
        .expect("add echo config")
        .id
}

#[test]
fn catalog_seeds_on_first_open() {
    let iso = Isolated::new("catalog");
    let client = iso.ensure();
    let configs = client.list_agent_configs().expect("list configs");
    assert_eq!(configs.len(), 6);
    let mut orders: Vec<u32> = configs.iter().map(|c| c.order).collect();
    orders.sort();
    assert_eq!(orders, vec![1, 2, 3, 4, 5, 6]);
    for config in &configs {
        assert!(config.id.starts_with("preset-"), "id: {}", config.id);
    }
    let claude = configs
        .iter()
        .find(|c| c.id == "preset-claude")
        .expect("claude preset");
    assert!(
        claude
            .args
            .contains(&"--dangerously-skip-permissions".to_string()),
        "claude args: {:?}",
        claude.args
    );
}

#[test]
fn argv_echo_run_tracked_gated_close_drops() {
    let iso = Isolated::new("argv");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    let home = client
        .create_standalone_session("ws-argv", "argv")
        .expect("home");
    let run = client
        .run_agent(
            "ws-argv",
            &home.id,
            &agent,
            "argv-prompt-marker-1",
            AgentDispatch::default(),
        )
        .expect("run agent");
    assert_eq!(run.workspace_id, "ws-argv");
    assert_eq!(run.worktree_id, home.id);
    assert!(run.id.starts_with("term-"), "run id: {}", run.id);

    // Screen shows the prompt once the shell runs the launch line.
    assert!(
        wait_for(Duration::from_secs(20), || screen(&client, &run.id)
            .contains("argv-prompt-marker-1")),
        "screen: {:?}",
        screen(&client, &run.id)
    );
    // Tracked in the workspace list; invisible elsewhere.
    let listed = client.list_agent_runs("ws-argv").expect("list runs");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, run.id);
    assert!(client.list_agent_runs("ws-other").expect("list").is_empty());

    // Close drops the run record (terminal half closes too).
    client.close_terminal("ws-argv", &run.id).expect("close");
    assert!(client.list_agent_runs("ws-argv").expect("list").is_empty());
    assert!(client.agent_run(&run.id).is_err());
}

#[test]
fn stdin_heredoc_run_delivers_prompt() {
    let iso = Isolated::new("stdin");
    let client = iso.ensure();
    let agent = client
        .add_agent_config(
            "Cat",
            "cat",
            vec![],
            PromptTransport::Stdin,
            vec![],
            BTreeMap::new(),
        )
        .expect("add cat config")
        .id;
    let home = client
        .create_standalone_session("ws-stdin", "stdin")
        .expect("home");
    let run = client
        .run_agent(
            "ws-stdin",
            &home.id,
            &agent,
            "stdin-prompt-marker-2",
            AgentDispatch::default(),
        )
        .expect("run agent");
    assert!(
        wait_for(Duration::from_secs(20), || screen(&client, &run.id)
            .contains("stdin-prompt-marker-2")),
        "screen: {:?}",
        screen(&client, &run.id)
    );
}

#[test]
fn unknown_agent_and_home_rejected() {
    let iso = Isolated::new("rejects");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    let home = client
        .create_standalone_session("ws-rej", "rej")
        .expect("home");

    let err = client
        .run_agent(
            "ws-rej",
            &home.id,
            "no-such-agent",
            "hi",
            AgentDispatch::default(),
        )
        .expect_err("unknown agent must be rejected");
    assert!(err.contains("unknown agent"), "unexpected: {err}");

    let err = client
        .run_agent(
            "ws-rej",
            "wt-missing",
            &agent,
            "hi",
            AgentDispatch::default(),
        )
        .expect_err("unknown home must be rejected");
    assert!(err.contains("unknown worktree"), "unexpected: {err}");

    // Foreign workspace cannot launch into another workspace's home.
    let err = client
        .run_agent(
            "ws-foreign",
            &home.id,
            &agent,
            "hi",
            AgentDispatch::default(),
        )
        .expect_err("foreign workspace must be gated");
    assert!(err.contains("unknown worktree"), "unexpected: {err}");
}

#[test]
fn teardown_ends_runs() {
    let iso = Isolated::new("teardown");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    let home = client
        .create_standalone_session("ws-tear", "tear")
        .expect("home");
    let run = client
        .run_agent(
            "ws-tear",
            &home.id,
            &agent,
            "teardown-marker-3",
            AgentDispatch::default(),
        )
        .expect("run agent");
    assert!(
        wait_for(Duration::from_secs(20), || screen(&client, &run.id)
            .contains("teardown-marker-3")),
        "screen: {:?}",
        screen(&client, &run.id)
    );
    assert_eq!(client.list_agent_runs("ws-tear").expect("list").len(), 1);
    client
        .teardown_standalone_session("ws-tear", &home.id)
        .expect("teardown");
    assert!(client.list_agent_runs("ws-tear").expect("list").is_empty());
    assert!(client.agent_run(&run.id).is_err());
}

#[test]
fn overrides_swap_command_observably() {
    let iso = Isolated::new("overrides");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    let home = client
        .create_standalone_session("ws-over", "over")
        .expect("home");
    let dispatch = AgentDispatch {
        overrides: AgentOverrides {
            command: Some("printf".to_string()),
            extra_args: vec!["SWAP:%s\n".to_string()],
            model: None,
            env: BTreeMap::new(),
        },
        ..Default::default()
    };
    let run = client
        .run_agent("ws-over", &home.id, &agent, "override-probe-4", dispatch)
        .expect("run agent");
    assert!(
        wait_for(Duration::from_secs(20), || screen(&client, &run.id)
            .contains("SWAP:")),
        "swapped command output missing, screen: {:?}",
        screen(&client, &run.id)
    );
}

#[test]
fn context_tail_and_staged_attachments_reach_prompt() {
    let iso = Isolated::new("ctx");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    let home = client
        .create_standalone_session("ws-ctx", "ctx")
        .expect("home");

    // Old head (padded) + tail marker: a small budget keeps only the tail.
    let ctx_path = iso.base.join("notes.txt");
    let mut body = String::new();
    for i in 0..40 {
        body.push_str(&format!("HEAD-PAD-LINE-{i:02}\n"));
    }
    body.push_str("TAIL-MARKER-CTX-5\n");
    std::fs::write(&ctx_path, &body).unwrap();

    let attach_path = iso.base.join("evidence.txt");
    std::fs::write(&attach_path, "ATTACH-BODY-456\n").unwrap();

    let dispatch = AgentDispatch {
        context_from: vec![ctx_path.to_string_lossy().to_string()],
        context_chars: 120,
        attachments: vec![attach_path.to_string_lossy().to_string()],
        ..Default::default()
    };
    let run = client
        .run_agent("ws-ctx", &home.id, &agent, "ctx-task-5", dispatch)
        .expect("run agent");
    assert!(
        wait_for(Duration::from_secs(20), || screen(&client, &run.id)
            .contains("TAIL-MARKER-CTX-5")),
        "context tail missing, screen: {:?}",
        screen(&client, &run.id)
    );
    assert!(
        wait_for(Duration::from_secs(20), || screen(&client, &run.id)
            .contains("evidence.txt")),
        "attachment manifest missing, screen: {:?}",
        screen(&client, &run.id)
    );
    // Staged flat copy exists in the home with its body intact.
    let staged = PathBuf::from(&home.path).join("evidence.txt");
    assert_eq!(
        std::fs::read_to_string(&staged).expect("staged file"),
        "ATTACH-BODY-456\n"
    );
}

#[test]
fn readiness_reports_honest_shape() {
    let iso = Isolated::new("ready");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);

    // echo is on PATH in test environments: installed with a location detail.
    let ready = client.check_agent_readiness(&agent).expect("readiness");
    assert_eq!(ready.agent, agent);
    assert_eq!(ready.label, "Echo");
    assert!(ready.installed, "detail: {}", ready.detail);
    assert!(!ready.detail.is_empty());

    // Unknown references report uninstalled, never an RPC error.
    let missing = client
        .check_agent_readiness("no-such-agent")
        .expect("readiness");
    assert!(!missing.installed);
    assert!(!missing.detail.is_empty());

    // Preset that is not installed here is equally honest.
    let bogus = client
        .add_agent_config(
            "Bogus",
            "definitely-not-a-real-binary-xyz",
            vec![],
            PromptTransport::Argv,
            vec![],
            BTreeMap::new(),
        )
        .expect("add bogus")
        .id;
    let probe = client.check_agent_readiness(&bogus).expect("readiness");
    assert_eq!(probe.label, "Bogus");
    assert!(!probe.installed);
    assert!(!probe.detail.is_empty());
}

#[test]
fn capacity_gate_rejects_the_fifth_concurrent_run() {
    let iso = Isolated::new("capacity");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    let home = client
        .create_standalone_session("ws-cap", "cap")
        .expect("home");
    let mut runs = Vec::new();
    for _ in 0..4 {
        runs.push(
            client
                .run_agent("ws-cap", &home.id, &agent, "cap", AgentDispatch::default())
                .expect("run under capacity"),
        );
    }
    let err = client
        .run_agent("ws-cap", &home.id, &agent, "over", AgentDispatch::default())
        .expect_err("fifth run refused");
    assert!(err.contains("capacity"), "actionable refusal: {err}");
    // Closing one frees exactly one slot.
    client.close_terminal("ws-cap", &runs[0].id).expect("close");
    client
        .run_agent(
            "ws-cap",
            &home.id,
            &agent,
            "again",
            AgentDispatch::default(),
        )
        .expect("slot freed");
}

#[test]
fn propose_returns_empty_without_plan() {
    let iso = Isolated::new("propose");
    let client = iso.ensure();
    let ws = client
        .create_workspace("/tmp/dragent-propose-none")
        .expect("ws");
    assert!(client.propose_agent_runs(&ws).expect("propose").is_empty());
    assert!(
        client
            .propose_agent_runs("00000000-0000-0000-0000-000000000000")
            .expect("unknown workspace proposes nothing")
            .is_empty()
    );
}

#[test]
fn watchdog_closes_runs_whose_lease_is_gone() {
    let iso = Isolated::new("lease-watch");
    let client = iso.ensure();
    let agent = add_echo(&client, "Echo", PromptTransport::Argv);
    // Real canonical workspace so the watchdog has state to judge by.
    let ws = client
        .create_workspace("/tmp/dragent-lease-watch")
        .expect("workspace");
    let home = client
        .create_standalone_session(&ws, "lease")
        .expect("home");
    // Lease id that canonical state never issued: no authority behind it.
    let run = client
        .run_agent(
            &ws,
            &home.id,
            &agent,
            "doomed",
            AgentDispatch {
                lease_id: Some("00000000-0000-0000-0000-000000000000".to_string()),
                ..Default::default()
            },
        )
        .expect("run starts (enforcement is async)");
    // The driver tick notices within seconds and disposes the session.
    assert!(
        wait_for(Duration::from_secs(20), || client
            .agent_run(&run.id)
            .is_err()),
        "watchdog should close the authority-less run"
    );
    assert!(client.list_agent_runs(&ws).expect("list").is_empty());
}
