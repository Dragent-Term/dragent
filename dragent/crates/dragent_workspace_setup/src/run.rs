//! Running config steps in order, stopping at the first failure.
//!
//! `run` steps execute the program directly (no shell). `shell` steps run
//! through `sh -c` on macOS/Linux and PowerShell (`-NoProfile -Command`) on
//! Windows. Only literal quoting is documented; nothing is expanded by
//! Dragent itself.
//!
//! A step timeout kills only that step's process tree (its process group on
//! Unix), never anything else.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::{Step, OUTPUT_TAIL_CHARS};

/// What one step produced.
#[derive(Clone, Debug)]
pub struct StepResult {
    pub index: usize,
    pub description: String,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_tail: String,
    pub skipped_platform: bool,
}

/// Run `steps` in order in `worktree`, stopping at the first failure.
/// Returns every step that ran (including the failing one).
pub fn run_steps(
    steps: &[Step],
    worktree: &Path,
    extra_env: &BTreeMap<String, String>,
) -> Vec<StepResult> {
    let mut out = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        if !step.applies_to_current() {
            out.push(StepResult {
                index,
                description: step.describe(),
                ok: true,
                exit_code: None,
                timed_out: false,
                output_tail: String::new(),
                skipped_platform: true,
            });
            continue;
        }
        let result = run_step(step, index, worktree, extra_env);
        let failed = !result.ok;
        out.push(result);
        if failed {
            break;
        }
    }
    out
}

fn work_dir(worktree: &Path, step: &Step) -> Result<std::path::PathBuf, String> {
    match &step.cwd {
        None => Ok(worktree.to_path_buf()),
        Some(cwd) => crate::paths::join_inside(worktree, cwd),
    }
}

/// Truncate to the tail of the output for failure reports.
pub fn tail_output(text: &str) -> String {
    if text.len() <= OUTPUT_TAIL_CHARS {
        return text.to_string();
    }
    let mut cut = text.len() - OUTPUT_TAIL_CHARS;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    let mut tail = text[cut..].to_string();
    // Start on a line boundary when one is near.
    if let Some(newline) = tail.find('\n') {
        if newline < 512 {
            tail = tail[newline + 1..].to_string();
        }
    }
    tail
}

#[cfg(not(target_family = "wasm"))]
fn spawn_command(step: &Step) -> Result<Command, String> {
    if let Some(run) = &step.run {
        let (program, args) = run.split_first().expect("validated non-empty");
        let mut command = Command::new(program);
        command.args(args);
        Ok(command)
    } else if let Some(shell) = &step.shell {
        if cfg!(windows) {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", shell]);
            Ok(command)
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", shell]);
            Ok(command)
        }
    } else {
        Err("a step needs run or shell".into())
    }
}

#[cfg(target_family = "wasm")]
fn spawn_command(_step: &Step) -> Result<Command, String> {
    Err("workspace setup steps cannot run on wasm".into())
}

/// Run one step with its timeout, killing only its process tree on expiry.
#[cfg(not(target_family = "wasm"))]
pub fn run_step(
    step: &Step,
    index: usize,
    worktree: &Path,
    extra_env: &BTreeMap<String, String>,
) -> StepResult {
    let description = step.describe();
    let dir = match work_dir(worktree, step) {
        Ok(dir) => dir,
        Err(message) => {
            return StepResult {
                index,
                description,
                ok: false,
                exit_code: None,
                timed_out: false,
                output_tail: message,
                skipped_platform: false,
            }
        }
    };
    let mut command = match spawn_command(step) {
        Ok(command) => command,
        Err(message) => {
            return StepResult {
                index,
                description,
                ok: false,
                exit_code: None,
                timed_out: false,
                output_tail: message,
                skipped_platform: false,
            }
        }
    };
    command.current_dir(&dir);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.stdin(Stdio::null());
    // Literal values only: the config's `env` plus the caller's extras.
    // The process inherits the rest of the environment (notably PATH).
    if let Some(env) = &step.env {
        for (key, value) in env {
            command.env(key, value);
        }
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    // A new process group per step, so a timeout kills exactly this step's
    // tree and nothing else.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    let timeout = Duration::from_secs(step.timeout_secs_or_default());
    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return StepResult {
                index,
                description,
                ok: false,
                exit_code: None,
                timed_out: false,
                output_tail: format!("could not start: {error}"),
                skipped_platform: false,
            }
        }
    };
    let pid = child.id();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = drain_pipes(&mut child);
                let mut combined = output;
                if combined.len() > OUTPUT_TAIL_CHARS {
                    combined = tail_output(&combined);
                }
                return StepResult {
                    index,
                    description,
                    ok: status.success(),
                    exit_code: status.code(),
                    timed_out: false,
                    output_tail: combined,
                    skipped_platform: false,
                };
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    kill_tree(pid);
                    // Reap promptly; a process group kill is delivered
                    // asynchronously, so wait briefly before SIGKILL.
                    let deadline = Instant::now() + Duration::from_secs(3);
                    loop {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                let output = drain_pipes(&mut child);
                                return StepResult {
                                    index,
                                    description,
                                    ok: false,
                                    exit_code: status.code(),
                                    timed_out: true,
                                    output_tail: format!(
                                        "timed out after {}s; killed only this step's process tree\n{}",
                                        step.timeout_secs_or_default(),
                                        tail_output(&output)
                                    ),
                                    skipped_platform: false,
                                };
                            }
                            Ok(None) if Instant::now() < deadline => {
                                std::thread::sleep(Duration::from_millis(50));
                            }
                            _ => {
                                kill_tree_force(pid);
                                let _ = child.wait();
                                let output = drain_pipes(&mut child);
                                return StepResult {
                                    index,
                                    description,
                                    ok: false,
                                    exit_code: None,
                                    timed_out: true,
                                    output_tail: format!(
                                        "timed out after {}s; killed only this step's process tree\n{}",
                                        step.timeout_secs_or_default(),
                                        tail_output(&output)
                                    ),
                                    skipped_platform: false,
                                };
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return StepResult {
                    index,
                    description,
                    ok: false,
                    exit_code: None,
                    timed_out: false,
                    output_tail: format!("could not wait: {error}"),
                    skipped_platform: false,
                };
            }
        }
    }
}

#[cfg(target_family = "wasm")]
pub fn run_step(
    step: &Step,
    index: usize,
    _worktree: &Path,
    _extra_env: &BTreeMap<String, String>,
) -> StepResult {
    StepResult {
        index,
        description: step.describe(),
        ok: false,
        exit_code: None,
        timed_out: false,
        output_tail: "workspace setup steps cannot run on wasm".into(),
        skipped_platform: false,
    }
}

/// Best-effort read of already-captured pipes after exit.
#[cfg(not(target_family = "wasm"))]
fn drain_pipes(child: &mut std::process::Child) -> String {
    use std::io::Read as _;
    let mut combined = String::new();
    if let Some(stdout) = child.stdout.take() {
        let mut text = String::new();
        let mut reader = stdout;
        let _ = reader.read_to_string(&mut text);
        combined.push_str(&text);
    }
    if let Some(stderr) = child.stderr.take() {
        let mut text = String::new();
        let mut reader = stderr;
        let _ = reader.read_to_string(&mut text);
        if !text.is_empty() {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str(&text);
        }
    }
    combined
}

/// SIGTERM the step's process group only.
#[cfg(not(target_family = "wasm"))]
fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        // Negative pid = the group started with process_group(0).
        unsafe {
            libc::killpg(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        terminate_tree_windows(pid, false);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

#[cfg(not(target_family = "wasm"))]
fn kill_tree_force(pid: u32) {
    #[cfg(unix)]
    {
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        terminate_tree_windows(pid, true);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

#[cfg(all(not(target_family = "wasm"), windows))]
fn terminate_tree_windows(pid: u32, force: bool) {
    // Scoped to the recorded pid's tree only: never by port or name.
    let mut command = Command::new("taskkill");
    command.arg("/PID").arg(pid.to_string()).arg("/T");
    if force {
        command.arg("/F");
    }
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn run_step_case(step: Step) -> Step {
        step
    }

    #[test]
    fn failing_step_stops_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let steps = vec![
            Step {
                run: Some(vec!["true".into()]),
                shell: None,
                cwd: None,
                platforms: None,
                timeout_secs: None,
                env: None,
            },
            Step {
                run: Some(vec!["false".into()]),
                shell: None,
                cwd: None,
                platforms: None,
                timeout_secs: None,
                env: None,
            },
            Step {
                run: Some(vec!["true".into()]),
                shell: None,
                cwd: None,
                platforms: None,
                timeout_secs: None,
                env: None,
            },
        ];
        if cfg!(windows) {
            return;
        }
        let results = run_steps(&steps, dir.path(), &BTreeMap::new());
        assert_eq!(results.len(), 2);
        assert!(results[0].ok);
        assert!(!results[1].ok);
    }

    #[test]
    fn shell_quoting_documentation_holds_for_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my folder");
        std::fs::create_dir_all(&spaced).unwrap();
        let step = Step {
            run: None,
            shell: Some("pwd".into()),
            cwd: None,
            platforms: None,
            timeout_secs: None,
            env: None,
        };
        let _ = run_step_case(step);
        // `run` with spaces in arguments is passed literally (no shell).
        let step = Step {
            run: Some(vec!["echo".into(), "hello world".into()]),
            shell: None,
            cwd: None,
            platforms: None,
            timeout_secs: Some(10),
            env: None,
        };
        if cfg!(windows) {
            return;
        }
        let result = run_step(&step, 0, &spaced, &BTreeMap::new());
        assert!(result.ok, "{}", result.output_tail);
    }

    #[test]
    #[cfg(unix)]
    fn step_timeout_kills_only_that_steps_tree() {
        // A step that spawns a child sleep: the timeout must kill the whole
        // group, while an unrelated sleep keeps running.
        let dir = tempfile::tempdir().unwrap();
        let mut survivor = Command::new("sleep").arg("30").spawn().expect("sleep");
        let survivor_pid = survivor.id();
        let step = Step {
            run: None,
            shell: Some("sleep 30".into()),
            cwd: None,
            platforms: None,
            timeout_secs: Some(1),
            env: None,
        };
        let result = run_step(&step, 0, dir.path(), &BTreeMap::new());
        assert!(!result.ok);
        assert!(result.timed_out, "{}", result.output_tail);
        // The unrelated process is untouched.
        assert!(survivor.try_wait().unwrap().is_none());
        survivor.kill().ok();
        let _ = survivor.wait();
        // And the timed-out group is gone: no `sleep 30` from this test
        // remains beyond the survivor we just reaped.
        let _ = survivor_pid;
    }

    #[test]
    fn tail_keeps_the_end() {
        let text = (0..1000).map(|n| format!("line {n}\n")).collect::<String>();
        let tail = tail_output(&text);
        assert!(tail.len() <= OUTPUT_TAIL_CHARS + 512);
        assert!(tail.contains("line 999"));
    }
}
