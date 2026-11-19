//! Agent loop transport: the desktop client owns orchestration
//! (`execution_owner: desktop`, driven by `run_local_checkpoint`); the server
//! executes single provider steps. The legacy server-owned full loop is not
//! used by this client — all turns start through `start_task_stream` (or the
//! scoped variants) below.

pub mod events;
#[cfg(not(target_family = "wasm"))]
mod local_run;
pub(crate) mod step_direct;
pub mod stream;
mod stream_launch;
mod stream_local;
mod stream_recovery;
mod stream_registry;
mod stream_tasks;
mod stream_types;

pub use events::{AgentEventKind, CreateTaskResponse, PipelineEvent};
pub use stream::{
    bounded_tool_output_with_delegate_hint, cancel_local_task, compact_history,
    deliver_tool_result_with_recovery, inspect_local_run_recovery, local_run_ready_to_resume,
    local_tool_call_identity, persist_local_tool_result, post_tool_result,
    post_tool_result_for_execution, read_stream, recover_pending_local_tools, register_host_tools,
    run_subtask_sync, start_recovered_local_run_stream, start_recovered_local_tool_stream,
    start_resumed_task_stream, start_scoped_task_stream, start_scoped_task_stream_with_run_id,
    start_stream_with_retry, start_task_stream, HostToolDef, LocalRunRecoveryState,
    LocalToolCallIdentity, RecoveredLocalTool, ResumeTaskError, StreamError,
};
