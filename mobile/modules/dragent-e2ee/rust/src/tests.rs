//! Host tests of the binding's own logic, driven through the exported API
//! against the crate's desktop-side primitives.

use std::sync::Arc;

use dragent_e2ee::channel::{self as ch, kk_listen, PinLookup};
use dragent_e2ee::codec::{catalog_blob, hs, part, push_plain};
use dragent_e2ee::noise::pairing::{PairingResponder, PairingSecret};
use dragent_e2ee::noise::{
    CatalogPrologue, KkPrologue, PairingPrologue, PushPrologue, StaticKeypair,
};
use dragent_e2ee::seal::{catalog, push};
use dragent_e2ee::{ChannelId, Id};
use zeroize::Zeroizing;

use crate::*;

const ACCOUNT: &str = "11111111-2222-4333-8444-555555555555";
const PHONE: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const DESKTOP: &str = "01234567-89ab-4cde-8f01-23456789abcd";

fn id(s: &str) -> Id {
    Id::parse(s).unwrap()
}

fn ids(chan: [u8; 16]) -> ChannelIds {
    ChannelIds {
        account: ACCOUNT.into(),
        phone: PHONE.into(),
        desktop: DESKTOP.into(),
        chan: chan.to_vec(),
    }
}

fn phone_keypair() -> (Arc<Keypair>, StaticKeypair) {
    let k = Keypair::generate().unwrap();
    let copy =
        StaticKeypair::from_secret_bytes(&Zeroizing::new(k.secret_bytes().try_into().unwrap()));
    (k, copy)
}

#[test]
fn keypair_round_trips_and_rejects_bad_lengths() {
    let k = Keypair::generate().unwrap();
    let again = Keypair::from_secret(k.secret_bytes()).unwrap();
    assert_eq!(k.public_key(), again.public_key());
    assert!(matches!(
        Keypair::from_secret(vec![0; 31]),
        Err(E2eeError::InvalidArgument { .. })
    ));
    assert!(!format!("{k:?}").contains(&format!("{:02x}", k.secret_bytes()[0]).repeat(4)));
}

#[test]
fn pairing_flow_against_the_desktop_responder() {
    let (phone, phone_static) = phone_keypair();
    let desktop = StaticKeypair::generate().unwrap();
    let push_key = Keypair::generate().unwrap();
    let p = [7u8; 32];
    let chan = [4u8; 16];
    let start = start_pairing(
        &phone,
        push_key.public_key(),
        desktop.public().as_bytes().to_vec(),
        p.to_vec(),
        ids(chan),
        "Jo's phone".into(),
        PhonePlatform::Ios,
    )
    .unwrap();

    let pro = PairingPrologue {
        account: id(ACCOUNT),
        phone: id(PHONE),
        desktop: id(DESKTOP),
        chan: ChannelId(chan),
    };
    let psk = PairingSecret::from_bytes(Zeroizing::new(p)).psk(&id(DESKTOP));
    let (resp, phone_key, payload) =
        PairingResponder::read_hs1(&desktop, &psk, &pro, &start.hs1).unwrap();
    assert_eq!(phone_key, phone_static.public());
    let hello = hs::decode_pairing(&payload).unwrap();
    assert_eq!(hello.device_name, "Jo's phone");
    assert_eq!(hello.platform, hs::Platform::Ios);
    assert_eq!(hello.push_pub.to_vec(), push_key.public_key());

    let (dt, hs2) = resp.write_hs2().unwrap();
    let chan_obj = start.handshake.finish(hs2, 0).unwrap();
    assert!(matches!(
        start.handshake.finish(vec![], 0),
        Err(E2eeError::Consumed)
    ));
    assert_eq!(chan_obj.state(), ChannelStateKind::Open);

    // pair-confirm {h} as DATA ctr 0; the desktop checks h.
    let mut d = ch::Channel::responder(ch::Purpose::Pairing, ChannelId(chan), dt, 0);
    let h = chan_obj.handshake_hash();
    let confirm = encode_ctrl("pair-confirm".into(), format!("{{\"h\":\"{}\"}}", h.len())).unwrap();
    let recs = encode_records(MessageType::Ctrl, 1, confirm).unwrap();
    let frame = chan_obj.seal_frame(recs[0].clone()).unwrap();
    let f = decode_frame(frame).unwrap();
    assert_eq!((f.kind, f.ctr, f.chan), (FrameKind::Data, 0, chan.to_vec()));
    assert!(matches!(
        d.receive(f.ctr, &f.body).unwrap(),
        ch::Inbound::Confirming(_)
    ));
    assert!(dragent_e2ee::noise::ct_eq(d.handshake_hash(), &h));
}

#[test]
fn pairing_with_wrong_p_fails_at_the_desktop() {
    let (phone, _) = phone_keypair();
    let desktop = StaticKeypair::generate().unwrap();
    let start = start_pairing(
        &phone,
        vec![1; 32],
        desktop.public().as_bytes().to_vec(),
        vec![7; 32],
        ids([4; 16]),
        "x".into(),
        PhonePlatform::Android,
    )
    .unwrap();
    let pro = PairingPrologue {
        account: id(ACCOUNT),
        phone: id(PHONE),
        desktop: id(DESKTOP),
        chan: ChannelId([4; 16]),
    };
    let wrong = PairingSecret::from_bytes(Zeroizing::new([8u8; 32])).psk(&id(DESKTOP));
    assert!(PairingResponder::read_hs1(&desktop, &wrong, &pro, &start.hs1).is_err());
    assert!(start_pairing(
        &phone,
        vec![1; 32],
        vec![0; 32],
        vec![7; 31],
        ids([4; 16]),
        "x".into(),
        PhonePlatform::Ios
    )
    .is_err());
}

fn kk(
    phone: &Keypair,
    phone_static: &StaticKeypair,
    desktop: &StaticKeypair,
) -> (Arc<Channel>, ch::Channel) {
    let chan = [9u8; 16];
    let start = start_kk(
        phone,
        desktop.public().as_bytes().to_vec(),
        ids(chan),
        vec!["term".into()],
    )
    .unwrap();
    let pro = KkPrologue {
        account: id(ACCOUNT),
        phone: id(PHONE),
        desktop: id(DESKTOP),
        chan: ChannelId(chan),
    };
    let l = kk_listen(
        desktop,
        PinLookup::Pinned(phone_static.public()),
        &pro,
        &start.hs1,
        br#"{"v":1,"caps":["seed"]}"#,
        0,
    )
    .unwrap();
    assert_eq!(
        hs::decode_hello(&l.hs1_payload).unwrap().caps,
        vec!["term".to_string()]
    );
    let done = start.handshake.finish(l.hs2, 0).unwrap();
    assert_eq!(done.caps, vec!["seed".to_string()]);
    (done.channel, l.channel)
}

#[test]
fn kk_channel_gap_and_replay_rules_hold_through_the_binding() {
    let (phone, phone_static) = phone_keypair();
    let desktop = StaticKeypair::generate().unwrap();
    let (p, mut d) = kk(&phone, &phone_static, &desktop);
    let hello = p
        .seal(
            encode_records(MessageType::Ctrl, 1, b"hello {}".to_vec())
                .unwrap()
                .remove(0),
        )
        .unwrap();
    assert!(matches!(
        d.receive(hello.ctr, &hello.ciphertext).unwrap(),
        ch::Inbound::Confirming(_)
    ));
    d.confirm().unwrap();

    let r = |n: u32| {
        encode_records(MessageType::Json, n, b"{}".to_vec())
            .unwrap()
            .remove(0)
    };
    let o0 = d.seal(&r(1)).unwrap();
    let o1 = d.seal(&r(2)).unwrap();
    let o2 = d.seal(&r(3)).unwrap();
    let o3 = d.seal(&r(4)).unwrap();
    let re = Reassembler::new();
    match p.receive(o0.ctr, o0.ciphertext.clone()).unwrap() {
        Inbound::Record { record } => assert_eq!(re.feed(record).unwrap().unwrap().content, b"{}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        p.receive(o0.ctr, o0.ciphertext).unwrap(),
        Inbound::Duplicate
    ));
    p.receive(o1.ctr, o1.ciphertext).unwrap();
    // Frame 2 deleted by the relay: frame 3 is a protocol error and the keys go.
    let _ = o2;
    assert!(matches!(
        p.receive(o3.ctr, o3.ciphertext),
        Err(E2eeError::ProtocolError { .. })
    ));
    assert!(!p.has_keys());
    assert_eq!(
        p.state(),
        ChannelStateKind::Closed {
            reason: "protocol_error".into()
        }
    );
    assert!(matches!(p.seal(r(5)), Err(E2eeError::ChannelClosed { .. })));
}

#[test]
fn kk_with_wrong_pin_and_quiesce() {
    let (phone, phone_static) = phone_keypair();
    let desktop = StaticKeypair::generate().unwrap();
    let other = StaticKeypair::generate().unwrap();
    let start = start_kk(
        &phone,
        other.public().as_bytes().to_vec(),
        ids([9; 16]),
        vec![],
    )
    .unwrap();
    let pro = KkPrologue {
        account: id(ACCOUNT),
        phone: id(PHONE),
        desktop: id(DESKTOP),
        chan: ChannelId([9; 16]),
    };
    assert!(kk_listen(
        &desktop,
        PinLookup::Pinned(phone_static.public()),
        &pro,
        &start.hs1,
        b"",
        0
    )
    .is_err());

    let (p, _) = kk(&phone, &phone_static, &desktop);
    p.seal(vec![0; 64]).unwrap();
    let resume = p.quiesce();
    assert_eq!((resume.chan, resume.last_sent_ctr), (vec![9; 16], Some(0)));
    assert!(matches!(p.seal(vec![0; 64]), Err(E2eeError::NotReady)));
    p.close(LocalCloseReason::Rekeyed);
    assert!(!p.has_keys());
}

fn sealed_catalog(
    desktop: &StaticKeypair,
    phone_pub: &dragent_e2ee::noise::PublicKey,
    version: u64,
    json: &[u8],
) -> Vec<u8> {
    let (k, nonce, body) = catalog::seal_body(&id(ACCOUNT), &id(DESKTOP), version, json).unwrap();
    let pro = CatalogPrologue {
        account: id(ACCOUNT),
        desktop: id(DESKTOP),
        phone: id(PHONE),
        version,
    };
    let wrap = catalog::wrap_key(desktop, phone_pub, &pro, &k).unwrap();
    catalog_blob::CatalogBlob {
        desktop: id(DESKTOP),
        version,
        recipients: vec![catalog_blob::Recipient {
            phone: id(PHONE),
            wrap,
        }],
        nonce,
        body,
    }
    .encode()
    .unwrap()
}

fn temp_path(tag: &str) -> String {
    let b = random_bytes(8).unwrap();
    std::env::temp_dir()
        .join(format!(
            "dragent_e2ee_ffi_{tag}_{}",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ))
        .join("state.v1")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn pin_store_catalog_floor_rollback_and_equivocation() {
    let (phone, phone_static) = phone_keypair();
    let desktop = StaticKeypair::generate().unwrap();
    let key = generate_state_key().unwrap();
    let path = temp_path("catalog");
    let store = open_pin_store(path.clone(), key.clone(), 0).unwrap().store;

    // Unpaired desktop: refused.
    let v5 = sealed_catalog(&desktop, &phone_static.public(), 5, br#"{"v":5}"#);
    assert!(matches!(
        store.open_catalog(
            &phone,
            ACCOUNT.into(),
            PHONE.into(),
            DESKTOP.into(),
            v5.clone()
        ),
        Err(E2eeError::WrongSender)
    ));

    // L6: pair-done carries version 9; the old version-5 blob is a rollback.
    store
        .pin_desktop(
            DESKTOP.into(),
            desktop.public().as_bytes().to_vec(),
            "Mac".into(),
            9,
            1,
        )
        .unwrap();
    assert!(matches!(
        store.open_catalog(&phone, ACCOUNT.into(), PHONE.into(), DESKTOP.into(), v5),
        Err(E2eeError::CatalogRollback)
    ));
    let v9 = sealed_catalog(&desktop, &phone_static.public(), 9, br#"{"v":9}"#);
    assert_eq!(
        store
            .open_catalog(
                &phone,
                ACCOUNT.into(),
                PHONE.into(),
                DESKTOP.into(),
                v9.clone()
            )
            .unwrap(),
        CatalogResult::Accepted {
            version: 9,
            json: br#"{"v":9}"#.to_vec()
        }
    );
    assert_eq!(
        store
            .open_catalog(&phone, ACCOUNT.into(), PHONE.into(), DESKTOP.into(), v9)
            .unwrap(),
        // The accepted blob reopens (after a restart the phone has the mark,
        // not the plaintext); the mark stays at 9.
        CatalogResult::Accepted {
            version: 9,
            json: br#"{"v":9}"#.to_vec()
        }
    );
    assert_eq!(store.catalog_high_water(DESKTOP.into()).unwrap(), Some(9));
    let v9b = sealed_catalog(&desktop, &phone_static.public(), 9, br#"{"v":"other"}"#);
    assert!(matches!(
        store.open_catalog(&phone, ACCOUNT.into(), PHONE.into(), DESKTOP.into(), v9b),
        Err(E2eeError::CatalogEquivocation)
    ));
    // Sealed by another key: wrong sender.
    let impostor = StaticKeypair::generate().unwrap();
    let v10 = sealed_catalog(&impostor, &phone_static.public(), 10, b"{}");
    assert!(matches!(
        store.open_catalog(&phone, ACCOUNT.into(), PHONE.into(), DESKTOP.into(), v10),
        Err(E2eeError::WrongSender)
    ));

    // Persist, reopen, and the mark survives; a stale generation is lost.
    let g = store.commit().unwrap();
    assert_eq!(g, 1);
    let again = open_pin_store(path.clone(), key.clone(), g).unwrap();
    assert_eq!(again.bump_keychain_to, None);
    assert_eq!(
        again.store.catalog_high_water(DESKTOP.into()).unwrap(),
        Some(9)
    );
    assert_eq!(again.store.desktops()[0].name, "Mac");
    again.store.commit().unwrap();
    // Crash between rename and keychain bump: accepted, with a bump.
    assert_eq!(
        open_pin_store(path.clone(), key.clone(), 1)
            .unwrap()
            .bump_keychain_to,
        Some(2)
    );
    assert!(matches!(
        open_pin_store(path.clone(), key.clone(), 3),
        Err(E2eeError::StoreLost { .. })
    ));
    let wrong = generate_state_key().unwrap();
    assert!(matches!(
        open_pin_store(path.clone(), wrong, 2),
        Err(E2eeError::StoreLost { .. })
    ));
    // Keychain says a store exists but the file is gone: lost, never empty.
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        open_pin_store(path, key, 2),
        Err(E2eeError::StoreLost { .. })
    ));
}

#[test]
fn pin_store_revoke_and_rotation() {
    let store = open_pin_store(temp_path("revoke"), generate_state_key().unwrap(), 0)
        .unwrap()
        .store;
    let k1 = vec![1u8; 32];
    let k2 = vec![2u8; 32];
    store
        .pin_desktop(DESKTOP.into(), k1.clone(), "Mac".into(), 1, 0)
        .unwrap();
    store
        .announce_rotation(DESKTOP.into(), k2.clone(), 10)
        .unwrap();
    assert_eq!(
        store.candidate_keys(DESKTOP.into(), 11).unwrap(),
        vec![k2.clone(), k1.clone()]
    );
    assert!(!store.confirm_rotation(DESKTOP.into(), k1.clone()).unwrap());
    assert!(store.confirm_rotation(DESKTOP.into(), k2.clone()).unwrap());
    assert_eq!(
        store.lookup(DESKTOP.into()).unwrap(),
        PinState::Pinned { identity_key: k2 }
    );
    assert!(store.revoke(DESKTOP.into(), 20).unwrap());
    assert_eq!(store.lookup(DESKTOP.into()).unwrap(), PinState::Tombstoned);
    assert_eq!(store.catalog_high_water(DESKTOP.into()).unwrap(), None);
    assert!(store.lookup("NOT-A-UUID".into()).is_err());
}

#[test]
fn push_pins_round_trip_and_reject_duplicates() {
    let pins = vec![
        PushPin {
            desktop: DESKTOP.into(),
            key: vec![3; 32],
        },
        PushPin {
            desktop: PHONE.into(),
            key: vec![4; 32],
        },
    ];
    let bytes = encode_push_pins(pins.clone()).unwrap();
    assert_eq!(decode_push_pins(bytes).unwrap(), pins);
    assert!(encode_push_pins(vec![pins[0].clone(), pins[0].clone()]).is_err());
    assert!(decode_push_pins(br#"{"v":2,"pins":[]}"#.to_vec()).is_err());
    assert!(decode_push_pins(br#"{"v":1,"pins":[{"desktop":"x","key":"00"}]}"#.to_vec()).is_err());
}

fn sealed_push(
    desktop: &StaticKeypair,
    push_pub: &dragent_e2ee::noise::PublicKey,
    inner_desktop: &str,
    issued: u64,
) -> Vec<u8> {
    let plain = push_plain::PushPlain::notification(
        id(inner_desktop),
        [5; 16],
        issued,
        2,
        "Build failed",
        "3 tests",
        id(DESKTOP),
        id(PHONE),
    )
    .unwrap()
    .encode()
    .unwrap();
    let pro = PushPrologue {
        account: id(ACCOUNT),
        phone: id(PHONE),
    };
    push::seal(desktop, push_pub, &pro, &plain)
        .unwrap()
        .to_vec()
}

#[test]
fn push_open_checks_sender_inner_id_staleness_and_rotation() {
    let desktop = StaticKeypair::generate().unwrap();
    let (push_key, push_static) = phone_keypair();
    let pins = vec![PushPin {
        desktop: DESKTOP.into(),
        key: desktop.public().as_bytes().to_vec(),
    }];
    let now = 1_000_000_000;
    let good = sealed_push(&desktop, &push_static.public(), DESKTOP, now - 1000);
    assert_eq!(good.len() as u32, push_sealed_len());
    let n = open_push(
        vec![push_key.clone()],
        ACCOUNT.into(),
        PHONE.into(),
        good.clone(),
        pins.clone(),
        now,
    )
    .unwrap();
    assert_eq!(
        (n.title.as_str(), n.body.as_str(), n.desktop_id.as_str()),
        ("Build failed", "3 tests", DESKTOP)
    );

    // Rotation: the previous key still opens it when listed second.
    let (new_key, _) = phone_keypair();
    assert!(open_push(
        vec![new_key.clone(), push_key.clone()],
        ACCOUNT.into(),
        PHONE.into(),
        good.clone(),
        pins.clone(),
        now
    )
    .is_ok());
    assert!(open_push(
        vec![new_key],
        ACCOUNT.into(),
        PHONE.into(),
        good.clone(),
        pins.clone(),
        now
    )
    .is_err());
    // Unpinned sender (after unpair): failure → placeholder.
    assert!(matches!(
        open_push(
            vec![push_key.clone()],
            ACCOUNT.into(),
            PHONE.into(),
            good.clone(),
            vec![],
            now
        ),
        Err(E2eeError::WrongSender)
    ));
    // Pinned key, but the inner desktop_id names another desktop.
    let lying = sealed_push(&desktop, &push_static.public(), PHONE, now);
    assert!(matches!(
        open_push(
            vec![push_key.clone()],
            ACCOUNT.into(),
            PHONE.into(),
            lying,
            pins.clone(),
            now
        ),
        Err(E2eeError::WrongSender)
    ));
    // Older than 24 h.
    let stale = sealed_push(
        &desktop,
        &push_static.public(),
        DESKTOP,
        now - MAX_PUSH_AGE_MS - 1,
    );
    assert!(open_push(
        vec![push_key.clone()],
        ACCOUNT.into(),
        PHONE.into(),
        stale,
        pins.clone(),
        now
    )
    .is_err());
    // Wrong phone in the prologue, truncation, bit flip.
    assert!(open_push(
        vec![push_key.clone()],
        ACCOUNT.into(),
        DESKTOP.into(),
        good.clone(),
        pins.clone(),
        now
    )
    .is_err());
    assert!(open_push(
        vec![push_key.clone()],
        ACCOUNT.into(),
        PHONE.into(),
        good[..1119].to_vec(),
        pins.clone(),
        now
    )
    .is_err());
    let mut flipped = good;
    flipped[500] ^= 1;
    assert!(open_push(
        vec![push_key],
        ACCOUNT.into(),
        PHONE.into(),
        flipped,
        pins,
        now
    )
    .is_err());
}

#[test]
fn codecs_round_trip() {
    let chan = random_channel_id().unwrap();
    let f = encode_hs1_frame(
        FramePurpose::Companion,
        chan.clone(),
        DESKTOP.into(),
        vec![1, 2, 3],
    )
    .unwrap();
    let d = decode_frame(f.clone()).unwrap();
    assert_eq!(
        (d.kind, d.step, d.purpose, d.peer.as_deref()),
        (
            FrameKind::Hs,
            1,
            Some(FramePurpose::Companion),
            Some(DESKTOP)
        )
    );
    assert_eq!(encode_frame(d).unwrap(), f);
    // The body is the rest of the WebSocket message; a short header fails.
    assert!(decode_frame(f[..43].to_vec()).is_err());

    // A 200,000-byte message spans 4 records and reassembles.
    let content: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
    let recs = encode_records(MessageType::Json, 7, content.clone()).unwrap();
    assert_eq!(recs.len(), 4);
    assert!(recs[..3].iter().all(|r| r.len() == 65_519));
    let re = Reassembler::new();
    let mut got = None;
    for r in recs {
        got = re.feed(r).unwrap();
    }
    assert_eq!(
        got.unwrap(),
        Message {
            mtype: MessageType::Json,
            content
        }
    );
    assert_eq!(encode_pad_record(64).unwrap().len(), 64);
    assert!(re.feed(encode_pad_record(64).unwrap()).unwrap().is_none());
    assert!(matches!(
        re.feed(vec![0xff; 64]),
        Err(E2eeError::ProtocolError { .. })
    ));

    let c =
        decode_ctrl(encode_ctrl("bye".into(), r#"{"reason":"rekeyed"}"#.into()).unwrap()).unwrap();
    assert_eq!(
        (c.name.as_str(), c.json.as_str()),
        ("bye", r#"{"reason":"rekeyed"}"#)
    );
    assert!(encode_ctrl("two words".into(), "{}".into()).is_err());
    assert_eq!(sha256(b"abc".to_vec()).len(), 32);
    assert!(random_bytes(1025).is_err());
}

#[test]
fn part_session_applies_atomically() {
    use sha2::Digest;
    let payload = vec![42u8; 3000];
    let sum: [u8; 32] = sha2::Sha256::digest(&payload).into();
    let begin = part::encode_begin(&part::PartBegin {
        part_id: "p1".into(),
        purpose: part::Purpose::Seed,
        request_id: None,
        stream_handle: Some(3),
        total_bytes: 3000,
        part_count: 2,
        sha256: sum,
    })
    .unwrap();
    let s = PartSession::begin(begin.clone()).unwrap();
    assert_eq!(s.info().stream_handle, Some(3));
    s.feed(part::encode_chunk("p1", 1, &payload[1500..]).unwrap())
        .unwrap();
    s.feed(part::encode_chunk("p1", 0, &payload[..1500]).unwrap())
        .unwrap();
    assert_eq!(s.end(part::encode_end("p1").unwrap()).unwrap(), payload);
    assert!(matches!(
        s.end(part::encode_end("p1").unwrap()),
        Err(E2eeError::Consumed)
    ));

    // A missing chunk fails atomically.
    let s = PartSession::begin(begin).unwrap();
    s.feed(part::encode_chunk("p1", 0, &payload[..1500]).unwrap())
        .unwrap();
    assert!(matches!(
        s.end(part::encode_end("p1").unwrap()),
        Err(E2eeError::Rejected { .. })
    ));
}

#[test]
fn benchmarks_run() {
    let n = std::env::var("E2EE_BENCH_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let r = run_native_benchmarks(n).unwrap();
    for b in &r {
        println!("[e2ee-bench] {}: {:.2} µs/op over {}", b.name, b.micros_per_op, b.iterations);
    }
    assert_eq!(r.len(), 6);
    assert!(r.iter().all(|b| b.micros_per_op > 0.0));
}
