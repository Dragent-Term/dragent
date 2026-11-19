#!/usr/bin/env python3
"""Generate E1b codec test vectors (docs/design/e2ee.md wire formats).

Independent Python implementation of the layouts in dragent_e2ee/src/codec/*.rs.
The Rust loader (src/codec/vectors.rs) decodes every vector with the real
codec and checks the fields plus byte-exact re-encoding, so any divergence
between this script and the codec fails the build.

Vector file schemas (all objects: {"name", ...input..., "expect" | "expect_error"}):
- header.json: "bytes_hex" = one 44+B frame. expect: kind "hs"|"data"|"close",
  step u8, purpose "companion"|"pairing"|null, chan 32-hex, peer UUID|null,
  ctr u64, body_hex. error: the HeaderError variant name (BadLength,
  BadVersion, BadKind, BadStep, BadPurpose, MissingPeer, UnexpectedPeer,
  NonZeroCtr, BadCloseBody, BodyTooLong, Truncated).
- record.json: "bytes_hex" = one L-byte record. expect: first/last/pad_only
  bool, mtype "json"|"term"|"ctrl"|null, msg_id u32, total_len u32,
  content_hex. error: RecordError variant.
- term.json: "bytes_hex" = TERM payload. expect: handle u32, seq/base/total
  u64, alive/reset bool, exit_code i32|null, size [rows,cols]|null, data_hex.
  error: TermError variant.
- part.json: "content_utf8" (CTRL begin/end) or "json_utf8" (chunk). expect
  for begin: part_id, purpose, request_id|null, stream_handle|null,
  total_bytes u64, part_count u32, sha256_hex. chunk: part_id, index u32,
  bytes_hex. end: part_id. Error entries carry "expect_error".
- catalog_blob.json: "bytes_hex" = padded blob. expect: desktop UUID,
  version u64, recipients [{phone UUID, wrap_hex}], nonce_hex, body_hex.
- push.json: "bytes_hex" = 1,024 bytes. expect: desktop_id UUID, id_hex,
  issued_at_ms u64, kind u8, dismiss bool, title/body str, workspace_id/tab_id
  UUID, dismiss_ids [32-hex]. error: PushError variant.
- hs.json: "json_utf8". kk_hello expect.caps [str]; pairing_hello expect
  device_name, platform, push_pub_hex. Error entries name the decoder in
  "kind" ("hello"|"pairing").
"""
import base64, hashlib, json, os, struct

OUT = os.path.dirname(os.path.abspath(__file__))
os.makedirs(OUT, exist_ok=True)

ACCOUNT = "11111111-2222-4333-8444-555555555555"
PHONE = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
DESKTOP = "01234567-89ab-4cde-8f01-23456789abcd"
WORKSPACE = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"
TAB = "cccccccc-dddd-4eee-8fff-000000000000"
CHAN = bytes(range(0, 256, 17))[:16].hex()  # 00112233...eeff
assert CHAN == "00112233445566778899aabbccddeeff", CHAN
CHAN_B = bytes.fromhex(CHAN)


def uuid_hex(u):
    return u.replace("-", "")


def b64url(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def dump(name, vectors):
    with open(os.path.join(OUT, name), "w") as f:
        json.dump({"vectors": vectors}, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"{name}: {len(vectors)} vectors")


# ---- header (§4.1) ----
def frame(kind, step, purpose, peer, ctr, body):
    h = struct.pack(">BB", 1, kind) + bytes([step, purpose])
    h += CHAN_B + (bytes.fromhex(uuid_hex(peer)) if peer else bytes(16))
    h += struct.pack(">Q", ctr) + body
    return h.hex()


PEER = PHONE
HS1_BODY = b"handshake-bytes"
header_vectors = [
    {"name": "hs1_companion", "bytes_hex": frame(1, 1, 1, PEER, 0, HS1_BODY),
     "expect": {"kind": "hs", "step": 1, "purpose": "companion",
                "chan": CHAN, "peer": PEER, "ctr": 0, "body_hex": HS1_BODY.hex()}},
    {"name": "hs1_pairing", "bytes_hex": frame(1, 1, 2, DESKTOP, 0, b"\x00" * 32),
     "expect": {"kind": "hs", "step": 1, "purpose": "pairing",
                "chan": CHAN, "peer": DESKTOP, "ctr": 0, "body_hex": "00" * 32}},
    {"name": "hs2_companion", "bytes_hex": frame(1, 2, 1, None, 0, b"\x11" * 48),
     "expect": {"kind": "hs", "step": 2, "purpose": "companion",
                "chan": CHAN, "peer": None, "ctr": 0, "body_hex": "11" * 48}},
    {"name": "data_ctr41", "bytes_hex": frame(2, 0, 0, None, 41, bytes([7] * 100)),
     "expect": {"kind": "data", "step": 0, "purpose": None,
                "chan": CHAN, "peer": None, "ctr": 41, "body_hex": "07" * 100}},
    {"name": "close_protocol_error", "bytes_hex": frame(3, 0, 0, None, 0, b"\x01"),
     "expect": {"kind": "close", "step": 0, "purpose": None,
                "chan": CHAN, "peer": None, "ctr": 0, "body_hex": "01"}},
    {"name": "close_unknown_peer", "bytes_hex": frame(3, 0, 0, None, 0, b"\x02"),
     "expect": {"kind": "close", "step": 0, "purpose": None,
                "chan": CHAN, "peer": None, "ctr": 0, "body_hex": "02"}},
    # Rejections (the relay's length-and-enum check).
    {"name": "err_bad_version", "bytes_hex": "02" + frame(1, 1, 1, PEER, 0, b"x")[2:],
     "expect": {"error": "BadVersion"}},
    {"name": "err_bad_kind", "bytes_hex": frame(4, 0, 0, None, 0, b"x"),
     "expect": {"error": "BadKind"}},
    {"name": "err_hs_step0", "bytes_hex": frame(1, 0, 1, None, 0, b"x"),
     "expect": {"error": "BadStep"}},
    {"name": "err_hs_purpose0", "bytes_hex": frame(1, 1, 0, PEER, 0, b"x"),
     "expect": {"error": "BadPurpose"}},
    {"name": "err_data_purpose", "bytes_hex": frame(2, 0, 1, None, 0, b"y"),
     "expect": {"error": "BadPurpose"}},
    {"name": "err_data_peer", "bytes_hex": frame(2, 0, 0, PEER, 0, b"y"),
     "expect": {"error": "UnexpectedPeer"}},
    {"name": "err_hs2_peer", "bytes_hex": frame(1, 2, 1, PEER, 0, b"x"),
     "expect": {"error": "UnexpectedPeer"}},
    {"name": "err_hs1_no_peer", "bytes_hex": frame(1, 1, 1, None, 0, b"x"),
     "expect": {"error": "MissingPeer"}},
    {"name": "err_close_2byte_body",
     "bytes_hex": frame(3, 0, 0, None, 0, b"\x01") + "02",
     "expect": {"error": "BadCloseBody"}},
    {"name": "err_truncated", "bytes_hex": "01" * 43, "expect": {"error": "Truncated"}},
]
dump("header.json", header_vectors)

# ---- record (§4.4) ----
LENS = [64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65519]


def record(length, flags, mtype, msg_id, total, content: bytes):
    assert length in LENS and len(content) <= length - 12
    r = bytes([flags, mtype]) + struct.pack(">IIH", msg_id, total, len(content)) + content
    return (r + bytes(length - len(r))).hex()


record_vectors = [
    {"name": "json_single", "bytes_hex": record(64, 0x03, 1, 1, 3, b"{}x"),
     "expect": {"first": True, "last": True, "pad_only": False, "mtype": "json",
                "msg_id": 1, "total_len": 3, "content_hex": b"{}x".hex()}},
    {"name": "term_frag_first",
     "bytes_hex": record(64, 0x01, 2, 2, 60, bytes([1] * 52)),
     "expect": {"first": True, "last": False, "pad_only": False, "mtype": "term",
                "msg_id": 2, "total_len": 60, "content_hex": "01" * 52}},
    {"name": "term_frag_last",
     "bytes_hex": record(64, 0x02, 0, 2, 0, bytes([2] * 8)),
     "expect": {"first": False, "last": True, "pad_only": False, "mtype": None,
                "msg_id": 2, "total_len": 0, "content_hex": "02" * 8}},
    {"name": "pad_only_128", "bytes_hex": record(128, 0x04, 0, 0, 0, b""),
     "expect": {"first": False, "last": False, "pad_only": True, "mtype": None,
                "msg_id": 0, "total_len": 0, "content_hex": ""}},
    {"name": "err_bad_length", "bytes_hex": "00" * 63, "expect": {"error": "BadLength"}},
    {"name": "err_reserved_flags",
     "bytes_hex": "0b" + record(64, 0x03, 1, 1, 3, b"{}x")[2:],
     "expect": {"error": "BadFlags"}},
    {"name": "err_msg_id_zero",
     "bytes_hex": record(64, 0x03, 1, 0, 3, b"{}x"), "expect": {"error": "BadMsgId"}},
    {"name": "err_bad_mtype",
     "bytes_hex": record(64, 0x03, 9, 1, 3, b"{}x"), "expect": {"error": "BadMtype"}},
]
# Nonzero padding: valid record with last byte flipped.
pad = bytearray.fromhex(record(64, 0x03, 3, 7, 1, b"h"))
pad[-1] = 1
record_vectors.append({"name": "err_bad_padding", "bytes_hex": bytes(pad).hex(),
                       "expect": {"error": "BadPadding"}})
dump("record.json", record_vectors)

# ---- TERM (§4.4) ----
def term(handle, seq, base, total, flags, exit_code, size, data: bytes):
    t = struct.pack(">IQQQ", handle, seq, base, total) + bytes([flags])
    if exit_code is not None:
        t += struct.pack(">i", exit_code)
    if size is not None:
        t += struct.pack(">HH", *size)
    return (t + data).hex()


term_vectors = [
    {"name": "full", "bytes_hex": term(7, 100, 90, 200, 0x0D, 0, (24, 80), b"\x1b[1mhi"),
     "expect": {"handle": 7, "seq": 100, "base": 90, "total": 200,
                "alive": True, "reset": False, "exit_code": 0,
                "size": [24, 80], "data_hex": b"\x1b[1mhi".hex()}},
    {"name": "minimal_reset", "bytes_hex": term(1, 0, 0, 0, 0x02, None, None, b""),
     "expect": {"handle": 1, "seq": 0, "base": 0, "total": 0,
                "alive": False, "reset": True, "exit_code": None,
                "size": None, "data_hex": ""}},
    {"name": "err_truncated", "bytes_hex": "00" * 28, "expect": {"error": "Truncated"}},
    {"name": "err_bad_flags",
     "bytes_hex": term(1, 0, 0, 0, 0x12, None, None, b""),
     "expect": {"error": "BadFlags"}},
    {"name": "err_counter_too_large",
     "bytes_hex": term(1, 1 << 53, 0, 0, 0x01, None, None, b""),
     "expect": {"error": "CounterTooLarge"}},
]
dump("term.json", term_vectors)

# ---- part (§4.5) ----
PAYLOAD = bytes([9] * 100)
DIGEST = hashlib.sha256(PAYLOAD).hexdigest()
BEGIN = ('part-begin {"part_id":"seed-7","purpose":"seed",'
         f'"stream_handle":3,"total_bytes":100,"part_count":2,"sha256":"{DIGEST}"}}')
C0 = json.dumps({"part_id": "seed-7", "index": 0, "bytes": b64url(bytes([9] * 60))},
                separators=(",", ":"))
C1 = json.dumps({"part_id": "seed-7", "index": 1, "bytes": b64url(bytes([9] * 40))},
                separators=(",", ":"))
part_vectors = [
    {"name": "begin_seed", "content_utf8": BEGIN,
     "expect": {"part_id": "seed-7", "purpose": "seed", "request_id": None,
                "stream_handle": 3, "total_bytes": 100, "part_count": 2,
                "sha256_hex": DIGEST}},
    {"name": "chunk_0", "json_utf8": C0,
     "expect": {"part_id": "seed-7", "index": 0, "bytes_hex": "09" * 60}},
    {"name": "chunk_1", "json_utf8": C1,
     "expect": {"part_id": "seed-7", "index": 1, "bytes_hex": "09" * 40}},
    {"name": "end", "content_utf8": 'part-end {"part_id":"seed-7"}',
     "expect": {"part_id": "seed-7"}},
    {"name": "err_wrong_command", "content_utf8": 'part-end {"part_id":"x"}',
     "begin": False, "expect_error": "NotJson or WrongCommand"},
]
# Over-cap begin (reply over 1 MiB) and bad purpose, as raw contents.
part_vectors += [
    {"name": "err_reply_over_cap",
     "content_utf8": 'part-begin {"part_id":"r","purpose":"reply","total_bytes":2097152,'
                      '"part_count":4,"sha256":"' + "0" * 64 + '"}',
     "expect_error": "BadTotal"},
    {"name": "err_bad_purpose",
     "content_utf8": 'part-begin {"part_id":"p","purpose":"bulk","total_bytes":10,'
                      '"part_count":2,"sha256":"' + "0" * 64 + '"}',
     "expect_error": "BadPurpose"},
    {"name": "err_bad_hash",
     "content_utf8": 'part-begin {"part_id":"p","purpose":"reply","total_bytes":10,'
                      '"part_count":2,"sha256":"ZZ"}',
     "expect_error": "BadHash"},
]
dump("part.json", part_vectors)

# ---- catalog blob (§6.5) ----
WRAP = bytes([3] * 136)
NONCE = bytes([5] * 24)
BODY = b'{"projects":[]}'
CATALOG_JSON = (f'{{"v":1,"desktop":"{DESKTOP}","version":9,"recipients":['
                f'{{"phone":"{PHONE}","wrap":"{b64url(WRAP)}"}}],'
                f'"nonce":"{b64url(NONCE)}","body":"{b64url(BODY)}"}}')
CATALOG_PADDED = CATALOG_JSON + " " * (4096 - len(CATALOG_JSON))
assert len(CATALOG_PADDED) == 4096
catalog_vectors = [
    {"name": "v9_one_recipient", "bytes_hex": CATALOG_PADDED.encode().hex(),
     "expect": {"desktop": DESKTOP, "version": 9,
                "recipients": [{"phone": PHONE, "wrap_hex": WRAP.hex()}],
                "nonce_hex": NONCE.hex(), "body_hex": BODY.hex()}},
    {"name": "err_not_bucket", "bytes_hex": b"{}".hex(), "expect_error": "BadPadding"},
    {"name": "err_bad_version",
     "bytes_hex": (CATALOG_JSON.replace('"v":1', '"v":2', 1)
                   + " " * (4096 - len(CATALOG_JSON))).encode().hex(),
     "expect_error": "BadVersion"},
]
dump("catalog_blob.json", catalog_vectors)

# ---- push plaintext (§8) ----
def push(desktop, nid, issued, kind, dismiss, title: bytes, body: bytes,
         workspace, tab, ids):
    # No cap asserts: the err_* vectors below are over-limit on purpose.
    p = b"\x01" + bytes.fromhex(uuid_hex(desktop)) + nid + struct.pack(">Q", issued)
    p += bytes([kind, dismiss, len(title)]) + title + struct.pack(">H", len(body)) + body
    p += bytes.fromhex(uuid_hex(workspace) + uuid_hex(tab)) + bytes([len(ids)])
    for i in ids:
        p += i
    return (p + bytes(1024 - len(p))).hex()


TITLE_MAX = "é" * 120          # 240 bytes of multibyte UTF-8
BODY_MAX = "ü" * 300           # 600 bytes
assert len(TITLE_MAX.encode()) == 240 and len(BODY_MAX.encode()) == 600
NID = bytes(range(16))
push_max = push(DESKTOP, NID, 1729000000000, 3, 0, TITLE_MAX.encode(),
                BODY_MAX.encode(), WORKSPACE, TAB, [])
assert len(bytes.fromhex(push_max)) == 1024
IDS58 = [bytes([i] * 16) for i in range(58)]
push_dismiss = push(DESKTOP, bytes([2] * 16), 1729000000000, 0, 1, b"", b"",
                    WORKSPACE, TAB, IDS58)
assert len(bytes.fromhex(push_dismiss)) == 1024
push_vectors = [
    {"name": "max_notification", "bytes_hex": push_max,
     "expect": {"desktop_id": DESKTOP, "id_hex": NID.hex(),
                "issued_at_ms": 1729000000000, "kind": 3, "dismiss": False,
                "title": TITLE_MAX, "body": BODY_MAX,
                "workspace_id": WORKSPACE, "tab_id": TAB, "dismiss_ids": []}},
    {"name": "max_dismiss_58", "bytes_hex": push_dismiss,
     "expect": {"desktop_id": DESKTOP, "id_hex": "02" * 16,
                "issued_at_ms": 1729000000000, "kind": 0, "dismiss": True,
                "title": "", "body": "",
                "workspace_id": WORKSPACE, "tab_id": TAB,
                "dismiss_ids": [b.hex() for b in IDS58]}},
    # Over-limit rejections (raw bytes the constructors must refuse).
    {"name": "err_title_241",
     "bytes_hex": push(DESKTOP, NID, 0, 0, 0, b"a" * 241, b"", WORKSPACE, TAB, []),
     "expect_error": "TitleTooLong"},
    {"name": "err_body_601",
     "bytes_hex": push(DESKTOP, NID, 0, 0, 0, b"", b"b" * 601, WORKSPACE, TAB, []),
     "expect_error": "BodyTooLong"},
    {"name": "err_dismiss_59",
     "bytes_hex": push(DESKTOP, NID, 0, 0, 1, b"", b"", WORKSPACE, TAB,
                        [bytes(16)] * 59),
     "expect_error": "TooManyDismiss"},
    {"name": "err_show_with_ids",
     "bytes_hex": push(DESKTOP, NID, 0, 0, 0, b"T", b"B", WORKSPACE, TAB,
                        [bytes(16)]),
     "expect_error": "BadDismissShape"},
    {"name": "err_bad_version",
     "bytes_hex": "02" + push(DESKTOP, NID, 0, 0, 0, b"", b"", WORKSPACE, TAB, [])[2:],
     "expect_error": "BadVersion"},
]
dump("push.json", push_vectors)

# ---- handshake payloads ----
HELLO = b'{"v":1,"caps":["term","files"]}'
PAIRING = json.dumps({"device_name": "Jahsmine's iPhone", "platform": "ios",
                      "push_pub": b64url(bytes([11] * 32))}, separators=(",", ":"))
hs_vectors = [
    {"name": "kk_hello", "json_utf8": HELLO.decode(),
     "expect": {"caps": ["term", "files"]}},
    {"name": "pairing_hello", "json_utf8": PAIRING,
     "expect": {"device_name": "Jahsmine's iPhone", "platform": "ios",
                "push_pub_hex": "0b" * 32}},
    {"name": "err_hello_v2", "json_utf8": '{"v":2,"caps":[]}', "kind": "hello",
     "expect_error": "BadVersion"},
    {"name": "err_pairing_platform", "json_utf8": PAIRING.replace("ios", "web"),
     "kind": "pairing", "expect_error": "BadPlatform"},
]
dump("hs.json", hs_vectors)
print("wrote", OUT)
