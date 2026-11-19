//! Repository-relative paths that must stay inside the worktree.
//!
//! Every `cwd`, `from`/`from_example` and `to` is a `/`-separated path
//! relative to the repository root. Absolute paths, Windows drive letters,
//! UNC prefixes, `..` escapes and NUL bytes are refused, so a config can
//! never reach outside the worktree or sweep a home folder. Paths with
//! spaces are fine.

use std::path::{Component, Path, PathBuf};

/// Why a configured path is refused.
pub fn check_relative_inside(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Err("path must not be empty".into());
    }
    if raw.contains('\0') {
        return Err(format!("\"{raw}\" contains a NUL byte"));
    }
    // Normalise backslashes only for the safety check: configs use `/`,
    // but a Windows-style escape must not slip through on Unix.
    let probe = raw.replace('\\', "/");
    if probe.starts_with('/') || probe.starts_with("//") {
        return Err(format!("\"{raw}\" must be relative, not absolute"));
    }
    if probe.len() >= 2 && probe.as_bytes()[1] == b':' {
        return Err(format!("\"{raw}\" must be relative, not absolute"));
    }
    let path = Path::new(raw);
    let mut depth: i32 = 0;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!("\"{raw}\" must stay inside the worktree"));
            }
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!("\"{raw}\" escapes the worktree (..)"));
                }
            }
            Component::CurDir => {}
            Component::Normal(part) => {
                if part == ".." {
                    return Err(format!("\"{raw}\" escapes the worktree (..)"));
                }
                depth += 1;
            }
        }
    }
    // A second pass on the `/`-split form catches `..` hidden behind
    // backslashes on Unix (e.g. `..\\..`).
    let mut slash_depth: i32 = 0;
    for part in probe.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            slash_depth -= 1;
            if slash_depth < 0 {
                return Err(format!("\"{raw}\" escapes the worktree (..)"));
            }
        } else {
            slash_depth += 1;
        }
    }
    Ok(())
}

/// Join a validated relative path onto the worktree root.
pub fn join_inside(worktree: &Path, raw: &str) -> Result<PathBuf, String> {
    check_relative_inside(raw)?;
    Ok(worktree.join(raw.replace('\\', "/"))).map(|joined| joined)
}

/// The worktree a path belongs to: the path itself when it holds a
/// `.dragent` marker or a `.git` entry, else the nearest ancestor that
/// does, else the canonicalized path itself.
pub fn resolve_worktree(path: &Path) -> PathBuf {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if absolute.join(".dragent").is_dir() || absolute.join(".git").exists() {
        return absolute;
    }
    let mut current = absolute.as_path();
    while let Some(parent) = current.parent() {
        if parent.join(".dragent").is_dir() || parent.join(".git").exists() {
            return parent.to_path_buf();
        }
        if parent == current {
            break;
        }
        current = parent;
    }
    absolute
}

/// Find the main checkout of `worktree` (the repository the worktree was
/// created from) by reading `.git/worktrees` or `commondir`, for `copy`.
pub fn main_checkout_of(worktree: &Path) -> Option<PathBuf> {
    let dot_git = worktree.join(".git");
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = text.strip_prefix("gitdir:")?.trim();
    // `<main>/.git/worktrees/<name>` -> `<main>`.
    let gitdir = Path::new(gitdir);
    let mut current = gitdir;
    while let Some(parent) = current.parent() {
        if parent
            .file_name()
            .map(|name| name == "worktrees")
            .unwrap_or(false)
        {
            // `.../.git/worktrees` -> `...` is the main checkout's `.git`.
            let dot = parent.parent()?;
            return dot.parent().map(Path::to_path_buf);
        }
        current = parent;
    }
    // A `commondir` file inside a worktree `.git` dir points at the main
    // checkout's `.git` dir.
    let commondir = gitdir.join("commondir");
    let target = std::fs::read_to_string(commondir).ok()?;
    let main_dot_git = gitdir.join(target.trim());
    main_dot_git.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_are_refused_and_spaces_allowed() {
        for bad in [
            "",
            "/etc/passwd",
            "C:\\Windows",
            "../escape",
            "a/../../escape",
            "..\\escape",
            "a\\..\\..\\escape",
            "//server/share",
        ] {
            assert!(check_relative_inside(bad).is_err(), "{bad}");
        }
        for good in [
            ".env",
            ".env.example",
            "apps/web/.env",
            "my folder/.env",
            "path with spaces/file.txt",
            "./relative.txt",
        ] {
            assert!(check_relative_inside(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn join_stays_inside() {
        let root = Path::new("/repo");
        assert_eq!(
            join_inside(root, "a b/c.env").unwrap(),
            Path::new("/repo/a b/c.env")
        );
        assert!(join_inside(root, "../out").is_err());
    }

    #[test]
    fn main_checkout_detection_handles_plain_repos() {
        // A plain repository has a `.git` directory, not a worktree file.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(main_checkout_of(dir.path()).is_none());
    }
}
