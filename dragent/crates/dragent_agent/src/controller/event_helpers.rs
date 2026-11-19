//! Stream-error mapping + clock helpers.
//!
//! Moved verbatim from `app/src/agent/controller/events.rs` (pure, zero
//! WarpUI deps). `sanitize_server_error` calls the moved
//! `crate::error_display::humanize_agent_error` — one definition, no duplicate.

use crate::block::ErrorKind;
use crate::pipeline::StreamError;

pub fn friendly_stream_error(e: &StreamError) -> (String, ErrorKind) {
    match e {
        StreamError::Auth(_) => (
            "You're not signed in. Please sign in and try again.".to_string(),
            ErrorKind::Auth,
        ),
        StreamError::ProviderKey(msg) => (
            format!("The model provider didn't accept your API key ({msg}). Check the key in Settings, or pick another model."),
            ErrorKind::Internal,
        ),
        StreamError::InsufficientCredits(msg) => (msg.clone(), ErrorKind::InsufficientCredits),
        StreamError::RateLimited(retry_time) => {
            let msg = if retry_time.is_empty() {
                "You've hit the usage limit. Please try again later.".to_string()
            } else {
                format!(
                    "You've hit the usage limit. Try again after {}.",
                    retry_time
                )
            };
            (msg, ErrorKind::InsufficientCredits)
        }
        StreamError::Network(_) => (
            "Connection error. Please try again.".to_string(),
            ErrorKind::Network,
        ),
        StreamError::Timeout(_) => (
            "The server stopped responding. Please try again.".to_string(),
            ErrorKind::Network,
        ),
        StreamError::Parse(_) => (
            "Something went wrong. Please try again.".to_string(),
            ErrorKind::Internal,
        ),
    }
}

/// Map a raw server-sent error string to a user-friendly (message, kind) pair.
/// The raw string is already logged before this is called; it must not reach the UI.
pub fn sanitize_server_error(raw: &str) -> (String, ErrorKind) {
    // One user-facing message source, shared with the self-heal path, so every
    // agent error reads consistently. The ErrorKind is classified separately
    // because it drives behavior (Auth → auto sign-out).
    let message = crate::error_display::humanize_agent_error(raw);
    let lower = raw.to_lowercase();
    // An error inside a stream that was opened with a valid session is not a
    // sign-in problem unless it says so plainly: a provider's "401" or
    // "Unauthorized" relayed by the server (or any text with "401" in it)
    // used to sign people out of Dragent.
    let kind = if lower.contains("unauthenticated")
        || lower.contains("authentication required")
        || lower.contains("session expired")
    {
        ErrorKind::Auth
    } else if lower.contains("rate limit")
        || lower.contains("please wait")
        || lower.contains("allowance")
        || lower.contains("credit")
        || lower.contains("402")
    {
        ErrorKind::InsufficientCredits
    } else if lower.contains("tls")
        || lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("network")
        || lower.contains("provider returned")
        || lower.contains("tcp")
    {
        ErrorKind::Network
    } else {
        ErrorKind::Internal
    };
    (message, kind)
}

pub fn unix_now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod sign_out_tests {
    use super::*;

    /// Only a dead Dragent session may sign anyone out.
    #[test]
    fn provider_and_stray_401_errors_never_sign_out() {
        for raw in [
            "provider returned 401 Unauthorized",
            "Unauthorized: invalid x-api-key",
            "context is 4019 tokens over the limit",
        ] {
            assert_ne!(sanitize_server_error(raw).1, ErrorKind::Auth, "{raw}");
        }
        assert_eq!(sanitize_server_error("session expired").1, ErrorKind::Auth);
        assert_ne!(friendly_stream_error(&StreamError::ProviderKey("bad key".into())).1, ErrorKind::Auth);
        assert_eq!(friendly_stream_error(&StreamError::Auth("x".into())).1, ErrorKind::Auth);
    }
}
