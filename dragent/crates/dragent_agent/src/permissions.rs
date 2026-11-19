//! Path-access permissions for agent tools.
//! Prevents tools from reading sensitive or out-of-scope paths.

use std::path::{Path, PathBuf};

// ── AllowedRoots ──────────────────────────────────────────────────────────────

/// A set of allowed root directories that agent tools may read.
///
/// Any path that does not fall under one of these roots, or that matches a
/// hard-coded deny list, is rejected.
#[derive(Clone, Debug)]
pub struct AllowedRoots {
    roots: Vec<PathBuf>,
}

/// Reasons a path check can fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// Path contains `..` components that would escape the allowed roots.
    PathTraversal,
    /// Path falls under a hard-coded sensitive location.
    SensitivePath,
    /// Path does not fall under any allowed root.
    OutsideAllowedRoots,
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathTraversal => write!(f, "path traversal detected (..)"),
            Self::SensitivePath => write!(f, "sensitive path denied"),
            Self::OutsideAllowedRoots => write!(f, "path is outside allowed roots"),
        }
    }
}

/// Hard-coded path prefixes that are always denied.
const DENY_PREFIXES: &[&str] = &[
    "/etc/", "/etc", // exact match for bare /etc
];

/// Home-relative paths that are always denied (expanded at check time).
const DENY_HOME_RELATIVE: &[&str] = &[".ssh"];

impl AllowedRoots {
    /// Create a new instance from a list of allowed root directories.
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
        }
    }

    /// Build roots from the terminal cwd and optional project root (deduped).
    pub fn for_session(cwd: &Path, project_root: Option<&Path>) -> Self {
        let mut roots = vec![cwd.to_path_buf()];
        if let Some(root) = project_root {
            if root != cwd {
                roots.push(root.to_path_buf());
            }
        }
        Self::new(roots)
    }

    /// Check whether `path` is allowed.
    ///
    /// Returns `Ok(())` if access is permitted or `Err(DenyReason)` otherwise.
    /// The path is canonicalized logically (no filesystem access) before checking.
    pub fn check(&self, path: &Path) -> Result<(), DenyReason> {
        // 1. Reject any path that still contains raw `..` components — even if
        //    the resolved canonical path would be safe we deny at this layer to
        //    avoid surprises. Callers should resolve symlinks themselves if needed.
        let path_str = path.to_string_lossy();
        if has_dotdot(path) {
            return Err(DenyReason::PathTraversal);
        }

        // 2. Reject hard-coded sensitive absolute prefixes.
        for prefix in DENY_PREFIXES {
            if path_str == *prefix || path_str.starts_with(&format!("{}/", prefix)) {
                return Err(DenyReason::SensitivePath);
            }
        }

        // 3. Reject home-relative sensitive directories.
        if let Some(home) = dirs::home_dir() {
            for rel in DENY_HOME_RELATIVE {
                let sensitive = home.join(rel);
                let sensitive_str = sensitive.to_string_lossy();
                if path_str == sensitive_str.as_ref()
                    || path_str.starts_with(&format!("{}/", sensitive_str))
                {
                    return Err(DenyReason::SensitivePath);
                }
            }
        }

        // 4. Path must fall under at least one allowed root.
        if self.roots.is_empty() {
            // No roots configured — deny everything.
            return Err(DenyReason::OutsideAllowedRoots);
        }

        for root in &self.roots {
            if path.starts_with(root) {
                return Ok(());
            }
        }

        Err(DenyReason::OutsideAllowedRoots)
    }
}

/// Returns true if the path contains any `..` component.
fn has_dotdot(path: &Path) -> bool {
    use std::path::Component;
    path.components().any(|c| c == Component::ParentDir)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(paths: &[&str]) -> AllowedRoots {
        AllowedRoots::new(paths.iter().map(|s| PathBuf::from(s)))
    }

    #[test]
    fn allowed_path_under_root_passes() {
        let ar = roots(&["/home/user/project"]);
        assert_eq!(
            ar.check(Path::new("/home/user/project/src/main.rs")),
            Ok(())
        );
    }

    #[test]
    fn path_outside_all_roots_denied() {
        let ar = roots(&["/home/user/project"]);
        let result = ar.check(Path::new("/home/other/secret.txt"));
        assert_eq!(result, Err(DenyReason::OutsideAllowedRoots));
    }

    #[test]
    fn dotdot_traversal_denied() {
        let ar = roots(&["/home/user/project"]);
        let result = ar.check(Path::new("/home/user/project/../../../etc/passwd"));
        assert_eq!(result, Err(DenyReason::PathTraversal));
    }

    #[test]
    fn etc_prefix_denied() {
        let ar = roots(&["/home/user/project", "/etc"]);
        let result = ar.check(Path::new("/etc/passwd"));
        assert_eq!(result, Err(DenyReason::SensitivePath));
    }

    #[test]
    fn etc_subdirectory_denied() {
        let ar = roots(&["/home/user/project"]);
        let result = ar.check(Path::new("/etc/ssh/sshd_config"));
        assert_eq!(result, Err(DenyReason::SensitivePath));
    }

    #[test]
    fn ssh_home_dir_denied() {
        // Only run when we can determine the home directory.
        if let Some(home) = dirs::home_dir() {
            let ar = roots(&[home.to_str().unwrap()]);
            let ssh_path = home.join(".ssh").join("id_rsa");
            let result = ar.check(&ssh_path);
            assert_eq!(result, Err(DenyReason::SensitivePath));
        }
    }

    #[test]
    fn no_roots_denies_everything() {
        let ar = AllowedRoots::new(std::iter::empty());
        let result = ar.check(Path::new("/home/user/project/file.rs"));
        assert_eq!(result, Err(DenyReason::OutsideAllowedRoots));
    }

    #[test]
    fn root_itself_is_allowed() {
        let ar = roots(&["/home/user/project"]);
        assert_eq!(ar.check(Path::new("/home/user/project")), Ok(()));
    }

    #[test]
    fn for_session_includes_cwd_and_project_root() {
        let ar = AllowedRoots::for_session(
            Path::new("/home/user/project/pkg"),
            Some(Path::new("/home/user/project")),
        );
        assert_eq!(
            ar.check(Path::new("/home/user/project/src/main.rs")),
            Ok(())
        );
        assert_eq!(ar.check(Path::new("/home/user/project/pkg/lib.rs")), Ok(()));
    }
}
