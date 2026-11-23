//! Parse auth-callback deep links for this edition's scheme.

use anyhow::{anyhow, Context, Result};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthCallback {
    /// The app's refresh token. Empty when the link carried `code` instead.
    pub refresh_token: String,
    pub state: String,
    pub user_id: Option<String>,
    /// One-time code from a PKCE sign-in, redeemed with the verifier.
    pub code: Option<String>,
}

const CALLBACK_HOST: &str = "auth";
const CALLBACK_PATH: &str = "/callback";

pub fn is_auth_callback_url(raw: &str) -> bool {
    parse_auth_callback(raw).is_ok()
}

pub fn parse_auth_callback(raw: &str) -> Result<AuthCallback> {
    let url = Url::parse(raw.trim()).context("invalid callback URL")?;
    let expected = dragent_paths::url_scheme();
    if url.scheme() != expected {
        anyhow::bail!("unexpected URL scheme: {}", url.scheme());
    }
    if !is_auth_callback_path(&url) {
        anyhow::bail!(
            "unexpected callback path: {}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or(""),
            url.path()
        );
    }

    // Current builds get a one-time code; builds from before PKCE got the
    // refresh token itself.
    let code = optional_query_param(&url, "code");
    let refresh_token = match code {
        Some(_) => String::new(),
        None => required_query_param(&url, "refresh_token")?,
    };
    let state = required_query_param(&url, "state")?;
    let user_id = optional_query_param(&url, "user_id");

    Ok(AuthCallback {
        refresh_token,
        state,
        user_id,
        code,
    })
}

fn is_auth_callback_path(url: &Url) -> bool {
    if url.path() == "/auth/callback" {
        return true;
    }
    url.host_str() == Some(CALLBACK_HOST) && url.path() == CALLBACK_PATH
}

fn required_query_param(url: &Url, key: &str) -> Result<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("missing `{key}` in auth callback"))
}

fn optional_query_param(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::deep_link::custom_scheme;

    fn callback_url(path_and_query: &str) -> String {
        format!("{}://{path_and_query}", custom_scheme())
    }

    fn other_scheme_url(path_and_query: &str) -> String {
        let other = if custom_scheme() == "dragent" {
            "dragent-dev"
        } else {
            "dragent"
        };
        format!("{other}://{path_and_query}")
    }

    #[test]
    fn parses_valid_callback() {
        let cb = parse_auth_callback(&callback_url(
            "auth/callback?refresh_token=rt_abc&state=csrf-1&user_id=u1",
        ))
        .unwrap();
        assert_eq!(cb.refresh_token, "rt_abc");
        assert_eq!(cb.state, "csrf-1");
        assert_eq!(cb.user_id.as_deref(), Some("u1"));
    }

    #[test]
    fn rejects_missing_state() {
        assert!(parse_auth_callback(&callback_url("auth/callback?refresh_token=rt")).is_err());
        let coded =
            parse_auth_callback(&callback_url("auth/callback?code=c1&state=s1&user_id=u1"))
                .unwrap();
        assert_eq!(coded.code.as_deref(), Some("c1"));
        assert!(coded.refresh_token.is_empty());
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(
            parse_auth_callback("https://example.com/auth/callback?refresh_token=rt&state=s")
                .is_err()
        );
    }

    #[test]
    fn rejects_the_other_editions_scheme() {
        // A Dev build must not accept the installed app's `dragent://` links
        // (and vice versa): they belong to the other edition.
        assert!(
            parse_auth_callback(&other_scheme_url(
                "auth/callback?refresh_token=rt_abc&state=csrf-1"
            ))
            .is_err(),
            "must reject the other edition's scheme"
        );
    }
}
