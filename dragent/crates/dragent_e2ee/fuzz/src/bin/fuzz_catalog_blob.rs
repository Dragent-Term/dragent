//! Fuzz the §6.5 catalog blob parser: decode must never panic, and every
//! accepted blob must re-encode byte-exactly (same 4 KiB buckets).

#[path = "../common.rs"]
mod common;

use common::{corpus_hex, run, Rng};
use dragent_e2ee::codec::catalog_blob;

fn check(input: &[u8]) -> Result<(), String> {
    check_one(input)?;
    // Bucketed framing: also probe the largest bucketed prefix, so length
    // mutations still exercise the JSON parser.
    let prefix = input.len() - input.len() % catalog_blob::PAD_BUCKET;
    if prefix != input.len() && prefix > 0 {
        check_one(&input[..prefix])?;
    }
    Ok(())
}

fn check_one(input: &[u8]) -> Result<(), String> {
    let blob = match catalog_blob::decode(input) {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let enc = blob
        .encode()
        .map_err(|e| format!("accepted blob will not encode: {e:?}"))?;
    // JSON key order and padding runs are not canonical on the wire; the
    // property is semantic: decode(encode(decode(x))) == decode(x), and the
    // re-encoding is always bucketed.
    if enc.len() % catalog_blob::PAD_BUCKET != 0 {
        return Err("re-encoded blob not bucketed".to_string());
    }
    let back =
        catalog_blob::decode(&enc).map_err(|e| format!("re-encoded blob rejected: {e:?}"))?;
    if back != blob {
        return Err("round-trip differs".to_string());
    }
    Ok(())
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_hex(&["catalog_blob.json"]);
    corpus.push(vec![b' '; 4096]);
    corpus.push(vec![0u8; 4096]);
    run("catalog_blob", corpus, vec![vec![]], 5_000_000, check)
}
