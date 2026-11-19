//! On-device benchmarks for the §11 budget (§12.12): the operations the phone
//! performs, timed natively on the device. Throwaway keys only; nothing here
//! touches stored keys or state. The JS side adds the JSI crossing.
//!
//! Desktop-side steps (the KK responder, the catalog and push seals) run here
//! only to produce real inputs for the phone-side operations being timed.

use std::time::Instant;

use dragent_e2ee::channel::{self as ch, kk_listen, PinLookup};
use dragent_e2ee::codec::{catalog_blob, push_plain, record, term};
use dragent_e2ee::noise::kk::KkInitiator;
use dragent_e2ee::noise::{CatalogPrologue, KkPrologue, PushPrologue, StaticKeypair};
use dragent_e2ee::seal::{catalog, pin_store, push};
use dragent_e2ee::{ChannelId, Id};

use crate::E2eeError;

/// One timed operation.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BenchResult {
    /// What was timed.
    pub name: String,
    /// Iterations run.
    pub iterations: u32,
    /// Mean microseconds per operation.
    pub micros_per_op: f64,
}

fn time<F: FnMut() -> Result<(), E2eeError>>(
    out: &mut Vec<BenchResult>,
    name: &str,
    iterations: u32,
    mut f: F,
) -> Result<(), E2eeError> {
    f()?; // warm-up
    let start = Instant::now();
    for _ in 0..iterations {
        f()?;
    }
    out.push(BenchResult {
        name: name.to_string(),
        iterations,
        micros_per_op: start.elapsed().as_secs_f64() * 1e6 / f64::from(iterations),
    });
    Ok(())
}

fn kk_pair(
    phone: &StaticKeypair,
    desktop: &StaticKeypair,
    pro: &KkPrologue,
) -> Result<(ch::Channel, ch::Channel), E2eeError> {
    let (init, hs1) = KkInitiator::start(phone, &desktop.public(), pro, br#"{"v":1,"caps":[]}"#)?;
    let listened = kk_listen(
        desktop,
        PinLookup::Pinned(phone.public()),
        pro,
        &hs1,
        br#"{"v":1,"caps":[]}"#,
        0,
    )
    .map_err(|_| E2eeError::Internal {
        detail: "bench listen".into(),
    })?;
    let (t, _) = init.read_hs2(&listened.hs2)?;
    let mut p = ch::Channel::initiator(ch::Purpose::Companion, pro.chan, t, 0);
    let mut d = listened.channel;
    // Confirm: phone's ctr 0 is the hello.
    let hello = p.seal(&[0u8; 64])?;
    d.receive(hello.ctr, &hello.ciphertext)?;
    d.confirm()?;
    Ok((p, d))
}

/// Time the phone-side operations of §11 with `iterations` runs each.
#[uniffi::export]
pub fn run_native_benchmarks(iterations: u32) -> Result<Vec<BenchResult>, E2eeError> {
    let iterations = iterations.clamp(1, 100_000);
    let mut out = Vec::new();
    let phone = StaticKeypair::generate()?;
    let desktop = StaticKeypair::generate()?;
    let pro = KkPrologue {
        account: Id([1; 16]),
        phone: Id([2; 16]),
        desktop: Id([3; 16]),
        chan: ChannelId([4; 16]),
    };

    // KK handshake, both sides (the phone's share is about half).
    time(
        &mut out,
        "kk_handshake_both_sides",
        iterations.min(2_000),
        || kk_pair(&phone, &desktop, &pro).map(|_| ()),
    )?;

    let (mut p, mut d) = kk_pair(&phone, &desktop, &pro)?;

    // A keystroke: one 64-byte JSON record, phone seal (with header).
    let mut keystroke = Vec::new();
    record::encode(
        &mut keystroke,
        64,
        true,
        true,
        false,
        Some(record::MsgType::Json),
        1,
        20,
        &[b'x'; 20],
    )
    .map_err(E2eeError::malformed)?;
    time(&mut out, "seal_64b_record", iterations, || {
        let o = p.seal(&keystroke)?;
        d.receive(o.ctr, &o.ciphertext)?;
        Ok(())
    })?;

    // Desktop → phone: open a 64-byte record, then a 64 KiB TERM record.
    let echo = keystroke.clone();
    let mut sealed_small = Vec::with_capacity(iterations as usize + 1);
    for _ in 0..=iterations {
        sealed_small.push(d.seal(&echo)?);
    }
    let mut it = sealed_small.into_iter();
    time(&mut out, "open_64b_record", iterations, || {
        let o = it.next().ok_or(E2eeError::Internal {
            detail: "bench".into(),
        })?;
        p.receive(o.ctr, &o.ciphertext)?;
        Ok(())
    })?;

    let big_iters = iterations.min(2_000);
    let data = vec![b'a'; term::MAX_TERM_LEN - 41];
    let mut payload = Vec::new();
    term::TermHead {
        handle: 1,
        seq: 1,
        base: 0,
        total: data.len() as u64,
        alive: true,
        reset: false,
        exit_code: None,
        size: None,
        data: &data,
    }
    .encode(&mut payload)
    .map_err(E2eeError::malformed)?;
    let mut big = Vec::new();
    record::encode(
        &mut big,
        65_519,
        true,
        true,
        false,
        Some(record::MsgType::Term),
        2,
        payload.len() as u32,
        &payload,
    )
    .map_err(E2eeError::malformed)?;
    let mut sealed_big = Vec::with_capacity(big_iters as usize + 1);
    for _ in 0..=big_iters {
        sealed_big.push(d.seal(&big)?);
    }
    let mut it = sealed_big.into_iter();
    time(
        &mut out,
        "open_64kib_term_record_and_decode",
        big_iters,
        || {
            let o = it.next().ok_or(E2eeError::Internal {
                detail: "bench".into(),
            })?;
            if let ch::Inbound::Record(r) = p.receive(o.ctr, &o.ciphertext)? {
                let rec = record::decode(&r).map_err(E2eeError::malformed)?;
                term::decode(rec.content).map_err(E2eeError::malformed)?;
            }
            Ok(())
        },
    )?;

    // Push: open a sealed notification with the push key (the NSE's work).
    let push_key = StaticKeypair::generate()?;
    let ppro = PushPrologue {
        account: Id([1; 16]),
        phone: Id([2; 16]),
    };
    let plain = push_plain::PushPlain::notification(
        Id([3; 16]),
        [9; 16],
        0,
        1,
        "Agent needs you",
        "Approve the command?",
        Id([5; 16]),
        Id([6; 16]),
    )
    .map_err(E2eeError::malformed)?
    .encode()
    .map_err(E2eeError::malformed)?;
    let sealed = push::seal(&desktop, &push_key.public(), &ppro, &plain)?;
    time(&mut out, "open_push", iterations.min(2_000), || {
        let (_, pt) = push::open(&push_key, &ppro, &sealed)?;
        push_plain::decode(&pt[..]).map_err(E2eeError::malformed)?;
        Ok(())
    })?;

    // Catalog: unwrap and open a ~16 KiB catalog blob.
    let json = vec![b' '; 16 * 1024];
    let (k, nonce, body) = catalog::seal_body(&Id([1; 16]), &Id([3; 16]), 7, &json)?;
    let cpro = CatalogPrologue {
        account: Id([1; 16]),
        desktop: Id([3; 16]),
        phone: Id([2; 16]),
        version: 7,
    };
    let wrap = catalog::wrap_key(&desktop, &phone.public(), &cpro, &k)?;
    let blob = catalog_blob::CatalogBlob {
        desktop: Id([3; 16]),
        version: 7,
        recipients: vec![catalog_blob::Recipient {
            phone: Id([2; 16]),
            wrap,
        }],
        nonce,
        body,
    }
    .encode()
    .map_err(E2eeError::malformed)?;
    time(
        &mut out,
        "open_catalog_16kib",
        iterations.min(2_000),
        || {
            let mut store = pin_store::PinStore::default();
            crate::catalog::open(
                &mut store,
                crate::catalog::OpenArgs {
                    identity: &phone,
                    desktop_pin: &desktop.public(),
                    account: Id([1; 16]),
                    phone: Id([2; 16]),
                    desktop: Id([3; 16]),
                    blob: &blob,
                },
            )
            .map(|_| ())
        },
    )?;
    Ok(out)
}
