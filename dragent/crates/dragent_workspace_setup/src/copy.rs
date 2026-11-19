//! Explicit `copy` handling: bring named ignored local files from the
//! project's main checkout into a new worktree, or create them from a
//! committed example when missing.
//!
//! Only listed files move, never directories, never globs, never anything
//! outside either tree. `if_missing` (default true) never overwrites: an
//! existing destination is always preserved.

use std::path::Path;

use crate::config::CopyEntry;

/// What one copy entry did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyResult {
    pub to: String,
    pub action: CopyAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyAction {
    /// Copied from the main checkout.
    Copied,
    /// Created from the committed example.
    CreatedFromExample,
    /// Left alone (already there with `if_missing`, or nothing to copy).
    Skipped(String),
}

/// Apply `entries`, resolving `from` against `main_checkout` (falling back
/// to `worktree` itself when it is the main checkout) and `to` inside
/// `worktree`.
pub fn apply_copy(
    entries: &[CopyEntry],
    worktree: &Path,
    main_checkout: Option<&Path>,
) -> Vec<CopyResult> {
    let main = main_checkout.unwrap_or(worktree);
    let mut out = Vec::new();
    for entry in entries {
        out.push(apply_one(entry, worktree, main));
    }
    out
}

/// Why writing `destination` isn't safe: it's a link (writing through it
/// could change, or empty, the file it points at, even the main checkout's
/// own), or a folder on the way resolves outside the worktree. Such links
/// are someone's choice and are left as they are.
fn unsafe_destination(worktree: &Path, destination: &Path) -> Option<String> {
    if std::fs::symlink_metadata(destination).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Some("is a link: left as it is".into());
    }
    let root = worktree.canonicalize().ok()?;
    let mut folder = destination.parent();
    while let Some(dir) = folder {
        if dir.exists() {
            return match dir.canonicalize() {
                Ok(real) if real.starts_with(&root) => None,
                _ => Some("its folder leads outside the worktree (a link): left as it is".into()),
            };
        }
        folder = dir.parent();
    }
    None
}

fn apply_one(entry: &CopyEntry, worktree: &Path, main: &Path) -> CopyResult {
    let to_relative = entry.to.clone();
    let destination = match crate::paths::join_inside(worktree, &entry.to) {
        Ok(path) => path,
        Err(message) => {
            return CopyResult {
                to: to_relative,
                action: CopyAction::Skipped(message),
            }
        }
    };
    if let Some(reason) = unsafe_destination(worktree, &destination) {
        return CopyResult {
            to: to_relative,
            action: CopyAction::Skipped(reason),
        };
    }
    if entry.if_missing_or_default() && destination.exists() {
        return CopyResult {
            to: to_relative,
            action: CopyAction::Skipped("exists (preserved)".into()),
        };
    }
    // Never overwrite when `if_missing` is set; without it an explicit
    // entry may refresh the destination, but still only the listed file.
    if let Some(from) = &entry.from {
        let source = match crate::paths::join_inside(main, from) {
            Ok(path) => path,
            Err(message) => {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Skipped(message),
                }
            }
        };
        if !source.is_file() {
            return CopyResult {
                to: to_relative,
                action: CopyAction::Skipped(format!(
                    "source {} not present in main checkout",
                    source.display()
                )),
            };
        }
        if destination.exists() && entry.if_missing_or_default() {
            return CopyResult {
                to: to_relative,
                action: CopyAction::Skipped("exists (preserved)".into()),
            };
        }
        if let Some(parent) = destination.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Skipped("could not create parent".into()),
                };
            }
        }
        match std::fs::copy(&source, &destination) {
            Ok(_) => {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Copied,
                }
            }
            Err(error) => {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Skipped(format!("copy failed: {error}")),
                }
            }
        }
    }
    if let Some(example) = &entry.from_example {
        if destination.exists() {
            return CopyResult {
                to: to_relative,
                action: CopyAction::Skipped("exists (preserved)".into()),
            };
        }
        let example_path = match crate::paths::join_inside(worktree, example) {
            Ok(path) => path,
            Err(message) => {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Skipped(message),
                }
            }
        };
        if !example_path.is_file() {
            return CopyResult {
                to: to_relative,
                action: CopyAction::Skipped(format!(
                    "example {} not present",
                    example_path.display()
                )),
            };
        }
        if let Some(parent) = destination.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Skipped("could not create parent".into()),
                };
            }
        }
        match std::fs::copy(&example_path, &destination) {
            Ok(_) => {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::CreatedFromExample,
                }
            }
            Err(error) => {
                return CopyResult {
                    to: to_relative,
                    action: CopyAction::Skipped(format!("copy failed: {error}")),
                }
            }
        }
    }
    CopyResult {
        to: to_relative,
        action: CopyAction::Skipped("nothing to copy".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CopyEntry;

    fn entry_from(from: &str, to: &str) -> CopyEntry {
        CopyEntry {
            from: Some(from.into()),
            from_example: None,
            to: to.into(),
            if_missing: None,
        }
    }

    #[test]
    fn if_missing_never_overwrites() {
        let main = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::write(main.path().join(".env"), "A=1").unwrap();
        std::fs::write(work.path().join(".env"), "kept").unwrap();
        let results = apply_copy(
            &[entry_from(".env", ".env")],
            work.path(),
            Some(main.path()),
        );
        assert_eq!(
            results[0].action,
            CopyAction::Skipped("exists (preserved)".into())
        );
        assert_eq!(
            std::fs::read_to_string(work.path().join(".env")).unwrap(),
            "kept"
        );
    }

    #[test]
    fn copies_when_missing_and_creates_from_example() {
        let main = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::write(main.path().join(".env"), "A=1").unwrap();
        std::fs::write(work.path().join(".env.example"), "EXAMPLE=1").unwrap();
        let results = apply_copy(
            &[
                entry_from(".env", ".env"),
                CopyEntry {
                    from: None,
                    from_example: Some(".env.example".into()),
                    to: "from-example.env".into(),
                    if_missing: None,
                },
            ],
            work.path(),
            Some(main.path()),
        );
        assert_eq!(results[0].action, CopyAction::Copied);
        assert_eq!(results[1].action, CopyAction::CreatedFromExample);
        assert_eq!(
            std::fs::read_to_string(work.path().join(".env")).unwrap(),
            "A=1"
        );
    }

    #[test]
    fn second_run_preserves_copied_files() {
        let main = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::write(main.path().join(".env"), "A=1").unwrap();
        let entries = vec![entry_from(".env", ".env")];
        apply_copy(&entries, work.path(), Some(main.path()));
        std::fs::write(work.path().join(".env"), "edited").unwrap();
        let again = apply_copy(&entries, work.path(), Some(main.path()));
        assert_eq!(
            again[0].action,
            CopyAction::Skipped("exists (preserved)".into())
        );
        assert_eq!(
            std::fs::read_to_string(work.path().join(".env")).unwrap(),
            "edited"
        );
    }

    #[test]
    fn escapes_are_skipped_not_copied() {
        let main = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let results = apply_copy(
            &[CopyEntry {
                from: Some("../outside".into()),
                from_example: None,
                to: ".env".into(),
                if_missing: None,
            }],
            work.path(),
            Some(main.path()),
        );
        assert!(matches!(results[0].action, CopyAction::Skipped(_)));
        assert!(!work.path().join(".env").exists());
    }

    #[cfg(unix)]
    #[test]
    fn links_in_the_worktree_are_never_written_through() {
        let main = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(main.path().join(".env"), "A=1").unwrap();
        std::fs::write(outside.path().join("secret"), "keep").unwrap();
        // The worktree's .env links to a file elsewhere, and a folder links out.
        std::os::unix::fs::symlink(outside.path().join("secret"), work.path().join(".env")).unwrap();
        std::os::unix::fs::symlink(outside.path(), work.path().join("conf")).unwrap();
        let entries: Vec<CopyEntry> = serde_json::from_value(serde_json::json!([
            {"from": ".env", "to": ".env", "if_missing": false},
            {"from": ".env", "to": "conf/.env"}
        ]))
        .unwrap();
        let results = apply_copy(&entries, work.path(), Some(main.path()));
        assert!(results
            .iter()
            .all(|result| matches!(&result.action, CopyAction::Skipped(why) if why.contains("left as it is"))),
            "{results:?}");
        assert_eq!(std::fs::read_to_string(outside.path().join("secret")).unwrap(), "keep");
        assert!(!outside.path().join(".env").exists());
    }

}
