//! Items from connected services (issues, tasks, cards, threads, errors,
//! messages, emails) in one shape whatever the service.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Provider;

/// How much of an item's text travels with it.
pub(crate) const BODY_CHARS: usize = 12_000;
/// Results per search.
pub(crate) const LIMIT: usize = 40;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub provider: Provider,
    /// How the service names it: ENG-12, #42, BACKEND-3K, #general.
    pub key: String,
    pub title: String,
    pub url: String,
    pub body: String,
    /// Where it lives or what state it's in: a project, a board, a channel.
    #[serde(default)]
    pub detail: Option<String>,
    /// The branch to work on it in: the service's own suggestion, else one
    /// made the way such services make them.
    pub branch: String,
    /// The service's own id for fetching the rest of it (see [`details`]).
    #[serde(default)]
    pub reference: Option<String>,
    /// How finished it is, when the service said (see [`ItemState`]).
    /// Older stored items have none.
    #[serde(default)]
    pub state: Option<ItemState>,
    /// Who holds it, as the service names them.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Its tags, as the service names them.
    #[serde(default)]
    pub labels: Vec<String>,
    /// When it last changed, as Unix milliseconds, when known.
    #[serde(default)]
    pub updated_at_ms: Option<i64>,
}

/// How finished an item is, whatever the service calls it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateCategory {
    /// Still to do.
    Open,
    /// Being worked on.
    Started,
    /// Finished.
    Done,
    /// Dropped.
    Canceled,
}

/// An item's state: how finished it is, and the service's own word for it
/// ("In Progress", "archived").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemState {
    pub category: StateCategory,
    /// What people read: the service's own state name.
    #[serde(default)]
    pub label: String,
}

impl ItemState {
    pub fn new(category: StateCategory, label: impl Into<String>) -> Self {
        ItemState {
            category,
            label: label.into(),
        }
    }

    pub fn open(label: impl Into<String>) -> Self {
        Self::new(StateCategory::Open, label)
    }

    pub fn started(label: impl Into<String>) -> Self {
        Self::new(StateCategory::Started, label)
    }

    pub fn done(label: impl Into<String>) -> Self {
        Self::new(StateCategory::Done, label)
    }

    pub fn canceled(label: impl Into<String>) -> Self {
        Self::new(StateCategory::Canceled, label)
    }
}

impl StateCategory {
    /// A guess from a state name, for services without typed states: done
    /// words finish it, canceled words drop it, started words are work in
    /// progress, the rest is still to do.
    pub fn from_name(name: &str) -> Self {
        let name = name.trim().to_lowercase().replace(['_', '-'], " ");
        let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
        if has(&[
            "cancel", "abandon", "obsolete", "duplicate", "won fix", "wont fix", "declined",
            "rejected",
        ]) {
            StateCategory::Canceled
        } else if has(&[
            "done", "complete", "resolved", "fixed", "closed", "merged", "archive", "shipped",
            "verified", "finish",
        ]) {
            StateCategory::Done
        } else if has(&[
            "progress", "review", "doing", "working", "started", "stuck", "develop", "test",
        ]) {
            StateCategory::Started
        } else {
            StateCategory::Open
        }
    }
}

impl Item {
    /// What [`get`], [`comment`] and [`set_status`] take to find it again:
    /// the service's own id where its key isn't enough (a Trello card, an
    /// issue in another GitLab project, a Slack thread).
    pub fn id(&self) -> &str {
        self.reference
            .as_deref()
            .filter(|reference| !reference.is_empty())
            .unwrap_or(&self.key)
    }
}

/// What can be done with a service's items besides finding them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Abilities {
    /// Fetch one in full, with its comments where the service has them.
    pub read: bool,
    pub comment: bool,
    /// Move it to another status (close, resolve, a workflow state).
    pub status: bool,
    pub create: bool,
}

pub fn abilities(provider: Provider) -> Abilities {
    let all = Abilities {
        read: true,
        comment: true,
        status: true,
        create: true,
    };
    match provider {
        Provider::Github
        | Provider::Gitlab
        | Provider::Forgejo
        | Provider::Linear
        | Provider::Jira
        | Provider::Plane
        | Provider::Youtrack
        | Provider::Asana => all,
        Provider::Trello | Provider::Monday | Provider::Plain | Provider::Sentry => Abilities {
            create: false,
            ..all
        },
        Provider::Notion | Provider::Slack => Abilities {
            read: true,
            comment: true,
            ..Abilities::default()
        },
        Provider::Featurebase | Provider::Gmail => Abilities {
            read: true,
            ..Abilities::default()
        },
        Provider::Teams => Abilities::default(),
    }
}

/// A new item to file with a service.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    /// Where it goes, as the service names it: a Linear team, a Jira or
    /// YouTrack project key, a GitLab project path, an `owner/repo`. Empty
    /// for the project's own repository or the account's only team.
    pub place: Option<String>,
}

fn connected(provider: Provider) -> Result<super::Connection, String> {
    super::connection(provider).ok_or_else(|| {
        format!(
            "{} isn't connected. Connect it in Dragent under Settings → Integrations.",
            provider.label()
        )
    })
}

fn unable(provider: Provider, what: &str) -> String {
    format!("Dragent can't {what} with {} yet.", provider.label())
}

/// Items matching `query` (recent, open or assigned ones when it's empty,
/// where the service can list without a search). `project_root` picks the
/// repository for code hosts. Blocking.
pub fn search(provider: Provider, project_root: &Path, query: &str) -> Result<Vec<Item>, String> {
    let query = query.trim();
    if provider == Provider::Github {
        return super::services::github::search(project_root, query);
    }
    let mut connection = connected(provider)?;
    super::services::search(&mut connection, project_root, query)
}

/// One item in full: its text and, where the service has them, its
/// comments. `id` is an [`Item::id`]. Blocking.
pub fn get(provider: Provider, project_root: &Path, id: &str) -> Result<Item, String> {
    let id = id.trim();
    if !abilities(provider).read {
        return Err(unable(provider, "read items"));
    }
    let mut item = if provider == Provider::Github {
        super::services::github::read(project_root, id)?
    } else {
        super::services::get(&mut connected(provider)?, project_root, id)?
    };
    item.body = clip(&item.body);
    Ok(item)
}

/// Add a comment to an item, as the connected account. Blocking.
pub fn comment(
    provider: Provider,
    project_root: &Path,
    id: &str,
    text: &str,
) -> Result<(), String> {
    if !abilities(provider).comment {
        return Err(unable(provider, "comment"));
    }
    if text.trim().is_empty() {
        return Err("Write the comment first.".into());
    }
    super::policy::check_writes()?;
    if provider == Provider::Github {
        return super::services::github::comment(project_root, id.trim(), text.trim());
    }
    super::services::comment(
        &mut connected(provider)?,
        project_root,
        id.trim(),
        text.trim(),
    )
}

/// Move an item to `status` (a name the service shows, matched loosely).
/// Returns the status it's in now. Blocking.
pub fn set_status(
    provider: Provider,
    project_root: &Path,
    id: &str,
    status: &str,
) -> Result<String, String> {
    if !abilities(provider).status {
        return Err(unable(provider, "change statuses"));
    }
    if status.trim().is_empty() {
        return Err("Say which status to move it to.".into());
    }
    super::policy::check_writes()?;
    if provider == Provider::Github {
        return super::services::github::set_status(project_root, id.trim(), status.trim());
    }
    super::services::set_status(
        &mut connected(provider)?,
        project_root,
        id.trim(),
        status.trim(),
    )
}

/// File a new item. Blocking. When `place` is empty the Tasks defaults
/// (default repo / Linear team) fill it, so new work lands where configured.
pub fn create(provider: Provider, project_root: &Path, draft: &Draft) -> Result<Item, String> {
    if !abilities(provider).create {
        return Err(unable(provider, "create items"));
    }
    if draft.title.trim().is_empty() {
        return Err("Give it a title.".into());
    }
    super::policy::check_writes()?;
    let explicit = draft
        .place
        .as_deref()
        .map(str::trim)
        .filter(|place| !place.is_empty())
        .map(str::to_string);
    let place = explicit.or_else(|| {
        // Tasks defaults; AppSettings::global() is cheap and never touches disk here.
        crate::settings::AppSettings::global()
            .tasks
            .default_place(provider.key())
    });
    let draft = Draft {
        title: draft.title.trim().to_string(),
        body: draft.body.trim().to_string(),
        place,
    };
    if provider == Provider::Github {
        return super::services::github::create(project_root, &draft);
    }
    super::services::create(&mut connected(provider)?, project_root, &draft)
}

/// Whether the service only finds items for a search (it can't list
/// recent ones).
pub fn needs_query(provider: Provider) -> bool {
    matches!(provider, Provider::Slack | Provider::Teams)
}

/// The full text of `item`, for services whose search returns only a
/// summary (a Notion page's blocks, a Slack thread, an email). `None`
/// when the search already brought it all. Blocking.
pub fn details(item: &Item) -> Result<Option<String>, String> {
    if item.reference.is_none() || item.provider == Provider::Github {
        return Ok(None);
    }
    let Some(mut connection) = super::connection(item.provider) else {
        return Ok(None);
    };
    super::services::details(&mut connection, item).map(|body| body.map(|body| clip(&body)))
}

pub(crate) fn clip(text: &str) -> String {
    text.chars().take(BODY_CHARS).collect()
}

/// A branch-safe slug of `title`.
pub(crate) fn slug(title: &str, max: usize) -> String {
    let mut slug = String::new();
    for character in title.chars() {
        if slug.len() >= max {
            break;
        }
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// `{prefix}-{slug}`, the way code hosts and trackers name item branches.
pub(crate) fn branch_for(prefix: &str, title: &str) -> String {
    let prefix = prefix.trim_start_matches('#');
    let slug = slug(title, 40);
    match (prefix.is_empty(), slug.is_empty()) {
        (_, true) => prefix.to_string(),
        (true, false) => slug,
        (false, false) => format!("{prefix}-{slug}"),
    }
}

/// `body`, then the comments (author and text, oldest first) under a
/// heading of their own.
pub(crate) fn with_comments(body: &str, comments: &[(String, String)]) -> String {
    let comments: Vec<String> = comments
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(author, text)| {
            let author = if author.trim().is_empty() {
                "Someone"
            } else {
                author.trim()
            };
            format!("{author}: {}", text.trim())
        })
        .collect();
    let body = body.trim();
    if comments.is_empty() {
        return body.to_string();
    }
    let comments = format!("Comments:\n\n{}", comments.join("\n\n"));
    if body.is_empty() {
        comments
    } else {
        format!("{body}\n\n{comments}")
    }
}

/// The option `wanted` names among `options` (id, name): the same name in
/// any case, else the only one starting with it, else the only one
/// containing it. `what` names them in the error ("Linear statuses").
pub(crate) fn choose<'a>(
    wanted: &str,
    options: &'a [(String, String)],
    what: &str,
) -> Result<&'a (String, String), String> {
    let wanted = wanted.trim().to_lowercase();
    let exact: Vec<_> = options
        .iter()
        .filter(|(_, name)| name.to_lowercase() == wanted)
        .collect();
    let found = if exact.len() == 1 {
        exact
    } else {
        let starting: Vec<_> = options
            .iter()
            .filter(|(_, name)| name.to_lowercase().starts_with(&wanted))
            .collect();
        if starting.len() == 1 {
            starting
        } else {
            options
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&wanted))
                .collect()
        }
    };
    match found.as_slice() {
        [one] => Ok(one),
        _ => {
            let names: Vec<&str> = options.iter().map(|(_, name)| name.as_str()).collect();
            Err(if names.is_empty() {
                format!("There are no {what} to choose from.")
            } else {
                format!(
                    "No single one of the {what} matches “{}”. They are: {}.",
                    wanted,
                    names.join(", ")
                )
            })
        }
    }
}

/// The first line of `text`, cut to a title's length.
pub(crate) fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut = cut.trim_end().to_string();
    cut.push('…');
    cut
}

/// When `value` last changed, as Unix milliseconds: a number is already
/// one (seconds when small), a string is read as date and time.
pub(crate) fn updated_ms(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64().map(|ms| {
            if ms.abs() < 100_000_000_000 {
                ms * 1000
            } else {
                ms
            }
        }),
        serde_json::Value::String(text) => parse_time_ms(text.trim()),
        _ => None,
    }
}

/// Date and time text as Unix milliseconds: RFC 3339 first, then the
/// shapes trackers actually send (Jira's `+0000` without a colon).
fn parse_time_ms(text: &str) -> Option<i64> {
    if text.is_empty() {
        return None;
    }
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(time.timestamp_millis());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%d %H:%M:%S%.f %z",
        "%Y-%m-%d",
    ] {
        if let Ok(time) = chrono::DateTime::parse_from_str(text, format) {
            return Some(time.timestamp_millis());
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(text, format) {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|midnight| midnight.and_utc().timestamp_millis());
        }
    }
    None
}

/// `Open` from `opened`: the service's word with its first letter up, for
/// the states services only send lowercase.
pub(crate) fn state_label(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_and_titles_read_cleanly() {
        assert_eq!(
            branch_for("42", "Fix: login fails on Safari!"),
            "42-fix-login-fails-on-safari"
        );
        assert_eq!(branch_for("#7", "Docs"), "7-docs");
        assert_eq!(branch_for("PROJ-7", "  "), "PROJ-7");
        assert_eq!(branch_for("", "Only a title"), "only-a-title");
        assert_eq!(
            first_line("\n\n  Deploy failed on main  \nmore", 80),
            "Deploy failed on main"
        );
        assert_eq!(first_line("abcdefghij", 6), "abcde…");
    }

    #[test]
    fn options_are_chosen_by_name_loosely() {
        let states: Vec<(String, String)> = ["Todo", "In Progress", "In Review", "Done"]
            .iter()
            .enumerate()
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect();
        assert_eq!(choose("done", &states, "statuses").unwrap().1, "Done");
        assert_eq!(
            choose("in prog", &states, "statuses").unwrap().1,
            "In Progress"
        );
        assert_eq!(
            choose("review", &states, "statuses").unwrap().1,
            "In Review"
        );
        let error = choose("in", &states, "Linear statuses").unwrap_err();
        assert!(
            error.contains("Todo, In Progress, In Review, Done"),
            "{error}"
        );
    }

    #[test]
    fn comments_follow_the_text() {
        let comments = vec![
            ("Ada".to_string(), "Repro attached".to_string()),
            ("".to_string(), "Seen on 2.1 too".to_string()),
            ("Bob".to_string(), "  ".to_string()),
        ];
        assert_eq!(
            with_comments("Uploads fail", &comments),
            "Uploads fail\n\nComments:\n\nAda: Repro attached\n\nSomeone: Seen on 2.1 too"
        );
        assert_eq!(with_comments("Body", &[]), "Body");
    }

    #[test]
    fn an_item_is_found_again_by_its_services_id() {
        let mut item = Item {
            provider: Provider::Trello,
            key: "#4".into(),
            title: "Card".into(),
            url: String::new(),
            body: String::new(),
            detail: None,
            branch: String::new(),
            reference: Some("aBcD1234".into()),
            state: Some(ItemState::open("To Do")),
            assignee: Some("Ada".into()),
            labels: vec!["bug".into()],
            updated_at_ms: Some(1_790_000_000_000),
        };
        assert_eq!(item.id(), "aBcD1234");
        item.reference = None;
        assert_eq!(item.id(), "#4");
        assert!(abilities(Provider::Linear).create && !abilities(Provider::Teams).read);
    }

    #[test]
    fn old_stored_items_without_a_state_still_load() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "provider": "trello",
            "key": "#4",
            "title": "Card",
            "url": "https://trello.com/c/aBcD1234",
            "body": "Do it",
            "detail": "Sprint",
            "branch": "abcd1234-card",
            "reference": "aBcD1234",
        }))
        .unwrap();
        assert_eq!(item.state, None);
        assert_eq!(item.assignee, None);
        assert!(item.labels.is_empty());
        assert_eq!(item.updated_at_ms, None);
    }

    #[test]
    fn states_and_times_read_from_whatever_services_send() {
        assert_eq!(StateCategory::from_name("In Progress"), StateCategory::Started);
        assert_eq!(StateCategory::from_name("Done"), StateCategory::Done);
        assert_eq!(StateCategory::from_name("archived"), StateCategory::Done);
        assert_eq!(StateCategory::from_name("Canceled"), StateCategory::Canceled);
        assert_eq!(StateCategory::from_name("To Do"), StateCategory::Open);
        assert_eq!(state_label("opened"), "Opened");
        assert_eq!(
            updated_ms(&serde_json::json!("2026-10-24T12:00:00.000Z")),
            Some(1_792_843_200_000)
        );
        assert_eq!(
            updated_ms(&serde_json::json!("2026-10-24T12:00:00.000+0000")),
            Some(1_792_843_200_000)
        );
        assert_eq!(updated_ms(&serde_json::json!(1_792_843_200_000_i64)), Some(1_792_843_200_000));
        assert_eq!(updated_ms(&serde_json::Value::Null), None);
    }
}
