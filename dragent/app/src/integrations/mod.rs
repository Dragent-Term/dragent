//! Connections to the services work comes from (code hosts, trackers,
//! support inboxes, error monitoring, chat and email): their items start
//! workspaces, agents use them through [`tools`] (Dragent's own agent
//! directly, other agents through the `dragent mcp` [`bridge`] and
//! `dragent integrations`), and automations watch them for what's new.
//!
//! A connection is a pasted key kept on Dragent's server ([`remote`]: the
//! user's own, or one an admin shared with the organisation), a key in this
//! computer's keychain when the server can't keep it, the GitHub CLI's own
//! sign-in, or an OAuth sign-in whose tokens stay in the keychain. Callers
//! ask [`connection`] (or [`items`]) for a provider and never read
//! credentials themselves: a request built by a service's code goes
//! through [`Connection::send`], which adds this computer's key or hands
//! the request to the server, which adds the one it keeps.
//!
//! Where this is going: browser sign-ins registered with each service, so
//! the server also receives their events; they arrive as another way a
//! server connection was made, and providers, items and the links
//! workspaces keep don't change shape.

pub mod agents;
pub mod api;
pub mod bridge;
pub mod describe;
pub mod errors;
pub mod items;
pub mod links;
pub mod marks;
mod oauth;
pub mod policy;
pub mod remote;
mod services;
pub mod tools;
pub mod tracker;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Github,
    Gitlab,
    Forgejo,
    Linear,
    Jira,
    Plane,
    Youtrack,
    Asana,
    Trello,
    Monday,
    Notion,
    Featurebase,
    Plain,
    Sentry,
    Slack,
    #[serde(rename = "microsoft_teams")]
    Teams,
    Gmail,
}

/// What a provider is for, as the Integrations page groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Code,
    Issues,
    Projects,
    Docs,
    Support,
    Monitoring,
    Messages,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code hosting",
            Self::Issues => "Issue tracking",
            Self::Projects => "Projects & tasks",
            Self::Docs => "Docs",
            Self::Support => "Feedback & support",
            Self::Monitoring => "Error monitoring",
            Self::Messages => "Messages",
        }
    }
}

/// How a provider looks: its mark (a bundled single-colour logo, or
/// letters on a tile for brands without one) in its brand colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logo {
    Svg(&'static str),
    Letters(&'static str),
}

/// One value Connect asks for.
#[derive(Debug)]
pub struct Field {
    pub id: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub secret: bool,
    pub required: bool,
    /// Filled in when left empty (a cloud service's address).
    pub default: Option<&'static str>,
}

const fn field(id: &'static str, label: &'static str, placeholder: &'static str) -> Field {
    Field {
        id,
        label,
        placeholder,
        secret: false,
        required: true,
        default: None,
    }
}

const fn secret(id: &'static str, label: &'static str, placeholder: &'static str) -> Field {
    Field {
        id,
        label,
        placeholder,
        secret: true,
        required: true,
        default: None,
    }
}

const fn server(label: &'static str, default: &'static str) -> Field {
    Field {
        id: "url",
        label,
        placeholder: default,
        secret: false,
        required: false,
        default: Some(default),
    }
}

/// How a provider is signed in to.
#[derive(Debug)]
pub enum Auth {
    /// The GitHub CLI's own sign-in.
    GithubCli,
    /// Values pasted into a form (keys, tokens, addresses).
    Fields(&'static [Field]),
    /// A browser sign-in with the provider.
    OAuth(oauth::App),
}

impl Provider {
    pub const ALL: [Provider; 17] = [
        Self::Github,
        Self::Gitlab,
        Self::Forgejo,
        Self::Linear,
        Self::Jira,
        Self::Plane,
        Self::Youtrack,
        Self::Asana,
        Self::Trello,
        Self::Monday,
        Self::Notion,
        Self::Featurebase,
        Self::Plain,
        Self::Sentry,
        Self::Slack,
        Self::Teams,
        Self::Gmail,
    ];

    /// The provider whose [`key`](Self::key) this is.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Forgejo => "forgejo",
            Self::Linear => "linear",
            Self::Jira => "jira",
            Self::Plane => "plane",
            Self::Youtrack => "youtrack",
            Self::Asana => "asana",
            Self::Trello => "trello",
            Self::Monday => "monday",
            Self::Notion => "notion",
            Self::Featurebase => "featurebase",
            Self::Plain => "plain",
            Self::Sentry => "sentry",
            Self::Slack => "slack",
            Self::Teams => "microsoft_teams",
            Self::Gmail => "gmail",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Github => "GitHub",
            Self::Gitlab => "GitLab",
            Self::Forgejo => "Forgejo",
            Self::Linear => "Linear",
            Self::Jira => "Jira",
            Self::Plane => "Plane",
            Self::Youtrack => "YouTrack",
            Self::Asana => "Asana",
            Self::Trello => "Trello",
            Self::Monday => "monday.com",
            Self::Notion => "Notion",
            Self::Featurebase => "Featurebase",
            Self::Plain => "Plain",
            Self::Sentry => "Sentry",
            Self::Slack => "Slack",
            Self::Teams => "Microsoft Teams",
            Self::Gmail => "Gmail",
        }
    }

    pub fn category(self) -> Category {
        match self {
            Self::Github | Self::Gitlab | Self::Forgejo => Category::Code,
            Self::Linear | Self::Jira | Self::Plane | Self::Youtrack => Category::Issues,
            Self::Asana | Self::Trello | Self::Monday => Category::Projects,
            Self::Notion => Category::Docs,
            Self::Featurebase | Self::Plain => Category::Support,
            Self::Sentry => Category::Monitoring,
            Self::Slack | Self::Teams | Self::Gmail => Category::Messages,
        }
    }

    /// Its keys can belong to a workspace rather than a person (a Sentry
    /// internal integration, a Notion integration, a GitLab group token),
    /// so a team can share one connection.
    pub fn shareable(self) -> bool {
        matches!(
            self,
            Self::Gitlab | Self::Notion | Self::Featurebase | Self::Plain | Self::Sentry
        )
    }

    /// What one of its items is called: "issue", "card", "email".
    pub fn item(self) -> &'static str {
        match self {
            Self::Github
            | Self::Gitlab
            | Self::Forgejo
            | Self::Linear
            | Self::Jira
            | Self::Youtrack
            | Self::Sentry => "issue",
            Self::Plane => "work item",
            Self::Asana => "task",
            Self::Trello => "card",
            Self::Monday => "item",
            Self::Notion => "page",
            Self::Featurebase => "post",
            Self::Plain => "thread",
            Self::Slack | Self::Teams => "message",
            Self::Gmail => "email",
        }
    }

    /// "an issue", "a card".
    pub fn an_item(self) -> String {
        let item = self.item();
        let article = if item.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        };
        format!("{article} {item}")
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Github => "Start work from issues and pull requests in your repositories.",
            Self::Gitlab => "Start work from GitLab issues, on gitlab.com or your own server.",
            Self::Forgejo => "Start work from issues on a Forgejo or Gitea server.",
            Self::Linear => "Start work from Linear issues, on the branch Linear suggests.",
            Self::Jira => "Start work from issues on your Atlassian site.",
            Self::Plane => "Start work from Plane work items, in the cloud or self-hosted.",
            Self::Youtrack => "Start work from YouTrack issues.",
            Self::Asana => "Start work from Asana tasks.",
            Self::Trello => "Start work from Trello cards on your boards.",
            Self::Monday => "Start work from items on your monday.com boards.",
            Self::Notion => "Give the agent a Notion page: a spec, a plan, a ticket.",
            Self::Featurebase => "Start work from feature requests and bug reports.",
            Self::Plain => "Start work from customer support threads.",
            Self::Sentry => "Fix errors straight from Sentry issues.",
            Self::Slack => "Hand the agent a Slack message and its thread.",
            Self::Teams => "Hand the agent a Microsoft Teams message.",
            Self::Gmail => "Hand the agent an email and its thread.",
        }
    }

    pub fn logo(self) -> Logo {
        match self {
            Self::Github => Logo::Svg("icons/integrations/github.svg"),
            Self::Gitlab => Logo::Svg("icons/integrations/gitlab.svg"),
            Self::Forgejo => Logo::Svg("icons/integrations/forgejo.svg"),
            Self::Linear => Logo::Svg("icons/integrations/linear.svg"),
            Self::Jira => Logo::Svg("icons/integrations/jira.svg"),
            Self::Plane => Logo::Letters("P"),
            Self::Youtrack => Logo::Letters("YT"),
            Self::Asana => Logo::Svg("icons/integrations/asana.svg"),
            Self::Trello => Logo::Svg("icons/integrations/trello.svg"),
            Self::Monday => Logo::Svg("icons/integrations/monday.svg"),
            Self::Notion => Logo::Svg("icons/integrations/notion.svg"),
            Self::Featurebase => Logo::Letters("F"),
            Self::Plain => Logo::Letters("P"),
            Self::Sentry => Logo::Svg("icons/integrations/sentry.svg"),
            Self::Slack => Logo::Svg("icons/integrations/slack.svg"),
            Self::Teams => Logo::Letters("T"),
            Self::Gmail => Logo::Svg("icons/integrations/gmail.svg"),
        }
    }

    /// The brand's colour, as `0xRRGGBB`.
    pub fn color(self) -> u32 {
        match self {
            Self::Github => 0x181717,
            Self::Gitlab => 0xFC6D26,
            Self::Forgejo => 0xFB923C,
            Self::Linear => 0x5E6AD2,
            Self::Jira => 0x0052CC,
            Self::Plane => 0x3F76FF,
            Self::Youtrack => 0xFF318C,
            Self::Asana => 0xF06A6A,
            Self::Trello => 0x0052CC,
            Self::Monday => 0x6161FF,
            Self::Notion => 0x000000,
            Self::Featurebase => 0x2563EB,
            Self::Plain => 0x111111,
            Self::Sentry => 0x362D59,
            Self::Slack => 0x4A154B,
            Self::Teams => 0x5059C9,
            Self::Gmail => 0xEA4335,
        }
    }

    pub fn auth(self) -> Auth {
        const GITLAB: &[Field] = &[
            secret("token", "Access token", "glpat-…"),
            server("Server", "https://gitlab.com"),
        ];
        const FORGEJO: &[Field] = &[
            field("url", "Server", "https://codeberg.org"),
            secret("token", "Access token", "Access token"),
        ];
        const LINEAR: &[Field] = &[secret("api_key", "API key", "lin_api_…")];
        const JIRA: &[Field] = &[
            field("url", "Site", "https://your-team.atlassian.net"),
            field("email", "Email", "you@company.com"),
            secret("token", "API token", "Atlassian API token"),
        ];
        const PLANE: &[Field] = &[
            field(
                "workspace",
                "Workspace",
                "Workspace slug, as in its address",
            ),
            secret("api_key", "API key", "plane_api_…"),
            server("Server", "https://api.plane.so"),
        ];
        const YOUTRACK: &[Field] = &[
            field("url", "Server", "https://your-team.youtrack.cloud"),
            secret("token", "Permanent token", "perm:…"),
        ];
        const ASANA: &[Field] = &[secret("token", "Personal access token", "2/1234…")];
        const TRELLO: &[Field] = &[
            field("api_key", "API key", "Your Power-Up's API key"),
            secret("token", "Token", "Token for that key"),
        ];
        const MONDAY: &[Field] = &[secret("token", "API token", "eyJhbGciOi…")];
        const NOTION: &[Field] = &[secret("token", "Integration secret", "ntn_…")];
        const FEATUREBASE: &[Field] = &[secret("api_key", "API key", "sk_…")];
        const PLAIN: &[Field] = &[secret("api_key", "API key", "plainApiKey_…")];
        const SENTRY: &[Field] = &[
            field(
                "organization",
                "Organization",
                "Organization slug, as in its address",
            ),
            secret("token", "Auth token", "sntryu_…"),
            server("Server", "https://sentry.io"),
        ];
        const SLACK: &[Field] = &[secret("token", "User token", "xoxp-…")];
        match self {
            Self::Github => Auth::GithubCli,
            Self::Gitlab => Auth::Fields(GITLAB),
            Self::Forgejo => Auth::Fields(FORGEJO),
            Self::Linear => Auth::Fields(LINEAR),
            Self::Jira => Auth::Fields(JIRA),
            Self::Plane => Auth::Fields(PLANE),
            Self::Youtrack => Auth::Fields(YOUTRACK),
            Self::Asana => Auth::Fields(ASANA),
            Self::Trello => Auth::Fields(TRELLO),
            Self::Monday => Auth::Fields(MONDAY),
            Self::Notion => Auth::Fields(NOTION),
            Self::Featurebase => Auth::Fields(FEATUREBASE),
            Self::Plain => Auth::Fields(PLAIN),
            Self::Sentry => Auth::Fields(SENTRY),
            Self::Slack => Auth::Fields(SLACK),
            Self::Teams => Auth::OAuth(oauth::MICROSOFT),
            Self::Gmail => Auth::OAuth(oauth::GOOGLE),
        }
    }

    /// What Connect asks for (nothing for GitHub and browser sign-ins).
    pub fn fields(self) -> &'static [Field] {
        match self.auth() {
            Auth::Fields(fields) => fields,
            Auth::GithubCli | Auth::OAuth(_) => &[],
        }
    }

    /// Where to get what Connect asks for: a sentence and a link.
    pub fn help(self) -> (&'static str, &'static str) {
        match self {
            Self::Github => (
                "Dragent uses the GitHub CLI's own sign-in for GitHub issues and pull requests.",
                "https://cli.github.com",
            ),
            Self::Gitlab => (
                "Create a personal access token with the api scope (read_api if agents should only read). Leave Server empty for gitlab.com.",
                "https://gitlab.com/-/user_settings/personal_access_tokens",
            ),
            Self::Forgejo => (
                "Create an access token under Settings → Applications on your server, with read and write access to issues.",
                "https://forgejo.org/docs/latest/user/api-usage/",
            ),
            Self::Linear => (
                "Create a personal API key in Linear under Settings → Security & access.",
                "https://linear.app/settings/account/security",
            ),
            Self::Jira => (
                "Create an API token for your Atlassian account, and use the email you sign in with.",
                "https://id.atlassian.com/manage-profile/security/api-tokens",
            ),
            Self::Plane => (
                "Create an API key under Workspace settings → API tokens. Leave Server empty for Plane Cloud.",
                "https://developers.plane.so/api-reference/introduction",
            ),
            Self::Youtrack => (
                "Create a permanent token in your YouTrack profile under Account Security.",
                "https://www.jetbrains.com/help/youtrack/cloud/manage-permanent-token.html",
            ),
            Self::Asana => (
                "Create a personal access token in the Asana developer console.",
                "https://app.asana.com/0/my-apps",
            ),
            Self::Trello => (
                "Create a Power-Up to get an API key, then generate a token for it on the same page.",
                "https://trello.com/power-ups/admin",
            ),
            Self::Monday => (
                "Copy your personal API token from your avatar menu → Developers → My access tokens.",
                "https://developer.monday.com/api-reference/docs/authentication",
            ),
            Self::Notion => (
                "Create an internal integration, copy its secret, then share the pages you want with it. Allow it to insert comments so agents can comment.",
                "https://www.notion.so/profile/integrations",
            ),
            Self::Featurebase => (
                "Create an API key in Featurebase under Settings → API.",
                "https://help.featurebase.app/articles/api",
            ),
            Self::Plain => (
                "Create an API key in Plain under Settings → API keys, with read and write access to threads.",
                "https://www.plain.com/docs/api-reference/authentication",
            ),
            Self::Sentry => (
                "Create a personal token with project:read, event:read and event:write (to comment and resolve). Leave Server empty for sentry.io.",
                "https://sentry.io/settings/account/api/auth-tokens/",
            ),
            Self::Slack => (
                "Create a Slack app with the search:read, channels:history and chat:write user scopes, install it, and copy its User OAuth Token.",
                "https://api.slack.com/apps",
            ),
            Self::Teams => (
                "Sign in with your Microsoft work account in the browser.",
                "https://learn.microsoft.com/microsoftteams/",
            ),
            Self::Gmail => (
                "Sign in with your Google account in the browser. Dragent only reads mail.",
                "https://support.google.com/mail",
            ),
        }
    }

    /// Whether Dragent shows it at all. Services whose browser sign-in has
    /// no registered Dragent app yet (Teams, Gmail) stay out of sight until
    /// a build has one.
    pub fn offered(self) -> bool {
        self.unavailable().is_none()
    }

    /// Why it can't be connected in this build, if it can't (a browser
    /// sign-in that has no registered app yet).
    pub fn unavailable(self) -> Option<&'static str> {
        match self.auth() {
            Auth::OAuth(app) if app.client_id().is_none() => {
                Some("Sign-in isn't available in this build of Dragent yet.")
            }
            _ => None,
        }
    }
}

/// Where a connection's credentials come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Saved on this computer, in its keychain.
    Local,
    /// The GitHub CLI's own sign-in on this computer.
    GithubCli,
    /// An environment variable, read but never saved.
    Environment,
    /// Kept on Dragent's server for the user alone.
    Account,
    /// Kept on Dragent's server for the whole organisation, connected by
    /// `by`; `manageable` when the user may replace or remove it.
    Team { by: String, manageable: bool },
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Self::Local => "Saved on this computer".into(),
            Self::GithubCli => "Signed in with the GitHub CLI".into(),
            Self::Environment => "From an environment variable".into(),
            Self::Account => "Saved to your Dragent account".into(),
            Self::Team { by, .. } if by.is_empty() => "Shared with your team".into(),
            Self::Team { by, .. } => format!("Shared with your team by {by}"),
        }
    }

    /// Whether the user may disconnect it here.
    pub fn removable(&self) -> bool {
        match self {
            Self::Local | Self::Account => true,
            Self::Team { manageable, .. } => *manageable,
            Self::GithubCli | Self::Environment => false,
        }
    }
}

pub type Credentials = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub provider: Provider,
    pub source: Source,
    /// Who it's connected as, when the provider said.
    pub account: Option<String>,
    /// For one kept on Dragent's server, its id there. Its credentials
    /// then hold only the values that aren't secret: requests go through
    /// the server, which adds the key.
    pub(crate) remote: Option<String>,
    credentials: Credentials,
}

impl Connection {
    pub(crate) fn credential(&self, id: &str) -> Option<&str> {
        self.credentials.get(id).map(String::as_str)
    }

    /// A credential that must be there.
    pub(crate) fn required(&self, id: &str) -> Result<&str, String> {
        self.credential(id)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "Reconnect {} in Settings → Integrations.",
                    self.provider.label()
                )
            })
    }

    /// The service's address: what was entered, else the cloud default.
    pub(crate) fn base_url(&self) -> String {
        let entered = self.credential("url").filter(|url| !url.is_empty());
        let default = self
            .provider
            .fields()
            .iter()
            .find(|field| field.id == "url")
            .and_then(|field| field.default);
        entered.or(default).map(normalize_url).unwrap_or_default()
    }
}

/// What the Integrations page shows for a provider.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connected {
        account: Option<String>,
        source: Source,
    },
    NotConnected,
    /// It can't be connected from Dragent yet, and why (the GitHub CLI is
    /// missing or signed out, or a sign-in isn't available).
    Unavailable(String),
}

// ── Saved credentials ────────────────────────────────────────────────────────

trait Secrets: Send + Sync {
    fn load(&self, account: &str) -> Option<String>;
    fn save(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str);
}

/// The OS keychain, beside the model provider keys.
struct Keychain;

const KEYCHAIN_SERVICE: &str = "com.dragent.app";

impl Secrets for Keychain {
    fn load(&self, account: &str) -> Option<String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .ok()?
            .get_password()
            .ok()
    }

    fn save(&self, account: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .and_then(|entry| entry.set_password(value))
            .map_err(|error| format!("Couldn't save to the keychain: {error}"))
    }

    fn delete(&self, account: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, account) {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(error) => log::warn!("[integrations] keychain delete for {account}: {error}"),
            }
        }
    }
}

/// One provider's keychain item.
#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    credentials: Credentials,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    connected_at_ms: Option<i64>,
}

fn keychain_account(provider: Provider) -> String {
    format!("integration/{}", provider.key())
}

/// Where the project prompt kept a Linear key before integrations existed.
fn legacy_linear_key_path() -> PathBuf {
    dragent_paths::platform_support().join("linear_api_key")
}

/// Credentials saved under earlier field names.
fn upgrade(provider: Provider, mut credentials: Credentials) -> Credentials {
    let mut rename = |from: &str, to: &str| {
        if let Some(value) = credentials.remove(from) {
            credentials.entry(to.to_string()).or_insert(value);
        }
    };
    match provider {
        Provider::Jira => {
            rename("site", "url");
            rename("api_token", "token");
        }
        Provider::Gitlab => rename("host", "url"),
        _ => {}
    }
    credentials
}

fn load_saved(secrets: &dyn Secrets, provider: Provider) -> Option<Connection> {
    let saved: Saved = serde_json::from_str(&secrets.load(&keychain_account(provider))?).ok()?;
    Some(Connection {
        provider,
        source: Source::Local,
        account: saved.account,
        remote: None,
        credentials: upgrade(provider, saved.credentials),
    })
}

fn store(secrets: &dyn Secrets, connection: &Connection) -> Result<(), String> {
    let saved = Saved {
        credentials: connection.credentials.clone(),
        account: connection.account.clone(),
        connected_at_ms: Some(chrono::Utc::now().timestamp_millis()),
    };
    let value = serde_json::to_string(&saved).map_err(|error| error.to_string())?;
    secrets.save(&keychain_account(connection.provider), &value)
}

/// A provider's saved connection; a Linear key from before integrations
/// moves into the keychain on the way.
fn find(
    secrets: &dyn Secrets,
    legacy_linear: &std::path::Path,
    provider: Provider,
) -> Option<Connection> {
    if let Some(connection) = load_saved(secrets, provider) {
        return Some(connection);
    }
    if provider != Provider::Linear {
        return None;
    }
    let key = std::fs::read_to_string(legacy_linear)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    if let Some(key) = key {
        let connection = Connection {
            provider,
            source: Source::Local,
            account: None,
            remote: None,
            credentials: Credentials::from([("api_key".to_string(), key)]),
        };
        match store(secrets, &connection) {
            Ok(()) => {
                let _ = std::fs::remove_file(legacy_linear);
            }
            Err(error) => {
                log::warn!("[integrations] moving the Linear key to the keychain: {error}")
            }
        }
        return Some(connection);
    }
    std::env::var("LINEAR_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .map(|key| Connection {
            provider,
            source: Source::Environment,
            account: None,
            remote: None,
            credentials: Credentials::from([("api_key".to_string(), key)]),
        })
}

/// Keychain reads are slow and can prompt, and the project prompt asks on
/// every frame its menu is open: remember the answer until it changes here.
fn cache() -> &'static Mutex<HashMap<Provider, Option<Connection>>> {
    static CACHE: OnceLock<Mutex<HashMap<Provider, Option<Connection>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember(provider: Provider, connection: Option<Connection>) {
    cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(provider, connection);
}

/// A provider's connection: the one on this computer, else the user's own
/// on Dragent's server, else the organisation's. GitHub has none: its
/// requests go through the GitHub CLI (see [`status`]). While
/// [`with_account`] is active on this thread, the account it names instead.
pub fn connection(provider: Provider) -> Option<Connection> {
    if provider == Provider::Github {
        return None;
    }
    if let Some(selector) = selected_account() {
        return connection_for(provider, &selector);
    }
    local_connection(provider).or_else(|| remote::current().for_provider(provider).map(from_remote))
}

fn local_connection(provider: Provider) -> Option<Connection> {
    // Tests never read this computer's keychain.
    if cfg!(test) {
        return None;
    }
    cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .entry(provider)
        .or_insert_with(|| find(&Keychain, &legacy_linear_key_path(), provider))
        .clone()
}

/// The connection an account selector names: `local`, `personal`, `team`,
/// or a connection's id on Dragent's server.
pub fn connection_for(provider: Provider, selector: &str) -> Option<Connection> {
    match selector {
        "local" => local_connection(provider),
        _ => pick(&remote::current().connections, provider, selector).map(from_remote),
    }
}

/// The server-kept connection of `provider` a selector names: `personal`,
/// `team`, or its id.
fn pick<'a>(
    connections: &'a [remote::RemoteConnection],
    provider: Provider,
    selector: &str,
) -> Option<&'a remote::RemoteConnection> {
    connections.iter().find(|c| {
        c.provider() == Some(provider)
            && match selector {
                "personal" => !c.shared,
                "team" => c.shared,
                id => c.id == id,
            }
    })
}

thread_local! {
    static ACCOUNT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn selected_account() -> Option<String> {
    ACCOUNT.with(|account| account.borrow().clone())
}

/// Run `work` with every request on this thread using the account
/// `selector` names (see [`accounts`]); `None` keeps the default one.
pub fn with_account<T>(selector: Option<&str>, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACCOUNT.with(|account| *account.borrow_mut() = self.0.take());
        }
    }
    let selector = selector.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let _restore = Restore(ACCOUNT.with(|account| account.replace(selector)));
    work()
}

/// One account a provider can be used through. No secrets.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Account {
    /// What `--account` / the `account` argument takes.
    pub selector: String,
    /// `personal`, `team`, `local`, `github_cli` or `environment`.
    pub kind: &'static str,
    /// Who it's connected as, when the provider said.
    pub label: Option<String>,
    /// The one used when no account is named.
    pub default: bool,
    /// For a team connection, who connected it.
    pub shared_by: Option<String>,
    pub can_manage: bool,
    /// What the server last knew of it (empty for local ones).
    pub state: String,
}

/// Every connection that could serve `provider`, the default first. Blocking
/// for GitHub (asks the GitHub CLI).
pub fn accounts(provider: Provider) -> Vec<Account> {
    if provider == Provider::Github {
        return match services::github::cli_status() {
            Status::Connected { account, .. } => vec![Account {
                selector: "github_cli".into(),
                kind: "github_cli",
                label: account,
                default: true,
                shared_by: None,
                can_manage: false,
                state: String::new(),
            }],
            _ => Vec::new(),
        };
    }
    build_accounts(provider, local_connection(provider), &remote::current().connections)
}

fn build_accounts(
    provider: Provider,
    local: Option<Connection>,
    connections: &[remote::RemoteConnection],
) -> Vec<Account> {
    let mut found = Vec::new();
    if let Some(local) = local {
        found.push(Account {
            selector: "local".into(),
            kind: if local.source == Source::Environment { "environment" } else { "local" },
            label: local.account,
            default: false,
            shared_by: None,
            can_manage: true,
            state: String::new(),
        });
    }
    for kept in connections.iter().filter(|c| c.provider() == Some(provider)) {
        found.push(Account {
            selector: if kept.shared { "team" } else { "personal" }.into(),
            kind: if kept.shared { "team" } else { "personal" },
            label: kept.account.clone(),
            default: false,
            shared_by: Some(kept.owner_name.clone()).filter(|_| kept.shared && !kept.owner_name.is_empty()),
            can_manage: kept.can_manage || !kept.shared,
            state: kept.status.clone(),
        });
    }
    // The default is what `connection` picks: this computer's, else the
    // user's own on the server, else the team's.
    let default = found
        .iter()
        .position(|a| a.kind == "local" || a.kind == "environment")
        .or_else(|| found.iter().position(|a| a.kind == "personal"))
        .or_else(|| found.iter().position(|a| a.kind == "team"));
    if let Some(index) = default {
        found[index].default = true;
        let first = found.remove(index);
        found.insert(0, first);
    }
    found
}

fn from_remote(remote: &remote::RemoteConnection) -> Connection {
    let provider = remote.provider().unwrap_or(Provider::Linear);
    Connection {
        provider,
        source: if remote.shared {
            Source::Team {
                by: remote.owner_name.clone(),
                manageable: remote.can_manage,
            }
        } else {
            Source::Account
        },
        account: remote.account.clone(),
        remote: Some(remote.id.clone()),
        credentials: remote.config.clone().into_iter().collect(),
    }
}

/// Save a connection whose credentials changed (refreshed OAuth tokens).
pub(crate) fn update(connection: &Connection) {
    if let Err(error) = store(&Keychain, connection) {
        log::warn!(
            "[integrations] saving {}: {error}",
            connection.provider.label()
        );
    }
    remember(connection.provider, Some(connection.clone()));
}

/// Whether the provider's items can be searched. GitHub always tries: the
/// GitHub CLI reports its own problems.
pub fn is_connected(provider: Provider) -> bool {
    provider == Provider::Github || connection(provider).is_some()
}

/// Check `credentials` with the provider, then save them: a connection
/// shared with the organisation on Dragent's server (it has to be, for
/// teammates to use it); the person's own in this computer's keychain, so
/// their keys never leave it. A browser sign-in ignores
/// them, opens the browser instead, and stays on this computer. Blocking:
/// run it off the main thread.
pub fn connect(
    provider: Provider,
    credentials: Credentials,
    shared: bool,
) -> Result<Connection, String> {
    let (credentials, keyed) = match provider.auth() {
        Auth::GithubCli => return Err("GitHub connects through the GitHub CLI.".into()),
        Auth::Fields(_) => (normalize(provider, credentials)?, true),
        Auth::OAuth(app) => (oauth::sign_in(&app)?, false),
    };
    let mut connection = Connection {
        provider,
        source: Source::Local,
        account: None,
        remote: None,
        credentials,
    };
    connection.account = services::verify(&mut connection)?;
    if keyed && shared && remote::in_app() && remote::current().available {
        let saved = remote::save(
            provider,
            shared,
            connection.account.as_deref(),
            &connection.credentials,
        )?;
        // The server has it now: no copy stays here.
        forget_local(provider);
        let refreshed = remote::current();
        let kept = refreshed
            .connections
            .iter()
            .find(|candidate| candidate.id == saved.id)
            .unwrap_or(&saved);
        return Ok(from_remote(kept));
    }
    if shared {
        return Err(
            "Sharing connections with the team needs Dragent's server, which isn't available."
                .into(),
        );
    }
    store(&Keychain, &connection)?;
    remember(provider, Some(connection.clone()));
    Ok(connection)
}

fn forget_local(provider: Provider) {
    Keychain.delete(&keychain_account(provider));
    if provider == Provider::Linear {
        let _ = std::fs::remove_file(legacy_linear_key_path());
    }
    remember(provider, None);
}

/// Remove the provider's connection: this computer's copy, else the
/// user's own on the server, else (when they may) the organisation's.
/// Blocking for one on the server.
pub fn disconnect(provider: Provider) -> Result<(), String> {
    let local = cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&provider)
        .cloned()
        .flatten();
    if local.is_some() || find(&Keychain, &legacy_linear_key_path(), provider).is_some() {
        forget_local(provider);
        return Ok(());
    }
    match remote::current().for_provider(provider) {
        Some(found) if !found.shared || found.can_manage => remote::delete(&found.id),
        Some(_) => Err(
            "Only whoever connected it, or an admin, can disconnect the team's connection.".into(),
        ),
        None => Ok(()),
    }
}

/// What the Integrations page shows. Blocking (GitHub runs the GitHub CLI):
/// run it off the main thread.
pub fn status(provider: Provider) -> Status {
    if provider == Provider::Github {
        return services::github::cli_status();
    }
    match connection(provider) {
        Some(connection) => Status::Connected {
            account: connection.account,
            source: connection.source,
        },
        None => match provider.unavailable() {
            Some(reason) => Status::Unavailable(reason.to_string()),
            None => Status::NotConnected,
        },
    }
}

/// Trim the values, fill in defaults, and refuse missing ones.
fn normalize(provider: Provider, credentials: Credentials) -> Result<Credentials, String> {
    let mut out = Credentials::new();
    for field in provider.fields() {
        let value = credentials
            .get(field.id)
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if value.is_empty() {
            if field.required {
                return Err(format!("Enter the {}.", field.label.to_lowercase()));
            }
            continue;
        }
        let value = if field.id == "url" {
            normalize_url(&value)
        } else {
            value
        };
        out.insert(field.id.to_string(), value);
    }
    Ok(out)
}

/// `https://host/path` with no trailing slash, from whatever was pasted.
fn normalize_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    }
}

// ── Talking to providers ─────────────────────────────────────────────────────

impl Connection {
    /// Send `request` (built without the key) and read the service's JSON
    /// answer: straight to the service with this computer's key, or
    /// through Dragent's server, which adds the one it keeps.
    pub(crate) fn send(&self, request: reqwest::blocking::RequestBuilder) -> Result<Value, String> {
        let Some(id) = &self.remote else {
            return read_json(self.provider, self.authorize(request)?.send());
        };
        let request = request
            .build()
            .map_err(|error| format!("Couldn't build the request: {error}"))?;
        let (status, body) = remote::send(id, request)?;
        let status = reqwest::StatusCode::from_u16(status)
            .map_err(|_| "Dragent's server sent back an unreadable answer.".to_string())?;
        interpret(
            self.provider,
            status,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
            matches!(self.source, Source::Team { .. }),
        )
    }

    /// The key on a request, the way the provider takes it (Dragent's
    /// server does the same for the ones it keeps).
    fn authorize(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder, String> {
        Ok(match self.provider {
            Provider::Gitlab => request.header("PRIVATE-TOKEN", self.required("token")?),
            Provider::Forgejo => request.header(
                "Authorization",
                format!("token {}", self.required("token")?),
            ),
            Provider::Linear => request.header("Authorization", self.required("api_key")?),
            Provider::Jira => {
                request.basic_auth(self.required("email")?, Some(self.required("token")?))
            }
            Provider::Plane | Provider::Featurebase => {
                request.header("X-API-Key", self.required("api_key")?)
            }
            Provider::Monday => request.header("Authorization", self.required("token")?),
            Provider::Trello => request.query(&[
                ("key", self.required("api_key")?),
                ("token", self.required("token")?),
            ]),
            Provider::Plain => request.bearer_auth(self.required("api_key")?),
            Provider::Youtrack
            | Provider::Asana
            | Provider::Notion
            | Provider::Sentry
            | Provider::Slack => request.bearer_auth(self.required("token")?),
            // Their own sign-ins add their own.
            Provider::Github | Provider::Teams | Provider::Gmail => request,
        })
    }
}

pub(crate) fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Dragent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Couldn't start a connection: {error}"))
}

/// The JSON a request returned, or what went wrong in words.
pub(crate) fn read_json(
    provider: Provider,
    response: Result<reqwest::blocking::Response, reqwest::Error>,
) -> Result<Value, String> {
    let label = provider.label();
    let response = response.map_err(|error| format!("Couldn't reach {label}: {error}"))?;
    let status = response.status();
    let body: Value = response.json().unwrap_or(Value::Null);
    interpret(provider, status, body, false)
}

/// Phrases that tell a rejected key (expired, revoked) from a refused
/// request (the key is fine; the account may not do this). `errors`
/// classifies errors by them, so only these functions write them.
pub(crate) const REJECTED_KEY: &str = "didn't accept";
pub(crate) const REFUSED_REQUEST: &str = "refused this request";

pub(crate) fn expired_message(label: &str, team: bool) -> String {
    if team {
        format!("{label} {REJECTED_KEY} your team's key: it has expired or been revoked. Ask whoever connected it to replace it in Settings → Integrations.")
    } else {
        format!("{label} {REJECTED_KEY} the credentials: they have expired or been revoked. Reconnect the account in Settings → Integrations.")
    }
}

pub(crate) fn forbidden_message(label: &str, team: bool) -> String {
    let whose = if team { "the team's connection" } else { "the connected account" };
    format!("{label} {REFUSED_REQUEST}: {whose} is signed in, but lacks the permission, token scope or access to this resource that it needs. Reconnecting the same account won't fix that.")
}

/// The JSON of an answer, or what went wrong in words. `team`: the
/// organisation's connection, which someone else manages.
fn interpret(
    provider: Provider,
    status: reqwest::StatusCode,
    body: Value,
    team: bool,
) -> Result<Value, String> {
    let label = provider.label();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(expired_message(label, team));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(forbidden_message(label, team));
    }
    if !status.is_success() {
        let detail = body["errorMessages"][0]
            .as_str()
            .or_else(|| body["message"].as_str())
            .or_else(|| body["detail"].as_str())
            .or_else(|| body["error"]["message"].as_str())
            .or_else(|| body["errors"][0]["message"].as_str())
            .or_else(|| body["error_description"].as_str())
            .unwrap_or("");
        return Err(format!("{label} answered {status}. {detail}")
            .trim()
            .to_string());
    }
    if let Some(message) = body["errors"][0]["message"].as_str() {
        return Err(format!("{label}: {message}"));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);

    impl Secrets for Memory {
        fn load(&self, account: &str) -> Option<String> {
            self.0.lock().unwrap().get(account).cloned()
        }
        fn save(&self, account: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(account.into(), value.into());
            Ok(())
        }
        fn delete(&self, account: &str) {
            self.0.lock().unwrap().remove(account);
        }
    }

    pub(crate) fn credentials(pairs: &[(&str, &str)]) -> Credentials {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn kept(id: &str, shared: bool, account: &str) -> remote::RemoteConnection {
        remote::RemoteConnection {
            id: id.into(),
            provider: "sentry".into(),
            shared,
            account: Some(account.into()),
            config: BTreeMap::new(),
            key_hint: Some("sk-…abcd".into()),
            status: "ok".into(),
            owner_name: if shared { "Grace".into() } else { String::new() },
            can_manage: false,
        }
    }

    #[test]
    fn accounts_list_every_connection_with_the_default_first_and_no_secrets() {
        let connections = vec![kept("c1", true, "acme"), kept("c2", false, "Ada")];
        let found = build_accounts(Provider::Sentry, None, &connections);
        let selectors: Vec<&str> = found.iter().map(|a| a.selector.as_str()).collect();
        assert_eq!(selectors, ["personal", "team"]);
        assert!(found[0].default && !found[1].default);
        assert_eq!(found[1].shared_by.as_deref(), Some("Grace"));
        assert!(!serde_json::to_string(&found).unwrap().contains("abcd"));
        assert!(build_accounts(Provider::Linear, None, &connections).is_empty());
        // The same choice `connection` makes: the user's own before the team's.
        assert_eq!(pick(&connections, Provider::Sentry, "personal").unwrap().id, "c2");
        assert_eq!(pick(&connections, Provider::Sentry, "team").unwrap().id, "c1");
        assert_eq!(pick(&connections, Provider::Sentry, "c1").unwrap().id, "c1");
        assert!(pick(&connections, Provider::Linear, "team").is_none());
    }

    #[test]
    fn a_named_account_applies_to_the_thread_and_is_restored() {
        assert_eq!(selected_account(), None);
        with_account(Some(" team "), || {
            assert_eq!(selected_account().as_deref(), Some("team"));
            with_account(None, || assert_eq!(selected_account(), None));
            assert_eq!(selected_account().as_deref(), Some("team"));
        });
        assert_eq!(selected_account(), None);
    }

    #[test]
    fn a_rejected_key_and_a_refused_request_read_differently() {
        let team = interpret(Provider::Sentry, reqwest::StatusCode::UNAUTHORIZED, Value::Null, true).unwrap_err();
        assert!(team.contains(REJECTED_KEY) && team.contains("replace it"));
        let denied = interpret(Provider::Jira, reqwest::StatusCode::FORBIDDEN, Value::Null, false).unwrap_err();
        assert!(denied.contains(REFUSED_REQUEST) && denied.contains("won't fix"));
        assert!(!denied.contains(REJECTED_KEY));
    }

    #[test]
    fn items_read_naturally() {
        assert_eq!(Provider::Linear.an_item(), "an issue");
        assert_eq!(Provider::Trello.an_item(), "a card");
        assert_eq!(Provider::Gmail.an_item(), "an email");
    }

    #[test]
    fn every_provider_is_described() {
        let mut keys = std::collections::HashSet::new();
        for provider in Provider::ALL {
            assert!(keys.insert(provider.key()), "{provider:?} has its own key");
            assert!(!provider.label().is_empty() && !provider.description().is_empty());
            let (help, url) = provider.help();
            assert!(!help.is_empty() && url.starts_with("https://"));
            for field in provider.fields() {
                assert!(!field.label.is_empty(), "{provider:?} {}", field.id);
            }
            if let Logo::Svg(path) = provider.logo() {
                assert!(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("assets/dragent")
                        .join(path)
                        .is_file(),
                    "{path} is bundled"
                );
            }
        }
    }

    #[test]
    fn connections_round_trip_through_the_keychain_item() {
        let secrets = Memory::default();
        let connection = Connection {
            provider: Provider::Jira,
            source: Source::Local,
            account: Some("Ada (ada@example.com)".into()),
            remote: None,
            credentials: credentials(&[
                ("url", "https://acme.atlassian.net"),
                ("email", "ada@example.com"),
                ("token", "t"),
            ]),
        };
        store(&secrets, &connection).unwrap();
        assert!(secrets.load("integration/jira").is_some());
        let loaded = find(
            &secrets,
            std::path::Path::new("/nonexistent"),
            Provider::Jira,
        )
        .unwrap();
        assert_eq!(loaded, connection);
        assert!(find(
            &secrets,
            std::path::Path::new("/nonexistent"),
            Provider::Gitlab
        )
        .is_none());
    }

    #[test]
    fn credentials_saved_under_old_names_still_work() {
        let secrets = Memory::default();
        secrets
            .save(
                "integration/jira",
                r#"{"credentials":{"site":"acme.atlassian.net","email":"a@b.c","api_token":"t"}}"#,
            )
            .unwrap();
        let jira = find(
            &secrets,
            std::path::Path::new("/nonexistent"),
            Provider::Jira,
        )
        .unwrap();
        assert_eq!(jira.base_url(), "https://acme.atlassian.net");
        assert_eq!(jira.credential("token"), Some("t"));
    }

    #[test]
    fn an_old_linear_key_file_moves_into_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("linear_api_key");
        std::fs::write(&legacy, "lin_api_123\n").unwrap();
        let secrets = Memory::default();
        let found = find(&secrets, &legacy, Provider::Linear).unwrap();
        assert_eq!(found.credential("api_key"), Some("lin_api_123"));
        assert!(!legacy.exists(), "the plain file is gone");
        assert_eq!(
            find(&secrets, &legacy, Provider::Linear)
                .unwrap()
                .credential("api_key"),
            Some("lin_api_123")
        );
    }

    #[test]
    fn connect_values_are_trimmed_defaulted_and_required() {
        let jira = normalize(
            Provider::Jira,
            credentials(&[
                ("url", " acme.atlassian.net/ "),
                ("email", "a@b.c"),
                ("token", "x"),
            ]),
        )
        .unwrap();
        assert_eq!(jira["url"], "https://acme.atlassian.net");
        assert_eq!(
            normalize(
                Provider::Jira,
                credentials(&[("url", "acme.atlassian.net"), ("email", "a@b.c")])
            )
            .unwrap_err(),
            "Enter the api token."
        );
        let gitlab = Connection {
            provider: Provider::Gitlab,
            source: Source::Local,
            account: None,
            remote: None,
            credentials: normalize(
                Provider::Gitlab,
                credentials(&[("token", "glpat-1"), ("url", "")]),
            )
            .unwrap(),
        };
        assert!(gitlab.credential("url").is_none());
        assert_eq!(gitlab.base_url(), "https://gitlab.com");
        let own = Connection {
            credentials: normalize(
                Provider::Gitlab,
                credentials(&[("token", "glpat-1"), ("url", "http://git.acme.dev/")]),
            )
            .unwrap(),
            ..gitlab
        };
        assert_eq!(own.base_url(), "http://git.acme.dev");
    }
}

pub(crate) mod ssh_bridge;
