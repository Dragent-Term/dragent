//! Fuzz the handshake payload parsers (KK hello, pairing hello): decode
//! must never panic, and accepted payloads must re-encode byte-exactly.

#[path = "../common.rs"]
mod common;

use common::{corpus_str, run, Rng};
use dragent_e2ee::codec::hs;

fn check(input: &[u8]) -> Result<(), String> {
    // JSON key order is not canonical on the wire; the property is semantic.
    if let Ok(h) = hs::decode_hello(input) {
        let enc = hs::encode_hello(&h).map_err(|e| format!("accepted hello will not encode: {e:?}"))?;
        if hs::decode_hello(&enc).map_err(|e| format!("re-encoded hello rejected: {e:?}"))? != h {
            return Err("hello round-trip differs".to_string());
        }
    }
    if let Ok(p) = hs::decode_pairing(input) {
        let enc = hs::encode_pairing(&p)
            .map_err(|e| format!("accepted pairing will not encode: {e:?}"))?;
        if hs::decode_pairing(&enc).map_err(|e| format!("re-encoded pairing rejected: {e:?}"))? != p {
            return Err("pairing round-trip differs".to_string());
        }
    }
    Ok(())
}

fn main() -> ! {
    let _ = Rng::seeded(0);
    let mut corpus = corpus_str(&["hs.json"], &["json_utf8"]);
    corpus.push(br#"{"v":1,"caps":[]}"#.to_vec());
    corpus.push(br#"{"device_name":"","platform":"","push_pub":""}"#.to_vec());
    run("hs_payloads", corpus, vec![vec![]], 4_096, check)
}
