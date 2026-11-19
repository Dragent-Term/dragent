//! The desktop end of the phone relay's encrypted channels
//! (docs/design/e2ee.md §4.1–§4.4, §6.2, §6.4, §6.6).
//!
//! [`E2eEndpoint`] is the responder for every channel a phone opens to this
//! desktop through the relay's host socket:
//!
//! - **KK** (`purpose = companion`, §4.2): HS1 is checked against the pin for
//!   the relay-written `peer`; HS2 goes back; **nothing is served** until the
//!   phone's first DATA authenticates and is CTRL `hello`. Then
//!   [`E2eEvent::Opened`] tells the companion host (E3b) it may serve.
//! - **IKpsk1** (`purpose = pairing`, §6.2): HS1 must prove the QR secret
//!   `P`; the desktop keeps one tentative record, pins nothing, and commits
//!   only on a matching `pair-confirm {h}`; three failures burn `P`.
//! - **DATA** (§4.3): counters, replay and gaps through
//!   [`dragent_e2ee::channel::Channel`]; any violation runs the
//!   **`protocol_error` hook** ([`E2eEndpoint::protocol_error`]): CLOSE
//!   `protocol_error`, zeroise the keys, discard partial messages, and report
//!   [`E2eEvent::Closed`] with [`ChannelEnd::Lost`] so the host fails pending
//!   requests and **discards every unwritten input** from that channel.
//! - **Records** (§4.4): padding buckets, fragmentation and reassembly.
//!
//! It is a plain state machine: bytes in ([`E2eEndpoint::on_frame`]), frames
//! and events out ([`Output`]), time passed in as a monotonic `now_ms`. The
//! caller owns the socket and the order in which frames are written. The
//! §4.6 resume itself (draining the old channel, `input_lost`) is the host's
//! (E3b), built on [`dragent_e2ee::channel::resume`]; this module gives it
//! the pieces: `hello`'s resume point, per-channel progress, the resume
//! ceiling, wind-down and the per-pair channel limit.
//!
//! The CTRL commands this desktop and the phone exchange are encoded by
//! [`dragent_e2ee::codec::ctrl::Ctrl`] (canonical `<name> <json>`, pinned by
//! `vectors/codec/ctrl.json`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use dragent_e2ee::channel::resume::{HelloResume, PairChannels};
use dragent_e2ee::channel::{
    kk_listen, Channel, ChannelError, ChannelState, CloseReason, Inbound, PinLookup,
    ProtocolViolation, Purpose as ChannelPurpose,
};
pub use dragent_e2ee::codec::ctrl::{Ctrl, CtrlError, MAX_DESKTOP_NAME};
use dragent_e2ee::codec::header::{self, CloseReason as WireClose, Head, Kind, Purpose};
use dragent_e2ee::codec::record::{
    self, MsgType, ReassembleError, Reassembler, RecordError, MAX_RECORD_CONTENT, MAX_TOTAL_LEN,
};
use dragent_e2ee::codec::{b64, hs};
use dragent_e2ee::noise::pairing::{PairingPsk, PairingResponder, PairingSecret};
use dragent_e2ee::noise::{ct_eq, KkPrologue, PairingPrologue, PublicKey};
use dragent_e2ee::zeroize::Zeroizing;
use dragent_e2ee::{ChannelId, Id, RandomError};

use super::e2e_store::{wall_ms, E2eDesktop, StoreFailure};

/// `P` is held in memory for at most this long (§6.2 step 2).
pub const PAIRING_SECRET_TTL_MS: u64 = 10 * 60 * 1000;
/// Three failed tentatives burn `P` (§6.2 step 9).
pub const MAX_PAIRING_FAILURES: u8 = 3;
/// Unconfirmed KK channels kept per phone; an older one is dropped first.
/// Confirmed channels are limited separately (§4.6: 2 per pair).
pub const MAX_PENDING_PER_PHONE: usize = 2;
/// Channel ids remembered after use, so a replayed HS1 changes nothing.
const SEEN_CHANS_CAP: usize = 4096;

/// Cut `name` to at most [`MAX_DESKTOP_NAME`] bytes on a char boundary
/// (`pair-done` requires 1..=128 bytes).
fn clamp_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "Dragent desktop".to_string();
    }
    let mut end = name.len().min(MAX_DESKTOP_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

// ---------------------------------------------------------------------------
// Frames and records
// ---------------------------------------------------------------------------

fn encode_frame(head: Head<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    head.encode(&mut out)
        .expect("frames built here always pass the header check");
    out
}

fn hs2_frame(purpose: Purpose, chan: ChannelId, body: &[u8]) -> Vec<u8> {
    encode_frame(Head {
        kind: Kind::Hs,
        step: 2,
        purpose: Some(purpose),
        chan,
        peer: None,
        ctr: 0,
        body,
    })
}

fn close_frame(chan: ChannelId, reason: WireClose) -> Vec<u8> {
    encode_frame(Head {
        kind: Kind::Close,
        step: 0,
        purpose: None,
        chan,
        peer: None,
        ctr: 0,
        body: &[reason.byte()],
    })
}

fn data_frame(chan: ChannelId, ctr: u64, body: &[u8]) -> Vec<u8> {
    encode_frame(Head {
        kind: Kind::Data,
        step: 0,
        purpose: None,
        chan,
        peer: None,
        ctr,
        body,
    })
}

/// Sending failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// A CTRL command outside its limits (nothing was sent).
    BadCtrl(CtrlError),
    /// No such channel (closed, or never opened).
    UnknownChannel,
    /// Not allowed now: before confirmation (§4.2), or after the channel
    /// closed.
    Channel(ChannelError),
    /// Over the 1 MiB message cap: use the §4.5 part protocol.
    TooLarge,
    /// The per-direction message id is exhausted (the channel must be
    /// replaced long before this).
    MessageIdExhausted,
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::BadCtrl(e) => write!(f, "{e}"),
            SendError::UnknownChannel => f.write_str("no such channel"),
            SendError::Channel(e) => write!(f, "{e}"),
            SendError::TooLarge => f.write_str("message over the 1 MiB cap"),
            SendError::MessageIdExhausted => f.write_str("message ids exhausted"),
        }
    }
}

impl std::error::Error for SendError {}

/// Split one message into §4.4 records (padded to the smallest bucket), seal
/// each as the channel's next DATA frame, and return the frames in order.
///
/// `next_msg_id` is the sender's per-direction message counter (starts at 1).
/// Used by the endpoint and by the scripted phone in tests, so both ends
/// frame identically.
pub fn seal_message(
    channel: &mut Channel,
    next_msg_id: &mut u32,
    mtype: MsgType,
    content: &[u8],
) -> Result<Vec<Vec<u8>>, SendError> {
    match channel.state() {
        ChannelState::Open | ChannelState::WindingDown => {}
        ChannelState::Closed(r) => return Err(SendError::Channel(ChannelError::Closed(r))),
        _ => return Err(SendError::Channel(ChannelError::NotReady)),
    }
    if channel.is_quiesced() {
        return Err(SendError::Channel(ChannelError::NotReady));
    }
    if content.len() > MAX_TOTAL_LEN {
        return Err(SendError::TooLarge);
    }
    let msg_id = *next_msg_id;
    if msg_id == 0 {
        return Err(SendError::MessageIdExhausted);
    }
    let chunks: Vec<&[u8]> = if content.is_empty() {
        vec![&[][..]]
    } else {
        content.chunks(MAX_RECORD_CONTENT).collect()
    };
    let n = chunks.len();
    let mut frames = Vec::with_capacity(n);
    for (i, chunk) in chunks.into_iter().enumerate() {
        let first = i == 0;
        let last = i + 1 == n;
        let len = record::smallest_len(chunk.len()).expect("chunks fit one record");
        let mut rec = Zeroizing::new(Vec::with_capacity(len));
        record::encode(
            &mut rec,
            len,
            first,
            last,
            false,
            first.then_some(mtype),
            msg_id,
            if first { content.len() as u32 } else { 0 },
            chunk,
        )
        .expect("records built here are valid");
        let out = channel.seal(&rec).map_err(SendError::Channel)?;
        frames.push(data_frame(channel.id(), out.ctr, &out.ciphertext));
    }
    *next_msg_id = msg_id.wrapping_add(1);
    Ok(frames)
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Why a channel was declared lost (§4.3 `protocol_error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolFault {
    /// Counter, AEAD, gap, or resume-ceiling violation (§4.3, §4.6 step 3).
    Counter(ProtocolViolation),
    /// A record failed its checks (§4.4).
    Record(RecordError),
    /// Reassembly limits or ordering (§4.4).
    Reassembly(ReassembleError),
    /// A message id didn't increase.
    MessageOrder,
    /// A CTRL command was malformed.
    Ctrl(CtrlError),
    /// A CTRL command that has no place here (a second `hello`, a
    /// `pair-confirm` on a live channel).
    UnexpectedCtrl(&'static str),
    /// The first DATA was not the confirming record (§4.2).
    NotConfirmed,
    /// Reported by the companion host (E3b): a malformed envelope, a part
    /// protocol violation, an inconsistent resume.
    Host(&'static str),
}

impl fmt::Display for ProtocolFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolFault::Counter(v) => write!(f, "{v}"),
            ProtocolFault::Record(e) => write!(f, "record: {e}"),
            ProtocolFault::Reassembly(e) => write!(f, "reassembly: {e:?}"),
            ProtocolFault::MessageOrder => f.write_str("message id did not increase"),
            ProtocolFault::Ctrl(e) => write!(f, "{e}"),
            ProtocolFault::UnexpectedCtrl(name) => write!(f, "unexpected CTRL {name}"),
            ProtocolFault::NotConfirmed => f.write_str("first record was not a confirmation"),
            ProtocolFault::Host(why) => f.write_str(why),
        }
    }
}

/// How a confirmed channel ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelEnd {
    /// §4.3 `protocol_error`: CLOSE sent, keys zeroised, partial messages
    /// discarded. Fail pending requests, **discard all unwritten input** and
    /// report "lost" ("Connection to `<desktop>` was interrupted").
    Lost(ProtocolFault),
    /// §4.6 step 5: the wind-down after `bye{rekeyed}` finished.
    Rekeyed,
    /// §4.6 step 5: a third channel for the pair evicted this one.
    Evicted,
    /// §6.4: the phone was removed. Discard unwritten input; release handles.
    Revoked,
    /// The relay or phone closed it (CLOSE frame, `relay-chan-closed`, or the
    /// host socket dropped). Unauthenticated: only ever a reconnect.
    PeerClosed,
    /// Closed by the host (lease ended, shutdown).
    Local,
}

impl ChannelEnd {
    /// Whether input from this channel not yet written to a PTY must be
    /// discarded now (§4.3 step 5, §6.4 step 2).
    pub fn discards_unwritten_input(&self) -> bool {
        matches!(self, ChannelEnd::Lost(_) | ChannelEnd::Revoked)
    }
}

/// Something the host must act on.
#[derive(Debug)]
pub enum E2eEvent {
    /// A KK channel is confirmed (§4.2 `open`): serve it from now on.
    Opened {
        /// The channel.
        chan: ChannelId,
        /// The phone.
        phone: Id,
        /// `hello`'s §4.6 resume point. `None`: a fresh session, rebind
        /// nothing. Check that `resume.chan` belongs to the same phone
        /// ([`E2eEndpoint::channel_phone`]) before using it.
        resume: Option<HelloResume>,
        /// The phone's capabilities from HS1.
        caps: Vec<String>,
    },
    /// A complete message on an open (or winding-down) channel.
    Message {
        /// The channel.
        chan: ChannelId,
        /// The phone.
        phone: Id,
        /// The counter of the frame that completed the message. Progress
        /// through any frame is [`E2eEndpoint::processed_through`].
        ctr: u64,
        /// JSON, TERM or CTRL. CTRL here is never `hello`, `pair-confirm`,
        /// `pair-done` or `identity-rotate` (the endpoint handles those).
        mtype: MsgType,
        /// The content.
        content: Zeroizing<Vec<u8>>,
    },
    /// A confirmed channel ended. Its keys are already zeroised.
    Closed {
        /// The channel.
        chan: ChannelId,
        /// The phone.
        phone: Id,
        /// Why.
        end: ChannelEnd,
        /// The last counter processed from the phone (for the §4.6 resume
        /// ledger).
        processed_through: Option<u64>,
    },
    /// §6.2 step 8: a phone is pinned and `pair-done` was sent. Seal the
    /// catalog for it at exactly `catalog_version` (E3c).
    Paired {
        /// The new phone.
        phone: Id,
        /// Its name from the encrypted pairing payload.
        name: String,
        /// The reserved catalog version.
        catalog_version: u64,
    },
    /// A pairing attempt failed (bad confirmation, timeout, closed channel).
    PairingFailed {
        /// `P` was burned after three failures; a new QR is needed.
        burned: bool,
    },
    /// `P` expired (10 minutes) or was cancelled.
    PairingExpired,
    /// A phone announced a new identity (and maybe push) key (§6.6).
    IdentityRotated {
        /// The phone.
        phone: Id,
    },
    /// Persisting a pin-store change failed (the change still holds in
    /// memory where that is the fail-closed choice: revocation).
    StoreWriteFailed(String),
}

/// What [`E2eEndpoint`] asks the caller to do, in order.
#[derive(Debug)]
pub enum Output {
    /// Write this binary frame to the host socket.
    Send(Vec<u8>),
    /// Act on this event.
    Event(E2eEvent),
}

// ---------------------------------------------------------------------------
// Endpoint
// ---------------------------------------------------------------------------

/// Endpoint configuration.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// The signed-in account (every §5 prologue binds it).
    pub account: Id,
    /// This desktop's name for `pair-done` (cut to 128 bytes).
    pub desktop_name: String,
    /// Capabilities sent in HS2.
    pub caps: Vec<String>,
}

/// What the pairing QR / link carries besides the code and gateway (§6.2
/// step 3): `d`, `k` and `p`.
pub struct PairingQr {
    /// `d`: this desktop's id.
    pub desktop: Id,
    /// `k`: the identity public key.
    pub key: PublicKey,
    /// `p`: the 32-byte secret. Never persisted, never logged.
    pub secret: Zeroizing<[u8; 32]>,
}

impl PairingQr {
    /// `k` as base64url without padding.
    pub fn key_b64(&self) -> String {
        b64::encode(self.key.as_bytes())
    }

    /// `p` as base64url without padding.
    pub fn secret_b64(&self) -> Zeroizing<String> {
        Zeroizing::new(b64::encode(&self.secret[..]))
    }
}

impl fmt::Debug for PairingQr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingQr")
            .field("desktop", &self.desktop)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

struct PairingOffer {
    psk: PairingPsk,
    expires_ms: u64,
    expected_phone: Option<Id>,
    failures: u8,
    tentative: Option<ChannelId>,
}

struct Tentative {
    identity: PublicKey,
    push: PublicKey,
    name: String,
}

struct Entry {
    phone: Id,
    channel: Channel,
    rx: Reassembler,
    last_first_msg_id: u32,
    next_msg_id: u32,
    /// The phone key this handshake authenticated.
    key: PublicKey,
    caps: Vec<String>,
    /// Confirmed and reported with [`E2eEvent::Opened`].
    opened: bool,
    /// Pairing channels: the tentative record.
    pairing: Option<Tentative>,
    seq: u64,
}

#[derive(Default)]
struct SeenChans {
    order: VecDeque<ChannelId>,
    set: HashSet<ChannelId>,
}

impl SeenChans {
    fn contains(&self, c: &ChannelId) -> bool {
        self.set.contains(c)
    }

    fn insert(&mut self, c: ChannelId) {
        if self.set.insert(c) {
            if self.order.len() == SEEN_CHANS_CAP {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
            self.order.push_back(c);
        }
    }
}

/// The desktop's responder for every encrypted channel on the host socket.
pub struct E2eEndpoint {
    desktop: Arc<E2eDesktop>,
    account: Id,
    desktop_name: String,
    caps: Vec<u8>,
    channels: HashMap<ChannelId, Entry>,
    pairs: HashMap<Id, PairChannels>,
    seen: SeenChans,
    pairing: Option<PairingOffer>,
    next_seq: u64,
    wall: fn() -> u64,
    #[cfg(test)]
    zeroised: Vec<(ChannelId, bool)>,
}

impl fmt::Debug for E2eEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("E2eEndpoint")
            .field("channels", &self.channels.len())
            .field("pairing", &self.pairing.is_some())
            .finish_non_exhaustive()
    }
}

impl E2eEndpoint {
    /// A new endpoint (one per host socket session; channels don't outlive
    /// the socket they were opened on).
    pub fn new(desktop: Arc<E2eDesktop>, config: EndpointConfig) -> Result<Self, hs::HsError> {
        let caps = hs::encode_hello(&hs::Hello { caps: config.caps })?;
        Ok(E2eEndpoint {
            desktop,
            account: config.account,
            desktop_name: clamp_name(&config.desktop_name),
            caps,
            channels: HashMap::new(),
            pairs: HashMap::new(),
            seen: SeenChans::default(),
            pairing: None,
            next_seq: 0,
            wall: wall_ms,
            #[cfg(test)]
            zeroised: Vec::new(),
        })
    }

    /// The shared desktop state (identity key, pin store).
    pub fn desktop(&self) -> &Arc<E2eDesktop> {
        &self.desktop
    }

    // -- pairing ------------------------------------------------------------

    /// §6.2 step 2: generate `P` for a new pairing code and return what the
    /// QR carries. Replaces (and discards) any earlier offer.
    pub fn begin_pairing(&mut self, now_ms: u64) -> Result<(PairingQr, Vec<Output>), RandomError> {
        let mut out = self.end_pairing(false);
        let secret = PairingSecret::generate()?;
        let desktop = self.desktop.device();
        self.pairing = Some(PairingOffer {
            psk: secret.psk(&desktop),
            expires_ms: now_ms.saturating_add(PAIRING_SECRET_TTL_MS),
            expected_phone: None,
            failures: 0,
            tentative: None,
        });
        let qr = PairingQr {
            desktop,
            key: self.desktop.public(),
            secret: Zeroizing::new(*secret.as_bytes()),
        };
        // `secret` drops here and wipes itself; only the PSK stays.
        out.retain(|o| !matches!(o, Output::Event(E2eEvent::PairingExpired)));
        Ok((qr, out))
    }

    /// §6.2 step 10: the phone that redeemed the code (learned from the
    /// server). A pairing HS1 whose relay-written `peer` differs is refused.
    /// Routing consistency only: `P` is what authenticates.
    pub fn set_pairing_peer(&mut self, phone: Id) {
        if let Some(offer) = &mut self.pairing {
            offer.expected_phone = Some(phone);
        }
    }

    /// Whether a pairing secret is live.
    pub fn pairing_active(&self) -> bool {
        self.pairing.is_some()
    }

    /// Discard `P` and any tentative record (the code was cancelled).
    pub fn cancel_pairing(&mut self) -> Vec<Output> {
        self.end_pairing(true)
    }

    fn end_pairing(&mut self, report: bool) -> Vec<Output> {
        let Some(offer) = self.pairing.take() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(chan) = offer.tentative {
            self.drop_entry(chan);
        }
        if report {
            out.push(Output::Event(E2eEvent::PairingExpired));
        }
        out
    }

    /// A tentative failed (§6.2 step 9).
    fn pairing_failure(&mut self) -> Vec<Output> {
        let Some(offer) = &mut self.pairing else {
            return Vec::new();
        };
        offer.tentative = None;
        offer.failures = offer.failures.saturating_add(1);
        let burned = offer.failures >= MAX_PAIRING_FAILURES;
        if burned {
            self.pairing = None;
            log::warn!("[e2ee] pairing secret burned after repeated failures");
        }
        vec![Output::Event(E2eEvent::PairingFailed { burned })]
    }

    // -- frames in ----------------------------------------------------------

    /// One binary frame from the host socket. Frames that fail the §4.1
    /// header check, or name no channel of ours, are ignored.
    pub fn on_frame(&mut self, bytes: &[u8], now_ms: u64) -> Vec<Output> {
        let Ok((frame, _)) = header::decode(bytes) else {
            return Vec::new();
        };
        match frame.kind {
            Kind::Hs => match (frame.step, frame.purpose, frame.peer) {
                (1, Some(Purpose::Companion), Some(phone)) => {
                    self.on_kk_hs1(frame.chan, phone, frame.body, now_ms)
                }
                (1, Some(Purpose::Pairing), Some(phone)) => {
                    self.on_pairing_hs1(frame.chan, phone, frame.body, now_ms)
                }
                // The desktop is always the responder: it never receives HS2.
                _ => Vec::new(),
            },
            Kind::Data => self.on_data(frame.chan, frame.ctr, frame.body, now_ms),
            // An unauthenticated hint: it can only cause a reconnect.
            Kind::Close => self.finish(frame.chan, ChannelEnd::PeerClosed),
        }
    }

    /// The relay's `relay-chan-closed {chan}` text frame.
    pub fn on_relay_chan_closed(&mut self, chan: ChannelId) -> Vec<Output> {
        self.finish(chan, ChannelEnd::PeerClosed)
    }

    /// The host socket dropped: every channel on it is gone.
    pub fn on_socket_lost(&mut self) -> Vec<Output> {
        let chans: Vec<ChannelId> = self.channels.keys().copied().collect();
        chans
            .into_iter()
            .flat_map(|c| self.finish(c, ChannelEnd::PeerClosed))
            .collect()
    }

    fn on_kk_hs1(&mut self, chan: ChannelId, phone: Id, body: &[u8], now_ms: u64) -> Vec<Output> {
        log::info!("[e2ee] phone {phone} opened a channel (KK handshake received)");
        // A replayed HS1 (same chan: the chan is in the prologue) changes
        // nothing and gets no answer.
        if self.channels.contains_key(&chan) || self.seen.contains(&chan) {
            return Vec::new();
        }
        let refuse = || vec![Output::Send(close_frame(chan, WireClose::UnknownPeer))];
        let keys = self.desktop.store().candidate_keys(&phone, (self.wall)());
        if keys.is_empty() {
            // Routing ids only (never keys or content).
            let store = self.desktop.store();
            let paired: Vec<String> = store.paired_phones().iter().map(|p| p.id.to_string()).collect();
            log::warn!(
                "[e2ee] refused a phone channel: no pinned key for phone {} (removed: {}; paired phones: {:?})",
                phone,
                store.is_tombstoned(&phone),
                paired
            );
            return refuse();
        }
        let prologue = KkPrologue {
            account: self.account,
            phone,
            desktop: self.desktop.device(),
            chan,
        };
        let identity = self.desktop.identity();
        let Some((listened, key)) = keys.into_iter().find_map(|k| {
            kk_listen(
                &identity,
                PinLookup::Pinned(k),
                &prologue,
                body,
                &self.caps,
                now_ms,
            )
            .ok()
            .map(|l| (l, k))
        }) else {
            log::warn!(
                "[e2ee] refused a phone channel: the handshake from phone {} didn't verify against its pinned key (account {}, desktop {})",
                phone,
                self.account,
                self.desktop.device()
            );
            return refuse();
        };
        let Ok(hello) = hs::decode_hello(&listened.hs1_payload) else {
            return vec![Output::Send(close_frame(chan, WireClose::ProtocolError))];
        };
        self.seen.insert(chan);

        // Bound unconfirmed channels per phone (replays or a stuck phone).
        let mut pending: Vec<(u64, ChannelId)> = self
            .channels
            .iter()
            .filter(|(_, e)| e.phone == phone && !e.opened && e.pairing.is_none())
            .map(|(c, e)| (e.seq, *c))
            .collect();
        pending.sort();
        while pending.len() >= MAX_PENDING_PER_PHONE {
            let (_, old) = pending.remove(0);
            self.drop_entry(old);
        }

        self.insert(Entry {
            phone,
            channel: listened.channel,
            rx: Reassembler::new(),
            last_first_msg_id: 0,
            next_msg_id: 1,
            key,
            caps: hello.caps,
            opened: false,
            pairing: None,
            seq: 0,
        });
        vec![Output::Send(hs2_frame(
            Purpose::Companion,
            chan,
            &listened.hs2,
        ))]
    }

    fn on_pairing_hs1(
        &mut self,
        chan: ChannelId,
        phone: Id,
        body: &[u8],
        now_ms: u64,
    ) -> Vec<Output> {
        log::info!("[e2ee] phone {phone} started pairing (pairing handshake received)");
        if self.channels.contains_key(&chan) || self.seen.contains(&chan) {
            return Vec::new();
        }
        let refuse = vec![Output::Send(close_frame(chan, WireClose::UnknownPeer))];
        let Some(offer) = &self.pairing else {
            log::warn!("[e2ee] pairing refused: no pairing code is open on this desktop");
            return refuse;
        };
        if now_ms >= offer.expires_ms {
            log::warn!("[e2ee] pairing refused: the pairing code expired");
            let mut out = self.end_pairing(true);
            out.extend(refuse);
            return out;
        }
        if offer.expected_phone.is_some_and(|p| p != phone) {
            log::warn!(
                "[e2ee] pairing refused: phone {phone} isn't the one that redeemed the code ({:?})",
                offer.expected_phone
            );
            return refuse;
        }
        if offer.tentative.is_some() {
            // A second tentative for this `P`.
            log::warn!("[e2ee] pairing refused: another phone's pairing is already in progress");
            return refuse;
        }
        let prologue = PairingPrologue {
            account: self.account,
            phone,
            desktop: self.desktop.device(),
            chan,
        };
        let identity = self.desktop.identity();
        // Failing here means no knowledge of `P` (or the wrong desktop key):
        // nothing is kept and nothing counts against `P`.
        let Ok((responder, phone_key, payload)) =
            PairingResponder::read_hs1(&identity, &offer.psk, &prologue, body)
        else {
            log::warn!("[e2ee] pairing refused: the handshake didn't match this code's secret or this desktop's key");
            return refuse;
        };
        self.seen.insert(chan);
        let Ok(hello) = hs::decode_pairing(&payload).inspect_err(|e| {
            log::warn!("[e2ee] pairing refused: the phone's details were malformed ({e:?})");
        }) else {
            // Proved `P` but sent a malformed payload: a failed attempt.
            let mut out = vec![Output::Send(close_frame(chan, WireClose::ProtocolError))];
            out.extend(self.pairing_failure());
            return out;
        };
        let Ok((transport, hs2)) = responder.write_hs2() else {
            return refuse;
        };
        if let Some(offer) = &mut self.pairing {
            offer.tentative = Some(chan);
        }
        self.insert(Entry {
            phone,
            channel: Channel::responder(ChannelPurpose::Pairing, chan, transport, now_ms),
            rx: Reassembler::new(),
            last_first_msg_id: 0,
            next_msg_id: 1,
            key: phone_key,
            caps: Vec::new(),
            opened: false,
            pairing: Some(Tentative {
                identity: phone_key,
                push: PublicKey(hello.push_pub),
                name: hello.device_name,
            }),
            seq: 0,
        });
        vec![Output::Send(hs2_frame(Purpose::Pairing, chan, &hs2))]
    }

    fn insert(&mut self, mut entry: Entry) {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        self.channels.insert(entry.channel.id(), entry);
    }

    fn on_data(&mut self, chan: ChannelId, ctr: u64, body: &[u8], now_ms: u64) -> Vec<Output> {
        let Some(entry) = self.channels.get_mut(&chan) else {
            return Vec::new();
        };
        match entry.channel.receive(ctr, body) {
            Ok(Inbound::Duplicate) => Vec::new(),
            Ok(Inbound::Confirming(pt)) => self.on_confirming(chan, pt, now_ms),
            Ok(Inbound::Record(pt)) => self.on_record(chan, ctr, pt),
            Err(ChannelError::Protocol(v)) => self.protocol_error(chan, ProtocolFault::Counter(v)),
            Err(_) => Vec::new(),
        }
    }

    /// Decode one record and feed the reassembler.
    fn decode_record(
        entry: &mut Entry,
        pt: &[u8],
    ) -> Result<Option<record::Message>, ProtocolFault> {
        let rec = record::decode(pt).map_err(ProtocolFault::Record)?;
        if rec.first && !rec.pad_only {
            if rec.msg_id <= entry.last_first_msg_id {
                return Err(ProtocolFault::MessageOrder);
            }
            entry.last_first_msg_id = rec.msg_id;
        }
        entry.rx.feed(&rec).map_err(ProtocolFault::Reassembly)
    }

    fn on_confirming(
        &mut self,
        chan: ChannelId,
        pt: Zeroizing<Vec<u8>>,
        _now_ms: u64,
    ) -> Vec<Output> {
        let Some(entry) = self.channels.get_mut(&chan) else {
            return Vec::new();
        };
        let ctrl = match Self::decode_record(entry, &pt) {
            Ok(Some(msg)) if msg.mtype == MsgType::Ctrl => {
                let c = Ctrl::decode(&msg.content);
                drop(Zeroizing::new(msg.content));
                c
            }
            Ok(_) => Err(CtrlError::BadFraming),
            Err(fault) => {
                entry.channel.reject_confirmation();
                return self.protocol_error(chan, fault);
            }
        };
        match (entry.channel.purpose(), ctrl) {
            (ChannelPurpose::Companion, Ok(Some(Ctrl::Hello { resume }))) => {
                self.open_companion(chan, resume)
            }
            (ChannelPurpose::Pairing, Ok(Some(Ctrl::PairConfirm { h })))
                if ct_eq(entry.channel.handshake_hash(), &h) =>
            {
                log::info!("[e2ee] phone confirmed pairing; pinning it and sending pair-done");
                self.commit_pairing(chan)
            }
            (purpose, ctrl) => {
                if purpose == ChannelPurpose::Pairing {
                    log::warn!("[e2ee] pairing refused: the phone's confirmation didn't match ({:?})", ctrl.map(|c| c.map(|c| c.name())));
                }
                entry.channel.reject_confirmation();
                self.protocol_error(chan, ProtocolFault::NotConfirmed)
            }
        }
    }

    fn open_companion(&mut self, chan: ChannelId, resume: Option<HelloResume>) -> Vec<Output> {
        let Some(entry) = self.channels.get_mut(&chan) else {
            return Vec::new();
        };
        if entry.channel.confirm().is_err() {
            return Vec::new();
        }
        entry.opened = true;
        let phone = entry.phone;
        let key = entry.key;
        let caps = std::mem::take(&mut entry.caps);
        let mut out = Vec::new();
        // §6.6 step 2: a confirmed handshake with an announced key re-pins it.
        if let Err(e) = self.desktop.store().confirm_key(&phone, &key) {
            out.push(Output::Event(E2eEvent::StoreWriteFailed(e.to_string())));
        }
        if let Some(evicted) = self.pairs.entry(phone).or_default().admit(chan) {
            out.extend(self.finish(evicted, ChannelEnd::Evicted));
        }
        out.push(Output::Event(E2eEvent::Opened {
            chan,
            phone,
            resume,
            caps,
        }));
        out
    }

    fn commit_pairing(&mut self, chan: ChannelId) -> Vec<Output> {
        let Some(entry) = self.channels.get_mut(&chan) else {
            return Vec::new();
        };
        if entry.channel.confirm().is_err() {
            return Vec::new();
        }
        let phone = entry.phone;
        let Some(t) = entry.pairing.take() else {
            return Vec::new();
        };
        let result = self.desktop.store().pin_phone(
            phone,
            t.identity,
            t.push,
            t.name.clone(),
            (self.wall)(),
        );
        let catalog_version = match result {
            Ok(v) => v,
            Err(e) => {
                // Nothing was pinned; `P` is kept for another attempt.
                self.drop_entry(chan);
                let mut out = vec![Output::Event(E2eEvent::StoreWriteFailed(e.to_string()))];
                out.extend(self.pairing_failure());
                return out;
            }
        };
        // `P` is consumed: the PSK is dropped (and wiped) with the offer.
        self.pairing = None;
        let mut out = Vec::new();
        let done = Ctrl::PairDone {
            desktop_name: self.desktop_name.clone(),
            catalog_version,
        };
        if let (Some(entry), Ok(content)) = (self.channels.get_mut(&chan), done.encode()) {
            if let Ok(frames) = seal_message(
                &mut entry.channel,
                &mut entry.next_msg_id,
                MsgType::Ctrl,
                &content,
            ) {
                out.extend(frames.into_iter().map(Output::Send));
            }
        }
        // The pairing channel has done its job: zeroise it. The phone closes
        // its end after `pair-done` and opens a KK channel.
        self.drop_entry(chan);
        // A re-paired phone with a new key: channels under its old key end.
        let stale: Vec<ChannelId> = self
            .channels
            .iter()
            .filter(|(_, e)| e.phone == phone && !e.key.ct_eq(&t.identity))
            .map(|(c, _)| *c)
            .collect();
        for c in stale {
            out.extend(self.finish(c, ChannelEnd::Revoked));
        }
        out.push(Output::Event(E2eEvent::Paired {
            phone,
            name: t.name,
            catalog_version,
        }));
        out
    }

    fn on_record(&mut self, chan: ChannelId, ctr: u64, pt: Zeroizing<Vec<u8>>) -> Vec<Output> {
        let Some(entry) = self.channels.get_mut(&chan) else {
            return Vec::new();
        };
        let phone = entry.phone;
        let msg = match Self::decode_record(entry, &pt) {
            Ok(Some(m)) => m,
            Ok(None) => return Vec::new(),
            Err(fault) => return self.protocol_error(chan, fault),
        };
        let mtype = msg.mtype;
        let content = Zeroizing::new(msg.content);
        if mtype == MsgType::Ctrl {
            match Ctrl::decode(&content) {
                Err(e) => return self.protocol_error(chan, ProtocolFault::Ctrl(e)),
                Ok(Some(
                    c @ (Ctrl::Hello { .. } | Ctrl::PairConfirm { .. } | Ctrl::PairDone { .. }),
                )) => {
                    return self.protocol_error(chan, ProtocolFault::UnexpectedCtrl(c.name()));
                }
                Ok(Some(Ctrl::IdentityRotate {
                    new_pub,
                    new_push_pub,
                })) => {
                    let r = self.desktop.store().record_rotation(
                        &phone,
                        new_pub,
                        new_push_pub,
                        (self.wall)(),
                    );
                    return vec![Output::Event(match r {
                        Ok(()) => E2eEvent::IdentityRotated { phone },
                        Err(e) => E2eEvent::StoreWriteFailed(e.to_string()),
                    })];
                }
                Ok(_) => {}
            }
        }
        vec![Output::Event(E2eEvent::Message {
            chan,
            phone,
            ctr,
            mtype,
            content,
        })]
    }

    // -- the protocol_error hook --------------------------------------------

    /// §4.3 `protocol_error` for `chan`. Runs, in order:
    ///
    /// 1. CLOSE `protocol_error` (the returned frame);
    /// 2. zeroise the channel's keys;
    /// 3. discard every partial message;
    /// 4. and 5. report [`E2eEvent::Closed`] with [`ChannelEnd::Lost`], on
    ///    which the host fails pending requests and **discards all input from
    ///    the channel not yet written to a PTY**.
    ///
    /// The endpoint calls this itself for counter, AEAD, record, reassembly
    /// and CTRL violations. The companion host (E3b) calls it for violations
    /// only it can see: a malformed envelope, a part-protocol error, an
    /// inconsistent resume. A tentative pairing channel counts as a failed
    /// attempt. Idempotent: an unknown or already-closed channel returns
    /// nothing.
    pub fn protocol_error(&mut self, chan: ChannelId, fault: ProtocolFault) -> Vec<Output> {
        self.finish(chan, ChannelEnd::Lost(fault))
    }

    /// Close a channel locally (lease ended, shutdown). Not for protocol
    /// errors: use [`Self::protocol_error`].
    pub fn close(&mut self, chan: ChannelId) -> Vec<Output> {
        self.finish(chan, ChannelEnd::Local)
    }

    /// Remove a channel without reporting it (unconfirmed or pairing
    /// channels), zeroising its keys.
    fn drop_entry(&mut self, chan: ChannelId) {
        if let Some(mut e) = self.channels.remove(&chan) {
            e.channel.close(CloseReason::Local);
            self.note_zeroised(chan, &e);
            if e.opened {
                if let Some(p) = self.pairs.get_mut(&e.phone) {
                    p.remove(&chan);
                }
            }
            if self.pairing.as_ref().and_then(|o| o.tentative) == Some(chan) {
                if let Some(o) = &mut self.pairing {
                    o.tentative = None;
                }
            }
        }
    }

    fn note_zeroised(&mut self, _chan: ChannelId, e: &Entry) {
        debug_assert!(!e.channel.has_keys(), "closed channel kept its keys");
        #[cfg(test)]
        self.zeroised.push((_chan, e.channel.has_keys()));
    }

    /// End a channel: zeroise, discard partials, CLOSE on `Lost`, report.
    fn finish(&mut self, chan: ChannelId, end: ChannelEnd) -> Vec<Output> {
        let Some(mut e) = self.channels.remove(&chan) else {
            return Vec::new();
        };
        let reason = match &end {
            ChannelEnd::Lost(ProtocolFault::Counter(v)) => CloseReason::ProtocolError(*v),
            ChannelEnd::Lost(ProtocolFault::NotConfirmed) => CloseReason::NotConfirmed,
            ChannelEnd::Lost(_) | ChannelEnd::Local => CloseReason::Local,
            ChannelEnd::Rekeyed => CloseReason::Rekeyed,
            ChannelEnd::Evicted => CloseReason::Evicted,
            ChannelEnd::Revoked => CloseReason::Revoked,
            ChannelEnd::PeerClosed => CloseReason::PeerClosed,
        };
        e.channel.close(reason);
        // Partial messages go with the reassembler.
        e.rx = Reassembler::new();
        self.note_zeroised(chan, &e);

        let mut out = Vec::new();
        if let ChannelEnd::Lost(fault) = &end {
            out.push(Output::Send(close_frame(chan, WireClose::ProtocolError)));
            log::warn!("[e2ee] channel lost: {}", fault_class(fault));
        }
        if e.pairing.is_some() {
            let counted = self.pairing.as_ref().and_then(|o| o.tentative) == Some(chan);
            if counted && end != ChannelEnd::Revoked {
                out.extend(self.pairing_failure());
            } else if let Some(o) = &mut self.pairing {
                if o.tentative == Some(chan) {
                    o.tentative = None;
                }
            }
            return out;
        }
        if e.opened {
            if let Some(p) = self.pairs.get_mut(&e.phone) {
                p.remove(&chan);
            }
            out.push(Output::Event(E2eEvent::Closed {
                chan,
                phone: e.phone,
                end,
                processed_through: e.channel.processed_through(),
            }));
        }
        out
    }

    // -- timers -------------------------------------------------------------

    /// Advance timers: confirmation timeouts (10 s KK, 30 s pairing), the end
    /// of a wind-down (10 s), and `P`'s 10-minute life.
    pub fn poll(&mut self, now_ms: u64) -> Vec<Output> {
        let mut out = Vec::new();
        if self
            .pairing
            .as_ref()
            .is_some_and(|o| now_ms >= o.expires_ms)
        {
            out.extend(self.end_pairing(true));
        }
        let mut due: Vec<(u64, ChannelId, CloseReason)> = Vec::new();
        for (chan, e) in self.channels.iter_mut() {
            if let Some(reason) = e.channel.poll(now_ms) {
                due.push((e.seq, *chan, reason));
            }
        }
        due.sort_by_key(|(seq, _, _)| *seq);
        for (_, chan, reason) in due {
            let end = match reason {
                CloseReason::Rekeyed => ChannelEnd::Rekeyed,
                _ => ChannelEnd::Local,
            };
            let is_pairing = self
                .channels
                .get(&chan)
                .is_some_and(|e| e.pairing.is_some());
            if is_pairing {
                // A tentative that timed out: a failed attempt (§6.2 step 9).
                if let Some(e) = self.channels.remove(&chan) {
                    self.note_zeroised(chan, &e);
                }
                out.extend(self.pairing_failure());
            } else {
                out.extend(self.finish(chan, end));
            }
        }
        out
    }

    // -- sending ------------------------------------------------------------

    /// Send one message (≤ 1 MiB) on a channel. Returns the DATA frames, in
    /// order. Refused before confirmation (§4.2: nothing is served before
    /// `hello`).
    pub fn send(
        &mut self,
        chan: ChannelId,
        mtype: MsgType,
        content: &[u8],
    ) -> Result<Vec<Vec<u8>>, SendError> {
        let entry = self
            .channels
            .get_mut(&chan)
            .ok_or(SendError::UnknownChannel)?;
        if !entry.opened {
            return Err(SendError::Channel(ChannelError::NotReady));
        }
        seal_message(&mut entry.channel, &mut entry.next_msg_id, mtype, content)
    }

    /// Send a CTRL command (e.g. `bye{rekeyed}`, `input_lost`).
    pub fn send_ctrl(&mut self, chan: ChannelId, ctrl: &Ctrl) -> Result<Vec<Vec<u8>>, SendError> {
        let content = ctrl.encode().map_err(SendError::BadCtrl)?;
        self.send(chan, MsgType::Ctrl, &content)
    }

    // -- §4.6 pieces for the host -------------------------------------------

    /// The phone a channel belongs to.
    pub fn channel_phone(&self, chan: &ChannelId) -> Option<Id> {
        self.channels.get(chan).map(|e| e.phone)
    }

    /// The confirmed channels of a phone, oldest first.
    pub fn channels_of(&self, phone: &Id) -> Vec<ChannelId> {
        self.pairs
            .get(phone)
            .map(|p| p.channels().copied().collect())
            .unwrap_or_default()
    }

    /// Whether the host may serve requests on `chan` now (confirmed and not
    /// winding down).
    pub fn may_serve(&self, chan: &ChannelId) -> bool {
        self.channels
            .get(chan)
            .is_some_and(|e| e.opened && e.channel.may_serve())
    }

    /// The last counter processed from the phone on a live channel
    /// (`Some(None)`: none yet; `None`: no such channel).
    pub fn processed_through(&self, chan: &ChannelId) -> Option<Option<u64>> {
        self.channels
            .get(chan)
            .map(|e| e.channel.processed_through())
    }

    /// §4.6 step 3: the phone promised nothing after `last_sent_ctr` on this
    /// (old) channel; a later frame is a protocol error.
    pub fn set_resume_ceiling(&mut self, chan: &ChannelId, last_sent_ctr: Option<u64>) -> bool {
        match self.channels.get_mut(chan) {
            Some(e) => {
                e.channel.set_resume_ceiling(last_sent_ctr);
                true
            }
            None => false,
        }
    }

    /// §4.6 step 5: after sending `bye{rekeyed}`, take no new requests on
    /// `chan`; [`Self::poll`] closes it 10 s later ([`ChannelEnd::Rekeyed`]).
    pub fn begin_wind_down(&mut self, chan: &ChannelId, now_ms: u64) -> bool {
        match self.channels.get_mut(chan) {
            Some(e) if e.opened => {
                e.channel.begin_wind_down(now_ms);
                true
            }
            _ => false,
        }
    }

    // -- revocation and reset -----------------------------------------------

    /// §6.4 desktop revocation (Settings → Mobile, or a revocation the server
    /// reported). Removes the pin and writes the tombstone first, then closes
    /// and zeroises every channel with the phone, aborts its pending
    /// handshakes and pairing tentative, and reports each confirmed channel
    /// as [`ChannelEnd::Revoked`] (the host then fails requests, releases
    /// handles and subscriptions, discards unwritten input and stops stream
    /// writers). The revocation holds in memory even if the write fails.
    ///
    /// The caller then re-seals the catalog without the phone (E3c) and only
    /// then calls `DELETE /v1/companion/devices/{id}`.
    pub fn revoke_phone(&mut self, phone: &Id) -> (Result<bool, StoreFailure>, Vec<Output>) {
        let result = self.desktop.store().revoke(phone, (self.wall)());
        let mut out = Vec::new();
        if let Err(e) = &result {
            out.push(Output::Event(E2eEvent::StoreWriteFailed(e.to_string())));
        }
        out.extend(self.close_phone(phone));
        (result, out)
    }

    fn close_phone(&mut self, phone: &Id) -> Vec<Output> {
        let mut chans: Vec<(u64, ChannelId)> = self
            .channels
            .iter()
            .filter(|(_, e)| e.phone == *phone)
            .map(|(c, e)| (e.seq, *c))
            .collect();
        chans.sort();
        let mut out = Vec::new();
        for (_, c) in chans {
            out.extend(self.finish(c, ChannelEnd::Revoked));
        }
        self.pairs.remove(phone);
        out
    }

    /// §6.6 compromise recovery ("Reset security keys"): new keys now, every
    /// phone pin deleted and tombstoned, every channel closed and zeroised,
    /// any pairing code (bound to the old key) discarded. Every phone must
    /// pair again; show [`super::e2e_store::ResetNotice::KeysReset`].
    pub fn reset_keys(&mut self) -> (Result<Vec<Id>, StoreFailure>, Vec<Output>) {
        let result = self.desktop.reset_keys((self.wall)());
        let mut out = self.end_pairing(true);
        let phones: HashSet<Id> = self.channels.values().map(|e| e.phone).collect();
        for p in phones {
            out.extend(self.close_phone(&p));
        }
        if let Err(e) = &result {
            out.push(Output::Event(E2eEvent::StoreWriteFailed(e.to_string())));
        }
        (result, out)
    }
}

fn fault_class(f: &ProtocolFault) -> &'static str {
    match f {
        ProtocolFault::Counter(ProtocolViolation::CounterOutOfRange { .. }) => "counter range",
        ProtocolFault::Counter(ProtocolViolation::AuthenticationFailed { .. }) => "authentication",
        ProtocolFault::Counter(ProtocolViolation::Gap { .. }) => "gap",
        ProtocolFault::Counter(ProtocolViolation::BeyondResumePoint { .. }) => "beyond resume",
        ProtocolFault::Counter(ProtocolViolation::InconsistentResume) => "inconsistent resume",
        ProtocolFault::Record(_) => "record",
        ProtocolFault::Reassembly(_) => "reassembly",
        ProtocolFault::MessageOrder => "message order",
        ProtocolFault::Ctrl(_) => "ctrl",
        ProtocolFault::UnexpectedCtrl(_) => "unexpected ctrl",
        ProtocolFault::NotConfirmed => "not confirmed",
        ProtocolFault::Host(_) => "host",
    }
}

#[cfg(test)]
mod tests {
    //! §12.2 (channel) and §12.5 (pairing) desktop-side cases, plus the §12.3
    //! replay cases, against a scripted phone and a scripted malicious relay:
    //! the test sits between the phone and the endpoint and drops, reorders,
    //! replays, splices, rewrites or bit-flips frames.

    use super::*;
    use crate::realtime::e2e_keys::MemorySecretStore;
    use crate::realtime::e2e_store::tests::{device, TempHome};
    use dragent_e2ee::codec::hs::{Hello, PairingHello, Platform};
    use dragent_e2ee::noise::kk::KkInitiator;
    use dragent_e2ee::noise::pairing::PairingInitiator;
    use dragent_e2ee::noise::StaticKeypair;

    const ACCOUNT: Id = Id([0xac; 16]);

    struct Rig {
        _home: TempHome,
        desktop: Arc<E2eDesktop>,
        ep: E2eEndpoint,
    }

    fn rig(tag: &str) -> Rig {
        let home = TempHome::new(tag);
        let secrets = Arc::new(MemorySecretStore::new());
        let desktop = Arc::new(E2eDesktop::open(&home.0, device(), secrets, 1_000).unwrap());
        let mut ep = E2eEndpoint::new(
            desktop.clone(),
            EndpointConfig {
                account: ACCOUNT,
                desktop_name: "Studio Mac".into(),
                caps: vec!["term".into()],
            },
        )
        .unwrap();
        ep.wall = || 5_000;
        Rig {
            _home: home,
            desktop,
            ep,
        }
    }

    struct Phone {
        id: Id,
        keys: StaticKeypair,
        push: StaticKeypair,
    }

    fn phone(b: u8) -> Phone {
        Phone {
            id: Id([b; 16]),
            keys: StaticKeypair::generate().unwrap(),
            push: StaticKeypair::generate().unwrap(),
        }
    }

    fn pin(r: &Rig, p: &Phone) {
        r.desktop
            .store()
            .pin_phone(p.id, p.keys.public(), p.push.public(), "Pixel".into(), 1)
            .unwrap();
    }

    /// The phone's end of a channel.
    struct PhoneChan {
        ch: Channel,
        next_msg_id: u32,
        rx: Reassembler,
    }

    impl PhoneChan {
        fn chan(&self) -> ChannelId {
            self.ch.id()
        }

        fn send(&mut self, mtype: MsgType, content: &[u8]) -> Vec<Vec<u8>> {
            seal_message(&mut self.ch, &mut self.next_msg_id, mtype, content).unwrap()
        }

        fn one(&mut self, mtype: MsgType, content: &[u8]) -> Vec<u8> {
            let mut f = self.send(mtype, content);
            assert_eq!(f.len(), 1);
            f.remove(0)
        }

        fn ctrl(&mut self, c: &Ctrl) -> Vec<u8> {
            self.one(MsgType::Ctrl, &c.encode().unwrap())
        }

        fn json(&mut self, text: &str) -> Vec<u8> {
            self.one(MsgType::Json, text.as_bytes())
        }

        /// Open a desktop DATA frame; `Some` when it completes a message.
        fn open(&mut self, frame: &[u8]) -> Option<record::Message> {
            let (f, _) = header::decode(frame).unwrap();
            assert_eq!(f.kind, Kind::Data);
            let Inbound::Record(pt) = self.ch.receive(f.ctr, f.body).unwrap() else {
                panic!("phone expected a record");
            };
            self.rx.feed(&record::decode(&pt).unwrap()).unwrap()
        }
    }

    fn sends(out: &[Output]) -> Vec<Vec<u8>> {
        out.iter()
            .filter_map(|o| match o {
                Output::Send(f) => Some(f.clone()),
                _ => None,
            })
            .collect()
    }

    fn events(out: Vec<Output>) -> Vec<E2eEvent> {
        out.into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    fn is_close(frame: &[u8], reason: WireClose) -> bool {
        let (f, _) = header::decode(frame).unwrap();
        f.kind == Kind::Close && f.body == [reason.byte()]
    }

    fn only_close(out: &[Output], reason: WireClose) {
        let s = sends(out);
        assert_eq!(s.len(), 1, "one frame: {out:?}");
        assert!(
            is_close(&s[0], reason),
            "expected CLOSE {reason:?}: {out:?}"
        );
        assert!(
            out.iter().all(|o| matches!(o, Output::Send(_))),
            "no events: {out:?}"
        );
    }

    fn reframe(frame: &[u8], chan: ChannelId, ctr: u64) -> Vec<u8> {
        let (f, _) = header::decode(frame).unwrap();
        data_frame(chan, ctr, f.body)
    }

    fn kk_hs1_for(
        p: &Phone,
        desktop_key: PublicKey,
        chan: ChannelId,
        account: Id,
        peer: Id,
    ) -> (KkInitiator, Vec<u8>) {
        let prologue = KkPrologue {
            account,
            phone: p.id,
            desktop: device(),
            chan,
        };
        let payload = hs::encode_hello(&Hello {
            caps: vec!["files".into()],
        })
        .unwrap();
        let (init, body) = KkInitiator::start(&p.keys, &desktop_key, &prologue, &payload).unwrap();
        (
            init,
            encode_frame(Head::hs1(Purpose::Companion, chan, peer, &body)),
        )
    }

    /// Handshake up to HS2: the channel is in `awaiting_confirm` on the desktop.
    fn handshake(r: &mut Rig, p: &Phone, now: u64) -> PhoneChan {
        let chan = ChannelId::random().unwrap();
        let (init, hs1) = kk_hs1_for(p, r.desktop.public(), chan, ACCOUNT, p.id);
        let out = r.ep.on_frame(&hs1, now);
        let s = sends(&out);
        assert_eq!(s.len(), 1, "{out:?}");
        let (f, _) = header::decode(&s[0]).unwrap();
        assert_eq!(
            (f.kind, f.step, f.purpose),
            (Kind::Hs, 2, Some(Purpose::Companion))
        );
        let (t, caps) = init.read_hs2(f.body).unwrap();
        assert_eq!(
            hs::decode_hello(&caps).unwrap().caps,
            vec!["term".to_string()]
        );
        PhoneChan {
            ch: Channel::initiator(ChannelPurpose::Companion, chan, t, now),
            next_msg_id: 1,
            rx: Reassembler::new(),
        }
    }

    /// Full KK open with `hello {resume}`.
    fn connect_resume(r: &mut Rig, p: &Phone, now: u64, resume: Option<HelloResume>) -> PhoneChan {
        let mut pc = handshake(r, p, now);
        let hello = pc.ctrl(&Ctrl::Hello { resume });
        let ev = events(r.ep.on_frame(&hello, now));
        assert!(
            matches!(ev.as_slice(), [.., E2eEvent::Opened { chan, phone, resume: got, caps }]
                if *chan == pc.chan() && *phone == p.id && *got == resume && caps == &vec!["files".to_string()]),
            "{ev:?}"
        );
        pc
    }

    fn connect(r: &mut Rig, p: &Phone, now: u64) -> PhoneChan {
        connect_resume(r, p, now, None)
    }

    fn messages(out: Vec<Output>) -> Vec<(u64, Vec<u8>)> {
        events(out)
            .into_iter()
            .filter_map(|e| match e {
                E2eEvent::Message { ctr, content, .. } => Some((ctr, content.to_vec())),
                _ => None,
            })
            .collect()
    }

    fn assert_lost(out: &[Output], chan: ChannelId, want: impl Fn(&ProtocolFault) -> bool) {
        let s = sends(out);
        assert!(
            s.iter().any(|f| is_close(f, WireClose::ProtocolError)),
            "CLOSE protocol_error sent: {out:?}"
        );
        assert!(
            out.iter().any(|o| matches!(o,
                Output::Event(E2eEvent::Closed { chan: c, end: ChannelEnd::Lost(fault), .. })
                    if *c == chan && want(fault))),
            "Closed(Lost) reported: {out:?}"
        );
        assert!(
            !out.iter()
                .any(|o| matches!(o, Output::Event(E2eEvent::Message { .. }))),
            "nothing processed: {out:?}"
        );
    }

    // ---- §4.2: no data before confirmation -------------------------------

    #[test]
    fn kk_open_serve_and_reply() {
        let mut r = rig("kk_open");
        let p = phone(2);
        pin(&r, &p);
        let mut pc = connect(&mut r, &p, 0);
        assert!(r.ep.may_serve(&pc.chan()));
        let req = pc.json(r#"{"type":"rpc-request","id":1}"#);
        assert_eq!(
            messages(r.ep.on_frame(&req, 1)),
            vec![(1, br#"{"type":"rpc-request","id":1}"#.to_vec())]
        );
        let reply = r.ep.send(pc.chan(), MsgType::Json, b"reply").unwrap();
        let m = pc.open(&reply[0]).unwrap();
        assert_eq!(
            (m.mtype, m.content.as_slice()),
            (MsgType::Json, &b"reply"[..])
        );
        assert_eq!(r.ep.processed_through(&pc.chan()), Some(Some(1)));
        assert_eq!(r.ep.channels_of(&p.id), vec![pc.chan()]);
    }

    #[test]
    fn nothing_is_served_before_hello() {
        let mut r = rig("before_hello");
        let p = phone(2);
        pin(&r, &p);
        let mut pc = handshake(&mut r, &p, 0);
        assert!(!r.ep.may_serve(&pc.chan()));
        assert_eq!(
            r.ep.send(pc.chan(), MsgType::Json, b"x"),
            Err(SendError::Channel(ChannelError::NotReady))
        );
        // A request as the first DATA: dropped, zeroised, never processed.
        let req = pc.json(r#"{"type":"rpc-request"}"#);
        let out = r.ep.on_frame(&req, 1);
        only_close(&out, WireClose::ProtocolError);
        assert!(r.ep.channels.is_empty());
        assert_eq!(r.ep.zeroised, vec![(pc.chan(), false)]);
        // A hello after that reaches nothing.
        let hello = pc.ctrl(&Ctrl::Hello { resume: None });
        assert!(r.ep.on_frame(&hello, 2).is_empty());
    }

    #[test]
    fn other_ctrl_as_first_record_is_not_confirmation() {
        let mut r = rig("first_bye");
        let p = phone(2);
        pin(&r, &p);
        for first in [
            Ctrl::Bye { reason: "x".into() },
            Ctrl::PairConfirm { h: [0; 32] },
            Ctrl::InputLostAck,
        ] {
            let mut pc = handshake(&mut r, &p, 0);
            let f = pc.ctrl(&first);
            only_close(&r.ep.on_frame(&f, 1), WireClose::ProtocolError);
        }
        assert!(r.ep.channels.is_empty());
    }

    #[test]
    fn unconfirmed_channel_dies_after_10_s() {
        let mut r = rig("confirm_timeout");
        let p = phone(2);
        pin(&r, &p);
        let mut pc = handshake(&mut r, &p, 1_000);
        assert!(r.ep.poll(1_000 + CONFIRM_TIMEOUT - 1).is_empty());
        assert_eq!(r.ep.channels.len(), 1);
        assert!(
            r.ep.poll(1_000 + CONFIRM_TIMEOUT).is_empty(),
            "nothing to report"
        );
        assert!(r.ep.channels.is_empty());
        let hello = pc.ctrl(&Ctrl::Hello { resume: None });
        assert!(r.ep.on_frame(&hello, 20_000).is_empty());
    }

    const CONFIRM_TIMEOUT: u64 = dragent_e2ee::channel::CONFIRM_TIMEOUT_MS;

    // ---- §4.3 counter sequences ------------------------------------------

    /// hello (ctr 0) then `n` one-record requests (ctr 1..=n), not delivered.
    fn sealed(pc: &mut PhoneChan, n: usize) -> Vec<Vec<u8>> {
        let mut frames = vec![pc.ctrl(&Ctrl::Hello { resume: None })];
        for i in 1..=n {
            frames.push(pc.json(&format!("key{i}")));
        }
        frames
    }

    fn rig_with_handshake(tag: &str) -> (Rig, Phone, PhoneChan) {
        let mut r = rig(tag);
        let p = phone(2);
        pin(&r, &p);
        let pc = handshake(&mut r, &p, 0);
        (r, p, pc)
    }

    #[test]
    fn counters_0_1_2_are_clean() {
        let (mut r, _, mut pc) = rig_with_handshake("ctr012");
        let f = sealed(&mut pc, 2);
        assert!(matches!(
            events(r.ep.on_frame(&f[0], 0)).as_slice(),
            [E2eEvent::Opened { .. }]
        ));
        assert_eq!(
            messages(r.ep.on_frame(&f[1], 0)),
            vec![(1, b"key1".to_vec())]
        );
        assert_eq!(
            messages(r.ep.on_frame(&f[2], 0)),
            vec![(2, b"key2".to_vec())]
        );
    }

    #[test]
    fn gap_0_1_3_is_protocol_error_and_3_is_never_processed() {
        let (mut r, _, mut pc) = rig_with_handshake("ctr013");
        let f = sealed(&mut pc, 3);
        r.ep.on_frame(&f[0], 0);
        assert_eq!(messages(r.ep.on_frame(&f[1], 0)).len(), 1);
        let out = r.ep.on_frame(&f[3], 0);
        assert_lost(&out, pc.chan(), |fault| {
            *fault
                == ProtocolFault::Counter(ProtocolViolation::Gap {
                    expected: 2,
                    got: 3,
                })
        });
        // The missing frame arriving late is not processed either.
        assert!(r.ep.on_frame(&f[2], 0).is_empty());
        assert_eq!(r.ep.zeroised, vec![(pc.chan(), false)]);
    }

    #[test]
    fn a_first_frame_of_2_is_protocol_error() {
        let (mut r, _, mut pc) = rig_with_handshake("ctr2first");
        let f = sealed(&mut pc, 2);
        let out = r.ep.on_frame(&f[2], 0);
        // Not yet confirmed, so no Closed event: just CLOSE and zeroise.
        only_close(&out, WireClose::ProtocolError);
        assert!(r.ep.on_frame(&f[0], 0).is_empty(), "never opened");
    }

    #[test]
    fn replayed_data_is_dropped_silently() {
        let (mut r, _, mut pc) = rig_with_handshake("replay");
        let f = sealed(&mut pc, 2);
        r.ep.on_frame(&f[0], 0);
        r.ep.on_frame(&f[1], 0);
        assert!(r.ep.on_frame(&f[1], 0).is_empty());
        assert!(r.ep.on_frame(&f[0], 0).is_empty());
        assert_eq!(
            messages(r.ep.on_frame(&f[2], 0)),
            vec![(2, b"key2".to_vec())]
        );
    }

    #[test]
    fn forged_high_counter_with_bad_tag() {
        let (mut r, _, mut pc) = rig_with_handshake("forged");
        let f = sealed(&mut pc, 1);
        r.ep.on_frame(&f[0], 0);
        let forged = reframe(&f[1], pc.chan(), 1 << 20);
        let out = r.ep.on_frame(&forged, 0);
        assert_lost(&out, pc.chan(), |fault| {
            *fault
                == ProtocolFault::Counter(ProtocolViolation::AuthenticationFailed { ctr: 1 << 20 })
        });
    }

    #[test]
    fn counter_at_or_above_2_pow_32() {
        for ctr in [1u64 << 32, u64::MAX] {
            let (mut r, _, mut pc) = rig_with_handshake("ctr32");
            let f = sealed(&mut pc, 1);
            r.ep.on_frame(&f[0], 0);
            let out = r.ep.on_frame(&reframe(&f[1], pc.chan(), ctr), 0);
            assert_lost(&out, pc.chan(), |fault| {
                *fault == ProtocolFault::Counter(ProtocolViolation::CounterOutOfRange { ctr })
            });
        }
    }

    #[test]
    fn reorder_is_rejected() {
        let (mut r, _, mut pc) = rig_with_handshake("reorder");
        let f = sealed(&mut pc, 2);
        r.ep.on_frame(&f[0], 0);
        let out = r.ep.on_frame(&f[2], 0);
        assert_lost(&out, pc.chan(), |fault| {
            matches!(fault, ProtocolFault::Counter(ProtocolViolation::Gap { .. }))
        });
        assert!(r.ep.on_frame(&f[1], 0).is_empty());
    }

    #[test]
    fn bit_flip_is_rejected() {
        let (mut r, _, mut pc) = rig_with_handshake("bitflip");
        let f = sealed(&mut pc, 1);
        r.ep.on_frame(&f[0], 0);
        let mut flipped = f[1].clone();
        let i = header::HEADER_LEN + 3;
        flipped[i] ^= 0x01;
        let out = r.ep.on_frame(&flipped, 0);
        assert_lost(&out, pc.chan(), |fault| {
            matches!(
                fault,
                ProtocolFault::Counter(ProtocolViolation::AuthenticationFailed { ctr: 1 })
            )
        });
    }

    #[test]
    fn cross_channel_splice_and_old_channel_replay_fail_aead() {
        let mut r = rig("splice");
        let p = phone(2);
        pin(&r, &p);
        let mut a = connect(&mut r, &p, 0);
        let mut b = connect(&mut r, &p, 0);
        let a1 = a.json("for A");
        // The relay moves A's frame onto B with B's next counter.
        let out = r.ep.on_frame(&reframe(&a1, b.chan(), 1), 0);
        assert_lost(&out, b.chan(), |fault| {
            matches!(
                fault,
                ProtocolFault::Counter(ProtocolViolation::AuthenticationFailed { .. })
            )
        });
        // A is untouched and still processes its own frame.
        assert_eq!(
            messages(r.ep.on_frame(&a1, 0)),
            vec![(1, b"for A".to_vec())]
        );
        // An old channel's frame replayed into a new channel after A dies.
        let a2 = a.json("old");
        r.ep.on_socket_lost();
        let mut c = connect(&mut r, &p, 1);
        let out = r.ep.on_frame(&reframe(&a2, c.chan(), 1), 1);
        assert_lost(&out, c.chan(), |_| true);
        let _ = b.json("unused");
        let _ = c.json("unused");
    }

    #[test]
    fn hs1_bit_flip_and_prologue_mismatch_are_refused_without_state() {
        let mut r = rig("hs1bad");
        let p = phone(2);
        pin(&r, &p);
        let chan = ChannelId::random().unwrap();
        // Wrong account in the prologue.
        let (_, hs1) = kk_hs1_for(&p, r.desktop.public(), chan, Id([1; 16]), p.id);
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        // Bit flip in HS1.
        let chan = ChannelId::random().unwrap();
        let (_, mut hs1) = kk_hs1_for(&p, r.desktop.public(), chan, ACCOUNT, p.id);
        let last = hs1.len() - 1;
        hs1[last] ^= 0x80;
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        // The phone pinned another desktop key.
        let chan = ChannelId::random().unwrap();
        let other = StaticKeypair::generate().unwrap();
        let (_, hs1) = kk_hs1_for(&p, other.public(), chan, ACCOUNT, p.id);
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        assert!(r.ep.channels.is_empty());
    }

    #[test]
    fn unknown_or_substituted_peer_is_refused() {
        let mut r = rig("peer");
        let a = phone(2);
        let b = phone(3);
        pin(&r, &b);
        // Unpinned phone.
        let (_, hs1) = kk_hs1_for(
            &a,
            r.desktop.public(),
            ChannelId::random().unwrap(),
            ACCOUNT,
            a.id,
        );
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        // The relay rewrites `peer` to a pinned phone: the pin doesn't match.
        let (_, hs1) = kk_hs1_for(
            &a,
            r.desktop.public(),
            ChannelId::random().unwrap(),
            ACCOUNT,
            b.id,
        );
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        assert!(r.ep.channels.is_empty());
    }

    #[test]
    fn replayed_hs1_changes_nothing_and_gets_no_service() {
        let mut r = rig("hs1replay");
        let p = phone(2);
        pin(&r, &p);
        let chan = ChannelId::random().unwrap();
        let (_, hs1) = kk_hs1_for(&p, r.desktop.public(), chan, ACCOUNT, p.id);
        assert_eq!(sends(&r.ep.on_frame(&hs1, 0)).len(), 1);
        assert!(r.ep.on_frame(&hs1, 0).is_empty(), "replay while pending");
        assert_eq!(r.ep.channels.len(), 1);
        r.ep.poll(CONFIRM_TIMEOUT);
        assert!(r.ep.channels.is_empty());
        assert!(
            r.ep.on_frame(&hs1, 20_000).is_empty(),
            "replay after it died"
        );
        assert!(r.ep.channels.is_empty());
    }

    #[test]
    fn pending_handshakes_per_phone_are_bounded() {
        let mut r = rig("pending");
        let p = phone(2);
        pin(&r, &p);
        for _ in 0..5 {
            handshake(&mut r, &p, 0);
        }
        assert_eq!(r.ep.channels.len(), MAX_PENDING_PER_PHONE);
    }

    // ---- §4.6 pieces: rekey, wind-down, eviction ---------------------------

    #[test]
    fn rekey_resume_ceiling_and_old_channel_dies_after_10_s() {
        let mut r = rig("rekey");
        let p = phone(2);
        pin(&r, &p);
        let mut a = connect(&mut r, &p, 0);
        let a1 = a.json("k1");
        let a2 = a.json("k2");
        r.ep.on_frame(&a1, 0);
        // The phone opens B, promising it sent nothing on A after ctr 2.
        let resume = HelloResume {
            chan: a.chan(),
            last_sent_ctr: Some(2),
        };
        let b = connect_resume(&mut r, &p, 100, Some(resume));
        assert_eq!(r.ep.channel_phone(&resume.chan), Some(p.id));
        assert!(r.ep.set_resume_ceiling(&a.chan(), Some(2)));
        assert_eq!(
            messages(r.ep.on_frame(&a2, 100)).len(),
            1,
            "through ctr 2 is fine"
        );
        // Wind-down: no new requests, owed replies still go out, then it dies.
        assert!(r.ep.begin_wind_down(&a.chan(), 200));
        assert!(!r.ep.may_serve(&a.chan()));
        let bye =
            r.ep.send_ctrl(
                a.chan(),
                &Ctrl::Bye {
                    reason: "rekeyed".into(),
                },
            )
            .unwrap();
        let m = a.open(&bye[0]).unwrap();
        assert_eq!(
            Ctrl::decode(&m.content).unwrap(),
            Some(Ctrl::Bye {
                reason: "rekeyed".into()
            })
        );
        assert!(r.ep.poll(200 + 9_999).is_empty());
        let ev = events(r.ep.poll(200 + 10_000));
        assert!(
            matches!(ev.as_slice(), [E2eEvent::Closed { chan, end: ChannelEnd::Rekeyed, processed_through: Some(2), .. }] if *chan == a.chan()),
            "{ev:?}"
        );
        assert!(r.ep.may_serve(&b.chan()));
    }

    #[test]
    fn old_channel_frame_beyond_resume_point_is_protocol_error() {
        let mut r = rig("beyond");
        let p = phone(2);
        pin(&r, &p);
        let mut a = connect(&mut r, &p, 0);
        let a1 = a.json("k1");
        let a2 = a.json("sent after HS1: forbidden");
        r.ep.on_frame(&a1, 0);
        connect_resume(
            &mut r,
            &p,
            0,
            Some(HelloResume {
                chan: a.chan(),
                last_sent_ctr: Some(1),
            }),
        );
        r.ep.set_resume_ceiling(&a.chan(), Some(1));
        let out = r.ep.on_frame(&a2, 0);
        assert_lost(&out, a.chan(), |fault| {
            matches!(
                fault,
                ProtocolFault::Counter(ProtocolViolation::BeyondResumePoint { .. })
            )
        });
    }

    #[test]
    fn third_channel_evicts_the_oldest() {
        let mut r = rig("evict");
        let p = phone(2);
        pin(&r, &p);
        let a = connect(&mut r, &p, 0);
        let b = connect(&mut r, &p, 0);
        let mut c = handshake(&mut r, &p, 0);
        let hello = c.ctrl(&Ctrl::Hello { resume: None });
        let ev = events(r.ep.on_frame(&hello, 0));
        assert!(
            matches!(ev.as_slice(), [E2eEvent::Closed { chan, end: ChannelEnd::Evicted, .. }, E2eEvent::Opened { .. }] if *chan == a.chan()),
            "{ev:?}"
        );
        assert_eq!(r.ep.channels_of(&p.id), vec![b.chan(), c.chan()]);
        assert!(r.ep.zeroised.contains(&(a.chan(), false)));
    }

    // ---- the protocol_error hook -------------------------------------------

    #[test]
    fn protocol_error_hook_closes_zeroises_discards_and_reports() {
        let mut r = rig("hook");
        let p = phone(2);
        pin(&r, &p);
        let mut a = connect(&mut r, &p, 0);
        // A two-record message, of which only the first record arrives.
        let big = vec![b'x'; MAX_RECORD_CONTENT + 10];
        let frames = a.send(MsgType::Json, &big);
        assert_eq!(frames.len(), 2);
        assert!(r.ep.on_frame(&frames[0], 0).is_empty());
        assert_eq!(r.ep.channels[&a.chan()].rx.partial_count(), 1);
        // E3b reports a violation only it can see.
        let out =
            r.ep.protocol_error(a.chan(), ProtocolFault::Host("bad envelope"));
        assert_lost(&out, a.chan(), |f| {
            *f == ProtocolFault::Host("bad envelope")
        });
        let ev = events(out);
        let E2eEvent::Closed {
            end,
            processed_through,
            ..
        } = &ev[0]
        else {
            panic!()
        };
        assert!(end.discards_unwritten_input());
        assert_eq!(*processed_through, Some(1));
        assert_eq!(r.ep.zeroised, vec![(a.chan(), false)]);
        // The rest of the message, and anything else, reaches nothing.
        assert!(r.ep.on_frame(&frames[1], 0).is_empty());
        assert!(r
            .ep
            .protocol_error(a.chan(), ProtocolFault::Host("again"))
            .is_empty());
        assert_eq!(
            r.ep.send(a.chan(), MsgType::Json, b"x"),
            Err(SendError::UnknownChannel)
        );
    }

    #[test]
    fn malformed_records_and_ctrl_are_protocol_errors() {
        let mut r = rig("records");
        let p = phone(2);
        pin(&r, &p);
        // A repeated message id.
        let mut a = connect(&mut r, &p, 0);
        r.ep.on_frame(&a.json("one"), 0);
        a.next_msg_id -= 1;
        let out = r.ep.on_frame(&a.json("same id"), 0);
        assert_lost(&out, a.chan(), |f| *f == ProtocolFault::MessageOrder);
        // A malformed CTRL body.
        let mut b = connect(&mut r, &p, 0);
        let out =
            r.ep.on_frame(&b.one(MsgType::Ctrl, br#"bye {"reason": "x"}"#), 0);
        assert_lost(&out, b.chan(), |f| {
            matches!(f, ProtocolFault::Ctrl(CtrlError::NotCanonical))
        });
        // A second hello on a live channel.
        let mut c = connect(&mut r, &p, 0);
        let out = r.ep.on_frame(&c.ctrl(&Ctrl::Hello { resume: None }), 0);
        assert_lost(&out, c.chan(), |f| {
            *f == ProtocolFault::UnexpectedCtrl("hello")
        });
    }

    #[test]
    fn large_messages_fragment_and_reassemble() {
        let mut r = rig("large");
        let p = phone(2);
        pin(&r, &p);
        let mut a = connect(&mut r, &p, 0);
        let big: Vec<u8> = (0..MAX_TOTAL_LEN).map(|i| i as u8).collect();
        let frames = a.send(MsgType::Json, &big);
        assert_eq!(frames.len(), MAX_TOTAL_LEN.div_ceil(MAX_RECORD_CONTENT));
        assert!(frames.iter().all(|f| f.len() <= header::MAX_FRAME_LEN));
        let mut got = Vec::new();
        for f in &frames {
            got.extend(messages(r.ep.on_frame(f, 0)));
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, big);
        let back = r.ep.send(a.chan(), MsgType::Term, &big).unwrap();
        let mut done = None;
        for f in &back {
            done = a.open(f).or(done);
        }
        assert_eq!(done.unwrap().content, big);
        let too_big = vec![0u8; MAX_TOTAL_LEN + 1];
        assert_eq!(
            r.ep.send(a.chan(), MsgType::Json, &too_big),
            Err(SendError::TooLarge)
        );
        // Every record is one of the padding buckets.
        let small = r.ep.send(a.chan(), MsgType::Json, b"k").unwrap();
        assert_eq!(small[0].len(), header::HEADER_LEN + 64 + 16);
    }

    #[test]
    fn close_hints_and_socket_loss_are_peer_closed_not_lost() {
        let mut r = rig("peerclosed");
        let p = phone(2);
        pin(&r, &p);
        let a = connect(&mut r, &p, 0);
        let b = connect(&mut r, &p, 0);
        let ev = events(r.ep.on_frame(&close_frame(a.chan(), WireClose::ProtocolError), 0));
        assert!(matches!(
            ev.as_slice(),
            [E2eEvent::Closed {
                end: ChannelEnd::PeerClosed,
                ..
            }]
        ));
        let ev = events(r.ep.on_socket_lost());
        assert!(
            matches!(ev.as_slice(), [E2eEvent::Closed { chan, end: ChannelEnd::PeerClosed, .. }] if *chan == b.chan())
        );
        assert!(!ChannelEnd::PeerClosed.discards_unwritten_input());
    }

    // ---- §6.4 revocation, §6.6 rotation and reset ----------------------------

    #[test]
    fn revocation_closes_everything_and_kk_then_fails() {
        let mut r = rig("revoke");
        let p = phone(2);
        let other = phone(3);
        pin(&r, &p);
        pin(&r, &other);
        let a = connect(&mut r, &p, 0);
        let _pending = handshake(&mut r, &p, 0);
        let keep = connect(&mut r, &other, 0);
        let (res, out) = r.ep.revoke_phone(&p.id);
        assert!(res.unwrap());
        let ev = events(out);
        assert!(
            matches!(ev.as_slice(), [E2eEvent::Closed { chan, end: ChannelEnd::Revoked, .. }] if *chan == a.chan()),
            "{ev:?}"
        );
        assert_eq!(
            r.ep.channels.len(),
            1,
            "only the other phone's channel remains"
        );
        assert!(r.ep.may_serve(&keep.chan()));
        assert_eq!(r.desktop.store().lookup(&p.id), PinLookup::Tombstoned);
        let (_, hs1) = kk_hs1_for(
            &p,
            r.desktop.public(),
            ChannelId::random().unwrap(),
            ACCOUNT,
            p.id,
        );
        only_close(&r.ep.on_frame(&hs1, 1), WireClose::UnknownPeer);
    }

    #[test]
    fn identity_rotation_is_pinned_after_a_confirming_handshake() {
        let mut r = rig("rotation");
        let mut p = phone(2);
        pin(&r, &p);
        let mut a = connect(&mut r, &p, 0);
        let new_keys = StaticKeypair::generate().unwrap();
        let new_push = StaticKeypair::generate().unwrap();
        let f = a.ctrl(&Ctrl::IdentityRotate {
            new_pub: new_keys.public(),
            new_push_pub: Some(new_push.public()),
        });
        let ev = events(r.ep.on_frame(&f, 0));
        assert!(matches!(ev.as_slice(), [E2eEvent::IdentityRotated { phone }] if *phone == p.id));
        assert_eq!(
            r.desktop.store().phone(&p.id).unwrap().push,
            Some(new_push.public())
        );
        p.keys = new_keys;
        connect(&mut r, &p, 1);
        assert_eq!(
            r.desktop.store().lookup(&p.id),
            PinLookup::Pinned(p.keys.public())
        );
    }

    #[test]
    fn reset_keys_closes_everything_and_old_key_handshakes_fail() {
        let mut r = rig("resetkeys");
        let p = phone(2);
        pin(&r, &p);
        let a = connect(&mut r, &p, 0);
        let old = r.desktop.public();
        r.ep.begin_pairing(0).unwrap();
        let (res, out) = r.ep.reset_keys();
        assert_eq!(res.unwrap(), vec![p.id]);
        let ev = events(out);
        assert!(ev.iter().any(|e| matches!(e, E2eEvent::Closed { chan, end: ChannelEnd::Revoked, .. } if *chan == a.chan())));
        assert!(ev.iter().any(|e| matches!(e, E2eEvent::PairingExpired)));
        assert!(!r.ep.pairing_active());
        assert_ne!(r.desktop.public(), old);
        let (_, hs1) = kk_hs1_for(&p, old, ChannelId::random().unwrap(), ACCOUNT, p.id);
        only_close(&r.ep.on_frame(&hs1, 1), WireClose::UnknownPeer);
    }

    // ---- §6.2 pairing (§12.5 desktop side) ---------------------------------

    fn pairing_hs1(
        p: &Phone,
        qr: &PairingQr,
        secret: &[u8; 32],
        chan: ChannelId,
        peer: Id,
    ) -> (PairingInitiator, Vec<u8>) {
        let psk = PairingSecret::from_bytes(Zeroizing::new(*secret)).psk(&qr.desktop);
        let prologue = PairingPrologue {
            account: ACCOUNT,
            phone: p.id,
            desktop: qr.desktop,
            chan,
        };
        let payload = hs::encode_pairing(&PairingHello {
            device_name: "Pixel 9".into(),
            platform: Platform::Android,
            push_pub: p.push.public().0,
        })
        .unwrap();
        let (init, body) =
            PairingInitiator::start(&p.keys, &qr.key, &psk, &prologue, &payload).unwrap();
        (
            init,
            encode_frame(Head::hs1(Purpose::Pairing, chan, peer, &body)),
        )
    }

    /// Pairing up to HS2; returns the phone's channel.
    fn pairing_handshake(r: &mut Rig, p: &Phone, qr: &PairingQr, now: u64) -> PhoneChan {
        let chan = ChannelId::random().unwrap();
        let (init, hs1) = pairing_hs1(p, qr, &qr.secret, chan, p.id);
        let out = r.ep.on_frame(&hs1, now);
        let s = sends(&out);
        assert_eq!(s.len(), 1, "{out:?}");
        let (f, _) = header::decode(&s[0]).unwrap();
        assert_eq!(
            (f.kind, f.step, f.purpose),
            (Kind::Hs, 2, Some(Purpose::Pairing))
        );
        let t = init.read_hs2(f.body).unwrap();
        PhoneChan {
            ch: Channel::initiator(ChannelPurpose::Pairing, chan, t, now),
            next_msg_id: 1,
            rx: Reassembler::new(),
        }
    }

    #[test]
    fn pairing_commits_only_on_a_matching_confirmation() {
        let mut r = rig("pair_ok");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        assert_eq!(qr.key, r.desktop.public());
        assert_eq!(qr.desktop, device());
        r.ep.set_pairing_peer(p.id);
        let mut pc = pairing_handshake(&mut r, &p, &qr, 1);
        // Tentative: nothing pinned, P not consumed, nothing can be sent.
        assert_eq!(r.desktop.store().lookup(&p.id), PinLookup::Unpinned);
        assert!(r.ep.pairing_active());
        assert!(r.ep.send(pc.chan(), MsgType::Json, b"x").is_err());
        let v_before = r.desktop.store().catalog_version();
        let h = *pc.ch.handshake_hash();
        let out = r.ep.on_frame(&pc.ctrl(&Ctrl::PairConfirm { h }), 2);
        let frames = sends(&out);
        assert_eq!(frames.len(), 1);
        let done = pc.open(&frames[0]).unwrap();
        let catalog_version = v_before + 1;
        assert_eq!(
            Ctrl::decode(&done.content).unwrap(),
            Some(Ctrl::PairDone {
                desktop_name: "Studio Mac".into(),
                catalog_version
            })
        );
        let ev = events(out);
        assert!(
            matches!(ev.as_slice(), [E2eEvent::Paired { phone, name, catalog_version: v }]
                if *phone == p.id && name == "Pixel 9" && *v == catalog_version),
            "{ev:?}"
        );
        let pinned = r.desktop.store().phone(&p.id).unwrap();
        assert_eq!(
            (pinned.identity, pinned.push, pinned.name.as_str()),
            (p.keys.public(), Some(p.push.public()), "Pixel 9")
        );
        assert!(!r.ep.pairing_active(), "P consumed");
        assert!(r.ep.channels.is_empty(), "pairing channel zeroised");
        // The new pin works for KK.
        connect(&mut r, &p, 3);
    }

    #[test]
    fn hs1_without_p_gives_nothing_and_costs_nothing() {
        let mut r = rig("pair_nop");
        let relay = phone(9);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        for _ in 0..5 {
            let guess = [7u8; 32];
            let (_, hs1) = pairing_hs1(&relay, &qr, &guess, ChannelId::random().unwrap(), relay.id);
            only_close(&r.ep.on_frame(&hs1, 1), WireClose::UnknownPeer);
        }
        assert_eq!(r.desktop.store().lookup(&relay.id), PinLookup::Unpinned);
        assert!(r.ep.pairing_active(), "not burned, not consumed");
        assert!(r.ep.channels.is_empty());
        // The real phone still pairs.
        let p = phone(2);
        let mut pc = pairing_handshake(&mut r, &p, &qr, 2);
        let h = *pc.ch.handshake_hash();
        r.ep.on_frame(&pc.ctrl(&Ctrl::PairConfirm { h }), 3);
        assert!(r.desktop.store().phone(&p.id).is_some());
    }

    #[test]
    fn substituted_desktop_key_in_hs1_is_refused() {
        // The relay swapped the key in the redeem response; the phone uses the
        // QR's key, so the relay can't be the desktop, and a phone that used
        // the relay's key doesn't reach this desktop either.
        let mut r = rig("pair_subkey");
        let p = phone(2);
        let (mut qr, _) = r.ep.begin_pairing(0).unwrap();
        qr.key = StaticKeypair::generate().unwrap().public();
        let (_, hs1) = pairing_hs1(&p, &qr, &qr.secret, ChannelId::random().unwrap(), p.id);
        only_close(&r.ep.on_frame(&hs1, 1), WireClose::UnknownPeer);
    }

    #[test]
    fn replayed_pairing_hs1_commits_nothing() {
        let mut r = rig("pair_replay");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        let chan = ChannelId::random().unwrap();
        let (_, hs1) = pairing_hs1(&p, &qr, &qr.secret, chan, p.id);
        assert_eq!(sends(&r.ep.on_frame(&hs1, 0)).len(), 1);
        assert!(r.ep.on_frame(&hs1, 1).is_empty(), "replay: no state change");
        // Never confirmed: the 30 s timeout discards the tentative.
        let ev = events(r.ep.poll(dragent_e2ee::channel::PAIRING_CONFIRM_TIMEOUT_MS));
        assert!(
            matches!(ev.as_slice(), [E2eEvent::PairingFailed { burned: false }]),
            "{ev:?}"
        );
        assert!(
            r.ep.on_frame(&hs1, 40_000).is_empty(),
            "replay after timeout"
        );
        assert_eq!(r.desktop.store().lookup(&p.id), PinLookup::Unpinned);
    }

    #[test]
    fn wrong_confirmation_hash_commits_nothing() {
        let mut r = rig("pair_badh");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        let mut pc = pairing_handshake(&mut r, &p, &qr, 0);
        let out =
            r.ep.on_frame(&pc.ctrl(&Ctrl::PairConfirm { h: [0; 32] }), 1);
        assert!(sends(&out)
            .iter()
            .any(|f| is_close(f, WireClose::ProtocolError)));
        assert!(events(out)
            .iter()
            .any(|e| matches!(e, E2eEvent::PairingFailed { burned: false })));
        assert_eq!(r.desktop.store().lookup(&p.id), PinLookup::Unpinned);
        assert!(r.ep.pairing_active());
    }

    #[test]
    fn three_failures_burn_p() {
        let mut r = rig("pair_burn");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        // 1: wrong hash.
        let mut a = pairing_handshake(&mut r, &p, &qr, 0);
        let ev = events(r.ep.on_frame(&a.ctrl(&Ctrl::PairConfirm { h: [1; 32] }), 0));
        assert!(ev
            .iter()
            .any(|e| matches!(e, E2eEvent::PairingFailed { burned: false })));
        // 2: the channel closed.
        let b = pairing_handshake(&mut r, &p, &qr, 0);
        let ev = events(r.ep.on_frame(&close_frame(b.chan(), WireClose::ProtocolError), 0));
        assert!(
            matches!(ev.as_slice(), [E2eEvent::PairingFailed { burned: false }]),
            "{ev:?}"
        );
        // 3: timeout.
        pairing_handshake(&mut r, &p, &qr, 0);
        let ev = events(r.ep.poll(dragent_e2ee::channel::PAIRING_CONFIRM_TIMEOUT_MS));
        assert!(
            matches!(ev.as_slice(), [E2eEvent::PairingFailed { burned: true }]),
            "{ev:?}"
        );
        assert!(!r.ep.pairing_active());
        // A correct attempt is now refused.
        let (_, hs1) = pairing_hs1(&p, &qr, &qr.secret, ChannelId::random().unwrap(), p.id);
        only_close(&r.ep.on_frame(&hs1, 1), WireClose::UnknownPeer);
        assert_eq!(r.desktop.store().lookup(&p.id), PinLookup::Unpinned);
    }

    #[test]
    fn substituted_peer_is_rejected() {
        let mut r = rig("pair_peer");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        r.ep.set_pairing_peer(p.id);
        // The relay rewrites the initiator id to another device.
        let (_, hs1) = pairing_hs1(
            &p,
            &qr,
            &qr.secret,
            ChannelId::random().unwrap(),
            Id([7; 16]),
        );
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        // Even when the redeemer isn't known yet, the id is bound by the
        // prologue, so a rewritten id fails the handshake.
        r.ep.begin_pairing(0).unwrap();
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        let (_, hs1) = pairing_hs1(
            &p,
            &qr,
            &qr.secret,
            ChannelId::random().unwrap(),
            Id([7; 16]),
        );
        only_close(&r.ep.on_frame(&hs1, 0), WireClose::UnknownPeer);
        assert!(r.ep.pairing_active());
    }

    #[test]
    fn one_tentative_per_p_and_p_expires_after_10_minutes() {
        let mut r = rig("pair_expire");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        pairing_handshake(&mut r, &p, &qr, 0);
        let (_, hs1) = pairing_hs1(&p, &qr, &qr.secret, ChannelId::random().unwrap(), p.id);
        only_close(&r.ep.on_frame(&hs1, 1), WireClose::UnknownPeer);
        let ev = events(r.ep.poll(PAIRING_SECRET_TTL_MS));
        assert!(ev.iter().any(|e| matches!(e, E2eEvent::PairingExpired)));
        assert!(!r.ep.pairing_active());
        assert!(r.ep.channels.is_empty());
    }

    #[test]
    fn a_pairing_channel_is_not_confirmed_by_hello() {
        let mut r = rig("pair_hello");
        let p = phone(2);
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        let mut pc = pairing_handshake(&mut r, &p, &qr, 0);
        let out = r.ep.on_frame(&pc.ctrl(&Ctrl::Hello { resume: None }), 1);
        assert!(events(out)
            .iter()
            .any(|e| matches!(e, E2eEvent::PairingFailed { .. })));
        assert_eq!(r.desktop.store().lookup(&p.id), PinLookup::Unpinned);
    }

    #[test]
    fn a_revoked_phone_can_pair_again_and_revoking_aborts_a_tentative() {
        let mut r = rig("pair_again");
        let p = phone(2);
        pin(&r, &p);
        r.ep.revoke_phone(&p.id).0.unwrap();
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        let _pc = pairing_handshake(&mut r, &p, &qr, 0);
        // Revocation aborts the tentative without counting it.
        let (_, out) = r.ep.revoke_phone(&p.id);
        assert!(events(out).is_empty());
        assert!(r.ep.channels.is_empty());
        let mut pc = pairing_handshake(&mut r, &p, &qr, 1);
        let h = *pc.ch.handshake_hash();
        r.ep.on_frame(&pc.ctrl(&Ctrl::PairConfirm { h }), 2);
        assert_eq!(
            r.desktop.store().lookup(&p.id),
            PinLookup::Pinned(p.keys.public())
        );
    }

    #[test]
    fn pairing_qr_never_prints_the_secret() {
        let mut r = rig("pair_debug");
        let (qr, _) = r.ep.begin_pairing(0).unwrap();
        let printed = format!("{qr:?}");
        assert!(!printed.contains(&*qr.secret_b64()));
        assert_eq!(qr.secret_b64().len(), 43);
        assert_eq!(qr.key_b64().len(), 43);
    }
}
