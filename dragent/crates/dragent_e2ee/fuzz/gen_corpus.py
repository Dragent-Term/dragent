#!/usr/bin/env python3
"""Write checked-in fuzz seeds: fuzz/corpus/<target>/NNN.bin.

Small deterministic boundary inputs (empty, zeros, 0xff, truncated and
off-by-one framings, plus a few valid vectors). The drivers also seed from
vectors/codec/*.json at runtime; these files are the interesting cases worth
keeping in git. Regenerate with: python3 fuzz/gen_corpus.py
"""
import json, os

ROOT = os.path.dirname(os.path.abspath(__file__))
VEC = os.path.join(ROOT, "..", "vectors", "codec")


def vecs(name):
    with open(os.path.join(VEC, name)) as f:
        return json.load(f)["vectors"]


def corpus(target):
    d = os.path.join(ROOT, "corpus", target)
    os.makedirs(d, exist_ok=True)
    # Wipe only our numbered seeds (leave any hand-added files alone).
    for f in os.listdir(d):
        if f.endswith(".bin"):
            os.remove(os.path.join(d, f))
    return d


def write(d, i, data: bytes):
    assert len(data) <= 70_000, (i, len(data))
    with open(os.path.join(d, f"{i:03d}.bin"), "wb") as f:
        f.write(data)


def hexv(v):
    return bytes.fromhex(v["bytes_hex"])


# header: framing sizes, truncated, off-by-one body, unknown enums.
d = corpus("header")
valid = [v for v in vecs("header.json") if "expect" in v and "error" not in v["expect"]]
write(d, 0, b"")
write(d, 1, bytes(43))
write(d, 2, bytes(44))
write(d, 3, bytes([0xFF] * 44))
write(d, 4, hexv(valid[0])[:43])
write(d, 5, hexv(valid[0]) + b"\x00")
write(d, 6, bytes([1, 9, 9, 9]) + bytes(40))
write(d, 7, hexv(valid[3]))
write(d, 8, hexv(valid[4]))

# record: legal/illegal lengths, truncated, all-flags.
d = corpus("record")
write(d, 0, b"")
write(d, 1, bytes(64))
write(d, 2, bytes([0xFF] * 64))
write(d, 3, bytes(63))
write(d, 4, bytes(65))
write(d, 5, bytes(65519))
write(d, 6, bytes([0xFF] * 128))
write(d, 7, hexv([v for v in vecs("record.json") if v["name"] == "term_frag_first"][0]))
write(d, 8, hexv([v for v in vecs("record.json") if v["name"] == "pad_only_128"][0]))

# term: truncated fixed part, all-flags, max counters.
d = corpus("term")
write(d, 0, b"")
write(d, 1, bytes(28))
write(d, 2, bytes([0xFF] * 29))
write(d, 3, bytes(29) + b"data-follows")
write(d, 4, hexv([v for v in vecs("term.json") if v["name"] == "full"][0]))
write(d, 5, hexv([v for v in vecs("term.json") if v["name"] == "minimal_reset"][0]))

# part: fragments of each framing, empty JSON, wrong commands.
d = corpus("part")
begins = [v["content_utf8"].encode() for v in vecs("part.json") if "content_utf8" in v and "expect" in v or v.get("name") == "begin_seed"]
chunks = [v["json_utf8"].encode() for v in vecs("part.json") if "json_utf8" in v]
write(d, 0, b"")
write(d, 1, b"part-begin ")
write(d, 2, b"part-begin {}")
write(d, 3, b"{}")
write(d, 4, b"part-end {}")
i = 5
for b in begins[:1] + chunks[:2]:
    write(d, i, b)
    i += 1
    write(d, i, b[: len(b) // 2])
    i += 1

# catalog_blob: bucket boundaries, truncated, spaces.
d = corpus("catalog_blob")
write(d, 0, b"")
write(d, 1, bytes(4095))
write(d, 2, bytes(4096))
write(d, 3, bytes([0xFF] * 4096))
write(d, 4, b" " * 4096)
write(d, 5, hexv([v for v in vecs("catalog_blob.json") if v["name"] == "v9_one_recipient"][0])[:100])
write(d, 6, hexv([v for v in vecs("catalog_blob.json") if v["name"] == "v9_one_recipient"][0]))

# push_blob: exact/truncated/extended framings, field-boundary flips.
d = corpus("push_blob")
pv = [v for v in vecs("push.json") if v["name"] == "max_notification"][0]
full = hexv(pv)
write(d, 0, b"")
write(d, 1, bytes(1024))
write(d, 2, bytes([0xFF] * 1024))
write(d, 3, full)
write(d, 4, full[:1023])
write(d, 5, full + b"\x00")
write(d, 6, bytes([1]) + full[1:])
write(d, 7, full[:44] + bytes([241]) + full[45:])

# hs_payloads: truncated, wrong types, near-valid.
d = corpus("hs_payloads")
write(d, 0, b"")
write(d, 1, b"{}")
write(d, 2, b'{"v":1,"caps":[]}')
write(d, 3, b'{"v":1,"caps":[]'[:10])
write(d, 4, b'{"device_name":"P","platform":"ios","push_pub":""}')
write(d, 5, b"\x00" * 64)

print("corpus written")
