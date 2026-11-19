//! Stable error labels and actionable recovery information for agent clients.
use serde_json::{json, Value};

/// Every error code computer use reports, as agents should match on them.
pub const CODES: &[&str] = &[
    "app_not_found",
    "app_blocked",
    "window_not_found",
    "window_not_focused",
    "window_stale",
    "unsupported_capability",
    "permission_denied",
    "element_not_found",
    "element_not_clickable",
    "action_not_supported",
    "invalid_argument",
    "action_timeout",
    "screenshot_failed",
    "accessibility_error",
];

pub fn details(message: &str) -> Value {
    if let Ok(value) = serde_json::from_str::<Value>(message) {
        if value["code"].is_string() && value["message"].is_string() {
            return value;
        }
    }
    let prefix = message.split(':').next().unwrap_or_default();
    let codes = CODES;
    let lower = message.to_lowercase();
    let code = if codes.contains(&prefix) {
        prefix
    } else if lower.contains("computer use is off")
        || lower.contains("allowed list")
        || lower.contains("allowlist")
        || lower.contains("permission")
        || lower.contains("grant accessibility")
        || lower.contains("isn't allowed")
    {
        "permission_denied"
    } else if lower.contains("holds passwords") || lower.contains("off limits") {
        "app_blocked"
    } else if lower.contains("no running app") || lower.contains("no app is running") {
        "app_not_found"
    } else if lower.contains("take a snapshot") || lower.contains("no element") {
        "element_not_found"
    } else if lower.contains("snapshot changed") || lower.contains("window has closed") {
        "window_stale"
    } else if lower.contains("timed out") || lower.contains("in time") {
        "action_timeout"
    } else if lower.contains("no window") || lower.contains("no open window") {
        "window_not_found"
    } else {
        "accessibility_error"
    };
    let next = match code {
        "app_not_found"=>"List running apps and select an exact app id or pid. Website names refer to their desktop browser.",
        "app_blocked"=>"Leave this app to the person; desktop control is deliberately unavailable for it.",
        "window_not_found"|"window_stale"=>"List windows, choose a current id, and take a new snapshot of that window.",
        "window_not_focused"=>"Inspect the focused receiver. Bring the selected window forward once with restore_window, or ask the person to focus it.",
        "element_not_found"|"element_not_clickable"=>"Take a new snapshot and choose a visible element with a frame; do not infer indexes from the element count.",
        "action_not_supported"=>"Inspect the element's advertised actions and choose an available action.",
        "action_timeout"=>"Inspect the app before repeating any action: input may already have been delivered.",
        "permission_denied"=>"Check computer permissions and Computer use settings. The person grants OS access and app permissions.",
        "unsupported_capability"=>"Check capabilities and platform dependencies. Prefer an available accessibility action.",
        "invalid_argument"=>"Correct the arguments using dragent computer help before retrying.",
        _=>"Read the app state and capability/permission status before choosing another operation.",
    };
    json!({"code":code,"message":message,"next_steps":[next]})
}
