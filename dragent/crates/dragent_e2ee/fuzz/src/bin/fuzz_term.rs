//! Fuzz the §4.4 TERM parser: decode must never panic, and every accepted
//! payload must re-encode byte-exactly.

#[path = "../common.rs"]
mod common;

use common::{corpus_hex, run, Rng};
use dragent_e2ee::codec::term;

fn check(input: &[u8]) -> Result<(), String> {
    let t = match term::decode(input) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let head = term::TermHead {
        handle: t.handle,
        seq: t.seq,
        base: t.base,
        total: t.total,
        alive: t.alive,
        reset: t.reset,
        exit_code: t.exit_code,
        size: t.size,
        data: t.data,
    };
    let mut enc = Vec::new();
    head.encode(&mut enc).map_err(|e| format!("accepted TERM will not encode: {e:?}"))?;
    if enc != input {
        return Err("re-encode differs".to_string());
    }
    Ok(())
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_hex(&["term.json"]);
    corpus.push(vec![0u8; 29]);
    corpus.push(vec![0xffu8; 64]);
    run("term", corpus, vec![vec![]], 70_000, check)
}
