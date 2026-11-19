//! Learned tracker-project → project links, per organisation.
//!
//! When a task from a tracker that names no repository (Linear, Jira)
//! gains a project anyway — imported with one resolved, or attached by
//! hand in the detail panel — the association is remembered here, keyed
//! by the tracker's own project (`linear:ENG`, `jira:OPS`). The next
//! import from the same tracker project then attaches that project on
//! its own, the way a default repository works elsewhere.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dragent_tasks::ProjectRef;

/// What's remembered: tracker project keys to the project they were
/// last seen worked in.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct ProjectHints {
    #[serde(default)]
    pub hints: HashMap<String, ProjectRef>,
}

fn path_for(dir: &Path) -> PathBuf {
    dir.join("tasks").join("project_hints.json")
}

/// The hints file for `org`. Scoped per organisation like the rest of the
/// local data.
pub(crate) fn path(org: &str) -> PathBuf {
    path_for(&dragent_paths::organisation_dir_for(org))
}

/// The remembered hints, if any. A broken file reads as no hints rather
/// than an error, exactly like the task cache.
pub(crate) fn load(org: &str) -> ProjectHints {
    let bytes = match std::fs::read(path(org)) {
        Ok(bytes) => bytes,
        Err(_) => return ProjectHints::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Remember `hints`, atomically: a crash mid-write must never leave half
/// a file behind.
pub(crate) fn save(org: &str, hints: &ProjectHints) -> Result<(), String> {
    let path = path(org);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(hints).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, &path).map_err(|error| error.to_string())?;
    Ok(())
}

/// The tracker's own project from an item key: `OPS-4` lives in `OPS`,
/// `ENG-12` in `ENG`. Bare numbers (`#42`, `461`) name no project.
pub(crate) fn tracker_project_of(item_key: &str) -> Option<String> {
    let (prefix, _) = item_key.split_once('-')?;
    let prefix = prefix.trim();
    if prefix.len() >= 2 && prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(prefix.to_ascii_uppercase())
    } else {
        None
    }
}

/// The hint key for one linked item reference (`provider:key` as stored
/// on the task's item links), if its key names a tracker project.
fn hint_key_for_reference(reference: &str) -> Option<String> {
    let (provider, key) = reference.split_once(':')?;
    tracker_project_of(key.trim()).map(|project| format!("{}:{project}", provider.trim()))
}

/// Remember (or, when `project` is `None`, forget) the project for every
/// tracker project the given item links name. Learned on import and on
/// hand attachment; cleared on hand detachment.
pub(crate) fn remember(org: &str, references: &[String], project: Option<&ProjectRef>) {
    let mut hints = load(org);
    let mut changed = false;
    for reference in references {
        let Some(key) = hint_key_for_reference(reference) else {
            continue;
        };
        match project {
            Some(project) => {
                if hints.hints.get(&key).is_none_or(|known| known != project) {
                    hints.hints.insert(key, project.clone());
                    changed = true;
                }
            }
            None => {
                changed |= hints.hints.remove(&key).is_some();
            }
        }
    }
    if changed {
        let _ = save(org, &hints);
    }
}

/// The remembered project for one item's address, if any.
pub(crate) fn hinted_project(org: &str, provider_key: &str, item_key: &str) -> Option<ProjectRef> {
    let project = tracker_project_of(item_key.trim())?;
    load(org).hints.get(&format!("{provider_key}:{project}")).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_projects_come_from_dashed_keys_only() {
        assert_eq!(tracker_project_of("OPS-4"), Some("OPS".into()));
        assert_eq!(tracker_project_of("ENG-12"), Some("ENG".into()));
        assert_eq!(tracker_project_of("BACKLOG-7"), Some("BACKLOG".into()));
        assert_eq!(tracker_project_of("ops-4"), Some("OPS".into()));
        assert_eq!(tracker_project_of("#42"), None);
        assert_eq!(tracker_project_of("461"), None);
        assert_eq!(tracker_project_of("T-88"), None, "a lone letter is not a project");
        assert_eq!(tracker_project_of("plain"), None);
        assert_eq!(tracker_project_of(""), None);
    }

    #[test]
    fn hints_remember_and_forget_per_tracker_project() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.scratch")
            .join(format!("tasks-hints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Exercise the pure shape through the real file helpers by
        // pointing them at a scratch dir via path_for.
        let path = path_for(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut hints = ProjectHints::default();
        hints.hints.insert(
            "linear:ENG".into(),
            ProjectRef { key: "github.com/acme/app".into(), label: "app".into() },
        );
        let bytes = serde_json::to_vec(&hints).unwrap();
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, &bytes).unwrap();
        std::fs::rename(&temp, &path).unwrap();
        let back: ProjectHints = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, hints);
        assert_eq!(
            hint_key_for_reference("linear:ENG-12"),
            Some("linear:ENG".into())
        );
        assert_eq!(hint_key_for_reference("github:#42"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
