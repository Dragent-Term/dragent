//! Bounded project-tree discovery for the future file surface.

use dragent_workspace_model::{DocumentEntry, DocumentRef, ProjectDocumentIndex};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

// Version-control internals and OS thumbnails excluded from the tree —
// never user build output. Build outputs (`node_modules`, `target`,
// `dist`, `build`, `coverage`) are intentionally LISTED: the explorer
// shows everything scanned. The `is_ignored` flag + a future panel toggle
// can hide ignored paths on demand.
pub(crate) const IGNORED_DIRECTORIES: &[&str] = &[".git", ".hg", ".svn", ".jj", ".sl"];

pub(crate) const IGNORED_FILES: &[&str] = &[".DS_Store", "Thumbs.db"];

// Heavy dependency/build outputs: LISTED as single entries (visible,
// expandable on demand) but never descended into by the automatic walk.
// Descending into `node_modules`/`target` on every background refresh makes
// add/delete updates take forever; on-demand `discover_directory` still
// loads their contents when the user expands them.
pub(crate) const LAZY_DIRECTORIES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    ".next",
    ".nuxt",
    "vendor",
    "Pods",
    "__pycache__",
];

// Hard bound on automatic-walk entries: keeps every background refresh fast
// so external add/delete shows promptly. Deeper content remains reachable
// via on-demand expansion. Display truncates separately at 20k.
pub(crate) const MAX_AUTO_WALK_ENTRIES: usize = 50_000;

/// Own gitignore implementation (written from scratch — no upstream code).
/// Covers `.gitignore` per directory, `.git/info/exclude`, and best-effort
/// global excludes. Matched paths are flagged `is_ignored` but still listed;
/// trust/execution gating lives elsewhere.

/// Re-exported area modules; existing `super::project_documents::…` paths
/// keep working. Canonical homes are the documents_* modules.
pub(crate) use super::documents_entries::*;
pub(crate) use super::documents_ignore::*;
pub(crate) use super::documents_imports::*;
pub(crate) use super::documents_io::*;

mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn skips_only_vcs_internals_but_lists_build_output() {
        let root = std::env::temp_dir().join(format!(
            "dragent-project-documents-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::create_dir_all(root.join(".github")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("node_modules/ignored.js"), "ignored").unwrap();
        fs::write(root.join(".env"), "SECRET=hidden").unwrap();
        fs::write(root.join(".github/workflow.yml"), "hidden").unwrap();
        fs::write(root.join(".git/config"), "hidden").unwrap();
        let root_index = discover(&root);
        assert!(root_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path == Path::new("src")));
        // Only VCS internals are excluded: node_modules is listed.
        assert!(root_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path.starts_with("node_modules")));
        assert!(!root_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path.starts_with(".git")));
        let src_index = discover_directory(&root, Path::new("src"));
        assert!(src_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path == Path::new("src/main.rs")));
        assert!(!src_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path.starts_with("node_modules")));
        assert!(root_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path == Path::new(".env")));
        assert!(root_index
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path == Path::new(".github")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gitignored_files_are_listed_but_flagged() {
        let root = std::env::temp_dir().join(format!(
            "dragent-gitignore-flag-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitignore"), "*.log\nbuild/\n!keep.log\n").unwrap();
        fs::write(root.join("debug.log"), "x").unwrap();
        fs::write(root.join("keep.log"), "x").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/out.js"), "x").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        let index = discover(&root);
        let flag = |rel: &str| {
            index
                .entries()
                .iter()
                .find(|entry| entry.document.relative_path == Path::new(rel))
                .map(|entry| entry.is_ignored)
        };
        // Listed but flagged…
        assert_eq!(flag("debug.log"), Some(true));
        assert_eq!(flag("build"), Some(true));
        // Heavy-dir contents are NOT auto-walked (keeps refreshes fast) —
        // they load flagged via on-demand expansion instead.
        assert_eq!(flag("build/out.js"), None);
        // …while negation and normal files stay unflagged.
        assert_eq!(flag("keep.log"), Some(false));
        assert_eq!(flag("src/main.rs"), Some(false));
        let expanded = discover_directory(&root, Path::new("build"));
        assert!(expanded.entries().iter().any(|entry| {
            entry.document.relative_path == Path::new("build/out.js") && entry.is_ignored
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loaded_directory_refresh_finds_external_files() {
        let root = std::env::temp_dir().join(format!(
            "dragent-tree-refresh-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/existing.md"), "existing").unwrap();
        fs::write(root.join("docs/added.md"), "added elsewhere").unwrap();

        let index = discover_loaded_directories(&root, &[PathBuf::from("docs")]);
        assert!(index
            .entries()
            .iter()
            .any(|entry| { entry.document.relative_path == Path::new("docs/added.md") }));

        fs::remove_file(root.join("docs/existing.md")).unwrap();
        fs::rename(root.join("docs/added.md"), root.join("docs/renamed.md")).unwrap();
        let refreshed = discover_loaded_directories(&root, &[PathBuf::from("docs")]);
        assert!(refreshed
            .entries()
            .iter()
            .any(|entry| entry.document.relative_path == Path::new("docs/renamed.md")));
        assert!(!refreshed.entries().iter().any(|entry| {
            matches!(
                entry.document.relative_path.to_str(),
                Some("docs/existing.md" | "docs/added.md")
            )
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn tree_watcher_reports_external_file_creation() {
        use std::sync::atomic::Ordering;

        let root = std::env::temp_dir().join(format!(
            "dragent-tree-watcher-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let (tx, rx) = async_channel::unbounded();
        let stop = spawn_tree_watcher(root.clone(), tx);
        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let mut ready = false;
        while std::time::Instant::now() < ready_deadline {
            if rx.try_recv().is_ok() {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(ready, "project tree watcher did not become ready");
        fs::write(root.join("created-elsewhere.md"), "new").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let mut observed = false;
        while std::time::Instant::now() < deadline {
            if let Ok(event) = rx.try_recv() {
                if !event.changed_paths.is_empty() {
                    observed = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stop.store(true, Ordering::Relaxed);
        let _ = fs::remove_dir_all(root);
        assert!(
            observed,
            "project tree watcher did not report file creation"
        );
    }

    #[test]
    fn read_preview_rejects_parent_paths() {
        let preview = read_preview(Path::new("/tmp"), Path::new("../outside.txt"));
        assert_eq!(
            preview.error.as_deref(),
            Some("File is outside the project")
        );
        assert!(preview.text.is_empty());
    }

    #[test]
    fn read_preview_loads_a_small_text_file() {
        let root = std::env::temp_dir().join(format!(
            "dragent-preview-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("README.md"), "hello preview").unwrap();
        let preview = read_preview(&root, Path::new("README.md"));
        assert_eq!(preview.text, "hello preview");
        assert_eq!(preview.error, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_preview_loads_text_above_the_legacy_two_mib_limit() {
        let root = std::env::temp_dir().join(format!(
            "dragent-large-preview-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let contents = "let value = 42;\n".repeat(220_000); // ~3 MiB
        assert!(contents.len() > 2 * 1024 * 1024);
        fs::write(root.join("large.rs"), &contents).unwrap();

        let preview = read_preview(&root, Path::new("large.rs"));

        assert_eq!(preview.error, None);
        assert_eq!(preview.text.len(), contents.len());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_preview_opens_visual_assets_without_text_decoding() {
        let root = std::env::temp_dir().join(format!(
            "dragent-visual-preview-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        // A minimal file is enough here: classification happens before image
        // decoding, which is performed lazily by the UI image cache.
        fs::write(root.join("logo.svg"), b"<svg />").unwrap();
        let preview = read_preview(&root, Path::new("logo.svg"));
        assert_eq!(preview.error, None);
        assert!(preview.text.is_empty());
        assert_eq!(
            preview.visual_path,
            Some(root.join("logo.svg").canonicalize().unwrap())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_preview_reports_a_stale_tree_entry() {
        let root = std::env::temp_dir().join(format!(
            "dragent-stale-preview-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        let preview = read_preview(&root, Path::new("removed-after-scan.rs"));

        assert_eq!(
            preview.error.as_deref(),
            Some("File is unavailable. Refresh the file tree and try again.")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_text_replaces_a_project_file() {
        let root = std::env::temp_dir().join(format!(
            "dragent-save-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.md"), "before").unwrap();
        save_text(&root, Path::new("notes.md"), "after", None).unwrap();
        assert_eq!(fs::read_to_string(root.join("notes.md")).unwrap(), "after");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_text_refuses_to_overwrite_an_external_change() {
        let root = std::env::temp_dir().join(format!(
            "dragent-save-conflict-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.md");
        fs::write(&path, "opened").unwrap();
        let opened = read_preview(&root, Path::new("notes.md"));
        fs::write(&path, "changed elsewhere").unwrap();

        let error =
            save_text(&root, Path::new("notes.md"), "my draft", opened.fingerprint).unwrap_err();

        assert!(error.contains("changed on disk"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed elsewhere");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_text_rejects_paths_outside_the_project() {
        let result = save_text(Path::new("/tmp"), Path::new("../outside.txt"), "no", None);
        assert!(result.is_err());
    }

    #[test]
    fn explorer_copy_uses_non_destructive_copy_names() {
        let root = std::env::temp_dir().join(format!(
            "dragent-explorer-copy-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.md"), "one").unwrap();

        let first = copy_entry(&root, Path::new("notes.md"), Path::new("")).unwrap();
        let second = copy_entry(&root, Path::new("notes.md"), Path::new("")).unwrap();

        assert_eq!(first, Path::new("notes copy.md"));
        assert_eq!(second, Path::new("notes copy 2.md"));
        assert_eq!(fs::read_to_string(root.join(first)).unwrap(), "one");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explorer_folder_rename_updates_alias_and_relative_imports() {
        let root = std::env::temp_dir().join(format!(
            "dragent-explorer-import-rename-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src/components/ui")).unwrap();
        fs::create_dir_all(root.join("src/pages")).unwrap();
        fs::write(
            root.join("src/components/ui/badge.tsx"),
            "export const Badge = () => null;",
        )
        .unwrap();
        fs::write(
            root.join("src/app.tsx"),
            "import { Badge } from '@/components/ui/badge';\nconst note = '@/components/ui/badge';\n",
        )
        .unwrap();
        fs::write(
            root.join("src/pages/home.tsx"),
            "export { Badge } from '../components/ui/badge';\n",
        )
        .unwrap();

        let changed = update_imports_for_rename(
            &root,
            Path::new("src/components/ui"),
            Path::new("src/components/uis"),
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(changed.len(), 2);
        assert_eq!(
            fs::read_to_string(root.join("src/app.tsx")).unwrap(),
            "import { Badge } from '@/components/uis/badge';\nconst note = '@/components/ui/badge';\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("src/pages/home.tsx")).unwrap(),
            "export { Badge } from '../components/uis/badge';\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explorer_import_update_refuses_to_overwrite_a_dirty_buffer() {
        let root = std::env::temp_dir().join(format!(
            "dragent-explorer-import-dirty-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src/components/ui")).unwrap();
        fs::write(root.join("src/components/ui/badge.ts"), "export {};").unwrap();
        fs::write(root.join("src/app.ts"), "import '@/components/ui/badge';\n").unwrap();
        let protected = HashSet::from([PathBuf::from("src/app.ts")]);

        let error = update_imports_for_rename(
            &root,
            Path::new("src/components/ui"),
            Path::new("src/components/uis"),
            &protected,
        )
        .unwrap_err();

        assert!(error.contains("Save src/app.ts"));
        assert_eq!(
            fs::read_to_string(root.join("src/app.ts")).unwrap(),
            "import '@/components/ui/badge';\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explorer_folder_copy_preserves_nested_contents() {
        let root = std::env::temp_dir().join(format!(
            "dragent-explorer-folder-copy-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/nested/lib.rs"), "pub fn value() {}").unwrap();

        let copied = copy_entry(&root, Path::new("src"), Path::new("")).unwrap();

        assert_eq!(copied, Path::new("src copy"));
        assert_eq!(
            fs::read_to_string(root.join("src copy/nested/lib.rs")).unwrap(),
            "pub fn value() {}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explorer_mutations_reject_path_syntax_in_names() {
        let root = std::env::temp_dir().join(format!(
            "dragent-explorer-name-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        assert!(create_entry(&root, Path::new(""), "../outside", false).is_err());
        assert!(create_entry(&root, Path::new(""), "nested/file", false).is_err());
        assert!(!root.parent().unwrap().join("outside").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explorer_move_and_rename_remain_inside_project() {
        let root = std::env::temp_dir().join(format!(
            "dragent-explorer-move-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("before.txt"), "value").unwrap();

        let renamed = rename_entry(&root, Path::new("before.txt"), "after.txt").unwrap();
        let moved = move_entry(&root, &renamed, Path::new("target")).unwrap();

        assert_eq!(moved, Path::new("target/after.txt"));
        assert_eq!(fs::read_to_string(root.join(moved)).unwrap(), "value");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn checkpoint_is_kept_inside_dragent_metadata() {
        let root = std::env::temp_dir().join(format!(
            "dragent-checkpoint-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let checkpoint = write_checkpoint(&root, Path::new("src/lib.rs"), "+change\n").unwrap();
        assert!(checkpoint.starts_with(dragent_paths::workspace(&root).join("checkpoints")));
        assert_eq!(fs::read_to_string(checkpoint).unwrap(), "+change\n");
        let _ = fs::remove_dir_all(root);
    }
}
