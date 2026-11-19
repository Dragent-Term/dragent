//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agentic_attention::require_non_empty;
use crate::is_safe_relative_path;
use crate::DomainValidationError;
use crate::ExecutionLocation;
use crate::StructuredRecordStatus;
use crate::{
    AcceptanceCriterionId, ActorId, AgentInstanceId, ArtifactId, PlanTaskId, RoleDefinitionId,
    TeamId, WorkContractId, WorkItemId,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkContractStatus {
    #[default]
    Draft,
    Proposed,
    Approved,
    Active,
    Completed,
    Revoked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkBudget {
    pub maximum_input_tokens: Option<u64>,
    pub maximum_output_tokens: Option<u64>,
    pub maximum_cost_micros: Option<u64>,
    pub maximum_duration_ms: Option<u64>,
    pub maximum_tool_calls: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CapabilityGrant {
    pub capability: String,
    pub scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PermissionGrant {
    pub permission: String,
    pub scope: Option<String>,
    pub requires_approval: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkContract {
    pub id: WorkContractId,
    pub work_item_id: WorkItemId,
    pub role_definition_id: RoleDefinitionId,
    pub version: u64,
    /// Draft and proposed contracts deliberately have no runtime agent.
    pub agent_instance_id: Option<AgentInstanceId>,
    pub status: WorkContractStatus,
    pub mission: String,
    pub task_ids: Vec<PlanTaskId>,
    pub input_artifact_ids: Vec<ArtifactId>,
    pub expected_outputs: Vec<String>,
    pub success_criteria: Vec<AcceptanceCriterionId>,
    pub write_scope: Vec<PathBuf>,
    pub capabilities: Vec<CapabilityGrant>,
    pub tools: Vec<String>,
    pub permissions: Vec<PermissionGrant>,
    pub budget: WorkBudget,
    pub escalation_rules: Vec<String>,
    pub execution_location: ExecutionLocation,
    /// Optional per-role model route. `None` delegates selection to the
    /// workspace launch policy; explicit routes remain bound to this contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model_route: Option<String>,
    pub created_by: ActorId,
    pub updated_by: ActorId,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl WorkContract {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.version == 0 {
            return Err(DomainValidationError::ZeroVersion("work contract"));
        }
        require_non_empty("work contract mission", &self.mission)?;
        if self.success_criteria.is_empty() {
            return Err(DomainValidationError::SuccessCriteriaRequired);
        }
        if matches!(
            self.status,
            WorkContractStatus::Active | WorkContractStatus::Completed
        ) && self.agent_instance_id.is_none()
        {
            return Err(DomainValidationError::RuntimeAgentRequired);
        }
        if matches!(
            self.status,
            WorkContractStatus::Draft | WorkContractStatus::Proposed
        ) && self.agent_instance_id.is_some()
        {
            return Err(DomainValidationError::RuntimeAgentForbidden);
        }
        for path in &self.write_scope {
            if !is_safe_relative_path(path) {
                return Err(DomainValidationError::InvalidWriteScope { path: path.clone() });
            }
        }
        let mut permission_keys = HashSet::new();
        for grant in &self.permissions {
            let valid = match grant.permission.as_str() {
                "project_read" => grant.scope.is_none() && !grant.requires_approval,
                "project_write" => grant.scope.as_ref().is_some_and(|scope| {
                    let scope = Path::new(scope);
                    is_safe_relative_path(scope)
                        && self.write_scope.iter().any(|path| path == scope)
                }),
                "command_execute" | "agent_delegate" | "external_write" => grant.scope.is_none(),
                _ => false,
            };
            let key = (grant.permission.as_str(), grant.scope.as_deref());
            if !valid || !permission_keys.insert(key) {
                return Err(DomainValidationError::InvalidPermissionGrant);
            }
        }
        if self.preferred_model_route.as_deref().is_some_and(|route| {
            route.trim().is_empty()
                || route.len() > 200
                || route
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
                || route.eq_ignore_ascii_case("auto")
        }) {
            return Err(DomainValidationError::InvalidModelRoute);
        }
        Ok(())
    }
}

/// Whether changing a contract's model route invalidates a prior plan
/// approval. `None` is Auto: a server-resolved route, not a model name, so
/// the server may change defaults later without invalidating anything stored
/// here. Any concrete change (including to/from Auto) affects execution and
/// requires the plan to return to Draft for re-approval.
pub fn contract_route_change_invalidates_approval(
    old_route: Option<&str>,
    new_route: Option<&str>,
) -> bool {
    old_route != new_route
}

/// Tasks pinned to `agent:<key>` run on that agent CLI (Claude Code, Codex,
/// …) in a terminal of their own, on the person's own account with it: they
/// use neither Dragent's model catalog nor its account tier.
pub const AGENT_ROUTE_PREFIX: &str = "agent:";

pub fn agent_route(key: &str) -> String {
    format!("{AGENT_ROUTE_PREFIX}{key}")
}

/// The agent CLI a route pins (`claude`, `codex`, …), if it pins one.
pub fn route_agent(route: &str) -> Option<&str> {
    route
        .strip_prefix(AGENT_ROUTE_PREFIX)
        .filter(|key| !key.is_empty())
}

/// Validate every task's model route before coordination starts.
///
/// - `None` (Auto) is always valid: the server resolves the route for the
///   account tier at execution time. Nothing is hardcoded client-side.
/// - Locked (free-tier) accounts stay on Auto. A stored route equal to the
///   free-tier route is Auto-equivalent — the Principal routinely pins it
///   while fully aware of the account's models, and the locked picker gives
///   the user no way to unselect it — so it passes. Any other concrete
///   route blocks with a reset action.
/// - Paid accounts with a non-empty catalog must use Auto or a catalog route.
///   An empty catalog means the server list is unavailable: stored routes
///   pass here and the execution resolver reports billing failures loudly.
/// - An agent route (`agent:<key>`) passes on any tier, but only while that
///   agent is in the catalog: installed on this computer.
///
/// Messages stay user-facing: no internal task IDs leak into the UI.
pub fn validate_task_model_routes_for_coordination(
    tasks: &[(&str, Option<&str>)],
    catalog_routes: &[String],
    locked_to_free: bool,
    free_tier_route: Option<&str>,
) -> Result<(), String> {
    for &(title, route) in tasks {
        let Some(route) = route else {
            continue;
        };
        if let Some(agent) = route_agent(route) {
            if !catalog_routes.iter().any(|candidate| candidate == route) {
                return Err(format!(
                    "`{title}` is assigned to the `{agent}` agent, which isn't installed on this computer. Pick another agent or reset it to Auto before coordination."
                ));
            }
            continue;
        }
        if locked_to_free {
            let is_free_route = free_tier_route.is_some_and(|free| free == route);
            if !is_free_route {
                return Err(format!(
                    "`{title}` pins model `{route}` but this account is on the free tier. Reset it to Auto (server-resolved) before coordination."
                ));
            }
            continue;
        }
        if !catalog_routes.is_empty() && !catalog_routes.iter().any(|candidate| candidate == route)
        {
            return Err(format!(
                "`{title}` pins model `{route}`, which is not in the current catalog. Pick an available model or reset to Auto before coordination."
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProposedTeamRole {
    pub role_definition_id: RoleDefinitionId,
    pub work_contract_id: WorkContractId,
    pub rationale: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProposedTeam {
    pub id: TeamId,
    pub work_item_id: WorkItemId,
    pub version: u64,
    pub roles: Vec<ProposedTeamRole>,
    pub status: StructuredRecordStatus,
    pub created_by: ActorId,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl ProposedTeam {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.version == 0 {
            return Err(DomainValidationError::ZeroVersion("proposed team"));
        }
        if self.roles.is_empty() {
            return Err(DomainValidationError::ProposedTeamRequired);
        }
        let mut role_definition_ids = HashSet::new();
        let mut work_contract_ids = HashSet::new();
        for role in &self.roles {
            require_non_empty("proposed role rationale", &role.rationale)?;
            if !role_definition_ids.insert(role.role_definition_id)
                || !work_contract_ids.insert(role.work_contract_id)
            {
                return Err(DomainValidationError::DuplicateIdentity(
                    "proposed team role",
                ));
            }
        }
        Ok(())
    }
}
