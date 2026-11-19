//! The phone's handshakes and channels: pairing (IKpsk1, §6.2) and the live
//! companion channel (KK, §4.2), both as initiator, and the §4.2/§4.3 channel
//! state machine from `dragent_e2ee::channel`.

use std::sync::{Arc, Mutex};

use dragent_e2ee::channel::{self as ch, ChannelError};
use dragent_e2ee::codec::{header, hs};
use dragent_e2ee::noise::kk::KkInitiator;
use dragent_e2ee::noise::pairing::{PairingInitiator, PairingSecret};
use dragent_e2ee::noise::{KkPrologue, PairingPrologue};
use dragent_e2ee::ChannelId;
use zeroize::{Zeroize, Zeroizing};

use crate::{lock, parse_chan, parse_id, parse_key, E2eeError, Keypair};

/// Ids that bind a handshake (§5 prologues).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChannelIds {
    /// The account (lowercase canonical UUID).
    pub account: String,
    /// This phone's device id.
    pub phone: String,
    /// The desktop's device id.
    pub desktop: String,
    /// The 16-byte channel id (initiator-chosen, [`crate::random_channel_id`]).
    pub chan: Vec<u8>,
}

struct Parsed {
    account: dragent_e2ee::Id,
    phone: dragent_e2ee::Id,
    desktop: dragent_e2ee::Id,
    chan: ChannelId,
}

impl ChannelIds {
    fn parse(&self) -> Result<Parsed, E2eeError> {
        Ok(Parsed {
            account: parse_id("account", &self.account)?,
            phone: parse_id("phone", &self.phone)?,
            desktop: parse_id("desktop", &self.desktop)?,
            chan: parse_chan(&self.chan)?,
        })
    }
}

/// Phone platform class (§6.2 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhonePlatform {
    /// iOS.
    Ios,
    /// Android.
    Android,
}

/// The pairing HS1 to send, and the handshake that reads HS2.
#[derive(uniffi::Record)]
pub struct PairingStart {
    /// The Noise HS1 message (the frame body; wrap with
    /// [`crate::encode_hs1_frame`] and purpose `Pairing`, peer = desktop).
    pub hs1: Vec<u8>,
    /// Call [`PairingHandshake::finish`] with HS2.
    pub handshake: Arc<PairingHandshake>,
}

/// A pairing handshake waiting for HS2.
#[derive(uniffi::Object)]
pub struct PairingHandshake {
    state: Mutex<Option<(PairingInitiator, ChannelId)>>,
}

/// §6.2 step 5: build pairing HS1 to the desktop key **from the QR** (`k`),
/// with `psk = HKDF(P, desktop_id)`. The payload is `{device_name, platform,
/// push_pub}`; the device name never goes to the server.
///
/// `secret_p` (the QR's `p`) is wiped on the Rust side before this returns;
/// the caller drops its own copy at once (§6.2 step 3).
#[uniffi::export]
pub fn start_pairing(
    identity: &Keypair,
    push_public_key: Vec<u8>,
    desktop_key_from_qr: Vec<u8>,
    mut secret_p: Vec<u8>,
    ids: ChannelIds,
    device_name: String,
    platform: PhonePlatform,
) -> Result<PairingStart, E2eeError> {
    let p = <[u8; 32]>::try_from(&secret_p[..]).map(Zeroizing::new);
    secret_p.zeroize();
    let p = PairingSecret::from_bytes(p.map_err(|_| E2eeError::invalid("P must be 32 bytes"))?);
    let ids = ids.parse()?;
    let desktop_key = parse_key("desktop key", &desktop_key_from_qr)?;
    let push_pub = <[u8; 32]>::try_from(&push_public_key[..])
        .map_err(|_| E2eeError::invalid("push public key must be 32 bytes"))?;
    let payload = Zeroizing::new(
        hs::encode_pairing(&hs::PairingHello {
            device_name,
            platform: match platform {
                PhonePlatform::Ios => hs::Platform::Ios,
                PhonePlatform::Android => hs::Platform::Android,
            },
            push_pub,
        })
        .map_err(E2eeError::malformed)?,
    );
    let prologue = PairingPrologue {
        account: ids.account,
        phone: ids.phone,
        desktop: ids.desktop,
        chan: ids.chan,
    };
    let psk = p.psk(&ids.desktop);
    let (init, hs1) =
        PairingInitiator::start(&identity.inner, &desktop_key, &psk, &prologue, &payload)?;
    Ok(PairingStart {
        hs1,
        handshake: Arc::new(PairingHandshake {
            state: Mutex::new(Some((init, ids.chan))),
        }),
    })
}

#[uniffi::export]
impl PairingHandshake {
    /// §6.2 step 7: process HS2 (empty payload). Returns the open pairing
    /// channel; send DATA ctr 0 = CTRL `pair-confirm {h}` with
    /// `h = channel.handshake_hash()`. Single use.
    pub fn finish(&self, hs2: Vec<u8>, now_ms: u64) -> Result<Arc<Channel>, E2eeError> {
        let (init, chan) = lock(&self.state).take().ok_or(E2eeError::Consumed)?;
        let transport = init.read_hs2(&hs2)?;
        Ok(Channel::wrap(ch::Channel::initiator(
            ch::Purpose::Pairing,
            chan,
            transport,
            now_ms,
        )))
    }

    /// Abandon the handshake now (pairing cancelled or timed out).
    pub fn abandon(&self) {
        lock(&self.state).take();
    }
}

/// The KK HS1 to send, and the handshake that reads HS2.
#[derive(uniffi::Record)]
pub struct KkStart {
    /// The Noise HS1 message (frame body; purpose `Companion`, peer = desktop).
    pub hs1: Vec<u8>,
    /// Call [`KkHandshake::finish`] with HS2.
    pub handshake: Arc<KkHandshake>,
}

/// A KK handshake waiting for HS2.
#[derive(uniffi::Object)]
pub struct KkHandshake {
    state: Mutex<Option<(KkInitiator, ChannelId)>>,
}

/// HS2 verified: the open channel and the desktop's caps.
#[derive(uniffi::Record)]
pub struct KkFinished {
    /// The channel, `open`. Its DATA ctr 0 must be CTRL `hello`.
    pub channel: Arc<Channel>,
    /// The desktop's capability list from HS2.
    pub caps: Vec<String>,
}

/// §4.2: build KK HS1 to the pinned desktop key. The payload is
/// `{"v":1,"caps":[…]}` (replayable, so it carries nothing that matters).
#[uniffi::export]
pub fn start_kk(
    identity: &Keypair,
    desktop_pin: Vec<u8>,
    ids: ChannelIds,
    caps: Vec<String>,
) -> Result<KkStart, E2eeError> {
    let ids = ids.parse()?;
    let pin = parse_key("desktop pin", &desktop_pin)?;
    let payload = hs::encode_hello(&hs::Hello { caps }).map_err(E2eeError::malformed)?;
    let prologue = KkPrologue {
        account: ids.account,
        phone: ids.phone,
        desktop: ids.desktop,
        chan: ids.chan,
    };
    let (init, hs1) = KkInitiator::start(&identity.inner, &pin, &prologue, &payload)?;
    Ok(KkStart {
        hs1,
        handshake: Arc::new(KkHandshake {
            state: Mutex::new(Some((init, ids.chan))),
        }),
    })
}

#[uniffi::export]
impl KkHandshake {
    /// Process HS2. Success authenticates the pinned desktop. Single use.
    pub fn finish(&self, hs2: Vec<u8>, now_ms: u64) -> Result<KkFinished, E2eeError> {
        let (init, chan) = lock(&self.state).take().ok_or(E2eeError::Consumed)?;
        let (transport, payload) = init.read_hs2(&hs2)?;
        let caps = hs::decode_hello(&payload)
            .map_err(E2eeError::malformed)?
            .caps;
        Ok(KkFinished {
            channel: Channel::wrap(ch::Channel::initiator(
                ch::Purpose::Companion,
                chan,
                transport,
                now_ms,
            )),
            caps,
        })
    }

    /// Abandon the handshake now.
    pub fn abandon(&self) {
        lock(&self.state).take();
    }
}

/// The §4.2 state of a channel.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ChannelStateKind {
    /// Responder only: waiting for the first DATA.
    AwaitingConfirm,
    /// Responder only: the first DATA is being checked.
    Confirming,
    /// Open.
    Open,
    /// Responder only: winding down after a rekey.
    WindingDown,
    /// Closed; keys zeroised.
    Closed {
        /// Why (`protocol_error`, `rekeyed`, `revoked`, …).
        reason: String,
    },
}

/// Why the phone closes a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LocalCloseReason {
    /// Closed locally (app shutdown, lease ended, superseded after rekey).
    Local,
    /// The relay or desktop closed it (CLOSE frame, socket gone).
    PeerClosed,
    /// §6.4: the desktop was unpaired.
    Revoked,
    /// §4.6: the old channel finished after a rekey (`bye{rekeyed}` seen).
    Rekeyed,
}

/// A sealed DATA frame.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SealedData {
    /// Header `ctr` (also the AEAD nonce).
    pub ctr: u64,
    /// Transport ciphertext with tag (header `body`).
    pub ciphertext: Vec<u8>,
}

/// The result of receiving one DATA frame.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum Inbound {
    /// The next record in sequence: process it.
    Record {
        /// The decrypted record (decode with [`crate::decode_record`] or feed a
        /// [`crate::Reassembler`]).
        record: Vec<u8>,
    },
    /// Responder only (never on the phone): the confirming record.
    Confirming {
        /// The record.
        record: Vec<u8>,
    },
    /// A replay or duplicate: dropped silently.
    Duplicate,
}

/// `hello {resume: {chan, last_sent_ctr}}` data (§4.6 step 1).
#[derive(Debug, Clone, uniffi::Record)]
pub struct HelloResume {
    /// The old channel's id (16 bytes).
    pub chan: Vec<u8>,
    /// The last counter sent on it (`None`: nothing was sent).
    pub last_sent_ctr: Option<u64>,
}

/// One end of a channel after its handshake (§4.2, §4.3).
#[derive(uniffi::Object)]
pub struct Channel {
    inner: Mutex<ch::Channel>,
    chan: ChannelId,
}

impl Channel {
    fn wrap(c: ch::Channel) -> Arc<Self> {
        let chan = c.id();
        Arc::new(Channel {
            inner: Mutex::new(c),
            chan,
        })
    }
}

fn close_reason_name(r: ch::CloseReason) -> String {
    match r {
        ch::CloseReason::ProtocolError(_) => "protocol_error",
        ch::CloseReason::ConfirmTimeout => "confirm_timeout",
        ch::CloseReason::NotConfirmed => "not_confirmed",
        ch::CloseReason::Rekeyed => "rekeyed",
        ch::CloseReason::Evicted => "evicted",
        ch::CloseReason::Revoked => "revoked",
        ch::CloseReason::PeerClosed => "peer_closed",
        ch::CloseReason::Local => "local",
    }
    .to_string()
}

impl From<ChannelError> for E2eeError {
    fn from(e: ChannelError) -> Self {
        match e {
            ChannelError::Protocol(v) => E2eeError::ProtocolError {
                detail: v.to_string(),
            },
            ChannelError::Closed(r) => E2eeError::ChannelClosed {
                reason: close_reason_name(r),
            },
            ChannelError::NotReady => E2eeError::NotReady,
            ChannelError::CounterExhausted => E2eeError::CounterExhausted,
            ChannelError::TooLarge => E2eeError::TooLarge,
        }
    }
}

#[uniffi::export]
impl Channel {
    /// The 16-byte channel id.
    pub fn chan_id(&self) -> Vec<u8> {
        self.chan.as_bytes().to_vec()
    }

    /// The current state.
    pub fn state(&self) -> ChannelStateKind {
        match lock(&self.inner).state() {
            ch::ChannelState::AwaitingConfirm => ChannelStateKind::AwaitingConfirm,
            ch::ChannelState::Confirming => ChannelStateKind::Confirming,
            ch::ChannelState::Open => ChannelStateKind::Open,
            ch::ChannelState::WindingDown => ChannelStateKind::WindingDown,
            ch::ChannelState::Closed(r) => ChannelStateKind::Closed {
                reason: close_reason_name(r),
            },
        }
    }

    /// The handshake hash `h` (pairing: `pair-confirm {h}`, §6.2 step 7).
    pub fn handshake_hash(&self) -> Vec<u8> {
        lock(&self.inner).handshake_hash().to_vec()
    }

    /// The peer's authenticated static key.
    pub fn remote_static(&self) -> Vec<u8> {
        lock(&self.inner).remote_static().as_bytes().to_vec()
    }

    /// Whether keys are still held (false once closed: zeroised).
    pub fn has_keys(&self) -> bool {
        lock(&self.inner).has_keys()
    }

    /// The last counter processed from the peer.
    pub fn processed_through(&self) -> Option<u64> {
        lock(&self.inner).processed_through()
    }

    /// The last counter sent (`None` if nothing was sent).
    pub fn last_sent_ctr(&self) -> Option<u64> {
        lock(&self.inner).last_sent_ctr()
    }

    /// Whether [`Channel::quiesce`] was called.
    pub fn is_quiesced(&self) -> bool {
        lock(&self.inner).is_quiesced()
    }

    /// Seal one record (a full §4.4 record, padded) as the next DATA frame.
    pub fn seal(&self, record: Vec<u8>) -> Result<SealedData, E2eeError> {
        let out = lock(&self.inner).seal(&record)?;
        Ok(SealedData {
            ctr: out.ctr,
            ciphertext: out.ciphertext,
        })
    }

    /// Seal one record and encode the whole DATA frame (44-byte §4.1 header
    /// plus ciphertext): one FFI crossing per keystroke.
    pub fn seal_frame(&self, record: Vec<u8>) -> Result<Vec<u8>, E2eeError> {
        let out = lock(&self.inner).seal(&record)?;
        let mut frame = Vec::with_capacity(header::HEADER_LEN + out.ciphertext.len());
        header::Head {
            kind: header::Kind::Data,
            step: 0,
            purpose: None,
            chan: self.chan,
            peer: None,
            ctr: out.ctr,
            body: &out.ciphertext,
        }
        .encode(&mut frame)
        .map_err(E2eeError::malformed)?;
        Ok(frame)
    }

    /// §4.3: receive one DATA frame. A protocol violation closes and zeroises
    /// the channel before this returns ([`E2eeError::ProtocolError`]).
    pub fn receive(&self, ctr: u64, ciphertext: Vec<u8>) -> Result<Inbound, E2eeError> {
        Ok(match lock(&self.inner).receive(ctr, &ciphertext)? {
            ch::Inbound::Record(r) => Inbound::Record { record: r.to_vec() },
            ch::Inbound::Confirming(r) => Inbound::Confirming { record: r.to_vec() },
            ch::Inbound::Duplicate => Inbound::Duplicate,
        })
    }

    /// §4.6 step 1: HS1 for a new channel has been sent, so no more DATA here.
    /// Returns the `resume` for the new channel's `hello`.
    pub fn quiesce(&self) -> HelloResume {
        let r = lock(&self.inner).quiesce();
        HelloResume {
            chan: r.chan.as_bytes().to_vec(),
            last_sent_ctr: r.last_sent_ctr,
        }
    }

    /// Advance timers; returns the close reason if this call closed it.
    pub fn poll(&self, now_ms: u64) -> Option<String> {
        lock(&self.inner).poll(now_ms).map(close_reason_name)
    }

    /// §4.6: whether to rekey now (2^20 frames or 60 minutes).
    pub fn rekey_due(&self, now_ms: u64) -> bool {
        lock(&self.inner).rekey_due(now_ms)
    }

    /// Close now and zeroise the keys (idempotent; the first reason sticks).
    pub fn close(&self, reason: LocalCloseReason) {
        lock(&self.inner).close(match reason {
            LocalCloseReason::Local => ch::CloseReason::Local,
            LocalCloseReason::PeerClosed => ch::CloseReason::PeerClosed,
            LocalCloseReason::Revoked => ch::CloseReason::Revoked,
            LocalCloseReason::Rekeyed => ch::CloseReason::Rekeyed,
        });
    }
}
