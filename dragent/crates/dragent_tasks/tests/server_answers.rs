//! Answers recorded from the server's own handler: the desktop must read
//! them. Regenerate them when the server's answers change on purpose.

use dragent_tasks::{ActivityKind, ActorKind, LinkKind, ListResponse, Priority, Status, TaskDetail};

#[test]
fn a_task_page_answer_reads_whole() {
    let detail: TaskDetail = serde_json::from_str(include_str!("fixtures/server_detail.json")).unwrap();
    let task = &detail.task;
    assert_eq!(task.key, "DRA-1");
    assert_eq!((task.status, task.priority), (Status::InProgress, Priority::High));
    assert_eq!(task.labels, ["bug"]);
    assert_eq!(task.project.as_ref().map(|project| project.key.as_str()), Some("github.com/acme/app"));
    assert!(task.started_at_ms.is_some() && task.completed_at_ms.is_none());
    assert_eq!(task.assignee.as_ref().map(|person| person.name.as_str()), Some("Ada"));
    assert_eq!(task.comment_count, 1);
    assert_eq!(task.links_of(LinkKind::PullRequest).next().and_then(|link| link.state.as_deref()), Some("open"));
    assert_eq!(detail.comments[0].body, "On it");
    let status_change = detail.activity.iter().find(|entry| entry.kind == ActivityKind::Status).unwrap();
    assert_eq!(status_change.actor_kind, ActorKind::Agent);
    assert_eq!(status_change.to.as_ref().and_then(|value| value.as_str()), Some("in_progress"));
}

#[test]
fn a_list_answer_reads_whole() {
    let list: ListResponse = serde_json::from_str(include_str!("fixtures/server_list.json")).unwrap();
    assert_eq!(list.prefix, "DRA");
    assert_eq!(list.tasks.len(), 1);
    assert!(list.deleted_ids.is_empty() && list.next_cursor.is_none());
}
