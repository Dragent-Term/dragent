//! §4.5 large payloads: the bounded part protocol.
//!
//! A logical payload over the 1 MiB message cap is never sent as one message:
//!
//! 1. CTRL `part-begin {part_id, purpose, request_id?, stream_handle?,
//!    total_bytes, part_count, sha256}`;
//! 2. `part_count` JSON messages, each ≤ 1 MiB, of the form `{part_id, index,
//!    bytes: <base64 slice>}`;
//! 3. CTRL `part-end {part_id}`.
//!
//! The receiver buffers up to the purpose cap, checks `sha256` at `part-end`,
//! and applies the payload atomically: a partial payload is never rendered or
//! returned as success. Caps (§4.5, enforced at both ends):
//!
//! | Purpose | Cap |
//! |---|----|
//! | terminal seed | 16 MiB |
//! | screen grid | 4 MiB |
//! | RPC reply | 1 MiB |
//!
//! Choices made here:
//! - CTRL framing: the record content is the ASCII command word, one space,
//!   then compact JSON (`part-begin {"part_id":…}`). [`split_ctrl`] parses any
//!   such CTRL record; it is shared so every CTRL command frames identically.
//! - `part_id` is an opaque string: nonempty, at most 128 bytes. It only needs
//!   to be unique per sender while the part is open.
//! - `request_id` is a string or a u64-range number (JSON-RPC ids); anything
//!   else is rejected.
//! - `sha256` is 64 lowercase hex digits over the concatenated decoded chunks.
//! - Every chunk carries at least 1 byte, so `part_count ≤ total_bytes`; each
//!   decoded chunk is at most 1 MiB (the JSON message cap bounds the encoded
//!   form, and base64 only shrinks it).
//! - Base64 is base64url without padding (see [`crate::codec::b64`]).

use sha2::{Digest, Sha256};

use crate::codec::b64;

/// Purpose caps in bytes (§4.5).
pub const SEED_CAP: usize = 16 * 1024 * 1024;
/// Screen grid cap in bytes (§4.5).
pub const SCREEN_CAP: usize = 4 * 1024 * 1024;
/// RPC reply cap in bytes (§4.5).
pub const REPLY_CAP: usize = 1024 * 1024;
/// Largest decoded chunk: one JSON message is at most 1 MiB (§4.5).
pub const MAX_CHUNK_DECODED: usize = 1024 * 1024;
/// Longest `part_id` in bytes (chosen here).
pub const MAX_PART_ID_LEN: usize = 128;

/// What the part carries (and therefore its cap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Terminal seed (scrollback + screen): 16 MiB.
    Seed,
    /// Screen grid: 4 MiB.
    Screen,
    /// A large RPC reply: 1 MiB.
    Reply,
}

impl Purpose {
    /// The cap in bytes.
    pub fn cap(self) -> usize {
        match self {
            Purpose::Seed => SEED_CAP,
            Purpose::Screen => SCREEN_CAP,
            Purpose::Reply => REPLY_CAP,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Purpose::Seed => "seed",
            Purpose::Screen => "screen",
            Purpose::Reply => "reply",
        }
    }

    fn parse(s: &str) -> Option<Purpose> {
        match s {
            "seed" => Some(Purpose::Seed),
            "screen" => Some(Purpose::Screen),
            "reply" => Some(Purpose::Reply),
            _ => None,
        }
    }
}

/// Split a CTRL record content into its command word and JSON body:
/// `<name> <json>`. Used by every CTRL command so they all frame identically.
pub fn split_ctrl(content: &[u8]) -> Result<(&str, &str), PartError> {
    let text = std::str::from_utf8(content).map_err(|_| PartError::NotJson)?;
    let (name, json) = text.split_once(' ').ok_or(PartError::BadCtrlFraming)?;
    if name.is_empty() || json.is_empty() {
        return Err(PartError::BadCtrlFraming);
    }
    Ok((name, json))
}

/// Encode a CTRL record content: `<name> <json>`.
pub fn encode_ctrl(name: &str, json: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 1 + json.len());
    out.extend_from_slice(name.as_bytes());
    out.push(b' ');
    out.extend_from_slice(json.as_bytes());
    out
}

/// Why a part message was rejected. Every variant is a protocol error for the
/// channel: the payload fails atomically (`seed_failed` / `reply_failed`), and
/// a repeated failure never triggers a reconnect loop (§4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartError {
    /// Not `<name> <json>` framing, or not UTF-8.
    BadCtrlFraming,
    /// The JSON body does not parse, has the wrong shape, or (for typed
    /// messages) an unknown field.
    NotJson,
    /// Wrong CTRL command word for this message.
    WrongCommand {
        /// The command word seen.
        got: String,
    },
    /// `part_id` empty or over 128 bytes.
    BadPartId,
    /// Unknown purpose (want `seed`, `screen` or `reply`).
    BadPurpose {
        /// The purpose seen.
        got: String,
    },
    /// `total_bytes` is 0 or over the purpose cap.
    BadTotal {
        /// The total seen.
        total: u64,
        /// The cap for the purpose.
        cap: usize,
    },
    /// `part_count` is 0 or over `total_bytes`.
    BadPartCount {
        /// The count seen.
        count: u64,
    },
    /// `sha256` is not 64 lowercase hex digits.
    BadHash {
        /// The hash seen.
        got: String,
    },
    /// `request_id` is neither a string nor a number.
    BadRequestId,
    /// A chunk index is at or past `part_count`.
    BadIndex {
        /// The index seen.
        index: u64,
        /// The part count.
        count: u32,
    },
    /// A chunk decodes to 0 bytes or over 1 MiB.
    BadChunkSize {
        /// The decoded length.
        len: usize,
    },
    /// A chunk fails base64url decoding.
    BadChunkEncoding,
    /// A chunk repeats an index that already arrived.
    DuplicateChunk {
        /// The index.
        index: u32,
    },
    /// `part-end` arrived with chunks missing.
    MissingChunks {
        /// How many are still missing.
        missing: u32,
    },
    /// The concatenated chunks do not match `sha256`.
    HashMismatch,
    /// A chunk or `part-end` names a part that is not open (or another part).
    UnknownPart {
        /// The part id seen.
        got: String,
    },
}

impl std::fmt::Display for PartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartError::BadCtrlFraming => f.write_str("bad CTRL framing"),
            PartError::NotJson => f.write_str("part message is not the expected JSON"),
            PartError::WrongCommand { got } => write!(f, "wrong CTRL command {got:?}"),
            PartError::BadPartId => f.write_str("bad part id"),
            PartError::BadPurpose { got } => write!(f, "bad part purpose {got:?}"),
            PartError::BadTotal { total, cap } => {
                write!(f, "part total {total} outside 1..={cap}")
            }
            PartError::BadPartCount { count } => write!(f, "bad part count {count}"),
            PartError::BadHash { .. } => f.write_str("bad part sha256"),
            PartError::BadRequestId => f.write_str("bad part request id"),
            PartError::BadIndex { index, count } => {
                write!(f, "part index {index} outside 0..{count}")
            }
            PartError::BadChunkSize { len } => write!(f, "bad part chunk size {len}"),
            PartError::BadChunkEncoding => f.write_str("part chunk is not base64url"),
            PartError::DuplicateChunk { index } => write!(f, "duplicate part chunk {index}"),
            PartError::MissingChunks { missing } => write!(f, "{missing} part chunks missing"),
            PartError::HashMismatch => f.write_str("part sha256 mismatch"),
            PartError::UnknownPart { .. } => f.write_str("part id is not open"),
        }
    }
}

impl std::error::Error for PartError {}

fn check_part_id(id: &str) -> Result<(), PartError> {
    if id.is_empty() || id.len() > MAX_PART_ID_LEN {
        return Err(PartError::BadPartId);
    }
    Ok(())
}

fn parse_hash(hex: &str) -> Result<[u8; 32], PartError> {
    if hex.len() != 64 || !hex.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
        return Err(PartError::BadHash { got: hex.to_string() });
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = (hex_val(chunk[0]) << 4) | hex_val(chunk[1]);
    }
    Ok(out)
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => 0,
    }
}

fn hash_hex(sum: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(64);
    for &b in sum {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0xf) as usize] as char);
    }
    s
}

/// A validated `part-begin`: what the sender promised and the cap it must fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartBegin {
    /// Opaque id, unique per sender while open.
    pub part_id: String,
    /// What the part carries.
    pub purpose: Purpose,
    /// The RPC request this replies to (replies only).
    pub request_id: Option<RequestId>,
    /// The terminal stream this seeds (seeds only).
    pub stream_handle: Option<u32>,
    /// Total decoded bytes across all chunks (1..= cap).
    pub total_bytes: u64,
    /// How many chunks follow (1..= total_bytes).
    pub part_count: u32,
    /// SHA-256 over the concatenated decoded chunks.
    pub sha256: [u8; 32],
}

/// A JSON-RPC request id: a string or a number (§4.5 `request_id?`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestId {
    /// A string id.
    Str(String),
    /// A numeric id.
    Num(u64),
}

/// Decode and validate a CTRL `part-begin` record content.
pub fn decode_begin(content: &[u8]) -> Result<PartBegin, PartError> {
    let (name, json) = split_ctrl(content)?;
    if name != "part-begin" {
        return Err(PartError::WrongCommand { got: name.to_string() });
    }
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| PartError::NotJson)?;
    let o = v.as_object().ok_or(PartError::NotJson)?;
    if o.keys().any(|k| {
        !matches!(
            k.as_str(),
            "part_id" | "purpose" | "request_id" | "stream_handle" | "total_bytes" | "part_count" | "sha256"
        )
    }) {
        return Err(PartError::NotJson);
    }
    let part_id = o.get("part_id").and_then(|v| v.as_str()).ok_or(PartError::NotJson)?;
    check_part_id(part_id)?;
    let purpose = o.get("purpose").and_then(|v| v.as_str()).ok_or(PartError::NotJson)?;
    let purpose = Purpose::parse(purpose)
        .ok_or_else(|| PartError::BadPurpose { got: purpose.to_string() })?;
    let request_id = match o.get("request_id") {
        None => None,
        Some(serde_json::Value::String(s)) => Some(RequestId::Str(s.clone())),
        Some(serde_json::Value::Number(n)) => {
            Some(RequestId::Num(n.as_u64().ok_or(PartError::BadRequestId)?))
        }
        Some(_) => return Err(PartError::BadRequestId),
    };
    let stream_handle = match o.get("stream_handle") {
        None => None,
        Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(PartError::NotJson)?),
    };
    let total_bytes = o.get("total_bytes").and_then(|v| v.as_u64()).ok_or(PartError::NotJson)?;
    if total_bytes == 0 || total_bytes > purpose.cap() as u64 {
        return Err(PartError::BadTotal { total: total_bytes, cap: purpose.cap() });
    }
    let part_count = o.get("part_count").and_then(|v| v.as_u64()).ok_or(PartError::NotJson)?;
    if part_count == 0 || part_count > total_bytes {
        return Err(PartError::BadPartCount { count: part_count });
    }
    let sha = o.get("sha256").and_then(|v| v.as_str()).ok_or(PartError::NotJson)?;
    let sha256 = parse_hash(sha)?;
    Ok(PartBegin {
        part_id: part_id.to_string(),
        purpose,
        request_id,
        stream_handle,
        total_bytes,
        part_count: part_count as u32,
        sha256,
    })
}

/// Encode a CTRL `part-begin` record content.
pub fn encode_begin(b: &PartBegin) -> Result<Vec<u8>, PartError> {
    check_part_id(&b.part_id)?;
    if b.total_bytes == 0 || b.total_bytes > b.purpose.cap() as u64 {
        return Err(PartError::BadTotal { total: b.total_bytes, cap: b.purpose.cap() });
    }
    if b.part_count == 0 || u64::from(b.part_count) > b.total_bytes {
        return Err(PartError::BadPartCount { count: u64::from(b.part_count) });
    }
    let mut json = format!(
        "{{\"part_id\":{},\"purpose\":\"{}\"",
        serde_json::to_string(&b.part_id).expect("part id serializes"),
        b.purpose.name(),
    );
    if let Some(id) = &b.request_id {
        let id_json = match id {
            RequestId::Str(s) => serde_json::to_string(s).expect("request id serializes"),
            RequestId::Num(n) => n.to_string(),
        };
        json.push_str(&format!(",\"request_id\":{id_json}"));
    }
    if let Some(h) = b.stream_handle {
        json.push_str(&format!(",\"stream_handle\":{h}"));
    }
    json.push_str(&format!(
        ",\"total_bytes\":{},\"part_count\":{},\"sha256\":\"{}\"}}",
        b.total_bytes,
        b.part_count,
        hash_hex(&b.sha256)
    ));
    Ok(encode_ctrl("part-begin", &json))
}

/// A validated part chunk: `{part_id, index, bytes}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartChunk {
    /// Which part this belongs to.
    pub part_id: String,
    /// Chunk index (0..part_count).
    pub index: u32,
    /// Decoded bytes (1..= 1 MiB).
    pub bytes: Vec<u8>,
}

/// Decode and validate a JSON part-chunk message body.
pub fn decode_chunk(json: &[u8]) -> Result<PartChunk, PartError> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|_| PartError::NotJson)?;
    let o = v.as_object().ok_or(PartError::NotJson)?;
    if o.keys().any(|k| !matches!(k.as_str(), "part_id" | "index" | "bytes")) {
        return Err(PartError::NotJson);
    }
    let part_id = o.get("part_id").and_then(|v| v.as_str()).ok_or(PartError::NotJson)?;
    check_part_id(part_id)?;
    let index = o.get("index").and_then(|v| v.as_u64()).ok_or(PartError::NotJson)?;
    let index = u32::try_from(index).map_err(|_| PartError::NotJson)?;
    let enc = o.get("bytes").and_then(|v| v.as_str()).ok_or(PartError::NotJson)?;
    let bytes = b64::decode(enc).map_err(|_| PartError::BadChunkEncoding)?;
    if bytes.is_empty() || bytes.len() > MAX_CHUNK_DECODED {
        return Err(PartError::BadChunkSize { len: bytes.len() });
    }
    Ok(PartChunk { part_id: part_id.to_string(), index, bytes })
}

/// Encode a JSON part-chunk message body.
pub fn encode_chunk(part_id: &str, index: u32, bytes: &[u8]) -> Result<Vec<u8>, PartError> {
    check_part_id(part_id)?;
    if bytes.is_empty() || bytes.len() > MAX_CHUNK_DECODED {
        return Err(PartError::BadChunkSize { len: bytes.len() });
    }
    Ok(format!(
        "{{\"part_id\":{},\"index\":{},\"bytes\":\"{}\"}}",
        serde_json::to_string(part_id).expect("part id serializes"),
        index,
        b64::encode(bytes)
    )
    .into_bytes())
}

/// Decode and validate a CTRL `part-end` record content; returns the part id.
pub fn decode_end(content: &[u8]) -> Result<String, PartError> {
    let (name, json) = split_ctrl(content)?;
    if name != "part-end" {
        return Err(PartError::WrongCommand { got: name.to_string() });
    }
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| PartError::NotJson)?;
    let o = v.as_object().ok_or(PartError::NotJson)?;
    if o.len() != 1 || !o.contains_key("part_id") {
        return Err(PartError::NotJson);
    }
    let part_id = o["part_id"].as_str().ok_or(PartError::NotJson)?;
    check_part_id(part_id)?;
    Ok(part_id.to_string())
}

/// Encode a CTRL `part-end` record content.
pub fn encode_end(part_id: &str) -> Result<Vec<u8>, PartError> {
    check_part_id(part_id)?;
    Ok(encode_ctrl(
        "part-end",
        &format!("{{\"part_id\":{}}}", serde_json::to_string(part_id).expect("serializes")),
    ))
}

/// One open part on the receiving side: buffers chunks up to the purpose cap
/// and releases the payload atomically at `part-end` after the sha256 check.
/// A hash mismatch, a cap overrun or an abort fails the payload; the caller
/// shows `seed_failed` / `reply_failed` with one manual retry (§4.5).
#[derive(Debug)]
pub struct PartSession {
    begin: PartBegin,
    chunks: Vec<Option<Vec<u8>>>,
    buffered: u64,
    finished: bool,
}

impl PartSession {
    /// Open a part from a validated `part-begin`. The cap was already checked
    /// at decode; this only allocates the index table.
    pub fn open(begin: PartBegin) -> Self {
        let count = begin.part_count as usize;
        let mut chunks = Vec::with_capacity(count);
        chunks.resize_with(count, || None);
        PartSession { begin, chunks, buffered: 0, finished: false }
    }

    /// The part this session holds.
    pub fn part_id(&self) -> &str {
        &self.begin.part_id
    }

    /// Feed one validated chunk. Chunks may arrive in any order, each index
    /// exactly once.
    pub fn feed(&mut self, chunk: PartChunk) -> Result<(), PartError> {
        if self.finished || chunk.part_id != self.begin.part_id {
            return Err(PartError::UnknownPart { got: chunk.part_id });
        }
        if chunk.index >= self.begin.part_count {
            return Err(PartError::BadIndex {
                index: u64::from(chunk.index),
                count: self.begin.part_count,
            });
        }
        let slot = &mut self.chunks[chunk.index as usize];
        if slot.is_some() {
            return Err(PartError::DuplicateChunk { index: chunk.index });
        }
        self.buffered += chunk.bytes.len() as u64;
        if self.buffered > self.begin.total_bytes {
            return Err(PartError::BadTotal {
                total: self.buffered,
                cap: self.begin.purpose.cap(),
            });
        }
        *slot = Some(chunk.bytes);
        Ok(())
    }

    /// Close the part: every chunk must be present and the sha256 must match.
    /// Returns the payload bytes on success; the session is spent either way.
    pub fn end(mut self, part_id: &str) -> Result<Vec<u8>, PartError> {
        if self.finished || part_id != self.begin.part_id {
            return Err(PartError::UnknownPart { got: part_id.to_string() });
        }
        self.finished = true;
        let mut out = Vec::with_capacity(self.begin.total_bytes as usize);
        for slot in &self.chunks {
            match slot {
                Some(bytes) => out.extend_from_slice(bytes),
                None => {
                    let missing =
                        self.chunks.iter().filter(|s| s.is_none()).count() as u32;
                    return Err(PartError::MissingChunks { missing });
                }
            }
        }
        if out.len() as u64 != self.begin.total_bytes {
            return Err(PartError::HashMismatch);
        }
        let sum = Sha256::digest(&out);
        if sum.as_slice() != self.begin.sha256 {
            return Err(PartError::HashMismatch);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin_fixture() -> PartBegin {
        let payload = vec![9u8; 100];
        let mut sum = [0u8; 32];
        sum.copy_from_slice(&Sha256::digest(&payload));
        PartBegin {
            part_id: "seed-7".to_string(),
            purpose: Purpose::Seed,
            request_id: None,
            stream_handle: Some(3),
            total_bytes: 100,
            part_count: 2,
            sha256: sum,
        }
    }

    #[test]
    fn begin_end_chunk_round_trip() {
        let b = begin_fixture();
        let enc = encode_begin(&b).unwrap();
        assert!(enc.starts_with(b"part-begin "));
        let back = decode_begin(&enc).unwrap();
        assert_eq!(back, b);

        // Optional request ids (string and numeric) survive the round trip.
        for id in [RequestId::Str("rpc-9".to_string()), RequestId::Num(42)] {
            let mut with_id = b.clone();
            with_id.request_id = Some(id.clone());
            with_id.stream_handle = None;
            let back = decode_begin(&encode_begin(&with_id).unwrap()).unwrap();
            assert_eq!(back.request_id, Some(id));
            assert_eq!(back.stream_handle, None);
        }

        let c0 = encode_chunk("seed-7", 0, &[9u8; 60]).unwrap();
        let c1 = encode_chunk("seed-7", 1, &[9u8; 40]).unwrap();
        // Chunks may arrive in any order, each index exactly once.
        let mut session = PartSession::open(back);
        session.feed(decode_chunk(&c1).unwrap()).unwrap();
        session.feed(decode_chunk(&c0).unwrap()).unwrap();
        let payload = session.end("seed-7").unwrap();
        assert_eq!(payload, vec![9u8; 100]);
    }

    #[test]
    fn ctrl_framing() {
        assert_eq!(split_ctrl(b"part-end {\"a\":1}").unwrap(), ("part-end", "{\"a\":1}"));
        assert_eq!(split_ctrl(b"nospaces").unwrap_err(), PartError::BadCtrlFraming);
        assert_eq!(split_ctrl(b"").unwrap_err(), PartError::BadCtrlFraming);
        assert_eq!(
            decode_end(&encode_end("p1").unwrap()).unwrap(),
            "p1".to_string()
        );
        assert!(matches!(
            decode_end(b"part-begin {\"part_id\":\"p1\"}"),
            Err(PartError::WrongCommand { .. })
        ));
    }

    #[test]
    fn begin_rejections() {
        let mut bad = begin_fixture();
        bad.total_bytes = 0;
        assert!(matches!(
            encode_begin(&bad),
            Err(PartError::BadTotal { .. })
        ));
        bad = begin_fixture();
        bad.total_bytes = SEED_CAP as u64 + 1;
        assert!(matches!(encode_begin(&bad), Err(PartError::BadTotal { .. })));
        bad = begin_fixture();
        bad.part_count = 0;
        assert!(matches!(encode_begin(&bad), Err(PartError::BadPartCount { .. })));
        bad = begin_fixture();
        bad.part_count = 101;
        assert!(matches!(encode_begin(&bad), Err(PartError::BadPartCount { .. })));
        // Unknown purpose, bad hash, bad request id, unknown fields.
        let raw = b"part-begin {\"part_id\":\"p\",\"purpose\":\"bulk\",\"total_bytes\":10,\"part_count\":2,\"sha256\":\"00\"}";
        assert!(matches!(decode_begin(raw), Err(PartError::BadPurpose { .. })));
        let raw = b"part-begin {\"part_id\":\"p\",\"purpose\":\"reply\",\"total_bytes\":10,\"part_count\":2,\"sha256\":\"ZZ\"}";
        assert!(matches!(decode_begin(raw), Err(PartError::BadHash { .. })));
        let raw = b"part-begin {\"part_id\":\"p\",\"purpose\":\"reply\",\"request_id\":[],\"total_bytes\":10,\"part_count\":2,\"sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}";
        assert_eq!(decode_begin(raw).unwrap_err(), PartError::BadRequestId);
        let raw = b"part-begin {\"part_id\":\"p\",\"purpose\":\"reply\",\"extra\":1,\"total_bytes\":10,\"part_count\":2,\"sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}";
        assert_eq!(decode_begin(raw).unwrap_err(), PartError::NotJson);
        // Reply cap is 1 MiB.
        let raw = b"part-begin {\"part_id\":\"p\",\"purpose\":\"reply\",\"total_bytes\":2097152,\"part_count\":4,\"sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}";
        assert!(matches!(decode_begin(raw), Err(PartError::BadTotal { .. })));
    }

    #[test]
    fn session_failures_are_atomic() {
        // Trailing garbage after the JSON is rejected (no silent truncation
        // of the framing), while trailing JSON whitespace is canonical.
        let mut noisy = encode_begin(&begin_fixture()).unwrap();
        noisy.extend_from_slice(b" }");
        assert_eq!(decode_begin(&noisy).unwrap_err(), PartError::NotJson);
        let mut spaced = encode_begin(&begin_fixture()).unwrap();
        spaced.extend_from_slice(b"  ");
        assert_eq!(decode_begin(&spaced).unwrap(), begin_fixture());        // Hash mismatch: nothing is returned, and the payload is unusable.
        let mut b = begin_fixture();
        b.sha256 = [1u8; 32];
        let mut session = PartSession::open(b);
        session.feed(decode_chunk(&encode_chunk("seed-7", 0, &[9u8; 60]).unwrap()).unwrap()).unwrap();
        session.feed(decode_chunk(&encode_chunk("seed-7", 1, &[9u8; 40]).unwrap()).unwrap()).unwrap();
        assert_eq!(session.end("seed-7").unwrap_err(), PartError::HashMismatch);
        // Duplicate chunk, wrong part, missing chunk at end.
        let mut session = PartSession::open(begin_fixture());
        let c0 = decode_chunk(&encode_chunk("seed-7", 0, &[9u8; 60]).unwrap()).unwrap();
        session.feed(c0.clone()).unwrap();
        assert_eq!(
            session.feed(c0).unwrap_err(),
            PartError::DuplicateChunk { index: 0 }
        );
        assert!(matches!(
            session.feed(PartChunk {
                part_id: "other".to_string(),
                index: 0,
                bytes: vec![1]
            }),
            Err(PartError::UnknownPart { .. })
        ));
        assert_eq!(
            session.end("seed-7").unwrap_err(),
            PartError::MissingChunks { missing: 1 }
        );
        // Index past the count, empty chunk.
        assert_eq!(
            decode_chunk(&encode_chunk("seed-7", 2, b"x").unwrap()).map(|c| {
                let mut s = PartSession::open(begin_fixture());
                s.feed(c)
            }),
            Ok(Err(PartError::BadIndex { index: 2, count: 2 }))
        );
        assert_eq!(
            encode_chunk("seed-7", 0, b"").unwrap_err(),
            PartError::BadChunkSize { len: 0 }
        );
    }
}
