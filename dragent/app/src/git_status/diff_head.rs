//! Git diff --numstat parsing and line-stat fetching vs HEAD.

use crate::console::git_status::GitDiffFileStat;
use crate::process_ext::NoWindow;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum GitDiffTarget {
    Unstaged,
    Staged,
}

impl GitDiffTarget {
    pub fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Unstaged => ("Index", "Working Tree"),
            Self::Staged => ("HEAD", "Index"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitFileDiff {
    pub relative_path: PathBuf,
    /// Path of the base blob when Git detected a rename/copy.
    pub base_relative_path: PathBuf,
    pub target: GitDiffTarget,
    pub old_text: String,
    pub new_text: String,
    pub diff: dragent_document::DocumentDiff,
    pub full_diff: dragent_document::DocumentDiff,
    /// Precomputed lexical state for compact multi-file previews. Building
    /// these vectors while diffing keeps whole-file scans off every UI frame.
    pub old_highlight_states: Vec<dragent_editor::HighlightState>,
    pub new_highlight_states: Vec<dragent_editor::HighlightState>,
    /// Tree-sitter highlights of each side, computed with the diff (off the
    /// UI thread). Preferred over the lexical states whenever the file's
    /// grammar is available; `None` for binary files or unknown languages.
    pub old_syntax: Option<std::sync::Arc<dragent_editor::DocumentHighlights>>,
    pub new_syntax: Option<std::sync::Arc<dragent_editor::DocumentHighlights>>,
    /// Compact-preview measurements cached with the diff. The multi-file view
    /// is rebuilt frequently while scrolling and editing, so it must not walk
    /// every changed line merely to size the shared canvas or paint stats.
    pub additions: usize,
    pub deletions: usize,
    pub max_changed_line_chars: usize,
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitChangedFile {
    pub relative_path: PathBuf,
    pub base_relative_path: Option<PathBuf>,
    pub target: GitDiffTarget,
    pub status: char,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitMultiDiff {
    pub title: String,
    pub subtitle: Option<String>,
    /// Historical commit reviews share the multi-file presentation but must
    /// never expose worktree/index mutation actions.
    pub read_only: bool,
    pub files: Vec<GitMultiDiffFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitMultiDiffFile {
    pub change: GitChangedFile,
    pub diff: Result<GitFileDiff, String>,
}

impl GitFileDiff {
    pub fn additions(&self) -> usize {
        self.additions
    }

    pub fn deletions(&self) -> usize {
        self.deletions
    }
}

/// Run `git diff --numstat HEAD` (with fallback to `--cached`) and return
/// `(total_added, total_deleted)` line counts across all changed files.
///
/// Uses a HEAD-relative diff so staged and unstaged changes are counted
/// relative to the last commit — no double-counting.
pub fn fetch_line_stats(cwd: &Path) -> (u32, u32) {
    let (a, d, _) = fetch_file_stats(cwd);
    (a, d)
}

/// Run `git diff --numstat HEAD` and return totals plus per-file breakdown.
pub fn fetch_file_stats(cwd: &Path) -> (u32, u32, Vec<GitDiffFileStat>) {
    let (a, d, files) = run_numstat_files(cwd, &["diff", "--numstat", "HEAD"]);
    if a > 0 || d > 0 || !files.is_empty() {
        return (a, d, files);
    }
    run_numstat_files(cwd, &["diff", "--numstat", "--cached"])
}

/// Scoped numstat for incremental refresh: per-file rows for exactly the
/// given `:(literal)` pathspecs. Totals are recomputed by the caller from
/// the merged file list, so only rows are returned.
pub fn fetch_file_stats_scoped(cwd: &Path, specs: &[String]) -> Vec<GitDiffFileStat> {
    let mut head_args: Vec<String> = vec![
        "diff".to_string(),
        "--numstat".to_string(),
        "HEAD".to_string(),
    ];
    if !specs.is_empty() {
        head_args.push("--".to_string());
        head_args.extend(specs.iter().cloned());
    }
    let head_refs: Vec<&str> = head_args.iter().map(String::as_str).collect();
    let (a, d, files) = run_numstat_files(cwd, &head_refs);
    if a > 0 || d > 0 || !files.is_empty() {
        return files;
    }
    let mut cached_args: Vec<String> = vec![
        "diff".to_string(),
        "--numstat".to_string(),
        "--cached".to_string(),
    ];
    if !specs.is_empty() {
        cached_args.push("--".to_string());
        cached_args.extend(specs.iter().cloned());
    }
    let cached_refs: Vec<&str> = cached_args.iter().map(String::as_str).collect();
    let (_, _, files) = run_numstat_files(cwd, &cached_refs);
    files
}

/// Run `git <args>` and parse numstat output into totals + per-file stats.
fn run_numstat_files(cwd: &Path, args: &[&str]) -> (u32, u32, Vec<GitDiffFileStat>) {
    let out = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(args)
        .current_dir(cwd)
        .output();
    let Ok(out) = out else { return (0, 0, vec![]) };
    if !out.status.success() {
        return (0, 0, vec![]);
    }
    parse_numstat_files(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `git diff --numstat` output into `(total_added, total_deleted)`.
///
/// Each line is: `<added>\t<deleted>\t<path>`
/// Binary files show `-\t-\t<path>` and are skipped.
pub fn parse_numstat(output: &str) -> (u32, u32) {
    let (a, d, _) = parse_numstat_files(output);
    (a, d)
}

/// Parse `git diff --numstat` output into totals and per-file stats.
pub fn parse_numstat_files(output: &str) -> (u32, u32, Vec<GitDiffFileStat>) {
    let mut total_added = 0u32;
    let mut total_deleted = 0u32;
    let mut files = Vec::new();
    for line in output.lines() {
        let mut cols = line.splitn(3, '\t');
        let (Some(a_str), Some(d_str), Some(path_str)) = (cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        let Ok(a) = a_str.trim().parse::<u32>() else {
            continue;
        };
        let Ok(d) = d_str.trim().parse::<u32>() else {
            continue;
        };
        total_added += a;
        total_deleted += d;

        let path = Path::new(path_str.trim());
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.trim().to_string());
        let dir_short = shorten_dir(
            path.parent()
                .map(|p| p.to_str().unwrap_or(""))
                .unwrap_or(""),
        );
        files.push(GitDiffFileStat {
            filename,
            dir_short,
            added: a,
            deleted: d,
            rel_path: path_str.trim().to_string(),
        });
    }
    (total_added, total_deleted, files)
}

/// Shorten a directory path for compact display, e.g. "app/src/agent/controller" → "…/agent/controller".
fn shorten_dir(dir: &str) -> String {
    if dir.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = dir.split('/').collect();
    if parts.len() <= 3 {
        dir.to_string()
    } else {
        format!("…/{}", parts[parts.len().saturating_sub(2)..].join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn committed_repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "--quiet"]);
        git(temp.path(), &["config", "user.name", "Diff Test"]);
        git(temp.path(), &["config", "user.email", "diff@example.test"]);
        std::fs::write(temp.path().join("sample.rs"), "fn old() {}\n").unwrap();
        git(temp.path(), &["add", "sample.rs"]);
        git(temp.path(), &["commit", "--quiet", "-m", "initial"]);
        temp
    }

    #[test]
    fn parse_numstat_mixed() {
        let input = "5\t3\tsrc/foo.rs\n2\t1\tsrc/bar.rs";
        assert_eq!(parse_numstat(input), (7, 4));
    }

    #[test]
    fn parse_numstat_binary_skip() {
        let input = "-\t-\tbinary.png\n1\t0\tsrc/f.rs";
        assert_eq!(parse_numstat(input), (1, 0));
    }

    #[test]
    fn parse_numstat_empty() {
        assert_eq!(parse_numstat(""), (0, 0));
    }

    #[test]
    fn structured_diff_uses_index_for_unstaged_and_head_for_staged() {
        let temp = committed_repo();
        std::fs::write(temp.path().join("sample.rs"), "fn staged() {}\n").unwrap();
        git(temp.path(), &["add", "sample.rs"]);
        std::fs::write(temp.path().join("sample.rs"), "fn working() {}\n").unwrap();

        let unstaged =
            fetch_structured_file_diff(temp.path(), "sample.rs", GitDiffTarget::Unstaged).unwrap();
        assert_eq!(unstaged.old_text, "fn staged() {}\n");
        assert_eq!(unstaged.new_text, "fn working() {}\n");

        let staged =
            fetch_structured_file_diff(temp.path(), "sample.rs", GitDiffTarget::Staged).unwrap();
        assert_eq!(staged.old_text, "fn old() {}\n");
        assert_eq!(staged.new_text, "fn staged() {}\n");
    }

    #[test]
    fn partially_staged_file_produces_two_distinct_comparisons() {
        let temp = committed_repo();
        std::fs::write(
            temp.path().join("sample.rs"),
            "fn staged_change() {}\nfn second() {}\n",
        )
        .unwrap();
        git(temp.path(), &["add", "sample.rs"]);
        std::fs::write(
            temp.path().join("sample.rs"),
            "fn staged_change() {}\nfn working_change() {}\n",
        )
        .unwrap();

        let staged =
            fetch_structured_file_diff(temp.path(), "sample.rs", GitDiffTarget::Staged).unwrap();
        let working =
            fetch_structured_file_diff(temp.path(), "sample.rs", GitDiffTarget::Unstaged).unwrap();

        assert_eq!(staged.old_text, "fn old() {}\n");
        assert_eq!(staged.new_text, "fn staged_change() {}\nfn second() {}\n");
        assert_eq!(working.old_text, staged.new_text);
        assert_eq!(
            working.new_text,
            "fn staged_change() {}\nfn working_change() {}\n"
        );
        assert_ne!(staged.diff, working.diff);
    }

    #[test]
    fn structured_worktree_files_dedup_identical_rows() {
        // git can emit the same untracked record twice; it must render once.
        let files = parse_structured_worktree_files(b"?? dup.rs\0?? dup.rs\0 M kept.rs\0").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, PathBuf::from("dup.rs"));
        // Same path across targets (partially staged) must NOT collapse.
        let files = parse_structured_worktree_files(b"MM both.rs\0").unwrap();
        assert_eq!(files.len(), 2);
        assert!(files
            .iter()
            .any(|file| file.target == GitDiffTarget::Staged));
        assert!(files
            .iter()
            .any(|file| file.target == GitDiffTarget::Unstaged));
    }

    #[test]
    fn nul_containing_files_are_treated_as_binary() {
        let diff = structured_diff_from_bytes(
            PathBuf::from("asset.bin"),
            PathBuf::from("asset.bin"),
            GitDiffTarget::Unstaged,
            b"valid utf8\0old".to_vec(),
            b"valid utf8\0new".to_vec(),
        );
        assert!(diff.binary);
        assert!(diff.diff.is_empty());
    }

    #[test]
    fn structured_diff_ignores_windows_checkout_line_endings() {
        let diff = structured_diff_from_bytes(
            PathBuf::from("sample.rs"),
            PathBuf::from("sample.rs"),
            GitDiffTarget::Unstaged,
            b"fn first() {}\nfn second() {}\n".to_vec(),
            b"fn first() {}\r\nfn second() {}\r\n".to_vec(),
        );

        assert!(diff.diff.is_empty());
        assert!(diff.full_diff.is_empty());
        assert_eq!(diff.old_text, diff.new_text);
    }

    #[test]
    fn structured_diff_treats_untracked_file_as_an_addition() {
        let temp = committed_repo();
        std::fs::write(temp.path().join("new.txt"), "first\nsecond\n").unwrap();
        let diff =
            fetch_structured_file_diff(temp.path(), "new.txt", GitDiffTarget::Unstaged).unwrap();
        assert!(diff.old_text.is_empty());
        assert_eq!(diff.additions(), 2);
        assert_eq!(diff.deletions(), 0);
    }

    #[test]
    fn structured_diff_rejects_paths_outside_the_repository() {
        let temp = committed_repo();
        let error =
            fetch_structured_file_diff(temp.path(), "../outside.txt", GitDiffTarget::Unstaged)
                .unwrap_err();
        assert!(error.contains("repository-relative"));
    }

    #[test]
    fn structured_worktree_files_preserve_both_comparison_targets() {
        let diff = parse_structured_worktree_files(b"MM sample.rs\0?? new file.txt\0").unwrap();
        assert_eq!(diff.len(), 3);
        assert!(diff.iter().any(|file| {
            file.relative_path == Path::new("sample.rs")
                && file.target == GitDiffTarget::Staged
                && file.status == 'M'
        }));
        assert!(diff.iter().any(|file| {
            file.relative_path == Path::new("sample.rs")
                && file.target == GitDiffTarget::Unstaged
                && file.status == 'M'
        }));
        assert!(diff.iter().any(|file| {
            file.relative_path == Path::new("new file.txt")
                && file.target == GitDiffTarget::Unstaged
                && file.status == '?'
        }));
    }

    #[test]
    fn structured_worktree_files_skip_rename_source_record() {
        let diff = parse_structured_worktree_files(b"R  new.rs\0old.rs\0 M tracked.rs\0").unwrap();
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].relative_path, Path::new("new.rs"));
        assert_eq!(
            diff[0].base_relative_path.as_deref(),
            Some(Path::new("old.rs"))
        );
        assert_eq!(diff[0].target, GitDiffTarget::Staged);
        assert_eq!(diff[1].relative_path, Path::new("tracked.rs"));
        assert_eq!(diff[1].target, GitDiffTarget::Unstaged);
    }

    #[test]
    fn structured_staged_rename_uses_the_original_head_blob() {
        let temp = committed_repo();
        git(temp.path(), &["mv", "sample.rs", "renamed.rs"]);

        let diff =
            fetch_structured_file_diff(temp.path(), "renamed.rs", GitDiffTarget::Staged).unwrap();
        assert_eq!(diff.base_relative_path, Path::new("sample.rs"));
        assert_eq!(diff.old_text, "fn old() {}\n");
        assert_eq!(diff.new_text, "fn old() {}\n");
    }

    #[test]
    fn commit_diff_is_a_structured_read_only_multi_file_review() {
        let temp = committed_repo();
        std::fs::write(temp.path().join("sample.rs"), "fn newer() {}\n").unwrap();
        git(temp.path(), &["add", "sample.rs"]);
        git(
            temp.path(),
            &["commit", "--quiet", "-m", "structured commit"],
        );

        let review = fetch_commit_diff(temp.path(), "HEAD").unwrap();
        assert_eq!(review.title, "structured commit");
        assert!(review.read_only);
        assert_eq!(review.files.len(), 1);
        assert_eq!(review.files[0].change.relative_path, Path::new("sample.rs"));
        let file = review.files[0].diff.as_ref().unwrap();
        assert_eq!(file.old_text, "fn old() {}\n");
        assert_eq!(file.new_text, "fn newer() {}\n");
    }
}

/// Unified diff for one file. `staged` selects the index-vs-HEAD diff
/// (`git diff --cached`) instead of the working-tree-vs-index diff.
pub fn fetch_file_diff(cwd: &Path, rel_path: &str, staged: bool) -> Result<String, String> {
    let root = crate::git_status::repo_root(cwd);
    let mut args: Vec<&str> = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(rel_path);
    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(&args)
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git diff failed: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Load an explicit Git comparison pair and compute display hunks without
/// parsing terminal-oriented patch output.
pub fn fetch_structured_file_diff(
    cwd: &Path,
    rel_path: &str,
    target: GitDiffTarget,
) -> Result<GitFileDiff, String> {
    let root = crate::git_status::repo_root(cwd);
    let path = PathBuf::from(rel_path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Git diff path must be repository-relative".to_string());
    }
    let base_path = resolve_diff_base_path(&root, &path, target).unwrap_or_else(|| path.clone());
    let base_spec_path = base_path.to_string_lossy();
    let (old_bytes, new_bytes) = match target {
        GitDiffTarget::Unstaged => (
            read_git_blob(&root, &format!(":{base_spec_path}"))?,
            read_worktree_file(&root, &path)?,
        ),
        GitDiffTarget::Staged => (
            read_git_blob(&root, &format!("HEAD:{base_spec_path}"))?,
            read_git_blob(&root, &format!(":{rel_path}"))?,
        ),
    };
    Ok(structured_diff_from_bytes(
        path, base_path, target, old_bytes, new_bytes,
    ))
}

fn resolve_diff_base_path(
    root: &Path,
    relative_path: &Path,
    target: GitDiffTarget,
) -> Option<PathBuf> {
    let mut command = crate::git_cmd::GitCommand::new();
    command.no_window().arg("diff");
    if target == GitDiffTarget::Staged {
        command.arg("--cached");
    }
    // Rename detection needs both the source and destination in the candidate
    // set. Restricting the pathspec to the destination makes Git report an
    // addition and loses the original blob identity.
    let output = command
        .args(["--name-status", "--find-renames", "-z"])
        .current_dir(root)
        .output()
        .ok()?
        .stdout;
    let fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut index = 0;
    while index < fields.len() {
        let field = String::from_utf8_lossy(fields[index]);
        let mut status_and_path = field.splitn(2, '\t');
        let status = status_and_path.next().unwrap_or_default();
        let inline_old = status_and_path.next();
        if status.starts_with('R') || status.starts_with('C') {
            let (old, new, consumed) = if let Some(old) = inline_old {
                (old.as_bytes(), *fields.get(index + 1)?, 2)
            } else {
                (*fields.get(index + 1)?, *fields.get(index + 2)?, 3)
            };
            if Path::new(std::str::from_utf8(new).ok()?) == relative_path {
                return Some(PathBuf::from(std::str::from_utf8(old).ok()?));
            }
            index += consumed;
        } else {
            index += 1;
        }
    }
    None
}

fn structured_diff_from_bytes(
    path: PathBuf,
    base_path: PathBuf,
    target: GitDiffTarget,
    old_bytes: Vec<u8>,
    new_bytes: Vec<u8>,
) -> GitFileDiff {
    let binary = old_bytes.contains(&0)
        || new_bytes.contains(&0)
        || std::str::from_utf8(&old_bytes).is_err()
        || std::str::from_utf8(&new_bytes).is_err();
    // Git blobs conventionally contain LF while Windows working-tree files
    // may contain CRLF because of core.autocrlf. Comparing the raw forms makes
    // every line look changed and can turn the quadratic diff calculation
    // into minutes or hours of work for otherwise ordinary source files.
    let old_text = normalize_diff_line_endings(String::from_utf8(old_bytes).unwrap_or_default());
    let new_text = normalize_diff_line_endings(String::from_utf8(new_bytes).unwrap_or_default());
    let diff = if binary {
        dragent_document::DocumentDiff::default()
    } else {
        dragent_document::DocumentDiff::between(&old_text, &new_text)
    };
    let full_diff = if binary {
        dragent_document::DocumentDiff::default()
    } else {
        let context_lines = old_text.lines().count().max(new_text.lines().count());
        dragent_document::DocumentDiff::between_with_context(&old_text, &new_text, context_lines)
    };
    let (old_highlight_states, new_highlight_states) = if binary {
        (Vec::new(), Vec::new())
    } else {
        let language = dragent_editor::Language::from_path(&path);
        (
            dragent_editor::LexicalHighlighter::line_start_states(language, &old_text),
            dragent_editor::LexicalHighlighter::line_start_states(language, &new_text),
        )
    };
    let grammar = (!binary)
        .then(|| {
            dragent_editor::grammar_for(Some(&path), dragent_editor::Language::from_path(&path))
        })
        .flatten();
    let syntax = |text: &str| {
        grammar
            .and_then(|grammar| dragent_editor::highlight_document(grammar, text))
            .map(std::sync::Arc::new)
    };
    let (old_syntax, new_syntax) = (syntax(&old_text), syntax(&new_text));
    let mut additions = 0;
    let mut deletions = 0;
    let mut max_changed_line_chars = 0;
    for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
        match line.kind {
            dragent_document::DiffLineKind::Added => additions += 1,
            dragent_document::DiffLineKind::Removed => deletions += 1,
            dragent_document::DiffLineKind::Context => {}
        }
        max_changed_line_chars = max_changed_line_chars.max(line.text.chars().count());
    }
    GitFileDiff {
        relative_path: path,
        base_relative_path: base_path,
        target,
        old_text,
        new_text,
        diff,
        full_diff,
        old_highlight_states,
        new_highlight_states,
        old_syntax,
        new_syntax,
        additions,
        deletions,
        max_changed_line_chars,
        binary,
    }
}

fn normalize_diff_line_endings(text: String) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

fn read_git_blob(root: &Path, spec: &str) -> Result<Vec<u8>, String> {
    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["show", spec])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git show failed: {error}"))?;
    // A missing side is expected for added/deleted files and unborn HEADs.
    Ok(if output.status.success() {
        output.stdout
    } else {
        Vec::new()
    })
}

fn read_worktree_file(root: &Path, relative_path: &Path) -> Result<Vec<u8>, String> {
    let path = root.join(relative_path);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

pub fn fetch_structured_worktree_files(cwd: &Path) -> Result<GitMultiDiff, String> {
    let started = std::time::Instant::now();
    let root = crate::git_status::repo_root(cwd);
    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["status", "--porcelain", "-z", "--untracked-files=all"])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git status failed: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let changes = parse_structured_worktree_files(&output.stdout)?;
    let status_ms = started.elapsed().as_millis();
    let files = load_worktree_diffs(&root, changes)?;
    log::info!("[git-multi-diff] prepared {} files in {}ms (status {}ms)",
        files.len(), started.elapsed().as_millis(), status_ms);
    Ok(GitMultiDiff {
        title: "Uncommitted Changes".to_string(),
        subtitle: Some("HEAD, index, and working tree".to_string()),
        read_only: false,
        files,
    })
}

fn load_worktree_diff(
    root: &Path,
    blobs: &std::collections::HashMap<String, Vec<u8>>,
    change: GitChangedFile,
) -> GitMultiDiffFile {
    // `git status` already resolved rename/copy source paths. Reusing that
    // metadata avoids running a repository-wide `git diff --find-renames`
    // once for every file in the review.
    let base_path = change
        .base_relative_path
        .clone()
        .unwrap_or_else(|| change.relative_path.clone());
    let base_spec_path = base_path.to_string_lossy().into_owned();
    let path = &change.relative_path;
    let diff = (|| {
        let (old_bytes, new_bytes) = match change.target {
            GitDiffTarget::Unstaged => (
                blobs
                    .get(&format!(":{base_spec_path}"))
                    .cloned()
                    .unwrap_or_default(),
                read_worktree_file(root, path)?,
            ),
            GitDiffTarget::Staged => (
                blobs
                    .get(&format!("HEAD:{base_spec_path}"))
                    .cloned()
                    .unwrap_or_default(),
                blobs
                    .get(&format!(":{}", path.to_string_lossy()))
                    .cloned()
                    .unwrap_or_default(),
            ),
        };
        Ok(structured_diff_from_bytes(
            path.clone(),
            base_path,
            change.target,
            old_bytes,
            new_bytes,
        ))
    })();
    GitMultiDiffFile { change, diff }
}

fn worktree_blob_specs(changes: &[GitChangedFile]) -> Vec<String> {
    let mut specs = Vec::new();
    for change in changes {
        let base = change
            .base_relative_path
            .as_ref()
            .unwrap_or(&change.relative_path)
            .to_string_lossy();
        match change.target {
            GitDiffTarget::Unstaged => specs.push(format!(":{base}")),
            GitDiffTarget::Staged => {
                specs.push(format!("HEAD:{base}"));
                specs.push(format!(":{}", change.relative_path.to_string_lossy()));
            }
        }
    }
    specs.sort();
    specs.dedup();
    specs
}

fn read_git_blobs(
    root: &Path,
    specs: &[String],
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    if specs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    // Bound each IPC response rather than accumulating every repository blob
    // in a single host frame. If a batch exceeds the host transport limit,
    // retry smaller groups; one individually oversized blob remains an error.
    if specs.len() > 16 {
        let mut blobs = std::collections::HashMap::with_capacity(specs.len());
        for chunk in specs.chunks(16) {
            blobs.extend(read_git_blobs(root, chunk)?);
        }
        return Ok(blobs);
    }
    // One host request/process per batch. Keep newline-containing
    // paths on the single-object path so older Git versions remain supported.
    let (ordinary, unusual): (Vec<_>, Vec<_>) = specs.iter()
        .partition(|spec| !spec.contains('\n') && !spec.contains('\r'));
    let mut blobs = std::collections::HashMap::with_capacity(specs.len());
    if !ordinary.is_empty() {
        let mut input = Vec::new();
        for spec in &ordinary {
            input.extend_from_slice(spec.as_bytes());
            input.push(b'\n');
        }
        let output = crate::git_cmd::GitCommand::new()
            .args(["cat-file", "--batch"])
            .current_dir(root)
            .output_with_stdin(&input);
        let output = match output {
            Ok(output) => output,
            Err(_) if specs.len() > 1 => {
                let middle = specs.len() / 2;
                let mut split = read_git_blobs(root, &specs[..middle])?;
                split.extend(read_git_blobs(root, &specs[middle..])?);
                return Ok(split);
            }
            Err(error) => return Err(format!("git cat-file batch failed: {error}")),
        };
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        let mut remaining = output.stdout.as_slice();
        for spec in ordinary {
            let end = remaining.iter().position(|byte| *byte == b'\n')
                .ok_or_else(|| "Incomplete Git blob header".to_string())?;
            let header = std::str::from_utf8(&remaining[..end])
                .map_err(|_| "Invalid Git blob header".to_string())?;
            remaining = &remaining[end + 1..];
            if header.ends_with(" missing") {
                blobs.insert(spec.clone(), Vec::new());
                continue;
            }
            let size = header.rsplit(' ').next()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| "Invalid Git blob size".to_string())?;
            if remaining.get(size) != Some(&b'\n') {
                return Err("Incomplete Git blob body".to_string());
            }
            blobs.insert(spec.clone(), remaining[..size].to_vec());
            remaining = &remaining[size + 1..];
        }
    }
    for spec in unusual {
        blobs.insert(spec.clone(), read_git_blob(root, spec)?);
    }
    Ok(blobs)
}

#[cfg(not(target_family = "wasm"))]
fn load_worktree_diffs(
    root: &Path,
    changes: Vec<GitChangedFile>,
) -> Result<Vec<GitMultiDiffFile>, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    if changes.len() <= 1 {
        let blobs = read_git_blobs(root, &worktree_blob_specs(&changes))?;
        return Ok(changes
            .into_iter()
            .map(|change| load_worktree_diff(root, &blobs, change))
            .collect());
    }
    let blobs = read_git_blobs(root, &worktree_blob_specs(&changes))?;
    let worker_count = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(2, 8)
        .min(changes.len());
    let next = AtomicUsize::new(0);
    let results = Mutex::new(vec![None; changes.len()]);
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(change) = changes.get(index).cloned() else {
                    break;
                };
                results.lock().unwrap()[index] = Some(load_worktree_diff(root, &blobs, change));
            });
        }
    });
    Ok(results
        .into_inner()
        .unwrap()
        .into_iter()
        .flatten()
        .collect())
}

#[cfg(target_family = "wasm")]
fn load_worktree_diffs(
    root: &Path,
    changes: Vec<GitChangedFile>,
) -> Result<Vec<GitMultiDiffFile>, String> {
    let blobs = read_git_blobs(root, &worktree_blob_specs(&changes))?;
    Ok(changes
        .into_iter()
        .map(|change| load_worktree_diff(root, &blobs, change))
        .collect())
}

fn parse_structured_worktree_files(output: &[u8]) -> Result<Vec<GitChangedFile>, String> {
    let records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() < 4 || record[2] != b' ' {
            index += 1;
            continue;
        }
        let staged = record[0] as char;
        let unstaged = record[1] as char;
        let path = String::from_utf8(record[3..].to_vec())
            .map_err(|_| "Git path is not valid UTF-8".to_string())?;
        let renamed = matches!(staged, 'R' | 'C') || matches!(unstaged, 'R' | 'C');
        let base_relative_path = renamed
            .then(|| records.get(index + 1))
            .flatten()
            .and_then(|record| String::from_utf8(record.to_vec()).ok())
            .map(PathBuf::from);
        if unstaged != ' ' || (staged == '?' && unstaged == '?') {
            files.push(GitChangedFile {
                relative_path: PathBuf::from(&path),
                base_relative_path: base_relative_path.clone(),
                target: GitDiffTarget::Unstaged,
                status: unstaged,
            });
        }
        if staged != ' ' && staged != '?' {
            files.push(GitChangedFile {
                relative_path: PathBuf::from(path),
                base_relative_path,
                target: GitDiffTarget::Staged,
                status: staged,
            });
        }
        index += 1;
        if renamed {
            // Porcelain v1 -z emits the original rename/copy path as the next
            // NUL record. The first record contains the destination we open.
            index += 1;
        }
    }
    // Identical duplicate rows (same path, target, and status — git can emit
    // these for untracked paths) collapse to one so a file never appears
    // twice in the same section.
    files.dedup_by(|a, b| {
        a.relative_path == b.relative_path && a.target == b.target && a.status == b.status
    });
    Ok(files)
}

/// Read-only structured multi-file comparison for one historical commit.
pub fn fetch_commit_diff(cwd: &Path, sha: &str) -> Result<GitMultiDiff, String> {
    let root = crate::git_status::repo_root(cwd);
    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .args([
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            sha,
        ])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git diff-tree failed: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let metadata = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["show", "-s", "--format=%s%x1f%an%x1f%ae%x1f%ar", sha])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git show failed: {error}"))?;
    if !metadata.status.success() {
        return Err(String::from_utf8_lossy(&metadata.stderr).trim().to_string());
    }
    let metadata = String::from_utf8_lossy(&metadata.stdout);
    let mut fields = metadata.trim().split('\u{1f}');
    let title = fields.next().unwrap_or("Commit").to_string();
    let author = fields.next().unwrap_or("Unknown author");
    let email = fields.next().unwrap_or("");
    let time = fields.next().unwrap_or("");
    let subtitle = Some(format!("{author}  •  {time}  •  {email}"));
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let columns = line.split('\t').collect::<Vec<_>>();
        let Some(status) = columns.first().and_then(|value| value.chars().next()) else {
            continue;
        };
        let Some(path) = columns.last().filter(|path| !path.is_empty()) else {
            continue;
        };
        let base_path = if matches!(status, 'R' | 'C') && columns.len() >= 3 {
            columns[1]
        } else {
            path
        };
        let old_bytes = read_git_blob(&root, &format!("{sha}^:{base_path}"))?;
        let new_bytes = read_git_blob(&root, &format!("{sha}:{path}"))?;
        let change = GitChangedFile {
            relative_path: PathBuf::from(path),
            base_relative_path: (base_path != *path).then(|| PathBuf::from(base_path)),
            target: GitDiffTarget::Staged,
            status,
        };
        files.push(GitMultiDiffFile {
            diff: Ok(structured_diff_from_bytes(
                change.relative_path.clone(),
                PathBuf::from(base_path),
                change.target,
                old_bytes,
                new_bytes,
            )),
            change,
        });
    }
    Ok(GitMultiDiff {
        title,
        subtitle,
        read_only: true,
        files,
    })
}
