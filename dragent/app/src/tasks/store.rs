//! The model the Tasks page reads: the organisation's tasks, its members,
//! and the signed-in user, with pure filter/sort helpers for the list.
//!
//! The page opens on the cached list instantly, then [`TasksStore::refresh`]
//! brings it up to date: a full list the first time, an incremental one
//! (`updated_after_ms`) after that. A task returned by any mutation is
//! applied at once, so the page shows a change it just made without waiting
//! for the next refresh.

use dragent_tasks::{ListQuery, Person, Priority, Status, Task};

/// Which tasks a filtered list holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    /// Backlog through review.
    #[default]
    Open,
    /// Done and canceled.
    Closed,
    /// Everything.
    All,
    /// Only these statuses.
    Keys(Vec<Status>),
}

/// Whose tasks a filtered list holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AssigneeFilter {
    #[default]
    Any,
    /// Assigned to the signed-in user.
    Me,
    /// Assigned to nobody.
    None,
    /// Assigned to this member.
    User(String),
}

/// What a filtered list holds. Every field narrows; all must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskFilter {
    pub status: StatusFilter,
    pub assignee: AssigneeFilter,
    /// Tasks carrying this label.
    pub label: Option<String>,
    /// Tasks of this project ([`dragent_tasks::ProjectRef::key`]).
    pub project: Option<String>,
    /// Words in the key, title or description.
    pub search: String,
}

/// What one [`TasksStore::refresh`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// Tasks added or changed.
    pub changed: usize,
    /// Tasks deleted on the server since.
    pub deleted: usize,
    /// Whether this was the first, full load.
    pub full: bool,
}

pub struct TasksStore {
    tasks: Vec<Task>,
    prefix: String,
    members: Vec<Person>,
    user_id: String,
    /// The newest `updated_at_ms` seen: the next refresh asks only for what
    /// changed after it. `None` until the first full list lands.
    last_updated_ms: Option<i64>,
}

impl TasksStore {
    /// Open on the cached list, if there is one. Members and the user follow
    /// on [`TasksStore::refresh`] and [`TasksStore::load_members`].
    pub fn new(org: &str) -> Self {
        let cached = super::cache::load(org);
        let last_updated_ms = cached
            .as_ref()
            .map(|cached| cached.tasks.iter().map(|task| task.updated_at_ms).max())
            .unwrap_or(None);
        TasksStore {
            tasks: cached
                .as_ref()
                .map(|cached| cached.tasks.clone())
                .unwrap_or_default(),
            prefix: cached.map(|cached| cached.prefix).unwrap_or_default(),
            members: Vec::new(),
            user_id: current_user_id(),
            last_updated_ms,
        }
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// The organisation's key prefix (`DRA`), once a list has landed.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn members(&self) -> &[Person] {
        &self.members
    }

    /// The signed-in user's id, for the "assigned to me" filter.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// Whether a full list has landed (from cache or a refresh).
    pub fn has_tasks(&self) -> bool {
        self.last_updated_ms.is_some() || !self.tasks.is_empty()
    }

    /// A task by its key (`DRA-12`, any case) or id.
    pub fn by_key(&self, key_or_id: &str) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|task| task.id == key_or_id || task.key.eq_ignore_ascii_case(key_or_id))
    }

    /// Bring the list up to date: a full list the first time (following
    /// every page), then only what changed since, removing what was
    /// deleted. The cache is rewritten on success.
    pub fn refresh(&mut self, org: &str) -> Result<RefreshReport, String> {
        let full = self.last_updated_ms.is_none();
        let mut query = ListQuery::default();
        if let Some(updated_after_ms) = (!full).then_some(self.last_updated_ms).flatten() {
            query.updated_after_ms = Some(updated_after_ms);
        }
        let mut report = RefreshReport {
            full,
            ..Default::default()
        };
        if full {
            self.tasks.clear();
        }
        loop {
            let answer = super::client::list(org, &query)?;
            if !answer.prefix.is_empty() {
                self.prefix = answer.prefix;
            }
            report.changed += answer.tasks.len();
            report.deleted += answer.deleted_ids.len();
            self.merge(answer.tasks, &answer.deleted_ids);
            match answer.next_cursor {
                Some(cursor) if !cursor.is_empty() => query.cursor = Some(cursor),
                _ => break,
            }
        }
        let _ = super::cache::save(
            org,
            &super::cache::stamped(self.tasks.clone(), self.prefix.clone()),
        );
        Ok(report)
    }

    /// Fold changed tasks in and deleted ones out. Changed tasks replace by
    /// id; unknown ones join.
    pub fn merge(&mut self, changed: Vec<Task>, deleted_ids: &[String]) {
        for task in changed {
            self.apply_task(task);
        }
        if !deleted_ids.is_empty() {
            self.tasks
                .retain(|task| !deleted_ids.iter().any(|id| id == &task.id));
        }
        self.last_updated_ms = self.tasks.iter().map(|task| task.updated_at_ms).max();
    }

    /// Show a task a mutation just returned, without waiting for refresh.
    pub fn apply_task(&mut self, task: Task) {
        match self.tasks.iter_mut().find(|known| known.id == task.id) {
            Some(known) => *known = task,
            None => self.tasks.push(task),
        }
        self.last_updated_ms = self
            .last_updated_ms
            .into_iter()
            .chain(self.tasks.iter().map(|task| task.updated_at_ms))
            .max();
    }

    /// Forget a task known to be deleted.
    pub fn remove_task(&mut self, id: &str) {
        self.tasks.retain(|task| task.id != id);
    }

    /// Fetch the organisation's members and re-read the signed-in user.
    pub fn load_members(&mut self, org: &str) -> Result<(), String> {
        self.members = super::client::members(org)?;
        self.user_id = current_user_id();
        Ok(())
    }

    /// The store's tasks through `filter`, sorted for the list.
    pub fn filtered(&self, filter: &TaskFilter) -> Vec<Task> {
        let mut tasks = filter_tasks(&self.tasks, filter, &self.user_id);
        sort_tasks(&mut tasks);
        tasks
    }

    /// Display name for a member id: name, else email, else the id.
    pub fn member_name(&self, id: &str) -> String {
        self.members
            .iter()
            .find(|member| member.id == id)
            .map(|member| {
                let shown = member.display();
                if shown == "Someone" {
                    id.into()
                } else {
                    shown.into()
                }
            })
            .unwrap_or_else(|| id.into())
    }
}

/// The signed-in user's id, from the live organisation or what the app last
/// cached.
fn current_user_id() -> String {
    crate::cli::live_organisation()
        .or_else(crate::organisation_context::cached_active)
        .map(|active| active.user_id)
        .unwrap_or_default()
}

/// The tasks matching `filter`. Pure, so the page and the tests share it.
pub fn filter_tasks(tasks: &[Task], filter: &TaskFilter, me: &str) -> Vec<Task> {
    let words: Vec<String> = filter
        .search
        .split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect();
    let label = filter.label.as_deref().map(str::to_ascii_lowercase);
    tasks
        .iter()
        .filter(|task| match &filter.status {
            StatusFilter::Open => task.status.is_open(),
            StatusFilter::Closed => !task.status.is_open(),
            StatusFilter::All => true,
            StatusFilter::Keys(keys) => keys.contains(&task.status),
        })
        .filter(|task| match &filter.assignee {
            AssigneeFilter::Any => true,
            AssigneeFilter::Me => task
                .assignee
                .as_ref()
                .is_some_and(|assignee| assignee.id == me),
            AssigneeFilter::None => task.assignee.is_none(),
            AssigneeFilter::User(id) => task
                .assignee
                .as_ref()
                .is_some_and(|assignee| &assignee.id == id),
        })
        .filter(|task| {
            label.as_ref().is_none_or(|label| {
                task.labels
                    .iter()
                    .any(|task| task.to_ascii_lowercase() == *label)
            })
        })
        .filter(|task| {
            filter.project.as_ref().is_none_or(|wanted| {
                task.project
                    .as_ref()
                    .is_some_and(|project| &project.key == wanted)
            })
        })
        .filter(|task| {
            words.iter().all(|word| {
                task.key.to_ascii_lowercase().contains(word)
                    || task.title.to_ascii_lowercase().contains(word)
                    || task.description.to_ascii_lowercase().contains(word)
            })
        })
        .cloned()
        .collect()
}

fn status_order(status: Status) -> usize {
    Status::ALL
        .iter()
        .position(|known| *known == status)
        .unwrap_or(usize::MAX)
}

/// The list order: status first, then most urgent, then most recently
/// updated.
pub fn sort_tasks(tasks: &mut [Task]) {
    tasks.sort_by(|a, b| {
        status_order(a.status)
            .cmp(&status_order(b.status))
            .then(a.priority.rank().cmp(&b.priority.rank()))
            .then(b.updated_at_ms.cmp(&a.updated_at_ms))
            .then(a.key.cmp(&b.key))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_tasks::{Priority, Status};

    fn task(key: &str, status: Status, priority: Priority, updated: i64) -> Task {
        Task {
            id: format!("id-{key}"),
            key: key.into(),
            number: 1,
            title: format!("Fix {key}"),
            description: format!("about {key}"),
            status,
            priority,
            assignee: None,
            creator: None,
            project: None,
            labels: Vec::new(),
            due_at_ms: None,
            created_at_ms: 1,
            updated_at_ms: updated,
            started_at_ms: None,
            completed_at_ms: None,
            links: Vec::new(),
            comment_count: 0,
        }
    }

    fn assigned(mut task: Task, id: &str) -> Task {
        task.assignee = Some(Person {
            id: id.into(),
            name: String::new(),
            email: format!("{id}@example.com"),
        });
        task
    }

    fn store() -> TasksStore {
        TasksStore {
            tasks: vec![
                assigned(task("DRA-1", Status::Todo, Priority::High, 10), "me"),
                task("DRA-2", Status::Done, Priority::Urgent, 30),
                assigned(task("DRA-3", Status::InProgress, Priority::Low, 20), "them"),
                Task {
                    labels: vec!["Bug".into()],
                    ..task("DRA-4", Status::Backlog, Priority::None, 5)
                },
            ],
            prefix: "DRA".into(),
            members: vec![Person {
                id: "me".into(),
                name: "Me".into(),
                email: "me@example.com".into(),
            }],
            user_id: "me".into(),
            last_updated_ms: Some(30),
        }
    }

    #[test]
    fn filters_narrow_by_status_assignee_label_project_and_words() {
        let all = store();
        assert_eq!(
            store().filtered(&TaskFilter::default()).len(),
            3,
            "open by default"
        );
        assert_eq!(
            store()
                .filtered(&TaskFilter {
                    status: StatusFilter::Closed,
                    ..Default::default()
                })
                .len(),
            1
        );
        let mine = store().filtered(&TaskFilter {
            assignee: AssigneeFilter::Me,
            ..Default::default()
        });
        assert_eq!(
            mine.iter()
                .map(|task| task.key.as_str())
                .collect::<Vec<_>>(),
            ["DRA-1"]
        );
        let unassigned = store().filtered(&TaskFilter {
            status: StatusFilter::All,
            assignee: AssigneeFilter::None,
            ..Default::default()
        });
        assert_eq!(unassigned.len(), 2);
        let bugs = store().filtered(&TaskFilter {
            status: StatusFilter::All,
            label: Some("bug".into()),
            ..Default::default()
        });
        assert_eq!(
            bugs.iter()
                .map(|task| task.key.as_str())
                .collect::<Vec<_>>(),
            ["DRA-4"]
        );
        let keys = store().filtered(&TaskFilter {
            status: StatusFilter::Keys(vec![Status::Done, Status::Backlog]),
            ..Default::default()
        });
        assert_eq!(keys.len(), 2);
        let words = store().filtered(&TaskFilter {
            status: StatusFilter::All,
            search: "fix DRA-3".into(),
            ..Default::default()
        });
        assert_eq!(
            words
                .iter()
                .map(|task| task.key.as_str())
                .collect::<Vec<_>>(),
            ["DRA-3"]
        );
        let missing = store().filtered(&TaskFilter {
            status: StatusFilter::All,
            search: "fix absent".into(),
            ..Default::default()
        });
        assert!(missing.is_empty(), "every word must match");
    }

    #[test]
    fn lists_sort_by_status_then_priority_then_recency() {
        let mut tasks = vec![
            task("DRA-9", Status::Done, Priority::Urgent, 99),
            task("DRA-1", Status::Todo, Priority::Low, 10),
            task("DRA-2", Status::Todo, Priority::High, 5),
            task("DRA-3", Status::Backlog, Priority::Urgent, 1),
            task("DRA-4", Status::Todo, Priority::High, 50),
        ];
        sort_tasks(&mut tasks);
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.key.as_str())
                .collect::<Vec<_>>(),
            ["DRA-3", "DRA-4", "DRA-2", "DRA-1", "DRA-9"]
        );
    }

    #[test]
    fn merging_applies_changes_removes_deletions_and_moves_the_cursor() {
        let mut all = store();
        all.merge(
            vec![
                Task {
                    title: "Renamed".into(),
                    updated_at_ms: 40,
                    ..task("DRA-1", Status::InProgress, Priority::High, 40)
                },
                task("DRA-5", Status::Todo, Priority::Medium, 35),
            ],
            &["id-DRA-3".to_string()],
        );
        assert_eq!(all.by_key("dra-1").unwrap().title, "Renamed");
        assert!(all.by_key("DRA-3").is_none());
        assert!(all.by_key("DRA-5").is_some());
        assert_eq!(all.last_updated_ms, Some(40));
    }

    #[test]
    fn an_applied_task_replaces_by_id_and_a_removed_one_leaves() {
        let mut all = store();
        all.apply_task(Task {
            title: "Renamed".into(),
            ..task("DRA-2", Status::Done, Priority::Urgent, 30)
        });
        assert_eq!(
            all.tasks
                .iter()
                .filter(|task| task.id == "id-DRA-2")
                .count(),
            1
        );
        assert_eq!(all.by_key("DRA-2").unwrap().title, "Renamed");
        all.remove_task("id-DRA-2");
        assert!(all.by_key("DRA-2").is_none());
    }

    #[test]
    fn member_names_fall_back_to_email_then_id() {
        let all = store();
        assert_eq!(all.member_name("me"), "Me");
        assert_eq!(all.member_name("nobody"), "nobody");
    }
}
