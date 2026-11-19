#!/usr/bin/env python3
"""Generate vectors/codec/ctrl.json: the CTRL control messages (src/codec/ctrl.rs).

An independent implementation of the canonical encoding: compact JSON in the
listed field order (json.dumps with separators=(",", ":") and
ensure_ascii=False, which escapes exactly as JSON.stringify does), unpadded
base64url, channel ids as 32 lowercase hex digits. Rejection vectors are
written by hand. Run from anywhere:

    python3 -I vectors/codec/gen_ctrl.py > vectors/codec/ctrl.json
"""

import base64
import json
import sys


def b64(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def body(obj) -> str:
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False)


def content(name: str, obj) -> str:
    return name + " " + body(obj)


CHAN = bytes(range(0x00, 0x10))
CHAN_HEX = CHAN.hex()
H = bytes((i * 7 + 3) & 0xFF for i in range(32))
NEW_PUB = bytes([0x21] * 32)
NEW_PUSH = bytes([0x42] * 32)

ok = []
err = []


def good(name, text, expect):
    ok.append({"name": name, "content_utf8": text, "content_hex": text.encode().hex(), "expect": expect})


def bad(name, raw, error):
    if isinstance(raw, str):
        raw = raw.encode()
    v = {"name": name, "content_hex": raw.hex(), "expect_error": error}
    try:
        v = {"name": name, "content_utf8": raw.decode("utf-8"), **{k: v[k] for k in ("content_hex", "expect_error")}}
    except UnicodeDecodeError:
        pass
    err.append(v)


# ---- valid -----------------------------------------------------------------

good("hello_fresh", content("hello", {}), {"ctrl": "hello", "resume": None})
good(
    "hello_resume",
    content("hello", {"resume": {"chan": CHAN_HEX, "last_sent_ctr": 41}}),
    {"ctrl": "hello", "resume": {"chan": CHAN_HEX, "last_sent_ctr": 41}},
)
good(
    "hello_resume_nothing_sent",
    content("hello", {"resume": {"chan": CHAN_HEX, "last_sent_ctr": None}}),
    {"ctrl": "hello", "resume": {"chan": CHAN_HEX, "last_sent_ctr": None}},
)
good(
    "hello_resume_max_ctr",
    content("hello", {"resume": {"chan": "f" * 32, "last_sent_ctr": 2**32 - 1}}),
    {"ctrl": "hello", "resume": {"chan": "f" * 32, "last_sent_ctr": 2**32 - 1}},
)
good("bye_rekeyed", content("bye", {"reason": "rekeyed"}), {"ctrl": "bye", "reason": "rekeyed"})
good("bye_max_reason_64_bytes", content("bye", {"reason": "é" * 32}), {"ctrl": "bye", "reason": "é" * 32})
good("bye_escapes", content("bye", {"reason": 'a"b\\c\nd\te'}), {"ctrl": "bye", "reason": 'a"b\\c\nd\te'})
good("bye_control_char", content("bye", {"reason": "x\x01y\x1f"}), {"ctrl": "bye", "reason": "x\x01y\x1f"})
good("bye_slash_unescaped", content("bye", {"reason": "a/b"}), {"ctrl": "bye", "reason": "a/b"})
good("pair_confirm", content("pair-confirm", {"h": b64(H)}), {"ctrl": "pair-confirm", "h_hex": H.hex()})
good(
    "pair_done",
    content("pair-done", {"desktop_name": "Jahsmine's MacBook Pro", "catalog_version": 9}),
    {"ctrl": "pair-done", "desktop_name": "Jahsmine's MacBook Pro", "catalog_version": 9},
)
NAME128 = "✓" * 42 + "ab"
assert len(NAME128.encode()) == 128
good(
    "pair_done_max",
    content("pair-done", {"desktop_name": NAME128, "catalog_version": 2**53 - 1}),
    {"ctrl": "pair-done", "desktop_name": NAME128, "catalog_version": 2**53 - 1},
)
good(
    "identity_rotate",
    content("identity-rotate", {"new_pub": b64(NEW_PUB)}),
    {"ctrl": "identity-rotate", "new_pub_hex": NEW_PUB.hex(), "new_push_pub_hex": None},
)
good(
    "identity_rotate_with_push",
    content("identity-rotate", {"new_pub": b64(NEW_PUB), "new_push_pub": b64(NEW_PUSH)}),
    {"ctrl": "identity-rotate", "new_pub_hex": NEW_PUB.hex(), "new_push_pub_hex": NEW_PUSH.hex()},
)
good("input_lost", content("input_lost", {"handles": [1, 2, 7]}), {"ctrl": "input_lost", "handles": [1, 2, 7]})
good("input_lost_empty", content("input_lost", {"handles": []}), {"ctrl": "input_lost", "handles": []})
good(
    "input_lost_u32_bounds",
    content("input_lost", {"handles": [0, 2**32 - 1]}),
    {"ctrl": "input_lost", "handles": [0, 2**32 - 1]},
)
good("input_lost_ack", content("input_lost_ack", {}), {"ctrl": "input_lost_ack"})
# Part-protocol words pass through to codec::part (ctrl = null).
for word in ("part-begin", "part", "part-end", "part-abort"):
    good("passthrough_" + word.replace("-", "_"), word + ' {"part_id":"p1"}', {"ctrl": None})

# ---- framing ---------------------------------------------------------------

bad("err_not_utf8", b"bye {\"reason\":\"\xff\"}", "BadFraming")
bad("err_no_space", "hello{}", "BadFraming")
bad("err_empty_body", "hello ", "BadFraming")
bad("err_empty_name", " {}", "BadFraming")
bad("err_empty", "", "BadFraming")
bad("err_unknown_command", "helo {}", "UnknownCommand")
bad("err_uppercase_command", "HELLO {}", "UnknownCommand")
bad("err_input_lost_hyphen", "input-lost {\"handles\":[]}", "UnknownCommand")

# ---- JSON shape --------------------------------------------------------------

bad("err_hello_unknown_field", 'hello {"v":1}', "NotJson")
bad("err_hello_array", "hello []", "NotJson")
bad("err_hello_trailing_bytes", "hello {}x", "NotJson")
bad("err_hello_two_objects", "hello {}{}", "NotJson")
bad("err_resume_unknown_field", 'hello {"resume":{"chan":"%s","last_sent_ctr":1,"x":0}}' % CHAN_HEX, "NotJson")
bad("err_resume_ctr_negative", 'hello {"resume":{"chan":"%s","last_sent_ctr":-1}}' % CHAN_HEX, "NotJson")
bad("err_resume_ctr_float", 'hello {"resume":{"chan":"%s","last_sent_ctr":1.0}}' % CHAN_HEX, "NotJson")
bad("err_resume_ctr_exponent", 'hello {"resume":{"chan":"%s","last_sent_ctr":1e3}}' % CHAN_HEX, "NotJson")
bad("err_resume_ctr_string", 'hello {"resume":{"chan":"%s","last_sent_ctr":"1"}}' % CHAN_HEX, "NotJson")
bad("err_resume_ctr_leading_zero", 'hello {"resume":{"chan":"%s","last_sent_ctr":01}}' % CHAN_HEX, "NotJson")
bad("err_resume_chan_number", 'hello {"resume":{"chan":5,"last_sent_ctr":1}}', "NotJson")
bad("err_duplicate_field", 'bye {"reason":"a","reason":"b"}', "NotJson")
bad("err_bye_reason_number", 'bye {"reason":7}', "NotJson")
bad("err_bye_missing_reason", "bye {}", "NotJson")
bad("err_pair_confirm_missing", "pair-confirm {}", "NotJson")
bad("err_pair_done_missing_version", 'pair-done {"desktop_name":"Mac"}', "NotJson")
bad("err_pair_done_version_too_big_for_u64", 'pair-done {"desktop_name":"Mac","catalog_version":18446744073709551616}', "NotJson")
bad("err_rotate_missing_new_pub", 'identity-rotate {"new_push_pub":"%s"}' % b64(NEW_PUSH), "NotJson")
bad("err_input_lost_not_array", 'input_lost {"handles":1}', "NotJson")
bad("err_input_lost_negative", 'input_lost {"handles":[-1]}', "NotJson")
bad("err_input_lost_over_u32", 'input_lost {"handles":[4294967296]}', "NotJson")
bad("err_input_lost_ack_field", 'input_lost_ack {"x":1}', "NotJson")
bad("err_input_lost_ack_array", "input_lost_ack []", "NotJson")

# ---- field ranges ------------------------------------------------------------

bad("err_resume_chan_short", 'hello {"resume":{"chan":"%s","last_sent_ctr":1}}' % CHAN_HEX[:30], "BadField")
bad("err_resume_chan_long", 'hello {"resume":{"chan":"%s00","last_sent_ctr":1}}' % CHAN_HEX, "BadField")
bad("err_resume_chan_uppercase", 'hello {"resume":{"chan":"%s","last_sent_ctr":1}}' % ("AB" * 16), "BadField")
bad("err_resume_chan_not_hex", 'hello {"resume":{"chan":"%s","last_sent_ctr":1}}' % ("zz" * 16), "BadField")
bad("err_resume_chan_uuid_form", 'hello {"resume":{"chan":"00010203-0405-0607-0809-0a0b0c0d0e0f","last_sent_ctr":1}}', "BadField")
bad("err_resume_ctr_2_pow_32", 'hello {"resume":{"chan":"%s","last_sent_ctr":4294967296}}' % CHAN_HEX, "BadField")
bad("err_bye_empty_reason", 'bye {"reason":""}', "BadField")
bad("err_bye_reason_65_bytes", content("bye", {"reason": "x" * 65}), "BadField")
bad("err_pair_confirm_padded", content("pair-confirm", {"h": b64(H) + "="}), "BadField")
bad("err_pair_confirm_std_alphabet", content("pair-confirm", {"h": base64.b64encode(bytes([0xFB] * 32)).decode().rstrip("=")}), "BadField")
ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
_last = ALPHABET.index(b64(H)[-1])
assert _last & 3 == 0  # 32 bytes = 43 chars; the last char's low 2 bits are padding
bad("err_pair_confirm_noncanonical_bits", content("pair-confirm", {"h": b64(H)[:-1] + ALPHABET[_last | 1]}), "BadField")
bad("err_pair_confirm_31_bytes", content("pair-confirm", {"h": b64(H[:31])}), "BadField")
bad("err_pair_confirm_33_bytes", content("pair-confirm", {"h": b64(H + b"\x00")}), "BadField")
bad("err_pair_confirm_hex", content("pair-confirm", {"h": H.hex()}), "BadField")
bad("err_pair_done_empty_name", content("pair-done", {"desktop_name": "", "catalog_version": 1}), "BadField")
bad("err_pair_done_name_129_bytes", content("pair-done", {"desktop_name": NAME128 + "c", "catalog_version": 1}), "BadField")
bad("err_pair_done_version_0", content("pair-done", {"desktop_name": "Mac", "catalog_version": 0}), "BadField")
bad("err_pair_done_version_2_pow_53", content("pair-done", {"desktop_name": "Mac", "catalog_version": 2**53}), "BadField")
bad("err_rotate_short_key", content("identity-rotate", {"new_pub": b64(NEW_PUB[:31])}), "BadField")
bad("err_rotate_bad_push", content("identity-rotate", {"new_pub": b64(NEW_PUB), "new_push_pub": b64(NEW_PUSH) + "A"}), "BadField")
bad("err_input_lost_unsorted", content("input_lost", {"handles": [2, 1]}), "BadField")
bad("err_input_lost_duplicate", content("input_lost", {"handles": [1, 1]}), "BadField")
bad("err_input_lost_4097", content("input_lost", {"handles": list(range(4097))}), "BadField")

# ---- canonical form ----------------------------------------------------------

bad("err_hello_space_inside", "hello { }", "NotCanonical")
bad("err_hello_trailing_newline", "hello {}\n", "NotCanonical")
bad("err_two_spaces", "hello  {}", "NotJson")
bad("err_hello_resume_null", 'hello {"resume":null}', "NotCanonical")
bad("err_resume_missing_ctr", 'hello {"resume":{"chan":"%s"}}' % CHAN_HEX, "NotCanonical")
bad("err_resume_field_order", 'hello {"resume":{"last_sent_ctr":1,"chan":"%s"}}' % CHAN_HEX, "NotCanonical")
bad("err_bye_needless_escape", 'bye {"reason":"\\u0061"}', "NotCanonical")
bad("err_bye_escaped_slash", 'bye {"reason":"a\\/b"}', "NotCanonical")
bad("err_bye_uppercase_unicode_escape", 'bye {"reason":"x\\u001Fy"}', "NotCanonical")
bad("err_pair_done_field_order", 'pair-done {"catalog_version":9,"desktop_name":"Mac"}', "NotCanonical")
bad("err_pair_done_spaces", 'pair-done {"desktop_name": "Mac", "catalog_version": 9}', "NotCanonical")
bad("err_rotate_push_null", 'identity-rotate {"new_pub":"%s","new_push_pub":null}' % b64(NEW_PUB), "NotCanonical")
bad("err_input_lost_spaces", 'input_lost {"handles":[1, 2]}', "NotCanonical")

json.dump({"vectors": ok + err}, sys.stdout, indent=2, ensure_ascii=False)
sys.stdout.write("\n")
