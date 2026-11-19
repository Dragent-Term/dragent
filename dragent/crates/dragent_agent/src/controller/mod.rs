//! Agent controller role type plus the ctx-free controller helpers.
//!
//! Thin-slice extraction (Phase 5e): only pure helpers live here so far —
//! tool labels, ask_user/todowrite parsing, session/checkpoint/event helpers.
//! Everything WarpUI-bound (`AgentController` struct, `ModelContext` pumps,
//! `ctx.emit` fan-out, `ctx.spawn` timers) stays app-side until the
//! `ControllerDeps` + `EventSink` seam lands. The viewer and app re-export
//! these paths so existing `crate::agent::controller::*` imports resolve to
//! one definition.

pub mod blocks;
pub mod checkpoint_helpers;
pub mod checkpoints;
pub mod compaction;
pub mod core;
pub mod event_handlers_text;
pub mod event_handlers_tools;
pub mod event_helpers;
pub mod events;
pub mod loop_guard;
pub mod question_parse;
pub mod questions;
pub mod retry;
pub mod run;
pub mod session;
pub mod session_helpers;
pub mod subagent;
pub mod submit;
pub mod tools;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum AgentRole {
    #[default]
    Default,
    Coordinator,
    Worker,
}

impl AgentRole {
    /// Structured system prompt section injected for Coordinator mode.
    pub fn system_prompt_suffix(&self) -> &'static str {
        match self {
            Self::Default | Self::Worker => "",
            Self::Coordinator => {
                "\n\n\
## Coordinator mode

You are the **planning and orchestration layer**. You coordinate work but do NOT execute \
tasks directly. Write/exec tools (write_file, edit_file, batch_edit, apply_patch, run_command) \
are not available to you — delegate all execution to Worker sub-agents via `spawn_subagent`.

### Workflow

**Phase 1 — Research**: For broad exploration, delegate: launch up to 3 explore workers \
IN PARALLEL in a single turn (one `spawn_subagent` call per worker, `role: explore` for \
research, `role: worker` only when the subtask must change files). Each worker prompt \
must be self-contained — the worker sees none of your context. Do narrow lookups \
yourself; fan out anything that touches more than two or three files.

**Phase 2 — Synthesis**: Analyse findings. Identify which files need changing and why. \
Plan the work as discrete, self-contained tasks.

**Phase 3 — Implementation**: For each task, call `spawn_subagent` with a fully \
self-contained prompt. Each worker prompt must include all context the worker needs — \
file paths, exact changes required, acceptance criteria. Independent tasks may run in \
parallel — launch their workers in the same turn instead of one at a time. Workers \
cannot communicate with each other, so do not refer to \"the previous worker's output\" \
unless you explicitly include that output in the next worker's prompt. Trust a worker's \
returned summary; only re-verify its claims with your own tools when the risk warrants it.

**Phase 4 — Verification**: After all workers complete, run the project-appropriate \
build/test/lint/typecheck commands (exit 0 required). Spot-check changed files with \
read_file/grep only as a supplement — not a substitute for a green check. Report the \
outcome in a few lines. The execution cards already show commands, exit codes, and \
diffs — do not repeat them."
            }
        }
    }
}
