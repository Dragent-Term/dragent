//! Client-side compaction trigger policy.
//!
//! The server compacts using its database-owned context window, but the
//! client used to send the full history on every turn with no backstop: if
//! the server-side pass ever missed, the only client valve was the 200-turn
//! hard stop ("use /clear", destroying context). `compact_history` (the
//! `/api/v1/agent/compact` endpoint: summarize the middle, preserve the
//! first user message and recent turns) existed with zero callers.
//!
//! This module is the missing trigger: a cheap estimate-based threshold the
//! controller checks before opening a stream. On trip the caller compacts,
//! sends the compacted history, and merges back any messages appended during
//! the turn (see the merge-back slot in the controller). On any compaction
//! error the caller sends the full history — a backstop must never fail a
//! turn.

/// Estimated-token threshold above which the controller compacts before
/// opening a stream. Conservative against a 200k-class window: the estimate
/// (`chars / 3`) already carries a ~33% safety buffer, so 100k estimated is
/// roughly 75k real tokens — early enough to compact once, not every turn.
pub const COMPACT_THRESHOLD_TOKENS: usize = 100_000;

/// Minimum history size worth compacting. Below this the fixed cost of a
/// summarization round outweighs any saving.
pub const COMPACT_MIN_MESSAGES: usize = 10;

/// True when `history` should be compacted before opening a stream.
pub fn should_compact(message_count: usize, estimated_tokens: usize) -> bool {
    message_count >= COMPACT_MIN_MESSAGES && estimated_tokens >= COMPACT_THRESHOLD_TOKENS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_only_when_both_thresholds_met() {
        assert!(!should_compact(5, 200_000));
        assert!(!should_compact(50, 50_000));
        assert!(should_compact(50, 150_000));
        assert!(should_compact(
            COMPACT_MIN_MESSAGES,
            COMPACT_THRESHOLD_TOKENS
        ));
    }

    #[test]
    fn short_sessions_never_compact() {
        assert!(!should_compact(0, 0));
        assert!(!should_compact(9, usize::MAX));
    }
}
