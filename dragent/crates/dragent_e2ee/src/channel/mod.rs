//! Channel state: the §4.2 companion state machine, the §4.3 DATA receiver
//! and the §4.6 resume bookkeeping. All of it is pure state driven by the
//! caller's event loop; time is a caller-supplied monotonic clock in
//! milliseconds (`now_ms`), so nothing here reads a clock or does I/O.
//!
//! - [`receiver::Receiver`]: per channel and direction, replay/ordering with
//!   **any gap a protocol error** (§4.3).
//! - [`state::Channel`]: one channel's keys, counters and §4.2 state (no data
//!   before confirmation; zeroise on close).
//! - [`resume`]: rekey/reconnect resume (§4.6) on both ends, including the
//!   security review's implementer notes 1–3.

use std::fmt;

pub mod receiver;
pub mod resume;
pub mod state;

pub use receiver::{Received, Receiver};
pub use state::{
    kk_listen, Channel, ChannelError, ChannelState, Inbound, ListenRefusal, Listened, Outbound,
    PinLookup, Purpose, Role,
};

/// Counters at or above this are a protocol error (§4.3).
pub const COUNTER_LIMIT: u64 = 1 << 32;
/// The phone rekeys after this many DATA frames on a channel (§4.6).
pub const REKEY_AFTER_FRAMES: u64 = 1 << 20;
/// ... or after this long (§4.6).
pub const REKEY_AFTER_MS: u64 = 60 * 60 * 1000;
/// The KK responder drops a channel not confirmed within this (§4.2).
pub const CONFIRM_TIMEOUT_MS: u64 = 10_000;
/// The pairing responder discards a tentative record after this (§6.2 step 9).
pub const PAIRING_CONFIRM_TIMEOUT_MS: u64 = 30_000;
/// The desktop waits at most this long for the old channel to drain (§4.6 step 3).
pub const DRAIN_TIMEOUT_MS: u64 = 10_000;
/// Replies already owed on an old channel may finish within this (§4.6 step 5).
pub const WIND_DOWN_MS: u64 = 10_000;
/// At most this many channels per (phone, desktop) pair (§4.6 step 5).
pub const MAX_CHANNELS_PER_PAIR: usize = 2;

/// Why a channel hit `protocol_error`. Every variant means: CLOSE
/// `protocol_error`, keys zeroised (already done by [`Channel`]), partial
/// messages discarded, pending requests failed, and on the desktop all input
/// from the channel not yet written to a PTY discarded (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolViolation {
    /// `ctr >= 2^32`.
    CounterOutOfRange {
        /// The counter received.
        ctr: u64,
    },
    /// The DATA frame failed AEAD (bad tag, wrong key, forged counter).
    AuthenticationFailed {
        /// The counter received.
        ctr: u64,
    },
    /// An authenticated frame arrived out of sequence: a frame is missing.
    Gap {
        /// The counter that was due.
        expected: u64,
        /// The counter received.
        got: u64,
    },
    /// An old channel sent a frame above the `last_sent_ctr` the phone
    /// promised in its new channel's `hello` (§4.6 step 3).
    BeyondResumePoint {
        /// The promised last counter (`None`: nothing at all was promised).
        last_sent_ctr: Option<u64>,
        /// The counter received.
        got: u64,
    },
    /// The resume in `hello` contradicts what the desktop processed on the old
    /// channel (it processed more than the phone says it sent).
    InconsistentResume,
}

impl fmt::Display for ProtocolViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolViolation::CounterOutOfRange { ctr } => write!(f, "counter {ctr} out of range"),
            ProtocolViolation::AuthenticationFailed { ctr } => {
                write!(f, "frame {ctr} failed authentication")
            }
            ProtocolViolation::Gap { expected, got } => {
                write!(f, "frame {got} arrived when {expected} was due")
            }
            ProtocolViolation::BeyondResumePoint { last_sent_ctr, got } => {
                write!(
                    f,
                    "old-channel frame {got} beyond resume point {last_sent_ctr:?}"
                )
            }
            ProtocolViolation::InconsistentResume => f.write_str("inconsistent resume point"),
        }
    }
}

impl std::error::Error for ProtocolViolation {}

/// Why a channel closed. The wire encoding of CLOSE reasons belongs to the
/// codec; this is the state-machine view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// §4.3: see [`ProtocolViolation`]. Send CLOSE `protocol_error`; report "lost".
    ProtocolError(ProtocolViolation),
    /// §4.2: not confirmed within the timeout. Drop silently.
    ConfirmTimeout,
    /// §4.2: the first DATA authenticated but wasn't `hello` (or
    /// `pair-confirm`, or its `h` didn't match). Drop.
    NotConfirmed,
    /// §4.6 step 5: the old channel finished winding down after a resume.
    Rekeyed,
    /// §4.6 step 5: a third channel for the pair evicted this one.
    Evicted,
    /// §6.4: the peer was revoked or unpaired.
    Revoked,
    /// The peer or relay closed it (CLOSE frame, socket gone).
    PeerClosed,
    /// Closed locally for any other reason (shutdown, lease ended).
    Local,
}
