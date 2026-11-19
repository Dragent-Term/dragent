//! Fuzzy-match candidate scoring for `@`-mention chips in the AI input box.
//!
//! Pure scoring logic only — the popup's open/selected/candidates state lives
//! directly on `InputEditor` (see `workspace/input_editor.rs`) so the popup
//! can render as a caret-anchored overlay inside the editor's own tree
//! without a cross-model round trip.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::console::fs_suggest::get_match_type_and_score;

/// The files `@` can offer in the repository around `cwd`, relative to the
/// returned root. Picking a file needs only its name, so this reads the folder
/// rather than depending on the code index, which embeds every file to answer
/// the same question. Follows the same ignore rules as the index, so anything
/// excluded from it stays out of the picker too. Slow on big trees: call off
/// the UI thread. Kept briefly because it runs on every keystroke.
#[cfg(not(target_family = "wasm"))]
pub fn project_files(cwd: &Path) -> (PathBuf, Arc<Vec<PathBuf>>) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const MAX_FILES: usize = 10_000;
    const KEEP_FOR: Duration = Duration::from_secs(15);
    type Listing = (PathBuf, Instant, Arc<Vec<PathBuf>>);
    static LAST: Mutex<Option<Listing>> = Mutex::new(None);

    let root = crate::git_status::repo_root(cwd);
    if let Ok(last) = LAST.lock() {
        if let Some((listed, at, files)) = last.as_ref() {
            if *listed == root && at.elapsed() < KEEP_FOR {
                return (root, files.clone());
            }
        }
    }
    let settings = crate::settings::AppSettings::global().index.clone();
    let mut walker = ignore::WalkBuilder::new(&root);
    walker
        .hidden(true)
        .git_ignore(settings.respect_gitignore)
        .require_git(false);
    if let Some(global) = dragent_index::walk::global_ignore_path() {
        walker.add_ignore(global);
    }
    let files: Vec<PathBuf> = walker
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.path().strip_prefix(&root).ok().map(Path::to_path_buf))
        .take(MAX_FILES)
        .collect();
    let files = Arc::new(files);
    if let Ok(mut last) = LAST.lock() {
        *last = Some((root.clone(), Instant::now(), files.clone()));
    }
    (root, files)
}

#[cfg(target_family = "wasm")]
pub fn project_files(cwd: &Path) -> (PathBuf, Arc<Vec<PathBuf>>) {
    (cwd.to_path_buf(), Arc::new(Vec::new()))
}

/// A single `@`-mention candidate: a project file the user can pick to embed
/// as a chip.
#[derive(Clone, Debug, PartialEq)]
pub struct MentionEntry {
    /// Absolute path — this is what gets loaded as attachment content at submit time.
    pub path: PathBuf,
    /// Relative-to-repo-root display label — this is the literal text the
    /// chip renders in the input buffer.
    pub label: String,
    pub is_dir: bool,
}

/// Score `candidates` (paths relative to `repo_root`, as returned by
/// `CodeIndexManager::list_indexed_paths`) against `query`, filter non-matches,
/// and return the top `limit` entries best-match-first.
pub fn score_and_filter(
    candidates: &[PathBuf],
    repo_root: &Path,
    query: &str,
    limit: usize,
) -> Vec<MentionEntry> {
    let mut scored: Vec<_> = candidates
        .iter()
        .filter_map(|rel| {
            let label = rel.to_string_lossy().replace('\\', "/");
            let (match_type, score) = get_match_type_and_score(query, &label)?;
            Some((
                match_type,
                score,
                MentionEntry {
                    path: repo_root.join(rel),
                    label,
                    is_dir: false,
                },
            ))
        })
        .collect();

    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.2.label.cmp(&b.2.label))
    });

    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, entry)| entry)
        .collect()
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn the_picker_lists_project_files_without_an_index() {
        let root = std::env::temp_dir().join(format!("dragent-mentions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("build/out.rs"), "// generated\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );

        let (listed_root, files) = project_files(&root.join("src"));
        assert_eq!(
            listed_root.canonicalize().unwrap(),
            root.canonicalize().unwrap(),
            "a subfolder lists its whole repository"
        );
        assert!(files.contains(&PathBuf::from("src/main.rs")), "{files:?}");
        assert!(!files.contains(&PathBuf::from("build/out.rs")), "{files:?}");

        let entries = score_and_filter(&files, &listed_root, "main", 20);
        assert_eq!(entries.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
