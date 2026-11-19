//! Test-only harness for the phone transport tests (see Cargo.toml).
//!
//! `e2ee-transport-harness <request-fifo> <response-fifo>` reads one JSON
//! request per line and writes one JSON response per line:
//! `{"ok": <value>}` or `{"err": {"code": "E2EE_…", "message": "…"}}`.
//! Byte strings are lowercase hex in both directions.
//!
//! Phone-side ops (`phone.*`, `obj.*`) call E4's binding crate exactly as the
//! iOS/Android module does. Desktop-side ops (`desk.*`) are a scripted peer on
//! `dragent_e2ee`: the test decides every frame it sends.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;

use dragent_e2ee::channel::{kk_listen, Channel as DChannel, ChannelState, PinLookup};
use dragent_e2ee::channel::Inbound as DInbound;
use dragent_e2ee::codec::ctrl::Ctrl;
use dragent_e2ee::codec::{catalog_blob, header, hs, part, record, term};
use dragent_e2ee::noise::{CatalogPrologue, KkPrologue, PublicKey, StaticKeypair};
use dragent_e2ee::seal::catalog as cat;
use dragent_e2ee::{ChannelId, Id};
use dragent_e2ee_ffi as ffi;
use serde_json::{json, Value};

type R = Result<Value, (String, String)>;

fn err(code: &str, msg: impl Into<String>) -> (String, String) {
    (code.to_string(), msg.into())
}

fn ffi_err(e: ffi::E2eeError) -> (String, String) {
    (e.code().to_string(), format!("{e}"))
}

fn hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

fn unhex(s: &str) -> Result<Vec<u8>, (String, String)> {
    if s.len() % 2 != 0 {
        return Err(err("HARNESS", "odd hex"));
    }
    let v = |c: u8| match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(err("HARNESS", "bad hex")),
    };
    s.as_bytes()
        .chunks(2)
        .map(|p| Ok(v(p[0])? << 4 | v(p[1])?))
        .collect()
}

fn arg<'a>(req: &'a Value, k: &str) -> Result<&'a Value, (String, String)> {
    req.get(k).ok_or_else(|| err("HARNESS", format!("missing {k}")))
}
fn s(req: &Value, k: &str) -> Result<String, (String, String)> {
    arg(req, k)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| err("HARNESS", format!("{k} not a string")))
}
fn b(req: &Value, k: &str) -> Result<Vec<u8>, (String, String)> {
    unhex(&s(req, k)?)
}
fn n(req: &Value, k: &str) -> Result<u64, (String, String)> {
    arg(req, k)?
        .as_u64()
        .ok_or_else(|| err("HARNESS", format!("{k} not a u64")))
}
fn id(req: &Value, k: &str) -> Result<Id, (String, String)> {
    Id::parse(&s(req, k)?).map_err(|_| err("HARNESS", format!("{k} not an id")))
}
fn key(bytes: &[u8]) -> Result<PublicKey, (String, String)> {
    PublicKey::from_slice(bytes).ok_or_else(|| err("HARNESS", "bad key"))
}
fn chan_of(bytes: &[u8]) -> Result<ChannelId, (String, String)> {
    <[u8; 16]>::try_from(bytes)
        .map(ChannelId)
        .map_err(|_| err("HARNESS", "bad chan"))
}

enum Obj {
    Kk(Arc<ffi::KkHandshake>),
    Channel(Arc<ffi::Channel>),
    Reasm(Arc<ffi::Reassembler>),
    Part(Arc<ffi::PartSession>),
    Store(Arc<ffi::PinStore>),
}

struct DeskChan {
    channel: DChannel,
    phone: Id,
    reasm: record::Reassembler,
    next_msg: u32,
}

struct Desk {
    keys: StaticKeypair,
    account: Id,
    desktop: Id,
    pins: HashMap<Id, PinLookup>,
    chans: HashMap<[u8; 16], DeskChan>,
}

struct Phone {
    identity: Option<Arc<ffi::Keypair>>,
    push: Option<Arc<ffi::Keypair>>,
    dir: PathBuf,
    state_key: Option<Vec<u8>>,
    generation: u64,
    push_pins: Vec<u8>,
}

struct H {
    phone: Phone,
    objs: HashMap<String, Obj>,
    next: u64,
    desks: HashMap<String, Desk>,
}

impl H {
    fn put(&mut self, o: Obj) -> String {
        self.next += 1;
        let k = format!("o{}", self.next);
        self.objs.insert(k.clone(), o);
        k
    }

    fn identity(&self) -> Result<Arc<ffi::Keypair>, (String, String)> {
        self.phone
            .identity
            .clone()
            .ok_or_else(|| err("E2EE_NO_IDENTITY", "no identity key"))
    }

    fn store_path(&self) -> String {
        self.phone.dir.join("state.v1").to_string_lossy().into_owned()
    }

    fn handle(&mut self, req: &Value) -> R {
        let op = s(req, "op")?;
        match op.as_str() {
            // ---------------------------------------------------------- phone
            "phone.init" => {
                self.phone.dir = PathBuf::from(s(req, "dir")?);
                Ok(Value::Null)
            }
            "phone.identityPublicKey" => Ok(self
                .phone
                .identity
                .as_ref()
                .map(|k| Value::String(hex(&k.public_key())))
                .unwrap_or(Value::Null)),
            "phone.createIdentityKey" => {
                if self.phone.identity.is_none() {
                    self.phone.identity = Some(ffi::Keypair::generate().map_err(ffi_err)?);
                }
                Ok(json!(hex(&self.identity()?.public_key())))
            }
            "phone.pushPublicKey" => Ok(self
                .phone
                .push
                .as_ref()
                .map(|k| Value::String(hex(&k.public_key())))
                .unwrap_or(Value::Null)),
            "phone.createPushKey" => {
                if self.phone.push.is_none() {
                    self.phone.push = Some(ffi::Keypair::generate().map_err(ffi_err)?);
                }
                Ok(json!(hex(&self.phone.push.as_ref().unwrap().public_key())))
            }
            "phone.deleteAllKeys" => {
                self.phone.identity = None;
                self.phone.push = None;
                self.phone.push_pins.clear();
                let _ = std::fs::remove_file(self.store_path());
                self.phone.state_key = None;
                self.phone.generation = 0;
                Ok(Value::Null)
            }
            "phone.listPushPins" => {
                if self.phone.push_pins.is_empty() {
                    return Ok(json!([]));
                }
                let pins = ffi::decode_push_pins(self.phone.push_pins.clone()).map_err(ffi_err)?;
                Ok(Value::Array(
                    pins.iter()
                        .map(|p| json!({"desktop": p.desktop, "key": hex(&p.key)}))
                        .collect(),
                ))
            }
            "phone.setPushPin" | "phone.removePushPin" => {
                let desktop = s(req, "desktop")?;
                let mut pins = if self.phone.push_pins.is_empty() {
                    Vec::new()
                } else {
                    ffi::decode_push_pins(self.phone.push_pins.clone()).map_err(ffi_err)?
                };
                pins.retain(|p| p.desktop != desktop);
                if op == "phone.setPushPin" {
                    pins.push(ffi::PushPin {
                        desktop,
                        key: b(req, "key")?,
                    });
                }
                self.phone.push_pins = if pins.is_empty() {
                    Vec::new()
                } else {
                    ffi::encode_push_pins(pins).map_err(ffi_err)?
                };
                Ok(Value::Null)
            }
            "phone.openPinStore" => {
                let key = match &self.phone.state_key {
                    Some(k) => k.clone(),
                    None => {
                        if self.phone.generation != 0 {
                            return Err(err("E2EE_STORE_LOST", "missing state key"));
                        }
                        let k = ffi::generate_state_key().map_err(ffi_err)?;
                        self.phone.state_key = Some(k.clone());
                        k
                    }
                };
                let opened = ffi::open_pin_store(self.store_path(), key, self.phone.generation)
                    .map_err(ffi_err)?;
                if let Some(g) = opened.bump_keychain_to {
                    self.phone.generation = g;
                }
                Ok(json!(self.put(Obj::Store(opened.store))))
            }
            "phone.resetPinStore" => {
                let _ = std::fs::remove_file(self.store_path());
                self.phone.state_key = None;
                self.phone.generation = 0;
                Ok(Value::Null)
            }
            "phone.tamperPinStore" => {
                // Test-only: flip a byte of the sealed file (§6.1 "fails its check").
                let p = self.store_path();
                let mut bytes = std::fs::read(&p).map_err(|e| err("HARNESS", e.to_string()))?;
                let i = bytes.len() / 2;
                bytes[i] ^= 1;
                std::fs::write(&p, bytes).map_err(|e| err("HARNESS", e.to_string()))?;
                Ok(Value::Null)
            }
            "phone.startKk" => {
                let ids = ffi::ChannelIds {
                    account: s(req, "account")?,
                    phone: s(req, "phone")?,
                    desktop: s(req, "desktop")?,
                    chan: b(req, "chan")?,
                };
                let caps = arg(req, "caps")?
                    .as_array()
                    .ok_or_else(|| err("HARNESS", "caps"))?
                    .iter()
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect();
                let identity = self.identity()?;
                let start = ffi::start_kk(&identity, b(req, "pin")?, ids, caps)
                    .map_err(ffi_err)?;
                Ok(json!({"hs1": hex(&start.hs1), "obj": self.put(Obj::Kk(start.handshake))}))
            }
            "phone.randomChannelId" => Ok(json!(hex(&ffi::random_channel_id().map_err(ffi_err)?))),
            "phone.randomBytes" => Ok(json!(hex(
                &ffi::random_bytes(n(req, "n")? as u32).map_err(ffi_err)?
            ))),
            "phone.sha256" => Ok(json!(hex(&ffi::sha256(b(req, "data")?)))),
            "phone.decodeFrame" => {
                let f = ffi::decode_frame(b(req, "bytes")?).map_err(ffi_err)?;
                Ok(json!({
                    "kind": match f.kind { ffi::FrameKind::Hs => "hs", ffi::FrameKind::Data => "data", ffi::FrameKind::Close => "close" },
                    "step": f.step,
                    "purpose": f.purpose.map(|p| match p { ffi::FramePurpose::Companion => "companion", ffi::FramePurpose::Pairing => "pairing" }),
                    "chan": hex(&f.chan),
                    "peer": f.peer,
                    "ctr": f.ctr,
                    "body": hex(&f.body),
                }))
            }
            "phone.encodeFrame" => {
                let kind = match s(req, "kind")?.as_str() {
                    "hs" => ffi::FrameKind::Hs,
                    "data" => ffi::FrameKind::Data,
                    _ => ffi::FrameKind::Close,
                };
                let purpose = match req.get("purpose").and_then(Value::as_str) {
                    Some("companion") => Some(ffi::FramePurpose::Companion),
                    Some("pairing") => Some(ffi::FramePurpose::Pairing),
                    _ => None,
                };
                let f = ffi::Frame {
                    kind,
                    step: n(req, "step")? as u8,
                    purpose,
                    chan: b(req, "chan")?,
                    peer: req.get("peer").and_then(Value::as_str).map(str::to_string),
                    ctr: n(req, "ctr")?,
                    body: b(req, "body")?,
                };
                Ok(json!(hex(&ffi::encode_frame(f).map_err(ffi_err)?)))
            }
            "phone.encodeHs1Frame" => {
                let purpose = if s(req, "purpose")? == "pairing" {
                    ffi::FramePurpose::Pairing
                } else {
                    ffi::FramePurpose::Companion
                };
                Ok(json!(hex(&ffi::encode_hs1_frame(
                    purpose,
                    b(req, "chan")?,
                    s(req, "peer")?,
                    b(req, "body")?
                )
                .map_err(ffi_err)?)))
            }
            "phone.encodeRecords" => {
                let mtype = mtype_ffi(&s(req, "mtype")?)?;
                let recs = ffi::encode_records(mtype, n(req, "msgId")? as u32, b(req, "content")?)
                    .map_err(ffi_err)?;
                Ok(Value::Array(recs.iter().map(|r| json!(hex(r))).collect()))
            }
            "phone.encodePadRecord" => Ok(json!(hex(
                &ffi::encode_pad_record(n(req, "len")? as u32).map_err(ffi_err)?
            ))),
            "phone.decodeTerm" => {
                let t = ffi::decode_term(b(req, "content")?).map_err(ffi_err)?;
                Ok(json!({
                    "handle": t.handle, "seq": t.seq, "base": t.base, "total": t.total,
                    "alive": t.alive, "reset": t.reset, "exitCode": t.exit_code,
                    "rows": t.rows, "cols": t.cols, "data": hex(&t.data),
                }))
            }
            "phone.decodeCtrl" => {
                let c = ffi::decode_ctrl(b(req, "content")?).map_err(ffi_err)?;
                Ok(json!({"name": c.name, "json": c.json}))
            }
            "phone.encodeCtrl" => Ok(json!(hex(
                &ffi::encode_ctrl(s(req, "name")?, s(req, "json")?).map_err(ffi_err)?
            ))),
            "phone.createReassembler" => Ok(json!(self.put(Obj::Reasm(ffi::Reassembler::new())))),
            "phone.beginPart" => {
                let p = ffi::PartSession::begin(b(req, "content")?).map_err(ffi_err)?;
                Ok(json!(self.put(Obj::Part(p))))
            }

            // -------------------------------------------------- phone objects
            "obj.release" => {
                self.objs.remove(&s(req, "obj")?);
                Ok(Value::Null)
            }
            "obj.call" => self.obj_call(req),

            // -------------------------------------------------------- desktop
            "desk.new" => {
                let name = s(req, "name")?;
                let d = Desk {
                    keys: StaticKeypair::generate().map_err(|_| err("E2EE_RANDOM", "rng"))?,
                    account: id(req, "account")?,
                    desktop: id(req, "desktop")?,
                    pins: HashMap::new(),
                    chans: HashMap::new(),
                };
                let public = hex(d.keys.public().as_bytes());
                self.desks.insert(name, d);
                Ok(json!(public))
            }
            "desk.pin" => {
                let d = self.desk(req)?;
                let phone = id(req, "phone")?;
                let k = key(&b(req, "key")?)?;
                d.pins.insert(phone, PinLookup::Pinned(k));
                Ok(Value::Null)
            }
            "desk.tombstone" => {
                let d = self.desk(req)?;
                let phone = id(req, "phone")?;
                d.pins.insert(phone, PinLookup::Tombstoned);
                let dead: Vec<[u8; 16]> = d
                    .chans
                    .iter()
                    .filter(|(_, c)| c.phone == phone)
                    .map(|(k, _)| *k)
                    .collect();
                for k in dead {
                    if let Some(mut c) = d.chans.remove(&k) {
                        c.channel.close(dragent_e2ee::channel::CloseReason::Revoked);
                    }
                }
                Ok(Value::Null)
            }
            "desk.listen" => {
                let now = n(req, "now")?;
                let bytes = b(req, "frame")?;
                let caps: Vec<String> = req
                    .get("caps")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let d = self.desk(req)?;
                let (f, _) = header::decode(&bytes).map_err(|e| err("E2EE_MALFORMED", format!("{e:?}")))?;
                if f.kind != header::Kind::Hs || f.step != 1 {
                    return Err(err("HARNESS", "not HS1"));
                }
                let phone = f.peer.ok_or_else(|| err("HARNESS", "no peer"))?;
                let pin = d.pins.get(&phone).copied().unwrap_or(PinLookup::Unpinned);
                let prologue = KkPrologue {
                    account: d.account,
                    phone,
                    desktop: d.desktop,
                    chan: f.chan,
                };
                let payload = hs::encode_hello(&hs::Hello { caps })
                    .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                match kk_listen(&d.keys, pin, &prologue, f.body, &payload, now) {
                    Ok(l) => {
                        let mut out = Vec::new();
                        header::Head {
                            kind: header::Kind::Hs,
                            step: 2,
                            purpose: Some(header::Purpose::Companion),
                            chan: f.chan,
                            peer: None,
                            ctr: 0,
                            body: &l.hs2,
                        }
                        .encode(&mut out)
                        .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                        d.chans.insert(
                            *f.chan.as_bytes(),
                            DeskChan {
                                channel: l.channel,
                                phone,
                                reasm: record::Reassembler::new(),
                                next_msg: 1,
                            },
                        );
                        Ok(json!({"hs2": hex(&out), "chan": hex(f.chan.as_bytes()), "phone": phone.to_string()}))
                    }
                    Err(_) => {
                        let mut out = Vec::new();
                        let reason = [header::CloseReason::UnknownPeer.byte()];
                        header::Head {
                            kind: header::Kind::Close,
                            step: 0,
                            purpose: None,
                            chan: f.chan,
                            peer: None,
                            ctr: 0,
                            body: &reason,
                        }
                        .encode(&mut out)
                        .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                        Ok(json!({"refused": true, "close": hex(&out)}))
                    }
                }
            }
            "desk.receive" => {
                let bytes = b(req, "frame")?;
                let d = self.desk(req)?;
                let (f, _) = header::decode(&bytes).map_err(|e| err("E2EE_MALFORMED", format!("{e:?}")))?;
                let c = d
                    .chans
                    .get_mut(f.chan.as_bytes())
                    .ok_or_else(|| err("HARNESS", "unknown chan"))?;
                if f.kind == header::Kind::Close {
                    c.channel.close(dragent_e2ee::channel::CloseReason::PeerClosed);
                    return Ok(json!({"closed": true, "reason": f.body.first().copied()}));
                }
                if f.kind != header::Kind::Data {
                    return Err(err("HARNESS", "not DATA"));
                }
                let inbound = match c.channel.receive(f.ctr, f.body) {
                    Ok(i) => i,
                    Err(e) => return Ok(json!({"protocolError": format!("{e}")})),
                };
                let (rec, confirming) = match inbound {
                    DInbound::Duplicate => return Ok(json!({"duplicate": true})),
                    DInbound::Record(r) => (r, false),
                    DInbound::Confirming(r) => (r, true),
                };
                let decoded = match record::decode(&rec) {
                    Ok(r) => r,
                    Err(e) => {
                        c.channel.close(dragent_e2ee::channel::CloseReason::Local);
                        return Ok(json!({"protocolError": format!("record {e:?}")}));
                    }
                };
                let msg = match c.reasm.feed(&decoded) {
                    Ok(m) => m,
                    Err(e) => {
                        c.channel.close(dragent_e2ee::channel::CloseReason::Local);
                        return Ok(json!({"protocolError": format!("reassembly {e:?}")}));
                    }
                };
                if confirming {
                    let ok = matches!(
                        &msg,
                        Some(m) if m.mtype == record::MsgType::Ctrl
                            && matches!(Ctrl::decode(&m.content), Ok(Some(Ctrl::Hello { .. })))
                    );
                    if ok {
                        c.channel.confirm().map_err(|e| err("HARNESS", format!("{e}")))?;
                    } else {
                        c.channel.reject_confirmation();
                        return Ok(json!({"notConfirmed": true}));
                    }
                }
                let Some(m) = msg else {
                    return Ok(json!({"partial": true, "ctr": f.ctr}));
                };
                let mut out = json!({
                    "ctr": f.ctr,
                    "mtype": match m.mtype { record::MsgType::Json => "json", record::MsgType::Term => "term", record::MsgType::Ctrl => "ctrl" },
                    "content": hex(&m.content),
                    "text": String::from_utf8_lossy(&m.content),
                });
                if m.mtype == record::MsgType::Ctrl {
                    // The desktop's canonical CTRL check (part commands decode as None).
                    out["ctrlCanonical"] = json!(Ctrl::decode(&m.content).is_ok());
                }
                Ok(out)
            }
            "desk.send" => {
                let mtype = mtype_rec(&s(req, "mtype")?)?;
                let content = b(req, "content")?;
                let chan = b(req, "chan")?;
                let d = self.desk(req)?;
                let c = d
                    .chans
                    .get_mut(&chan[..])
                    .ok_or_else(|| err("HARNESS", "unknown chan"))?;
                let msg_id = c.next_msg;
                c.next_msg += 1;
                let total = content.len() as u32;
                let mut chunks: Vec<&[u8]> = content.chunks(record::MAX_RECORD_CONTENT).collect();
                if chunks.is_empty() {
                    chunks.push(&[]);
                }
                let k = chunks.len();
                let mut frames = Vec::new();
                for (i, chunk) in chunks.into_iter().enumerate() {
                    let first = i == 0;
                    let len = record::smallest_len(chunk.len()).ok_or_else(|| err("E2EE_TOO_LARGE", "record"))?;
                    let mut rec = Vec::new();
                    record::encode(
                        &mut rec, len, first, i + 1 == k, false,
                        first.then_some(mtype), msg_id, if first { total } else { 0 }, chunk,
                    )
                    .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                    let o = c.channel.seal(&rec).map_err(|e| err("E2EE_CHANNEL_CLOSED", format!("{e}")))?;
                    let mut fr = Vec::new();
                    header::Head {
                        kind: header::Kind::Data,
                        step: 0,
                        purpose: None,
                        chan: c.channel.id(),
                        peer: None,
                        ctr: o.ctr,
                        body: &o.ciphertext,
                    }
                    .encode(&mut fr)
                    .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                    frames.push(json!(hex(&fr)));
                }
                Ok(Value::Array(frames))
            }
            "desk.closeFrame" => {
                let chan = chan_of(&b(req, "chan")?)?;
                let reason = [n(req, "reason")? as u8];
                let mut out = Vec::new();
                header::Head {
                    kind: header::Kind::Close,
                    step: 0,
                    purpose: None,
                    chan,
                    peer: None,
                    ctr: 0,
                    body: &reason,
                }
                .encode(&mut out)
                .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                if let Some(d) = req.get("desk").and_then(Value::as_str).and_then(|k| self.desks.get_mut(k)) {
                    if let Some(c) = d.chans.get_mut(chan.as_bytes()) {
                        c.channel.close(dragent_e2ee::channel::CloseReason::Local);
                    }
                }
                Ok(json!(hex(&out)))
            }
            "desk.chanState" => {
                let chan = b(req, "chan")?;
                let d = self.desk(req)?;
                let c = d.chans.get(&chan[..]).ok_or_else(|| err("HARNESS", "unknown chan"))?;
                Ok(json!({
                    "state": match c.channel.state() {
                        ChannelState::AwaitingConfirm => "awaiting_confirm".to_string(),
                        ChannelState::Confirming => "confirming".to_string(),
                        ChannelState::Open => "open".to_string(),
                        ChannelState::WindingDown => "winding_down".to_string(),
                        ChannelState::Closed(r) => format!("closed:{r:?}"),
                    },
                    "processedThrough": c.channel.processed_through(),
                    "lastSentCtr": c.channel.last_sent_ctr(),
                }))
            }
            "desk.term" => {
                let data = b(req, "data")?;
                let size = match (req.get("rows").and_then(Value::as_u64), req.get("cols").and_then(Value::as_u64)) {
                    (Some(r), Some(c)) => Some((r as u16, c as u16)),
                    _ => None,
                };
                let head = term::TermHead {
                    handle: n(req, "handle")? as u32,
                    seq: n(req, "seq")?,
                    base: n(req, "base")?,
                    total: n(req, "total")?,
                    alive: req.get("alive").and_then(Value::as_bool).unwrap_or(true),
                    reset: req.get("reset").and_then(Value::as_bool).unwrap_or(false),
                    exit_code: req.get("exitCode").and_then(Value::as_i64).map(|x| x as i32),
                    size,
                    data: &data,
                };
                let mut out = Vec::new();
                head.encode(&mut out).map_err(|e| err("E2EE_MALFORMED", format!("{e:?}")))?;
                Ok(json!(hex(&out)))
            }
            "desk.partBegin" => {
                let payload = b(req, "payload")?;
                let sha: [u8; 32] = <[u8; 32]>::try_from(&ffi::sha256(payload.clone())[..]).unwrap();
                let sha = match req.get("sha256").and_then(Value::as_str) {
                    Some(h) => <[u8; 32]>::try_from(&unhex(h)?[..]).map_err(|_| err("HARNESS", "sha"))?,
                    None => sha,
                };
                let purpose = match s(req, "purpose")?.as_str() {
                    "seed" => part::Purpose::Seed,
                    "screen" => part::Purpose::Screen,
                    _ => part::Purpose::Reply,
                };
                let begin = part::PartBegin {
                    part_id: s(req, "partId")?,
                    purpose,
                    request_id: req.get("requestId").and_then(Value::as_str).map(|r| part::RequestId::Str(r.to_string())),
                    stream_handle: req.get("streamHandle").and_then(Value::as_u64).map(|h| h as u32),
                    total_bytes: payload.len() as u64,
                    part_count: n(req, "partCount")? as u32,
                    sha256: sha,
                };
                Ok(json!(hex(&part::encode_begin(&begin).map_err(|e| err("HARNESS", format!("{e:?}")))?)))
            }
            "desk.partChunk" => Ok(json!(hex(
                &part::encode_chunk(&s(req, "partId")?, n(req, "index")? as u32, &b(req, "bytes")?)
                    .map_err(|e| err("HARNESS", format!("{e:?}")))?
            ))),
            "desk.partEnd" => Ok(json!(hex(
                &part::encode_end(&s(req, "partId")?).map_err(|e| err("HARNESS", format!("{e:?}")))?
            ))),
            "desk.ctrl" => {
                let c = match s(req, "name")?.as_str() {
                    "bye" => Ctrl::Bye { reason: s(req, "reason")? },
                    "input_lost" => Ctrl::InputLost {
                        handles: arg(req, "handles")?
                            .as_array()
                            .ok_or_else(|| err("HARNESS", "handles"))?
                            .iter()
                            .filter_map(Value::as_u64)
                            .map(|h| h as u32)
                            .collect(),
                    },
                    "pair-done" => Ctrl::PairDone {
                        desktop_name: s(req, "desktopName")?,
                        catalog_version: n(req, "catalogVersion")?,
                    },
                    "input_lost_ack" => Ctrl::InputLostAck,
                    other => return Err(err("HARNESS", format!("ctrl {other}"))),
                };
                Ok(json!(hex(&c.encode().map_err(|e| err("HARNESS", format!("{e}")))?)))
            }
            "desk.sealCatalog" => {
                let version = n(req, "version")?;
                let body_json = b(req, "json")?;
                let phones = arg(req, "phones")?
                    .as_array()
                    .ok_or_else(|| err("HARNESS", "phones"))?
                    .clone();
                let d = self.desk(req)?;
                let (k, nonce, body) = cat::seal_body(&d.account, &d.desktop, version, &body_json)
                    .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                let mut recipients = Vec::new();
                for p in phones {
                    let phone = id(&p, "id")?;
                    let pk = key(&b(&p, "key")?)?;
                    let prologue = CatalogPrologue {
                        account: d.account,
                        desktop: d.desktop,
                        phone,
                        version,
                    };
                    let wrap = cat::wrap_key(&d.keys, &pk, &prologue, &k)
                        .map_err(|e| err("HARNESS", format!("{e:?}")))?;
                    recipients.push(catalog_blob::Recipient { phone, wrap });
                }
                let blob = catalog_blob::CatalogBlob {
                    desktop: d.desktop,
                    version,
                    recipients,
                    nonce,
                    body,
                };
                Ok(json!(hex(&blob.encode().map_err(|e| err("HARNESS", format!("{e:?}")))?)))
            }
            other => Err(err("HARNESS", format!("unknown op {other}"))),
        }
    }

    fn desk(&mut self, req: &Value) -> Result<&mut Desk, (String, String)> {
        let k = s(req, "desk")?;
        self.desks.get_mut(&k).ok_or_else(|| err("HARNESS", "unknown desk"))
    }

    fn obj_call(&mut self, req: &Value) -> R {
        let k = s(req, "obj")?;
        let method = s(req, "method")?;
        let identity = self.phone.identity.clone();
        let o = self.objs.get(&k).ok_or_else(|| err("E2EE_CONSUMED", "released"))?;
        match o {
            Obj::Kk(h) => match method.as_str() {
                "finish" => {
                    let f = h.finish(b(req, "hs2")?, n(req, "now")?).map_err(ffi_err)?;
                    let caps = f.caps.clone();
                    let obj = self.put(Obj::Channel(f.channel));
                    Ok(json!({"obj": obj, "caps": caps}))
                }
                "abandon" => {
                    h.abandon();
                    Ok(Value::Null)
                }
                _ => Err(err("HARNESS", "kk method")),
            },
            Obj::Channel(c) => {
                let c = c.clone();
                chan_call(&c, &method, req)
            }
            Obj::Reasm(r) => match method.as_str() {
                "feed" => match r.feed(b(req, "record")?).map_err(ffi_err)? {
                    None => Ok(Value::Null),
                    Some(m) => Ok(json!({"mtype": mtype_name(m.mtype), "content": hex(&m.content)})),
                },
                "buffered" => Ok(json!(r.buffered())),
                "clear" => {
                    r.clear();
                    Ok(Value::Null)
                }
                _ => Err(err("HARNESS", "reasm method")),
            },
            Obj::Part(p) => match method.as_str() {
                "info" => {
                    let i = p.info();
                    Ok(json!({
                        "partId": i.part_id,
                        "purpose": match i.purpose { ffi::PartPurpose::Seed => "seed", ffi::PartPurpose::Screen => "screen", ffi::PartPurpose::Reply => "reply" },
                        "requestIdJson": i.request_id_json,
                        "streamHandle": i.stream_handle,
                        "totalBytes": i.total_bytes,
                        "partCount": i.part_count,
                    }))
                }
                "feed" => {
                    p.feed(b(req, "chunk")?).map_err(ffi_err)?;
                    Ok(Value::Null)
                }
                "end" => Ok(json!(hex(&p.end(b(req, "content")?).map_err(ffi_err)?))),
                "abort" => {
                    p.abort();
                    Ok(Value::Null)
                }
                _ => Err(err("HARNESS", "part method")),
            },
            Obj::Store(st) => {
                let st = st.clone();
                let r = store_call(&st, identity, &method, req)?;
                if method == "commit" {
                    if let Some(g) = r.as_u64() {
                        self.phone.generation = g;
                    }
                }
                Ok(r)
            }
        }
    }
}

fn mtype_ffi(m: &str) -> Result<ffi::MessageType, (String, String)> {
    Ok(match m {
        "json" => ffi::MessageType::Json,
        "term" => ffi::MessageType::Term,
        "ctrl" => ffi::MessageType::Ctrl,
        _ => return Err(err("HARNESS", "mtype")),
    })
}
fn mtype_rec(m: &str) -> Result<record::MsgType, (String, String)> {
    Ok(match m {
        "json" => record::MsgType::Json,
        "term" => record::MsgType::Term,
        "ctrl" => record::MsgType::Ctrl,
        _ => return Err(err("HARNESS", "mtype")),
    })
}
fn mtype_name(m: ffi::MessageType) -> &'static str {
    match m {
        ffi::MessageType::Json => "json",
        ffi::MessageType::Term => "term",
        ffi::MessageType::Ctrl => "ctrl",
    }
}

fn chan_call(c: &ffi::Channel, method: &str, req: &Value) -> R {
    match method {
        "chanId" => Ok(json!(hex(&c.chan_id()))),
        "state" => Ok(match c.state() {
            ffi::ChannelStateKind::AwaitingConfirm => json!({"state": "awaiting_confirm"}),
            ffi::ChannelStateKind::Confirming => json!({"state": "confirming"}),
            ffi::ChannelStateKind::Open => json!({"state": "open"}),
            ffi::ChannelStateKind::WindingDown => json!({"state": "winding_down"}),
            ffi::ChannelStateKind::Closed { reason } => json!({"state": "closed", "reason": reason}),
        }),
        "handshakeHash" => Ok(json!(hex(&c.handshake_hash()))),
        "remoteStatic" => Ok(json!(hex(&c.remote_static()))),
        "hasKeys" => Ok(json!(c.has_keys())),
        "processedThrough" => Ok(json!(c.processed_through())),
        "lastSentCtr" => Ok(json!(c.last_sent_ctr())),
        "isQuiesced" => Ok(json!(c.is_quiesced())),
        "seal" => {
            let o = c.seal(b(req, "record")?).map_err(ffi_err)?;
            Ok(json!({"ctr": o.ctr, "ciphertext": hex(&o.ciphertext)}))
        }
        "sealFrame" => Ok(json!(hex(&c.seal_frame(b(req, "record")?).map_err(ffi_err)?))),
        "receive" => match c.receive(n(req, "ctr")?, b(req, "ciphertext")?).map_err(ffi_err)? {
            ffi::Inbound::Record { record } => Ok(json!({"kind": "record", "record": hex(&record)})),
            ffi::Inbound::Confirming { record } => Ok(json!({"kind": "confirming", "record": hex(&record)})),
            ffi::Inbound::Duplicate => Ok(json!({"kind": "duplicate"})),
        },
        "quiesce" => {
            let r = c.quiesce();
            Ok(json!({"chan": hex(&r.chan), "lastSentCtr": r.last_sent_ctr}))
        }
        "poll" => Ok(json!(c.poll(n(req, "now")?))),
        "rekeyDue" => Ok(json!(c.rekey_due(n(req, "now")?))),
        "close" => {
            c.close(match s(req, "reason")?.as_str() {
                "peer_closed" => ffi::LocalCloseReason::PeerClosed,
                "revoked" => ffi::LocalCloseReason::Revoked,
                "rekeyed" => ffi::LocalCloseReason::Rekeyed,
                _ => ffi::LocalCloseReason::Local,
            });
            Ok(Value::Null)
        }
        _ => Err(err("HARNESS", format!("channel method {method}"))),
    }
}

fn store_call(st: &ffi::PinStore, identity: Option<Arc<ffi::Keypair>>, method: &str, req: &Value) -> R {
    match method {
        "generation" => Ok(json!(st.generation())),
        "desktops" => Ok(Value::Array(
            st.desktops()
                .iter()
                .map(|d| {
                    json!({"id": d.id, "identityKey": hex(&d.identity_key), "name": d.name,
                        "pinnedAtMs": d.pinned_at_ms, "rotationKey": d.rotation_key.as_ref().map(|k| hex(k))})
                })
                .collect(),
        )),
        "lookup" => Ok(match st.lookup(s(req, "device")?).map_err(ffi_err)? {
            ffi::PinState::Pinned { identity_key } => json!({"state": "pinned", "identityKey": hex(&identity_key)}),
            ffi::PinState::Unpinned => json!({"state": "unpinned"}),
            ffi::PinState::Tombstoned => json!({"state": "tombstoned"}),
        }),
        "candidateKeys" => Ok(Value::Array(
            st.candidate_keys(s(req, "device")?, n(req, "now")?)
                .map_err(ffi_err)?
                .iter()
                .map(|k| json!(hex(k)))
                .collect(),
        )),
        "pinDesktop" => {
            st.pin_desktop(s(req, "device")?, b(req, "key")?, s(req, "name")?, n(req, "catalogVersion")?, n(req, "now")?)
                .map_err(ffi_err)?;
            Ok(Value::Null)
        }
        "revoke" => Ok(json!(st.revoke(s(req, "device")?, n(req, "now")?).map_err(ffi_err)?)),
        "openCatalog" => {
            let identity = identity.ok_or_else(|| err("E2EE_NO_IDENTITY", "no identity"))?;
            match st
                .open_catalog(&identity, s(req, "account")?, s(req, "phone")?, s(req, "desktop")?, b(req, "blob")?)
                .map_err(ffi_err)?
            {
                ffi::CatalogResult::Accepted { version, json } => {
                    Ok(json!({"status": "accepted", "version": version, "json": hex(&json)}))
                }
                ffi::CatalogResult::Unchanged { version } => Ok(json!({"status": "unchanged", "version": version})),
            }
        }
        "catalogHighWater" => Ok(json!(st.catalog_high_water(s(req, "desktop")?).map_err(ffi_err)?)),
        "commit" => Ok(json!(st.commit().map_err(ffi_err)?)),
        _ => Err(err("HARNESS", format!("store method {method}"))),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: e2ee-transport-harness <request-fifo> <response-fifo>");
        std::process::exit(2);
    }
    let input = File::open(&args[1]).expect("open request fifo");
    let mut output = OpenOptions::new().write(true).open(&args[2]).expect("open response fifo");
    let mut h = H {
        phone: Phone {
            identity: None,
            push: None,
            dir: std::env::temp_dir(),
            state_key: None,
            generation: 0,
            push_pins: Vec::new(),
        },
        objs: HashMap::new(),
        next: 0,
        desks: HashMap::new(),
    };
    for line in BufReader::new(input).lines() {
        let Ok(line) = line else { break };
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.handle(&req))) {
                Ok(Ok(v)) => json!({"ok": v}),
                Ok(Err((code, message))) => json!({"err": {"code": code, "message": message}}),
                Err(_) => json!({"err": {"code": "E2EE_INTERNAL", "message": "panic"}}),
            },
            Err(_) => json!({"err": {"code": "HARNESS", "message": "bad json"}}),
        };
        let mut text = resp.to_string();
        text.push('\n');
        if output.write_all(text.as_bytes()).is_err() || output.flush().is_err() {
            break;
        }
    }
}
