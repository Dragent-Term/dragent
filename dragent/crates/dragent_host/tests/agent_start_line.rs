//! Agents waiting for memory, and agents paused over it, end to end: a real
//! host and terminal service, real shells and launch monitors, and a stand-in
//! agent. Memory pressure is read from a file the test writes.
#![cfg(unix)]

use dragent_host::protocol::{AgentResources, AgentRunState};
use dragent_host::{agent_sessions, AgentDispatch, HostClient};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use uuid::Uuid;

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

/// Says it started, repeats each line, and takes memory on request.
const AGENT: &str = r#"#!/usr/bin/env python3
import sys
print("STARTED", flush=True)
held = []
for line in sys.stdin:
    line = line.strip()
    if line.startswith("grow "):
        held.append(bytearray(b"x") * (int(line[5:]) << 20))
    print("HEARD<" + line + ">", flush=True)
"#;

static LOCK: Mutex<()> = Mutex::new(());

struct Line {
    _lock: MutexGuard<'static, ()>,
    services: dragent_host::testing::ServiceGuard,
    dir: PathBuf,
    client: HostClient,
    workspace: String,
    home: String,
}

impl Line {
    fn new(pressure: &str) -> Self {
        let lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let id = Uuid::new_v4().simple().to_string();
        let dir = std::env::temp_dir().join(format!("dsl-{}", &id[..8]));
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::create_dir_all(dir.join("work")).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(dir.join("pressure"), pressure).unwrap();
        // SAFETY: tests in this executable serialize on `LOCK`.
        unsafe {
            for (name, path) in [
                ("DRAGENT_HOME", dir.clone()),
                ("DRAGENT_DEV_HOME", dir.clone()),
                (dragent_host::ENDPOINT_FILE_ENV, dir.join("host.json")),
                ("DRAGENT_WORKSPACE_DB", dir.join("db.sqlite")),
                ("HOME", dir.join("home")),
                ("ZDOTDIR", dir.join("home")),
                ("XDG_CONFIG_HOME", dir.join("home/.config")),
                (
                    dragent_host::agent_sleep::TEST_PRESSURE_ENV,
                    dir.join("pressure"),
                ),
            ] {
                std::env::set_var(name, path);
            }
        }
        let agent = dir.join("claude");
        std::fs::write(&agent, AGENT).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let services = dragent_host::testing::ServiceGuard::new();
        let client = HostClient::ensure_host_with(&helper()).unwrap();
        let root = dir.join("work").to_string_lossy().into_owned();
        let workspace = client.create_local_workspace(&root).unwrap();
        let home = client
            .ensure_terminal_home(&workspace, &root, &root)
            .unwrap()
            .id;
        Self {
            _lock: lock,
            services,
            dir,
            client,
            workspace,
            home,
        }
    }

    fn pressure(&self, pressure: &str) {
        std::fs::write(self.dir.join("pressure"), pressure).unwrap();
    }

    /// Start the stand-in as a tab does: typed into its terminal, monitored.
    fn launch(&self) -> Agent {
        let pane = Uuid::new_v4();
        let command = format!("'{}'", self.dir.join("claude").display());
        let launch = agent_sessions::wrap_command_with_helper(pane, "sh", &command, &helper());
        let terminal = self
            .client
            .create_terminal_with_shell(
                &self.workspace,
                &self.home,
                Some(&launch),
                None,
                160,
                24,
                Some("/bin/bash"),
            )
            .unwrap()
            .id;
        Agent { pane, terminal }
    }

    fn output(&self, terminal: &str) -> String {
        self.client
            .read_terminal(&self.workspace, terminal, None)
            .map(|read| read.0)
            .unwrap_or_default()
    }

    fn started(&self, agent: &Agent) -> bool {
        self.output(&agent.terminal).contains("STARTED")
    }

    fn type_in(&self, agent: &Agent, bytes: &[u8]) {
        self.client
            .send_terminal_input(&self.workspace, &agent.terminal, bytes)
            .unwrap();
    }

    fn heard(&self, agent: &Agent, text: &str) -> bool {
        self.output(&agent.terminal)
            .contains(&format!("HEARD<{text}>"))
    }

    fn resources(&self) -> AgentResources {
        self.client.agent_resources().unwrap_or_default()
    }

    fn waiting(&self) -> Vec<Uuid> {
        let waiting = self.resources().waiting;
        waiting.iter().map(|w| w.id.parse().unwrap()).collect()
    }

    fn paused(&self) -> Vec<Uuid> {
        let paused = self.resources().paused;
        paused.iter().map(|p| p.pane.parse().unwrap()).collect()
    }

    /// Kill the host and start another; the terminal service stays.
    fn replace_host(&mut self) {
        let pid = self.client.endpoint().pid;
        kill_host(&self.client);
        self.client = HostClient::ensure_host_with(&helper()).unwrap();
        assert_ne!(self.client.endpoint().pid, pid);
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        self.services.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Kill the host this test started, until this process knows it's gone.
fn kill_host(client: &HostClient) {
    // SAFETY: the pid of the host this test started.
    unsafe { libc::kill(client.endpoint().pid as i32, libc::SIGKILL) };
    wait_for("the host to be gone", 5, || !client.ping());
}

struct Agent {
    pane: Uuid,
    terminal: String,
}

#[track_caller]
fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn with_memory_to_spare_any_number_start_at_once() {
    let line = Line::new("elevated");
    let agents: Vec<Agent> = (0..6).map(|_| line.launch()).collect();
    for agent in &agents {
        wait_for("start", 20, || line.started(agent));
    }
    assert!(line.waiting().is_empty());
}

#[test]
fn at_critical_memory_starts_wait_and_then_go_in_order_one_at_a_time() {
    let line = Line::new("critical");
    let mut agents = Vec::new();
    for place in 1..=3 {
        agents.push(line.launch());
        wait_for("a place in the line", 20, || line.waiting().len() == place);
        let pane = agents[place - 1].pane;
        wait_for("its marker", 5, || agent_sessions::is_waiting(pane));
    }
    let order: Vec<Uuid> = agents.iter().map(|a| a.pane).collect();
    assert_eq!(line.waiting(), order);
    let says = line.output(&agents[0].terminal);
    assert!(
        says.contains("Waiting for memory") && says.contains("Press S"),
        "{says}"
    );
    assert!(agents.iter().all(|agent| !line.started(agent)));
    // Waiting isn't running: not listed as such once memory was measured.
    std::thread::sleep(Duration::from_secs(12));
    assert_eq!(line.waiting(), order);
    assert_eq!(line.resources().running, []);
    // While it waits its record says it hasn't begun a conversation.
    let record = agent_sessions::load(agents[0].pane).unwrap();
    assert!(record.active && !record.used && record.terminal_id == agents[0].terminal);

    line.pressure("normal");
    let mut went = Vec::new();
    wait_for("all three", 60, || {
        for agent in &agents {
            if line.started(agent) && !went.iter().any(|(pane, _)| *pane == agent.pane) {
                went.push((agent.pane, Instant::now()));
            }
        }
        went.len() == 3
    });
    let went_in: Vec<Uuid> = went.iter().map(|(pane, _)| *pane).collect();
    assert_eq!(went_in, order);
    for pair in went.windows(2) {
        let gap = pair[1].1 - pair[0].1;
        assert!(gap >= Duration::from_secs(4), "{gap:?} apart");
    }
    assert!(line.waiting().is_empty());
    // Each one started once, and has its terminal to itself.
    for agent in &agents {
        assert_eq!(line.output(&agent.terminal).matches("STARTED").count(), 1);
        line.type_in(agent, b"hello\r");
        wait_for("the agent hears", 10, || line.heard(agent, "hello"));
        assert!(!agent_sessions::is_waiting(agent.pane));
    }
}

#[test]
fn a_wait_ends_when_the_person_says_so_and_for_nothing_else() {
    let line = Line::new("critical");
    let first = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 1);
    let second = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 2);
    let third = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 3);

    // A message typed in by something else isn't an answer.
    line.type_in(&first, b"\x1b[200~start the tests\x1b[201~");
    std::thread::sleep(Duration::from_millis(300));
    line.type_in(&first, b"\r");
    std::thread::sleep(Duration::from_secs(5));
    assert!(!line.started(&first));

    // Start anyway, from the middle of the line.
    line.client.start_anyway(&second.pane.to_string()).unwrap();
    wait_for("start anyway", 10, || line.started(&second));
    assert_eq!(line.waiting(), [first.pane, third.pane]);

    // Called off: back to a shell, nothing started.
    line.type_in(&third, b"\x03");
    wait_for("cancelled", 10, || line.waiting() == [first.pane]);
    line.type_in(&third, b"echo rc=$?\r");
    wait_for("its shell", 10, || {
        line.output(&third.terminal).contains("rc=130")
    });
    assert!(!line.started(&third));
    assert!(!agent_sessions::load(third.pane).unwrap().active);

    // S in its own terminal.
    line.type_in(&first, b"s");
    wait_for("S", 10, || line.started(&first));
    assert!(line.waiting().is_empty());
    line.type_in(&first, b"typed\r");
    wait_for("its terminal back", 10, || line.heard(&first, "typed"));
    // What was typed while it waited didn't reach it.
    assert!(!line.heard(&first, "start the tests"));
}

#[test]
fn a_wait_outlives_the_host_and_keeps_its_place() {
    let mut line = Line::new("critical");
    let first = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 1);
    let second = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 2);
    line.replace_host();
    wait_for("both again", 15, || {
        line.waiting() == [first.pane, second.pane]
    });
    line.replace_host();
    line.pressure("normal");
    wait_for("the first", 30, || line.started(&first));
    assert!(!line.started(&second), "one at a time, in the old order");
    wait_for("the second", 30, || line.started(&second));
    for agent in [&first, &second] {
        assert_eq!(line.output(&agent.terminal).matches("STARTED").count(), 1);
    }
}

#[test]
fn without_a_host_a_waiting_agent_still_starts() {
    let line = Line::new("critical");
    let agent = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 1);
    kill_host(&line.client);
    line.pressure("normal");
    wait_for("its monitor decides", 20, || {
        agent_sessions::load(agent.pane)
            .is_some_and(|record| record.used || record.conversation_id.is_some())
            || !agent_sessions::is_waiting(agent.pane)
    });
    let client = HostClient::ensure_host_with(&helper()).unwrap();
    wait_for("started", 20, || {
        client
            .read_terminal(&line.workspace, &agent.terminal, None)
            .is_ok_and(|read| read.0.contains("STARTED"))
    });
}

#[test]
fn closing_a_waiting_agents_terminal_takes_it_out_of_the_line() {
    let line = Line::new("critical");
    let agent = line.launch();
    wait_for("waiting", 20, || line.waiting().len() == 1);
    line.client
        .close_terminal(&line.workspace, &agent.terminal)
        .unwrap();
    assert!(line.waiting().is_empty(), "gone from the line at once");
    std::thread::sleep(Duration::from_secs(3));
    assert!(line.waiting().is_empty(), "and stays gone");
    assert!(!agent_sessions::is_waiting(agent.pane));
}

#[test]
fn a_set_cap_counts_running_agents_only() {
    let line = Line::new("normal");
    std::fs::write(line.dir.join("resource-limits.json"), r#"{"max_agents":1}"#).unwrap();
    let first = line.launch();
    wait_for("under the cap", 20, || line.started(&first));
    let second = line.launch();
    wait_for("at the cap", 20, || line.waiting() == [second.pane]);
    // It waits, and so holds no place of its own: the cap is still one.
    std::thread::sleep(Duration::from_secs(6));
    assert!(!line.started(&second));
    line.client
        .close_terminal(&line.workspace, &first.terminal)
        .unwrap();
    wait_for("a place free", 30, || line.started(&second));
    // Cap removed: no more waiting.
    let third = line.launch();
    wait_for("at the cap", 20, || line.waiting() == [third.pane]);
    std::fs::remove_file(line.dir.join("resource-limits.json")).unwrap();
    wait_for("no cap", 20, || line.started(&third));
}

#[test]
fn a_host_run_opens_its_terminal_and_waits_there() {
    let line = Line::new("critical");
    let agent = line
        .client
        .add_agent_config(
            "Echo",
            "echo",
            vec![],
            dragent_host::PromptTransport::Argv,
            vec![],
            Default::default(),
        )
        .unwrap()
        .id;
    let home = line
        .client
        .create_standalone_session("ws", "ws")
        .unwrap()
        .id;
    let run = |prompt: &str| {
        line.client
            .run_agent("ws", &home, &agent, prompt, AgentDispatch::default())
            .unwrap()
    };
    let first = run("it's quoted");
    let state = |id: &str| line.client.agent_run(id).unwrap().state;
    wait_for("waiting", 20, || state(&first.id) == AgentRunState::Waiting);
    let second = run("two");
    wait_for("waiting", 20, || {
        state(&second.id) == AgentRunState::Waiting
    });
    line.client.start_anyway(&second.id).unwrap();
    wait_for("start anyway", 10, || {
        state(&second.id) != AgentRunState::Waiting
    });
    line.pressure("normal");
    wait_for("the rest", 30, || {
        line.client
            .agent_run(&first.id)
            .unwrap()
            .output
            .contains("\nit's quoted")
    });
}

#[test]
fn a_runaway_is_paused_resumed_by_a_later_host_and_can_be_closed() {
    let mut line = Line::new("normal");
    let agent = line.launch();
    wait_for("started", 20, || line.started(&agent));
    // No limit set, and memory to spare: nothing is paused.
    line.type_in(&agent, b"grow 300\r");
    wait_for("grown", 20, || line.heard(&agent, "grow 300"));
    wait_for("measured", 30, || {
        line.resources().running.iter().any(|a| a.bytes > 300 << 20)
    });
    assert!(line.paused().is_empty());
    std::fs::write(
        line.dir.join("resource-limits.json"),
        format!(r#"{{"agent_memory_limit_bytes":{}}}"#, 256u64 << 20),
    )
    .unwrap();
    wait_for("paused", 30, || line.paused() == [agent.pane]);
    line.type_in(&agent, b"are you there\r");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!line.heard(&agent, "are you there"));
    // Its shell still thinks it runs: no prompt, nothing suspended.
    let screen = line.output(&agent.terminal).to_lowercase();
    assert!(
        !screen.contains("stopped") && !screen.contains("suspended"),
        "{screen}"
    );

    line.replace_host();
    assert_eq!(line.paused(), [agent.pane], "known to the next host");
    line.client.resume_agent(&agent.terminal).unwrap();
    // It hears what was typed meanwhile, from its own terminal.
    wait_for("heard", 10, || line.heard(&agent, "are you there"));
    line.type_in(&agent, b"and now\r");
    wait_for("still its terminal", 10, || line.heard(&agent, "and now"));
    // The person let it have that much: it isn't paused again for it.
    std::thread::sleep(Duration::from_secs(25));
    assert!(line.paused().is_empty());
    // Until it outgrows that.
    line.type_in(&agent, b"grow 300\r");
    wait_for("paused again", 40, || line.paused() == [agent.pane]);
    line.client
        .close_terminal(&line.workspace, &agent.terminal)
        .unwrap();
    wait_for("closed while paused", 30, || line.paused().is_empty());
    let panes = dragent_host::agent_memory::wrappers(&dragent_host::agent_memory::processes());
    assert!(!panes.contains_key(&agent.pane), "nothing of it is left");
}

#[test]
fn a_builds_terminal_is_told_about_the_shared_slots() {
    let line = Line::new("normal");
    let terminal = line
        .client
        .create_terminal_with_shell(
            &line.workspace,
            &line.home,
            Some("echo \"[$CARGO_MAKEFLAGS]\""),
            None,
            300,
            24,
            Some("/bin/bash"),
        )
        .unwrap();
    let auth = dragent_host::build_slots::auth(&line.dir);
    if dragent_host::build_slots::make_support() == dragent_host::build_slots::MakeSupport::Neither
    {
        return;
    }
    let expected = format!(
        "[--jobserver-auth={}]",
        auth.expect("the service opened a pool")
    );
    wait_for("the variable", 20, || {
        line.output(&terminal.id).contains(&expected)
    });
}
