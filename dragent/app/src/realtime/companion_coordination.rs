//! Live Agent-coordination reads for the Dragent companion (native host).
//!
//! Read-only phone projections over the host's team store
//! (`dragent_host::team`), served through the existing companion
//! host-request/reply framing (`companion_host` dispatches here).
//!
//! Methods (allowlisted on the native host):
//! - `coordination.list`: `{session_id, expires_at, project_id?}` ->
//!   `{items:[{id,title,project_path,status,task_count,done_count,
//!   running_count,blocked_count,needs_user,updated_at}], truncated}`
//! - `coordination.get`: `{session_id, expires_at, team_id}` ->
//!   `{team:{id,title,project_path,status,goal,tasks:[{id,title,status,
//!   agent,model,depends_on,result_summary}], messages:[{id,from,label,
//!   kind,text,created_at}]}, truncated}`
//!
//! Safety rules (mirroring `companion_host` + `desktop_publisher`):
//! - The UI thread only ever *notes* the authoritative sidebar project
//!   allowlist (already-resolved in-memory metadata, no disk/network). The
//!   daemon worker owns all `HostClient.team` I/O.
//! - `List` is never called with `project_root: None`: one filtered `List`
//!   per allowlisted project, merged. A phone-supplied `project_id` must be
//!   in the allowlist or it is `gone` (sanitized, no leak).
//! - `Get` verifies the returned team's `project_root` is allowlisted,
//!   otherwise `gone`.
//! - The current [`AccountKey`] owner is verified before *and* after daemon
//!   I/O; any account/org change invalidates the result (`gone`).
//! - No mutation: no `Inbox` read-marking, no team writes.
//! - Projection only: task prompts, workspaces, panes/terminals, lead
//!   sessions, credentials, env and courier state are never emitted.
//! - Bounds: 100 teams / 60 tasks / 100 messages, titles to 512 chars,
//!   message bodies to 8000 chars, total response `<= 512 KiB`, with
//!   `truncated: true` whenever anything was cut to fit.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::desktop_catalog::AccountKey;

// ── Bounds ─────────────────────────────────────────────────────────────────

/// Largest phone-visible team list (one entry per team).
pub(crate) const MAX_TEAMS: usize = 100;
/// Largest task list inside a detail view (matches the store's own cap).
pub(crate) const MAX_TASKS: usize = 60;
/// Largest message list inside a detail view (store keeps 2000; phones see
/// the most recent slice).
pub(crate) const MAX_MESSAGES: usize = 100;
/// Title/goal/summary text bound (chars, char-boundary truncated).
pub(crate) const MAX_TITLE_CHARS: usize = 512;
/// Message body bound (chars, char-boundary truncated).
pub(crate) const MAX_MESSAGE_CHARS: usize = 8000;
/// Project path bound (chars). Paths longer than this are invalid.
pub(crate) const MAX_PROJECT_CHARS: usize = 4096;
/// Team id bound (chars). Store ids are `team-` + 8 hex; the bound only
/// rejects pathological input without leaking.
pub(crate) const MAX_TEAM_ID_CHARS: usize = 128;
/// Per-task dependency entries carried (team has at most 60 tasks).
pub(crate) const MAX_DEPENDS: usize = 20;
/// Total serialized result bound (matches the host-reply frame budget).
pub(crate) const MAX_RESPONSE: usize = 512 * 1024;
/// Inbound params bound (matches `companion_host`).
pub(crate) const MAX_PARAMS: usize = 64 * 1024;
/// Coordination lease, mirroring the terminal bridge (24h).
pub(crate) const MAX_LEASE_MS: u64 = 24 * 60 * 60 * 1000;
/// Largest integer the relay/phone path carries exactly (`2^53 - 1`).
pub(crate) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

// ── Owner + allowlist registry ─────────────────────────────────────────────
// The UI thread notes the sidebar's already-resolved project paths; the
// daemon worker snapshots them for filtering. No filesystem reads here.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub account: AccountKey,
    pub generation: u64,
}

#[derive(Default)]
struct Registry {
    owner: Option<AccountKey>,
    generation: u64,
    projects: HashMap<String, HashSet<String>>,
}

impl Registry {
    fn note(&mut self, contributor: String, account: AccountKey, projects: Vec<String>) {
        if self.owner.as_ref() != Some(&account) {
            self.owner = Some(account);
            self.generation = self.generation.wrapping_add(1);
            self.projects.clear();
        }
        let kept: HashSet<String> = projects
            .into_iter()
            .filter(|p| !p.is_empty() && p.chars().count() <= MAX_PROJECT_CHARS)
            .take(super::desktop_catalog::MAX_PROJECTS)
            .collect();
        self.projects.insert(contributor, kept);
    }

    fn retire(&mut self, contributor: &str) {
        self.projects.remove(contributor);
        if self.projects.is_empty() {
            self.owner = None;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    fn current(&self) -> Option<Owner> {
        self.owner.as_ref().map(|account| Owner {
            account: account.clone(),
            generation: self.generation,
        })
    }

    fn allowlist(&self, owner: &Owner) -> Option<HashSet<String>> {
        if self.owner.as_ref() != Some(&owner.account) || self.generation != owner.generation {
            return None;
        }
        let mut out = HashSet::new();
        for set in self.projects.values() {
            out.extend(set.iter().cloned());
        }
        Some(out)
    }

    fn snapshot_for(&self, expected: &AccountKey) -> Option<(Owner, HashSet<String>)> {
        let owner = self.current()?;
        if &owner.account != expected {
            return None;
        }
        let allowlist = self.allowlist(&owner)?;
        Some((owner, allowlist))
    }
}

struct Core {
    registry: Mutex<Registry>,
}

impl Core {
    fn new() -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
        }
    }

    fn current(&self) -> Option<Owner> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current()
    }

    fn is_current(&self, owner: &Owner) -> bool {
        self.current().as_ref() == Some(owner)
    }
}

static CORE: OnceLock<Core> = OnceLock::new();

fn core() -> &'static Core {
    CORE.get_or_init(Core::new)
}

/// Note this window's authoritative sidebar projects (already resolved
/// in-memory by the collector; no disk/network here).
pub(crate) fn note_projects(contributor: String, account: AccountKey, projects: Vec<String>) {
    core()
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .note(contributor, account, projects);
}

pub(crate) fn note_removed(contributor: &str) {
    core()
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retire(contributor);
}

pub(crate) fn note_signed_out(contributor: &str) {
    note_removed(contributor);
}

/// Sidebar-project allowlist for an account (E3b E2EE file serving, §2.1 F2).
/// Returns `None` when nothing is noted yet (nothing shared); `Some(set)`
/// otherwise (possibly empty, meaning explicitly nothing shared). Pure read,
/// no disk/network.
pub(crate) fn allowlist_for_test(account: &AccountKey) -> Option<HashSet<String>> {
    let registry = core().registry.lock().unwrap_or_else(|e| e.into_inner());
    let owner = registry.current()?;
    if &owner.account != account {
        return None;
    }
    registry.allowlist(&owner)
}

// ── Params ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    session_id: Uuid,
    expires_at: u64,
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    session_id: Uuid,
    expires_at: u64,
    team_id: String,
}

fn parse<T: serde::de::DeserializeOwned>(
    value: Value,
) -> Result<T, super::companion_host::Failure> {
    use super::companion_host::Failure;
    if serde_json::to_vec(&value)
        .map_err(|_| Failure::Invalid)?
        .len()
        > MAX_PARAMS
    {
        return Err(Failure::Invalid);
    }
    serde_json::from_value(value).map_err(|_| Failure::Invalid)
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn lease_ok(session_id: &Uuid, expires_at: u64, now: u64) -> bool {
    !session_id.is_nil() && expires_at > now && expires_at <= now.saturating_add(MAX_LEASE_MS)
}

// ── Pure projection ────────────────────────────────────────────────────────

/// Truncate to a char boundary; reports whether anything was cut.
pub(crate) fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    (text.chars().take(max).collect(), true)
}

pub(crate) fn team_status_str(status: dragent_host::team::TeamStatus) -> &'static str {
    use dragent_host::team::TeamStatus;
    match status {
        TeamStatus::Planning => "planning",
        TeamStatus::Running => "running",
        TeamStatus::Done => "done",
        TeamStatus::Stopped => "stopped",
    }
}

pub(crate) fn task_status_str(status: dragent_host::team::TaskStatus) -> &'static str {
    status.key()
}

pub(crate) fn message_kind_str(kind: dragent_host::team::MessageKind) -> &'static str {
    use dragent_host::team::MessageKind;
    match kind {
        MessageKind::Note => "note",
        MessageKind::Question => "question",
        MessageKind::Answer => "answer",
        MessageKind::Handoff => "handoff",
        MessageKind::Finding => "finding",
        MessageKind::NeedsUser => "needs_user",
    }
}

/// `(from, label)` for a message party. `from` is the stable kind
/// (`lead`/`user`/`task`/`dragent`); `label` is the display identity
/// (the task id for workers).
pub(crate) fn party_ids(party: &dragent_host::team::Party) -> (&'static str, String) {
    use dragent_host::team::Party;
    match party {
        Party::Lead => ("lead", "lead".to_string()),
        Party::User => ("user", "user".to_string()),
        Party::Task(id) => ("task", id.clone()),
        Party::Dragent => ("dragent", "dragent".to_string()),
    }
}

/// Preferred model label for a task: the chosen model, else the user's
/// next-attempt pick, else the model the last attempt ran on.
pub(crate) fn task_model(task: &dragent_host::team::Task) -> Option<String> {
    task.model
        .clone()
        .or_else(|| task.user_model.clone())
        .or_else(|| task.attempt_model.clone())
        .filter(|m| !m.is_empty())
}

/// Project a team summary to the phone list shape. Titles are bounded;
/// counts come straight from the store's summary.
pub(crate) fn project_list_item(team: &dragent_host::team::TeamSummary) -> (Value, bool) {
    let (title, title_cut) = truncate_chars(&team.title, MAX_TITLE_CHARS);
    let (project_path, path_cut) = truncate_chars(&team.project_root, MAX_PROJECT_CHARS);
    let truncated = title_cut || path_cut;
    (
        json!({
            "id": team.id,
            "title": title,
            "project_path": project_path,
            "status": team_status_str(team.status),
            "task_count": team.tasks,
            "done_count": team.done,
            "running_count": team.running,
            "blocked_count": team.blocked,
            "needs_user": team.needs_user > 0,
            "updated_at": team.updated_at_ms,
        }),
        truncated,
    )
}

/// Build the `coordination.list` result from already-fetched summaries.
/// `allowed` is the sidebar allowlist; `filter` is the phone's optional
/// project filter (already validated to be allowlisted). Sorts most-recent
/// first, caps at [`MAX_TEAMS`].
pub(crate) fn build_list_result(
    mut teams: Vec<dragent_host::team::TeamSummary>,
    allowed: &HashSet<String>,
    filter: Option<&str>,
) -> Value {
    teams.retain(|t| allowed.contains(&t.project_root));
    if let Some(want) = filter {
        teams.retain(|t| t.project_root == want);
    }
    teams.sort_by_key(|t| std::cmp::Reverse(t.updated_at_ms));
    let mut truncated = teams.len() > MAX_TEAMS;
    teams.truncate(MAX_TEAMS);
    let mut items = Vec::with_capacity(teams.len());
    for team in &teams {
        let (item, cut) = project_list_item(team);
        truncated = truncated || cut;
        items.push(item);
    }
    let mut result = json!({"items": items, "truncated": truncated});
    let (sized, size_cut) = enforce_size(&result, truncated);
    result = sized;
    if size_cut {
        result["truncated"] = json!(true);
    }
    result
}

/// Most models offered for one agent.
const MAX_MODEL_CHOICES: usize = 200;

/// The models each unfinished task's agent may move to, by agent key.
fn model_choices(team: &dragent_host::team::Team) -> Value {
    let mut choices = serde_json::Map::new();
    for task in team.tasks.iter().take(MAX_TASKS).filter(|task| !task.status.is_finished()) {
        let Some(key) = task.agent.as_deref() else { continue };
        if choices.contains_key(key) {
            continue;
        }
        let Some(agent) = crate::automations::executor::prompt_agent(key) else { continue };
        let models: Vec<Value> = crate::teams::models::cli_choices(agent)
            .into_iter()
            .take(MAX_MODEL_CHOICES)
            .map(|(id, label)| json!({"id": id, "label": label}))
            .collect();
        choices.insert(key.to_string(), Value::Array(models));
    }
    Value::Object(choices)
}

/// A worktree as the person reads it: its task and folder name, never the
/// full path.
fn worktree_names(items: &[(String, std::path::PathBuf)]) -> Vec<String> {
    items
        .iter()
        .take(MAX_TASKS)
        .map(|(task, path)| {
            let folder = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            truncate_chars(&format!("{task} ({folder})"), MAX_TITLE_CHARS).0
        })
        .collect()
}

fn cost_str(cost: dragent_host::team::Cost) -> &'static str {
    match cost {
        dragent_host::team::Cost::Cheapest => "cheapest",
        dragent_host::team::Cost::Balanced => "balanced",
        dragent_host::team::Cost::Best => "best",
    }
}

/// The newest message for the person, when nothing they sent followed it
/// (the desktop's question card).
pub(crate) fn open_question(team: &dragent_host::team::Team) -> Option<&dragent_host::team::Message> {
    use dragent_host::team::Party;
    let latest = team.messages.iter().rev().find(|message| message.to == Party::User)?;
    let answered = team
        .messages
        .iter()
        .any(|later| later.id > latest.id && later.from == Party::User);
    (!answered).then_some(latest)
}

/// Point the detail's lead and workers at the phone's tabs for their
/// terminals, where those are open.
fn attach_tabs(result: &mut Value, team: &dragent_host::team::Team, expected: &AccountKey) {
    let lead = team.lead.as_ref().and_then(|lead| lead.pane.clone());
    let workers: Vec<(String, String)> = team
        .tasks
        .iter()
        .take(MAX_TASKS)
        .filter_map(|task| Some((task.id.clone(), task.worker.as_ref()?.pane.clone()?)))
        .collect();
    let panes: HashSet<String> = lead.iter().cloned().chain(workers.iter().map(|(_, pane)| pane.clone())).collect();
    let tabs = super::companion_host::tabs_for_panes(expected, &panes);
    if tabs.is_empty() {
        return;
    }
    if let Some(tab) = lead.and_then(|pane| tabs.get(&pane)) {
        result["team"]["lead_tab_id"] = json!(tab);
    }
    if let Some(list) = result["team"]["tasks"].as_array_mut() {
        for entry in list {
            let id = entry["id"].as_str().unwrap_or_default().to_string();
            if let Some(tab) = workers.iter().find(|(task, _)| *task == id).and_then(|(_, pane)| tabs.get(pane)) {
                entry["worker_tab_id"] = json!(tab);
            }
        }
    }
}

/// Build the `coordination.get` result from a full team. Caps tasks at
/// [`MAX_TASKS`] and messages at the most-recent [`MAX_MESSAGES`]; bounds
/// all text; never emits prompts, workspaces, panes, or credentials.
pub(crate) fn build_detail_result(team: &dragent_host::team::Team) -> Value {
    let (title, title_cut) = truncate_chars(&team.title, MAX_TITLE_CHARS);
    let (project_path, path_cut) = truncate_chars(&team.project_root, MAX_PROJECT_CHARS);
    // The store keeps the goal as the title; expose both under the phone
    // schema without inventing a second source.
    let (goal, goal_cut) = truncate_chars(&team.title, MAX_TITLE_CHARS);
    let mut truncated = title_cut || path_cut || goal_cut || team.tasks.len() > MAX_TASKS;

    let mut tasks = Vec::new();
    for task in team.tasks.iter().take(MAX_TASKS) {
        let (task_title, tcut) = truncate_chars(&task.title, MAX_TITLE_CHARS);
        truncated = truncated || tcut;
        let mut depends: Vec<String> = task.depends_on.iter().cloned().collect();
        if depends.len() > MAX_DEPENDS {
            depends.truncate(MAX_DEPENDS);
            truncated = true;
        }
        let (summary, scut) = match task.result.as_ref() {
            Some(result) => {
                let (s, cut) = truncate_chars(&result.summary, MAX_TITLE_CHARS);
                (Some(s), cut)
            }
            None => (None, false),
        };
        truncated = truncated || scut;
        // Task ids are store-generated (`t1`, …); bound defensively.
        let (task_id, idcut) = truncate_chars(&task.id, MAX_TEAM_ID_CHARS);
        truncated = truncated || idcut;
        tasks.push(json!({
            "id": task_id,
            "title": task_title,
            "status": task_status_str(task.status),
            "agent": task.agent.clone(),
            "model": task_model(task),
            "depends_on": depends,
            "result_summary": summary,
            "reason": task.reason.as_deref().map(|reason| truncate_chars(reason, MAX_TITLE_CHARS).0),
            // Filled in by `execute_get` when the worker's terminal is open.
            "worker_tab_id": null,
            // As the desktop's model button: the person's choice when they
            // made one, else the model the task runs on.
            "model_locked": task.model_locked,
            "chosen_model": if task.model_locked { task.user_model.clone() } else { task.model.clone() },
            "can_set_model": !task.status.is_finished()
                && task.agent.as_deref().and_then(crate::automations::executor::prompt_agent).is_some(),
        }));
    }

    let total_messages = team.messages.len();
    truncated = truncated || total_messages > MAX_MESSAGES;
    // Most-recent slice: the phone catches up from the tail.
    let start = total_messages.saturating_sub(MAX_MESSAGES);
    let mut messages = Vec::new();
    for message in team.messages.iter().skip(start) {
        let (from, label) = party_ids(&message.from);
        let (text, tcut) = truncate_chars(&message.body, MAX_MESSAGE_CHARS);
        truncated = truncated || tcut;
        // `created_at` must stay exactly representable for the phone.
        let created_at = (message.at_ms.max(0) as u64).min(MAX_SAFE_INTEGER);
        messages.push(json!({
            "id": message.id.to_string(),
            "from": from,
            "label": label,
            "kind": message_kind_str(message.kind),
            "text": text,
            "created_at": created_at,
        }));
    }

    let mut result = json!({
        "team": {
            "id": team.id,
            "title": title,
            "project_path": project_path,
            "status": team_status_str(team.status),
            "goal": goal,
            "tasks": tasks,
            "messages": messages,
            "max_parallel": team.max_parallel_per_agent,
            "auto_start": team.auto_start,
            "lead_answers_prompts": team.lead_answers_prompts,
            "cost": cost_str(team.cost),
            "lead_agent": team.lead.as_ref().and_then(|lead| lead.agent.clone()),
            "roster": team.roster.iter().take(MAX_TASKS).map(|agent| agent.agent.clone()).collect::<Vec<_>>(),
            "outcome": team.outcome.as_deref().map(|outcome| truncate_chars(outcome, MAX_MESSAGE_CHARS).0),
            // The lead's or a worker's message for the person that nothing
            // they sent has followed yet: what "needs you" is about.
            "question": open_question(team).map(|message| {
                let (_, label) = party_ids(&message.from);
                json!({
                    "id": message.id.to_string(),
                    "kind": message_kind_str(message.kind),
                    "label": label,
                    "text": truncate_chars(&message.body, MAX_MESSAGE_CHARS).0,
                })
            }),
            "lead_tab_id": null,
            "model_choices": model_choices(team),
            // Filled in by `execute_get` (they look at the lead's session
            // and the worktrees).
            "needs_new_lead": false,
            "cleanup": null,
        },
        "truncated": truncated,
    });
    let (sized, size_cut) = enforce_size_detail(&mut result, truncated);
    result = sized;
    if size_cut {
        result["truncated"] = json!(true);
    }
    result
}

/// Shrink a list/detail result until it fits [`MAX_RESPONSE`]. Drops
/// messages first, then tasks, then marks truncated. Returns the (possibly
/// shrunk) value and whether shrinking cut anything.
fn enforce_size(result: &Value, already: bool) -> (Value, bool) {
    let mut result = result.clone();
    let mut truncated = already;
    let fits = |v: &Value| {
        serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX) <= MAX_RESPONSE - 1024
    };
    // Fast path: already fits.
    if fits(&result) {
        return (result, false);
    }
    // Lists only carry items; drop from the tail (oldest) until it fits.
    loop {
        let len = result
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if len == 0 || fits(&result) {
            break;
        }
        if let Some(items) = result.get_mut("items").and_then(|v| v.as_array_mut()) {
            items.pop();
            truncated = true;
        } else {
            break;
        }
    }
    (result, truncated)
}

fn enforce_size_detail(result: &mut Value, already: bool) -> (Value, bool) {
    let mut truncated = already;
    let fits = |v: &Value| {
        serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX) <= MAX_RESPONSE - 1024
    };
    if fits(result) {
        return (result.clone(), false);
    }
    // Drop oldest messages first (keep the tail already selected; pop from
    // the front by draining), then tasks from the tail.
    loop {
        let len = result
            .get("team")
            .and_then(|t| t.get("messages"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if len == 0 {
            break;
        }
        if fits(result) {
            break;
        }
        if let Some(messages) = result
            .get_mut("team")
            .and_then(|t| t.get_mut("messages"))
            .and_then(|v| v.as_array_mut())
        {
            messages.remove(0);
            truncated = true;
        } else {
            break;
        }
    }
    loop {
        let len = result
            .get("team")
            .and_then(|t| t.get("tasks"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if len == 0 {
            break;
        }
        if fits(result) {
            break;
        }
        if let Some(tasks) = result
            .get_mut("team")
            .and_then(|t| t.get_mut("tasks"))
            .and_then(|v| v.as_array_mut())
        {
            tasks.pop();
            truncated = true;
        } else {
            break;
        }
    }
    (result.clone(), truncated)
}

// ── Daemon I/O (bounded worker threads, never the UI thread) ──────────────
// Never `List(None)`: one filtered `List` per allowlisted project.

/// The project folders this owner's sidebar shows (what a phone may act
/// in), when `expected` is still the signed-in account.
pub(crate) fn project_roots(expected: &AccountKey) -> Option<HashSet<String>> {
    snapshot(expected).map(|(_, roots)| roots)
}

fn snapshot(expected: &AccountKey) -> Option<(Owner, HashSet<String>)> {
    if dragent_paths::organisation_id() != expected.org {
        return None;
    }
    let registry = core().registry.lock().unwrap_or_else(|e| e.into_inner());
    registry.snapshot_for(expected)
}

fn valid_team_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_TEAM_ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_project_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= MAX_PROJECT_CHARS && !id.contains('\0')
}

/// `coordination.list` implementation. Verifies the owner before and after
/// daemon I/O; any account/org change invalidates the result.
pub(crate) fn execute_list(
    expected: &AccountKey,
    params: Value,
) -> Result<Value, super::companion_host::Failure> {
    use super::companion_host::Failure;
    use dragent_host::team::{TeamRequest, TeamResponse};

    let args: ListParams = parse(params)?;
    let now = epoch_ms();
    if !lease_ok(&args.session_id, args.expires_at, now) {
        return Err(Failure::Invalid);
    }
    if let Some(project) = args.project_id.as_deref() {
        if !valid_project_id(project) {
            return Err(Failure::Invalid);
        }
    }

    let (owner, allowlist) = snapshot(expected).ok_or(Failure::Gone)?;
    if !core().is_current(&owner) {
        return Err(Failure::Gone);
    }
    if let Some(project) = args.project_id.as_deref() {
        if !allowlist.contains(project) {
            // Sanitized: same code whether the project is unknown or simply
            // not in this window's sidebar.
            return Err(Failure::Gone);
        }
    }
    if allowlist.is_empty() {
        if !core().is_current(&owner)
            || dragent_paths::organisation_id() != expected.org
            || epoch_ms() >= args.expires_at
        {
            return Err(Failure::Gone);
        }
        return Ok(json!({"items": [], "truncated": false}));
    }

    let wanted: Vec<String> = match args.project_id.clone() {
        Some(single) => vec![single],
        None => {
            let mut all: Vec<String> = allowlist.iter().cloned().collect();
            all.sort();
            all
        }
    };

    // Daemon I/O: one filtered List per project. A connect failure is
    // `unavailable` (never a faked empty list).
    let deadline = Instant::now() + Duration::from_secs(8);
    let client = dragent_host::HostClient::connect_existing().map_err(|_| Failure::Unavailable)?;
    let mut merged: Vec<dragent_host::team::TeamSummary> = Vec::new();
    for project in wanted {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(Failure::Unavailable)?;
        match client.team_with_timeout(
            TeamRequest::List {
                project_root: Some(project),
            },
            remaining,
        ) {
            Ok(TeamResponse::Teams(mut teams)) => merged.append(&mut teams),
            Ok(_) => return Err(Failure::Unavailable),
            Err(_) => return Err(Failure::Unavailable),
        }
    }

    // Post-I/O: owner, lease and allowlist still hold, or the result is stale.
    if !core().is_current(&owner)
        || dragent_paths::organisation_id() != expected.org
        || epoch_ms() >= args.expires_at
    {
        return Err(Failure::Gone);
    }
    let fresh = snapshot(expected).ok_or(Failure::Gone)?;
    if fresh.0 != owner {
        return Err(Failure::Gone);
    }
    Ok(build_list_result(
        merged,
        &fresh.1,
        args.project_id.as_deref(),
    ))
}

/// `coordination.get` implementation. Same owner-before/after discipline;
/// the team's project must still be allowlisted.
pub(crate) fn execute_get(
    expected: &AccountKey,
    params: Value,
) -> Result<Value, super::companion_host::Failure> {
    use super::companion_host::Failure;
    use dragent_host::team::{TeamRequest, TeamResponse};

    let args: GetParams = parse(params)?;
    let now = epoch_ms();
    if !lease_ok(&args.session_id, args.expires_at, now) || !valid_team_id(&args.team_id) {
        return Err(Failure::Invalid);
    }

    let (owner, allowlist) = snapshot(expected).ok_or(Failure::Gone)?;
    if !core().is_current(&owner) {
        return Err(Failure::Gone);
    }

    let deadline = Instant::now() + Duration::from_secs(8);
    let client = dragent_host::HostClient::connect_existing().map_err(|_| Failure::Unavailable)?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(Failure::Unavailable)?;
    let team = match client.team_with_timeout(
        TeamRequest::Get {
            team: args.team_id.clone(),
        },
        remaining,
    ) {
        Ok(TeamResponse::Team(team)) => *team,
        Ok(_) => return Err(Failure::Unavailable),
        // Sanitized: unknown ids and daemon errors that name the id both
        // surface as `gone`/`unavailable` without the store's message.
        Err(_) => return Err(Failure::Gone),
    };

    if !core().is_current(&owner)
        || dragent_paths::organisation_id() != expected.org
        || epoch_ms() >= args.expires_at
    {
        return Err(Failure::Gone);
    }
    let fresh = snapshot(expected).ok_or(Failure::Gone)?;
    if fresh.0 != owner {
        return Err(Failure::Gone);
    }
    if !fresh.1.contains(&team.project_root) {
        return Err(Failure::Gone);
    }
    let mut result = build_detail_result(&team);
    attach_tabs(&mut result, &team, expected);
    // The page's "Start a new lead" and "Clean up", on the same rules.
    result["team"]["needs_new_lead"] = json!(crate::teams::lead_gone(&client, &team).0);
    if let Some(plan) = crate::teams::cleanup(&team) {
        result["team"]["cleanup"] = json!({
            "remove": worktree_names(&plan.remove),
            "keep": worktree_names(&plan.keep),
            "dirty": worktree_names(&plan.dirty),
        });
    }
    Ok(result)
}

/// Inside the end-to-end encrypted channel the channel itself is the lease
/// (its phone and owner are checked before any request runs), so the phone
/// no longer sends the relay-era `session_id`/`expires_at`. Fill them in
/// for a short window when absent; the reads keep their checks.
pub(crate) fn with_channel_lease(mut params: Value) -> Value {
    if let Some(object) = params.as_object_mut() {
        if !object.contains_key("session_id") {
            object.insert("session_id".into(), json!(Uuid::new_v4()));
        }
        if !object.contains_key("expires_at") {
            object.insert("expires_at".into(), json!(epoch_ms() + 60_000));
        }
    }
    params
}

/// Changes the phone may make to a team.
pub(crate) const WRITE_METHODS: &[&str] = &[
    "coordination.send",
    "coordination.stop",
    "coordination.configure",
    "coordination.cancel_task",
    "coordination.forget",
    "coordination.task_model",
    "coordination.new_lead",
    "coordination.clean_up",
];
/// Largest message the phone may send to a lead.
const MAX_SEND_CHARS: usize = 8000;
/// Most workers of one agent the phone may allow at once.
const MAX_PARALLEL: u32 = 8;

type Job = Box<dyn FnOnce() -> Result<Value, super::companion_host::Failure> + Send>;

/// Check a team change on the socket thread; the returned work checks the
/// team is in a project this desktop shows (as reads do), makes the change
/// and answers with the team as it is now, off the socket thread.
pub(crate) fn write_job(expected: &AccountKey, method: &str, params: &Value) -> Result<Job, super::companion_host::Failure> {
    use super::companion_host::Failure;
    use dragent_host::team::{MessageKind, Party, TeamRequest};

    let object = params.as_object().ok_or(Failure::Invalid)?;
    let allowed: &[&str] = match method {
        "coordination.send" => &["team_id", "body"],
        "coordination.stop" => &["team_id"],
        "coordination.configure" => &["team_id", "max_parallel", "auto_start", "lead_answers_prompts", "cost"],
        "coordination.cancel_task" => &["team_id", "task_id"],
        "coordination.forget" => &["team_id"],
        "coordination.task_model" => &["team_id", "task_id", "model"],
        "coordination.new_lead" | "coordination.clean_up" => &["team_id"],
        _ => return Err(Failure::Invalid),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    let team = object.get("team_id").and_then(Value::as_str).filter(|id| valid_team_id(id)).ok_or(Failure::Invalid)?.to_string();
    // The changes the desktop's page makes through a window or its own
    // tool rather than one store request.
    if matches!(method, "coordination.task_model" | "coordination.new_lead" | "coordination.clean_up") {
        let task_model = if method == "coordination.task_model" {
            let task = object
                .get("task_id")
                .and_then(Value::as_str)
                .filter(|id| valid_team_id(id))
                .ok_or(Failure::Invalid)?
                .to_string();
            // `null`: the agent's own choice.
            let model = match object.get("model") {
                None => return Err(Failure::Invalid),
                Some(Value::Null) => None,
                Some(Value::String(model)) if !model.is_empty() && model.len() <= 200 && model != "none" => Some(model.clone()),
                Some(_) => return Err(Failure::Invalid),
            };
            Some((task, model))
        } else {
            None
        };
        let expected = expected.clone();
        let method = method.to_string();
        return Ok(Box::new(move || {
            let before = execute_get(&expected, with_channel_lease(json!({"team_id": team})))?;
            if before["team"]["id"] != json!(team) {
                return Err(Failure::Gone);
            }
            let client = dragent_host::HostClient::connect_existing().map_err(|_| Failure::Unavailable)?;
            let full = match client.team_with_timeout(TeamRequest::Get { team: team.clone() }, Duration::from_secs(8)) {
                Ok(dragent_host::team::TeamResponse::Team(full)) => *full,
                _ => return Err(Failure::Unavailable),
            };
            match (method.as_str(), task_model) {
                ("coordination.task_model", Some((task, model))) => {
                    let entry = full.task(&task).ok_or(Failure::Gone)?;
                    let agent = entry
                        .agent
                        .as_deref()
                        .and_then(crate::automations::executor::prompt_agent)
                        .filter(|_| !entry.status.is_finished())
                        .ok_or(Failure::Invalid)?;
                    // Only a model the desktop's picker offers.
                    if let Some(model) = &model {
                        if !crate::teams::models::cli_choices(agent).iter().any(|(id, _)| id == model) {
                            return Err(Failure::Invalid);
                        }
                    }
                    crate::team_source::call(
                        "task_model",
                        &json!({"team": team, "task": task, "model": model.as_deref().unwrap_or("none")}),
                        &crate::cli::team::Caller::person(),
                    )
                    .map_err(|_| Failure::Unavailable)?;
                }
                ("coordination.new_lead", _) => {
                    let (gone, cwd) = crate::teams::lead_gone(&client, &full);
                    if !gone {
                        return Err(Failure::Invalid);
                    }
                    let plan = crate::teams::new_lead_plan(&full, cwd);
                    let answer = super::companion_starts::start_in_a_window(
                        &expected,
                        super::companion_starts::StartKind::NewLead {
                            project: full.project_root.clone(),
                            team: full.id.clone(),
                            agent: plan.agent,
                            model: plan.model,
                            cwd: plan.cwd,
                            prompt: plan.prompt,
                        },
                    )?;
                    if answer["started"] != json!(true) {
                        return Ok(answer);
                    }
                }
                ("coordination.clean_up", _) => {
                    let plan = crate::teams::cleanup(&full).ok_or(Failure::Invalid)?;
                    let answer = super::companion_starts::start_in_a_window(
                        &expected,
                        super::companion_starts::StartKind::CleanUp {
                            project: full.project_root.clone(),
                            remove: plan.remove.into_iter().map(|(_, path)| path).collect(),
                        },
                    )?;
                    if answer["started"] != json!(true) {
                        return Ok(answer);
                    }
                }
                _ => return Err(Failure::Invalid),
            }
            execute_get(&expected, with_channel_lease(json!({"team_id": team})))
        }));
    }
    let request = match method {
        "coordination.send" => {
            let body = object
                .get("body")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|body| !body.is_empty() && body.chars().count() <= MAX_SEND_CHARS && !body.contains('\0'))
                .ok_or(Failure::Invalid)?;
            TeamRequest::Send {
                team: team.clone(),
                from: Party::User,
                to: Party::Lead,
                kind: MessageKind::Note,
                body: body.to_string(),
                reply_to: None,
                choices: Vec::new(),
            }
        }
        "coordination.stop" => TeamRequest::Stop { team: team.clone() },
        "coordination.forget" => TeamRequest::Forget { team: team.clone() },
        "coordination.cancel_task" => TeamRequest::CancelTask {
            team: team.clone(),
            task: object
                .get("task_id")
                .and_then(Value::as_str)
                .filter(|id| valid_team_id(id))
                .ok_or(Failure::Invalid)?
                .to_string(),
        },
        _ => {
            let max_parallel = match object.get("max_parallel") {
                None => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .filter(|n| (1..=u64::from(MAX_PARALLEL)).contains(n))
                        .ok_or(Failure::Invalid)? as u32,
                ),
            };
            let auto_start = match object.get("auto_start") {
                None => None,
                Some(value) => Some(value.as_bool().ok_or(Failure::Invalid)?),
            };
            let lead_answers_prompts = match object.get("lead_answers_prompts") {
                None => None,
                Some(value) => Some(value.as_bool().ok_or(Failure::Invalid)?),
            };
            let cost = match object.get("cost") {
                None => None,
                Some(value) => Some(match value.as_str() {
                    Some("cheapest") => dragent_host::team::Cost::Cheapest,
                    Some("balanced") => dragent_host::team::Cost::Balanced,
                    Some("best") => dragent_host::team::Cost::Best,
                    _ => return Err(Failure::Invalid),
                }),
            };
            if max_parallel.is_none() && auto_start.is_none() && lead_answers_prompts.is_none() && cost.is_none() {
                return Err(Failure::Invalid);
            }
            TeamRequest::Configure {
                team: team.clone(),
                cost,
                max_parallel_per_agent: max_parallel,
                auto_start,
                lead_answers_prompts,
            }
        }
    };
    let expected = expected.clone();
    let forget = method == "coordination.forget";
    Ok(Box::new(move || {
        // The same checks a read makes: owner, organisation, and a team in
        // a project this desktop's sidebar shows.
        let before = execute_get(&expected, with_channel_lease(json!({"team_id": team})))?;
        if before["team"]["id"] != json!(team) {
            return Err(Failure::Gone);
        }
        let client = dragent_host::HostClient::connect_existing().map_err(|_| Failure::Unavailable)?;
        let request = match request {
            // As the desktop's Send: a message after the lead's open
            // question answers it.
            TeamRequest::Send { team: id, from, to, body, .. } => {
                let reply_to = match client.team_with_timeout(TeamRequest::Get { team: id.clone() }, Duration::from_secs(8)) {
                    Ok(dragent_host::team::TeamResponse::Team(full)) => open_question(&full)
                        .filter(|message| message.from == Party::Lead)
                        .map(|message| message.id),
                    _ => None,
                };
                TeamRequest::Send {
                    team: id,
                    from,
                    to,
                    kind: if reply_to.is_some() { MessageKind::Answer } else { MessageKind::Note },
                    body,
                    reply_to,
                    choices: Vec::new(),
                }
            }
            other => other,
        };
        client
            .team_with_timeout(request, Duration::from_secs(8))
            .map_err(|_| Failure::Unavailable)?;
        if forget {
            return Ok(json!({"forgotten": true}));
        }
        execute_get(&expected, with_channel_lease(json!({"team_id": team})))
    }))
}

#[cfg(test)]
#[path = "companion_coordination_tests.rs"]
mod tests;
