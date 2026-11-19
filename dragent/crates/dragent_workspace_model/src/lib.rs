//! Small, UI-agnostic domain types for Dragent's engineering workspace.
//!
//! This crate intentionally contains no filesystem, database, model, or UI
//! code. It is the stable vocabulary shared by persistence, the Rust workspace
//! shell, and the future orchestration event stream.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A deliberately conservative bound for in-memory semantic history. Durable
/// persistence owns the complete audit trail; the UI only needs a recent tail.
pub const MAX_LIVE_WORKSPACE_EVENTS: usize = 4_096;
pub const MAX_PROJECT_DOCUMENTS: usize = 20_000;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            PartialEq,
            Eq,
            Hash,
            Serialize,
            Deserialize,
            schemars::JsonSchema,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
    };
}

id_type!(ProjectId);
id_type!(WorkItemId);
id_type!(DocumentId);
id_type!(ArtifactId);
id_type!(WorkBriefId);
id_type!(RequirementId);
id_type!(DecisionId);
id_type!(AcceptanceCriterionId);
id_type!(ConstraintId);
id_type!(RiskId);
id_type!(PlanId);
id_type!(PlanTaskId);
id_type!(OrganizationId);
id_type!(TeamId);
id_type!(RoleDefinitionId);
id_type!(AgentInstanceId);
id_type!(ExecutionLeaseId);
id_type!(ExecutionAttemptId);
id_type!(ExecutionIsolationId);
id_type!(BillingOperationId);
id_type!(WorkContractId);
id_type!(ThreadId);
id_type!(AgentMessageId);
id_type!(EvidenceId);
id_type!(ShapingMemoryNoteId);
id_type!(AttentionItemId);
id_type!(CheckpointId);
id_type!(ActorId);
id_type!(DeviceId);
id_type!(SurfaceId);
id_type!(LayoutTemplateId);
id_type!(LayoutInstanceId);

mod agentic;
mod agentic_actor;
mod agentic_attention;
mod agentic_comms;
mod agentic_contract;
mod agentic_execution;
mod agentic_plan;
mod agentic_shaping;
mod browser;
mod layout;
mod model_coordination;
mod model_documents;
mod model_events;
mod model_state;
#[cfg(test)]
mod model_tests;
mod model_work;
mod review;
mod scheduler;
mod surface;

pub use agentic::*;
pub use browser::*;
pub use layout::*;
pub use model_coordination::*;
pub use model_documents::*;
pub use model_events::*;
pub use model_state::*;
pub use model_work::*;
pub use review::*;
pub use scheduler::*;
pub use surface::*;
