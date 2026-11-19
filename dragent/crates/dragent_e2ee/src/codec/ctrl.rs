//! CTRL control messages (§4.4 `CTRL`, §4.6, §6.2, §6.6), other than the
//! part protocol (`part-begin`, `part`, `part-end`, `part-abort`, which
//! [`crate::codec::part`] owns).
//!
//! # Wire form
//!
//! A CTRL message's content is `<name> <json>` (the framing of
//! [`crate::codec::part::split_ctrl`]): the command word, one space, and a
//! JSON object. **The content must be exactly the canonical encoding**:
//! the JSON is compact (no whitespace), fields appear in the order listed
//! below, optional fields are present or absent as listed, strings use the
//! minimal JSON escapes (exactly what `JSON.stringify`, `serde_json` and
//! Python's `json.dumps(..., separators=(",", ":"), ensure_ascii=False)`
//! produce), and numbers are plain non-negative integers. Anything else —
//! extra or missing fields, other orders, whitespace, duplicate keys,
//! escaped characters that needn't be, trailing bytes — is rejected. One
//! message has exactly one encoding, so both ends agree byte for byte.
//!
//! | name | json (canonical, fields in this order) |
//! |---|---|
//! | `hello` | `{}` or `{"resume":{"chan":"<32 lowercase hex>","last_sent_ctr":<0..2^32−1>\|null}}` |
//! | `bye` | `{"reason":"<1..64 bytes>"}` |
//! | `pair-confirm` | `{"h":"<base64url of exactly 32 bytes>"}` |
//! | `pair-done` | `{"desktop_name":"<1..128 bytes>","catalog_version":<1..2^53−1>}` |
//! | `identity-rotate` | `{"new_pub":"<b64url 32>"}` or `{"new_pub":"<b64url 32>","new_push_pub":"<b64url 32>"}` |
//! | `input_lost` | `{"handles":[<u32>,…]}` (strictly ascending, at most 4,096) |
//! | `input_lost_ack` | `{}` |
//! | `control_taken` | `{"handles":[<u32>,…]}` (strictly ascending, at most 4,096) |
//!
//! base64url is unpadded and canonical ([`crate::codec::b64`]). `chan` is
//! the channel id spelled as in the §5 prologue.
//!
//! Choices made here (the design names the commands and their fields but
//! not their encoding): the JSON shapes above, the canonical-bytes rule,
//! and the limits. `vectors/codec/ctrl.json` pins every shape and every
//! rejection; `vectors/codec/gen_ctrl.py` is the independent generator.

use serde::{Deserialize, Serialize};

use crate::channel::resume::{Handle, HelloResume};
use crate::codec::b64;
use crate::codec::part::encode_ctrl;
use crate::ids::ChannelId;
use crate::noise::PublicKey;

/// Longest `bye` reason, in bytes.
pub const MAX_BYE_REASON: usize = 64;
/// Longest `pair-done` desktop name, in bytes.
pub const MAX_DESKTOP_NAME: usize = 128;
/// Most handles in one `input_lost`.
pub const MAX_INPUT_LOST_HANDLES: usize = 4096;
/// Largest catalog version (§6.5: ≤ 2^53 − 1).
pub const MAX_CATALOG_VERSION: u64 = (1 << 53) - 1;
/// `last_sent_ctr` must be below this (§4.3: counters are < 2^32).
pub const COUNTER_LIMIT: u64 = 1 << 32;

/// The part-protocol command words, decoded by [`crate::codec::part`].
pub const PART_COMMANDS: [&str; 4] = ["part-begin", "part", "part-end", "part-abort"];

/// A CTRL control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ctrl {
    /// The phone's first DATA on a KK channel (§4.2), with the §4.6 resume
    /// point when it replaces an earlier channel.
    Hello {
        /// `None`: a fresh session; nothing is rebound.
        resume: Option<HelloResume>,
    },
    /// Authenticated goodbye, e.g. `rekeyed` (§4.6 step 5).
    Bye {
        /// The reason.
        reason: String,
    },
    /// The phone's pairing confirmation: the handshake hash (§6.2 step 7).
    PairConfirm {
        /// `h`.
        h: [u8; 32],
    },
    /// The desktop's pairing commit (§6.2 step 8).
    PairDone {
        /// The desktop's name.
        desktop_name: String,
        /// The catalog version the desktop is about to seal for this phone.
        catalog_version: u64,
    },
    /// Routine key rotation (§6.6).
    IdentityRotate {
        /// The new identity key.
        new_pub: PublicKey,
        /// The phone's new push key, rotated on the same schedule.
        new_push_pub: Option<PublicKey>,
    },
    /// §4.6 step 4: input on these handles was lost between channels.
    InputLost {
        /// The affected terminal handles, strictly ascending.
        handles: Vec<Handle>,
    },
    /// The phone dropped its queued input and showed the notice (§4.6).
    InputLostAck,
    /// The desktop took these terminals back: their handles are released and
    /// the phone leaves them. Sent by the desktop only; a peer that doesn't
    /// know the command ignores it.
    ControlTaken {
        /// The released terminal handles, strictly ascending.
        handles: Vec<Handle>,
    },
}

/// Why CTRL content was rejected. Every variant is a protocol error for the
/// channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtrlError {
    /// Not UTF-8, or not `<name> <json>`.
    BadFraming,
    /// A command word that is neither a [`Ctrl`] nor a part command.
    UnknownCommand {
        /// The word.
        name: String,
    },
    /// The JSON doesn't parse into the command's shape: unknown, missing or
    /// duplicate fields, wrong types, trailing bytes.
    NotJson,
    /// A field is out of range: a length, a key or hash size, a base64url or
    /// hex spelling, a counter, a version, an unsorted handle list.
    BadField {
        /// The field.
        field: &'static str,
    },
    /// It parsed, but these are not the canonical bytes (whitespace, field
    /// order, needless escapes, a missing optional `null`, …).
    NotCanonical,
}

impl std::fmt::Display for CtrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CtrlError::BadFraming => f.write_str("bad CTRL framing"),
            CtrlError::UnknownCommand { name } => write!(f, "unknown CTRL command {name:?}"),
            CtrlError::NotJson => f.write_str("CTRL body is not the expected JSON"),
            CtrlError::BadField { field } => write!(f, "CTRL field {field} out of range"),
            CtrlError::NotCanonical => f.write_str("CTRL content is not canonical"),
        }
    }
}

impl std::error::Error for CtrlError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<ResumeWire>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeWire {
    chan: String,
    last_sent_ctr: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ByeWire {
    reason: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairConfirmWire {
    h: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairDoneWire {
    desktop_name: String,
    catalog_version: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RotateWire {
    new_pub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_push_pub: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputLostWire {
    handles: Vec<Handle>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyWire {}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("CTRL bodies always serialize")
}

fn parse<'a, T: Deserialize<'a>>(s: &'a str) -> Result<T, CtrlError> {
    serde_json::from_str(s).map_err(|_| CtrlError::NotJson)
}

fn chan_hex(c: &ChannelId) -> String {
    c.hex()
}

fn parse_chan_hex(s: &str) -> Result<ChannelId, CtrlError> {
    let bad = CtrlError::BadField { field: "chan" };
    let b = s.as_bytes();
    if b.len() != 32 {
        return Err(bad);
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 16];
    for (i, pair) in b.chunks_exact(2).enumerate() {
        out[i] = (nibble(pair[0]).ok_or(bad.clone())? << 4) | nibble(pair[1]).ok_or(bad.clone())?;
    }
    Ok(ChannelId(out))
}

fn bytes32(s: &str, field: &'static str) -> Result<[u8; 32], CtrlError> {
    b64::decode(s)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
        .ok_or(CtrlError::BadField { field })
}

fn check_len(s: &str, min: usize, max: usize, field: &'static str) -> Result<(), CtrlError> {
    if s.len() < min || s.len() > max {
        return Err(CtrlError::BadField { field });
    }
    Ok(())
}

impl Ctrl {
    /// The command word.
    pub fn name(&self) -> &'static str {
        match self {
            Ctrl::Hello { .. } => "hello",
            Ctrl::Bye { .. } => "bye",
            Ctrl::PairConfirm { .. } => "pair-confirm",
            Ctrl::PairDone { .. } => "pair-done",
            Ctrl::IdentityRotate { .. } => "identity-rotate",
            Ctrl::InputLost { .. } => "input_lost",
            Ctrl::InputLostAck => "input_lost_ack",
            Ctrl::ControlTaken { .. } => "control_taken",
        }
    }

    /// Check the limits a decoder enforces, so nothing is sent that the peer
    /// would reject.
    pub fn validate(&self) -> Result<(), CtrlError> {
        match self {
            Ctrl::Hello { resume } => {
                if resume
                    .and_then(|r| r.last_sent_ctr)
                    .is_some_and(|c| c >= COUNTER_LIMIT)
                {
                    return Err(CtrlError::BadField {
                        field: "last_sent_ctr",
                    });
                }
            }
            Ctrl::Bye { reason } => check_len(reason, 1, MAX_BYE_REASON, "reason")?,
            Ctrl::PairConfirm { .. } | Ctrl::IdentityRotate { .. } | Ctrl::InputLostAck => {}
            Ctrl::PairDone {
                desktop_name,
                catalog_version,
            } => {
                check_len(desktop_name, 1, MAX_DESKTOP_NAME, "desktop_name")?;
                if *catalog_version == 0 || *catalog_version > MAX_CATALOG_VERSION {
                    return Err(CtrlError::BadField {
                        field: "catalog_version",
                    });
                }
            }
            Ctrl::InputLost { handles } | Ctrl::ControlTaken { handles } => {
                if handles.len() > MAX_INPUT_LOST_HANDLES
                    || handles.windows(2).any(|w| w[0] >= w[1])
                {
                    return Err(CtrlError::BadField { field: "handles" });
                }
            }
        }
        Ok(())
    }

    /// The canonical CTRL content. Fails only if [`Ctrl::validate`] fails.
    pub fn encode(&self) -> Result<Vec<u8>, CtrlError> {
        self.validate()?;
        let body = match self {
            Ctrl::Hello { resume } => to_json(&HelloWire {
                resume: resume.map(|r| ResumeWire {
                    chan: chan_hex(&r.chan),
                    last_sent_ctr: r.last_sent_ctr,
                }),
            }),
            Ctrl::Bye { reason } => to_json(&ByeWire {
                reason: reason.clone(),
            }),
            Ctrl::PairConfirm { h } => to_json(&PairConfirmWire { h: b64::encode(h) }),
            Ctrl::PairDone {
                desktop_name,
                catalog_version,
            } => to_json(&PairDoneWire {
                desktop_name: desktop_name.clone(),
                catalog_version: *catalog_version,
            }),
            Ctrl::IdentityRotate {
                new_pub,
                new_push_pub,
            } => to_json(&RotateWire {
                new_pub: b64::encode(new_pub.as_bytes()),
                new_push_pub: new_push_pub.map(|k| b64::encode(k.as_bytes())),
            }),
            Ctrl::InputLost { handles } => to_json(&InputLostWire {
                handles: handles.clone(),
            }),
            Ctrl::InputLostAck => to_json(&EmptyWire {}),
            Ctrl::ControlTaken { handles } => to_json(&InputLostWire {
                handles: handles.clone(),
            }),
        };
        Ok(encode_ctrl(self.name(), &body))
    }

    /// Decode CTRL content. `Ok(None)` for a part-protocol command (decode
    /// it with [`crate::codec::part`]); any other unknown word is an error.
    pub fn decode(content: &[u8]) -> Result<Option<Ctrl>, CtrlError> {
        let text = std::str::from_utf8(content).map_err(|_| CtrlError::BadFraming)?;
        let (name, body) = text.split_once(' ').ok_or(CtrlError::BadFraming)?;
        if name.is_empty() || body.is_empty() {
            return Err(CtrlError::BadFraming);
        }
        if PART_COMMANDS.contains(&name) {
            return Ok(None);
        }
        // serde would also read a struct from a JSON array: only objects.
        if !body.starts_with('{') {
            return Err(CtrlError::NotJson);
        }
        let ctrl = match name {
            "hello" => {
                let w: HelloWire = parse(body)?;
                let resume = match w.resume {
                    None => None,
                    Some(r) => Some(HelloResume {
                        chan: parse_chan_hex(&r.chan)?,
                        last_sent_ctr: r.last_sent_ctr,
                    }),
                };
                Ctrl::Hello { resume }
            }
            "bye" => Ctrl::Bye {
                reason: parse::<ByeWire>(body)?.reason,
            },
            "pair-confirm" => Ctrl::PairConfirm {
                h: bytes32(&parse::<PairConfirmWire>(body)?.h, "h")?,
            },
            "pair-done" => {
                let w: PairDoneWire = parse(body)?;
                Ctrl::PairDone {
                    desktop_name: w.desktop_name,
                    catalog_version: w.catalog_version,
                }
            }
            "identity-rotate" => {
                let w: RotateWire = parse(body)?;
                Ctrl::IdentityRotate {
                    new_pub: PublicKey(bytes32(&w.new_pub, "new_pub")?),
                    new_push_pub: w
                        .new_push_pub
                        .as_deref()
                        .map(|s| bytes32(s, "new_push_pub").map(PublicKey))
                        .transpose()?,
                }
            }
            "input_lost" => Ctrl::InputLost {
                handles: parse::<InputLostWire>(body)?.handles,
            },
            "input_lost_ack" => {
                let _: EmptyWire = parse(body)?;
                Ctrl::InputLostAck
            }
            "control_taken" => Ctrl::ControlTaken {
                handles: parse::<InputLostWire>(body)?.handles,
            },
            other => {
                return Err(CtrlError::UnknownCommand {
                    name: other.chars().take(64).collect(),
                })
            }
        };
        ctrl.validate()?;
        if ctrl.encode()? != content {
            return Err(CtrlError::NotCanonical);
        }
        Ok(Some(ctrl))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn hex(s: &str) -> Vec<u8> {
        assert!(s.len().is_multiple_of(2), "odd hex");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    fn key(v: &serde_json::Value) -> PublicKey {
        PublicKey(hex(v.as_str().unwrap()).try_into().unwrap())
    }

    /// Build the expected message from a vector's `expect` object.
    fn expected(e: &serde_json::Value) -> Option<Ctrl> {
        let ctrl = e["ctrl"].as_str()?;
        Some(match ctrl {
            "hello" => Ctrl::Hello {
                resume: e["resume"].as_object().map(|r| HelloResume {
                    chan: ChannelId(hex(r["chan"].as_str().unwrap()).try_into().unwrap()),
                    last_sent_ctr: r["last_sent_ctr"].as_u64(),
                }),
            },
            "bye" => Ctrl::Bye {
                reason: e["reason"].as_str().unwrap().into(),
            },
            "pair-confirm" => Ctrl::PairConfirm {
                h: hex(e["h_hex"].as_str().unwrap()).try_into().unwrap(),
            },
            "pair-done" => Ctrl::PairDone {
                desktop_name: e["desktop_name"].as_str().unwrap().into(),
                catalog_version: e["catalog_version"].as_u64().unwrap(),
            },
            "identity-rotate" => Ctrl::IdentityRotate {
                new_pub: key(&e["new_pub_hex"]),
                new_push_pub: e.get("new_push_pub_hex").filter(|v| !v.is_null()).map(key),
            },
            "input_lost" => Ctrl::InputLost {
                handles: e["handles"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h.as_u64().unwrap() as u32)
                    .collect(),
            },
            "input_lost_ack" => Ctrl::InputLostAck,
            other => panic!("unknown ctrl {other}"),
        })
    }

    #[test]
    fn vectors() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vectors/codec/ctrl.json");
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read ctrl.json")).unwrap();
        let vectors = file["vectors"].as_array().unwrap();
        let (mut ok, mut err) = (0, 0);
        for v in vectors {
            let name = v["name"].as_str().unwrap();
            let content = hex(v["content_hex"].as_str().unwrap());
            if let Some(utf8) = v.get("content_utf8").and_then(|u| u.as_str()) {
                assert_eq!(
                    utf8.as_bytes(),
                    content.as_slice(),
                    "{name}: utf8/hex disagree"
                );
            }
            if let Some(want) = v.get("expect_error").and_then(|e| e.as_str()) {
                let got = Ctrl::decode(&content).expect_err(name);
                let dbg = format!("{got:?}");
                assert!(dbg.starts_with(want), "{name}: want {want}, got {dbg}");
                err += 1;
                continue;
            }
            let e = &v["expect"];
            let got = Ctrl::decode(&content).unwrap_or_else(|x| panic!("{name}: {x:?}"));
            match expected(e) {
                None => assert_eq!(got, None, "{name}: part command passes through"),
                Some(want) => {
                    assert_eq!(got.as_ref(), Some(&want), "{name}");
                    assert_eq!(want.encode().unwrap(), content, "{name}: re-encode");
                }
            }
            ok += 1;
        }
        assert!(
            ok >= 15 && err >= 30,
            "vector set incomplete: {ok} ok, {err} err"
        );
    }

    #[test]
    fn encode_refuses_what_decode_would_reject() {
        for bad in [
            Ctrl::Bye {
                reason: String::new(),
            },
            Ctrl::Bye {
                reason: "x".repeat(65),
            },
            Ctrl::PairDone {
                desktop_name: String::new(),
                catalog_version: 1,
            },
            Ctrl::PairDone {
                desktop_name: "a".into(),
                catalog_version: 0,
            },
            Ctrl::PairDone {
                desktop_name: "a".into(),
                catalog_version: MAX_CATALOG_VERSION + 1,
            },
            Ctrl::InputLost {
                handles: vec![2, 1],
            },
            Ctrl::InputLost {
                handles: vec![1, 1],
            },
            Ctrl::Hello {
                resume: Some(HelloResume {
                    chan: ChannelId([0; 16]),
                    last_sent_ctr: Some(COUNTER_LIMIT),
                }),
            },
        ] {
            assert!(bad.encode().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn round_trips() {
        let all = [
            Ctrl::Hello { resume: None },
            Ctrl::Hello {
                resume: Some(HelloResume {
                    chan: ChannelId([0xab; 16]),
                    last_sent_ctr: None,
                }),
            },
            Ctrl::Bye {
                reason: "rekeyed".into(),
            },
            Ctrl::PairConfirm { h: [7; 32] },
            Ctrl::PairDone {
                desktop_name: "Studio \"Mac\" ✓".into(),
                catalog_version: MAX_CATALOG_VERSION,
            },
            Ctrl::IdentityRotate {
                new_pub: PublicKey([1; 32]),
                new_push_pub: None,
            },
            Ctrl::InputLost {
                handles: vec![0, 7, u32::MAX],
            },
            Ctrl::InputLostAck,
            Ctrl::ControlTaken {
                handles: vec![3, 9],
            },
        ];
        for c in all {
            let bytes = c.encode().unwrap();
            assert_eq!(Ctrl::decode(&bytes).unwrap(), Some(c));
        }
    }
}
