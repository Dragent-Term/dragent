//! High-level init / launch / cleanup used by the CLI and the automatic
//! new-worktree hook.
//!
//! `run_init` checks `env_required` first (naming the missing ones, never
//! printing values), then applies `copy`, then runs `init` steps in order.
//! `launch_profile` runs a profile's setup steps and leaves its last step
//! running in the background, recorded with pid and start time. `run_cleanup`
//! stops recorded launches for the worktree, then runs `cleanup` steps.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{missing_env, WorkspaceConfig};
use crate::copy::{apply_copy, CopyAction};
use crate::ready::wait_for_ready;
use crate::registry::{self, InitRecord, LaunchRecord, ReadyState};
use crate::run::{run_steps, StepResult};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// What `setup check` shows: the plan for this platform.
#[derive(Clone, Debug)]
pub struct CheckPlan {
    pub worktree: String,
    pub has_config: bool,
    pub init_steps: Vec<PlannedStep>,
    pub missing_env: Vec<String>,
    pub copy: Vec<String>,
    pub launch_profiles: Vec<String>,
    pub cleanup_steps: usize,
}

#[derive(Clone, Debug)]
pub struct PlannedStep {
    pub index: usize,
    pub description: String,
    pub cwd: Option<String>,
    pub timeout_secs: u64,
    pub platforms: Option<Vec<String>>,
}

/// Build the plan for this platform without running anything.
/// Never creates files.
pub fn check_plan(worktree: &Path, config: Option<&WorkspaceConfig>) -> CheckPlan {
    let Some(config) = config else {
        return CheckPlan {
            worktree: worktree.display().to_string(),
            has_config: false,
            init_steps: Vec::new(),
            missing_env: Vec::new(),
            copy: Vec::new(),
            launch_profiles: Vec::new(),
            cleanup_steps: 0,
        };
    };
    let init_steps = config
        .init_plan()
        .into_iter()
        .map(|(index, step)| PlannedStep {
            index,
            description: step.describe(),
            cwd: step.cwd.clone(),
            timeout_secs: step.timeout_secs_or_default(),
            platforms: step.platforms.clone(),
        })
        .collect();
    let mut launch_profiles: Vec<String> = config.launch.keys().cloned().collect();
    launch_profiles.sort();
    CheckPlan {
        worktree: worktree.display().to_string(),
        has_config: true,
        init_steps,
        missing_env: missing_env(&config.env_required),
        copy: config.copy.iter().map(|entry| entry.to.clone()).collect(),
        launch_profiles,
        cleanup_steps: config.cleanup.len(),
    }
}

/// Run init: env gate, copy, then steps. Records the outcome for
/// `setup status` when `state_dir` is given.
pub fn run_init(
    config: &WorkspaceConfig,
    worktree: &Path,
    main_checkout: Option<&Path>,
    state_dir: Option<&Path>,
) -> InitOutcome {
    let missing = missing_env(&config.env_required);
    if !missing.is_empty() {
        let outcome = InitOutcome {
            ok: false,
            steps: Vec::new(),
            copied: Vec::new(),
            missing_env: missing.clone(),
            failed_early: true,
        };
        if let Some(state_dir) = state_dir {
            let _ = registry::record_init(
                state_dir,
                &InitRecord {
                    worktree: worktree.display().to_string(),
                    at_ms: now_ms(),
                    ok: false,
                    steps_run: 0,
                    steps_skipped: 0,
                    failed_step: None,
                    failed_description: None,
                    exit_code: None,
                    timed_out: false,
                    output_tail: format!("missing required environment: {}", missing.join(", ")),
                    missing_env: missing,
                    copied: Vec::new(),
                },
            );
        }
        return outcome;
    }
    let copies = apply_copy(&config.copy, worktree, main_checkout);
    let copied: Vec<String> = copies
        .iter()
        .filter(|result| {
            matches!(
                result.action,
                CopyAction::Copied | CopyAction::CreatedFromExample
            )
        })
        .map(|result| result.to.clone())
        .collect();
    let steps = run_steps(&config.init, worktree, &BTreeMap::new());
    let ok = steps.iter().all(|step| step.ok);
    let (failed_step, failed_description, exit_code, timed_out, output_tail) = steps
        .iter()
        .find(|step| !step.ok)
        .map(|step| {
            (
                Some(step.index),
                Some(step.description.clone()),
                step.exit_code,
                step.timed_out,
                step.output_tail.clone(),
            )
        })
        .unwrap_or((None, None, None, false, String::new()));
    let skipped = steps.iter().filter(|step| step.skipped_platform).count();
    if let Some(state_dir) = state_dir {
        let _ = registry::record_init(
            state_dir,
            &InitRecord {
                worktree: worktree.display().to_string(),
                at_ms: now_ms(),
                ok,
                steps_run: steps.len(),
                steps_skipped: skipped,
                failed_step,
                failed_description,
                exit_code,
                timed_out,
                output_tail: output_tail.clone(),
                missing_env: Vec::new(),
                copied: copied.clone(),
            },
        );
    }
    InitOutcome {
        ok,
        steps,
        copied,
        missing_env: Vec::new(),
        failed_early: false,
    }
}

pub struct InitOutcome {
    pub ok: bool,
    pub steps: Vec<StepResult>,
    pub copied: Vec<String>,
    pub missing_env: Vec<String>,
    pub failed_early: bool,
}

/// Launch a profile: run its non-final steps to completion, then start its
/// last step in the background and record it. Returns the record.
#[cfg(not(target_family = "wasm"))]
pub fn launch_profile(
    config: &WorkspaceConfig,
    profile_name: &str,
    worktree: &Path,
    state_dir: &Path,
) -> Result<LaunchResult, String> {
    let profile = config
        .launch
        .get(profile_name)
        .ok_or_else(|| format!("unknown launch profile \"{profile_name}\""))?;
    if profile.steps.is_empty() {
        return Err("a launch profile needs at least one step".into());
    }
    // Setup steps (all but the last) run to completion first.
    let (setup, last) = profile.steps.split_at(profile.steps.len() - 1);
    let setup_results = run_steps(setup, worktree, &BTreeMap::new());
    if let Some(failed) = setup_results.iter().find(|step| !step.ok) {
        return Err(format!(
            "launch setup step {} failed ({}): {}",
            failed.index,
            failed.description,
            crate::run::tail_output(&failed.output_tail)
        ));
    }
    let last_step = &last[0];
    if !last_step.applies_to_current() {
        return Err(format!(
            "launch profile \"{profile_name}\" does not run on {}",
            crate::platform::current()
        ));
    }
    let log_path = registry::log_path_for(state_dir, worktree, profile_name);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create log dir: {error}"))?;
    }
    let log_file = std::fs::File::create(&log_path)
        .map_err(|error| format!("could not create log {}: {error}", log_path.display()))?;
    let dir = match &last_step.cwd {
        None => worktree.to_path_buf(),
        Some(cwd) => crate::paths::join_inside(worktree, cwd)?,
    };
    let mut command = build_launch_command(last_step)?;
    command.current_dir(&dir);
    command.stdout(
        log_file
            .try_clone()
            .map_err(|error| format!("log: {error}"))?,
    );
    command.stderr(log_file);
    command.stdin(std::process::Stdio::null());
    if let Some(env) = &last_step.env {
        for (key, value) in env {
            command.env(key, value);
        }
    }
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
    let child = command
        .spawn()
        .map_err(|error| format!("could not start launch: {error}"))?;
    let pid = child.id();
    // Detach: the record owns the process, not this call.
    std::mem::forget(child);
    let start_time = registry::process_start_time(pid).unwrap_or_else(|| {
        // The process exited immediately or the table hasn't settled:
        // poll briefly before giving up the start time.
        let mut found = None;
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if let Some(start) = registry::process_start_time(pid) {
                found = Some(start);
                break;
            }
        }
        found.unwrap_or(0)
    });
    // The ready probe runs after the record exists, so `status` sees the
    // launch even while its service is still starting.
    let record = LaunchRecord {
        profile: profile_name.to_string(),
        worktree: worktree.display().to_string(),
        pid,
        start_time,
        log_path: log_path.display().to_string(),
        started_at_ms: now_ms(),
        command: last_step.describe(),
        ready: None,
    };
    registry::record_launch(state_dir, &record)?;
    let ready = profile.ready.as_ref().map(wait_for_ready);
    let mut record = record;
    if let Some(ready) = &ready {
        record.ready = Some(ReadyState {
            ready: ready.ready,
            detail: ready.detail.clone(),
        });
        let _ = registry::record_launch(state_dir, &record);
    }
    Ok(LaunchResult {
        record,
        setup_results,
        ready,
    })
}

#[cfg(target_family = "wasm")]
pub fn launch_profile(
    _config: &WorkspaceConfig,
    _profile_name: &str,
    _worktree: &Path,
    _state_dir: &Path,
) -> Result<LaunchResult, String> {
    Err("launch profiles cannot run on wasm".into())
}

#[cfg(not(target_family = "wasm"))]
fn build_launch_command(step: &crate::config::Step) -> Result<std::process::Command, String> {
    if let Some(run) = &step.run {
        let (program, args) = run.split_first().expect("validated non-empty");
        let mut command = std::process::Command::new(program);
        command.args(args);
        Ok(command)
    } else if let Some(shell) = &step.shell {
        if cfg!(windows) {
            let mut command = std::process::Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", shell]);
            Ok(command)
        } else {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", shell]);
            Ok(command)
        }
    } else {
        Err("a step needs run or shell".into())
    }
}

pub struct LaunchResult {
    pub record: LaunchRecord,
    pub setup_results: Vec<StepResult>,
    pub ready: Option<crate::ready::ReadyResult>,
}

/// Stop one recorded launch and forget it. Never signals anything that is
/// not the recorded process.
pub fn stop_launch(
    state_dir: &Path,
    worktree: &Path,
    profile: &str,
) -> Result<StopOutcome, String> {
    let key = worktree.display().to_string();
    let canonical = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf())
        .display()
        .to_string();
    let launches = registry::list_launches(state_dir, None);
    let record = launches
        .iter()
        .find(|record| {
            (record.worktree == key || record.worktree == canonical) && record.profile == profile
        })
        .cloned()
        .ok_or_else(|| format!("no running {profile} launch in this worktree"))?;
    match registry::stop_recorded(&record) {
        Ok(stopped) => {
            let _ = registry::remove_launch(state_dir, &record.worktree, &record.profile);
            Ok(StopOutcome {
                stopped,
                pid: record.pid,
            })
        }
        Err(message) => {
            // A reused pid is left alone; drop the stale record so it does
            // not block a fresh launch.
            let _ = registry::remove_launch(state_dir, &record.worktree, &record.profile);
            Err(message)
        }
    }
}

pub struct StopOutcome {
    pub stopped: bool,
    pub pid: u32,
}

/// Run `cleanup` steps after stopping this worktree's launches.
pub fn run_cleanup(config: &WorkspaceConfig, worktree: &Path, state_dir: &Path) -> CleanupOutcome {
    let launches = registry::list_launches(state_dir, Some(worktree));
    let mut stopped = Vec::new();
    for record in &launches {
        match registry::stop_recorded(record) {
            Ok(true) => stopped.push(record.profile.clone()),
            Ok(false) => {}
            Err(_) => {}
        }
        let _ = registry::remove_launch(state_dir, &record.worktree, &record.profile);
    }
    let steps = run_steps(&config.cleanup, worktree, &BTreeMap::new());
    let ok = steps.iter().all(|step| step.ok);
    CleanupOutcome { ok, stopped, steps }
}

pub struct CleanupOutcome {
    pub ok: bool,
    pub stopped: Vec<String>,
    pub steps: Vec<StepResult>,
}
