//! `dragent standup`: a read-only recap of what happened across Dragent.
//!
//! Collects workspaces and branches, running agent sessions with a short
//! excerpt, teams with task results (including review and QA outcomes),
//! automation runs, and the artifact links inside those records. Each source
//! is collected independently: one failing source becomes an entry in
//! `errors` and makes the report `partial`, and never erases the rest.
//!
//! Strictly read-only: this command never starts agents, sends messages,
//! changes tasks, closes workspaces, or answers approval prompts. It only
//! reads the same local records the other commands read.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use dragent_host::outcome::{Failure, Status};

const HELP: &str = "\
Read-only recap of what happened across Dragent.

Usage: dragent standup [options]
       dragent standup setup [options]

Collects workspaces and branches, running agent sessions with a short
excerpt, teams with task results (including review and QA outcomes),
automation runs, and the artifact links in those records. Each source is
independent: one failing source becomes an entry in `errors` and makes the
report `partial`, without erasing the rest.

Options:
  --project PATH     Only work of this project (workspaces, agents, teams, automations)
  --since WHEN       Only items at or after WHEN: a duration (30m, 24h, 7d, 2w)
                     or an ISO date/datetime (2026-10-25, 2026-10-25T09:00:00Z)
  --limit N          At most N items per source (default 20, at most 100)
  --setup            Report current configuration instead: installed agents,
                     automations, connected services, skills, MCP servers, projects
  --json             Machine-readable JSON (default for agents)
  --text             Human-readable text
  --quiet            Only key ids and paths, one per line

Evidence: every result carries `evidence`: `verified` for a recorded result
or finished run status, `claimed` for an agent's own statement (a summary,
a reply), `excerpt` for terminal text. Linked work groups one workspace,
agent session, team task and automation run that share a workspace, branch
or agent pane, so the same work is not counted three times.

Read-only: starts nothing, sends nothing, changes nothing.
";

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;
const MAX_EXCERPT_CHARS: usize = 1200;
const MAX_TEXT_CHARS: usize = 2000;
const MAX_ARTIFACTS: usize = 100;
const RUN_PAGE_SIZE: usize = 25;

/// One source's failure inside a partial report.
#[derive(Clone, Debug)]
struct SourceError {
    source: &'static str,
    status: &'static str,
    code: &'static str,
    message: String,
}

impl SourceError {
    fn entry(&self) -> Value {
        json!({
            "source": self.source,
            "status": self.status,
            "code": self.code,
            "message": self.message,
        })
    }
}

fn source_failed(
    source: &'static str,
    status: Status,
    code: &'static str,
    message: String,
) -> SourceError {
    SourceError {
        source,
        status: status.as_str(),
        code,
        message,
    }
}

#[derive(Clone, Debug, Default)]
struct Opts {
    project: Option<PathBuf>,
    since_raw: Option<String>,
    limit: usize,
    setup: bool,
    json: bool,
    text: bool,
    quiet: bool,
    help: bool,
}

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

fn absolute(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    path.canonicalize().unwrap_or(path)
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts {
        limit: DEFAULT_LIMIT,
        ..Opts::default()
    };
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "setup" {
            opts.setup = true;
        } else if arg == "--setup" {
            opts.setup = true;
        } else if arg == "--json" {
            opts.json = true;
        } else if arg == "--text" {
            opts.text = true;
        } else if arg == "--quiet" {
            opts.quiet = true;
        } else if arg == "--help" || arg == "-h" || arg == "help" {
            opts.help = true;
        } else if arg == "--project" || arg == "--since" || arg == "--limit" {
            let name = arg.trim_start_matches("--");
            let value = iter.next().ok_or_else(|| {
                Failure::new(
                    Status::InvalidInput,
                    "usage",
                    format!("--{name} needs a value"),
                )
                .next("See `dragent standup --help`.")
                .encode()
            })?;
            assign(&mut opts, name, value)?;
        } else if let Some(name) = arg.strip_prefix("--") {
            if let Some((name, value)) = name.split_once('=') {
                assign(&mut opts, name, value)?;
            } else {
                return Err(Failure::new(
                    Status::InvalidInput,
                    "usage",
                    format!("unknown option `--{name}` (see `dragent standup --help`)"),
                )
                .next("See `dragent standup --help`.")
                .encode());
            }
        } else {
            return Err(Failure::new(
                Status::InvalidInput,
                "usage",
                format!("unknown argument `{arg}` (see `dragent standup --help`)"),
            )
            .next("See `dragent standup --help`.")
            .encode());
        }
    }
    if opts.json && opts.text {
        return Err(Failure::new(
            Status::InvalidInput,
            "usage",
            "use --json or --text, not both",
        )
        .encode());
    }
    Ok(opts)
}

fn assign(opts: &mut Opts, name: &str, value: &str) -> Result<(), String> {
    match name {
        "project" => opts.project = Some(absolute(value)),
        "since" => opts.since_raw = Some(value.to_string()),
        "limit" => {
            let limit: usize = value.parse().map_err(|_| {
                Failure::new(
                    Status::InvalidInput,
                    "usage",
                    format!("--limit takes a number, not `{value}`"),
                )
                .encode()
            })?;
            if limit == 0 {
                return Err(Failure::new(
                    Status::InvalidInput,
                    "usage",
                    "--limit takes a number from 1 to 100",
                )
                .encode());
            }
            opts.limit = limit.min(MAX_LIMIT);
        }
        _ => {
            return Err(Failure::new(
                Status::InvalidInput,
                "usage",
                format!("unknown option `--{name}` (see `dragent standup --help`)"),
            )
            .encode())
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `--since` as Unix milliseconds: a duration (`30m`, `24h`, `7d`, `2w`)
/// or an ISO date/datetime (`2026-10-25`, `2026-10-25T09:00:00Z`).
fn parse_since(raw: &str, now_ms: i64) -> Result<i64, String> {
    let text = raw.trim();
    if let Some(cut) = duration_ms(text) {
        return Ok(now_ms.saturating_sub(cut));
    }
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(time.timestamp_millis());
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let day = date.and_hms_opt(0, 0, 0).expect("midnight exists");
        return Ok(
            chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(day, chrono::Utc)
                .timestamp_millis(),
        );
    }
    Err(format!(
        "--since takes a duration like 30m, 24h, 7d, or an ISO date like 2026-10-25, not `{raw}`"
    ))
}

/// A duration string as milliseconds, or `None` when it isn't one.
fn duration_ms(text: &str) -> Option<i64> {
    let lower = text.trim().to_ascii_lowercase().replace(' ', "");
    let split = lower
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_ascii_digit())
        .map(|(index, _)| index + 1)
        .unwrap_or(0);
    let (number, unit) = lower.split_at(split);
    if number.is_empty() || unit.is_empty() {
        return None;
    }
    let amount: i64 = number.parse().ok()?;
    let factor: i64 = match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hour" | "hours" => 3_600_000,
        "d" | "day" | "days" => 86_400_000,
        "w" | "week" | "weeks" => 7 * 86_400_000,
        _ => return None,
    };
    amount.checked_mul(factor)
}

/// Keep items without a timestamp: absence of a time is not lateness.
fn within_window(at_ms: Option<i64>, since_ms: Option<i64>) -> bool {
    match (at_ms, since_ms) {
        (_, None) => true,
        (None, Some(_)) => true,
        (Some(at), Some(since)) => at >= since,
    }
}

/// Shorten to `max` characters, reporting whether anything was cut.
fn bound_text(text: &str, max: usize) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return (text.to_string(), false);
    }
    let mut short: String = chars.into_iter().take(max).collect();
    short.push('…');
    (short, true)
}

/// At most `limit` items, reporting whether the rest was dropped.
fn cap_list<T>(mut items: Vec<T>, limit: usize) -> (Vec<T>, bool) {
    if items.len() > limit {
        items.truncate(limit);
        (items, true)
    } else {
        (items, false)
    }
}

fn report_status(errors: &[Value]) -> &'static str {
    if errors.is_empty() {
        "ok"
    } else {
        "partial"
    }
}

/// Evidence for a recorded result or finished run status.
fn verified() -> &'static str {
    "verified"
}

/// Evidence for an agent's own statement (a summary, a reply).
fn claimed() -> &'static str {
    "claimed"
}

/// Evidence for terminal text.
fn excerpt_evidence() -> &'static str {
    "excerpt"
}

fn normalize_key(text: &str) -> String {
    let trimmed = text.trim().trim_end_matches('/');
    trimmed.to_string()
}

fn same_location(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && normalize_key(a) == normalize_key(b)
}

/// Pull artifact links out of free text: URLs and absolute paths.
/// Returns de-duplicated values in first-seen order.
fn extract_artifacts(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let clean = token.trim_matches(|ch: char| {
            ch.is_ascii_punctuation() && ch != '/' && ch != '~' && ch != '.' && ch != ':'
                || matches!(ch, '(' | ')' | '[' | ']' | '"' | '\'' | ',' | ';')
        });
        let clean = clean.trim_end_matches(|ch| matches!(ch, '.' | ',' | ';' | ':' | ')' | ']'));
        if clean.is_empty() {
            continue;
        }
        let is_url = clean.starts_with("http://") || clean.starts_with("https://");
        let is_path =
            clean.starts_with('/') && clean.len() > 1 || clean.starts_with("~/") && clean.len() > 2;
        if (is_url || is_path) && !out.contains(&clean.to_string()) {
            out.push(clean.to_string());
        }
        if out.len() >= MAX_ARTIFACTS {
            break;
        }
    }
    out
}

pub(crate) fn run(args: &[String]) -> Result<String, String> {
    let opts = parse(args)?;
    if opts.help {
        return Ok(HELP.trim_end().to_string());
    }
    let since_ms = match &opts.since_raw {
        Some(raw) => Some(parse_since(raw, now_ms()).map_err(|message| {
            Failure::new(Status::InvalidInput, "usage", message)
                .next("See `dragent standup --help`.")
                .encode()
        })?),
        None => None,
    };
    // JSON for agents unless the caller asked for text; `--quiet` prints ids.
    let json = opts.json || (!opts.text && !opts.quiet && agent_environment());
    if opts.setup {
        let report = setup_report(since_ms, opts.limit);
        return Ok(render_setup(&report, &opts, json));
    }
    let report = collect(since_ms, &opts);
    Ok(render(&report, &opts, json))
}

// ── Collection ───────────────────────────────────────────────────────────────

fn collect(since_ms: Option<i64>, opts: &Opts) -> Value {
    let generated = now_ms();
    let mut errors: Vec<Value> = Vec::new();
    // Liveness: without the host endpoint, excerpts and aliveness could not
    // be checked. Local records below still stand on their own.
    if dragent_host::HostClient::connect_existing().is_err() {
        errors.push(
            source_failed(
                "host",
                Status::Unavailable,
                "app_not_open",
                "Dragent's host isn't reachable, so agent aliveness and excerpts could not be checked; the records below are local only.".to_string(),
            )
            .entry(),
        );
    }
    let (workspaces, workspaces_truncated) =
        match collect_workspaces(opts.project.as_deref(), opts.limit) {
            Ok((rows, truncated)) => (rows, truncated),
            Err(error) => {
                errors.push(error.entry());
                (Vec::new(), false)
            }
        };
    let (agents, agents_truncated) =
        match collect_agents(opts.project.as_deref(), since_ms, opts.limit) {
            Ok((rows, truncated)) => (rows, truncated),
            Err(error) => {
                errors.push(error.entry());
                (Vec::new(), false)
            }
        };
    let (teams, teams_truncated) =
        match collect_teams(opts.project.as_deref(), since_ms, opts.limit) {
            Ok((rows, truncated)) => (rows, truncated),
            Err(error) => {
                errors.push(error.entry());
                (Vec::new(), false)
            }
        };
    let (automations, automations_truncated) =
        match collect_automations(opts.project.as_deref(), since_ms, opts.limit) {
            Ok((rows, truncated)) => (rows, truncated),
            Err(error) => {
                errors.push(error.entry());
                (Vec::new(), false)
            }
        };
    let linked = link_work(&workspaces, &agents, &teams, &automations);
    let artifacts = collect_artifacts(&teams, &automations, &agents);
    json!({
        "status": report_status(&errors),
        "generated_at_ms": generated,
        "scope": coverage(),
        "project": opts.project.as_ref().map(|path| path.display().to_string()),
        "since_ms": since_ms,
        "limit": opts.limit,
        "truncated": {
            "workspaces": workspaces_truncated,
            "agents": agents_truncated,
            "teams": teams_truncated,
            "automations": automations_truncated,
        },
        "workspaces": workspaces,
        "agents": agents,
        "teams": teams,
        "automations": automations,
        "linked": linked,
        "artifacts": artifacts,
        "errors": errors,
    })
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// Saved projects and workspaces, read-only from the persistence database.
fn saved_state() -> Result<(Vec<PathBuf>, Vec<(String, PathBuf)>), String> {
    let path = dragent_persistence::database_path().map_err(|error| format!("{error:#}"))?;
    if !path.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut conn = dragent_persistence::establish_readonly(&path)
        .map_err(|error| format!("couldn't read Dragent's data: {error:#}"))?;
    let session = dragent_persistence::load_document_session(&mut conn)
        .map_err(|error| format!("{error:#}"))?;
    let catalogue = dragent_persistence::workspace_catalogue::load_local_workspaces(&mut conn)
        .map_err(|error| format!("{error:#}"))?;
    let sessions_root = crate::workspace::project_naming::automation_sessions_root();
    let mut projects: Vec<PathBuf> = Vec::new();
    for root in session.open_project_roots {
        if root.is_dir() && !root.starts_with(&sessions_root) && !projects.contains(&root) {
            projects.push(root);
        }
    }
    let mut workspaces: Vec<(String, PathBuf)> = Vec::new();
    for workspace in session.workspaces.into_iter().chain(catalogue) {
        if workspace.root.is_dir() && !workspaces.iter().any(|(_, root)| root == &workspace.root) {
            workspaces.push((workspace.title, workspace.root));
        }
    }
    Ok((projects, workspaces))
}

fn project_of(root: &Path, projects: &[PathBuf]) -> Option<PathBuf> {
    let sessions = crate::workspace::project_naming::automation_sessions_root();
    (!root.starts_with(&sessions)).then(|| {
        crate::workspace::project_naming::sidebar_project_root(&root.to_path_buf(), projects)
    })
}

fn collect_workspaces(
    project: Option<&Path>,
    limit: usize,
) -> Result<(Vec<Value>, bool), SourceError> {
    let (projects, workspaces) = saved_state()
        .map_err(|message| source_failed("workspaces", Status::Failed, "read_failed", message))?;
    let mut rows: Vec<Value> = Vec::new();
    for (title, root) in workspaces {
        let owning = project_of(&root, &projects);
        if let Some(wanted) = project {
            if owning.as_deref() != Some(wanted) {
                continue;
            }
        }
        // A remote workspace's folder here is an empty stand-in: its branch
        // is on its machine.
        let branch = owning
            .as_ref()
            .filter(|_| !crate::cli::scope::is_remote(&root))
            .and_then(|_| crate::workspace::project_naming::local_branch_name(&root));
        rows.push(json!({
            "name": title,
            "path": root.display().to_string(),
            "machine": crate::cli::scope::machine_of(&root),
            "branch": branch,
            "project": owning.map(|path| path.display().to_string()),
            "evidence": verified(),
        }));
    }
    rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let (rows, truncated) = cap_list(rows, limit);
    Ok((rows, truncated))
}

#[derive(Clone, Debug)]
struct SessionRow {
    id: String,
    provider: String,
    state: String,
    since_ms: Option<i64>,
    cwd: String,
}

fn activity_state(event: &str) -> &'static str {
    match event {
        "Start" => "working",
        "PermissionRequest" => "needs_input",
        "Failed" => "failed",
        _ => "waiting",
    }
}

/// Agent sessions from the local records. Excerpts come from the host when
/// it answers; otherwise the session is still listed without one.
fn scan_sessions() -> Vec<SessionRow> {
    let dir = dragent_paths::organisation_dir().join("agent-sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut rows: Vec<SessionRow> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<uuid::Uuid>().ok())
        .filter_map(dragent_host::agent_sessions::load)
        .filter(|session| session.active)
        .map(|session| {
            let activity = dragent_host::agent_sessions::load_activity(session.pane_uuid);
            let (state, since_ms) = match activity {
                Some(activity) => (
                    activity_state(&activity.event).to_string(),
                    Some(activity.at_ms as i64),
                ),
                None => ("unknown".to_string(), None),
            };
            SessionRow {
                id: session.pane_uuid.to_string()[..8].to_string(),
                provider: session.provider,
                state,
                since_ms,
                cwd: session.cwd.display().to_string(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.cwd.cmp(&b.cwd));
    rows
}

fn short_excerpt(full: &str) -> (String, bool) {
    // Last rows first: what a person looking at the pane sees now.
    let mut rows: Vec<&str> = full.lines().collect();
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    let start = rows.len().saturating_sub(30);
    bound_text(&rows[start..].join("\n"), MAX_EXCERPT_CHARS)
}

fn collect_agents(
    project: Option<&Path>,
    since_ms: Option<i64>,
    limit: usize,
) -> Result<(Vec<Value>, bool), SourceError> {
    let sessions = scan_sessions();
    let client = dragent_host::HostClient::connect_existing().ok();
    let mut rows: Vec<Value> = Vec::new();
    for session in sessions {
        if let Some(wanted) = project {
            let cwd = Path::new(&session.cwd);
            if !cwd.starts_with(wanted) {
                continue;
            }
        }
        if !within_window(session.since_ms, since_ms) {
            continue;
        }
        let (excerpt, excerpt_truncated, excerpt_note) = match &client {
            Some(client) => {
                let pane: uuid::Uuid = session_full_id(&session.id)
                    .and_then(|id| dragent_host::agent_sessions::load(id))
                    .map(|loaded| loaded.pane_uuid)
                    .unwrap_or_else(|| uuid::Uuid::nil());
                // The full session record carries the workspace and terminal
                // ids the screen read needs; fall back to no excerpt when the
                // record is gone (it ended between the scan and this read).
                match read_excerpt(client, &pane) {
                    Ok(text) => {
                        let (short, truncated) = short_excerpt(&text);
                        (short, truncated, Value::Null)
                    }
                    Err(_) => (
                        String::new(),
                        false,
                        Value::String("the session ended before its screen could be read".into()),
                    ),
                }
            }
            None => (
                String::new(),
                false,
                Value::String("the host isn't reachable, so no excerpt".into()),
            ),
        };
        rows.push(json!({
            "id": session.id,
            "agent": session.provider,
            "state": session.state,
            "state_evidence": verified(),
            "since_ms": session.since_ms,
            "cwd": session.cwd,
            "excerpt": excerpt,
            "excerpt_truncated": excerpt_truncated,
            "excerpt_note": excerpt_note,
            "evidence": excerpt_evidence(),
        }));
    }
    let (rows, truncated) = cap_list(rows, limit);
    Ok((rows, truncated))
}

/// The session's full pane id from its eight-character prefix, when the
/// record is still there.
fn session_full_id(prefix: &str) -> Option<uuid::Uuid> {
    let dir = dragent_paths::organisation_dir().join("agent-sessions");
    std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<uuid::Uuid>().ok())
        .find(|id| id.to_string().starts_with(prefix))
}

fn read_excerpt(client: &dragent_host::HostClient, pane: &uuid::Uuid) -> Result<String, String> {
    let Some(session) = dragent_host::agent_sessions::load(*pane) else {
        return Err("session ended".into());
    };
    let screen = client.read_terminal_screen(&session.workspace_id, &session.terminal_id)?;
    let rows: Vec<String> = screen
        .lines
        .iter()
        .map(|row| {
            row.spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    let mut rows = rows;
    while rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    Ok(rows.join("\n"))
}

fn collect_teams(
    project: Option<&Path>,
    since_ms: Option<i64>,
    limit: usize,
) -> Result<(Vec<Value>, bool), SourceError> {
    let response =
        dragent_host::team::execute(dragent_host::team::TeamRequest::List { project_root: None })
            .map_err(|message| source_failed("teams", Status::Failed, "read_failed", message))?;
    let dragent_host::team::TeamResponse::Teams(summaries) = response else {
        return Err(source_failed(
            "teams",
            Status::Failed,
            "read_failed",
            "unexpected team answer".to_string(),
        ));
    };
    let mut rows: Vec<Value> = Vec::new();
    for summary in summaries {
        if let Some(wanted) = project {
            if Path::new(&summary.project_root) != wanted {
                continue;
            }
        }
        if !within_window(Some(summary.updated_at_ms), since_ms) {
            continue;
        }
        let response = dragent_host::team::execute(dragent_host::team::TeamRequest::Get {
            team: summary.id.clone(),
        })
        .map_err(|message| source_failed("teams", Status::Failed, "read_failed", message))?;
        let dragent_host::team::TeamResponse::Team(team) = response else {
            continue;
        };
        let mut tasks: Vec<Value> = Vec::new();
        for task in &team.tasks {
            if !within_window(Some(task.updated_at_ms), since_ms) {
                continue;
            }
            let (summary_text, summary_truncated) = task
                .result
                .as_ref()
                .map(|result| bound_text(&result.summary, MAX_TEXT_CHARS))
                .unwrap_or_default();
            tasks.push(json!({
                "id": task.id,
                "title": task.title,
                "status": task.status.key(),
                "kind": match task.kind {
                    dragent_host::team::TaskKind::Review => "review",
                    dragent_host::team::TaskKind::Work => "work",
                },
                "review_of": task.review_of,
                "qa": task.qa,
                "verdict": task.result.as_ref().and_then(|result| result.verdict.map(|verdict| match verdict {
                    dragent_host::team::Verdict::Approve => "approve",
                    dragent_host::team::Verdict::Changes => "changes",
                })),
                "workspace": task.workspace,
                "branch": task.branch,
                "updated_at_ms": task.updated_at_ms,
                "finished_at_ms": task.finished_at_ms,
                "evidence": verified(),
                "result": task.result.as_ref().map(|result| json!({
                    "summary": summary_text,
                    "summary_truncated": summary_truncated,
                    "files": result.files,
                    "checks": result.checks,
                    "evidence": claimed(),
                })),
                "reason": task.reason,
            }));
        }
        tasks.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let (tasks, tasks_truncated) = cap_list(tasks, limit);
        // Recent messages that need the person, so a recap can name the
        // concrete decision waiting on them.
        let mut waiting: Vec<Value> = Vec::new();
        for message in team.messages.iter().rev() {
            if waiting.len() >= limit {
                break;
            }
            let needs_user = message.kind == dragent_host::team::MessageKind::NeedsUser
                || message.to == dragent_host::team::Party::User && !message.read;
            if !needs_user || !within_window(Some(message.at_ms), since_ms) {
                continue;
            }
            let (body, truncated) = bound_text(&message.body, MAX_TEXT_CHARS);
            waiting.push(json!({
                "id": message.id,
                "from": message.from.label(),
                "kind": match message.kind {
                    dragent_host::team::MessageKind::Note => "note",
                    dragent_host::team::MessageKind::Question => "question",
                    dragent_host::team::MessageKind::Answer => "answer",
                    dragent_host::team::MessageKind::Handoff => "handoff",
                    dragent_host::team::MessageKind::Finding => "finding",
                    dragent_host::team::MessageKind::NeedsUser => "needs_user",
                },
                "body": body,
                "truncated": truncated,
                "at_ms": message.at_ms,
                "evidence": claimed(),
            }));
        }
        waiting.reverse();
        rows.push(json!({
            "id": team.id,
            "title": team.title,
            "project_root": team.project_root,
            "status": match team.status {
                dragent_host::team::TeamStatus::Planning => "planning",
                dragent_host::team::TeamStatus::Running => "running",
                dragent_host::team::TeamStatus::Done => "done",
                dragent_host::team::TeamStatus::Stopped => "stopped",
            },
            "updated_at_ms": team.updated_at_ms,
            "evidence": verified(),
            "tasks": tasks,
            "tasks_truncated": tasks_truncated,
            "waiting_on_user": waiting,
        }));
    }
    rows.sort_by(|a, b| {
        b["updated_at_ms"]
            .as_i64()
            .cmp(&a["updated_at_ms"].as_i64())
    });
    let (rows, truncated) = cap_list(rows, limit);
    Ok((rows, truncated))
}

fn collect_automations(
    project: Option<&Path>,
    since_ms: Option<i64>,
    limit: usize,
) -> Result<(Vec<Value>, bool), SourceError> {
    let path = dragent_paths::organisation_dir().join("automations.sqlite");
    if !path.exists() {
        return Ok((Vec::new(), false));
    }
    let store = dragent_automations::Store::open(&path).map_err(|error| {
        source_failed(
            "automations",
            Status::Failed,
            "read_failed",
            format!("{error:#}"),
        )
    })?;
    let mut automations = store.list_automations().map_err(|error| {
        source_failed(
            "automations",
            Status::Failed,
            "read_failed",
            format!("{error:#}"),
        )
    })?;
    automations.sort_by(|a, b| a.name.cmp(&b.name));
    let mut rows: Vec<Value> = Vec::new();
    for automation in automations {
        if let Some(wanted) = project {
            if automation.is_session() || automation.project_root.as_path() != wanted {
                continue;
            }
        }
        let mut runs = paged_runs(&store, &automation.id, since_ms, limit).map_err(|message| {
            source_failed("automations", Status::Failed, "read_failed", message)
        })?;
        // Newest first for the recap; the store already pages that way.
        runs.sort_by(|a: &Value, b: &Value| b["at_ms"].as_i64().cmp(&a["at_ms"].as_i64()));
        rows.push(json!({
            "id": automation.id,
            "name": automation.name,
            "enabled": automation.enabled,
            "schedule": automation.schedule,
            "agent": automation.agent,
            "project": (!automation.is_session()).then(|| automation.project_root.display().to_string()),
            "updated_at_ms": automation.updated_at_ms,
            "evidence": verified(),
            "runs": runs,
        }));
        if rows.len() >= limit {
            break;
        }
    }
    let (rows, truncated) = cap_list(rows, limit);
    Ok((rows, truncated))
}

/// Runs newest first, following the store's `before_seq` pages until `limit`
/// runs are collected or older runs fall outside the window.
fn paged_runs(
    store: &dragent_automations::Store,
    automation_id: &str,
    since_ms: Option<i64>,
    limit: usize,
) -> Result<Vec<Value>, String> {
    use dragent_automations::RunFilter;
    let mut out: Vec<Value> = Vec::new();
    let mut before: Option<i64> = None;
    loop {
        let page = store
            .list_runs(automation_id, RunFilter::All, before, RUN_PAGE_SIZE)
            .map_err(|error| format!("{error:#}"))?;
        if page.is_empty() {
            break;
        }
        for run in &page {
            before = Some(run.seq);
            let at = run.display_time_ms().or(run.result_at_ms());
            if !within_window(at, since_ms) {
                // Runs come newest first: once one is older than the window
                // and the window is set, the rest are older too.
                if since_ms.is_some() && at.is_some() {
                    // Still finish this page's window check, then stop.
                    continue;
                }
            }
            let agent_state = run
                .progress
                .as_ref()
                .map(|progress| progress.state.as_str());
            let (reply, reply_truncated) = run
                .progress
                .as_ref()
                .and_then(|progress| progress.reply.as_deref())
                .map(|reply| bound_text(reply, MAX_TEXT_CHARS))
                .unwrap_or_default();
            // Stop paging past the window once a dated run is too old.
            if since_ms.is_some_and(|since| at.is_some_and(|at| at < since)) {
                continue;
            }
            out.push(json!({
                "id": run.id,
                "status": run.status.as_str(),
                "agent_state": agent_state,
                "at_ms": at,
                "workspace": run.workspace_path.as_ref().map(|path| path.display().to_string()),
                "branch": run.branch,
                "pane": run.agent_pane(),
                "reply": reply,
                "reply_truncated": reply_truncated,
                "reply_evidence": claimed(),
                "error": run.error.as_ref().map(|error| json!({
                    "step": match error.step {
                        dragent_automations::RunStep::Queue => "queue",
                        dragent_automations::RunStep::Workspace => "workspace",
                        dragent_automations::RunStep::Agent => "agent",
                        dragent_automations::RunStep::Run => "run",
                    },
                    "code": error.code,
                    "message": error.message,
                })),
                "evidence": verified(),
            }));
            if out.len() >= limit {
                break;
            }
        }
        if out.len() >= limit || page.len() < RUN_PAGE_SIZE {
            break;
        }
        // If every run on this page was older than the window, stop paging.
        if since_ms.is_some()
            && page.iter().all(|run| {
                let at = run.display_time_ms().or(run.result_at_ms());
                at.is_some_and(|at| at < since_ms.unwrap())
            })
        {
            break;
        }
    }
    let (out, _) = cap_list(out, limit);
    Ok(out)
}

/// One group per shared workspace, branch or agent pane, so the same work
/// counted in several sources is read once.
fn link_work(
    workspaces: &[Value],
    agents: &[Value],
    teams: &[Value],
    automations: &[Value],
) -> Vec<Value> {
    #[derive(Default)]
    struct Group {
        workspace: Option<String>,
        branch: Option<String>,
        teams: Vec<String>,
        tasks: Vec<String>,
        agents: Vec<String>,
        runs: Vec<String>,
        reasons: Vec<String>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut place = |workspace: Option<&str>,
                     branch: Option<&str>,
                     pane: Option<&str>,
                     add: &dyn Fn(&mut Group)| {
        let hit = groups.iter_mut().find(|group| {
            workspace.is_some_and(|wanted| {
                group
                    .workspace
                    .as_deref()
                    .is_some_and(|have| same_location(have, wanted))
            }) || branch
                .is_some_and(|wanted| !wanted.is_empty() && group.branch.as_deref() == Some(wanted))
                || pane.is_some_and(|wanted| {
                    !wanted.is_empty()
                        && (group.agents.iter().any(|have| have == wanted)
                            || group.tasks.iter().any(|have| have == wanted)
                            || group.runs.iter().any(|have| have == wanted))
                })
        });
        let group = match hit {
            Some(group) => group,
            None => {
                groups.push(Group::default());
                groups.last_mut().expect("pushed")
            }
        };
        if group.workspace.is_none() {
            group.workspace = workspace.map(str::to_string);
        }
        if group.branch.is_none() {
            group.branch = branch
                .filter(|branch| !branch.is_empty())
                .map(str::to_string);
        }
        add(group);
        let reason = if workspace.is_some() {
            "shared workspace"
        } else if branch.is_some_and(|branch| !branch.is_empty()) {
            "shared branch"
        } else {
            "shared agent pane"
        };
        if !group.reasons.iter().any(|have| have == reason) {
            group.reasons.push(reason.to_string());
        }
    };
    for workspace in workspaces {
        let path = workspace["path"].as_str();
        let branch = workspace["branch"].as_str();
        place(path, branch, None, &|_| {});
    }
    for agent in agents {
        let id = agent["id"].as_str().unwrap_or_default().to_string();
        let cwd = agent["cwd"].as_str();
        place(cwd, None, Some(&id), &|group| {
            if !group.agents.contains(&id) {
                group.agents.push(id.clone());
            }
        });
    }
    for team in teams {
        let team_id = team["id"].as_str().unwrap_or_default().to_string();
        for task in team["tasks"].as_array().into_iter().flatten() {
            let task_id = format!("{} {}", team_id, task["id"].as_str().unwrap_or_default());
            let workspace = task["workspace"].as_str();
            let branch = task["branch"].as_str();
            place(workspace, branch, None, &|group| {
                if !group.teams.contains(&team_id) {
                    group.teams.push(team_id.clone());
                }
                if !group.tasks.contains(&task_id) {
                    group.tasks.push(task_id.clone());
                }
            });
        }
    }
    for automation in automations {
        let automation_id = automation["id"].as_str().unwrap_or_default().to_string();
        for run in automation["runs"].as_array().into_iter().flatten() {
            let run_id = format!(
                "{} {}",
                automation_id,
                run["id"].as_str().unwrap_or_default()
            );
            let workspace = run["workspace"].as_str();
            let branch = run["branch"].as_str();
            let pane = run["pane"].as_str();
            place(workspace, branch, pane, &|group| {
                if !group.runs.contains(&run_id) {
                    group.runs.push(run_id.clone());
                }
            });
        }
    }
    groups
        .into_iter()
        .enumerate()
        .filter(|(_, group)| {
            let count = usize::from(group.workspace.is_some())
                + group.teams.len()
                + group.tasks.len()
                + group.agents.len()
                + group.runs.len();
            count > 1
                || group.teams.len() + group.tasks.len() + group.agents.len() + group.runs.len() > 1
        })
        .map(|(index, group)| {
            json!({
                "id": format!("link-{}", index + 1),
                "workspace": group.workspace,
                "branch": group.branch,
                "teams": group.teams,
                "tasks": group.tasks,
                "agents": group.agents,
                "runs": group.runs,
                "reason": group.reasons.join(", "),
            })
        })
        .collect()
}

fn collect_artifacts(teams: &[Value], automations: &[Value], agents: &[Value]) -> Vec<Value> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    let mut push = |value: &str, from: String| {
        for artifact in extract_artifacts(value) {
            if seen.contains(&artifact) || out.len() >= MAX_ARTIFACTS {
                continue;
            }
            seen.push(artifact.clone());
            out.push(json!({ "value": artifact, "from": from, "evidence": verified() }));
        }
    };
    for team in teams {
        for task in team["tasks"].as_array().into_iter().flatten() {
            let from = format!(
                "team {} task {}",
                team["id"].as_str().unwrap_or("?"),
                task["id"].as_str().unwrap_or("?")
            );
            if let Some(summary) = task["result"]["summary"].as_str() {
                push(summary, from.clone());
            }
            if let Some(reason) = task["reason"].as_str() {
                push(reason, from.clone());
            }
            for file in task["result"]["files"].as_array().into_iter().flatten() {
                if let Some(file) = file.as_str() {
                    push(file, from.clone());
                }
            }
            for check in task["result"]["checks"].as_array().into_iter().flatten() {
                if let Some(check) = check.as_str() {
                    push(check, from.clone());
                }
            }
        }
        for message in team["waiting_on_user"].as_array().into_iter().flatten() {
            if let Some(body) = message["body"].as_str() {
                push(
                    body,
                    format!(
                        "team {} message {}",
                        team["id"].as_str().unwrap_or("?"),
                        message["id"]
                    ),
                );
            }
        }
    }
    for automation in automations {
        for run in automation["runs"].as_array().into_iter().flatten() {
            if let Some(reply) = run["reply"].as_str() {
                push(
                    reply,
                    format!(
                        "automation {} run {}",
                        automation["id"].as_str().unwrap_or("?"),
                        run["id"].as_str().unwrap_or("?")
                    ),
                );
            }
        }
    }
    for agent in agents {
        if let Some(excerpt) = agent["excerpt"].as_str() {
            push(
                excerpt,
                format!("agent {}", agent["id"].as_str().unwrap_or("?")),
            );
        }
    }
    out
}

// ── Setup (current configuration only) ───────────────────────────────────────

fn setup_report(since_ms: Option<i64>, limit: usize) -> Value {
    let _ = since_ms;
    let generated = now_ms();
    let mut errors: Vec<Value> = Vec::new();
    let agents = installed_agents();
    let (automations, automations_truncated) = match setup_automations(limit) {
        Ok((rows, truncated)) => (rows, truncated),
        Err(error) => {
            errors.push(error.entry());
            (Vec::new(), false)
        }
    };
    let services = setup_services(&mut errors);
    let skills = setup_skills();
    let mcp_servers = setup_mcp_servers(&mut errors);
    let (projects, projects_truncated) = match setup_projects() {
        Ok(rows) => {
            let (rows, truncated) = cap_list(rows, limit);
            (rows, truncated)
        }
        Err(error) => {
            errors.push(error.entry());
            (Vec::new(), false)
        }
    };
    json!({
        "status": report_status(&errors),
        "generated_at_ms": generated,
        "mode": "setup",
        "limit": limit,
        "truncated": { "automations": automations_truncated, "projects": projects_truncated },
        "installed_agents": agents,
        "automations": automations,
        "services": services,
        "skills": skills,
        "mcp_servers": mcp_servers,
        "projects": projects,
        "errors": errors,
    })
}

fn installed_agents() -> Vec<Value> {
    let installed = crate::workspace::detect_prompt_agents();
    crate::workspace::PromptAgent::ALL
        .into_iter()
        .filter(|agent| {
            *agent == crate::workspace::PromptAgent::Dragent || installed.contains(agent)
        })
        .map(|agent| {
            json!({
                "key": agent.command_key(),
                "name": agent.label(),
                "evidence": verified(),
            })
        })
        .collect()
}

fn setup_automations(limit: usize) -> Result<(Vec<Value>, bool), SourceError> {
    let path = dragent_paths::organisation_dir().join("automations.sqlite");
    if !path.exists() {
        return Ok((Vec::new(), false));
    }
    let store = dragent_automations::Store::open(&path).map_err(|error| {
        source_failed(
            "automations",
            Status::Failed,
            "read_failed",
            format!("{error:#}"),
        )
    })?;
    let mut automations = store.list_automations().map_err(|error| {
        source_failed(
            "automations",
            Status::Failed,
            "read_failed",
            format!("{error:#}"),
        )
    })?;
    automations.sort_by(|a, b| a.name.cmp(&b.name));
    let rows: Vec<Value> = automations
        .into_iter()
        .map(|automation| {
            json!({
                "id": automation.id,
                "name": automation.name,
                "enabled": automation.enabled,
                "schedule": automation.schedule,
                "timezone": automation.timezone,
                "agent": automation.agent,
                "project": (!automation.is_session()).then(|| automation.project_root.display().to_string()),
                "watch": automation.watch.map(|watch| json!({
                    "service": watch.service,
                    "query": watch.query,
                })),
                "evidence": verified(),
            })
        })
        .collect();
    Ok(cap_list(rows, limit))
}

fn setup_services(errors: &mut Vec<Value>) -> Vec<Value> {
    // `services()` reads local connection records; a failure to read them is
    // a partial setup report, not an empty service list.
    match std::panic::catch_unwind(crate::integrations::tools::services) {
        Ok(rows) => rows.as_array().cloned().unwrap_or_default(),
        Err(_) => {
            errors.push(
                source_failed(
                    "services",
                    Status::Failed,
                    "read_failed",
                    "connected services could not be read".to_string(),
                )
                .entry(),
            );
            Vec::new()
        }
    }
}

fn setup_skills() -> Value {
    let bundled: Vec<String> = dragent_host::agent_skills::BUNDLED_SKILLS
        .iter()
        .map(|skill| skill.name.to_string())
        .collect();
    json!({
        "bundled": bundled,
        "shared_folders_installed": crate::cli::agent_skill::enabled(),
        "desktop_guidance_installed": crate::cli::agent_skill::computer_enabled(),
        "evidence": verified(),
    })
}

fn setup_mcp_servers(errors: &mut Vec<Value>) -> Vec<Value> {
    // Missing file means no servers, not a failure.
    let path = crate::settings::mcp_config::global_mcp_path();
    if !path.exists() {
        return Vec::new();
    }
    let entries = crate::settings::mcp_config::load_entries();
    // `load_entries` reads one file; an empty result with a present file is
    // still reported, with the file's presence as the evidence.
    let _ = errors;
    entries
        .into_iter()
        .map(|server| {
            json!({
                "name": server.name,
                "transport": server.transport.label(),
                "command": server.display_command(),
                "enabled": server.enabled,
                "evidence": verified(),
            })
        })
        .collect()
}

fn setup_projects() -> Result<Vec<Value>, SourceError> {
    let (projects, _) = saved_state()
        .map_err(|message| source_failed("projects", Status::Failed, "read_failed", message))?;
    let mut rows: Vec<Value> = projects
        .into_iter()
        .map(|root| {
            json!({
                "name": project_name(&root),
                "path": root.display().to_string(),
                "evidence": verified(),
            })
        })
        .collect();
    rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(rows)
}

// ── Rendering ────────────────────────────────────────────────────────────────

fn render(report: &Value, opts: &Opts, json: bool) -> String {
    if opts.quiet {
        return quiet_ids(report);
    }
    if json {
        return serde_json::to_string_pretty(report).unwrap_or_default();
    }
    render_text(report)
}

fn quiet_ids(report: &Value) -> String {
    let mut out: Vec<String> = Vec::new();
    for workspace in report["workspaces"].as_array().into_iter().flatten() {
        if let Some(path) = workspace["path"].as_str() {
            out.push(path.to_string());
        }
    }
    for team in report["teams"].as_array().into_iter().flatten() {
        if let Some(id) = team["id"].as_str() {
            out.push(id.to_string());
        }
    }
    for automation in report["automations"].as_array().into_iter().flatten() {
        if let Some(id) = automation["id"].as_str() {
            out.push(id.to_string());
        }
    }
    out.join("\n")
}

fn render_text(report: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Standup · {} · {} workspace{}, {} agent{}, {} team{}, {} automation{}\n",
        report["status"].as_str().unwrap_or("ok"),
        count(&report["workspaces"]),
        plural(count(&report["workspaces"])),
        count(&report["agents"]),
        plural(count(&report["agents"])),
        count(&report["teams"]),
        plural(count(&report["teams"])),
        count(&report["automations"]),
        plural(count(&report["automations"])),
    ));
    if let Some(since) = report["since_ms"].as_i64() {
        out.push_str(&format!("Since: {}\n", since));
    }
    section(&mut out, "Workspaces", &report["workspaces"], |item| {
        format!(
            "{} · branch {} · {}",
            item["path"].as_str().unwrap_or("-"),
            item["branch"].as_str().unwrap_or("-"),
            item["project"].as_str().unwrap_or("sessions"),
        )
    });
    section(&mut out, "Agents", &report["agents"], |item| {
        format!(
            "{} {} · {} · {}",
            item["id"].as_str().unwrap_or("?"),
            item["agent"].as_str().unwrap_or("agent"),
            item["state"].as_str().unwrap_or("unknown"),
            item["cwd"].as_str().unwrap_or("-"),
        )
    });
    section(&mut out, "Teams", &report["teams"], |item| {
        let done: Vec<&str> = item["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|task| task["status"].as_str() == Some("done"))
            .filter_map(|task| task["id"].as_str())
            .collect();
        format!(
            "{} {} · {} · done: {}",
            item["id"].as_str().unwrap_or("?"),
            item["title"].as_str().unwrap_or(""),
            item["status"].as_str().unwrap_or("?"),
            if done.is_empty() {
                "-".to_string()
            } else {
                done.join(", ")
            },
        )
    });
    section(&mut out, "Automations", &report["automations"], |item| {
        let latest = item["runs"].as_array().and_then(|runs| runs.first());
        format!(
            "{} {} · {} · latest: {}",
            item["id"].as_str().unwrap_or("?"),
            item["name"].as_str().unwrap_or(""),
            if item["enabled"].as_bool() == Some(true) {
                "active"
            } else {
                "paused"
            },
            latest
                .map(|run| format!(
                    "{} {}",
                    run["status"].as_str().unwrap_or("?"),
                    run["agent_state"].as_str().unwrap_or("")
                ))
                .unwrap_or_else(|| "no runs".to_string()),
        )
    });
    let linked = report["linked"].as_array().cloned().unwrap_or_default();
    if !linked.is_empty() {
        out.push_str("\nLinked work (same work in several sources):\n");
        for group in linked {
            out.push_str(&format!(
                "  {} · {} · teams {} · tasks {} · agents {} · runs {} ({})\n",
                group["id"].as_str().unwrap_or("?"),
                group["workspace"]
                    .as_str()
                    .or(group["branch"].as_str())
                    .unwrap_or("-"),
                join(&group["teams"]),
                join(&group["tasks"]),
                join(&group["agents"]),
                join(&group["runs"]),
                group["reason"].as_str().unwrap_or(""),
            ));
        }
    }
    let artifacts = report["artifacts"].as_array().cloned().unwrap_or_default();
    if !artifacts.is_empty() {
        out.push_str("\nArtifacts:\n");
        for artifact in artifacts.iter().take(20) {
            out.push_str(&format!(
                "  {} (from {})\n",
                artifact["value"].as_str().unwrap_or(""),
                artifact["from"].as_str().unwrap_or("?"),
            ));
        }
    }
    let errors = report["errors"].as_array().cloned().unwrap_or_default();
    if !errors.is_empty() {
        out.push_str("\nIncomplete sources:\n");
        for error in errors {
            out.push_str(&format!(
                "  {}: {} ({})\n",
                error["source"].as_str().unwrap_or("?"),
                error["message"].as_str().unwrap_or(""),
                error["code"].as_str().unwrap_or(""),
            ));
        }
    }
    out.trim_end().to_string()
}

fn render_setup(report: &Value, opts: &Opts, json: bool) -> String {
    if opts.quiet {
        let mut out: Vec<String> = Vec::new();
        for agent in report["installed_agents"].as_array().into_iter().flatten() {
            if let Some(key) = agent["key"].as_str() {
                out.push(key.to_string());
            }
        }
        for project in report["projects"].as_array().into_iter().flatten() {
            if let Some(path) = project["path"].as_str() {
                out.push(path.to_string());
            }
        }
        return out.join("\n");
    }
    if json {
        return serde_json::to_string_pretty(report).unwrap_or_default();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "Setup · {} · current configuration only\n",
        report["status"].as_str().unwrap_or("ok")
    ));
    section(
        &mut out,
        "Installed agents",
        &report["installed_agents"],
        |item| item["key"].as_str().unwrap_or("?").to_string(),
    );
    section(&mut out, "Automations", &report["automations"], |item| {
        format!(
            "{} {} · {}",
            item["name"].as_str().unwrap_or("?"),
            item["schedule"].as_str().unwrap_or(""),
            if item["enabled"].as_bool() == Some(true) {
                "active"
            } else {
                "paused"
            },
        )
    });
    section(
        &mut out,
        "Connected services",
        &report["services"],
        |item| {
            format!(
                "{} · {}",
                item["service"].as_str().unwrap_or("?"),
                if item["connected"].as_bool() == Some(true) {
                    "connected"
                } else {
                    "not connected"
                },
            )
        },
    );
    section(&mut out, "Projects", &report["projects"], |item| {
        item["path"].as_str().unwrap_or("?").to_string()
    });
    let errors = report["errors"].as_array().cloned().unwrap_or_default();
    if !errors.is_empty() {
        out.push_str("\nIncomplete sources:\n");
        for error in errors {
            out.push_str(&format!(
                "  {}: {}\n",
                error["source"].as_str().unwrap_or("?"),
                error["message"].as_str().unwrap_or(""),
            ));
        }
    }
    out.trim_end().to_string()
}

fn count(value: &Value) -> usize {
    value.as_array().map(Vec::len).unwrap_or(0)
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn join(value: &Value) -> String {
    let items: Vec<&str> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str())
        .collect();
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(", ")
    }
}

fn section(out: &mut String, title: &str, items: &Value, line: impl Fn(&Value) -> String) {
    let items = items.as_array().cloned().unwrap_or_default();
    out.push_str(&format!("\n{title} ({}):\n", items.len()));
    if items.is_empty() {
        out.push_str("  -\n");
        return;
    }
    for item in items {
        out.push_str(&format!("  {}\n", line(&item)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(items: &[&str]) -> Vec<Value> {
        items.iter().map(|item| json!({"id": item})).collect()
    }

    #[test]
    fn partial_when_one_source_fails() {
        let mut errors: Vec<Value> = Vec::new();
        let workspaces = vec![json!({"path": "/repo/a"})];
        errors.push(
            source_failed(
                "teams",
                Status::Unavailable,
                "app_not_open",
                "closed".into(),
            )
            .entry(),
        );
        let report = json!({
            "status": report_status(&errors),
            "workspaces": workspaces,
            "teams": Vec::<Value>::new(),
            "errors": errors,
        });
        assert_eq!(report["status"], "partial");
        assert_eq!(report["workspaces"].as_array().unwrap().len(), 1);
        assert_eq!(report["errors"][0]["source"], "teams");
        assert_eq!(report["errors"][0]["status"], "unavailable");
        assert_eq!(report["errors"][0]["code"], "app_not_open");
    }

    #[test]
    fn same_work_links_across_sources() {
        let workspaces = vec![json!({"path": "/repo/a", "branch": "feat-x"})];
        let agents = vec![json!({"id": "aaaa1111", "cwd": "/repo/a"})];
        let teams = vec![json!({
            "id": "team-1",
            "tasks": [{"id": "t1", "workspace": "/repo/a", "branch": "feat-x"}],
        })];
        let automations = vec![json!({
            "id": "auto-1",
            "runs": [{"id": "run-1", "workspace": "/repo/a", "branch": "feat-x"}],
        })];
        let linked = link_work(&workspaces, &agents, &teams, &automations);
        assert_eq!(linked.len(), 1, "one group for the same work: {linked:?}");
        assert_eq!(linked[0]["agents"], json!(["aaaa1111"]));
        assert!(linked[0]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task.as_str().unwrap().contains("t1")));
        assert!(linked[0]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run.as_str().unwrap().contains("run-1")));
    }

    #[test]
    fn evidence_marks_verified_claimed_and_excerpt() {
        assert_eq!(verified(), "verified");
        assert_eq!(claimed(), "claimed");
        assert_eq!(excerpt_evidence(), "excerpt");
        // A recorded team result and a finished run are verified; the words
        // inside them are the agent's claim; terminal text is an excerpt.
        let task = json!({"status": "done", "evidence": verified(), "result": {"summary": "did it", "evidence": claimed()}});
        assert_eq!(task["evidence"], "verified");
        assert_eq!(task["result"]["evidence"], "claimed");
        let agent = json!({"state_evidence": verified(), "evidence": excerpt_evidence()});
        assert_eq!(agent["state_evidence"], "verified");
        assert_eq!(agent["evidence"], "excerpt");
        let _ = value(&["a"]);
    }

    #[test]
    fn since_filters_by_time_and_keeps_undated() {
        let now = 1_000_000i64;
        let since = now - 86_400_000;
        assert!(within_window(Some(now), Some(since)));
        assert!(!within_window(Some(since - 1), Some(since)));
        assert!(
            within_window(None, Some(since)),
            "no timestamp is not lateness"
        );
        assert!(within_window(Some(1), None));
        assert_eq!(parse_since("7d", now).unwrap(), now - 7 * 86_400_000);
        assert_eq!(parse_since("24h", now).unwrap(), now - 24 * 3_600_000);
        assert_eq!(parse_since("30m", now).unwrap(), now - 30 * 60_000);
        assert!(parse_since("2026-10-25", now).unwrap() > 0);
        assert!(parse_since("not-a-time", now).is_err());
    }

    #[test]
    fn output_is_bounded_per_source_and_per_string() {
        let items: Vec<Value> = (0..50).map(|index| json!({"id": index})).collect();
        let (capped, truncated) = cap_list(items, 20);
        assert_eq!(capped.len(), 20);
        assert!(truncated);
        let (short, cut) = bound_text(&"x".repeat(5000), MAX_EXCERPT_CHARS);
        assert!(cut);
        assert!(short.chars().count() <= MAX_EXCERPT_CHARS + 1);
        let artifacts = extract_artifacts(
            "see https://example.com/x and /repo/a/b for details https://example.com/x",
        );
        assert_eq!(
            artifacts,
            vec!["https://example.com/x".to_string(), "/repo/a/b".to_string()]
        );
    }
}

/// Which machines the report covers. Agent sessions, teams and automations
/// are read from this computer; a remote machine's own teams and sessions
/// live on that machine and are named here as not covered.
fn coverage() -> Value {
    let remote: Vec<String> = dragent_host::machine_registry::MachineRegistry::load()
        .map(|registry| registry.machines.iter().map(|machine| machine.name()).collect())
        .unwrap_or_default();
    json!({
        "machine": dragent_host::scope::this_machine(),
        "not_covered": remote,
        "note": if remote.is_empty() {
            "Everything here is on this computer."
        } else {
            "Agent sessions, teams and automations are this computer's. Workspaces on remote machines are listed with their machine; their teams and sessions run there (`dragent team list` in one of their terminals)."
        },
    })
}

