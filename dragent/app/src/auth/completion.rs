//! Parse sign-in completion from deep links, HTTPS redirects, or pasted tokens.

use anyhow::{anyhow, Context, Result};
use url::Url;

use super::callback::{parse_auth_callback, AuthCallback};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthCompletion {
    /// Full callback with CSRF state (deep link or copied redirect URL).
    Callback(AuthCallback),
    /// Raw refresh token pasted from the browser (no redirect required).
    RefreshTokenOnly(String),
}

/// Accepts this edition's deep link (`dragent://…` / `dragent-dev://…`),
/// `https://…/callback?…`, or a bare refresh token string.
pub fn parse_auth_completion(raw: &str) -> Result<AuthCompletion> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty sign-in data");
    }

    if trimmed.contains("://") {
        if let Ok(callback) = parse_auth_callback(trimmed) {
            return Ok(AuthCompletion::Callback(callback));
        }
        if let Ok(callback) = parse_https_callback(trimmed) {
            return Ok(AuthCompletion::Callback(callback));
        }
        anyhow::bail!("unrecognized sign-in URL");
    }

    if looks_like_refresh_token(trimmed) {
        return Ok(AuthCompletion::RefreshTokenOnly(trimmed.to_string()));
    }

    anyhow::bail!("paste a refresh token or full callback URL")
}

fn parse_https_callback(raw: &str) -> Result<AuthCallback> {
    let url = Url::parse(raw).context("invalid HTTPS callback URL")?;
    if url.scheme() != "https" {
        anyhow::bail!("expected https callback URL");
    }
    if !url.path().ends_with("/callback") && !url.path().ends_with("/auth/callback") {
        anyhow::bail!("unexpected HTTPS callback path");
    }

    let refresh_token = required_query_param(&url, "refresh_token")?;
    let state = required_query_param(&url, "state")?;
    let user_id = optional_query_param(&url, "user_id");

    Ok(AuthCallback {
        refresh_token,
        state,
        user_id,
        code: None,
    })
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

/// Whether `value` looks like a pasted refresh token (exported for clipboard extraction).
pub(crate) fn looks_like_refresh_token(value: &str) -> bool {
    value.len() >= 16 && !value.contains(' ') && !value.contains('\n') && !value.contains('\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dragent_callback() {
        let raw = format!(
            "{}://auth/callback?refresh_token=rt_abc&state=csrf-1",
            dragent_paths::url_scheme()
        );
        let completion = parse_auth_completion(&raw).unwrap();
        assert!(matches!(completion, AuthCompletion::Callback(_)));
    }

    #[test]
    fn rejects_the_other_editions_callback() {
        let other = if dragent_paths::url_scheme() == "dragent" {
            "dragent-dev"
        } else {
            "dragent"
        };
        let raw = format!("{other}://auth/callback?refresh_token=rt_abc&state=csrf-1");
        assert!(
            parse_auth_completion(&raw).is_err(),
            "must not accept the other edition's link: {raw}"
        );
    }

    #[test]
    fn parses_https_callback() {
        let completion = parse_auth_completion(
            "https://app.example.test/auth/callback?refresh_token=rt_abc&state=csrf-1",
        )
        .unwrap();
        assert!(matches!(completion, AuthCompletion::Callback(_)));
    }

    #[test]
    fn parses_base64_refresh_token_with_padding() {
        let token = "rt_live_abcdefghijklmnopqrstuvwxyz==";
        let completion = parse_auth_completion(token).unwrap();
        assert!(matches!(
            completion,
            AuthCompletion::RefreshTokenOnly(ref t) if t == token
        ));
    }
}
