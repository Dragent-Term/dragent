//! Parse `dragent://auth/callback` deep links.

use anyhow::{anyhow, Context, Result};
use url::Url;

use super::deep_link::CUSTOM_SCHEME;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthCallback {
    pub refresh_token: String,
    pub state: String,
    pub user_id: Option<String>,
}

const CALLBACK_HOST: &str = "auth";
const CALLBACK_PATH: &str = "/callback";

pub fn is_auth_callback_url(raw: &str) -> bool {
    parse_auth_callback(raw).is_ok()
}

pub fn parse_auth_callback(raw: &str) -> Result<AuthCallback> {
    let url = Url::parse(raw.trim()).context("invalid callback URL")?;
    if url.scheme() != CUSTOM_SCHEME {
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

    let refresh_token = required_query_param(&url, "refresh_token")?;
    let state = required_query_param(&url, "state")?;
    let user_id = optional_query_param(&url, "user_id");

    Ok(AuthCallback {
        refresh_token,
        state,
        user_id,
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

    #[test]
    fn parses_valid_callback() {
        let cb = parse_auth_callback(
            "dragent://auth/callback?refresh_token=rt_abc&state=csrf-1&user_id=u1",
        )
        .unwrap();
        assert_eq!(cb.refresh_token, "rt_abc");
        assert_eq!(cb.state, "csrf-1");
        assert_eq!(cb.user_id.as_deref(), Some("u1"));
    }

    #[test]
    fn rejects_missing_state() {
        assert!(parse_auth_callback("dragent://auth/callback?refresh_token=rt").is_err());
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(
            parse_auth_callback("https://example.com/auth/callback?refresh_token=rt&state=s")
                .is_err()
        );
    }
}
