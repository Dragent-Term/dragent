//! Git worktree operations shared by Source Control and isolated agent runs.
//!
//! Callers must invoke these blocking operations from a background worker.
//! Paths are passed as individual process arguments and removals are accepted
//! only for worktrees Git itself reports for the repository, or whose `.git`
//! pointer file shows them to be one.

#[cfg(not(target_family = "wasm"))]
use std::io::Write;
use std::path::{Path, PathBuf};

use dragent_workspace_model::{
    ExecutionChangeSummary, ExecutionFileChange, ExecutionFileChangeKind, ExecutionIsolation,
    ExecutionIsolationKind,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWorktree {
    pub path: PathBuf,
    pub repository_root: PathBuf,
    pub base_head: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntegrationPathState {
    pub relative_path: PathBuf,
    pub before_kind: String,
    pub before_hash: String,
    pub before_mode: u32,
    pub after_kind: String,
    pub after_hash: String,
    pub after_mode: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentIntegrationPlan {
    pub primary_root: PathBuf,
    /// Fingerprint of the exact alternate-index snapshot encoded by `patch`.
    /// Review receipts are compared to this value, never to a later worktree
    /// read that could race plan construction.
    #[serde(default)]
    pub candidate_fingerprint: String,
    pub patch: Vec<u8>,
    pub paths: Vec<IntegrationPathState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationPlanState {
    Before,
    Applied,
    Ambiguous,
}

/// Capture the attributable delta owned by an isolated execution. This is
/// metadata for review and cloud synchronization; source text remains in Git.
#[cfg(not(target_family = "wasm"))]
pub fn capture_agent_change_summary(
    worktree: &Path,
    isolation: &ExecutionIsolation,
    captured_at_unix_ms: i64,
) -> Result<ExecutionChangeSummary, String> {
    if isolation.kind != ExecutionIsolationKind::GitWorktree {
        return Err("Only a Git worktree isolation can produce writable changes".into());
    }
    let base_revision = isolation
        .base_revision
        .as_deref()
        .ok_or_else(|| "The isolated execution has no base revision".to_string())?;
    let delta = snapshot_agent_delta(worktree, base_revision)?;
    let kinds = parse_name_status_z(&delta.names)?;
    let mut files = parse_numstat_z(&delta.stats, &kinds)?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let summary = ExecutionChangeSummary {
        isolation_id: isolation.id,
        base_revision: base_revision.to_string(),
        files,
        captured_at_unix_ms,
    };
    summary
        .validate(isolation)
        .map_err(|error| format!("Invalid isolated change summary: {error}"))?;
    Ok(summary)
}

#[cfg(target_family = "wasm")]
pub fn capture_agent_change_summary(
    _worktree: &Path,
    _isolation: &ExecutionIsolation,
    _captured_at_unix_ms: i64,
) -> Result<ExecutionChangeSummary, String> {
    Err("Git worktree changes are unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
fn parse_name_status_z(
    bytes: &[u8],
) -> Result<std::collections::HashMap<PathBuf, ExecutionFileChangeKind>, String> {
    let fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut result = std::collections::HashMap::new();
    let mut index = 0;
    while index < fields.len() {
        let status = std::str::from_utf8(fields[index])
            .map_err(|_| "Git returned non-UTF-8 change status".to_string())?;
        index += 1;
        let kind = match status.as_bytes().first().copied() {
            Some(b'A') => ExecutionFileChangeKind::Added,
            Some(b'D') => ExecutionFileChangeKind::Deleted,
            Some(b'R' | b'C') => ExecutionFileChangeKind::Renamed,
            _ => ExecutionFileChangeKind::Modified,
        };
        if matches!(status.as_bytes().first(), Some(b'R' | b'C')) {
            if index + 1 >= fields.len() {
                return Err("Git returned an incomplete rename record".into());
            }
            index += 1; // old path
        }
        let path = fields
            .get(index)
            .ok_or_else(|| "Git returned an incomplete change record".to_string())?;
        index += 1;
        let path = std::str::from_utf8(path)
            .map_err(|_| "Git returned a non-UTF-8 changed path".to_string())?;
        result.insert(PathBuf::from(path), kind);
    }
    Ok(result)
}

#[cfg(not(target_family = "wasm"))]
fn parse_numstat_z(
    bytes: &[u8],
    kinds: &std::collections::HashMap<PathBuf, ExecutionFileChangeKind>,
) -> Result<Vec<ExecutionFileChange>, String> {
    let mut files = Vec::new();
    for record in bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = std::str::from_utf8(record)
            .map_err(|_| "Git returned non-UTF-8 change statistics".to_string())?;
        let mut fields = record.splitn(3, '\t');
        let added = fields.next().unwrap_or_default();
        let deleted = fields.next().unwrap_or_default();
        let path = fields.next().unwrap_or_default();
        // Rename numstat records may encode an empty path followed by two NUL
        // paths. Name-status already gives the canonical destination; those
        // rare records are recovered below from the status map.
        if path.is_empty() {
            continue;
        }
        let relative_path = PathBuf::from(path);
        let binary = added == "-" || deleted == "-";
        files.push(ExecutionFileChange {
            kind: if binary {
                ExecutionFileChangeKind::Binary
            } else {
                kinds
                    .get(&relative_path)
                    .copied()
                    .unwrap_or(ExecutionFileChangeKind::Modified)
            },
            relative_path,
            added_lines: (!binary).then(|| added.parse::<u64>().unwrap_or(0)),
            deleted_lines: (!binary).then(|| deleted.parse::<u64>().unwrap_or(0)),
        });
    }
    for (path, kind) in kinds {
        if !files.iter().any(|file| &file.relative_path == path) {
            files.push(ExecutionFileChange {
                relative_path: path.clone(),
                kind: *kind,
                added_lines: None,
                deleted_lines: None,
            });
        }
    }
    Ok(files)
}

#[cfg(not(target_family = "wasm"))]
#[derive(PartialEq, Eq)]
struct SourceSnapshot {
    status: Vec<u8>,
    tracked_patch: Vec<u8>,
    untracked: Vec<(String, String)>,
}

pub fn agent_worktree_path(project_id: Uuid, attempt_id: Uuid) -> PathBuf {
    dragent_paths::home()
        .join("agent-worktrees")
        .join(project_id.simple().to_string())
        .join(attempt_id.simple().to_string())
}

/// Parse `git worktree list --porcelain`. Unknown future fields are ignored.
pub fn parse_worktree_list(output: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in output.lines() {
        if line.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(previous) = current.take() {
                entries.push(previous);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                head: String::new(),
                branch: None,
                bare: false,
                locked: false,
                prunable: false,
            });
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            if let Some(entry) = current.as_mut() {
                entry.head = head.to_string();
            }
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if let Some(entry) = current.as_mut() {
                entry.branch = Some(branch.to_string());
            }
        } else if line == "bare" {
            if let Some(entry) = current.as_mut() {
                entry.bare = true;
            }
        } else if line.starts_with("locked") {
            if let Some(entry) = current.as_mut() {
                entry.locked = true;
            }
        } else if line.starts_with("prunable") {
            if let Some(entry) = current.as_mut() {
                entry.prunable = true;
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

#[cfg(not(target_family = "wasm"))]
fn git_output(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    use crate::process_ext::NoWindow;

    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("Could not run Git: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("Git exited with {}", output.status)
        } else {
            stderr
        })
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn fetch_worktrees(cwd: &Path) -> Result<Vec<WorktreeEntry>, String> {
    let root = super::repo_root(cwd);
    let output = git_output(&root, &["worktree", "list", "--porcelain"])?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned non-UTF-8 worktree metadata".to_string())?;
    Ok(parse_worktree_list(&text))
}

#[cfg(target_family = "wasm")]
pub fn fetch_worktrees(_cwd: &Path) -> Result<Vec<WorktreeEntry>, String> {
    Err("Git worktrees are unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
pub fn add_worktree(
    cwd: &Path,
    new_path: &Path,
    branch: &str,
    create_branch: bool,
) -> Result<(), String> {
    use crate::process_ext::NoWindow;

    let root = super::git_toplevel(cwd)
        .ok_or_else(|| format!("{} is not inside a Git repository", cwd.display()))?;
    if !new_path.is_absolute() {
        return Err("A Git worktree path must be absolute".into());
    }
    if new_path.exists() {
        return Err(format!(
            "The worktree destination already exists: {}",
            new_path.display()
        ));
    }
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("A branch or revision is required for a Git worktree".into());
    }
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create worktree parent {}: {error}",
                parent.display()
            )
        })?;
    }
    let mut command = crate::git_cmd::GitCommand::new();
    command
        .no_window()
        .arg("-C")
        .arg(&root)
        .args(["worktree", "add"]);
    if create_branch {
        command.args(["-b", branch]);
    }
    command.arg(new_path);
    if !create_branch {
        command.arg(branch);
    }
    command_output(command, "Git worktree add")
}

#[cfg(target_family = "wasm")]
pub fn add_worktree(
    _cwd: &Path,
    _new_path: &Path,
    _branch: &str,
    _create_branch: bool,
) -> Result<(), String> {
    Err("Git worktrees are unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
fn command_output(command: crate::git_cmd::GitCommand, operation: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("Could not run Git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("{operation} exited with {}", output.status)
        } else {
            stderr
        })
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn remove_worktree(cwd: &Path, worktree_path: &Path, force: bool) -> Result<(), String> {
    use crate::process_ext::NoWindow;

    let root = super::git_toplevel(cwd)
        .ok_or_else(|| format!("{} is not inside a Git repository", cwd.display()))?;
    let requested = worktree_path
        .canonicalize()
        .unwrap_or_else(|_| worktree_path.to_path_buf());
    let root = root.canonicalize().unwrap_or(root);
    if requested == root {
        return Err("The repository's primary worktree cannot be removed".into());
    }
    let registered = fetch_worktrees(&root)?
        .into_iter()
        .any(|entry| entry.path.canonicalize().unwrap_or(entry.path) == requested);
    if !registered {
        return Err(format!(
            "Git does not report {} as a worktree for this repository",
            worktree_path.display()
        ));
    }
    let mut command = crate::git_cmd::GitCommand::new();
    command
        .no_window()
        .arg("-C")
        .arg(&root)
        .args(["worktree", "remove"]);
    if force {
        command.arg("--force");
    }
    command.arg(worktree_path);
    command_output(command, "Git worktree remove")
}

#[cfg(target_family = "wasm")]
pub fn remove_worktree(_cwd: &Path, _worktree_path: &Path, _force: bool) -> Result<(), String> {
    Err("Git worktrees are unavailable in the web client".into())
}

/// Delete a linked worktree whose terminals were just closed, discarding
/// whatever is in it.
///
/// Its folder is renamed out of the way, which takes the same moment however
/// much it holds, and git's record of it is dropped; the files are deleted
/// afterwards in the background. A checkout with build output is minutes of
/// deleting on Windows, and the caller used to wait for all of it.
///
/// Closing the terminals is asynchronous, and Windows refuses to rename or
/// delete a directory that is still a process's working directory (or holds
/// an open file), so that refusal is retried until those shells have exited.
#[cfg(not(target_family = "wasm"))]
pub fn force_remove_worktree(checkout: &Path, worktree: &Path) -> Result<(), String> {
    // Read before anything is deleted: this file is what proves the folder is
    // a worktree, and of which checkout.
    let Some((pointer, main)) = linked_worktree_pointer(worktree) else {
        return remove_unlinked_worktree(checkout, worktree);
    };
    let Some(aside) = move_aside(worktree) else {
        return delete_worktree_in_place(&main, worktree, &pointer);
    };
    // A checkout deleted or moved by hand leaves nothing to unregister from,
    // and its worktrees could otherwise never be removed.
    if main.is_dir() {
        if let Err(detail) = unregister_worktree(&main, worktree) {
            // Git refused (a locked worktree): put the folder back untouched.
            let _ = std::fs::rename(&aside, worktree);
            return Err(detail);
        }
    }
    let removed = worktree.to_path_buf();
    crate::bg::detach(move || {
        delete_moved_worktrees(&aside);
        let org = dragent_paths::organisation_dir();
        if let Err(error) = dragent_host::build_cache::remove_for_worktree(&org, &removed) {
            log::warn!("[worktree] build folder of {}: {error}", removed.display());
        }
    });
    Ok(())
}

/// The text of a linked worktree's `.git` pointer file and the main checkout
/// it names (`<main>/.git/worktrees/<id>`). `None` for a main checkout, a
/// plain folder, or a path that isn't on this computer.
#[cfg(not(target_family = "wasm"))]
fn linked_worktree_pointer(worktree: &Path) -> Option<(String, PathBuf)> {
    let pointer = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let git_dir = Path::new(pointer.trim().strip_prefix("gitdir: ")?);
    let git_dir = if git_dir.is_absolute() {
        git_dir.to_path_buf()
    } else {
        worktree.join(git_dir)
    };
    let worktrees = git_dir.parent()?;
    let common = worktrees.parent()?;
    let main = common.parent()?.to_path_buf();
    (worktrees.file_name()? == "worktrees" && common.file_name()? == ".git")
        .then_some((pointer, main))
}

/// Start of the name a worktree's folder carries while its files are deleted,
/// followed by the Unix time it was moved.
#[cfg(not(target_family = "wasm"))]
const ASIDE_PREFIX: &str = ".dragent-deleting-";

/// How long ago a folder was moved aside, from its name.
#[cfg(not(target_family = "wasm"))]
fn aside_age(name: &str) -> Option<std::time::Duration> {
    let moved: u64 = name.strip_prefix(ASIDE_PREFIX)?.split('-').next()?.parse().ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(now.saturating_sub(std::time::Duration::from_secs(moved)))
}

/// Rename `worktree` to a sibling name. `None` when it can't be moved.
#[cfg(not(target_family = "wasm"))]
fn move_aside(worktree: &Path) -> Option<PathBuf> {
    // Windows' ERROR_ACCESS_DENIED and ERROR_SHARING_VIOLATION.
    const FILE_OPEN_INSIDE: i32 = 5;
    const PROCESS_INSIDE: i32 = 32;
    worktree.file_name()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let aside = worktree.with_file_name(format!(
        "{ASIDE_PREFIX}{}-{}",
        now.as_secs(),
        now.subsec_nanos()
    ));
    let started = std::time::Instant::now();
    loop {
        let patience = match std::fs::rename(worktree, &aside) {
            Ok(()) => return Some(aside),
            Err(_) if !cfg!(windows) => return None,
            Err(error) => match error.raw_os_error() {
                // A process is still inside the folder: the shells being
                // closed. Nothing can remove it until they exit.
                Some(PROCESS_INSIDE) => std::time::Duration::from_secs(15),
                // A file in it is open, which rules out a rename for as long
                // as it stays open but not always deleting in place.
                Some(FILE_OPEN_INSIDE) => std::time::Duration::from_secs(2),
                _ => return None,
            },
        };
        if started.elapsed() >= patience {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(not(target_family = "wasm"))]
fn git_worktree_remove(repository: &Path, worktree: &Path) -> Result<std::process::Output, String> {
    use crate::process_ext::NoWindow;
    crate::git_cmd::GitCommand::new()
        .args(["worktree", "remove", "--force", "--"])
        .arg(worktree)
        .current_dir(repository)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .map_err(|error| format!("Could not remove worktree: {error}"))
}

/// Drop git's record of a worktree whose folder is already gone. Git no
/// longer knowing the worktree counts as done.
#[cfg(not(target_family = "wasm"))]
fn unregister_worktree(repository: &Path, worktree: &Path) -> Result<(), String> {
    let output = git_worktree_remove(repository, worktree)?;
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() || detail.to_ascii_lowercase().contains("is not a working tree") {
        Ok(())
    } else if detail.is_empty() {
        Err(format!("git worktree remove exited with {}", output.status))
    } else {
        Err(detail)
    }
}

/// Delete a folder that was moved aside, along with any an earlier run left
/// by quitting before it had finished.
#[cfg(not(target_family = "wasm"))]
fn delete_moved_worktrees(aside: &Path) {
    // A fresh one may belong to a removal that is about to put it back.
    const ABANDONED: std::time::Duration = std::time::Duration::from_secs(600);
    let mut doomed = vec![aside.to_path_buf()];
    if let Some(siblings) = aside.parent().and_then(|parent| std::fs::read_dir(parent).ok()) {
        doomed.extend(
            siblings
                .flatten()
                .filter(|entry| {
                    aside_age(&entry.file_name().to_string_lossy())
                        .is_some_and(|age| age > ABANDONED)
                })
                .map(|entry| entry.path()),
        );
    }
    for folder in doomed {
        for attempt in 1..=5 {
            match std::fs::remove_dir_all(&folder) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if attempt == 5 => {
                    log::warn!("[worktree] could not delete {}: {error}", folder.display());
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_secs(2)),
            }
        }
    }
}

/// Remove a linked worktree where it stands, for when a file open in it rules
/// out moving it aside. The caller waits for the files to be deleted.
#[cfg(not(target_family = "wasm"))]
fn delete_worktree_in_place(main: &Path, worktree: &Path, pointer: &str) -> Result<(), String> {
    if main.is_dir() {
        // Git refuses a locked worktree before touching it. Past that it
        // drops its record even when it could not delete every file.
        let output = git_worktree_remove(main, worktree)?;
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let unregistered = output.status.success()
            || detail.to_ascii_lowercase().contains("is not a working tree")
            || is_sharing_error(&detail);
        if !unregistered {
            return Err(if detail.is_empty() {
                format!("git worktree remove exited with {}", output.status)
            } else {
                detail
            });
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut delay = std::time::Duration::from_millis(200);
    loop {
        let error = match std::fs::remove_dir_all(worktree) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => error,
        };
        if backoff(deadline, &mut delay) {
            continue;
        }
        // What is left is no longer registered with git. Keeping the pointer
        // lets the next attempt still recognise it as this worktree.
        let _ = std::fs::write(worktree.join(".git"), pointer);
        return Err(format!(
            "A program is still using files in {}. Close it and try again. ({error})",
            worktree.display()
        ));
    }
}

/// `git worktree remove --force` for a folder without a worktree's `.git`
/// pointer file: what an earlier removal left, or a path on another machine.
/// Only git decides here, and a folder that still holds files is never
/// deleted.
#[cfg(not(target_family = "wasm"))]
fn remove_unlinked_worktree(checkout: &Path, worktree: &Path) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut delay = std::time::Duration::from_millis(200);
    loop {
        let output = git_worktree_remove(checkout, worktree)?;
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let removed = output.status.success();
        // Git deletes a worktree's files, then its directory. On Windows the
        // directory can survive (still some process's working directory),
        // leaving an empty folder git no longer tracks ("is not a working
        // tree" on the next attempt). Removing it completes the removal; a
        // folder that still holds files is never deleted here.
        if removed || detail.to_ascii_lowercase().contains("is not a working tree") {
            match remove_empty_dir(worktree) {
                Ok(()) => return Ok(()),
                Err(error) if !removed && error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                    return Err(detail);
                }
                Err(error) => {
                    if backoff(deadline, &mut delay) {
                        continue;
                    }
                    return Err(format!(
                        "Removed the worktree but could not delete its folder {}: {error}",
                        worktree.display()
                    ));
                }
            }
        }
        if is_sharing_error(&detail) && backoff(deadline, &mut delay) {
            continue;
        }
        return Err(if detail.is_empty() {
            format!("git worktree remove exited with {}", output.status)
        } else if is_sharing_error(&detail) {
            format!(
                "{detail}\nA program is still using files in {}. Close it and try again.",
                worktree.display()
            )
        } else {
            detail
        });
    }
}

/// Wait before retrying a removal Windows refused while a just-closed shell
/// exits. False once out of time, or on other platforms (no such refusals).
#[cfg(not(target_family = "wasm"))]
fn backoff(deadline: std::time::Instant, delay: &mut std::time::Duration) -> bool {
    if !cfg!(windows) || std::time::Instant::now() >= deadline {
        return false;
    }
    std::thread::sleep(*delay);
    *delay = (*delay * 2).min(std::time::Duration::from_secs(2));
    true
}

/// Remove `path` if it is an empty directory; already gone counts as removed.
#[cfg(not(target_family = "wasm"))]
fn remove_empty_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// A Windows file-in-use failure, as reported by git.
fn is_sharing_error(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    ["permission denied", "access is denied", "being used by another process", "resource busy", "directory not empty"]
        .iter()
        .any(|needle| detail.contains(needle))
}

/// Explicitly discard an isolated execution checkout after review. Worktrees
/// are otherwise retained across restarts so review evidence cannot vanish.
#[cfg(not(target_family = "wasm"))]
pub fn discard_agent_worktree(cwd: &Path, worktree_path: &Path) -> Result<(), String> {
    use crate::process_ext::NoWindow;

    let root = super::git_toplevel(cwd)
        .ok_or_else(|| format!("{} is not inside a Git repository", cwd.display()))?;
    let unlock = crate::git_cmd::GitCommand::new()
        .no_window()
        .arg("-C")
        .arg(&root)
        .args(["worktree", "unlock"])
        .arg(worktree_path)
        .output()
        .map_err(|error| format!("Could not unlock agent worktree: {error}"))?;
    if !unlock.status.success() {
        let stderr = String::from_utf8_lossy(&unlock.stderr);
        if !stderr.contains("not locked") {
            return Err(stderr.trim().to_string());
        }
    }
    remove_worktree(&root, worktree_path, true)
}

#[cfg(target_family = "wasm")]
pub fn discard_agent_worktree(_cwd: &Path, _worktree_path: &Path) -> Result<(), String> {
    Err("Git worktrees are unavailable in the web client".into())
}

/// Apply one reviewed worktree delta atomically at Git's patch-validation
/// boundary. A failed check leaves the primary checkout untouched and reports
/// a conflict; successful integration deliberately leaves normal unstaged
/// working-tree edits for the user to inspect and commit.
#[cfg(not(target_family = "wasm"))]
pub fn integrate_agent_worktree(
    primary_checkout: &Path,
    worktree_path: &Path,
    isolation: &ExecutionIsolation,
) -> Result<(), String> {
    let root = super::git_toplevel(primary_checkout).ok_or_else(|| {
        format!(
            "{} is not inside a Git repository",
            primary_checkout.display()
        )
    })?;
    let registered = fetch_worktrees(&root)?
        .into_iter()
        .any(|entry| entry.path.canonicalize().ok() == worktree_path.canonicalize().ok());
    if !registered || isolation.kind != ExecutionIsolationKind::GitWorktree {
        return Err("The reviewed execution is not a registered isolated Git worktree".into());
    }
    let base = isolation
        .base_revision
        .as_deref()
        .ok_or_else(|| "The reviewed execution has no immutable base revision".to_string())?;
    let patch = snapshot_agent_delta(worktree_path, base)?.patch;
    if patch.is_empty() {
        return Ok(());
    }
    apply_patch_input(&root, &patch, true)?;
    apply_patch_input(&root, &patch, false)
}

/// Prepare the durable, path-scoped proof needed to reconcile a crash between
/// applying a reviewed Git delta and committing its canonical completion.
#[cfg(not(target_family = "wasm"))]
pub fn prepare_agent_integration_plan(
    primary_checkout: &Path,
    worktree_path: &Path,
    isolation: &ExecutionIsolation,
) -> Result<AgentIntegrationPlan, String> {
    let root = super::git_toplevel(primary_checkout).ok_or_else(|| {
        format!(
            "{} is not inside a Git repository",
            primary_checkout.display()
        )
    })?;
    let root = root.canonicalize().unwrap_or(root);
    let registered = fetch_worktrees(&root)?
        .into_iter()
        .any(|entry| entry.path.canonicalize().ok() == worktree_path.canonicalize().ok());
    if !registered || isolation.kind != ExecutionIsolationKind::GitWorktree {
        return Err("The reviewed execution is not a registered isolated Git worktree".into());
    }
    let base = isolation
        .base_revision
        .as_deref()
        .ok_or_else(|| "The reviewed execution has no immutable base revision".to_string())?;
    let delta = snapshot_agent_delta(worktree_path, base)?;
    if delta.patch.is_empty() {
        return Err("The reviewed execution has no Git delta to integrate".into());
    }
    let mut paths = delta
        .paths
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|raw| {
            let relative = std::str::from_utf8(raw)
                .map(PathBuf::from)
                .map_err(|_| "Git returned a non-UTF-8 integration path".to_string())?;
            validate_integration_path(&relative)?;
            let before = path_identity(&root.join(&relative))?;
            let after = delta
                .after_identities
                .get(&relative)
                .cloned()
                .unwrap_or_else(|| ("missing".into(), String::new(), 0));
            Ok(IntegrationPathState {
                relative_path: relative,
                before_kind: before.0,
                before_hash: before.1,
                before_mode: before.2,
                after_kind: after.0,
                after_hash: after.1,
                after_mode: after.2,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    paths.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    paths.dedup_by(|left, right| left.relative_path == right.relative_path);
    let candidate_fingerprint = fingerprint_agent_delta(base, &delta);
    let plan = AgentIntegrationPlan {
        primary_root: root,
        candidate_fingerprint,
        patch: delta.patch,
        paths,
    };
    if classify_agent_integration(&plan)? != IntegrationPlanState::Before {
        return Err("The primary checkout changed while integration was prepared".into());
    }
    Ok(plan)
}

#[cfg(target_family = "wasm")]
pub fn prepare_agent_integration_plan(
    _primary_checkout: &Path,
    _worktree_path: &Path,
    _isolation: &ExecutionIsolation,
) -> Result<AgentIntegrationPlan, String> {
    Err("Git worktree integration is unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
pub fn classify_agent_integration(
    plan: &AgentIntegrationPlan,
) -> Result<IntegrationPlanState, String> {
    let root = plan.primary_root.canonicalize().map_err(|error| {
        format!(
            "Could not resolve integration checkout {}: {error}",
            plan.primary_root.display()
        )
    })?;
    if root != plan.primary_root {
        return Err("The integration checkout identity changed".into());
    }
    let mut before = true;
    let mut after = true;
    for path in &plan.paths {
        validate_integration_path(&path.relative_path)?;
        let current = path_identity(&root.join(&path.relative_path))?;
        before &= current
            == (
                path.before_kind.clone(),
                path.before_hash.clone(),
                path.before_mode,
            );
        after &= current
            == (
                path.after_kind.clone(),
                path.after_hash.clone(),
                path.after_mode,
            );
    }
    Ok(if before {
        IntegrationPlanState::Before
    } else if after {
        IntegrationPlanState::Applied
    } else {
        IntegrationPlanState::Ambiguous
    })
}

#[cfg(target_family = "wasm")]
pub fn classify_agent_integration(
    _plan: &AgentIntegrationPlan,
) -> Result<IntegrationPlanState, String> {
    Err("Git worktree integration is unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
pub fn apply_agent_integration_plan(plan: &AgentIntegrationPlan) -> Result<(), String> {
    match classify_agent_integration(plan)? {
        IntegrationPlanState::Applied => return Ok(()),
        IntegrationPlanState::Ambiguous => {
            return Err(
                "Integration target paths no longer match either the prepared or applied state"
                    .into(),
            )
        }
        IntegrationPlanState::Before => {}
    }
    apply_patch_input(&plan.primary_root, &plan.patch, true)?;
    apply_patch_input(&plan.primary_root, &plan.patch, false)?;
    if classify_agent_integration(plan)? != IntegrationPlanState::Applied {
        return Err(
            "Git reported success but integrated paths do not match the reviewed candidate".into(),
        );
    }
    Ok(())
}

#[cfg(target_family = "wasm")]
pub fn apply_agent_integration_plan(_plan: &AgentIntegrationPlan) -> Result<(), String> {
    Err("Git worktree integration is unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
fn validate_integration_path(path: &Path) -> Result<(), String> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("Unsafe integration path: {}", path.display()));
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn path_identity(path: &Path) -> Result<(String, String, u32), String> {
    use sha2::{Digest, Sha256};
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(("missing".into(), String::new(), 0));
        }
        Err(error) => return Err(format!("Could not inspect {}: {error}", path.display())),
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111
    };
    #[cfg(not(unix))]
    let mode = 0;
    let (kind, bytes) = if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .map_err(|error| format!("Could not read symlink {}: {error}", path.display()))?;
        ("symlink", target.to_string_lossy().as_bytes().to_vec())
    } else if metadata.is_file() {
        (
            "file",
            std::fs::read(path)
                .map_err(|error| format!("Could not read {}: {error}", path.display()))?,
        )
    } else {
        return Err(format!(
            "Unsupported integration target type: {}",
            path.display()
        ));
    };
    Ok((kind.into(), format!("{:x}", Sha256::digest(bytes)), mode))
}

/// Build the complete worktree delta through an alternate index. Ordinary
/// `git diff <base>` omits untracked files; the temporary index lets Git
/// classify and encode every current filesystem entry without changing the
/// agent checkout's actual staging area.
#[cfg(not(target_family = "wasm"))]
struct AgentDelta {
    names: Vec<u8>,
    stats: Vec<u8>,
    patch: Vec<u8>,
    paths: Vec<u8>,
    after_identities: std::collections::HashMap<PathBuf, (String, String, u32)>,
}

#[cfg(not(target_family = "wasm"))]
fn fingerprint_agent_delta(base: &str, delta: &AgentDelta) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update((base.len() as u64).to_le_bytes());
    hash.update(base.as_bytes());
    hash.update((delta.patch.len() as u64).to_le_bytes());
    hash.update(&delta.patch);
    format!("{:x}", hash.finalize())
}

#[cfg(not(target_family = "wasm"))]
fn snapshot_agent_delta(worktree: &Path, base: &str) -> Result<AgentDelta, String> {
    use crate::process_ext::NoWindow;

    let temporary_index =
        std::env::temp_dir().join(format!("dragent-agent-delta-{}.index", Uuid::new_v4()));
    let run = (|| {
        let command = |args: &[&str]| -> Result<std::process::Output, String> {
            let mut command = crate::git_cmd::GitCommand::new();
            command
                .no_window()
                .arg("-C")
                .arg(worktree)
                .env("GIT_INDEX_FILE", &temporary_index)
                .args(args);
            let output = command
                .output()
                .map_err(|error| format!("Could not inspect isolated agent changes: {error}"))?;
            if output.status.success() {
                Ok(output)
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
            }
        };
        command(&["read-tree", base])?;
        command(&["add", "-A", "--"])?;
        let names = command(&[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            "--find-renames",
            base,
            "--",
        ])?
        .stdout;
        let stats = command(&[
            "diff",
            "--cached",
            "--numstat",
            "-z",
            "--find-renames",
            base,
            "--",
        ])?
        .stdout;
        let patch = command(&["diff", "--cached", "--binary", base, "--"])?.stdout;
        let paths = command(&[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            base,
            "--",
        ])?
        .stdout;
        let stage = command(&["ls-files", "--stage", "-z", "--"])?;
        let changed_paths = paths
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .collect::<std::collections::HashSet<_>>();
        let mut after_identities = std::collections::HashMap::new();
        for record in stage
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
                continue;
            };
            if !changed_paths.contains(&record[tab + 1..]) {
                continue;
            }
            let metadata = std::str::from_utf8(&record[..tab])
                .map_err(|_| "Git returned non-UTF-8 index metadata".to_string())?;
            let relative = std::str::from_utf8(&record[tab + 1..])
                .map(PathBuf::from)
                .map_err(|_| "Git returned a non-UTF-8 integration path".to_string())?;
            let mut fields = metadata.split_whitespace();
            let mode = fields.next().unwrap_or_default();
            let object = fields.next().unwrap_or_default();
            let bytes = command(&["cat-file", "blob", object])?.stdout;
            use sha2::{Digest, Sha256};
            let kind = if mode == "120000" { "symlink" } else { "file" };
            let executable = if mode == "100755" { 0o111 } else { 0 };
            after_identities.insert(
                relative,
                (
                    kind.into(),
                    format!("{:x}", Sha256::digest(bytes)),
                    executable,
                ),
            );
        }
        Ok(AgentDelta {
            names,
            stats,
            patch,
            paths,
            after_identities,
        })
    })();
    let _ = std::fs::remove_file(&temporary_index);
    let _ = std::fs::remove_file(temporary_index.with_extension("index.lock"));
    run
}

#[cfg(target_family = "wasm")]
pub fn integrate_agent_worktree(
    _primary_checkout: &Path,
    _worktree_path: &Path,
    _isolation: &ExecutionIsolation,
) -> Result<(), String> {
    Err("Git worktree integration is unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
fn apply_patch_input(cwd: &Path, patch: &[u8], check_only: bool) -> Result<(), String> {
    use crate::process_ext::NoWindow;
    use std::process::Stdio;

    let mut command = crate::git_cmd::GitCommand::new();
    command
        .no_window()
        .arg("-C")
        .arg(cwd)
        .args(["apply", "--binary", "--whitespace=nowarn"]);
    if check_only {
        command.arg("--check");
    }
    let output = command
        .arg("-")
        .output_with_stdin(patch)
        .map_err(|error| format!("Could not run Git patch integration: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let details = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if check_only {
            format!("Agent changes conflict with the current checkout: {details}")
        } else {
            format!("Could not integrate reviewed agent changes: {details}")
        })
    }
}

/// Prepare a detached checkout for one agent attempt. Tracked and untracked
/// filesystem changes are copied into a synthetic detached baseline commit,
/// leaving the user's index and working tree untouched.
#[cfg(not(target_family = "wasm"))]
pub fn prepare_agent_worktree(
    cwd: &Path,
    project_id: Uuid,
    attempt_id: Uuid,
) -> Result<AgentWorktree, String> {
    prepare_agent_worktree_at(cwd, &agent_worktree_path(project_id, attempt_id))
}

#[cfg(target_family = "wasm")]
pub fn prepare_agent_worktree(
    _cwd: &Path,
    _project_id: Uuid,
    _attempt_id: Uuid,
) -> Result<AgentWorktree, String> {
    Err("Git worktrees are unavailable in the web client".into())
}

#[cfg(not(target_family = "wasm"))]
fn prepare_agent_worktree_at(cwd: &Path, destination: &Path) -> Result<AgentWorktree, String> {
    use crate::process_ext::NoWindow;

    let repository_root = super::git_toplevel(cwd)
        .ok_or_else(|| format!("{} is not inside a Git repository", cwd.display()))?;
    let repository_root = repository_root.canonicalize().unwrap_or(repository_root);
    let snapshot_before = capture_source_snapshot(&repository_root)?;
    let head = git_output(&repository_root, &["rev-parse", "HEAD"])?;
    let source_head = String::from_utf8(head.stdout)
        .map_err(|_| "Git returned a non-UTF-8 HEAD revision".to_string())?
        .trim()
        .to_string();

    if destination.exists() {
        let registered = fetch_worktrees(&repository_root)?
            .into_iter()
            .any(|entry| entry.path.canonicalize().ok() == destination.canonicalize().ok());
        if !registered {
            return Err(format!(
                "The agent checkout path already exists but is not the expected Git worktree: {}",
                destination.display()
            ));
        }
        let baseline = git_output(destination, &["rev-parse", "HEAD"])?;
        let base_head = String::from_utf8(baseline.stdout)
            .map_err(|_| "Git returned a non-UTF-8 agent baseline".to_string())?
            .trim()
            .to_string();
        adopt_agent_checkout_with_host(&repository_root, destination);
        return Ok(AgentWorktree {
            path: destination.to_path_buf(),
            repository_root,
            base_head,
        });
    }

    add_worktree(&repository_root, destination, &source_head, false)?;
    if let Err(error) = seed_agent_baseline(&repository_root, destination, &snapshot_before) {
        cleanup_failed_worktree(&repository_root, destination);
        return Err(error);
    }
    let snapshot_after = match capture_source_snapshot(&repository_root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            cleanup_failed_worktree(&repository_root, destination);
            return Err(error);
        }
    };
    if snapshot_after != snapshot_before {
        cleanup_failed_worktree(&repository_root, destination);
        return Err(
            "The primary checkout changed while Dragent prepared agent isolation; retry from the updated state."
                .into(),
        );
    }
    let lock = crate::git_cmd::GitCommand::new()
        .no_window()
        .arg("-C")
        .arg(&repository_root)
        .args(["worktree", "lock", "--reason", "Dragent agent execution"])
        .arg(destination)
        .output()
        .map_err(|error| format!("Could not lock agent worktree: {error}"))?;
    if !lock.status.success() {
        let message = String::from_utf8_lossy(&lock.stderr).trim().to_string();
        cleanup_failed_worktree(&repository_root, destination);
        return Err(if message.is_empty() {
            "Could not lock the isolated agent worktree".into()
        } else {
            message
        });
    }
    let baseline = git_output(destination, &["rev-parse", "HEAD"])?;
    let base_head = String::from_utf8(baseline.stdout)
        .map_err(|_| "Git returned a non-UTF-8 agent baseline".to_string())?
        .trim()
        .to_string();
    adopt_agent_checkout_with_host(&repository_root, destination);
    Ok(AgentWorktree {
        path: destination.to_path_buf(),
        repository_root,
        base_head,
    })
}

#[cfg(not(target_family = "wasm"))]
fn adopt_agent_checkout_with_host(repository_root: &Path, destination: &Path) {
    // Unit tests never start or touch a live host (the test binary would be
    // spawned as one).
    if cfg!(test) {
        return;
    }
    let result = (|| -> Result<(), String> {
        let client = dragent_host::HostClient::ensure_host()?;
        // Stable per repository root: re-creating the same root reuses the
        // workspace, so repeated agent runs converge on one ledger entry
        // (adopt itself is idempotent per checkout path).
        let repo = repository_root.to_string_lossy().into_owned();
        let workspace_id = client.create_workspace(&repo)?;
        client.adopt_checkout(
            &workspace_id,
            &repo,
            &destination.to_string_lossy(),
            "agent",
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        log::warn!(
            "[agent-worktree] host adopt failed ({error}); checkout works locally, host reconcile will not cover it"
        );
    }
}

#[cfg(not(target_family = "wasm"))]
fn seed_agent_baseline(
    repository_root: &Path,
    destination: &Path,
    source: &SourceSnapshot,
) -> Result<(), String> {
    use crate::process_ext::NoWindow;
    use std::process::Stdio;

    if source.status.is_empty() {
        return Ok(());
    }
    if !source.tracked_patch.is_empty() {
        let output = crate::git_cmd::GitCommand::new()
            .no_window()
            .arg("-C")
            .arg(destination)
            .args(["apply", "--binary", "--whitespace=nowarn", "-"])
            .output_with_stdin(&source.tracked_patch)
            .map_err(|error| format!("Could not apply the tracked baseline: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
    }

    for (relative, _) in &source.untracked {
        let relative = Path::new(relative);
        let source = repository_root.join(relative);
        let target = destination.join(relative);
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|error| format!("Could not inspect {}: {error}", source.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "Unsupported untracked entry in agent baseline: {}",
                relative.display()
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Could not create baseline directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        std::fs::copy(&source, &target).map_err(|error| {
            format!(
                "Could not copy untracked baseline file {}: {error}",
                relative.display()
            )
        })?;
    }

    let mut add = crate::git_cmd::GitCommand::new();
    add.no_window()
        .arg("-C")
        .arg(destination)
        .args(["add", "-A"]);
    command_output(add, "Git baseline staging")?;
    let mut commit = crate::git_cmd::GitCommand::new();
    commit.no_window().arg("-C").arg(destination).args([
        "-c",
        "user.name=Dragent Workspace",
        "-c",
        "user.email=workspace@dragent.local",
        "commit",
        "-qm",
        "Dragent isolated execution baseline",
    ]);
    command_output(commit, "Git baseline commit")
}

#[cfg(not(target_family = "wasm"))]
fn capture_source_snapshot(repository_root: &Path) -> Result<SourceSnapshot, String> {
    let status = git_output(
        repository_root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?
    .stdout;
    let tracked_patch = git_output(repository_root, &["diff", "--binary", "HEAD", "--"])?.stdout;
    let paths = git_output(
        repository_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?
    .stdout;
    let mut untracked = Vec::new();
    for raw_path in paths
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(raw_path)
            .map_err(|_| "An untracked path is not valid UTF-8; isolation cannot copy it safely")?;
        let hash = git_output(repository_root, &["hash-object", "--", path])?;
        let hash = String::from_utf8(hash.stdout)
            .map_err(|_| "Git returned a non-UTF-8 object hash".to_string())?
            .trim()
            .to_string();
        untracked.push((path.to_string(), hash));
    }
    Ok(SourceSnapshot {
        status,
        tracked_patch,
        untracked,
    })
}

/// Reuse the isolation snapshot to bind a test result to the actual candidate
/// bytes (including untracked files), not just a filename/count summary.
#[cfg(not(target_family = "wasm"))]
pub fn verification_fingerprint(root: &Path) -> Result<String, String> {
    let head = git_output(root, &["rev-parse", "HEAD"])?;
    let base = String::from_utf8(head.stdout)
        .map_err(|_| "Git returned a non-UTF-8 HEAD revision".to_string())?;
    let base = base.trim();
    let delta = snapshot_agent_delta(root, base)?;
    Ok(fingerprint_agent_delta(base, &delta))
}

#[cfg(not(target_family = "wasm"))]
fn cleanup_failed_worktree(repository_root: &Path, destination: &Path) {
    use crate::process_ext::NoWindow;

    let _ = crate::git_cmd::GitCommand::new()
        .no_window()
        .arg("-C")
        .arg(repository_root)
        .args(["worktree", "remove", "--force"])
        .arg(destination)
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(&repo).args(args).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["init", "-q"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", worktree.to_str().unwrap()]);
        (temp, repo, worktree)
    }

    #[test]
    fn removing_a_worktree_leaves_no_folder_behind() {
        let (_temp, repo, worktree) = repo_with_worktree();
        force_remove_worktree(&repo, &worktree).unwrap();
        assert!(!worktree.exists());
    }

    fn folders_being_deleted(parent: &Path) -> usize {
        std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(ASIDE_PREFIX))
            .count()
    }

    #[test]
    fn removal_unregisters_the_worktree_and_deletes_its_files_afterwards() {
        let (temp, repo, worktree) = repo_with_worktree();
        std::fs::write(worktree.join("untracked.txt"), "discarded").unwrap();
        force_remove_worktree(&repo, &worktree).unwrap();
        assert!(!worktree.exists());
        assert_eq!(fetch_worktrees(&repo).unwrap().len(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while folders_being_deleted(temp.path()) > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(folders_being_deleted(temp.path()), 0);
    }

    #[test]
    fn a_worktree_whose_repository_is_gone_is_removed() {
        let (_temp, repo, worktree) = repo_with_worktree();
        std::fs::remove_dir_all(&repo).unwrap();
        force_remove_worktree(&repo, &worktree).unwrap();
        assert!(!worktree.exists());
    }

    #[test]
    fn a_worktree_git_refuses_to_remove_is_left_as_it_was() {
        let (temp, repo, worktree) = repo_with_worktree();
        let lock = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["worktree", "lock"])
            .arg(&worktree)
            .output()
            .unwrap();
        assert!(lock.status.success());
        assert!(force_remove_worktree(&repo, &worktree).is_err());
        assert!(worktree.join(".git").is_file());
        assert_eq!(folders_being_deleted(temp.path()), 0);
        assert_eq!(fetch_worktrees(&repo).unwrap().len(), 2);
    }

    #[test]
    fn only_folders_moved_aside_long_ago_count_as_abandoned() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let age = |moved: u64| aside_age(&format!("{ASIDE_PREFIX}{moved}-7")).unwrap();
        assert!(age(now) < std::time::Duration::from_secs(60));
        assert!(age(now - 3600) >= std::time::Duration::from_secs(3600));
        assert!(aside_age("feature-branch").is_none());
        assert!(aside_age(".dragent-deleting-soon").is_none());
    }

    #[test]
    fn an_empty_folder_left_by_an_earlier_removal_is_removed() {
        let (_temp, repo, worktree) = repo_with_worktree();
        force_remove_worktree(&repo, &worktree).unwrap();
        // What Windows leaves when the directory was still in use: empty and
        // no longer registered with git.
        std::fs::create_dir(&worktree).unwrap();
        force_remove_worktree(&repo, &worktree).unwrap();
        assert!(!worktree.exists());
    }

    #[test]
    fn an_unregistered_folder_with_files_is_never_deleted() {
        let (_temp, repo, worktree) = repo_with_worktree();
        force_remove_worktree(&repo, &worktree).unwrap();
        std::fs::create_dir(&worktree).unwrap();
        std::fs::write(worktree.join("keep.txt"), "user data").unwrap();
        assert!(force_remove_worktree(&repo, &worktree).is_err());
        assert!(worktree.join("keep.txt").is_file());
    }

    #[test]
    fn file_in_use_failures_are_retried_and_others_are_not() {
        assert!(is_sharing_error(
            "error: failed to delete 'C:/Users/x/.dragent/worktrees/app/hy': Permission denied"
        ));
        assert!(is_sharing_error(
            "The process cannot access the file because it is being used by another process."
        ));
        assert!(!is_sharing_error("fatal: 'hy' is not a working tree"));
        assert!(!is_sharing_error("fatal: '/repo/hy' contains modified or untracked files"));
    }

    #[test]
    fn parse_two_worktrees() {
        let output = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo-feature\nHEAD def\ndetached\nlocked dragent\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch.as_deref(), Some("refs/heads/main"));
        assert!(worktrees[1].locked);
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn adds_lists_and_safely_removes_a_detached_worktree() {
        use crate::process_ext::NoWindow;

        let repository = tempfile::tempdir().unwrap();
        let checkout_parent = tempfile::tempdir().unwrap();
        let checkout = checkout_parent.path().join("agent-checkout");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .no_window()
                .arg("-C")
                .arg(repository.path())
                .args(args)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.email", "tests@dragent.dev"])
            .status
            .success());
        assert!(git(&["config", "user.name", "Dragent Tests"])
            .status
            .success());
        std::fs::write(repository.path().join("README.md"), "base\n").unwrap();
        assert!(git(&["add", "README.md"]).status.success());
        assert!(git(&["commit", "-qm", "base"]).status.success());

        add_worktree(repository.path(), &checkout, "HEAD", false).unwrap();
        assert!(checkout.join("README.md").is_file());
        assert!(fetch_worktrees(repository.path())
            .unwrap()
            .iter()
            .any(|entry| entry.path.canonicalize().ok() == checkout.canonicalize().ok()));
        assert!(remove_worktree(repository.path(), repository.path(), false).is_err());
        remove_worktree(repository.path(), &checkout, false).unwrap();
        assert!(!checkout.exists());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn agent_worktree_is_detached_reusable_and_does_not_mutate_primary_checkout() {
        use crate::process_ext::NoWindow;

        let repository = tempfile::tempdir().unwrap();
        let checkout_parent = tempfile::tempdir().unwrap();
        let checkout = checkout_parent.path().join("isolated-agent");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .no_window()
                .arg("-C")
                .arg(repository.path())
                .args(args)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.email", "tests@dragent.dev"])
            .status
            .success());
        assert!(git(&["config", "user.name", "Dragent Tests"])
            .status
            .success());
        std::fs::write(repository.path().join("main.rs"), "fn main() {}\n").unwrap();
        assert!(git(&["add", "main.rs"]).status.success());
        assert!(git(&["commit", "-qm", "base"]).status.success());
        std::fs::write(repository.path().join("main.rs"), "fn user_draft() {}\n").unwrap();
        std::fs::create_dir(repository.path().join("notes")).unwrap();
        std::fs::write(
            repository.path().join("notes/context.txt"),
            "draft context\n",
        )
        .unwrap();

        let prepared = prepare_agent_worktree_at(repository.path(), &checkout).unwrap();
        assert_eq!(prepared.path, checkout);
        assert_eq!(
            std::fs::read_to_string(checkout.join("main.rs")).unwrap(),
            "fn user_draft() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("notes/context.txt")).unwrap(),
            "draft context\n"
        );
        std::fs::write(checkout.join("main.rs"), "fn changed_by_agent() {}\n").unwrap();
        std::fs::write(
            checkout.join("created_by_agent.rs"),
            "pub const READY: bool = true;\n",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(repository.path().join("main.rs")).unwrap(),
            "fn user_draft() {}\n"
        );
        let attempt_id = dragent_workspace_model::ExecutionAttemptId::new();
        let isolation = ExecutionIsolation::git_worktree(attempt_id, prepared.base_head.clone());
        let changes = capture_agent_change_summary(&checkout, &isolation, 10).unwrap();
        assert_eq!(changes.isolation_id, isolation.id);
        assert_eq!(changes.files.len(), 2);
        let modified = changes
            .files
            .iter()
            .find(|file| file.relative_path == PathBuf::from("main.rs"))
            .unwrap();
        assert_eq!(modified.kind, ExecutionFileChangeKind::Modified);
        assert_eq!(modified.added_lines, Some(1));
        assert_eq!(modified.deleted_lines, Some(1));
        let added = changes
            .files
            .iter()
            .find(|file| file.relative_path == PathBuf::from("created_by_agent.rs"))
            .unwrap();
        assert_eq!(added.kind, ExecutionFileChangeKind::Added);
        assert_eq!(added.added_lines, Some(1));
        assert_eq!(added.deleted_lines, Some(0));
        std::fs::write(
            repository.path().join("main.rs"),
            "fn competing_user_edit() {}\n",
        )
        .unwrap();
        assert!(integrate_agent_worktree(repository.path(), &checkout, &isolation).is_err());
        assert_eq!(
            std::fs::read_to_string(repository.path().join("main.rs")).unwrap(),
            "fn competing_user_edit() {}\n"
        );
        assert!(!repository.path().join("created_by_agent.rs").exists());
        std::fs::write(repository.path().join("main.rs"), "fn user_draft() {}\n").unwrap();
        integrate_agent_worktree(repository.path(), &checkout, &isolation).unwrap();
        assert_eq!(
            std::fs::read_to_string(repository.path().join("main.rs")).unwrap(),
            "fn changed_by_agent() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("created_by_agent.rs")).unwrap(),
            "pub const READY: bool = true;\n"
        );
        let reused = prepare_agent_worktree_at(repository.path(), &checkout).unwrap();
        assert_eq!(reused.base_head, prepared.base_head);
        discard_agent_worktree(repository.path(), &checkout).unwrap();
        assert!(!checkout.exists());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn durable_integration_plan_recovers_once_and_preserves_unrelated_edits() {
        use crate::process_ext::NoWindow;

        let repository = tempfile::tempdir().unwrap();
        let checkout_parent = tempfile::tempdir().unwrap();
        let checkout = checkout_parent.path().join("recovery-checkout");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .no_window()
                .arg("-C")
                .arg(repository.path())
                .args(args)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.email", "tests@dragent.dev"])
            .status
            .success());
        assert!(git(&["config", "user.name", "Dragent Tests"])
            .status
            .success());
        std::fs::write(repository.path().join("owned.txt"), "before\n").unwrap();
        std::fs::write(repository.path().join("unrelated.txt"), "original\n").unwrap();
        assert!(git(&["add", "."]).status.success());
        assert!(git(&["commit", "-qm", "base"]).status.success());

        let prepared = prepare_agent_worktree_at(repository.path(), &checkout).unwrap();
        std::fs::write(checkout.join("owned.txt"), "after\n").unwrap();
        std::fs::write(checkout.join("created.txt"), "created\n").unwrap();
        let attempt_id = dragent_workspace_model::ExecutionAttemptId::new();
        let isolation = ExecutionIsolation::git_worktree(attempt_id, prepared.base_head);
        let plan =
            prepare_agent_integration_plan(repository.path(), &checkout, &isolation).unwrap();
        assert_eq!(
            plan.candidate_fingerprint,
            verification_fingerprint(&checkout).unwrap(),
            "the durable patch and reviewed fingerprint must describe one snapshot"
        );
        assert_eq!(
            classify_agent_integration(&plan).unwrap(),
            IntegrationPlanState::Before
        );

        let encoded = serde_json::to_string(&plan).unwrap();
        let recovered: AgentIntegrationPlan = serde_json::from_str(&encoded).unwrap();
        std::fs::write(checkout.join("owned.txt"), "unreviewed later bytes\n").unwrap();
        assert_ne!(
            recovered.candidate_fingerprint,
            verification_fingerprint(&checkout).unwrap()
        );
        std::fs::write(repository.path().join("unrelated.txt"), "user edit\n").unwrap();
        apply_agent_integration_plan(&recovered).unwrap();
        assert_eq!(
            classify_agent_integration(&recovered).unwrap(),
            IntegrationPlanState::Applied
        );
        apply_agent_integration_plan(&recovered).unwrap();
        assert_eq!(
            std::fs::read_to_string(repository.path().join("owned.txt")).unwrap(),
            "after\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("created.txt")).unwrap(),
            "created\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.path().join("unrelated.txt")).unwrap(),
            "user edit\n"
        );

        std::fs::write(repository.path().join("owned.txt"), "foreign edit\n").unwrap();
        assert_eq!(
            classify_agent_integration(&recovered).unwrap(),
            IntegrationPlanState::Ambiguous
        );
        assert!(apply_agent_integration_plan(&recovered).is_err());
        assert_eq!(
            std::fs::read_to_string(repository.path().join("owned.txt")).unwrap(),
            "foreign edit\n"
        );
        discard_agent_worktree(repository.path(), &checkout).unwrap();
    }
}
