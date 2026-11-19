use std::io::{BufReader, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::*;

/// Bytes written to one end come out of the other.
pub(crate) struct ChannelWriter(pub(crate) mpsc::Sender<Vec<u8>>);
impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf.to_vec()).map_err(|_| std::io::ErrorKind::BrokenPipe)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) struct ChannelReader {
    pub(crate) rx: mpsc::Receiver<Vec<u8>>,
    pub(crate) buffer: Vec<u8>,
}
impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.buffer.is_empty() {
            match self.rx.recv() {
                Ok(bytes) => self.buffer = bytes,
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buffer.len());
        out[..n].copy_from_slice(&self.buffer[..n]);
        self.buffer.drain(..n);
        Ok(n)
    }
}

/// A scripted agent on the other end of in-memory pipes: `script` sees each
/// line the client writes and returns the lines to answer with. Returns the
/// client's reader and writer.
pub(crate) fn scripted(
    mut script: impl FnMut(&Value) -> Vec<Value> + Send + 'static,
) -> (BufReader<ChannelReader>, ChannelWriter) {
    let (to_agent_tx, to_agent_rx) = mpsc::channel::<Vec<u8>>();
    let (to_client_tx, to_client_rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut pending = Vec::new();
        while let Ok(bytes) = to_agent_rx.recv() {
            pending.extend(bytes);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let message: Value = serde_json::from_slice(&line).unwrap();
                for reply in script(&message) {
                    let mut out = serde_json::to_vec(&reply).unwrap();
                    out.push(b'\n');
                    if to_client_tx.send(out).is_err() {
                        return;
                    }
                }
            }
        }
    });
    (BufReader::new(ChannelReader { rx: to_client_rx, buffer: Vec::new() }), ChannelWriter(to_agent_tx))
}

/// A fake agent: `script` sees each message the client sends and returns
/// the lines to send back.
fn fake(mut script: impl FnMut(&Value) -> Vec<Value> + Send + 'static) -> Agent {
    let (to_agent_tx, to_agent_rx) = mpsc::channel::<Vec<u8>>();
    let (to_client_tx, to_client_rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut pending = Vec::new();
        while let Ok(bytes) = to_agent_rx.recv() {
            pending.extend(bytes);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let message: Value = serde_json::from_slice(&line).unwrap();
                for reply in script(&message) {
                    let mut out = serde_json::to_vec(&reply).unwrap();
                    out.push(b'\n');
                    if to_client_tx.send(out).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Agent::over(
        BufReader::new(ChannelReader { rx: to_client_rx, buffer: Vec::new() }),
        ChannelWriter(to_agent_tx),
    )
}

fn answer(message: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": message["id"], "result": result})
}

#[test]
fn a_session_lists_its_models_and_modes_from_config_options() {
    let agent = fake(|m| match m["method"].as_str() {
        Some("initialize") => vec![answer(m, json!({"protocolVersion": 1}))],
        Some("session/new") => vec![answer(
            m,
            json!({"sessionId": "s1", "configOptions": [
                {"id": "model", "category": "model", "type": "select", "currentValue": "b",
                 "options": [{"value": "a", "name": "Model A"}, {"group": "More", "name": "More", "options": [{"value": "b", "name": "Model B"}]}]},
                {"id": "mode", "category": "mode", "type": "select", "currentValue": "agent",
                 "options": [{"value": "agent", "name": "Agent"}, {"value": "plan", "name": "Plan"}]},
            ]}),
        )],
        _ => vec![],
    });
    agent.initialize().unwrap();
    let session = agent.new_session(std::path::Path::new("/tmp")).unwrap();
    assert_eq!(session.id, "s1");
    let model = session.options.model.unwrap();
    assert_eq!(model.current_name(), "Model B");
    assert_eq!(model.choices.len(), 2);
    assert_eq!(model.via, SetVia::ConfigOption("model".into()));
    assert_eq!(session.options.mode.unwrap().choices[1].name, "Plan");
}

#[test]
fn older_agents_list_models_and_modes_as_objects() {
    let options = SessionOptions::parse(&json!({
        "models": {"currentModelId": "auto", "availableModels": [{"modelId": "auto", "name": "Auto"}, {"modelId": "pro", "name": "Pro"}]},
        "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Default"}, {"id": "yolo", "name": "YOLO"}]},
    }));
    let model = options.model.unwrap();
    assert_eq!((model.current.as_str(), model.via), ("auto", SetVia::Model));
    assert_eq!(options.mode.unwrap().via, SetVia::Mode);
}

#[test]
fn a_turn_streams_its_reply_and_tools_then_ends() {
    let agent = fake(|m| match m["method"].as_str() {
        Some("session/prompt") => vec![
            json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls", "kind": "execute", "status": "pending"}}}),
            json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "a.txt"}}, {"type": "diff", "path": "/x", "oldText": null, "newText": "new"}]}}}),
            json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": {"sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": {"type": "text", "text": "hello"}}}}),
            answer(m, json!({"stopReason": "end_turn"})),
        ],
        _ => vec![],
    });
    let events = agent.take_events().unwrap();
    assert_eq!(agent.prompt("s1", "hi").unwrap(), StopReason::EndTurn);
    let mut tool = None;
    let mut text = String::new();
    while let Ok(event) = events.recv_timeout(Duration::from_millis(200)) {
        match event {
            Event::Update { update: Update::ToolCall(call), .. } => tool = Some(call),
            Event::Update { update: Update::ToolPatch(patch), .. } => tool.as_mut().unwrap().apply(patch),
            Event::Update { update: Update::AgentText { text: t, .. }, .. } => text.push_str(&t),
            _ => {}
        }
    }
    let tool = tool.unwrap();
    assert!(tool.finished());
    assert_eq!(tool.content[0], ToolContent::Text("a.txt".into()));
    assert_eq!(tool.content[1], ToolContent::Diff { path: "/x".into(), old: None, new: "new".into() });
    assert_eq!(text, "hello");
}

#[test]
fn cancel_ends_the_running_turn() {
    // The agent holds the prompt until it hears the cancel.
    let held = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let agent = std::sync::Arc::new(fake({
        let held = held.clone();
        move |m| match m["method"].as_str() {
            Some("session/prompt") => {
                *held.lock().unwrap() = Some(m["id"].clone());
                vec![]
            }
            Some("session/cancel") => {
                let id = held.lock().unwrap().take().unwrap();
                vec![json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "cancelled"}})]
            }
            _ => vec![],
        }
    }));
    let turn = {
        let agent = agent.clone();
        std::thread::spawn(move || agent.prompt("s1", "count to a million"))
    };
    std::thread::sleep(Duration::from_millis(100));
    agent.cancel("s1").unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::Cancelled);
}

#[test]
fn choosing_a_model_returns_the_new_options() {
    let agent = fake(|m| match m["method"].as_str() {
        Some("session/set_config_option") => {
            assert_eq!(m["params"]["value"], "a");
            vec![answer(m, json!({"configOptions": [{"id": "model", "category": "model", "type": "select", "currentValue": "a", "options": [{"value": "a", "name": "A"}]}]}))]
        }
        _ => vec![],
    });
    let options = agent.set_option("s1", &SetVia::ConfigOption("model".into()), "a").unwrap().unwrap();
    assert_eq!(options.model.unwrap().current, "a");
}

#[test]
fn errors_and_permission_requests_reach_the_app() {
    let agent = fake(|m| match m["method"].as_str() {
        Some("session/prompt") => vec![
            json!({"jsonrpc": "2.0", "id": "perm-1", "method": "session/request_permission", "params": {"sessionId": "s1", "toolCall": {"toolCallId": "t9", "title": "rm -rf build", "kind": "execute"}, "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"}, {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}}),
        ],
        // The client's answer to the permission request.
        None => {
            assert_eq!(m["id"], "perm-1");
            assert_eq!(m["result"]["outcome"]["optionId"], "yes");
            vec![json!({"jsonrpc": "2.0", "id": 0, "error": {"code": -32603, "message": "This model is not available in your country"}})]
        }
        _ => vec![],
    });
    let events = agent.take_events().unwrap();
    let agent = std::sync::Arc::new(agent);
    let turn = {
        let agent = agent.clone();
        std::thread::spawn(move || agent.prompt("s1", "clean"))
    };
    let Event::Permission(request) = events.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!() };
    assert_eq!(request.tool.title, "rm -rf build");
    assert_eq!(request.options.len(), 2);
    agent.answer_permission(&request.request, Some("yes")).unwrap();
    let error = turn.join().unwrap().unwrap_err();
    assert!(error.message.contains("not available"));
}

#[test]
fn an_agent_that_exits_fails_its_waiting_requests() {
    let agent = fake(|m| match m["method"].as_str() {
        Some("initialize") => panic!("agent crashed"),
        _ => vec![],
    });
    let error = agent.initialize().unwrap_err();
    assert_eq!(error.code, -1);
}

#[test]
fn agents_that_speak_acp_themselves_have_a_launch_command() {
    assert_eq!(launch("opencode"), Some(("opencode", &["acp"][..])));
    assert_eq!(launch("gemini").unwrap().1, &["--acp"]);
    assert!(launch("claude").is_none());
}

/// Talks to the real OpenCode on this machine: `cargo test -p dragent_acp -- --ignored`.
#[test]
#[ignore]
fn live_opencode_turn_and_cancel() {
    let dir = std::env::temp_dir().join("dragent-acp-live");
    std::fs::create_dir_all(&dir).unwrap();
    let agent = std::sync::Arc::new(
        Agent::spawn(std::path::Path::new("opencode"), &["acp".to_string()], &dir, &[]).unwrap(),
    );
    let events = agent.take_events().unwrap();
    agent.initialize().unwrap();
    let session = agent.new_session(&dir).unwrap();
    let model = session.options.model.clone().expect("models");
    eprintln!("model {} of {}", model.current_name(), model.choices.len());
    assert_eq!(agent.prompt(&session.id, "Reply with just the word: hello").unwrap(), StopReason::EndTurn);
    let mut said = String::new();
    while let Ok(event) = events.recv_timeout(Duration::from_millis(300)) {
        if let Event::Update { update: Update::AgentText { text, .. }, .. } = event {
            said.push_str(&text);
        }
    }
    assert!(said.to_lowercase().contains("hello"), "{said}");
    let turn = {
        let agent = agent.clone();
        let id = session.id.clone();
        std::thread::spawn(move || agent.prompt(&id, "Count slowly from 1 to 500, one number per line."))
    };
    std::thread::sleep(Duration::from_secs(5));
    agent.cancel(&session.id).unwrap();
    assert_eq!(turn.join().unwrap().unwrap(), StopReason::Cancelled);
}
