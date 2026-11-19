//! Wire codecs for the phone transport (E5a): the §4.1 frame header, §4.4
//! records and reassembly, TERM, CTRL framing and the §4.5 part protocol.
//! Thin wrappers over `dragent_e2ee::codec`; every decoder is total.

use std::sync::{Arc, Mutex};

use dragent_e2ee::codec::{header, part, record, term};

use crate::{lock, parse_chan, parse_id, E2eeError};

/// §4.1 frame kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FrameKind {
    /// Noise handshake message.
    Hs,
    /// Noise transport ciphertext.
    Data,
    /// Unauthenticated close hint.
    Close,
}

/// §4.1 channel purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FramePurpose {
    /// KK companion channel.
    Companion,
    /// IKpsk1 pairing.
    Pairing,
}

/// A decoded (or to-be-encoded) §4.1 frame.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Frame {
    /// HS, DATA or CLOSE.
    pub kind: FrameKind,
    /// HS step (1..=3); 0 otherwise.
    pub step: u8,
    /// HS only.
    pub purpose: Option<FramePurpose>,
    /// 16-byte channel id.
    pub chan: Vec<u8>,
    /// HS step 1 only: the target (sent) or initiator (received) device id.
    pub peer: Option<String>,
    /// DATA only.
    pub ctr: u64,
    /// The body.
    pub body: Vec<u8>,
}

fn kind_out(k: header::Kind) -> FrameKind {
    match k {
        header::Kind::Hs => FrameKind::Hs,
        header::Kind::Data => FrameKind::Data,
        header::Kind::Close => FrameKind::Close,
    }
}

fn purpose_in(p: FramePurpose) -> header::Purpose {
    match p {
        FramePurpose::Companion => header::Purpose::Companion,
        FramePurpose::Pairing => header::Purpose::Pairing,
    }
}

/// Decode one binary WebSocket message as a §4.1 frame (the body is the rest
/// of the message).
#[uniffi::export]
pub fn decode_frame(bytes: Vec<u8>) -> Result<Frame, E2eeError> {
    let (f, len) = header::decode(&bytes).map_err(E2eeError::malformed)?;
    if len != bytes.len() {
        return Err(E2eeError::malformed("trailing bytes after frame"));
    }
    Ok(Frame {
        kind: kind_out(f.kind),
        step: f.step,
        purpose: f.purpose.map(|p| match p {
            header::Purpose::Companion => FramePurpose::Companion,
            header::Purpose::Pairing => FramePurpose::Pairing,
        }),
        chan: f.chan.as_bytes().to_vec(),
        peer: f.peer.map(|p| p.to_string()),
        ctr: f.ctr,
        body: f.body.to_vec(),
    })
}

/// Encode a §4.1 frame (header checks included).
#[uniffi::export]
pub fn encode_frame(frame: Frame) -> Result<Vec<u8>, E2eeError> {
    let peer = frame
        .peer
        .as_deref()
        .map(|p| parse_id("peer", p))
        .transpose()?;
    let head = header::Head {
        kind: match frame.kind {
            FrameKind::Hs => header::Kind::Hs,
            FrameKind::Data => header::Kind::Data,
            FrameKind::Close => header::Kind::Close,
        },
        step: frame.step,
        purpose: frame.purpose.map(purpose_in),
        chan: parse_chan(&frame.chan)?,
        peer,
        ctr: frame.ctr,
        body: &frame.body,
    };
    let mut out = Vec::with_capacity(header::HEADER_LEN + frame.body.len());
    head.encode(&mut out).map_err(E2eeError::malformed)?;
    Ok(out)
}

/// An HS step 1 frame to `peer` (the target desktop).
#[uniffi::export]
pub fn encode_hs1_frame(
    purpose: FramePurpose,
    chan: Vec<u8>,
    peer: String,
    body: Vec<u8>,
) -> Result<Vec<u8>, E2eeError> {
    let head = header::Head::hs1(
        purpose_in(purpose),
        parse_chan(&chan)?,
        parse_id("peer", &peer)?,
        &body,
    );
    let mut out = Vec::with_capacity(header::HEADER_LEN + body.len());
    head.encode(&mut out).map_err(E2eeError::malformed)?;
    Ok(out)
}

/// §4.4 message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MessageType {
    /// JSON envelopes.
    Json,
    /// Terminal output.
    Term,
    /// Control messages.
    Ctrl,
}

fn mtype_in(m: MessageType) -> record::MsgType {
    match m {
        MessageType::Json => record::MsgType::Json,
        MessageType::Term => record::MsgType::Term,
        MessageType::Ctrl => record::MsgType::Ctrl,
    }
}

fn mtype_out(m: record::MsgType) -> MessageType {
    match m {
        record::MsgType::Json => MessageType::Json,
        record::MsgType::Term => MessageType::Term,
        record::MsgType::Ctrl => MessageType::Ctrl,
    }
}

/// Split one message into padded §4.4 records: full 65,519-byte records,
/// then the smallest bucket that fits the tail. Each is one DATA plaintext
/// (seal with [`crate::Channel::seal_frame`]). `msg_id` starts at 1 per
/// direction and rises by one per message.
#[uniffi::export]
pub fn encode_records(
    mtype: MessageType,
    msg_id: u32,
    content: Vec<u8>,
) -> Result<Vec<Vec<u8>>, E2eeError> {
    if content.len() > record::MAX_TOTAL_LEN {
        return Err(E2eeError::TooLarge);
    }
    let total = content.len() as u32;
    let mut out = Vec::new();
    let mut chunks: Vec<&[u8]> = content.chunks(record::MAX_RECORD_CONTENT).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    let n = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let first = i == 0;
        let last = i + 1 == n;
        let len = record::smallest_len(chunk.len()).ok_or(E2eeError::TooLarge)?;
        let mut rec = Vec::with_capacity(len);
        record::encode(
            &mut rec,
            len,
            first,
            last,
            false,
            first.then(|| mtype_in(mtype)),
            msg_id,
            if first { total } else { 0 },
            chunk,
        )
        .map_err(E2eeError::malformed)?;
        out.push(rec);
    }
    Ok(out)
}

/// A padding-only record of length `len` (one of the §4.4 sizes).
#[uniffi::export]
pub fn encode_pad_record(len: u32) -> Result<Vec<u8>, E2eeError> {
    let mut rec = Vec::with_capacity(len as usize);
    record::encode(&mut rec, len as usize, false, false, true, None, 0, 0, &[])
        .map_err(E2eeError::malformed)?;
    Ok(rec)
}

/// A complete message.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Message {
    /// Its type.
    pub mtype: MessageType,
    /// Its content bytes.
    pub content: Vec<u8>,
}

/// §4.4 receiver: decodes records and reassembles messages (≤ 4 partials,
/// ≤ 2 MiB buffered). Any error is a `protocol_error` for the channel.
#[derive(uniffi::Object)]
pub struct Reassembler {
    inner: Mutex<record::Reassembler>,
}

impl Default for Reassembler {
    fn default() -> Self {
        Reassembler {
            inner: Mutex::new(record::Reassembler::new()),
        }
    }
}

#[uniffi::export]
impl Reassembler {
    /// A fresh reassembler (one per channel and direction).
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Feed one decrypted record. Returns a message when one completes.
    pub fn feed(&self, record_bytes: Vec<u8>) -> Result<Option<Message>, E2eeError> {
        let rec = record::decode(&record_bytes).map_err(|e| E2eeError::ProtocolError {
            detail: format!("record: {e:?}"),
        })?;
        let msg = lock(&self.inner)
            .feed(&rec)
            .map_err(|e| E2eeError::ProtocolError {
                detail: format!("reassembly: {e:?}"),
            })?;
        Ok(msg.map(|m| Message {
            mtype: mtype_out(m.mtype),
            content: m.content,
        }))
    }

    /// Bytes buffered in partial messages.
    pub fn buffered(&self) -> u64 {
        lock(&self.inner).buffered() as u64
    }

    /// Drop every partial message (on `protocol_error` or channel close).
    pub fn clear(&self) {
        *lock(&self.inner) = record::Reassembler::new();
    }
}

/// A decoded TERM payload (§4.4).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TermOutput {
    /// Desktop-minted handle.
    pub handle: u32,
    /// Stream sequence number.
    pub seq: u64,
    /// First byte the phone still holds.
    pub base: u64,
    /// Total bytes written on the stream.
    pub total: u64,
    /// PTY alive.
    pub alive: bool,
    /// Stream reset.
    pub reset: bool,
    /// Exit code when ended with one.
    pub exit_code: Option<i32>,
    /// Rows, when reported.
    pub rows: Option<u16>,
    /// Columns, when reported.
    pub cols: Option<u16>,
    /// Raw VT bytes.
    pub data: Vec<u8>,
}

/// Decode a TERM message content.
#[uniffi::export]
pub fn decode_term(content: Vec<u8>) -> Result<TermOutput, E2eeError> {
    let t = term::decode(&content).map_err(E2eeError::malformed)?;
    Ok(TermOutput {
        handle: t.handle,
        seq: t.seq,
        base: t.base,
        total: t.total,
        alive: t.alive,
        reset: t.reset,
        exit_code: t.exit_code,
        rows: t.size.map(|s| s.0),
        cols: t.size.map(|s| s.1),
        data: t.data.to_vec(),
    })
}

/// A CTRL message split into its command and JSON body.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CtrlMessage {
    /// `hello`, `bye`, `pair-confirm`, …
    pub name: String,
    /// The JSON body text.
    pub json: String,
}

/// Split a CTRL message content (`<name> <json>`).
#[uniffi::export]
pub fn decode_ctrl(content: Vec<u8>) -> Result<CtrlMessage, E2eeError> {
    let (name, json) = part::split_ctrl(&content).map_err(E2eeError::malformed)?;
    Ok(CtrlMessage {
        name: name.to_string(),
        json: json.to_string(),
    })
}

/// Encode a CTRL message content (`<name> <json>`).
#[uniffi::export]
pub fn encode_ctrl(name: String, json: String) -> Result<Vec<u8>, E2eeError> {
    if name.is_empty() || name.contains(' ') || json.is_empty() {
        return Err(E2eeError::invalid(
            "ctrl name must be one word and json non-empty",
        ));
    }
    Ok(part::encode_ctrl(&name, &json))
}

/// §4.5 part purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PartPurpose {
    /// Terminal seed, 16 MiB cap.
    Seed,
    /// Screen grid, 4 MiB cap.
    Screen,
    /// RPC reply, 1 MiB cap.
    Reply,
}

/// What a validated `part-begin` promised.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PartInfo {
    /// The part id.
    pub part_id: String,
    /// What it carries.
    pub purpose: PartPurpose,
    /// Replies: the request id as JSON text (a string or a number).
    pub request_id_json: Option<String>,
    /// Seeds: the stream handle.
    pub stream_handle: Option<u32>,
    /// Total decoded bytes.
    pub total_bytes: u64,
    /// Chunk count.
    pub part_count: u32,
}

/// One §4.5 part being received: chunks in any order, each once; applied
/// atomically at `part-end` after the SHA-256 check.
#[derive(uniffi::Object)]
pub struct PartSession {
    inner: Mutex<Option<part::PartSession>>,
    info: PartInfo,
}

#[uniffi::export]
impl PartSession {
    /// Open from a CTRL `part-begin` message content (cap checked here).
    #[uniffi::constructor]
    pub fn begin(ctrl_content: Vec<u8>) -> Result<Arc<Self>, E2eeError> {
        let b = part::decode_begin(&ctrl_content).map_err(E2eeError::malformed)?;
        let info = PartInfo {
            part_id: b.part_id.clone(),
            purpose: match b.purpose {
                part::Purpose::Seed => PartPurpose::Seed,
                part::Purpose::Screen => PartPurpose::Screen,
                part::Purpose::Reply => PartPurpose::Reply,
            },
            request_id_json: b.request_id.as_ref().map(|r| match r {
                part::RequestId::Str(s) => serde_json::Value::String(s.clone()).to_string(),
                part::RequestId::Num(n) => n.to_string(),
            }),
            stream_handle: b.stream_handle,
            total_bytes: b.total_bytes,
            part_count: b.part_count,
        };
        Ok(Arc::new(PartSession {
            inner: Mutex::new(Some(part::PartSession::open(b))),
            info,
        }))
    }

    /// The `part-begin` it was opened with.
    pub fn info(&self) -> PartInfo {
        self.info.clone()
    }

    /// Feed one JSON chunk message content (`{part_id, index, bytes}`).
    pub fn feed(&self, chunk_json: Vec<u8>) -> Result<(), E2eeError> {
        let chunk = part::decode_chunk(&chunk_json).map_err(E2eeError::malformed)?;
        lock(&self.inner)
            .as_mut()
            .ok_or(E2eeError::Consumed)?
            .feed(chunk)
            .map_err(E2eeError::malformed)
    }

    /// CTRL `part-end`: all chunks present and the hash matches → the payload.
    /// The session is spent either way.
    pub fn end(&self, ctrl_content: Vec<u8>) -> Result<Vec<u8>, E2eeError> {
        let part_id = part::decode_end(&ctrl_content).map_err(E2eeError::malformed)?;
        let session = lock(&self.inner).take().ok_or(E2eeError::Consumed)?;
        session.end(&part_id).map_err(|e| E2eeError::Rejected {
            detail: format!("part: {e:?}"),
        })
    }

    /// `part-abort` or channel close: drop everything buffered.
    pub fn abort(&self) {
        lock(&self.inner).take();
    }
}
