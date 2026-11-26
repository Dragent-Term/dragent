//! Machine reboot: the host, the PTY daemon, every shell, launch monitor and
//! agent process is gone at once. Only disk survives. A fresh host must then
//! start new agent processes that resume each pane's exact conversation.

mod support;

use dragent_host::{HostClient, agent_sessions};
use support::{Harness, helper, wait};
use uuid::Uuid;

#[cfg(unix)]
const SHELL_NAME: &str = "sh";
#[cfg(not(unix))]
const SHELL_NAME: &str = "powershell";

#[cfg(unix)]
const TERMINAL_SHELL: Option<&str> = Some("/bin/bash");
#[cfg(not(unix))]
const TERMINAL_SHELL: Option<&str> = None;

/// Every agent with an exact-identity recovery mechanism.
const RESUMABLE: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "mimo",
    "gemini",
    "qwen",
    "copilot",
    "codebuddy",
    "kimi",
    "cursor-agent",
    "droid",
    "muse",
];

#[track_caller]
fn ready_pids(h: &Harness, terminal: &str) -> (i32, i32) {
    let text = wait(|| {
        let t = h.output(terminal);
        t.contains("READY|").then_some(t)
    });
    let line = text
        .lines()
        .find_map(|line| line.find("READY|").map(|i| &line[i..]))
        .unwrap();
    let parts: Vec<_> = line.trim().split('|').collect();
    (parts[2].parse().unwrap(), parts[3].parse().unwrap())
}

#[cfg(unix)]
fn kill(pid: i32, signal: i32) {
    assert!(pid > 1 && pid != std::process::id() as i32);
    unsafe {
        libc::kill(pid, signal);
    }
}

/// Hard-kill, portable: `SIGKILL` on Unix, a forceful `taskkill /F` on
/// Windows. Windows has no notion of a signal an arbitrary external process
/// can catch and exit cleanly from — the production monitor's
/// graceful-shutdown path instead watches for a console close/shutdown
/// control event, which only a real OS shutdown or console-window close can
/// raise (`GenerateConsoleCtrlEvent` only supports `CTRL_C`/`CTRL_BREAK`), so
/// that specific path has no automated-test equivalent on Windows.
fn kill_hard(pid: i32) {
    assert!(pid > 1 && pid != std::process::id() as i32);
    #[cfg(unix)]
    kill(pid, libc::SIGKILL);
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
    }
}

fn alive(pid: i32) -> bool {
    #[cfg(unix)]
    unsafe {
        // The host is this test's child: reap it, since a zombie still
        // answers signal 0.
        libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG);
        return libc::kill(pid, 0) == 0;
    }
    #[cfg(not(unix))]
    {
        let id = sysinfo::Pid::from(pid as usize);
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[id]), true);
        sys.process(id).is_some()
    }
}

/// Launch a shell command as the host would, in a new terminal of `h`.
fn launch_command(h: &Harness, pane: Uuid, command: &str) -> String {
    let launch = agent_sessions::wrap_command_with_helper(pane, SHELL_NAME, command, &helper());
    h.client
        .create_terminal_with_shell(
            &h.workspace,
            &h.home,
            Some(&launch),
            Some(h.dir.to_str().unwrap()),
            80,
            24,
            TERMINAL_SHELL,
        )
        .unwrap()
        .id
}

#[track_caller]
fn used(pane: Uuid) -> agent_sessions::AgentSession {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let record = agent_sessions::load(pane);
        if let Some(record) = record.clone().filter(|r| r.used) {
            return record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the submitted prompt was not recorded: {record:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[test]
fn every_resumable_agent_returns_to_its_pane_after_total_process_loss() {
    let mut h = Harness::new();
    let mut panes = Vec::new();
    for provider in RESUMABLE {
        let (pane, terminal) = h.launch(provider);
        let record = agent_sessions::load(pane).unwrap();
        assert!(!record.used, "{provider} counted as used before any prompt");
        let id = record.conversation_id.clone().unwrap();
        h.input(&terminal, &id, "before-reboot");
        // The host saw a line submitted to this agent.
        used(pane);
        panes.push((pane, terminal, record));
    }
    // Two panes of one agent never share a conversation.
    let (twin, twin_terminal) = h.launch("gemini");
    let twin_record = agent_sessions::load(twin).unwrap();
    assert_ne!(twin_record.conversation_id, panes[4].2.conversation_id);
    h.input(
        &twin_terminal,
        twin_record.conversation_id.as_deref().unwrap(),
        "before-reboot",
    );
    panes.push((twin, twin_terminal, twin_record));

    // Opened but never prompted: nothing would be lost by starting over.
    let (idle, idle_terminal) = h.launch("gemini");
    let idle_record = agent_sessions::load(idle).unwrap();
    assert!(!idle_record.used);

    // The agent exits normally. An ordinary shell must not restart it, but
    // restoring an agent pane explicitly reopens the saved conversation.
    let (quit, quit_terminal) = h.launch("qwen");
    let quit_id = agent_sessions::load(quit).unwrap().conversation_id.unwrap();
    h.input(&quit_terminal, &quit_id, "hello");
    h.client
        .send_terminal_input(&h.workspace, &quit_terminal, b"quit\n")
        .unwrap();
    let ended = wait(|| agent_sessions::load(quit).filter(|r| r.last_exit_code.is_some()));
    assert!(!ended.active && !ended.nothing_to_resume());

    // An agent without exact identity is launched untracked: never guessed.
    let untracked = Uuid::new_v4();
    assert_eq!(
        agent_sessions::wrap_command_with_helper(untracked, SHELL_NAME, "crush", &helper()),
        "crush"
    );

    // Orderly shutdown signals the agent and its monitor together; the agent
    // exits cleanly. That is not the user ending the conversation. Unix only:
    // Windows has no equivalent of sending an arbitrary external process a
    // signal it can catch and exit cleanly from (see `kill_hard` above), so
    // panes[0] instead joins the power-loss group below on Windows.
    #[cfg(unix)]
    {
        let (pane, terminal, record) = &panes[0];
        let (agent, monitor) = ready_pids(&h, terminal);
        kill(agent, libc::SIGTERM);
        kill(monitor, libc::SIGTERM);
        let shut = wait(|| agent_sessions::load(*pane).filter(|r| r.last_exit_code.is_some()));
        assert_eq!(shut.last_exit_code, Some(0));
        assert!(shut.active && !shut.resume_failed());
        assert_eq!(shut.generation, record.generation);
        wait(|| (!alive(monitor)).then_some(()));
    }

    // Power loss: nothing records anything.
    let mut gone = Vec::new();
    #[cfg(unix)]
    let to_power_cycle: &[(Uuid, String, agent_sessions::AgentSession)] = &panes[1..];
    #[cfg(not(unix))]
    let to_power_cycle: &[(Uuid, String, agent_sessions::AgentSession)] = &panes;
    for terminal in to_power_cycle
        .iter()
        .map(|(_, terminal, _)| terminal)
        .chain([&idle_terminal])
    {
        let (agent, monitor) = ready_pids(&h, terminal);
        kill_hard(monitor);
        kill_hard(agent);
        gone.extend([agent, monitor]);
    }
    let host = dragent_host::discovery::read_endpoint().unwrap().pid as i32;
    // Shells live in the PTY daemon; unless it dies too, this is only an app
    // crash (which reattaches instead of resuming).
    let daemon =
        dragent_host::pty_supervisor::read_manifest(&dragent_host::pty_supervisor::org_dir())
            .expect("PTY daemon manifest")
            .pid as i32;
    kill_hard(host);
    kill_hard(daemon);
    gone.extend([host, daemon]);
    for pid in gone {
        wait(|| (!alive(pid)).then_some(()));
    }
    assert!(HostClient::connect_existing().is_err());

    // Boot: a fresh host from a clean process environment.
    h.client = HostClient::ensure_host_with(&helper()).unwrap();
    for (pane, old_terminal, record) in &panes {
        let restored = h.restore_outcome(&h.client, *pane, old_terminal).unwrap();
        assert!(restored.resumed, "{} was not resumed", record.provider);
        assert!(!restored.terminal_id.is_empty() && restored.terminal_id != *old_terminal);
        let resumed = wait(|| {
            agent_sessions::load(*pane)
                .filter(|r| r.generation != record.generation && r.conversation_id.is_some())
        });
        assert_eq!(
            resumed.conversation_id, record.conversation_id,
            "{} resumed a different conversation",
            record.provider
        );
        assert!(resumed.resumed && resumed.active && resumed.used);
        h.input(
            &restored.terminal_id,
            record.conversation_id.as_deref().unwrap(),
            "after-reboot",
        );
        // A second window restoring the same snapshot adopts this process.
        assert_eq!(
            h.restore(&h.client, *pane, old_terminal).unwrap().1,
            restored.terminal_id
        );
    }
    // The minted conversation was created once, at first launch only.
    let minted = std::fs::read_to_string(h.dir.join("minted")).unwrap();
    assert_eq!(minted.lines().count(), 1);

    // A regular terminal gets a shell, without reviving its ended agent.
    let shell = agent_sessions::restore_terminal(
        &h.client,
        agent_sessions::RestoreTerminal {
            pane_uuid: quit,
            workspace_id: &h.workspace,
            home_id: &h.home,
            terminal_id: &quit_terminal,
            root: h.dir.to_str().unwrap(),
            cwd: h.dir.to_str().unwrap(),
            shell: TERMINAL_SHELL,
            allow_new_shell: true,
            helper: Some(&helper()),
        },
    ).unwrap();
    assert!(!shell.resumed && !shell.terminal_id.is_empty());
    assert!(!agent_sessions::load(quit).unwrap().active);
    h.client.close_terminal(&h.workspace, &shell.terminal_id).unwrap();

    // Reopening its agent pane deliberately resumes the exact conversation.
    let reopened = h.restore_outcome(&h.client, quit, &quit_terminal).unwrap();
    assert!(reopened.resumed);
    h.input(&reopened.terminal_id, &quit_id, "after-reopen");
    assert_eq!(agent_sessions::load(quit).unwrap().conversation_id.as_deref(), Some(quit_id.as_str()));

    // The idle pane's resume fails (the agent never stored it). Nothing was
    // said, so it starts over under the same id instead of reporting failure.
    let attempt = h.restore_outcome(&h.client, idle, &idle_terminal).unwrap();
    assert!(attempt.resumed);
    let failed = wait(|| agent_sessions::load(idle).filter(|r| r.resume_failed()));
    assert!(failed.nothing_to_resume());
    let again = agent_sessions::start_over_command(&failed, SHELL_NAME).unwrap();
    let fresh_terminal = launch_command(&h, idle, &again);
    let fresh = wait(|| {
        agent_sessions::load(idle).filter(|r| r.active && r.generation != failed.generation)
    });
    assert_eq!(fresh.conversation_id, idle_record.conversation_id);
    assert!(!fresh.resumed && !fresh.nothing_to_resume());
    h.input(
        &fresh_terminal,
        idle_record.conversation_id.as_deref().unwrap(),
        "first-prompt",
    );
    used(idle);

    // A pane with no recorded agent is not given a guessed conversation.
    let count = h.client.list_terminals(&h.workspace).unwrap().len();
    assert!(
        h.restore(&h.client, untracked, "lost-pty")
            .unwrap()
            .1
            .is_empty()
    );
    assert_eq!(count, h.client.list_terminals(&h.workspace).unwrap().len());
}
