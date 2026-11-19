//! Browser sign-in (OAuth 2 with PKCE and a loopback redirect) for the
//! providers with no personal keys: Microsoft (Teams) and Google (Gmail).
//!
//! Each needs an app registered with the provider; its client id is baked
//! in at build time (`DRAGENT_MICROSOFT_CLIENT_ID`, `DRAGENT_GOOGLE_CLIENT_ID`
//! and Google's installed-app secret `DRAGENT_GOOGLE_CLIENT_SECRET`). A
//! build without one says so instead of offering a sign-in that can't work.

use serde_json::Value;

use super::{Connection, Credentials, Provider};
use crate::agent::tools_mcp_auth::{
    oauth_encode, open_in_browser, pkce_challenge, pkce_generate_verifier, wait_for_oauth_code,
};

#[derive(Clone, Copy, Debug)]
pub struct App {
    provider: Provider,
    authorize_url: &'static str,
    token_url: &'static str,
    scopes: &'static str,
    client_id: Option<&'static str>,
    client_secret: Option<&'static str>,
    /// More authorize parameters (Google returns a refresh token only when
    /// asked for offline access with consent).
    extra: &'static [(&'static str, &'static str)],
}

pub const MICROSOFT: App = App {
    provider: Provider::Teams,
    authorize_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    scopes: "offline_access User.Read Chat.Read ChannelMessage.Read.All",
    client_id: option_env!("DRAGENT_MICROSOFT_CLIENT_ID"),
    client_secret: None,
    extra: &[],
};

pub const GOOGLE: App = App {
    provider: Provider::Gmail,
    authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    scopes: "openid email https://www.googleapis.com/auth/gmail.readonly",
    client_id: option_env!("DRAGENT_GOOGLE_CLIENT_ID"),
    client_secret: option_env!("DRAGENT_GOOGLE_CLIENT_SECRET"),
    extra: &[("access_type", "offline"), ("prompt", "consent")],
};

impl App {
    pub fn client_id(&self) -> Option<&'static str> {
        self.client_id.filter(|id| !id.trim().is_empty())
    }
}

/// How long the browser has to come back.
const SIGN_IN_SECONDS: u64 = 300;

/// Sign in through the browser; the tokens it returns, ready to save.
pub(super) fn sign_in(app: &App) -> Result<Credentials, String> {
    let client_id = app.client_id().ok_or_else(|| {
        format!(
            "{} sign-in isn't available in this build of Dragent yet.",
            app.provider.label()
        )
    })?;
    let verifier = pkce_generate_verifier();
    let challenge = pkce_challenge(&verifier);
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Couldn't wait for the sign-in: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("Couldn't wait for the sign-in: {error}"))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256",
        app.authorize_url,
        oauth_encode(client_id),
        oauth_encode(&redirect),
        oauth_encode(app.scopes),
    );
    for (key, value) in app.extra {
        url.push_str(&format!("&{key}={}", oauth_encode(value)));
    }
    open_in_browser(&url);
    let code = wait_for_oauth_code(listener, SIGN_IN_SECONDS)
        .map_err(|error| format!("The sign-in didn't finish: {error}"))?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", client_id.to_string()),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = app.client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    tokens(app, &form, None)
}

/// A request to the token endpoint, as credentials to save. A refresh may
/// not return a new refresh token: keep `previous_refresh`.
fn tokens(
    app: &App,
    form: &[(&str, String)],
    previous_refresh: Option<&str>,
) -> Result<Credentials, String> {
    let response = super::http()?.post(app.token_url).form(form).send();
    let data = super::read_json(app.provider, response)?;
    parse_tokens(
        &data,
        chrono::Utc::now().timestamp_millis(),
        previous_refresh,
    )
}

fn parse_tokens(
    data: &Value,
    now_ms: i64,
    previous_refresh: Option<&str>,
) -> Result<Credentials, String> {
    let access = data["access_token"]
        .as_str()
        .ok_or("The sign-in returned no token.")?;
    let mut credentials = Credentials::from([("access_token".to_string(), access.to_string())]);
    if let Some(refresh) = data["refresh_token"].as_str().or(previous_refresh) {
        credentials.insert("refresh_token".into(), refresh.to_string());
    }
    let expires_in = data["expires_in"].as_i64().unwrap_or(3600);
    credentials.insert(
        "expires_at_ms".into(),
        (now_ms + expires_in * 1000).to_string(),
    );
    Ok(credentials)
}

/// A usable access token for `connection`, refreshed (and saved) first
/// when it's about to expire.
pub(super) fn access_token(connection: &mut Connection, app: &App) -> Result<String, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let expires = connection
        .credential("expires_at_ms")
        .and_then(|at| at.parse::<i64>().ok())
        .unwrap_or(0);
    if expires - 60_000 > now {
        return connection.required("access_token").map(str::to_string);
    }
    let refresh = connection.required("refresh_token")?.to_string();
    let client_id = app
        .client_id()
        .ok_or("Sign-in isn't available in this build.")?;
    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh.clone()),
        ("client_id", client_id.to_string()),
    ];
    if let Some(secret) = app.client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    if app.provider == Provider::Teams {
        form.push(("scope", app.scopes.to_string()));
    }
    let fresh = tokens(app, &form, Some(&refresh)).map_err(|error| {
        format!(
            "{error} Sign in to {} again in Settings → Integrations.",
            app.provider.label()
        )
    })?;
    let token = fresh["access_token"].clone();
    connection.credentials.extend(fresh);
    super::update(connection);
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokens_keep_the_refresh_token_and_when_they_expire() {
        let first = parse_tokens(
            &json!({"access_token": "a1", "refresh_token": "r1", "expires_in": 3599}),
            1_000,
            None,
        )
        .unwrap();
        assert_eq!(first["access_token"], "a1");
        assert_eq!(first["refresh_token"], "r1");
        assert_eq!(first["expires_at_ms"], (1_000 + 3_599_000).to_string());
        // A refresh without a new refresh token keeps the old one.
        let refreshed = parse_tokens(
            &json!({"access_token": "a2", "expires_in": 60}),
            5_000,
            Some("r1"),
        )
        .unwrap();
        assert_eq!(refreshed["refresh_token"], "r1");
        assert!(parse_tokens(&json!({"error": "invalid_grant"}), 0, None).is_err());
    }
}
