//! Browser pane identities and navigation policy shared by the UI and agent bridge.
//!
//! This module does not start a browser. A renderer must apply `BrowserUrl` validation
//! to redirects and popups as well as address-bar and agent navigation requests.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ProjectId, SurfaceBinding, SurfaceDescriptor, SurfaceKey, SurfaceKind, SurfaceScope};

/// Validated top-level destination. Deserialization uses the same policy as navigation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BrowserUrl(String);

impl BrowserUrl {
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        if input.is_empty() || input.len() > 16_384 || input.chars().any(char::is_control) {
            return Err("Enter a valid HTTP or HTTPS address".into());
        }
        if input == "about:blank" {
            return Ok(Self(input.into()));
        }
        let parsed = url::Url::parse(input).map_err(|_| "Enter a valid HTTP or HTTPS address")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err("Browser panes only support HTTP and HTTPS pages".into());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Sign in on the page instead of including credentials in the address".into());
        }
        Ok(Self(parsed.into()))
    }

    /// Human input may omit a scheme. Only loopback defaults to plain HTTP.
    /// Explicit unsupported schemes are rejected rather than rewritten as a URL.
    pub fn from_address_bar(input: &str) -> Result<Self, String> {
        let input = input.trim();
        if input.contains("://") || input.starts_with("about:") {
            return Self::parse(input);
        }
        let candidate = url::Url::parse(&format!("https://{input}"))
            .map_err(|_| "Enter a valid address")?;
        let loopback = match candidate.host() {
            Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        // A colon is only accepted here as a valid host port (or IPv6 literal).
        // Inputs such as javascript:alert(1) fail parsing above.
        Self::parse(&format!("{}://{input}", if loopback { "http" } else { "https" }))
    }

    /// Address bar entry: an address when it looks like one, otherwise a web
    /// search, as in any browser.
    pub fn from_address_or_search(input: &str) -> Result<Self, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("Enter an address or search the web".into());
        }
        let looks_like_address = input.contains("://")
            || input.starts_with("about:")
            || (!input.contains(char::is_whitespace)
                && (input.contains('.')
                    || input.contains(':')
                    || input.split('/').next().is_some_and(|host| host.eq_ignore_ascii_case("localhost"))));
        if looks_like_address {
            return Self::from_address_bar(input);
        }
        let mut search = url::Url::parse("https://www.google.com/search").expect("static URL");
        search.query_pairs_mut().append_pair("q", input);
        Self::parse(search.as_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_blank(&self) -> bool {
        self.0 == "about:blank"
    }

    /// Host and port of a loopback address (`localhost`, `127.0.0.1`, `[::1]`),
    /// used to wait for a local development server before loading it.
    pub fn loopback_port(&self) -> Option<(String, u16)> {
        let parsed = url::Url::parse(&self.0).ok()?;
        let host = match parsed.host()? {
            url::Host::Domain(host) if host.eq_ignore_ascii_case("localhost") => host.to_string(),
            url::Host::Ipv4(ip) if ip.is_loopback() => ip.to_string(),
            url::Host::Ipv6(ip) if ip.is_loopback() => ip.to_string(),
            _ => return None,
        };
        Some((host, parsed.port_or_known_default()?))
    }

    /// Short label for tabs and status text: `host[:port]`.
    pub fn host_label(&self) -> String {
        url::Url::parse(&self.0)
            .ok()
            .and_then(|parsed| {
                let host = parsed.host_str()?.trim_start_matches("www.").to_string();
                Some(match parsed.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host,
                })
            })
            .unwrap_or_default()
    }

    /// Policy for navigations a page starts itself, including sub-frames.
    /// Embedded documents legitimately use `about:srcdoc`, `data:` and `blob:`
    /// frames; local files and app-launching schemes are never opened.
    pub fn page_may_navigate(input: &str) -> bool {
        let scheme = input
            .split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .unwrap_or_default();
        matches!(scheme.as_str(), "http" | "https" | "about" | "data" | "blob")
    }
}

impl TryFrom<String> for BrowserUrl {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<BrowserUrl> for String {
    fn from(value: BrowserUrl) -> Self {
        value.0
    }
}

/// Explicit ownership is required even when the target URL is public. Browser
/// cookies must never move to another organisation when the UI account changes;
/// project and workspace scope which agents may drive the pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserOwner {
    pub organisation: String,
    pub project_id: ProjectId,
    pub workspace_id: Uuid,
}

/// Only declarative state is restored. Renderer handles, navigation history,
/// POST bodies, pending agent commands, and credentials never enter this record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPaneSession {
    pub pane_id: Uuid,
    pub owner: BrowserOwner,
    pub profile_id: Uuid,
    pub url: BrowserUrl,
}

impl BrowserPaneSession {
    /// The organisation's shared profile: signing in once keeps every browser
    /// pane in that organisation signed in, across restarts.
    pub const DEFAULT_PROFILE: Uuid = Uuid::nil();

    pub fn new(owner: BrowserOwner, url: BrowserUrl) -> Self {
        Self::restore(Uuid::new_v4(), owner, url)
    }

    pub fn restore(pane_id: Uuid, owner: BrowserOwner, url: BrowserUrl) -> Self {
        Self { pane_id, owner, profile_id: Self::DEFAULT_PROFILE, url }
    }

    pub fn surface(&self) -> SurfaceDescriptor {
        SurfaceDescriptor::with_id(
            crate::SurfaceId(self.pane_id),
            SurfaceKey::new(
                SurfaceKind::Browser,
                SurfaceScope::Project { project_id: self.owner.project_id },
                SurfaceBinding::BrowserPane { pane_id: self.pane_id },
            ),
            0,
        )
    }

    /// Cookie and storage identity: one per organisation profile. Hashed so
    /// untrusted account names never become path components.
    pub fn storage_partition(&self) -> Uuid {
        let namespace = Uuid::from_u128(0x4d853f74_e70b_4460_90a5_226fe3ae07f9);
        let key = format!(
            "{}:{}:{}",
            self.owner.organisation.len(),
            self.owner.organisation,
            self.profile_id
        );
        Uuid::new_v5(&namespace, key.as_bytes())
    }

    pub fn authorize(&self, owner: &BrowserOwner) -> Result<(), String> {
        if &self.owner == owner {
            Ok(())
        } else {
            Err("Browser pane belongs to a different organisation or workspace".into())
        }
    }
}

/// Generation tags keep late callbacks from an old page from overwriting the
/// address, loading state, or errors of a more recently requested page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrowserNavigationId(Uuid);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserLoadState {
    Dormant,
    Loading,
    Ready,
    Failed(String),
}

pub struct BrowserPaneState {
    session: BrowserPaneSession,
    navigation: Option<BrowserNavigationId>,
    state: BrowserLoadState,
}

impl BrowserPaneState {
    pub fn restore(session: BrowserPaneSession, owner: &BrowserOwner) -> Result<Self, String> {
        session.authorize(owner)?;
        Ok(Self { session, navigation: None, state: BrowserLoadState::Dormant })
    }

    pub fn session(&self) -> &BrowserPaneSession { &self.session }
    pub fn load_state(&self) -> &BrowserLoadState { &self.state }

    pub fn navigate(&mut self, owner: &BrowserOwner, url: BrowserUrl)
        -> Result<BrowserNavigationId, String>
    {
        self.session.authorize(owner)?;
        let id = BrowserNavigationId(Uuid::new_v4());
        self.session.url = url;
        self.navigation = Some(id);
        self.state = BrowserLoadState::Loading;
        Ok(id)
    }

    pub fn complete(&mut self, id: BrowserNavigationId, result: Result<BrowserUrl, String>) -> bool {
        if self.navigation != Some(id) { return false; }
        self.navigation = None;
        match result {
            Ok(url) => { self.session.url = url; self.state = BrowserLoadState::Ready; }
            Err(error) => self.state = BrowserLoadState::Failed(error),
        }
        true
    }

    /// Cancel callbacks before the engine is detached or evicted. Waking the pane
    /// requires a new explicit navigation and never replays prior agent actions.
    pub fn suspend(&mut self) {
        self.navigation = None;
        self.state = BrowserLoadState::Dormant;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> BrowserOwner {
        BrowserOwner { organisation: "example".into(), project_id: ProjectId::new(), workspace_id: Uuid::new_v4() }
    }

    #[test]
    fn address_bar_handles_loopback_ipv6_and_https() {
        for (input, expected) in [
            ("localhost:3000/login", "http://localhost:3000/login"),
            ("127.0.0.1:8080", "http://127.0.0.1:8080/"),
            ("[::1]:3000", "http://[::1]:3000/"),
            ("example.com", "https://example.com/"),
        ] {
            assert_eq!(BrowserUrl::from_address_bar(input).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn blocks_privileged_urls_credentials_and_control_characters() {
        for input in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,hello",
            "chrome://settings", "devtools://inspect", "https://user:secret@example.com",
            "https://example.com/\nattack", "about:config"] {
            assert!(BrowserUrl::parse(input).is_err(), "{input}");
            assert!(BrowserUrl::from_address_bar(input).is_err(), "{input}");
        }
    }

    #[test]
    fn pages_may_open_embedded_frames_but_not_files_or_apps() {
        for allowed in ["https://example.com/", "http://localhost:3000", "about:srcdoc",
            "about:blank", "data:text/html,<p>hi</p>", "blob:https://example.com/1234"] {
            assert!(BrowserUrl::page_may_navigate(allowed), "{allowed}");
        }
        for blocked in ["file:///etc/passwd", "FILE:///etc/passwd", "javascript:alert(1)",
            "vscode://file/x", "mailto:a@example.com", "chrome://settings", "", "relative/path"] {
            assert!(!BrowserUrl::page_may_navigate(blocked), "{blocked}");
        }
    }

    #[test]
    fn address_bar_searches_words_and_phrases() {
        let go = |s: &str| BrowserUrl::from_address_or_search(s).unwrap();
        assert_eq!(go("localhost").as_str(), "http://localhost/");
        assert_eq!(go("localhost/admin").as_str(), "http://localhost/admin");
        assert_eq!(go("docs.rs").as_str(), "https://docs.rs/");
        assert_eq!(go("https://example.com/a b").as_str(), "https://example.com/a%20b");
        assert_eq!(go("rust lifetimes").as_str(), "https://www.google.com/search?q=rust+lifetimes");
        assert_eq!(go("wry").as_str(), "https://www.google.com/search?q=wry");
        // Unsupported schemes stay errors rather than becoming searches.
        assert!(BrowserUrl::from_address_or_search("file:///etc/passwd").is_err());
        assert!(BrowserUrl::from_address_or_search("   ").is_err());
    }

    #[test]
    fn loopback_ports_and_labels() {
        let url = |s: &str| BrowserUrl::from_address_bar(s).unwrap();
        assert_eq!(url("localhost:3000").loopback_port(), Some(("localhost".into(), 3000)));
        assert_eq!(url("127.0.0.1").loopback_port(), Some(("127.0.0.1".into(), 80)));
        assert_eq!(url("[::1]:8080").loopback_port(), Some(("::1".into(), 8080)));
        assert_eq!(url("example.com").loopback_port(), None);
        assert_eq!(url("www.example.com/docs").host_label(), "example.com");
        assert_eq!(url("localhost:5173/app").host_label(), "localhost:5173");
        assert!(BrowserUrl::parse("about:blank").unwrap().is_blank());
    }

    #[test]
    fn persisted_urls_cannot_bypass_validation() {
        assert!(serde_json::from_str::<BrowserUrl>("\"file:///etc/passwd\"").is_err());
    }

    #[test]
    fn access_is_scoped_to_workspace_and_storage_to_organisation() {
        let owner = owner();
        let session = BrowserPaneSession::new(owner.clone(), BrowserUrl::parse("about:blank").unwrap());
        for altered in [
            BrowserOwner { organisation: "other".into(), ..owner.clone() },
            BrowserOwner { project_id: ProjectId::new(), ..owner.clone() },
            BrowserOwner { workspace_id: Uuid::new_v4(), ..owner.clone() },
        ] {
            assert!(session.authorize(&altered).is_err());
            let mut other = session.clone();
            other.owner = altered.clone();
            // Sign-ins are shared within an organisation, never across one.
            assert_eq!(
                session.storage_partition() == other.storage_partition(),
                altered.organisation == owner.organisation
            );
        }
        // Length-prefixing keeps "ab"+":c" and "a"+"b:c" style names apart.
        let mut tricky = session.clone();
        tricky.owner.organisation = format!("{}:{}", owner.organisation, Uuid::nil());
        assert_ne!(session.storage_partition(), tricky.storage_partition());
        let restored: BrowserPaneSession = serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(session.surface(), restored.surface());
        assert_eq!(session.storage_partition(), restored.storage_partition());
        assert_eq!(BrowserPaneState::restore(restored, &owner).unwrap().load_state(), &BrowserLoadState::Dormant);
    }

    #[test]
    fn stale_callbacks_cannot_replace_new_navigation_or_wake_suspended_pane() {
        let owner = owner();
        let url = BrowserUrl::parse("https://example.com").unwrap();
        let session = BrowserPaneSession::new(owner.clone(), url.clone());
        let mut pane = BrowserPaneState::restore(session, &owner).unwrap();
        let first = pane.navigate(&owner, url.clone()).unwrap();
        let second = pane.navigate(&owner, url.clone()).unwrap();
        assert!(!pane.complete(first, Err("Old request failed".into())));
        assert_eq!(pane.load_state(), &BrowserLoadState::Loading);
        assert!(pane.complete(second, Ok(url.clone())));
        assert!(!pane.complete(second, Err("Duplicate callback".into())));
        let third = pane.navigate(&owner, url.clone()).unwrap();
        pane.suspend();
        assert!(!pane.complete(third, Ok(url)));
        assert_eq!(pane.load_state(), &BrowserLoadState::Dormant);
    }
}
