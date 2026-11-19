//! Typed GitHub and GitLab review operations shared by local and SSH screens.
#[cfg(not(target_family = "wasm"))]
mod gitlab;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

pub const SUMMARY_FIELDS: &str = "number,title,url,state,isDraft,author,baseRefName,headRefName,headRefOid,updatedAt,createdAt,additions,deletions,changedFiles,reviewDecision,statusCheckRollup,labels,assignees,reviewRequests";
// Large collections have dedicated, paginated operations. Keep the first
// overview response small instead of downloading those collections twice.
pub const DETAIL_FIELDS: &str = "number,id,title,body,url,state,isDraft,author,baseRefName,headRefName,headRefOid,updatedAt,createdAt,additions,deletions,changedFiles,reviewDecision,statusCheckRollup,labels,assignees,reviewRequests,mergeable,mergeStateStatus,autoMergeRequest,maintainerCanModify";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub root: String,
    pub operation: Operation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Operation {
    List {
        state: String,
        search: String,
        limit: u32,
    },
    Detail {
        number: u64,
    },
    Files {
        number: u64,
    },
    Threads {
        number: u64,
    },
    Commit {
        sha: String,
    },
    Diff {
        number: u64,
    },
    Create {
        title: String,
        body: String,
        base: String,
        draft: bool,
        /// Push `source` to the remote first.
        push: bool,
        /// The branch with the changes, any branch the person chose.
        #[serde(rename = "expected_branch")]
        source: String,
    },
    Edit {
        number: u64,
        title: String,
        body: String,
        base: String,
    },
    Metadata {
        number: u64,
        field: MetadataField,
        value: String,
        remove: bool,
    },
    Comment {
        number: u64,
        body: String,
    },
    Review {
        number: u64,
        head: String,
        kind: ReviewKind,
        body: String,
        comments: Vec<LineComment>,
    },
    Reply {
        number: u64,
        comment: u64,
        body: String,
    },
    Resolve {
        thread: String,
        resolved: bool,
    },
    SetState {
        number: u64,
        open: bool,
    },
    Draft {
        number: u64,
        draft: bool,
    },
    Merge {
        number: u64,
        head: String,
        method: MergeMethod,
        auto: bool,
    },
    DisableAutoMerge {
        number: u64,
    },
    UpdateBranch {
        number: u64,
        head: String,
    },
    Rerun {
        run: u64,
        failed_only: bool,
    },
    Logs {
        run: u64,
    },
    OverrideMerge {
        number: u64,
        head: String,
        method: MergeMethod,
        acknowledgement: String,
    },
    Blob {
        sha: String,
    },
    Branches,
    Repository,
    Commits {
        number: u64,
    },
    Conversation {
        number: u64,
    },
    Reviews {
        number: u64,
    },
    PrepareWorkspace {
        number: u64,
        head: String,
    },
    /// What a pull request from `source` into `base` would hold, worked out
    /// from the checkout (see [`compare`]).
    Compare {
        base: String,
        source: String,
    },
    /// The description templates the team keeps on its provider, read on
    /// `base` (the default branch): see [`github_templates`].
    Templates {
        base: String,
    },
    /// The commits and diff `source` adds over `base`, for writing its
    /// description (see [`changes`]).
    Changes {
        base: String,
        source: String,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MetadataField {
    Reviewer,
    Assignee,
    Label,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ReviewKind {
    Comment,
    Approve,
    RequestChanges,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineComment {
    pub path: String,
    pub line: u32,
    pub side: String,
    pub body: String,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub start_side: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Plan {
    args: Vec<String>,
    input: Option<String>,
}
fn plan(op: &Operation) -> Result<Plan, String> {
    use Operation::*;
    if serde_json::to_vec(op).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("The request is larger than 1 MB".into());
    }
    let mut a: Vec<String> = vec![];
    let mut input = None;
    macro_rules! args { ($($x:expr),* $(,)?) => { a=vec![$($x.to_string()),*] } }
    let api = |path: String, method: &str, body: Value| Plan {
        args: vec![
            "api".into(),
            path,
            "--method".into(),
            method.into(),
            "--input".into(),
            "-".into(),
        ],
        input: Some(body.to_string()),
    };
    match op {
        List {
            state,
            search,
            limit,
        } => {
            if !["open", "closed", "merged", "all"].contains(&state.as_str()) {
                return Err("Invalid PR state".into());
            }
            args![
                "pr",
                "list",
                "--state",
                state,
                "--limit",
                limit.clamp(&1, &500),
                "--json",
                SUMMARY_FIELDS
            ];
            if !search.trim().is_empty() {
                a.push(format!("--search={search}"));
            }
        }
        Detail { number } => args!["pr", "view", number, "--json", DETAIL_FIELDS],
        Files { number } => args![
            "api",
            format!("repos/{{owner}}/{{repo}}/pulls/{number}/files?per_page=100"),
            "--paginate",
            "--slurp"
        ],
        Commit { sha } => {
            valid_sha(sha)?;
            args![
                "api",
                format!("repos/{{owner}}/{{repo}}/commits/{sha}"),
                "--paginate",
                "--slurp"
            ]
        }
        Diff { number } => args!["pr", "diff", number, "--color", "never"],
        Threads { number } => {
            let query = r#"query($owner:String!,$name:String!,$number:Int!,$endCursor:String){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100,after:$endCursor){nodes{id isResolved isOutdated path line startLine diffSide comments(first:100){nodes{id databaseId body url createdAt author{login avatarUrl} diffHunk} pageInfo{hasNextPage endCursor}}} pageInfo{hasNextPage endCursor}}}}}"#;
            args![
                "api",
                "graphql",
                "--paginate",
                "--slurp",
                "-f",
                format!("query={query}"),
                "-F",
                "owner={owner}",
                "-F",
                "name={repo}",
                "-F",
                format!("number={number}")
            ];
        }
        Create {
            title,
            body,
            base,
            draft,
            source,
            ..
        } => {
            nonempty(title, "title")?;
            branch(base)?;
            branch(source)?;
            if base == source {
                return Err("Choose a source branch other than the base".into());
            }
            args![
                "pr",
                "create",
                format!("--title={title}"),
                "--body-file",
                "-",
                format!("--base={base}")
            ];
            a.push(format!("--head={source}"));
            if *draft {
                a.push("--draft".into())
            }
            input = Some(body.clone())
        }
        Edit {
            number,
            title,
            body,
            base,
        } => {
            nonempty(title, "title")?;
            branch(base)?;
            args![
                "pr",
                "edit",
                number,
                format!("--title={title}"),
                "--body-file",
                "-",
                format!("--base={base}")
            ];
            input = Some(body.clone())
        }
        Metadata {
            number,
            field,
            value,
            remove,
        } => {
            nonempty(value, "metadata")?;
            let flag = match (field, remove) {
                (MetadataField::Reviewer, false) => "--add-reviewer",
                (MetadataField::Reviewer, true) => "--remove-reviewer",
                (MetadataField::Assignee, false) => "--add-assignee",
                (MetadataField::Assignee, true) => "--remove-assignee",
                (MetadataField::Label, false) => "--add-label",
                (MetadataField::Label, true) => "--remove-label",
            };
            args!["pr", "edit", number, format!("{flag}={value}")]
        }
        Comment { number, body } => {
            nonempty(body, "comment")?;
            args!["pr", "comment", number, "--body-file", "-"];
            input = Some(body.clone())
        }
        Review {
            number,
            head,
            kind,
            body,
            comments,
        } => {
            valid_sha(head)?;
            if *kind == ReviewKind::RequestChanges
                || (*kind == ReviewKind::Comment && comments.is_empty())
            {
                nonempty(body, "review")?;
            }
            for c in comments {
                nonempty(&c.body, "line comment")?;
                if c.line == 0
                    || !["LEFT", "RIGHT"].contains(&c.side.as_str())
                    || c.path.is_empty()
                    || c.path.contains('\0')
                    || c.start_line.is_some_and(|n| n == 0 || n > c.line)
                    || c.start_side.as_ref().is_some_and(|side| side != &c.side)
                {
                    return Err("Select a changed line before commenting".into());
                }
            }
            let event = match kind {
                ReviewKind::Comment => "COMMENT",
                ReviewKind::Approve => "APPROVE",
                ReviewKind::RequestChanges => "REQUEST_CHANGES",
            };
            let comments = comments
                .iter()
                .map(|c| {
                    let mut value =
                        json!({"path":c.path,"line":c.line,"side":c.side,"body":c.body});
                    if let Some(start) = c.start_line {
                        value["start_line"] = json!(start);
                    }
                    if let Some(side) = &c.start_side {
                        value["start_side"] = json!(side);
                    }
                    value
                })
                .collect::<Vec<_>>();
            return Ok(api(
                format!("repos/{{owner}}/{{repo}}/pulls/{number}/reviews"),
                "POST",
                json!({"commit_id":head,"event":event,"body":body,"comments":comments}),
            ));
        }
        Reply {
            number,
            comment,
            body,
        } => {
            nonempty(body, "reply")?;
            return Ok(api(
                format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments/{comment}/replies"),
                "POST",
                json!({"body":body}),
            ));
        }
        Resolve { thread, resolved } => {
            nonempty(thread, "thread")?;
            let mutation = if *resolved {
                "resolveReviewThread"
            } else {
                "unresolveReviewThread"
            };
            let query = format!(
                "mutation($id:ID!){{{mutation}(input:{{threadId:$id}}){{thread{{id isResolved}}}}}}"
            );
            return Ok(Plan {
                args: vec!["api".into(), "graphql".into(), "--input".into(), "-".into()],
                input: Some(json!({"query":query,"variables":{"id":thread}}).to_string()),
            });
        }
        SetState { number, open } => args!["pr", if *open { "reopen" } else { "close" }, number],
        Draft { number, draft } => {
            args!["pr", "ready", number];
            if *draft {
                a.push("--undo".into())
            }
        }
        Merge {
            number,
            head,
            method,
            auto,
        } => {
            valid_sha(head)?;
            args![
                "pr",
                "merge",
                number,
                "--match-head-commit",
                head,
                match method {
                    MergeMethod::Squash => "--squash",
                    MergeMethod::Merge => "--merge",
                    MergeMethod::Rebase => "--rebase",
                }
            ];
            if *auto {
                a.push("--auto".into())
            }
        }
        OverrideMerge {
            number,
            head,
            method,
            acknowledgement,
        } => {
            valid_sha(head)?;
            if acknowledgement != "BYPASS_BRANCH_REQUIREMENTS" {
                return Err(
                    "Explicit acknowledgment is required to bypass branch requirements".into(),
                );
            }
            args![
                "pr",
                "merge",
                number,
                "--match-head-commit",
                head,
                "--admin",
                match method {
                    MergeMethod::Squash => "--squash",
                    MergeMethod::Merge => "--merge",
                    MergeMethod::Rebase => "--rebase",
                }
            ];
        }
        DisableAutoMerge { number } => args!["pr", "merge", number, "--disable-auto"],
        UpdateBranch { number, head } => {
            valid_sha(head)?;
            return Ok(api(
                format!("repos/{{owner}}/{{repo}}/pulls/{number}/update-branch"),
                "PUT",
                json!({"expected_head_sha":head}),
            ));
        }
        Rerun { run, failed_only } => {
            args!["run", "rerun", run];
            if *failed_only {
                a.push("--failed".into())
            }
        }
        Logs { run } => args!["run", "view", run, "--log-failed"],
        Blob { sha } => {
            valid_sha(sha)?;
            args!["api", format!("repos/{{owner}}/{{repo}}/git/blobs/{sha}")]
        }
        Repository => args![
            "repo",
            "view",
            "--json",
            "nameWithOwner,url,defaultBranchRef,viewerPermission,mergeCommitAllowed,rebaseMergeAllowed,squashMergeAllowed"
        ],
        Commits { number } => args![
            "api",
            format!("repos/{{owner}}/{{repo}}/pulls/{number}/commits?per_page=100"),
            "--paginate",
            "--slurp"
        ],
        Conversation { number } => args![
            "api",
            format!("repos/{{owner}}/{{repo}}/issues/{number}/comments?per_page=100"),
            "--paginate",
            "--slurp"
        ],
        Reviews { number } => args![
            "api",
            format!("repos/{{owner}}/{{repo}}/pulls/{number}/reviews?per_page=100"),
            "--paginate",
            "--slurp"
        ],
        PrepareWorkspace { head, .. } => {
            valid_sha(head)?;
            return Err("Workspace preparation is a Git operation".into());
        }
        Compare { .. } | Templates { .. } | Changes { .. } => {
            return Err("Reading branches and templates is a Git operation".into())
        }
        Branches => args![
            "api",
            "repos/{owner}/{repo}/branches?per_page=100",
            "--paginate",
            "--slurp"
        ],
    }
    if let Some(body) = &input {
        if body.len() > 1024 * 1024 {
            return Err("The request is larger than 1 MB".into());
        }
    }
    Ok(Plan { args: a, input })
}
fn nonempty(s: &str, what: &str) -> Result<(), String> {
    if s.trim().is_empty() || s.contains('\0') {
        Err(format!("Enter a valid {what}"))
    } else {
        Ok(())
    }
}
fn branch(s: &str) -> Result<(), String> {
    nonempty(s, "base branch")?;
    if s.starts_with('-') || s.contains(['\n', '\r']) {
        Err("Invalid base branch".into())
    } else {
        Ok(())
    }
}
fn valid_sha(s: &str) -> Result<(), String> {
    if (7..=64).contains(&s.len()) && s.bytes().all(|x| x.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(
            "The pull request head changed or could not be identified. Refresh and try again."
                .into(),
        )
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn execute(request: Request) -> Result<String, String> {
    let root = Path::new(&request.root)
        .canonicalize()
        .map_err(|e| format!("Project unavailable: {e}"))?;
    if !root.is_dir() {
        return Err("Select a project directory".into());
    }
    // A folder that isn't a repository, or one never pushed anywhere, has no
    // pull requests: an empty list, not a provider failure.
    if matches!(request.operation, Operation::List { .. }) && !has_remote(&root) {
        return Ok("[]".into());
    }
    if let Some(repository) = gitlab::detect(&root)? {
        return gitlab::execute(&root, &repository, &request.operation);
    }
    if let Operation::PrepareWorkspace { number, head } = &request.operation {
        valid_sha(head)?;
        let branch = format!("dragent/pr-{number}-{}", &head[..7]);
        // Reuse an existing review branch when it still points at the reviewed head.
        let existing = run_process(
            "git",
            &root,
            Plan {
                args: vec![
                    "rev-parse".into(),
                    "--verify".into(),
                    format!("refs/heads/{branch}"),
                ],
                input: None,
            },
        )
        .ok();
        if existing.as_deref().map(str::trim) != Some(head.as_str()) {
            let repository: Value = serde_json::from_str(&run_process(
                "gh",
                &root,
                Plan {
                    args: vec!["repo".into(), "view".into(), "--json".into(), "url".into()],
                    input: None,
                },
            )?)
            .map_err(|e| e.to_string())?;
            let url = repository["url"]
                .as_str()
                .ok_or("GitHub did not identify the PR repository")?;
            let remotes = run_process(
                "git",
                &root,
                Plan {
                    args: vec!["remote".into(), "-v".into()],
                    input: None,
                },
            )?;
            let fetch_from = remotes
                .lines()
                .filter_map(|line| {
                    let mut words = line.split_whitespace();
                    let name = words.next()?;
                    let remote = words.next()?;
                    (!name.starts_with('-') && remote_identity(remote) == remote_identity(url))
                        .then(|| name.to_string())
                })
                .next()
                .unwrap_or_else(|| url.to_string());
            // Fetch without checking out or resetting the user's current branch.
            run_process(
                "git",
                &root,
                Plan {
                    args: vec![
                        "fetch".into(),
                        "--no-tags".into(),
                        fetch_from,
                        format!("pull/{number}/head:refs/heads/{branch}"),
                    ],
                    input: None,
                },
            )?;
        }
        let actual = run_process(
            "git",
            &root,
            Plan {
                args: vec!["rev-parse".into(), format!("refs/heads/{branch}")],
                input: None,
            },
        )?;
        if actual.trim() != head {
            return Err(
                "The PR changed while its workspace was being prepared. Refresh and try again."
                    .into(),
            );
        }
        return Ok(branch);
    }
    match &request.operation {
        Operation::Compare { base, source } => {
            return compare(&root, "origin", base, source).map(|answer| answer.to_string())
        }
        Operation::Changes { base, source } => {
            return changes(&root, "origin", base, source).map(|answer| answer.to_string())
        }
        Operation::Templates { base } => {
            return github_templates(&root, base).map(|answer| answer.to_string())
        }
        _ => {}
    }
    let spec = plan(&request.operation)?;
    if let Operation::Create {
        source, push: true, ..
    } = &request.operation
    {
        // The branch needn't be checked out to be pushed.
        git(&root, &["push", "--set-upstream", "origin", source])?;
    }
    let mut output = run_process("gh", &root, spec)?;
    if matches!(request.operation, Operation::Repository) {
        let mut repository: Value = serde_json::from_str(&output).map_err(|e| e.to_string())?;
        let current = run_process(
            "git",
            &root,
            Plan {
                args: vec![
                    "symbolic-ref".into(),
                    "--quiet".into(),
                    "--short".into(),
                    "HEAD".into(),
                ],
                input: None,
            },
        )
        .unwrap_or_default();
        repository["currentBranch"] = json!(current.trim());
        repository["localBranches"] = local_branches(&root);
        output = repository.to_string();
    }
    if matches!(request.operation, Operation::Threads { .. }) {
        return expand_thread_comments(&root, output);
    }
    Ok(output)
}
fn remote_identity(remote: &str) -> Option<(String, String)> {
    let (host, path) = if let Some((_, rest)) = remote.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        (authority.rsplit('@').next()?.split(':').next()?, path)
    } else {
        let (host, path) = remote.rsplit('@').next()?.split_once(':')?;
        (host, path)
    };
    Some((
        host.to_ascii_lowercase(),
        path.trim_matches('/')
            .trim_end_matches(".git")
            .to_ascii_lowercase(),
    ))
}
#[cfg(not(target_family = "wasm"))]
fn expand_thread_comments(root: &Path, output: String) -> Result<String, String> {
    let mut pages: Value = serde_json::from_str(&output).map_err(|e| e.to_string())?;
    for page in pages.as_array_mut().into_iter().flatten() {
        for thread in page["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            if thread["comments"]["pageInfo"]["hasNextPage"] != true {
                continue;
            }
            let id = thread["id"].as_str().ok_or("Missing review thread id")?;
            let query = "query($id:ID!,$endCursor:String){node(id:$id){... on PullRequestReviewThread{comments(first:100,after:$endCursor){nodes{id databaseId body url createdAt author{login avatarUrl} diffHunk} pageInfo{hasNextPage endCursor}}}}}";
            let more = run_process(
                "gh",
                root,
                Plan {
                    args: vec![
                        "api".into(),
                        "graphql".into(),
                        "--paginate".into(),
                        "--slurp".into(),
                        "-f".into(),
                        format!("query={query}"),
                        "-f".into(),
                        format!("id={id}"),
                    ],
                    input: None,
                },
            )?;
            let more: Value = serde_json::from_str(&more).map_err(|e| e.to_string())?;
            thread["comments"]["nodes"] = Value::Array(
                more.as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|p| {
                        p["data"]["node"]["comments"]["nodes"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect(),
            );
        }
    }
    let output = pages.to_string();
    if output.len() > 8 * 1024 * 1024 {
        return Err(
            "Review conversations exceed the 8 MB response limit. Inspect this PR on GitHub."
                .into(),
        );
    }
    Ok(output)
}
#[cfg(not(target_family = "wasm"))]
/// Whether the checkout at `root` names any remote. False outside a git
/// repository.
fn has_remote(root: &Path) -> bool {
    git(root, &["remote"]).is_ok_and(|remotes| !remotes.trim().is_empty())
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_process(
        "git",
        root,
        Plan {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            input: None,
        },
    )
}

/// The checkout's own branches, most recently committed first, each with
/// its upstream and how many of its commits aren't pushed there: the
/// branches a pull request can come from besides those on the remote.
#[cfg(not(target_family = "wasm"))]
fn local_branches(root: &Path) -> Value {
    let Ok(listing) = git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)%09%(upstream:short)%09%(upstream:track,nobracket)",
            "refs/heads",
        ],
    ) else {
        return json!([]);
    };
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            let upstream = fields.next().unwrap_or("").trim();
            let track = fields.next().unwrap_or("");
            let unpushed = track
                .split(',')
                .find_map(|part| part.trim().strip_prefix("ahead "))
                .and_then(|count| count.parse::<u64>().ok())
                .unwrap_or(0);
            (!name.is_empty()).then(|| json!({ "name": name, "upstream": upstream, "unpushed": unpushed }))
        })
        .collect()
}

/// What a pull request from `source` into `base` would hold, from the
/// checkout alone so it works before anything is pushed and for both
/// providers: commits and changed files `source` adds over `base` (the
/// remote's copy when fetched), commits `base` has that `source` lacks,
/// whether they merge cleanly, and what must be pushed first. `known` is
/// false when the checkout has neither branch.
#[cfg(not(target_family = "wasm"))]
fn compare(root: &Path, remote: &str, base: &str, source: &str) -> Result<Value, String> {
    let refs = branch_refs(root, remote, base, source)?;
    let (has_local, has_pushed) = (refs.has_local, refs.has_pushed);
    let (Some(base_ref), Some(source_ref)) = (refs.base, refs.source.as_ref()) else {
        return Ok(json!({ "known": false, "local": has_local, "pushed": has_pushed }));
    };
    let (local, pushed) = (refs.local, refs.pushed);
    let range = format!("{base_ref}...{source_ref}");
    let counts = git(root, &["rev-list", "--left-right", "--count", &range])?;
    let mut counts = counts.split_whitespace().map(|n| n.parse::<u64>().unwrap_or(0));
    let (behind, ahead) = (counts.next().unwrap_or(0), counts.next().unwrap_or(0));
    let (files, additions, deletions) = shortstat(&git(root, &["diff", "--shortstat", &range]).unwrap_or_default());
    let unpushed = match (has_local, has_pushed) {
        (true, true) => git(root, &["rev-list", "--count", &format!("{pushed}..{local}")])
            .ok()
            .and_then(|n| n.trim().parse::<u64>().ok())
            .unwrap_or(0),
        (true, false) => ahead,
        _ => 0,
    };
    // Exit 0 merges cleanly; exit 1 with nothing on stderr is a conflict.
    // Anything else (git before 2.38) leaves it unknown.
    let mergeable = match git(root, &["merge-tree", "--write-tree", "--no-messages", &base_ref, source_ref]) {
        Ok(_) => json!(true),
        Err(error) if error.is_empty() => json!(false),
        Err(_) => Value::Null,
    };
    let subject = if ahead == 1 {
        git(root, &["log", "-1", "--format=%s", source_ref]).unwrap_or_default().trim().to_string()
    } else {
        String::new()
    };
    Ok(json!({
        "known": true,
        "local": has_local,
        "pushed": has_pushed,
        "ahead": ahead,
        "behind": behind,
        "files": files,
        "additions": additions,
        "deletions": deletions,
        "unpushed": unpushed,
        "mergeable": mergeable,
        "subject": subject,
    }))
}

/// Where `base` and `source` are in the checkout: the remote's `base` when
/// fetched, else the local one; the local `source` when there is one, else
/// the remote's.
struct BranchRefs {
    base: Option<String>,
    source: Option<String>,
    local: String,
    pushed: String,
    has_local: bool,
    has_pushed: bool,
}

#[cfg(not(target_family = "wasm"))]
fn branch_refs(root: &Path, remote: &str, base: &str, source: &str) -> Result<BranchRefs, String> {
    branch(base)?;
    branch(source)?;
    let exists = |reference: &str| {
        git(root, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")]).is_ok()
    };
    let local = format!("refs/heads/{source}");
    let pushed = format!("refs/remotes/{remote}/{source}");
    let (has_local, has_pushed) = (exists(&local), exists(&pushed));
    let source_ref = if has_local {
        Some(local.clone())
    } else if has_pushed {
        Some(pushed.clone())
    } else {
        None
    };
    let base_ref = [format!("refs/remotes/{remote}/{base}"), format!("refs/heads/{base}")]
        .into_iter()
        .find(|reference| exists(reference));
    Ok(BranchRefs { base: base_ref, source: source_ref, local, pushed, has_local, has_pushed })
}

/// What `source` adds over `base`, for a model to describe: its commit
/// messages oldest first, the diffstat, and the diff, cut to stay within a
/// prompt (`truncated` says so).
#[cfg(not(target_family = "wasm"))]
fn changes(root: &Path, remote: &str, base: &str, source: &str) -> Result<Value, String> {
    const MAX_DIFF: usize = 48 * 1024;
    let refs = branch_refs(root, remote, base, source)?;
    let (Some(base_ref), Some(source_ref)) = (refs.base, refs.source) else {
        return Err(format!("This checkout doesn't have both {base} and {source}. Fetch them, then try again."));
    };
    let log = git(
        root,
        &["log", "--reverse", "--max-count=60", "--format=%B%x1e", &format!("{base_ref}..{source_ref}")],
    )?;
    let commits: Vec<String> = log
        .split('\x1e')
        .map(|commit| commit.trim().to_string())
        .filter(|commit| !commit.is_empty())
        .collect();
    let range = format!("{base_ref}...{source_ref}");
    let stat = git(root, &["diff", "--stat=120", &range]).unwrap_or_default();
    let mut diff = git(root, &["diff", "--no-color", "--no-ext-diff", &range])?;
    let truncated = diff.len() > MAX_DIFF;
    if truncated {
        let mut end = MAX_DIFF;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
    }
    Ok(json!({ "commits": commits, "stat": stat.trim_end(), "diff": diff, "truncated": truncated }))
}

/// Where GitHub looks for pull request templates, among a repository's
/// files: one `pull_request_template.md` (or `.txt`) in `.github/`, the
/// root or `docs/`, and any number in a `PULL_REQUEST_TEMPLATE/` folder
/// there. File names match in any case. The single template comes first.
fn template_paths(paths: &[String]) -> Vec<String> {
    let text = |path: &str| path.ends_with(".md") || path.ends_with(".txt");
    let mut single = Vec::new();
    let mut several = Vec::new();
    for dir in [".github/", "", "docs/"] {
        for path in paths {
            let lower = path.to_ascii_lowercase();
            let Some(rest) = lower.strip_prefix(dir) else { continue };
            if matches!(rest, "pull_request_template.md" | "pull_request_template.txt") {
                single.push(path.clone());
            } else if let Some(name) = rest.strip_prefix("pull_request_template/") {
                if !name.contains('/') && text(name) {
                    several.push(path.clone());
                }
            }
        }
    }
    single.truncate(1);
    single.extend(several);
    single
}

/// A template's name: "Default" for the single one, else its file name.
fn template_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let lower = file.to_ascii_lowercase();
    if lower.starts_with("pull_request_template.") {
        "Default".into()
    } else {
        file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string()
    }
}

/// The pull request templates GitHub would offer: the repository's, read
/// from `base` without checking it out, else the defaults in the
/// organisation's `.github` repository.
#[cfg(not(target_family = "wasm"))]
fn github_templates(root: &Path, base: &str) -> Result<Value, String> {
    const MAX_TEMPLATE: usize = 64 * 1024;
    let reference = [format!("refs/remotes/origin/{base}"), format!("refs/heads/{base}"), "HEAD".into()]
        .into_iter()
        .find(|reference| {
            !base.starts_with('-')
                && git(root, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")]).is_ok()
        })
        .ok_or("The repository has no commits yet")?;
    // The root and the two folders GitHub reads, not the whole tree.
    let mut paths: Vec<String> = git(root, &["ls-tree", "--name-only", &reference])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let folders: Vec<String> = paths
        .iter()
        .filter(|path| matches!(path.to_ascii_lowercase().as_str(), ".github" | "docs" | "pull_request_template"))
        .cloned()
        .collect();
    for folder in folders {
        let listing = git(root, &["ls-tree", "-r", "--name-only", &reference, "--", &format!("{folder}/")]).unwrap_or_default();
        paths.extend(listing.lines().map(str::to_string));
    }
    let mut found: Vec<Value> = template_paths(&paths)
        .into_iter()
        .take(12)
        .filter_map(|path| {
            let body = git(root, &["show", &format!("{reference}:{path}")]).ok()?;
            (body.len() <= MAX_TEMPLATE).then(|| json!({ "name": template_name(&path), "path": path, "body": body, "source": "repository" }))
        })
        .collect();
    if found.is_empty() {
        let gh = |args: &[&str]| {
            run_process("gh", root, Plan { args: args.iter().map(|arg| arg.to_string()).collect(), input: None })
        };
        // No organisation repository, or no access to it, means no templates.
        let listing = gh(&["api", "repos/{owner}/.github/git/trees/HEAD?recursive=1", "--jq", ".tree[].path"]).unwrap_or_default();
        let paths: Vec<String> = listing.lines().map(str::to_string).collect();
        found = template_paths(&paths)
            .into_iter()
            .take(12)
            .filter_map(|path| {
                let encoded: String = path
                    .bytes()
                    .map(|b| if b.is_ascii_alphanumeric() || b"/-_.".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") })
                    .collect();
                let body = gh(&["api", "-H", "Accept: application/vnd.github.raw", &format!("repos/{{owner}}/.github/contents/{encoded}")]).ok()?;
                (body.len() <= MAX_TEMPLATE).then(|| json!({ "name": template_name(&path), "path": path, "body": body, "source": "organisation" }))
            })
            .collect();
    }
    Ok(Value::Array(found))
}

/// Files, insertions and deletions from `git diff --shortstat`.
fn shortstat(line: &str) -> (u64, u64, u64) {
    let mut totals = (0, 0, 0);
    for part in line.split(',') {
        let mut words = part.split_whitespace();
        let Some(count) = words.next().and_then(|n| n.parse::<u64>().ok()) else { continue };
        match words.next().unwrap_or("") {
            word if word.starts_with("file") => totals.0 = count,
            word if word.starts_with("insertion") => totals.1 = count,
            word if word.starts_with("deletion") => totals.2 = count,
            _ => {}
        }
    }
    totals
}

fn run_process(program: &str, root: &Path, spec: Plan) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut command = crate::host_tools::command(program);
    command
        .args(&spec.args)
        .current_dir(root)
        .env_remove("GH_REPO")
        .env_remove("GH_FORCE_TTY")
        .env("NO_COLOR", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GLAB_NO_PROMPT", "1")
        .env("GLAB_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PAGER", "")
        .env("PAGER", "cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child=command.spawn().map_err(|e|format!("Could not start {program} on the project machine: {e}. Sign in with the CLI for this repository's provider (gh auth login or glab auth login)."))?;
    let stdin = child.stdin.take();
    let body = spec.input;
    let writer = std::thread::spawn(move || {
        if let Some(mut pipe) = stdin {
            if let Some(body) = body {
                let _ = pipe.write_all(body.as_bytes());
            }
        }
    });
    fn reader<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut buf = [0; 8192];
            let mut large = false;
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if out.len() + n <= 8 * 1024 * 1024 {
                    out.extend_from_slice(&buf[..n]);
                } else {
                    large = true;
                }
            }
            (out, large)
        })
    }
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) if start.elapsed() < Duration::from_secs(90) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("{program} did not respond within 90 seconds. Refresh before retrying a write: it may have completed."));
            }
        }
    };
    let _ = writer.join();
    let (out, large) = stdout.join().unwrap_or_default();
    let (err, _) = stderr.join().unwrap_or_default();
    let status = status?;
    if !status.success() {
        return Err(String::from_utf8_lossy(&err).trim().to_string());
    }
    if large {
        return Err(
            "This response exceeds 8 MB. Filter the results or inspect the review on its provider.".into(),
        );
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_project_with_nowhere_to_review_lists_no_pull_requests() {
        let root = std::env::temp_dir()
            .join(format!("dragent-no-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let list = || {
            execute(Request {
                root: root.to_string_lossy().into_owned(),
                operation: Operation::List { state: "open".into(), search: String::new(), limit: 30 },
            })
        };
        // Not a repository, then one that was never pushed anywhere.
        assert_eq!(list().unwrap(), "[]");
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(init.success());
        assert_eq!(list().unwrap(), "[]");
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn review_fetch_uses_the_matching_repository_not_an_unrelated_fork() {
        assert_eq!(
            remote_identity("git@github.com:acme/app.git"),
            remote_identity("https://github.com/acme/app")
        );
        assert_ne!(
            remote_identity("git@github.com:someone/app.git"),
            remote_identity("https://github.com/acme/app")
        );
        assert_eq!(
            remote_identity("ssh://git@git.example.com:2222/acme/app.git"),
            remote_identity("https://git.example.com/acme/app")
        );
        assert_ne!(
            remote_identity("git@github.com.attacker.invalid:acme/app.git"),
            remote_identity("https://github.com/acme/app")
        );
    }
    #[test]
    fn override_requires_explicit_acknowledgement() {
        let mut operation = Operation::OverrideMerge {
            number: 7,
            head: "a".repeat(40),
            method: MergeMethod::Squash,
            acknowledgement: String::new(),
        };
        assert!(plan(&operation).is_err());
        if let Operation::OverrideMerge {
            acknowledgement, ..
        } = &mut operation
        {
            *acknowledgement = "BYPASS_BRANCH_REQUIREMENTS".into();
        }
        let p = plan(&operation).unwrap();
        assert!(p.args.contains(&"--admin".into()));
        assert!(p.args.contains(&"--match-head-commit".into()));
    }
    #[test]
    fn markdown_bullets_and_option_like_titles_are_literal() {
        let p = plan(&Operation::Create {
            title: "--web is documentation".into(),
            body: "- a bullet\n- another".into(),
            base: "main".into(),
            draft: true,
            push: false,
            source: "feature".into(),
        })
        .unwrap();
        assert!(p.args.contains(&"--title=--web is documentation".into()));
        assert_eq!(p.input.as_deref(), Some("- a bullet\n- another"));
    }
    #[test]
    fn new_remote_request_round_trips() {
        let request = crate::development::DevelopmentRequest::PullRequests(Request {
            root: "/demo/repo".into(),
            operation: Operation::Files { number: 128 },
        });
        let bytes = bincode::serialize(&request).unwrap();
        let decoded: crate::development::DevelopmentRequest = bincode::deserialize(&bytes).unwrap();
        assert!(matches!(
            decoded,
            crate::development::DevelopmentRequest::PullRequests(Request {
                operation: Operation::Files { number: 128 },
                ..
            })
        ));
    }
    #[test]
    #[cfg(unix)]
    fn process_transport_preserves_stdin_and_reports_failures() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("dragent-pr-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("fake-gh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = fail ]; then printf 'permission denied' >&2; exit 1; fi
/bin/cat
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let body = "- literal markdown\n$(not-executed)\nUnicode: café";
        assert_eq!(
            run_process(
                script.to_str().unwrap(),
                &root,
                Plan {
                    args: vec![],
                    input: Some(body.into())
                }
            )
            .unwrap(),
            body
        );
        assert!(run_process(
            script.to_str().unwrap(),
            &root,
            Plan {
                args: vec!["fail".into()],
                input: None
            }
        )
        .unwrap_err()
        .contains("permission denied"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn comments_are_stdin_not_shell_arguments() {
        let text = "$(touch /tmp/not-run) `whoami`\nquoted 'body'";
        let p = plan(&Operation::Comment {
            number: 7,
            body: text.into(),
        })
        .unwrap();
        assert_eq!(p.input.as_deref(), Some(text));
        assert!(!p.args.iter().any(|x| x.contains("touch")));
    }
    #[test]
    fn merge_is_pinned_and_never_bypasses_policy() {
        let p = plan(&Operation::Merge {
            number: 7,
            head: "a".repeat(40),
            method: MergeMethod::Squash,
            auto: false,
        })
        .unwrap();
        assert!(p.args.contains(&"--match-head-commit".into()));
        assert!(!p.args.contains(&"--admin".into()));
        assert!(!p.args.contains(&"--delete-branch".into()));
    }
    #[test]
    fn rejects_untrusted_refs_and_invalid_lines() {
        assert!(plan(&Operation::Commit {
            sha: "--help".into()
        })
        .is_err());
        assert!(plan(&Operation::Create {
            title: "Test".into(),
            body: String::new(),
            base: "--help".into(),
            draft: false,
            push: false,
            source: "feature".into(),
        })
        .is_err());
        assert!(plan(&Operation::Review {
            number: 1,
            head: "a".repeat(40),
            kind: ReviewKind::Comment,
            body: "ok".into(),
            comments: vec![LineComment {
                path: "src/a.rs".into(),
                line: 0,
                side: "RIGHT".into(),
                body: "fix".into(),
                start_line: None,
                start_side: None,
            }]
        })
        .is_err());
    }
    #[test]
    fn review_comments_round_trip_over_the_ssh_binary_protocol() {
        for start_line in [None, Some(7)] {
            let request = Request {
                root: "/demo/repo".into(),
                operation: Operation::Review {
                    number: 1,
                    head: "a".repeat(40),
                    kind: ReviewKind::Comment,
                    body: "Review".into(),
                    comments: vec![LineComment {
                        path: "src/a.rs".into(),
                        line: 9,
                        side: "RIGHT".into(),
                        body: "Comment".into(),
                        start_line,
                        start_side: start_line.map(|_| "RIGHT".into()),
                    }],
                },
            };
            let decoded: Request =
                bincode::deserialize(&bincode::serialize(&request).unwrap()).unwrap();
            if let Operation::Review { comments, .. } = decoded.operation {
                assert_eq!(comments[0].start_line, start_line);
            } else {
                panic!("wrong operation")
            }
        }
    }
    #[test]
    fn multiline_review_ranges_are_pinned_and_validated() {
        let mut operation = Operation::Review {
            number: 1,
            head: "a".repeat(40),
            kind: ReviewKind::Comment,
            body: String::new(),
            comments: vec![LineComment {
                path: "src/a.rs".into(),
                line: 9,
                side: "RIGHT".into(),
                body: "Check this range".into(),
                start_line: Some(7),
                start_side: Some("RIGHT".into()),
            }],
        };
        let request: Value =
            serde_json::from_str(plan(&operation).unwrap().input.as_ref().unwrap()).unwrap();
        assert_eq!(request["comments"][0]["start_line"], 7);
        if let Operation::Review { comments, .. } = &mut operation {
            comments[0].start_line = Some(10);
        }
        assert!(plan(&operation).is_err());
    }
    #[test]
    fn line_review_uses_exact_head_and_side() {
        let p = plan(&Operation::Review {
            number: 1,
            head: "a".repeat(40),
            kind: ReviewKind::Comment,
            body: String::new(),
            comments: vec![LineComment {
                path: "src/a.rs".into(),
                line: 3,
                side: "LEFT".into(),
                body: "fix".into(),
                start_line: None,
                start_side: None,
            }],
        })
        .unwrap();
        let body: Value = serde_json::from_str(p.input.as_ref().unwrap()).unwrap();
        assert_eq!(body["comments"][0]["side"], "LEFT");
        assert_eq!(body["commit_id"], "a".repeat(40));
    }
    #[test]
    fn shortstat_reads_files_and_lines() {
        assert_eq!(shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)"), (3, 10, 2));
        assert_eq!(shortstat(" 1 file changed, 1 deletion(-)"), (1, 0, 1));
        assert_eq!(shortstat(""), (0, 0, 0));
    }

    #[test]
    fn any_branch_compares_against_the_base_before_it_is_pushed() {
        let root = std::env::temp_dir().join(format!("dragent-pr-compare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Start"]);
        run(&["checkout", "-q", "-b", "feat/invites"]);
        std::fs::write(root.join("b.txt"), "two\nthree\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Add invitations"]);
        // Back on main: the source needn't be checked out.
        run(&["checkout", "-q", "main"]);

        let answer = compare(&root, "origin", "main", "feat/invites").unwrap();
        assert_eq!(answer["known"], true);
        assert_eq!(answer["ahead"], 1);
        assert_eq!(answer["behind"], 0);
        assert_eq!(answer["files"], 1);
        assert_eq!(answer["additions"], 2);
        assert_eq!(answer["pushed"], false);
        assert_eq!(answer["unpushed"], 1);
        assert_eq!(answer["subject"], "Add invitations");
        assert_eq!(answer["mergeable"], true);
        assert_eq!(compare(&root, "origin", "main", "nope").unwrap()["known"], false);
        assert!(compare(&root, "origin", "main", "--help").is_err());

        // Both change the same line: GitHub would say it can't merge cleanly.
        run(&["checkout", "-q", "-b", "clash", "HEAD"]);
        std::fs::write(root.join("a.txt"), "theirs\n").unwrap();
        run(&["commit", "-q", "-am", "Theirs"]);
        run(&["checkout", "-q", "main"]);
        std::fs::write(root.join("a.txt"), "ours\n").unwrap();
        run(&["commit", "-q", "-am", "Ours"]);
        let clash = compare(&root, "origin", "main", "clash").unwrap();
        assert_eq!(clash["mergeable"], false);
        assert_eq!(clash["behind"], 1);

        let branches = local_branches(&root);
        let names: Vec<&str> = branches.as_array().unwrap().iter().filter_map(|b| b["name"].as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"feat/invites"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn templates_are_found_where_github_looks() {
        let paths: Vec<String> = [
            "README.md",
            "docs/pull_request_template/Bug.md",
            ".github/PULL_REQUEST_TEMPLATE.md",
            "pull_request_template.md",
            "docs/pull_request_template/nested/skip.md",
            ".github/workflows/ci.yml",
        ]
        .iter()
        .map(|path| path.to_string())
        .collect();
        assert_eq!(
            template_paths(&paths),
            [".github/PULL_REQUEST_TEMPLATE.md", "docs/pull_request_template/Bug.md"]
        );
        assert_eq!(template_name(".github/PULL_REQUEST_TEMPLATE.md"), "Default");
        assert_eq!(template_name("docs/pull_request_template/Bug.md"), "Bug");
    }

    #[test]
    fn templates_and_changes_come_from_the_checkout() {
        let root = std::env::temp_dir().join(format!("dragent-pr-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".github/PULL_REQUEST_TEMPLATE")).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".github/pull_request_template.md"), "## Summary\n\n## Testing\n").unwrap();
        std::fs::write(root.join(".github/PULL_REQUEST_TEMPLATE/release.md"), "## Release notes\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Templates"]);
        run(&["checkout", "-q", "-b", "feat/x"]);
        std::fs::write(root.join("x.txt"), "x\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Add x", "-m", "Because x matters."]);

        let found = github_templates(&root, "main").unwrap();
        let names: Vec<&str> = found.as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["Default", "release"]);
        assert_eq!(found[0]["body"], "## Summary\n\n## Testing\n");
        assert_eq!(found[0]["source"], "repository");

        let answer = changes(&root, "origin", "main", "feat/x").unwrap();
        assert_eq!(answer["commits"], json!(["Add x\n\nBecause x matters."]));
        assert!(answer["diff"].as_str().unwrap().contains("+x"));
        assert!(answer["stat"].as_str().unwrap().contains("x.txt"));
        assert_eq!(answer["truncated"], false);
        assert!(changes(&root, "origin", "main", "missing").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_source_is_any_branch_but_the_base() {
        let create = |source: &str| {
            plan(&Operation::Create {
                title: "Invites".into(),
                body: String::new(),
                base: "main".into(),
                draft: false,
                push: false,
                source: source.into(),
            })
        };
        assert!(create("feat/other").unwrap().args.contains(&"--head=feat/other".into()));
        assert!(create("main").is_err());
        // The wire name stays the same for machines on an older runtime.
        let wire = serde_json::to_value(Operation::Create {
            title: "t".into(),
            body: String::new(),
            base: "main".into(),
            draft: false,
            push: false,
            source: "feat".into(),
        })
        .unwrap();
        assert_eq!(wire["Create"]["expected_branch"], "feat");
    }
}
