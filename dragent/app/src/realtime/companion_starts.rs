//! Work a paired phone starts on this desktop (Dragent companion): an agent
//! on one of the organisation's tasks, or a new coordination (a lead agent
//! and its workers on a goal).
//!
//! Both go through the same new-workspace prompt the desktop uses, on the
//! UI thread of a window that shows the project, so a start from the phone
//! names, branches and links its workspace exactly as one from the desktop.
//!
//! Methods:
//! - `tasks.start {task, agent, project?}` -> `{started, reason?}`
//! - `coordination.create {project, goal, lead, workers?, cost?}`
//!   -> `{started, reason?}`
//! - `coordination.options {}` -> `{projects:[{key?, label, path}],
//!   agents:[{key, label, models:[{id, label}]}]}` (the agents installed on
//!   this desktop, Dragent's own first)
//! - `sessions.start {project, agent, prompt, model?, workspace?, branch?}`
//!   -> `{started, reason?}`: an agent on `prompt`, in a new workspace of
//!   the project (from `branch`, else the project's default), or in the
//!   existing workspace folder `workspace`
//!
//! `project` is a project folder as the phone's catalog names it (its
//! `path`), and only one of this account's sidebar projects is accepted.
//! `started: false` comes with a plain reason the phone shows as is.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use super::companion_host::Failure;
use super::desktop_catalog::AccountKey;
use crate::workspace::PromptAgent;

pub(crate) const METHODS: &[&str] = &["tasks.start", "coordination.create", "coordination.options", "sessions.start"];

/// How long a start may wait for a window's UI thread (under the job's own
/// deadline, so the answer still reaches the phone).
const WINDOW_WAIT: Duration = Duration::from_secs(8);
const MAX_GOAL_CHARS: usize = 8000;
const MAX_WORKERS: usize = 8;

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

/// What a phone asked a window to start.
#[derive(Clone, Debug)]
pub(crate) enum StartKind {
    Task {
        task: Box<dragent_tasks::Task>,
        agent: PromptAgent,
        /// A sidebar project folder; else the task's own project.
        project: Option<String>,
    },
    Coordination {
        project: String,
        goal: String,
        lead: PromptAgent,
        workers: Vec<PromptAgent>,
        cost: dragent_host::team::Cost,
    },
    /// Work on a connected tracker's item (the Tasks page's Start on a
    /// tracker row), in `root` when the phone named a project.
    Item {
        item: Box<crate::integrations::tracker::TrackerItem>,
        root: Option<std::path::PathBuf>,
        agent: PromptAgent,
    },
    /// A new lead for a team whose lead stopped (the Coordination page's
    /// "Start a new lead"), in the window showing its project.
    NewLead {
        project: String,
        team: String,
        agent: String,
        model: Option<String>,
        cwd: std::path::PathBuf,
        prompt: String,
    },
    /// A new agent session from the phone: `agent` on `prompt` in a new
    /// workspace of `project` (branched from `branch` when given), or in the
    /// existing workspace folder `workspace`.
    Session {
        project: String,
        agent: PromptAgent,
        model: Option<String>,
        workspace: Option<std::path::PathBuf>,
        branch: Option<String>,
        prompt: String,
    },
    /// Continue an earlier conversation (Past sessions) in a new tab of the
    /// window showing `project`.
    Resume {
        project: String,
        session: Box<dragent_host::agent_history::PastSession>,
    },
    /// Close and remove a finished team's worktrees (its "Clean up").
    CleanUp {
        project: String,
        remove: Vec<std::path::PathBuf>,
    },
}

/// Why a window didn't start it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Not this window's account or project: ask the next window.
    NotHere,
    /// This window has the project but couldn't start: say why.
    Reason(String),
}

/// A start for a window's UI thread, which answers once on `reply`.
pub(crate) struct StartRequest {
    pub account: AccountKey,
    pub kind: StartKind,
    reply: std::sync::mpsc::SyncSender<Result<(), Refusal>>,
}
impl StartRequest {
    pub(crate) fn answer(self, result: Result<(), Refusal>) {
        let _ = self.reply.try_send(result);
    }
}

fn windows() -> &'static Mutex<HashMap<String, async_channel::Sender<StartRequest>>> {
    static WINDOWS: OnceLock<Mutex<HashMap<String, async_channel::Sender<StartRequest>>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A window starts taking phones' starts (one stream per window).
pub(crate) fn start_requests(contributor: String) -> async_channel::Receiver<StartRequest> {
    let (requests, receiver) = async_channel::bounded(4);
    let mut windows = windows().lock().unwrap_or_else(|e| e.into_inner());
    windows.retain(|_, sender| !sender.is_closed());
    windows.insert(contributor, requests);
    receiver
}

/// Offer the start to each window in turn until one has the project.
pub(crate) fn start_in_a_window(account: &AccountKey, kind: StartKind) -> Result<Value, Failure> {
    let senders: Vec<Arc<async_channel::Sender<StartRequest>>> = windows()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter(|sender| !sender.is_closed())
        .map(|sender| Arc::new(sender.clone()))
        .collect();
    for sender in senders {
        let (reply, answer) = std::sync::mpsc::sync_channel(1);
        if sender
            .try_send(StartRequest { account: account.clone(), kind: kind.clone(), reply })
            .is_err()
        {
            continue;
        }
        match answer.recv_timeout(WINDOW_WAIT) {
            Ok(Ok(())) => return Ok(json!({"started": true})),
            Ok(Err(Refusal::Reason(reason))) => return Ok(json!({"started": false, "reason": reason})),
            Ok(Err(Refusal::NotHere)) => continue,
            Err(_) => return Err(Failure::Unavailable),
        }
    }
    Ok(json!({
        "started": false,
        "reason": match kind {
            StartKind::Task { .. } | StartKind::Item { .. } | StartKind::Session { .. } => "That project isn't open on the desktop. Open it there (or choose another project), then start again.",
            _ => "That project isn't open on the desktop any more.",
        },
    }))
}

/// An agent by the key the phone was offered (`claude`, `codex`, `dragent`).
pub(crate) fn agent(key: &str) -> Option<PromptAgent> {
    PromptAgent::ALL.into_iter().find(|agent| agent.command_key() == key)
}

/// The agents a start may use: Dragent's own, then the agent CLIs the last
/// check found installed.
fn agents() -> Vec<PromptAgent> {
    let mut agents = vec![PromptAgent::Dragent];
    agents.extend(crate::workspace::installed_agents());
    agents
}

/// Projects and agents the phone's start forms offer.
pub(crate) fn start_options(expected: &AccountKey) -> Result<Value, Failure> {
    let mut roots: Vec<String> = super::companion_coordination::project_roots(expected)
        .ok_or(Failure::Gone)?
        .into_iter()
        .collect();
    roots.sort();
    let projects: Vec<Value> = roots
        .iter()
        .take(200)
        .map(|root| {
            let path = std::path::Path::new(root);
            let key = crate::workspace::origin_url(path)
                .and_then(|url| dragent_tasks::project_key(&url));
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| root.clone());
            json!({"path": root, "label": label, "key": key})
        })
        .collect();
    let agents: Vec<Value> = agents()
        .into_iter()
        .map(|agent| {
            // Dragent's own uses the desktop's chosen model.
            let models: Vec<Value> = if agent == PromptAgent::Dragent {
                Vec::new()
            } else {
                crate::teams::models::cli_choices(agent)
                    .into_iter()
                    .map(|(id, label)| json!({"id": id, "label": label}))
                    .collect()
            };
            json!({"key": agent.command_key(), "label": agent.label(), "models": models})
        })
        .collect();
    Ok(json!({"projects": projects, "agents": agents}))
}

fn text(object: &serde_json::Map<String, Value>, key: &str, max: usize) -> Result<Option<String>, Failure> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= max && !value.contains('\0') => Ok(Some(value.clone())),
        _ => Err(Failure::Invalid),
    }
}

fn project_path(object: &serde_json::Map<String, Value>) -> Result<Option<String>, Failure> {
    let project = text(object, "project", 4096)?;
    match project {
        Some(path) if path.is_empty() || path.contains("/../") || path.ends_with("/..") => Err(Failure::Invalid),
        other => Ok(other),
    }
}

/// The project must be one this account's sidebar shows.
fn check_project(expected: &AccountKey, project: Option<&str>) -> Result<(), Failure> {
    let roots = super::companion_coordination::project_roots(expected).ok_or(Failure::Gone)?;
    match project {
        Some(project) if !roots.contains(project) => Err(Failure::Gone),
        _ => Ok(()),
    }
}

/// `path` is the project's own folder or one of its git worktrees (asked of
/// git, off the UI thread).
pub(crate) fn is_workspace_of(project: &std::path::Path, path: &std::path::Path) -> bool {
    use crate::process_ext::NoWindow as _;
    let same = |a: &std::path::Path, b: &std::path::Path| a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y);
    if same(project, path) {
        return true;
    }
    let Some(output) = crate::git_cmd::GitCommand::new()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .ok()
        .filter(|output| output.status.success())
    else {
        return false;
    };
    crate::workspace::project_naming::parse_worktree_list(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .any(|(worktree, _)| same(&worktree, path))
}

/// Check a request on the socket thread and return the work to run off it.
pub(crate) fn job(expected: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    let allowed: &[&str] = match method {
        "tasks.start" => &["task", "agent", "project"],
        "coordination.create" => &["project", "goal", "lead", "workers", "cost"],
        "coordination.options" => &[],
        "sessions.start" => &["project", "agent", "prompt", "model", "workspace", "branch"],
        _ => return Err(Failure::Invalid),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    let expected = expected.clone();
    match method {
        "tasks.start" => {
            let task = text(object, "task", 64)?
                .filter(|task| !task.is_empty() && task.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
                .ok_or(Failure::Invalid)?;
            let agent = text(object, "agent", 64)?.as_deref().and_then(agent).ok_or(Failure::Invalid)?;
            let project = project_path(object)?;
            Ok(Box::new(move || {
                check_project(&expected, project.as_deref())?;
                if expected.org.is_empty() || dragent_paths::organisation_id() != expected.org {
                    return Err(Failure::Gone);
                }
                let org = crate::tasks::client::active_org().map_err(|_| Failure::Unavailable)?;
                if org != expected.org {
                    return Err(Failure::Gone);
                }
                let detail = crate::tasks::client::detail(&org, &task).map_err(|_| Failure::Unavailable)?;
                start_in_a_window(&expected, StartKind::Task { task: Box::new(detail.task), agent, project })
            }))
        }
        "coordination.create" => {
            let project = project_path(object)?.ok_or(Failure::Invalid)?;
            let goal = text(object, "goal", MAX_GOAL_CHARS)?
                .map(|goal| goal.trim().to_string())
                .filter(|goal| !goal.is_empty())
                .ok_or(Failure::Invalid)?;
            let lead = text(object, "lead", 64)?.as_deref().and_then(agent).ok_or(Failure::Invalid)?;
            let mut workers = Vec::new();
            if let Some(list) = object.get("workers") {
                let list = list.as_array().filter(|list| list.len() <= MAX_WORKERS).ok_or(Failure::Invalid)?;
                for key in list {
                    let worker = key.as_str().and_then(agent).ok_or(Failure::Invalid)?;
                    if !workers.contains(&worker) {
                        workers.push(worker);
                    }
                }
            }
            let cost = match text(object, "cost", 16)?.as_deref() {
                None => dragent_host::team::Cost::default(),
                Some("cheapest") => dragent_host::team::Cost::Cheapest,
                Some("balanced") => dragent_host::team::Cost::Balanced,
                Some("best") => dragent_host::team::Cost::Best,
                Some(_) => return Err(Failure::Invalid),
            };
            Ok(Box::new(move || {
                check_project(&expected, Some(&project))?;
                start_in_a_window(&expected, StartKind::Coordination { project, goal, lead, workers, cost })
            }))
        }
        "sessions.start" => {
            let project = project_path(object)?.ok_or(Failure::Invalid)?;
            let agent = text(object, "agent", 64)?.as_deref().and_then(agent).ok_or(Failure::Invalid)?;
            let prompt = text(object, "prompt", MAX_GOAL_CHARS)?
                .map(|prompt| prompt.trim().to_string())
                .filter(|prompt| !prompt.is_empty())
                .ok_or(Failure::Invalid)?;
            // Only a model the agent offers; Dragent's own takes none.
            let model = text(object, "model", 128)?.filter(|model| !model.is_empty());
            if let Some(model) = &model {
                let known = agent != PromptAgent::Dragent
                    && crate::teams::models::cli_choices(agent).iter().any(|(id, _)| id == model);
                if !known {
                    return Err(Failure::Invalid);
                }
            }
            let workspace = match text(object, "workspace", 4096)? {
                Some(path) if path.is_empty() || path.contains("/../") || path.ends_with("/..") || !path.starts_with('/') && !path.contains(":\\") => {
                    return Err(Failure::Invalid)
                }
                other => other.map(std::path::PathBuf::from),
            };
            let branch = text(object, "branch", 255)?.filter(|branch| !branch.is_empty());
            if branch.as_deref().is_some_and(|b| b.starts_with('-') || b.contains("..") || b.chars().any(|c| c.is_whitespace() || c.is_control())) {
                return Err(Failure::Invalid);
            }
            if workspace.is_some() && branch.is_some() {
                // A branch only says where a new workspace starts.
                return Err(Failure::Invalid);
            }
            Ok(Box::new(move || {
                check_project(&expected, Some(&project))?;
                if let Some(path) = &workspace {
                    if !is_workspace_of(std::path::Path::new(&project), path) {
                        return Ok(json!({
                            "started": false,
                            "reason": "That workspace isn't part of this project on the desktop any more. Choose another, or a new workspace.",
                        }));
                    }
                }
                start_in_a_window(&expected, StartKind::Session { project, agent, model, workspace, branch, prompt })
            }))
        }
        _ => Ok(Box::new(move || start_options(&expected))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> AccountKey {
        AccountKey { user_id: "u".into(), org: "org-1".into() }
    }

    #[test]
    fn requests_name_their_fields_exactly() {
        let refused = |method: &str, params: Value| job(&account(), method, &params).is_err();
        assert!(refused("tasks.start", json!({"task": "DRA-1"})), "no agent");
        assert!(refused("tasks.start", json!({"task": "DRA-1", "agent": "rm -rf"})));
        assert!(refused("tasks.start", json!({"task": "DRA-1", "agent": "claude", "project": "/a/../b"})));
        assert!(refused("coordination.create", json!({"project": "/repo", "goal": " ", "lead": "claude"})));
        assert!(refused("coordination.create", json!({"project": "/repo", "goal": "x", "lead": "claude", "cost": "free"})));
        assert!(refused("coordination.create", json!({"project": "/repo", "goal": "x", "lead": "claude", "workers": ["nope"]})));
        assert!(refused("coordination.options", json!({"x": 1})));
        let ok = |method: &str, params: Value| job(&account(), method, &params).is_ok();
        assert!(ok("tasks.start", json!({"task": "DRA-1", "agent": "claude"})));
        assert!(ok("tasks.start", json!({"task": "DRA-1", "agent": "dragent", "project": "/repo"})));
        assert!(ok("coordination.create", json!({"project": "/repo", "goal": "Ship it", "lead": "claude", "workers": ["codex", "codex"], "cost": "best"})));
        assert!(ok("coordination.options", json!({})));
    }

    #[test]
    fn new_sessions_are_checked_before_anything_starts() {
        let refused = |params: Value| job(&account(), "sessions.start", &params).is_err();
        let ok = |params: Value| job(&account(), "sessions.start", &params).is_ok();
        assert!(ok(json!({"project": "/repo", "agent": "claude", "prompt": "Fix the login"})));
        assert!(ok(json!({"project": "/repo", "agent": "dragent", "prompt": "x", "workspace": "/repo/.worktrees/a"})));
        assert!(ok(json!({"project": "/repo", "agent": "codex", "prompt": "x", "branch": "main"})));
        assert!(refused(json!({"project": "/repo", "agent": "claude"})), "no prompt");
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "   "})), "blank prompt");
        assert!(refused(json!({"project": "/repo", "agent": "rm -rf", "prompt": "x"})));
        assert!(refused(json!({"agent": "claude", "prompt": "x"})), "no project");
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "model": "not-a-model"})));
        assert!(refused(json!({"project": "/repo", "agent": "dragent", "prompt": "x", "model": "anything"})), "Dragent's own takes no model");
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "workspace": "/repo/../etc"})));
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "workspace": "relative/path"})));
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "branch": "--upload-pack=x"})));
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "branch": "a b"})));
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "workspace": "/repo/w", "branch": "main"})), "a branch is only for a new workspace");
        assert!(refused(json!({"project": "/repo", "agent": "claude", "prompt": "x", "extra": 1})));
        if let Some((id, _)) = crate::teams::models::cli_choices(PromptAgent::Claude).first() {
            assert!(ok(json!({"project": "/repo", "agent": "claude", "prompt": "x", "model": id})));
        }
    }

    #[test]
    fn agents_are_named_by_their_command_key() {
        assert_eq!(agent("dragent"), Some(PromptAgent::Dragent));
        assert_eq!(agent("claude"), Some(PromptAgent::Claude));
        assert_eq!(agent("Claude"), None);
    }

    #[test]
    fn a_start_outside_this_accounts_projects_is_refused() {
        let work = job(&account(), "coordination.create", &json!({"project": "/nowhere", "goal": "x", "lead": "claude"})).unwrap();
        assert_eq!(work().unwrap_err(), Failure::Gone);
    }
}
