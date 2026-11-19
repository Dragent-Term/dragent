//! Handshake payloads: the JSON carried inside HS1 (§4.2, §6.2 step 5).
//!
//! - KK HS1 (replayable, no side effects): `{"v":1,"caps":[…]}` — the phone's
//!   capability list. `v` must be 1; `caps` is an array of strings.
//! - Pairing HS1: `{device_name, platform, push_pub}` — the device name (which
//!   never goes to the server, §6.2), the platform class and the phone's push
//!   key. `platform` is `ios` or `android`; `push_pub` is the 32-byte X25519
//!   push public key as base64url (no padding).
//!
//! Choices made here: unknown fields are rejected (the handshake is versioned
//! by the §5 prologue, and strictness catches a relay swapping payloads);
//! `device_name` is nonempty and at most 128 chars; each cap is at most
//! 128 chars. `pair-confirm {h}` and `pair-done {desktop_name,
//! catalog_version}` are CTRL records owned by the channel users (E3a, E5a),
//! not parsed here — but see [`crate::codec::part::split_ctrl`] for framing.

use crate::codec::b64;

/// The phone's KK HS1 hello: version plus capability list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Capability list (may be empty).
    pub caps: Vec<String>,
}

/// The pairing HS1 payload (§6.2 step 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingHello {
    /// The phone's device name (never sent to the server).
    pub device_name: String,
    /// `ios` or `android`.
    pub platform: Platform,
    /// The phone's 32-byte push public key.
    pub push_pub: [u8; 32],
}

/// Phone platform class (§6.2 step 4: the redeem sends only this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// iOS.
    Ios,
    /// Android.
    Android,
}

impl Platform {
    fn name(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
        }
    }

    fn parse(s: &str) -> Option<Platform> {
        match s {
            "ios" => Some(Platform::Ios),
            "android" => Some(Platform::Android),
            _ => None,
        }
    }
}

/// Why a handshake payload was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HsError {
    /// Not the expected JSON shape (or not JSON, or unknown fields).
    NotJson,
    /// `v` is not 1 (KK hello only).
    BadVersion,
    /// `device_name` empty or over 128 chars, or a cap over 128 chars.
    BadString,
    /// `platform` is not `ios` or `android`.
    BadPlatform {
        /// The platform seen.
        got: String,
    },
    /// `push_pub` is not 32 bytes of base64url.
    BadPushKey,
}

impl std::fmt::Display for HsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HsError::NotJson => f.write_str("handshake payload is not the expected JSON"),
            HsError::BadVersion => f.write_str("handshake payload v is not 1"),
            HsError::BadString => f.write_str("bad handshake string field"),
            HsError::BadPlatform { got } => write!(f, "bad platform {got:?}"),
            HsError::BadPushKey => f.write_str("bad push public key"),
        }
    }
}

impl std::error::Error for HsError {}

/// Longest `device_name` or single cap in bytes (chosen here).
pub const MAX_STR_LEN: usize = 128;

/// Encode a KK HS1 hello.
pub fn encode_hello(hello: &Hello) -> Result<Vec<u8>, HsError> {
    for cap in &hello.caps {
        if cap.is_empty() || cap.len() > MAX_STR_LEN {
            return Err(HsError::BadString);
        }
    }
    let mut json = String::from("{\"v\":1,\"caps\":[");
    for (i, cap) in hello.caps.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&serde_json::to_string(cap).expect("cap serializes"));
    }
    json.push_str("]}");
    Ok(json.into_bytes())
}

/// Decode and validate a KK HS1 hello.
pub fn decode_hello(bytes: &[u8]) -> Result<Hello, HsError> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| HsError::NotJson)?;
    let o = v.as_object().ok_or(HsError::NotJson)?;
    if o.len() != 2 || o.get("v").and_then(|v| v.as_u64()) != Some(1) {
        if o.get("v").and_then(|v| v.as_u64()) != Some(1) {
            return Err(HsError::BadVersion);
        }
        return Err(HsError::NotJson);
    }
    let caps_json = o.get("caps").and_then(|v| v.as_array()).ok_or(HsError::NotJson)?;
    let mut caps = Vec::with_capacity(caps_json.len());
    for c in caps_json {
        let s = c.as_str().ok_or(HsError::NotJson)?;
        if s.is_empty() || s.len() > MAX_STR_LEN {
            return Err(HsError::BadString);
        }
        caps.push(s.to_string());
    }
    Ok(Hello { caps })
}

/// Encode a pairing HS1 payload.
pub fn encode_pairing(p: &PairingHello) -> Result<Vec<u8>, HsError> {
    if p.device_name.is_empty() || p.device_name.len() > MAX_STR_LEN {
        return Err(HsError::BadString);
    }
    Ok(format!(
        "{{\"device_name\":{},\"platform\":\"{}\",\"push_pub\":\"{}\"}}",
        serde_json::to_string(&p.device_name).expect("device name serializes"),
        p.platform.name(),
        b64::encode(&p.push_pub)
    )
    .into_bytes())
}

/// Decode and validate a pairing HS1 payload.
pub fn decode_pairing(bytes: &[u8]) -> Result<PairingHello, HsError> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| HsError::NotJson)?;
    let o = v.as_object().ok_or(HsError::NotJson)?;
    if o.len() != 3 || !o.contains_key("device_name") || !o.contains_key("platform") || !o.contains_key("push_pub") {
        return Err(HsError::NotJson);
    }
    let device_name = o["device_name"].as_str().ok_or(HsError::NotJson)?;
    if device_name.is_empty() || device_name.len() > MAX_STR_LEN {
        return Err(HsError::BadString);
    }
    let platform = o["platform"].as_str().ok_or(HsError::NotJson)?;
    let platform =
        Platform::parse(platform).ok_or_else(|| HsError::BadPlatform { got: platform.to_string() })?;
    let push_b64 = o["push_pub"].as_str().ok_or(HsError::NotJson)?;
    let push = b64::decode(push_b64).map_err(|_| HsError::BadPushKey)?;
    if push.len() != 32 {
        return Err(HsError::BadPushKey);
    }
    let mut push_pub = [0u8; 32];
    push_pub.copy_from_slice(&push);
    Ok(PairingHello { device_name: device_name.to_string(), platform, push_pub })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trip() {
        for caps in [vec![], vec!["term".to_string(), "files".to_string()]] {
            let h = Hello { caps };
            let back = decode_hello(&encode_hello(&h).unwrap()).unwrap();
            assert_eq!(back, h);
        }
        // The exact bytes the channel state machine sends today.
        assert_eq!(encode_hello(&Hello { caps: vec![] }).unwrap(), b"{\"v\":1,\"caps\":[]}");
    }

    #[test]
    fn pairing_round_trip() {
        let p = PairingHello {
            device_name: "Jahsmine's iPhone".to_string(),
            platform: Platform::Ios,
            push_pub: [11u8; 32],
        };
        let back = decode_pairing(&encode_pairing(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn rejections() {
        assert_eq!(decode_hello(b"{\"v\":2,\"caps\":[]}").unwrap_err(), HsError::BadVersion);
        // Trailing garbage after the JSON is rejected (trailing JSON
        // whitespace is canonical and accepted).
        assert!(decode_hello(b"{\"v\":1,\"caps\":[]} ").is_ok());
        assert_eq!(
            decode_hello(b"{\"v\":1,\"caps\":[]}xxx").unwrap_err(),
            HsError::NotJson
        );
        assert_eq!(decode_hello(b"{\"v\":1,\"caps\":[],\"x\":1}").unwrap_err(), HsError::NotJson);
        assert_eq!(
            decode_hello(b"{\"v\":1,\"caps\":[\"\"]}").unwrap_err(),
            HsError::BadString
        );
        let bad_platform = encode_pairing(&PairingHello {
            device_name: "P".to_string(),
            platform: Platform::Android,
            push_pub: [0u8; 32],
        })
        .unwrap();
        let bad_platform = String::from_utf8(bad_platform)
            .unwrap()
            .replace("android", "web");
        assert!(matches!(
            decode_pairing(bad_platform.as_bytes()),
            Err(HsError::BadPlatform { .. })
        ));
        let good = encode_pairing(&PairingHello {
            device_name: "P".to_string(),
            platform: Platform::Ios,
            push_pub: [0u8; 32],
        })
        .unwrap();
        let good = String::from_utf8(good).unwrap();
        // Short but valid base64url: decodes to 1 byte, not a 32-byte key.
        let short = good.replace("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "AA");
        assert_ne!(short, good);
        assert_eq!(decode_pairing(short.as_bytes()).unwrap_err(), HsError::BadPushKey);
        assert_eq!(
            encode_pairing(&PairingHello {
                device_name: String::new(),
                platform: Platform::Ios,
                push_pub: [0u8; 32],
            })
            .unwrap_err(),
            HsError::BadString
        );
    }
}
