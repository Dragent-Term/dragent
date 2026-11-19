//! Past sessions for a paired phone (Dragent companion): earlier agent
//! conversations started in a project or workspace folder (Claude Code,
//! Codex, Gemini, OpenCode, MiMo, Copilot, Cursor; see
//! `dragent_host::agent_history`), continuing one in a new desktop tab, and
//! deleting one for good.
//!
//! The phone names a folder and a conversation (agent + id) only: the
//! desktop finds the conversation again in its own history, so nothing the
//! phone sends is ever used as a path to read, run or delete.
//!
//! Methods:
//! - `history.list {folder}` -> `{sessions:[{provider, id, title,
//!   updated_at, account_label, size?}]}`
//! - `history.resume {folder, provider, id}` -> `{started, reason?}`
//! - `history.delete {folder, provider, id}` -> `{deleted:true, freed,
//!   reusable}` or `{deleted:false, reason}`

use std::path::{Path, PathBuf};

use dragent_host::agent_history::{self, PastSession};
use serde_json::{json, Map, Value};

use super::companion_host::Failure;
use super::companion_starts::{start_in_a_window, StartKind};
use super::desktop_catalog::AccountKey;

pub(crate) const METHODS: &[&str] = &["history.list", "history.resume", "history.delete"];

/// Conversations listed at most.
const LIMIT: usize = 80;

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

fn text(object: &Map<String, Value>, key: &str, max: usize) -> Result<String, Failure> {
    match object.get(key) {
        Some(Value::String(value)) if !value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control) => {
            Ok(value.clone())
        }
        _ => Err(Failure::Invalid),
    }
}

/// An absolute folder with no `..` in it.
fn folder(object: &Map<String, Value>) -> Result<PathBuf, Failure> {
    let path = text(object, "folder", 4096)?;
    let absolute = path.starts_with('/') || path.get(1..3) == Some(":\\");
    if !absolute || path.split(['/', '\\']).any(|part| part == "..") {
        return Err(Failure::Invalid);
    }
    Ok(PathBuf::from(path))
}

/// The account's project `folder` belongs to: the project itself, or one of
/// its workspaces (asked of git).
fn project_of(expected: &AccountKey, folder: &Path) -> Result<String, Failure> {
    let roots = super::companion_coordination::project_roots(expected).ok_or(Failure::Gone)?;
    roots
        .into_iter()
        .find(|root| super::companion_starts::is_workspace_of(Path::new(root), folder))
        .ok_or(Failure::Gone)
}

fn row(session: &PastSession) -> Value {
    json!({
        "provider": session.provider,
        "id": session.id,
        "title": session.title,
        "updated_at": session.updated_at,
        "account_label": if session.account == "system" { Value::Null } else { Value::String(session.account_label.clone()) },
        "size": session.size,
    })
}

/// The conversation as the desktop's own history has it, in `folder`.
fn find(folder: &Path, provider: &str, id: &str) -> Option<PastSession> {
    agent_history::list(&[folder.to_path_buf()], LIMIT * 4)
        .into_iter()
        .find(|session| session.provider == provider && session.id == id)
}

pub(crate) fn job(expected: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    let allowed: &[&str] = match method {
        "history.list" => &["folder"],
        "history.resume" | "history.delete" => &["folder", "provider", "id"],
        _ => return Err(Failure::Invalid),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    let folder = folder(object)?;
    let expected = expected.clone();
    if method == "history.list" {
        return Ok(Box::new(move || {
            project_of(&expected, &folder)?;
            let sessions: Vec<Value> = agent_history::list(&[folder], LIMIT).iter().map(row).collect();
            Ok(json!({"sessions": sessions}))
        }));
    }
    let provider = text(object, "provider", 32)?;
    let id = text(object, "id", 128)?;
    let method = method.to_string();
    Ok(Box::new(move || {
        let project = project_of(&expected, &folder)?;
        let Some(session) = find(&folder, &provider, &id) else {
            return Ok(json!({"started": false, "deleted": false, "reason": "That conversation isn't in this folder's history any more."}));
        };
        if method == "history.resume" {
            return start_in_a_window(&expected, StartKind::Resume { project, session: Box::new(session) });
        }
        Ok(match agent_history::delete(&session) {
            Ok(deleted) => json!({"deleted": true, "freed": deleted.freed, "reusable": deleted.reusable}),
            Err(reason) => json!({"deleted": false, "reason": reason}),
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> AccountKey {
        AccountKey { user_id: "u".into(), org: "org-1".into() }
    }

    #[test]
    fn requests_name_a_plain_folder_and_conversation() {
        let refused = |method: &str, params: Value| job(&account(), method, &params).is_err();
        assert!(!refused("history.list", json!({"folder": "/repo"})));
        assert!(!refused("history.resume", json!({"folder": "/repo/.wt/a", "provider": "claude", "id": "abc"})));
        assert!(refused("history.list", json!({})));
        assert!(refused("history.list", json!({"folder": "relative"})));
        assert!(refused("history.list", json!({"folder": "/repo/../etc"})));
        assert!(refused("history.list", json!({"folder": "/repo", "extra": 1})));
        assert!(refused("history.delete", json!({"folder": "/repo", "provider": "claude"})), "no id");
        assert!(refused("history.delete", json!({"folder": "/repo", "provider": "claude", "id": "a\nb"})));
        assert!(refused("history.nope", json!({"folder": "/repo"})));
    }

    #[test]
    fn a_folder_outside_this_accounts_projects_is_refused() {
        let work = job(&account(), "history.list", &json!({"folder": "/nowhere"})).unwrap();
        assert_eq!(work().unwrap_err(), Failure::Gone);
    }
}
