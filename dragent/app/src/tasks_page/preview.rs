//! Sample tasks for the screen previews (`ui_preview`): the page as it looks
//! with an organisation's work in it, without a server.

use dragent_tasks::{LinkKind, Person, Priority, Status, Task, TaskLink};

use super::{data, Layout, Source, TasksView};
use crate::integrations::items::{Item, ItemState, StateCategory};
use crate::integrations::tracker::TrackerItem;
use crate::integrations::Provider;

fn person(id: &str, name: &str) -> Person {
    Person {
        id: id.into(),
        name: name.into(),
        email: format!("{}@example.com", name.to_lowercase()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn task(number: i64, title: &str, status: Status, priority: Priority, assignee: Option<Person>, labels: &[&str], minutes_ago: i64) -> Task {
    let at = now_ms() - minutes_ago * 60_000;
    Task {
        id: format!("t{number}"),
        key: format!("DRA-{number}"),
        number,
        title: title.into(),
        description: String::new(),
        status,
        priority,
        assignee,
        creator: None,
        project: None,
        labels: labels.iter().map(|label| label.to_string()).collect(),
        due_at_ms: None,
        created_at_ms: at,
        updated_at_ms: at,
        started_at_ms: None,
        completed_at_ms: None,
        links: Vec::new(),
        comment_count: 0,
    }
}

fn link(kind: LinkKind, title: &str, state: &str) -> TaskLink {
    TaskLink {
        id: format!("l-{title}"),
        kind,
        title: title.into(),
        url: Some("https://github.com/acme/app/pull/88".into()),
        reference: title.into(),
        state: Some(state.into()),
        added_by: None,
        created_at_ms: 0,
    }
}

fn sample_tasks() -> Vec<Task> {
    let ada = person("u1", "Ada Lovelace");
    let grace = person("u2", "Grace Hopper");
    let mut redirect = task(12, "Fix the login redirect loop after SSO", Status::InProgress, Priority::Urgent, Some(ada.clone()), &["bug", "auth"], 14);
    redirect.links.push(link(LinkKind::Workspace, "dra-12-fix-the-login-redirect", "working"));
    let mut export = task(9, "Export a project's tasks as CSV", Status::InReview, Priority::Medium, Some(grace.clone()), &["feature"], 95);
    export.links.push(link(LinkKind::Workspace, "dra-9-export-tasks", "idle"));
    export.links.push(link(LinkKind::PullRequest, "#88 Export tasks as CSV", "open"));
    vec![
        redirect,
        task(14, "Show who is viewing a shared session", Status::Todo, Priority::High, Some(grace.clone()), &["sharing"], 40),
        task(15, "Retry uploads when the network drops", Status::Todo, Priority::None, None, &["reliability"], 300),
        export,
        task(16, "Keyboard shortcut to open the Tasks page", Status::Backlog, Priority::Low, None, &[], 2_000),
        task(17, "Remember the board's column widths", Status::Backlog, Priority::None, Some(ada.clone()), &["polish"], 4_000),
        task(7, "Dark theme contrast in the diff view", Status::Done, Priority::Medium, Some(ada), &["design"], 6_000),
    ]
}

fn sample_items() -> Vec<TrackerItem> {
    let item = |number: u32, title: &str, category: StateCategory, state: &str, assignee: Option<&str>, labels: &[&str]| {
        TrackerItem::from_item(Item {
            provider: Provider::Github,
            key: format!("#{number}"),
            title: title.into(),
            url: format!("https://github.com/acme/app/issues/{number}"),
            body: String::new(),
            detail: None,
            branch: format!("issue-{number}"),
            reference: None,
            state: Some(ItemState::new(category, state)),
            assignee: assignee.map(str::to_string),
            labels: labels.iter().map(|label| label.to_string()).collect(),
            updated_at_ms: Some(now_ms() - i64::from(number) * 600_000),
        })
    };
    vec![
        item(142, "Crash when a pane closes during a search", StateCategory::Open, "Open", Some("ada"), &["bug"]),
        item(139, "Support SSH config Include files", StateCategory::Open, "Open", None, &["enhancement"]),
        item(131, "Sidebar flickers on window resize", StateCategory::Started, "Open", Some("grace"), &["bug", "ui"]),
    ]
}

/// The page as a preview scene shows it.
pub(crate) fn seed(page: &mut TasksView, layout: Layout, tracker: bool) {
    page.org = Some("preview".into());
    page.me = Some("u1".into());
    page.loading = false;
    page.error = None;
    page.layout = layout;
    page.providers = vec![Provider::Github, Provider::Linear, Provider::Jira];
    page.snapshot = Some(data::Snapshot {
        tasks: sample_tasks(),
        members: vec![person("u1", "Ada Lovelace"), person("u2", "Grace Hopper")],
        prefix: "DRA".into(),
        seen_ms: 0,
    });
    if tracker {
        page.source = Source::Tracker(Provider::Github);
        page.tracker_items = sample_items();
    } else {
        page.source = Source::Dragent;
        page.tracker_items.clear();
    }
}
