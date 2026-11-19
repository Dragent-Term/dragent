//! §12.1 vectors on the device (feature `vectors`, simulator tests only).
//!
//! Runs `dragent_e2ee`'s own Noise and codec vector runners, compiled for the
//! phone's architecture, over vector files the test bundle passes in as text.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::E2eeError;

// The lead's safeguard: the vector runners (and snow's fixed-ephemeral hook)
// never ship. Release (optimised) builds of this crate can't enable them.
#[cfg(not(debug_assertions))]
compile_error!("the `vectors` feature is for debug simulator test builds only");

/// Run one vector file. `name` is `noise/kk.json`, `noise/ikpsk1.json`,
/// `noise/x.json` or `codec/<file>.json`; `json` is its contents. Returns the
/// number of vectors checked; a mismatch is [`E2eeError::Rejected`] with the
/// runner's message.
#[uniffi::export]
pub fn run_vectors(name: String, json: String) -> Result<u32, E2eeError> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if let Some(file) = name.strip_prefix("codec/") {
            dragent_e2ee::codec::vectors::run_json(file, &json)
        } else if name.starts_with("noise/") {
            dragent_e2ee::noise::vectors::run_json(&json)
        } else {
            panic!("unknown vector set {name}")
        }
    }));
    match result {
        Ok(n) => Ok(n as u32),
        Err(p) => Err(E2eeError::Rejected {
            detail: p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "vector mismatch".into()),
        }),
    }
}
