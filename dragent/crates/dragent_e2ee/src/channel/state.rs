//! §4.2 companion channel state machine: no data before confirmation.
//!
//! | State | Event | Action |
//! |---|---|---|
//! | Initiator `new` | — | Send HS1 (`{"v":1,"caps":[…]}`; never side effects) |
//! | Responder `listen` | HS1 decrypts with the pin for `peer` | → `awaiting_confirm`, send HS2. **No DATA.** |
//! | Responder `listen` | HS1 fails, or `peer` unpinned/tombstoned | CLOSE `unknown_peer`, keep no state |
//! | Initiator `hs_sent` | HS2 decrypts | → `open`; DATA ctr 0 = CTRL `hello {resume?}` |
//! | Responder `awaiting_confirm` | first DATA authenticates and is `hello` | → `open`; only now serve |
//! | Responder `awaiting_confirm` | 10 s timeout, or any other first frame | drop and zeroise |
//! | any | AEAD failure, malformed record, protocol violation | CLOSE `protocol_error`, zeroise, report "lost" |
//!
//! The initiator's `new`/`hs_sent` states are [`crate::noise::kk::KkInitiator`];
//! responder `listen` is [`kk_listen`]. A [`Channel`] exists once a side has
//! transport keys. Pairing uses the same machine with `pair-confirm {h}` as the
//! confirming record and a 30 s timeout ([`Purpose::Pairing`]).
//!
//! Record decoding is the codec's job, so confirmation is two-step: the first
//! authenticated DATA comes back as [`Inbound::Confirming`], the caller decodes
//! it and calls [`Channel::confirm`] or [`Channel::reject_confirmation`].

use std::fmt;

use zeroize::Zeroizing;

use super::receiver::{Received, Receiver};
use super::{
    CloseReason, ProtocolViolation, CONFIRM_TIMEOUT_MS, COUNTER_LIMIT, PAIRING_CONFIRM_TIMEOUT_MS,
    REKEY_AFTER_FRAMES, REKEY_AFTER_MS, WIND_DOWN_MS,
};
use crate::ids::ChannelId;
use crate::noise::kk::KkResponder;
use crate::noise::{KkPrologue, PublicKey, StaticKeypair, Transport, HASH_LEN};

/// Which end of the handshake this side was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Sent HS1 (the phone).
    Initiator,
    /// Answered with HS2 (the desktop).
    Responder,
}

/// What the channel is for (§4.1 `purpose`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The live companion channel (KK); confirmed by `hello`.
    Companion,
    /// Pairing (IKpsk1); confirmed by `pair-confirm {h}`.
    Pairing,
}

impl Purpose {
    fn confirm_timeout_ms(self) -> u64 {
        match self {
            Purpose::Companion => CONFIRM_TIMEOUT_MS,
            Purpose::Pairing => PAIRING_CONFIRM_TIMEOUT_MS,
        }
    }
}

/// The §4.2 state of a channel that has transport keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    /// Responder: HS2 sent; waiting for the first DATA. Nothing may be sent or served.
    AwaitingConfirm,
    /// Responder: the first DATA authenticated and was handed to the caller to
    /// check; [`Channel::confirm`] or [`Channel::reject_confirmation`] is due.
    Confirming,
    /// Confirmed (responder) or HS2 verified (initiator).
    Open,
    /// Responder, §4.6 step 5: `bye{rekeyed}` sent; owed replies may still be
    /// sent until the deadline, but no new requests are taken.
    WindingDown,
    /// Closed; keys are zeroised.
    Closed(CloseReason),
}

/// A sealed DATA frame to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    /// The counter (header `ctr`, and the AEAD nonce).
    pub ctr: u64,
    /// Noise transport ciphertext including the tag (header `body`).
    pub ciphertext: Vec<u8>,
}

/// The result of receiving one DATA frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    /// The next record in sequence: process it.
    Record(Zeroizing<Vec<u8>>),
    /// Responder only: the first record. It authenticated; decode it and call
    /// [`Channel::confirm`] if it is `hello` (companion) or a matching
    /// `pair-confirm` (pairing), otherwise [`Channel::reject_confirmation`].
    /// Do not act on it in any other way first.
    Confirming(Zeroizing<Vec<u8>>),
    /// A replay or duplicate, dropped silently.
    Duplicate,
}

/// A channel operation could not be performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// §4.3: the channel just closed with `protocol_error` and its keys are
    /// zeroised. The caller must send CLOSE `protocol_error`, discard partial
    /// messages, fail pending requests and (desktop) discard every unwritten
    /// input from this channel.
    Protocol(ProtocolViolation),
    /// The channel is closed (already, for this reason).
    Closed(CloseReason),
    /// Not allowed in the current state: sending before confirmation,
    /// receiving while a confirmation decision is due, or sending on a
    /// quiesced channel.
    NotReady,
    /// The send counter would reach 2^32: the channel should have rekeyed long ago.
    CounterExhausted,
    /// The record is larger than one transport message allows.
    TooLarge,
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelError::Protocol(v) => write!(f, "protocol error: {v}"),
            ChannelError::Closed(r) => write!(f, "channel closed: {r:?}"),
            ChannelError::NotReady => f.write_str("not allowed in the channel's current state"),
            ChannelError::CounterExhausted => f.write_str("send counter exhausted"),
            ChannelError::TooLarge => f.write_str("record too large"),
        }
    }
}

impl std::error::Error for ChannelError {}

/// One end of a channel after its handshake: keys, counters and §4.2 state.
pub struct Channel {
    role: Role,
    purpose: Purpose,
    chan: ChannelId,
    transport: Transport,
    state: ChannelState,
    /// Next counter to send.
    send_next: u64,
    receiver: Receiver,
    opened_at_ms: u64,
    deadline_ms: Option<u64>,
    /// §4.6 step 1 (initiator): no more DATA on this channel.
    quiesced: bool,
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("role", &self.role)
            .field("purpose", &self.purpose)
            .field("chan", &self.chan)
            .field("state", &self.state)
            .field("send_next", &self.send_next)
            .field("prior", &self.receiver.prior())
            .field("quiesced", &self.quiesced)
            .finish_non_exhaustive()
    }
}

/// What the responder knows about the HS1 `peer` (§4.2 `listen`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinLookup {
    /// Pinned with this identity key.
    Pinned(PublicKey),
    /// Never paired.
    Unpinned,
    /// Revoked or unpaired: tombstoned (§6.4).
    Tombstoned,
}

/// The responder refused HS1: CLOSE `unknown_peer` and keep no state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenRefusal {
    /// `peer` unpinned or tombstoned, or HS1 failed with its pin.
    UnknownPeer,
}

/// A KK HS1 the responder accepted.
#[derive(Debug)]
pub struct Listened {
    /// The channel, in `awaiting_confirm`.
    pub channel: Channel,
    /// The HS1 payload (the phone's caps). No side effects: HS1 is replayable.
    pub hs1_payload: Zeroizing<Vec<u8>>,
    /// HS2, to send.
    pub hs2: Vec<u8>,
}

/// §4.2 responder `listen`: process a KK HS1 and, if it decrypts with the pin
/// for `peer`, produce HS2 and a channel in `awaiting_confirm`.
///
/// Returns the HS1 payload (the phone's caps). HS1 is replayable, so the
/// payload must cause no side effects; nothing is served until
/// [`Channel::confirm`].
pub fn kk_listen(
    local: &StaticKeypair,
    pin: PinLookup,
    prologue: &KkPrologue,
    hs1: &[u8],
    hs2_payload: &[u8],
    now_ms: u64,
) -> Result<Listened, ListenRefusal> {
    let PinLookup::Pinned(phone_key) = pin else {
        return Err(ListenRefusal::UnknownPeer);
    };
    let (resp, payload) = KkResponder::read_hs1(local, &phone_key, prologue, hs1)
        .map_err(|_| ListenRefusal::UnknownPeer)?;
    let (transport, hs2) = resp
        .write_hs2(hs2_payload)
        .map_err(|_| ListenRefusal::UnknownPeer)?;
    let channel = Channel::responder(Purpose::Companion, prologue.chan, transport, now_ms);
    Ok(Listened {
        channel,
        hs1_payload: payload,
        hs2,
    })
}

impl Channel {
    /// The initiator's channel once HS2 has verified: `open`. Its first DATA
    /// (ctr 0) must be CTRL `hello` (companion) or `pair-confirm` (pairing).
    pub fn initiator(purpose: Purpose, chan: ChannelId, transport: Transport, now_ms: u64) -> Self {
        Self::new(
            Role::Initiator,
            purpose,
            chan,
            transport,
            ChannelState::Open,
            None,
            now_ms,
        )
    }

    /// The responder's channel once HS2 has been written: `awaiting_confirm`.
    pub fn responder(purpose: Purpose, chan: ChannelId, transport: Transport, now_ms: u64) -> Self {
        let deadline = now_ms.saturating_add(purpose.confirm_timeout_ms());
        Self::new(
            Role::Responder,
            purpose,
            chan,
            transport,
            ChannelState::AwaitingConfirm,
            Some(deadline),
            now_ms,
        )
    }

    fn new(
        role: Role,
        purpose: Purpose,
        chan: ChannelId,
        transport: Transport,
        state: ChannelState,
        deadline_ms: Option<u64>,
        now_ms: u64,
    ) -> Self {
        Channel {
            role,
            purpose,
            chan,
            transport,
            state,
            send_next: 0,
            receiver: Receiver::new(),
            opened_at_ms: now_ms,
            deadline_ms,
            quiesced: false,
        }
    }

    /// This channel's id.
    pub fn id(&self) -> ChannelId {
        self.chan
    }

    /// Initiator or responder.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Companion or pairing.
    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    /// The current §4.2 state.
    pub fn state(&self) -> ChannelState {
        self.state
    }

    /// The peer's authenticated static key.
    pub fn remote_static(&self) -> PublicKey {
        self.transport.remote_static()
    }

    /// The handshake hash `h` (pairing confirmation, §6.2).
    pub fn handshake_hash(&self) -> &[u8; HASH_LEN] {
        self.transport.handshake_hash()
    }

    /// Whether keys are still held. False once closed: the zeroisation hook
    /// has run.
    pub fn has_keys(&self) -> bool {
        self.transport.has_keys()
    }

    /// §4.2: the responder may serve requests, push the catalog or rebind
    /// handles only when this is true. On an initiator it is true once open.
    pub fn may_serve(&self) -> bool {
        self.state == ChannelState::Open
    }

    /// The last counter processed from the peer (§4.6 "processed through").
    pub fn processed_through(&self) -> Option<u64> {
        self.receiver.prior()
    }

    /// The last counter sent on this channel (`None` if nothing was sent).
    pub fn last_sent_ctr(&self) -> Option<u64> {
        self.send_next.checked_sub(1)
    }

    /// Whether this side has quiesced the channel (§4.6 step 1).
    pub fn is_quiesced(&self) -> bool {
        self.quiesced
    }

    /// Seal one record as the next DATA frame.
    pub fn seal(&mut self, record: &[u8]) -> Result<Outbound, ChannelError> {
        match self.state {
            ChannelState::Closed(r) => return Err(ChannelError::Closed(r)),
            ChannelState::AwaitingConfirm | ChannelState::Confirming => {
                return Err(ChannelError::NotReady)
            }
            ChannelState::Open | ChannelState::WindingDown => {}
        }
        if self.quiesced {
            return Err(ChannelError::NotReady);
        }
        if self.send_next >= COUNTER_LIMIT {
            return Err(ChannelError::CounterExhausted);
        }
        if record.len() > crate::noise::MAX_TRANSPORT_PLAINTEXT {
            return Err(ChannelError::TooLarge);
        }
        let ctr = self.send_next;
        let ciphertext = self
            .transport
            .seal(ctr, record)
            .map_err(|_| ChannelError::TooLarge)?;
        self.send_next += 1;
        Ok(Outbound { ctr, ciphertext })
    }

    /// Receive one DATA frame with header counter `ctr` (§4.3). On a protocol
    /// violation the channel closes and zeroises before this returns, so no
    /// later frame on it is ever opened.
    pub fn receive(&mut self, ctr: u64, ciphertext: &[u8]) -> Result<Inbound, ChannelError> {
        match self.state {
            ChannelState::Closed(r) => return Err(ChannelError::Closed(r)),
            ChannelState::Confirming => return Err(ChannelError::NotReady),
            ChannelState::AwaitingConfirm | ChannelState::Open | ChannelState::WindingDown => {}
        }
        let transport = &self.transport;
        match self
            .receiver
            .receive(ctr, |n| transport.open(n, ciphertext))
        {
            Ok(Received::Duplicate) => Ok(Inbound::Duplicate),
            Ok(Received::Process(plaintext)) => {
                if self.state == ChannelState::AwaitingConfirm {
                    self.state = ChannelState::Confirming;
                    Ok(Inbound::Confirming(plaintext))
                } else {
                    Ok(Inbound::Record(plaintext))
                }
            }
            Err(violation) => {
                self.close(CloseReason::ProtocolError(violation));
                Err(ChannelError::Protocol(violation))
            }
        }
    }

    /// The confirming record was `hello` (or a matching `pair-confirm`): go to
    /// `open`. Only now may the responder serve.
    pub fn confirm(&mut self) -> Result<(), ChannelError> {
        match self.state {
            ChannelState::Confirming => {
                self.state = ChannelState::Open;
                self.deadline_ms = None;
                Ok(())
            }
            ChannelState::Closed(r) => Err(ChannelError::Closed(r)),
            _ => Err(ChannelError::NotReady),
        }
    }

    /// The confirming record was anything else: drop and zeroise (§4.2).
    pub fn reject_confirmation(&mut self) {
        if self.state == ChannelState::Confirming {
            self.close(CloseReason::NotConfirmed);
        }
    }

    /// §4.6 step 1 (phone): the phone has sent HS1 for a new channel, so it
    /// sends no more DATA here. Returns `last_sent_ctr` for the new channel's
    /// `hello {resume: {chan, last_sent_ctr}}`. Receiving continues (output
    /// tail and `bye{rekeyed}`).
    pub fn quiesce(&mut self) -> super::resume::HelloResume {
        self.quiesced = true;
        super::resume::HelloResume {
            chan: self.chan,
            last_sent_ctr: self.last_sent_ctr(),
        }
    }

    /// §4.6 step 3 (desktop): the new channel's `hello` says the phone sent
    /// nothing here after `last_sent_ctr`; a later frame is a protocol error.
    pub fn set_resume_ceiling(&mut self, last_sent_ctr: Option<u64>) {
        self.receiver.set_resume_ceiling(last_sent_ctr);
    }

    /// §4.6 step 5 (desktop): `bye{rekeyed}` has been (or is about to be)
    /// sent. No new requests from here on; owed replies may still be sealed
    /// until the wind-down deadline, then [`Channel::poll`] closes it.
    pub fn begin_wind_down(&mut self, now_ms: u64) {
        if matches!(self.state, ChannelState::Open) {
            self.state = ChannelState::WindingDown;
            self.deadline_ms = Some(now_ms.saturating_add(WIND_DOWN_MS));
        }
    }

    /// Advance timers. Returns the close reason if this call closed the
    /// channel (confirmation timeout, or the end of a wind-down).
    pub fn poll(&mut self, now_ms: u64) -> Option<CloseReason> {
        let deadline = self.deadline_ms?;
        if now_ms < deadline {
            return None;
        }
        let reason = match self.state {
            ChannelState::AwaitingConfirm | ChannelState::Confirming => CloseReason::ConfirmTimeout,
            ChannelState::WindingDown => CloseReason::Rekeyed,
            _ => return None,
        };
        self.close(reason);
        Some(reason)
    }

    /// §4.6: whether the phone should rekey now (2^20 DATA frames in either
    /// direction, or 60 minutes).
    pub fn rekey_due(&self, now_ms: u64) -> bool {
        self.state == ChannelState::Open
            && (self.send_next >= REKEY_AFTER_FRAMES
                || self.receiver.processed_count() >= REKEY_AFTER_FRAMES
                || now_ms.saturating_sub(self.opened_at_ms) >= REKEY_AFTER_MS)
    }

    /// Close now and zeroise the keys. Idempotent: the first reason sticks.
    pub fn close(&mut self, reason: CloseReason) {
        if !matches!(self.state, ChannelState::Closed(_)) {
            self.state = ChannelState::Closed(reason);
        }
        self.deadline_ms = None;
        self.transport.zeroize();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ids::Id;
    use crate::noise::kk::KkInitiator;

    pub(crate) fn prologue(chan: u8) -> KkPrologue {
        KkPrologue {
            account: Id([1; 16]),
            phone: Id([2; 16]),
            desktop: Id([3; 16]),
            chan: ChannelId([chan; 16]),
        }
    }

    pub(crate) struct Pair {
        pub phone: Channel,
        pub desktop: Channel,
    }

    /// A confirmed phone ↔ desktop channel pair.
    pub(crate) fn open_pair(chan: u8, now: u64) -> Pair {
        let mut p = unconfirmed_pair(chan, now);
        let hello = p.phone.seal(b"hello").unwrap();
        match p.desktop.receive(hello.ctr, &hello.ciphertext).unwrap() {
            Inbound::Confirming(r) => assert_eq!(&r[..], b"hello"),
            other => panic!("{other:?}"),
        }
        p.desktop.confirm().unwrap();
        p
    }

    pub(crate) fn unconfirmed_pair(chan: u8, now: u64) -> Pair {
        let phone_k = StaticKeypair::generate().unwrap();
        let desk_k = StaticKeypair::generate().unwrap();
        let pro = prologue(chan);
        let (init, hs1) =
            KkInitiator::start(&phone_k, &desk_k.public(), &pro, br#"{"v":1,"caps":[]}"#).unwrap();
        let Listened {
            channel: desktop,
            hs2,
            ..
        } = kk_listen(
            &desk_k,
            PinLookup::Pinned(phone_k.public()),
            &pro,
            &hs1,
            b"[]",
            now,
        )
        .unwrap();
        let (t, _) = init.read_hs2(&hs2).unwrap();
        Pair {
            phone: Channel::initiator(Purpose::Companion, pro.chan, t, now),
            desktop,
        }
    }

    #[test]
    fn responder_sends_and_serves_nothing_before_confirmation() {
        let mut p = unconfirmed_pair(1, 0);
        assert_eq!(p.desktop.state(), ChannelState::AwaitingConfirm);
        assert!(!p.desktop.may_serve());
        assert_eq!(p.desktop.seal(b"x").unwrap_err(), ChannelError::NotReady);
        let hello = p.phone.seal(b"hello").unwrap();
        assert!(matches!(
            p.desktop.receive(0, &hello.ciphertext),
            Ok(Inbound::Confirming(_))
        ));
        assert!(!p.desktop.may_serve());
        assert_eq!(p.desktop.seal(b"x").unwrap_err(), ChannelError::NotReady);
        // A further frame can't be taken before the decision.
        let next = p.phone.seal(b"req").unwrap();
        assert_eq!(
            p.desktop.receive(1, &next.ciphertext).unwrap_err(),
            ChannelError::NotReady
        );
        p.desktop.confirm().unwrap();
        assert!(p.desktop.may_serve());
        assert!(matches!(
            p.desktop.receive(1, &next.ciphertext),
            Ok(Inbound::Record(_))
        ));
        assert!(p.desktop.seal(b"reply").is_ok());
    }

    #[test]
    fn any_other_first_frame_drops_and_zeroises() {
        let mut p = unconfirmed_pair(1, 0);
        let f = p.phone.seal(b"rpc-request").unwrap();
        assert!(matches!(
            p.desktop.receive(0, &f.ciphertext),
            Ok(Inbound::Confirming(_))
        ));
        p.desktop.reject_confirmation();
        assert_eq!(
            p.desktop.state(),
            ChannelState::Closed(CloseReason::NotConfirmed)
        );
        assert!(!p.desktop.has_keys());
    }

    #[test]
    fn unconfirmed_channel_times_out_after_10s() {
        let mut p = unconfirmed_pair(1, 1_000);
        assert_eq!(p.desktop.poll(10_999), None);
        assert_eq!(p.desktop.poll(11_000), Some(CloseReason::ConfirmTimeout));
        assert!(!p.desktop.has_keys());
    }

    #[test]
    fn first_frame_failing_aead_is_protocol_error() {
        let mut p = unconfirmed_pair(1, 0);
        let mut f = p.phone.seal(b"hello").unwrap();
        f.ciphertext[0] ^= 1;
        assert!(matches!(
            p.desktop.receive(0, &f.ciphertext),
            Err(ChannelError::Protocol(
                ProtocolViolation::AuthenticationFailed { ctr: 0 }
            ))
        ));
        assert!(!p.desktop.has_keys());
    }

    #[test]
    fn unknown_or_tombstoned_peer_is_refused_without_state() {
        let phone_k = StaticKeypair::generate().unwrap();
        let desk_k = StaticKeypair::generate().unwrap();
        let other = StaticKeypair::generate().unwrap();
        let pro = prologue(1);
        let (_, hs1) = KkInitiator::start(&phone_k, &desk_k.public(), &pro, b"").unwrap();
        for pin in [
            PinLookup::Unpinned,
            PinLookup::Tombstoned,
            PinLookup::Pinned(other.public()),
        ] {
            assert_eq!(
                kk_listen(&desk_k, pin, &pro, &hs1, b"", 0).unwrap_err(),
                ListenRefusal::UnknownPeer
            );
        }
    }

    #[test]
    fn replayed_hs1_gets_no_service() {
        // §12.3: a replayed HS1 makes no state change and gets no service. The
        // replayer can't produce the confirming DATA, so the channel times out.
        let phone_k = StaticKeypair::generate().unwrap();
        let desk_k = StaticKeypair::generate().unwrap();
        let pro = prologue(1);
        let (_, hs1) = KkInitiator::start(&phone_k, &desk_k.public(), &pro, b"").unwrap();
        let mut replay = kk_listen(
            &desk_k,
            PinLookup::Pinned(phone_k.public()),
            &pro,
            &hs1,
            b"",
            0,
        )
        .unwrap()
        .channel;
        assert!(!replay.may_serve());
        // The relay's own guess at a DATA frame fails AEAD.
        assert!(matches!(
            replay.receive(0, &[0u8; 40]),
            Err(ChannelError::Protocol(_))
        ));
        let mut replay = kk_listen(
            &desk_k,
            PinLookup::Pinned(phone_k.public()),
            &pro,
            &hs1,
            b"",
            0,
        )
        .unwrap()
        .channel;
        assert_eq!(
            replay.poll(CONFIRM_TIMEOUT_MS),
            Some(CloseReason::ConfirmTimeout)
        );
    }

    #[test]
    fn data_sequence_0_1_3_closes_and_never_processes_3() {
        let mut p = open_pair(1, 0);
        // phone ctr 0 was hello; send 1, 2, 3 and let the relay drop 2.
        let f1 = p.phone.seal(b"k1").unwrap();
        let _f2 = p.phone.seal(b"k2").unwrap();
        let f3 = p.phone.seal(b"k3").unwrap();
        let mut processed = vec![];
        if let Inbound::Record(r) = p.desktop.receive(f1.ctr, &f1.ciphertext).unwrap() {
            processed.push(r.to_vec());
        }
        assert_eq!(
            p.desktop.receive(f3.ctr, &f3.ciphertext).unwrap_err(),
            ChannelError::Protocol(ProtocolViolation::Gap {
                expected: 2,
                got: 3
            })
        );
        assert_eq!(processed, vec![b"k1".to_vec()]);
        assert!(!p.desktop.has_keys(), "zeroised on protocol_error");
        // Nothing after the error is ever processed, not even the late frame 2.
        assert!(matches!(
            p.desktop.receive(2, &_f2.ciphertext),
            Err(ChannelError::Closed(_))
        ));
    }

    #[test]
    fn reorder_splice_and_bitflip_are_rejected() {
        // Reorder: 2 before 1.
        let mut p = open_pair(1, 0);
        let f1 = p.phone.seal(b"a").unwrap();
        let f2 = p.phone.seal(b"b").unwrap();
        assert!(matches!(
            p.desktop.receive(f2.ctr, &f2.ciphertext),
            Err(ChannelError::Protocol(_))
        ));
        let _ = f1;

        // Cross-channel splice: a frame from channel A replayed into B fails AEAD.
        let mut a = open_pair(1, 0);
        let mut b = open_pair(2, 0);
        let fa = a.phone.seal(b"from a").unwrap();
        assert!(matches!(
            b.desktop.receive(fa.ctr, &fa.ciphertext),
            Err(ChannelError::Protocol(
                ProtocolViolation::AuthenticationFailed { .. }
            ))
        ));

        // Bit flip.
        let mut c = open_pair(3, 0);
        let mut f = c.phone.seal(b"x").unwrap();
        let last = f.ciphertext.len() - 1;
        f.ciphertext[last] ^= 1;
        assert!(matches!(
            c.desktop.receive(f.ctr, &f.ciphertext),
            Err(ChannelError::Protocol(_))
        ));
    }

    #[test]
    fn replayed_data_is_dropped() {
        let mut p = open_pair(1, 0);
        let f1 = p.phone.seal(b"a").unwrap();
        assert!(matches!(
            p.desktop.receive(f1.ctr, &f1.ciphertext),
            Ok(Inbound::Record(_))
        ));
        assert_eq!(
            p.desktop.receive(f1.ctr, &f1.ciphertext).unwrap(),
            Inbound::Duplicate
        );
        assert_eq!(
            p.desktop.receive(0, &f1.ciphertext).unwrap(),
            Inbound::Duplicate
        );
        assert!(p.desktop.has_keys());
    }

    #[test]
    fn both_directions_and_counters() {
        let mut p = open_pair(1, 0);
        for i in 0..3u64 {
            let f = p.desktop.seal(b"out").unwrap();
            assert_eq!(f.ctr, i);
            assert!(matches!(
                p.phone.receive(f.ctr, &f.ciphertext),
                Ok(Inbound::Record(_))
            ));
        }
        assert_eq!(p.desktop.last_sent_ctr(), Some(2));
        assert_eq!(p.phone.processed_through(), Some(2));
    }

    #[test]
    fn rekey_due_after_60_minutes() {
        let p = open_pair(1, 5);
        assert!(!p.phone.rekey_due(5 + REKEY_AFTER_MS - 1));
        assert!(p.phone.rekey_due(5 + REKEY_AFTER_MS));
    }

    #[test]
    fn rekey_due_after_2_pow_20_frames() {
        let mut p = open_pair(1, 0);
        p.phone.send_next = REKEY_AFTER_FRAMES - 1;
        assert!(!p.phone.rekey_due(0));
        p.phone.seal(b"x").unwrap();
        assert!(p.phone.rekey_due(0));
    }

    #[test]
    fn send_counter_never_reaches_2_pow_32() {
        let mut p = open_pair(1, 0);
        p.phone.send_next = COUNTER_LIMIT - 1;
        assert_eq!(p.phone.seal(b"x").unwrap().ctr, COUNTER_LIMIT - 1);
        assert_eq!(
            p.phone.seal(b"x").unwrap_err(),
            ChannelError::CounterExhausted
        );
    }

    #[test]
    fn quiesced_channel_sends_nothing_but_still_receives() {
        let mut p = open_pair(1, 0);
        let _ = p.phone.seal(b"k").unwrap();
        let r = p.phone.quiesce();
        assert_eq!(r.chan, ChannelId([1; 16]));
        assert_eq!(r.last_sent_ctr, Some(1));
        assert_eq!(p.phone.seal(b"more").unwrap_err(), ChannelError::NotReady);
        let out = p.desktop.seal(b"tail").unwrap();
        assert!(matches!(
            p.phone.receive(out.ctr, &out.ciphertext),
            Ok(Inbound::Record(_))
        ));
    }

    #[test]
    fn wind_down_stops_new_requests_and_closes_after_10s() {
        let mut p = open_pair(1, 0);
        p.desktop.begin_wind_down(100);
        assert!(!p.desktop.may_serve());
        assert!(p.desktop.seal(b"owed reply").is_ok());
        assert_eq!(p.desktop.poll(100 + WIND_DOWN_MS - 1), None);
        assert_eq!(
            p.desktop.poll(100 + WIND_DOWN_MS),
            Some(CloseReason::Rekeyed)
        );
        assert!(!p.desktop.has_keys());
        assert!(matches!(
            p.desktop.seal(b"x"),
            Err(ChannelError::Closed(CloseReason::Rekeyed))
        ));
    }

    #[test]
    fn close_is_idempotent_and_zeroises() {
        let mut p = open_pair(1, 0);
        p.desktop.close(CloseReason::Revoked);
        p.desktop.close(CloseReason::Local);
        assert_eq!(
            p.desktop.state(),
            ChannelState::Closed(CloseReason::Revoked)
        );
        assert!(!p.desktop.has_keys());
    }
}
