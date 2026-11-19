//! The issues each workspace was started from, so its header can show them.
//!
//! Kept per organisation in `workspace-issues.json`, keyed by the
//! workspace's folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::Provider;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedIssue {
    pub provider: Provider,
    pub key: String,
    pub title: String,
    pub url: String,
}

type Links = HashMap<PathBuf, Vec<LinkedIssue>>;

fn file(org_dir: &Path) -> PathBuf {
    org_dir.join("workspace-issues.json")
}

fn read(org_dir: &Path) -> Links {
    std::fs::read_to_string(file(org_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write(org_dir: &Path, links: &Links) -> Result<(), String> {
    let text = serde_json::to_string_pretty(links).map_err(|error| error.to_string())?;
    let path = file(org_dir);
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|error| format!("Couldn't save the workspace's issues: {error}"))
}

fn key(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// The header asks on every frame: keep the selected organisation's links
/// in memory.
fn cache() -> &'static Mutex<Option<(PathBuf, Links)>> {
    static CACHE: OnceLock<Mutex<Option<(PathBuf, Links)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn with_links<T>(org_dir: &Path, f: impl FnOnce(&mut Links) -> T) -> T {
    let mut cache = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    if cache.as_ref().map(|(dir, _)| dir.as_path()) != Some(org_dir) {
        *cache = Some((org_dir.to_path_buf(), read(org_dir)));
    }
    let (_, links) = cache.as_mut().expect("just filled");
    f(links)
}

/// The issues the workspace at `root` (its resolved folder, as workspace
/// records keep it) was started from. Cheap: the header asks every frame.
pub fn for_workspace(root: &Path) -> Vec<LinkedIssue> {
    for_workspace_in(&dragent_paths::organisation_dir(), root)
}

fn for_workspace_in(org_dir: &Path, root: &Path) -> Vec<LinkedIssue> {
    with_links(org_dir, |links| {
        links.get(root).cloned().unwrap_or_default()
    })
}

/// Remember that the workspace at `root` was started from `issues`.
pub fn record(root: &Path, issues: Vec<LinkedIssue>) -> Result<(), String> {
    record_in(&dragent_paths::organisation_dir(), root, issues)
}

fn record_in(org_dir: &Path, root: &Path, issues: Vec<LinkedIssue>) -> Result<(), String> {
    with_links(org_dir, |links| {
        if issues.is_empty() {
            links.remove(&key(root));
        } else {
            links.insert(key(root), issues);
        }
        // Forget workspaces whose folder is gone (deleted worktrees).
        links.retain(|root, _| root.exists());
        write(org_dir, links)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_remember_their_issues() {
        let org = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let resolved = workspace.path().canonicalize().unwrap();
        let gone = org.path().join("deleted-worktree");
        let issue = LinkedIssue {
            provider: Provider::Linear,
            key: "ENG-12".into(),
            title: "Retry uploads".into(),
            url: "https://linear.app/a/issue/ENG-12".into(),
        };
        assert!(for_workspace_in(org.path(), &resolved).is_empty());
        record_in(org.path(), workspace.path(), vec![issue.clone()]).unwrap();
        record_in(org.path(), &gone, vec![issue.clone()]).unwrap();
        assert_eq!(for_workspace_in(org.path(), &resolved), vec![issue.clone()]);
        // Read back from disk, not just the cache; the missing folder is dropped.
        let saved = read(org.path());
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[&key(workspace.path())], vec![issue]);
        record_in(org.path(), workspace.path(), Vec::new()).unwrap();
        assert!(for_workspace_in(org.path(), &resolved).is_empty());
    }
}
