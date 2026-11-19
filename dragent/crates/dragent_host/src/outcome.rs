//! One result contract for Dragent's command line, MCP server and agent
//! tools, so agents act on a status and a code instead of the wording of a
//! message.
//!
//! A failure travels through the existing `Result<String, String>` plumbing
//! as an encoded [`Failure`]; whatever prints or returns it last (the command
//! line, the MCP server) decodes it. Text that isn't an encoded failure reads
//! as a plain [`Status::Failed`].
//!
//! A successful JSON result may carry `"status"` itself (`accepted`,
//! `partial`, `unverified`); without one it means [`Status::Ok`].

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Done, and checked where the command checks.
    Ok,
    /// Started or queued; follow it with the returned id.
    Accepted,
    /// Part of it was done; the result says what is left.
    Partial,
    /// Delivered, but the outcome wasn't confirmed: look before relying on it.
    Unverified,
    /// Waiting for the person to allow it. Never answer for them.
    NeedsApproval,
    /// Sign-in or an account connection is needed.
    NeedsAuth,
    /// This build or platform can't do it.
    Unsupported,
    /// A tool, permission or service it needs is missing on this machine.
    MissingDependency,
    /// The command or its arguments are wrong.
    InvalidInput,
    /// The pane, window, element, task or item doesn't exist or is stale.
    InvalidTarget,
    /// Dragent (or the service behind it) isn't reachable.
    Unavailable,
    /// No answer in time. It may still have happened: inspect before retrying.
    Timeout,
    /// Stopped by the person or the app.
    Cancelled,
    /// Anything else.
    Failed,
}

impl Status {
    pub const ALL: [Status; 14] = [
        Status::Ok,
        Status::Accepted,
        Status::Partial,
        Status::Unverified,
        Status::NeedsApproval,
        Status::NeedsAuth,
        Status::Unsupported,
        Status::MissingDependency,
        Status::InvalidInput,
        Status::InvalidTarget,
        Status::Unavailable,
        Status::Timeout,
        Status::Cancelled,
        Status::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Accepted => "accepted",
            Status::Partial => "partial",
            Status::Unverified => "unverified",
            Status::NeedsApproval => "needs_approval",
            Status::NeedsAuth => "needs_auth",
            Status::Unsupported => "unsupported",
            Status::MissingDependency => "missing_dependency",
            Status::InvalidInput => "invalid_input",
            Status::InvalidTarget => "invalid_target",
            Status::Unavailable => "unavailable",
            Status::Timeout => "timeout",
            Status::Cancelled => "cancelled",
            Status::Failed => "failed",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Status::Ok => "Done, and checked where the command checks.",
            Status::Accepted => "Started or queued; follow it with the returned id.",
            Status::Partial => "Part of it was done; the result says what is left.",
            Status::Unverified => "Delivered, but the outcome wasn't confirmed; look before relying on it.",
            Status::NeedsApproval => "Waiting for the person to allow it; never answer for them.",
            Status::NeedsAuth => "Sign-in or an account connection is needed.",
            Status::Unsupported => "This build or platform can't do it.",
            Status::MissingDependency => "A tool, permission or service it needs is missing on this machine.",
            Status::InvalidInput => "The command or its arguments are wrong.",
            Status::InvalidTarget => "The pane, window, element, task or item doesn't exist or is stale; look it up again.",
            Status::Unavailable => "Dragent, or the service behind it, isn't reachable.",
            Status::Timeout => "No answer in time; it may still have happened, so inspect before retrying.",
            Status::Cancelled => "Stopped by the person or the app.",
            Status::Failed => "It didn't work; the message says why.",
        }
    }

    /// The command line's exit code: 0 for results, a distinct code for each
    /// kind of failure so scripts can branch without reading text.
    pub fn exit_code(self) -> i32 {
        match self {
            Status::Ok | Status::Accepted | Status::Partial | Status::Unverified => 0,
            Status::Failed => 1,
            Status::InvalidInput => 2,
            Status::NeedsAuth => 3,
            Status::Unavailable => 4,
            Status::InvalidTarget => 5,
            Status::NeedsApproval => 6,
            Status::Timeout => 7,
            Status::Cancelled => 8,
            Status::Unsupported => 9,
            Status::MissingDependency => 10,
        }
    }

    /// Whether nothing happened, so running the same command again is safe.
    fn nothing_happened(self) -> bool {
        matches!(
            self,
            Status::InvalidInput
                | Status::InvalidTarget
                | Status::NeedsAuth
                | Status::Unavailable
                | Status::Unsupported
                | Status::MissingDependency
        )
    }
}

/// Key that marks an encoded failure.
const MARKER: &str = "dragent_failure";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub status: Status,
    /// Stable, specific reason, e.g. `app_not_open`, `no_answer`.
    pub code: String,
    /// For people.
    pub message: String,
    /// What to do about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The request the app may still be working on (`dragent requests show ID`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// True when nothing happened, so the same command can simply run again.
    pub retry_safe: bool,
}

impl Failure {
    pub fn new(status: Status, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            next: None,
            request_id: None,
            retry_safe: status.nothing_happened(),
        }
    }

    pub fn next(mut self, next: impl Into<String>) -> Self {
        self.next = Some(next.into());
        self
    }

    pub fn request(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// For the `Err` side of the existing plumbing.
    pub fn encode(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        value[MARKER] = json!(1);
        value.to_string()
    }

    pub fn decode(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text.trim()).ok()?;
        value.get(MARKER)?;
        serde_json::from_value(value).ok()
    }

    /// Any error text as a failure: decoded when encoded, otherwise a plain
    /// [`Status::Failed`] carrying the text.
    pub fn from_error(text: &str) -> Self {
        Self::decode(text).unwrap_or_else(|| Failure {
            retry_safe: false,
            ..Failure::new(Status::Failed, "error", text.trim().trim_start_matches("dragent: "))
        })
    }

    /// The JSON agents read.
    pub fn to_json(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        value["ok"] = json!(false);
        value
    }
}

/// The contract itself, for capability discovery.
pub fn contract() -> Value {
    json!({
        "statuses": Status::ALL.iter().map(|status| json!({
            "status": status.as_str(),
            "meaning": status.meaning(),
            "exit_code": status.exit_code(),
        })).collect::<Vec<_>>(),
        "failure": {
            "fields": ["ok", "status", "code", "message", "next", "request_id", "retry_safe"],
            "where": "stdout as JSON in agent mode (stderr gets a line for people); \
                      MCP tool errors carry the same object as text and structuredContent",
        },
        "success": "The command's JSON; a \"status\" field of accepted, partial or unverified \
                    qualifies it, and its absence means ok.",
        "after_timeout": "dragent requests show REQUEST_ID says whether the app finished it and what it answered.",
        "idempotency": "--idempotency-key KEY: a repeat with the same key and arguments replays the \
                        first result, and refuses to run again while the first one's outcome is unknown.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_round_trip_and_plain_text_reads_as_failed() {
        let failure = Failure::new(Status::Timeout, "no_answer", "Dragent didn't answer in time")
            .next("dragent requests show abc")
            .request("abc");
        assert!(!failure.retry_safe, "a timeout may have happened");
        let decoded = Failure::decode(&failure.encode()).unwrap();
        assert_eq!(decoded, failure);
        let json = decoded.to_json();
        assert_eq!(json["ok"], false);
        assert_eq!(json["status"], "timeout");
        assert_eq!(json["request_id"], "abc");

        let plain = Failure::from_error("dragent: something broke");
        assert_eq!((plain.status, plain.code.as_str()), (Status::Failed, "error"));
        assert_eq!(plain.message, "something broke");
        assert!(Failure::decode("{\"code\":\"x\"}").is_none(), "only marked objects decode");
        assert!(Failure::new(Status::InvalidInput, "bad", "x").retry_safe);
    }

    #[test]
    fn every_status_has_a_distinct_failure_exit_code() {
        let mut codes = std::collections::HashSet::new();
        for status in Status::ALL {
            let code = status.exit_code();
            if code != 0 {
                assert!(codes.insert(code), "{status:?} shares exit code {code}");
            }
            let name = serde_json::to_value(status).unwrap();
            assert_eq!(name, status.as_str());
        }
        assert_eq!(contract()["statuses"].as_array().unwrap().len(), Status::ALL.len());
    }
}
