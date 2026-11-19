//! The organisation's connected trackers for the phone (Dragent companion):
//! the Tasks page's tracker view, through this desktop's own connections
//! (the keys never leave it). Same calls the desktop page makes.
//!
//! Methods:
//! - `tasks.trackers {}` -> `{trackers:[{key, label}]}`
//! - `tasks.tracker_items {provider, project?}` -> `{items:[…]}`
//! - `tasks.tracker_move {provider, project?, item, status}` -> `{item}`
//! - `tasks.tracker_import {provider, project?, item}` -> `{task}` (the task
//!   it was imported as before, when it was)
//! - `tasks.tracker_create {provider, project?, title, body?, place?}` -> `{item}`
//! - `tasks.tracker_start {provider, project?, item, agent}` -> `{started, reason?}`
//!
//! `project` is a sidebar project folder: code hosts answer per repository.
//! Without one, the tracker is asked from the home folder, as on the
//! desktop when no project is in front.

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use super::companion_host::Failure;
use super::desktop_catalog::AccountKey;
use crate::integrations::tracker::TrackerItem;
use crate::integrations::Provider;

pub(crate) const METHODS: &[&str] = &[
    "tasks.trackers",
    "tasks.tracker_items",
    "tasks.tracker_move",
    "tasks.tracker_import",
    "tasks.tracker_create",
    "tasks.tracker_start",
];

const MAX_ITEMS: usize = 300;
const MAX_TITLE_CHARS: usize = 500;
const MAX_BODY_CHARS: usize = 20_000;

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn text(object: &Map<String, Value>, key: &str, max: usize) -> Result<Option<String>, Failure> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= max && !value.contains('\0') => Ok(Some(value.clone())),
        _ => Err(Failure::Invalid),
    }
}

/// A connected tracker named by its key.
fn provider(object: &Map<String, Value>) -> Result<Provider, Failure> {
    text(object, "provider", 32)?
        .as_deref()
        .and_then(Provider::from_key)
        .filter(|provider| crate::integrations::tracker::tracker_providers().contains(provider))
        .ok_or(Failure::Invalid)
}

fn project(object: &Map<String, Value>) -> Result<Option<String>, Failure> {
    match text(object, "project", 4096)? {
        Some(path) if path.is_empty() || path.split('/').any(|part| part == "..") => Err(Failure::Invalid),
        other => Ok(other),
    }
}

/// Where the tracker is asked from: the sidebar project named, else home.
fn root(expected: &AccountKey, project: Option<&str>) -> Result<(PathBuf, Option<PathBuf>), Failure> {
    match project {
        Some(project) => {
            let roots = super::companion_coordination::project_roots(expected).ok_or(Failure::Gone)?;
            if !roots.contains(project) {
                return Err(Failure::Gone);
            }
            Ok((PathBuf::from(project), Some(PathBuf::from(project))))
        }
        None => Ok((dirs::home_dir().unwrap_or_default(), None)),
    }
}

fn connected(provider: Provider) -> Result<(), Failure> {
    crate::integrations::tracker::connected_trackers()
        .contains(&provider)
        .then_some(())
        .ok_or(Failure::Gone)
}

/// This desktop's organisation, when it's still the channel's.
fn organisation(expected: &AccountKey) -> Option<String> {
    if expected.org.is_empty() || dragent_paths::organisation_id() != expected.org {
        return None;
    }
    crate::tasks::client::active_org().ok().filter(|org| *org == expected.org)
}

/// The task an item was imported as, from the desktop's saved task list.
fn imported_as(org: Option<&str>, item: &TrackerItem) -> Option<String> {
    let cached = crate::tasks::cache::load(org?)?;
    let reference = dragent_tasks::item_reference(item.provider.key(), &item.key);
    cached
        .tasks
        .iter()
        .find(|task| task.links_of(dragent_tasks::LinkKind::Item).any(|link| link.reference == reference))
        .map(|task| task.key.clone())
}

fn item_json(item: &TrackerItem, imported: Option<String>) -> Value {
    let url = Some(item.url.as_str())
        .filter(|url| (url.starts_with("https://") || url.starts_with("http://")) && url.len() <= 2048);
    json!({
        "id": clip(item.id(), 256),
        "key": clip(&item.key, 128),
        "title": clip(&item.title, MAX_TITLE_CHARS),
        "url": url,
        "status": item.status.key(),
        "state_label": item.state_label.as_deref().map(|label| clip(label, 64)),
        "assignee": item.assignee.as_deref().map(|name| clip(name, 200)),
        "labels": item.labels.iter().take(20).map(|label| clip(label, 64)).collect::<Vec<_>>(),
        "updated_at": item.updated_at_ms,
        "imported_task": imported,
        "can_move": crate::integrations::tracker::tracker_providers().contains(&item.provider),
    })
}

/// The item `id` of the tracker, as it is now.
fn find(provider: Provider, root: &std::path::Path, id: &str) -> Result<TrackerItem, Failure> {
    crate::integrations::tracker::list(provider, root, "")
        .map_err(|_| Failure::Unavailable)?
        .into_iter()
        .find(|item| item.id() == id)
        .ok_or(Failure::Gone)
}

/// Check a request on the socket thread and return the work to run off it.
pub(crate) fn job(expected: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    let allowed: &[&str] = match method {
        "tasks.trackers" => &[],
        "tasks.tracker_items" => &["provider", "project"],
        "tasks.tracker_move" => &["provider", "project", "item", "status"],
        "tasks.tracker_import" => &["provider", "project", "item"],
        "tasks.tracker_create" => &["provider", "project", "title", "body", "place"],
        "tasks.tracker_start" => &["provider", "project", "item", "agent"],
        _ => return Err(Failure::Invalid),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    let expected = expected.clone();
    if method == "tasks.trackers" {
        return Ok(Box::new(move || {
            if dragent_paths::organisation_id() != expected.org {
                return Err(Failure::Gone);
            }
            let trackers: Vec<Value> = crate::integrations::tracker::connected_trackers()
                .into_iter()
                .map(|provider| json!({"key": provider.key(), "label": provider.label()}))
                .collect();
            Ok(json!({"trackers": trackers}))
        }));
    }
    let provider = provider(object)?;
    let project = project(object)?;
    let item_id = || {
        text(object, "item", 256)?
            .filter(|id| !id.is_empty())
            .ok_or(Failure::Invalid)
    };
    match method {
        "tasks.tracker_items" => Ok(Box::new(move || {
            let (root, _) = root(&expected, project.as_deref())?;
            connected(provider)?;
            let org = organisation(&expected);
            let items = crate::integrations::tracker::list(provider, &root, "").map_err(|_| Failure::Unavailable)?;
            let items: Vec<Value> = items
                .iter()
                .take(MAX_ITEMS)
                .map(|item| item_json(item, imported_as(org.as_deref(), item)))
                .collect();
            Ok(json!({"items": items}))
        })),
        "tasks.tracker_move" => {
            let id = item_id()?;
            let status = text(object, "status", 32)?
                .and_then(|key| crate::tasks::client::parse_status(&key).ok())
                .ok_or(Failure::Invalid)?;
            if !crate::integrations::tracker::supports_status(provider, status) {
                return Err(Failure::Invalid);
            }
            Ok(Box::new(move || {
                let (root, _) = root(&expected, project.as_deref())?;
                connected(provider)?;
                let mut item = find(provider, &root, &id)?;
                let label = crate::integrations::tracker::set_status(&root, &item, status).map_err(|_| Failure::Unavailable)?;
                item.status = status;
                item.state_label = Some(label).filter(|label| !label.is_empty()).or(item.state_label);
                Ok(json!({"item": item_json(&item, imported_as(organisation(&expected).as_deref(), &item))}))
            }))
        }
        "tasks.tracker_import" => {
            let id = item_id()?;
            Ok(Box::new(move || {
                let (root, _) = root(&expected, project.as_deref())?;
                connected(provider)?;
                let org = organisation(&expected).ok_or(Failure::Gone)?;
                let item = find(provider, &root, &id)?;
                // Imported before: that task, not a second one.
                if let Some(key) = imported_as(Some(&org), &item) {
                    let task = crate::tasks::client::detail(&org, &key).map_err(|_| Failure::Unavailable)?.task;
                    return Ok(json!({"task": super::companion_tasks::task_summary(&task)}));
                }
                let task = crate::tasks_page::data::import_item(&org, &item).map_err(|_| Failure::Unavailable)?;
                Ok(json!({"task": super::companion_tasks::task_summary(&task)}))
            }))
        }
        "tasks.tracker_create" => {
            let title = text(object, "title", MAX_TITLE_CHARS)?
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty())
                .ok_or(Failure::Invalid)?;
            let body = text(object, "body", MAX_BODY_CHARS)?.unwrap_or_default();
            let place = text(object, "place", 200)?.map(|place| place.trim().to_string()).filter(|place| !place.is_empty());
            Ok(Box::new(move || {
                let (root, _) = root(&expected, project.as_deref())?;
                connected(provider)?;
                let item = crate::tasks_page::data::create_tracker_item(provider, &root, title, body, place)
                    .map_err(|_| Failure::Unavailable)?;
                Ok(json!({"item": item_json(&item, None)}))
            }))
        }
        _ => {
            let id = item_id()?;
            let agent = text(object, "agent", 64)?
                .as_deref()
                .and_then(super::companion_starts::agent)
                .ok_or(Failure::Invalid)?;
            Ok(Box::new(move || {
                let (root, project_root) = root(&expected, project.as_deref())?;
                connected(provider)?;
                let item = find(provider, &root, &id)?;
                super::companion_starts::start_in_a_window(
                    &expected,
                    super::companion_starts::StartKind::Item { item: Box::new(item), root: project_root, agent },
                )
            }))
        }
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
        assert!(refused("tasks.tracker_items", json!({})), "no tracker");
        assert!(refused("tasks.tracker_items", json!({"provider": "slack"})), "not a tracker");
        assert!(refused("tasks.tracker_items", json!({"provider": "linear", "project": "/a/../b"})));
        assert!(refused("tasks.tracker_move", json!({"provider": "linear", "item": "x", "status": "sideways"})));
        assert!(refused("tasks.tracker_create", json!({"provider": "linear", "title": " "})));
        assert!(refused("tasks.tracker_start", json!({"provider": "linear", "item": "x"})), "no agent");
        assert!(refused("tasks.trackers", json!({"x": 1})));
        let ok = |method: &str, params: Value| job(&account(), method, &params).is_ok();
        assert!(ok("tasks.trackers", json!({})));
        assert!(ok("tasks.tracker_items", json!({"provider": "github", "project": "/repo"})));
        assert!(ok("tasks.tracker_move", json!({"provider": "linear", "item": "abc", "status": "done"})));
        assert!(ok("tasks.tracker_import", json!({"provider": "linear", "item": "abc"})));
        assert!(ok("tasks.tracker_create", json!({"provider": "github", "title": "Bug", "place": "acme/app"})));
        assert!(ok("tasks.tracker_start", json!({"provider": "linear", "item": "abc", "agent": "claude"})));
    }

    #[test]
    fn a_project_outside_this_accounts_sidebar_is_refused() {
        let work = job(&account(), "tasks.tracker_items", &json!({"provider": "linear", "project": "/nowhere"})).unwrap();
        assert_eq!(work().unwrap_err(), Failure::Gone);
    }
}
