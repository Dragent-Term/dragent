//! Collect a finished desktop sign-in from the server: by polling (no deep
//! link needed), or by redeeming the one-time code a deep link carried.
//!
//! Both prove they come from the desktop that started the sign-in with the
//! PKCE verifier (see `pkce.rs`); the server hands the session out once.

use std::time::Duration;

use log::debug;
use serde::Deserialize;

use super::callback::AuthCallback;

const POLL_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollResult {
    /// Session not ready yet — keep polling.
    Pending,
    /// Session ready with tokens.
    Ready(AuthCallback),
}

#[derive(Debug, Deserialize)]
struct SessionResponse {
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
}

/// Where a pending sign-in is polled (POST, with the verifier).
pub fn poll_session_url() -> String {
    format!("{}/auth/desktop/session", crate::api::backend_url())
}

fn exchange_url() -> String {
    format!("{}/auth/desktop/exchange", crate::api::backend_url())
}

pub fn auth_polling_enabled() -> bool {
    !std::env::var_os("DRAGENT_AUTH_POLL")
        .is_some_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}

/// Wasm: no `ureq` polling in the viewer (sign-in happens on the host page).
#[cfg(target_family = "wasm")]
pub fn try_poll_pending_session(_state: &str, _verifier: &str) -> PollResult {
    PollResult::Pending
}

#[cfg(target_family = "wasm")]
pub fn redeem_handoff(_state: &str, _code: &str, _verifier: &str) -> Result<AuthCallback, String> {
    Err("sign-in hand-off unavailable in the web viewer".to_string())
}

fn ready(state: &str, body: SessionResponse) -> Option<AuthCallback> {
    let refresh_token = body.refresh_token.filter(|t| !t.is_empty())?;
    Some(AuthCallback {
        refresh_token,
        state: state.to_string(),
        user_id: body.user_id,
        code: None,
    })
}

/// Ask the server whether the sign-in for `state` finished and was
/// approved. Blocking: call off the UI thread.
#[cfg(not(target_family = "wasm"))]
pub fn try_poll_pending_session(state: &str, verifier: &str) -> PollResult {
    if !auth_polling_enabled() {
        return PollResult::Pending;
    }
    let url = poll_session_url();
    let response = match ureq::post(&url)
        .timeout(POLL_TIMEOUT)
        .send_json(serde_json::json!({ "state": state, "verifier": verifier }))
    {
        Ok(response) => response,
        Err(err) => {
            debug!("auth poll unavailable ({url}): {err}");
            return PollResult::Pending;
        }
    };
    if response.status() != 200 {
        return PollResult::Pending;
    }
    match response.into_json::<SessionResponse>() {
        Ok(body) => ready(state, body).map_or(PollResult::Pending, PollResult::Ready),
        Err(err) => {
            debug!("auth poll JSON parse failed: {err}");
            PollResult::Pending
        }
    }
}

/// Trade a deep link's one-time code (plus the verifier) for the app's
/// refresh token. Blocking: call off the UI thread.
#[cfg(not(target_family = "wasm"))]
pub fn redeem_handoff(state: &str, code: &str, verifier: &str) -> Result<AuthCallback, String> {
    let response = ureq::post(&exchange_url())
        .timeout(POLL_TIMEOUT)
        .send_json(serde_json::json!({ "state": state, "code": code, "verifier": verifier }))
        .map_err(|err| match err {
            ureq::Error::Status(401, _) => {
                "This sign-in link was already used or has expired. Sign in again.".to_string()
            }
            other => format!("Couldn't reach Dragent to finish signing in: {other}"),
        })?;
    let body: SessionResponse = response
        .into_json()
        .map_err(|err| format!("Unexpected sign-in response: {err}"))?;
    ready(state, body).ok_or_else(|| "The sign-in response had no session.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_without_a_token_is_still_pending() {
        assert!(ready(
            "s",
            SessionResponse {
                refresh_token: None,
                user_id: None
            }
        )
        .is_none());
        assert!(ready(
            "s",
            SessionResponse {
                refresh_token: Some(String::new()),
                user_id: None
            }
        )
        .is_none());
        let cb = ready(
            "s",
            SessionResponse {
                refresh_token: Some("rt".into()),
                user_id: Some("u".into()),
            },
        )
        .unwrap();
        assert_eq!(
            (cb.refresh_token.as_str(), cb.state.as_str(), cb.code),
            ("rt", "s", None)
        );
    }

    #[test]
    fn polling_goes_to_the_backend() {
        assert!(poll_session_url().ends_with("/auth/desktop/session"));
    }
}
