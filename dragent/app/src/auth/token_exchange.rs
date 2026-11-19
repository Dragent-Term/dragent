//! Background token exchange via the GraphQL refreshToken mutation.
//! Used for silent session restore on startup and access-token caching.

use serde::Deserialize;

pub struct ExchangeResult {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub expires_in: u32,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Wasm: the viewer performs no blocking token exchange (no `ureq`); the host
/// page owns authentication.
#[cfg(target_family = "wasm")]
pub fn exchange(_refresh_token: &str) -> Result<ExchangeResult, String> {
    Err("token exchange unavailable in the web viewer".to_string())
}

/// Exchange a refresh token for fresh credentials.
/// Blocking — must be called off the UI thread (e.g., inside a timer callback or std thread).
#[cfg(not(target_family = "wasm"))]
pub fn exchange(refresh_token: &str) -> Result<ExchangeResult, String> {
    let base = crate::api::backend_url();
    let url = format!("{base}/query");

    let query = r#"
        mutation RefreshToken($token: String!) {
            refreshToken(refreshToken: $token) {
                accessToken
                refreshToken
                expiresIn
                user { id firstName lastName avatarUrl }
            }
        }
    "#;

    let payload = serde_json::json!({
        "query": query,
        "variables": { "token": refresh_token }
    });

    let response = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(8))
        .send_json(payload)
        .map_err(|e| format!("network error: {e}"))?;

    if response.status() != 200 {
        return Err(format!("HTTP {}", response.status()));
    }

    #[derive(Deserialize)]
    struct GqlResponse {
        data: Option<GqlData>,
        errors: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct GqlData {
        #[serde(rename = "refreshToken")]
        refresh_token: Payload,
    }
    #[derive(Deserialize)]
    struct Payload {
        #[serde(rename = "accessToken")]
        access_token: String,
        #[serde(rename = "refreshToken")]
        refresh_token: String,
        #[serde(rename = "expiresIn")]
        expires_in: u32,
        user: GqlUser,
    }
    #[derive(Deserialize)]
    struct GqlUser {
        id: String,
        #[serde(rename = "firstName")]
        first_name: Option<String>,
        #[serde(rename = "lastName")]
        last_name: Option<String>,
        #[serde(rename = "avatarUrl")]
        avatar_url: Option<String>,
    }

    let gql: GqlResponse = response
        .into_json()
        .map_err(|e| format!("parse error: {e}"))?;

    if let Some(errs) = gql.errors {
        return Err(format!("GraphQL errors: {errs}"));
    }

    let p = gql.data.ok_or("empty data")?.refresh_token;

    Ok(ExchangeResult {
        access_token: p.access_token,
        refresh_token: p.refresh_token,
        user_id: p.user.id,
        expires_in: p.expires_in,
        first_name: p.user.first_name,
        last_name: p.user.last_name,
        avatar_url: p.user.avatar_url,
    })
}
