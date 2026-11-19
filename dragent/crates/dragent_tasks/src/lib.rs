//! Tasks: the organisation's shared record of work to do, kept on the
//! Dragent server so everyone in it sees, assigns and follows the same
//! tasks. A task can be started in a workspace with an agent; the workspace,
//! its pull requests and any tracker item it came from are linked back to
//! it, so its page shows where the work is.
//!
//! This crate is the contract between the server and the desktop: the
//! shapes on the wire (see `API.md`), the fixed set of statuses and
//! priorities, and the helpers every side uses the same way (the prompt an
//! agent starts with, the branch it works on, how a project is named
//! across machines).

use serde::{Deserialize, Serialize};

/// Where a task is in its life. The set is fixed: every organisation and
/// every screen uses the same six.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
    Canceled,
}

impl Status {
    /// In the order lists and boards show them.
    pub const ALL: [Status; 6] = [
        Status::Backlog,
        Status::Todo,
        Status::InProgress,
        Status::InReview,
        Status::Done,
        Status::Canceled,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Todo => "todo",
            Status::InProgress => "in_progress",
            Status::InReview => "in_review",
            Status::Done => "done",
            Status::Canceled => "canceled",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Backlog => "Backlog",
            Status::Todo => "Todo",
            Status::InProgress => "In progress",
            Status::InReview => "In review",
            Status::Done => "Done",
            Status::Canceled => "Canceled",
        }
    }

    /// `in_progress`, `in-progress`, `In progress` and `inprogress` alike.
    pub fn from_key(key: &str) -> Option<Self> {
        let key: String = key
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Status::ALL.into_iter().find(|status| status.key().replace('_', "") == key)
    }

    /// Not finished: backlog through review.
    pub fn is_open(self) -> bool {
        !matches!(self, Status::Done | Status::Canceled)
    }

    /// Planned or being worked on: todo through review.
    pub fn is_active(self) -> bool {
        matches!(self, Status::Todo | Status::InProgress | Status::InReview)
    }
}

/// How urgent a task is. `None` means nobody said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    None,
    Urgent,
    High,
    Medium,
    Low,
}

impl Priority {
    pub const ALL: [Priority; 5] = [
        Priority::Urgent,
        Priority::High,
        Priority::Medium,
        Priority::Low,
        Priority::None,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Priority::None => "none",
            Priority::Urgent => "urgent",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Priority::None => "No priority",
            Priority::Urgent => "Urgent",
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim().to_ascii_lowercase();
        match key.as_str() {
            "" | "none" | "no priority" => Some(Priority::None),
            "1" => Some(Priority::Urgent),
            "2" => Some(Priority::High),
            "3" => Some(Priority::Medium),
            "4" => Some(Priority::Low),
            _ => Priority::ALL.into_iter().find(|priority| priority.key() == key),
        }
    }

    /// Most urgent first, no priority last.
    pub fn rank(self) -> u8 {
        match self {
            Priority::Urgent => 0,
            Priority::High => 1,
            Priority::Medium => 2,
            Priority::Low => 3,
            Priority::None => 4,
        }
    }
}

/// A member of the organisation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
}

impl Person {
    /// The name, else the email, else "Someone".
    pub fn display(&self) -> &str {
        if !self.name.trim().is_empty() {
            &self.name
        } else if !self.email.trim().is_empty() {
            &self.email
        } else {
            "Someone"
        }
    }
}

/// A project as teammates share it: everyone's checkout of the same
/// repository has the same `key` (its remote, see [`project_key`]), while
/// folders differ per machine and never leave it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRef {
    pub key: String,
    pub label: String,
}

/// The shared name of a repository from its remote address:
/// `git@github.com:Acme/App.git`, `https://github.com/acme/app` and
/// `ssh://git@github.com/acme/app.git` are all `github.com/acme/app`.
pub fn project_key(remote: &str) -> Option<String> {
    let mut rest = remote.trim();
    for scheme in ["https://", "http://", "ssh://", "git://", "git+ssh://"] {
        if let Some(after) = rest.strip_prefix(scheme) {
            rest = after;
            break;
        }
    }
    // user@host:path (scp form) or user@host/path.
    if let Some((_, after)) = rest.split_once('@') {
        rest = after;
    }
    let (host, path) = match rest.split_once(':') {
        // host:port/path keeps the path; host:path is the scp form.
        Some((host, after)) if !after.starts_with(|c: char| c.is_ascii_digit()) || !after.contains('/') => {
            (host, after)
        }
        Some((host, after)) => (host, after.split_once('/').map(|(_, path)| path).unwrap_or("")),
        None => rest.split_once('/')?,
    };
    let path = path.trim_matches('/').trim_end_matches(".git").trim_matches('/');
    // A one-letter "host" is a Windows drive (`C:\repos\app`): a folder on
    // one computer, not a repository teammates share.
    if host.is_empty() || path.is_empty() || host.len() == 1 || path.contains('\\') {
        return None;
    }
    Some(format!("{}/{}", host.to_ascii_lowercase(), path.to_ascii_lowercase()))
}

/// The shared project key behind a tracker's item address, when the
/// address names its repository: GitHub `…/owner/repo/issues/42` and
/// `…/pull/7`, GitLab `…/group/sub/repo/-/issues/1`. Linear, Jira and
/// other non-repo addresses give nothing back — those items attach to a
/// project by hand instead.
pub fn item_project_key(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("https://").or_else(|| url.trim().strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    if host.is_empty() || host.len() == 1 {
        return None;
    }
    // GitLab nests groups (`group/sub/repo/-/issues/1`); everything
    // before the `/-/` marker is the repository.
    if let Some((repo_path, _)) = path.split_once("/-/") {
        let repo_path = repo_path.trim_matches('/').to_ascii_lowercase();
        if repo_path.is_empty() || !repo_path.contains('/') {
            return None;
        }
        return Some(format!("{host}/{repo_path}"));
    }
    // GitHub-style: `owner/repo/issues/42`, `owner/repo/pull/7`.
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let (owner, repo, kind) = (parts.next()?, parts.next()?, parts.next()?);
    if !matches!(kind, "issues" | "pull") {
        return None;
    }
    Some(format!("{host}/{}/{}", owner.to_ascii_lowercase(), repo.to_ascii_lowercase()))
}

/// What a task links to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// A workspace the task was started in (see [`workspace_reference`]).
    Workspace,
    /// A pull request (its address).
    PullRequest,
    /// An item in a connected tracker (see [`item_reference`]).
    Item,
}

/// A workspace, pull request or tracker item the task is linked to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLink {
    pub id: String,
    pub kind: LinkKind,
    /// What people read: a workspace's name, a pull request's title, an
    /// item's key and title.
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    /// What identifies it: the same reference links once (see the
    /// `*_reference` helpers).
    pub reference: String,
    /// A pull request's `open`, `draft`, `merged` or `closed`; a workspace's
    /// last agent state. Free text for display.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub added_by: Option<Person>,
    #[serde(default)]
    pub created_at_ms: i64,
}

/// A workspace's link reference: the machine (its device id) and the
/// workspace's folder there. Only the machine that has it can open it;
/// teammates see that it exists and who started it.
pub fn workspace_reference(device_id: &str, root: &str) -> String {
    format!("{device_id}:{root}")
}

/// The machine and folder of a workspace link, when it is one.
pub fn parse_workspace_reference(reference: &str) -> Option<(&str, &str)> {
    reference
        .split_once(':')
        .filter(|(device, root)| !device.is_empty() && !root.is_empty())
}

/// A tracker item's link reference: its service and key (`linear:ENG-12`,
/// `github:acme/app#42`).
pub fn item_reference(provider: &str, key: &str) -> String {
    format!("{provider}:{key}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// The organisation's prefix and the task's number: `DRA-12`.
    pub key: String,
    pub number: i64,
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub description: String,
    pub status: Status,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub assignee: Option<Person>,
    #[serde(default)]
    pub creator: Option<Person>,
    #[serde(default)]
    pub project: Option<ProjectRef>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub due_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// When it first went in progress.
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    /// When it last went done or canceled.
    #[serde(default)]
    pub completed_at_ms: Option<i64>,
    #[serde(default)]
    pub links: Vec<TaskLink>,
    #[serde(default)]
    pub comment_count: u32,
}

impl Task {
    pub fn links_of(&self, kind: LinkKind) -> impl Iterator<Item = &TaskLink> {
        self.links.iter().filter(move |link| link.kind == kind)
    }
}

/// Who did something: a person by hand, an agent working for them, or
/// Dragent itself (a status that followed a merged pull request).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    #[default]
    Person,
    Agent,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub task_id: String,
    #[serde(default)]
    pub author: Option<Person>,
    #[serde(default)]
    pub actor_kind: ActorKind,
    /// Markdown.
    pub body: String,
    pub created_at_ms: i64,
    #[serde(default)]
    pub edited_at_ms: Option<i64>,
}

/// What changed in one history entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Created,
    Title,
    Description,
    Status,
    Priority,
    Assignee,
    Labels,
    Project,
    Due,
    LinkAdded,
    LinkRemoved,
    Deleted,
    Restored,
}

/// One entry in a task's history. `from` and `to` hold the values in their
/// wire form (a status key, a person, a link); `Description` carries none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub id: String,
    pub task_id: String,
    #[serde(default)]
    pub actor: Option<Person>,
    #[serde(default)]
    pub actor_kind: ActorKind,
    pub kind: ActivityKind,
    #[serde(default)]
    pub from: Option<serde_json::Value>,
    #[serde(default)]
    pub to: Option<serde_json::Value>,
    pub at_ms: i64,
}

// ── Wire shapes (see API.md) ────────────────────────────────────────────────

/// `GET …/tasks` filters. Every field is optional; see API.md for each.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListQuery {
    /// `open` (default), `closed`, `all`, or status keys joined by commas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `me`, `none`, or a member's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A [`ProjectRef::key`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Words in the key, title or description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Only tasks changed after this; deletions since come in `deleted_ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_after_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl ListQuery {
    /// `?status=open&assignee=me`, or empty when nothing is set.
    pub fn query_string(&self) -> String {
        let mut pairs = Vec::new();
        let mut push = |key: &str, value: Option<String>| {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                pairs.push(format!("{key}={}", encode(&value)));
            }
        };
        push("status", self.status.clone());
        push("assignee", self.assignee.clone());
        push("label", self.label.clone());
        push("project", self.project.clone());
        push("q", self.q.clone());
        push("updated_after_ms", self.updated_after_ms.map(|ms| ms.to_string()));
        push("limit", self.limit.map(|limit| limit.to_string()));
        push("cursor", self.cursor.clone());
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }
}

/// Percent-encoding for a query value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResponse {
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// With `updated_after_ms`: tasks deleted since.
    #[serde(default)]
    pub deleted_ids: Vec<String>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    /// The organisation's key prefix (`DRA`).
    #[serde(default)]
    pub prefix: String,
}

/// `GET …/tasks/{key or id}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: Task,
    #[serde(default)]
    pub comments: Vec<Comment>,
    /// Oldest first.
    #[serde(default)]
    pub activity: Vec<Activity>,
}

/// `POST …/tasks`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at_ms: Option<i64>,
    /// An agent is creating it for the person.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_agent: bool,
}

/// `PATCH …/tasks/{id}`: only what's given changes. For the clearable
/// fields, `Some(None)` sends `null` (clear it) and `None` leaves it out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "clearable")]
    pub assignee_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "clearable")]
    pub project: Option<Option<ProjectRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "clearable")]
    pub due_at_ms: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_agent: bool,
}

impl TaskPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.status.is_none()
            && self.priority.is_none()
            && self.assignee_id.is_none()
            && self.project.is_none()
            && self.labels.is_none()
            && self.due_at_ms.is_none()
    }
}

/// A field that can be left out (`None`), cleared (`Some(None)`, sent as
/// `null`) or set.
mod clearable {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// `POST …/tasks/{id}/comments`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewComment {
    pub body: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_agent: bool,
}

/// `POST …/tasks/{id}/links`: linking the same `kind` + `reference` again
/// updates that link instead of adding another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewLink {
    pub kind: LinkKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_agent: bool,
}

// ── Paths ───────────────────────────────────────────────────────────────────

pub fn tasks_path(org: &str) -> String {
    format!("/api/v1/organizations/{org}/tasks")
}

/// One task, by its id or its key (`DRA-12`).
pub fn task_path(org: &str, id_or_key: &str) -> String {
    format!("{}/{}", tasks_path(org), encode(id_or_key))
}

// ── Starting work ───────────────────────────────────────────────────────────

/// What an agent starting on `task` is told: its key and title, then its
/// description.
pub fn task_prompt(task: &Task) -> String {
    let header = format!("{}: {}", task.key, task.title.trim());
    let body = task.description.trim();
    if body.is_empty() {
        header
    } else {
        format!("{header}\n\n{body}")
    }
}

/// The branch a task is worked on: its key and a few words of its title,
/// `dra-12-fix-login-redirect`.
pub fn branch_name(key: &str, title: &str) -> String {
    let words = slug(title, 40);
    let key = slug(key, 20);
    match (key.is_empty(), words.is_empty()) {
        (false, false) => format!("{key}-{words}"),
        (false, true) => key,
        (true, _) => words,
    }
}

/// Lowercase letters and digits joined by single hyphens, cut at a word
/// boundary within `max` characters.
fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    for word in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_ascii_lowercase();
        let extra = if out.is_empty() { word.len() } else { word.len() + 1 };
        if out.len() + extra > max {
            if out.is_empty() {
                out.push_str(&word[..max.min(word.len())]);
            }
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task {
            id: "7f0c".into(),
            key: "DRA-12".into(),
            number: 12,
            title: "Fix the login redirect".into(),
            description: "It loops after **SSO**.".into(),
            status: Status::Todo,
            priority: Priority::High,
            assignee: None,
            creator: None,
            project: None,
            labels: vec!["bug".into()],
            due_at_ms: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            started_at_ms: None,
            completed_at_ms: None,
            links: Vec::new(),
            comment_count: 0,
        }
    }

    #[test]
    fn statuses_read_any_way_people_write_them() {
        for (text, status) in [
            ("in_progress", Status::InProgress),
            ("In progress", Status::InProgress),
            ("in-review", Status::InReview),
            ("DONE", Status::Done),
            ("canceled", Status::Canceled),
        ] {
            assert_eq!(Status::from_key(text), Some(status), "{text}");
        }
        assert_eq!(Status::from_key("cancelled-ish"), None);
        assert!(Status::Backlog.is_open() && !Status::Backlog.is_active());
        assert!(!Status::Done.is_open() && Status::InReview.is_active());
        assert_eq!(Priority::from_key("2"), Some(Priority::High));
        assert!(Priority::Urgent.rank() < Priority::None.rank());
    }

    #[test]
    fn a_repository_has_one_name_whatever_its_remote_looks_like() {
        for remote in [
            "git@github.com:Acme/App.git",
            "https://github.com/acme/app",
            "https://github.com/acme/app.git/",
            "ssh://git@github.com/acme/app.git",
            "ssh://git@github.com:22/acme/app.git",
        ] {
            assert_eq!(project_key(remote).as_deref(), Some("github.com/acme/app"), "{remote}");
        }
        assert_eq!(project_key("/Users/me/app"), None);
        assert_eq!(project_key("C:\\repos\\app.git"), None, "a Windows folder");
        assert_eq!(project_key("C:/repos/app.git"), None, "a Windows folder, forward slashes");
        assert_eq!(project_key(""), None);
    }

    #[test]
    fn item_addresses_resolve_to_their_repository() {
        // GitHub issues and pull requests, including hosted instances.
        assert_eq!(
            item_project_key("https://github.com/Acme/App/issues/42"),
            Some("github.com/acme/app".into())
        );
        assert_eq!(
            item_project_key("https://github.com/acme/app/pull/7"),
            Some("github.com/acme/app".into())
        );
        assert_eq!(
            item_project_key("https://ghe.example.com/acme/app/issues/1"),
            Some("ghe.example.com/acme/app".into())
        );
        // GitLab nests groups; the `/-/` marker ends the repository path.
        assert_eq!(
            item_project_key("https://gitlab.com/group/sub/repo/-/issues/1"),
            Some("gitlab.com/group/sub/repo".into())
        );
        assert_eq!(
            item_project_key("https://gitlab.com/group/repo/-/merge_requests/9"),
            Some("gitlab.com/group/repo".into())
        );
        // Non-repository addresses give nothing: Linear, Jira, blobs,
        // files, empty strings.
        assert_eq!(item_project_key("https://linear.app/acme/issue/ENG-12"), None);
        assert_eq!(item_project_key("https://acme.atlassian.net/browse/OPS-4"), None);
        assert_eq!(item_project_key("https://github.com/acme/app/blob/main/README.md"), None);
        assert_eq!(item_project_key("not a url"), None);
        assert_eq!(item_project_key(""), None);
    }

    #[test]
    fn an_agent_starts_with_the_key_title_and_description_on_a_named_branch() {
        assert_eq!(task_prompt(&task()), "DRA-12: Fix the login redirect\n\nIt loops after **SSO**.");
        assert_eq!(branch_name("DRA-12", "Fix the login redirect"), "dra-12-fix-the-login-redirect");
        assert_eq!(
            branch_name("DRA-7", "Make the sidebar's project order survive restarts and window changes"),
            "dra-7-make-the-sidebar-s-project-order-survive"
        );
        assert_eq!(branch_name("DRA-3", "!!!"), "dra-3");
    }

    #[test]
    fn a_patch_leaves_out_clears_or_sets() {
        let patch = TaskPatch {
            status: Some(Status::Done),
            assignee_id: Some(None),
            ..Default::default()
        };
        let wire = serde_json::to_value(&patch).unwrap();
        assert_eq!(wire, serde_json::json!({"status": "done", "assignee_id": null}));
        let back: TaskPatch = serde_json::from_value(wire).unwrap();
        assert_eq!(back.assignee_id, Some(None));
        assert_eq!(back.project, None, "absent stays absent");
        assert!(TaskPatch::default().is_empty());
    }

    #[test]
    fn list_filters_become_a_query_string() {
        let query = ListQuery {
            status: Some("todo,in_progress".into()),
            assignee: Some("me".into()),
            q: Some("login redirect".into()),
            ..Default::default()
        };
        assert_eq!(query.query_string(), "?status=todo,in_progress&assignee=me&q=login%20redirect");
        assert_eq!(ListQuery::default().query_string(), "");
        assert_eq!(task_path("org-1", "DRA-12"), "/api/v1/organizations/org-1/tasks/DRA-12");
    }

    #[test]
    fn link_references_name_the_machine_and_folder() {
        let reference = workspace_reference("dev-1", "/Users/me/app-wt/dra-12");
        assert_eq!(parse_workspace_reference(&reference), Some(("dev-1", "/Users/me/app-wt/dra-12")));
        // A Windows folder keeps its drive's colon.
        let device = "0b6f3c1e-7d2a-4c1b-9e5f-2a8d4c6b1f30";
        let reference = workspace_reference(device, r"C:\Users\me\app-wt\dra-12");
        assert_eq!(parse_workspace_reference(&reference), Some((device, r"C:\Users\me\app-wt\dra-12")));
        assert_eq!(item_reference("linear", "ENG-4"), "linear:ENG-4");
        let wire = serde_json::to_value(task()).unwrap();
        assert_eq!(wire["status"], "todo");
        assert_eq!(wire["priority"], "high");
    }

    #[test]
    fn new_task_with_todo_sends_todo_never_null() {
        // The new-task form always sends an explicit status; selecting Todo
        // must serialize as "todo", never null and never omitted.
        let new = NewTask {
            title: "Test".into(),
            status: Some(Status::Todo),
            ..Default::default()
        };
        let wire = serde_json::to_value(&new).unwrap();
        assert_eq!(
            wire.get("status"),
            Some(&serde_json::json!("todo")),
            "Todo must send \"todo\", got {wire}"
        );
        // Every other status is explicit too.
        for status in Status::ALL {
            let new = NewTask {
                title: "Test".into(),
                status: Some(status),
                ..Default::default()
            };
            let wire = serde_json::to_value(&new).unwrap();
            assert_eq!(wire.get("status"), Some(&serde_json::json!(status.key())));
        }
    }
}
