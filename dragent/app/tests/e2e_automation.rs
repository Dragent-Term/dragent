//! End-to-end test against the REAL application binary.
//!
//! Spawns target\debug\dragent.exe with `DRAGENT_AUTOMATION_PORT` set, drives
//! it over the JSON-lines automation protocol (app/src/automation.rs), and
//! asserts a submitted command flows through the real PTY, shell hooks, and
//! session core to a completed block with the right output.
//!
//! Needs a desktop session (the app opens a window) and a built binary:
//!   cargo test -p dragent --test e2e_automation -- --ignored --nocapture

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

struct App {
    child: std::process::Child,
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn connect(port: u16, deadline: Instant) -> TcpStream {
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => return s,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => panic!("automation port never came up: {e}"),
        }
    }
}

fn request(stream: &mut TcpStream, req: serde_json::Value) -> serde_json::Value {
    let mut line = req.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes()).expect("write request");
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("read reply");
    serde_json::from_str(&reply).expect("parse reply")
}

#[test]
#[ignore = "drives the real GUI binary; needs a desktop session"]
fn command_flows_through_real_app_to_completed_block() {
    // Derive a port from the PID to avoid collisions between parallel runs.
    let port = 20000 + (std::process::id() % 10_000) as u16;
    let exe = env!("CARGO_BIN_EXE_dragent");

    let child = std::process::Command::new(exe)
        .env("DRAGENT_AUTOMATION_PORT", port.to_string())
        .env("DRAGENT_INDEX_DISABLE", "1") // keep the test hermetic
        .spawn()
        .expect("spawn dragent");
    let _app = App { child };

    // Debug builds boot slowly; the port opens once the first session exists.
    let mut stream = connect(port, Instant::now() + Duration::from_secs(90));

    let pong = request(&mut stream, serde_json::json!({"cmd": "ping"}));
    assert_eq!(pong["ok"], true, "ping failed: {pong}");

    // Give the shell + hook bootstrap a moment to settle, then submit.
    std::thread::sleep(Duration::from_secs(5));
    let marker = format!("automation-e2e-{}", std::process::id());
    let run = request(
        &mut stream,
        serde_json::json!({"cmd": "run", "text": format!("echo {marker}")}),
    );
    assert_eq!(run["ok"], true, "run failed: {run}");

    // Poll until the echo block exists, is completed, and captured the output.
    let deadline = Instant::now() + Duration::from_secs(60);
    let block = loop {
        let reply = request(&mut stream, serde_json::json!({"cmd": "blocks"}));
        assert_eq!(reply["ok"], true, "blocks failed: {reply}");
        let found = reply["blocks"].as_array().and_then(|blocks| {
            blocks
                .iter()
                .find(|b| {
                    b["command"].as_str().is_some_and(|c| c.contains(&marker))
                        && b["completed"] == true
                })
                .cloned()
        });
        if let Some(b) = found {
            break b;
        }
        assert!(
            Instant::now() < deadline,
            "echo block never completed; last state: {reply}"
        );
        std::thread::sleep(Duration::from_millis(500));
    };

    let text = block["text"].as_str().unwrap_or_default();
    assert!(
        text.contains(&marker),
        "completed block missing echoed output: {block}"
    );
    assert_eq!(block["live"], false, "block should be snapshotted: {block}");

    let quit = request(&mut stream, serde_json::json!({"cmd": "quit"}));
    assert_eq!(quit["ok"], true);
}

#[test]
#[ignore = "drives a separate GUI test binary; run only in an isolated desktop test session"]
fn agentic_workspace_flag_and_layout_are_runtime_navigable() {
    let port = 30000 + (std::process::id() % 10_000) as u16;
    let exe = env!("CARGO_BIN_EXE_dragent");
    let project = std::env::temp_dir().join(format!(
        "dragent-agentic-workspace-e2e-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&project).expect("create isolated workspace");
    let child = std::process::Command::new(exe)
        .current_dir(&project)
        .env("DRAGENT_AUTOMATION_PORT", port.to_string())
        .env("DRAGENT_AGENTIC_WORKSPACE", "1")
        .env("DRAGENT_INDEX_DISABLE", "1")
        .spawn()
        .expect("spawn isolated Dragent test process");
    let _app = App { child };
    let mut stream = connect(port, Instant::now() + Duration::from_secs(90));

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = request(&mut stream, serde_json::json!({"cmd": "workspace_state"}));
        if state["ok"] == true && state["state"]["ready"] == true {
            assert_eq!(state["state"]["enabled"], true);
            assert_eq!(state["state"]["layout"], "develop");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "workspace bridge never became ready: {state}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }

    let switched = request(
        &mut stream,
        serde_json::json!({"cmd": "workspace_layout", "layout": "shape"}),
    );
    assert_eq!(switched["ok"], true, "shape selection failed: {switched}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = request(&mut stream, serde_json::json!({"cmd": "workspace_state"}));
        if state["state"]["layout"] == "shape" {
            assert!(state["state"]["work"].is_null());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Shape never became visible: {state}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let quit = request(&mut stream, serde_json::json!({"cmd": "quit"}));
    assert_eq!(quit["ok"], true);
}
