//! Choosing which gitignored files follow the project into a new worktree.
//!
//! A pattern without a `/` matches a file name at any depth (`.env` copies
//! `apps/web/.env` too); one with a `/` matches the path from the project
//! root. `*` stands for any run of characters within one path segment.

use std::path::{Path, PathBuf};

fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => {
            // `*` never crosses a segment.
            (0..=text.len())
                .take_while(|&skip| !text[..skip].contains(&b'/'))
                .any(|skip| glob(rest, &text[skip..]))
        }
        Some((head, rest)) => text.first() == Some(head) && glob(rest, &text[1..]),
    }
}

/// Whether `relative` (a `/`-separated path from the project root) is
/// selected by `pattern`.
pub fn matches(pattern: &str, relative: &str) -> bool {
    let pattern = pattern.trim().trim_start_matches("./");
    if pattern.is_empty() {
        return false;
    }
    if pattern.contains('/') {
        return glob(pattern.trim_start_matches('/').as_bytes(), relative.as_bytes());
    }
    let name = relative.rsplit('/').next().unwrap_or(relative);
    glob(pattern.as_bytes(), name.as_bytes())
}

/// Copy the `candidates` (paths relative to `from`) any pattern selects
/// into the same place under `to`, never overwriting what is already
/// there. Returns the paths copied.
pub fn copy_selected(
    from: &Path,
    to: &Path,
    candidates: &[String],
    patterns: &[String],
) -> Vec<PathBuf> {
    let mut copied = Vec::new();
    for relative in candidates {
        let relative = relative.trim_end_matches('/');
        if relative.is_empty() || !patterns.iter().any(|pattern| matches(pattern, relative)) {
            continue;
        }
        let source = from.join(relative);
        let target = to.join(relative);
        if !source.is_file() || target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                continue;
            }
        }
        if std::fs::copy(&source, &target).is_ok() {
            copied.push(PathBuf::from(relative));
        }
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_at_any_depth_and_paths_from_the_root() {
        assert!(matches(".env", ".env"));
        assert!(matches(".env", "apps/web/.env"));
        assert!(!matches(".env", ".env.local"));
        assert!(matches(".env.*.local", ".env.development.local"));
        assert!(!matches(".env.*.local", ".env.local"));
        assert!(matches("config/*.json", "config/secrets.json"));
        assert!(!matches("config/*.json", "config/nested/secrets.json"));
        assert!(!matches("config/*.json", "apps/config/secrets.json"));
        assert!(!matches("  ", ".env"));
    }

    #[test]
    fn copies_selected_files_without_overwriting() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(from.path().join("apps/web")).unwrap();
        std::fs::write(from.path().join(".env"), "A=1").unwrap();
        std::fs::write(from.path().join("apps/web/.env.local"), "B=2").unwrap();
        std::fs::write(from.path().join("notes.txt"), "no").unwrap();
        std::fs::write(to.path().join(".env"), "kept").unwrap();
        let candidates: Vec<String> = [".env", "apps/web/.env.local", "notes.txt", "node_modules/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let patterns = vec![".env".to_string(), ".env.local".to_string()];
        let copied = copy_selected(from.path(), to.path(), &candidates, &patterns);
        assert_eq!(copied, vec![PathBuf::from("apps/web/.env.local")]);
        assert_eq!(std::fs::read_to_string(to.path().join(".env")).unwrap(), "kept");
        assert!(!to.path().join("notes.txt").exists());
    }
}
