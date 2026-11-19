//! Work linked to the project prompt: pull requests, and issues from the
//! trackers connected in Settings → Integrations.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::integrations::Provider;
use crate::process_ext::NoWindow;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(from = "SavedKind")]
pub enum LinkKind {
    PullRequest,
    /// An issue, task, card, thread, error, message or email.
    Item(Provider),
}

/// Kinds as sessions saved them, before every provider had items.
#[derive(Deserialize)]
#[serde(untagged)]
enum SavedKind {
    Current(CurrentKind),
    Earlier(EarlierKind),
}

#[derive(Deserialize)]
enum CurrentKind {
    PullRequest,
    Item(Provider),
}

#[derive(Deserialize)]
enum EarlierKind {
    GithubIssue,
    LinearIssue,
    JiraIssue,
    GitlabIssue,
}

impl From<SavedKind> for LinkKind {
    fn from(saved: SavedKind) -> Self {
        match saved {
            SavedKind::Current(CurrentKind::PullRequest) => Self::PullRequest,
            SavedKind::Current(CurrentKind::Item(provider)) => Self::Item(provider),
            SavedKind::Earlier(EarlierKind::GithubIssue) => Self::Item(Provider::Github),
            SavedKind::Earlier(EarlierKind::LinearIssue) => Self::Item(Provider::Linear),
            SavedKind::Earlier(EarlierKind::JiraIssue) => Self::Item(Provider::Jira),
            SavedKind::Earlier(EarlierKind::GitlabIssue) => Self::Item(Provider::Gitlab),
        }
    }
}

impl LinkKind {
    /// Every provider's items, in the order Settings lists them, filtered to
    /// the visible task providers (Tasks settings). Pull requests stay: they
    /// are code, not tasks.
    pub fn items() -> impl Iterator<Item = LinkKind> {
        // Read once per call; the global snapshot is cheap (RwLock + Arc).
        let visible: std::collections::HashSet<String> = crate::settings::AppSettings::global()
            .tasks
            .visible_set();
        Provider::ALL.into_iter().filter(move |provider| {
            if !provider.offered() {
                return false;
            }
            match provider.key() {
                // Task providers honor their visibility setting.
                "github" | "linear" | "jira" | "gitlab" | "plane" | "youtrack" | "asana"
                | "trello" | "monday" => visible.contains(provider.key()),
                _ => true,
            }
        }).map(Self::Item)
    }

    /// Visible kinds ordered with the default source first (Tasks settings).
    pub fn ordered_visible() -> Vec<LinkKind> {
        let order = crate::settings::settings_tasks_hosts::ordered_visible_providers();
        let mut kinds: Vec<LinkKind> = Self::items().collect();
        kinds.sort_by_key(|kind| {
            let key = kind.provider().key().to_string();
            order.iter().position(|k| k == &key).unwrap_or(usize::MAX)
        });
        kinds
    }

    /// "Pull request", "Linear issue", "Trello card".
    pub fn label(self) -> String {
        match self {
            Self::PullRequest => "Pull request".into(),
            Self::Item(provider) => format!("{} {}", provider.label(), provider.item()),
        }
    }

    /// Where items of this kind come from.
    pub fn provider(self) -> Provider {
        match self {
            Self::PullRequest => Provider::Github,
            Self::Item(provider) => provider,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PromptLink {
    pub kind: LinkKind,
    pub identifier: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub number: Option<u64>,
    #[serde(default)]
    pub head_branch: Option<String>,
    /// The branch to work on an item in, as its service would name it.
    #[serde(default)]
    pub branch: Option<String>,
    /// Where it lives or what state it's in: a project, channel, sender.
    #[serde(default)]
    pub detail: Option<String>,
    /// The service's own id for fetching the rest of it.
    #[serde(default)]
    pub reference: Option<String>,
}

impl PromptLink {
    /// What the workspace remembers it was started from (items only).
    pub fn linked_issue(&self) -> Option<crate::integrations::links::LinkedIssue> {
        (self.kind != LinkKind::PullRequest).then(|| crate::integrations::links::LinkedIssue {
            provider: self.kind.provider(),
            key: self.identifier.clone(),
            title: self.title.clone(),
            url: self.url.clone(),
        })
    }

    /// The item it links, to fetch the rest of its text.
    pub fn item(&self) -> Option<crate::integrations::items::Item> {
        let LinkKind::Item(provider) = self.kind else {
            return None;
        };
        Some(crate::integrations::items::Item {
            provider,
            key: self.identifier.clone(),
            title: self.title.clone(),
            url: self.url.clone(),
            body: self.body.clone(),
            detail: self.detail.clone(),
                branch: self.branch.clone().unwrap_or_default(),
                reference: self.reference.clone(),
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
        })
    }
}

pub fn search(kind: LinkKind, root: &Path, query: &str) -> Result<Vec<PromptLink>, String> {
    let LinkKind::Item(provider) = kind else {
        return search_pull_requests(root, query);
    };
    // Hidden providers never query their service (Tasks visibility).
    if !crate::settings::settings_tasks_hosts::is_provider_visible(provider.key()) {
        return Ok(Vec::new());
    }
    Ok(crate::integrations::items::search(provider, root, query)?
        .into_iter()
        .map(|item| PromptLink {
            kind: LinkKind::Item(item.provider),
            number: item
                .key
                .strip_prefix('#')
                .and_then(|number| number.parse().ok()),
            identifier: item.key,
            title: item.title,
            url: item.url,
            body: item.body,
            head_branch: None,
            branch: Some(item.branch).filter(|branch| !branch.is_empty()),
            detail: item.detail,
            reference: item.reference,
        })
        .collect())
}

fn search_pull_requests(root: &Path, query: &str) -> Result<Vec<PromptLink>, String> {
    let mut command = crate::process_ext::gh_command();
    command.args([
        "pr",
        "list",
        "--limit",
        "40",
        "--state",
        "all",
        "--json",
        "number,title,url,body,headRefName,state",
    ]);
    if !query.trim().is_empty() {
        command.args(["--search", query.trim()]);
    }
    let output = command
        .current_dir(root)
        .no_window()
        .output()
        .map_err(|error| format!("Could not run GitHub CLI: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("GitHub search failed: {}", detail.trim()));
    }
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("GitHub returned invalid data: {error}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let number = row["number"].as_u64()?;
            let url = row["url"].as_str()?.to_string();
            let title = row["title"].as_str()?.to_string();
            Some(PromptLink {
                kind: LinkKind::PullRequest,
                identifier: format!("#{number}"),
                title,
                url,
                body: row["body"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(12_000)
                    .collect(),
                number: Some(number),
                head_branch: row["headRefName"].as_str().map(str::to_string),
                branch: None,
                detail: row["state"].as_str().map(|state| state.to_lowercase()),
                reference: None,
            })
        })
        .collect())
}

pub fn prompt_with_links(prompt: &str, links: &[PromptLink]) -> String {
    let mut parts = Vec::new();
    if !prompt.trim().is_empty() {
        parts.push(prompt.trim().to_string());
    } else if !links.is_empty() {
        parts.push("Work on the linked work below. Use its details as context.".to_string());
    }
    for link in links {
        let mut section = format!(
            "## Linked {} — {}: {}\n{}",
            link.kind.label(),
            link.identifier,
            link.title,
            link.url
        );
        // What Dragent's integration tools take, when the key isn't it.
        if let Some(reference) = link
            .reference
            .as_deref()
            .filter(|reference| !reference.is_empty() && *reference != link.identifier)
        {
            section.push_str(&format!("\nID for the integration tools: {reference}"));
        }
        if !link.body.trim().is_empty() {
            section.push_str("\n\n");
            section.push_str(link.body.trim());
        }
        parts.push(section);
    }
    parts.join("\n\n")
}

/// Open a PR in its own detached worktree, leaving the user's checkout alone.
pub fn checkout_pull_request(root: &Path, link: &PromptLink) -> Result<PathBuf, String> {
    let number = link
        .number
        .ok_or("The pull request number is unavailable")?;
    let checkout_root =
        super::project_naming::worktree_main_checkout(root).unwrap_or_else(|| root.to_path_buf());
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let target = super::project_naming::worktree_dir(
        &checkout_root,
        &format!("pr-{number}-{}", &suffix[..8]),
    );
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create PR worktree folder: {error}"))?;
    }
    let fetch = crate::git_cmd::GitCommand::new()
        .args(["fetch", "origin", &format!("refs/pull/{number}/head")])
        .current_dir(&checkout_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .map_err(|error| format!("Could not fetch pull request #{number}: {error}"))?;
    if !fetch.status.success() {
        return Err(format!(
            "Could not fetch pull request #{number}: {}",
            String::from_utf8_lossy(&fetch.stderr).trim()
        ));
    }
    let add = crate::git_cmd::GitCommand::new()
        .args(["worktree", "add", "--detach", "--"])
        .arg(&target)
        .arg("FETCH_HEAD")
        .current_dir(&checkout_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .no_window()
        .output()
        .map_err(|error| format!("Could not create PR worktree: {error}"))?;
    if !add.status.success() {
        return Err(format!(
            "Could not create PR worktree: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(kind: LinkKind) -> PromptLink {
        PromptLink {
            kind,
            identifier: "ENG-12".into(),
            title: "Retry uploads".into(),
            url: "https://linear.app/a/issue/ENG-12".into(),
            body: String::new(),
            number: None,
            head_branch: None,
            branch: Some("ada/eng-12-retry-uploads".into()),
            detail: Some("Engineering".into()),
            reference: None,
        }
    }

    #[test]
    fn items_belong_to_their_service_and_are_remembered() {
        assert_eq!(LinkKind::PullRequest.provider(), Provider::Github);
        assert_eq!(LinkKind::Item(Provider::Trello).label(), "Trello card");
        assert_eq!(LinkKind::Item(Provider::Plane).label(), "Plane work item");
        let remembered = link(LinkKind::Item(Provider::Linear))
            .linked_issue()
            .unwrap();
        assert_eq!(
            (remembered.provider, remembered.key.as_str()),
            (Provider::Linear, "ENG-12")
        );
        // A pull request is work to review, not what a workspace is for.
        assert!(link(LinkKind::PullRequest).linked_issue().is_none());
        assert_eq!(
            link(LinkKind::Item(Provider::Linear))
                .item()
                .unwrap()
                .branch,
            "ada/eng-12-retry-uploads"
        );
    }

    #[test]
    fn links_saved_by_earlier_versions_still_load() {
        let old = r#"{"kind":"LinearIssue","identifier":"ENG-1","title":"T","url":"u"}"#;
        let link: PromptLink = serde_json::from_str(old).unwrap();
        assert_eq!(link.kind, LinkKind::Item(Provider::Linear));
        assert_eq!(link.branch, None);
        let current = serde_json::to_string(&LinkKind::Item(Provider::Teams)).unwrap();
        assert_eq!(
            serde_json::from_str::<LinkKind>(&current).unwrap(),
            LinkKind::Item(Provider::Teams)
        );
        assert_eq!(
            serde_json::from_str::<LinkKind>(r#""PullRequest""#).unwrap(),
            LinkKind::PullRequest
        );
    }
}
