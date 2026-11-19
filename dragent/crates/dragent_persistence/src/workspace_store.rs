//! Durable, metadata-only storage for the engineering workspace projection.
//!
//! Hub: storage constants + re-exports of the store submodules.

pub(crate) const MAX_PERSISTED_EVENTS: i64 = 10_000;
pub(crate) const MAX_RESTORED_WORK_ITEMS: i64 = 100;
pub(crate) const MAX_CANONICAL_REPLAY_EVENTS: i64 = 4_096;
pub(crate) const DEFAULT_MAX_PARALLEL_TASKS: usize = 4;
pub(crate) const MAX_EXECUTION_LEASE_MS: i64 = 15 * 60 * 1_000;

pub use super::store_accept::*;
pub use super::store_integration::*;
pub use super::store_outcomes::*;
pub use super::store_projects::*;
