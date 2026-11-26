//! Project naming and label helpers for the workspace.
//!
//! Responsible for:
//! - default on-disk location for new projects
//! - validating user-supplied project names
//! - deriving a clone directory name from a repository URL/path
//! - recognizing placeholder work-item labels vs. user-authored titles
//!
//! Not responsible for:
//! - creating projects or tabs (owned by `WorkspaceView` / project flows)
//! - rendering labels (callers decide how to display these answers)
//!
//! All functions here are pure and UI-free.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::process_ext::NoWindow;

/// Default location for new projects (`~/projects`).
/// Created lazily on submit.
pub(crate) fn default_projects_dir() -> PathBuf {
    dragent_paths::organisation_dir().join("projects")
}

pub(crate) fn valid_project_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control)
        && name != "."
        && name != ".."
}

pub(crate) fn clone_target_name(repository: &str) -> Option<String> {
    let repository = repository.trim().trim_end_matches('/');
    if repository.is_empty() || repository.chars().any(char::is_control) {
        return None;
    }
    let repository_path = if let Some((scheme, authority_and_path)) = repository.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git" | "file") {
            return None;
        }
        let (authority, path) = authority_and_path.split_once('/')?;
        if scheme != "file" && (authority.is_empty() || repository.chars().any(char::is_whitespace))
        {
            return None;
        }
        path
    } else if Path::new(repository).exists() {
        repository
    } else if let Some((host, path)) = repository.split_once(':') {
        if host.is_empty()
            || path.is_empty()
            || host.contains('/')
            || repository.chars().any(char::is_whitespace)
        {
            return None;
        }
        path
    } else {
        return None;
    };
    let name = repository_path
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git");
    (!name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character)))
    .then(|| name.to_string())
}

pub(crate) fn is_default_work_label(label: &str) -> bool {
    let label = label.trim();
    label.is_empty()
        || matches!(label, "New work" | "New work item" | "Untitled work")
        || label.starts_with("Work · ")
}

pub(crate) fn should_replace_project_tab_label(
    custom_label: Option<&str>,
    folder_name: Option<&str>,
) -> bool {
    custom_label.is_none()
        || custom_label == folder_name
        || custom_label.is_some_and(is_default_work_label)
}

pub(crate) fn bounded_project_memory_text(value: &str, max_chars: usize) -> String {
    let mut text = value.trim().chars().take(max_chars).collect::<String>();
    if value.trim().chars().count() > max_chars {
        text.push('…');
    }
    text
}
pub(crate) fn default_pane_cwd() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Managed root for project worktrees: outside the repo, one folder
/// per project (`~/.dragent/worktrees/<project>/<worktree>/`), mirroring
/// `~/.dragent/worktrees/<project>/<worktree>` layout. Keeping
/// worktrees out of the checkout avoids untracked-noise in the parent repo.
pub(crate) fn worktrees_root() -> PathBuf {
    dragent_paths::organisation_dir().join("worktrees")
}

/// Display name for a project root inside the managed worktrees root.
pub(crate) fn worktree_project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_string()
}

/// Directory segment for a worktree branch. Slashes become dashes so
/// `feature/x` nests flatly instead of creating subdirectories.
pub(crate) fn sanitize_worktree_dir_name(branch: &str) -> String {
    let mut out: String = branch
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c if c.is_ascii_alphanumeric() || "-_.".contains(c) => c,
            _ => '-',
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches(['-', '.', ' ']).to_string();
    if out.is_empty() || out == "." || out == ".." {
        "worktree".to_string()
    } else {
        out.chars().take(64).collect()
    }
}

/// Checkout directory for a worktree of `root` on `branch`.
/// Honors Settings → General → Workspace directory + nesting,
/// and per-project base-path overrides.
pub(crate) fn worktree_dir(root: &Path, branch: &str) -> PathBuf {
    // A remote project's worktrees live on its machine, under that
    // machine's own ~/.dragent/worktrees.
    if let Some(remote) = crate::remote_fs::resolve(root) {
        let home = crate::remote_fs::home(&remote.machine_id).unwrap_or_else(|| "/tmp".into());
        return crate::remote_fs::alias(
            &remote.machine_id,
            &format!(
                "{home}/.dragent/worktrees/{}/{}",
                worktree_project_name(root),
                sanitize_worktree_dir_name(branch)
            ),
        );
    }
    let base = crate::settings::git_consumers::workspace_base_dir(Some(root));
    let segment = sanitize_worktree_dir_name(branch);
    if crate::settings::git_consumers::nest_workspaces() {
        base.join(worktree_project_name(root)).join(segment)
    } else {
        base.join(format!("{}-{}", worktree_project_name(root), segment))
    }
}

/// Resolve a linked worktree's main checkout from its `.git` pointer file.
pub(crate) fn worktree_main_checkout(root: &Path) -> Option<PathBuf> {
    let git_dir = linked_worktree_git_dir(root)?;
    let common_git_dir = git_dir.parent()?.parent()?;
    (common_git_dir.file_name()? == ".git")
        .then(|| common_git_dir.parent().map(Path::to_path_buf))
        .flatten()
}

/// The project heading under which a workspace appears in the sidebar.
/// Worktrees belong to their repository even when explicitly opened.
/// Canonical paths also merge symlink spellings of the same checkout.
///
/// Working it out reads the disk (canonical paths, a worktree's pointer
/// file), so the app never does it on its main thread: it asks
/// [`known_sidebar_project_root`], and a background pass resolves what's
/// new. This blocking form is for the command line and tests.
pub(crate) fn sidebar_project_root(
    root: &PathBuf,
    _open_roots: &[PathBuf],
) -> PathBuf {
    if let Some(found) = lock(known()).get(root.as_path()) {
        return found.clone();
    }
    let resolved = resolve_sidebar_project_root(root);
    lock(known()).insert(root.clone(), resolved.clone());
    resolved
}

/// The project heading, read from the disk. Blocking.
pub(crate) fn resolve_sidebar_project_root(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // Project-less automation sessions share one "Sessions" heading.
    let sessions = automation_sessions_root();
    let sessions = sessions.canonicalize().unwrap_or(sessions);
    if canonical.starts_with(&sessions) && canonical != sessions {
        return sessions;
    }
    let main = worktree_main_checkout(&canonical).unwrap_or(canonical);
    main.canonicalize().unwrap_or(main)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Folder → project heading, as worked out. Kept on disk too, so the first
/// frame after a restart already groups workspaces under their projects.
fn known() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(load_known()))
}

/// Folders the app asked about that aren't worked out yet.
fn pending() -> &'static Mutex<HashSet<PathBuf>> {
    static PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

fn known_file() -> PathBuf {
    dragent_paths::organisation_dir().join("project-roots.json")
}

fn load_known() -> HashMap<PathBuf, PathBuf> {
    // Tests never read this computer's saved headings.
    if cfg!(test) {
        return HashMap::new();
    }
    std::fs::read_to_string(known_file())
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<(PathBuf, PathBuf)>>(&text).ok())
        .map(|pairs| pairs.into_iter().collect())
        .unwrap_or_default()
}

fn save_known(known: &HashMap<PathBuf, PathBuf>) {
    if cfg!(test) {
        return;
    }
    let pairs: Vec<(&PathBuf, &PathBuf)> = known.iter().collect();
    let Ok(text) = serde_json::to_string(&pairs) else {
        return;
    };
    let path = known_file();
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// The project heading without touching the disk: what's been worked out,
/// else the folder itself until the background pass names it (see
/// [`take_pending_project_roots`]).
pub(crate) fn known_sidebar_project_root(root: &Path) -> PathBuf {
    if let Some(found) = lock(known()).get(root) {
        return found.clone();
    }
    lock(pending()).insert(root.to_path_buf());
    root.to_path_buf()
}

/// The folders still to work out, taken for a background pass.
pub(crate) fn take_pending_project_roots() -> Vec<PathBuf> {
    lock(pending()).drain().collect()
}

/// Keep headings a background pass worked out. Answers whether any differs
/// from what the sidebar showed (a new folder showed as itself).
pub(crate) fn remember_project_roots(found: Vec<(PathBuf, PathBuf)>) -> bool {
    let mut known = lock(known());
    let (mut learned, mut moved) = (false, false);
    for (root, project) in found {
        let shown = known.get(&root).cloned().unwrap_or_else(|| root.clone());
        moved |= shown != project;
        learned |= known.get(&root) != Some(&project);
        known.insert(root, project);
    }
    if learned {
        save_known(&known);
    }
    moved
}

/// Where project-less automation sessions work (one folder per session).
pub(crate) fn automation_sessions_root() -> PathBuf {
    dragent_paths::organisation_dir().join("automation-sessions")
}

/// The heading a sidebar project shows. `root` is a heading already (see
/// [`sidebar_project_root`]), so naming it reads nothing from the disk: the
/// sidebar names every project on every refresh.
pub(crate) fn sidebar_project_label(root: &Path) -> String {
    let sessions = automation_sessions_root();
    if root == sessions || canonical_sessions_root(&sessions).is_some_and(|canonical| root == canonical) {
        return "Sessions".to_string();
    }
    worktree_project_name(root)
}

/// The sessions folder's real path, worked out once it exists.
fn canonical_sessions_root(sessions: &Path) -> Option<PathBuf> {
    static CANONICAL: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let mut known = lock(CANONICAL.get_or_init(Default::default));
    if let Some(canonical) = known.get(sessions) {
        return Some(canonical.clone());
    }
    let canonical = sessions.canonicalize().ok()?;
    known.insert(sessions.to_path_buf(), canonical.clone());
    Some(canonical)
}

pub(crate) fn belongs_to_project(workspace_root: &Path, project_root: &Path) -> bool {
    same_folder(workspace_root, project_root)
        || (project_root == automation_sessions_root() && workspace_root.starts_with(project_root))
        || worktree_main_checkout(workspace_root)
            .is_some_and(|main| same_folder(&main, project_root))
}

/// Whether two paths spell the same folder, without reading the disk. Windows
/// writes one folder as `C:\a`, as `C:/a` (git's pointer files) and as
/// `\\?\C:\a` (its canonical form), and the sidebar holds all three.
pub(crate) fn same_folder(left: &Path, right: &Path) -> bool {
    fn plain(path: &Path) -> &Path {
        path.to_str()
            .and_then(|text| text.strip_prefix(r"\\?\"))
            .map_or(path, Path::new)
    }
    plain(left) == plain(right)
}

fn linked_worktree_git_dir(root: &Path) -> Option<PathBuf> {
    let pointer = crate::remote_fs::read_to_string_cached(&root.join(".git"))?;
    let git_dir = Path::new(pointer.trim().strip_prefix("gitdir: ")?);
    Some(if git_dir.is_absolute() {
        // A remote worktree points at a machine path.
        match crate::remote_fs::resolve(root) {
            Some(remote) => crate::remote_fs::alias_remote_path(&remote.machine_id, git_dir),
            None => git_dir.to_path_buf(),
        }
    } else {
        root.join(git_dir)
    })
}

pub(crate) fn linked_worktree_branch(root: &Path) -> Option<String> {
    let head = crate::remote_fs::read_to_string_cached(&linked_worktree_git_dir(root)?.join("HEAD"))?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

pub(crate) fn workspace_title_from_prompt(prompt: &str) -> Option<String> {
    let title = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.chars().count() > 100 {
        let prefix = title.chars().take(100).collect::<String>();
        prefix
            .rsplit_once(' ')
            .map(|(words, _)| words.to_string())
            .unwrap_or(prefix)
    } else {
        title
    };
    (!title.is_empty()).then_some(title)
}

pub(crate) fn workspace_branch_from_prompt(prompt: &str, suffix: &str) -> String {
    let mut slug = String::new();
    for character in prompt.chars() {
        if slug.len() >= 30 {
            break;
        }
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    // Stop between words: a name shouldn't end halfway through one.
    if prompt.chars().filter(char::is_ascii_alphanumeric).count() > slug.chars().filter(char::is_ascii_alphanumeric).count() {
        if let Some(end) = slug.rfind('-') {
            slug.truncate(end);
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "workspace" } else { slug };
    // Branch prefix/rename + Jira casing from Git settings.
    let prefix = crate::settings::git_consumers::worktree_branch_prefix();
    let raw = format!("{prefix}/{slug}-{suffix}");
    crate::settings::git_consumers::final_branch_name(&raw)
}

/// Parse `git worktree list --porcelain` into `(path, branch)` pairs.
/// Detached and bare entries are skipped: they have no branch identity.
pub(crate) fn parse_worktree_list(output: &str) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut path: Option<&str> = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = Some(rest.trim());
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(path) = path.take() {
                let branch = branch.trim();
                if !branch.is_empty() {
                    out.push((PathBuf::from(path), branch.to_string()));
                }
            }
        } else if line.trim().is_empty() {
            path = None;
        }
    }
    out
}

/// The checkout's current local branch, if `root` is a git repo on a branch.
/// Detached HEAD, non-repos, and git failures yield `None` so callers simply
/// omit the branch suffix. Pure and UI-free; runs `git` synchronously, so
/// callers must keep it off hot paths (tab-title refresh, project open).
/// Whether a new worktree can be created for the project at `root`: git
/// branches one from a commit, and a folder that isn't a repository, or one
/// only just initialised, has none. Read from the repository's files, so it
/// is cheap enough to ask while drawing.
pub(crate) fn worktree_available(root: &Path) -> bool {
    // A project on another machine isn't readable here; its machine decides.
    if crate::remote_fs::resolve(root).is_some() {
        return true;
    }
    let git = root.join(".git");
    let (git_dir, common) = if git.is_dir() {
        (git.clone(), git)
    } else {
        let Some(git_dir) = linked_worktree_git_dir(root) else {
            return false;
        };
        let common = git_dir
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| git_dir.clone(), Path::to_path_buf);
        (git_dir, common)
    };
    let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) else {
        return false;
    };
    let Some(branch) = head.trim().strip_prefix("ref: ") else {
        // Detached: HEAD is itself a commit.
        return !head.trim().is_empty();
    };
    common.join(branch).is_file()
        || std::fs::read_to_string(common.join("packed-refs"))
            .is_ok_and(|packed| packed.lines().any(|line| line.split(' ').nth(1) == Some(branch)))
        // Branches kept in a reftable aren't files; assume the usual case.
        || common.join("reftable").is_dir()
}

pub(crate) fn local_branch_name(root: &Path) -> Option<String> {
    // Most callers run on the UI thread (building the workspace at launch
    // among them), and git runs through the host service, which can be
    // slow to answer or still starting. HEAD's file answers without either.
    // Not for a remote machine's project: its folder here is only a stand-in,
    // and its repository is elsewhere.
    if crate::remote_fs::resolve(root).is_none() {
        if let Some(known) = head_branch_from_files(root) {
            return known;
        }
    }
    let output = crate::git_cmd::GitCommand::new()
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

/// The branch checked out at `root` read from the repository's own files, as
/// `git symbolic-ref --short -q HEAD` reports it: `Some(Some(branch))`, or
/// `Some(None)` when HEAD is detached or `root` is in no repository. `None`
/// when the files can't answer (unreadable, or a repository format that keeps
/// refs elsewhere), so the caller asks git.
pub(crate) fn head_branch_from_files(root: &Path) -> Option<Option<String>> {
    let Some(dot_git) = root
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|p| p.exists())
    else {
        return Some(None);
    };
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        // A linked worktree or submodule: `gitdir: <path>`, relative to the
        // folder holding the `.git` file.
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let target = text
            .lines()
            .find_map(|line| line.strip_prefix("gitdir:"))?
            .trim();
        let target = Path::new(target);
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            dot_git.parent()?.join(target)
        }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(reference) => {
            let branch = reference.trim().strip_prefix("refs/heads/")?;
            // The reftable format leaves a placeholder here.
            (!branch.is_empty() && branch != ".invalid").then(|| Some(branch.to_string()))
        }
        // A commit id: detached.
        None if head.len() >= 40 && head.bytes().all(|b| b.is_ascii_hexdigit()) => Some(None),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_file_answers_like_symbolic_ref() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git = |dir: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(dir)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
        };
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        git(&repo, &["init", "-q", "-b", "feature/x"]);
        assert_eq!(
            head_branch_from_files(&repo),
            Some(Some("feature/x".into()))
        );
        // From a folder inside the repository, like git.
        assert_eq!(
            head_branch_from_files(&repo.join("sub")),
            Some(Some("feature/x".into()))
        );
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        // A linked worktree keeps a `.git` file pointing at its own HEAD.
        let linked = root.join("linked");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                linked.to_str().unwrap(),
            ],
        );
        assert_eq!(head_branch_from_files(&linked), Some(Some("other".into())));
        assert_eq!(local_branch_name(&linked), Some("other".into()));
        // Detached.
        git(&repo, &["checkout", "-q", "--detach"]);
        assert_eq!(head_branch_from_files(&repo), Some(None));
        assert_eq!(local_branch_name(&repo), None);
    }

    #[test]
    fn local_branch_name_none_outside_git_repo() {
        // A plain directory is never a worktree: no git invocation can
        // produce a branch here, whether or not git is installed.
        let dir = std::env::temp_dir().join("dragent-no-repo-branch-probe");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(local_branch_name(&dir), None);
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn a_worktree_needs_a_repository_with_a_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
        };
        // A plain folder, then a repository nothing was committed to.
        assert!(!worktree_available(root));
        git(&["init", "-q"]);
        assert!(!worktree_available(root));
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "first"]);
        assert!(worktree_available(root));
        // Branches packed into one file still count.
        git(&["pack-refs", "--all"]);
        assert!(worktree_available(root));
        git(&["checkout", "-q", "--detach"]);
        assert!(worktree_available(root));
    }

    #[test]
    fn worktree_dir_stays_flat_per_project() {
        let root = Path::new("/Users/dev/my-project");
        // Slashes nest flatly so `feature/x` never creates subdirectories.
        assert_eq!(
            worktree_dir(root, "feature/x"),
            worktrees_root().join("my-project").join("feature-x")
        );
        assert_eq!(
            worktree_dir(root, "main"),
            worktrees_root().join("my-project").join("main")
        );
    }

    #[test]
    fn linked_worktree_resolves_to_parent_checkout() {
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
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature/new-home\n").unwrap();
        assert_eq!(worktree_main_checkout(&linked), Some(main.clone()));
        assert!(belongs_to_project(&linked, &main));
        assert!(belongs_to_project(&main, &main));
        assert!(!belongs_to_project(&linked, temp.path()));
        // Windows' canonical spelling of the same folders.
        let verbatim = |path: &Path| PathBuf::from(format!(r"\\?\{}", path.display()));
        assert!(belongs_to_project(&main, &verbatim(&main)));
        assert!(belongs_to_project(&linked, &verbatim(&main)));
        assert!(!same_folder(&linked, &verbatim(&main)));
        assert_eq!(
            linked_worktree_branch(&linked).as_deref(),
            Some("feature/new-home")
        );
        let canonical_main = main.canonicalize().unwrap();
        assert_eq!(sidebar_project_root(&linked, &[main.clone()]), canonical_main);
        assert_eq!(
            sidebar_project_root(&linked, &[main, linked.clone()]),
            canonical_main
        );
    }

    #[test]
    fn prompt_creates_separate_workspace_title_and_branch() {
        assert_eq!(
            workspace_title_from_prompt("  Fix  auth\nflow  "),
            Some("Fix auth flow".into())
        );
        // The branch carries the configured prefix (git username
        // by default) instead of a hardcoded `workspace/`.
        let branch = workspace_branch_from_prompt("Fix auth flow!", "a1b2c3d4");
        assert!(
            branch.ends_with("/fix-auth-flow-a1b2c3d4"),
            "unexpected branch: {branch}"
        );
        let fallback = workspace_branch_from_prompt("✨", "a1b2c3d4");
        assert!(
            fallback.ends_with("/workspace-a1b2c3d4"),
            "unexpected branch: {fallback}"
        );
    }

    #[test]
    fn sanitize_worktree_dir_name_rejects_junk() {
        assert_eq!(sanitize_worktree_dir_name(""), "worktree");
        assert_eq!(sanitize_worktree_dir_name(".."), "worktree");
        assert_eq!(sanitize_worktree_dir_name("a//b"), "a-b");
    }

    #[test]
    fn parse_worktree_list_skips_detached_and_bare() {
        let output = "worktree /repo\nbranch refs/heads/main\n\nworktree /wt/feat\nbranch refs/heads/feat\n\nworktree /wt/det\ndetached\n\nworktree /repo\nbare\n";
        let parsed = parse_worktree_list(output);
        assert_eq!(
            parsed,
            vec![
                (PathBuf::from("/repo"), "main".to_string()),
                (PathBuf::from("/wt/feat"), "feat".to_string()),
            ]
        );
    }

    #[test]
    fn a_new_folder_shows_as_itself_until_the_background_pass_names_it() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("checkout-never-seen");
        std::fs::create_dir_all(&folder).unwrap();
        // Asking never touches the disk: the folder stands for itself and
        // waits for the background pass.
        assert_eq!(known_sidebar_project_root(&folder), folder);
        assert!(take_pending_project_roots().contains(&folder));
        // The pass names it; a change asks for a re-sync, the same answer doesn't.
        let named = temp.path().join("main-checkout");
        assert!(remember_project_roots(vec![(folder.clone(), named.clone())]));
        assert_eq!(known_sidebar_project_root(&folder), named);
        assert!(!remember_project_roots(vec![(folder.clone(), named.clone())]));
        assert!(!take_pending_project_roots().contains(&folder));
    }
}
