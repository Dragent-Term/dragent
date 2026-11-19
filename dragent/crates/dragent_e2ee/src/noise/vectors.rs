//! Noise test vectors for KK, IKpsk1 and X (§12.1), run through this crate's
//! own builder and resolver (the zeroising X25519 and ChaChaPoly adapters), and
//! with the transport phase driven in stateless, explicit-nonce mode exactly
//! as the channel uses it.
//!
//! Vectors: `vectors/noise/{kk,ikpsk1,x}.json`, taken unmodified from the
//! cacophony and snow vector sets shipped in the `snow` 0.10.0 package.

use serde_json::Value;

use super::{hs_read, hs_write, Setup, Transport, IKPSK1, KK, X};
use crate::ids::hex_decode_lower;

fn hex(v: &Value) -> Vec<u8> {
    hex_decode_lower(v.as_str().expect("hex string")).expect("lowercase hex")
}

fn opt_hex(v: &Value, key: &str) -> Option<Vec<u8>> {
    v.get(key).filter(|x| !x.is_null()).map(hex)
}

fn psks(v: &Value, key: &str) -> Vec<[u8; 32]> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|p| hex(p).try_into().expect("32-byte psk"))
                .collect()
        })
        .unwrap_or_default()
}

struct Owned {
    prologue: Vec<u8>,
    local: Option<Vec<u8>>,
    remote: Option<Vec<u8>>,
    ephemeral: Option<Vec<u8>>,
    psks: Vec<[u8; 32]>,
}

impl Owned {
    fn from(v: &Value, side: &str) -> Self {
        Owned {
            prologue: opt_hex(v, &format!("{side}_prologue")).unwrap_or_default(),
            local: opt_hex(v, &format!("{side}_static")),
            remote: opt_hex(v, &format!("{side}_remote_static")),
            ephemeral: opt_hex(v, &format!("{side}_ephemeral")),
            psks: psks(v, &format!("{side}_psks")),
        }
    }

    fn build(&self, pattern: &'static str, initiator: bool) -> snow::HandshakeState {
        let mut s = Setup::new(pattern, &self.prologue);
        s.local_secret = self.local.as_deref();
        s.remote_public = self.remote.as_deref();
        s.fixed_ephemeral = self.ephemeral.as_deref();
        if pattern == IKPSK1 {
            assert_eq!(self.psks.len(), 1, "IKpsk1 has exactly one psk");
            s.psk = Some((1, &self.psks[0]));
        } else {
            assert!(self.psks.is_empty());
        }
        s.build(initiator).expect("handshake builds")
    }
}

fn run(file: &str, pattern: &'static str, handshake_messages: usize, one_way: bool) -> usize {
    let doc: Value = serde_json::from_str(file).expect("vector json");
    assert_eq!(doc["protocol_name"], pattern);
    let vectors = doc["vectors"].as_array().expect("vectors");
    assert!(!vectors.is_empty());
    for (vi, v) in vectors.iter().enumerate() {
        assert_eq!(v["protocol_name"], pattern);
        let init_side = Owned::from(v, "init");
        let resp_side = Owned::from(v, "resp");
        let mut init = init_side.build(pattern, true);
        let mut resp = resp_side.build(pattern, false);
        let messages = v["messages"].as_array().expect("messages");
        assert!(messages.len() > handshake_messages);

        // Handshake: messages alternate, initiator first.
        for (i, m) in messages.iter().take(handshake_messages).enumerate() {
            let (payload, expected) = (hex(&m["payload"]), hex(&m["ciphertext"]));
            let (send, recv) = if i % 2 == 0 {
                (&mut init, &mut resp)
            } else {
                (&mut resp, &mut init)
            };
            let ct = hs_write(send, &payload).unwrap();
            assert_eq!(ct, expected, "vector {vi} handshake message {i}");
            assert_eq!(
                &hs_read(recv, &ct).unwrap()[..],
                &payload[..],
                "vector {vi} msg {i}"
            );
        }
        assert!(init.is_handshake_finished() && resp.is_handshake_finished());
        if let Some(h) = opt_hex(v, "handshake_hash") {
            assert_eq!(
                init.get_handshake_hash(),
                &h[..],
                "vector {vi} handshake hash"
            );
            assert_eq!(
                resp.get_handshake_hash(),
                &h[..],
                "vector {vi} handshake hash"
            );
        }

        if one_way {
            // X: transport messages all go initiator -> responder. Our X API
            // is single-message, so drive snow's stateless mode directly.
            let it = init.into_stateless_transport_mode().unwrap();
            let rt = resp.into_stateless_transport_mode().unwrap();
            for (n, m) in messages.iter().skip(handshake_messages).enumerate() {
                let (payload, expected) = (hex(&m["payload"]), hex(&m["ciphertext"]));
                let mut out = vec![0u8; payload.len() + 16];
                let len = it.write_message(n as u64, &payload, &mut out).unwrap();
                assert_eq!(&out[..len], &expected[..], "vector {vi} transport {n}");
                let mut back = vec![0u8; expected.len()];
                let len = rt.read_message(n as u64, &expected, &mut back).unwrap();
                assert_eq!(&back[..len], &payload[..]);
            }
        } else {
            // Two-way: through our Transport with explicit per-direction nonces.
            let it = Transport::from_handshake(init).unwrap();
            let rt = Transport::from_handshake(resp).unwrap();
            let (mut n_ir, mut n_ri) = (0u64, 0u64);
            for (i, m) in messages.iter().enumerate().skip(handshake_messages) {
                let (payload, expected) = (hex(&m["payload"]), hex(&m["ciphertext"]));
                let (send, recv, n) = if i % 2 == 0 {
                    (&it, &rt, &mut n_ir)
                } else {
                    (&rt, &it, &mut n_ri)
                };
                let ct = send.seal(*n, &payload).unwrap();
                assert_eq!(ct, expected, "vector {vi} transport message {i}");
                assert_eq!(&recv.open(*n, &ct).unwrap()[..], &payload[..]);
                *n += 1;
            }
        }
    }
    vectors.len()
}

#[test]
fn noise_kk_vectors() {
    let n = run(include_str!("../../vectors/noise/kk.json"), KK, 2, false);
    assert!(n >= 2);
}

#[test]
fn noise_ikpsk1_vectors() {
    let n = run(
        include_str!("../../vectors/noise/ikpsk1.json"),
        IKPSK1,
        2,
        false,
    );
    assert!(n >= 1);
}

#[test]
fn noise_x_vectors() {
    let n = run(include_str!("../../vectors/noise/x.json"), X, 1, true);
    assert!(n >= 2);
}

/// Run one Noise vector file (`vectors/noise/{kk,ikpsk1,x}.json`) given as
/// JSON text, through this crate's resolver and transport, exactly as the
/// tests do (the `vectors` feature: the phone module runs these on the
/// simulators, §12.1). Panics on the first mismatch; returns the vector count.
pub fn run_json(json: &str) -> usize {
    let doc: Value = serde_json::from_str(json).expect("vector json");
    match doc["protocol_name"].as_str() {
        Some(p) if p == KK => run(json, KK, 2, false),
        Some(p) if p == IKPSK1 => run(json, IKPSK1, 2, false),
        Some(p) if p == X => run(json, X, 1, true),
        other => panic!("unsupported Noise vector file {other:?}"),
    }
}
