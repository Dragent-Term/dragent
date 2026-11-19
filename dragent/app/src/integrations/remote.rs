//! Connections kept on Dragent's server: the organisation's shared ones
//! and the user's own (the server side is `internal/integrations`). Their
//! keys stay there. A request to the service goes to the server instead,
//! built here without the key; the server adds it and sends it on.
//!
//! Only the app talks to the server: it's signed in, and the command line
//! never signs in. The app keeps what it last heard in a file beside the
//! organisation's other data, so `dragent integrations` and the MCP bridge
//! know these connections exist and hand their calls to the app.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Credentials, Provider};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteConnection {
    pub id: String,
    pub provider: String,
    pub shared: bool,
    #[serde(default)]
    pub account: Option<String>,
    /// The values that aren't secret (an address, a workspace).
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    #[serde(default)]
    pub key_hint: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub owner_name: String,
    /// The user may replace or remove it.
    #[serde(default)]
    pub can_manage: bool,
}

impl RemoteConnection {
    pub fn provider(&self) -> Option<Provider> {
        Provider::from_key(&self.provider)
    }
}

/// What the server said about the organisation's connections.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Remote {
    /// The organisation (server id) it's about.
    #[serde(default)]
    pub organisation: String,
    /// The server keeps connections: it's set up to, and the user is
    /// signed in.
    #[serde(default)]
    pub available: bool,
    /// The user may share connections with the team.
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub connections: Vec<RemoteConnection>,
}

impl Remote {
    /// The connection the user's requests to `provider` use: their own
    /// before the team's.
    pub fn for_provider(&self, provider: Provider) -> Option<&RemoteConnection> {
        let mine = self
            .connections
            .iter()
            .find(|c| !c.shared && c.provider() == Some(provider));
        mine.or_else(|| {
            self.connections
                .iter()
                .find(|c| c.shared && c.provider() == Some(provider))
        })
    }

    pub fn team(&self, provider: Provider) -> Option<&RemoteConnection> {
        self.connections
            .iter()
            .find(|c| c.shared && c.provider() == Some(provider))
    }
}

static IN_APP: AtomicBool = AtomicBool::new(false);

/// This process is the app, which may talk to the server.
pub fn set_in_app() {
    IN_APP.store(true, Ordering::Release);
}

pub fn in_app() -> bool {
    IN_APP.load(Ordering::Acquire)
}

fn memory() -> &'static Mutex<Option<Remote>> {
    static MEMORY: Mutex<Option<Remote>> = Mutex::new(None);
    &MEMORY
}

fn cache_path() -> PathBuf {
    dragent_paths::organisation_dir().join("integrations-remote.json")
}

/// The organisation the app has open, as the server knows it.
fn organisation() -> Option<String> {
    crate::cli::live_organisation()
        .or_else(crate::organisation_context::cached_active)
        .filter(|active| active.context == dragent_paths::organisation_id())
        .map(|active| active.organisation_id)
        .filter(|id| !id.is_empty())
}

/// What's known now, without asking the server: what the app last heard
/// (this process's, else the file it keeps). Empty for another
/// organisation's.
pub fn current() -> Remote {
    let Some(organisation) = organisation() else {
        return Remote::default();
    };
    let known = memory()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
        .or_else(|| {
            std::fs::read(cache_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Remote>(&bytes).ok())
        });
    known
        .filter(|remote| remote.organisation == organisation)
        .unwrap_or_default()
}

fn remember(remote: Remote) {
    if let Ok(bytes) = serde_json::to_vec_pretty(&remote) {
        let path = cache_path();
        if let Err(error) = std::fs::write(&path, bytes) {
            log::warn!("[integrations] couldn't save {}: {error}", path.display());
        }
    }
    *memory().lock().unwrap_or_else(|poison| poison.into_inner()) = Some(remote);
}

/// Ask the server again in the background when what's known is for
/// another organisation or a few minutes old, so teammates' changes reach
/// this app and the agents' settings follow.
pub fn refresh_if_stale() {
    static LAST: Mutex<Option<(String, std::time::Instant)>> = Mutex::new(None);
    const EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
    if !in_app() {
        return;
    }
    let Some(organisation) = organisation() else {
        return;
    };
    {
        let mut last = LAST.lock().unwrap_or_else(|poison| poison.into_inner());
        if last
            .as_ref()
            .is_some_and(|(asked, at)| *asked == organisation && at.elapsed() < EVERY)
        {
            return;
        }
        *last = Some((organisation, std::time::Instant::now()));
    }
    std::thread::spawn(|| {
        let before = current();
        match refresh() {
            Ok(after) if after.connections != before.connections => {
                super::agents::sync_in_background()
            }
            Ok(_) => {}
            Err(error) => log::info!("[integrations] the server's connections: {error}"),
        }
    });
}

fn base(organisation: &str) -> String {
    format!("/api/v1/organizations/{organisation}/integrations")
}

fn signed_in() -> Result<String, String> {
    if !in_app() {
        return Err("Only the Dragent app talks to Dragent's server.".into());
    }
    organisation()
        .ok_or_else(|| "Sign in to Dragent to keep connections on your account.".to_string())
}

#[derive(Deserialize)]
struct Listed {
    #[serde(default)]
    available: bool,
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    connections: Vec<RemoteConnection>,
}

/// Ask the server again (the app only). Blocking.
pub fn refresh() -> Result<Remote, String> {
    let organisation = signed_in()?;
    let value = crate::api::API_CLIENT
        .get(&base(&organisation), 20)
        .map_err(|error| error.user_message())?;
    let listed: Listed = serde_json::from_value(value)
        .map_err(|error| format!("Unexpected answer from Dragent's server: {error}"))?;
    let remote = Remote {
        organisation,
        available: listed.available,
        admin: listed.admin,
        connections: listed.connections,
    };
    remember(remote.clone());
    Ok(remote)
}

/// The secret values and the others, as the server keeps them apart.
fn split(
    provider: Provider,
    credentials: &Credentials,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let secret: Vec<&str> = provider
        .fields()
        .iter()
        .filter(|field| field.secret)
        .map(|field| field.id)
        .collect();
    let mut secrets = BTreeMap::new();
    let mut config = BTreeMap::new();
    for (key, value) in credentials {
        if secret.contains(&key.as_str()) {
            secrets.insert(key.clone(), value.clone());
        } else {
            config.insert(key.clone(), value.clone());
        }
    }
    (secrets, config)
}

/// Keep a checked connection on the server, for the user alone or, with
/// `shared`, the whole organisation. Blocking.
pub fn save(
    provider: Provider,
    shared: bool,
    account: Option<&str>,
    credentials: &Credentials,
) -> Result<RemoteConnection, String> {
    let organisation = signed_in()?;
    let (secrets, config) = split(provider, credentials);
    let body = json!({
        "provider": provider.key(),
        "shared": shared,
        "account": account,
        "config": config,
        "secrets": secrets,
    });
    let value = crate::api::API_CLIENT
        .put(&base(&organisation), &body, 30)
        .map_err(|error| error.user_message())?;
    let saved: RemoteConnection = serde_json::from_value(value)
        .map_err(|error| format!("Unexpected answer from Dragent's server: {error}"))?;
    let _ = refresh();
    Ok(saved)
}

pub fn delete(id: &str) -> Result<(), String> {
    let organisation = signed_in()?;
    crate::api::API_CLIENT
        .delete(&format!("{}/{id}", base(&organisation)), 20)
        .map_err(|error| error.user_message())?;
    let _ = refresh();
    Ok(())
}

/// Headers the server passes on to the service.
const PASSED: [&str; 4] = ["accept", "content-type", "notion-version", "api-version"];

/// `request` (built without a key) as the server takes it.
fn call(request: &reqwest::blocking::Request) -> Value {
    let headers: BTreeMap<String, String> = request
        .headers()
        .iter()
        .filter(|(name, _)| PASSED.contains(&name.as_str()))
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
        .collect();
    let body = request
        .body()
        .and_then(|body| body.as_bytes())
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .unwrap_or_default();
    json!({
        "method": request.method().as_str(),
        "url": request.url().as_str(),
        "headers": headers,
        "body": body,
    })
}

/// Send `request` through the server with connection `id`'s key: the
/// service's status and body. Blocking.
pub fn send(id: &str, request: reqwest::blocking::Request) -> Result<(u16, Vec<u8>), String> {
    let organisation = signed_in()?;
    let answer = crate::api::API_CLIENT
        .post(
            &format!("{}/{id}/request", base(&organisation)),
            &call(&request),
            40,
        )
        .map_err(|error| error.user_message())?;
    let status = answer["status"].as_u64().unwrap_or(0) as u16;
    let body = base64::engine::general_purpose::STANDARD
        .decode(answer["body"].as_str().unwrap_or_default())
        .map_err(|_| "Dragent's server sent back an unreadable answer.".to_string())?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(provider: &str, shared: bool) -> RemoteConnection {
        RemoteConnection {
            id: format!("{provider}-{shared}"),
            provider: provider.into(),
            shared,
            account: None,
            config: BTreeMap::new(),
            key_hint: None,
            status: "ok".into(),
            owner_name: String::new(),
            can_manage: false,
        }
    }

    #[test]
    fn the_users_own_connection_comes_before_the_teams() {
        let remote = Remote {
            organisation: "o".into(),
            available: true,
            admin: false,
            connections: vec![
                connection("sentry", true),
                connection("sentry", false),
                connection("notion", true),
                connection("bitbucket", false),
            ],
        };
        assert_eq!(
            remote.for_provider(Provider::Sentry).unwrap().id,
            "sentry-false"
        );
        assert_eq!(
            remote.for_provider(Provider::Notion).unwrap().id,
            "notion-true"
        );
        assert!(remote.for_provider(Provider::Linear).is_none());
        assert_eq!(remote.team(Provider::Sentry).unwrap().id, "sentry-true");
    }

    #[test]
    fn keys_and_settings_travel_apart() {
        let credentials: Credentials = [
            ("url", "https://acme.atlassian.net"),
            ("email", "a@b.c"),
            ("token", "secret"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let (secrets, config) = split(Provider::Jira, &credentials);
        assert_eq!(secrets.keys().collect::<Vec<_>>(), ["token"]);
        assert_eq!(config.keys().collect::<Vec<_>>(), ["email", "url"]);
    }

    #[test]
    fn requests_go_to_the_server_without_a_key() {
        let request = reqwest::blocking::Client::new()
            .post("https://api.linear.app/graphql")
            .header("Authorization", "never sent")
            .header("Cookie", "never sent")
            .json(&json!({"query": "{ viewer { id } }"}))
            .build()
            .unwrap();
        let sent = call(&request);
        assert_eq!(sent["method"], "POST");
        assert_eq!(sent["url"], "https://api.linear.app/graphql");
        assert_eq!(sent["headers"]["content-type"], "application/json");
        assert!(
            sent["headers"].get("authorization").is_none()
                && sent["headers"].get("cookie").is_none()
        );
        let body = base64::engine::general_purpose::STANDARD
            .decode(sent["body"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            r#"{"query":"{ viewer { id } }"}"#
        );
    }
}
