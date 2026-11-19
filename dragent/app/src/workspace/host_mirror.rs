//! Host-session mirror pump: byte-cursor tracking with gap resync.
//!
//! Framework-free: no warpui, no host RPC. The view layer (`view_session`)
//! polls `HostClient::read_terminal_since` and delivers [`MirrorMsg`]; this
//! module owns the cursor math so it is unit-testable in isolation.

/// Consecutive failures (usually [`MirrorMsg::Gone`]) before the pump gives
/// up and marks the mirror ended. The host run continues headless.
pub const MAX_MIRROR_FAILURES: u32 = 5;

/// One poll result from the host's terminal byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorMsg {
    Chunk {
        base_offset: u64,
        total_bytes: u64,
        data: Vec<u8>,
        /// The shell's exit code once it has exited.
        exit_code: Option<i32>,
    },
    /// The host answered and confirmed that the session no longer exists.
    Gone,
    /// The host or daemon could not answer; the session may still be alive.
    Unavailable,
}

/// The unseen suffix of a chunk that the view must feed to the mirror pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorFeed {
    pub bytes: Vec<u8>,
    pub gap: bool,
}

/// Cursor state for one bound host session mirror.
#[derive(Debug, Clone)]
pub struct HostMirror {
    pub session_id: String,
    pub next_offset: u64,
    pub ended: bool,
    pub failures: u32,
    /// The shell exited on its own (and its output is fully drained). An
    /// exited binding is finished, not lost: it is never respawned.
    pub exited: Option<i32>,
}

impl HostMirror {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            next_offset: 0,
            ended: false,
            failures: 0,
            exited: None,
        }
    }

    /// Fold one host chunk into the cursor.
    ///
    /// - Empty data is a no-op (cursor and failure budget untouched).
    /// - `base_offset > cursor` means the host rotated/truncated behind us:
    ///   resync by feeding the whole chunk and flagging the gap.
    /// - Otherwise skip the already-seen prefix and feed the unseen suffix.
    /// A successful (non-empty) apply clears the failure budget.
    pub fn apply_chunk(&mut self, base_offset: u64, total_bytes: u64, data: &[u8]) -> MirrorFeed {
        if data.is_empty() {
            return MirrorFeed {
                bytes: Vec::new(),
                gap: false,
            };
        }
        self.failures = 0;
        if base_offset > self.next_offset {
            // Turnover: bytes were lost behind us; feed everything we got.
            self.next_offset = base_offset
                .saturating_add(data.len() as u64)
                .min(total_bytes.max(base_offset));
            return MirrorFeed {
                bytes: data.to_vec(),
                gap: true,
            };
        }
        let seen = (self.next_offset - base_offset) as usize;
        if seen >= data.len() {
            // Fully stale redelivery: nothing new, cursor never moves back.
            return MirrorFeed {
                bytes: Vec::new(),
                gap: false,
            };
        }
        self.next_offset = base_offset.saturating_add(data.len() as u64);
        MirrorFeed {
            bytes: data[seen..].to_vec(),
            gap: false,
        }
    }

    /// Record one failed poll (host unreachable, session gone). Ends the
    /// mirror after [`MAX_MIRROR_FAILURES`] consecutive strikes.
    pub fn note_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= MAX_MIRROR_FAILURES {
            self.ended = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HostMirror, MAX_MIRROR_FAILURES};

    #[test]
    fn fresh_mirror_feeds_first_chunk_whole() {
        let mut mirror = HostMirror::new("run-1".to_string());
        let feed = mirror.apply_chunk(0, 5, b"hello");
        assert_eq!(feed.bytes, b"hello");
        assert!(!feed.gap);
        assert_eq!(mirror.next_offset, 5);
        assert_eq!(mirror.failures, 0);
        assert!(!mirror.ended);
    }

    #[test]
    fn incremental_chunks_skip_seen_prefix() {
        let mut mirror = HostMirror::new("run-1".to_string());
        let feed = mirror.apply_chunk(0, 5, b"hello");
        assert_eq!(feed.bytes, b"hello");
        // Overlapping redelivery: only the unseen suffix is fed.
        let feed = mirror.apply_chunk(3, 8, b"lo world");
        assert_eq!(feed.bytes, b" world");
        assert!(!feed.gap);
        assert_eq!(mirror.next_offset, 11);
    }

    #[test]
    fn turnover_resyncs_with_gap_flag() {
        let mut mirror = HostMirror::new("run-1".to_string());
        let feed = mirror.apply_chunk(0, 5, b"hello");
        assert_eq!(mirror.next_offset, 5);
        // Host rotated: base jumped past our cursor; feed all, flag the gap.
        let feed = mirror.apply_chunk(90, 100, b"new-tail!!");
        assert_eq!(feed.bytes, b"new-tail!!");
        assert!(feed.gap);
        assert_eq!(mirror.next_offset, 100);
    }

    #[test]
    fn empty_chunk_is_noop() {
        let mut mirror = HostMirror::new("run-1".to_string());
        let feed = mirror.apply_chunk(0, 5, b"hello");
        assert_eq!(mirror.next_offset, 5);
        let feed = mirror.apply_chunk(5, 5, b"");
        assert!(feed.bytes.is_empty());
        assert!(!feed.gap);
        assert_eq!(mirror.next_offset, 5);
        assert_eq!(mirror.failures, 0);
    }

    #[test]
    fn failure_budget_ends_mirror_on_fifth_strike() {
        let mut mirror = HostMirror::new("run-1".to_string());
        for _ in 0..MAX_MIRROR_FAILURES - 1 {
            mirror.note_failure();
            assert!(!mirror.ended);
        }
        mirror.note_failure();
        assert!(mirror.ended);
        // Success clears the budget again.
        let mut mirror = HostMirror::new("run-2".to_string());
        mirror.note_failure();
        mirror.note_failure();
        let feed = mirror.apply_chunk(0, 3, b"abc");
        assert_eq!(feed.bytes, b"abc");
        assert_eq!(mirror.failures, 0);
        assert!(!mirror.ended);
    }
}
