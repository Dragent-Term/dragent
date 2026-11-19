//! Review PR/ship query service for the workspace.
//!
//! Responsible for:
//! - querying local-only git state for the Review "ship" menu (`ReviewShipState`)
//! - querying `gh` for PR summary state, checks, and comments
//! - parsing `gh` JSON output into `project_sidebar` Review types
//! - per-project tab-switch cache records (`CachedReviewPr`)
//!
//! Not responsible for:
//! - rendering the Review UI (lives in `project_sidebar` + `WorkspaceView`)
//! - polling cadence / scheduling (owned by `WorkspaceView`)
//! - mutating sidebar state (the caller applies `QueriedReviewPr` results)
//!
//! All subprocess calls are local-only (no fetch, no network beyond what
//! `gh` itself does) and never prompt (`GIT_TERMINAL_PROMPT=0`).

use std::path::PathBuf;

use super::project_sidebar::{
    find_git_root, ReviewCheck, ReviewCheckStatus, ReviewComment, ReviewPullRequest,
};
use crate::process_ext::NoWindow;

/// Ship state behind the navbar Review control
/// logic): live worktree stats plus commits-ahead against the PR base
/// branch, refreshed on a 10s poll. No Commit face — committing lives in
/// the Source Control tab — so the menu only ever offers Push / Create PR.
#[derive(Clone, Default)]
pub(crate) struct ReviewShipState {
    pub project_root: Option<PathBuf>,
    pub in_git_repo: bool,
    pub branch: String,
    pub dirty_files: usize,
    pub additions: usize,
    pub deletions: usize,
    pub has_upstream: bool,
    /// Commits the remote lacks (push count).
    pub ahead: usize,
    pub base_branch: Option<String>,
    /// Commits the base lacks (PR gating: GitHub rejects empty PRs).
    pub commits_ahead: usize,
}

impl ReviewShipState {
    pub fn needs_push(&self) -> bool {
        self.in_git_repo && (!self.has_upstream || self.ahead > 0)
    }

    pub fn shows_stats(&self) -> bool {
        self.in_git_repo && self.dirty_files > 0
    }

    pub fn can_create_pr(&self) -> bool {
        self.project_root.is_some() && self.in_git_repo
    }

    /// None when Create PR may run; otherwise the greyed-out reason.
    pub fn create_pr_block_reason(&self) -> Option<String> {
        if !self.has_upstream {
            return Some("Push the branch first".to_string());
        }
        match &self.base_branch {
            None => Some("Could not determine the base branch".to_string()),
            Some(base) if self.commits_ahead == 0 => Some(format!("No commits ahead of {base}")),
            _ => None,
        }
    }
}

/// Query worktree + ahead/behind state with local-only git commands (no
/// fetch, no network): safe to run on a background poll. Never prompts —
/// `GIT_TERMINAL_PROMPT=0` guards credential lookups on odd configs.
pub(crate) fn query_review_ship_state(root: Option<PathBuf>) -> ReviewShipState {
    let Some(root) = root.filter(|root| root.is_dir()) else {
        return ReviewShipState::default();
    };
    fn git(root: &PathBuf, args: &[&str]) -> Option<String> {
        let output = crate::git_cmd::GitCommand::new()
            .args(args)
            .current_dir(root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .no_window()
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    if git(&root, &["rev-parse", "--show-toplevel"]).is_none() {
        // A project that is not a git checkout: Create PR and Push stay
        // hidden, exactly like a session without a project.
        return ReviewShipState {
            project_root: Some(root),
            ..ReviewShipState::default()
        };
    }
    // Independent queries run on scoped threads (one subprocess each):
    // wall time drops from their sum to the slowest one. The borrow of
    // `root` is sound — the scope joins everything before returning.
    let (branch, status, numstat, base) = std::thread::scope(|scope| {
        let branch = scope.spawn(|| git(&root, &["branch", "--show-current"]).unwrap_or_default());
        // One status call yields dirty entries, upstream presence, and
        // ahead/behind counts (`# branch.ab +a -b`).
        let status = scope.spawn(|| git(&root, &["status", "--porcelain=v2", "--branch"]));
        let numstat = scope.spawn(|| git(&root, &["diff", "HEAD", "--numstat"]));
        // PR base: origin's default branch, falling back to common names,
        // then the upstream branch. Commits-ahead gates Create PR.
        let base = scope.spawn(|| {
            git(&root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
                .and_then(|reference| reference.strip_prefix("refs/remotes/").map(str::to_string))
                .or_else(|| {
                    ["origin/main", "origin/master"]
                        .into_iter()
                        .find_map(|name| {
                            git(&root, &["rev-parse", "--verify", name]).map(|_| name.to_string())
                        })
                })
                .or_else(|| {
                    git(
                        &root,
                        &[
                            "rev-parse",
                            "--abbrev-ref",
                            "--symbolic-full-name",
                            "@{upstream}",
                        ],
                    )
                })
        });
        (
            branch.join().unwrap_or_default(),
            status.join().ok().flatten(),
            numstat.join().ok().flatten(),
            base.join().ok().flatten(),
        )
    });
    let mut state = ReviewShipState {
        project_root: Some(root.clone()),
        in_git_repo: true,
        branch,
        ..ReviewShipState::default()
    };

    if let Some(status) = status {
        let mut dirty = 0;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("# branch.upstream ") {
                if !rest.trim().is_empty() {
                    state.has_upstream = true;
                }
            } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
                let mut parts = rest.split_whitespace();
                state.ahead = parts
                    .next()
                    .and_then(|a| a.strip_prefix('+'))
                    .and_then(|a| a.parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with('#') || line.trim().is_empty() {
                // Header/comment lines carry no worktree changes.
            } else {
                dirty += 1;
            }
        }
        state.dirty_files = dirty;
    }

    // Worktree additions/deletions vs HEAD (staged + unstaged).
    if let Some(numstat) = numstat {
        for line in numstat.lines() {
            let mut parts = line.split_whitespace();
            let added: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let deleted: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            state.additions += added;
            state.deletions += deleted;
        }
    }

    if let Some(base) = base {
        state.commits_ahead = git(&root, &["rev-list", "--count", &format!("{base}..HEAD")])
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
        state.base_branch = Some(base);
    }
    state
}

pub(crate) struct QueriedReviewPr {
    pub pr: Option<ReviewPullRequest>,
    /// None keeps the previously loaded comments (comments fetch failed
    /// or was skipped this tick).
    pub comments: Option<Vec<ReviewComment>>,
    pub error: Option<String>,
}

/// Last-known Review state per project root. Tab switches paint from here
/// instantly and revalidate in the background, so moving between tabs never
/// flashes a loading screen for already-seen data.
#[derive(Clone, Default)]
pub(crate) struct CachedReviewPr {
    pub branch: String,
    pub pr: Option<ReviewPullRequest>,
    pub comments: Vec<ReviewComment>,
    pub error: Option<String>,
}

fn gh_text(root: &PathBuf, args: &[&str]) -> Result<String, String> {
    if let Some(remote) = crate::remote_fs::resolve(root) {
        if args.get(..2) != Some(&["pr", "view"]) {
            return Err("Remote review comments are not available through this request.".into());
        }
        let client = crate::remote_fs::client(&remote.machine_id)?;
        return match client.development(dragent_host::development::DevelopmentRequest::PullRequest { root: remote.path })? {
            dragent_host::development::DevelopmentResponse::Text(text) => Ok(text),
            _ => Err("The remote runtime did not return pull request details.".into()),
        };
    }
    let output = crate::process_ext::gh_command()
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PAGER", "")
        .no_window()
        .output()
        .map_err(|error| format!("Could not start gh: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn map_check_status(status: Option<&str>, conclusion: Option<&str>) -> ReviewCheckStatus {
    match conclusion {
        Some("SUCCESS") => ReviewCheckStatus::Success,
        Some("FAILURE") => ReviewCheckStatus::Failure,
        Some("CANCELLED") => ReviewCheckStatus::Cancelled,
        Some("SKIPPED") | Some("NEUTRAL") => ReviewCheckStatus::Skipped,
        _ => match status {
            Some("IN_PROGRESS") | Some("QUEUED") | Some("PENDING") | Some("REQUESTED")
            | Some("WAITING") => ReviewCheckStatus::Pending,
            Some("COMPLETED") => ReviewCheckStatus::Unknown,
            _ => ReviewCheckStatus::Unknown,
        },
    }
}

pub(crate) fn query_review_pr_state(root: Option<PathBuf>, want_comments: bool) -> QueriedReviewPr {
    let Some(root) = root
        .filter(|root| crate::remote_fs::is_remote(root) || root.is_dir())
        .and_then(|root| find_git_root(&root))
    else {
        return QueriedReviewPr {
            pr: None,
            comments: None,
            error: None,
        };
    };
    let raw = match gh_text(
        &root,
        &[
            "pr",
            "view",
            "--json",
            "number,title,url,state,isDraft,reviewDecision,baseRefName,headRefName,\
             statusCheckRollup",
        ],
    ) {
        Ok(raw) => raw,
        Err(stderr) => {
            // No PR on this branch is the normal empty state, not an error.
            if stderr.contains("no pull request") || stderr.contains("no pull requests") {
                return QueriedReviewPr {
                    pr: None,
                    comments: None,
                    error: None,
                };
            }
            return QueriedReviewPr {
                pr: None,
                comments: None,
                error: Some(stderr.chars().take(140).collect()),
            };
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => {
            return QueriedReviewPr {
                pr: None,
                comments: None,
                error: Some("Could not parse gh output.".to_string()),
            };
        }
    };

    let state = json_str(&value, "state").unwrap_or_default();
    let is_draft = value
        .get("isDraft")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let checks: Vec<ReviewCheck> = value
        .get("statusCheckRollup")
        .and_then(|value| value.as_array())
        .map(|rollup| {
            rollup
                .iter()
                .map(|check| {
                    let status = check.get("status").and_then(|s| s.as_str());
                    let conclusion = check.get("conclusion").and_then(|s| s.as_str());
                    ReviewCheck {
                        name: json_str(check, "name").unwrap_or_else(|| "check".to_string()),
                        status: map_check_status(status, conclusion),
                        url: json_str(check, "detailsUrl"),
                        duration_text: match (
                            json_str(check, "startedAt"),
                            json_str(check, "completedAt"),
                        ) {
                            (Some(start), Some(end)) => duration_text(&start, &end),
                            _ => None,
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let pr = ReviewPullRequest {
        number: value.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
        title: json_str(&value, "title").unwrap_or_default(),
        url: json_str(&value, "url").unwrap_or_default(),
        state: if is_draft {
            "draft".to_string()
        } else {
            match state.as_str() {
                "OPEN" => "open".to_string(),
                "CLOSED" => "closed".to_string(),
                "MERGED" => "merged".to_string(),
                _ => "open".to_string(),
            }
        },
        review_decision: match json_str(&value, "reviewDecision").as_deref() {
            Some("APPROVED") => "approved".to_string(),
            Some("CHANGES_REQUESTED") => "changes_requested".to_string(),
            _ => "pending".to_string(),
        },
        checks,
        base_branch: json_str(&value, "baseRefName").unwrap_or_default(),
        head_branch: json_str(&value, "headRefName").unwrap_or_default(),
    };

    if !want_comments {
        return QueriedReviewPr {
            pr: Some(pr),
            comments: None,
            error: None,
        };
    }

    let comments = query_pr_comments(&root, pr.number).unwrap_or_default();
    QueriedReviewPr {
        pr: Some(pr),
        comments: Some(comments),
        error: None,
    }
}

/// Review threads + conversation comments for a PR number. Failures yield
/// an empty list (the summary still renders; threads retry next tick).
fn query_pr_comments(root: &PathBuf, number: u64) -> Option<Vec<ReviewComment>> {
    let repo = gh_text(root, &["repo", "view", "--json", "nameWithOwner"])
        .ok()
        .and_then(|raw| {
            serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|value| json_str(&value, "nameWithOwner"))
        })?;
    let (owner, repo) = repo.split_once('/')?;
    let query = r#"query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 50) {
        nodes {
          id isResolved
          comments(first: 10) {
            nodes { id author { login avatarUrl } body path line url createdAt }
          }
        }
      }
      comments(first: 50) {
        nodes { id author { login avatarUrl } body url createdAt }
      }
    }
  }
}"#;
    let raw = gh_text(
        root,
        &[
            "api",
            "graphql",
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("repo={repo}"),
            "-F",
            &format!("number={number}"),
            "-f",
            &format!("query={query}"),
        ],
    )
    .ok()?;
    let data: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(parse_pr_comments(&data))
}

fn parse_pr_comments(data: &serde_json::Value) -> Vec<ReviewComment> {
    let pr = data
        .pointer("/data/repository/pullRequest")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let mut comments = Vec::new();
    if let Some(threads) = pr
        .pointer("/reviewThreads/nodes")
        .and_then(|value| value.as_array())
    {
        for thread in threads {
            let thread_id = json_str(thread, "id");
            let is_resolved = thread
                .get("isResolved")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let first = thread
                .pointer("/comments/nodes")
                .and_then(|value| value.as_array())
                .and_then(|nodes| nodes.first());
            let Some(first) = first else { continue };
            let author = first
                .pointer("/author/login")
                .and_then(|value| value.as_str())
                .unwrap_or("ghost")
                .to_string();
            let path = json_str(first, "path");
            comments.push(ReviewComment {
                id: thread_id.clone().unwrap_or_else(|| {
                    json_str(first, "id")
                        .unwrap_or_else(|| format!("{}-{}", author, comments.len()))
                }),
                thread_id,
                author,
                avatar_url: first
                    .pointer("/author/avatarUrl")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                body: json_str(first, "body").unwrap_or_default(),
                url: json_str(first, "url"),
                path: path.clone(),
                line: first
                    .get("line")
                    .and_then(|value| value.as_u64())
                    .map(|line| line as u32),
                is_resolved,
                created_at: json_str(first, "createdAt").unwrap_or_default(),
                kind: if path.is_some() {
                    "review".to_string()
                } else {
                    "comment".to_string()
                },
            });
        }
    }
    if let Some(nodes) = pr
        .pointer("/comments/nodes")
        .and_then(|value| value.as_array())
    {
        for node in nodes {
            comments.push(ReviewComment {
                id: json_str(node, "id").unwrap_or_else(|| format!("issue-{}", comments.len())),
                thread_id: None,
                author: node
                    .pointer("/author/login")
                    .and_then(|value| value.as_str())
                    .unwrap_or("ghost")
                    .to_string(),
                avatar_url: node
                    .pointer("/author/avatarUrl")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                body: json_str(node, "body").unwrap_or_default(),
                url: json_str(node, "url"),
                path: None,
                line: None,
                is_resolved: false,
                created_at: json_str(node, "createdAt").unwrap_or_default(),
                kind: "comment".to_string(),
            });
        }
    }
    comments
}

fn duration_text(start: &str, end: &str) -> Option<String> {
    let seconds = chrono::DateTime::parse_from_rfc3339(end)
        .ok()?
        .signed_duration_since(chrono::DateTime::parse_from_rfc3339(start).ok()?)
        .num_seconds();
    if seconds < 0 {
        return None;
    }
    if seconds < 60 {
        Some(format!("{seconds}s"))
    } else {
        Some(format!("{}m", seconds / 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_ship_parses_dirty_worktree() {
        let root = std::env::temp_dir().join(format!("dragent-review-ship-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
        };
        git(&["init"]).unwrap();
        git(&["config", "user.email", "test@dragent.dev"]).unwrap();
        git(&["config", "user.name", "dragent-test"]).unwrap();
        std::fs::write(root.join("file.txt"), "one\n").unwrap();
        git(&["add", "."]).unwrap();
        git(&["commit", "-m", "initial"]).unwrap();
        // Unstaged modification: dirty, one addition, no upstream.
        std::fs::write(root.join("file.txt"), "one\ntwo\n").unwrap();

        let state = query_review_ship_state(Some(root.clone()));
        assert!(state.in_git_repo);
        assert!(!state.branch.is_empty());
        assert_eq!(state.dirty_files, 1);
        assert_eq!(state.additions, 1);
        assert_eq!(state.deletions, 0);
        assert!(!state.has_upstream);
        assert!(state.needs_push());
        assert!(state.create_pr_block_reason().is_some());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn review_ship_rejects_non_repos() {
        let missing = std::env::temp_dir().join("dragent-review-ship-missing");
        let _ = std::fs::remove_dir_all(&missing);
        let state = query_review_ship_state(Some(missing));
        assert!(!state.in_git_repo);
        assert!(!state.needs_push());
        assert!(!state.can_create_pr());

        let plain =
            std::env::temp_dir().join(format!("dragent-review-ship-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).unwrap();
        let state = query_review_ship_state(Some(plain.clone()));
        assert!(!state.in_git_repo);
        assert!(!state.needs_push());
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[test]
    fn review_comments_parse_avatars_and_positions() {
        let data: serde_json::Value = serde_json::from_str(
            r#"{"data":{"repository":{"pullRequest":{
                "reviewThreads":{"nodes":[{
                    "id":"thread-1","isResolved":false,
                    "comments":{"nodes":[{
                        "id":"c1",
                        "author":{"login":"octocat","avatarUrl":"https://example.com/a.png"},
                        "body":"nice","path":"src/main.rs","line":42,
                        "url":"https://example.com/c1","createdAt":"2026-09-20T10:00:00Z"}]}}]},
                "comments":{"nodes":[{
                    "id":"c2","author":{"login":"ghost","avatarUrl":null},
                    "body":"general","url":null,"createdAt":"2026-09-20T11:00:00Z"}]}}}}}"#,
        )
        .unwrap();
        let comments = parse_pr_comments(&data);
        assert_eq!(comments.len(), 2);
        assert_eq!(
            comments[0].avatar_url.as_deref(),
            Some("https://example.com/a.png")
        );
        assert_eq!(comments[0].path.as_deref(), Some("src/main.rs"));
        assert_eq!(comments[0].line, Some(42));
        assert_eq!(comments[0].kind, "review");
        assert!(comments[1].avatar_url.is_none());
        assert_eq!(comments[1].kind, "comment");
    }
}
