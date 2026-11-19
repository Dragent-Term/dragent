//! Real host/PTYS and deterministic mock provider processes; no API credentials
//! or actual agent requests. All service endpoints and records are isolated.

mod support;

use dragent_host::{
    agent_sessions::{self, AgentSession},
    HostClient,
};
use support::{helper, wait, Harness};
use uuid::Uuid;

#[cfg(unix)]
const TERMINAL_SHELL: Option<&str> = Some("/bin/bash");
#[cfg(not(unix))]
const TERMINAL_SHELL: Option<&str> = None;

/// Spawn a hook `command` exactly as the provider's own shell would: `sh -c`
/// on Unix, `cmd /C` on Windows (the production `hook_command` built by
/// `install_capture` is already a ready-to-run command line on Windows, not
/// shell syntax, so no quoting beyond what `cmd` itself needs is required).
fn run_hook_command(command: &str) -> std::process::Child {
    #[cfg(unix)]
    let mut cmd = std::process::Command::new("/bin/sh");
    #[cfg(unix)]
    cmd.args(["-c", command]);
    #[cfg(not(unix))]
    let mut cmd = std::process::Command::new("cmd");
    #[cfg(not(unix))]
    cmd.args(["/C", command]);
    cmd.stdin(std::process::Stdio::piped()).spawn().unwrap()
}

#[test]
fn exact_agent_identity_survives_disconnect_and_recovers_after_process_loss() {
    let h = Harness::new();
    let mut sessions: Vec<(Uuid, String, AgentSession)> = Vec::new();
    for provider in [
        "claude",
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
    ] {
        let (pane, terminal) = h.launch(provider);
        let record = agent_sessions::load(pane).unwrap();
        assert_eq!(
            record.terminal_id, terminal,
            "host shell must pass transport identity"
        );
        assert_eq!(record.workspace_id, h.workspace);
        h.input(
            &terminal,
            record.conversation_id.as_deref().unwrap(),
            "before-disconnect",
        );
        sessions.push((pane, terminal, record));
    }
    assert_ne!(sessions[0].2.conversation_id, sessions[1].2.conversation_id);
    // A replacement GUI client reconnects to each original PTY, not a fresh shell.
    let reconnected = HostClient::connect_existing().unwrap();
    for (pane, terminal, record) in &sessions {
        assert_eq!(
            h.restore(&reconnected, *pane, terminal).unwrap().1,
            *terminal
        );
        assert_eq!(
            agent_sessions::load(*pane).unwrap().generation,
            record.generation
        );
        h.input(
            terminal,
            record.conversation_id.as_deref().unwrap(),
            "after-disconnect",
        );
    }
    // A normal terminal also keeps its original shell transport.
    let normal = h
        .client
        .create_terminal_with_shell(
            &h.workspace,
            &h.home,
            None,
            Some(h.dir.to_str().unwrap()),
            80,
            24,
            TERMINAL_SHELL,
        )
        .unwrap();
    assert_eq!(
        h.restore(&reconnected, Uuid::new_v4(), &normal.id)
            .unwrap()
            .1,
        normal.id
    );

    for (pane, old_terminal, record) in &sessions {
        h.crash(old_terminal);
        assert!(agent_sessions::load(*pane).unwrap().active);
        let outcome = h.restore_outcome(&reconnected, *pane, old_terminal).unwrap();
        assert!(outcome.resumed, "{}", record.provider);
        let new_terminal = outcome.terminal_id;
        assert!(!new_terminal.is_empty() && new_terminal != *old_terminal);
        let recovered = wait(|| {
            agent_sessions::load(*pane).filter(|r| {
                r.generation != record.generation && r.conversation_id == record.conversation_id
            })
        });
        assert!(recovered.resumed);
        h.input(
            &new_terminal,
            record.conversation_id.as_deref().unwrap(),
            "after-crash",
        );
        // A second window restoring an old snapshot adopts the replacement;
        // it must not launch the same conversation a second time.
        assert_eq!(
            h.restore(&reconnected, *pane, old_terminal).unwrap().1,
            new_terminal
        );
    }

    // A delayed hook from the old process cannot overwrite the new generation.
    let (pane, _, old_record) = &sessions[0];
    let current = agent_sessions::load(*pane).unwrap();
    use std::io::Write;
    let mut stale = std::process::Command::new(helper())
        .args([
            agent_sessions::HOOK_ARG,
            &pane.to_string(),
            &old_record.generation.to_string(),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    stale
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"session_id\":\"wrong-old-session\"}")
        .unwrap();
    assert!(stale.wait().unwrap().success());
    assert_eq!(
        agent_sessions::load(*pane).unwrap().conversation_id,
        current.conversation_id
    );

    // Without a provider conversation ID, never guess latest or create a shell.
    let count = h.client.list_terminals(&h.workspace).unwrap().len();
    assert!(h
        .restore(&reconnected, Uuid::new_v4(), "missing-pty")
        .unwrap()
        .1
        .is_empty());
    assert_eq!(count, h.client.list_terminals(&h.workspace).unwrap().len());

    // A failed exact-ID resume is explicit unavailable, even if its shell lives.
    let (pane, old) = h.launch("claude");
    let record = agent_sessions::load(pane).unwrap();
    let plugin = h
        .dir
        .join("data/agent-sessions")
        .join(pane.to_string())
        .join(format!("claude-{}/hooks/hooks.json", record.generation));
    let hook: serde_json::Value = serde_json::from_slice(&std::fs::read(plugin).unwrap()).unwrap();
    let command = hook["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let mut child = run_hook_command(command);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"session_id\":\"missing-transcript\"}")
        .unwrap();
    assert!(child.wait().unwrap().success());
    h.crash(&old);
    let failed = h.restore(&reconnected, pane, &old).unwrap().1;
    wait(|| agent_sessions::load(pane).filter(|r| r.resume_failed()));
    // A regular terminal can still use the surviving shell; an agent pane
    // must not mistake that shell for its failed conversation.
    let ordinary = agent_sessions::restore_terminal(
        &reconnected,
        agent_sessions::RestoreTerminal {
            pane_uuid: pane,
            workspace_id: &h.workspace,
            home_id: &h.home,
            terminal_id: &failed,
            root: h.dir.to_str().unwrap(),
            cwd: h.dir.to_str().unwrap(),
            shell: TERMINAL_SHELL,
            allow_new_shell: true,
            helper: Some(&helper()),
        },
    ).unwrap();
    assert_eq!(ordinary.terminal_id, failed);
    assert!(!ordinary.resumed);
    assert!(h.restore(&reconnected, pane, &failed).unwrap().1.is_empty());
    assert!(!h.client.list_terminals(&h.workspace).unwrap().iter().any(|s| s.id == failed));

    // An unreachable host is not proof of PTY loss; no recovery is attempted.
    h.client.shutdown().unwrap();
    wait(|| HostClient::connect_existing().err());
    assert!(h
        .restore(&reconnected, sessions[0].0, &sessions[0].1)
        .is_err());
}

