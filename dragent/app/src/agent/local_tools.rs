//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use crate::process_ext::NoWindow;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use base64::Engine as _;
use once_cell::sync::Lazy;
use uuid::Uuid;

/// Re-exported area modules; existing `local_tools::…` paths keep working.
/// Canonical homes are the tools_* modules.
pub(crate) use super::tools_background::*;
pub(crate) use super::tools_command::*;
pub(crate) use super::tools_files::*;
pub(crate) use super::tools_mcp_auth::*;
pub(crate) use super::tools_search::*;
pub(crate) use super::tools_shell::*;
pub(crate) use super::tools_subagent::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn verification_callback_records_real_exit_and_drained_output() {
        for script in [
            "printf 'passed'; exit 0",
            "sleep 0.1; printf 'failed'; exit 7",
        ] {
            let mut child = Command::new("/bin/sh")
                .args(["-c", script])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let completed = start_io_threads(
                stdout,
                stderr,
                Arc::new(std::sync::Mutex::new(String::new())),
                done.clone(),
                Arc::new(std::sync::Mutex::new(None)),
                child,
                Some(Box::new(move |code, output| {
                    tx.send((code, output)).unwrap();
                })),
            );
            let (code, output) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert_eq!(code, Some(if script.contains("exit 7") { 7 } else { 0 }));
            assert!(output.contains(if code == Some(0) { "passed" } else { "failed" }));
            completed
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(done.load(std::sync::atomic::Ordering::Acquire));
        }
    }

    #[test]
    fn visible_terminal_routing_requires_the_same_working_directory() {
        let pane = Uuid::new_v4();
        let project = Path::new("/workspace/project");
        set_project_cwd(project, pane);

        assert!(run_command_targets_terminal_cwd(
            r#"{"command":"cargo check"}"#,
            project,
            pane,
        ));
        assert!(run_command_targets_terminal_cwd(
            r#"{"command":"cargo check","cwd":"."}"#,
            project,
            pane,
        ));
        assert!(run_command_targets_terminal_cwd(
            r#"{"command":"cargo check","cwd":"/workspace/project/src/.."}"#,
            project,
            pane,
        ));
        assert!(!run_command_targets_terminal_cwd(
            r#"{"command":"npm test","cwd":"frontend"}"#,
            project,
            pane,
        ));
        assert!(!run_command_targets_terminal_cwd(
            r#"{"command":"npm test","cwd":"/workspace/other"}"#,
            project,
            pane,
        ));
        assert!(!run_command_targets_terminal_cwd("not json", project, pane,));
    }

    #[test]
    fn detects_commands_that_can_move_the_visible_shell() {
        assert!(command_may_change_directory("cd backend && cargo test"));
        assert!(command_may_change_directory("echo ready; pushd frontend"));
        assert!(command_may_change_directory("builtin cd ../server"));
        assert!(!command_may_change_directory("cargo test"));
        assert!(!command_may_change_directory("printf 'cd backend'"));
    }

    #[test]
    fn utf8_preview_truncation_never_splits_multibyte_text() {
        let text = "accessibility ✅ café — 日本語";
        let prefix = utf8_prefix(text, 13);
        let suffix = utf8_suffix(text, 13);
        assert!(text.is_char_boundary(prefix.len()));
        assert!(text.is_char_boundary(text.len() - suffix.len()));

        let mut owned = text.to_string();
        truncate_utf8(&mut owned, 13);
        assert!(owned.is_char_boundary(owned.len()));
        assert_eq!(owned, prefix);
    }

    #[test]
    fn every_filesystem_read_tool_reports_external_paths() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        let sibling = base.path().join("marketing");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let file = sibling.join("page.tsx");
        std::fs::write(&file, "export default function Page() {}\n").unwrap();
        let pane = Uuid::new_v4();
        set_project_cwd(&project, pane);

        let single_path_tools = [
            "read_file",
            "list_directory",
            "list_directory_with_sizes",
            "directory_tree",
            "search_files",
            "get_file_info",
            "grep",
        ];
        for tool in single_path_tools {
            let args = serde_json::json!({"path": file}).to_string();
            assert_eq!(
                external_read_paths_for_tool(tool, &args, pane).len(),
                1,
                "{tool} must share the external-read boundary"
            );
        }
        let args = serde_json::json!({"paths": [file]}).to_string();
        assert_eq!(
            external_read_paths_for_tool("read_multiple_files", &args, pane).len(),
            1
        );
    }

    #[test]
    fn external_read_is_denied_until_the_call_gets_a_one_shot_grant() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        let sibling = base.path().join("marketing");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let file = sibling.join("page.md");
        std::fs::write(&file, "Dragent marketing").unwrap();
        let pane = Uuid::new_v4();
        set_project_cwd(&project, pane);
        let args = serde_json::json!({"path": file}).to_string();

        let (denied, is_err) = execute_with_call_id("outside-denied", "read_file", &args, pane);
        assert!(is_err);
        assert!(denied.contains("Permission required"), "got: {denied}");

        grant_external_read_once("outside-approved", vec![sibling]);
        let (content, is_err) = execute_with_call_id("outside-approved", "read_file", &args, pane);
        assert!(!is_err, "got: {content}");
        assert!(content.contains("Dragent marketing"));
    }

    #[test]
    fn writes_outside_the_project_need_this_calls_grant() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        let home = base.path().join("home");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let pane = Uuid::new_v4();
        set_project_cwd(&project, pane);

        // Inside the project: no question.
        let inside = serde_json::json!({"path": "notes.txt", "content": "ok"}).to_string();
        let (out, is_err) = execute_with_call_id("w-inside", "write_file", &inside, pane);
        assert!(!is_err, "got: {out}");
        assert!(project.join("notes.txt").exists());

        // Outside: refused for every write tool until the call is granted.
        let rc = home.join("rc");
        let outside = serde_json::json!({"path": rc, "content": "evil"}).to_string();
        for (tool, args) in [
            ("write_file", outside.clone()),
            (
                "create_directory",
                serde_json::json!({"path": home.join("d")}).to_string(),
            ),
            (
                "batch_edit",
                serde_json::json!({"edits": [{"path": rc, "old_str": "a", "new_str": "b"}]})
                    .to_string(),
            ),
            (
                "apply_patch",
                serde_json::json!({"patch": format!(
                    "--- a/{0}\n+++ b/{0}\n@@ -0,0 +1 @@\n+evil\n", rc.display()
                )})
                .to_string(),
            ),
        ] {
            let (out, is_err) = execute_with_call_id("w-denied", tool, &args, pane);
            assert!(
                is_err && out.contains("outside the project"),
                "{tool}: {out}"
            );
        }
        assert!(!rc.exists());

        grant_external_read_once("w-approved", vec![home.clone()]);
        let (out, is_err) = execute_with_call_id("w-approved", "write_file", &outside, pane);
        assert!(!is_err, "got: {out}");
        // The grant was for that call only.
        let (_, is_err) = execute_with_call_id("w-approved", "write_file", &outside, pane);
        assert!(is_err);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_under_a_symlink_out_of_the_project_counts_as_outside() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        let elsewhere = base.path().join("elsewhere");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, project.join("link")).unwrap();
        let pane = Uuid::new_v4();
        set_project_cwd(&project, pane);
        let args = serde_json::json!({"path": "link/new.txt", "content": "x"}).to_string();
        assert_eq!(
            external_write_paths_for_tool("write_file", &args, pane).len(),
            1
        );
        let (out, is_err) = execute_with_call_id("w-link", "write_file", &args, pane);
        assert!(is_err, "got: {out}");
        assert!(!elsewhere.join("new.txt").exists());
    }

    #[test]
    fn quoted_external_path_in_safe_command_requires_scope() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        let sibling = base.path().join("marketing pages");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let pane = Uuid::new_v4();
        set_project_cwd(&project, pane);
        let command = format!("find \"{}\" -maxdepth 1 -type f", sibling.display());
        let args = serde_json::json!({"command": command}).to_string();
        let paths = external_read_paths_for_tool("run_command", &args, pane);
        assert_eq!(paths, vec![sibling.canonicalize().unwrap()]);
    }

    #[test]
    fn unknown_tool_returns_error() {
        let (out, is_err) = execute("nonexistent_tool", "{}", Uuid::nil());
        assert!(is_err);
        assert!(out.contains("Unknown tool"));
    }

    #[test]
    fn manage_task_status_polling_is_structurally_disabled() {
        let (out, is_err) = execute(
            "manage_task",
            r#"{"task_id":"any-task","action":"status"}"#,
            Uuid::nil(),
        );
        assert!(is_err);
        assert!(out.contains("status polling is disabled"), "got: {out}");

        let (out, is_err) = execute("manage_task", r#"{"task_id":"any-task"}"#, Uuid::nil());
        assert!(is_err);
        assert!(out.contains("status polling is disabled"), "got: {out}");
    }

    #[test]
    fn spawn_subagent_depth_gate_rejects_nested_spawn_offline() {
        // A nested worker attempting to spawn must be refused by the hard
        // gate without touching the network.
        let (out, is_err) = crate::agent::tools_subagent::with_test_depth(1, || {
            execute("spawn_subagent", r#"{"query":"hello"}"#, Uuid::nil())
        });
        assert!(is_err, "nested spawn must be refused");
        assert!(out.contains("depth limit"), "got: {out}");
    }

    #[test]
    fn spawn_subagent_depth_zero_passes_gate() {
        // Depth helper round-trips: default thread depth is 0 (allowed).
        assert_eq!(crate::agent::tools_subagent::test_depth(), 0);
    }

    #[test]
    fn spawn_subagent_unknown_role_errors_offline() {
        let (out, is_err) = execute(
            "spawn_subagent",
            r#"{"query":"x","role":"admin"}"#,
            Uuid::nil(),
        );
        assert!(is_err, "unknown role must be refused");
        assert!(out.contains("Unknown worker role"), "got: {out}");
    }

    #[test]
    fn spawn_subagent_missing_query_returns_error() {
        // Missing required "query" arg → should return an error without hitting network.
        let (out, is_err) = execute("spawn_subagent", "{}", Uuid::nil());
        assert!(is_err, "expected error for missing query arg");
        assert!(out.contains("missing required argument"), "got: {out}");
    }

    #[test]
    fn spawn_subagent_routes_to_exec() {
        // With a query arg, run_subtask_sync will either fail with a network
        // error (if backend is down) or succeed (if backend is running).
        // Either way, it confirms dispatch reached exec_spawn_subagent (not Unknown tool).
        let (out, is_err) = execute("spawn_subagent", r#"{"query":"hello"}"#, Uuid::nil());
        // Should be a network/worker error or success, not "Unknown tool".
        // The exact failure text varies by environment (backend down, signed
        // out, quota) — asserting specific error strings made this test flaky,
        // so only the dispatch itself is asserted.
        assert!(!out.contains("Unknown tool"), "dispatch failed: {out}");
        let _ = is_err; // any outcome is valid as long as dispatch happened
    }

    #[test]
    fn github_pr_tool_is_wired_and_validates_arguments_before_network() {
        let (out, is_err) = execute("github_create_pull_request", "{}", Uuid::nil());
        assert!(is_err);
        assert!(
            out.contains("missing required argument: title"),
            "got: {out}"
        );
    }

    #[test]
    fn read_file_nonexistent_returns_error() {
        let (out, is_err) = execute(
            "read_file",
            r#"{"path":"/nonexistent/path/xyz.rs"}"#,
            Uuid::nil(),
        );
        assert!(is_err);
        assert!(out.contains("Cannot open"));
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt").to_string_lossy().to_string();

        let (_, is_err) = execute(
            "write_file",
            &serde_json::json!({
                "path": path, "content": "hello world"
            })
            .to_string(),
            Uuid::nil(),
        );
        assert!(!is_err);

        let (out, is_err) = execute(
            "read_file",
            &serde_json::json!({"path": path}).to_string(),
            Uuid::nil(),
        );
        assert!(!is_err);
        assert!(out.contains("hello world"));
    }
}

#[cfg(all(test, unix))]
mod orchestration_tests {
    use super::*;

    fn run(args: serde_json::Value, call_id: &str) -> (String, bool) {
        run_in_pane(args, call_id, Uuid::new_v4())
    }

    fn run_in_pane(args: serde_json::Value, call_id: &str, pane: Uuid) -> (String, bool) {
        execute_with_call_id(call_id, "run_command", &args.to_string(), pane)
    }

    #[test]
    fn running_commands_are_scoped_per_pane() {
        let pane_a = Uuid::new_v4();
        let pane_b = Uuid::new_v4();
        let (_, _) = run_in_pane(
            serde_json::json!({"command": "sleep 5", "wait_ms_before_async": 200}),
            "call-pane-a",
            pane_a,
        );
        let a = running_agent_commands_for(pane_a);
        let b = running_agent_commands_for(pane_b);
        assert!(a.iter().any(|r| r.id == "call-pane-a"), "own pane sees it");
        assert!(
            !b.iter().any(|r| r.id == "call-pane-a"),
            "another pane must NOT see it"
        );
        kill_command("call-pane-a");
    }

    #[test]
    fn fast_foreground_command_completes_and_leaves_no_registry_entry() {
        let (out, is_err) = run(
            serde_json::json!({"command": "echo orchestration_ok"}),
            "call-fg-1",
        );
        assert!(!is_err, "unexpected error: {out}");
        assert!(out.contains("orchestration_ok"));
        assert!(
            !running_agent_commands().iter().any(|r| r.id == "call-fg-1"),
            "foreground entry must be cleaned up"
        );
    }

    #[test]
    fn slow_command_backgrounds_with_metacognitive_alert_and_is_visible() {
        let (out, is_err) = run(
            serde_json::json!({"command": "sleep 5", "wait_ms_before_async": 300}),
            "call-bg-1",
        );
        assert!(!is_err);
        assert!(out.contains("still running"), "alert text: {out}");
        assert!(
            out.contains("call-bg-1"),
            "task id must be the call id: {out}"
        );
        assert!(out.contains("Do NOT poll"));

        let runs = running_agent_commands();
        let run = runs
            .iter()
            .find(|r| r.id == "call-bg-1")
            .expect("backgrounded command must be visible in the registry");
        assert!(run.backgrounded);
        assert_eq!(run.command, "sleep 5");

        // Cancel it and confirm the registry clears via the watcher path.
        assert!(kill_command("call-bg-1"));
        for _ in 0..50 {
            if !running_agent_commands().iter().any(|r| r.id == "call-bg-1") {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("killed task never left the registry");
    }

    #[test]
    fn running_command_registry_notifies_its_pane() {
        let pane = Uuid::new_v4();
        let (tx, rx) = async_channel::bounded(8);
        register_activity_sender(pane, tx);

        let (_, is_err) = run_in_pane(
            serde_json::json!({"command": "sleep 5", "wait_ms_before_async": 100}),
            "call-activity-notify",
            pane,
        );
        assert!(!is_err);
        assert!(
            rx.try_recv().is_ok(),
            "starting/backgrounding a command must wake its pane"
        );
        kill_command("call-activity-notify");
    }

    #[test]
    fn kill_command_on_unknown_or_finished_task_is_false() {
        assert!(!kill_command("no-such-task"));
    }

    #[test]
    fn ask_user_tool_is_advertised_with_valid_schema() {
        // The client MUST advertise ask_user in extra_tools, or the model can
        // never call it (and falls back to asking as plain text).
        let defs = filesystem_tool_defs();
        let (_, desc, schema_json) = defs
            .iter()
            .find(|(name, _, _)| name == "ask_user")
            .expect("ask_user must be in filesystem_tool_defs");
        // Description must steer the model to the tool over a text list.
        assert!(
            desc.to_lowercase().contains("interactive") || desc.to_lowercase().contains("choose")
        );
        // Schema must be valid JSON with the expected `questions` array shape.
        let schema: serde_json::Value =
            serde_json::from_str(schema_json).expect("valid JSON schema");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["questions"]["type"], "array");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "questions"));

        let (_, todo_desc, todo_schema_json) = defs
            .iter()
            .find(|(name, _, _)| name == "todowrite")
            .expect("todowrite must be in filesystem_tool_defs");
        // Description must steer the model to proactive task tracking.
        assert!(todo_desc.to_lowercase().contains("task list"));
        // Schema must be valid JSON with the expected `todos` array shape.
        let todo_schema: serde_json::Value =
            serde_json::from_str(todo_schema_json).expect("valid JSON schema");
        assert_eq!(todo_schema["type"], "object");
        assert_eq!(todo_schema["properties"]["todos"]["type"], "array");
        assert_eq!(
            todo_schema["properties"]["todos"]["items"]["properties"]["content"]["type"],
            "string"
        );
        assert!(todo_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "todos"));

        let (_, pr_desc, pr_schema_json) = defs
            .iter()
            .find(|(name, _, _)| name == "github_create_pull_request")
            .expect("github_create_pull_request must be advertised");
        assert!(pr_desc.contains("gh"));
        let pr_schema: serde_json::Value =
            serde_json::from_str(pr_schema_json).expect("valid PR tool schema");
        for field in ["title", "body", "base", "head"] {
            assert!(pr_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == field));
        }
    }

    #[test]
    fn running_snapshot_reports_last_output_line() {
        let (_, _) = run(
            serde_json::json!({
                "command": "echo first_line; echo second_line; sleep 5",
                "wait_ms_before_async": 500
            }),
            "call-bg-2",
        );
        let runs = running_agent_commands();
        let run = runs.iter().find(|r| r.id == "call-bg-2").expect("visible");
        assert_eq!(run.last_line.as_deref(), Some("second_line"));
        kill_command("call-bg-2");
    }
}
