use dragent_host::pull_requests::{Operation, Request};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub root: PathBuf,
    pub label: String,
}
#[derive(Clone, Debug)]
pub struct Item {
    pub target: Target,
    pub data: Value,
}
impl Item {
    pub fn number(&self) -> u64 {
        self.data["number"].as_u64().unwrap_or(0)
    }
    pub fn key(&self) -> String {
        format!("{}#{}", self.target.root.display(), self.number())
    }
}
/// Cached diffs and discussions only belong to the same repository, head and state.
pub fn same_review_version(a: &Item, b: &Item) -> bool {
    a.key() == b.key()
        && !text(&a.data, "headRefOid").is_empty()
        && text(&a.data, "headRefOid") == text(&b.data, "headRefOid")
        && text(&a.data, "state") == text(&b.data, "state")
}
pub fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
pub fn array<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v[k].as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn login(v: &Value) -> &str {
    v["author"]["login"].as_str().unwrap_or("unknown")
}
pub fn number(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}
pub fn is_gitlab(v: &Value) -> bool {
    text(v, "provider") == "gitlab"
}
pub fn run(target: &Target, op: Operation) -> Result<String, String> {
    let request = Request {
        root: target.root.to_string_lossy().into_owned(),
        operation: op,
    };
    if let Some(remote) = crate::remote_fs::resolve(&target.root) {
        let client = crate::remote_fs::client(&remote.machine_id)?;
        let request = Request {
            root: remote.path,
            ..request
        };
        match client.development(dragent_host::development::DevelopmentRequest::PullRequests(request))?{dragent_host::development::DevelopmentResponse::Text(s)=>Ok(s),_=>Err("The remote runtime did not return pull request data. Update the runtime and reconnect.".into())}
    } else {
        dragent_host::pull_requests::execute(request)
    }
}
pub fn json(target: &Target, op: Operation) -> Result<Value, String> {
    serde_json::from_str(&run(target, op)?)
        .map_err(|e| format!("The repository provider returned an unreadable response: {e}"))
}

/// Provider CLIs and SSH requests block; never poll them on the UI executor.
pub async fn background<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = futures::channel::oneshot::channel();
    crate::bg::detach(move || {
        let _ = tx.send(job());
    });
    rx.await
        .map_err(|_| "The code review worker stopped unexpectedly".to_string())?
}

#[derive(Clone)]
pub struct InboxSnapshot {
    pub items: Vec<Item>,
    pub successful_roots: std::collections::HashSet<PathBuf>,
    pub errors: Vec<String>,
}

pub struct InboxUpdate {
    pub snapshot: InboxSnapshot,
    pub complete: bool,
}

/// Deliver completed requests immediately; a slow host must not hold up the
/// next host or the first visible result. Bound process concurrency to four.
fn fetch_parallel<J: Clone + Send + Sync>(
    jobs: Vec<J>,
    fetch: impl Fn(&J) -> Result<Value, String> + Sync,
    mut visit: impl FnMut(J, Result<Value, String>) -> bool,
) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..jobs.len().min(4) {
            let (jobs, fetch, next, cancelled, tx) = (&jobs, &fetch, &next, &cancelled, tx.clone());
            scope.spawn(move || loop {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fetch(job)))
                    .unwrap_or_else(|_| {
                        Err("The provider request worker stopped unexpectedly".into())
                    });
                if tx.send((job.clone(), result)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        for (job, result) in rx {
            if !visit(job, result) {
                cancelled.store(true, Ordering::Relaxed);
                break;
            }
        }
    });
}

pub fn inbox_stream(
    targets: Vec<Target>,
    state: String,
    search: String,
    limit: u32,
) -> futures::channel::mpsc::UnboundedReceiver<InboxUpdate> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    crate::bg::detach(move || {
        let count = targets.len();
        let mut completed = 0;
        let mut snapshot = InboxSnapshot {
            items: vec![],
            successful_roots: Default::default(),
            errors: vec![],
        };
        if count == 0 {
            let _ = tx.unbounded_send(InboxUpdate {
                snapshot,
                complete: true,
            });
            return;
        }
        fetch_parallel(
            targets,
            |target| {
                json(
                    target,
                    Operation::List {
                        state: state.clone(),
                        search: search.clone(),
                        limit,
                    },
                )
            },
            |target, result| {
                completed += 1;
                match result {
                    Ok(Value::Array(items)) => {
                        snapshot.successful_roots.insert(target.root.clone());
                        snapshot.items.extend(items.into_iter().map(|data| Item {
                            target: target.clone(),
                            data,
                        }));
                    }
                    Ok(_) => snapshot.errors.push(format!(
                        "{}: The provider returned an invalid review list",
                        target.label
                    )),
                    Err(error) => snapshot.errors.push(format!("{}: {error}", target.label)),
                }
                tx.unbounded_send(InboxUpdate {
                    snapshot: snapshot.clone(),
                    complete: completed == count,
                })
                .is_ok()
            },
        );
    });
    rx
}

/// An authoritative empty result removes old rows; a failed read keeps them.
pub fn reconcile_inbox(
    previous: &[Item],
    mut snapshot: InboxSnapshot,
    requested: &[Target],
) -> (Vec<Item>, Vec<String>) {
    snapshot.items.extend(
        previous
            .iter()
            .filter(|item| {
                requested
                    .iter()
                    .any(|target| target.root == item.target.root)
                    && !snapshot.successful_roots.contains(&item.target.root)
            })
            .cloned(),
    );
    let mut urls = std::collections::HashSet::new();
    snapshot.items.retain(|item| {
        let url = text(&item.data, "url");
        urls.insert(if url.is_empty() {
            item.key()
        } else {
            url.to_string()
        })
    });
    snapshot
        .items
        .sort_by(|a, b| text(&b.data, "updatedAt").cmp(text(&a.data, "updatedAt")));
    (snapshot.items, snapshot.errors)
}
pub fn flatten(v: Value) -> Vec<Value> {
    v.as_array()
        .map(|a| {
            a.iter()
                .flat_map(|v| v.as_array().cloned().unwrap_or_else(|| vec![v.clone()]))
                .collect()
        })
        .unwrap_or_default()
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckState {
    Pass,
    Fail,
    Running,
    Skipped,
}
pub fn check_state(v: &Value) -> CheckState {
    let status = text(v, "status");
    let conclusion = text(v, "conclusion");
    let state = text(v, "state");
    if [
        "FAILURE",
        "ERROR",
        "TIMED_OUT",
        "ACTION_REQUIRED",
        "STARTUP_FAILURE",
    ]
    .contains(&conclusion)
        || ["FAILURE", "ERROR"].contains(&state)
    {
        CheckState::Fail
    } else if ["QUEUED", "IN_PROGRESS", "PENDING", "WAITING", "REQUESTED"].contains(&status)
        || ["PENDING", "EXPECTED"].contains(&state)
    {
        CheckState::Running
    } else if conclusion == "SUCCESS" || state == "SUCCESS" {
        CheckState::Pass
    } else {
        CheckState::Skipped
    }
}
pub fn attention(v: &Value) -> (&'static str, &'static str) {
    if text(v, "state") != "OPEN" {
        return if text(v, "state") == "MERGED" {
            ("Merged", "This pull request has landed.")
        } else {
            ("Closed", "This pull request is closed.")
        };
    }
    if v["isDraft"].as_bool().unwrap_or(false) {
        return ("Draft", "Mark ready when the changes are ready for review.");
    }
    if text(v, "mergeable") == "CONFLICTING" {
        return (
            "Conflicts",
            "Resolve conflicts with the base branch before merging.",
        );
    }
    if array(v, "statusCheckRollup")
        .iter()
        .any(|x| check_state(x) == CheckState::Fail)
    {
        return (
            "Checks failing",
            "Inspect the failed checks or ask an agent to investigate.",
        );
    }
    if text(v, "reviewDecision") == "CHANGES_REQUESTED" {
        return (
            "Changes requested",
            "Address the review feedback and request another review.",
        );
    }
    if array(v, "statusCheckRollup")
        .iter()
        .any(|x| check_state(x) == CheckState::Running)
    {
        return ("Checks running", "Waiting for CI to finish.");
    }
    if text(v, "reviewDecision") == "REVIEW_REQUIRED" {
        return ("Review needed", "Request a reviewer or start your review.");
    }
    if text(v, "mergeable") == "MERGEABLE"
        && ["CLEAN", "HAS_HOOKS"].contains(&text(v, "mergeStateStatus"))
    {
        (
            "Ready to merge",
            "The latest reported merge requirements are satisfied.",
        )
    } else {
        (
            "Check requirements",
            if is_gitlab(v) {
                "GitLab has not confirmed that all merge requirements are satisfied."
            } else {
                "GitHub has not confirmed that all merge requirements are satisfied."
            },
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
    pub kind: char,
}
pub fn patch_lines(patch: &str) -> Vec<DiffLine> {
    let (mut old, mut new) = (0u32, 0u32);
    let mut out = vec![];
    for line in patch.lines().take(5000) {
        if line.starts_with("@@") {
            let parts: Vec<_> = line.split_whitespace().collect();
            let parse = |s: &str| {
                s.get(1..)
                    .unwrap_or("")
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .parse::<u32>()
                    .unwrap_or(0)
            };
            if parts.len() > 2 {
                old = parse(parts[1]);
                new = parse(parts[2]);
            }
            out.push(DiffLine {
                old: None,
                new: None,
                text: line.into(),
                kind: '@',
            });
            continue;
        }
        let kind = line.chars().next().unwrap_or(' ');
        if kind == '\\' {
            continue;
        }
        let (o, n) = match kind {
            '+' => (None, Some(new)),
            '-' => (Some(old), None),
            _ => (Some(old), Some(new)),
        };
        out.push(DiffLine {
            old: o,
            new: n,
            text: line.get(1..).unwrap_or("").into(),
            kind,
        });
        if kind != '+' {
            old += 1
        }
        if kind != '-' {
            new += 1
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn completed_project_is_delivered_before_a_slow_project() {
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::Duration;
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = release.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            fetch_parallel(
                vec!["slow", "fast"],
                |name| {
                    if *name == "slow" {
                        let (lock, ready) = &*gate;
                        let _guard = ready
                            .wait_timeout_while(
                                lock.lock().unwrap(),
                                Duration::from_secs(3),
                                |released| !*released,
                            )
                            .unwrap();
                    }
                    Ok(Value::Null)
                },
                |name, result| tx.send((name, result.is_ok())).is_ok(),
            );
        });
        let first = rx.recv_timeout(Duration::from_secs(2));
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        worker.join().unwrap();
        assert_eq!(first.unwrap(), ("fast", true));
        assert_eq!(rx.recv().unwrap(), ("slow", true));
    }
    #[test]
    fn cached_review_does_not_cross_project_head_or_state() {
        let a = Item {
            target: Target {
                root: "/a".into(),
                label: "A".into(),
            },
            data: json!({"number":1,"headRefOid":"a".repeat(40),"state":"OPEN"}),
        };
        let mut b = a.clone();
        assert!(same_review_version(&a, &b));
        b.data["headRefOid"] = json!("b".repeat(40));
        assert!(!same_review_version(&a, &b));
        b = a.clone();
        b.data["state"] = json!("MERGED");
        assert!(!same_review_version(&a, &b));
        b = a.clone();
        b.target.root = "/b".into();
        assert!(!same_review_version(&a, &b));
    }
    #[test]
    fn failed_refresh_keeps_rows_but_successful_empty_refresh_removes_them() {
        let a = Target {
            root: "/projects/a".into(),
            label: "A".into(),
        };
        let b = Target {
            root: "/projects/b".into(),
            label: "B".into(),
        };
        let row = |target: Target, n: u64| Item {
            target,
            data: json!({"number":n,"url":format!("https://example/{n}"),"updatedAt":"2026-10-26"}),
        };
        let previous = vec![row(a.clone(), 1), row(b.clone(), 2)];
        let snapshot = InboxSnapshot {
            items: vec![],
            successful_roots: [b.root.clone()].into(),
            errors: vec!["A: timed out".into()],
        };
        let (rows, errors) = reconcile_inbox(&previous, snapshot, &[a.clone(), b.clone()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].number(), 1);
        assert_eq!(errors.len(), 1);
        let snapshot = InboxSnapshot {
            items: vec![],
            successful_roots: [a.root.clone(), b.root.clone()].into(),
            errors: vec![],
        };
        assert!(reconcile_inbox(&previous, snapshot, &[a, b]).0.is_empty());
    }
    #[test]
    fn failed_project_refresh_does_not_restore_rows_from_another_project() {
        let a = Target {
            root: "/projects/a".into(),
            label: "A".into(),
        };
        let b = Target {
            root: "/projects/b".into(),
            label: "B".into(),
        };
        let previous = vec![Item {
            target: a,
            data: json!({"number":1}),
        }];
        let snapshot = InboxSnapshot {
            items: vec![],
            successful_roots: Default::default(),
            errors: vec!["B: offline".into()],
        };
        assert!(reconcile_inbox(&previous, snapshot, &[b]).0.is_empty());
    }
    #[test]
    fn skipped_is_not_passing_and_pending_is_not_failed() {
        assert_eq!(
            check_state(&json!({"status":"COMPLETED","conclusion":"SKIPPED"})),
            CheckState::Skipped
        );
        assert_eq!(
            check_state(&json!({"status":"IN_PROGRESS","conclusion":""})),
            CheckState::Running
        );
        assert_eq!(check_state(&json!({"state":"FAILURE"})), CheckState::Fail);
    }
    #[test]
    fn merge_readiness_requires_confirmed_policy() {
        assert_eq!(
            attention(&json!({"state":"OPEN","mergeable":"UNKNOWN"})).0,
            "Check requirements"
        );
        assert_eq!(
            attention(&json!({"state":"OPEN","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"}))
                .0,
            "Ready to merge"
        );
    }
    #[test]
    fn deletion_uses_left_and_addition_uses_right_coordinates() {
        let d = patch_lines("@@ -10,2 +10,2 @@\n-old\n+new\n same");
        assert_eq!((d[1].old, d[1].new), (Some(10), None));
        assert_eq!((d[2].old, d[2].new), (None, Some(10)));
        assert_eq!((d[3].old, d[3].new), (Some(11), Some(11)));
    }
}

#[derive(Clone)]
pub struct DetailSnapshot {
    pub item: Item,
    pub files: Vec<Value>,
    pub threads: Vec<Value>,
    pub repository: Value,
    pub warnings: Vec<String>,
    pub failed_sections: std::collections::HashSet<String>,
    pub loaded_sections: std::collections::HashSet<String>,
    pub complete: bool,
}

impl DetailSnapshot {
    fn apply_section(&mut self, name: &str, result: Result<Value, String>) {
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.failed_sections.insert(name.into());
                self.warnings.push(format!("{name}: {error}"));
                return;
            }
        };
        self.loaded_sections.insert(name.into());
        match name {
            "files" => self.files = flatten(value),
            "repository" => self.repository = value,
            "threads" => {
                self.threads = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|page| {
                        page["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
            }
            "commits" => {
                self.item.data["commits"] = Value::Array(flatten(value).into_iter().map(|c| {
                    if is_gitlab(&self.item.data) {
                        serde_json::json!({"oid":c["id"],"messageHeadline":c["title"],"committedDate":c["committed_date"]})
                    } else {
                        serde_json::json!({"oid":c["sha"],"messageHeadline":c["commit"]["message"].as_str().unwrap_or("").lines().next().unwrap_or(""),"committedDate":c["commit"]["committer"]["date"]})
                    }
                }).collect());
            }
            "comments" | "reviews" => {
                self.item.data[name] = Value::Array(
                    flatten(value)
                        .into_iter()
                        .map(|mut c| {
                            c["author"] = c["user"].clone();
                            c["createdAt"] = c["created_at"].clone();
                            c["url"] = c["html_url"].clone();
                            c
                        })
                        .collect(),
                );
            }
            _ => {}
        }
    }
}

pub fn detail_stream(
    item: Item,
) -> futures::channel::mpsc::UnboundedReceiver<Result<DetailSnapshot, String>> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    crate::bg::detach(move || {
        let n = item.number();
        let detail = match json(&item.target, Operation::Detail { number: n }) {
            Ok(detail) => detail,
            Err(error) => {
                let _ = tx.unbounded_send(Err(error));
                return;
            }
        };
        let mut snapshot = DetailSnapshot {
            item: Item {
                data: detail,
                ..item
            },
            files: vec![],
            threads: vec![],
            repository: Value::Null,
            warnings: vec![],
            failed_sections: Default::default(),
            loaded_sections: Default::default(),
            complete: false,
        };
        // Publish the overview before fetching any large collections.
        if tx.unbounded_send(Ok(snapshot.clone())).is_err() {
            return;
        }
        let jobs = vec![
            ("repository", Operation::Repository),
            ("files", Operation::Files { number: n }),
            ("threads", Operation::Threads { number: n }),
            ("commits", Operation::Commits { number: n }),
            ("comments", Operation::Conversation { number: n }),
            ("reviews", Operation::Reviews { number: n }),
        ];
        let total = jobs.len();
        let mut completed = 0;
        let target = snapshot.item.target.clone();
        fetch_parallel(
            jobs,
            |(_, operation)| json(&target, operation.clone()),
            |(name, _), result| {
                completed += 1;
                snapshot.apply_section(name, result);
                snapshot.complete = completed == total;
                tx.unbounded_send(Ok(snapshot.clone())).is_ok()
            },
        );
    });
    rx
}

/// Align a run of deletions and insertions into true side-by-side rows.
pub fn split_patch(lines: &[DiffLine]) -> Vec<(Option<DiffLine>, Option<DiffLine>)> {
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        if matches!(lines[i].kind, '-' | '+') {
            let mut old = vec![];
            let mut new = vec![];
            while i < lines.len() && matches!(lines[i].kind, '-' | '+') {
                if lines[i].kind == '-' {
                    old.push(lines[i].clone())
                } else {
                    new.push(lines[i].clone())
                }
                i += 1;
            }
            for j in 0..old.len().max(new.len()) {
                out.push((old.get(j).cloned(), new.get(j).cloned()));
            }
        } else {
            out.push((Some(lines[i].clone()), Some(lines[i].clone())));
            i += 1
        }
    }
    out
}

pub fn merge_allowed(repo: &Value, method: dragent_host::pull_requests::MergeMethod) -> bool {
    use dragent_host::pull_requests::MergeMethod;
    repo[match method {
        MergeMethod::Squash => "squashMergeAllowed",
        MergeMethod::Merge => "mergeCommitAllowed",
        MergeMethod::Rebase => "rebaseMergeAllowed",
    }]
    .as_bool()
    .unwrap_or(false)
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn split_diff_aligns_replacements_without_shifting_context() {
        let rows = split_patch(&patch_lines(
            "@@ -4,2 +4,3 @@\n-before\n+after\n+extra\n same",
        ));
        assert_eq!(rows[1].0.as_ref().unwrap().old, Some(4));
        assert_eq!(rows[1].1.as_ref().unwrap().new, Some(4));
        assert!(rows[2].0.is_none());
        assert_eq!(rows[3].0.as_ref().unwrap().old, Some(5));
        assert_eq!(rows[3].1.as_ref().unwrap().new, Some(6));
    }
    #[test]
    fn pagination_preserves_every_page_and_repository_merge_policy_is_explicit() {
        assert_eq!(flatten(json!([[{"id":1}],[{"id":2}]])).len(), 2);
        assert!(!merge_allowed(
            &Value::Null,
            dragent_host::pull_requests::MergeMethod::Squash
        ));
        assert!(merge_allowed(
            &json!({"squashMergeAllowed":true}),
            dragent_host::pull_requests::MergeMethod::Squash
        ));
    }
}
