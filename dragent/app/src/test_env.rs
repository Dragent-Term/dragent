//! Helpers for tests that redirect the home folder.
//!
//! A Dev build never writes into other apps' settings, and the tests prove
//! it by pointing `HOME` / `DRAGENT_DEV_HOME` at throwaway folders. Those
//! variables are process-wide, so every test that touches them holds this
//! lock and restores them afterwards. Other tests only read the real home,
//! and keep passing under any value.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Hold while changing process-wide variables.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Set `vars` (a `None` value removes the variable), run `f`, then restore.
pub(crate) fn with_vars<T>(vars: &[(&str, Option<&Path>)], f: impl FnOnce() -> T) -> T {
    let _held = lock();
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = vars
        .iter()
        .map(|(name, _)| (*name, std::env::var_os(name)))
        .collect();
    for (name, value) in vars {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    let result = f();
    for (name, value) in saved {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    result
}
