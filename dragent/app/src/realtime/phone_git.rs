//! A workspace's source control, for a phone.
//!
//! The same picture the right sidebar gives on the desktop: what is staged,
//! what isn't, how many lines each file gained and lost, and the diff of one
//! file; the branch's history; and the actions the person takes from there:
//! stage, unstage, discard, commit, push, switch branch and open a pull
//! request. Writes answer `{done, message}` so the phone can show git's own
//! words when something is refused.
//!
//! Git runs through [`crate::git_cmd::GitCommand`] like every other git call
//! in the app. Callers run these off the companion host's socket thread and
//! have already checked that the root is one the phone may read.

use serde_json::{json, Value};
use std::collections::HashMap;

/// Most changed files listed per section.
pub const MAX_FILES: usize = 500;
/// Largest diff sent for one file; longer diffs are cut and marked.
pub const MAX_DIFF_BYTES: usize = 256 * 1024;
/// An untracked file's lines are counted only below this size.
const MAX_COUNTED_BYTES: u64 = 512 * 1024;

fn git(root: &str, args: &[&str]) -> Result<std::process::Output, &'static str> {
    let mut command = crate::git_cmd::GitCommand::new();
    command.current_dir(root).args(args);
    command.output().map_err(|_| "unavailable")
}

/// A path inside the repository as the phone may name it: relative, no
/// traversal, no NUL, nothing that git would read as an option.
pub fn check_relative(path: &str) -> Result<&str, &'static str> {
    if path.is_empty() || path.len() > 4096 || path.starts_with('/') || path.starts_with('-') || path.contains('\0') {
        return Err("invalid path");
    }
    if path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("invalid path");
    }
    Ok(path)
}

fn status_word(code: char) -> &'static str {
    match code {
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'T' => "type_changed",
        'U' => "conflicted",
        _ => "modified",
    }
}

/// `--numstat -z` output as path -> (added, removed). Binary files have none.
fn numstat(output: &[u8]) -> HashMap<String, (Option<u64>, Option<u64>)> {
    let text = String::from_utf8_lossy(output);
    let mut fields = text.split('\0');
    let mut counts = HashMap::new();
    while let Some(record) = fields.next() {
        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        // A rename has an empty path here, then the old and new names.
        let path = if path.is_empty() {
            let _old = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        if !path.is_empty() {
            counts.insert(path, (added.parse().ok(), removed.parse().ok()));
        }
    }
    counts
}

fn untracked_lines(root: &str, path: &str) -> Option<u64> {
    let full = std::path::Path::new(root).join(path);
    let meta = std::fs::symlink_metadata(&full).ok()?;
    if !meta.is_file() || meta.len() > MAX_COUNTED_BYTES {
        return None;
    }
    let bytes = std::fs::read(&full).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let lines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    Some(if bytes.last().is_some_and(|&b| b != b'\n') { lines + 1 } else { lines })
}

/// The branch line of `status --porcelain=v1 --branch`:
/// `## main...origin/main [ahead 1, behind 2]`.
fn branch_line(line: &str) -> (Option<String>, Option<String>, u64, u64) {
    let rest = line.trim_start_matches("## ");
    if let Some(name) = rest.strip_prefix("No commits yet on ") {
        return (Some(name.trim().to_string()), None, 0, 0);
    }
    if rest.starts_with("HEAD (no branch)") {
        return (None, None, 0, 0);
    }
    let (names, counts) = match rest.split_once(" [") {
        Some((names, counts)) => (names, counts.trim_end_matches(']')),
        None => (rest, ""),
    };
    let (branch, upstream) = match names.split_once("...") {
        Some((branch, upstream)) => (branch, Some(upstream.to_string())),
        None => (names, None),
    };
    let count = |word: &str| -> u64 {
        counts
            .split(", ")
            .find_map(|part| part.strip_prefix(word))
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0)
    };
    (Some(branch.to_string()), upstream, count("ahead "), count("behind "))
}

/// Parsed `status --porcelain=v1 -z --branch`: the branch line and, per file,
/// (index code, worktree code, path, old path of a rename).
fn parse_status(output: &[u8]) -> (Option<String>, Vec<(char, char, String, Option<String>)>) {
    let text = String::from_utf8_lossy(output);
    let mut fields = text.split('\0');
    let mut branch = None;
    let mut files = Vec::new();
    while let Some(record) = fields.next() {
        if record.starts_with("## ") {
            branch = Some(record.to_string());
            continue;
        }
        let mut chars = record.chars();
        let (Some(x), Some(y), Some(' ')) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        let path = chars.as_str().to_string();
        if path.is_empty() {
            continue;
        }
        // Renames and copies carry the old name as the next field.
        let old = (x == 'R' || x == 'C' || y == 'R' || y == 'C')
            .then(|| fields.next().map(str::to_string))
            .flatten();
        files.push((x, y, path, old));
    }
    (branch, files)
}

/// `git.status`: branch, and the staged and unstaged changes of the
/// repository at `root` with per-file line counts.
pub fn status(root: &str) -> Result<Value, &'static str> {
    let out = git(root, &["status", "--porcelain=v1", "-z", "--branch", "--untracked-files=all"])?;
    if !out.status.success() {
        // Not a repository (or git refused): say so, it is not an error.
        return Ok(json!({"repository": false, "branch": null, "upstream": null, "ahead": 0, "behind": 0,
            "staged": [], "unstaged": [], "truncated": false}));
    }
    let (branch, files) = parse_status(&out.stdout);
    let (branch, upstream, ahead, behind) = branch.as_deref().map(branch_line).unwrap_or((None, None, 0, 0));
    let unstaged_counts = git(root, &["diff", "--numstat", "-z"]).map(|o| numstat(&o.stdout)).unwrap_or_default();
    let staged_counts = git(root, &["diff", "--cached", "--numstat", "-z"])
        .map(|o| numstat(&o.stdout))
        .unwrap_or_default();
    let row = |path: &str, old: &Option<String>, status: &str, counts: Option<&(Option<u64>, Option<u64>)>| {
        json!({"path": path, "old_path": old, "status": status,
            "added": counts.and_then(|c| c.0), "removed": counts.and_then(|c| c.1)})
    };
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut truncated = false;
    for (x, y, path, old) in &files {
        if staged.len() >= MAX_FILES || unstaged.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        if *x == '?' {
            let lines = untracked_lines(root, path);
            unstaged.push(row(path, &None, "untracked", Some(&(lines, Some(0)))));
            continue;
        }
        if *x == '!' {
            continue;
        }
        let conflicted = *x == 'U' || *y == 'U' || (*x == 'A' && *y == 'A') || (*x == 'D' && *y == 'D');
        if conflicted {
            unstaged.push(row(path, &None, "conflicted", unstaged_counts.get(path)));
            continue;
        }
        if *x != ' ' {
            staged.push(row(path, old, status_word(*x), staged_counts.get(path)));
        }
        if *y != ' ' {
            unstaged.push(row(path, &None, status_word(*y), unstaged_counts.get(path)));
        }
    }
    Ok(json!({"repository": true, "branch": branch, "upstream": upstream, "ahead": ahead, "behind": behind,
        "staged": staged, "unstaged": unstaged, "truncated": truncated}))
}

/// Which change of a file the phone wants to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
    Untracked,
}

impl Side {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "staged" => Some(Self::Staged),
            "unstaged" => Some(Self::Unstaged),
            "untracked" => Some(Self::Untracked),
            _ => None,
        }
    }
}

/// `git.diff`: the unified diff of one file, cut at [`MAX_DIFF_BYTES`].
pub fn diff(root: &str, file: &str, side: Side) -> Result<Value, &'static str> {
    let file = check_relative(file)?;
    let base = ["diff", "--no-color", "--no-ext-diff", "-U3"];
    let out = match side {
        Side::Staged => git(root, &[&base[..], &["--cached", "--", file]].concat())?,
        Side::Unstaged => git(root, &[&base[..], &["--", file]].concat())?,
        Side::Untracked => {
            // Never follow a symlink out of the repository.
            let full = std::path::Path::new(root).join(file);
            let meta = std::fs::symlink_metadata(&full).map_err(|_| "unavailable")?;
            if !meta.is_file() {
                return Err("unavailable");
            }
            git(root, &[&base[..], &["--no-index", "--", "/dev/null", file]].concat())?
        }
    };
    // `--no-index` exits 1 when the files differ: that is the answer.
    let ok = out.status.success() || (side == Side::Untracked && out.status.code() == Some(1));
    if !ok {
        return Err("unavailable");
    }
    let binary = out.stdout.windows(13).any(|w| w == b"Binary files ") || out.stdout.contains(&0);
    let (bytes, truncated) = if out.stdout.len() > MAX_DIFF_BYTES {
        (&out.stdout[..MAX_DIFF_BYTES], true)
    } else {
        (&out.stdout[..], false)
    };
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if truncated {
        // Finish on a whole line.
        if let Some(end) = text.rfind('\n') {
            text.truncate(end + 1);
        }
    }
    Ok(json!({"diff": if binary { String::new() } else { text }, "binary": binary, "truncated": truncated}))
}

// ---- Actions -------------------------------------------------------------

/// Most files one action names.
pub const MAX_ACTION_FILES: usize = 500;
/// Longest commit message, pull request title and body accepted.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// Git with prompts off: a push that needs credentials fails, never hangs.
fn git_quiet(root: &str, args: &[&str]) -> Result<std::process::Output, &'static str> {
    let mut command = crate::git_cmd::GitCommand::new();
    command
        .current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "");
    command.output().map_err(|_| "unavailable")
}

/// What an action answered: done, or why not, in git's words (cut short).
fn outcome(out: &std::process::Output) -> Value {
    if out.status.success() {
        return json!({"done": true, "message": null});
    }
    let mut text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    if text.chars().count() > 2000 {
        text = text.chars().take(2000).collect::<String>() + "…";
    }
    json!({"done": false, "message": if text.is_empty() { "Git refused that.".to_string() } else { text }})
}

/// Checked file names for an action: 1–[`MAX_ACTION_FILES`], each relative.
pub fn check_files(files: &[String]) -> Result<Vec<String>, &'static str> {
    if files.is_empty() || files.len() > MAX_ACTION_FILES {
        return Err("invalid");
    }
    files.iter().map(|f| check_relative(f).map(str::to_string)).collect()
}

/// A branch name as the phone may give it: what `git check-ref-format` takes,
/// and never an option.
pub fn check_branch(name: &str) -> Result<&str, &'static str> {
    if name.is_empty() || name.len() > 255 || name.starts_with('-') || name.contains('\0') {
        return Err("invalid");
    }
    let ok = std::process::Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok { Ok(name) } else { Err("invalid") }
}

/// `git.stage`: add these files' changes (new, changed or deleted).
pub fn stage(root: &str, files: &[String]) -> Result<Value, &'static str> {
    let files = check_files(files)?;
    let mut args = vec!["add", "-A", "--"];
    args.extend(files.iter().map(String::as_str));
    Ok(outcome(&git_quiet(root, &args)?))
}

/// `git.unstage`: take these files out of the next commit (their changes stay).
pub fn unstage(root: &str, files: &[String]) -> Result<Value, &'static str> {
    let files = check_files(files)?;
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(files.iter().map(String::as_str));
    let out = git_quiet(root, &args)?;
    if out.status.success() {
        return Ok(outcome(&out));
    }
    // No commit yet: there is nothing to restore from; drop them from the index.
    let mut args = vec!["rm", "-q", "--cached", "-r", "--"];
    args.extend(files.iter().map(String::as_str));
    Ok(outcome(&git_quiet(root, &args)?))
}

/// `git.discard`: throw away these files' unstaged changes; an untracked file
/// is deleted. Staged changes are kept.
pub fn discard(root: &str, files: &[String], untracked: bool) -> Result<Value, &'static str> {
    let files = check_files(files)?;
    let mut args = if untracked { vec!["clean", "-f", "-q", "--"] } else { vec!["restore", "--worktree", "--"] };
    args.extend(files.iter().map(String::as_str));
    Ok(outcome(&git_quiet(root, &args)?))
}

/// `git.commit`: commit what is staged, with the person's message. Hooks run
/// as on the desktop.
pub fn commit(root: &str, message: &str) -> Result<Value, &'static str> {
    let message = message.trim();
    if message.is_empty() || message.len() > MAX_MESSAGE || message.contains('\0') {
        return Err("invalid");
    }
    let out = git_quiet(root, &["commit", "-q", "-m", message])?;
    let mut answer = outcome(&out);
    if out.status.success() {
        let head = git(root, &["log", "-1", "--format=%h%x1f%s"]).ok();
        if let Some(head) = head.filter(|o| o.status.success()) {
            let line = String::from_utf8_lossy(&head.stdout).trim().to_string();
            let mut parts = line.splitn(2, '\u{1f}');
            answer["commit"] = json!({"hash": parts.next(), "subject": parts.next()});
        }
    }
    Ok(answer)
}

/// `git.push`: push the branch; one without an upstream is published to
/// `origin` under its own name.
pub fn push(root: &str) -> Result<Value, &'static str> {
    let upstream = git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    let args: &[&str] = if upstream { &["push", "-q"] } else { &["push", "-q", "-u", "origin", "HEAD"] };
    Ok(outcome(&git_quiet(root, args)?))
}

/// `git.log`: the branch's latest commits.
pub fn log(root: &str, limit: usize) -> Result<Value, &'static str> {
    let limit = limit.clamp(1, 200).to_string();
    let out = git(root, &["log", "-n", &limit, "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s%x1e"])?;
    if !out.status.success() {
        return Ok(json!({"commits": []}));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let commits: Vec<Value> = text
        .split('\u{1e}')
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let f: Vec<&str> = record.split('\u{1f}').collect();
            (f.len() == 5).then(|| json!({"hash": f[0], "short": f[1], "author": f[2],
                "at": f[3].parse::<i64>().unwrap_or(0), "subject": f[4]}))
        })
        .collect();
    Ok(json!({"commits": commits}))
}

/// `git.branches`: local branches, the current one marked.
pub fn branches(root: &str) -> Result<Value, &'static str> {
    let out = git(root, &["for-each-ref", "--sort=-committerdate", "--format=%(HEAD)%(refname:short)", "refs/heads"])?;
    if !out.status.success() {
        return Ok(json!({"branches": [], "current": null}));
    }
    let mut current = None;
    let mut list = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let (head, name) = line.split_at(1.min(line.len()));
        if name.is_empty() {
            continue;
        }
        if head == "*" {
            current = Some(name.to_string());
        }
        list.push(name.to_string());
        if list.len() >= 200 {
            break;
        }
    }
    Ok(json!({"branches": list, "current": current}))
}

/// `git.switch`: change to another local branch (git refuses when that would
/// lose changes, and says so).
pub fn switch(root: &str, branch: &str) -> Result<Value, &'static str> {
    let branch = check_branch(branch)?;
    Ok(outcome(&git_quiet(root, &["switch", "-q", branch])?))
}

/// `git.pr_create`: open a pull request for the branch with GitHub's CLI
/// (the person's own login). Answers its address.
pub fn create_pr(root: &str, title: &str, body: &str, draft: bool) -> Result<Value, &'static str> {
    let title = title.trim();
    if title.is_empty() || title.len() > 1024 || body.len() > MAX_MESSAGE || title.contains('\0') || body.contains('\0') {
        return Err("invalid");
    }
    let mut args = vec!["pr", "create", "--title", title, "--body", body];
    if draft {
        args.push("--draft");
    }
    let out = crate::process_ext::gh_command()
        .args(&args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "")
        .output()
        .map_err(|_| "unavailable")?;
    let mut answer = outcome(&out);
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        let url = text.split_whitespace().find(|word| word.starts_with("https://")).map(str::to_string);
        answer["url"] = json!(url);
    }
    Ok(answer)
}

/// The branch's pull request and its checks, from `gh pr view` in `root`:
/// `{pr: null, reason?}` when there is none (or gh can't say), else
/// `{pr: {number, title, url, state, draft, review, mergeable, head, base,
/// checks: [{name, workflow, state, url}]}}`. GitHub only.
pub fn pr_status(root: &str) -> Result<Value, &'static str> {
    let out = crate::process_ext::gh_command()
        .args([
            "pr",
            "view",
            "--json",
            "number,title,url,state,isDraft,reviewDecision,mergeable,headRefName,baseRefName,statusCheckRollup",
        ])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "")
        .output()
        .map_err(|_| "unavailable")?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).to_lowercase();
        let reason = if text.contains("no pull requests found") || text.contains("no open pull requests") {
            "This branch has no pull request yet."
        } else if text.contains("auth") || text.contains("gh auth login") {
            "The GitHub CLI on the desktop isn't signed in (gh auth login)."
        } else if text.contains("not a git repository") || text.contains("none of the git remotes") {
            "This workspace isn't a GitHub repository."
        } else {
            "The desktop couldn't read this branch's pull request."
        };
        return Ok(json!({"pr": null, "reason": reason}));
    }
    let value: Value = serde_json::from_slice(&out.stdout).map_err(|_| "unavailable")?;
    Ok(json!({"pr": pull_request(&value)}))
}

/// `gh pr view --json …` as the phone shows it.
fn pull_request(value: &Value) -> Value {
    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let checks: Vec<Value> = value
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .map(|list| list.iter().take(200).map(check).collect())
        .unwrap_or_default();
    json!({
        "number": value.get("number").and_then(Value::as_u64),
        "title": text("title"),
        "url": text("url"),
        "state": text("state").to_lowercase(),
        "draft": value.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        "review": text("reviewDecision").to_lowercase(),
        "mergeable": text("mergeable").to_lowercase(),
        "head": text("headRefName"),
        "base": text("baseRefName"),
        "checks": checks,
    })
}

/// One check run or commit status as `passed`, `failed`, `running`,
/// `pending`, `skipped`, `cancelled` or `neutral`.
fn check(item: &Value) -> Value {
    let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default().to_uppercase();
    let original = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let (name, url, state) = if item.get("context").is_some() {
        // A commit status.
        let state = match text("state").as_str() {
            "SUCCESS" => "passed",
            "FAILURE" | "ERROR" => "failed",
            "PENDING" | "EXPECTED" => "pending",
            _ => "neutral",
        };
        (original("context"), original("targetUrl"), state)
    } else {
        let state = match (text("status").as_str(), text("conclusion").as_str()) {
            ("COMPLETED", "SUCCESS") => "passed",
            ("COMPLETED", "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED") => "failed",
            ("COMPLETED", "SKIPPED") => "skipped",
            ("COMPLETED", "CANCELLED") => "cancelled",
            ("COMPLETED", _) => "neutral",
            ("IN_PROGRESS", _) => "running",
            _ => "pending",
        };
        (original("name"), original("detailsUrl"), state)
    };
    json!({"name": name, "workflow": original("workflowName"), "state": state, "url": url})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_requests_and_their_checks_read_plainly() {
        let pr = pull_request(&serde_json::json!({
            "number": 42, "title": "Fix login", "url": "https://github.com/o/r/pull/42", "state": "OPEN",
            "isDraft": false, "reviewDecision": "REVIEW_REQUIRED", "mergeable": "MERGEABLE",
            "headRefName": "fix-login", "baseRefName": "main",
            "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "test", "workflowName": "CI", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://ci/1"},
                {"__typename": "CheckRun", "name": "lint", "workflowName": "CI", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "https://ci/2"},
                {"__typename": "CheckRun", "name": "build", "workflowName": "CI", "status": "IN_PROGRESS", "conclusion": "", "detailsUrl": ""},
                {"__typename": "CheckRun", "name": "deploy", "status": "QUEUED"},
                {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED"},
                {"__typename": "StatusContext", "context": "vercel", "state": "SUCCESS", "targetUrl": "https://v/1"}
            ]
        }));
        assert_eq!(pr["number"], 42);
        assert_eq!(pr["state"], "open");
        assert_eq!(pr["review"], "review_required");
        assert_eq!(pr["mergeable"], "mergeable");
        let states: Vec<&str> = pr["checks"].as_array().unwrap().iter().map(|c| c["state"].as_str().unwrap()).collect();
        assert_eq!(states, ["passed", "failed", "running", "pending", "skipped", "passed"]);
        assert_eq!(pr["checks"][5]["name"], "vercel");
        assert_eq!(pr["checks"][1]["url"], "https://ci/2");
    }

    fn repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-phone-git-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.invalid"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("kept.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn status_lists_staged_and_unstaged_changes_with_line_counts() {
        let dir = repo("status");
        let root = dir.to_string_lossy().to_string();
        std::fs::write(dir.join("kept.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("new file.txt"), "a\nb\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        assert!(std::process::Command::new("git").current_dir(&dir).args(["add", "staged.txt"]).status().unwrap().success());
        let v = status(&root).unwrap();
        assert_eq!(v["repository"], true);
        assert_eq!(v["branch"], "main");
        let staged = v["staged"].as_array().unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!((staged[0]["path"].as_str(), staged[0]["status"].as_str(), staged[0]["added"].as_u64()), (Some("staged.txt"), Some("added"), Some(1)));
        let find = |path: &str| v["unstaged"].as_array().unwrap().iter().find(|f| f["path"] == path).cloned().unwrap();
        assert_eq!((find("kept.txt")["status"].as_str(), find("kept.txt")["added"].as_u64()), (Some("modified"), Some(1)));
        assert_eq!(find("gone.txt")["status"], "deleted");
        assert_eq!((find("new file.txt")["status"].as_str(), find("new file.txt")["added"].as_u64()), (Some("untracked"), Some(2)));
        // The diff of each kind of change.
        let d = diff(&root, "kept.txt", Side::Unstaged).unwrap();
        assert!(d["diff"].as_str().unwrap().contains("+three"));
        assert_eq!(d["truncated"], false);
        let d = diff(&root, "staged.txt", Side::Staged).unwrap();
        assert!(d["diff"].as_str().unwrap().contains("+s"));
        let d = diff(&root, "new file.txt", Side::Untracked).unwrap();
        assert!(d["diff"].as_str().unwrap().contains("+a\n+b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_unstage_discard_and_commit_from_the_phone() {
        let dir = repo("actions");
        let root = dir.to_string_lossy().to_string();
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        std::fs::write(dir.join("kept.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        std::fs::write(dir.join("junk.txt"), "j\n").unwrap();

        assert_eq!(stage(&root, &names(&["kept.txt", "new.txt"])).unwrap()["done"], true);
        let v = status(&root).unwrap();
        assert_eq!(v["staged"].as_array().unwrap().len(), 2);
        assert_eq!(unstage(&root, &names(&["new.txt"])).unwrap()["done"], true);
        assert_eq!(status(&root).unwrap()["staged"].as_array().unwrap().len(), 1);

        // Discard: an untracked file is deleted; a tracked change reverts.
        assert_eq!(discard(&root, &names(&["junk.txt"]), true).unwrap()["done"], true);
        assert!(!dir.join("junk.txt").exists());

        let done = commit(&root, "Count to three").unwrap();
        assert_eq!(done["done"], true);
        assert_eq!(done["commit"]["subject"], "Count to three");
        let nothing = commit(&root, "Again").unwrap();
        assert_eq!(nothing["done"], false, "nothing staged: git says so");
        assert!(nothing["message"].as_str().unwrap().len() > 0);
        assert!(commit(&root, "   ").is_err());

        let history = log(&root, 10).unwrap();
        let subjects: Vec<&str> = history["commits"].as_array().unwrap().iter().map(|c| c["subject"].as_str().unwrap()).collect();
        assert_eq!(subjects, vec!["Count to three", "first"]);

        // Branches and switching.
        assert!(std::process::Command::new("git").current_dir(&dir).args(["branch", "side"]).status().unwrap().success());
        let b = branches(&root).unwrap();
        assert_eq!(b["current"], "main");
        assert!(b["branches"].as_array().unwrap().iter().any(|n| n == "side"));
        assert_eq!(switch(&root, "side").unwrap()["done"], true);
        assert_eq!(branches(&root).unwrap()["current"], "side");
        assert_eq!(switch(&root, "no-such-branch").unwrap()["done"], false);
        assert!(switch(&root, "--orphan").is_err(), "never an option");

        // A push with nowhere to go fails with git's words, without hanging.
        let pushed = push(&root).unwrap();
        assert_eq!(pushed["done"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn actions_check_their_files() {
        assert!(check_files(&[]).is_err());
        assert!(check_files(&["../x".to_string()]).is_err());
        assert!(check_files(&["-rf".to_string()]).is_err());
        assert_eq!(check_files(&["a.rs".to_string()]).unwrap(), vec!["a.rs".to_string()]);
    }

    #[test]
    fn a_folder_that_is_not_a_repository_says_so() {
        let dir = std::env::temp_dir().join(format!("dragent-phone-git-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A parent repository must not answer for it.
        std::fs::write(dir.join(".git"), "not a repository").unwrap();
        let v = status(&dir.to_string_lossy()).unwrap();
        assert_eq!(v["repository"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_names_cannot_leave_the_repository_or_pass_options() {
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "-p", "--output=x", "a/./b"] {
            assert!(check_relative(bad).is_err(), "{bad}");
        }
        assert_eq!(check_relative("src/a b.rs"), Ok("src/a b.rs"));
    }

    #[test]
    fn the_branch_line_gives_upstream_and_distance() {
        assert_eq!(
            branch_line("## main...origin/main [ahead 2, behind 1]"),
            (Some("main".into()), Some("origin/main".into()), 2, 1)
        );
        assert_eq!(branch_line("## feature"), (Some("feature".into()), None, 0, 0));
        assert_eq!(branch_line("## No commits yet on main"), (Some("main".into()), None, 0, 0));
        assert_eq!(branch_line("## HEAD (no branch)"), (None, None, 0, 0));
    }

    #[test]
    fn renames_in_numstat_keep_the_new_name() {
        let counts = numstat(b"3\t1\tplain.rs\0-\t-\tpic.png\x002\t0\t\0old.rs\0new.rs\0");
        assert_eq!(counts["plain.rs"], (Some(3), Some(1)));
        assert_eq!(counts["pic.png"], (None, None));
        assert_eq!(counts["new.rs"], (Some(2), Some(0)));
    }
}
