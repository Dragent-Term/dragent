//! Persistent workspace trust decisions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ssh::SshDestination;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceTrustStore {
    #[serde(skip)]
    storage_path: Option<PathBuf>,
    #[serde(default)]
    trusted_roots: HashSet<PathBuf>,
    /// Most recently opened roots, newest first. Trust and recency remain
    /// separate concepts so this list can drive navigation without weakening
    /// the workspace trust boundary.
    #[serde(default)]
    recent_roots: Vec<PathBuf>,
    #[serde(default)]
    recent_ssh_hosts: Vec<SshDestination>,
    #[serde(default)]
    hidden_roots: HashSet<PathBuf>,
    #[serde(default)]
    project_groups: HashMap<PathBuf, Vec<String>>,
    #[serde(default)]
    workspace_groups: HashMap<PathBuf, String>,
}

impl WorkspaceTrustStore {
    pub fn load() -> Self {
        let path = store_path();
        let mut store: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        store.storage_path = Some(path);
        if store.move_worktree_folders_to_their_projects() {
            store.save();
        }
        store
    }

    /// Folders recorded under a worktree instead of its project (automation
    /// runs filed before the project was worked out) move to the project, so
    /// the runs show inside them. Answers whether anything moved.
    fn move_worktree_folders_to_their_projects(&mut self) -> bool {
        let misplaced: Vec<(PathBuf, PathBuf)> = self
            .project_groups
            .keys()
            .filter_map(|key| Some((key.clone(), normalized(&main_checkout(key)?))))
            .collect();
        for (worktree, project) in &misplaced {
            let moved = self.project_groups.remove(worktree).unwrap_or_default();
            let groups = self.project_groups.entry(project.clone()).or_default();
            for group in moved {
                if !groups.iter().any(|name| name.eq_ignore_ascii_case(&group)) {
                    groups.push(group);
                }
            }
        }
        !misplaced.is_empty()
    }

    /// Whether tooling may run with the current workspace trust policy.
    pub fn permits(&self, root: &Path, enforce: bool) -> bool {
        !enforce || self.is_trusted(root)
    }

    pub fn is_trusted(&self, root: &Path) -> bool {
        // Remote folders were picked on a machine the user added, and no
        // local tooling runs against them.
        if crate::remote_fs::is_remote(root) {
            return true;
        }
        let root = normalized(root);
        let trusted = |path: &Path| self.trusted_roots.iter().any(|trusted| path.starts_with(trusted));
        if trusted(&root) {
            return true;
        }
        // A worktree of a trusted project is trusted too: the person trusted
        // the project before Dragent made checkouts of it, which live in
        // another folder.
        main_checkout(&root).is_some_and(|main| trusted(&normalized(&main)))
    }

    pub fn trust(&mut self, root: &Path) {
        let root = normalized(root);
        self.trusted_roots.insert(root.clone());
        self.touch_recent(&root);
    }

    pub fn touch_recent(&mut self, root: &Path) {
        let root = normalized(root);
        // Worktrees are checkouts of a project, not projects themselves.
        // The recent-projects surface must only list actual (main) checkouts,
        // so never record a linked worktree here. Trust is unaffected —
        // `trust()` inserts into `trusted_roots` before reaching this.
        if super::project_naming::worktree_main_checkout(&root).is_some() {
            return;
        }
        self.hidden_roots.remove(&root);
        self.recent_roots.retain(|recent| recent != &root);
        self.recent_roots.insert(0, root);
        self.recent_roots.truncate(12);
        self.save();
    }

    pub fn is_hidden(&self, root: &Path) -> bool {
        self.hidden_roots.contains(&normalized(root))
    }

    pub fn hide(&mut self, root: &Path) {
        self.hidden_roots.insert(normalized(root));
        self.save();
    }

    pub fn show(&mut self, root: &Path) {
        self.hidden_roots.remove(&normalized(root));
        self.save();
    }

    pub fn groups(&self, root: &Path) -> Vec<String> {
        self.project_groups
            .get(&normalized(root))
            .cloned()
            .unwrap_or_default()
    }

    pub fn add_group(&mut self, root: &Path, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let groups = self.project_groups.entry(normalized(root)).or_default();
        if groups.iter().any(|group| group.eq_ignore_ascii_case(name)) {
            return false;
        }
        groups.push(name.to_string());
        self.save();
        true
    }

    pub fn workspace_group(&self, root: &Path) -> Option<&str> {
        self.workspace_groups
            .get(&normalized(root))
            .map(String::as_str)
    }

    pub fn remove_group(&mut self, project: &Path, group: &str) {
        let project = normalized(project);
        if let Some(groups) = self.project_groups.get_mut(&project) {
            groups.retain(|name| name != group);
        }
        self.workspace_groups.retain(|workspace, name| {
            name != group || !super::project_naming::belongs_to_project(workspace, &project)
        });
        self.save();
    }

    pub fn set_workspace_group(&mut self, root: &Path, group: Option<String>) {
        let root = normalized(root);
        if let Some(group) = group {
            self.workspace_groups.insert(root, group);
        } else {
            self.workspace_groups.remove(&root);
        }
        self.save();
    }

    pub fn forget_workspace(&mut self, root: &Path) {
        self.workspace_groups.remove(&normalized(root));
        self.save();
    }

    /// Remove Dragent's project navigation metadata without touching its folder.
    /// Deliberately keeps the trust decision: removing a sidebar entry must
    /// not revoke filesystem trust, otherwise re-adding the same folder
    /// re-prompts even though the user already trusted it.
    pub fn forget_project(&mut self, root: &Path) {
        let root = normalized(root);
        self.recent_roots.retain(|recent| !super::project_naming::belongs_to_project(recent, &root));
        self.hidden_roots.remove(&root);
        self.project_groups.remove(&root);
        self.workspace_groups.retain(|workspace, _| {
            !super::project_naming::belongs_to_project(workspace, &root)
        });
        self.save();
    }

    pub fn recent_roots(&self) -> Vec<PathBuf> {
        self.recent_roots
            .iter()
            .filter(|root| root.is_dir())
            // Belt to the `touch_recent` guard: entries recorded before the
            // worktree exclusion (or written by other paths) must never
            // surface as projects.
            .filter(|root| super::project_naming::worktree_main_checkout(root).is_none())
            .cloned()
            .collect()
    }

    pub fn touch_recent_ssh_host(&mut self, destination: SshDestination) {
        self.recent_ssh_hosts
            .retain(|recent| recent != &destination);
        self.recent_ssh_hosts.insert(0, destination);
        self.recent_ssh_hosts.truncate(12);
        self.save();
    }

    pub fn forget_ssh_host(&mut self, destination: &SshDestination) {
        self.recent_ssh_hosts.retain(|host| host != destination);
        self.save();
    }

    pub fn recent_ssh_hosts(&self) -> Vec<SshDestination> {
        self.recent_ssh_hosts.clone()
    }

    fn save(&self) {
        // Unit tests must never touch the developer's real trust file
        // (several state transitions save as a side effect).
        #[cfg(test)]
        return;
        let path = self.storage_path.clone().unwrap_or_else(store_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// The project a linked worktree belongs to, `None` for anything else.
/// Read from its `.git` pointer file, then remembered: trust is checked
/// while drawing.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    use std::sync::{Mutex, OnceLock};
    // Only worktrees found are remembered: a folder checked while git is
    // still making it a worktree is looked at again next time.
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(found) = known.lock().ok().and_then(|known| known.get(root).cloned()) {
        return Some(found);
    }
    // Only a folder holding a `.git` file can be a linked worktree.
    if !root.join(".git").is_file() {
        return None;
    }
    let found = super::project_naming::worktree_main_checkout(root)?;
    if let Ok(mut known) = known.lock() {
        known.insert(root.to_path_buf(), found.clone());
    }
    Some(found)
}

fn normalized(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn store_path() -> PathBuf {
    dragent_paths::organisation_dir().join("workspace-trust.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_trust_allows_tooling_without_persisting_a_grant() {
        let mut store = WorkspaceTrustStore::default();
        let root = Path::new("/work/untrusted");
        assert!(!store.permits(root, true));
        assert!(store.permits(root, false));
        assert!(!store.is_trusted(root));
        assert!(!store.permits(root, true));
        store.trusted_roots.insert(PathBuf::from("/work/trusted"));
        assert!(store.permits(Path::new("/work/trusted/src"), true));
    }

    #[test]
    fn trust_is_scoped_to_the_selected_root() {
        let mut store = WorkspaceTrustStore::default();
        store.trusted_roots.insert(PathBuf::from("/work/project"));
        assert!(store.is_trusted(Path::new("/work/project/src")));
        assert!(!store.is_trusted(Path::new("/work/other")));
    }

    #[test]
    fn recently_opened_roots_are_unique_and_newest_first() {
        let mut store = WorkspaceTrustStore::default();
        store.recent_roots = vec![PathBuf::from("/work/first")];
        let second = PathBuf::from("/work/second");
        store.recent_roots.insert(0, second.clone());
        store
            .recent_roots
            .retain(|root| root != Path::new("/work/first"));
        store.recent_roots.insert(0, PathBuf::from("/work/first"));

        assert_eq!(store.recent_roots[0], PathBuf::from("/work/first"));
        assert_eq!(store.recent_roots[1], second);
    }

    #[test]
    fn recent_ssh_hosts_are_unique_and_newest_first() {
        let mut store = WorkspaceTrustStore::default();
        let first = SshDestination::parse("dev@example.com").unwrap();
        let second = SshDestination::parse("ops@example.com:2222").unwrap();
        store.touch_recent_ssh_host(first.clone());
        store.touch_recent_ssh_host(second.clone());
        store.touch_recent_ssh_host(first.clone());

        assert_eq!(store.recent_ssh_hosts(), vec![first, second]);
    }

    #[test]
    fn hidden_projects_and_groups_survive_serialization() {
        let project = PathBuf::from("/project");
        let worktree = PathBuf::from("/project-worktree");
        let mut store = WorkspaceTrustStore::default();
        store.hidden_roots.insert(project.clone());
        store
            .project_groups
            .insert(project.clone(), vec!["Backend".into()]);
        store
            .workspace_groups
            .insert(worktree.clone(), "Backend".into());

        let restored: WorkspaceTrustStore =
            serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert!(restored.is_hidden(&project));
        assert_eq!(restored.groups(&project), vec!["Backend"]);
        assert_eq!(restored.workspace_group(&worktree), Some("Backend"));
    }

    #[test]
    fn recent_projects_exclude_worktrees() {
        // A linked worktree is a checkout of its main project, not a project
        // itself: opening one must never add it to the recent-projects list.
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let linked = temp.path().join("workspace");
        std::fs::create_dir_all(&linked).unwrap();
        let git_dir = main.join(".git/worktrees/workspace");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();

        let mut store = WorkspaceTrustStore::default();
        store.touch_recent(&linked);
        assert!(store.recent_roots.is_empty());

        // The main checkout still records normally (canonicalized, cf. the
        // macOS /var → /private/var symlink).
        let canonical_main = main.canonicalize().unwrap();
        store.touch_recent(&main);
        assert_eq!(store.recent_roots, vec![canonical_main.clone()]);

        // Entries recorded before the exclusion must not surface either.
        store.recent_roots.insert(0, linked.clone());
        let visible = store.recent_roots();
        assert_eq!(visible, vec![canonical_main]);
    }
    #[test]
    fn a_worktree_of_a_trusted_project_is_trusted() {
        // Worktrees live outside the project folder; opening files in one
        // must not ask for trust the person already gave the project.
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let linked = temp.path().join("worktrees/feature");
        let other = temp.path().join("elsewhere");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let git_dir = main.join(".git/worktrees/feature");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();

        let mut store = WorkspaceTrustStore::default();
        assert!(!store.is_trusted(&linked), "nothing trusted yet");
        store.trusted_roots.insert(main.canonicalize().unwrap());
        assert!(store.is_trusted(&linked));
        assert!(!store.is_trusted(&other), "an unrelated folder still asks");
    }

    #[test]
    fn folders_recorded_under_a_worktree_move_to_its_project() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let linked = temp.path().join("worktrees/run-1");
        std::fs::create_dir_all(&linked).unwrap();
        let git_dir = main.join(".git/worktrees/run-1");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();
        let (main, linked) = (main.canonicalize().unwrap(), linked.canonicalize().unwrap());

        let mut store = WorkspaceTrustStore::default();
        store.project_groups.insert(main.clone(), vec!["Automation".into()]);
        store.project_groups.insert(linked.clone(), vec!["automation".into(), "nightly".into()]);
        store.workspace_groups.insert(linked.clone(), "automation".into());
        assert!(store.move_worktree_folders_to_their_projects());
        assert_eq!(store.groups(&main), vec!["Automation", "nightly"]);
        assert!(store.groups(&linked).is_empty());
        assert_eq!(store.workspace_group(&linked), Some("automation"));
        assert!(!store.move_worktree_folders_to_their_projects(), "nothing left to move");
    }

    #[test]
    fn forgetting_a_project_keeps_its_trust() {
        // Removing a sidebar entry must not revoke filesystem trust:
        // re-adding the same folder must not re-prompt.
        let project = PathBuf::from("/work/project");
        let mut store = WorkspaceTrustStore::default();
        store.trusted_roots.insert(project.clone());
        store.recent_roots = vec![project.clone()];
        assert!(store.is_trusted(&project));
        store.forget_project(&project);
        assert!(store.is_trusted(&project));
        // Navigation metadata is still dropped (raw field: the accessor
        // filters non-existent dirs, which would vacate this assertion).
        assert!(!store.recent_roots.contains(&project));
    }
}
