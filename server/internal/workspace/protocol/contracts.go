// Package protocol defines the cloud mirror of the versioned agentic-workspace
// wire contract. Keep its JSON shape compatible with
// dragent_session_protocol::workspace and the shared fixtures under
// contracts/workspace.
package protocol

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	pathpkg "path"
	"strings"
	"unicode"

	"github.com/google/uuid"
)

const SchemaVersion = 1

const (
	CommandCreateWorkItem              = "create_work_item"
	CommandTransitionWork              = "transition_work"
	CommandTransitionPlanTask          = "transition_plan_task"
	CommandProvisionAgentInstance      = "provision_agent_instance"
	CommandActivateAgentInstance       = "activate_agent_instance"
	CommandRenewExecutionLease         = "renew_execution_lease"
	CommandPrepareAgentExecutionRetry  = "prepare_agent_execution_retry"
	CommandStartAgentExecution         = "start_agent_execution"
	CommandFinishAgentExecution        = "finish_agent_execution"
	CommandCancelAgentExecution        = "cancel_agent_execution"
	CommandPauseAgentExecution         = "pause_agent_execution"
	CommandResolveAgentExecutionReview = "resolve_agent_execution_review"
	CommandUpsertWorkBrief             = "upsert_work_brief"
	CommandUpsertRequirement           = "upsert_requirement"
	CommandUpsertDecision              = "upsert_decision"
	CommandUpsertAcceptanceCriterion   = "upsert_acceptance_criterion"
	CommandUpsertArtifact              = "upsert_artifact"
	CommandReplaceShapingRecords       = "replace_shaping_records"
	CommandReplaceArtifacts            = "replace_artifacts"
	CommandUpsertConstraint            = "upsert_constraint"
	CommandUpsertRisk                  = "upsert_risk"
	CommandUpsertPlan                  = "upsert_plan"
	CommandRevisePlan                  = "revise_plan"
	CommandUpsertRoleDefinition        = "upsert_role_definition"
	CommandUpsertWorkContract          = "upsert_work_contract"
	CommandUpsertProposedTeam          = "upsert_proposed_team"
	CommandDescribeRole                = "describe_role"
	CommandPostMessage                 = "post_message"
	CommandUpdateMessageDelivery       = "update_message_delivery"
	CommandRecordEvidence              = "record_evidence"
	CommandCompleteShapingTurn         = "complete_shaping_turn"
	CommandRaiseAttention              = "raise_attention"
	CommandResolveAttention            = "resolve_attention"
	EventWorkCreated                   = "work_created"
	EventWorkStatusChanged             = "work_status_changed"
	EventWorkBriefUpdated              = "work_brief_updated"
	EventRequirementUpdated            = "requirement_updated"
	EventDecisionUpdated               = "decision_updated"
	EventAcceptanceCriterionUpdated    = "acceptance_criterion_updated"
	EventArtifactUpdated               = "artifact_updated"
	EventArtifactsReplaced             = "artifacts_replaced"
	EventConstraintUpdated             = "constraint_updated"
	EventRiskUpdated                   = "risk_updated"
	EventPlanUpdated                   = "plan_updated"
	EventPlanTaskStatusChanged         = "plan_task_status_changed"
	EventAgentInstanceProvisioned      = "agent_instance_provisioned"
	EventAgentInstanceActivated        = "agent_instance_activated"
	EventExecutionLeaseRenewed         = "execution_lease_renewed"
	EventAgentExecutionRetryPrepared   = "agent_execution_retry_prepared"
	EventAgentExecutionStarted         = "agent_execution_started"
	EventAgentExecutionFinished        = "agent_execution_finished"
	EventAgentExecutionReviewResolved  = "agent_execution_review_resolved"
	EventRoleDefinitionUpdated         = "role_definition_updated"
	EventWorkContractUpdated           = "work_contract_updated"
	EventProposedTeamUpdated           = "proposed_team_updated"
	EventMessagePosted                 = "message_posted"
	EventMessageDeliveryUpdated        = "message_delivery_updated"
	EventShapingProposalUpdated        = "shaping_proposal_updated"
	EventShapingRecordsUpdated         = "shaping_records_updated"
	EventEvidenceRecorded              = "evidence_recorded"
	EventAttentionUpdated              = "attention_updated"
	EventCheckpointRecorded            = "checkpoint_recorded"
)

var knownCommandTypes = map[string]struct{}{
	CommandCreateWorkItem: {}, CommandTransitionWork: {}, CommandTransitionPlanTask: {}, CommandProvisionAgentInstance: {}, CommandActivateAgentInstance: {}, CommandRenewExecutionLease: {}, CommandPrepareAgentExecutionRetry: {}, CommandStartAgentExecution: {}, CommandFinishAgentExecution: {}, CommandCancelAgentExecution: {}, CommandPauseAgentExecution: {}, CommandResolveAgentExecutionReview: {}, CommandUpsertWorkBrief: {},
	CommandUpsertRequirement: {}, CommandUpsertDecision: {},
	CommandUpsertAcceptanceCriterion: {}, CommandUpsertArtifact: {}, CommandReplaceShapingRecords: {}, CommandReplaceArtifacts: {}, CommandUpsertConstraint: {},
	CommandUpsertRisk: {}, CommandUpsertPlan: {}, CommandRevisePlan: {}, CommandUpsertRoleDefinition: {},
	CommandUpsertWorkContract: {}, CommandUpsertProposedTeam: {}, CommandDescribeRole: {},
	CommandPostMessage: {}, CommandUpdateMessageDelivery: {}, CommandRecordEvidence: {}, CommandCompleteShapingTurn: {}, CommandRaiseAttention: {}, CommandResolveAttention: {},
}

var knownEventTypes = map[string]struct{}{
	EventWorkCreated: {}, EventWorkStatusChanged: {}, EventWorkBriefUpdated: {},
	EventRequirementUpdated: {}, EventDecisionUpdated: {},
	EventAcceptanceCriterionUpdated: {}, EventArtifactUpdated: {}, EventArtifactsReplaced: {}, EventConstraintUpdated: {}, EventRiskUpdated: {},
	EventPlanUpdated: {}, EventPlanTaskStatusChanged: {}, EventAgentInstanceProvisioned: {}, EventAgentInstanceActivated: {}, EventExecutionLeaseRenewed: {}, EventAgentExecutionRetryPrepared: {}, EventAgentExecutionStarted: {}, EventAgentExecutionFinished: {}, EventAgentExecutionReviewResolved: {}, EventRoleDefinitionUpdated: {}, EventWorkContractUpdated: {},
	EventProposedTeamUpdated: {}, EventMessagePosted: {}, EventMessageDeliveryUpdated: {}, EventShapingProposalUpdated: {}, EventShapingRecordsUpdated: {}, EventEvidenceRecorded: {},
	EventAttentionUpdated: {}, EventCheckpointRecorded: {},
}

type CommandEnvelope struct {
	CommandID        uuid.UUID       `json:"command_id"`
	SchemaVersion    uint32          `json:"schema_version"`
	ProjectID        uuid.UUID       `json:"project_id"`
	WorkItemID       uuid.UUID       `json:"work_item_id"`
	ActorID          uuid.UUID       `json:"actor_id"`
	DeviceID         *uuid.UUID      `json:"device_id,omitempty"`
	CorrelationID    uuid.UUID       `json:"correlation_id"`
	TimestampUnixMS  int64           `json:"timestamp_unix_ms"`
	ExpectedRevision *uint64         `json:"expected_revision,omitempty"`
	Payload          json.RawMessage `json:"payload"`
}

func (e CommandEnvelope) Validate() error {
	if err := validateCommon(e.SchemaVersion, e.ProjectID, e.ActorID, e.CorrelationID, e.TimestampUnixMS); err != nil {
		return err
	}
	if e.CommandID == uuid.Nil {
		return errors.New("command_id must not be nil")
	}
	if e.DeviceID != nil && *e.DeviceID == uuid.Nil {
		return errors.New("device_id must not be nil")
	}
	payloadType, err := payloadDiscriminator(e.Payload)
	if err != nil {
		return err
	}
	if _, ok := knownCommandTypes[payloadType]; !ok {
		return fmt.Errorf("unsupported command payload type %q", payloadType)
	}
	if e.WorkItemID == uuid.Nil {
		return errors.New("work_item_id must not be nil")
	}
	if requiresExpectedRevision(payloadType) && e.ExpectedRevision == nil {
		return errors.New("expected_revision is required for structured workspace edits")
	}
	if payloadType == CommandCreateWorkItem {
		if _, err := DecodeCreateWorkItem(e); err != nil {
			return err
		}
	}
	if payloadType == CommandTransitionWork {
		if _, err := DecodeTransitionWork(e); err != nil {
			return err
		}
	}
	if payloadType == CommandTransitionPlanTask {
		if _, err := DecodeTransitionPlanTask(e); err != nil {
			return err
		}
	}
	if payloadType == CommandProvisionAgentInstance {
		if _, err := DecodeProvisionAgentInstance(e); err != nil {
			return err
		}
	}
	if payloadType == CommandActivateAgentInstance {
		if _, err := DecodeActivateAgentInstance(e); err != nil {
			return err
		}
	}
	if payloadType == CommandRenewExecutionLease {
		if _, err := DecodeRenewExecutionLease(e); err != nil {
			return err
		}
	}
	if payloadType == CommandPrepareAgentExecutionRetry {
		if _, err := DecodePrepareAgentExecutionRetry(e); err != nil {
			return err
		}
	}
	if payloadType == CommandStartAgentExecution {
		if _, err := DecodeStartAgentExecution(e); err != nil {
			return err
		}
	}
	if payloadType == CommandFinishAgentExecution {
		if _, err := DecodeFinishAgentExecution(e); err != nil {
			return err
		}
	}
	if payloadType == CommandResolveAgentExecutionReview {
		if _, err := DecodeResolveAgentExecutionReview(e); err != nil {
			return err
		}
	}
	if payloadType == CommandCancelAgentExecution {
		if _, err := DecodeCancelAgentExecution(e); err != nil {
			return err
		}
	}
	if payloadType == CommandPauseAgentExecution {
		if _, err := DecodePauseAgentExecution(e); err != nil {
			return err
		}
	}
	if payloadType == CommandPostMessage {
		if _, err := DecodePostMessage(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpdateMessageDelivery {
		if _, err := DecodeUpdateMessageDelivery(e); err != nil {
			return err
		}
	}
	if payloadType == CommandRecordEvidence {
		if _, err := DecodeRecordEvidence(e); err != nil {
			return err
		}
	}
	if payloadType == CommandCompleteShapingTurn {
		payload, err := DecodeCompleteShapingTurn(e)
		if err != nil {
			return err
		}
		if payload.Plan != nil && e.ExpectedRevision == nil {
			return errors.New("expected_revision is required for a complete shaping snapshot")
		}
	}
	if payloadType == CommandRaiseAttention {
		if _, err := DecodeRaiseAttention(e); err != nil {
			return err
		}
	}
	if payloadType == CommandResolveAttention {
		if _, err := DecodeResolveAttention(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertWorkBrief {
		if _, err := DecodeUpsertWorkBrief(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertRequirement {
		if _, err := DecodeUpsertRequirement(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertDecision {
		if _, err := DecodeUpsertDecision(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertAcceptanceCriterion {
		if _, err := DecodeUpsertAcceptanceCriterion(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertArtifact {
		if _, err := DecodeUpsertArtifact(e); err != nil {
			return err
		}
	}
	if payloadType == CommandReplaceShapingRecords {
		if _, err := DecodeReplaceShapingRecords(e); err != nil {
			return err
		}
	}
	if payloadType == CommandReplaceArtifacts {
		if _, err := DecodeReplaceArtifacts(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertPlan {
		if _, err := DecodeUpsertPlan(e); err != nil {
			return err
		}
	}
	if payloadType == CommandRevisePlan {
		if _, err := DecodeRevisePlan(e); err != nil {
			return err
		}
	}
	if payloadType == CommandUpsertWorkContract {
		if _, err := DecodeUpsertWorkContract(e); err != nil {
			return err
		}
	}
	return nil
}

type EventEnvelope struct {
	EventID         uuid.UUID       `json:"event_id"`
	SchemaVersion   uint32          `json:"schema_version"`
	Sequence        uint64          `json:"sequence"`
	ProjectID       uuid.UUID       `json:"project_id"`
	WorkItemID      uuid.UUID       `json:"work_item_id"`
	ActorID         uuid.UUID       `json:"actor_id"`
	DeviceID        *uuid.UUID      `json:"device_id,omitempty"`
	TaskID          *uuid.UUID      `json:"task_id,omitempty"`
	CorrelationID   uuid.UUID       `json:"correlation_id"`
	CausationID     *uuid.UUID      `json:"causation_id,omitempty"`
	TimestampUnixMS int64           `json:"timestamp_unix_ms"`
	Payload         json.RawMessage `json:"payload"`
}

func (e EventEnvelope) Validate() error {
	if err := validateCommon(e.SchemaVersion, e.ProjectID, e.ActorID, e.CorrelationID, e.TimestampUnixMS); err != nil {
		return err
	}
	if e.EventID == uuid.Nil {
		return errors.New("event_id must not be nil")
	}
	if e.WorkItemID == uuid.Nil {
		return errors.New("work_item_id must not be nil")
	}
	if e.Sequence == 0 {
		return errors.New("sequence must start at 1")
	}
	for name, id := range map[string]*uuid.UUID{
		"device_id": e.DeviceID, "task_id": e.TaskID, "causation_id": e.CausationID,
	} {
		if id != nil && *id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	payloadType, err := payloadDiscriminator(e.Payload)
	if err != nil {
		return err
	}
	if _, ok := knownEventTypes[payloadType]; !ok {
		return fmt.Errorf("unsupported event payload type %q", payloadType)
	}
	if payloadType == EventWorkBriefUpdated {
		if _, err := DecodeWorkBriefUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventMessagePosted {
		if _, err := DecodeMessagePosted(e); err != nil {
			return err
		}
	}
	if payloadType == EventMessageDeliveryUpdated {
		if _, err := DecodeMessageDeliveryUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventShapingProposalUpdated {
		if _, err := DecodeShapingProposalUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventShapingRecordsUpdated {
		if _, err := DecodeShapingRecordsUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventRequirementUpdated {
		if _, err := DecodeRequirementUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventDecisionUpdated {
		if _, err := DecodeDecisionUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventAcceptanceCriterionUpdated {
		if _, err := DecodeAcceptanceCriterionUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventArtifactUpdated {
		if _, err := DecodeArtifactUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventArtifactsReplaced {
		if _, err := DecodeArtifactsReplaced(e); err != nil {
			return err
		}
	}
	if payloadType == EventPlanUpdated {
		if _, err := DecodePlanUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventPlanTaskStatusChanged {
		if _, err := DecodePlanTaskStatusChanged(e); err != nil {
			return err
		}
	}
	if payloadType == EventAgentInstanceProvisioned {
		if _, err := DecodeAgentInstanceProvisioned(e); err != nil {
			return err
		}
	}
	if payloadType == EventAgentInstanceActivated {
		if _, err := DecodeAgentInstanceActivated(e); err != nil {
			return err
		}
	}
	if payloadType == EventExecutionLeaseRenewed {
		if _, err := DecodeExecutionLeaseRenewed(e); err != nil {
			return err
		}
	}
	if payloadType == EventAgentExecutionRetryPrepared {
		if _, err := DecodeAgentExecutionRetryPrepared(e); err != nil {
			return err
		}
	}
	if payloadType == EventAgentExecutionStarted {
		if _, err := DecodeAgentExecutionStarted(e); err != nil {
			return err
		}
	}
	if payloadType == EventAgentExecutionFinished {
		if _, err := DecodeAgentExecutionFinished(e); err != nil {
			return err
		}
	}
	if payloadType == EventAgentExecutionReviewResolved {
		if _, err := DecodeAgentExecutionReviewResolved(e); err != nil {
			return err
		}
	}
	if payloadType == EventWorkContractUpdated {
		if _, err := DecodeWorkContractUpdated(e); err != nil {
			return err
		}
	}
	if payloadType == EventAttentionUpdated {
		if _, err := DecodeAttentionUpdated(e); err != nil {
			return err
		}
	}
	return nil
}

type CreateWorkItemCommand struct {
	Type              string `json:"type"`
	Title             string `json:"title"`
	ExecutionLocation string `json:"execution_location"`
}

type TransitionWorkCommand struct {
	Type   string `json:"type"`
	Status string `json:"status"`
}

type TransitionPlanTaskCommand struct {
	Type   string    `json:"type"`
	TaskID uuid.UUID `json:"task_id"`
	Status string    `json:"status"`
}

type ProvisionAgentInstanceCommand struct {
	Type            string    `json:"type"`
	AgentInstanceID uuid.UUID `json:"agent_instance_id"`
	ContractID      uuid.UUID `json:"contract_id"`
	ModelRoute      string    `json:"model_route"`
}

type ChargingAuthorization struct {
	OwnerID            uuid.UUID  `json:"owner_id"`
	Mode               string     `json:"mode"`
	BillingOperationID *uuid.UUID `json:"billing_operation_id"`
}

type ActivateAgentInstanceCommand struct {
	Type                 string                `json:"type"`
	AgentInstanceID      uuid.UUID             `json:"agent_instance_id"`
	LeaseID              uuid.UUID             `json:"lease_id"`
	EffectiveModelRoute  string                `json:"effective_model_route"`
	Charging             ChargingAuthorization `json:"charging"`
	LeaseExpiresAtUnixMS int64                 `json:"lease_expires_at_unix_ms"`
}

type RenewExecutionLeaseCommand struct {
	Type                 string    `json:"type"`
	LeaseID              uuid.UUID `json:"lease_id"`
	LeaseExpiresAtUnixMS int64     `json:"lease_expires_at_unix_ms"`
}

type PrepareAgentExecutionRetryCommand struct {
	Type                 string    `json:"type"`
	FailedAttemptID      uuid.UUID `json:"failed_attempt_id"`
	LeaseID              uuid.UUID `json:"lease_id"`
	LeaseExpiresAtUnixMS int64     `json:"lease_expires_at_unix_ms"`
}

type StartAgentExecutionCommand struct {
	Type            string             `json:"type"`
	AttemptID       uuid.UUID          `json:"attempt_id"`
	TaskID          uuid.UUID          `json:"task_id"`
	AgentInstanceID uuid.UUID          `json:"agent_instance_id"`
	LeaseID         uuid.UUID          `json:"lease_id"`
	Isolation       ExecutionIsolation `json:"isolation"`
}

type FinishAgentExecutionCommand struct {
	Type          string                  `json:"type"`
	AttemptID     uuid.UUID               `json:"attempt_id"`
	Outcome       string                  `json:"outcome"`
	Summary       string                  `json:"summary"`
	ChangeSummary *ExecutionChangeSummary `json:"change_summary,omitempty"`
}

type CancelAgentExecutionCommand struct {
	Type      string    `json:"type"`
	AttemptID uuid.UUID `json:"attempt_id"`
	Reason    string    `json:"reason"`
}

type PauseAgentExecutionCommand struct {
	Type      string    `json:"type"`
	AttemptID uuid.UUID `json:"attempt_id"`
	Reason    string    `json:"reason"`
}

type ResolveAgentExecutionReviewCommand struct {
	Type       string    `json:"type"`
	AttemptID  uuid.UUID `json:"attempt_id"`
	Resolution string    `json:"resolution"`
}

type AttentionItem struct {
	ID                 uuid.UUID   `json:"id"`
	WorkItemID         uuid.UUID   `json:"work_item_id"`
	TaskID             *uuid.UUID  `json:"task_id"`
	ThreadID           *uuid.UUID  `json:"thread_id"`
	ExecutionAttemptID *uuid.UUID  `json:"execution_attempt_id,omitempty"`
	ToolCallID         *string     `json:"tool_call_id,omitempty"`
	Kind               string      `json:"kind"`
	Status             string      `json:"status"`
	Summary            string      `json:"summary"`
	RequestedBy        uuid.UUID   `json:"requested_by"`
	AssignedTo         []uuid.UUID `json:"assigned_to"`
	CreatedAtUnixMS    int64       `json:"created_at_unix_ms"`
	ResolvedAtUnixMS   *int64      `json:"resolved_at_unix_ms"`
	ResolutionKind     *string     `json:"resolution_kind,omitempty"`
	Resolution         *string     `json:"resolution,omitempty"`
}

type RaiseAttentionCommand struct {
	Type          string        `json:"type"`
	AttentionItem AttentionItem `json:"attention_item"`
}

type ResolveAttentionCommand struct {
	Type            string    `json:"type"`
	AttentionItemID uuid.UUID `json:"attention_item_id"`
	Outcome         string    `json:"outcome"`
	Resolution      string    `json:"resolution"`
}

type AttentionUpdatedEvent struct {
	Type          string        `json:"type"`
	AttentionItem AttentionItem `json:"attention_item"`
}

type PlanTask struct {
	ID                 uuid.UUID   `json:"id"`
	PlanID             uuid.UUID   `json:"plan_id"`
	Title              string      `json:"title"`
	Description        string      `json:"description"`
	Status             string      `json:"status"`
	DependsOn          []uuid.UUID `json:"depends_on"`
	ReviewTargetTaskID *uuid.UUID  `json:"review_target_task_id,omitempty"`
	AcceptanceCriteria []uuid.UUID `json:"acceptance_criteria"`
	AssignedContractID *uuid.UUID  `json:"assigned_contract_id"`
}

type Plan struct {
	ID               uuid.UUID  `json:"id"`
	WorkItemID       uuid.UUID  `json:"work_item_id"`
	Version          uint64     `json:"version"`
	Status           string     `json:"status"`
	Tasks            []PlanTask `json:"tasks"`
	ApprovedBy       *uuid.UUID `json:"approved_by"`
	ApprovedAtUnixMS *int64     `json:"approved_at_unix_ms"`
	CreatedAtUnixMS  int64      `json:"created_at_unix_ms"`
	UpdatedAtUnixMS  int64      `json:"updated_at_unix_ms"`
}

type UpsertPlanCommand struct {
	Type string `json:"type"`
	Plan Plan   `json:"plan"`
}

type RevisePlanCommand struct {
	Type          string         `json:"type"`
	Plan          Plan           `json:"plan"`
	WorkContracts []WorkContract `json:"work_contracts"`
}

type PlanUpdatedEvent struct {
	Type string `json:"type"`
	Plan Plan   `json:"plan"`
}

type PlanTaskStatusChangedEvent struct {
	Type     string    `json:"type"`
	Plan     Plan      `json:"plan"`
	TaskID   uuid.UUID `json:"task_id"`
	Previous string    `json:"previous"`
	Current  string    `json:"current"`
}

type WorkBrief struct {
	ID              uuid.UUID `json:"id"`
	WorkItemID      uuid.UUID `json:"work_item_id"`
	Version         uint64    `json:"version"`
	Outcome         string    `json:"outcome"`
	Summary         string    `json:"summary"`
	Status          string    `json:"status"`
	CreatedBy       uuid.UUID `json:"created_by"`
	UpdatedBy       uuid.UUID `json:"updated_by"`
	CreatedAtUnixMS int64     `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64     `json:"updated_at_unix_ms"`
}

type WorkBriefUpdatedEvent struct {
	Type      string    `json:"type"`
	WorkBrief WorkBrief `json:"work_brief"`
}

type UpsertWorkBriefCommand struct {
	Type      string    `json:"type"`
	WorkBrief WorkBrief `json:"work_brief"`
}

type WorkBudget struct {
	MaximumInputTokens  *uint64 `json:"maximum_input_tokens"`
	MaximumOutputTokens *uint64 `json:"maximum_output_tokens"`
	MaximumCostMicros   *uint64 `json:"maximum_cost_micros"`
	MaximumDurationMS   *uint64 `json:"maximum_duration_ms"`
	MaximumToolCalls    *uint64 `json:"maximum_tool_calls"`
}

type CapabilityGrant struct {
	Capability string  `json:"capability"`
	Scope      *string `json:"scope"`
}

type PermissionGrant struct {
	Permission       string  `json:"permission"`
	Scope            *string `json:"scope"`
	RequiresApproval bool    `json:"requires_approval"`
}

type ArtifactRef struct {
	ID           uuid.UUID `json:"id"`
	WorkItemID   uuid.UUID `json:"work_item_id"`
	RelativePath string    `json:"relative_path"`
	Kind         string    `json:"kind"`
}

type WorkContract struct {
	ID                  uuid.UUID         `json:"id"`
	WorkItemID          uuid.UUID         `json:"work_item_id"`
	RoleDefinitionID    uuid.UUID         `json:"role_definition_id"`
	Version             uint64            `json:"version"`
	AgentInstanceID     *uuid.UUID        `json:"agent_instance_id"`
	Status              string            `json:"status"`
	Mission             string            `json:"mission"`
	TaskIDs             []uuid.UUID       `json:"task_ids"`
	InputArtifactIDs    []uuid.UUID       `json:"input_artifact_ids"`
	ExpectedOutputs     []string          `json:"expected_outputs"`
	SuccessCriteria     []uuid.UUID       `json:"success_criteria"`
	WriteScope          []string          `json:"write_scope"`
	Capabilities        []CapabilityGrant `json:"capabilities"`
	Tools               []string          `json:"tools"`
	Permissions         []PermissionGrant `json:"permissions"`
	Budget              WorkBudget        `json:"budget"`
	EscalationRules     []string          `json:"escalation_rules"`
	ExecutionLocation   string            `json:"execution_location"`
	PreferredModelRoute *string           `json:"preferred_model_route,omitempty"`
	CreatedBy           uuid.UUID         `json:"created_by"`
	UpdatedBy           uuid.UUID         `json:"updated_by"`
	CreatedAtUnixMS     int64             `json:"created_at_unix_ms"`
	UpdatedAtUnixMS     int64             `json:"updated_at_unix_ms"`
}

type UpsertWorkContractCommand struct {
	Type         string       `json:"type"`
	WorkContract WorkContract `json:"work_contract"`
}

type WorkContractUpdatedEvent struct {
	Type         string       `json:"type"`
	WorkContract WorkContract `json:"work_contract"`
}

type AgentInstance struct {
	ID                uuid.UUID  `json:"id"`
	WorkItemID        uuid.UUID  `json:"work_item_id"`
	RoleDefinitionID  uuid.UUID  `json:"role_definition_id"`
	ContractID        uuid.UUID  `json:"contract_id"`
	Status            string     `json:"status"`
	ExecutionLocation string     `json:"execution_location"`
	ModelRoute        string     `json:"model_route"`
	ActiveLeaseID     *uuid.UUID `json:"active_lease_id"`
	StartedAtUnixMS   *int64     `json:"started_at_unix_ms"`
	UpdatedAtUnixMS   int64      `json:"updated_at_unix_ms"`
}

type ExecutionLease struct {
	ID               uuid.UUID             `json:"id"`
	WorkItemID       uuid.UUID             `json:"work_item_id"`
	AgentInstanceID  uuid.UUID             `json:"agent_instance_id"`
	ContractID       uuid.UUID             `json:"contract_id"`
	Status           string                `json:"status"`
	Charging         ChargingAuthorization `json:"charging"`
	AcquiredAtUnixMS int64                 `json:"acquired_at_unix_ms"`
	ExpiresAtUnixMS  int64                 `json:"expires_at_unix_ms"`
	UpdatedAtUnixMS  int64                 `json:"updated_at_unix_ms"`
}

type ExecutionAttempt struct {
	ID               uuid.UUID               `json:"id"`
	WorkItemID       uuid.UUID               `json:"work_item_id"`
	TaskID           uuid.UUID               `json:"task_id"`
	AgentInstanceID  uuid.UUID               `json:"agent_instance_id"`
	ContractID       uuid.UUID               `json:"contract_id"`
	LeaseID          uuid.UUID               `json:"lease_id"`
	AttemptNumber    uint32                  `json:"attempt_number"`
	Status           string                  `json:"status"`
	ModelRoute       string                  `json:"model_route"`
	Isolation        *ExecutionIsolation     `json:"isolation,omitempty"`
	ChangeSummary    *ExecutionChangeSummary `json:"change_summary,omitempty"`
	ReviewResolution *string                 `json:"review_resolution,omitempty"`
	StartedAtUnixMS  int64                   `json:"started_at_unix_ms"`
	FinishedAtUnixMS *int64                  `json:"finished_at_unix_ms"`
	UpdatedAtUnixMS  int64                   `json:"updated_at_unix_ms"`
}

type ExecutionIsolation struct {
	ID              uuid.UUID  `json:"id"`
	Kind            string     `json:"kind"`
	BaseRevision    *string    `json:"base_revision,omitempty"`
	SourceAttemptID *uuid.UUID `json:"source_attempt_id,omitempty"`
}

type ExecutionFileChange struct {
	RelativePath string  `json:"relative_path"`
	Kind         string  `json:"kind"`
	AddedLines   *uint64 `json:"added_lines"`
	DeletedLines *uint64 `json:"deleted_lines"`
}

type ExecutionChangeSummary struct {
	IsolationID      uuid.UUID             `json:"isolation_id"`
	BaseRevision     string                `json:"base_revision"`
	Files            []ExecutionFileChange `json:"files"`
	CapturedAtUnixMS int64                 `json:"captured_at_unix_ms"`
}

type AgentInstanceProvisionedEvent struct {
	Type          string        `json:"type"`
	AgentInstance AgentInstance `json:"agent_instance"`
	WorkContract  WorkContract  `json:"work_contract"`
}

type AgentInstanceActivatedEvent struct {
	Type          string         `json:"type"`
	AgentInstance AgentInstance  `json:"agent_instance"`
	WorkContract  WorkContract   `json:"work_contract"`
	Lease         ExecutionLease `json:"lease"`
}

type ExecutionLeaseRenewedEvent struct {
	Type          string         `json:"type"`
	AgentInstance AgentInstance  `json:"agent_instance"`
	Lease         ExecutionLease `json:"lease"`
}

type AgentExecutionRetryPreparedEvent struct {
	Type          string         `json:"type"`
	AgentInstance AgentInstance  `json:"agent_instance"`
	Lease         ExecutionLease `json:"lease"`
	Plan          Plan           `json:"plan"`
}

type AgentExecutionStartedEvent struct {
	Type          string           `json:"type"`
	AgentInstance AgentInstance    `json:"agent_instance"`
	Lease         ExecutionLease   `json:"lease"`
	Attempt       ExecutionAttempt `json:"attempt"`
	Plan          Plan             `json:"plan"`
}

type AgentExecutionFinishedEvent struct {
	Type          string           `json:"type"`
	AgentInstance AgentInstance    `json:"agent_instance"`
	Lease         ExecutionLease   `json:"lease"`
	Attempt       ExecutionAttempt `json:"attempt"`
	Plan          Plan             `json:"plan"`
	Summary       string           `json:"summary"`
}

type AgentExecutionReviewResolvedEvent struct {
	Type          string           `json:"type"`
	AgentInstance AgentInstance    `json:"agent_instance"`
	WorkContract  WorkContract     `json:"work_contract"`
	Lease         ExecutionLease   `json:"lease"`
	Attempt       ExecutionAttempt `json:"attempt"`
	Plan          Plan             `json:"plan"`
	Resolution    string           `json:"resolution"`
}

type AgentMessage struct {
	ID              uuid.UUID                `json:"id"`
	ThreadID        uuid.UUID                `json:"thread_id"`
	WorkItemID      uuid.UUID                `json:"work_item_id"`
	SenderID        uuid.UUID                `json:"sender_id"`
	Kind            string                   `json:"kind"`
	Body            string                   `json:"body"`
	ReviewFinding   *StructuredReviewFinding `json:"review_finding,omitempty"`
	ReplyTo         *uuid.UUID               `json:"reply_to"`
	ArtifactIDs     []uuid.UUID              `json:"artifact_ids"`
	EvidenceIDs     []uuid.UUID              `json:"evidence_ids"`
	DeliveryState   string                   `json:"delivery_state"`
	CreatedAtUnixMS int64                    `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64                    `json:"updated_at_unix_ms"`
}

type StructuredReviewFinding struct {
	CandidateAttemptID uuid.UUID           `json:"candidate_attempt_id"`
	Verdict            string              `json:"verdict"`
	Summary            string              `json:"summary"`
	Findings           []ReviewFindingItem `json:"findings"`
}

type ReviewFindingItem struct {
	Severity     string  `json:"severity"`
	Summary      string  `json:"summary"`
	RelativePath *string `json:"relative_path,omitempty"`
	Line         *uint64 `json:"line,omitempty"`
}

type ShapingProposal struct {
	Revision           uint64   `json:"revision"`
	Outcome            string   `json:"outcome"`
	Requirements       []string `json:"requirements"`
	OpenDecisions      []string `json:"open_decisions"`
	AcceptanceCriteria []string `json:"acceptance_criteria"`
	PlanSummary        string   `json:"plan_summary"`
}

type ProvenanceSource struct {
	Type      string     `json:"type"`
	ThreadID  *uuid.UUID `json:"thread_id,omitempty"`
	MessageID *uuid.UUID `json:"message_id,omitempty"`
}

type Provenance struct {
	Source           ProvenanceSource `json:"source"`
	RecordedBy       uuid.UUID        `json:"recorded_by"`
	RecordedAtUnixMS int64            `json:"recorded_at_unix_ms"`
}

type ShapingRequirement struct {
	ID              uuid.UUID    `json:"id"`
	WorkItemID      uuid.UUID    `json:"work_item_id"`
	Summary         string       `json:"summary"`
	Details         string       `json:"details"`
	Priority        string       `json:"priority"`
	Status          string       `json:"status"`
	Provenance      []Provenance `json:"provenance"`
	CreatedAtUnixMS int64        `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64        `json:"updated_at_unix_ms"`
}

type ShapingDecision struct {
	ID              uuid.UUID    `json:"id"`
	WorkItemID      uuid.UUID    `json:"work_item_id"`
	Question        string       `json:"question"`
	Resolution      *string      `json:"resolution"`
	Rationale       *string      `json:"rationale"`
	Status          string       `json:"status"`
	Provenance      []Provenance `json:"provenance"`
	DecidedBy       *uuid.UUID   `json:"decided_by"`
	CreatedAtUnixMS int64        `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64        `json:"updated_at_unix_ms"`
}

type ShapingAcceptanceCriterion struct {
	ID              uuid.UUID    `json:"id"`
	WorkItemID      uuid.UUID    `json:"work_item_id"`
	Description     string       `json:"description"`
	Status          string       `json:"status"`
	EvidenceIDs     []uuid.UUID  `json:"evidence_ids"`
	WaiverReason    *string      `json:"waiver_reason"`
	WaivedBy        *uuid.UUID   `json:"waived_by"`
	Provenance      []Provenance `json:"provenance"`
	CreatedAtUnixMS int64        `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64        `json:"updated_at_unix_ms"`
}

type ShapingRecords struct {
	WorkItemID         uuid.UUID                    `json:"work_item_id"`
	ProposalRevision   uint64                       `json:"proposal_revision"`
	Requirements       []ShapingRequirement         `json:"requirements"`
	Decisions          []ShapingDecision            `json:"decisions"`
	AcceptanceCriteria []ShapingAcceptanceCriterion `json:"acceptance_criteria"`
}

type PostMessageCommand struct {
	Type    string               `json:"type"`
	Message AgentMessage         `json:"message"`
	Routing *AgentMessageRouting `json:"routing,omitempty"`
}

type UpdateMessageDeliveryCommand struct {
	Type          string    `json:"type"`
	MessageID     uuid.UUID `json:"message_id"`
	DeliveryState string    `json:"delivery_state"`
}

type Evidence struct {
	ID              uuid.UUID  `json:"id"`
	WorkItemID      uuid.UUID  `json:"work_item_id"`
	TaskID          *uuid.UUID `json:"task_id"`
	Kind            string     `json:"kind"`
	Summary         string     `json:"summary"`
	Source          string     `json:"source"`
	ProducedBy      uuid.UUID  `json:"produced_by"`
	CreatedAtUnixMS int64      `json:"created_at_unix_ms"`
}

type RecordEvidenceCommand struct {
	Type     string   `json:"type"`
	Evidence Evidence `json:"evidence"`
}

type AgentMessageRouting struct {
	RecipientIDs       []uuid.UUID `json:"recipient_ids"`
	TaskID             *uuid.UUID  `json:"task_id"`
	ExecutionAttemptID *uuid.UUID  `json:"execution_attempt_id"`
}

type RoleDefinition struct {
	ID           uuid.UUID       `json:"id"`
	Scope        json.RawMessage `json:"scope"`
	Kind         string          `json:"kind,omitempty"`
	Name         string          `json:"name"`
	Mission      string          `json:"mission"`
	Capabilities []struct {
		Capability string `json:"capability"`
		Reason     string `json:"reason"`
		Required   bool   `json:"required"`
	} `json:"capabilities"`
	PreferredTools  []string  `json:"preferred_tools"`
	CreatedBy       uuid.UUID `json:"created_by"`
	CreatedAtUnixMS int64     `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64     `json:"updated_at_unix_ms"`
}

type ProposedTeamRole struct {
	RoleDefinitionID uuid.UUID `json:"role_definition_id"`
	WorkContractID   uuid.UUID `json:"work_contract_id"`
	Rationale        string    `json:"rationale"`
}

type ProposedTeam struct {
	ID              uuid.UUID          `json:"id"`
	WorkItemID      uuid.UUID          `json:"work_item_id"`
	Version         uint64             `json:"version"`
	Roles           []ProposedTeamRole `json:"roles"`
	Status          string             `json:"status"`
	CreatedBy       uuid.UUID          `json:"created_by"`
	CreatedAtUnixMS int64              `json:"created_at_unix_ms"`
	UpdatedAtUnixMS int64              `json:"updated_at_unix_ms"`
}

type CompleteShapingTurnCommand struct {
	Type            string           `json:"type"`
	Message         AgentMessage     `json:"message"`
	Proposal        ShapingProposal  `json:"proposal"`
	Records         ShapingRecords   `json:"records"`
	Artifacts       []ArtifactRef    `json:"artifacts,omitempty"`
	RoleDefinitions []RoleDefinition `json:"role_definitions,omitempty"`
	WorkContracts   []WorkContract   `json:"work_contracts,omitempty"`
	ProposedTeam    *ProposedTeam    `json:"proposed_team,omitempty"`
	Plan            *Plan            `json:"plan,omitempty"`
}

type UpsertRequirementCommand struct {
	Type        string             `json:"type"`
	Requirement ShapingRequirement `json:"requirement"`
}

type UpsertDecisionCommand struct {
	Type     string          `json:"type"`
	Decision ShapingDecision `json:"decision"`
}

type UpsertAcceptanceCriterionCommand struct {
	Type                string                     `json:"type"`
	AcceptanceCriterion ShapingAcceptanceCriterion `json:"acceptance_criterion"`
}

type UpsertArtifactCommand struct {
	Type     string      `json:"type"`
	Artifact ArtifactRef `json:"artifact"`
}

type ReplaceShapingRecordsCommand struct {
	Type    string         `json:"type"`
	Records ShapingRecords `json:"records"`
}

type ReplaceArtifactsCommand struct {
	Type      string        `json:"type"`
	Artifacts []ArtifactRef `json:"artifacts"`
}

type MessagePostedEvent struct {
	Type    string               `json:"type"`
	Message AgentMessage         `json:"message"`
	Routing *AgentMessageRouting `json:"routing,omitempty"`
}

type MessageDeliveryUpdatedEvent struct {
	Type    string              `json:"type"`
	Message AgentMessage        `json:"message"`
	Routing AgentMessageRouting `json:"routing"`
}

type ShapingProposalUpdatedEvent struct {
	Type     string          `json:"type"`
	Proposal ShapingProposal `json:"proposal"`
}

type ShapingRecordsUpdatedEvent struct {
	Type    string         `json:"type"`
	Records ShapingRecords `json:"records"`
}

type RequirementUpdatedEvent struct {
	Type        string             `json:"type"`
	Requirement ShapingRequirement `json:"requirement"`
}

type DecisionUpdatedEvent struct {
	Type     string          `json:"type"`
	Decision ShapingDecision `json:"decision"`
}

type AcceptanceCriterionUpdatedEvent struct {
	Type                string                     `json:"type"`
	AcceptanceCriterion ShapingAcceptanceCriterion `json:"acceptance_criterion"`
}

type ArtifactUpdatedEvent struct {
	Type     string      `json:"type"`
	Artifact ArtifactRef `json:"artifact"`
}

type ArtifactsReplacedEvent struct {
	Type      string        `json:"type"`
	Artifacts []ArtifactRef `json:"artifacts"`
}

func DecodeCreateWorkItem(envelope CommandEnvelope) (CreateWorkItemCommand, error) {
	var payload CreateWorkItemCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandCreateWorkItem {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandCreateWorkItem, payload.Type)
	}
	if strings.TrimSpace(payload.Title) == "" {
		return payload, errors.New("title must not be empty")
	}
	if payload.ExecutionLocation != "local" && payload.ExecutionLocation != "cloud" && payload.ExecutionLocation != "hybrid" {
		return payload, fmt.Errorf("invalid execution_location %q", payload.ExecutionLocation)
	}
	return payload, nil
}

func DecodeTransitionWork(envelope CommandEnvelope) (TransitionWorkCommand, error) {
	var payload TransitionWorkCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandTransitionWork {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandTransitionWork, payload.Type)
	}
	if !isWorkStatus(payload.Status) {
		return payload, fmt.Errorf("invalid work status %q", payload.Status)
	}
	return payload, nil
}

func DecodeTransitionPlanTask(envelope CommandEnvelope) (TransitionPlanTaskCommand, error) {
	var payload TransitionPlanTaskCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandTransitionPlanTask {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandTransitionPlanTask, payload.Type)
	}
	if payload.TaskID == uuid.Nil {
		return payload, errors.New("task_id must not be nil")
	}
	if !isPlanTaskStatus(payload.Status) {
		return payload, fmt.Errorf("invalid plan task status %q", payload.Status)
	}
	return payload, nil
}

func DecodeProvisionAgentInstance(envelope CommandEnvelope) (ProvisionAgentInstanceCommand, error) {
	var payload ProvisionAgentInstanceCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandProvisionAgentInstance {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandProvisionAgentInstance, payload.Type)
	}
	if payload.AgentInstanceID == uuid.Nil {
		return payload, errors.New("agent_instance_id must not be nil")
	}
	if payload.ContractID == uuid.Nil {
		return payload, errors.New("contract_id must not be nil")
	}
	if strings.TrimSpace(payload.ModelRoute) == "" {
		return payload, errors.New("model_route must not be empty")
	}
	return payload, nil
}

func DecodeActivateAgentInstance(envelope CommandEnvelope) (ActivateAgentInstanceCommand, error) {
	var payload ActivateAgentInstanceCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandActivateAgentInstance {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandActivateAgentInstance, payload.Type)
	}
	if payload.AgentInstanceID == uuid.Nil {
		return payload, errors.New("agent_instance_id must not be nil")
	}
	if payload.LeaseID == uuid.Nil {
		return payload, errors.New("lease_id must not be nil")
	}
	if strings.TrimSpace(payload.EffectiveModelRoute) == "" {
		return payload, errors.New("effective_model_route must not be empty")
	}
	if payload.Charging.OwnerID != envelope.ActorID {
		return payload, errors.New("charging owner must match the command actor")
	}
	if err := validateChargingAuthorization(payload.Charging); err != nil {
		return payload, err
	}
	if payload.LeaseExpiresAtUnixMS <= envelope.TimestampUnixMS {
		return payload, errors.New("execution lease must expire after activation")
	}
	return payload, nil
}

func DecodeRenewExecutionLease(envelope CommandEnvelope) (RenewExecutionLeaseCommand, error) {
	var payload RenewExecutionLeaseCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandRenewExecutionLease {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandRenewExecutionLease, payload.Type)
	}
	if payload.LeaseID == uuid.Nil {
		return payload, errors.New("lease_id must not be nil")
	}
	if payload.LeaseExpiresAtUnixMS <= envelope.TimestampUnixMS {
		return payload, errors.New("renewed execution lease must expire after the command")
	}
	return payload, nil
}

func DecodePrepareAgentExecutionRetry(envelope CommandEnvelope) (PrepareAgentExecutionRetryCommand, error) {
	var payload PrepareAgentExecutionRetryCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandPrepareAgentExecutionRetry {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandPrepareAgentExecutionRetry, payload.Type)
	}
	if payload.FailedAttemptID == uuid.Nil {
		return payload, errors.New("failed_attempt_id must not be nil")
	}
	if payload.LeaseID == uuid.Nil {
		return payload, errors.New("lease_id must not be nil")
	}
	if payload.LeaseExpiresAtUnixMS <= envelope.TimestampUnixMS {
		return payload, errors.New("retry lease must expire after the command")
	}
	return payload, nil
}

func DecodeStartAgentExecution(envelope CommandEnvelope) (StartAgentExecutionCommand, error) {
	var payload StartAgentExecutionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandStartAgentExecution {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandStartAgentExecution, payload.Type)
	}
	identities := map[string]uuid.UUID{
		"attempt_id": payload.AttemptID, "task_id": payload.TaskID,
		"agent_instance_id": payload.AgentInstanceID, "lease_id": payload.LeaseID,
	}
	for name, id := range identities {
		if id == uuid.Nil {
			return payload, fmt.Errorf("%s must not be nil", name)
		}
	}
	if err := validateExecutionIsolation(payload.Isolation); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeFinishAgentExecution(envelope CommandEnvelope) (FinishAgentExecutionCommand, error) {
	var payload FinishAgentExecutionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandFinishAgentExecution {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandFinishAgentExecution, payload.Type)
	}
	if payload.AttemptID == uuid.Nil {
		return payload, errors.New("attempt_id must not be nil")
	}
	if payload.Outcome != "succeeded" && payload.Outcome != "failed" &&
		payload.Outcome != "cancelled" && payload.Outcome != "timed_out" {
		return payload, fmt.Errorf("invalid terminal execution outcome %q", payload.Outcome)
	}
	if strings.TrimSpace(payload.Summary) == "" {
		return payload, errors.New("execution outcome summary must not be empty")
	}
	if payload.ChangeSummary != nil && payload.ChangeSummary.CapturedAtUnixMS <= 0 {
		return payload, errors.New("execution change summary requires a capture timestamp")
	}
	return payload, nil
}

func DecodeCancelAgentExecution(envelope CommandEnvelope) (CancelAgentExecutionCommand, error) {
	var payload CancelAgentExecutionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandCancelAgentExecution {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandCancelAgentExecution, payload.Type)
	}
	if payload.AttemptID == uuid.Nil {
		return payload, errors.New("attempt_id must not be nil")
	}
	if strings.TrimSpace(payload.Reason) == "" {
		return payload, errors.New("execution cancellation reason must not be empty")
	}
	return payload, nil
}

func DecodePauseAgentExecution(envelope CommandEnvelope) (PauseAgentExecutionCommand, error) {
	var payload PauseAgentExecutionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandPauseAgentExecution {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandPauseAgentExecution, payload.Type)
	}
	if payload.AttemptID == uuid.Nil {
		return payload, errors.New("attempt_id must not be nil")
	}
	if strings.TrimSpace(payload.Reason) == "" {
		return payload, errors.New("execution pause reason must not be empty")
	}
	return payload, nil
}

func DecodeResolveAgentExecutionReview(envelope CommandEnvelope) (ResolveAgentExecutionReviewCommand, error) {
	var payload ResolveAgentExecutionReviewCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandResolveAgentExecutionReview {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandResolveAgentExecutionReview, payload.Type)
	}
	if payload.AttemptID == uuid.Nil {
		return payload, errors.New("attempt_id must not be nil")
	}
	if payload.Resolution != "accepted" && payload.Resolution != "integrated" && payload.Resolution != "revision_requested" && payload.Resolution != "discarded" {
		return payload, fmt.Errorf("invalid execution review resolution %q", payload.Resolution)
	}
	return payload, nil
}

func DecodeRaiseAttention(envelope CommandEnvelope) (RaiseAttentionCommand, error) {
	var payload RaiseAttentionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandRaiseAttention {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandRaiseAttention, payload.Type)
	}
	if err := validateAttentionItem(payload.AttentionItem, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.AttentionItem.RequestedBy != envelope.ActorID {
		return payload, errors.New("attention requester does not match the command actor")
	}
	return payload, nil
}

func DecodeResolveAttention(envelope CommandEnvelope) (ResolveAttentionCommand, error) {
	var payload ResolveAttentionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandResolveAttention {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandResolveAttention, payload.Type)
	}
	if payload.AttentionItemID == uuid.Nil {
		return payload, errors.New("attention_item_id must not be nil")
	}
	if payload.Outcome != "approved" && payload.Outcome != "rejected" &&
		payload.Outcome != "answered" && payload.Outcome != "dismissed" {
		return payload, errors.New("attention outcome is invalid")
	}
	if strings.TrimSpace(payload.Resolution) == "" {
		return payload, errors.New("resolution must not be empty")
	}
	return payload, nil
}

func DecodeAttentionUpdated(envelope EventEnvelope) (AttentionUpdatedEvent, error) {
	var payload AttentionUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAttentionUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAttentionUpdated, payload.Type)
	}
	if err := validateAttentionItem(payload.AttentionItem, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if (envelope.TaskID == nil) != (payload.AttentionItem.TaskID == nil) ||
		(envelope.TaskID != nil && *envelope.TaskID != *payload.AttentionItem.TaskID) {
		return payload, errors.New("event task_id does not match the attention item")
	}
	return payload, nil
}

func validateAttentionItem(attention AttentionItem, workItemID uuid.UUID) error {
	if attention.ID == uuid.Nil || attention.WorkItemID == uuid.Nil || attention.RequestedBy == uuid.Nil {
		return errors.New("attention identity must not be nil")
	}
	if attention.WorkItemID != workItemID {
		return errors.New("attention belongs to a different work item")
	}
	for _, optionalID := range []*uuid.UUID{attention.TaskID, attention.ThreadID, attention.ExecutionAttemptID} {
		if optionalID != nil && *optionalID == uuid.Nil {
			return errors.New("attention optional identity must not be nil")
		}
	}
	for _, assignee := range attention.AssignedTo {
		if assignee == uuid.Nil {
			return errors.New("attention assignee must not be nil")
		}
	}
	if strings.TrimSpace(attention.Summary) == "" || attention.CreatedAtUnixMS <= 0 {
		return errors.New("attention requires a summary and positive creation timestamp")
	}
	if attention.ToolCallID != nil && strings.TrimSpace(*attention.ToolCallID) == "" {
		return errors.New("attention tool_call_id must not be empty")
	}
	if attention.Kind != "decision" && attention.Kind != "approval" && attention.Kind != "permission" &&
		attention.Kind != "conflict" && attention.Kind != "budget" && attention.Kind != "failure" && attention.Kind != "review" {
		return fmt.Errorf("invalid attention kind %q", attention.Kind)
	}
	terminal := attention.Status == "resolved" || attention.Status == "dismissed"
	if !terminal && attention.Status != "open" && attention.Status != "acknowledged" {
		return fmt.Errorf("invalid attention status %q", attention.Status)
	}
	if terminal {
		if attention.ResolvedAtUnixMS == nil || *attention.ResolvedAtUnixMS < attention.CreatedAtUnixMS ||
			attention.ResolutionKind == nil || attention.Resolution == nil || strings.TrimSpace(*attention.Resolution) == "" {
			return errors.New("terminal attention requires a valid resolution and timestamp")
		}
		if attention.Kind == "permission" && *attention.ResolutionKind != "approved" && *attention.ResolutionKind != "rejected" {
			return errors.New("permission attention requires an approved or rejected outcome")
		}
		if (attention.Status == "dismissed") != (*attention.ResolutionKind == "dismissed") {
			return errors.New("dismissed attention status and outcome must match")
		}
	} else if attention.ResolvedAtUnixMS != nil || attention.ResolutionKind != nil || attention.Resolution != nil {
		return errors.New("open attention cannot carry resolution data")
	}
	if attention.Kind == "permission" && (attention.ExecutionAttemptID == nil || attention.ToolCallID == nil) {
		return errors.New("permission attention requires an execution attempt and tool call")
	}
	return nil
}

func validateChargingAuthorization(charging ChargingAuthorization) error {
	if charging.OwnerID == uuid.Nil {
		return errors.New("charging owner must not be nil")
	}
	switch charging.Mode {
	case "platform_credits":
		if charging.BillingOperationID == nil || *charging.BillingOperationID == uuid.Nil {
			return errors.New("platform credits require a billing operation")
		}
	case "local_compute", "bring_your_own_key":
		if charging.BillingOperationID != nil {
			return errors.New("local compute and BYOK must not cite a platform billing operation")
		}
	default:
		return fmt.Errorf("invalid charging mode %q", charging.Mode)
	}
	return nil
}

func isWorkStatus(status string) bool {
	switch status {
	case "draft", "understanding", "waiting_for_decision", "ready_to_plan", "planning",
		"ready_to_execute", "executing", "reviewing", "verifying", "needs_user_approval",
		"completed", "blocked", "paused", "cancelled":
		return true
	default:
		return false
	}
}

func isPlanTaskStatus(status string) bool {
	switch status {
	case "proposed", "ready", "running", "blocked", "reviewing", "verifying",
		"completed", "failed", "cancelled":
		return true
	default:
		return false
	}
}

func isPlanStatus(status string) bool {
	switch status {
	case "draft", "approved", "superseded":
		return true
	default:
		return false
	}
}

func DecodeUpsertPlan(envelope CommandEnvelope) (UpsertPlanCommand, error) {
	var payload UpsertPlanCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertPlan {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertPlan, payload.Type)
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.Plan.Status == "approved" && (payload.Plan.ApprovedBy == nil || *payload.Plan.ApprovedBy != envelope.ActorID) {
		return payload, errors.New("approved plan must attribute the command actor")
	}
	return payload, nil
}

func DecodeRevisePlan(envelope CommandEnvelope) (RevisePlanCommand, error) {
	var payload RevisePlanCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandRevisePlan {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandRevisePlan, payload.Type)
	}
	if err := validatePlanRevision(payload.Plan, payload.WorkContracts, envelope.WorkItemID, envelope.ActorID); err != nil {
		return payload, err
	}
	return payload, nil
}

func validatePlanRevision(plan Plan, contracts []WorkContract, workItemID, actorID uuid.UUID) error {
	if err := validatePlan(plan, workItemID); err != nil {
		return err
	}
	if plan.Status != "draft" || plan.ApprovedBy != nil || plan.ApprovedAtUnixMS != nil {
		return errors.New("a plan revision must return to an unapproved draft")
	}
	if len(plan.Tasks) == 0 || len(contracts) == 0 {
		return errors.New("a plan revision requires tasks and role contracts")
	}

	contractsByID := make(map[uuid.UUID]WorkContract, len(contracts))
	expectedTasks := make(map[uuid.UUID]map[uuid.UUID]struct{}, len(contracts))
	expectedCriteria := make(map[uuid.UUID]map[uuid.UUID]struct{}, len(contracts))
	for _, contract := range contracts {
		if err := validateWorkContract(contract, workItemID); err != nil {
			return err
		}
		if contract.UpdatedBy != actorID {
			return errors.New("revised work contract must attribute the command actor")
		}
		if contract.Status != "proposed" || contract.AgentInstanceID != nil {
			return errors.New("revised work contracts must be proposed and non-runtime")
		}
		if _, exists := contractsByID[contract.ID]; exists {
			return errors.New("a plan revision contains a duplicate role contract")
		}
		contractsByID[contract.ID] = contract
	}
	for _, task := range plan.Tasks {
		if task.Status != "proposed" {
			return errors.New("edited plan tasks must return to proposed")
		}
		if task.AssignedContractID == nil {
			return errors.New("every edited plan task requires an assigned role contract")
		}
		contractID := *task.AssignedContractID
		if _, exists := contractsByID[contractID]; !exists {
			return errors.New("a plan task references a contract outside the revision")
		}
		if expectedTasks[contractID] == nil {
			expectedTasks[contractID] = make(map[uuid.UUID]struct{})
		}
		expectedTasks[contractID][task.ID] = struct{}{}
		if expectedCriteria[contractID] == nil {
			expectedCriteria[contractID] = make(map[uuid.UUID]struct{})
		}
		for _, criterionID := range task.AcceptanceCriteria {
			expectedCriteria[contractID][criterionID] = struct{}{}
		}
	}
	for _, contract := range contracts {
		actualTasks := uuidSet(contract.TaskIDs)
		actualCriteria := uuidSet(contract.SuccessCriteria)
		if len(actualTasks) != len(contract.TaskIDs) || len(actualCriteria) != len(contract.SuccessCriteria) ||
			!sameUUIDSet(actualTasks, expectedTasks[contract.ID]) ||
			!sameUUIDSet(actualCriteria, expectedCriteria[contract.ID]) {
			return errors.New("role contract task and criterion links must exactly match the revised plan")
		}
	}
	return nil
}

func uuidSet(ids []uuid.UUID) map[uuid.UUID]struct{} {
	set := make(map[uuid.UUID]struct{}, len(ids))
	for _, id := range ids {
		set[id] = struct{}{}
	}
	return set
}

func sameUUIDSet(left, right map[uuid.UUID]struct{}) bool {
	if len(left) != len(right) {
		return false
	}
	for id := range left {
		if _, exists := right[id]; !exists {
			return false
		}
	}
	return true
}

func validatePlan(plan Plan, workItemID uuid.UUID) error {
	if plan.ID == uuid.Nil {
		return errors.New("plan id must not be nil")
	}
	if plan.WorkItemID != workItemID {
		return errors.New("plan belongs to a different work item")
	}
	if plan.Version == 0 {
		return errors.New("plan version must be positive")
	}
	if !isPlanStatus(plan.Status) {
		return fmt.Errorf("invalid plan status %q", plan.Status)
	}
	if plan.Status == "approved" && (plan.ApprovedBy == nil || plan.ApprovedAtUnixMS == nil) {
		return errors.New("approved plan requires attribution")
	}
	if plan.ApprovedBy != nil && *plan.ApprovedBy == uuid.Nil {
		return errors.New("approved_by must not be nil")
	}
	if plan.CreatedAtUnixMS <= 0 || plan.UpdatedAtUnixMS <= 0 {
		return errors.New("plan timestamps must be positive")
	}

	tasks := make(map[uuid.UUID]PlanTask, len(plan.Tasks))
	for _, task := range plan.Tasks {
		if task.ID == uuid.Nil {
			return errors.New("plan task id must not be nil")
		}
		if task.PlanID != plan.ID {
			return fmt.Errorf("plan task %s belongs to a different plan", task.ID)
		}
		if strings.TrimSpace(task.Title) == "" {
			return errors.New("plan task title must not be empty")
		}
		if !isPlanTaskStatus(task.Status) {
			return fmt.Errorf("invalid plan task status %q", task.Status)
		}
		if task.AssignedContractID != nil && *task.AssignedContractID == uuid.Nil {
			return errors.New("assigned_contract_id must not be nil")
		}
		if _, exists := tasks[task.ID]; exists {
			return fmt.Errorf("duplicate plan task %s", task.ID)
		}
		tasks[task.ID] = task
	}
	for _, task := range plan.Tasks {
		seenDependencies := make(map[uuid.UUID]struct{}, len(task.DependsOn))
		for _, dependencyID := range task.DependsOn {
			if dependencyID == task.ID {
				return fmt.Errorf("plan task %s depends on itself", task.ID)
			}
			if _, exists := tasks[dependencyID]; !exists {
				return fmt.Errorf("plan task %s references unknown dependency %s", task.ID, dependencyID)
			}
			if _, duplicate := seenDependencies[dependencyID]; duplicate {
				return fmt.Errorf("plan task %s repeats dependency %s", task.ID, dependencyID)
			}
			seenDependencies[dependencyID] = struct{}{}
		}
		if task.ReviewTargetTaskID != nil {
			if *task.ReviewTargetTaskID == uuid.Nil {
				return errors.New("review_target_task_id must not be nil")
			}
			if _, ok := seenDependencies[*task.ReviewTargetTaskID]; !ok {
				return fmt.Errorf("plan task %s review target must be one of its dependencies", task.ID)
			}
		}
	}

	visiting := make(map[uuid.UUID]bool, len(tasks))
	visited := make(map[uuid.UUID]bool, len(tasks))
	var visit func(uuid.UUID) error
	visit = func(taskID uuid.UUID) error {
		if visiting[taskID] {
			return errors.New("plan task dependencies contain a cycle")
		}
		if visited[taskID] {
			return nil
		}
		visiting[taskID] = true
		for _, dependencyID := range tasks[taskID].DependsOn {
			if err := visit(dependencyID); err != nil {
				return err
			}
		}
		visiting[taskID] = false
		visited[taskID] = true
		return nil
	}
	for taskID := range tasks {
		if err := visit(taskID); err != nil {
			return err
		}
	}
	return nil
}

func DecodePostMessage(envelope CommandEnvelope) (PostMessageCommand, error) {
	var payload PostMessageCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandPostMessage {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandPostMessage, payload.Type)
	}
	if err := validateMessage(payload.Message, envelope.WorkItemID, envelope.ActorID); err != nil {
		return payload, err
	}
	if err := validateMessageRouting(payload.Message, payload.Routing); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeUpdateMessageDelivery(envelope CommandEnvelope) (UpdateMessageDeliveryCommand, error) {
	var payload UpdateMessageDeliveryCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpdateMessageDelivery {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpdateMessageDelivery, payload.Type)
	}
	if payload.MessageID == uuid.Nil {
		return payload, errors.New("message_id must not be nil")
	}
	if payload.DeliveryState != "delivered" && payload.DeliveryState != "read" && payload.DeliveryState != "failed" {
		return payload, errors.New("message delivery update requires delivered, read, or failed")
	}
	return payload, nil
}

func DecodeRecordEvidence(envelope CommandEnvelope) (RecordEvidenceCommand, error) {
	var payload RecordEvidenceCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandRecordEvidence {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandRecordEvidence, payload.Type)
	}
	if payload.Evidence.ID == uuid.Nil || payload.Evidence.WorkItemID != envelope.WorkItemID || payload.Evidence.ProducedBy != envelope.ActorID {
		return payload, errors.New("evidence identity, work item, and producer must match the command")
	}
	if payload.Evidence.TaskID != nil && *payload.Evidence.TaskID == uuid.Nil {
		return payload, errors.New("evidence task_id must not be nil")
	}
	if payload.Evidence.CreatedAtUnixMS != envelope.TimestampUnixMS || strings.TrimSpace(payload.Evidence.Summary) == "" || strings.TrimSpace(payload.Evidence.Source) == "" {
		return payload, errors.New("evidence requires a summary, source, and command timestamp")
	}
	switch payload.Evidence.Kind {
	case "file_change", "command", "test", "review", "artifact", "user_confirmation", "external_reference":
	default:
		return payload, errors.New("evidence kind is invalid")
	}
	return payload, nil
}

func DecodeCompleteShapingTurn(envelope CommandEnvelope) (CompleteShapingTurnCommand, error) {
	var payload CompleteShapingTurnCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandCompleteShapingTurn {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandCompleteShapingTurn, payload.Type)
	}
	if err := validateMessage(payload.Message, envelope.WorkItemID, envelope.ActorID); err != nil {
		return payload, err
	}
	if err := validateShapingProposal(payload.Proposal); err != nil {
		return payload, err
	}
	if err := validateShapingRecords(payload.Records, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.Records.ProposalRevision != payload.Proposal.Revision {
		return payload, errors.New("shaping records revision does not match proposal revision")
	}
	if err := validateShapingRecordProvenance(payload.Records, payload.Message); err != nil {
		return payload, err
	}
	if (payload.ProposedTeam == nil) != (payload.Plan == nil) {
		return payload, errors.New("a complete shaping snapshot requires both a plan and proposed team")
	}
	for _, artifact := range payload.Artifacts {
		if err := validateArtifact(artifact, envelope.WorkItemID); err != nil {
			return payload, err
		}
	}
	if payload.Plan != nil {
		if err := validatePlanRevision(*payload.Plan, payload.WorkContracts, envelope.WorkItemID, envelope.ActorID); err != nil {
			return payload, err
		}
		contractIDs := make(map[uuid.UUID]struct{}, len(payload.WorkContracts))
		for _, contract := range payload.WorkContracts {
			contractIDs[contract.ID] = struct{}{}
		}
		definitions := make(map[uuid.UUID]RoleDefinition, len(payload.RoleDefinitions))
		for _, definition := range payload.RoleDefinitions {
			if definition.Kind != "" && definition.Kind != "worker" && definition.Kind != "reviewer" {
				return payload, errors.New("role definition kind must be worker or reviewer")
			}
			definitions[definition.ID] = definition
		}
		if len(payload.ProposedTeam.Roles) != len(contractIDs) || len(payload.ProposedTeam.Roles) != len(definitions) {
			return payload, errors.New("the proposed team must exactly match the supplied roles and contracts")
		}
		for _, role := range payload.ProposedTeam.Roles {
			if _, ok := contractIDs[role.WorkContractID]; !ok {
				return payload, errors.New("proposed team references an unknown work contract")
			}
			definition, ok := definitions[role.RoleDefinitionID]
			if !ok {
				return payload, errors.New("proposed team references an unknown role definition")
			}
			if definition.Kind == "reviewer" {
				for _, contract := range payload.WorkContracts {
					if contract.ID == role.WorkContractID && len(contract.WriteScope) != 0 {
						return payload, errors.New("reviewer work contracts must remain read-only")
					}
				}
			}
		}
	} else if len(payload.Artifacts) != 0 || len(payload.RoleDefinitions) != 0 || len(payload.WorkContracts) != 0 {
		return payload, errors.New("partial shaping snapshot records are not allowed")
	}
	return payload, nil
}

func DecodeUpsertRequirement(envelope CommandEnvelope) (UpsertRequirementCommand, error) {
	var payload UpsertRequirementCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertRequirement {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertRequirement, payload.Type)
	}
	if err := validateShapingRecords(ShapingRecords{WorkItemID: envelope.WorkItemID, ProposalRevision: 1, Requirements: []ShapingRequirement{payload.Requirement}}, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if !provenanceContainsActor(payload.Requirement.Provenance, envelope.ActorID) {
		return payload, errors.New("requirement provenance does not include the command actor")
	}
	return payload, nil
}

func DecodeUpsertDecision(envelope CommandEnvelope) (UpsertDecisionCommand, error) {
	var payload UpsertDecisionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertDecision {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertDecision, payload.Type)
	}
	if err := validateShapingRecords(ShapingRecords{WorkItemID: envelope.WorkItemID, ProposalRevision: 1, Decisions: []ShapingDecision{payload.Decision}}, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.Decision.Status == "resolved" && (payload.Decision.Resolution == nil || strings.TrimSpace(*payload.Decision.Resolution) == "") {
		return payload, errors.New("resolved decision requires a resolution")
	}
	if !provenanceContainsActor(payload.Decision.Provenance, envelope.ActorID) {
		return payload, errors.New("decision provenance does not include the command actor")
	}
	return payload, nil
}

func DecodeUpsertAcceptanceCriterion(envelope CommandEnvelope) (UpsertAcceptanceCriterionCommand, error) {
	var payload UpsertAcceptanceCriterionCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertAcceptanceCriterion {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertAcceptanceCriterion, payload.Type)
	}
	if err := validateShapingRecords(ShapingRecords{WorkItemID: envelope.WorkItemID, ProposalRevision: 1, AcceptanceCriteria: []ShapingAcceptanceCriterion{payload.AcceptanceCriterion}}, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if !provenanceContainsActor(payload.AcceptanceCriterion.Provenance, envelope.ActorID) {
		return payload, errors.New("acceptance criterion provenance does not include the command actor")
	}
	return payload, nil
}

func DecodeUpsertArtifact(envelope CommandEnvelope) (UpsertArtifactCommand, error) {
	var payload UpsertArtifactCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertArtifact {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertArtifact, payload.Type)
	}
	if err := validateArtifact(payload.Artifact, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeReplaceShapingRecords(envelope CommandEnvelope) (ReplaceShapingRecordsCommand, error) {
	var payload ReplaceShapingRecordsCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandReplaceShapingRecords {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandReplaceShapingRecords, payload.Type)
	}
	if err := validateShapingRecords(payload.Records, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeReplaceArtifacts(envelope CommandEnvelope) (ReplaceArtifactsCommand, error) {
	var payload ReplaceArtifactsCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandReplaceArtifacts {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandReplaceArtifacts, payload.Type)
	}
	seen := make(map[uuid.UUID]struct{}, len(payload.Artifacts))
	for _, artifact := range payload.Artifacts {
		if err := validateArtifact(artifact, envelope.WorkItemID); err != nil {
			return payload, err
		}
		if _, exists := seen[artifact.ID]; exists {
			return payload, errors.New("replacement artifacts contain a duplicate identity")
		}
		seen[artifact.ID] = struct{}{}
	}
	return payload, nil
}

func DecodePlanUpdated(envelope EventEnvelope) (PlanUpdatedEvent, error) {
	var payload PlanUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventPlanUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventPlanUpdated, payload.Type)
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodePlanTaskStatusChanged(envelope EventEnvelope) (PlanTaskStatusChangedEvent, error) {
	var payload PlanTaskStatusChangedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventPlanTaskStatusChanged {
		return payload, fmt.Errorf("expected %q payload, got %q", EventPlanTaskStatusChanged, payload.Type)
	}
	if envelope.TaskID == nil || *envelope.TaskID != payload.TaskID {
		return payload, errors.New("event task_id does not match the task transition")
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.Previous == payload.Current || !isPlanTaskStatus(payload.Previous) || !isPlanTaskStatus(payload.Current) {
		return payload, errors.New("task transition requires distinct known statuses")
	}
	for _, task := range payload.Plan.Tasks {
		if task.ID == payload.TaskID {
			if task.Status != payload.Current {
				return payload, errors.New("task transition status does not match the updated plan")
			}
			return payload, nil
		}
	}
	return payload, errors.New("task transition references an unknown plan task")
}

func DecodeWorkBriefUpdated(envelope EventEnvelope) (WorkBriefUpdatedEvent, error) {
	var payload WorkBriefUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventWorkBriefUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventWorkBriefUpdated, payload.Type)
	}
	if err := validateWorkBrief(payload.WorkBrief, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeUpsertWorkBrief(envelope CommandEnvelope) (UpsertWorkBriefCommand, error) {
	var payload UpsertWorkBriefCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertWorkBrief {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertWorkBrief, payload.Type)
	}
	if err := validateWorkBrief(payload.WorkBrief, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.WorkBrief.UpdatedBy != envelope.ActorID {
		return payload, errors.New("work brief updater does not match the command actor")
	}
	return payload, nil
}

func validateWorkBrief(brief WorkBrief, workItemID uuid.UUID) error {
	if brief.ID == uuid.Nil || brief.WorkItemID == uuid.Nil || brief.CreatedBy == uuid.Nil || brief.UpdatedBy == uuid.Nil {
		return errors.New("work brief identity must not be nil")
	}
	if brief.WorkItemID != workItemID {
		return errors.New("work brief belongs to a different work item")
	}
	if brief.Version == 0 || strings.TrimSpace(brief.Outcome) == "" {
		return errors.New("work brief requires a positive version and outcome")
	}
	return nil
}

func DecodeUpsertWorkContract(envelope CommandEnvelope) (UpsertWorkContractCommand, error) {
	var payload UpsertWorkContractCommand
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != CommandUpsertWorkContract {
		return payload, fmt.Errorf("expected %q payload, got %q", CommandUpsertWorkContract, payload.Type)
	}
	if err := validateWorkContract(payload.WorkContract, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.WorkContract.UpdatedBy != envelope.ActorID {
		return payload, errors.New("work contract updater does not match the command actor")
	}
	if payload.WorkContract.AgentInstanceID != nil {
		return payload, errors.New("agent instances must be bound by provision_agent_instance")
	}
	return payload, nil
}

func DecodeWorkContractUpdated(envelope EventEnvelope) (WorkContractUpdatedEvent, error) {
	var payload WorkContractUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventWorkContractUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventWorkContractUpdated, payload.Type)
	}
	if err := validateWorkContract(payload.WorkContract, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeAgentInstanceProvisioned(envelope EventEnvelope) (AgentInstanceProvisionedEvent, error) {
	var payload AgentInstanceProvisionedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAgentInstanceProvisioned {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAgentInstanceProvisioned, payload.Type)
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateWorkContract(payload.WorkContract, envelope.WorkItemID); err != nil {
		return payload, err
	}
	instance := payload.AgentInstance
	contract := payload.WorkContract
	if instance.Status != "pending" || contract.Status != "approved" ||
		instance.RoleDefinitionID != contract.RoleDefinitionID ||
		instance.ContractID != contract.ID || contract.AgentInstanceID == nil ||
		*contract.AgentInstanceID != instance.ID {
		return payload, errors.New("provisioned agent and approved contract binding do not match")
	}
	return payload, nil
}

func DecodeAgentInstanceActivated(envelope EventEnvelope) (AgentInstanceActivatedEvent, error) {
	var payload AgentInstanceActivatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAgentInstanceActivated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAgentInstanceActivated, payload.Type)
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateWorkContract(payload.WorkContract, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionLease(payload.Lease, envelope.WorkItemID); err != nil {
		return payload, err
	}
	instance := payload.AgentInstance
	contract := payload.WorkContract
	lease := payload.Lease
	if instance.Status != "ready" || contract.Status != "active" || lease.Status != "active" ||
		instance.RoleDefinitionID != contract.RoleDefinitionID || instance.ContractID != contract.ID ||
		lease.ContractID != contract.ID || lease.AgentInstanceID != instance.ID ||
		instance.ActiveLeaseID == nil || *instance.ActiveLeaseID != lease.ID ||
		contract.AgentInstanceID == nil || *contract.AgentInstanceID != instance.ID {
		return payload, errors.New("activated agent, lease, and active contract do not match")
	}
	return payload, nil
}

func DecodeExecutionLeaseRenewed(envelope EventEnvelope) (ExecutionLeaseRenewedEvent, error) {
	var payload ExecutionLeaseRenewedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventExecutionLeaseRenewed {
		return payload, fmt.Errorf("expected %q payload, got %q", EventExecutionLeaseRenewed, payload.Type)
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionLease(payload.Lease, envelope.WorkItemID); err != nil {
		return payload, err
	}
	instance, lease := payload.AgentInstance, payload.Lease
	if lease.Status != "active" || lease.AgentInstanceID != instance.ID ||
		lease.ContractID != instance.ContractID || instance.ActiveLeaseID == nil ||
		*instance.ActiveLeaseID != lease.ID {
		return payload, errors.New("renewed lease and active agent do not match")
	}
	return payload, nil
}

func DecodeAgentExecutionRetryPrepared(envelope EventEnvelope) (AgentExecutionRetryPreparedEvent, error) {
	var payload AgentExecutionRetryPreparedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAgentExecutionRetryPrepared {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAgentExecutionRetryPrepared, payload.Type)
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionLease(payload.Lease, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	instance, lease := payload.AgentInstance, payload.Lease
	readyTask := false
	for _, task := range payload.Plan.Tasks {
		if task.AssignedContractID != nil && *task.AssignedContractID == instance.ContractID && task.Status == "ready" {
			readyTask = true
			break
		}
	}
	if instance.Status != "ready" || lease.Status != "active" ||
		lease.AgentInstanceID != instance.ID || lease.ContractID != instance.ContractID ||
		instance.ActiveLeaseID == nil || *instance.ActiveLeaseID != lease.ID || !readyTask {
		return payload, errors.New("retry-prepared agent, lease, and plan do not match")
	}
	return payload, nil
}

func DecodeAgentExecutionStarted(envelope EventEnvelope) (AgentExecutionStartedEvent, error) {
	var payload AgentExecutionStartedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAgentExecutionStarted {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAgentExecutionStarted, payload.Type)
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionLease(payload.Lease, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionAttempt(payload.Attempt, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	instance, lease, attempt := payload.AgentInstance, payload.Lease, payload.Attempt
	if envelope.TaskID == nil || *envelope.TaskID != attempt.TaskID {
		return payload, errors.New("execution event task identity does not match its attempt")
	}
	var task *PlanTask
	for index := range payload.Plan.Tasks {
		if payload.Plan.Tasks[index].ID == attempt.TaskID {
			task = &payload.Plan.Tasks[index]
			break
		}
	}
	if task == nil || instance.Status != "running" || lease.Status != "active" ||
		attempt.Status != "running" || task.Status != "running" ||
		attempt.AgentInstanceID != instance.ID || attempt.ContractID != instance.ContractID ||
		attempt.LeaseID != lease.ID || lease.AgentInstanceID != instance.ID ||
		lease.ContractID != instance.ContractID || instance.ActiveLeaseID == nil ||
		*instance.ActiveLeaseID != lease.ID || task.AssignedContractID == nil ||
		*task.AssignedContractID != instance.ContractID || attempt.ModelRoute != instance.ModelRoute {
		return payload, errors.New("running agent, attempt, lease, and plan task do not match")
	}
	if lease.ExpiresAtUnixMS <= envelope.TimestampUnixMS || instance.StartedAtUnixMS == nil ||
		*instance.StartedAtUnixMS > envelope.TimestampUnixMS ||
		attempt.StartedAtUnixMS != envelope.TimestampUnixMS {
		return payload, errors.New("execution must start under an unexpired lease at the event timestamp")
	}
	return payload, nil
}

func DecodeAgentExecutionFinished(envelope EventEnvelope) (AgentExecutionFinishedEvent, error) {
	var payload AgentExecutionFinishedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAgentExecutionFinished {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAgentExecutionFinished, payload.Type)
	}
	if strings.TrimSpace(payload.Summary) == "" {
		return payload, errors.New("execution outcome summary must not be empty")
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionLease(payload.Lease, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionAttempt(payload.Attempt, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	instance, lease, attempt := payload.AgentInstance, payload.Lease, payload.Attempt
	if envelope.TaskID == nil || *envelope.TaskID != attempt.TaskID ||
		attempt.FinishedAtUnixMS == nil || *attempt.FinishedAtUnixMS != envelope.TimestampUnixMS {
		return payload, errors.New("execution finish identity or timestamp does not match its event")
	}
	var task *PlanTask
	for index := range payload.Plan.Tasks {
		if payload.Plan.Tasks[index].ID == attempt.TaskID {
			task = &payload.Plan.Tasks[index]
			break
		}
	}
	if task == nil || attempt.AgentInstanceID != instance.ID ||
		attempt.ContractID != instance.ContractID || attempt.LeaseID != lease.ID ||
		lease.AgentInstanceID != instance.ID || lease.ContractID != instance.ContractID ||
		task.AssignedContractID == nil || *task.AssignedContractID != instance.ContractID ||
		attempt.ModelRoute != instance.ModelRoute {
		return payload, errors.New("finished agent, attempt, lease, and plan task do not match")
	}
	lifecycleMatches := false
	switch attempt.Status {
	case "succeeded":
		lifecycleMatches = instance.Status == "reviewing" && task.Status == "reviewing" &&
			lease.Status == "active" && instance.ActiveLeaseID != nil && *instance.ActiveLeaseID == lease.ID
	case "failed":
		lifecycleMatches = instance.Status == "failed" && task.Status == "failed" &&
			lease.Status == "released" && instance.ActiveLeaseID == nil
	case "cancelled":
		lifecycleMatches = instance.Status == "cancelled" && task.Status == "cancelled" &&
			lease.Status == "cancelled" && instance.ActiveLeaseID == nil
	case "timed_out":
		lifecycleMatches = instance.Status == "failed" && task.Status == "failed" &&
			lease.Status == "expired" && instance.ActiveLeaseID == nil
	case "paused":
		lifecycleMatches = instance.Status == "ready" && task.Status == "ready" &&
			lease.Status == "active" && instance.ActiveLeaseID != nil && *instance.ActiveLeaseID == lease.ID
	}
	if !lifecycleMatches {
		return payload, errors.New("execution terminal outcome does not match runtime lifecycle state")
	}
	return payload, nil
}

func DecodeAgentExecutionReviewResolved(envelope EventEnvelope) (AgentExecutionReviewResolvedEvent, error) {
	var payload AgentExecutionReviewResolvedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAgentExecutionReviewResolved {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAgentExecutionReviewResolved, payload.Type)
	}
	if err := validateAgentInstance(payload.AgentInstance, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateWorkContract(payload.WorkContract, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionLease(payload.Lease, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validateExecutionAttempt(payload.Attempt, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if err := validatePlan(payload.Plan, envelope.WorkItemID); err != nil {
		return payload, err
	}
	if payload.Attempt.Status != "succeeded" ||
		(payload.Resolution == "integrated" && payload.Attempt.ChangeSummary == nil) ||
		payload.Attempt.AgentInstanceID != payload.AgentInstance.ID ||
		payload.Attempt.ContractID != payload.WorkContract.ID || payload.Attempt.LeaseID != payload.Lease.ID {
		return payload, errors.New("resolved execution review identities do not match")
	}
	taskStatus := ""
	for _, task := range payload.Plan.Tasks {
		if task.ID == payload.Attempt.TaskID {
			taskStatus = task.Status
			break
		}
	}
	switch payload.Resolution {
	case "accepted":
		if payload.Attempt.ReviewResolution == nil || *payload.Attempt.ReviewResolution != "accepted" {
			return payload, errors.New("accepted execution review attempt is not resolved")
		}
		if payload.Attempt.ChangeSummary != nil || len(payload.WorkContract.WriteScope) != 0 || payload.AgentInstance.Status != "completed" || payload.AgentInstance.ActiveLeaseID != nil || payload.WorkContract.Status != "completed" || payload.Lease.Status != "released" || taskStatus != "verifying" {
			return payload, errors.New("accepted execution review lifecycle does not match")
		}
	case "integrated":
		if payload.Attempt.ReviewResolution == nil || *payload.Attempt.ReviewResolution != "integrated" {
			return payload, errors.New("integrated execution review attempt is not resolved")
		}
		if payload.AgentInstance.Status != "completed" || payload.AgentInstance.ActiveLeaseID != nil || payload.WorkContract.Status != "completed" || payload.Lease.Status != "released" || taskStatus != "verifying" {
			return payload, errors.New("integrated execution review lifecycle does not match")
		}
	case "revision_requested":
		if payload.Attempt.ReviewResolution == nil || *payload.Attempt.ReviewResolution != "revision_requested" {
			return payload, errors.New("revision-requested execution review attempt is not resolved")
		}
		if payload.AgentInstance.Status != "ready" || payload.AgentInstance.ActiveLeaseID == nil || *payload.AgentInstance.ActiveLeaseID != payload.Lease.ID || payload.WorkContract.Status != "active" || payload.Lease.Status != "active" || taskStatus != "ready" {
			return payload, errors.New("revision-requested execution review lifecycle does not match")
		}
	case "discarded":
		if payload.Attempt.ReviewResolution == nil || *payload.Attempt.ReviewResolution != "discarded" {
			return payload, errors.New("discarded execution review attempt is not resolved")
		}
		if payload.AgentInstance.Status != "cancelled" || payload.AgentInstance.ActiveLeaseID != nil || payload.WorkContract.Status != "revoked" || payload.Lease.Status != "cancelled" || taskStatus != "cancelled" {
			return payload, errors.New("discarded execution review lifecycle does not match")
		}
	default:
		return payload, fmt.Errorf("invalid execution review resolution %q", payload.Resolution)
	}
	return payload, nil
}

func validateExecutionAttempt(attempt ExecutionAttempt, workItemID uuid.UUID) error {
	for name, id := range map[string]uuid.UUID{
		"execution attempt id": attempt.ID, "work item id": attempt.WorkItemID,
		"task id": attempt.TaskID, "agent instance id": attempt.AgentInstanceID,
		"contract id": attempt.ContractID, "lease id": attempt.LeaseID,
	} {
		if id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	if attempt.WorkItemID != workItemID {
		return errors.New("execution attempt belongs to a different work item")
	}
	if attempt.AttemptNumber == 0 || strings.TrimSpace(attempt.ModelRoute) == "" ||
		attempt.StartedAtUnixMS <= 0 || attempt.UpdatedAtUnixMS <= 0 {
		return errors.New("execution attempt has invalid metadata")
	}
	if attempt.Isolation != nil {
		if err := validateExecutionIsolation(*attempt.Isolation); err != nil {
			return err
		}
	}
	if attempt.ChangeSummary != nil {
		if attempt.Isolation == nil || attempt.Isolation.Kind != "git_worktree" ||
			attempt.ChangeSummary.IsolationID != attempt.Isolation.ID ||
			attempt.Isolation.BaseRevision == nil ||
			attempt.ChangeSummary.BaseRevision != *attempt.Isolation.BaseRevision ||
			attempt.ChangeSummary.CapturedAtUnixMS <= 0 {
			return errors.New("execution change summary does not match its isolation")
		}
	}
	terminal := attempt.Status == "succeeded" || attempt.Status == "failed" ||
		attempt.Status == "cancelled" || attempt.Status == "timed_out" || attempt.Status == "paused"
	if attempt.Status == "running" && attempt.FinishedAtUnixMS != nil {
		return errors.New("running execution attempt must not have a finish timestamp")
	}
	if attempt.Status == "running" && attempt.ChangeSummary != nil {
		return errors.New("running execution attempt must not have a change summary")
	}
	if attempt.ReviewResolution != nil {
		if attempt.Status != "succeeded" ||
			(*attempt.ReviewResolution != "accepted" && *attempt.ReviewResolution != "integrated" && *attempt.ReviewResolution != "revision_requested" && *attempt.ReviewResolution != "discarded") ||
			(*attempt.ReviewResolution == "integrated" && attempt.ChangeSummary == nil) {
			return errors.New("execution review resolution requires a successful attributable attempt")
		}
	}
	if terminal && (attempt.FinishedAtUnixMS == nil || *attempt.FinishedAtUnixMS < attempt.StartedAtUnixMS) {
		return errors.New("terminal execution attempt requires a valid finish timestamp")
	}
	if attempt.Status != "running" && !terminal {
		return fmt.Errorf("invalid execution attempt status %q", attempt.Status)
	}
	return nil
}

func validateExecutionIsolation(isolation ExecutionIsolation) error {
	if isolation.ID == uuid.Nil {
		return errors.New("execution isolation id must not be nil")
	}
	switch isolation.Kind {
	case "shared_project":
		if isolation.BaseRevision != nil || isolation.SourceAttemptID != nil {
			return errors.New("shared project isolation must not have a base or source")
		}
	case "git_worktree":
		if isolation.SourceAttemptID != nil || isolation.BaseRevision == nil || strings.TrimSpace(*isolation.BaseRevision) == "" || len(*isolation.BaseRevision) > 128 {
			return errors.New("Git worktree isolation requires a base revision")
		}
	case "review_candidate":
		if isolation.SourceAttemptID == nil || *isolation.SourceAttemptID == uuid.Nil || isolation.BaseRevision == nil || strings.TrimSpace(*isolation.BaseRevision) == "" || len(*isolation.BaseRevision) > 128 {
			return errors.New("review candidate isolation requires a source attempt and base revision")
		}
	default:
		return fmt.Errorf("invalid execution isolation kind %q", isolation.Kind)
	}
	if isolation.BaseRevision != nil {
		for _, character := range *isolation.BaseRevision {
			if !strings.ContainsRune("0123456789abcdefABCDEF", character) {
				return errors.New("execution isolation base revision must be hexadecimal")
			}
		}
	}
	return nil
}

func validateExecutionLease(lease ExecutionLease, workItemID uuid.UUID) error {
	for name, id := range map[string]uuid.UUID{
		"execution lease id": lease.ID, "work item id": lease.WorkItemID,
		"agent instance id": lease.AgentInstanceID, "contract id": lease.ContractID,
	} {
		if id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	if lease.WorkItemID != workItemID {
		return errors.New("execution lease belongs to a different work item")
	}
	if lease.Status != "active" && lease.Status != "released" && lease.Status != "expired" && lease.Status != "cancelled" {
		return fmt.Errorf("invalid execution lease status %q", lease.Status)
	}
	if err := validateChargingAuthorization(lease.Charging); err != nil {
		return err
	}
	if lease.AcquiredAtUnixMS <= 0 || lease.UpdatedAtUnixMS <= 0 || lease.ExpiresAtUnixMS <= lease.AcquiredAtUnixMS {
		return errors.New("invalid execution lease window")
	}
	return nil
}

func validateAgentInstance(instance AgentInstance, workItemID uuid.UUID) error {
	for name, id := range map[string]uuid.UUID{
		"agent instance id": instance.ID, "work item id": instance.WorkItemID,
		"role definition id": instance.RoleDefinitionID, "contract id": instance.ContractID,
	} {
		if id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	if instance.WorkItemID != workItemID {
		return errors.New("agent instance belongs to a different work item")
	}
	if !isAgentInstanceStatus(instance.Status) {
		return fmt.Errorf("invalid agent instance status %q", instance.Status)
	}
	if instance.ExecutionLocation != "local" && instance.ExecutionLocation != "cloud" && instance.ExecutionLocation != "hybrid" {
		return fmt.Errorf("invalid execution_location %q", instance.ExecutionLocation)
	}
	if strings.TrimSpace(instance.ModelRoute) == "" {
		return errors.New("agent model route must not be empty")
	}
	if instance.UpdatedAtUnixMS <= 0 {
		return errors.New("agent instance timestamp must be positive")
	}
	if instance.Status == "pending" && instance.ActiveLeaseID != nil {
		return errors.New("pending agent must not have an active lease")
	}
	if instance.Status != "pending" && instance.Status != "completed" && instance.Status != "failed" && instance.Status != "cancelled" && instance.ActiveLeaseID == nil {
		return errors.New("active agent status requires an execution lease")
	}
	if instance.Status == "pending" && instance.StartedAtUnixMS != nil {
		return errors.New("pending agents must not have a start timestamp")
	}
	if instance.Status != "pending" && instance.Status != "ready" && instance.Status != "cancelled" && instance.StartedAtUnixMS == nil {
		return errors.New("started agent status requires a start timestamp")
	}
	return nil
}

func isAgentInstanceStatus(status string) bool {
	switch status {
	case "pending", "ready", "running", "waiting", "blocked", "reviewing", "completed", "failed", "cancelled":
		return true
	default:
		return false
	}
}

func validateWorkContract(contract WorkContract, workItemID uuid.UUID) error {
	for name, id := range map[string]uuid.UUID{
		"work contract id": contract.ID, "work item id": contract.WorkItemID,
		"role definition id": contract.RoleDefinitionID, "creator id": contract.CreatedBy,
		"updater id": contract.UpdatedBy,
	} {
		if id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	if contract.WorkItemID != workItemID {
		return errors.New("work contract belongs to a different work item")
	}
	if contract.Version == 0 {
		return errors.New("work contract version must be positive")
	}
	if strings.TrimSpace(contract.Mission) == "" {
		return errors.New("work contract mission must not be empty")
	}
	if len(contract.SuccessCriteria) == 0 {
		return errors.New("work contract requires success criteria")
	}
	for _, id := range contract.SuccessCriteria {
		if id == uuid.Nil {
			return errors.New("work contract success criteria must not contain nil identifiers")
		}
	}
	for _, ids := range [][]uuid.UUID{contract.TaskIDs, contract.InputArtifactIDs} {
		for _, id := range ids {
			if id == uuid.Nil {
				return errors.New("work contract references must not contain nil identifiers")
			}
		}
	}
	if contract.AgentInstanceID != nil && *contract.AgentInstanceID == uuid.Nil {
		return errors.New("agent_instance_id must not be nil")
	}
	switch contract.Status {
	case "draft", "proposed":
		if contract.AgentInstanceID != nil {
			return errors.New("draft and proposed work contracts cannot identify a runtime agent")
		}
	case "approved", "revoked":
	case "active", "completed":
		if contract.AgentInstanceID == nil {
			return errors.New("active and completed work contracts require a runtime agent")
		}
	default:
		return fmt.Errorf("invalid work contract status %q", contract.Status)
	}
	if contract.ExecutionLocation != "local" && contract.ExecutionLocation != "cloud" && contract.ExecutionLocation != "hybrid" {
		return fmt.Errorf("invalid execution_location %q", contract.ExecutionLocation)
	}
	if contract.PreferredModelRoute != nil {
		route := *contract.PreferredModelRoute
		if strings.TrimSpace(route) == "" || len(route) > 200 || strings.EqualFold(route, "auto") || strings.IndexFunc(route, func(character rune) bool {
			return unicode.IsSpace(character) || unicode.IsControl(character)
		}) >= 0 {
			return fmt.Errorf("invalid preferred_model_route %q", route)
		}
	}
	writeScopes := make(map[string]struct{}, len(contract.WriteScope))
	for _, scope := range contract.WriteScope {
		trimmed := strings.TrimSpace(scope)
		cleaned := pathpkg.Clean(trimmed)
		if trimmed == "" || pathpkg.IsAbs(trimmed) || cleaned == ".." || strings.HasPrefix(cleaned, "../") {
			return fmt.Errorf("invalid work contract write scope %q", scope)
		}
		writeScopes[cleaned] = struct{}{}
	}
	permissionKeys := make(map[string]struct{}, len(contract.Permissions))
	for _, grant := range contract.Permissions {
		key := grant.Permission + "\x00"
		if grant.Scope != nil {
			key += *grant.Scope
		}
		if _, duplicate := permissionKeys[key]; duplicate {
			return fmt.Errorf("duplicate work contract permission %q", grant.Permission)
		}
		permissionKeys[key] = struct{}{}
		switch grant.Permission {
		case "project_read":
			if grant.Scope != nil || grant.RequiresApproval {
				return errors.New("project_read must be unscoped and allowed")
			}
		case "project_write":
			if grant.Scope == nil {
				return errors.New("project_write requires a validated write scope")
			}
			cleaned := pathpkg.Clean(strings.TrimSpace(*grant.Scope))
			if _, ok := writeScopes[cleaned]; !ok {
				return errors.New("project_write permission is outside the contract write scope")
			}
		case "command_execute", "agent_delegate", "external_write":
			if grant.Scope != nil {
				return fmt.Errorf("%s must be unscoped", grant.Permission)
			}
		default:
			return fmt.Errorf("unsupported work contract permission %q", grant.Permission)
		}
	}
	return nil
}

func DecodeMessagePosted(envelope EventEnvelope) (MessagePostedEvent, error) {
	var payload MessagePostedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventMessagePosted {
		return payload, fmt.Errorf("expected %q payload, got %q", EventMessagePosted, payload.Type)
	}
	if err := validateMessage(payload.Message, envelope.WorkItemID, envelope.ActorID); err != nil {
		return payload, err
	}
	if err := validateMessageRouting(payload.Message, payload.Routing); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeMessageDeliveryUpdated(envelope EventEnvelope) (MessageDeliveryUpdatedEvent, error) {
	var payload MessageDeliveryUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventMessageDeliveryUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventMessageDeliveryUpdated, payload.Type)
	}
	if err := validateMessage(payload.Message, envelope.WorkItemID, payload.Message.SenderID); err != nil {
		return payload, err
	}
	if err := validateMessageRouting(payload.Message, &payload.Routing); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeShapingProposalUpdated(envelope EventEnvelope) (ShapingProposalUpdatedEvent, error) {
	var payload ShapingProposalUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventShapingProposalUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventShapingProposalUpdated, payload.Type)
	}
	if err := validateShapingProposal(payload.Proposal); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeShapingRecordsUpdated(envelope EventEnvelope) (ShapingRecordsUpdatedEvent, error) {
	var payload ShapingRecordsUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventShapingRecordsUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventShapingRecordsUpdated, payload.Type)
	}
	if err := validateShapingRecords(payload.Records, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeRequirementUpdated(envelope EventEnvelope) (RequirementUpdatedEvent, error) {
	var payload RequirementUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventRequirementUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventRequirementUpdated, payload.Type)
	}
	if err := validateShapingRecords(ShapingRecords{WorkItemID: envelope.WorkItemID, ProposalRevision: 1, Requirements: []ShapingRequirement{payload.Requirement}}, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeDecisionUpdated(envelope EventEnvelope) (DecisionUpdatedEvent, error) {
	var payload DecisionUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventDecisionUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventDecisionUpdated, payload.Type)
	}
	if err := validateShapingRecords(ShapingRecords{WorkItemID: envelope.WorkItemID, ProposalRevision: 1, Decisions: []ShapingDecision{payload.Decision}}, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeAcceptanceCriterionUpdated(envelope EventEnvelope) (AcceptanceCriterionUpdatedEvent, error) {
	var payload AcceptanceCriterionUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventAcceptanceCriterionUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventAcceptanceCriterionUpdated, payload.Type)
	}
	if err := validateShapingRecords(ShapingRecords{WorkItemID: envelope.WorkItemID, ProposalRevision: 1, AcceptanceCriteria: []ShapingAcceptanceCriterion{payload.AcceptanceCriterion}}, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeArtifactUpdated(envelope EventEnvelope) (ArtifactUpdatedEvent, error) {
	var payload ArtifactUpdatedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventArtifactUpdated {
		return payload, fmt.Errorf("expected %q payload, got %q", EventArtifactUpdated, payload.Type)
	}
	if err := validateArtifact(payload.Artifact, envelope.WorkItemID); err != nil {
		return payload, err
	}
	return payload, nil
}

func DecodeArtifactsReplaced(envelope EventEnvelope) (ArtifactsReplacedEvent, error) {
	var payload ArtifactsReplacedEvent
	if err := decodeStrict(envelope.Payload, &payload); err != nil {
		return payload, err
	}
	if payload.Type != EventArtifactsReplaced {
		return payload, fmt.Errorf("expected %q payload, got %q", EventArtifactsReplaced, payload.Type)
	}
	seen := make(map[uuid.UUID]struct{}, len(payload.Artifacts))
	for _, artifact := range payload.Artifacts {
		if err := validateArtifact(artifact, envelope.WorkItemID); err != nil {
			return payload, err
		}
		if _, exists := seen[artifact.ID]; exists {
			return payload, errors.New("replacement artifacts contain a duplicate identity")
		}
		seen[artifact.ID] = struct{}{}
	}
	return payload, nil
}

func validateArtifact(artifact ArtifactRef, workItemID uuid.UUID) error {
	if artifact.ID == uuid.Nil || artifact.WorkItemID == uuid.Nil {
		return errors.New("artifact identities must not be nil")
	}
	if artifact.WorkItemID != workItemID {
		return errors.New("artifact belongs to a different work item")
	}
	path := strings.TrimSpace(artifact.RelativePath)
	cleaned := pathpkg.Clean(path)
	if path == "" || pathpkg.IsAbs(path) || cleaned == ".." || strings.HasPrefix(cleaned, "../") {
		return fmt.Errorf("invalid artifact relative_path %q", artifact.RelativePath)
	}
	switch artifact.Kind {
	case "product_brief", "requirements", "decision", "technical_plan", "test_plan", "review", "design", "other":
		return nil
	default:
		return fmt.Errorf("invalid artifact kind %q", artifact.Kind)
	}
}

func validateMessage(message AgentMessage, workItemID, actorID uuid.UUID) error {
	for name, id := range map[string]uuid.UUID{
		"message id": message.ID, "message thread id": message.ThreadID,
		"message work item id": message.WorkItemID, "message sender id": message.SenderID,
	} {
		if id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	if message.WorkItemID != workItemID {
		return errors.New("message belongs to a different work item")
	}
	if message.SenderID != actorID {
		return errors.New("message sender does not match the envelope actor")
	}
	if strings.TrimSpace(message.Body) == "" {
		return errors.New("message body must not be empty")
	}
	if message.Kind == "review_finding" {
		if message.ReviewFinding == nil || message.ReviewFinding.CandidateAttemptID == uuid.Nil || strings.TrimSpace(message.ReviewFinding.Summary) == "" {
			return errors.New("review finding messages require structured findings")
		}
		finding := message.ReviewFinding
		if finding.Verdict != "pass" && finding.Verdict != "needs_revision" && finding.Verdict != "blocked" {
			return errors.New("invalid review verdict")
		}
		if finding.Verdict == "needs_revision" && len(finding.Findings) == 0 {
			return errors.New("needs_revision requires actionable findings")
		}
		for _, item := range finding.Findings {
			if strings.TrimSpace(item.Summary) == "" || (item.Severity != "note" && item.Severity != "warning" && item.Severity != "error" && item.Severity != "critical") {
				return errors.New("invalid review finding item")
			}
			if item.Line != nil && *item.Line == 0 {
				return errors.New("review finding line must be positive")
			}
			if item.RelativePath != nil {
				cleaned := pathpkg.Clean(strings.TrimSpace(*item.RelativePath))
				if cleaned == "." || pathpkg.IsAbs(cleaned) || cleaned == ".." || strings.HasPrefix(cleaned, "../") {
					return errors.New("review finding path must be project-relative")
				}
			}
		}
	} else if message.ReviewFinding != nil {
		return errors.New("structured findings are only valid on review finding messages")
	}
	if message.CreatedAtUnixMS <= 0 || message.UpdatedAtUnixMS <= 0 {
		return errors.New("message timestamps must be positive")
	}
	if message.ReplyTo != nil && *message.ReplyTo == uuid.Nil {
		return errors.New("message reply_to must not be nil")
	}
	for _, ids := range [][]uuid.UUID{message.ArtifactIDs, message.EvidenceIDs} {
		for _, id := range ids {
			if id == uuid.Nil {
				return errors.New("message references must not contain nil identifiers")
			}
		}
	}
	return nil
}

func validateMessageRouting(message AgentMessage, routing *AgentMessageRouting) error {
	operational := message.Kind == "handoff" || message.Kind == "blocker" ||
		message.Kind == "decision_request" || message.Kind == "review_finding" ||
		message.Kind == "revision_request" || message.Kind == "approval"
	if !operational && routing == nil {
		return nil
	}
	if routing == nil || len(routing.RecipientIDs) == 0 {
		return errors.New("operational agent messages require routing metadata and recipients")
	}
	seen := map[uuid.UUID]struct{}{}
	for _, recipient := range routing.RecipientIDs {
		if recipient == uuid.Nil || recipient == message.SenderID {
			return errors.New("agent message recipients must be non-nil and exclude the sender")
		}
		if _, exists := seen[recipient]; exists {
			return errors.New("agent message recipients must be distinct")
		}
		seen[recipient] = struct{}{}
	}
	// Every operational kind but `decision_request` is about exactly one
	// task. `decision_request` is also how a human addresses a work-item-level
	// question to the Principal while agents run — the one operational kind
	// allowed to omit the task; non-operational kinds (`discussion`,
	// `status_update` — e.g. the Principal's answer to that human) never
	// needed one (mirrors the Rust validators).
	taskIDRequired := operational && message.Kind != "decision_request"
	if (routing.TaskID != nil && *routing.TaskID == uuid.Nil) ||
		(taskIDRequired && routing.TaskID == nil) ||
		(routing.ExecutionAttemptID != nil && *routing.ExecutionAttemptID == uuid.Nil) ||
		(message.Kind == "approval" && message.ReplyTo == nil) {
		return errors.New("agent message routing does not match its message kind")
	}
	return nil
}

func validateShapingProposal(proposal ShapingProposal) error {
	if proposal.Revision == 0 {
		return errors.New("shaping proposal revision must be positive")
	}
	if strings.TrimSpace(proposal.Outcome) == "" || strings.TrimSpace(proposal.PlanSummary) == "" {
		return errors.New("shaping proposal requires an outcome and plan summary")
	}
	for _, values := range [][]string{proposal.Requirements, proposal.OpenDecisions, proposal.AcceptanceCriteria} {
		for _, value := range values {
			if strings.TrimSpace(value) == "" {
				return errors.New("shaping proposal list items must not be empty")
			}
		}
	}
	return nil
}

func validateShapingRecords(records ShapingRecords, workItemID uuid.UUID) error {
	if records.WorkItemID == uuid.Nil || records.WorkItemID != workItemID {
		return errors.New("shaping records belong to a different work item")
	}
	if records.ProposalRevision == 0 {
		return errors.New("shaping records revision must be positive")
	}
	identities := make(map[uuid.UUID]struct{}, len(records.Requirements)+len(records.Decisions)+len(records.AcceptanceCriteria))
	validateRecord := func(id, recordWorkItemID uuid.UUID, content string, provenance []Provenance) error {
		if id == uuid.Nil {
			return errors.New("shaping record identity must not be nil")
		}
		if _, exists := identities[id]; exists {
			return errors.New("shaping record identities must be unique")
		}
		identities[id] = struct{}{}
		if recordWorkItemID != records.WorkItemID {
			return errors.New("shaping record belongs to a different work item")
		}
		if strings.TrimSpace(content) == "" {
			return errors.New("shaping record content must not be empty")
		}
		if len(provenance) == 0 {
			return errors.New("shaping record provenance must not be empty")
		}
		for _, entry := range provenance {
			if entry.RecordedBy == uuid.Nil || entry.RecordedAtUnixMS <= 0 {
				return errors.New("shaping record provenance requires actor and timestamp")
			}
			switch entry.Source.Type {
			case "agent_message":
				if entry.Source.ThreadID == nil || *entry.Source.ThreadID == uuid.Nil || entry.Source.MessageID == nil || *entry.Source.MessageID == uuid.Nil {
					return errors.New("agent-message provenance requires thread and message identities")
				}
			case "manual_edit":
				if entry.Source.ThreadID != nil || entry.Source.MessageID != nil {
					return errors.New("manual-edit provenance must not carry message identities")
				}
			default:
				return errors.New("unsupported shaping record provenance source")
			}
		}
		return nil
	}
	for _, requirement := range records.Requirements {
		if err := validateRecord(requirement.ID, requirement.WorkItemID, requirement.Summary, requirement.Provenance); err != nil {
			return err
		}
		if requirement.Priority != "low" && requirement.Priority != "normal" && requirement.Priority != "high" && requirement.Priority != "critical" {
			return errors.New("requirement priority is invalid")
		}
		if requirement.Status != "draft" && requirement.Status != "accepted" && requirement.Status != "rejected" && requirement.Status != "superseded" {
			return errors.New("requirement status is invalid")
		}
	}
	for _, decision := range records.Decisions {
		if err := validateRecord(decision.ID, decision.WorkItemID, decision.Question, decision.Provenance); err != nil {
			return err
		}
		if decision.Status != "open" && decision.Status != "resolved" && decision.Status != "superseded" {
			return errors.New("decision status is invalid")
		}
		if decision.Status == "resolved" && (decision.Resolution == nil || strings.TrimSpace(*decision.Resolution) == "") {
			return errors.New("resolved decision requires a resolution")
		}
	}
	for _, criterion := range records.AcceptanceCriteria {
		if err := validateRecord(criterion.ID, criterion.WorkItemID, criterion.Description, criterion.Provenance); err != nil {
			return err
		}
		for _, evidenceID := range criterion.EvidenceIDs {
			if evidenceID == uuid.Nil {
				return errors.New("acceptance criterion evidence must not contain nil identifiers")
			}
		}
		if criterion.Status != "pending" && criterion.Status != "satisfied" && criterion.Status != "failed" && criterion.Status != "waived" {
			return errors.New("acceptance criterion status is invalid")
		}
		if criterion.Status == "satisfied" && len(criterion.EvidenceIDs) == 0 {
			return errors.New("satisfied acceptance criterion requires evidence")
		}
		if criterion.Status == "waived" && (criterion.WaivedBy == nil || criterion.WaiverReason == nil || strings.TrimSpace(*criterion.WaiverReason) == "") {
			return errors.New("waived acceptance criterion requires attribution")
		}
	}
	return nil
}

func validateShapingRecordProvenance(records ShapingRecords, message AgentMessage) error {
	validate := func(entries []Provenance) error {
		for _, entry := range entries {
			if entry.RecordedBy == message.SenderID &&
				entry.Source.Type == "agent_message" &&
				entry.Source.ThreadID != nil &&
				*entry.Source.ThreadID == message.ThreadID &&
				entry.Source.MessageID != nil &&
				*entry.Source.MessageID == message.ID {
				return nil
			}
		}
		return errors.New("shaping record provenance does not reference the principal message")
	}
	for _, requirement := range records.Requirements {
		if err := validate(requirement.Provenance); err != nil {
			return err
		}
	}
	for _, decision := range records.Decisions {
		if err := validate(decision.Provenance); err != nil {
			return err
		}
	}
	for _, criterion := range records.AcceptanceCriteria {
		if err := validate(criterion.Provenance); err != nil {
			return err
		}
	}
	return nil
}

func provenanceContainsActor(entries []Provenance, actorID uuid.UUID) bool {
	for _, entry := range entries {
		if entry.RecordedBy == actorID {
			return true
		}
	}
	return false
}

func requiresExpectedRevision(payloadType string) bool {
	switch payloadType {
	case CommandTransitionWork, CommandTransitionPlanTask, CommandProvisionAgentInstance, CommandActivateAgentInstance, CommandRenewExecutionLease, CommandPrepareAgentExecutionRetry, CommandStartAgentExecution, CommandFinishAgentExecution, CommandCancelAgentExecution, CommandPauseAgentExecution, CommandResolveAgentExecutionReview, CommandUpdateMessageDelivery, CommandRecordEvidence, CommandRaiseAttention, CommandResolveAttention, CommandUpsertWorkBrief, CommandUpsertRequirement,
		CommandUpsertDecision, CommandUpsertAcceptanceCriterion, CommandUpsertArtifact, CommandReplaceShapingRecords, CommandReplaceArtifacts, CommandUpsertConstraint,
		CommandUpsertRisk, CommandUpsertPlan, CommandRevisePlan, CommandUpsertWorkContract, CommandUpsertProposedTeam:
		return true
	default:
		return false
	}
}

func validateCommon(version uint32, projectID, actorID, correlationID uuid.UUID, timestamp int64) error {
	if version != SchemaVersion {
		return fmt.Errorf("unsupported schema_version %d (supported %d)", version, SchemaVersion)
	}
	for name, id := range map[string]uuid.UUID{
		"project_id": projectID, "actor_id": actorID, "correlation_id": correlationID,
	} {
		if id == uuid.Nil {
			return fmt.Errorf("%s must not be nil", name)
		}
	}
	if timestamp <= 0 {
		return errors.New("timestamp_unix_ms must be positive")
	}
	return nil
}

func payloadDiscriminator(raw json.RawMessage) (string, error) {
	var header struct {
		Type string `json:"type"`
	}
	if err := json.Unmarshal(raw, &header); err != nil {
		return "", fmt.Errorf("decode payload discriminator: %w", err)
	}
	if strings.TrimSpace(header.Type) == "" {
		return "", errors.New("payload type is required")
	}
	return header.Type, nil
}

func decodeStrict(raw json.RawMessage, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return fmt.Errorf("decode workspace payload: %w", err)
	}
	return nil
}
