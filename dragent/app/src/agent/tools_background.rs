//! Background tasks: completions, running commands, IO threads.
//!
//! Moved verbatim; same behavior.

//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use super::tools_command::*;
use super::tools_files::*;
use super::tools_mcp_auth::*;
use super::tools_search::*;
use super::tools_shell::*;
use super::tools_subagent::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
/// Background completion record — single definition in
/// `dragent_agent::background` shared by the engine core and the Host.
pub use dragent_agent::background::BackgroundCompletion;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) static COMPLETION_SENDERS: Lazy<
    std::sync::Mutex<Vec<(Uuid, std::sync::mpsc::SyncSender<BackgroundCompletion>)>>,
> = Lazy::new(|| std::sync::Mutex::new(Vec::new()));

/// Per-pane UI wakeups for changes to the running-command registry. The
/// activity strip must repaint because process state changed, independently of
/// whether the model happens to make another tool call.
pub(crate) static ACTIVITY_SENDERS: Lazy<std::sync::Mutex<Vec<(Uuid, async_channel::Sender<()>)>>> =
    Lazy::new(|| std::sync::Mutex::new(Vec::new()));

pub fn register_activity_sender(pane_id: Uuid, tx: async_channel::Sender<()>) {
    if let Ok(mut senders) = ACTIVITY_SENDERS.lock() {
        senders.retain(|(id, sender)| *id != pane_id && !sender.is_closed());
        senders.push((pane_id, tx));
    }
}

pub fn rebind_activity_sender(old_pane_id: Uuid, new_pane_id: Uuid) {
    if old_pane_id == new_pane_id {
        return;
    }
    if let Ok(mut senders) = ACTIVITY_SENDERS.lock() {
        for (id, _) in senders.iter_mut() {
            if *id == old_pane_id {
                *id = new_pane_id;
                return;
            }
        }
    }
}

pub(crate) fn notify_activity(pane_id: Uuid) {
    if let Ok(mut senders) = ACTIVITY_SENDERS.lock() {
        senders.retain(|(_, sender)| !sender.is_closed());
        for (id, sender) in senders.iter() {
            if *id == pane_id {
                let _ = sender.try_send(());
            }
        }
    }
}

/// Register a sender that is notified whenever a background task finishes.
/// The controller calls this once at startup; the sender is cloned into every
/// task that gets backgrounded.
pub fn register_completion_sender(
    pane_id: Uuid,
    tx: std::sync::mpsc::SyncSender<BackgroundCompletion>,
) {
    if let Ok(mut senders) = COMPLETION_SENDERS.lock() {
        // Prune dead senders with a probe (receivers ignore an empty task
        // id). Never a blocking `send`: these channels are bounded and
        // nothing drains them while the window is still being built, so a
        // full one would park the main thread forever (startup freeze,
        // 2026-11-12). Full means the receiver is alive; only a closed
        // channel is dropped.
        senders.retain(|(_, s)| {
            !matches!(
                s.try_send(BackgroundCompletion {
                    task_id: String::new(),
                    command: String::new(),
                    exit_code: None,
                    output: String::new(),
                }),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            )
        });
        senders.push((pane_id, tx));
    }
}

/// Update the pane UUID under which a completion sender is registered.
/// Called by `set_persistence` after the controller learns its final VIEW UUID,
/// which differs from the CTRL UUID generated at construction time.
pub fn rebind_completion_sender(old_pane_id: Uuid, new_pane_id: Uuid) {
    if old_pane_id == new_pane_id {
        return;
    }
    if let Ok(mut senders) = COMPLETION_SENDERS.lock() {
        for (id, _) in senders.iter_mut() {
            if *id == old_pane_id {
                *id = new_pane_id;
                return;
            }
        }
    }
}

pub(crate) fn notify_completion(pane_id: Uuid, completion: BackgroundCompletion) {
    if let Ok(senders) = COMPLETION_SENDERS.lock() {
        for (sid, tx) in senders.iter() {
            if *sid == pane_id {
                let _ = tx.try_send(completion.clone());
            }
        }
    }
}

// ── run_command / manage_task ─────────────────────────────────────────────────

/// Maximum bytes of combined stdout + stderr kept per background task.
pub(crate) const MAX_OUTPUT: usize = 100_000;

/// Return a UTF-8-safe prefix measured in bytes. Tool output is arbitrary
/// user/process text, so byte offsets must never be used directly for a
/// `str` slice or `String::truncate`.
pub(crate) fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Return a UTF-8-safe suffix measured in bytes.
pub(crate) fn utf8_suffix(text: &str, max_bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

pub(crate) fn truncate_utf8(text: &mut String, max_bytes: usize) {
    let safe_len = utf8_prefix(text, max_bytes).len();
    text.truncate(safe_len);
}

pub(crate) struct BackgroundTask {
    pub(crate) command: String,
    pub(crate) stdin: std::sync::Mutex<Option<std::process::ChildStdin>>,
    pub(crate) log: Arc<std::sync::Mutex<String>>,
    pub(crate) done: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) exit_code: Arc<std::sync::Mutex<Option<i32>>>,
    pub(crate) started_at: web_time::Instant,
    /// Session leader pid (child has its own session via setsid), so kill can
    /// take out the whole process tree.
    pub(crate) pid: u32,
    /// False while the agent is still blocked on this command (foreground
    /// window); true once it was handed back as a background task.
    pub(crate) backgrounded: std::sync::atomic::AtomicBool,
    /// Pane that ran this command. UI surfaces (the activity strip) are
    /// per-pane — a command must never leak into another tab's strip.
    pub(crate) pane_id: Uuid,
    /// Working directory the command ran in. Used to dedup "same command in the
    /// same path" re-runs so a re-triggered `npm test` stops the stale instance
    /// instead of stacking a second identical process.
    pub(crate) cwd: Option<std::path::PathBuf>,
}

/// Every agent-run shell command, foreground or backgrounded, keyed by the
/// pipeline call_id when available. This is what the UI's activity strip and
/// live ToolStep rows read — a running command is never invisible.
pub(crate) static BACKGROUND_TASKS: Lazy<std::sync::Mutex<HashMap<String, BackgroundTask>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

/// Bounded snapshot for the conversation UI; never wait on an output writer.
pub(crate) fn agent_command_output(id: &str) -> Option<String> {
    let tasks = BACKGROUND_TASKS.try_lock().ok()?;
    let task = tasks.get(id)?;
    let log = task.log.try_lock().ok()?;
    let tail = utf8_suffix(&log, 32 * 1024);
    Some(if tail.len() < log.len() {
        format!("Earlier output omitted from live preview.\n{tail}")
    } else {
        tail.to_string()
    })
}

/// Point-in-time view of one running agent command, for the UI.
#[derive(Clone, Debug)]
pub struct RunningCommand {
    pub id: String,
    pub command: String,
    pub elapsed_secs: u64,
    pub backgrounded: bool,
    /// Most recent non-empty output line, truncated for display.
    pub last_line: Option<String>,
}

/// All agent commands still running right now (foreground + backgrounded),
/// across every pane. UI callers should use [`running_agent_commands_for`].
pub fn running_agent_commands() -> Vec<RunningCommand> {
    running_commands_filtered(None)
}

/// Commands still running that belong to one pane — the per-tab view.
pub fn running_agent_commands_for(pane_id: Uuid) -> Vec<RunningCommand> {
    running_commands_filtered(Some(pane_id))
}

/// Only the commands the current agent turn is still BLOCKED on (foreground).
/// A backgrounded command (e.g. a dev server) is deliberately excluded: it must
/// not lock the chat UI nor be killed by the agent Stop button. The passive
/// activity strip still shows backgrounded commands with their own per-command
/// cancel — they're visible, just not tied to the global agent Stop.
pub fn foreground_agent_commands_for(pane_id: Uuid) -> Vec<RunningCommand> {
    running_commands_filtered(Some(pane_id))
        .into_iter()
        .filter(|c| !c.backgrounded)
        .collect()
}

pub(crate) fn running_workers_filtered(pane: Option<Uuid>) -> Vec<RunningCommand> {
    let workers = match WORKERS.lock() {
        Ok(w) => w,
        Err(p) => p.into_inner(),
    };
    workers
        .iter()
        .filter(|(_, w)| pane.is_none_or(|p| w.pane_id == p))
        .filter(|(_, w)| !w.done.load(std::sync::atomic::Ordering::Acquire))
        .map(|(id, w)| {
            let mut q = w.query.clone();
            if q.len() > 60 {
                truncate_utf8(&mut q, 60);
                q.push('…');
            }
            RunningCommand {
                id: id.clone(),
                command: format!("worker: {q}"),
                elapsed_secs: w.started_at.elapsed().as_secs(),
                backgrounded: true,
                last_line: None,
            }
        })
        .collect()
}

pub(crate) fn running_commands_filtered(pane: Option<Uuid>) -> Vec<RunningCommand> {
    let tasks = match BACKGROUND_TASKS.lock() {
        Ok(t) => t,
        Err(p) => p.into_inner(),
    };
    let mut out: Vec<RunningCommand> = tasks
        .iter()
        .filter(|(_, t)| pane.is_none_or(|p| t.pane_id == p))
        .filter(|(_, t)| !t.done.load(std::sync::atomic::Ordering::Acquire))
        .map(|(id, t)| {
            let last_line = t.log.lock().ok().and_then(|b| {
                b.lines().rev().find(|l| !l.trim().is_empty()).map(|l| {
                    let mut s = l.trim().to_string();
                    if s.len() > 120 {
                        truncate_utf8(&mut s, 120);
                        s.push('…');
                    }
                    s
                })
            });
            RunningCommand {
                id: id.clone(),
                command: t.command.clone(),
                elapsed_secs: t.started_at.elapsed().as_secs(),
                backgrounded: t.backgrounded.load(std::sync::atomic::Ordering::Relaxed),
                last_line,
            }
        })
        .collect();
    out.extend(running_workers_filtered(pane));
    out.sort_by(|a, b| b.elapsed_secs.cmp(&a.elapsed_secs));
    out
}

/// Kill a running agent command and its whole process tree. Returns true if
/// the task existed and a kill was attempted. The normal completion pipeline
/// (wait thread → done flag → watcher → notify_completion) handles the rest,
/// so the agent still gets resumed with the tail of the output.
pub fn kill_command(task_id: &str) -> bool {
    // Worker sub-agents first: cancellation abandons the stream (its thread
    // ends with the task) and suppresses the completion notification.
    if let Ok(mut workers) = WORKERS.lock() {
        if let Some(w) = workers.remove(task_id) {
            let pane_id = w.pane_id;
            w.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            w.done.store(true, std::sync::atomic::Ordering::Release);
            log::info!("[worker] {task_id} cancelled by user");
            drop(workers);
            notify_activity(pane_id);
            return true;
        }
    }
    let (pid, log) = {
        let tasks = match BACKGROUND_TASKS.lock() {
            Ok(t) => t,
            Err(p) => p.into_inner(),
        };
        let Some(task) = tasks.get(task_id) else {
            return false;
        };
        if task.done.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        (task.pid, task.log.clone())
    };
    if let Ok(mut b) = log.lock() {
        b.push_str(
            "
[process killed]
",
        );
    }
    #[cfg(unix)]
    unsafe {
        // The child is a session leader (setsid), so its pgid == pid; negative
        // pid signals the entire group.
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .no_window()
            .output();
    }
    log::info!("[run_command] task {task_id} (pid {pid}) killed");
    true
}

/// Stop any still-running agent command that is byte-for-byte the SAME command
/// in the SAME pane and working directory before starting a new one. Without this, an
/// agent that re-triggers a command (e.g. `npm test`) while the previous run is
/// still going would stack a second identical process — and quickly a pile of
/// them. Pane identity is part of the key: a command started in one tab must
/// never kill an independently managed process in another tab.
pub(crate) fn stop_duplicate_running(
    command: &str,
    cwd: &Option<std::path::PathBuf>,
    pane_id: Uuid,
) {
    // Collect ids first, then kill OUTSIDE the lock — kill_command locks
    // BACKGROUND_TASKS itself, so holding it here would deadlock.
    let dupes: Vec<String> = {
        let tasks = match BACKGROUND_TASKS.lock() {
            Ok(t) => t,
            Err(p) => p.into_inner(),
        };
        tasks
            .iter()
            .filter(|(_, t)| {
                t.command == command
                    && &t.cwd == cwd
                    && t.pane_id == pane_id
                    && !t.done.load(std::sync::atomic::Ordering::Acquire)
            })
            .map(|(id, _)| id.clone())
            .collect()
    };
    if dupes.is_empty() {
        return;
    }
    for id in &dupes {
        log::info!("[run_command] stopping duplicate of `{command}` (task {id}) before re-running");
        kill_command(id);
    }
    // Give the killed process(es) a moment to release resources (ports, file
    // locks) before the replacement spawns.
    std::thread::sleep(std::time::Duration::from_millis(150));
}

pub(crate) fn spawn_child(
    command: &str,
    effective_cwd: &Option<std::path::PathBuf>,
) -> Result<std::process::Child, String> {
    // Use the same shell precedence as the interactive terminal:
    // Settings default_shell > $SHELL > resolve_shell() fallback.
    let settings_shell = crate::console::editor_settings::EditorSettings::global()
        .terminal
        .default_shell
        .clone();
    let shell = dragent_pty::resolve_shell(settings_shell.or_else(|| std::env::var("SHELL").ok()));
    let mut cmd = Command::new(&shell);
    cmd.no_window();

    // Use the correct invocation flag for the detected shell.
    #[cfg(windows)]
    {
        let lower = shell.to_ascii_lowercase();
        if lower.contains("powershell") || lower.contains("pwsh") {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        } else if lower.contains("cmd") {
            cmd.args(["/c", command]);
        } else {
            // Git Bash or other POSIX-like shell on Windows
            cmd.arg("-c").arg(command);
        }
    }
    #[cfg(not(windows))]
    {
        cmd.arg("-c").arg(command);
    }

    dragent_pty::configure_std_shell_command(&mut cmd, &shell);
    cmd.env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("TERM", "dumb");
    if let Some(ref dir) = effective_cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Give each child its own session so background processes (dev servers, etc.)
    // are not counted in dragent's process group by the OS memory pressure manager.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()
        .map_err(|e| format!("Failed to spawn '{command}': {e}"))
}

pub(crate) fn start_io_threads(
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
    log: Arc<std::sync::Mutex<String>>,
    done: Arc<std::sync::atomic::AtomicBool>,
    exit_code: Arc<std::sync::Mutex<Option<i32>>>,
    mut child: std::process::Child,
    verification: Option<crate::agent::host_verification::Completion>,
) -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (drained_tx, drained_rx) = std::sync::mpsc::channel::<()>();
    {
        let log = log.clone();
        let drained_tx = drained_tx.clone();
        crate::bg::detach(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines().flatten() {
                let mut b = log.lock().unwrap();
                if b.len() < MAX_OUTPUT {
                    b.push_str(&line);
                    b.push('\n');
                }
            }
            let _ = drained_tx.send(());
        });
    }
    {
        let log = log.clone();
        crate::bg::detach(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines().flatten() {
                let mut b = log.lock().unwrap();
                if b.len() < MAX_OUTPUT {
                    b.push_str(&line);
                    b.push('\n');
                }
            }
            let _ = drained_tx.send(());
        });
    }
    crate::bg::detach(move || {
        match child.wait() {
            Ok(status) => {
                *exit_code.lock().unwrap() = status.code();
            }
            Err(e) => {
                log::warn!("[run_command] wait error: {e}");
            }
        }
        if let Some(record) = verification {
            // Drain short-lived commands before snapshotting their output.
            // Descendants can retain pipes indefinitely: never block on them.
            for _ in 0..2 {
                let _ = drained_rx.recv_timeout(std::time::Duration::from_secs(1));
            }
            record(*exit_code.lock().unwrap(), log.lock().unwrap().clone());
        }
        done.store(true, std::sync::atomic::Ordering::Release);
        let _ = tx.send(());
    });
    rx
}

/// Foreground window before a command is automatically backgrounded — both the
/// default (when the model omits `wait_ms_before_async`) and the HARD CAP on
/// whatever the model requests. Long enough that a normal build/test/install
/// finishes in-turn (stays foreground); short enough that a never-ending
/// process (a dev server the model mis-judged as synchronous) can't lock the
/// turn indefinitely — past this it backgrounds, the turn ends, the input
/// unlocks, and the job keeps running quietly. Backgrounding does NOT kill it;
/// completion auto-resumes the agent. The model is still told to set this
/// "explicitly low" for known long-runners (dev servers) so they detach sooner.
pub(crate) const AUTO_BACKGROUND_AFTER_MS: u64 = 90_000;

/// Optional hard wall-clock cap on a BACKGROUNDED agent command. `None` by
/// default: a detached process (a dev server, a watch mode) runs until it exits,
/// the user kills it (activity strip / block), or the pane/window closes — the
/// terminal-native behavior, matching how background shells work elsewhere.
/// Set DRAGENT_CMD_TIMEOUT_SECS to opt into a backstop that SIGKILLs a runaway
/// after that many seconds (min 15). Foreground commands are already bounded by
/// AUTO_BACKGROUND_AFTER_MS (they background, not die), so this only affects
/// already-backgrounded ones.
pub(crate) fn command_hard_cap() -> Option<std::time::Duration> {
    std::env::var("DRAGENT_CMD_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|secs| std::time::Duration::from_secs(secs.max(15)))
}

#[cfg(test)]
mod completion_sender_tests {
    use super::*;

    /// Restoring many agent panes at launch registers a sender per pane while
    /// nothing drains the channels yet; a full channel must not block the next
    /// registration (it froze the app on startup).
    #[test]
    fn registering_beside_a_full_channel_never_blocks() {
        let (full_tx, _full_rx) = std::sync::mpsc::sync_channel::<BackgroundCompletion>(1);
        let full_pane = Uuid::new_v4();
        register_completion_sender(full_pane, full_tx);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..3 {
                let (tx, rx) = std::sync::mpsc::sync_channel::<BackgroundCompletion>(1);
                std::mem::forget(rx);
                register_completion_sender(Uuid::new_v4(), tx);
            }
            let _ = done_tx.send(());
        });
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok(),
            "registering a completion sender blocked on a full channel"
        );
        let senders = COMPLETION_SENDERS.lock().unwrap();
        assert!(
            senders.iter().any(|(id, _)| *id == full_pane),
            "a full but live channel was pruned"
        );
    }
}

#[cfg(test)]
mod utf8_cut_tests {
    use super::*;

    #[test]
    fn cuts_never_split_a_multi_byte_character() {
        // "é" is 2 bytes and "🙂" is 4, so byte 3 and byte 6 both land inside one.
        let text = "aé🙂z";
        assert_eq!(utf8_prefix(text, 2), "a");
        assert_eq!(utf8_prefix(text, 3), "aé");
        assert_eq!(utf8_prefix(text, 6), "aé");
        assert_eq!(utf8_prefix(text, 7), "aé🙂");
        assert_eq!(utf8_prefix(text, 100), text);
        assert_eq!(utf8_prefix(text, 0), "");
        assert_eq!(utf8_suffix(text, 2), "z");
        assert_eq!(utf8_suffix(text, 5), "🙂z");

        let mut owned = String::from("ab🙂");
        truncate_utf8(&mut owned, 4);
        assert_eq!(owned, "ab");
    }

    #[test]
    fn every_byte_cut_of_mixed_text_is_safe() {
        let text = "naïve café 🙂 日本語";
        for max in 0..=text.len() + 1 {
            let cut = utf8_prefix(text, max);
            assert!(cut.len() <= max && text.starts_with(cut));
        }
    }
}
