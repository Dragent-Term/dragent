//! Scheduled agent runs ("automations"): the data model, five-field cron
//! schedules, persistent storage, and the scheduler that decides when runs
//! start. Executing a run (preparing its workspace, launching its agent)
//! is the host application's job; it reports progress back through
//! [`Scheduler`].

pub mod copy_files;
pub mod cron;
pub mod model;
pub mod scheduler;
#[cfg(not(target_family = "wasm"))]
pub mod store;
#[cfg(target_family = "wasm")]
#[path = "store_wasm.rs"]
pub mod store;

/// Time zones schedules can be read in.
pub use chrono_tz::Tz;
pub use cron::{Period, Repeat, Schedule};
pub use model::{
    codes, local_time_zone, normalize_tags, says_nothing_to_report, AgentState, Automation,
    DefinitionError, MissedPolicy, OverlapPolicy, PromptSource, PromptVersion, Run, RunError,
    RunFilter, RunProgress, RunSnapshot, RunStatus, RunStep, RunTerminal, RunWorkspace, Trigger,
    Watch, WorkspaceMode, DEFAULT_COPY_PATTERNS, DEFAULT_TAG, MAX_RETRIES_LIMIT, NOTHING_TO_REPORT,
    RUN_RETENTION_LIMIT,
};
pub use scheduler::{Active, Reconciled, Scheduler, FREE_AUTOMATION_LIMIT};
pub use store::Store;
