//! GitLab REST adapter. `glab` owns authentication on the project machine;
//! this module never reads or forwards an access token.
use super::{
    branch, changes, compare, local_branches, nonempty, remote_identity, run_process, valid_sha,
    LineComment, MergeMethod, MetadataField, Operation, Plan, ReviewKind,
};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(super) struct Repository {
    host: String,
    id: u64,
    remote: String,
    url: String,
    settings: Value,
}

type CachedProject = Arc<Mutex<Option<(Instant, Repository)>>>;
fn project_cache(root: &Path, remote: &str) -> CachedProject {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<(PathBuf, String), CachedProject>>> =
        OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 128 {
        cache.clear();
    }
    cache
        .entry((root.to_path_buf(), remote.to_string()))
        .or_default()
        .clone()
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_process(
        "git",
        root,
        Plan {
            args: args.iter().map(|s| (*s).into()).collect(),
            input: None,
        },
    )
}

fn encode(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn glab(
    root: &Path,
    host: &str,
    path: &str,
    method: &str,
    body: Option<Value>,
    paginate: bool,
) -> Result<String, String> {
    let mut args = vec![
        "api".into(),
        "--hostname".into(),
        host.into(),
        "--method".into(),
        method.into(),
        "--output".into(),
        "json".into(),
    ];
    if paginate {
        args.push("--paginate".into());
    }
    if body.is_some() {
        args.extend(["--input".into(), "-".into()]);
    }
    args.push(path.into());
    run_process(
        "glab",
        root,
        Plan {
            args,
            input: body.map(|v| v.to_string()),
        },
    )
}

fn read(root: &Path, repo: &Repository, path: &str, paginate: bool) -> Result<Value, String> {
    serde_json::from_str(&glab(root, &repo.host, path, "GET", None, paginate)?)
        .map_err(|e| format!("GitLab returned an unreadable response: {e}"))
}

fn write(
    root: &Path,
    repo: &Repository,
    path: &str,
    method: &str,
    body: Value,
) -> Result<Value, String> {
    let raw = glab(root, &repo.host, path, method, Some(body), false)?;
    if raw.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&raw).map_err(|e| format!("GitLab returned an unreadable response: {e}"))
}

fn base(repo: &Repository) -> String {
    format!("projects/{}", repo.id)
}
fn bounded_list(
    limit: u32,
    mut fetch: impl FnMut(u32, u32) -> Result<Value, String>,
) -> Result<Vec<Value>, String> {
    let limit = limit.clamp(1, 500) as usize;
    let per_page = limit.min(100) as u32;
    let mut result = Vec::new();
    for page in 1.. {
        let value = fetch(page, per_page)?;
        let items = value
            .as_array()
            .ok_or("GitLab returned an invalid merge request list")?;
        result.extend(items.iter().take(limit - result.len()).cloned());
        if result.len() >= limit || items.len() < per_page as usize {
            break;
        }
    }
    Ok(result)
}
fn mr(repo: &Repository, number: u64) -> String {
    format!("{}/merge_requests/{number}", base(repo))
}

pub(super) fn detect(root: &Path) -> Result<Option<Repository>, String> {
    let remote = match git(root, &["remote", "get-url", "origin"]) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let Some((host, slug)) = remote_identity(remote.trim()) else {
        return Ok(None);
    };
    if host == "github.com" || host.ends_with(".github.com") {
        return Ok(None);
    }
    if host.is_empty()
        || host.starts_with('-')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
    {
        return Err("Invalid repository host".into());
    }
    // Concurrent section reads share one project lookup. Cache metadata only;
    // every actual operation still uses the machine's current CLI credentials.
    let slot = project_cache(root, remote.trim());
    let mut cached = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, repository)) = &*cached {
        if at.elapsed() < Duration::from_secs(60) {
            return Ok(Some(repository.clone()));
        }
    }
    let path = format!("projects/{}", encode(&slug));
    let response = glab(root, &host, &path, "GET", None, false);
    let project = match response {
        Ok(raw) => serde_json::from_str::<Value>(&raw).map_err(|e| format!("GitLab project response: {e}"))?,
        Err(e) if host.contains("gitlab") => return Err(format!("GitLab project unavailable: {e}. Install glab and run glab auth login on the project machine.")),
        Err(_) => return Ok(None),
    };
    let id = project["id"]
        .as_u64()
        .ok_or("GitLab did not identify the project")?;
    let repository = Repository {
        host,
        id,
        remote: "origin".into(),
        url: project["web_url"].as_str().unwrap_or("").into(),
        settings: project,
    };
    *cached = Some((Instant::now(), repository.clone()));
    Ok(Some(repository))
}

fn user(v: &Value) -> Value {
    json!({"login":v["username"],"name":v["name"],"url":v["web_url"],"avatarUrl":v["avatar_url"]})
}
fn state(v: &Value) -> &str {
    match v["state"].as_str().unwrap_or("") {
        "opened" => "OPEN",
        "merged" => "MERGED",
        _ => "CLOSED",
    }
}
fn draft(v: &Value) -> bool {
    v["draft"].as_bool().unwrap_or_else(|| {
        let title = v["title"].as_str().unwrap_or("");
        title.starts_with("Draft:") || title.starts_with("WIP:") || title.starts_with("[Draft]")
    })
}
fn draft_title(title: &str, make_draft: bool) -> String {
    let plain = title
        .strip_prefix("Draft:")
        .or_else(|| title.strip_prefix("WIP:"))
        .or_else(|| title.strip_prefix("[Draft]"))
        .unwrap_or(title)
        .trim_start();
    if make_draft {
        format!("Draft: {plain}")
    } else {
        plain.to_owned()
    }
}
fn change_count(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| {
            v.as_str().and_then(|s| {
                let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
        })
        .unwrap_or(0)
}
fn status(v: &Value) -> (&'static str, &'static str) {
    match v["status"].as_str().unwrap_or("") {
        "success" => ("COMPLETED", "SUCCESS"),
        "failed" => ("COMPLETED", "FAILURE"),
        "canceled" | "skipped" | "manual" => ("COMPLETED", "SKIPPED"),
        "created" | "pending" | "running" | "preparing" | "waiting_for_resource" => {
            ("IN_PROGRESS", "")
        }
        _ => ("IN_PROGRESS", ""),
    }
}
fn checks(root: &Path, repo: &Repository, mr: &Value) -> Result<Vec<Value>, String> {
    let Some(pipeline) = mr["head_pipeline"]["id"].as_u64() else {
        return Ok(vec![]);
    };
    let path = format!("{}/pipelines/{pipeline}/jobs?per_page=100", base(repo));
    let jobs = read(root, repo, &path, true)?;
    Ok(jobs.as_array().into_iter().flatten().map(|job| {
        let (status, conclusion) = status(job);
        json!({"name":job["name"],"status":status,"conclusion":conclusion,"detailsUrl":job["web_url"],"runId":job["id"],"startedAt":job["started_at"],"completedAt":job["finished_at"]})
    }).collect())
}
fn normalize(mr: &Value, jobs: Vec<Value>, approvals: Option<&Value>) -> Value {
    let merge_status = mr["detailed_merge_status"].as_str().unwrap_or("");
    // List responses do not carry complete approval/CI context. Only detail
    // reads may advertise a merge as ready.
    let clean =
        approvals.is_some() && (merge_status == "mergeable" || merge_status == "can_be_merged");
    let conflict = merge_status == "conflict" || mr["has_conflicts"] == true;
    let review = if merge_status == "requested_changes" {
        "CHANGES_REQUESTED"
    } else if approvals
        .and_then(|v| v["approvals_left"].as_u64())
        .unwrap_or(0)
        > 0
    {
        "REVIEW_REQUIRED"
    } else {
        ""
    };
    let labels: Vec<Value> = mr["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            if l.is_string() {
                json!({"name":l})
            } else {
                l.clone()
            }
        })
        .collect();
    let reviewers: Vec<Value> = mr["reviewers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(user)
        .collect();
    let assignees: Vec<Value> = mr["assignees"]
        .as_array()
        .into_iter()
        .flatten()
        .map(user)
        .collect();
    let number = mr["iid"].as_u64().unwrap_or(0);
    json!({
        "provider":"gitlab","number":number,"id":format!("gitlab:{number}"),"title":mr["title"],
        "body":mr["description"],"url":mr["web_url"],"state":state(mr),"isDraft":draft(mr),
        "author":user(&mr["author"]),"baseRefName":mr["target_branch"],"headRefName":mr["source_branch"],
        "headRefOid":mr["diff_refs"]["head_sha"].as_str().or_else(||mr["sha"].as_str()).unwrap_or(""),
        "updatedAt":mr["updated_at"],"createdAt":mr["created_at"],"additions":mr["additions"].as_u64().unwrap_or(0),
        "deletions":mr["deletions"].as_u64().unwrap_or(0),"changedFiles":change_count(&mr["changes_count"]),
        "reviewDecision":review,"statusCheckRollup":jobs,"labels":labels,"assignees":assignees,"reviewRequests":reviewers,
        "mergeable":if conflict {"CONFLICTING"} else if clean {"MERGEABLE"} else {"UNKNOWN"},
        "mergeStateStatus":if clean {"CLEAN"} else {"BLOCKED"},"autoMergeRequest":if mr["auto_merge_enabled"] == true || mr["merge_when_pipeline_succeeds"] == true {json!({})} else {Value::Null},
        "gitlabDiffRefs":mr["diff_refs"],"gitlabProjectId":mr["project_id"],"gitlabSourceProjectId":mr["source_project_id"]
    })
}
fn files(root: &Path, repo: &Repository, number: u64) -> Result<Vec<Value>, String> {
    let request = read(root, repo, &mr(repo, number), false)?;
    let head = request["diff_refs"]["head_sha"]
        .as_str()
        .or_else(|| request["sha"].as_str())
        .unwrap_or("");
    let diffs = read(
        root,
        repo,
        &format!("{}/diffs?per_page=100", mr(repo, number)),
        true,
    )?;
    Ok(diffs.as_array().into_iter().flatten().map(|f| {
        let patch = f["diff"].as_str().unwrap_or("");
        let adds = patch.lines().filter(|l|l.starts_with('+')&&!l.starts_with("+++ ")).count();
        let dels = patch.lines().filter(|l|l.starts_with('-')&&!l.starts_with("--- ")).count();
        let status = if f["new_file"] == true {"added"} else if f["deleted_file"] == true {"removed"} else if f["renamed_file"] == true {"renamed"} else {"modified"};
        let path=f["new_path"].as_str().unwrap_or("");
        let escaped=path.split('/').map(encode).collect::<Vec<_>>().join("/");
        let blob_url=if head.is_empty(){String::new()}else{format!("{}/-/blob/{}/{}",repo.url,encode(head),escaped)};
        let image=matches!(path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str(),"png"|"jpg"|"jpeg"|"webp");
        let blob_id=if image && patch.is_empty() && !head.is_empty() && status!="removed" {
            read(root,repo,&format!("{}/repository/files/{}?ref={}",base(repo),encode(path),encode(head)),false).ok().and_then(|v|v["blob_id"].as_str().map(str::to_owned)).unwrap_or_default()
        }else{String::new()};
        json!({"filename":f["new_path"],"previous_filename":f["old_path"],"status":status,"patch":patch,"additions":adds,"deletions":dels,"blob_url":blob_url,"sha":blob_id,"gitlabCollapsed":f["collapsed"],"gitlabTooLarge":f["too_large"]})
    }).collect())
}
fn notes(root: &Path, repo: &Repository, number: u64) -> Result<Vec<Value>, String> {
    let raw = read(
        root,
        repo,
        &format!("{}/notes?per_page=100", mr(repo, number)),
        true,
    )?;
    Ok(raw.as_array().into_iter().flatten().filter(|n|n["system"] != true && !n["position"].is_object()).map(|n| {
        json!({"id":n["id"],"body":n["body"],"user":user(&n["author"]),"created_at":n["created_at"],"html_url":n["web_url"]})
    }).collect())
}
fn discussions(root: &Path, repo: &Repository, number: u64) -> Result<Value, String> {
    let raw = read(
        root,
        repo,
        &format!("{}/discussions?per_page=100", mr(repo, number)),
        true,
    )?;
    let threads: Vec<Value> = raw.as_array().into_iter().flatten().filter(|d| d["notes"].as_array().into_iter().flatten().any(|n|n["position"].is_object())).map(|d| {
        let first = d["notes"].as_array().and_then(|n|n.first()).unwrap_or(&Value::Null).clone();
        let comments: Vec<Value> = d["notes"].as_array().into_iter().flatten().map(|n| json!({"id":n["id"],"databaseId":n["id"],"body":n["body"],"url":n["web_url"],"createdAt":n["created_at"],"author":user(&n["author"]),"diffHunk":""})).collect();
        json!({"id":format!("{number}:{}",d["id"].as_str().unwrap_or("")),"isResolved":first["resolved"],"isOutdated":first["position"].is_null(),"path":first["position"]["new_path"].as_str().or_else(||first["position"]["old_path"].as_str()).unwrap_or(""),"line":first["position"]["new_line"].as_u64().or_else(||first["position"]["old_line"].as_u64()).unwrap_or(0),"comments":{"nodes":comments}})
    }).collect();
    Ok(json!([{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":threads}}}}}]))
}

fn current_branch(root: &Path) -> Result<String, String> {
    Ok(git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?
        .trim()
        .into())
}
fn checked_head(
    root: &Path,
    repo: &Repository,
    number: u64,
    expected: &str,
) -> Result<Value, String> {
    valid_sha(expected)?;
    let mr = read(root, repo, &mr(repo, number), false)?;
    let actual = mr["diff_refs"]["head_sha"]
        .as_str()
        .or_else(|| mr["sha"].as_str())
        .unwrap_or("");
    if actual != expected {
        return Err(
            "The merge request has new commits. Refresh before acting on its reviewed head.".into(),
        );
    }
    Ok(mr)
}
fn line_code(path: &str, side: &str, line: u32) -> String {
    let hash = Sha1::digest(path.as_bytes());
    if side == "LEFT" {
        format!("{hash:x}_{line}_0")
    } else {
        format!("{hash:x}_0_{line}")
    }
}
fn line_position(
    c: &LineComment,
    mr: &Value,
    old_path: &str,
    new_path: &str,
) -> Result<Value, String> {
    if c.line == 0
        || !["LEFT", "RIGHT"].contains(&c.side.as_str())
        || c.path.is_empty()
        || c.path.contains('\0')
    {
        return Err("Select a changed line before commenting".into());
    }
    let refs = &mr["diff_refs"];
    let (base, start, head) = (
        refs["base_sha"].as_str().unwrap_or(""),
        refs["start_sha"].as_str().unwrap_or(""),
        refs["head_sha"].as_str().unwrap_or(""),
    );
    valid_sha(base)?;
    valid_sha(start)?;
    valid_sha(head)?;
    let mut p = json!({"position_type":"text","base_sha":base,"start_sha":start,"head_sha":head,"old_path":old_path,"new_path":new_path});
    if c.side == "LEFT" {
        p["old_line"] = json!(c.line);
    } else {
        p["new_line"] = json!(c.line);
    }
    if let Some(from) = c.start_line {
        if from == 0 || from > c.line || c.start_side.as_deref().is_some_and(|s| s != c.side) {
            return Err("Select a valid line range".into());
        }
        let path = if c.side == "LEFT" { old_path } else { new_path };
        let line = |n: u32| {
            if c.side == "LEFT" {
                json!({"type":"old","old_line":n,"line_code":line_code(path,&c.side,n)})
            } else {
                json!({"type":"new","new_line":n,"line_code":line_code(path,&c.side,n)})
            }
        };
        p["line_range"] = json!({"start":line(from),"end":line(c.line)});
    }
    Ok(p)
}
fn path_for_comment(
    root: &Path,
    repo: &Repository,
    number: u64,
    c: &LineComment,
) -> Result<(String, String), String> {
    let all = files(root, repo, number)?;
    let f = all
        .iter()
        .find(|f| f["filename"] == c.path)
        .ok_or("The reviewed file changed. Refresh before commenting.")?;
    Ok((
        f["previous_filename"].as_str().unwrap_or(&c.path).into(),
        f["filename"].as_str().unwrap_or(&c.path).into(),
    ))
}
fn review(
    root: &Path,
    repo: &Repository,
    number: u64,
    head: &str,
    kind: ReviewKind,
    body: &str,
    comments: &[LineComment],
) -> Result<String, String> {
    let current = checked_head(root, repo, number, head)?;
    if kind == ReviewKind::RequestChanges {
        nonempty(body, "change request summary")?;
        let draft_path = format!("{}/draft_notes", mr(repo, number));
        let existing = read(root, repo, &draft_path, true)?;
        if !existing.as_array().is_some_and(Vec::is_empty) {
            return Err("You already have pending GitLab review notes. Publish or discard them on GitLab before requesting changes here so this action does not publish unrelated drafts.".into());
        }
        let mut prepared = Vec::with_capacity(comments.len());
        for c in comments {
            nonempty(&c.body, "line comment")?;
            let (old_path, new_path) = path_for_comment(root, repo, number, c)?;
            prepared.push((
                c.body.as_str(),
                line_position(c, &current, &old_path, &new_path)?,
            ));
        }
        if prepared.is_empty() {
            write(root, repo, &draft_path, "POST", json!({"note":body}))?;
        } else {
            for (index, (note, position)) in prepared.iter().enumerate() {
                if let Err(e) = write(
                    root,
                    repo,
                    &draft_path,
                    "POST",
                    json!({"note":note,"position":position}),
                ) {
                    return Err(format!("Created {index} of {} GitLab draft note(s), then failed: {e}. Review your pending notes on GitLab before retrying.",prepared.len()));
                }
            }
        }
        let mut publish = json!({"reviewer_state":"requested_changes"});
        if !prepared.is_empty() {
            publish["note"] = json!(body);
        }
        write(root, repo, &format!("{draft_path}/bulk_publish"), "POST", publish)
            .map_err(|e| format!("GitLab kept the review notes as drafts because publishing failed: {e}. Review them on GitLab before retrying."))?;
        return Ok("Changes requested on GitLab".into());
    }
    if kind == ReviewKind::Comment && comments.is_empty() {
        nonempty(body, "review comment")?;
    }
    let mut posted = 0;
    for c in comments {
        nonempty(&c.body, "line comment")?;
        let (old_path, new_path) = path_for_comment(root, repo, number, c)?;
        let position = line_position(c, &current, &old_path, &new_path)?;
        if let Err(e) = write(
            root,
            repo,
            &format!("{}/discussions", mr(repo, number)),
            "POST",
            json!({"body":c.body,"position":position}),
        ) {
            return Err(format!("Posted {posted} of {} GitLab discussion(s), then failed: {e}. Refresh before retrying to avoid duplicates.",comments.len()));
        }
        posted += 1;
    }
    if !body.trim().is_empty() {
        if let Err(e) = write(
            root,
            repo,
            &format!("{}/notes", mr(repo, number)),
            "POST",
            json!({"body":body}),
        ) {
            return Err(format!("Posted {posted} line discussion(s), but the review message failed: {e}. Refresh before retrying."));
        }
    }
    if kind == ReviewKind::Approve {
        if let Err(e) = write(
            root,
            repo,
            &format!("{}/approve", mr(repo, number)),
            "POST",
            json!({"sha":head}),
        ) {
            return Err(format!("Posted {posted} line discussion(s), but approval failed: {e}. Refresh before retrying."));
        }
    }
    Ok("Review submitted on GitLab".into())
}

fn change_metadata(
    root: &Path,
    repo: &Repository,
    number: u64,
    field: MetadataField,
    value: &str,
    remove: bool,
) -> Result<String, String> {
    nonempty(value, "metadata")?;
    let path = mr(repo, number);
    let body = match field {
        MetadataField::Label => {
            if remove {
                json!({"remove_labels":value})
            } else {
                json!({"add_labels":value})
            }
        }
        MetadataField::Reviewer | MetadataField::Assignee => {
            let name = value.trim_start_matches('@');
            let users = read(
                root,
                repo,
                &format!("users?username={}", encode(name)),
                false,
            )?;
            let id = users
                .as_array()
                .into_iter()
                .flatten()
                .find(|u| u["username"].as_str() == Some(name))
                .and_then(|u| u["id"].as_u64())
                .ok_or("GitLab user not found. Enter an exact username.")?;
            let current = read(root, repo, &path, false)?;
            let key = if field == MetadataField::Reviewer {
                "reviewers"
            } else {
                "assignees"
            };
            let mut ids: Vec<u64> = current[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|u| u["id"].as_u64())
                .collect();
            if remove {
                ids.retain(|n| *n != id);
            } else if !ids.contains(&id) {
                ids.push(id);
            }
            if field == MetadataField::Reviewer {
                json!({"reviewer_ids":ids})
            } else {
                json!({"assignee_ids":ids})
            }
        }
    };
    write(root, repo, &path, "PUT", body)?;
    Ok("Metadata updated on GitLab".into())
}

/// Merge request description templates: the project's and those its
/// groups pass down, as GitLab lists them, else the files in the
/// repository's `.gitlab/merge_request_templates/` on `target`. "Default"
/// first, as GitLab applies it on its own.
fn templates(root: &Path, repo: &Repository, target: &str) -> Value {
    let listed = read(root, repo, &format!("{}/templates/merge_requests", base(repo)), true)
        .ok()
        .and_then(|list| list.as_array().cloned())
        .unwrap_or_default();
    let mut found: Vec<Value> = listed
        .iter()
        .filter_map(|item| item["key"].as_str().or_else(|| item["name"].as_str()))
        .take(12)
        .filter_map(|key| {
            let template = read(root, repo, &format!("{}/templates/merge_requests/{}", base(repo), encode(key)), false).ok()?;
            let body = template["content"].as_str()?;
            Some(json!({ "name": key, "body": body, "source": "repository" }))
        })
        .collect();
    if found.is_empty() && branch(target).is_ok() {
        let reference = format!("{}/{target}", repo.remote);
        let listing = git(root, &["ls-tree", "--name-only", &reference, "--", ".gitlab/merge_request_templates/"]).unwrap_or_default();
        found = listing
            .lines()
            .filter(|path| path.ends_with(".md"))
            .take(12)
            .filter_map(|path| {
                let body = git(root, &["show", &format!("{reference}:{path}")]).ok()?;
                let name = path.rsplit('/').next()?.trim_end_matches(".md");
                Some(json!({ "name": name, "path": path, "body": body, "source": "repository" }))
            })
            .collect();
    }
    found.sort_by_key(|template| template["name"].as_str().is_none_or(|name| !name.eq_ignore_ascii_case("default")));
    Value::Array(found)
}

pub(super) fn execute(root: &Path, repo: &Repository, op: &Operation) -> Result<String, String> {
    use Operation::*;
    let output:Value=match op {
        List{state,search,limit}=>{
            let state=match state.as_str(){"open"=>"opened","closed"=>"closed","merged"=>"merged","all"=>"all",_=>return Err("Invalid merge request state".into())};
            let mut scope="all";
            let mut extra=vec![];
            let mut words=vec![];
            for word in search.split_whitespace(){
                if word=="author:@me" {scope="created_by_me";}
                else if word=="review-requested:@me" {scope="reviews_for_me";}
                else if let Some(s)=word.strip_prefix("author:"){extra.push(format!("author_username={}",encode(s)));}
                else if let Some(s)=word.strip_prefix("head:"){extra.push(format!("source_branch={}",encode(s)));}
                else if let Some(s)=word.strip_prefix("label:"){extra.push(format!("labels={}",encode(s)));}
                else if word.contains(':'){return Err(format!("GitLab search does not support '{word}'. Use author:, head:, label:, or plain text."));}
                else {words.push(word);}
            }
            if !words.is_empty(){extra.push(format!("search={}",encode(&words.join(" "))));}
            let path=format!("{}/merge_requests?state={state}&scope={scope}&order_by=updated_at&sort=desc{}",base(repo),extra.into_iter().map(|s|format!("&{s}")).collect::<String>());
            let raw=bounded_list(*limit,|page,per_page|read(root,repo,&format!("{path}&page={page}&per_page={per_page}"),false))?;
            Value::Array(raw.iter().map(|v|normalize(v,vec![],None)).collect())
        }
        Detail{number}=>{
            let raw=read(root,repo,&mr(repo,*number),false)?;
            let (approvals,jobs)=std::thread::scope(|scope| {
                let approvals=scope.spawn(||read(root,repo,&format!("{}/approvals",mr(repo,*number)),false).ok());
                let jobs=checks(root,repo,&raw).unwrap_or_default();
                (approvals.join().unwrap_or(None),jobs)
            });
            normalize(&raw,jobs,approvals.as_ref())
        }
        Files{number}=>Value::Array(files(root,repo,*number)?),
        Threads{number}=>discussions(root,repo,*number)?,
        Commits{number}=>read(root,repo,&format!("{}/commits?per_page=100",mr(repo,*number)),true)?,
        Conversation{number}=>Value::Array(notes(root,repo,*number)?),
        Reviews{number}=>{
            let approval=read(root,repo,&format!("{}/approvals",mr(repo,*number)),false)?;
            Value::Array(approval["approved_by"].as_array().into_iter().flatten().map(|a|json!({"user":user(&a["user"]),"state":"APPROVED","body":"","created_at":a["approved_at"]})).collect())
        }
        Repository=>{
            let squash=repo.settings["squash_option"].as_str().unwrap_or("")!="never";
            json!({"provider":"gitlab","nameWithOwner":repo.settings["path_with_namespace"],"url":repo.url,"defaultBranchRef":{"name":repo.settings["default_branch"]},"currentBranch":current_branch(root).unwrap_or_default(),"localBranches":local_branches(root),"mergeCommitAllowed":true,"squashMergeAllowed":squash,"rebaseMergeAllowed":false,"viewerPermission":""})
        }
        Branches=>read(root,repo,&format!("{}/repository/branches?per_page=100",base(repo)),true)?,
        Commit{sha}=>{
            valid_sha(sha)?;
            let raw=read(root,repo,&format!("{}/repository/commits/{sha}/diff?per_page=100",base(repo)),true)?;
            let normalized:Vec<Value>=raw.as_array().into_iter().flatten().map(|f|json!({"filename":f["new_path"],"previous_filename":f["old_path"],"status":if f["new_file"]==true{"added"}else if f["deleted_file"]==true{"removed"}else if f["renamed_file"]==true{"renamed"}else{"modified"},"patch":f["diff"],"additions":0,"deletions":0})).collect();
            json!({"files":normalized})
        }
        Diff{number}=>return glab(root,&repo.host,&format!("{}/raw_diffs",mr(repo,*number)),"GET",None,false),
        Blob{sha}=>{valid_sha(sha)?;read(root,repo,&format!("{}/repository/blobs/{sha}",base(repo)),false)?},
        Logs{run}=>return glab(root,&repo.host,&format!("{}/jobs/{run}/trace",base(repo)),"GET",None,false),
        Rerun{run,..}=>write(root,repo,&format!("{}/jobs/{run}/retry",base(repo)),"POST",json!({}))?,
        Create{title,body,base:target,draft,push,source}=>{
            nonempty(title,"merge request title")?;branch(target)?;branch(source)?;
            if target==source {return Err("Source and target branches must differ".into());}
            if *push { git(root,&["push","--set-upstream",&repo.remote,source])?; }
            let title=draft_title(title,*draft);
            let created=write(root,repo,&format!("{}/merge_requests",base(repo)),"POST",json!({"source_branch":source,"target_branch":target,"title":title,"description":body}))?;
            return created["web_url"].as_str().map(str::to_owned).ok_or("GitLab created a merge request without a URL".into());
        }
        Edit{number,title,body,base:target}=>{
            nonempty(title,"merge request title")?;branch(target)?;
            write(root,repo,&mr(repo,*number),"PUT",json!({"title":title,"description":body,"target_branch":target}))?
        }
        Metadata{number,field,value,remove}=>return change_metadata(root,repo,*number,*field,value,*remove),
        Comment{number,body}=>{
            nonempty(body,"comment")?;
            write(root,repo,&format!("{}/notes",mr(repo,*number)),"POST",json!({"body":body}))?
        }
        Review{number,head,kind,body,comments}=>return review(root,repo,*number,head,*kind,body,comments),
        Reply{number,comment,body}=>{
            nonempty(body,"reply")?;
            let all=read(root,repo,&format!("{}/discussions?per_page=100",mr(repo,*number)),true)?;
            let thread=all.as_array().into_iter().flatten().find(|d|d["notes"].as_array().into_iter().flatten().any(|n|n["id"].as_u64()==Some(*comment))).and_then(|d|d["id"].as_str()).ok_or("This GitLab discussion is no longer available. Refresh and try again.")?;
            write(root,repo,&format!("{}/discussions/{}/notes",mr(repo,*number),encode(thread)),"POST",json!({"body":body}))?
        }
        Resolve{thread,resolved}=>{
            let (number,id)=thread.split_once(':').ok_or("Refresh this GitLab discussion before resolving it")?;
            let number=number.parse::<u64>().map_err(|_|"Invalid GitLab discussion")?;
            write(root,repo,&format!("{}/discussions/{}",mr(repo,number),encode(id)),"PUT",json!({"resolved":resolved}))?
        }
        SetState{number,open}=>write(root,repo,&mr(repo,*number),"PUT",json!({"state_event":if *open{"reopen"}else{"close"}}))?,
        Draft{number,draft}=>{
            let current=read(root,repo,&mr(repo,*number),false)?;
            let title=current["title"].as_str().ok_or("GitLab did not return the merge request title")?;
            let title=draft_title(title,*draft);
            let changed=write(root,repo,&mr(repo,*number),"PUT",json!({"title":title}))?;
            if self::draft(&changed)!=*draft{return Err("GitLab did not apply the requested draft state. Refresh before trying again.".into());}
            changed
        }
        Merge{number,head,method,auto}=>{
            checked_head(root,repo,*number,head)?;
            if *method==MergeMethod::Rebase{return Err("GitLab rebase is a separate branch update. Choose Squash or Project merge method.".into());}
            write(root,repo,&format!("{}/merge",mr(repo,*number)),"PUT",json!({"sha":head,"squash":*method==MergeMethod::Squash,"auto_merge":auto,"should_remove_source_branch":false}))?
        }
        DisableAutoMerge{number}=>{
            let path=format!("{}/cancel_auto_merge",mr(repo,*number));
            match write(root,repo,&path,"POST",json!({})){
                Ok(v)=>v,
                Err(_)=>write(root,repo,&format!("{}/cancel_merge_when_pipeline_succeeds",mr(repo,*number)),"POST",json!({}))?,
            }
        }
        UpdateBranch{number,head}=>{
            checked_head(root,repo,*number,head)?;
            write(root,repo,&format!("{}/rebase",mr(repo,*number)),"PUT",json!({}))?
        }
        Compare{base:target,source}=>compare(root,&repo.remote,target,source)?,
        Changes{base:target,source}=>changes(root,&repo.remote,target,source)?,
        Templates{base:target}=>templates(root,repo,target),
        OverrideMerge{..}=>return Err("GitLab does not expose the GitHub administrator bypass. Follow this project's merge requirements.".into()),
        PrepareWorkspace{number,head}=>{
            valid_sha(head)?;
            let name=format!("dragent/mr-{number}-{}",&head[..7]);
            let existing=git(root,&["rev-parse","--verify",&format!("refs/heads/{name}")]).ok();
            if existing.as_deref().map(str::trim)!=Some(head.as_str()){
                git(root,&["fetch","--no-tags",&repo.remote,&format!("refs/merge-requests/{number}/head:refs/heads/{name}")])?;
            }
            let actual=git(root,&["rev-parse",&format!("refs/heads/{name}")])?;
            if actual.trim()!=head{return Err("The merge request changed while preparing its workspace. Refresh and try again.".into());}
            return Ok(name);
        }
    };
    Ok(output.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inbox_stops_at_the_requested_limit_instead_of_fetching_all_history() {
        let mut calls = vec![];
        let rows = bounded_list(30, |page, size| {
            calls.push((page, size));
            Ok(json!((0..size)
                .map(|i| json!({"iid":(page-1)*size+i}))
                .collect::<Vec<_>>()))
        })
        .unwrap();
        assert_eq!(rows.len(), 30);
        assert_eq!(calls, vec![(1, 30)]);
        let mut calls = 0;
        let rows = bounded_list(130, |_, size| {
            calls += 1;
            Ok(json!(vec![Value::Null; size as usize]))
        })
        .unwrap();
        assert_eq!(rows.len(), 130);
        assert_eq!(calls, 2);
        let mut calls = 0;
        assert!(bounded_list(500, |_, _| {
            calls += 1;
            Ok(json!([]))
        })
        .unwrap()
        .is_empty());
        assert_eq!(calls, 1);
    }
    #[test]
    fn maps_gitlab_review_state_without_claiming_list_items_are_merge_ready() {
        let raw = json!({"iid":42,"web_url":"https://gitlab.example/acme/app/-/merge_requests/42","title":"Draft: Improve auth","state":"opened","draft":true,"author":{"username":"ada"},"target_branch":"main","source_branch":"auth","sha":"a".repeat(40),"detailed_merge_status":"mergeable","labels":["security"],"reviewers":[{"username":"sam"}]});
        let list = normalize(&raw, vec![], None);
        assert_eq!(list["provider"], "gitlab");
        assert_eq!(list["number"], 42);
        assert_eq!(list["isDraft"], true);
        assert_eq!(list["mergeStateStatus"], "BLOCKED");
        assert_eq!(list["labels"][0]["name"], "security");
        let detail = normalize(
            &raw,
            vec![json!({"status":"COMPLETED","conclusion":"FAILURE"})],
            Some(&json!({"approvals_left":1})),
        );
        assert_eq!(detail["reviewDecision"], "REVIEW_REQUIRED");
        assert_eq!(detail["statusCheckRollup"].as_array().unwrap().len(), 1);
        let mut blocked = raw.clone();
        blocked["detailed_merge_status"] = json!("requested_changes");
        assert_eq!(
            normalize(&blocked, vec![], Some(&json!({"approvals_left":0})))["reviewDecision"],
            "CHANGES_REQUESTED"
        );
    }
    #[test]
    fn gitlab_line_positions_use_the_current_diff_version_and_selected_side() {
        let current = json!({"diff_refs":{"base_sha":"b".repeat(40),"start_sha":"c".repeat(40),"head_sha":"a".repeat(40)}});
        let mut c = LineComment {
            path: "src/a.rs".into(),
            line: 9,
            side: "RIGHT".into(),
            body: "check".into(),
            start_line: Some(7),
            start_side: Some("RIGHT".into()),
        };
        let p = line_position(&c, &current, "src/old.rs", "src/a.rs").unwrap();
        assert_eq!(p["head_sha"], "a".repeat(40));
        assert_eq!(p["new_line"], 9);
        assert_eq!(p["old_path"], "src/old.rs");
        assert_eq!(p["line_range"]["start"]["new_line"], 7);
        assert!(p["line_range"]["end"]["line_code"]
            .as_str()
            .unwrap()
            .ends_with("_0_9"));
        c.side = "LEFT".into();
        c.start_line = None;
        c.start_side = None;
        let left = line_position(&c, &current, "src/old.rs", "src/a.rs").unwrap();
        assert_eq!(left["old_line"], 9);
        assert!(left["new_line"].is_null());
        c.line = 0;
        assert!(line_position(&c, &current, "src/old.rs", "src/a.rs").is_err());
    }
    #[test]
    fn api_paths_encode_untrusted_segments() {
        assert_eq!(encode("group/sub project"), "group%2Fsub%20project");
        assert_eq!(encode("?x=1&y=2"), "%3Fx%3D1%26y%3D2");
        assert_eq!(
            status(&json!({"status":"failed"})),
            ("COMPLETED", "FAILURE")
        );
        assert_eq!(
            status(&json!({"status":"skipped"})),
            ("COMPLETED", "SKIPPED")
        );
        assert_eq!(change_count(&json!("1000+")), 1000);
        assert_eq!(draft_title("WIP: hello", false), "hello");
        assert_eq!(draft_title("[Draft] hello", true), "Draft: hello");
    }
}
