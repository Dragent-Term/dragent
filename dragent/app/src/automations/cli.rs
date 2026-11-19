//! `dragent automations …`: manage automations from a terminal, so an agent
//! can create and tune them ("Create with AI") the same way a person does
//! in the Automations page. Commands edit the organisation's automation
//! store directly; a running app notices within seconds, starts what is
//! due, and mirrors changes to the organisation's registry.

use std::path::{Path, PathBuf};

use dragent_automations::{
    local_time_zone, normalize_tags, Automation, MissedPolicy, OverlapPolicy, PromptSource,
    RunFilter, RunStatus, Schedule, Scheduler, Store, WorkspaceMode, FREE_AUTOMATION_LIMIT,
};
use dragent_host::outcome::{Failure, Status};

use crate::workspace::PromptAgent;

const HELP: &str = "\
Manage Dragent automations: agents that start on a schedule.

Usage: dragent automations <command> [options]

Commands:
  list                         List your automations
  get <id>                     Show one automation
  create [options]             Create an automation (needs --name, --prompt, --schedule)
  update <id> [options]        Change an automation (only the options given)
  preview [id] [--schedule CRON] [--timezone ZONE] [--count N]
                               Show the next run times without creating anything
  run <id>                     Start a run now (Dragent must be open)
  run-get <run-id>             Show one run with its result
  cancel <run-id>              Stop a scheduled, queued or starting run
  pause <id> | resume <id>     Stop or restart scheduled runs
  delete <id>                  Delete an automation and its history
  runs <id> [--limit N]        Recent runs, newest first: what each run's agent
                               is doing (working, needs_input, done, failed,
                               ended) and the start of its last reply
  prompt get <id>              Print the prompt
  prompt set <id> (--prompt T | --file F)
  prompt history <id>          Earlier prompts, newest first
  copy-files <project> [--set P1,P2,...]
                               Gitignored files copied into new worktrees

Options for create and update:
  --name NAME
  --prompt TEXT | --prompt-file FILE
  --schedule \"CRON\"             Five fields: minute hour day month weekday
                               e.g. \"0 9 * * 1-5\" = weekdays at 9:00
  --timezone ZONE              IANA zone the schedule runs in (default: this
                               computer's). Runs happen at local wall-clock
                               times in that zone.
  --missed skip|catch-up       Missed runs while the app was closed: skip them
                               (default) or run the most recent one once
  --overlap skip|queue|allow   A due run while the previous is still starting:
                               skip it (default), queue it, or allow both
  --max-retries N              Retry a failed start up to N times (0-10, default 0)
  --retry-delay SECS           Base delay before the first retry (10-86400,
                               default 300); each retry waits twice as long
  --retention N                Keep the last N runs per automation (1-1000,
                               default 100); the planned run and runs still
                               waiting or working are always kept
  --agent KEY                  claude, codex, gemini, opencode, dragent, …
  --project PATH | --session   Work in a project, or in a project-less folder
  --workspace new|branch|pinned
                               new: a fresh worktree each run (default)
                               branch: a worktree on --branch
                               pinned: the same workspace every run
  --base-branch B              Start new worktrees from B
  --push                       Publish each new branch to the remote
  --branch B                   The branch for --workspace branch
  --path P                     The workspace for --workspace pinned
                               (default: the project's own checkout)
  --continue-session           With pinned: send each prompt to the agent the
                               previous run left running
  --watch SERVICE              Poll a connected service's search (linear,
                               sentry, …) on the schedule; a run starts only
                               when the search finds something new. This is a
                               polling watch, not instant webhook delivery.
  --watch-query Q              What the watch searches for (default: recent or assigned)
  --check-every MINUTES        With --watch: check every 5, 15, 30 or 60 minutes
  --no-watch                   Run on the schedule only (replacing the trigger
                               needs this or --watch explicitly)
  --services read|write        Whether runs may comment, change status and create items
                               in connected services (default: read)
  --tag T                      Sidebar folder for run workspaces (repeatable)
  --no-tags                    Don't file runs into a folder
  --paused                     Create it switched off

A 02:30 daily run skips the spring-forward day (that time never happens)
and runs once, at the first 02:30, on the fall-back day (that time happens
twice). Preview it with `automations preview --schedule \"30 2 * * *\"`.

Add --json to any command for machine-readable output. A trial `run` answers
{\"status\": \"accepted\", \"run_id\": \"…\"}; bad input fails with status
invalid_input and a precise message.
";

/// The organisation's `dragent` launcher, for agents to call (they don't
/// inherit the app's organisation). Returns its path.
pub fn install_shim() -> Result<PathBuf, String> {
    crate::cli::install_launchers()
}

struct Flags {
    positional: Vec<String>,
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

const SWITCHES: [&str; 7] = [
    "json",
    "push",
    "session",
    "continue-session",
    "no-tags",
    "paused",
    "no-watch",
];

fn parse(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags {
        positional: Vec::new(),
        values: Vec::new(),
        switches: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(name) = arg.strip_prefix("--") else {
            flags.positional.push(arg.clone());
            continue;
        };
        if let Some((name, value)) = name.split_once('=') {
            flags.values.push((name.to_string(), value.to_string()));
        } else if SWITCHES.contains(&name) {
            flags.switches.push(name.to_string());
        } else if name == "help" {
            flags.positional.insert(0, "help".into());
        } else {
            let value = iter.next().ok_or_else(|| {
                Failure::new(
                    Status::InvalidInput,
                    "missing_value",
                    format!("--{name} needs a value"),
                )
                .next(format!("Pass --{name} VALUE."))
                .encode()
            })?;
            flags.values.push((name.to_string(), value.clone()));
        }
    }
    Ok(flags)
}

impl Flags {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }

    fn on(&self, name: &str) -> bool {
        self.switches.iter().any(|switch| switch == name)
    }

    fn id(&self, index: usize) -> Result<&str, String> {
        self.positional
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| {
                Failure::new(Status::InvalidInput, "missing_id", "missing automation id")
                    .next("Pass an automation id from `dragent automations list`.")
                    .encode()
            })
    }
}

fn invalid_input(code: &str, message: impl Into<String>, next: impl Into<String>) -> String {
    Failure::new(Status::InvalidInput, code, message)
        .next(next)
        .encode()
}

fn invalid_target(code: &str, message: impl Into<String>, next: impl Into<String>) -> String {
    Failure::new(Status::InvalidTarget, code, message)
        .next(next)
        .encode()
}

fn failed(code: &str, message: impl Into<String>, next: impl Into<String>) -> String {
    Failure::new(Status::Failed, code, message)
        .next(next)
        .encode()
}

fn definition_failure(error: &dragent_automations::DefinitionError) -> String {
    use dragent_automations::DefinitionError as E;
    let (code, next) = match error {
        E::NameRequired => ("name_required", "Give the automation a name with --name."),
        E::PromptRequired => (
            "prompt_required",
            "Write the prompt with --prompt or --prompt-file.",
        ),
        E::AgentRequired => (
            "agent_required",
            "Choose an agent with --agent (see `dragent agents list`).",
        ),
        E::ServiceRequired => (
            "service_required",
            "Choose the service to watch with --watch.",
        ),
        E::ProjectRequired => ("project_required", "Pass --project PATH or --session."),
        E::BranchRequired => ("branch_required", "Pass --branch for --workspace branch."),
        E::TimeZone(_) => (
            "invalid_timezone",
            "Use an IANA zone like America/New_York or Africa/Lagos.",
        ),
        E::Schedule(_) => (
            "invalid_schedule",
            "Use five fields: minute hour day month weekday, e.g. \"0 9 * * 1-5\".",
        ),
        E::Retries(_) => ("invalid_retries", "Use --max-retries 0 to 10."),
        E::RetryDelay(_) => (
            "invalid_retry_delay",
            "Use --retry-delay 10 to 86400 seconds.",
        ),
        E::Retention(_) => ("invalid_retention", "Use --retention 1 to 1000."),
        E::MissedPolicy(_) => ("invalid_missed_policy", "Use --missed skip or catch-up."),
        E::OverlapPolicy(_) => (
            "invalid_overlap_policy",
            "Use --overlap skip, queue or allow.",
        ),
    };
    invalid_input(code, error.to_string(), next)
}

fn open_store() -> Result<Store, String> {
    let path = dragent_paths::organisation_dir().join("automations.sqlite");
    Store::open(&path).map_err(|error| {
        failed(
            "store_unavailable",
            format!("couldn't open {}: {error:#}", path.display()),
            "Check the organisation folder is writable, then try again.",
        )
    })
}

fn owner_id(store: &Store) -> String {
    crate::cli::live_organisation()
        .or_else(crate::organisation_context::cached_active)
        .map(|active| active.user_id)
        .filter(|id| !id.is_empty())
        .or_else(|| store.meta(super::OWNER_KEY).ok().flatten())
        .unwrap_or_default()
}

fn scheduler(store: &Store) -> Scheduler {
    let owner = owner_id(store);
    let unlimited =
        store.meta(super::PLAN_LIMIT_KEY).ok().flatten().as_deref() == Some("unlimited");
    Scheduler {
        owner_id: (!owner.is_empty()).then_some(owner),
        automation_limit: (!unlimited).then_some(FREE_AUTOMATION_LIMIT),
        ..Scheduler::default()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn find(store: &Store, id: &str) -> Result<Automation, String> {
    if let Ok(Some(automation)) = store.get_automation(id) {
        return Ok(automation);
    }
    // Accept a unique id prefix, as printed by `list`.
    let matches: Vec<Automation> = store
        .list_automations()
        .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?
        .into_iter()
        .filter(|automation| automation.id.starts_with(id))
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => Err(invalid_target(
            "no_automation",
            format!("no automation {id}"),
            "List automations with `dragent automations list`, then use a current id.",
        )),
        _ => Err(invalid_target(
            "ambiguous_id",
            format!("{id} matches several automations; use more of the id"),
            "List automations with `dragent automations list`, then use a longer prefix.",
        )),
    }
}

fn find_run(store: &Store, id: &str) -> Result<dragent_automations::Run, String> {
    // The full id, or a unique prefix of it as printed by `runs`.
    let mut runs = store
        .runs_matching(id)
        .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?;
    match runs.len() {
        1 => Ok(runs.remove(0)),
        0 => Err(invalid_target(
            "no_run",
            format!("no run {id}"),
            "List runs with `dragent automations runs <automation-id>`, then use a current run id.",
        )),
        _ => Err(invalid_target(
            "ambiguous_id",
            format!("{id} matches several runs; use more of the id"),
            "Use a longer prefix of the run id.",
        )),
    }
}

fn json(automation: &Automation, store: &Store) -> serde_json::Value {
    let next = next_run(store, automation);
    let last = store.latest_run(&automation.id).ok().flatten();
    serde_json::json!({
        "id": automation.id,
        "name": automation.name,
        "enabled": automation.enabled,
        "schedule": automation.schedule,
        "schedule_label": describe(automation),
        "timezone": automation.timezone,
        "missed": automation.missed_policy.as_str(),
        "overlap": automation.overlap_policy.as_str(),
        "max_retries": automation.max_retries,
        "retry_delay_secs": automation.retry_delay_secs,
        "retention": automation.run_retention,
        "agent": automation.agent,
        "agent_may_wait_for_approval": approval_note(automation).is_some(),
        "project": (!automation.is_session()).then(|| automation.project_root.display().to_string()),
        "session": automation.is_session(),
        "workspace": automation.workspace,
        "tags": automation.tags,
        "continue_session": automation.continue_session,
        "services": if automation.service_writes { "write" } else { "read" },
        "watch": automation.watch,
        "prompt": automation.prompt,
        "next_run_at": next,
        "last_run": last.map(|run| run_json(&run)),
    })
}

fn run_json(run: &dragent_automations::Run) -> serde_json::Value {
    serde_json::json!({
        "id": run.id,
        "automation_id": run.automation_id,
        "status": run.status.as_str(),
        "trigger": match run.trigger {
            dragent_automations::Trigger::Schedule => "schedule",
            dragent_automations::Trigger::Manual => "manual",
        },
        "agent": run.progress.as_ref().map(|progress| progress.state.as_str()),
        "reply": run.progress.as_ref().and_then(|progress| progress.reply.as_deref()),
        "at": run.display_time_ms(),
        "scheduled_at": run.scheduled_at_ms,
        "started_at": run.started_at_ms,
        "finished_at": run.finished_at_ms,
        "workspace": run.workspace_path,
        "branch": run.branch,
        "attempt": run.attempt,
        "retry_of": run.retry_of,
        "warning": run.warning,
        "continued": run.continued,
        "error": run.error.as_ref().map(super::executor::describe_error),
        "error_code": run.error.as_ref().map(|error| error.code.as_str()),
    })
}

/// A warning when the automation's agent may stop for approval during a
/// run, where nobody is there to give it.
fn approval_note(automation: &Automation) -> Option<String> {
    let agent = super::executor::prompt_agent(&automation.agent)?;
    let settings = crate::setup::UiSettings::load();
    (!super::executor::runs_unattended(agent, &settings)).then(|| {
        format!(
            "Note: {} may stop to ask for approval during a run, and runs are unattended. \
             Its command in Settings → Agents needs its skip-approval option for it to work on its own.",
            agent.label()
        )
    })
}

/// When it runs next: the run the app has planned, else the schedule's
/// next time (the app plans that run within seconds of seeing the change).
fn next_run(store: &Store, automation: &Automation) -> Option<i64> {
    if !automation.enabled {
        return None;
    }
    store
        .scheduled_run(&automation.id)
        .ok()
        .flatten()
        .and_then(|run| run.scheduled_at_ms)
        .or_else(|| automation.next_occurrence_ms(now_ms()))
}

fn describe(automation: &Automation) -> String {
    if let Some(watch) = &automation.watch {
        return super::watch::describe(watch, &automation.schedule);
    }
    Schedule::parse(&automation.schedule)
        .map(|schedule| schedule.describe())
        .unwrap_or_else(|_| automation.schedule.clone())
}

fn time(ms: Option<i64>) -> String {
    ms.map(super::format_date_time)
        .unwrap_or_else(|| "—".into())
}

fn pretty(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

pub(crate) fn run(args: &[String]) -> Result<String, String> {
    let flags = parse(args)?;
    let command = flags
        .positional
        .first()
        .cloned()
        .unwrap_or_else(|| "help".into());
    let rest = Flags {
        positional: flags.positional.iter().skip(1).cloned().collect(),
        values: flags.values.clone(),
        switches: flags.switches.clone(),
    };
    if command == "help" {
        return Ok(HELP.trim_end().to_string());
    }
    let as_json = rest.on("json");
    if command == "preview" && rest.value("schedule").is_some() {
        // A candidate schedule's preview is date arithmetic: no store.
        return preview(None, &rest, as_json);
    }
    let store = open_store()?;
    match command.as_str() {
        "list" => {
            let scheduler = scheduler(&store);
            let automations: Vec<Automation> = store
                .list_automations()
                .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?
                .into_iter()
                .filter(|automation| scheduler.owns(automation))
                .collect();
            if as_json {
                let list: Vec<_> = automations.iter().map(|a| json(a, &store)).collect();
                return Ok(pretty(serde_json::Value::Array(list)));
            }
            let records: Vec<_> = automations
                .iter()
                .map(|automation| json(automation, &store))
                .collect();
            if let Some(output) = crate::cli::presentation::records(
                &records,
                "No automations yet. Create one with dragent automations create.",
            ) {
                return Ok(output);
            }
            if automations.is_empty() {
                return Ok(
                    "No automations yet. Create one with `dragent automations create`.".into(),
                );
            }
            let lines: Vec<String> = automations
                .iter()
                .map(|automation| {
                    let next = next_run(&store, automation);
                    format!(
                        "{}  {}  [{}]  {}  next: {}",
                        &automation.id[..8.min(automation.id.len())],
                        automation.name,
                        if automation.enabled {
                            "active"
                        } else {
                            "paused"
                        },
                        describe(automation),
                        time(next),
                    )
                })
                .collect();
            Ok(lines.join("\n"))
        }
        "get" => {
            let automation = find(&store, rest.id(0)?)?;
            Ok(pretty(json(&automation, &store)))
        }
        "create" => {
            let scheduler = scheduler(&store);
            let owned = store
                .list_automations()
                .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?
                .into_iter()
                .filter(|automation| scheduler.owns(automation))
                .count();
            if scheduler
                .automation_limit
                .is_some_and(|limit| owned >= limit)
            {
                return Err(failed(
                    "plan_limit",
                    super::LIMIT_MESSAGE,
                    "Upgrade to Pro for unlimited automations, or delete one you no longer need.",
                ));
            }
            let mut automation = Automation::new(
                "",
                PathBuf::new(),
                "",
                "claude",
                "",
                WorkspaceMode::default(),
                now_ms(),
            );
            automation.timezone = local_time_zone();
            automation.owner_id = owner_id(&store);
            apply(&mut automation, &rest)?;
            if rest.on("paused") {
                automation.enabled = false;
            }
            save(&store, automation, PromptSource::Agent, as_json)
        }
        "update" => {
            let mut automation = find(&store, rest.id(0)?)?;
            apply(&mut automation, &rest)?;
            automation.revision = automation.revision.saturating_add(1);
            save(&store, automation, PromptSource::Agent, as_json)
        }
        "preview" => return preview(Some(&store), &rest, as_json),
        "pause" | "resume" => {
            let mut automation = find(&store, rest.id(0)?)?;
            let enable = command == "resume";
            if enable {
                let scheduler = scheduler(&store);
                let all = store.list_automations().map_err(|error| {
                    failed("store_unavailable", format!("{error:#}"), "Try again.")
                })?;
                if !scheduler.runnable(all.iter()).contains(&automation.id) {
                    return Err(failed(
                        "plan_limit",
                        super::LIMIT_MESSAGE,
                        "Upgrade to Pro for unlimited automations, or delete one you no longer need.",
                    ));
                }
            }
            automation.enabled = enable;
            automation.revision = automation.revision.saturating_add(1);
            let name = automation.name.clone();
            save(&store, automation, PromptSource::Agent, false)?;
            Ok(format!(
                "{name} {}.",
                if enable { "resumed" } else { "paused" }
            ))
        }
        "delete" => {
            let automation = find(&store, rest.id(0)?)?;
            store
                .delete_automation(&automation.id)
                .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?;
            super::team::queue_delete(&store, &automation.id);
            Ok(format!("Deleted {}.", automation.name))
        }
        "run" => {
            let automation = find(&store, rest.id(0)?)?;
            if !automation.enabled {
                return Err(invalid_input(
                    "automation_paused",
                    "the automation is paused; resume it first",
                    "Resume it with `dragent automations resume ID`, then run it.",
                ));
            }
            let scheduler = scheduler(&store);
            let all = store
                .list_automations()
                .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?;
            if !scheduler.runnable(all.iter()).contains(&automation.id) {
                return Err(failed(
                    "plan_limit",
                    super::LIMIT_MESSAGE,
                    "Upgrade to Pro for unlimited automations, or delete one you no longer need.",
                ));
            }
            let run = scheduler
                .run_now(&store, &automation)
                .map_err(|error| failed("queue_failed", format!("{error:#}"), "Try again."))?;
            if as_json {
                return Ok(pretty(serde_json::json!({
                    "status": "accepted",
                    "run_id": run.id,
                    "automation_id": automation.id,
                })));
            }
            Ok(format!(
                "Queued a run of {} ({}). It starts within seconds while Dragent is open.",
                automation.name, run.id
            ))
        }
        "run-get" | "run_get" | "show-run" => {
            let id = rest.id(0)?;
            let run = find_run(&store, id)?;
            if as_json {
                return Ok(pretty(run_json(&run)));
            }
            Ok(format_run(&run))
        }
        "cancel" => {
            let id = rest.id(0)?;
            return cancel_run(&store, id, as_json);
        }
        "runs" => {
            // `runs <automation-id> [<run-id>]`: a run id reads one run's result.
            if rest.positional.len() >= 2 {
                let run = find_run(&store, rest.id(1)?)?;
                if as_json {
                    return Ok(pretty(run_json(&run)));
                }
                return Ok(format_run(&run));
            }
            let automation = find(&store, rest.id(0)?)?;
            let limit = rest
                .value("limit")
                .map(|value| {
                    value.parse::<usize>().map_err(|_| {
                        invalid_input(
                            "invalid_limit",
                            format!("--limit takes a number, not “{value}”"),
                            "Pass --limit N, e.g. --limit 10.",
                        )
                    })
                })
                .transpose()?
                .unwrap_or(10)
                .min(1000);
            let runs = store
                .list_runs(&automation.id, RunFilter::All, None, limit)
                .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?;
            if as_json {
                return Ok(pretty(serde_json::to_value(&runs).unwrap_or_default()));
            }
            let records = serde_json::to_value(&runs).unwrap_or_default();
            if let Some(output) = crate::cli::presentation::records(
                records.as_array().map(Vec::as_slice).unwrap_or_default(),
                "No runs yet.",
            ) {
                return Ok(output);
            }
            if runs.is_empty() {
                return Ok("No runs yet.".into());
            }
            Ok(runs
                .iter()
                .map(|run| {
                    let detail = run
                        .error
                        .as_ref()
                        .map(super::executor::describe_error)
                        .or_else(|| run.workspace_path.as_ref().map(|p| p.display().to_string()))
                        .unwrap_or_default();
                    // A started run: how its agent got on, and what it said.
                    let status = match &run.progress {
                        Some(progress) if run.status == RunStatus::Started => {
                            progress.state.as_str()
                        }
                        _ => run.status.as_str(),
                    };
                    let mut line = format!(
                        "{}  {}  {}  {}",
                        run.id,
                        time(run.display_time_ms()),
                        status,
                        detail
                    );
                    if let Some(reply) = run
                        .progress
                        .as_ref()
                        .and_then(|progress| progress.reply.as_deref())
                    {
                        for reply_line in super::progress::reply_preview(reply).lines() {
                            line.push_str("\n    ");
                            line.push_str(reply_line);
                        }
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "prompt" => {
            let action = rest.positional.first().map(String::as_str).unwrap_or("get");
            let automation = find(&store, rest.id(1)?)?;
            match action {
                "get" => Ok(automation.prompt),
                "history" => {
                    let versions = store.prompt_versions(&automation.id).map_err(|error| {
                        failed("store_unavailable", format!("{error:#}"), "Try again.")
                    })?;
                    if as_json {
                        let list: Vec<_> = versions
                            .iter()
                            .map(|v| serde_json::json!({"id": v.id, "source": v.source.as_str(), "at": v.created_at_ms, "prompt": v.prompt}))
                            .collect();
                        return Ok(pretty(serde_json::Value::Array(list)));
                    }
                    Ok(versions
                        .iter()
                        .map(|v| {
                            format!(
                                "#{} {} ({})\n{}\n",
                                v.id,
                                time(Some(v.created_at_ms)),
                                v.source.as_str(),
                                v.prompt
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"))
                }
                "set" => {
                    let mut automation = automation;
                    automation.prompt = prompt_from(&rest)?.ok_or_else(|| {
                        invalid_input(
                            "missing_prompt",
                            "give the prompt with --prompt or --file",
                            "Pass --prompt TEXT or --file PATH.",
                        )
                    })?;
                    automation.revision = automation.revision.saturating_add(1);
                    save(&store, automation, PromptSource::Agent, as_json)
                }
                other => Err(invalid_input(
                    "unknown_prompt_command",
                    format!("unknown prompt command {other}"),
                    "Use prompt get, prompt set or prompt history.",
                )),
            }
        }
        "copy-files" => {
            let project = rest.positional.first().map(PathBuf::from).ok_or_else(|| {
                invalid_input(
                    "missing_project",
                    "missing project path",
                    "Pass a project path, e.g. `dragent automations copy-files /path/to/repo`.",
                )
            })?;
            let project = project.canonicalize().unwrap_or(project);
            if let Some(list) = rest.value("set") {
                let patterns: Vec<String> = list
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
                store
                    .set_copy_patterns(&project, &patterns)
                    .map_err(|error| {
                        failed("store_unavailable", format!("{error:#}"), "Try again.")
                    })?;
            }
            Ok(store
                .copy_patterns(&project)
                .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?
                .join("\n"))
        }
        other => Err(invalid_input(
            "unknown_command",
            format!("unknown command {other}. Run `dragent automations help`."),
            "`dragent capabilities --json` lists every command this build has.",
        )),
    }
}

fn prompt_from(flags: &Flags) -> Result<Option<String>, String> {
    if let Some(file) = flags.value("prompt-file").or_else(|| flags.value("file")) {
        return std::fs::read_to_string(file).map(Some).map_err(|error| {
            invalid_input(
                "unreadable_prompt_file",
                format!("couldn't read {file}: {error}"),
                "Check the path, then try again.",
            )
        });
    }
    Ok(flags
        .value("prompt")
        .or_else(|| flags.value("text"))
        .map(str::to_string))
}

fn effective_zone(timezone: &str) -> String {
    if timezone.trim().is_empty() {
        local_time_zone()
    } else {
        timezone.to_string()
    }
}

fn format_preview_time(ms: i64, zone: &dragent_automations::Tz) -> String {
    use chrono::TimeZone;
    zone.timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%b %-d, %Y, %-I:%M %p %Z").to_string())
        .unwrap_or_default()
}

fn parse_from_ms(value: Option<&str>) -> Result<i64, String> {
    let Some(value) = value else {
        return Ok(now_ms());
    };
    if let Ok(ms) = value.parse::<i64>() {
        return Ok(ms);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|at| at.timestamp_millis())
        .map_err(|_| {
            invalid_input(
                "invalid_from",
                format!("--from takes an RFC3339 time or milliseconds, not “{value}”"),
                "Pass e.g. --from 2026-03-08T00:00:00Z.",
            )
        })
}

/// Show the next run times for an existing automation or a candidate
/// schedule, without creating anything.
fn preview(store: Option<&Store>, flags: &Flags, as_json: bool) -> Result<String, String> {
    let count = flags
        .value("count")
        .or_else(|| flags.value("limit"))
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                invalid_input(
                    "invalid_count",
                    format!("--count takes a number, not “{value}”"),
                    "Pass --count N, e.g. --count 5.",
                )
            })
        })
        .transpose()?
        .unwrap_or(5);
    if count == 0 || count > 100 {
        return Err(invalid_input(
            "invalid_count",
            "--count takes 1 to 100",
            "Pass --count N, e.g. --count 5.",
        ));
    }
    let from_ms = parse_from_ms(flags.value("from"))?;
    if let Some(schedule) = flags.value("schedule") {
        let timezone = flags.value("timezone").unwrap_or("").to_string();
        let zone_name = effective_zone(&timezone);
        let zone: dragent_automations::Tz = zone_name.parse().map_err(|_| {
            invalid_input(
                "invalid_timezone",
                format!("{zone_name} isn't a time zone this app knows."),
                "Use an IANA zone like America/New_York or Africa/Lagos.",
            )
        })?;
        let runs =
            dragent_automations::scheduler::preview_schedule(schedule, &zone_name, from_ms, count)
                .map_err(|message| {
                    invalid_input(
                        "invalid_schedule",
                        message,
                        "Use five fields: minute hour day month weekday, e.g. \"0 9 * * 1-5\".",
                    )
                })?;
        if as_json {
            let list: Vec<_> = runs
                .iter()
                .map(|at| serde_json::json!({"at": at, "local": format_preview_time(*at, &zone)}))
                .collect();
            return Ok(pretty(serde_json::json!({
                "schedule": schedule,
                "timezone": zone_name,
                "from": from_ms,
                "runs": list,
            })));
        }
        let lines: Vec<String> = runs
            .iter()
            .map(|at| format_preview_time(*at, &zone))
            .collect();
        if lines.is_empty() {
            return Ok("The schedule never runs.".into());
        }
        return Ok(lines.join("\n"));
    }
    let id = flags.positional.first().map(String::as_str).ok_or_else(|| {
        invalid_input(
            "missing_schedule",
            "preview needs an automation id or --schedule",
            "Pass an automation id, or preview a candidate with --schedule \"0 9 * * *\" --timezone ZONE.",
        )
    })?;
    let automation = match store {
        Some(store) => find(store, id)?,
        None => find(&open_store()?, id)?,
    };
    let zone_name = effective_zone(&automation.timezone);
    let zone: dragent_automations::Tz = zone_name.parse().map_err(|_| {
        invalid_input(
            "invalid_timezone",
            format!("{zone_name} isn't a time zone this app knows."),
            "Fix the automation's timezone with `dragent automations update ID --timezone ZONE`.",
        )
    })?;
    let runs = dragent_automations::scheduler::preview_schedule(
        &automation.schedule,
        &zone_name,
        from_ms,
        count,
    )
    .map_err(|message| {
        invalid_input(
            "invalid_schedule",
            message,
            "Fix the schedule with `dragent automations update ID --schedule CRON`.",
        )
    })?;
    if as_json {
        let list: Vec<_> = runs
            .iter()
            .map(|at| serde_json::json!({"at": at, "local": format_preview_time(*at, &zone)}))
            .collect();
        return Ok(pretty(serde_json::json!({
            "automation_id": automation.id,
            "schedule": automation.schedule,
            "timezone": zone_name,
            "from": from_ms,
            "runs": list,
        })));
    }
    let lines: Vec<String> = runs
        .iter()
        .map(|at| format_preview_time(*at, &zone))
        .collect();
    if lines.is_empty() {
        return Ok("The schedule never runs.".into());
    }
    Ok(lines.join("\n"))
}

/// Stop a scheduled, queued or starting run.
fn cancel_run(store: &Store, id: &str, as_json: bool) -> Result<String, String> {
    // A run id (or its unique prefix); failing that, an automation id whose
    // planned occurrence is cancelled.
    let run = match find_run(store, id) {
        Ok(run) => run,
        Err(error) if Failure::decode(&error).is_some_and(|failure| failure.code == "no_run") => {
            let Ok(automation) = find(store, id) else {
                return Err(error);
            };
            match store.scheduled_run(&automation.id) {
                Ok(Some(planned)) => planned,
                _ => {
                    return Err(invalid_target(
                        "nothing_to_cancel",
                        format!("{} has no planned run to cancel", automation.name),
                        "List runs with `dragent automations runs ID` and cancel one by its run id.",
                    ))
                }
            }
        }
        Err(error) => return Err(error),
    };
    let scheduler = scheduler(store);
    match scheduler
        .cancel(store, &run.id, now_ms())
        .map_err(|error| failed("cancel_failed", format!("{error:#}"), "Try again."))?
    {
        Some(cancelled) => {
            if as_json {
                return Ok(pretty(serde_json::json!({
                    "run_id": cancelled.id,
                    "automation_id": cancelled.automation_id,
                    "run_status": cancelled.status.as_str(),
                })));
            }
            Ok(format!(
                "Cancelled {} ({}).",
                cancelled.id,
                cancelled.status.as_str()
            ))
        }
        None => Err(invalid_target(
            "run_not_cancellable",
            "The run already finished; only scheduled, queued or starting runs can be cancelled",
            "List runs with `dragent automations runs <automation-id>` to pick a current one.",
        )),
    }
}

fn format_run(run: &dragent_automations::Run) -> String {
    let mut out = format!(
        "{}  {}  {}  {}",
        run.id,
        time(run.display_time_ms()),
        run.status.as_str(),
        run.error
            .as_ref()
            .map(super::executor::describe_error)
            .or_else(|| run.workspace_path.as_ref().map(|p| p.display().to_string()))
            .unwrap_or_default(),
    );
    if run.attempt > 0 {
        out.push_str(&format!("  attempt {}", run.attempt));
    }
    if let Some(progress) = &run.progress {
        out.push_str(&format!("  agent: {}", progress.state.as_str()));
        if let Some(reply) = &progress.reply {
            out.push('\n');
            out.push_str(reply);
        }
    }
    if let Some(warning) = &run.warning {
        out.push_str(&format!("\nWarning: {warning}"));
    }
    out
}

fn absolute(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    path.canonicalize().unwrap_or(path)
}

/// Apply create/update options to `automation`.
fn apply(automation: &mut Automation, flags: &Flags) -> Result<(), String> {
    if let Some(name) = flags.value("name") {
        automation.name = name.to_string();
    }
    if let Some(prompt) = prompt_from(flags)? {
        automation.prompt = prompt;
    }
    if let Some(schedule) = flags.value("schedule") {
        automation.schedule = schedule.to_string();
    }
    if let Some(zone) = flags.value("timezone") {
        automation.timezone = zone.to_string();
    }
    if let Some(agent) = flags.value("agent") {
        // A misspelt agent would only fail when the run starts.
        if super::executor::prompt_agent(agent).is_none() {
            let known: Vec<&str> = PromptAgent::ALL
                .iter()
                .map(|agent| agent.command_key())
                .collect();
            return Err(invalid_input(
                "unknown_agent",
                format!("unknown agent {agent}. Use one of: {}", known.join(", ")),
                "See `dragent agents list` for agents this computer can start.",
            ));
        }
        automation.agent = agent.to_string();
    }
    if let Some(missed) = flags.value("missed") {
        automation.missed_policy = MissedPolicy::parse(missed).ok_or_else(|| {
            invalid_input(
                "invalid_missed_policy",
                format!("--missed is skip or catch-up, not “{missed}”"),
                "Pass --missed skip or --missed catch-up.",
            )
        })?;
    }
    if let Some(overlap) = flags.value("overlap") {
        automation.overlap_policy = OverlapPolicy::parse(overlap).ok_or_else(|| {
            invalid_input(
                "invalid_overlap_policy",
                format!("--overlap is skip, queue or allow, not “{overlap}”"),
                "Pass --overlap skip, queue or allow.",
            )
        })?;
    }
    if let Some(retries) = flags.value("max-retries") {
        automation.max_retries = retries.parse().map_err(|_| {
            invalid_input(
                "invalid_retries",
                format!("--max-retries takes 0 to 10, not “{retries}”"),
                "Pass --max-retries N, e.g. --max-retries 3.",
            )
        })?;
    }
    if let Some(delay) = flags.value("retry-delay") {
        automation.retry_delay_secs = delay.parse().map_err(|_| {
            invalid_input(
                "invalid_retry_delay",
                format!("--retry-delay takes seconds (10-86400), not “{delay}”"),
                "Pass --retry-delay SECS, e.g. --retry-delay 300.",
            )
        })?;
    }
    if let Some(keep) = flags.value("retention") {
        automation.run_retention = keep.parse().map_err(|_| {
            invalid_input(
                "invalid_retention",
                format!("--retention takes 1 to 1000, not “{keep}”"),
                "Pass --retention N, e.g. --retention 100.",
            )
        })?;
    }
    if flags.on("session") {
        automation.project_root = PathBuf::new();
    } else if let Some(project) = flags.value("project") {
        let project = absolute(project);
        if crate::remote_fs::resolve(&project).is_some() {
            return Err(invalid_input(
                "remote_project",
                format!("{}: {}", project.display(), super::REMOTE_PROJECT_MESSAGE),
                "Choose a project on this computer.",
            ));
        }
        if !project.is_dir() {
            return Err(invalid_input(
                "missing_project",
                format!("{} isn't a folder", project.display()),
                "Pass --project PATH to a folder on this computer, or --session.",
            ));
        }
        automation.project_root = project;
    }
    let base_branch = flags.value("base-branch").map(str::to_string);
    match flags.value("workspace") {
        Some("new") | Some("worktree") => {
            automation.workspace = WorkspaceMode::NewWorktree {
                base_branch,
                push_branch: flags.on("push"),
            };
        }
        Some("branch") => {
            let branch = flags.value("branch").ok_or_else(|| {
                invalid_input(
                    "missing_branch",
                    "--workspace branch needs --branch",
                    "Pass --workspace branch --branch NAME.",
                )
            })?;
            automation.workspace = WorkspaceMode::ExistingBranch {
                branch: branch.to_string(),
            };
        }
        Some("pinned") | Some("current") => {
            automation.workspace = WorkspaceMode::Pinned {
                path: flags.value("path").map(absolute),
            };
        }
        Some(other) => {
            return Err(invalid_input(
                "unknown_workspace",
                format!("unknown workspace kind {other} (new, branch, pinned)"),
                "Use --workspace new, branch or pinned.",
            ))
        }
        None => {
            if let WorkspaceMode::NewWorktree {
                base_branch: current_base,
                push_branch,
            } = &mut automation.workspace
            {
                if base_branch.is_some() {
                    *current_base = base_branch;
                }
                if flags.on("push") {
                    *push_branch = true;
                }
            }
        }
    }
    if automation.is_session() && !automation.workspace.is_pinned() {
        // A session has no repository to branch from: each run gets a
        // fresh folder unless one is pinned.
        automation.workspace = WorkspaceMode::Pinned { path: None };
    }
    if flags.on("continue-session") {
        automation.continue_session = true;
    }
    // Replacing the trigger is explicit: changing between a plain schedule
    // and a watch needs --watch or --no-watch. Updating one field (name,
    // prompt, retention, …) never touches the trigger on its own.
    if flags.on("no-watch") {
        automation.watch = None;
    } else if let Some(service) = flags.value("watch") {
        let provider = crate::integrations::tools::provider(service).ok_or_else(|| {
            invalid_input(
                "unknown_service",
                format!("unknown service “{service}” (see `dragent integrations list`)"),
                "See `dragent integrations list` for connected services.",
            )
        })?;
        automation.watch = Some(dragent_automations::Watch {
            service: provider.key().to_string(),
            query: flags.value("watch-query").unwrap_or_default().to_string(),
        });
    } else if let (Some(query), Some(watch)) =
        (flags.value("watch-query"), automation.watch.as_mut())
    {
        watch.query = query.to_string();
    }
    if let Some(every) = flags.value("check-every") {
        let minutes: u32 = every.trim_end_matches('m').parse().map_err(|_| {
            invalid_input(
                "invalid_check_every",
                format!("--check-every takes minutes (5, 15, 30 or 60), not “{every}”"),
                "Pass --check-every 5, 15, 30 or 60.",
            )
        })?;
        if minutes == 0 {
            return Err(invalid_input(
                "invalid_check_every",
                "--check-every takes at least 1 minute",
                "Pass --check-every 5, 15, 30 or 60.",
            ));
        }
        automation.schedule = super::watch::interval_schedule(minutes);
    }
    if automation.watch.is_some() && automation.schedule.trim().is_empty() {
        automation.schedule = super::watch::interval_schedule(15);
    }
    match flags.value("services") {
        None => {}
        Some("read") | Some("read-only") => automation.service_writes = false,
        Some("write") | Some("read-write") => automation.service_writes = true,
        Some(other) => {
            return Err(invalid_input(
                "invalid_services",
                format!("--services is read or write, not “{other}”"),
                "Pass --services read or --services write.",
            ))
        }
    }
    let tags = flags.all("tag");
    if flags.on("no-tags") {
        automation.tags.clear();
    } else if !tags.is_empty() {
        automation.tags = normalize_tags(tags);
    }
    Ok(())
}

fn save(
    store: &Store,
    mut automation: Automation,
    source: PromptSource,
    as_json: bool,
) -> Result<String, String> {
    automation.tags = normalize_tags(std::mem::take(&mut automation.tags));
    // A pinned session folder is only meaningful once a run created it.
    if automation.is_session() {
        if let WorkspaceMode::Pinned { path: Some(path) } = &automation.workspace {
            if !Path::new(path).is_dir() {
                return Err(invalid_input(
                    "missing_workspace",
                    format!("{} doesn't exist", path.display()),
                    "Pass a folder that exists, or leave --path off for a fresh folder per run.",
                ));
            }
        }
    }
    automation
        .validate()
        .map_err(|error| definition_failure(&error))?;
    automation.updated_at_ms = now_ms();
    store
        .save_automation(&automation)
        .map_err(|error| failed("store_unavailable", format!("{error:#}"), "Try again."))?;
    let _ = store.add_prompt_version(&automation.id, &automation.prompt, source, now_ms());
    if as_json {
        return Ok(pretty(json(&automation, store)));
    }
    let mut saved = format!(
        "Saved {} ({}): {}{}.",
        automation.name,
        automation.id,
        describe(&automation),
        if automation.enabled { "" } else { ", paused" }
    );
    if let Some(note) = approval_note(&automation) {
        saved.push_str("\n");
        saved.push_str(&note);
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn options_apply_to_an_automation() {
        let project = tempfile::tempdir().unwrap();
        let mut automation = Automation::new(
            "",
            PathBuf::new(),
            "",
            "claude",
            "",
            WorkspaceMode::default(),
            0,
        );
        let flags = parse(&args(&[
            "--name",
            "Bug sweep",
            "--prompt",
            "Find bugs",
            "--schedule",
            "0 9 * * 1-5",
            "--project",
            project.path().to_str().unwrap(),
            "--workspace",
            "new",
            "--base-branch",
            "main",
            "--push",
            "--tag",
            "Nightly",
            "--tag",
            "bugs",
            "--timezone=Africa/Lagos",
        ]))
        .unwrap();
        apply(&mut automation, &flags).unwrap();
        assert_eq!(automation.name, "Bug sweep");
        assert_eq!(
            automation.workspace,
            WorkspaceMode::NewWorktree {
                base_branch: Some("main".into()),
                push_branch: true
            }
        );
        assert_eq!(
            automation.tags,
            vec!["bugs".to_string(), "nightly".to_string()]
        );
        assert_eq!(automation.timezone, "Africa/Lagos");
        assert!(automation.validate().is_ok());

        // A session can't branch: it falls back to a fresh folder per run.
        let flags = parse(&args(&["--session", "--no-tags"])).unwrap();
        apply(&mut automation, &flags).unwrap();
        assert!(automation.is_session());
        assert_eq!(automation.workspace, WorkspaceMode::Pinned { path: None });
        assert!(automation.tags.is_empty());

        assert!(parse(&args(&["--name"])).is_err());
        let flags = parse(&args(&["--workspace", "sideways"])).unwrap();
        assert!(apply(&mut automation, &flags).is_err());

        // A remote project's local stand-in can't host a run.
        let remote = crate::remote_fs::mounts_root().join("machine-1/home/me/app");
        let flags = parse(&args(&["--project", remote.to_str().unwrap()])).unwrap();
        let error = apply(&mut automation, &flags).unwrap_err();
        assert!(error.contains("remote machine"), "{error}");
    }

    #[test]
    fn partial_updates_keep_every_other_field() {
        let project = tempfile::tempdir().unwrap();
        let mut automation = Automation::new(
            "Nightly",
            project.path().to_path_buf(),
            "0 9 * * *",
            "claude",
            "Review",
            WorkspaceMode::default(),
            0,
        );
        automation.timezone = "UTC".into();
        automation.missed_policy = MissedPolicy::CatchUp;
        automation.max_retries = 2;
        automation.run_retention = 50;
        let before = automation.clone();
        // Updating one field preserves every other field.
        let flags = parse(&args(&["--name", "Renamed"])).unwrap();
        apply(&mut automation, &flags).unwrap();
        assert_eq!(automation.name, "Renamed");
        assert_eq!(automation.schedule, before.schedule);
        assert_eq!(automation.timezone, before.timezone);
        assert_eq!(automation.missed_policy, before.missed_policy);
        assert_eq!(automation.max_retries, before.max_retries);
        assert_eq!(automation.run_retention, before.run_retention);
        assert_eq!(automation.agent, before.agent);
        assert_eq!(automation.prompt, before.prompt);
        assert_eq!(automation.workspace, before.workspace);
        // Replacing the trigger is explicit: a new watch needs --watch, and
        // removing it needs --no-watch. A plain field update keeps the watch.
        let flags = parse(&args(&["--watch", "__unknown_service_xyz__"])).unwrap();
        assert!(
            apply(&mut automation, &flags).is_err(),
            "unknown services are rejected"
        );
        automation.watch = Some(dragent_automations::Watch {
            service: "linear".into(),
            query: "old".into(),
        });
        let flags = parse(&args(&["--name", "Still watched"])).unwrap();
        apply(&mut automation, &flags).unwrap();
        assert!(automation.watch.is_some(), "a field update keeps the watch");
        let flags = parse(&args(&["--no-watch"])).unwrap();
        apply(&mut automation, &flags).unwrap();
        assert!(
            automation.watch.is_none(),
            "--no-watch explicitly clears it"
        );
        // New policies apply cleanly.
        let flags = parse(&args(&[
            "--missed",
            "skip",
            "--overlap",
            "queue",
            "--max-retries",
            "3",
            "--retry-delay",
            "120",
            "--retention",
            "25",
        ]))
        .unwrap();
        apply(&mut automation, &flags).unwrap();
        assert_eq!(automation.missed_policy, MissedPolicy::Skip);
        assert_eq!(automation.overlap_policy, OverlapPolicy::Queue);
        assert_eq!(automation.max_retries, 3);
        assert_eq!(automation.retry_delay_secs, 120);
        assert_eq!(automation.run_retention, 25);
        assert!(automation.validate().is_ok());
        for bad in [
            vec!["--missed", "sometimes"],
            vec!["--overlap", "later"],
            vec!["--max-retries", "many"],
            vec!["--retention", "many"],
        ] {
            let flags = parse(&args(&bad)).unwrap();
            assert!(apply(&mut automation, &flags).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn invalid_schedules_fail_as_invalid_input() {
        let mut automation = Automation::new(
            "Nightly",
            PathBuf::from("/repo"),
            "not a schedule",
            "claude",
            "Review",
            WorkspaceMode::default(),
            0,
        );
        let error = definition_failure(&automation.validate().unwrap_err());
        let failure = Failure::decode(&error).expect("result contract failure");
        assert_eq!(failure.status, Status::InvalidInput);
        assert_eq!(failure.code, "invalid_schedule");
        assert!(
            failure.message.contains("The schedule isn't valid"),
            "{failure:?}"
        );
        automation.schedule = "0 9 * * *".into();
        automation.timezone = "Mars/Olympus".into();
        let error = definition_failure(&automation.validate().unwrap_err());
        let failure = Failure::decode(&error).expect("result contract failure");
        assert_eq!(
            (failure.status, failure.code.as_str()),
            (Status::InvalidInput, "invalid_timezone")
        );
    }

    #[test]
    fn preview_rejects_bad_input_as_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automations.sqlite");
        // Point the preview at a throwaway store by opening it directly: the
        // command itself is tested through the CLI binary, this checks the
        // validation mapping it relies on.
        let store = Store::open(&path).unwrap();
        let flags = parse(&args(&["--schedule", "not a schedule"])).unwrap();
        let error = preview(Some(&store), &flags, true).unwrap_err();
        let failure = Failure::decode(&error).expect("result contract failure");
        assert_eq!(
            (failure.status, failure.code.as_str()),
            (Status::InvalidInput, "invalid_schedule")
        );
        let flags = parse(&args(&[
            "--schedule",
            "0 9 * * *",
            "--timezone",
            "Mars/Olympus",
        ]))
        .unwrap();
        let error = preview(Some(&store), &flags, true).unwrap_err();
        assert_eq!(Failure::decode(&error).unwrap().code, "invalid_timezone");
        let flags = parse(&args(&["--schedule", "0 9 * * *", "--count", "0"])).unwrap();
        assert_eq!(
            Failure::decode(&preview(Some(&store), &flags, true).unwrap_err())
                .unwrap()
                .code,
            "invalid_count"
        );
    }
}
