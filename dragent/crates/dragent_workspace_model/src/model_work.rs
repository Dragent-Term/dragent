//! Small, UI-agnostic domain types for Dragent's engineering workspace.
//!
//! This crate intentionally contains no filesystem, database, model, or UI
//! code. It is the stable vocabulary shared by persistence, the Rust workspace
//! shell, and the future orchestration event stream.

use std::path::PathBuf;

use crate::ProjectId;
use crate::WorkItemId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A deliberately conservative bound for in-memory semantic history. Durable
/// persistence owns the complete audit trail; the UI only needs a recent tail.

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Project {
    #[serde(default = "local_organisation_id")]
    pub organisation_id: String,
    pub id: ProjectId,
    pub root: PathBuf,
    pub display_name: String,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl Project {
    pub fn new(root: PathBuf, now_unix_ms: i64) -> Self {
        Self::new_for_organisation("local", root, now_unix_ms)
    }

    pub fn new_for_organisation(organisation_id: &str, root: PathBuf, now_unix_ms: i64) -> Self {
        let root_key = root.to_string_lossy();
        let identity = if organisation_id == "local" {
            root_key.to_string()
        } else {
            // Length framing avoids delimiter collisions; retain legacy local IDs.
            format!("{}:{}{}", organisation_id.len(), organisation_id, root_key)
        };
        let display_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Project")
            .to_string();
        Self {
            organisation_id: organisation_id.to_owned(),
            // A project identity is stable across launches and does not need
            // a separate registry or an ever-growing list of random records.
            id: ProjectId(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes())),
            root,
            display_name,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        }
    }
}

fn local_organisation_id() -> String {
    "local".into()
}

#[cfg(test)]
mod organisation_tests {
    use super::*;

    #[test]
    fn project_identity_includes_exact_organisation() {
        let root = PathBuf::from("/same/repository");
        let a = Project::new_for_organisation("Org A", root.clone(), 1);
        let b = Project::new_for_organisation("org a", root.clone(), 1);
        assert_ne!(a.id, b.id);
        assert_eq!(
            a.id,
            Project::new_for_organisation("Org A", root.clone(), 2).id
        );
        assert_eq!(
            Project::new(root.clone(), 1).id,
            Project::new_for_organisation("local", root, 1).id
        );
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    #[default]
    Draft,
    Understanding,
    WaitingForDecision,
    ReadyToPlan,
    Planning,
    ReadyToExecute,
    Executing,
    Reviewing,
    Verifying,
    NeedsUserApproval,
    Completed,
    Blocked,
    Paused,
    Cancelled,
}

impl WorkStatus {
    /// Whether coordination has started: the work left Planning behind, so
    /// the Coordinate view owns it and the Plan action navigates there
    /// instead of re-approving.
    pub fn has_started_coordination(self) -> bool {
        matches!(
            self,
            Self::ReadyToExecute
                | Self::Executing
                | Self::Reviewing
                | Self::Verifying
                | Self::NeedsUserApproval
                | Self::Completed
                | Self::Blocked
                | Self::Paused
        )
    }

    /// Returns whether the lifecycle may move directly from `self` to `next`.
    /// The model may suggest a transition, but only application code applies
    /// this deterministic state machine.
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Draft => matches!(next, Self::Understanding | Self::Cancelled),
            Self::Understanding => matches!(
                next,
                Self::WaitingForDecision
                    | Self::ReadyToPlan
                    | Self::Planning
                    | Self::Paused
                    | Self::Blocked
                    | Self::Cancelled
            ),
            Self::WaitingForDecision => matches!(
                next,
                Self::Understanding
                    | Self::ReadyToPlan
                    | Self::Paused
                    | Self::Blocked
                    | Self::Cancelled
            ),
            Self::ReadyToPlan => matches!(
                next,
                Self::Understanding | Self::Planning | Self::Paused | Self::Cancelled
            ),
            Self::Planning => matches!(
                next,
                Self::Understanding
                    | Self::WaitingForDecision
                    | Self::ReadyToExecute
                    | Self::Paused
                    | Self::Blocked
                    | Self::Cancelled
            ),
            Self::ReadyToExecute => matches!(
                next,
                Self::Planning | Self::Executing | Self::Paused | Self::Cancelled
            ),
            Self::Executing => matches!(
                next,
                Self::Reviewing | Self::Paused | Self::Blocked | Self::Cancelled
            ),
            Self::Reviewing => matches!(
                next,
                Self::Executing
                    | Self::Verifying
                    | Self::NeedsUserApproval
                    | Self::Paused
                    | Self::Blocked
                    | Self::Cancelled
            ),
            Self::Verifying => matches!(
                next,
                Self::Executing
                    | Self::Reviewing
                    | Self::NeedsUserApproval
                    | Self::Completed
                    | Self::Paused
                    | Self::Blocked
                    | Self::Cancelled
            ),
            Self::NeedsUserApproval => matches!(
                next,
                Self::Reviewing | Self::Verifying | Self::Completed | Self::Cancelled
            ),
            Self::Paused | Self::Blocked => matches!(
                next,
                Self::Understanding
                    | Self::Planning
                    | Self::ReadyToExecute
                    | Self::Executing
                    | Self::Reviewing
                    | Self::Verifying
                    | Self::Cancelled
            ),
            Self::Completed | Self::Cancelled => false,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkTransitionError {
    pub from: WorkStatus,
    pub to: WorkStatus,
}

impl std::fmt::Display for WorkTransitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid work lifecycle transition from {:?} to {:?}",
            self.from, self.to
        )
    }
}

impl std::error::Error for WorkTransitionError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkItem {
    pub id: WorkItemId,
    pub project_id: ProjectId,
    pub title: String,
    pub status: WorkStatus,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl WorkItem {
    pub fn new(project_id: ProjectId, title: impl Into<String>, now_unix_ms: i64) -> Self {
        Self {
            id: WorkItemId::new(),
            project_id,
            title: title.into(),
            status: WorkStatus::Draft,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        }
    }

    pub fn transition_to(
        &mut self,
        next: WorkStatus,
        now_unix_ms: i64,
    ) -> Result<(), WorkTransitionError> {
        if !self.status.can_transition_to(next) {
            return Err(WorkTransitionError {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        self.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }
}

pub fn derive_work_item_title(outcome: &str) -> String {
    let first_line = outcome
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Untitled work")
        .trim_start_matches(['#', '-', '*', '>'])
        .trim();
    let normalized = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = normalized.chars().take(72).collect::<String>();
    if title.is_empty() {
        "Untitled work".into()
    } else {
        title
    }
}
