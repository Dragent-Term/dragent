//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ActorId, AgentInstanceId, AgentMessageId, DecisionId, DeviceId, DocumentId, EvidenceId,
    ProjectId, ThreadId,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionLocation {
    #[default]
    Local,
    Cloud,
    Hybrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    PrincipalAgent,
    WorkerAgent,
    ReviewerAgent,
    Service,
    Device,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActorRef {
    pub id: ActorId,
    pub kind: ActorKind,
    pub display_name: String,
    pub device_id: Option<DeviceId>,
}

impl ActorRef {
    /// Stable project-scoped identity used while a local workspace has no
    /// account-backed actor directory. Cloud publication can explicitly map
    /// this actor without changing historical message attribution.
    pub fn local_user(project_id: ProjectId, device_id: Option<DeviceId>) -> Self {
        Self {
            id: ActorId(Uuid::new_v5(&project_id.0, b"actor:local-user")),
            kind: ActorKind::User,
            display_name: "You".into(),
            device_id,
        }
    }

    pub fn principal_agent(project_id: ProjectId) -> Self {
        Self {
            id: ActorId(Uuid::new_v5(&project_id.0, b"actor:principal-agent")),
            kind: ActorKind::PrincipalAgent,
            display_name: "Principal Agent".into(),
            device_id: None,
        }
    }

    /// Stable actor identity for a durable runtime agent instance. The actor
    /// survives retries because the instance, rather than an attempt, owns it.
    pub fn agent_instance(agent_instance_id: AgentInstanceId, kind: ActorKind) -> Self {
        debug_assert!(matches!(
            kind,
            ActorKind::WorkerAgent | ActorKind::ReviewerAgent
        ));
        Self {
            id: ActorId(Uuid::new_v5(&agent_instance_id.0, b"actor:agent-instance")),
            kind,
            display_name: match kind {
                ActorKind::ReviewerAgent => "Reviewer Agent",
                _ => "Worker Agent",
            }
            .into(),
            device_id: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProvenanceSource {
    ManualEdit,
    UserMessage {
        thread_id: ThreadId,
        message_id: AgentMessageId,
    },
    AgentMessage {
        thread_id: ThreadId,
        message_id: AgentMessageId,
    },
    Document {
        document_id: DocumentId,
        relative_path: PathBuf,
    },
    Evidence {
        evidence_id: EvidenceId,
    },
    Decision {
        decision_id: DecisionId,
    },
    AgentFinding {
        actor_id: ActorId,
        summary: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Provenance {
    pub source: ProvenanceSource,
    pub recorded_by: ActorId,
    pub recorded_at_unix_ms: i64,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StructuredRecordStatus {
    #[default]
    Draft,
    Accepted,
    Rejected,
    Superseded,
}
