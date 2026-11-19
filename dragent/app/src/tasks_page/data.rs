//! What the Tasks page reads and changes, through the tasks client
//! (`crate::tasks::client`). Every call blocks: the page makes them off the
//! main thread. The last full list is kept on disk so the page opens at once
//! and still reads while offline.

use std::path::Path;

use dragent_tasks::{LinkKind, ListQuery, NewComment, NewLink, NewTask, Person, ProjectRef, Status, Task, TaskDetail, TaskLink, TaskPatch};

use crate::integrations::tracker::TrackerItem;
use crate::tasks::{cache, client};

/// One look at the organisation's tasks.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub tasks: Vec<Task>,
    pub members: Vec<Person>,
    pub prefix: String,
    /// The newest `updated_at_ms` seen: the next look asks only for what
    /// changed after it.
    pub seen_ms: i64,
}

/// The list as it was last saved here, to show before the server answers.
pub(crate) fn cached(org: &str) -> Option<Snapshot> {
    let cached = cache::load(org)?;
    let seen_ms = cached.tasks.iter().map(|task| task.updated_at_ms).max().unwrap_or(0);
    Some(Snapshot {
        tasks: cached.tasks,
        members: Vec::new(),
        prefix: cached.prefix,
        seen_ms,
    })
}

/// The organisation's tasks: everything the first time, then only what
/// changed since `previous`.
pub(crate) fn load(org: &str, previous: Option<&Snapshot>) -> Result<Snapshot, String> {
    let mut snapshot = previous.cloned().unwrap_or_default();
    // A snapshot from the disk cache has no members yet: ask for everything
    // then too, so tasks deleted meanwhile drop out.
    let incremental = previous.is_some_and(|previous| !previous.members.is_empty());
    if !incremental {
        snapshot.tasks.clear();
        snapshot.seen_ms = 0;
    }
    let mut query = ListQuery {
        status: Some("all".into()),
        limit: Some(500),
        updated_after_ms: incremental.then_some(snapshot.seen_ms),
        ..Default::default()
    };
    loop {
        let page = client::list(org, &query)?;
        snapshot.prefix = page.prefix;
        snapshot.tasks.retain(|task| !page.deleted_ids.contains(&task.id));
        for task in page.tasks {
            snapshot.seen_ms = snapshot.seen_ms.max(task.updated_at_ms);
            match snapshot.tasks.iter_mut().find(|known| known.id == task.id) {
                Some(known) => *known = task,
                None => snapshot.tasks.push(task),
            }
        }
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(cursor),
            None => break,
        }
    }
    if snapshot.members.is_empty() {
        snapshot.members = client::members(org).unwrap_or_default();
    }
    let _ = cache::save(org, &cache::stamped(snapshot.tasks.clone(), snapshot.prefix.clone()));
    Ok(snapshot)
}

pub(crate) fn create(org: &str, new: &NewTask) -> Result<Task, String> {
    client::create(org, new)
}

/// File a new item with a connected tracker (GitHub issue, Linear issue,
/// …): title and body travel over, `place` names where (a team, project
/// or `owner/repo`), blank for the tracker's default. Blocking.
pub(crate) fn create_tracker_item(
    provider: crate::integrations::Provider,
    root: &Path,
    title: String,
    body: String,
    place: Option<String>,
) -> Result<TrackerItem, String> {
    let draft = crate::integrations::items::Draft { title, body, place };
    Ok(TrackerItem::from_item(crate::integrations::items::create(provider, root, &draft)?))
}

/// A tracker item saved as a native task: its title, body, labels and
/// status travel over, its project resolves from its address when that
/// names a repository (otherwise the task starts project-less, ready to
/// attach in its detail panel), and the task links back to the item.
pub(crate) fn import_item(org: &str, item: &TrackerItem) -> Result<Task, String> {
    let hinted = crate::tasks::project_hints::hinted_project(org, item.provider.key(), &item.key);
    let (new, link) = import_parts(item, hinted);
    let task = create(org, &new)?;
    if let Err(error) = client::add_link(org, &task.id, &link).map(|_| ()) {
        return Err(format!("Task {} created, but its tracker link failed: {error}", task.key));
    }
    // The address named its repository: remember it for this tracker
    // project, so the next import attaches on its own.
    if let Some(resolved) = dragent_tasks::item_project_key(&item.url)
        .and_then(|key| new.project.as_ref().filter(|project| project.key == key))
    {
        crate::tasks::project_hints::remember(org, &[link.reference.clone()], Some(resolved));
    }
    Ok(task)
}

fn import_parts(item: &TrackerItem, hinted: Option<ProjectRef>) -> (NewTask, NewLink) {
    let title = item.title.trim();
    let title = if title.is_empty() { item.key.clone() } else { title.to_string() };
    let body = item.item.body.trim();
    let labels = client::clean_labels(item.labels.clone());
    let project = dragent_tasks::item_project_key(&item.url)
        .map(|key| {
            let label = key.rsplit('/').next().unwrap_or(&key).to_string();
            ProjectRef { key, label }
        })
        .or(hinted);
    let new = NewTask {
        title,
        description: (!body.is_empty()).then(|| body.to_string()),
        status: Some(item.status),
        priority: None,
        assignee_id: None,
        project,
        labels: (!labels.is_empty()).then_some(labels),
        due_at_ms: None,
        by_agent: false,
    };
    let link = NewLink {
        kind: LinkKind::Item,
        title: format!("{} {}", item.key, item.title).trim().to_string(),
        url: (!item.url.trim().is_empty()).then(|| item.url.clone()),
        reference: dragent_tasks::item_reference(item.provider.key(), &item.key),
        state: item.state_label.clone(),
        by_agent: false,
    };
    (new, link)
}

pub(crate) fn update(org: &str, id: &str, patch: &TaskPatch) -> Result<Task, String> {
    client::patch(org, id, patch)
}

pub(crate) fn set_status(org: &str, id: &str, status: Status) -> Result<Task, String> {
    update(
        org,
        id,
        &TaskPatch {
            status: Some(status),
            ..Default::default()
        },
    )
}

/// Link a workspace, pull request or tracker item to a task.
pub(crate) fn add_link(org: &str, id: &str, link: &NewLink) -> Result<TaskLink, String> {
    client::add_link(org, id, link)
}

/// One task, as it is now.
pub(crate) fn get_task(org: &str, id: &str) -> Result<Task, String> {
    Ok(get_detail(org, id)?.task)
}

/// One task with its comments and history.
pub(crate) fn get_detail(org: &str, id: &str) -> Result<TaskDetail, String> {
    client::detail(org, id)
}

pub(crate) fn delete_task(org: &str, id: &str) -> Result<(), String> {
    client::delete(org, id)
}

pub(crate) fn add_comment(org: &str, id: &str, body: &str) -> Result<(), String> {
    let comment = NewComment {
        body: body.to_string(),
        by_agent: false,
    };
    client::add_comment(org, id, &comment).map(|_| ())
}

pub(crate) fn edit_comment(org: &str, id: &str, comment: &str, body: &str) -> Result<(), String> {
    client::edit_comment(org, id, comment, body).map(|_| ())
}

pub(crate) fn delete_comment(org: &str, id: &str, comment: &str) -> Result<(), String> {
    client::delete_comment(org, id, comment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::Provider;
    use crate::integrations::tracker::TrackerItem;
    use dragent_tasks::LinkKind;

    fn item(provider: Provider, key: &str, title: &str, url: &str, body: &str, labels: Vec<&str>) -> TrackerItem {
        TrackerItem {
            provider,
            key: key.into(),
            title: title.into(),
            url: url.into(),
            status: Status::Todo,
            state_label: None,
            assignee: None,
            labels: labels.into_iter().map(str::to_string).collect(),
            updated_at_ms: None,
            branch: String::new(),
            item: crate::integrations::items::Item {
                provider,
                key: key.into(),
                title: title.into(),
                url: url.into(),
                body: body.into(),
                detail: None,
                branch: String::new(),
                reference: None,
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
            },
        }
    }

    #[test]
    fn github_issues_import_with_their_repository_attached() {
        let (new, link) = import_parts(
            &item(
                Provider::Github,
                "#42",
                "Fix login",
                "https://github.com/Acme/App/issues/42",
                "It loops.",
                vec!["Bug", "bug", " "],
            ),
            None,
        );
        assert_eq!(new.title, "Fix login");
        assert_eq!(new.description.as_deref(), Some("It loops."));
        assert_eq!(new.status, Some(Status::Todo));
        assert_eq!(new.labels.as_deref(), Some(&["bug".to_string()][..]));
        let project = new.project.expect("repository resolves");
        assert_eq!(project.key, "github.com/acme/app");
        assert_eq!(link.kind, LinkKind::Item);
        assert_eq!(link.reference, "github:#42");
        assert_eq!(link.url.as_deref(), Some("https://github.com/Acme/App/issues/42"));
    }

    #[test]
    fn items_without_a_repository_still_import_project_less() {
        let (new, link) = import_parts(
            &item(
                Provider::Linear,
                "ENG-12",
                " ",
                "https://linear.app/acme/issue/ENG-12",
                " ",
                vec![],
            ),
            None,
        );
        assert_eq!(new.title, "ENG-12", "empty titles fall back to the key");
        assert_eq!(new.description, None);
        assert_eq!(new.project, None, "picked by hand in the detail panel instead");
        assert_eq!(new.labels, None);
        assert_eq!(link.reference, "linear:ENG-12");
    }

    #[test]
    fn hinted_tracker_projects_attach_without_a_repository_url() {
        let hinted = Some(ProjectRef { key: "github.com/acme/app".into(), label: "app".into() });
        let (new, link) = import_parts(
            &item(
                Provider::Linear,
                "ENG-13",
                "Polish",
                "https://linear.app/acme/issue/ENG-13",
                "",
                vec![],
            ),
            hinted,
        );
        assert_eq!(new.project.as_ref().map(|project| project.key.as_str()), Some("github.com/acme/app"));
        assert_eq!(link.reference, "linear:ENG-13");
    }
}
