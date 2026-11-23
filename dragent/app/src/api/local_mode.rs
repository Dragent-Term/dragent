//! Local mode: a Dev edition build with no Dragent server chosen.
//!
//! A contributor's build must work without a Dragent server or account. A Dev
//! build talks to a server only when `DRAGENT_DEV_SERVER` names one:
//!
//! * an `http(s)://…` URL → that server, with its web origin from
//!   `DRAGENT_WEB_ORIGIN`. Either your own server (a Dragent server you run
//!   yourself), or the service your installed Dragent uses, which
//!   `scripts/dev.sh --account` reads from it (`dragent service-address`) so
//!   you sign in with your own account as a separate device;
//! * `account` on its own (not through the scripts) → local mode, with a line
//!   saying how to use your account.
//!
//! Without it the build is in local mode: no backend URL, no realtime socket,
//! no sign-in, and the app runs as the local demo identity in the `local`
//! organisation. Every server call answers [`super::ApiError::LocalMode`]
//! without touching the network, and the UI says [`NEEDS_ACCOUNT`] where a
//! server feature lives. The Release edition never enters local mode.

use std::sync::OnceLock;

use dragent_paths::Edition;

/// Runtime variable that picks a server for a Dev build.
pub const DEV_SERVER_ENV: &str = "DRAGENT_DEV_SERVER";

/// Short, calm label shown where a server feature would be.
pub const NEEDS_ACCOUNT: &str = "Needs a Dragent account";

/// One line explaining why, for places with room for a sentence.
pub const NEEDS_ACCOUNT_DETAIL: &str =
    "This Dragent Dev build runs without a server. Set DRAGENT_DEV_SERVER to use account features.";

/// The local demo identity.
pub const DEMO_FIRST_NAME: &str = "Dragent";
pub const DEMO_LAST_NAME: &str = "Dev";
pub const DEMO_NAME: &str = "Dragent Dev";
pub const DEMO_EMAIL: &str = "dev@localhost";
/// The demo identity's user id, as the command line and MCP server see it.
/// Nothing on any server has this id.
pub const DEMO_USER_ID: &str = "local";
/// The organisation the demo identity is in (`dragent_paths`' own `local`).
pub const DEMO_ORGANISATION: &str = "Local";

/// The local demo identity as the app answers `dragent whoami`: signed in,
/// in the `local` organisation, with no token behind it.
pub(crate) fn identity_json() -> serde_json::Value {
    let context = dragent_paths::local_organisation_id();
    serde_json::json!({
        "signed_in": true,
        "local": true,
        "user_id": DEMO_USER_ID,
        "name": DEMO_NAME,
        "email": DEMO_EMAIL,
        "organisation": DEMO_ORGANISATION,
        "organisation_id": context,
        "context": context,
    })
}

/// What `account` says when set directly: the address comes from the
/// installed Dragent, which the scripts read.
pub(crate) const ACCOUNT_NEEDS_SCRIPT: &str = "DRAGENT_DEV_SERVER=account is resolved by scripts/dev.sh --account (scripts/dev.ps1 -Account), which reads the address from your installed Dragent; running in local mode.";

/// The server a Dev build was pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevServer {
    pub backend: String,
    /// Empty when the person gave a backend URL but no web origin.
    pub web: String,
}

/// Parse `DRAGENT_DEV_SERVER`. `None` = no server (local mode). An unusable
/// value is treated as no server: a typo must never send a Dev build to some
/// other address.
pub(crate) fn parse_dev_server(
    value: Option<&str>,
    web_override: Option<&str>,
) -> Option<DevServer> {
    let value = value?.trim().trim_end_matches('/');
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("account") {
        log::warn!("[config] {ACCOUNT_NEEDS_SCRIPT}");
        return None;
    }
    if !super::backend::is_dialable(value) {
        log::error!(
            "[config] ignoring {DEV_SERVER_ENV}={value:?}: not an http(s) URL; running in local mode"
        );
        return None;
    }
    let web = web_override
        .map(|w| w.trim().trim_end_matches('/'))
        .filter(|w| super::backend::is_dialable(w))
        .unwrap_or("")
        .to_string();
    Some(DevServer {
        backend: value.to_string(),
        web,
    })
}

/// Local mode for a given edition and server choice. Pure, for tests.
pub(crate) fn local_mode_for(edition: Edition, server: Option<&DevServer>) -> bool {
    edition == Edition::Dev && server.is_none()
}

/// The server this Dev build uses, read once per process.
pub(crate) fn dev_server() -> Option<&'static DevServer> {
    static CHOICE: OnceLock<Option<DevServer>> = OnceLock::new();
    CHOICE
        .get_or_init(|| {
            parse_dev_server(
                std::env::var(DEV_SERVER_ENV).ok().as_deref(),
                std::env::var("DRAGENT_WEB_ORIGIN").ok().as_deref(),
            )
        })
        .as_ref()
}

/// True when this build must not contact any Dragent server: Dev edition and
/// no `DRAGENT_DEV_SERVER`. Always false for the Release edition and for the
/// web viewer (which is served by a server and talks to its gateway).
pub fn is_local_mode() -> bool {
    #[cfg(target_family = "wasm")]
    {
        false
    }
    #[cfg(not(target_family = "wasm"))]
    {
        local_mode_for(dragent_paths::edition(), dev_server())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(url: &str) -> DevServer {
        parse_dev_server(Some(url), None).expect("server")
    }

    #[test]
    fn local_mode_truth_table() {
        let chosen = server("http://localhost:8080");
        assert!(local_mode_for(Edition::Dev, None));
        assert!(!local_mode_for(Edition::Dev, Some(&chosen)));
        assert!(!local_mode_for(Edition::Release, None));
        assert!(!local_mode_for(Edition::Release, Some(&chosen)));
    }

    #[test]
    fn account_on_its_own_stays_local_mode() {
        assert_eq!(parse_dev_server(Some("account"), None), None);
        assert_eq!(
            parse_dev_server(Some(" Account "), Some("http://localhost:3000")),
            None
        );
    }

    #[test]
    fn staging_and_beta_are_not_accepted() {
        for name in ["staging", "Staging", "beta", "stagin"] {
            assert_eq!(parse_dev_server(Some(name), None), None, "{name}");
        }
    }

    #[test]
    fn a_url_picks_that_backend() {
        let s = parse_dev_server(
            Some("http://localhost:8080/"),
            Some("http://localhost:3000"),
        )
        .unwrap();
        assert_eq!(s.backend, "http://localhost:8080");
        assert_eq!(s.web, "http://localhost:3000");
        assert_eq!(server("https://api.example.com").web, "");
    }

    #[test]
    fn missing_or_unusable_values_mean_no_server() {
        assert_eq!(parse_dev_server(None, None), None);
        assert_eq!(parse_dev_server(Some("  "), None), None);
        assert_eq!(parse_dev_server(Some("acount"), None), None);
        assert_eq!(parse_dev_server(Some("localhost:8080"), None), None);
    }
}
