//! Turn raw agent/provider errors into calm, user-facing messages.
//!
//! Technical detail (GraphQL locations, provider URLs, internal wrapping) stays
//! in the logs; the user sees something they can act on — especially for the
//! common, expected cases like hitting a usage limit.
//!
//! Moved verbatim from `app/src/agent/error_display.rs` (pure string mapping,
//! zero WarpUI deps). The app re-exports this path.

/// Map a raw error string to a user-optimized message. The raw text is assumed
/// to already be logged by the caller.
pub fn humanize_agent_error(raw: &str) -> String {
    let low = raw.to_lowercase();

    // ── Usage limits (expected, actionable) ───────────────────────────────────
    // Purchased credits available but switched off — the one-toggle fix.
    if low.contains("credit usage not enabled") || low.contains("enable \"use my credits\"") {
        return "You've hit your plan limit, but you have purchased credits. \
                Turn on \"Use my credits\" in Settings to keep going."
            .to_string();
    }
    // Free monthly allowance spent — check FIRST (more specific than the paid
    // branch, since both messages now contain "monthly allowance").
    if low.contains("free monthly allowance")
        || (low.contains("free") && low.contains("allowance") && low.contains("resets on"))
    {
        let mut msg = "You've used your free monthly allowance.".to_string();
        if let Some(when) = after(raw, "resets on ").or_else(|| after(raw, "reset on ")) {
            msg.push_str(&format!(" It resets on {when}."));
        }
        return msg;
    }
    // Monthly allowance spent (paid).
    if low.contains("monthly allowance") {
        let mut msg = "You've used your monthly allowance.".to_string();
        if let Some(when) = after(raw, "renews on ") {
            msg.push_str(&format!(" It renews on {when}."));
        }
        return msg;
    }
    if low.contains("insufficient") && low.contains("credit") {
        return "You're out of credits. Top up your balance to continue using the agent."
            .to_string();
    }
    if low.contains("too many requests") || (low.contains("rate") && low.contains("limit")) {
        return "You're going a bit fast — please wait a moment and try again.".to_string();
    }

    // ── Auth ──────────────────────────────────────────────────────────────────
    if low.contains("unauthorized")
        || low.contains("authentication required")
        || low.contains("invalid user")
    {
        return "Your session expired. Please sign in again.".to_string();
    }

    // ── Upstream / provider failures (hide the technical detail) ──────────────
    if low.contains("provider returned")
        || low.contains("api.anthropic")
        || low.contains("api.openai")
        || low.contains("/v1/messages")
        || low.contains("/v1/chat")
        || low.contains("bad gateway")
        || low.contains("502")
        || low.contains("503")
    {
        return "The AI service had a temporary problem. Please try again in a moment.".to_string();
    }
    if low.contains("timed out") || low.contains("timeout") || low.contains("deadline") {
        return "That took too long and timed out. Please try again.".to_string();
    }
    if low.contains("network") || low.contains("connection") || low.contains("dial ") {
        return "Couldn't reach the service. Check your connection and try again.".to_string();
    }

    // ── Unknown / internal — generic (raw is in the logs) ─────────────────────
    "Something went wrong. Please try again.".to_string()
}

/// Return the trimmed remainder of `s` after the first occurrence of `marker`,
/// used to lift a reset timestamp out of a server message.
fn after(s: &str, marker: &str) -> Option<String> {
    s.find(marker)
        .map(|i| {
            s[i + marker.len()..]
                .trim()
                .trim_end_matches('.')
                .to_string()
        })
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monthly_allowance_keeps_renewal_date() {
        let raw = "your monthly allowance is used up. It renews on January 2, 2026 15:04 UTC";
        let got = humanize_agent_error(raw);
        assert!(got.contains("monthly allowance"));
        assert!(got.contains("renews on January 2, 2026 15:04 UTC"));
    }

    #[test]
    fn free_monthly_allowance_is_distinct_from_paid() {
        let raw = "your free monthly allowance resets on January 2, 2026 15:04 UTC";
        let got = humanize_agent_error(raw);
        assert!(got.contains("free monthly allowance"), "got: {got}");
        assert!(
            got.contains("resets on January 2, 2026 15:04 UTC"),
            "got: {got}"
        );
    }

    #[test]
    fn credits_not_enabled_prompts_the_toggle() {
        let raw = "credit usage not enabled: you have purchased credits available. \
                   Enable \"Use my credits\" in Settings to continue past this limit";
        let got = humanize_agent_error(raw);
        assert!(got.contains("Use my credits"));
    }

    #[test]
    fn provider_error_is_hidden() {
        let raw =
            "agent step failed: provider returned error for https://api.anthropic.com/v1/messages";
        let got = humanize_agent_error(raw);
        assert!(!got.contains("anthropic"));
        assert!(got.contains("temporary problem"));
    }

    #[test]
    fn unknown_is_generic() {
        assert_eq!(
            humanize_agent_error("weird internal thing {column:17}"),
            "Something went wrong. Please try again."
        );
    }
}
