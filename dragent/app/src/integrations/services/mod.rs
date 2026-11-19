//! Each service's own requests: who the credentials belong to, searching
//! its items, and fetching the rest of one.

mod asana;
mod featurebase;
mod forgejo;
pub(super) mod github;
mod gitlab;
mod gmail;
mod jira;
mod linear;
mod monday;
mod notion;
mod plain;
mod plane;
mod sentry;
mod slack;
mod teams;
mod trello;
mod youtrack;

use std::path::Path;

use super::items::{Draft, Item};
use super::{Connection, Provider};
use crate::process_ext::NoWindow;

/// Who `connection`'s credentials belong to, as the service names them.
/// Some services also note what later requests need (Asana's workspace,
/// Plain's workspace id).
pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    match connection.provider {
        Provider::Github => Ok(None),
        Provider::Gitlab => gitlab::verify(connection),
        Provider::Forgejo => forgejo::verify(connection),
        Provider::Linear => linear::verify(connection),
        Provider::Jira => jira::verify(connection),
        Provider::Plane => plane::verify(connection),
        Provider::Youtrack => youtrack::verify(connection),
        Provider::Asana => asana::verify(connection),
        Provider::Trello => trello::verify(connection),
        Provider::Monday => monday::verify(connection),
        Provider::Notion => notion::verify(connection),
        Provider::Featurebase => featurebase::verify(connection),
        Provider::Plain => plain::verify(connection),
        Provider::Sentry => sentry::verify(connection),
        Provider::Slack => slack::verify(connection),
        Provider::Teams => teams::verify(connection),
        Provider::Gmail => gmail::verify(connection),
    }
}

pub(super) fn search(
    connection: &mut Connection,
    root: &Path,
    query: &str,
) -> Result<Vec<Item>, String> {
    match connection.provider {
        Provider::Github => github::search(root, query),
        Provider::Gitlab => gitlab::search(connection, root, query),
        Provider::Forgejo => forgejo::search(connection, root, query),
        Provider::Linear => linear::search(connection, query),
        Provider::Jira => jira::search(connection, query),
        Provider::Plane => plane::search(connection, query),
        Provider::Youtrack => youtrack::search(connection, query),
        Provider::Asana => asana::search(connection, query),
        Provider::Trello => trello::search(connection, query),
        Provider::Monday => monday::search(connection, query),
        Provider::Notion => notion::search(connection, query),
        Provider::Featurebase => featurebase::search(connection, query),
        Provider::Plain => plain::search(connection, query),
        Provider::Sentry => sentry::search(connection, query),
        Provider::Slack => slack::search(connection, query),
        Provider::Teams => teams::search(connection, query),
        Provider::Gmail => gmail::search(connection, query),
    }
}

pub(super) fn details(connection: &mut Connection, item: &Item) -> Result<Option<String>, String> {
    let Some(reference) = item.reference.as_deref() else {
        return Ok(None);
    };
    match connection.provider {
        Provider::Notion => notion::details(connection, reference).map(Some),
        Provider::Sentry => sentry::details(connection, reference).map(Some),
        Provider::Slack => slack::details(connection, reference).map(Some),
        Provider::Gmail => gmail::details(connection, reference).map(Some),
        _ => Ok(None),
    }
}

/// One item in full. `id` is an [`Item::id`].
pub(super) fn get(connection: &mut Connection, root: &Path, id: &str) -> Result<Item, String> {
    match connection.provider {
        Provider::Gitlab => gitlab::read(connection, root, id),
        Provider::Forgejo => forgejo::read(connection, root, id),
        Provider::Linear => linear::read(connection, id),
        Provider::Jira => jira::read(connection, id),
        Provider::Plane => plane::read(connection, id),
        Provider::Youtrack => youtrack::read(connection, id),
        Provider::Asana => asana::read(connection, id),
        Provider::Trello => trello::read(connection, id),
        Provider::Monday => monday::read(connection, id),
        Provider::Notion => notion::read(connection, id),
        Provider::Featurebase => featurebase::read(connection, id),
        Provider::Plain => plain::read(connection, id),
        Provider::Sentry => sentry::read(connection, id),
        Provider::Slack => slack::read(connection, id),
        Provider::Gmail => gmail::read(connection, id),
        Provider::Github | Provider::Teams => Err(cannot(connection.provider)),
    }
}

pub(super) fn comment(
    connection: &mut Connection,
    root: &Path,
    id: &str,
    text: &str,
) -> Result<(), String> {
    match connection.provider {
        Provider::Gitlab => gitlab::comment(connection, root, id, text),
        Provider::Forgejo => forgejo::comment(connection, root, id, text),
        Provider::Linear => linear::comment(connection, id, text),
        Provider::Jira => jira::comment(connection, id, text),
        Provider::Plane => plane::comment(connection, id, text),
        Provider::Youtrack => youtrack::comment(connection, id, text),
        Provider::Asana => asana::comment(connection, id, text),
        Provider::Trello => trello::comment(connection, id, text),
        Provider::Monday => monday::comment(connection, id, text),
        Provider::Notion => notion::comment(connection, id, text),
        Provider::Plain => plain::comment(connection, id, text),
        Provider::Sentry => sentry::comment(connection, id, text),
        Provider::Slack => slack::comment(connection, id, text),
        _ => Err(cannot(connection.provider)),
    }
}

pub(super) fn set_status(
    connection: &mut Connection,
    root: &Path,
    id: &str,
    status: &str,
) -> Result<String, String> {
    match connection.provider {
        Provider::Gitlab => gitlab::set_status(connection, root, id, status),
        Provider::Forgejo => forgejo::set_status(connection, root, id, status),
        Provider::Linear => linear::set_status(connection, id, status),
        Provider::Jira => jira::set_status(connection, id, status),
        Provider::Plane => plane::set_status(connection, id, status),
        Provider::Youtrack => youtrack::set_status(connection, id, status),
        Provider::Asana => asana::set_status(connection, id, status),
        Provider::Trello => trello::set_status(connection, id, status),
        Provider::Monday => monday::set_status(connection, id, status),
        Provider::Plain => plain::set_status(connection, id, status),
        Provider::Sentry => sentry::set_status(connection, id, status),
        _ => Err(cannot(connection.provider)),
    }
}

pub(super) fn create(
    connection: &mut Connection,
    root: &Path,
    draft: &Draft,
) -> Result<Item, String> {
    match connection.provider {
        Provider::Gitlab => gitlab::create(connection, root, draft),
        Provider::Forgejo => forgejo::create(connection, root, draft),
        Provider::Linear => linear::create(connection, draft),
        Provider::Jira => jira::create(connection, draft),
        Provider::Plane => plane::create(connection, draft),
        Provider::Youtrack => youtrack::create(connection, draft),
        Provider::Asana => asana::create(connection, draft),
        _ => Err(cannot(connection.provider)),
    }
}

fn cannot(provider: Provider) -> String {
    format!("Dragent can't do that with {} yet.", provider.label())
}

/// Open or closed, from the many ways of saying either. `None` for
/// anything else.
pub(super) fn open_or_closed(status: &str) -> Option<bool> {
    match status
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .as_str()
    {
        "open" | "opened" | "reopen" | "reopened" | "todo" | "to do" | "active" | "incomplete"
        | "not done" | "unresolved" => Some(true),
        "closed" | "close" | "done" | "complete" | "completed" | "resolved" | "resolve"
        | "fixed" | "finished" => Some(false),
        _ => None,
    }
}

/// Text as HTML paragraphs, for services whose comments take HTML.
pub(super) fn html(text: &str) -> String {
    text.split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| {
            let escaped = paragraph
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\n', "<br/>");
            format!("<p>{escaped}</p>")
        })
        .collect()
}

/// "Name (detail)", or whichever of the two there is.
pub(super) fn named(name: Option<&str>, detail: Option<&str>) -> Option<String> {
    match (
        name.filter(|n| !n.is_empty()),
        detail.filter(|d| !d.is_empty()),
    ) {
        (Some(name), Some(detail)) => Some(format!("{name} ({detail})")),
        (Some(name), None) => Some(name.to_string()),
        (None, detail) => detail.map(str::to_string),
    }
}

/// "a · b", skipping what's missing.
pub(super) fn joined(parts: &[Option<&str>]) -> Option<String> {
    let parts: Vec<&str> = parts
        .iter()
        .flatten()
        .copied()
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The repository's `origin` address.
pub(super) fn origin(root: &Path) -> Option<String> {
    if std::env::var_os("DRAGENT_REMOTE_PROJECT").as_deref()==Some(root.as_os_str()) {
        if let Ok(origin)=std::env::var("DRAGENT_REMOTE_GIT_ORIGIN"){return Some(origin);}
    }
    crate::git_cmd::GitCommand::new()
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .no_window()
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The host of an `https://host/…` address.
pub(super) fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':']).next().unwrap_or(rest)
}

/// `owner/repo` (or `group/sub/repo`) when `remote` points at `host`.
pub(super) fn repository(remote: &str, host: &str) -> Option<String> {
    let remote = remote.trim();
    let path = if let Some(rest) = remote.strip_prefix("git@") {
        let (remote_host, path) = rest.split_once(':')?;
        (remote_host == host).then_some(path)?
    } else {
        let without_scheme = remote.split_once("://").map(|(_, rest)| rest)?;
        let without_user = without_scheme
            .rsplit_once('@')
            .map_or(without_scheme, |(_, rest)| rest);
        let (authority, path) = without_user.split_once('/')?;
        (authority.split(':').next()? == host).then_some(path)?
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    (!path.is_empty()).then(|| path.to_string())
}

/// Text of an HTML snippet: tags dropped, entities read, blank runs kept short.
pub(super) fn strip_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for character in value.chars() {
        match (in_tag, character) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                if matches!(
                    name.as_str(),
                    "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3"
                ) {
                    out.push('\n');
                }
            }
            (true, character) => tag.push(character),
            (false, character) => out.push(character),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = Vec::new();
    for line in out.lines().map(str::trim_end) {
        if line.trim().is_empty() && lines.last().is_some_and(|last| last.trim().is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_come_from_the_remote() {
        assert_eq!(
            repository("git@gitlab.com:group/sub/app.git\n", "gitlab.com").as_deref(),
            Some("group/sub/app")
        );
        assert_eq!(
            repository("https://gitlab.com/group/app.git", "gitlab.com").as_deref(),
            Some("group/app")
        );
        assert_eq!(
            repository("ssh://git@git.acme.dev:2222/team/app.git", "git.acme.dev").as_deref(),
            Some("team/app")
        );
        assert_eq!(
            repository("https://oauth2:tok@codeberg.org/g/a", "codeberg.org").as_deref(),
            Some("g/a")
        );
        assert_eq!(
            repository("git@github.com:group/app.git", "gitlab.com"),
            None
        );
        assert_eq!(host("https://git.acme.dev:3000/path"), "git.acme.dev");
    }

    #[test]
    fn statuses_and_html_for_simple_services() {
        assert_eq!(open_or_closed("Done"), Some(false));
        assert_eq!(open_or_closed("re-open"), None);
        assert_eq!(open_or_closed("to_do"), Some(true));
        assert_eq!(open_or_closed("In review"), None);
        assert_eq!(
            html("Fixed in <b>2.1</b>\nthanks\n\nNext"),
            "<p>Fixed in &lt;b&gt;2.1&lt;/b&gt;<br/>thanks</p><p>Next</p>"
        );
    }

    #[test]
    fn html_reads_as_text() {
        assert_eq!(
            strip_html("<p>Hi&nbsp;there</p><p></p><p><b>Bold</b> &amp; more</p>"),
            "Hi there\n\nBold & more"
        );
        assert_eq!(
            joined(&[Some("Backend"), None, Some("open")]).as_deref(),
            Some("Backend · open")
        );
        assert_eq!(named(None, Some("ada@x.io")).as_deref(), Some("ada@x.io"));
    }
}
