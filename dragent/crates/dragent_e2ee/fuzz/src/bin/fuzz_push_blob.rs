//! Fuzz the §8 push plaintext parser: decode must never panic, and every
//! accepted 1,024-byte plaintext must re-encode byte-exactly.

#[path = "../common.rs"]
mod common;

use common::{corpus_hex, run, Rng};
use dragent_e2ee::codec::push_plain;

fn check(input: &[u8]) -> Result<(), String> {
    check_one(input)?;
    // Fixed framing: a mutation that only changes the length never parses,
    // so also probe the 1,024-byte framing of every input. Both are parser
    // inputs the phone's NSE path must handle without panicking.
    if input.len() != push_plain::PLAINTEXT_LEN {
        let mut framed = [0u8; push_plain::PLAINTEXT_LEN];
        let n = input.len().min(framed.len());
        framed[..n].copy_from_slice(&input[..n]);
        check_one(&framed)?;
    }
    Ok(())
}

fn check_one(input: &[u8]) -> Result<(), String> {
    let p = match push_plain::decode(input) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let enc = p
        .encode()
        .map_err(|e| format!("accepted push will not encode: {e:?}"))?;
    if enc.as_slice() != input {
        return Err("re-encode differs".to_string());
    }
    Ok(())
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_hex(&["push.json"]);
    corpus.push(vec![0u8; 1024]);
    corpus.push(vec![0xffu8; 1024]);
    run("push_blob", corpus, vec![vec![]], 4_096, check)
}
