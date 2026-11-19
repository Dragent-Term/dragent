//! Shell commands: run_command, GitHub PR, task management.
//!
//! Moved verbatim; same behavior.

//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use super::tools_background::*;
use super::tools_files::*;
use super::tools_mcp_auth::*;
use super::tools_search::*;
use super::tools_shell::*;
use super::tools_subagent::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;
/// Create a pull request through the user's local `gh` authentication.
///
/// This intentionally does not accept a token and does not invoke a shell:
/// GitHub credentials stay in the user's GitHub CLI/keychain configuration,
/// while each argument is passed directly to `gh` to avoid interpolation.
pub(crate) fn exec_github_create_pull_request(args: &serde_json::Value) -> (String, bool) {
    let title = match str_arg(args, "title") {
        Ok(v) if !v.trim().is_empty() => v,
        Ok(_) => return ("title must not be empty".to_string(), true),
        Err(e) => return (e, true),
    };
    let body = match str_arg(args, "body") {
        Ok(v) => v,
        Err(e) => return (e, true),
    };
    let base = match str_arg(args, "base") {
        Ok(v) if !v.trim().is_empty() => v,
        Ok(_) => return ("base must not be empty".to_string(), true),
        Err(e) => return (e, true),
    };
    let head = match str_arg(args, "head") {
        Ok(v) if !v.trim().is_empty() => v,
        Ok(_) => return ("head must not be empty".to_string(), true),
        Err(e) => return (e, true),
    };

    let pane = current_pane();
    let cwd = SHELL_STATE
        .lock()
        .ok()
        .and_then(|states| states.get(&pane).and_then(|state| state.cwd.clone()))
        .map(std::path::PathBuf::from)
        .or_else(|| PROJECT_CWD.lock().ok().and_then(|m| m.get(&pane).cloned()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    if !cwd.is_dir() {
        return (
            format!("Working directory does not exist: {}", cwd.display()),
            true,
        );
    }

    let mut command = crate::process_ext::gh_command();
    command.no_window();
    command
        .arg("pr")
        .arg("create")
        .arg("--title")
        .arg(title)
        .arg("--body")
        .arg(body)
        .arg("--base")
        .arg(base)
        .arg("--head")
        .arg(head);
    if let Some(repo) = args["repo"].as_str().filter(|v| !v.trim().is_empty()) {
        command.arg("--repo").arg(repo);
    }
    if args["draft"].as_bool().unwrap_or(false) {
        command.arg("--draft");
    }

    let output = match command
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) => return (format!("Unable to run GitHub CLI (`gh`): {error}"), true),
    };
    let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    if text.len() > MAX_OUTPUT {
        truncate_utf8(&mut text, MAX_OUTPUT);
        text.push_str("\n…(output truncated)…");
    }
    if output.status.success() {
        (format!("Pull request created successfully.\n{text}"), false)
    } else {
        let code = output.status.code().unwrap_or(-1);
        (format!("GitHub CLI failed (exit {code}).\n{text}"), true)
    }
}

pub(crate) fn exec_run_command(args: &serde_json::Value) -> (String, bool) {
    let command = match str_arg(args, "command") {
        Ok(c) => c,
        Err(e) => return (e, true),
    };
    let task_id = CALL_ID
        .with(|c| c.borrow().clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()[..8].to_string());
    crate::agent::checkpoint_store::begin_shell_checkpoint(&task_id);
    // Foreground window before auto-backgrounding. The model's value is only a
    // HINT: it is HARD-CAPPED at AUTO_BACKGROUND_AFTER_MS so a single command can
    // never block the turn (and lock the input / Stop button) indefinitely.
    //   - `0` used to mean "wait forever" — but a command that never exits (e.g.
    //     a `yarn dev` the model wrongly judged synchronous) then deadlocked the
    //     agent in "working", and because it stayed a FOREGROUND task the only
    //     escape (Stop) killed it. Now `0` → the cap.
    //   - Omitted → the cap, so the user is never left blind.
    // Past the cap the command backgrounds, the turn ends, the input unlocks,
    // and the job keeps running quietly (and now survives Stop, which only kills
    // foreground commands). Long-but-finite commands auto-resume when they exit.
    let requested = args["wait_ms_before_async"]
        .as_u64()
        .unwrap_or(AUTO_BACKGROUND_AFTER_MS);
    let wait_ms = if requested == 0 {
        AUTO_BACKGROUND_AFTER_MS
    } else {
        requested.min(AUTO_BACKGROUND_AFTER_MS)
    };

    let pane_id = current_pane();

    let explicit_cwd = args["cwd"].as_str();
    let effective_cwd: Option<std::path::PathBuf> = if let Some(dir) = explicit_cwd {
        // Tool cwd values are project-relative unless absolute. Resolving here
        // keeps background routing consistent with every other local tool and
        // avoids inheriting the application's process directory by accident.
        Some(resolve_path(dir))
    } else {
        let state_cwd = SHELL_STATE.lock().ok().and_then(|m| {
            m.get(&pane_id)
                .and_then(|s| s.cwd.as_ref())
                .map(std::path::PathBuf::from)
        });
        state_cwd.or_else(|| {
            PROJECT_CWD
                .lock()
                .ok()
                .and_then(|m| m.get(&pane_id).cloned())
        })
    };

    // Dedup: if this exact command is already running in this pane and directory,
    // stop the stale instance before starting a fresh one so identical commands
    // (e.g. a re-triggered `npm test`) never stack up.
    stop_duplicate_running(command, &effective_cwd, pane_id);

    let verification = match crate::agent::host_verification::begin(
        pane_id,
        &task_id,
        args,
        command,
        &effective_cwd,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            crate::agent::checkpoint_store::finish_shell_checkpoint(&task_id);
            return (format!("Could not record verification: {error}"), true);
        }
    };

    let mut child = match spawn_child(command, &effective_cwd) {
        Err(e) => {
            if let Some(record) = verification {
                record(None, e.clone());
            }
            crate::agent::checkpoint_store::finish_shell_checkpoint(&task_id);
            return (e, true);
        }
        Ok(c) => c,
    };
    let pid = child.id();

    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let log = Arc::new(std::sync::Mutex::new(String::new()));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let exit_code = Arc::new(std::sync::Mutex::new(None::<i32>));

    let completion_rx = start_io_threads(
        stdout,
        stderr,
        log.clone(),
        done.clone(),
        exit_code.clone(),
        child,
        verification,
    );

    // Register immediately — foreground runs must be as visible (and as
    // killable) as backgrounded ones. Keyed by the pipeline call_id so the
    // ToolStep block rendering this call can find its live process.
    BACKGROUND_TASKS.lock().unwrap().insert(
        task_id.clone(),
        BackgroundTask {
            command: command.to_string(),
            stdin: std::sync::Mutex::new(stdin),
            log: log.clone(),
            done: done.clone(),
            exit_code: exit_code.clone(),
            started_at: web_time::Instant::now(),
            pid,
            backgrounded: std::sync::atomic::AtomicBool::new(false),
            pane_id,
            cwd: effective_cwd.clone(),
        },
    );
    notify_activity(pane_id);

    // Try to complete within the (bounded) foreground window. If the command is
    // still running after it, background it and return immediately so the turn
    // ends and the input unlocks — the watcher resumes the agent when it exits.
    let finished = completion_rx
        .recv_timeout(std::time::Duration::from_millis(wait_ms))
        .is_ok();

    if finished {
        std::thread::sleep(std::time::Duration::from_millis(50));
        BACKGROUND_TASKS.lock().unwrap().remove(&task_id);
        notify_activity(pane_id);
        let raw = log.lock().unwrap().clone();
        let exit = *exit_code.lock().unwrap();
        if let Some(ref dir) = effective_cwd {
            if let Ok(mut map) = SHELL_STATE.lock() {
                map.entry(pane_id).or_default().cwd = Some(dir.to_string_lossy().into_owned());
            }
        }
        let mut display = format_command_output(&raw, exit);
        if exit == Some(0) {
            display.push_str(
                "\n[system] This command completed successfully. Before calling another tool, \
                 decide whether this result satisfies the user's requested outcome. If it does, \
                 stop and report the conclusion; do not run redundant confirmation commands.",
            );
        }
        // Evidence gate: only a *successful* verification command clears
        // unverified edits. Failed checks keep the gate armed and tell the model.
        match crate::agent::session_facts::record_verification_result(pane_id, command, exit) {
            crate::agent::session_facts::VerificationOutcome::Passed => {}
            crate::agent::session_facts::VerificationOutcome::Failed => {
                display.push_str(
                    "\n[system] Verification failed (non-zero exit). Edits remain unverified — \
                     fix the failure and re-run the check before claiming the task is done.",
                );
            }
            crate::agent::session_facts::VerificationOutcome::Ignored => {}
        }
        crate::agent::checkpoint_store::finish_shell_checkpoint(&task_id);
        return (display, exit.map_or(false, |c| c != 0));
    }

    // Command is still running past the foreground window — hand it back to
    // the model as a background task (the registry entry already exists).
    if let Ok(tasks) = BACKGROUND_TASKS.lock() {
        if let Some(t) = tasks.get(&task_id) {
            t.backgrounded
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    // A dev server may run for hours. Publish filesystem effects from its
    // startup window now so the restore control does not wait for process exit.
    crate::agent::checkpoint_store::finish_shell_checkpoint(&task_id);
    notify_activity(pane_id);
    let preview = {
        let b = log.lock().unwrap();
        if b.trim().is_empty() {
            "(no output yet)".to_string()
        } else {
            b.clone()
        }
    };

    // Watcher: enforce the hard cap, then fire the completion notification the
    // moment the process exits so the agent resumes without polling. Cleans up
    // the registry entry afterwards so the UI activity strip clears.
    {
        let task_id_c = task_id.clone();
        let command_c = command.to_string();
        let log_c = log.clone();
        let done_c = done.clone();
        let exit_code_c = exit_code.clone();
        let started = web_time::Instant::now();
        crate::bg::detach(move || {
            // Opt-in backstop only: without DRAGENT_CMD_TIMEOUT_SECS a backgrounded
            // process (e.g. a dev server) is never auto-killed — it runs until it
            // exits, the user kills it, or the pane closes.
            let cap = command_hard_cap();
            let mut killed_by_cap = false;
            loop {
                if done_c.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                if let Some(cap) = cap {
                    if !killed_by_cap && started.elapsed() > cap {
                        killed_by_cap = true;
                        log::warn!(
                            "[run_command] task {task_id_c} exceeded hard cap ({}s) — killing",
                            cap.as_secs()
                        );
                        kill_command(&task_id_c);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            std::thread::sleep(std::time::Duration::from_millis(100)); // let readers flush
            let mut output = log_c.lock().map(|g| g.clone()).unwrap_or_default();
            if killed_by_cap {
                output.push_str(
                    "\n[system] This command hung past the hard time limit and was killed. \
                     Do not simply retry it — briefly tell the user what happened and try \
                     a different approach.",
                );
            }
            let exit = exit_code_c.lock().ok().and_then(|g| *g);
            if let Ok(mut t) = BACKGROUND_TASKS.lock() {
                t.remove(&task_id_c);
            }
            crate::agent::checkpoint_store::finish_shell_checkpoint(&task_id_c);
            notify_activity(pane_id);
            notify_completion(
                pane_id,
                BackgroundCompletion {
                    task_id: task_id_c,
                    command: command_c,
                    exit_code: exit,
                    output,
                },
            );
        });
    }

    let msg = format!(
        "[system] `{command}` is still running after {}s — it now continues in the \
         background as task {task_id}.\n\nOutput so far:\n{preview}\n\n\
         What to do now:\n\
         1. In one short sentence, tell the user what is running and what you are \
         waiting for (they can see a live status bar, so keep it brief).\n\
         2. If other useful work is possible in parallel, continue with it; \
         otherwise end your turn.\n\
         3. Do NOT poll with manage_task — the system resumes you automatically \
         with the full output the moment the task finishes. Status polling is \
         disabled; manage_task is only available to send input or kill the task.",
        wait_ms / 1000,
    );
    (msg, false)
}

pub(crate) fn exec_manage_task(args: &serde_json::Value) -> (String, bool) {
    let action = args["action"].as_str().unwrap_or("");
    if action.is_empty() || action == "status" {
        return (
            "Background-task status polling is disabled. The system will deliver the full output \
             automatically when the task finishes. Do not call manage_task again; end the turn or \
             continue independent work."
                .to_string(),
            true,
        );
    }

    let task_id = match args["task_id"].as_str() {
        Some(id) => id,
        None => return ("manage_task requires task_id".to_string(), true),
    };

    let tasks = BACKGROUND_TASKS.lock().unwrap();
    let Some(task) = tasks.get(task_id) else {
        return (format!("No background task '{task_id}' found."), true);
    };

    match action {
        "send_input" => {
            let input = args["input"].as_str().unwrap_or("");
            use std::io::Write;
            let mut stdin_guard = task.stdin.lock().unwrap();
            if let Some(ref mut stdin) = *stdin_guard {
                let line = if input.ends_with('\n') {
                    input.to_string()
                } else {
                    format!("{input}\n")
                };
                match stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()) {
                    Ok(_) => (format!("Sent to task {task_id}: {input:?}"), false),
                    Err(e) => (format!("Failed to write to task {task_id}: {e}"), true),
                }
            } else {
                (format!("Task {task_id} stdin is not available."), true)
            }
        }
        "kill" => {
            let command = task.command.clone();
            drop(tasks);
            if kill_command(task_id) {
                // The watcher observes the death, notifies completion, and
                // removes the registry entry — same path as a natural exit.
                (
                    format!("Task {task_id} ({command}) has been killed."),
                    false,
                )
            } else {
                (format!("Task {task_id} already finished."), false)
            }
        }
        other => (
            format!("Unknown action '{other}'. Use: send_input, kill"),
            true,
        ),
    }
}

pub(crate) fn format_command_output(raw: &str, exit_code: Option<i32>) -> String {
    if raw.trim().is_empty() {
        return format!("[exit {}]", exit_code.unwrap_or(0));
    }
    if raw.len() > MAX_OUTPUT {
        let half = MAX_OUTPUT / 2;
        return format!(
            "{}\n... [{} bytes truncated] ...\n{}",
            utf8_prefix(raw, half),
            raw.len() - MAX_OUTPUT,
            utf8_suffix(raw, half)
        );
    }
    raw.to_string()
}

/// ≈4 chars/token — the standard cheap estimate for the metrics log line.
pub(crate) fn approx_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}
