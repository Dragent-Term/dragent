//! Codec benchmarks (E1b): `cargo bench -p dragent_e2ee`.
//!
//! Harness-free (`harness = false` in the crate manifest): each section runs a
//! fixed iteration count with [`std::hint::black_box`] and prints ns/op plus
//! throughput. Numbers go to the E1b done summary and §11's acceptance gate:
//! key→echo p95 on a real phone regresses by ≤ 1 ms against plaintext.
//!
//! The crypto halves (Noise seal/open, XChaCha20-Poly1305) belong to E1a and
//! the transport benches (§11 table) to E4; what is timed here is the codec
//! framing around them: header, record, TERM, part, catalog/push blobs and
//! the base64url alphabet.

use std::hint::black_box;
use std::time::Instant;

use dragent_e2ee::codec::{b64, catalog_blob, header, hs, part, push_plain, record, term};
use dragent_e2ee::{ChannelId, Id};

fn bench(name: &str, iters: u64, mut f: impl FnMut()) {
    // Warm up, then time.
    for _ in 0..1000 {
        f();
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ns = start.elapsed().as_nanos() as f64 / iters as f64;
    println!("{name:38} {ns:12.1} ns/op");
}

fn id(s: &str) -> Id {
    Id::parse(s).unwrap()
}

const DESKTOP: &str = "01234567-89ab-4cde-8f01-23456789abcd";
const PHONE: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const ACCOUNT: &str = "11111111-2222-4333-8444-555555555555";
const WS: &str = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
const TAB: &str = "cccccccc-dddd-4eee-8fff-000000000000";

fn main() {
    println!("dragent_e2ee codec benches (release, single thread)");
    let chan = ChannelId([0x42; 16]);
    let peer = id(PHONE);

    // Header: HS1 and a 100-byte DATA frame.
    let hs1 = header::Head {
        kind: header::Kind::Hs,
        step: 1,
        purpose: Some(header::Purpose::Companion),
        chan,
        peer: Some(peer),
        ctr: 0,
        body: &[9u8; 15],
    };
    let mut buf = Vec::new();
    hs1.encode(&mut buf).unwrap();
    let hs1_bytes = buf.clone();
    bench("header encode hs1", 200_000, || {
        let mut out = Vec::new();
        hs1.encode(&mut out).unwrap();
        black_box(out);
    });
    bench("header decode hs1", 200_000, || {
        black_box(header::decode(black_box(&hs1_bytes)).unwrap());
    });
    let data = header::Head {
        kind: header::Kind::Data,
        step: 0,
        purpose: None,
        chan,
        peer: None,
        ctr: 41,
        body: &[7u8; 100],
    };
    let mut buf = Vec::new();
    data.encode(&mut buf).unwrap();
    let data_bytes = buf.clone();
    bench("header decode data", 200_000, || {
        black_box(header::decode(black_box(&data_bytes)).unwrap());
    });

    // Record: 64-byte and full 65,519-byte frames.
    for &len in &[64usize, 65519] {
        let content = vec![0xabu8; len - record::RECORD_OVERHEAD];
        let mut enc = Vec::new();
        record::encode(
            &mut enc, len, true, true, false, Some(record::MsgType::Term), 1,
            content.len() as u32, &content,
        )
        .unwrap();
        let l = enc.len();
        bench(&format!("record encode {l}B"), if len > 4096 { 2_000 } else { 100_000 }, || {
            let mut out = Vec::new();
            record::encode(
                &mut out, len, true, true, false, Some(record::MsgType::Term), 1,
                content.len() as u32, black_box(&content),
            )
            .unwrap();
            black_box(out);
        });
        bench(&format!("record decode {l}B"), if len > 4096 { 2_000 } else { 100_000 }, || {
            black_box(record::decode(black_box(&enc)).unwrap());
        });
    }

    // TERM: a 1 KiB and a near-maximum (~64 KiB) slice.
    for &n in &[1024usize, 65474] {
        let data = vec![0x1bu8; n];
        let head = term::TermHead {
            handle: 7, seq: 100, base: 90, total: 200, alive: true, reset: false,
            exit_code: None, size: Some((24, 80)), data: &data,
        };
        let mut enc = Vec::new();
        head.encode(&mut enc).unwrap();
        bench(&format!("term round {}KB", enc.len() / 1024), if n > 4096 { 5_000 } else { 50_000 }, || {
            let mut out = Vec::new();
            let h = term::TermHead { data: black_box(&data), ..head };
            h.encode(&mut out).unwrap();
            black_box(term::decode(&out).unwrap());
        });
    }

    // Push plaintext: max notification and max dismissal.
    let title = "é".repeat(120);
    let body = "ü".repeat(300);
    let notif = push_plain::PushPlain::notification(
        id(DESKTOP), [7u8; 16], 1_729_000_000_000, 3, &title, &body, id(WS), id(TAB),
    )
    .unwrap();
    let enc_notif = notif.encode().unwrap();
    bench("push encode max-notif", 50_000, || {
        black_box(notif.encode().unwrap());
    });
    bench("push decode max-notif", 50_000, || {
        black_box(push_plain::decode(black_box(&enc_notif)).unwrap());
    });
    let ids58: Vec<[u8; 16]> = (0..58u8).map(|i| [i; 16]).collect();
    let dism = push_plain::PushPlain::dismissal(
        id(DESKTOP), [2u8; 16], 1_729_000_000_000, 0, id(WS), id(TAB), ids58,
    )
    .unwrap();
    let enc_dism = dism.encode().unwrap();
    bench("push round max-dismiss", 50_000, || {
        let p = push_plain::decode(black_box(&enc_dism)).unwrap();
        black_box(p.encode().unwrap());
    });

    // Catalog blob: small catalog encode + decode.
    let blob = catalog_blob::CatalogBlob {
        desktop: id(DESKTOP),
        version: 9,
        recipients: vec![
            catalog_blob::Recipient {
                phone: id(PHONE),
                wrap: vec![3u8; 136],
            },
            catalog_blob::Recipient {
                phone: id(ACCOUNT),
                wrap: vec![4u8; 136],
            },
        ],
        nonce: [5u8; 24],
        body: vec![0x7bu8; 2048],
    };
    let enc_blob = blob.encode().unwrap();
    bench("catalog encode 2K", 5_000, || {
        black_box(blob.encode().unwrap());
    });
    bench("catalog decode 4K-bucket", 5_000, || {
        black_box(catalog_blob::decode(black_box(&enc_blob)).unwrap());
    });

    // Part: begin/end framing and a 64 KiB chunk.
    let digest = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        [9u8; 65536].hash(&mut h);
        let x = h.finish();
        let mut s = [0u8; 32];
        s[..8].copy_from_slice(&x.to_be_bytes());
        s[8..16].copy_from_slice(&x.to_be_bytes());
        s[16..24].copy_from_slice(&x.to_be_bytes());
        s[24..32].copy_from_slice(&x.to_be_bytes());
        s
    };
    let begin = part::PartBegin {
        part_id: "bench-1".to_string(),
        purpose: part::Purpose::Seed,
        request_id: None,
        stream_handle: Some(3),
        total_bytes: 65536,
        part_count: 1,
        sha256: digest,
    };
    let enc_begin = part::encode_begin(&begin).unwrap();
    bench("part begin round", 50_000, || {
        let b = part::decode_begin(black_box(&enc_begin)).unwrap();
        black_box(part::encode_begin(&b).unwrap());
    });
    let chunk_data = vec![9u8; 65536];
    let enc_chunk = part::encode_chunk("bench-1", 0, &chunk_data).unwrap();
    bench("part chunk encode 64K", 2_000, || {
        black_box(part::encode_chunk("bench-1", 0, black_box(&chunk_data)).unwrap());
    });
    bench("part chunk decode 64K", 2_000, || {
        black_box(part::decode_chunk(black_box(&enc_chunk)).unwrap());
    });

    // Handshake payloads.
    let hello = hs::Hello { caps: vec!["term".to_string(), "files".to_string()] };
    let enc_hello = hs::encode_hello(&hello).unwrap();
    bench("hs hello round", 100_000, || {
        let h = hs::decode_hello(black_box(&enc_hello)).unwrap();
        black_box(hs::encode_hello(&h).unwrap());
    });

    // Base64url at the sealed-push size (1,120 bytes -> 1,494 chars).
    let sealed = vec![0x55u8; 1120];
    let enc_sealed = b64::encode(&sealed);
    bench("b64 encode 1120B", 50_000, || {
        black_box(b64::encode(black_box(&sealed)));
    });
    bench("b64 decode 1494ch", 50_000, || {
        black_box(b64::decode(black_box(&enc_sealed)).unwrap());
    });

    println!("done");
}
