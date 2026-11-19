//! Fuzz the §4.4 record parser and the reassembler: decode must never
//! panic, accepted records re-encode byte-exactly, and arbitrary record
//! sequences fed to the reassembler only ever complete valid messages.

#[path = "../common.rs"]
mod common;

use common::{corpus_hex, run, Rng};
use dragent_e2ee::codec::record;

fn check(input: &[u8]) -> Result<(), String> {
    // Whole input as one record.
    if let Ok(r) = record::decode(input) {
        let mut enc = Vec::new();
        record::encode(
            &mut enc, r.len, r.first, r.last, r.pad_only, r.mtype, r.msg_id, r.total_len,
            r.content,
        )
        .map_err(|e| format!("accepted record will not encode: {e:?}"))?;
        if enc != input {
            return Err("re-encode differs".to_string());
        }
    }
    // Slices of the input as a record sequence through the reassembler.
    let mut re = record::Reassembler::new();
    let mut at = 0;
    while at < input.len() {
        // Take a legal-length window when the bytes suggest one, else a blob.
        let take = match input[at..].first() {
            Some(&b) => {
                let lens = record::RECORD_LENS;
                let l = lens[(b as usize) % lens.len()];
                l.min(input.len() - at)
            }
            None => break,
        };
        if take < 1 {
            break;
        }
        let piece = &input[at..at + take];
        at += take;
        if let Ok(r) = record::decode(piece) {
            // Reassembly errors are fine (they are protocol errors on the
            // channel); a completed message must be well-formed.
            if let Ok(Some(m)) = re.feed(&r) {
                if m.content.len() > record::MAX_TOTAL_LEN {
                    return Err("message over 1 MiB".to_string());
                }
            }
        }
    }
    Ok(())
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_hex(&["record.json"]);
    for &l in &[64usize, 128, 1024, 65519] {
        corpus.push(vec![0u8; l]);
        corpus.push(vec![0xffu8; l]);
    }
    // A two-record fragmented message (valid sequence shape).
    let mut a = Vec::new();
    record::encode(&mut a, 64, true, false, false, Some(record::MsgType::Term), 9, 60, &[1u8; 52])
        .unwrap();
    let mut b = Vec::new();
    record::encode(&mut b, 64, false, true, false, None, 9, 0, &[2u8; 8]).unwrap();
    a.extend_from_slice(&b);
    corpus.push(a);
    run("record", corpus, vec![vec![0u8; 63]], 70_000, check)
}
