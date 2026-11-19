//! The one randomness source: the operating system generator through
//! `getrandom` 0.3.4 (§3.2). Nothing in this crate uses any other RNG.

use std::fmt;

/// The operating system's random number generator failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomError;

impl fmt::Display for RandomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the operating system random number generator failed")
    }
}

impl std::error::Error for RandomError {}

/// Fill `dest` from the operating system generator.
pub fn fill_random(dest: &mut [u8]) -> Result<(), RandomError> {
    getrandom::fill(dest).map_err(|_| RandomError)
}

/// `snow`'s RNG hook, backed by [`fill_random`].
pub(crate) struct OsRandom;

impl snow::types::Random for OsRandom {
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), snow::Error> {
        fill_random(dest).map_err(|_| snow::Error::Rng)
    }
}
