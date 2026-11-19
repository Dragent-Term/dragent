//! Semantic retry policy for opening provider streams.
//!
//! Covers the failure the transport reconnect cannot: the *establishment*
//! POST failing with a transient provider/billing error (HTTP 429 quota,
//! 5xx, timeouts). Previously any of these immediately became an error card
//! and the user had to continue manually; now bounded retries with
//! exponential backoff (honoring the server's wait hint) happen first.
//!
//! Never retried: auth, insufficient credits, parse errors, and definitive
//! failures — retrying those burns quota or loops forever. Overflow is never
//! retried (that is the compaction path, not the retry path).

use std::time::Duration;

use crate::pipeline::StreamError;

/// Bounded retry configuration for stream establishment.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Total attempts including the first try.
    pub max_attempts: u32,
    /// Base backoff: attempt `n` (0-based) waits `base * 2^n` plus jitter.
    pub base_delay: Duration,
    /// Upper bound for any single wait without an explicit server hint.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            // A handful of tries, then surface the error to the user.
            max_attempts: 5,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// True when `error` is worth another establishment attempt.
pub fn is_retryable(error: &StreamError) -> bool {
    match error {
        StreamError::RateLimited(_) | StreamError::Network(_) | StreamError::Timeout(_) => true,
        StreamError::Auth(_)
        | StreamError::ProviderKey(_)
        | StreamError::InsufficientCredits(_)
        | StreamError::Parse(_) => false,
    }
}

/// How long to wait before attempt `attempt` (0-based, i.e. the wait preceding
/// retry number `attempt + 1`). A server-provided wait hint wins; otherwise
/// exponential backoff with a small deterministic jitter so synchronized
/// clients do not retry in lockstep.
pub fn delay_for_attempt(policy: &RetryPolicy, attempt: u32, error: &StreamError) -> Duration {
    let hinted = match error {
        StreamError::RateLimited(hint) => parse_retry_after_seconds(hint),
        _ => None,
    };
    if let Some(secs) = hinted {
        // Honor the server's hint, clamped to something sane: a hint is
        // authoritative, but never sleep longer than one minute per wait.
        return Duration::from_secs(secs.min(60)).max(Duration::from_secs(1));
    }
    let exp = policy
        .base_delay
        .checked_mul(1 << attempt.min(10))
        .unwrap_or(policy.max_delay);
    let capped = exp.min(policy.max_delay);
    // Deterministic ~10% jitter from the attempt index (no RNG dependency in
    // the core crate).
    let jitter_ms = (attempt as u64 * 137 % 200).min(capped.as_millis() as u64 / 5 + 1);
    capped + Duration::from_millis(jitter_ms)
}

/// Best-effort parse of a human wait hint ("Try again after 45s",
/// "after 2 minutes", "retry in 90 seconds", "resets in 5m") into seconds.
/// Returns `None` when no duration is recognizable — the caller falls back to
/// exponential backoff.
pub fn parse_retry_after_seconds(hint: &str) -> Option<u64> {
    let lower = hint.to_lowercase();
    // Find the first number and interpret it with the unit word that follows.
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let num: u64 = lower[i..j].parse().ok()?;
            // First alphabetic word after the number, if any. Matching whole
            // words (not prefixes) so "error 429 soon" never parses as a wait.
            let rest = lower[j..].trim_start();
            let word: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let secs = match word.as_str() {
                "ms" | "millis" | "millisecond" | "milliseconds" => num / 1000,
                "s" | "sec" | "secs" | "second" | "seconds" => num,
                "m" | "min" | "mins" | "minute" | "minutes" => num * 60,
                "h" | "hr" | "hrs" | "hour" | "hours" => num * 3600,
                "d" | "day" | "days" => num * 86400,
                "" => {
                    // Bare number with no unit: only trust it in an explicit
                    // "after N" / "in N" position, otherwise ignore (so an
                    // "error 429" never parses as a wait).
                    let before = lower[..i].trim_end();
                    if before.ends_with("after") || before.ends_with("in") {
                        num
                    } else {
                        i = j;
                        continue;
                    }
                }
                _ => {
                    i = j;
                    continue;
                }
            };
            return Some(secs);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_classification() {
        assert!(is_retryable(&StreamError::RateLimited("x".into())));
        assert!(is_retryable(&StreamError::Network("x".into())));
        assert!(is_retryable(&StreamError::Timeout("x".into())));
        assert!(!is_retryable(&StreamError::Auth("x".into())));
        assert!(!is_retryable(&StreamError::InsufficientCredits("x".into())));
        assert!(!is_retryable(&StreamError::Parse("x".into())));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RetryPolicy::default();
        let err = StreamError::Network("down".into());
        let d0 = delay_for_attempt(&policy, 0, &err);
        let d1 = delay_for_attempt(&policy, 1, &err);
        let d2 = delay_for_attempt(&policy, 2, &err);
        assert!(d0 >= Duration::from_secs(2) && d0 < Duration::from_secs(3));
        assert!(d1 >= Duration::from_secs(4) && d1 < Duration::from_secs(6));
        assert!(d2 >= Duration::from_secs(8) && d2 < Duration::from_secs(11));
        let d9 = delay_for_attempt(&policy, 9, &err);
        assert!(d9 <= Duration::from_secs(31));
    }

    #[test]
    fn server_hint_wins_and_clamps() {
        let policy = RetryPolicy::default();
        let err = StreamError::RateLimited("Try again after 45s".into());
        assert_eq!(delay_for_attempt(&policy, 0, &err), Duration::from_secs(45));
        let err = StreamError::RateLimited("after 2 minutes".into());
        // Parsed as 120s but clamped to the 60s per-wait ceiling.
        assert_eq!(delay_for_attempt(&policy, 3, &err), Duration::from_secs(60));
        // Unparseable hint falls back to backoff.
        let err = StreamError::RateLimited("slow down".into());
        assert!(delay_for_attempt(&policy, 0, &err) < Duration::from_secs(3));
    }

    #[test]
    fn parses_common_hint_shapes() {
        assert_eq!(parse_retry_after_seconds("Try again after 45s"), Some(45));
        assert_eq!(parse_retry_after_seconds("after 2 minutes"), Some(120));
        assert_eq!(parse_retry_after_seconds("retry in 90 seconds"), Some(90));
        assert_eq!(parse_retry_after_seconds("resets in 5m"), Some(300));
        assert_eq!(parse_retry_after_seconds("slow down"), None);
        assert_eq!(parse_retry_after_seconds("error 429"), None);
    }
}
