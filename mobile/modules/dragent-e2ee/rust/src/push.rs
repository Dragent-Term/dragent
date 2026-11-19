//! §8 push, phone side: the push-pin list and opening a sealed notification.
//!
//! The push-pin list maps a desktop's identity public key to its device id.
//! It lives in the keychain access group shared with the iOS Notification
//! Service Extension (`AfterFirstUnlockThisDeviceOnly`), or Keystore-wrapped
//! without the unlock requirement on Android (§6.7), so it can be read while
//! the phone is locked. The stored value is [`encode_push_pins`]'s bytes:
//!
//! ```json
//! {"v":1,"pins":[{"desktop":"<uuid>","key":"<64 lowercase hex>"}]}
//! ```
//!
//! [`open_push`] is the one call the extension / messaging service makes: on
//! any error it shows the constant placeholder from the app bundle, never
//! anything from the payload.

use dragent_e2ee::codec::push_plain;
use dragent_e2ee::noise::PushPrologue;
use dragent_e2ee::seal::push as push_seal;
use serde::{Deserialize, Serialize};

use crate::{parse_id, parse_key, E2eeError, Keypair};

/// §8 step 4.3: notifications older than this are dropped.
pub const MAX_PUSH_AGE_MS: u64 = 24 * 60 * 60 * 1000;

/// The sealed size of every notification (§8).
#[uniffi::export]
pub fn push_sealed_len() -> u32 {
    push_seal::SEALED_LEN as u32
}

/// One push-pin entry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PushPin {
    /// The desktop's device id.
    pub desktop: String,
    /// The desktop's identity public key (32 bytes).
    pub key: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinsDoc {
    v: u8,
    pins: Vec<PinEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinEntry {
    desktop: String,
    key: String,
}

fn hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    b.iter()
        .flat_map(|x| [D[(x >> 4) as usize] as char, D[(x & 15) as usize] as char])
        .collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let v = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    s.as_bytes()
        .chunks(2)
        .map(|p| Some(v(p[0])? << 4 | v(p[1])?))
        .collect()
}

/// Encode the push-pin list for the keychain. One entry per desktop; the
/// same desktop or key twice is refused.
#[uniffi::export]
pub fn encode_push_pins(pins: Vec<PushPin>) -> Result<Vec<u8>, E2eeError> {
    let mut entries = Vec::with_capacity(pins.len());
    for (i, p) in pins.iter().enumerate() {
        let id = parse_id("desktop", &p.desktop)?;
        let key = parse_key("desktop key", &p.key)?;
        if pins[..i]
            .iter()
            .any(|q| q.desktop == p.desktop || q.key == p.key)
        {
            return Err(E2eeError::invalid("duplicate push pin"));
        }
        entries.push(PinEntry {
            desktop: id.to_string(),
            key: hex(key.as_bytes()),
        });
    }
    serde_json::to_vec(&PinsDoc {
        v: 1,
        pins: entries,
    })
    .map_err(|_| E2eeError::Internal {
        detail: "push pins json".into(),
    })
}

/// Decode the push-pin list from the keychain. A malformed list is an error
/// (the caller treats it as "no pins": every push shows the placeholder).
#[uniffi::export]
pub fn decode_push_pins(bytes: Vec<u8>) -> Result<Vec<PushPin>, E2eeError> {
    let doc: PinsDoc =
        serde_json::from_slice(&bytes).map_err(|_| E2eeError::malformed("push pins json"))?;
    if doc.v != 1 {
        return Err(E2eeError::malformed("push pins version"));
    }
    let mut out = Vec::with_capacity(doc.pins.len());
    for e in doc.pins {
        let id = parse_id("desktop", &e.desktop)?;
        let key = unhex(&e.key)
            .and_then(|b| dragent_e2ee::noise::PublicKey::from_slice(&b))
            .ok_or_else(|| E2eeError::malformed("push pin key"))?;
        out.push(PushPin {
            desktop: id.to_string(),
            key: key.as_bytes().to_vec(),
        });
    }
    Ok(out)
}

/// A decrypted, verified notification.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PushNotification {
    /// The sending desktop (checked against the push-pin list).
    pub desktop_id: String,
    /// Random notification id (16 bytes; APNs collapse id / Android tag).
    pub id: Vec<u8>,
    /// When the desktop built it (ms since the Unix epoch).
    pub issued_at_ms: u64,
    /// Notification kind.
    pub kind: u8,
    /// `true`: a dismissal (Android only); remove `dismiss_ids`.
    pub dismiss: bool,
    /// Title (empty for dismissals).
    pub title: String,
    /// Body (empty for dismissals).
    pub body: String,
    /// Workspace it opens.
    pub workspace_id: String,
    /// Tab it opens.
    pub tab_id: String,
    /// Ids to remove (16 bytes each).
    pub dismiss_ids: Vec<Vec<u8>>,
}

/// §8 steps 4.1–4.3 / 5.1–5.2: open a sealed notification.
///
/// - `push_keys`: the current push key first, then a previous one still in
///   its 7-day rotation window (§6.6). The first that opens it wins.
/// - The Noise X sender static key must be in `pins`, and the pinned desktop
///   id must equal `desktop_id` inside the plaintext. Unknown key = failure.
/// - Older than 24 hours (by `now_ms`) = failure. Duplicate ids are the
///   caller's check (it holds the seen-id list).
///
/// Any error: show the constant placeholder.
#[uniffi::export]
pub fn open_push(
    push_keys: Vec<std::sync::Arc<Keypair>>,
    account: String,
    phone: String,
    sealed: Vec<u8>,
    pins: Vec<PushPin>,
    now_ms: u64,
) -> Result<PushNotification, E2eeError> {
    if sealed.len() != push_seal::SEALED_LEN {
        return Err(E2eeError::malformed("sealed push length"));
    }
    let prologue = PushPrologue {
        account: parse_id("account", &account)?,
        phone: parse_id("phone", &phone)?,
    };
    let mut opened = None;
    for k in &push_keys {
        if let Ok(r) = push_seal::open(&k.inner, &prologue, &sealed) {
            opened = Some(r);
            break;
        }
    }
    let (sender, plain) = opened.ok_or_else(|| E2eeError::Rejected {
        detail: "push did not open with any push key".into(),
    })?;
    let mut pinned_desktop = None;
    for p in &pins {
        let key = parse_key("pin key", &p.key)?;
        if key.ct_eq(&sender) {
            pinned_desktop = Some(parse_id("pin desktop", &p.desktop)?);
        }
    }
    let pinned_desktop = pinned_desktop.ok_or(E2eeError::WrongSender)?;
    let n = push_plain::decode(&plain[..]).map_err(E2eeError::malformed)?;
    if n.desktop_id != pinned_desktop {
        return Err(E2eeError::WrongSender);
    }
    if now_ms.saturating_sub(n.issued_at_ms) > MAX_PUSH_AGE_MS {
        return Err(E2eeError::Rejected {
            detail: "push is stale".into(),
        });
    }
    Ok(PushNotification {
        desktop_id: n.desktop_id.to_string(),
        id: n.id.to_vec(),
        issued_at_ms: n.issued_at_ms,
        kind: n.kind,
        dismiss: n.dismiss,
        title: n.title,
        body: n.body,
        workspace_id: n.workspace_id.to_string(),
        tab_id: n.tab_id.to_string(),
        dismiss_ids: n.dismiss_ids.iter().map(|d| d.to_vec()).collect(),
    })
}
