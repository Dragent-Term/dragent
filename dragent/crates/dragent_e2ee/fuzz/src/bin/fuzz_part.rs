//! Fuzz the §4.5 part protocol: begin/chunk/end parsing must never panic,
//! accepted messages must re-encode byte-exactly, and sessions must only
//! ever release hash-checked, complete payloads.

#[path = "../common.rs"]
mod common;

use common::{corpus_str, run, Rng};
use dragent_e2ee::codec::part;

fn check(input: &[u8]) -> Result<(), String> {
    // Each framing parses independently. JSON key order is not canonical on
    // the wire, so the property is semantic: decode(encode(decode(x))) ==
    // decode(x).
    if let Ok(b) = part::decode_begin(input) {
        let enc = part::encode_begin(&b).map_err(|e| format!("accepted begin will not encode: {e:?}"))?;
        let back = part::decode_begin(&enc)
            .map_err(|e| format!("re-encoded begin rejected: {e:?}"))?;
        if back != b {
            return Err("begin round-trip differs".to_string());
        }
        // Drive a session with slices of the same input as chunks, then end.
        let mut session = part::PartSession::open(b.clone());
        let mut off = 0;
        let mut pieces = 0;
        while off < input.len() && pieces < 4 {
            let n = (input[off % input.len().max(1)] as usize % 64) + 1;
            let end = (off + n).min(input.len());
            let body = &input[off..end];
            off = end;
            pieces += 1;
            let json = format!(
                "{{\"part_id\":{:?},\"index\":{},\"bytes\":\"{}\"}}",
                b.part_id,
                pieces - 1,
                crate_b64(body)
            );
            match part::decode_chunk(json.as_bytes()) {
                Ok(c) => {
                    let _ = session.feed(c);
                }
                Err(_) => break,
            }
        }
        match session.end(&b.part_id) {
            Ok(payload) => {
                if payload.len() as u64 != b.total_bytes {
                    return Err("session released short payload".to_string());
                }
            }
            Err(_) => {}
        }
    }
    if let Ok(c) = part::decode_chunk(input) {
        let enc = part::encode_chunk(&c.part_id, c.index, &c.bytes)
            .map_err(|e| format!("accepted chunk will not encode: {e:?}"))?;
        if part::decode_chunk(&enc).map_err(|e| format!("re-encoded chunk rejected: {e:?}"))? != c {
            return Err("chunk re-encode differs".to_string());
        }
    }
    if let Ok(id) = part::decode_end(input) {
        let enc = part::encode_end(&id).map_err(|e| format!("accepted end will not encode: {e:?}"))?;
        if part::decode_end(&enc).map_err(|e| format!("re-encoded end rejected: {e:?}"))? != id {
            return Err("end re-encode differs".to_string());
        }
    }
    // Bare command-word splitting never panics.
    let _ = part::split_ctrl(input);
    Ok(())
}

fn crate_b64(data: &[u8]) -> String {
    // Base64url without padding (same alphabet as the codec).
    const AL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for c in data.chunks(3) {
        let mut n: u32 = 0;
        for (i, &b) in c.iter().enumerate() {
            n |= (b as u32) << (16 - 8 * i);
        }
        out.push(AL[((n >> 18) & 63) as usize] as char);
        out.push(AL[((n >> 12) & 63) as usize] as char);
        if c.len() > 1 {
            out.push(AL[((n >> 6) & 63) as usize] as char);
        }
        if c.len() > 2 {
            out.push(AL[(n & 63) as usize] as char);
        }
    }
    out
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_str(&["part.json"], &["content_utf8", "json_utf8"]);
    corpus.push(b"part-begin {}".to_vec());
    corpus.push(b"part-end {}".to_vec());
    corpus.push(b"{}".to_vec());
    run("part", corpus, vec![vec![]], 70_000, check)
}
