//! `dragent setup`: reproducible repository setup for new workspaces.
//!
//! A repository describes `.dragent/workspace.json` (version 1) with `init`
//! steps, named `launch` profiles, `cleanup` steps, required environment
//! names and explicit `copy` entries. The user-local override
//! `.dragent/workspace.local.json` merges over it key by key.
//!
//! Repository commands run only for projects the person trusted
//! (`WorkspaceTrustStore::is_trusted`); otherwise nothing runs and the
//! result says how to trust the project. Launched processes are recorded
//! with pid and start time, and stopping signals only the recorded process
//! whose start time still matches.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use dragent_host::outcome::{Failure, Status};
use dragent_workspace_setup::config::load_merged;

const HELP: &str = "\
Prepare a repository for reproducible work in new workspaces.

Usage: dragent setup <command>

  check [PATH]              Validate the config and show this platform's plan
  init [PATH]               Copy local files and run init steps in order
  launch PROFILE [PATH]     Run a profile's setup, then start its last step
  status [PATH]             Last init result and running launches
  stop [PROFILE] [PATH]     Stop one launch (or every launch here)
  cleanup [PATH]            Stop launches, then run cleanup steps

PATH defaults to the current directory's repository. Steps run in order and
stop at the first failure, reported with the step, exit code and the tail of
its output. `launch` prints the pid, log path and readiness result. Results
are JSON for agents (--json, the default for agents; --text for people).

Repository commands run only for trusted projects: trust the project in
Dragent (open it and accept the trust prompt), then run again. There is no
flag to bypass this. Secrets stay out of the config: `env_required` names
variables, and `setup check` lists which are missing without printing values.
";

fn agent_environment() -> bool {
    let set = |name: &str| {
        std::env::var_os(name)
            .is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    };
    [
        "CI",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "GEMINI_CLI",
        "QWEN_CODE",
        "DRAGENT_JSON",
    ]
    .iter()
    .any(|name| set(name))
}

fn json_mode(args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--text") {
        return false;
    }
    args.iter().any(|arg| arg == "--json") || agent_environment()
}

fn invalid(code: &str, message: impl Into<String>) -> String {
    Failure::new(Status::InvalidInput, code, message)
        .next("`dragent setup help` shows the commands.")
        .encode()
}

fn state_dir() -> PathBuf {
    dragent_paths::organisation_dir().join("workspace-setup")
}

/// The worktree for an optional PATH argument: the argument itself (or the
/// current directory) resolved to its repository root.
fn resolve_worktree(arg: Option<&str>) -> Result<PathBuf, String> {
    let base = match arg {
        Some(path) => {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            };
            if !path.exists() {
                return Err(invalid(
                    "no_such_path",
                    format!("{} doesn't exist", path.display()),
                ));
            }
            path.canonicalize().unwrap_or(path)
        }
        None => std::env::current_dir().map_err(|error| {
            invalid(
                "no_working_directory",
                format!("could not read the current directory: {error}"),
            )
        })?,
    };
    if !base.is_dir() {
        return Err(invalid(
            "not_a_directory",
            format!("{} isn't a directory", base.display()),
        ));
    }
    // A remote workspace's folder here is an empty stand-in: running its
    // setup on this computer would do the wrong work silently.
    crate::cli::scope::refuse_remote(
        &base,
        "remote_workspace",
        "workspace setup runs only on this computer in this build",
        "Run the repository's own setup commands in a terminal of that workspace (it runs on that machine).",
    )?;
    Ok(dragent_workspace_setup::paths::resolve_worktree(&base))
}

fn main_checkout_of(worktree: &Path) -> Option<PathBuf> {
    dragent_workspace_setup::paths::main_checkout_of(worktree)
}

/// Whether repository commands may run here: the worktree or its main
/// checkout is trusted. Language-server trust settings don't matter.
fn is_trusted_project(worktree: &Path) -> bool {
    let store = crate::workspace::trust_store::WorkspaceTrustStore::load();
    if store.is_trusted(worktree) {
        return true;
    }
    main_checkout_of(worktree).is_some_and(|main| store.is_trusted(&main))
}

fn needs_approval(worktree: &Path) -> String {
    Failure::new(
        Status::NeedsApproval,
        "untrusted_project",
        format!(
            "{} isn't a trusted project, so nothing ran. Open it in Dragent and trust it (the trust prompt), then run again.",
            worktree.display()
        ),
    )
    .next("Trust the project in Dragent first; there is no flag to bypass this.")
    .encode()
}

fn load_config(
    worktree: &Path,
) -> Result<Option<dragent_workspace_setup::WorkspaceConfig>, String> {
    load_merged(worktree).map_err(|error| {
        Failure::new(
            Status::InvalidInput,
            "invalid_config",
            format!("{}: {}", error.path, error.message),
        )
        .next(format!(
            "Fix {} (see `dragent setup check --text`), then run again.",
            error.path
        ))
        .encode()
    })
}

pub fn run(args: &[String]) -> Result<String, String> {
    let json = json_mode(args);
    let words: Vec<&String> = args.iter().filter(|arg| !arg.starts_with("--")).collect();
    let (subcommand, rest) = match words.as_slice() {
        [] => ("help", &[][..]),
        [first, rest @ ..] => (first.as_str(), rest),
    };
    // Flags that take no value for this family.
    for arg in args {
        if arg.starts_with("--")
            && !matches!(
                arg.as_str(),
                "--json" | "--text" | "--quiet" | "--help" | "-h"
            )
        {
            if let Some(name) = arg.strip_prefix("--") {
                if !name.contains('=') {
                    return Err(invalid(
                        "usage",
                        format!("unknown option `--{name}` (see `dragent setup help`)"),
                    ));
                }
            }
        }
    }
    match subcommand {
        "help" | "--help" | "-h" => Ok(HELP.trim_end().to_string()),
        "check" => {
            let path = rest.first().map(|path| path.as_str());
            if rest.len() > 1 {
                return Err(invalid("usage", "usage: dragent setup check [PATH]"));
            }
            let output = check(path)?;
            Ok(render(&output, json))
        }
        "init" => {
            let path = rest.first().map(|path| path.as_str());
            if rest.len() > 1 {
                return Err(invalid("usage", "usage: dragent setup init [PATH]"));
            }
            let output = init(path)?;
            Ok(render(&output, json))
        }
        "launch" => {
            let (profile, path) = match rest {
                [] => {
                    return Err(invalid(
                        "usage",
                        "usage: dragent setup launch PROFILE [PATH]",
                    ))
                }
                [profile] => (profile.as_str(), None),
                [profile, path] => (profile.as_str(), Some(path.as_str())),
                _ => {
                    return Err(invalid(
                        "usage",
                        "usage: dragent setup launch PROFILE [PATH]",
                    ))
                }
            };
            let output = launch(profile, path)?;
            Ok(render(&output, json))
        }
        "status" => {
            let path = rest.first().map(|path| path.as_str());
            if rest.len() > 1 {
                return Err(invalid("usage", "usage: dragent setup status [PATH]"));
            }
            let output = status(path)?;
            Ok(render(&output, json))
        }
        "stop" => {
            let (profile, path) = match rest {
                [] => (None, None),
                [one] => {
                    // One word: a profile of this checkout's config, else a
                    // path when it looks like one, else a profile name.
                    let profile_here = resolve_worktree(None)
                        .ok()
                        .and_then(|worktree| load_config(&worktree).ok().flatten())
                        .is_some_and(|config| config.launch.contains_key(one.as_str()));
                    let looks_like_path = !profile_here
                        && (one.contains('/') || one.contains('\\') || Path::new(one.as_str()).exists());
                    if looks_like_path {
                        (None, Some(one.as_str()))
                    } else {
                        (Some(one.as_str()), None)
                    }
                }
                [profile, path] => (Some(profile.as_str()), Some(path.as_str())),
                _ => {
                    return Err(invalid(
                        "usage",
                        "usage: dragent setup stop [PROFILE] [PATH]",
                    ))
                }
            };
            let output = stop(profile, path)?;
            Ok(render(&output, json))
        }
        "cleanup" => {
            let path = rest.first().map(|path| path.as_str());
            if rest.len() > 1 {
                return Err(invalid("usage", "usage: dragent setup cleanup [PATH]"));
            }
            let output = cleanup(path)?;
            Ok(render(&output, json))
        }
        other => Err(invalid(
            "usage",
            format!("unknown setup command `{other}` (see `dragent setup help`)"),
        )),
    }
}

fn render(value: &Value, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    }
    render_text(value)
}

fn render_text(value: &Value) -> String {
    let mut out = String::new();
    if let Some(worktree) = value.get("worktree").and_then(Value::as_str) {
        out.push_str(&format!("Worktree: {worktree}\n"));
    }
    if let Some(trusted) = value.get("trusted").and_then(Value::as_bool) {
        out.push_str(&format!(
            "Trusted: {}\n",
            if trusted {
                "yes"
            } else {
                "no — open it in Dragent and trust it"
            }
        ));
    }
    if let Some(config) = value.get("has_config").and_then(Value::as_bool) {
        out.push_str(&format!(
            "Config: {}\n",
            if config {
                "yes"
            } else {
                "none (.dragent/workspace.json)"
            }
        ));
    }
    if let Some(steps) = value.get("init_steps").and_then(Value::as_array) {
        out.push_str(&format!("Init steps ({}):\n", steps.len()));
        for step in steps {
            out.push_str(&format!(
                "  [{}] {}\n",
                step["index"].as_u64().unwrap_or(0),
                step["description"].as_str().unwrap_or("-")
            ));
        }
    }
    if let Some(missing) = value.get("missing_env").and_then(Value::as_array) {
        if missing.is_empty() {
            out.push_str("Required env: all set\n");
        } else {
            out.push_str("Missing env (names only):\n");
            for name in missing.iter().filter_map(Value::as_str) {
                out.push_str(&format!("  {name}\n"));
            }
        }
    }
    if let Some(copies) = value.get("copy").and_then(Value::as_array) {
        if !copies.is_empty() {
            out.push_str("Copy:\n");
            for entry in copies.iter().filter_map(Value::as_str) {
                out.push_str(&format!("  {entry}\n"));
            }
        }
    }
    if let Some(profiles) = value.get("launch_profiles").and_then(Value::as_array) {
        if !profiles.is_empty() {
            out.push_str("Launch profiles:\n");
            for profile in profiles.iter().filter_map(Value::as_str) {
                out.push_str(&format!("  {profile}\n"));
            }
        }
    }
    if let Some(ok) = value.get("ok").and_then(Value::as_bool) {
        out.push_str(&format!("Result: {}\n", if ok { "ok" } else { "failed" }));
    }
    if let Some(detail) = value.get("message").and_then(Value::as_str) {
        out.push_str(&format!("{detail}\n"));
    }
    if let Some(ready) = value.get("ready") {
        out.push_str(&format!(
            "Ready: {} — {}\n",
            ready["ready"].as_bool().unwrap_or(false),
            ready["detail"].as_str().unwrap_or("")
        ));
    }
    if let Some(pid) = value.get("pid").and_then(Value::as_u64) {
        out.push_str(&format!("Pid: {pid}\n"));
    }
    if let Some(log) = value.get("log_path").and_then(Value::as_str) {
        out.push_str(&format!("Log: {log}\n"));
    }
    out.trim_end().to_string()
}

fn check(path: Option<&str>) -> Result<Value, String> {
    let worktree = resolve_worktree(path)?;
    let config = load_config(&worktree)?;
    let trusted = is_trusted_project(&worktree);
    let plan = dragent_workspace_setup::orchestrate::check_plan(&worktree, config.as_ref());
    Ok(json!({
        "worktree": plan.worktree,
        "trusted": trusted,
        "has_config": plan.has_config,
        "config_files": [".dragent/workspace.json", ".dragent/workspace.local.json"],
        "platform": dragent_workspace_setup::platform::current(),
        "init_steps": plan.init_steps.iter().map(|step| json!({
            "index": step.index,
            "description": step.description,
            "cwd": step.cwd,
            "timeout_secs": step.timeout_secs,
            "platforms": step.platforms,
        })).collect::<Vec<_>>(),
        "missing_env": plan.missing_env,
        "copy": plan.copy,
        "launch_profiles": plan.launch_profiles,
        "cleanup_steps": plan.cleanup_steps,
        "next": if !trusted {
            "Trust the project in Dragent, then `dragent setup init`."
        } else if !plan.has_config {
            "Add .dragent/workspace.json to describe init, launch and cleanup."
        } else if !plan.missing_env.is_empty() {
            "Set the missing environment names, then `dragent setup init`."
        } else {
            "`dragent setup init` runs the plan; `dragent setup launch PROFILE` starts a service."
        },
    }))
}

fn init(path: Option<&str>) -> Result<Value, String> {
    let worktree = resolve_worktree(path)?;
    if !is_trusted_project(&worktree) {
        return Err(needs_approval(&worktree));
    }
    let Some(config) = load_config(&worktree)? else {
        return Err(Failure::new(
            Status::InvalidInput,
            "no_config",
            format!(
                "no .dragent/workspace.json in {} (check `dragent setup check`)",
                worktree.display()
            ),
        )
        .next("Add .dragent/workspace.json, or run `dragent setup check` to see the plan.")
        .encode());
    };
    let main = main_checkout_of(&worktree);
    let outcome = dragent_workspace_setup::orchestrate::run_init(
        &config,
        &worktree,
        main.as_deref(),
        Some(&state_dir()),
    );
    if !outcome.missing_env.is_empty() {
        return Err(Failure::new(
            Status::Failed,
            "missing_env",
            format!(
                "missing required environment: {} (names only; values are never printed)",
                outcome.missing_env.join(", ")
            ),
        )
        .next("Set the named variables in the environment, then run again.")
        .encode());
    }
    if !outcome.ok {
        let failed = outcome
            .steps
            .iter()
            .find(|step| !step.ok)
            .expect("a failing step");
        if failed.timed_out {
            return Err(Failure::new(
                Status::Timeout,
                "step_timeout",
                format!(
                    "init step {} ({}) timed out after {}s; only that step's process tree was killed\n{}",
                    failed.index,
                    failed.description,
                    config.init.get(failed.index).map(|step| step.timeout_secs_or_default()).unwrap_or(600),
                    failed.output_tail
                ),
            )
            .next("Raise its timeout_secs or make the step faster, then run again.")
            .encode());
        }
        return Err(Failure::new(
            Status::Failed,
            "step_failed",
            format!(
                "init step {} ({}) failed{}:\n{}",
                failed.index,
                failed.description,
                failed
                    .exit_code
                    .map(|code| format!(" with exit {code}"))
                    .unwrap_or_default(),
                failed.output_tail
            ),
        )
        .next("Fix the step (or the repository), then run again. Steps before it already ran.")
        .encode());
    }
    Ok(json!({
        "worktree": worktree.display().to_string(),
        "trusted": true,
        "ok": true,
        "steps_run": outcome.steps.len(),
        "steps": outcome.steps.iter().map(|step| json!({
            "index": step.index,
            "description": step.description,
            "ok": step.ok,
            "skipped_platform": step.skipped_platform,
        })).collect::<Vec<_>>(),
        "copied": outcome.copied,
    }))
}

fn launch(profile: &str, path: Option<&str>) -> Result<Value, String> {
    let worktree = resolve_worktree(path)?;
    if !is_trusted_project(&worktree) {
        return Err(needs_approval(&worktree));
    }
    let Some(config) = load_config(&worktree)? else {
        return Err(Failure::new(
            Status::InvalidInput,
            "no_config",
            format!("no .dragent/workspace.json in {}", worktree.display()),
        )
        .next("Add .dragent/workspace.json with a launch profile, then run again.")
        .encode());
    };
    if !config.launch.contains_key(profile) {
        let mut names: Vec<&String> = config.launch.keys().collect();
        names.sort();
        return Err(Failure::new(
            Status::InvalidInput,
            "unknown_profile",
            format!(
                "unknown launch profile \"{profile}\" (have: {})",
                names
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .next("Pick a profile from `dragent setup check`.")
        .encode());
    }
    match dragent_workspace_setup::orchestrate::launch_profile(
        &config,
        profile,
        &worktree,
        &state_dir(),
    ) {
        Ok(result) => Ok(json!({
            "worktree": worktree.display().to_string(),
            "trusted": true,
            "status": if result.ready.as_ref().is_some_and(|ready| !ready.ready) { "unverified" } else { "ok" },
            "profile": result.record.profile,
            "pid": result.record.pid,
            "log_path": result.record.log_path,
            "command": result.record.command,
            "ready": result.ready.map(|ready| json!({"ready": ready.ready, "detail": ready.detail})),
        })),
        Err(message) if message.starts_with("launch setup step") => {
            Err(Failure::new(Status::Failed, "step_failed", message)
                .next("Fix the profile's setup step, then run again.")
                .encode())
        }
        Err(message) => Err(Failure::new(Status::Failed, "launch_failed", message).encode()),
    }
}

fn status(path: Option<&str>) -> Result<Value, String> {
    let worktree = resolve_worktree(path)?;
    let trusted = is_trusted_project(&worktree);
    let state = state_dir();
    let last_init = dragent_workspace_setup::registry::load_init(&state, &worktree);
    let launches = dragent_workspace_setup::registry::list_launches(&state, Some(&worktree));
    let running: Vec<Value> = launches
        .iter()
        .map(|record| {
            let alive = dragent_workspace_setup::registry::is_running(record);
            json!({
                "profile": record.profile,
                "pid": record.pid,
                "running": alive,
                "log_path": record.log_path,
                "command": record.command,
                "started_at_ms": record.started_at_ms,
                "ready": record.ready,
            })
        })
        .collect();
    Ok(json!({
        "worktree": worktree.display().to_string(),
        "trusted": trusted,
        "last_init": last_init.map(|record| json!({
            "at_ms": record.at_ms,
            "ok": record.ok,
            "steps_run": record.steps_run,
            "steps_skipped": record.steps_skipped,
            "failed_step": record.failed_step,
            "exit_code": record.exit_code,
            "timed_out": record.timed_out,
            "output_tail": record.output_tail,
            "missing_env": record.missing_env,
            "copied": record.copied,
        })),
        "launches": running,
    }))
}

fn stop(profile: Option<&str>, path: Option<&str>) -> Result<Value, String> {
    let worktree = resolve_worktree(path)?;
    let state = state_dir();
    match profile {
        Some(profile) => {
            match dragent_workspace_setup::orchestrate::stop_launch(&state, &worktree, profile) {
                Ok(outcome) => Ok(json!({
                    "worktree": worktree.display().to_string(),
                    "profile": profile,
                    "stopped": outcome.stopped,
                    "pid": outcome.pid,
                })),
                Err(message) => Err(Failure::new(Status::InvalidTarget, "not_running", message)
                    .next("`dragent setup status` lists running launches.")
                    .encode()),
            }
        }
        None => {
            let launches =
                dragent_workspace_setup::registry::list_launches(&state, Some(&worktree));
            let mut stopped = Vec::new();
            for record in &launches {
                if dragent_workspace_setup::registry::stop_recorded(record).unwrap_or(false) {
                    stopped.push(record.profile.clone());
                }
                let _ = dragent_workspace_setup::registry::remove_launch(
                    &state,
                    &record.worktree,
                    &record.profile,
                );
            }
            Ok(json!({
                "worktree": worktree.display().to_string(),
                "stopped": stopped,
            }))
        }
    }
}

fn cleanup(path: Option<&str>) -> Result<Value, String> {
    let worktree = resolve_worktree(path)?;
    let state = state_dir();
    // Stopping is always safe (only recorded processes whose start time
    // still matches); the cleanup *steps* need trust.
    let trusted = is_trusted_project(&worktree);
    let Some(config) = load_config(&worktree)? else {
        // No config: still stop launches, then report nothing else to do.
        let launches = dragent_workspace_setup::registry::list_launches(&state, Some(&worktree));
        let mut stopped = Vec::new();
        for record in &launches {
            if dragent_workspace_setup::registry::stop_recorded(record).unwrap_or(false) {
                stopped.push(record.profile.clone());
            }
            let _ = dragent_workspace_setup::registry::remove_launch(
                &state,
                &record.worktree,
                &record.profile,
            );
        }
        return Ok(json!({
            "worktree": worktree.display().to_string(),
            "trusted": trusted,
            "ok": true,
            "stopped": stopped,
            "steps": [],
        }));
    };
    if !trusted && !config.cleanup.is_empty() {
        // Stop first (safe), then refuse the steps.
        let launches = dragent_workspace_setup::registry::list_launches(&state, Some(&worktree));
        for record in &launches {
            let _ = dragent_workspace_setup::registry::stop_recorded(record);
            let _ = dragent_workspace_setup::registry::remove_launch(
                &state,
                &record.worktree,
                &record.profile,
            );
        }
        return Err(needs_approval(&worktree));
    }
    let outcome = dragent_workspace_setup::orchestrate::run_cleanup(&config, &worktree, &state);
    if !outcome.ok {
        let failed = outcome
            .steps
            .iter()
            .find(|step| !step.ok)
            .expect("a failing step");
        if failed.timed_out {
            return Err(Failure::new(
                Status::Timeout,
                "step_timeout",
                format!(
                    "cleanup step {} ({}) timed out; only that step's process tree was killed\n{}",
                    failed.index, failed.description, failed.output_tail
                ),
            )
            .encode());
        }
        return Err(Failure::new(
            Status::Failed,
            "step_failed",
            format!(
                "cleanup step {} ({}) failed{}:\n{}",
                failed.index,
                failed.description,
                failed
                    .exit_code
                    .map(|code| format!(" with exit {code}"))
                    .unwrap_or_default(),
                failed.output_tail
            ),
        )
        .encode());
    }
    Ok(json!({
        "worktree": worktree.display().to_string(),
        "trusted": trusted,
        "ok": true,
        "stopped": outcome.stopped,
        "steps": outcome.steps.iter().map(|step| json!({
            "index": step.index,
            "description": step.description,
            "ok": step.ok,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_parses_profile_and_path() {
        // One existing dir reads as a path (stop everything there).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().display().to_string();
        assert!(Path::new(&path).exists());
    }

    #[test]
    fn unknown_setup_command_is_invalid_input() {
        let failure = Failure::from_error(&run(&["bogus".to_string()]).unwrap_err());
        assert_eq!(
            (failure.status, failure.code.as_str()),
            (Status::InvalidInput, "usage")
        );
    }
}
