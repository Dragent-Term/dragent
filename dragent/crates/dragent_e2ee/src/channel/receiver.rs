//! §4.3 DATA receive algorithm: replay, ordering, and no gaps.
//!
//! ```text
//! on DATA frame with counter c:
//!   if c >= 2^32:                              protocol_error
//!   if prior != None and c <= prior:           drop silently    // replay or duplicate
//!   plaintext = AEAD_open(key, nonce = c, ct)  else protocol_error (prior unchanged)
//!   expected = (prior == None) ? 0 : prior + 1
//!   if c != expected:                          protocol_error   // a frame is missing: never process
//!   prior = c                                  // committed only after AEAD success and the gap check
//!   process record(plaintext)
//! ```
//!
//! Plus §4.6 step 3: once a resume names this (old) channel's
//! `last_sent_ctr`, an authenticated frame above it is a protocol error.
//!
//! The receiver itself does not latch failures; [`super::Channel`] closes and
//! zeroises on the first error so that nothing after it is ever opened.

use super::{ProtocolViolation, COUNTER_LIMIT};

/// What to do with a DATA frame that passed the checks.
#[derive(Debug, PartialEq, Eq)]
pub enum Received<T> {
    /// Process this record now. It is the next frame in sequence.
    Process(T),
    /// A replay or duplicate: drop it silently.
    Duplicate,
}

/// Receive-side state for one channel direction.
#[derive(Debug, Clone, Default)]
pub struct Receiver {
    prior: Option<u64>,
    /// `Some(x)`: §4.6 resume ceiling; frames above `x` are a violation
    /// (`Some(None)`: the phone sent nothing on this channel, so any frame is).
    ceiling: Option<Option<u64>>,
}

impl Receiver {
    /// A fresh receiver (`prior = None`).
    pub fn new() -> Self {
        Self::default()
    }

    /// The last counter processed, if any.
    pub fn prior(&self) -> Option<u64> {
        self.prior
    }

    /// Frames received and processed so far.
    pub fn processed_count(&self) -> u64 {
        self.prior.map_or(0, |p| p + 1)
    }

    /// §4.6 step 3: the sender promised to send nothing after
    /// `last_sent_ctr` on this channel.
    pub fn set_resume_ceiling(&mut self, last_sent_ctr: Option<u64>) {
        self.ceiling = Some(last_sent_ctr);
    }

    /// Run the §4.3 algorithm for a frame with counter `c`. `open` performs
    /// the AEAD open with nonce `c`; it is called at most once, and only for a
    /// frame that is not a replay.
    pub fn receive<T, E>(
        &mut self,
        c: u64,
        open: impl FnOnce(u64) -> Result<T, E>,
    ) -> Result<Received<T>, ProtocolViolation> {
        if c >= COUNTER_LIMIT {
            return Err(ProtocolViolation::CounterOutOfRange { ctr: c });
        }
        if let Some(p) = self.prior {
            if c <= p {
                return Ok(Received::Duplicate);
            }
        }
        let plaintext = open(c).map_err(|_| ProtocolViolation::AuthenticationFailed { ctr: c })?;
        let expected = self.prior.map_or(0, |p| p + 1);
        if c != expected {
            return Err(ProtocolViolation::Gap { expected, got: c });
        }
        if let Some(ceiling) = self.ceiling {
            if ceiling.is_none_or(|last| c > last) {
                return Err(ProtocolViolation::BeyondResumePoint {
                    last_sent_ctr: ceiling,
                    got: c,
                });
            }
        }
        self.prior = Some(c);
        Ok(Received::Process(plaintext))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A stand-in AEAD: a frame is authentic iff its tag byte is `ok`.
    fn deliver(
        r: &mut Receiver,
        log: &RefCell<Vec<u64>>,
        c: u64,
        authentic: bool,
    ) -> Result<Received<u64>, ProtocolViolation> {
        r.receive(c, |n| if authentic { Ok(n) } else { Err(()) })
            .inspect(|got| {
                if let Received::Process(n) = got {
                    log.borrow_mut().push(*n);
                }
            })
    }

    #[test]
    fn clean_sequence_0_1_2() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        for c in 0..3 {
            assert_eq!(deliver(&mut r, &log, c, true), Ok(Received::Process(c)));
        }
        assert_eq!(*log.borrow(), vec![0, 1, 2]);
        assert_eq!(r.prior(), Some(2));
    }

    #[test]
    fn gap_0_1_3_is_protocol_error_and_3_is_never_processed() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        deliver(&mut r, &log, 0, true).unwrap();
        deliver(&mut r, &log, 1, true).unwrap();
        assert_eq!(
            deliver(&mut r, &log, 3, true),
            Err(ProtocolViolation::Gap {
                expected: 2,
                got: 3
            })
        );
        assert_eq!(*log.borrow(), vec![0, 1], "frame 3 never processed");
        assert_eq!(r.prior(), Some(1), "prior not advanced past the gap");
    }

    #[test]
    fn first_frame_2_is_protocol_error() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        assert_eq!(
            deliver(&mut r, &log, 2, true),
            Err(ProtocolViolation::Gap {
                expected: 0,
                got: 2
            })
        );
        assert!(log.borrow().is_empty());
        assert_eq!(r.prior(), None);
    }

    #[test]
    fn replayed_1_is_dropped_without_opening() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        deliver(&mut r, &log, 0, true).unwrap();
        deliver(&mut r, &log, 1, true).unwrap();
        let opened = RefCell::new(false);
        let got = r.receive(1, |n| {
            *opened.borrow_mut() = true;
            Ok::<_, ()>(n)
        });
        assert_eq!(got, Ok(Received::Duplicate));
        assert!(!*opened.borrow(), "a replay is never decrypted");
        assert_eq!(deliver(&mut r, &log, 2, true), Ok(Received::Process(2)));
    }

    #[test]
    fn forged_2_pow_20_with_bad_tag_leaves_prior_unchanged() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        deliver(&mut r, &log, 0, true).unwrap();
        assert_eq!(
            deliver(&mut r, &log, 1 << 20, false),
            Err(ProtocolViolation::AuthenticationFailed { ctr: 1 << 20 })
        );
        assert_eq!(r.prior(), Some(0));
    }

    #[test]
    fn ctr_at_or_above_2_pow_32_is_protocol_error() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        for c in [1u64 << 32, u64::MAX] {
            assert_eq!(
                deliver(&mut r, &log, c, true),
                Err(ProtocolViolation::CounterOutOfRange { ctr: c })
            );
        }
        assert_eq!(r.prior(), None);
    }

    #[test]
    fn resume_ceiling() {
        let (mut r, log) = (Receiver::new(), RefCell::new(vec![]));
        deliver(&mut r, &log, 0, true).unwrap();
        r.set_resume_ceiling(Some(1));
        assert_eq!(deliver(&mut r, &log, 1, true), Ok(Received::Process(1)));
        assert_eq!(
            deliver(&mut r, &log, 2, true),
            Err(ProtocolViolation::BeyondResumePoint {
                last_sent_ctr: Some(1),
                got: 2
            })
        );
        let mut fresh = Receiver::new();
        fresh.set_resume_ceiling(None);
        assert_eq!(
            deliver(&mut fresh, &log, 0, true),
            Err(ProtocolViolation::BeyondResumePoint {
                last_sent_ctr: None,
                got: 0
            })
        );
    }
}
