//! Fuzz the §4.1 frame header: decode must never panic, and every accepted
//! frame must re-encode byte-exactly, with the split-read path agreeing.

#[path = "../common.rs"]
mod common;

use common::{corpus_hex, run, Rng};
use dragent_e2ee::codec::header;

fn check(input: &[u8]) -> Result<(), String> {
    let (f, len) = match header::decode(input) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    if len != input.len() {
        return Err(format!("decode length {len} != input {}", input.len()));
    }
    let head = header::Head {
        kind: f.kind,
        step: f.step,
        purpose: f.purpose,
        chan: f.chan,
        peer: f.peer,
        ctr: f.ctr,
        body: f.body,
    };
    let mut enc = Vec::new();
    head.encode(&mut enc).map_err(|e| format!("accepted frame will not encode: {e:?}"))?;
    if enc != input {
        return Err("re-encode differs".to_string());
    }
    if input.len() >= header::HEADER_LEN {
        let split = header::decode_header(&input[..header::HEADER_LEN], f.body.len())
            .map_err(|e| format!("split path disagrees: {e:?}"))?;
        if (split.kind, split.step, split.purpose, split.chan, split.peer, split.ctr)
            != (f.kind, f.step, f.purpose, f.chan, f.peer, f.ctr)
        {
            return Err("split path disagrees".to_string());
        }
    }
    Ok(())
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_hex(&["header.json"]);
    corpus.push(vec![0u8; 44]);
    corpus.push((0..=255u8).collect());
    run("header", corpus, vec![vec![1, 1, 1, 1]], header::MAX_FRAME_LEN, check)
}
