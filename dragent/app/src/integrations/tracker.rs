//! Connected trackers' items as task-like rows: the Tasks page shows these
//! beside Dragent's own tasks, in the same list and board.
//!
//! A [`TrackerItem`] is one of [`tracker_providers`] services' items with
//! its state mapped onto our fixed [`Status`] scale. States and statuses
//! move through the existing [`items`] requests, so nothing here talks to a
//! service itself.

use std::path::Path;

use dragent_tasks::Status;

use super::items::{Item, ItemState, StateCategory};
use super::Provider;

/// An item from a connected tracker shaped like a task row.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackerItem {
    pub provider: Provider,
    pub key: String,
    pub title: String,
    pub url: String,
    /// Its state on our scale (see [`status_from`]).
    pub status: Status,
    /// The service's own word for its state ("In Progress"), when it said.
    pub state_label: Option<String>,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub updated_at_ms: Option<i64>,
    pub branch: String,
    /// The underlying item, for reading, commenting and linking.
    pub item: Item,
}

impl TrackerItem {
    pub fn from_item(item: Item) -> Self {
        let key = if item.key.trim().is_empty() {
            item.id().to_string()
        } else {
            item.key.clone()
        };
        let (status, state_label) = match &item.state {
            Some(state) => (status_from(state), Some(state.label.clone())),
            None => (Status::Todo, None),
        };
        TrackerItem {
            provider: item.provider,
            title: item.title.clone(),
            url: item.url.clone(),
            branch: item.branch.clone(),
            assignee: item.assignee.clone(),
            labels: item.labels.clone(),
            updated_at_ms: item.updated_at_ms,
            key,
            status,
            state_label,
            item,
        }
    }

    /// What [`items::get`](super::items::get) and friends take to find it again.
    pub fn id(&self) -> &str {
        self.item.id()
    }
}

/// The services whose items are work to do: the code hosts and trackers.
/// Chat, mail, docs and error monitoring are not: their items start work but
/// never sit on the board. Callers check
/// [`is_connected`](super::is_connected) themselves.
pub fn tracker_providers() -> Vec<Provider> {
    vec![
        Provider::Github,
        Provider::Gitlab,
        Provider::Forgejo,
        Provider::Linear,
        Provider::Jira,
        Provider::Plane,
        Provider::Youtrack,
        Provider::Asana,
        Provider::Trello,
        Provider::Monday,
    ]
}

/// The trackers this person has connected: a saved connection, or for
/// GitHub a signed-in GitHub CLI. Blocking (keychain, `gh`): run it off the
/// main thread.
pub fn connected_trackers() -> Vec<Provider> {
    tracker_providers()
        .into_iter()
        .filter(|provider| matches!(super::status(*provider), super::Status::Connected { .. }))
        .collect()
}

fn is_tracker(provider: Provider) -> bool {
    tracker_providers().contains(&provider)
}

/// A tracker's items as task rows: its open ones when `words` is empty,
/// like today's search, else the ones matching every word.
pub fn list(provider: Provider, root: &Path, words: &str) -> Result<Vec<TrackerItem>, String> {
    if !is_tracker(provider) {
        return Err(format!(
            "{} isn't a work tracker: the Tasks page only lists {}.",
            provider.label(),
            readable_providers(),
        ));
    }
    Ok(super::items::search(provider, root, words)?
        .into_iter()
        .map(TrackerItem::from_item)
        .collect())
}

/// Move `item` to `status` on its service, through the existing
/// [`items::set_status`](super::items::set_status). Done and canceled close
/// or resolve it, back to a todo state reopens it, and in progress or in
/// review moves it there where the service has such a state. Answers the
/// service's own state name afterwards, or a clear error when the service
/// can't do that.
pub fn set_status(root: &Path, item: &TrackerItem, status: Status) -> Result<String, String> {
    let wanted = status_target(item.provider, status)?;
    super::items::set_status(item.provider, root, item.id(), wanted)
}

/// Whether pushing `status` to `provider` can work at all: some services
/// simply have no such state (a GitHub issue is only open or closed).
pub fn supports_status(provider: Provider, status: Status) -> bool {
    status_target(provider, status).is_ok()
}

/// Best-effort push of a native status to linked tracker items, after the
/// native change itself succeeded. Providers with no such state are
/// skipped; answers the failures that really failed.
pub fn push_status(root: &Path, items: &[(Provider, String)], status: Status) -> Vec<String> {
    let mut failures = Vec::new();
    for (provider, key) in items {
        if !supports_status(*provider, status) {
            continue;
        }
        let item = TrackerItem::from_item(Item {
            provider: *provider,
            key: key.clone(),
            title: String::new(),
            url: String::new(),
            body: String::new(),
            detail: None,
            branch: String::new(),
            reference: None,
            state: None,
            assignee: None,
            labels: Vec::new(),
            updated_at_ms: None,
        });
        if let Err(error) = set_status(root, &item, status) {
            failures.push(error);
        }
    }
    failures
}

/// Our status on the service's words, for [`set_status`].
fn status_target(provider: Provider, status: Status) -> Result<&'static str, String> {
    match provider {
        // Open or closed, nothing between.
        Provider::Github | Provider::Gitlab | Provider::Forgejo => match status {
            Status::Done | Status::Canceled => Ok("closed"),
            Status::Backlog | Status::Todo => Ok("open"),
            Status::InProgress | Status::InReview => Err(format!(
                "{} issues are either open or closed: they have no “{}” state.",
                provider.label(),
                status.label(),
            )),
        },
        Provider::Asana => match status {
            Status::Done | Status::Canceled => Ok("complete"),
            Status::Backlog | Status::Todo => Ok("incomplete"),
            Status::InProgress | Status::InReview => Err(format!(
                "{} tasks are either complete or incomplete: they have no “{}” state.",
                provider.label(),
                status.label(),
            )),
        },
        Provider::Trello => match status {
            Status::Done | Status::Canceled => Ok("closed"),
            Status::Backlog | Status::Todo => Ok("Todo"),
            Status::InProgress => Ok("In Progress"),
            Status::InReview => Ok("In Review"),
        },
        Provider::Youtrack => match status {
            Status::Backlog | Status::Todo => Ok("Open"),
            Status::InProgress => Ok("In Progress"),
            Status::InReview => Ok("In Review"),
            Status::Done => Ok("Fixed"),
            Status::Canceled => Ok("Canceled"),
        },
        Provider::Linear
        | Provider::Jira
        | Provider::Plane
        | Provider::Monday
        | Provider::Notion
        | Provider::Featurebase
        | Provider::Plain
        | Provider::Sentry
        | Provider::Slack
        | Provider::Teams
        | Provider::Gmail => Ok(match status {
            Status::Backlog => "Backlog",
            Status::Todo => "Todo",
            Status::InProgress => "In Progress",
            Status::InReview => "In Review",
            Status::Done => "Done",
            Status::Canceled => "Canceled",
        }),
    }
}

/// Our status for a service's state.
pub fn status_from(state: &ItemState) -> Status {
    let label = state.label.trim().to_lowercase();
    match state.category {
        StateCategory::Open if label.contains("backlog") => Status::Backlog,
        StateCategory::Open => Status::Todo,
        StateCategory::Started if label.contains("review") => Status::InReview,
        StateCategory::Started => Status::InProgress,
        StateCategory::Done => Status::Done,
        StateCategory::Canceled => Status::Canceled,
    }
}

fn readable_providers() -> String {
    tracker_providers()
        .iter()
        .map(|provider| provider.label())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::items::ItemState;

    fn item(provider: Provider) -> TrackerItem {
        TrackerItem::from_item(Item {
            provider,
            key: "ENG-12".into(),
            title: "Retry uploads".into(),
            url: "https://linear.app/a/issue/ENG-12".into(),
            body: String::new(),
            detail: None,
            branch: "eng-12-retry-uploads".into(),
            reference: None,
            state: Some(ItemState::started("In Progress")),
            assignee: Some("Ada".into()),
            labels: vec!["backend".into()],
            updated_at_ms: Some(1_792_843_200_000),
        })
    }

    #[test]
    fn only_code_hosts_and_trackers_sit_on_the_board() {
        let providers = tracker_providers();
        for provider in [
            Provider::Github,
            Provider::Gitlab,
            Provider::Forgejo,
            Provider::Linear,
            Provider::Jira,
            Provider::Plane,
            Provider::Youtrack,
            Provider::Asana,
            Provider::Trello,
            Provider::Monday,
        ] {
            assert!(providers.contains(&provider), "{provider:?} is work to do");
        }
        for provider in [
            Provider::Notion,
            Provider::Featurebase,
            Provider::Plain,
            Provider::Sentry,
            Provider::Slack,
            Provider::Teams,
            Provider::Gmail,
        ] {
            assert!(!providers.contains(&provider), "{provider:?} is not");
        }
    }

    #[test]
    fn states_map_onto_our_statuses() {
        for (category, label, status) in [
            (StateCategory::Open, "Open", Status::Todo),
            (StateCategory::Open, "Backlog", Status::Backlog),
            (StateCategory::Open, "To Do", Status::Todo),
            (StateCategory::Started, "In Progress", Status::InProgress),
            (StateCategory::Started, "Doing", Status::InProgress),
            (StateCategory::Started, "In Review", Status::InReview),
            (StateCategory::Done, "Closed", Status::Done),
            (StateCategory::Done, "Merged", Status::Done),
            (StateCategory::Canceled, "Canceled", Status::Canceled),
        ] {
            assert_eq!(
                status_from(&ItemState::new(category, label)),
                status,
                "{label}"
            );
        }
    }

    #[test]
    fn items_become_rows_with_their_state_and_holder() {
        let row = item(Provider::Linear);
        assert_eq!(
            (
                row.provider,
                row.key.as_str(),
                row.status,
                row.state_label.as_deref(),
                row.assignee.as_deref(),
                row.branch.as_str(),
            ),
            (
                Provider::Linear,
                "ENG-12",
                Status::InProgress,
                Some("In Progress"),
                Some("Ada"),
                "eng-12-retry-uploads",
            )
        );
        assert_eq!(row.labels, vec!["backend"]);
        assert_eq!(row.updated_at_ms, Some(1_792_843_200_000));
        assert_eq!(row.id(), "ENG-12");
    }

    #[test]
    fn rows_without_a_state_are_todo_and_borrow_their_id() {
        let mut bare = item(Provider::Asana);
        bare.item.key.clear();
        bare.item.reference = Some("1204567890123".into());
        bare.item.state = None;
        let row = TrackerItem::from_item(bare.item);
        assert_eq!(row.key, "1204567890123");
        assert_eq!((row.status, row.state_label), (Status::Todo, None));
    }

    #[test]
    fn statuses_move_services_their_own_way() {
        assert_eq!(
            status_target(Provider::Github, Status::Done),
            Ok("closed")
        );
        assert_eq!(
            status_target(Provider::Github, Status::Backlog),
            Ok("open")
        );
        assert_eq!(
            status_target(Provider::Asana, Status::Canceled),
            Ok("complete")
        );
        assert_eq!(status_target(Provider::Trello, Status::Done), Ok("closed"));
        assert_eq!(
            status_target(Provider::Trello, Status::InReview),
            Ok("In Review")
        );
        assert_eq!(
            status_target(Provider::Youtrack, Status::Done),
            Ok("Fixed")
        );
        assert_eq!(
            status_target(Provider::Youtrack, Status::Todo),
            Ok("Open")
        );
        assert_eq!(
            status_target(Provider::Linear, Status::InProgress),
            Ok("In Progress")
        );
        assert_eq!(status_target(Provider::Monday, Status::Done), Ok("Done"));
    }

    #[test]
    fn services_without_a_state_say_so() {
        let error = status_target(Provider::Github, Status::InProgress).unwrap_err();
        assert!(
            error.contains("either open or closed") && error.contains("In progress"),
            "{error}"
        );
        let error = status_target(Provider::Asana, Status::InReview).unwrap_err();
        assert!(
            error.contains("either complete or incomplete") && error.contains("In review"),
            "{error}"
        );
    }

    #[test]
    fn only_trackers_list_and_move_without_talking_to_anyone() {
        let root = std::path::Path::new("/nonexistent");
        let error = list(Provider::Slack, root, "").unwrap_err();
        assert!(error.contains("isn't a work tracker"), "{error}");
        let row = item(Provider::Github);
        let error = set_status(root, &row, Status::InProgress).unwrap_err();
        assert!(error.contains("either open or closed"), "{error}");
    }

    #[test]
    fn status_push_skips_states_a_service_does_not_have() {
        assert!(supports_status(Provider::Linear, Status::InProgress));
        assert!(supports_status(Provider::Jira, Status::Done));
        assert!(supports_status(Provider::Github, Status::Done));
        assert!(supports_status(Provider::Github, Status::Todo));
        assert!(!supports_status(Provider::Github, Status::InProgress));
        assert!(!supports_status(Provider::Github, Status::InReview));
        assert!(!supports_status(Provider::Asana, Status::InProgress));
        assert!(supports_status(Provider::Trello, Status::InProgress));
        // push_status skips the impossible silently: no service is
        // contacted, so nothing can fail.
        let root = std::path::Path::new("/nonexistent");
        let failures = push_status(root, &[(Provider::Github, "#1".into())], Status::InProgress);
        assert!(failures.is_empty());
    }
}
