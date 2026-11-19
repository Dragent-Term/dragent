//! Failures of connected-service calls as the shared result contract
//! (`dragent_host::outcome`), so an agent can tell a key that stopped
//! working from an account that may not do this, and a write that never
//! happened from one that may have.
//!
//! The services' code reports problems as plain sentences (the Integrations
//! page shows them to people). [`classify`] reads them once, at the edge
//! every agent entry point shares: [`super::tools::call`].

use dragent_host::outcome::{Failure, Status};
use serde_json::{json, Value};

/// Every code this module can produce: (code, status, what it means, what
/// to do). Listed by `dragent integrations describe`.
pub const CODES: &[(&str, Status, &str, &str)] = &[
    (
        "not_connected",
        Status::NeedsAuth,
        "The service has no connection (or no usable sign-in) in this organisation.",
        "Ask the person to connect it in Settings → Integrations. Do not look for credentials.",
    ),
    (
        "credentials_expired",
        Status::NeedsAuth,
        "The service rejected the stored key (HTTP 401): it expired, was revoked or was rotated.",
        "Ask the person to reconnect this account (Settings → Integrations), or whoever connected a team key to replace it. Nothing else is wrong with the account.",
    ),
    (
        "credentials_incomplete",
        Status::NeedsAuth,
        "The stored connection lacks a value the service needs.",
        "Ask the person to reconnect the service in Settings → Integrations.",
    ),
    (
        "insufficient_permission",
        Status::NeedsAuth,
        "The key works but the account may not do this or see this resource (HTTP 403): role, project membership or sharing.",
        "Reconnecting the same account will not help. Check `integrations describe` for the account and scope used, try another account with --account, or ask an admin to grant access.",
    ),
    (
        "insufficient_scope",
        Status::NeedsAuth,
        "The key works but its token scope does not cover this operation.",
        "Add the scope to the token (or app) at the service, then reconnect with the new token. Reconnecting without changing the scope will fail the same way.",
    ),
    (
        "unknown_account",
        Status::InvalidInput,
        "The named account is not one of this service's connected accounts.",
        "Use a selector from `dragent integrations describe SERVICE` (accounts[].selector), or leave --account out for the default.",
    ),
    (
        "not_found",
        Status::InvalidTarget,
        "The item, project or path does not exist for this account. Some services answer 404 for a resource the account cannot see.",
        "Find the id again with `search` (use its `id` field). If it should exist, check the account and scope in `integrations describe`.",
    ),
    (
        "ambiguous_target",
        Status::InvalidInput,
        "A status or place named loosely matched none or several of the service's.",
        "Use one of the names the message lists.",
    ),
    (
        "rejected_by_service",
        Status::InvalidInput,
        "The service refused the input (HTTP 4xx other than 401, 403, 404, 429).",
        "Fix the input the message names; the request was not applied.",
    ),
    (
        "rate_limited",
        Status::Unavailable,
        "The service is throttling this account.",
        "Wait, then retry once; narrow the query rather than paging faster.",
    ),
    (
        "service_error",
        Status::Unavailable,
        "The service failed on its side (HTTP 5xx).",
        "Retry later. For a write, first read the item to see whether it was applied.",
    ),
    (
        "unreachable",
        Status::Unavailable,
        "The service could not be reached.",
        "Check the network, or retry later. Nothing was sent when this is a connection failure.",
    ),
    (
        "timeout",
        Status::Timeout,
        "A read got no answer in time.",
        "Retry the read, with a narrower query if it keeps timing out.",
    ),
    (
        "write_outcome_unknown",
        Status::Timeout,
        "A write got no answer in time: it may or may not have been applied.",
        "Do not repeat it yet. Read the item back (`dragent integrations show SERVICE ID`; for create, search the title) and retry only if the change is absent. See `dragent requests show ID` if a request id is given.",
    ),
    (
        "app_not_open",
        Status::Unavailable,
        "This connection is kept on Dragent's server, which only the open desktop app can reach.",
        "Ask the person to open the Dragent app, then retry.",
    ),
    (
        "dependency_missing",
        Status::MissingDependency,
        "The GitHub CLI (gh) is not installed here.",
        "Ask the person to install it and run gh auth login.",
    ),
    (
        "not_supported",
        Status::Unsupported,
        "Dragent cannot do that operation with this service yet.",
        "Check `integrations describe SERVICE` for the operations it supports, or try `integrations api`.",
    ),
    (
        "api_path_invalid",
        Status::InvalidInput,
        "The path for `api` must be relative to the service's API root.",
        "Use a path like /user (see api.example in `integrations describe SERVICE`), never a full URL.",
    ),
];

fn failure(code: &str, message: &str) -> Failure {
    let (_, status, _, next) = CODES
        .iter()
        .find(|entry| entry.0 == code)
        .copied()
        .unwrap_or((code, Status::Failed, "", ""));
    Failure::new(status, code, message).next(next)
}

/// `error` (a service's sentence, or an encoded failure already) as an
/// encoded [`Failure`]; unrecognised text is returned unchanged.
/// `write`: the call changes something, so silence after sending is not
/// proof that it did not.
pub fn classify(error: &str, write: bool) -> String {
    if Failure::decode(error).is_some() {
        return error.to_string();
    }
    match code_for(error, write) {
        Some(code) => failure(code, error.trim()).encode(),
        None => error.to_string(),
    }
}

/// The code a service's sentence means.
pub fn code_for(error: &str, write: bool) -> Option<&'static str> {
    let text = error.to_lowercase();
    let has = |needle: &str| text.contains(needle);
    Some(
        if has(super::REFUSED_REQUEST) || has("http 403") || has("resource not accessible") {
            if has("missing a scope") || has("required scope") {
                "insufficient_scope"
            } else {
                "insufficient_permission"
            }
        } else if has(super::REJECTED_KEY) || has("bad credentials") || has("http 401") {
            "credentials_expired"
        } else if has("auth login")
            || has("isn't signed in")
            || has("isn't connected")
            || has("not logged")
        {
            "not_connected"
        } else if text.starts_with("reconnect ") {
            "credentials_incomplete"
        } else if has("open the dragent app") {
            "app_not_open"
        } else if has("install the github cli") {
            "dependency_missing"
        } else if has("answered 404")
            || has("http 404")
            || has("could not resolve to")
            || has("not found")
            || has(" has no ")
        {
            "not_found"
        } else if has("answered 429") || has("rate limit") {
            "rate_limited"
        } else if has("answered 5") || has("http 5") {
            "service_error"
        } else if has("timed out") || has("timeout") {
            if write {
                "write_outcome_unknown"
            } else {
                "timeout"
            }
        } else if has("couldn't reach") {
            // A refused or unresolved connection never sent anything; a
            // dropped one may have.
            let never_sent = has("dns")
                || has("refused")
                || has("error trying to connect")
                || has("certificate");
            if write && !never_sent {
                "write_outcome_unknown"
            } else {
                "unreachable"
            }
        } else if has("no single one of the") || has("there are no ") {
            "ambiguous_target"
        } else if has("answered 4") {
            "rejected_by_service"
        } else if has("can't ") && has(" yet") {
            "not_supported"
        } else {
            return None;
        },
    )
}

/// The codes as JSON, for discovery.
pub fn codes_json() -> Value {
    Value::Array(
        CODES
            .iter()
            .map(|(code, status, meaning, next)| {
                json!({"code": code, "status": status.as_str(), "meaning": meaning, "next": next})
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::{expired_message, forbidden_message};

    fn code(error: &str, write: bool) -> String {
        Failure::from_error(&classify(error, write)).code
    }

    #[test]
    fn a_rejected_key_is_not_a_refused_request() {
        assert_eq!(
            code(&expired_message("Linear", false), false),
            "credentials_expired"
        );
        assert_eq!(
            code(&expired_message("Sentry", true), false),
            "credentials_expired"
        );
        assert_eq!(
            code(&forbidden_message("Jira", false), false),
            "insufficient_permission"
        );
        assert_eq!(
            code(&forbidden_message("Jira", true), true),
            "insufficient_permission"
        );
        let expired = Failure::from_error(&classify(&expired_message("Linear", false), false));
        let forbidden = Failure::from_error(&classify(&forbidden_message("Linear", false), false));
        assert_eq!(expired.status, Status::NeedsAuth);
        assert_eq!(forbidden.status, Status::NeedsAuth);
        assert!(expired.next.unwrap().contains("reconnect"));
        assert!(forbidden.next.unwrap().contains("will not help"));
    }

    #[test]
    fn a_missing_scope_says_to_change_the_token_first() {
        let slack = "Slack refused this request because the token is missing a scope (search:read). Add it to the app, reinstall it, then reconnect with the new token.";
        assert_eq!(code(slack, false), "insufficient_scope");
        assert_eq!(
            code("GitHub failed: HTTP 401: Bad credentials", false),
            "credentials_expired"
        );
        assert_eq!(
            code(
                "The GitHub CLI said: HTTP 403: Resource not accessible by integration",
                false
            ),
            "insufficient_permission"
        );
        assert_eq!(
            code(
                "The GitHub CLI isn't signed in. Run gh auth login in a terminal.",
                false
            ),
            "not_connected"
        );
        assert_eq!(
            code(
                "Linear isn't connected. Connect it in Dragent under Settings → Integrations.",
                false
            ),
            "not_connected"
        );
        assert_eq!(
            code("Reconnect Jira in Settings → Integrations.", false),
            "credentials_incomplete"
        );
    }

    #[test]
    fn missing_targets_limits_and_outages_have_their_own_codes() {
        assert_eq!(
            code("Jira answered 404 Not Found. Issue does not exist", false),
            "not_found"
        );
        assert_eq!(code("Linear has no issue ENG-9.", false), "not_found");
        assert_eq!(
            code("GitLab answered 429 Too Many Requests.", false),
            "rate_limited"
        );
        assert_eq!(
            code("GitLab answered 502 Bad Gateway.", false),
            "service_error"
        );
        assert_eq!(
            code("Jira answered 400 Bad Request. bad jql", false),
            "rejected_by_service"
        );
        assert_eq!(
            code(
                "No single one of the Linear statuses matches “x”. They are: Todo.",
                true
            ),
            "ambiguous_target"
        );
        assert_eq!(
            code("Dragent can't create items with Slack yet.", true),
            "not_supported"
        );
        assert_eq!(classify("something odd", false), "something odd");
    }

    #[test]
    fn a_write_that_got_no_answer_may_have_happened() {
        let timed_out = "Couldn't reach Linear: error sending request: operation timed out";
        assert_eq!(code(timed_out, false), "timeout");
        assert_eq!(code(timed_out, true), "write_outcome_unknown");
        let failure = Failure::from_error(&classify(timed_out, true));
        assert_eq!(failure.status, Status::Timeout);
        assert!(!failure.retry_safe);
        assert!(failure.next.unwrap().contains("Read the item back"));
        // Never connected: nothing was sent, so a write is safe to retry.
        assert_eq!(
            code(
                "Couldn't reach Linear: error trying to connect: dns error",
                true
            ),
            "unreachable"
        );
    }

    #[test]
    fn encoded_failures_pass_through_and_every_code_is_documented() {
        let own = Failure::new(Status::InvalidInput, "missing_argument", "x").encode();
        assert_eq!(classify(&own, true), own);
        for (code, _, meaning, next) in CODES {
            assert!(!meaning.is_empty() && !next.is_empty(), "{code}");
        }
    }
}
