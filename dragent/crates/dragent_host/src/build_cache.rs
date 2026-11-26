//! One build-cache root per repository, with a private Cargo target folder
//! per worktree.
//!
//! A terminal in a linked git worktree gets `CARGO_TARGET_DIR` pointing to
//! `<org>/build-cache/<repo>/worktrees/<worktree>/target`, so every build
//! folder of a repository is in one place to inspect and clear. The folder
//! is never shared between worktrees: Cargo names workspace artifacts
//! without the checkout's path and judges freshness by modification times,
//! so one worktree would silently run code built from another's sources.
//! What is safe to share (compiled dependencies, by content) goes through
//! `sccache` where it is installed.
//!
//! A repository's main checkout and plain folders keep Cargo's own default.
//! A value already set always wins. The folder goes with its worktree; one
//! whose worktree was removed some other way is found by the sweep, from the
//! path written beside it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Folder under the organisation dir holding every repository's build cache.
pub const BUILD_CACHE_DIRNAME: &str = "build-cache";
pub const TARGET_DIRNAME: &str = "target";
const WORKTREES_DIRNAME: &str = "worktrees";
/// Beside a target folder: the path of the worktree it belongs to.
const OWNER_FILENAME: &str = "worktree.txt";
/// Cargo's own marker at the top of every target folder.
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// A filesystem-safe name for `path`, distinct per path.
fn id_for(path: &Path, fallback: &str) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    let hash: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = crate::worktrees::sanitize_segment(base, 32, fallback);
    format!("{stem}-{hash}")
}

/// The linked worktree `dir` is in, and its repository's main checkout.
/// `None` in a main checkout, a submodule or outside git.
fn linked_worktree(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let worktree = dir.ancestors().find(|dir| dir.join(".git").exists())?;
    let pointer = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let git_dir = worktree.join(pointer.trim().strip_prefix("gitdir:")?.trim());
    let worktrees = git_dir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees.parent()?;
    let repo = if common.file_name()? == ".git" {
        common.parent()?
    } else {
        common
    };
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    Some((worktree.to_path_buf(), repo))
}

/// Where the linked worktree `dir` is in keeps its build.
pub fn worktree_cache_dir(org_dir: &Path, dir: &Path) -> Option<PathBuf> {
    let (worktree, repo) = linked_worktree(dir)?;
    Some(
        org_dir
            .join(BUILD_CACHE_DIRNAME)
            .join(id_for(&repo, "repo"))
            .join(WORKTREES_DIRNAME)
            .join(id_for(&worktree, "worktree")),
    )
}

/// Whether `sccache` is installed (probed once per process).
pub fn sccache_available() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| crate::agents::which("sccache").is_some())
}

/// Shell fragment that sets `key` to `value` only when `key` is currently
/// unset, so a user/project value always wins. `quote` renders a shell-safe
/// literal (see [`crate::agent_sessions::quote`]).
pub fn conditional_default_export(
    kind: dragent_shell_hooks::ShellKind,
    quote: &dyn Fn(&str) -> String,
    key: &str,
    value: &str,
) -> String {
    use dragent_shell_hooks::ShellKind;
    let quoted = quote(value);
    match kind {
        ShellKind::Fish => format!("if not set -q {key}; set -gx {key} {quoted}; end; "),
        ShellKind::PowerShell => format!("if (-not $env:{key}) {{ $env:{key} = {quoted} }}; "),
        ShellKind::Nu => format!("if ($env.{key}? | is-empty) {{ $env.{key} = {quoted} }}; "),
        ShellKind::Zsh | ShellKind::Bash | ShellKind::Other => {
            format!("if [ -z \"${{{key}:-}}\" ]; then export {key}={quoted}; fi; ")
        }
    }
}

/// Shell prefix for a terminal in `dir` (canonical): where Cargo builds, and
/// the compiler cache where there is one. Each only when unset.
pub fn bootstrap_prefix(
    kind: dragent_shell_hooks::ShellKind,
    quote: &dyn Fn(&str) -> String,
    org_dir: &Path,
    dir: &Path,
) -> String {
    let Some(cache) = worktree_cache_dir(org_dir, dir) else {
        return String::new();
    };
    let Some((worktree, _)) = linked_worktree(dir) else {
        return String::new();
    };
    // Without its owner's path the folder could never be swept.
    let owned = std::fs::create_dir_all(&cache).and_then(|()| {
        std::fs::write(
            cache.join(OWNER_FILENAME),
            worktree.to_string_lossy().as_bytes(),
        )
    });
    if let Err(error) = owned {
        log::warn!("[build-cache] {}: {error}", cache.display());
        return String::new();
    }
    let target = cache.join(TARGET_DIRNAME);
    let mut out =
        conditional_default_export(kind, quote, "CARGO_TARGET_DIR", &target.to_string_lossy());
    if sccache_available() {
        out.push_str(&conditional_default_export(
            kind,
            quote,
            "RUSTC_WRAPPER",
            "sccache",
        ));
    }
    out
}

/// Whether `dir` looks like a Cargo target folder (its own marker).
fn is_cargo_target(dir: &Path) -> bool {
    std::fs::read(dir.join(CACHEDIR_TAG))
        .map(|tag| tag.starts_with(CACHEDIR_SIGNATURE))
        .unwrap_or(false)
}

/// Bytes under `dir`, not following symlinks.
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Every worktree's folder under `org_dir`, with the worktree it is for.
fn worktree_dirs(org_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let entries = |dir: &Path| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    };
    entries(&org_dir.join(BUILD_CACHE_DIRNAME))
        .iter()
        .flat_map(|repo| entries(&repo.join(WORKTREES_DIRNAME)))
        .filter_map(|dir| {
            let owner = std::fs::read_to_string(dir.join(OWNER_FILENAME)).ok()?;
            (!owner.is_empty()).then(|| (dir, PathBuf::from(owner)))
        })
        .collect()
}

/// Remove one worktree's folder: only a Cargo target and our own note.
/// Returns the bytes freed.
fn remove_dir(dir: &Path) -> std::io::Result<u64> {
    let target = dir.join(TARGET_DIRNAME);
    let mut freed = 0;
    if target.symlink_metadata().is_ok() {
        if !is_cargo_target(&target) {
            return Err(std::io::Error::other(format!(
                "{} isn't a Cargo target folder",
                target.display()
            )));
        }
        freed = dir_size(&target);
        std::fs::remove_dir_all(&target)?;
    }
    let _ = std::fs::remove_file(dir.join(OWNER_FILENAME));
    // Anything else in there is not ours, and keeps the folder.
    let _ = std::fs::remove_dir(dir);
    if let Some(worktrees) = dir.parent() {
        let _ = std::fs::remove_dir(worktrees);
        if let Some(repo) = worktrees.parent() {
            let _ = std::fs::remove_dir(repo);
        }
    }
    Ok(freed)
}

/// Remove the build folder of `worktree`, which is being or has been
/// removed. Returns the bytes freed.
pub fn remove_for_worktree(org_dir: &Path, worktree: &Path) -> std::io::Result<u64> {
    let canonical = worktree.canonicalize().ok().or_else(|| {
        let parent = worktree.parent()?.canonicalize().ok()?;
        Some(parent.join(worktree.file_name()?))
    });
    let mut freed = 0;
    for (dir, owner) in worktree_dirs(org_dir) {
        if owner == worktree || Some(&owner) == canonical.as_ref() {
            freed += remove_dir(&dir)?;
        }
    }
    Ok(freed)
}

/// Remove the build folders of worktrees that no longer exist. Returns how
/// many, and the bytes freed.
pub fn remove_orphans(org_dir: &Path) -> (usize, u64) {
    let (mut folders, mut bytes) = (0, 0);
    for (dir, owner) in worktree_dirs(org_dir) {
        // Where its parent folder is missing too, a drive may be unplugged.
        let gone = !owner.exists() && owner.parent().is_some_and(Path::exists);
        if !gone {
            continue;
        }
        match remove_dir(&dir) {
            Ok(freed) => {
                folders += 1;
                bytes += freed;
            }
            Err(error) => log::warn!("[build-cache] {}: {error}", dir.display()),
        }
    }
    (folders, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A repository with linked worktrees `a` and `b`, and an organisation.
    fn repo() -> (Scratch, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("dragent-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("repo")).unwrap();
        let base = base.canonicalize().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(base.join("repo"))
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
        git(&["worktree", "add", "-q", "../a"]);
        git(&["worktree", "add", "-q", "../b"]);
        let org = base.join("org");
        (Scratch(base.clone()), base, org)
    }

    fn built(org: &Path, worktree: &Path) -> PathBuf {
        let quote = |s: &str| format!("'{s}'");
        let prefix = bootstrap_prefix(dragent_shell_hooks::ShellKind::Bash, &quote, org, worktree);
        let cache = worktree_cache_dir(org, worktree).unwrap();
        let target = cache.join(TARGET_DIRNAME);
        assert!(prefix.contains(&format!("CARGO_TARGET_DIR='{}'", target.display())));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(CACHEDIR_TAG), CACHEDIR_SIGNATURE).unwrap();
        std::fs::write(target.join("blob"), vec![0u8; 2048]).unwrap();
        cache
    }

    #[test]
    fn each_worktree_builds_in_its_own_folder_under_its_repositorys_root() {
        let (_scratch, base, org) = repo();
        let a = worktree_cache_dir(&org, &base.join("a")).unwrap();
        let b = worktree_cache_dir(&org, &base.join("b")).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.parent(), b.parent(), "one root for the repository");
        std::fs::create_dir_all(base.join("a/crates/x")).unwrap();
        assert_eq!(worktree_cache_dir(&org, &base.join("a/crates/x")), Some(a));
        // The person's own checkout and plain folders are left alone.
        assert_eq!(worktree_cache_dir(&org, &base.join("repo")), None);
        assert_eq!(worktree_cache_dir(&org, &base), None);
    }

    #[test]
    fn a_removed_worktrees_build_folder_goes_and_the_others_stay() {
        let (_scratch, base, org) = repo();
        let a = built(&org, &base.join("a"));
        let b = built(&org, &base.join("b"));
        std::fs::remove_dir_all(base.join("a")).unwrap();
        assert!(remove_for_worktree(&org, &base.join("a")).unwrap() >= 2048);
        assert!(!a.exists());
        assert!(b.join(TARGET_DIRNAME).exists());
        // Removed behind our back: the sweep finds it.
        assert_eq!(remove_orphans(&org), (0, 0));
        std::fs::remove_dir_all(base.join("b")).unwrap();
        assert_eq!(remove_orphans(&org).0, 1);
        assert!(!b.exists());
        assert!(!org.join(BUILD_CACHE_DIRNAME).join("x").exists());
    }

    #[test]
    fn nothing_but_a_cargo_target_is_ever_removed() {
        let (_scratch, base, org) = repo();
        let a = built(&org, &base.join("a"));
        std::fs::remove_file(a.join(TARGET_DIRNAME).join(CACHEDIR_TAG)).unwrap();
        assert!(remove_for_worktree(&org, &base.join("a")).is_err());
        assert!(a.join(TARGET_DIRNAME).join("blob").exists());
        let b = built(&org, &base.join("b"));
        std::fs::write(b.join("mine.txt"), "keep").unwrap();
        remove_for_worktree(&org, &base.join("b")).unwrap();
        assert!(b.join("mine.txt").exists());
        assert!(!b.join(TARGET_DIRNAME).exists());
    }

    #[test]
    fn exports_set_defaults_without_clobbering() {
        use dragent_shell_hooks::ShellKind;
        let quote = |s: &str| format!("'{s}'");
        let bash =
            conditional_default_export(ShellKind::Bash, &quote, "CARGO_TARGET_DIR", "/cache/t");
        assert!(bash.contains("[ -z \"${CARGO_TARGET_DIR:-}\" ]"), "{bash}");
        assert!(
            bash.contains("export CARGO_TARGET_DIR='/cache/t'"),
            "{bash}"
        );
        let fish =
            conditional_default_export(ShellKind::Fish, &quote, "CARGO_TARGET_DIR", "/cache/t");
        assert!(fish.contains("if not set -q CARGO_TARGET_DIR"), "{fish}");
        let ps = conditional_default_export(
            ShellKind::PowerShell,
            &quote,
            "CARGO_TARGET_DIR",
            "/cache/t",
        );
        assert!(ps.contains("$env:CARGO_TARGET_DIR"), "{ps}");
        let nu = conditional_default_export(ShellKind::Nu, &quote, "CARGO_TARGET_DIR", "/cache/t");
        assert!(nu.contains("$env.CARGO_TARGET_DIR?"), "{nu}");
    }

    /// The conditional actually honours a pre-set value in a real shell.
    #[cfg(unix)]
    #[test]
    fn shell_fragment_keeps_a_user_value_and_fills_a_missing_one() {
        use dragent_shell_hooks::ShellKind;
        let quote = |s: &str| format!("'{s}'");
        let fragment =
            conditional_default_export(ShellKind::Bash, &quote, "CARGO_TARGET_DIR", "/cache/t");
        let kept = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("CARGO_TARGET_DIR=/user/keep; {fragment} echo \"$CARGO_TARGET_DIR\""),
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&kept.stdout).trim(), "/user/keep");
        let filled = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("unset CARGO_TARGET_DIR; {fragment} echo \"$CARGO_TARGET_DIR\""),
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&filled.stdout).trim(), "/cache/t");
    }
}
