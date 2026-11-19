use serde::{Deserialize, Serialize};
// `web_time::Instant` is `web_time::Instant` on native and a browser clock on
// wasm, so `AgentBlock::created_at` aligns with the terminal `Block`'s timing.
use std::path::PathBuf;
use uuid::Uuid;
use web_time::Instant;

// ── Shared attachment/snapshot types (moved here from the app so both the
// engine core and the UI use one definition; serde shapes unchanged) ─────────

/// Lightweight copy of a completed block — no `Arc`, no UI types.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockContextSnapshot {
    pub block_id: usize,
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub output_text: String,
    pub output_truncated: bool,
    pub failed: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AttachedBlock {
    pub block_id: usize,
    pub snapshot: BlockContextSnapshot,
}

/// Display record kept on a sent user-prompt block (image → thumbnail).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserBlockAttachment {
    pub name: String,
    pub is_image: bool,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// Deserialize `Vec<AgentBlock>` that may be stored in either the new format
/// `[{id, conversation_id, created_at_unix_ms, kind}, ...]` or the legacy format
/// `[AgentBlockKind, ...]` written before `AgentBlock` gained an `id` field.
fn deserialize_agent_block_vec<'de, D>(deserializer: D) -> Result<Vec<AgentBlock>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    let nil = Uuid::nil();
    Ok(raw
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            // New format has both "id" and "kind".
            if v.get("id").is_some() && v.get("kind").is_some() {
                serde_json::from_value::<AgentBlock>(v)
                    .unwrap_or_else(|_| AgentBlock::new(i, nil, AgentBlockKind::Empty))
            } else {
                // Legacy format: the value itself is an AgentBlockKind.
                let kind =
                    serde_json::from_value::<AgentBlockKind>(v).unwrap_or(AgentBlockKind::Empty);
                AgentBlock::new(i, nil, kind)
            }
        })
        .collect())
}

// ── Height estimates (static, Layer 4) ────────────────────────────────────────

pub const AGENT_BLOCK_HEIGHT_QUERY: f32 = 56.0;
pub const AGENT_BLOCK_HEIGHT_APPROVAL: f32 = 136.0;
/// Question card: header + question text + progress dots + keyboard hints,
/// before options.
pub const AGENT_BLOCK_HEIGHT_QUESTION_BASE: f32 = 152.0;
/// Each selectable option row (label + description).
pub const AGENT_BLOCK_HEIGHT_QUESTION_OPTION: f32 = 46.0;
pub const AGENT_BLOCK_HEIGHT_RESULT: f32 = 80.0;
/// Todo card: header (~41px) + body padding (22px). Each todo row adds one
/// unit below. Kept tight to the paint code — the block list trusts these.
pub const AGENT_BLOCK_HEIGHT_TODO_BASE: f32 = 64.0;
/// One todo row: 12.5px text + 6px row spacing.
pub const AGENT_BLOCK_HEIGHT_TODO_ROW: f32 = 22.0;
pub const AGENT_BLOCK_HEIGHT_TOOL_STEP: f32 = 44.0;
pub const AGENT_BLOCK_HEIGHT_THINKING: f32 = 32.0;
/// Base height for a message block (header). Each text line adds 18px.
const AGENT_BLOCK_MESSAGE_BASE: f32 = 56.0;
/// Base height for a plan review block (header + buttons). Each plan line adds 16px.
const AGENT_BLOCK_PLAN_BASE: f32 = 120.0;
/// Reserved height for one Mermaid diagram within a message's height estimate.
/// Diagrams render as a bordered header + image (see `render_mermaid_canvas`),
/// far taller than the ~18px/line used for plain text. Without this, the
/// block-list's pre-layout estimate badly undershoots and the diagram visibly
/// overlaps the block below it for the frame(s) before its real (measured)
/// height propagates back through the async height-index rebuild.
const AGENT_BLOCK_MERMAID_DIAGRAM_ESTIMATE_PX: f32 = 420.0;

/// Count ```mermaid fenced code blocks in raw markdown `text` — same fence
/// syntax `render_mermaid_canvas` reacts to (see terminal/render/markdown.rs).
fn count_mermaid_diagrams(text: &str) -> usize {
    let mut count = 0;
    let mut in_mermaid = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if in_mermaid {
            if trimmed.starts_with("```") {
                in_mermaid = false;
            }
        } else if trimmed
            .strip_prefix("```")
            .is_some_and(|rest| rest.trim().eq_ignore_ascii_case("mermaid"))
        {
            in_mermaid = true;
            count += 1;
        }
    }
    count
}

// ── Tool step status ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToolStepStatus {
    Running,
    Done,
    Error,
}

// ── Approval status ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    /// Approved and the user chose "Allow always" — persisted in PermissionStore.
    ApprovedAlways,
    Rejected,
}

// ── User question (agent-asks-user) ───────────────────────────────────────────

/// One selectable answer to a `UserQuestion`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    /// One-line explanation of what choosing this means. May be empty.
    #[serde(default)]
    pub description: String,
}

fn default_true() -> bool {
    true
}

/// One question in a `UserQuestion` block. Questions are answered one at a
/// time; the card advances to the next as each is resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuestionItem {
    /// Short chip label (e.g. "Auth method"). May be empty.
    #[serde(default)]
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    /// When true the user may pick several options and confirm; otherwise the
    /// first click answers and advances.
    #[serde(default)]
    pub multi_select: bool,
    /// When true (default) the card offers a free-text "Other" field alongside
    /// the options. The model can set it false to force a strict choice.
    #[serde(default = "default_true")]
    pub custom: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum QuestionStatus {
    /// Still collecting answers (see `current` for which question is showing).
    Pending,
    /// Every question answered — the result was posted back to the agent.
    Answered,
    /// The turn was stopped/cleared before all questions were answered.
    Dismissed,
}

/// One entry in the agent's task list (the `todowrite` tool). Statuses mirror
/// opencode: pending / in_progress / completed / cancelled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TodoListItem {
    /// Brief description of the task.
    pub content: String,
    /// Current status: pending, in_progress, completed, cancelled.
    #[serde(default)]
    pub status: String,
    /// Priority: high, medium, low.
    #[serde(default)]
    pub priority: String,
}

// ── Plan status ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlanStatus {
    /// Waiting for the user to click Implement or Refine.
    Pending,
    /// User approved — implementation is underway.
    Approved,
    /// User asked to refine — they are typing a follow-up in the input bar.
    Refining,
}

// ── Error kind ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ErrorKind {
    Auth,
    InsufficientCredits,
    Network,
    Busy,
    Internal,
}

// ── Block kinds ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAuthor {
    pub display_name: String,
    #[serde(default)]
    pub avatar_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentBlockKind {
    UserQuery {
        text: String,
        /// Account that submitted this prompt. Older persisted conversations
        /// omit it and continue to render with the traditional "You" header.
        #[serde(default)]
        author: Option<AgentAuthor>,
        attached_blocks: Vec<AttachedBlock>,
        /// Files the user attached to this message (images/text), kept so the
        /// prompt block shows them after send — a thumbnail or named chip that
        /// records what went with the message. `#[serde(default)]` so older
        /// persisted blocks without the field still load.
        #[serde(default)]
        attachments: Vec<UserBlockAttachment>,
        /// Set once this message's turn makes its first file mutation: the
        /// `git stash create` SHA to restore the worktree to the state BEFORE
        /// this message ran. `None` = the message changed no files, so it has
        /// no restore point (only mutating messages do).
        #[serde(default)]
        checkpoint: Option<String>,
    },
    AgentMessage {
        text: String,
        /// True while the LLM is actively streaming this message. Cleared on Done.
        /// Defaults to false when deserializing persisted blocks from disk.
        #[serde(default)]
        is_streaming: bool,
    },
    /// A system continuation fed to the agent — a finished background command's
    /// output, an auto-resume note, etc. NOT a user prompt: rendered distinctly
    /// and never eligible for a restore checkpoint.
    SystemNote {
        text: String,
    },
    ApprovalRequest {
        action_id: Uuid,
        command: String,
        reason: String,
        status: ApprovalStatus,
    },
    /// The agent is asking the user to choose between options (the `ask_user`
    /// tool). Questions are presented one at a time; answering one advances to
    /// the next, and once all are answered the combined result is posted back
    /// to the agent as the tool result.
    UserQuestion {
        action_id: Uuid,
        questions: Vec<QuestionItem>,
        /// Index of the question currently shown (0-based).
        current: usize,
        /// Selected option labels per question; `answers[i]` matches
        /// `questions[i]`. Filled with empty vecs at creation.
        answers: Vec<Vec<String>>,
        status: QuestionStatus,
        /// Keyboard cursor over the current question's options (Alt+Up/Down).
        /// View-only navigation state; reset when the card advances.
        #[serde(default)]
        cursor: usize,
    },
    ActionResult {
        action_id: Uuid,
        command: String,
        exit_code: Option<i32>,
        output_preview: String,
    },
    /// Live task list from the `todowrite` tool. Replaced wholesale on each
    /// call (opencode parity) — there is at most one current list, rendered
    /// as `# Todos` with per-row status markers.
    TodoList {
        todos: Vec<TodoListItem>,
    },
    /// Glass-box: one step the agent is performing (tool call with live status).
    ToolStep {
        /// Stable ID matching the SSE tool_id so updates find the right block.
        tool_id: String,
        name: String,
        /// Unicode emoji icon, e.g. "📄", "🔍".
        icon: String,
        args_summary: String,
        status: ToolStepStatus,
        result_summary: Option<String>,
        duration_ms: Option<i64>,
        #[serde(default)]
        expanded: bool,
        /// Structured ±diff captured for edit tools, PERSISTED with the block
        /// so diff cards survive app restarts (the in-memory registry is only
        /// the capture path during the live session).
        #[serde(default)]
        diff: Option<super::edit_diff::EditDiff>,
    },
    /// Glass-box: extended thinking/reasoning text from the LLM.
    ThinkingStep {
        /// Accumulated reasoning text (grows as deltas arrive).
        content: String,
        /// When true, the LLM has finished producing reasoning and moved to action.
        complete: bool,
        /// User toggled view state (collapsed vs expanded).
        #[serde(default)]
        expanded: bool,
    },
    /// The LLM produced an implementation plan and is waiting for the user to
    /// approve it before executing any tool calls.
    PlanReview {
        plan_id: Uuid,
        /// The plan text as written by the LLM (marker stripped).
        content: String,
        status: PlanStatus,
    },
    /// A user prompt submitted while the agent was busy, waiting its turn in the
    /// FIFO queue. Renders as a "Queued" pill; becomes a real UserQuery when the
    /// current turn finishes and this one starts. `queue_id` ties it to the
    /// queued entry so it can be cancelled before it runs.
    QueuedMessage {
        queue_id: Uuid,
        text: String,
    },
    /// Transient placeholder shown immediately after a query is submitted while
    /// the server is processing. Removed when the first real event arrives.
    Loading {
        message: String,
    },
    /// Error card shown when the backend or network returns an error.
    Error {
        message: String,
        kind: ErrorKind,
        #[serde(default)]
        task_id: Option<String>,
    },
    Empty,
    ActivityGroup {
        #[serde(deserialize_with = "deserialize_agent_block_vec")]
        blocks: Vec<AgentBlock>,
        #[serde(default)]
        expanded: bool,
    },
    /// Collapsible wrapper for all work done in one agent turn.
    /// While streaming: `duration_ms` is None and the header shows a shimmer.
    /// When done: `duration_ms` is set and the group collapses to "Worked for Xm Ys".
    TurnGroup {
        #[serde(deserialize_with = "deserialize_agent_block_vec")]
        children: Vec<AgentBlock>,
        expanded: bool,
        started_at_ms: i64,
        duration_ms: Option<i64>,
        /// How the turn ENDED when it didn't finish normally ("Stopped",
        /// "Failed"). Drives the header so an interrupted/errored turn never
        /// keeps claiming "Working…".
        #[serde(default)]
        status_note: Option<String>,
    },
}

impl AgentBlockKind {
    pub fn estimated_height_px(&self) -> f32 {
        match self {
            Self::UserQuery {
                attached_blocks, ..
            } => {
                let base = AGENT_BLOCK_HEIGHT_QUERY;
                if attached_blocks.is_empty() {
                    base
                } else {
                    base + 24.0
                }
            }
            Self::SystemNote { text } => {
                let lines = text.lines().count().max(1).min(6);
                AGENT_BLOCK_HEIGHT_THINKING + lines as f32 * 14.0
            }
            Self::AgentMessage { text, .. } => {
                let lines = text.lines().count().max(1).min(20);
                let diagrams = count_mermaid_diagrams(text);
                AGENT_BLOCK_MESSAGE_BASE
                    + lines as f32 * 18.0
                    + diagrams as f32 * AGENT_BLOCK_MERMAID_DIAGRAM_ESTIMATE_PX
            }
            Self::ApprovalRequest { .. } => AGENT_BLOCK_HEIGHT_APPROVAL,
            Self::UserQuestion {
                questions,
                current,
                status,
                ..
            } => match status {
                // Answered/Dismissed collapse to a compact per-question summary.
                QuestionStatus::Answered | QuestionStatus::Dismissed => {
                    AGENT_BLOCK_MESSAGE_BASE + questions.len() as f32 * 22.0
                }
                QuestionStatus::Pending => {
                    let opts = questions
                        .get(*current)
                        .map(|q| q.options.len())
                        .unwrap_or(0);
                    // +1 row for the always-present "Other" free-text field.
                    AGENT_BLOCK_HEIGHT_QUESTION_BASE
                        + (opts as f32 + 1.0) * AGENT_BLOCK_HEIGHT_QUESTION_OPTION
                }
            },
            Self::ActionResult { output_preview, .. } => {
                let lines = output_preview.lines().count().min(5);
                AGENT_BLOCK_HEIGHT_RESULT + lines as f32 * 16.0
            }
            Self::TodoList { todos } => {
                AGENT_BLOCK_HEIGHT_TODO_BASE
                    + todos.len().max(1) as f32 * AGENT_BLOCK_HEIGHT_TODO_ROW
            }
            Self::ToolStep {
                result_summary,
                expanded,
                ..
            } => {
                if *expanded {
                    if let Some(res) = result_summary {
                        let lines = res.lines().count().min(20);
                        AGENT_BLOCK_HEIGHT_TOOL_STEP + lines as f32 * 14.0
                    } else {
                        AGENT_BLOCK_HEIGHT_TOOL_STEP
                    }
                } else {
                    AGENT_BLOCK_HEIGHT_TOOL_STEP
                }
            }
            Self::ThinkingStep {
                content, expanded, ..
            } => {
                if *expanded {
                    let lines = content.lines().count().min(20);
                    AGENT_BLOCK_HEIGHT_THINKING + lines as f32 * 14.0
                } else {
                    AGENT_BLOCK_HEIGHT_THINKING
                }
            }
            Self::PlanReview { content, .. } => {
                let lines = content.lines().count().min(30);
                AGENT_BLOCK_PLAN_BASE + lines as f32 * 16.0
            }
            Self::QueuedMessage { .. } => AGENT_BLOCK_HEIGHT_QUERY,
            Self::Loading { .. } => AGENT_BLOCK_HEIGHT_THINKING,
            Self::Error { message, .. } => {
                let lines = message.lines().count().max(1).min(8);
                AGENT_BLOCK_MESSAGE_BASE + lines as f32 * 18.0
            }
            Self::Empty => 0.0,
            Self::ActivityGroup { blocks, expanded } => {
                if *expanded {
                    blocks
                        .iter()
                        .fold(32.0, |h, b| h + b.kind.estimated_height_px())
                } else {
                    32.0
                }
            }
            Self::TurnGroup {
                children, expanded, ..
            } => {
                if *expanded {
                    children
                        .iter()
                        .fold(32.0, |h, c| h + c.kind.estimated_height_px())
                } else {
                    32.0
                }
            }
        }
    }
}

// ── Agent block ───────────────────────────────────────────────────────────────

fn default_instant() -> Instant {
    Instant::now()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentBlock {
    pub id: usize,
    pub conversation_id: Uuid,
    #[serde(skip, default = "default_instant")]
    pub created_at: Instant,
    pub created_at_unix_ms: i64,
    pub kind: AgentBlockKind,
}

impl AgentBlock {
    pub fn new(id: usize, conversation_id: Uuid, kind: AgentBlockKind) -> Self {
        let created_at_unix_ms = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Self {
            id,
            conversation_id,
            created_at: Instant::now(),
            created_at_unix_ms,
            kind,
        }
    }

    pub fn new_with_timestamp(
        id: usize,
        conversation_id: Uuid,
        kind: AgentBlockKind,
        created_at_unix_ms: i64,
    ) -> Self {
        let now_unix_ms = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let elapsed_ms = (now_unix_ms - created_at_unix_ms).max(0) as u64;
        let created_at = Instant::now()
            .checked_sub(std::time::Duration::from_millis(elapsed_ms))
            .unwrap_or_else(Instant::now);
        Self {
            id,
            conversation_id,
            created_at,
            created_at_unix_ms,
            kind,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_query_height_is_constant() {
        let b = AgentBlockKind::UserQuery {
            text: "hello".into(),
            author: None,
            attached_blocks: Vec::new(),
            attachments: Vec::new(),
            checkpoint: None,
        };
        assert!((b.estimated_height_px() - AGENT_BLOCK_HEIGHT_QUERY).abs() < 0.1);
    }

    #[test]
    fn message_height_scales_with_lines() {
        let one = AgentBlockKind::AgentMessage {
            text: "one line".into(),
            is_streaming: false,
        };
        let three = AgentBlockKind::AgentMessage {
            text: "a\nb\nc".into(),
            is_streaming: false,
        };
        assert!(three.estimated_height_px() > one.estimated_height_px());
    }

    #[test]
    fn approval_height_constant() {
        let b = AgentBlockKind::ApprovalRequest {
            action_id: Uuid::new_v4(),
            command: "echo hi".into(),
            reason: "test".into(),
            status: ApprovalStatus::Pending,
        };
        assert!((b.estimated_height_px() - AGENT_BLOCK_HEIGHT_APPROVAL).abs() < 0.1);
    }

    #[test]
    fn block_ids_are_unique() {
        let conv = Uuid::new_v4();
        let a = AgentBlock::new(
            0,
            conv,
            AgentBlockKind::UserQuery {
                text: "a".into(),
                author: None,
                attached_blocks: Vec::new(),
                attachments: Vec::new(),
                checkpoint: None,
            },
        );
        let b = AgentBlock::new(
            1,
            conv,
            AgentBlockKind::UserQuery {
                text: "b".into(),
                author: None,
                attached_blocks: Vec::new(),
                attachments: Vec::new(),
                checkpoint: None,
            },
        );
        assert_ne!(a.id, b.id);
    }
}
