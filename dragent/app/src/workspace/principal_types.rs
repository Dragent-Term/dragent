//! Principal protocol types: tools, drafts, routes, memory, passes.
//!
//! Moved verbatim from shape_principal; same behavior.

//! Principal shaping turn for the Shape workspace.
//!
//! The model proposes structured work and may research the project with
//! read-only tools (same terminal-agent rhythm: Working → Reading/Searching →
//! Streaming); it still cannot execute commands, edit files, create runtime
//! agents, or grant authority. Parsing and normalization happen off the UI
//! thread before the result reaches workspace state.

use super::principal_normalize::*;
use super::principal_stream::*;
use super::principal_text::*;
use super::principal_validate::*;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize};

use dragent_workspace_model::{AgentRoleKind, Plan, ShapingProposal};

/// Read-only research tools the Principal may call while shaping — the same
/// filesystem search family the terminal agent uses for findings. No write,
/// execute, or grant tools: proposals stay reviewable drafts until approval
/// and the scheduler still owns runtime creation.
pub(crate) const PRINCIPAL_READ_TOOLS: &[&str] = &[
    "read_file",
    "read_multiple_files",
    "list_directory",
    "list_directory_with_sizes",
    "directory_tree",
    "get_file_info",
    "list_allowed_directories",
    "grep",
    "find",
    "search_files",
    "search_codebase",
];

pub(crate) fn is_principal_read_tool(name: &str) -> bool {
    PRINCIPAL_READ_TOOLS.contains(&name)
}

pub(crate) const MAX_ITEMS_PER_SECTION: usize = 20;
pub(crate) const MAX_ITEM_CHARS: usize = 600;
pub(crate) const MAX_PROPOSED_ROLES: usize = 8;
pub(crate) const MAX_PLAN_TASKS: usize = 32;
pub(crate) const MAX_REPOSITORY_REFERENCES: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrincipalRoleDraft {
    #[serde(default, deserialize_with = "deserialize_principal_role_kind")]
    pub kind: AgentRoleKind,
    pub name: String,
    pub mission: String,
    pub rationale: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub write_scope: Vec<String>,
    #[serde(default)]
    pub preferred_model_route: Option<String>,
}

pub(crate) fn deserialize_principal_role_kind<'de, D>(
    deserializer: D,
) -> Result<AgentRoleKind, D::Error>
where
    D: Deserializer<'de>,
{
    let label = String::deserialize(deserializer)?;
    let normalized = label.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    Ok(
        if normalized.contains("review")
            || matches!(
                normalized.as_str(),
                "qa" | "quality_assurance" | "tester" | "verifier"
            )
        {
            AgentRoleKind::Reviewer
        } else {
            // The canonical authority only distinguishes workers from independent
            // reviewers. Architecture, research, implementation, coordination,
            // and other worker specializations remain names/missions rather than
            // new authority-bearing enum variants.
            AgentRoleKind::Worker
        },
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PrincipalTurnKind {
    #[default]
    Conversation,
    Proposal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PrincipalModelRoute {
    pub route: String,
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrincipalPlanTaskDraft {
    /// Stable, proposal-local identity used by `depends_on`.
    pub key: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub review_target: Option<String>,
    pub assigned_role: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrincipalShapeResult {
    #[serde(default)]
    pub turn_kind: PrincipalTurnKind,
    /// Missing/alternate keys (`message`) default to empty and are repaired in
    /// `parse_response` — a dropped field must never discard an otherwise
    /// valid proposal.
    #[serde(default, alias = "message")]
    pub assistant_message: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub open_decisions: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub plan_summary: String,
    #[serde(default)]
    pub plan_tasks: Vec<PrincipalPlanTaskDraft>,
    #[serde(default)]
    pub proposed_roles: Vec<PrincipalRoleDraft>,
    /// Project-relative files that materially informed this proposal. These
    /// become durable document provenance; they are not execution evidence.
    #[serde(default)]
    pub repository_references: Vec<String>,
    /// Gather-mode readiness signal: the model sets this when it has
    /// presented the confirmation summary and believes requirements are
    /// complete. Unlocks the Approve Shape button; never canonical.
    #[serde(default)]
    pub requirements_complete: bool,
    /// Durable work-item memory notes emitted by the model each turn
    /// (decisions, approvals, preferences, changes, rejections). Parsed
    /// tolerantly and stored canonically — this is the Principal's brain,
    /// not conversation.
    #[serde(default)]
    pub memory_notes: Vec<PrincipalMemoryNote>,
}

/// One durable memory fact for the work-item brain. Kinds are fixed so the
/// prompt can group them; free-form kinds collapse to `note`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrincipalMemoryNote {
    pub kind: String,
    pub text: String,
}

/// Canonical memory kinds. Anything else normalizes to `note` — the brain
/// keeps the fact even when the model invents a label.
pub(crate) const MEMORY_KINDS: &[&str] = &[
    "decision",
    "approval",
    "preference",
    "change",
    "rejected",
    "note",
];

pub(crate) const MAX_MEMORY_NOTES_PER_TURN: usize = 10;
pub(crate) const MAX_MEMORY_NOTE_CHARS: usize = 300;

pub(crate) fn normalize_memory_kind(kind: &str) -> String {
    let normalized = kind.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    if MEMORY_KINDS.contains(&normalized.as_str()) {
        normalized
    } else {
        "note".to_string()
    }
}

pub(crate) fn normalize_memory_notes(notes: Vec<PrincipalMemoryNote>) -> Vec<PrincipalMemoryNote> {
    let mut out = Vec::new();
    for note in notes.into_iter().take(MAX_MEMORY_NOTES_PER_TURN) {
        let text = normalize_text(note.text);
        if text.is_empty() {
            continue;
        }
        let text = if text.chars().count() > MAX_MEMORY_NOTE_CHARS {
            text.chars().take(MAX_MEMORY_NOTE_CHARS).collect()
        } else {
            text
        };
        let normalized = PrincipalMemoryNote {
            kind: normalize_memory_kind(&note.kind),
            text,
        };
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    out
}
