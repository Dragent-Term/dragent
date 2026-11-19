//! P1 scaffold: hunk/line-level staging + conflict-state grouping.
//!
//! Behind `git-hunk-stage` flag. Reuses `diff_head::GitFileDiff` + `fetch_structured_file_diff`
//! to generate patch hunks, then applies via `git apply --cached` rather than whole-file `git add`.
//! Conflict states (U/AA/DD/AU/UA/DU/UD) are surfaced as a dedicated group (VS Code Merge section).

use crate::process_ext::NoWindow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    Unmerged,      // U
    BothAdded,     // AA
    BothDeleted,   // DD
    AddedByUs,     // AU
    AddedByThem,   // UA
    DeletedByUs,   // DU
    DeletedByThem, // UD
}

pub fn classify_conflict(index: char, work: char) -> Option<ConflictKind> {
    match (index, work) {
        ('U', 'U') => Some(ConflictKind::Unmerged),
        ('A', 'A') => Some(ConflictKind::BothAdded),
        ('D', 'D') => Some(ConflictKind::BothDeleted),
        ('A', 'U') => Some(ConflictKind::AddedByUs),
        ('U', 'A') => Some(ConflictKind::AddedByThem),
        ('D', 'U') => Some(ConflictKind::DeletedByUs),
        ('U', 'D') => Some(ConflictKind::DeletedByThem),
        _ if index == 'U' || work == 'U' => Some(ConflictKind::Unmerged),
        _ => None,
    }
}

/// One hunk that can be staged independently via `git apply --cached`.
#[derive(Clone, Debug)]
pub struct StageableHunk {
    pub header: String, // @@ -a,b +c,d @@
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<HunkLine>,
    /// File prelude plus exactly one hunk, ready for `git apply`.
    patch: String,
}

#[derive(Clone, Debug)]
pub struct HunkLine {
    pub kind: HunkLineKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkLineKind {
    Context,
    Added,
    Removed,
}

/// Extract stageable hunks for `rel_path`/`target` using existing structured diff.
/// Off-thread caller: `ctx.spawn(async { hunk_stage::stageable_hunks(...) })`.
pub fn stageable_hunks(cwd: &Path, rel_path: &str, staged: bool) -> Vec<StageableHunk> {
    stageable_hunks_result(cwd, rel_path, staged).unwrap_or_default()
}

fn stageable_hunks_result(
    cwd: &Path,
    rel_path: &str,
    staged: bool,
) -> Result<Vec<StageableHunk>, String> {
    validate_relative_path(rel_path)?;
    let root = crate::git_status::repo_root(cwd);
    let mut command = crate::git_cmd::GitCommand::new();
    command
        .no_window()
        .args(["diff", "--no-color", "--no-ext-diff", "--unified=3"]);
    if staged {
        command.arg("--cached");
    }
    let output = command
        .args(["--", rel_path])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git diff failed: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut patch = String::from_utf8_lossy(&output.stdout).into_owned();

    // `git diff` omits untracked files. Ask Git to produce its canonical
    // /dev/null patch so new files can still be staged by hunk.
    if patch.is_empty() && !staged && !is_tracked(&root, rel_path) && root.join(rel_path).is_file()
    {
        let output = crate::git_cmd::GitCommand::new()
            .no_window()
            .args([
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--unified=3",
                "--no-index",
                "--",
                "/dev/null",
                rel_path,
            ])
            .current_dir(&root)
            .output()
            .map_err(|error| format!("git diff for new file failed: {error}"))?;
        // `--no-index` returns 1 when differences were found.
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        patch = String::from_utf8_lossy(&output.stdout).into_owned();
    }

    Ok(split_file_patch_into_hunks(&patch))
}

fn is_tracked(root: &Path, rel_path: &str) -> bool {
    crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["ls-files", "--error-unmatch", "--", rel_path])
        .current_dir(root)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn validate_relative_path(rel_path: &str) -> Result<(), String> {
    let path = PathBuf::from(rel_path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Git hunk path must be repository-relative".to_string());
    }
    Ok(())
}

fn split_file_patch_into_hunks(patch: &str) -> Vec<StageableHunk> {
    let lines = patch.lines().collect::<Vec<_>>();
    let Some(first_hunk) = lines.iter().position(|line| line.starts_with("@@ ")) else {
        return Vec::new();
    };
    let mut prelude = lines[..first_hunk].join("\n");
    prelude.push('\n');
    let mut starts = lines
        .iter()
        .enumerate()
        .skip(first_hunk)
        .filter_map(|(index, line)| line.starts_with("@@ ").then_some(index))
        .collect::<Vec<_>>();
    starts.push(lines.len());

    starts
        .windows(2)
        .map(|window| {
            let start = window[0];
            let end = window[1];
            let body = &lines[start + 1..end];
            let parsed_lines = body
                .iter()
                .filter_map(|line| {
                    let (kind, text) = match line.as_bytes().first().copied() {
                        Some(b'+') => (HunkLineKind::Added, &line[1..]),
                        Some(b'-') => (HunkLineKind::Removed, &line[1..]),
                        Some(b' ') => (HunkLineKind::Context, &line[1..]),
                        _ => return None,
                    };
                    Some(HunkLine {
                        kind,
                        text: text.to_string(),
                    })
                })
                .collect();
            let mut one_hunk = prelude.clone();
            one_hunk.push_str(&lines[start..end].join("\n"));
            one_hunk.push('\n');
            let (old_start, new_start) = parse_hunk_starts(lines[start]).unwrap_or_default();
            StageableHunk {
                header: lines[start].to_string(),
                old_start,
                new_start,
                lines: parsed_lines,
                patch: one_hunk,
            }
        })
        .collect()
}

fn parse_hunk_starts(header: &str) -> Option<(usize, usize)> {
    let mut fields = header.split_whitespace();
    (fields.next()? == "@@").then_some(())?;
    let old_start = fields
        .next()?
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new_start = fields
        .next()?
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old_start, new_start))
}

/// Apply an INDEX → WORKTREE hunk to the index.
pub fn stage_hunk(cwd: &Path, _rel_path: &str, hunk: &StageableHunk) -> Result<(), String> {
    apply_patch(cwd, hunk, true, false)
}

/// Remove a HEAD → INDEX hunk from the index without touching the worktree.
pub fn unstage_hunk(cwd: &Path, _rel_path: &str, hunk: &StageableHunk) -> Result<(), String> {
    apply_patch(cwd, hunk, true, true)
}

/// Restore one INDEX → WORKTREE hunk from the index. The caller must obtain
/// destructive-action confirmation first.
pub fn discard_hunk(cwd: &Path, rel_path: &str, hunk_index: usize) -> Result<(), String> {
    discard_hunk_matching(cwd, rel_path, hunk_index, None)
}

pub fn discard_hunk_matching(
    cwd: &Path,
    rel_path: &str,
    hunk_index: usize,
    expected_starts: Option<(usize, usize)>,
) -> Result<(), String> {
    let hunks = stageable_hunks_result(cwd, rel_path, false)?;
    let hunk = expected_starts
        .and_then(|(old_start, new_start)| {
            hunks
                .iter()
                .find(|hunk| hunk.old_start == old_start && hunk.new_start == new_start)
        })
        .or_else(|| hunks.get(hunk_index))
        .ok_or_else(|| "Git hunk no longer exists".to_string())?;
    apply_patch(cwd, hunk, false, true)
}

fn apply_patch(
    cwd: &Path,
    hunk: &StageableHunk,
    cached: bool,
    reverse: bool,
) -> Result<(), String> {
    let root = crate::git_status::repo_root(cwd);
    let mut command = crate::git_cmd::GitCommand::new();
    command.no_window().arg("apply");
    if cached {
        command.arg("--cached");
    }
    if reverse {
        command.arg("--reverse");
    }
    command
        .arg("--whitespace=nowarn")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output_with_stdin(hunk.patch.as_bytes())
        .map_err(|error| format!("git apply failed: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("git apply exited with {}", output.status)
        } else {
            stderr
        })
    }
}

/// Range selection is not exposed by the editor yet. Keep this explicit so a
/// future caller cannot accidentally stage an entire hunk instead.
pub fn stage_lines(
    _cwd: &Path,
    _rel_path: &str,
    _hunk_index: usize,
    _line_range: std::ops::Range<usize>,
) -> Result<(), String> {
    Err("line-range staging is not available yet; stage the hunk instead".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
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
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn repo_with_two_hunks() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "--quiet"]);
        git(temp.path(), &["config", "user.name", "Hunk Test"]);
        git(temp.path(), &["config", "user.email", "hunk@example.test"]);
        let original = (1..=30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(temp.path().join("sample.txt"), original).unwrap();
        git(temp.path(), &["add", "sample.txt"]);
        git(temp.path(), &["commit", "--quiet", "-m", "initial"]);
        let changed = (1..=30)
            .map(|line| match line {
                3 => "changed three\n".to_string(),
                27 => "changed twenty seven\n".to_string(),
                _ => format!("line {line}\n"),
            })
            .collect::<String>();
        std::fs::write(temp.path().join("sample.txt"), changed).unwrap();
        temp
    }

    #[test]
    fn conflict_classification() {
        assert_eq!(classify_conflict('U', 'U'), Some(ConflictKind::Unmerged));
        assert_eq!(classify_conflict('A', 'A'), Some(ConflictKind::BothAdded));
        assert_eq!(classify_conflict('M', 'M'), None);
    }

    #[test]
    fn stages_and_unstages_only_the_selected_hunk() {
        let repo = repo_with_two_hunks();
        let hunks = stageable_hunks(repo.path(), "sample.txt", false);
        assert_eq!(hunks.len(), 2);
        stage_hunk(repo.path(), "sample.txt", &hunks[0]).unwrap();
        let staged = git(repo.path(), &["diff", "--cached", "--", "sample.txt"]);
        let working = git(repo.path(), &["diff", "--", "sample.txt"]);
        assert!(staged.contains("changed three"));
        assert!(!staged.contains("changed twenty seven"));
        assert!(working.contains("changed twenty seven"));

        let staged_hunk = stageable_hunks(repo.path(), "sample.txt", true).remove(0);
        unstage_hunk(repo.path(), "sample.txt", &staged_hunk).unwrap();
        assert!(git(repo.path(), &["diff", "--cached"]).is_empty());
        assert_eq!(stageable_hunks(repo.path(), "sample.txt", false).len(), 2);
    }

    #[test]
    fn discards_only_the_selected_worktree_hunk() {
        let repo = repo_with_two_hunks();
        discard_hunk(repo.path(), "sample.txt", 0).unwrap();
        let working = git(repo.path(), &["diff", "--", "sample.txt"]);
        assert!(!working.contains("changed three"));
        assert!(working.contains("changed twenty seven"));
    }

    #[test]
    fn stages_an_untracked_file_as_an_added_hunk() {
        let repo = repo_with_two_hunks();
        std::fs::write(repo.path().join("new.txt"), "first\nsecond\n").unwrap();

        let hunks = stageable_hunks(repo.path(), "new.txt", false);
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0]
            .lines
            .iter()
            .all(|line| line.kind == HunkLineKind::Added));

        stage_hunk(repo.path(), "new.txt", &hunks[0]).unwrap();
        let staged = git(repo.path(), &["diff", "--cached", "--", "new.txt"]);
        assert!(staged.contains("new file mode"));
        assert!(staged.contains("+first"));
        assert!(staged.contains("+second"));
    }

    #[test]
    fn rejects_paths_that_escape_the_repository() {
        let repo = repo_with_two_hunks();
        assert!(stageable_hunks_result(repo.path(), "../outside", false).is_err());
    }
}
