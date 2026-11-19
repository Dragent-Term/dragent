//! `cargo run -p dragent` builds no `dragent-host` helper. Without it, a
//! restored terminal must still reattach and run commands: only a new shell
//! could connect when restoring required the helper.

mod support;

use dragent_host::{
    agent_sessions::{self, RestoreTerminal, Restored},
    HostClient,
};
use support::{wait, Harness};
use uuid::Uuid;

#[cfg(unix)]
const SHELL: &str = "/bin/bash";
#[cfg(not(unix))]
const SHELL: &str = "powershell.exe";

/// Only prints the expected text if the shell ran it, not by echoing the input.
#[cfg(unix)]
const COMMAND: &str = "echo restored-$((40+2))\n";
#[cfg(not(unix))]
const COMMAND: &str = "'restored-' + (40+2)\r";

fn restore(
    h: &Harness,
    client: &HostClient,
    terminal: &str,
    root: &str,
    allow_new_shell: bool,
) -> Result<Restored, String> {
    agent_sessions::restore_terminal(
        client,
        RestoreTerminal {
            pane_uuid: Uuid::new_v4(),
            workspace_id: &h.workspace,
            home_id: &h.home,
            terminal_id: terminal,
            root,
            cwd: root,
            shell: Some(SHELL),
            allow_new_shell,
            helper: None,
        },
    )
}

/// The checkpoint the app applies before it binds the tab's input.
fn wait_for_prompt(client: &HostClient, h: &Harness, terminal: &str) {
    wait(|| {
        let mut ready = false;
        let _ = client.stream_terminal_operations(&h.workspace, terminal, 0, |chunk| {
            ready = chunk.resume.is_some_and(|resume| resume.shell_ready);
            false
        });
        ready.then_some(())
    });
}

fn assert_runs_commands(client: &HostClient, h: &Harness, terminal: &str) {
    wait_for_prompt(client, h, terminal);
    client
        .send_terminal_input(&h.workspace, terminal, COMMAND.as_bytes())
        .unwrap();
    wait(|| {
        client
            .read_terminal(&h.workspace, terminal, None)
            .unwrap()
            .0
            .contains("restored-42")
            .then_some(())
    });
}

#[test]
fn restored_terminals_work_without_the_session_helper() {
    let h = Harness::new();
    let root = h.dir.to_str().unwrap();
    let running = h
        .client
        .create_terminal_with_shell(&h.workspace, &h.home, None, Some(root), 80, 24, Some(SHELL))
        .unwrap();
    wait_for_prompt(&h.client, &h, &running.id);

    // A relaunched app reconnects to the shell that kept running.
    let relaunched = HostClient::connect_existing().unwrap();
    let restored = restore(&h, &relaunched, &running.id, root, true).unwrap();
    assert_eq!(restored.terminal_id, running.id);
    assert!(!restored.resumed);
    assert_runs_commands(&relaunched, &h, &restored.terminal_id);

    // A plain terminal whose shell is gone reopens a usable one.
    let reopened = restore(&h, &relaunched, "lost-terminal", root, true).unwrap();
    assert!(!reopened.terminal_id.is_empty() && reopened.terminal_id != running.id);
    assert_runs_commands(&relaunched, &h, &reopened.terminal_id);

    // An agent pane with nothing to resume is reported, not failed.
    let agent = restore(&h, &relaunched, "lost-agent", root, false).unwrap();
    assert!(agent.terminal_id.is_empty());

    // The app stops retrying on exactly this error.
    let missing = h.dir.join("deleted-folder");
    let error = restore(&h, &relaunched, "lost-terminal", missing.to_str().unwrap(), true)
        .unwrap_err();
    assert!(error.starts_with("project root unavailable"), "{error}");
}
