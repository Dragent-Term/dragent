//! The Codex and Claude Code drivers against scripted agents speaking the
//! message shapes recorded from the real CLIs, and (ignored) against the real
//! CLIs on this machine.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::tests::scripted;
use crate::*;

fn drain(events: &std::sync::mpsc::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(event) = events.recv_timeout(Duration::from_millis(200)) {
        out.push(event);
    }
    out
}

fn said(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Update { update: Update::AgentText { text, .. }, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// ── Codex ─────────────────────────────────────────────────────────────────

/// A Codex app-server that knows one thread and runs every turn the same way:
/// a command, then a streamed reply. A turn whose text says "wait" runs until
/// interrupted.
fn fake_codex() -> Codex {
    let (reader, writer) = scripted(|m| {
        let id = m["id"].clone();
        let answer = |result: Value| json!({"id": id, "result": result});
        match m["method"].as_str() {
            Some("initialize") => vec![answer(json!({"userAgent": "codex/0.169.0"}))],
            Some("model/list") => vec![answer(json!({"data": [
                {"id": "gpt-a", "model": "gpt-a", "displayName": "GPT A", "description": "Fast", "hidden": false, "isDefault": true,
                 "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": ""}, {"reasoningEffort": "high", "description": ""}],
                 "defaultReasoningEffort": "low"},
                {"id": "gpt-b", "model": "gpt-b", "displayName": "GPT B", "description": "", "hidden": false, "isDefault": false,
                 "supportedReasoningEfforts": [{"reasoningEffort": "medium", "description": ""}], "defaultReasoningEffort": "medium"},
                {"id": "secret", "displayName": "Hidden", "hidden": true, "supportedReasoningEfforts": []}
            ]}))],
            Some("thread/start") => vec![answer(json!({"thread": {"id": "t1"}, "model": "gpt-a", "reasoningEffort": "low"}))],
            Some("turn/start") => {
                assert_eq!(m["params"]["threadId"], "t1");
                let text = m["params"]["input"][0]["text"].as_str().unwrap_or_default().to_string();
                let started = json!({"method": "turn/started", "params": {"threadId": "t1", "turn": {"id": "u1", "status": "inProgress"}}});
                if text.contains("wait") {
                    return vec![answer(json!({"turn": {"id": "u1", "status": "inProgress"}})), started];
                }
                vec![
                    answer(json!({"turn": {"id": "u1", "status": "inProgress"}})),
                    started,
                    json!({"method": "item/started", "params": {"threadId": "t1", "turnId": "u1", "item": {"type": "commandExecution", "id": "c1", "command": "/bin/zsh -lc ls", "status": "inProgress"}}}),
                    json!({"method": "item/completed", "params": {"threadId": "t1", "turnId": "u1", "item": {"type": "commandExecution", "id": "c1", "command": "/bin/zsh -lc ls", "status": "completed", "aggregatedOutput": "a.txt"}}}),
                    json!({"method": "item/started", "params": {"threadId": "t1", "turnId": "u1", "item": {"type": "fileChange", "id": "f1", "status": "inProgress", "changes": [{"path": "/p/a.rs", "kind": {"type": "update"}, "diff": "-old\n+new\n+more\n"}]}}}),
                    json!({"method": "item/completed", "params": {"threadId": "t1", "turnId": "u1", "item": {"type": "fileChange", "id": "f1", "status": "completed", "changes": [{"path": "/p/a.rs", "kind": {"type": "update"}, "diff": "-old\n+new\n+more\n"}]}}}),
                    json!({"method": "item/agentMessage/delta", "params": {"threadId": "t1", "turnId": "u1", "itemId": "m1", "delta": "hel"}}),
                    json!({"method": "item/agentMessage/delta", "params": {"threadId": "t1", "turnId": "u1", "itemId": "m1", "delta": "lo"}}),
                    json!({"method": "item/completed", "params": {"threadId": "t1", "turnId": "u1", "item": {"type": "agentMessage", "id": "m1", "text": "hello"}}}),
                    json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "u1", "status": "completed"}}}),
                ]
            }
            Some("turn/interrupt") => vec![
                answer(json!({})),
                json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "u1", "status": "interrupted"}}}),
            ],
            _ => vec![],
        }
    });
    Codex::over(reader, writer)
}

#[test]
fn codex_lists_visible_models_with_their_efforts() {
    let codex = fake_codex();
    let session = codex.open(std::path::Path::new("/tmp"), None).unwrap();
    assert_eq!(session.id, "t1");
    let model = session.options.model.unwrap();
    assert_eq!(model.choices.len(), 2, "hidden models are left out");
    assert_eq!(model.current_name(), "GPT A");
    let effort = session.options.effort.unwrap();
    assert_eq!(effort.current, "low");
    assert_eq!(effort.choices.len(), 2);
}

#[test]
fn codex_turn_streams_steps_and_reply_then_ends() {
    let codex = fake_codex();
    let events = codex.take_events().unwrap();
    let session = codex.open(std::path::Path::new("/tmp"), None).unwrap();
    assert_eq!(codex.prompt(&session.id, "list files").unwrap(), StopReason::EndTurn);
    let seen = drain(&events);
    assert_eq!(said(&seen), "hello", "streamed text is not repeated by the completed message");
    let command = seen.iter().find_map(|e| match e {
        Event::Update { update: Update::ToolCall(call), .. } if call.kind == "execute" => Some(call.clone()),
        _ => None,
    });
    assert_eq!(command.unwrap().title, "ls");
    let patch = seen.iter().find_map(|e| match e {
        Event::Update { update: Update::ToolPatch(p), .. } if p.id == "f1" => p.content.clone(),
        _ => None,
    });
    assert!(matches!(&patch.unwrap()[0], ToolContent::Patch { path, .. } if path == "/p/a.rs"));
}

#[test]
fn codex_stop_interrupts_the_running_turn() {
    let codex = Arc::new(fake_codex());
    let session = codex.open(std::path::Path::new("/tmp"), None).unwrap();
    let turn = {
        let codex = codex.clone();
        let id = session.id.clone();
        std::thread::spawn(move || codex.prompt(&id, "wait for me"))
    };
    std::thread::sleep(Duration::from_millis(200));
    codex.cancel(&session.id).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::Cancelled);
}

#[test]
fn codex_choosing_a_model_resets_an_effort_it_lacks() {
    let codex = fake_codex();
    codex.open(std::path::Path::new("/tmp"), None).unwrap();
    let options = codex.set_option("t1", &SetVia::Model, "gpt-b").unwrap().unwrap();
    assert_eq!(options.model.unwrap().current, "gpt-b");
    assert_eq!(options.effort.unwrap().current, "medium", "low isn't offered by gpt-b: its default instead");
}

#[test]
fn codex_approvals_reach_the_app_and_are_answered() {
    let answered = Arc::new(Mutex::new(None::<Value>));
    let (reader, writer) = scripted({
        let answered = answered.clone();
        move |m| {
            let id = m["id"].clone();
            match m["method"].as_str() {
                Some("initialize") => vec![json!({"id": id, "result": {}})],
                Some("model/list") => vec![json!({"id": id, "result": {"data": []}})],
                Some("thread/start") => vec![json!({"id": id, "result": {"thread": {"id": "t1"}, "model": "m"}})],
                Some("turn/start") => vec![
                    json!({"id": id, "result": {"turn": {"id": "u1"}}}),
                    json!({"id": "ask-1", "method": "item/commandExecution/requestApproval", "params": {"threadId": "t1", "turnId": "u1", "itemId": "c9", "command": "rm -rf build", "startedAtMs": 0}}),
                ],
                None => {
                    // The client's answer to the approval.
                    *answered.lock().unwrap() = Some(m.clone());
                    vec![json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "u1", "status": "completed"}}})]
                }
                _ => vec![],
            }
        }
    });
    let codex = Arc::new(Codex::over(reader, writer));
    let events = codex.take_events().unwrap();
    codex.open(std::path::Path::new("/tmp"), None).unwrap();
    let turn = {
        let codex = codex.clone();
        std::thread::spawn(move || codex.prompt("t1", "clean"))
    };
    let request = loop {
        match events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Permission(request) => break request,
            _ => continue,
        }
    };
    assert_eq!(request.tool.title, "rm -rf build");
    assert_eq!(request.options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["accept", "acceptForSession", "decline"]);
    codex.answer_permission(&request.request, Some("accept")).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::EndTurn);
    let answer = answered.lock().unwrap().clone().unwrap();
    assert_eq!(answer["id"], "ask-1");
    assert_eq!(answer["result"]["decision"], "accept");
}

// ── Claude Code ───────────────────────────────────────────────────────────

/// Claude Code's streaming mode: models on `initialize`, a turn that runs a
/// tool after asking, streams its reply and ends with a result.
fn fake_claude() -> Claude {
    let (reader, writer) = scripted(|m| match m["type"].as_str() {
        Some("control_request") => {
            let id = m["request_id"].clone();
            let ok = |response: Value| json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": response}});
            match m["request"]["subtype"].as_str() {
                Some("initialize") => vec![ok(json!({"models": [
                    {"value": "default", "displayName": "Default (recommended)", "description": "Opus"},
                    {"value": "sonnet", "displayName": "Sonnet", "description": "Fast"}
                ]}))],
                Some("set_model") => vec![ok(json!({}))],
                Some("interrupt") => vec![
                    ok(json!({})),
                    json!({"type": "result", "subtype": "error_during_execution", "is_error": true}),
                ],
                _ => vec![],
            }
        }
        Some("user") => {
            let text = m["message"]["content"][0]["text"].as_str().unwrap_or_default();
            if text.contains("wait") {
                return vec![];
            }
            vec![json!({"type": "control_request", "request_id": "perm-1", "request": {"subtype": "can_use_tool", "tool_name": "Write", "input": {"file_path": "/p/a.txt", "content": "hi\n"}, "tool_use_id": "w1"}})]
        }
        // The permission answer: run the tool and reply.
        Some("control_response") => {
            assert_eq!(m["response"]["request_id"], "perm-1");
            assert_eq!(m["response"]["response"]["behavior"], "allow");
            vec![
                json!({"type": "assistant", "message": {"id": "msg1", "content": [{"type": "tool_use", "id": "w1", "name": "Write", "input": {"file_path": "/p/a.txt", "content": "hi\n"}}]}}),
                json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "w1", "content": "ok", "is_error": false}]}}),
                json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "msg2"}}}),
                json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "do"}}}),
                json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "ne"}}}),
                json!({"type": "assistant", "message": {"id": "msg2", "content": [{"type": "text", "text": "done"}]}}),
                json!({"type": "result", "subtype": "success", "is_error": false}),
            ]
        }
        _ => vec![],
    });
    Claude::over(reader, writer)
}

#[test]
fn claude_lists_its_models_and_switches_them() {
    let claude = fake_claude();
    let session = claude.open(std::path::Path::new("/tmp"), None).unwrap();
    assert_eq!(session.id.len(), 36, "a fresh session id is a UUID");
    let model = session.options.model.unwrap();
    assert_eq!(model.current, "default");
    assert_eq!(model.choices.len(), 2);
    let options = claude.set_option(&session.id, &SetVia::Model, "sonnet").unwrap().unwrap();
    assert_eq!(options.model.unwrap().current, "sonnet");
}

#[test]
fn claude_turn_asks_runs_the_tool_streams_and_ends() {
    let claude = Arc::new(fake_claude());
    let events = claude.take_events().unwrap();
    let session = claude.open(std::path::Path::new("/tmp"), None).unwrap();
    let turn = {
        let claude = claude.clone();
        let id = session.id.clone();
        std::thread::spawn(move || claude.prompt(&id, "write a.txt"))
    };
    let request = loop {
        match events.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Permission(request) => break request,
            _ => continue,
        }
    };
    assert_eq!(request.tool.kind, "edit");
    assert_eq!(request.tool.locations, vec!["/p/a.txt".to_string()]);
    claude.answer_permission(&request.request, Some("allow")).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::EndTurn);
    let seen = drain(&events);
    assert_eq!(said(&seen), "done", "streamed text is not repeated by the whole message");
    assert!(seen.iter().any(|e| matches!(e, Event::Update { update: Update::ToolPatch(p), .. } if p.id == "w1" && p.status.as_deref() == Some("completed"))));
}

#[test]
fn claude_stop_interrupts_the_running_turn() {
    let claude = Arc::new(fake_claude());
    let session = claude.open(std::path::Path::new("/tmp"), None).unwrap();
    let turn = {
        let claude = claude.clone();
        let id = session.id.clone();
        std::thread::spawn(move || claude.prompt(&id, "wait for me"))
    };
    std::thread::sleep(Duration::from_millis(200));
    claude.cancel(&session.id).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::Cancelled);
}

#[test]
fn every_chat_agent_has_a_driver() {
    for provider in ["claude", "codex", "opencode", "gemini", "qwen", "cursor-agent", "copilot"] {
        assert!(driver_for(provider).is_some(), "{provider}");
    }
    assert_eq!(driver_for("codex").unwrap().0, DriverKind::Codex);
    assert_eq!(driver_for("claude").unwrap().0, DriverKind::Claude);
    assert!(launch("claude").is_none(), "Claude isn't an ACP agent");
}

// ── Live, against the CLIs on this machine ──────────────────────────────

fn live_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("dragent-driver-live");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `cargo test -p dragent_acp -- --ignored live_codex`
#[test]
#[ignore]
fn live_codex_turn_and_interrupt() {
    let dir = live_dir();
    let codex: Arc<dyn ChatDriver> = start("codex", std::path::Path::new("codex"), &[], &dir, &[]).unwrap();
    let events = codex.take_events().unwrap();
    let session = codex.open(&dir, None).unwrap();
    eprintln!("codex models: {:?}", session.options.model.as_ref().map(|m| m.choices.len()));
    assert_eq!(codex.prompt(&session.id, "Reply with just the word: hello").unwrap(), StopReason::EndTurn);
    assert!(said(&drain(&events)).to_lowercase().contains("hello"));
    let turn = {
        let codex = codex.clone();
        let id = session.id.clone();
        std::thread::spawn(move || codex.prompt(&id, "Count slowly from 1 to 500, one number per line."))
    };
    std::thread::sleep(Duration::from_secs(6));
    codex.cancel(&session.id).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::Cancelled);
}

/// `cargo test -p dragent_acp -- --ignored live_claude`
#[test]
#[ignore]
fn live_claude_turn_and_interrupt() {
    let dir = live_dir();
    let claude: Arc<dyn ChatDriver> = start("claude", std::path::Path::new("claude"), &[], &dir, &[]).unwrap();
    let events = claude.take_events().unwrap();
    let session = claude.open(&dir, None).unwrap();
    let models = session.options.model.clone().expect("models");
    eprintln!("claude models: {:?}", models.choices.iter().map(|c| &c.id).collect::<Vec<_>>());
    claude.set_option(&session.id, &SetVia::Model, "haiku").unwrap();
    assert_eq!(claude.prompt(&session.id, "Reply with just the word: hello").unwrap(), StopReason::EndTurn);
    assert!(said(&drain(&events)).to_lowercase().contains("hello"));
    let turn = {
        let claude = claude.clone();
        let id = session.id.clone();
        std::thread::spawn(move || claude.prompt(&id, "Write a 3000 word essay about the history of terminals."))
    };
    std::thread::sleep(Duration::from_secs(6));
    claude.cancel(&session.id).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::Cancelled);
}

/// A real process that refuses to continue a conversation (as Claude Code
/// does for one it can't open) but otherwise answers: the driver starts a
/// new conversation, and the refused attempt's exit is never reported as
/// the agent stopping (that once restarted and killed the healthy agent).
#[test]
#[cfg(unix)]
fn claude_falls_back_to_a_new_conversation_without_reporting_an_exit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("fake-claude-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(
        &script,
        r#"#!/bin/sh
for a in "$@"; do
  if [ "$a" = "--resume" ]; then echo "No conversation found" >&2; exit 1; fi
done
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=$(printf '%s' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"models":[{"value":"default","displayName":"Default"}]}}}\n' "$id";;
    *'"type":"user"'*)
      printf '{"type":"result","subtype":"success","is_error":false}\n';;
  esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let claude: Arc<dyn ChatDriver> = start("claude", &script, &[], &dir, &[]).unwrap();
    let events = claude.take_events().unwrap();
    let session = claude.open(&dir, Some("never-saved")).unwrap();
    assert_ne!(session.id, "never-saved", "a new conversation instead");
    assert_eq!(claude.prompt(&session.id, "hi").unwrap(), StopReason::EndTurn);
    let seen = drain(&events);
    assert!(!seen.iter().any(|e| matches!(e, Event::Exited)), "the refused attempt's exit is old news");
    claude.shutdown();
    std::fs::remove_dir_all(&dir).ok();
}
