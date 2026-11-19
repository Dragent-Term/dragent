//! Repository setup for Dragent workspaces: `.dragent/workspace.json`.
//!
//! A repository describes how to prepare a fresh worktree (`init`), how to
//! start its services (`launch` profiles with an optional readiness probe)
//! and how to stop them (`cleanup`), which environment names must exist and
//! which ignored local files to bring along (`copy`). The local override
//! `.dragent/workspace.local.json` merges over the shared file key by key.
//!
//! Steps run in order and stop at the first failure. Repository commands run
//! only for projects the person trusted; launched processes are recorded
//! with pid and start time, and stopping signals only the recorded process
//! whose start time still matches (pid reuse can never kill something else).

pub mod config;
pub mod copy;
pub mod orchestrate;
pub mod paths;
pub mod platform;
pub mod ready;
pub mod registry;
pub mod run;

pub use config::{
    CopyEntry, LaunchProfile, ReadyCheck, Step, WorkspaceConfig, CONFIG_REL,
    DEFAULT_READY_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS, LOCAL_REL, OUTPUT_TAIL_CHARS, VERSION,
};
pub use paths::{join_inside, resolve_worktree};
