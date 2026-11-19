//! Subagents, skills, and REPL sessions.
//!
//! Moved verbatim; same behavior.

//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use super::tools_background::*;
use super::tools_command::*;
use super::tools_files::*;
use super::tools_mcp_auth::*;
use super::tools_search::*;
use super::tools_shell::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;
/// One background Worker sub-agent. Mirrors the shape of BackgroundTask so
/// the same UI surfaces (activity strip, cancel) cover both.
pub(crate) struct WorkerTask {
    pub(crate) query: String,
    pub(crate) started_at: web_time::Instant,
    pub(crate) done: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) cancelled: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) pane_id: Uuid,
}

pub(crate) static WORKERS: Lazy<std::sync::Mutex<HashMap<String, WorkerTask>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Foreground window before a worker is handed back as a background task —
/// identical rhythm to run_command's auto-backgrounding, so the parent agent
/// is never blocked (and never times out) waiting on a long subtask. This is
/// also what makes parallel fan-out work: each spawn returns within this
/// window, so the model can launch several workers in one turn.
pub(crate) const WORKER_FOREGROUND_MS: u64 = 10_000;

// Nesting depth of the current execution thread inside spawned workers.
// 0 = terminal agent / coordinator; incremented for each worker level so
// nested spawns hit the hard depth gate in `dragent_agent` policy instead
// of relying on prompt text. (Plain comment: rustdoc skips macro bodies.)
thread_local! {
    static SUBAGENT_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Test hook: read the current thread's subagent depth.
#[cfg(test)]
pub(crate) fn test_depth() -> u32 {
    SUBAGENT_DEPTH.get()
}
/// Test hook: run `f` with the subagent depth overridden.
#[cfg(test)]
pub(crate) fn with_test_depth<R>(depth: u32, f: impl FnOnce() -> R) -> R {
    let prev = SUBAGENT_DEPTH.get();
    SUBAGENT_DEPTH.set(depth);
    let out = f();
    SUBAGENT_DEPTH.set(prev);
    out
}

pub(crate) fn exec_spawn_subagent(args: &serde_json::Value) -> (String, bool) {
    let query = match str_arg(args, "query") {
        Ok(q) => q.to_string(),
        Err(e) => return (e, true),
    };
    let model = args["model_name"]
        .as_str()
        .unwrap_or("claude-sonnet-4-6")
        .to_string();
    // Hard nesting gate (one level): prompt text ("do not spawn") is
    // advisory; this refuses. Depth comes from the executing thread: the
    // terminal agent runs at 0, each worker body runs at parent + 1.
    let depth = SUBAGENT_DEPTH.get();
    if !dragent_agent::controller::subagent::depth_allows_spawn(depth) {
        return (
            dragent_agent::controller::subagent::depth_reject_message(),
            true,
        );
    }
    // Worker role: "worker" (default, full tools) or "explore" (read-only
    // research). Unknown roles fail fast rather than silently running full.
    let role = match args
        .get("role")
        .and_then(|v| v.as_str())
        .map(dragent_agent::controller::subagent::WorkerRole::parse)
        .transpose()
    {
        Ok(role) => role.unwrap_or_default(),
        Err(e) => return (e, true),
    };
    // Subagent inherits the parent pane's ID so its tool calls share the same
    // shell state and cwd as the parent agent on this tab.
    let pane_id = current_pane();

    let worker_id = CALL_ID
        .with(|c| c.borrow().clone())
        .map(|c| format!("w-{c}"))
        .unwrap_or_else(|| format!("w-{}", &uuid::Uuid::new_v4().to_string()[..8]));

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    WORKERS.lock().unwrap().insert(
        worker_id.clone(),
        WorkerTask {
            query: query.clone(),
            started_at: web_time::Instant::now(),
            done: done.clone(),
            cancelled: cancelled.clone(),
            pane_id,
        },
    );
    notify_activity(pane_id);

    let (tx, rx) = std::sync::mpsc::channel::<(String, bool)>();
    {
        let query_t = query.clone();
        let worker_id_t = worker_id.clone();
        let done_t = done.clone();
        let cancelled_t = cancelled.clone();
        crate::bg::detach(move || {
            // This thread executes the worker body: nested tool calls (and
            // any nested spawn attempt) run at parent depth + 1.
            SUBAGENT_DEPTH.set(depth + 1);
            let started = web_time::Instant::now();
            let outcome = crate::agent::pipeline::run_subtask_sync(&model, &query_t, pane_id, role);
            SUBAGENT_DEPTH.set(depth);
            done_t.store(true, std::sync::atomic::Ordering::Release);
            let secs = started.elapsed().as_secs();
            // Structured handoff: id, duration, task, then the result body.
            let (body, is_err) = match outcome {
                Ok(text) => (text, false),
                Err(e) => (format!("Worker failed: {e}"), true),
            };
            let structured = format!(
                "[worker {worker_id_t} finished in {secs}s]\nTask: {query_t}\nResult:\n{body}"
            );
            // Foreground waiter first; if it's gone (backgrounded), notify.
            if tx.send((structured.clone(), is_err)).is_err()
                && !cancelled_t.load(std::sync::atomic::Ordering::Acquire)
            {
                notify_completion(
                    pane_id,
                    BackgroundCompletion {
                        task_id: worker_id_t.clone(),
                        command: format!("worker: {query_t}"),
                        exit_code: Some(if is_err { 1 } else { 0 }),
                        output: structured,
                    },
                );
            }
            if let Ok(mut w) = WORKERS.lock() {
                w.remove(&worker_id_t);
            }
            notify_activity(pane_id);
        });
    }

    match rx.recv_timeout(std::time::Duration::from_millis(WORKER_FOREGROUND_MS)) {
        Ok((result, is_err)) => {
            WORKERS.lock().unwrap().remove(&worker_id);
            notify_activity(pane_id);
            (result, is_err)
        }
        Err(_) => {
            // Dropping rx tells the worker thread to deliver via notification.
            drop(rx);
            (
                format!(
                    "[system] Worker {worker_id} is still running — it continues in the \
                 background. Tell the user in one short sentence what it is working \
                 on; you may spawn more workers or do other work in parallel. You \
                 will be resumed automatically with its result when it finishes. \
                 Do NOT poll."
                ),
                false,
            )
        }
    }
}

// ── skill ─────────────────────────────────────────────────────────────────────
// Discover and load Agent Skills from .agents/skills/ and ~/.agents/skills/.

pub(crate) fn exec_skill(args: &serde_json::Value) -> (String, bool) {
    let name = match str_arg(args, "name") {
        Ok(n) => n,
        Err(e) => return (e, true),
    };
    let skill_args = args["args"].as_str().unwrap_or("");

    let cwd = PROJECT_CWD
        .lock()
        .ok()
        .and_then(|m| m.get(&current_pane()).cloned())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // List mode
    if name.eq_ignore_ascii_case("list") {
        let skills = crate::agent::skill_discovery::discover_skills(&cwd);
        return (
            crate::agent::skill_discovery::format_skill_list(&skills),
            false,
        );
    }

    let skills = crate::agent::skill_discovery::discover_skills(&cwd);

    // Skill names are spec-validated and globally unique after scope overrides.
    let skill = skills.iter().find(|s| s.name == name);
    if let Some(skill) = skill.filter(|s| s.disable_model_invocation) {
        return (
            format!(
                "The '{name}' skill only runs when the user types {}. Ask them to run it if it fits.",
                skill.slash_name()
            ),
            true,
        );
    }

    match skill {
        None => (
            format!(
                "Skill '{name}' not found. Use skill {{\"name\":\"list\"}} to see all available skills."
            ),
            true,
        ),
        Some(s) => match crate::agent::skill_discovery::render_skill_envelope(s) {
            Ok(mut content) => {
                if !skill_args.is_empty() {
                    content.push_str(&format!("\n<user_arguments>{}</user_arguments>", skill_args));
                }
                (content, false)
            }
            Err(error) => (error, true),
        },
    }
}

// ── repl ──────────────────────────────────────────────────────────────────────
// Persistent interpreter sessions. State (variables, functions, cwd) survives
// across calls within the same agent session.
//
// Design:
//   - One session per language, stored in REPL_SESSIONS global.
//   - Code is wrapped with a sentinel print so we know when execution finished.
//   - A background thread feeds stdout lines to a channel; we recv_timeout.
//   - On EOF or error the dead session is evicted so the next call respawns it.

pub(crate) const REPL_SENTINEL: &str = "__DRAGENT_REPL_DONE_9f4b2c__";
pub(crate) const REPL_TIMEOUT_SECS: u64 = 30;

pub(crate) struct ReplSession {
    pub(crate) stdin: std::io::BufWriter<std::process::ChildStdin>,
    pub(crate) stdout_rx: std::sync::mpsc::Receiver<Option<String>>,
    pub(crate) stderr_rx: std::sync::mpsc::Receiver<Option<String>>,
    pub(crate) _stdout_thread: std::thread::JoinHandle<()>,
    pub(crate) _stderr_thread: std::thread::JoinHandle<()>,
}

pub(crate) static REPL_SESSIONS: Lazy<
    std::sync::Mutex<HashMap<(Uuid, String), Arc<std::sync::Mutex<ReplSession>>>>,
> = Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

pub(crate) fn spawn_repl_session(language: &str) -> Result<ReplSession, String> {
    let (program, args): (&str, &[&str]) = match language {
        "python" => {
            if cfg!(windows) {
                ("python", &["-u", "-i", "-q"])
            } else {
                ("python3", &["-u", "-i", "-q"])
            }
        }
        "node" => ("node", &["--interactive"]),
        "bash" => {
            if cfg!(windows) {
                ("bash", &["--noprofile", "--norc"])
            } else {
                ("bash", &["--noprofile", "--norc"])
            }
        }
        other => {
            return Err(format!(
                "Unsupported REPL language: '{other}'. Supported: python, node, bash"
            ))
        }
    };

    let settings_shell = crate::console::editor_settings::EditorSettings::global()
        .terminal
        .default_shell
        .clone();
    let shell = dragent_pty::resolve_shell(settings_shell.or_else(|| std::env::var("SHELL").ok()));
    let mut child_cmd = std::process::Command::new(program);
    child_cmd.no_window();
    dragent_pty::configure_std_shell_command(&mut child_cmd, &shell);

    let mut child = child_cmd
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start {language} interpreter ({program}): {e}"))?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Keep child running; wait in a detached thread.
    crate::bg::detach(move || {
        let _ = child.wait();
    });

    let (out_tx, out_rx) = std::sync::mpsc::channel::<Option<String>>();
    let out_thread = {
        let tx = out_tx;
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines().flatten() {
                if tx.send(Some(line)).is_err() {
                    break;
                }
            }
            let _ = tx.send(None); // signal EOF
        })
    };

    let (err_tx, err_rx) = std::sync::mpsc::channel::<Option<String>>();
    let err_thread = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines().flatten() {
            if err_tx.send(Some(line)).is_err() {
                break;
            }
        }
        let _ = err_tx.send(None);
    });

    Ok(ReplSession {
        stdin: std::io::BufWriter::new(stdin),
        stdout_rx: out_rx,
        stderr_rx: err_rx,
        _stdout_thread: out_thread,
        _stderr_thread: err_thread,
    })
}

/// Build the code wrapper that surrounds user code with a sentinel so we know
/// when execution is complete. We base64-encode the code to avoid any escaping
/// issues regardless of what the code contains.
pub(crate) fn wrap_code_with_sentinel(code: &str, language: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(code.as_bytes());
    match language {
        "python" => format!(
            "import base64 as __b64, sys as __sys\n\
             try:\n\
             \texec(__b64.b64decode(b'{encoded}').decode())\n\
             except Exception as __e:\n\
             \tprint(f'[repl error] {{__e}}', file=__sys.stderr)\n\
             print('{REPL_SENTINEL}')\n"
        ),
        "node" => format!(
            "try{{eval(Buffer.from('{encoded}','base64').toString())}}catch(e){{process.stderr.write('[repl error] '+String(e)+'\\n')}};process.stdout.write('{REPL_SENTINEL}\\n');\n"
        ),
        _ /* bash */ => format!(
            "{}\necho '{}'\n",
            code, REPL_SENTINEL
        ),
    }
}

pub(crate) fn run_code_in_session(
    session: &mut ReplSession,
    code: &str,
    language: &str,
) -> Result<String, String> {
    use std::io::Write;

    let wrapped = wrap_code_with_sentinel(code, language);
    session
        .stdin
        .write_all(wrapped.as_bytes())
        .map_err(|e| format!("Failed to write to {language} REPL stdin: {e}"))?;
    session
        .stdin
        .flush()
        .map_err(|e| format!("Failed to flush {language} REPL stdin: {e}"))?;

    let deadline = web_time::Instant::now() + std::time::Duration::from_secs(REPL_TIMEOUT_SECS);
    let mut stdout_lines: Vec<String> = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(web_time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "REPL timed out after {REPL_TIMEOUT_SECS}s. The code may be in an infinite loop."
            ));
        }

        match session.stdout_rx.recv_timeout(remaining) {
            Ok(Some(line)) => {
                // Strip Python REPL prompts before comparing / storing
                let stripped = line
                    .trim_start_matches(">>> ")
                    .trim_start_matches("... ")
                    .to_string();
                if stripped == REPL_SENTINEL {
                    break;
                }
                stdout_lines.push(stripped);
            }
            Ok(None) => {
                return Err("REPL interpreter exited unexpectedly. Session evicted.".to_string());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!("REPL timed out after {REPL_TIMEOUT_SECS}s."));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("REPL stdout channel disconnected. Session evicted.".to_string());
            }
        }
    }

    // Drain any buffered stderr (non-blocking)
    let mut stderr_lines: Vec<String> = Vec::new();
    loop {
        match session.stderr_rx.try_recv() {
            Ok(Some(line)) if !line.trim().is_empty() => stderr_lines.push(line),
            _ => break,
        }
    }

    // Trim trailing empty lines from stdout
    while stdout_lines
        .last()
        .map(|l: &String| l.trim().is_empty())
        .unwrap_or(false)
    {
        stdout_lines.pop();
    }

    let mut result = stdout_lines.join("\n");
    if !stderr_lines.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr_lines.join("\n"));
    }
    if result.is_empty() {
        result = "(no output)".to_string();
    }
    Ok(result)
}

pub(crate) fn exec_repl(args: &serde_json::Value) -> (String, bool) {
    let code = match str_arg(args, "code") {
        Ok(c) => c,
        Err(e) => return (e, true),
    };
    let language = args["language"].as_str().unwrap_or("python");
    let key = (current_pane(), language.to_string());

    // Get-or-create the session for this pane+language (brief lock on the map).
    let session_arc = {
        let mut map = REPL_SESSIONS.lock().unwrap();
        match map.get(&key) {
            Some(arc) => Arc::clone(arc),
            None => match spawn_repl_session(language) {
                Err(e) => return (e, true),
                Ok(session) => {
                    let arc = Arc::new(std::sync::Mutex::new(session));
                    map.insert(key.clone(), Arc::clone(&arc));
                    arc
                }
            },
        }
    };

    // Lock the specific session (may take up to REPL_TIMEOUT_SECS) — the map
    // lock is already released so other languages can proceed concurrently.
    let mut session = match session_arc.lock() {
        Ok(s) => s,
        Err(_) => return ("REPL session lock poisoned.".to_string(), true),
    };

    match run_code_in_session(&mut *session, code, language) {
        Ok(output) => (output, false),
        Err(e) => {
            // Evict the dead session so the next call spawns a fresh interpreter.
            drop(session); // release before locking the map
            REPL_SESSIONS.lock().unwrap().remove(&key);
            (
                format!(
                    "{e}\nSession has been reset. The next call will start a fresh interpreter."
                ),
                true,
            )
        }
    }
}

// ── sleep ─────────────────────────────────────────────────────────────────────
// Non-blocking agent sleep. Prefer this over run_command("sleep N") — it doesn't
// hold a shell process open and signals intent clearly in tool logs.
