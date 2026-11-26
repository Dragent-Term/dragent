//! Shared content-hash embedding cache identity for the worktrees of one repository.
//!
//! Only the shared content-hash embedding cache is keyed by the repo (via
//! `git --git-common-dir`, identical for every worktree of that repo): a
//! second worktree reuses every embedding whose content is unchanged and only
//! embeds its own differences. Each worktree keeps its own on-disk store,
//! snapshot and in-memory watcher state, keyed by the worktree path.

use std::path::{Path, PathBuf};

use super::index_batch::normalize_root;

/// Index key for `path`: the canonicalized git common dir when `path` is
/// inside a git repository (identical for every worktree of that repo),
/// otherwise the normalized path itself (one index per non-git folder).
///
/// Idempotent: feeding a key back in returns it unchanged (a common dir is
/// itself a git dir), so parked keys re-register as themselves on enable.
pub fn repo_index_key(path: &Path) -> PathBuf {
    let normalized = normalize_root(path);
    if is_git_dir(&normalized) {
        return normalized;
    }
    git_common_dir(&normalized)
        .map(|dir| normalize_root(&dir))
        .unwrap_or(normalized)
}

/// A git directory: a main checkout's `.git`, or a linked worktree's gitdir
/// (identified by its `commondir` pointer). Never matches a worktree root.
fn is_git_dir(path: &Path) -> bool {
    path.join("HEAD").is_file()
        && (path.join("objects").is_dir() || path.join("commondir").is_file())
}

/// `git rev-parse --git-common-dir` under `path`. Relative output (`.git`
/// for a main checkout, a sibling dir for a linked worktree) resolves against
/// `path`; absolute output is used as-is. Returns `None` when git is missing,
/// `path` is not a repository, or the command fails — the caller then falls
/// back to a per-folder index.
fn git_common_dir(path: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let dir = raw.trim();
    if dir.is_empty() {
        return None;
    }
    let candidate = Path::new(dir);
    Some(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        path.join(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git should run");
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        dir
    }

    #[test]
    fn worktrees_of_one_repo_share_one_index_key() {
        if !git_available() {
            eprintln!("SKIP: git not on PATH");
            return;
        }
        let repo = init_repo();
        // A linked worktree lives elsewhere on disk but shares the repo.
        let wt_parent = tempfile::tempdir().unwrap();
        let wt = wt_parent.path().join("wt");
        git(
            repo.path(),
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        std::fs::write(wt.join("a.txt"), "hello\n").unwrap();

        let main_key = repo_index_key(repo.path());
        let wt_key = repo_index_key(&wt);
        assert_eq!(main_key, wt_key, "same repo must resolve to one index key");

        // A subdirectory of a worktree resolves to the same key.
        std::fs::create_dir(wt.join("sub")).unwrap();
        assert_eq!(repo_index_key(&wt.join("sub")), main_key);
    }

    #[test]
    fn different_repos_stay_separate() {
        if !git_available() {
            eprintln!("SKIP: git not on PATH");
            return;
        }
        let a = init_repo();
        let b = init_repo();
        assert_ne!(
            repo_index_key(a.path()),
            repo_index_key(b.path()),
            "different repos must not share an index key"
        );
    }

    #[test]
    fn non_git_folder_keys_by_its_own_path() {
        let dir = tempfile::tempdir().unwrap();
        // Not a git repo (or git missing): falls back to the normalized path.
        let key = repo_index_key(dir.path());
        assert_eq!(key, normalize_root(dir.path()));
    }

    #[test]
    fn repo_key_is_idempotent() {
        if !git_available() {
            eprintln!("SKIP: git not on PATH");
            return;
        }
        let repo = init_repo();
        let key = repo_index_key(repo.path());
        assert_eq!(repo_index_key(&key), key);
    }
}
