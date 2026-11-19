//! Poll the web app for a completed desktop sign-in (no deep link required).

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
struct PollResponse {
    // The web route returns {"pending":true} with HTTP 200 while the browser
    // login is still in progress. Keep this optional so that response is
    // treated as Pending instead of being reported as a JSON parse failure.
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
}

pub fn poll_session_url(state: &str) -> String {
    let origin = crate::api::web_origin();
    format!("{origin}/auth/desktop/session?state={state}")
}

pub fn auth_polling_enabled() -> bool {
    !std::env::var_os("DRAGENT_AUTH_POLL")
        .is_some_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}

/// Wasm: no `ureq` polling in the viewer (sign-in happens on the host page).
#[cfg(target_family = "wasm")]
pub fn try_poll_pending_session(_state: &str) -> PollResult {
    PollResult::Pending
}

/// Ask the web backend whether sign-in finished for this CSRF `state`.
#[cfg(not(target_family = "wasm"))]
pub fn try_poll_pending_session(state: &str) -> PollResult {
    if !auth_polling_enabled() {
        return PollResult::Pending;
    }

    let url = poll_session_url(state);
    let response = match ureq::get(&url).timeout(POLL_TIMEOUT).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(204, _)) => {
            return PollResult::Pending;
        }
        Err(err) => {
            debug!("auth poll unavailable ({url}): {err}");
            return PollResult::Pending;
        }
    };

    if response.status() == 204 {
        return PollResult::Pending;
    }

    if response.status() != 200 {
        debug!(
            "auth poll unexpected status {} for {url}",
            response.status()
        );
        return PollResult::Pending;
    }

    match response.into_json::<PollResponse>() {
        Ok(body)
            if body
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.is_empty()) =>
        {
            PollResult::Ready(AuthCallback {
                refresh_token: body.refresh_token.unwrap_or_default(),
                state: state.to_string(),
                user_id: body.user_id,
            })
        }
        Ok(_) => PollResult::Pending,
        Err(err) => {
            debug!("auth poll JSON parse failed: {err}");
            PollResult::Pending
        }
    }
}
