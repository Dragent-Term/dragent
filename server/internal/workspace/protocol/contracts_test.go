package protocol

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"

	"github.com/google/uuid"
)

func sharedFixture(t *testing.T, name string) []byte {
	t.Helper()
	_, filename, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve test source path")
	}
	path := filepath.Join(filepath.Dir(filename), "..", "..", "..", "..", "contracts", "workspace", "v1", "fixtures", name)
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read shared fixture %s: %v", path, err)
	}
	return data
}

func assertSameJSON(t *testing.T, expected, actual []byte) {
	t.Helper()
	var expectedValue, actualValue any
	if err := json.Unmarshal(expected, &expectedValue); err != nil {
		t.Fatalf("decode expected JSON: %v", err)
	}
	if err := json.Unmarshal(actual, &actualValue); err != nil {
		t.Fatalf("decode actual JSON: %v", err)
	}
	if !reflect.DeepEqual(expectedValue, actualValue) {
		t.Fatalf("JSON mismatch\nexpected: %s\nactual: %s", expected, actual)
	}
}

func TestSharedCreateWorkItemFixture(t *testing.T) {
	fixture := sharedFixture(t, "create_work_item.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeCreateWorkItem(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.ExecutionLocation != "hybrid" {
		t.Fatalf("execution location = %q, want hybrid", payload.ExecutionLocation)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedRaiseAttentionFixture(t *testing.T) {
	fixture := sharedFixture(t, "raise_attention.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeRaiseAttention(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.AttentionItem.Kind != "decision" {
		t.Fatalf("attention kind = %q, want decision", payload.AttentionItem.Kind)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedRoutedAgentMessageFixture(t *testing.T) {
	fixture := sharedFixture(t, "post_agent_message.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodePostMessage(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Message.Kind != "handoff" {
		t.Fatalf("message kind = %q, want handoff", payload.Message.Kind)
	}
	if payload.Routing == nil || len(payload.Routing.RecipientIDs) != 1 {
		t.Fatalf("routing recipients = %#v, want one recipient", payload.Routing)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedStructuredReviewFindingFixture(t *testing.T) {
	fixture := sharedFixture(t, "post_review_finding.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodePostMessage(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Message.ReviewFinding == nil || payload.Message.ReviewFinding.Verdict != "needs_revision" {
		t.Fatalf("structured review finding = %#v", payload.Message.ReviewFinding)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedMessageDeliveryUpdateFixture(t *testing.T) {
	fixture := sharedFixture(t, "update_message_delivery.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeUpdateMessageDelivery(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.DeliveryState != "read" {
		t.Fatalf("delivery state = %q, want read", payload.DeliveryState)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedRecordEvidenceFixture(t *testing.T) {
	fixture := sharedFixture(t, "record_evidence.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeRecordEvidence(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Evidence.Kind != "test" {
		t.Fatalf("evidence kind = %q, want test", payload.Evidence.Kind)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedWorkBriefUpdatedFixture(t *testing.T) {
	fixture := sharedFixture(t, "work_brief_updated.event.json")
	var envelope EventEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode event envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate event envelope: %v", err)
	}
	payload, err := DecodeWorkBriefUpdated(envelope)
	if err != nil {
		t.Fatalf("decode event payload: %v", err)
	}
	if payload.WorkBrief.Version != 2 {
		t.Fatalf("work brief version = %d, want 2", payload.WorkBrief.Version)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode event envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestWorkBriefEditRequiresRevisionAndUpdaterActor(t *testing.T) {
	fixture := sharedFixture(t, "work_brief_updated.event.json")
	var event EventEnvelope
	if err := json.Unmarshal(fixture, &event); err != nil {
		t.Fatal(err)
	}
	updated, err := DecodeWorkBriefUpdated(event)
	if err != nil {
		t.Fatal(err)
	}
	payload, _ := json.Marshal(UpsertWorkBriefCommand{
		Type:      CommandUpsertWorkBrief,
		WorkBrief: updated.WorkBrief,
	})
	command := CommandEnvelope{
		CommandID:       uuid.New(),
		SchemaVersion:   SchemaVersion,
		ProjectID:       event.ProjectID,
		WorkItemID:      event.WorkItemID,
		ActorID:         event.ActorID,
		CorrelationID:   event.CorrelationID,
		TimestampUnixMS: event.TimestampUnixMS,
		Payload:         payload,
	}
	if err := command.Validate(); err == nil {
		t.Fatal("work brief edit without expected revision was accepted")
	}
	revision := uint64(7)
	command.ExpectedRevision = &revision
	if err := command.Validate(); err != nil {
		t.Fatalf("valid work brief edit was rejected: %v", err)
	}
	command.ActorID = uuid.New()
	if err := command.Validate(); err == nil {
		t.Fatal("work brief edit from a different actor was accepted")
	}
}

func TestTransitionWorkRequiresRevisionAndKnownStatus(t *testing.T) {
	payload, _ := json.Marshal(TransitionWorkCommand{
		Type:   CommandTransitionWork,
		Status: "understanding",
	})
	command := CommandEnvelope{
		CommandID:       uuid.New(),
		SchemaVersion:   SchemaVersion,
		ProjectID:       uuid.New(),
		WorkItemID:      uuid.New(),
		ActorID:         uuid.New(),
		CorrelationID:   uuid.New(),
		TimestampUnixMS: 1,
		Payload:         payload,
	}
	if err := command.Validate(); err == nil {
		t.Fatal("transition without expected revision was accepted")
	}
	revision := uint64(1)
	command.ExpectedRevision = &revision
	if err := command.Validate(); err != nil {
		t.Fatalf("valid transition was rejected: %v", err)
	}
	payload, _ = json.Marshal(TransitionWorkCommand{
		Type:   CommandTransitionWork,
		Status: "teleporting",
	})
	command.Payload = payload
	if err := command.Validate(); err == nil {
		t.Fatal("unknown work status was accepted")
	}
}

func TestSharedTransitionPlanTaskFixture(t *testing.T) {
	fixture := sharedFixture(t, "transition_plan_task.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeTransitionPlanTask(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Status != "ready" {
		t.Fatalf("task status = %q, want ready", payload.Status)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedProvisionAgentInstanceFixture(t *testing.T) {
	fixture := sharedFixture(t, "provision_agent_instance.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeProvisionAgentInstance(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.ModelRoute != "auto" {
		t.Fatalf("model route = %q, want auto", payload.ModelRoute)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedActivateAgentInstanceFixture(t *testing.T) {
	fixture := sharedFixture(t, "activate_agent_instance.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeActivateAgentInstance(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Charging.Mode != "local_compute" {
		t.Fatalf("charging mode = %q, want local_compute", payload.Charging.Mode)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedStartAgentExecutionFixture(t *testing.T) {
	fixture := sharedFixture(t, "start_agent_execution.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeStartAgentExecution(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.AttemptID == uuid.Nil || payload.TaskID == uuid.Nil {
		t.Fatal("execution start fixture lost stable attempt/task identity")
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedFinishAgentExecutionFixture(t *testing.T) {
	fixture := sharedFixture(t, "finish_agent_execution.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeFinishAgentExecution(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Outcome != "succeeded" {
		t.Fatalf("outcome = %q, want succeeded", payload.Outcome)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedResolveAgentExecutionReviewFixture(t *testing.T) {
	fixture := sharedFixture(t, "resolve_agent_execution_review.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeResolveAgentExecutionReview(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Resolution != "integrated" {
		t.Fatalf("resolution = %q, want integrated", payload.Resolution)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedRequestAgentExecutionRevisionFixture(t *testing.T) {
	fixture := sharedFixture(t, "request_agent_execution_revision.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeResolveAgentExecutionReview(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Resolution != "revision_requested" {
		t.Fatalf("resolution = %q, want revision_requested", payload.Resolution)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedAcceptAgentExecutionResultFixture(t *testing.T) {
	fixture := sharedFixture(t, "accept_agent_execution_result.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeResolveAgentExecutionReview(envelope)
	if err != nil {
		t.Fatalf("decode command payload: %v", err)
	}
	if payload.Resolution != "accepted" {
		t.Fatalf("resolution = %q, want accepted", payload.Resolution)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestSharedCompleteShapingTurnFixture(t *testing.T) {
	fixture := sharedFixture(t, "complete_shaping_turn.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatalf("decode command envelope: %v", err)
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("validate command envelope: %v", err)
	}
	payload, err := DecodeCompleteShapingTurn(envelope)
	if err != nil {
		t.Fatalf("decode complete shaping turn: %v", err)
	}
	if payload.Proposal.Revision != 2 {
		t.Fatalf("proposal revision = %d, want 2", payload.Proposal.Revision)
	}
	if payload.Records.ProposalRevision != 2 || len(payload.Records.Requirements) != 1 || len(payload.Records.Decisions) != 1 || len(payload.Records.AcceptanceCriteria) != 1 {
		t.Fatalf("unexpected normalized shaping records: %+v", payload.Records)
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatalf("encode command envelope: %v", err)
	}
	assertSameJSON(t, fixture, encoded)
}

func TestCompleteShapingTurnRecordsMustCitePrincipalMessage(t *testing.T) {
	fixture := sharedFixture(t, "complete_shaping_turn.command.json")
	var envelope CommandEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatal(err)
	}
	var payload CompleteShapingTurnCommand
	if err := json.Unmarshal(envelope.Payload, &payload); err != nil {
		t.Fatal(err)
	}
	wrongMessageID := uuid.New()
	payload.Records.Requirements[0].Provenance[0].Source.MessageID = &wrongMessageID
	envelope.Payload, _ = json.Marshal(payload)
	if err := envelope.Validate(); err == nil {
		t.Fatal("mismatched shaping record provenance was accepted")
	}
}

func TestStructuredRecordEditsRequireRevisionAndActorProvenance(t *testing.T) {
	fixture := sharedFixture(t, "complete_shaping_turn.command.json")
	var complete CommandEnvelope
	if err := json.Unmarshal(fixture, &complete); err != nil {
		t.Fatal(err)
	}
	completed, err := DecodeCompleteShapingTurn(complete)
	if err != nil {
		t.Fatal(err)
	}
	payload, _ := json.Marshal(UpsertRequirementCommand{
		Type:        CommandUpsertRequirement,
		Requirement: completed.Records.Requirements[0],
	})
	complete.CommandID = uuid.New()
	complete.CorrelationID = uuid.New()
	complete.TimestampUnixMS++
	complete.Payload = payload
	complete.ExpectedRevision = nil
	if err := complete.Validate(); err == nil {
		t.Fatal("structured edit without expected revision was accepted")
	}
	revision := uint64(3)
	complete.ExpectedRevision = &revision
	if err := complete.Validate(); err != nil {
		t.Fatalf("valid structured edit was rejected: %v", err)
	}
	var edit UpsertRequirementCommand
	if err := json.Unmarshal(complete.Payload, &edit); err != nil {
		t.Fatal(err)
	}
	edit.Requirement.Provenance = []Provenance{{
		Source:           ProvenanceSource{Type: "manual_edit"},
		RecordedBy:       complete.ActorID,
		RecordedAtUnixMS: complete.TimestampUnixMS,
	}}
	complete.Payload, _ = json.Marshal(edit)
	if err := complete.Validate(); err != nil {
		t.Fatalf("manual edit provenance from the command actor was rejected: %v", err)
	}
	complete.ActorID = uuid.New()
	if err := complete.Validate(); err == nil {
		t.Fatal("structured edit without actor provenance was accepted")
	}
}

func TestValidationRejectsVersionIdentityAndSequenceErrors(t *testing.T) {
	fixture := sharedFixture(t, "work_brief_updated.event.json")
	var envelope EventEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatal(err)
	}

	envelope.SchemaVersion++
	if err := envelope.Validate(); err == nil {
		t.Fatal("unknown schema version was accepted")
	}
	envelope.SchemaVersion = SchemaVersion
	envelope.EventID = uuid.Nil
	if err := envelope.Validate(); err == nil {
		t.Fatal("nil event id was accepted")
	}
	envelope.EventID = uuid.New()
	envelope.Sequence = 0
	if err := envelope.Validate(); err == nil {
		t.Fatal("zero event sequence was accepted")
	}
}

func TestWorkBriefCannotEscapeEnvelopeScope(t *testing.T) {
	fixture := sharedFixture(t, "work_brief_updated.event.json")
	var envelope EventEnvelope
	if err := json.Unmarshal(fixture, &envelope); err != nil {
		t.Fatal(err)
	}
	var payload WorkBriefUpdatedEvent
	if err := json.Unmarshal(envelope.Payload, &payload); err != nil {
		t.Fatal(err)
	}
	payload.WorkBrief.WorkItemID = uuid.New()
	envelope.Payload, _ = json.Marshal(payload)
	if err := envelope.Validate(); err == nil {
		t.Fatal("mismatched work brief identity was accepted")
	}
}

func TestWorkContractEditRequiresRevisionAndUpdaterActor(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	criterionID := uuid.New()
	contract := validWorkContract(workItemID, actorID, criterionID)
	payload, err := json.Marshal(UpsertWorkContractCommand{
		Type:         CommandUpsertWorkContract,
		WorkContract: contract,
	})
	if err != nil {
		t.Fatal(err)
	}
	command := CommandEnvelope{
		CommandID:       uuid.New(),
		SchemaVersion:   SchemaVersion,
		ProjectID:       uuid.New(),
		WorkItemID:      workItemID,
		ActorID:         actorID,
		CorrelationID:   uuid.New(),
		TimestampUnixMS: 1,
		Payload:         payload,
	}
	if err := command.Validate(); err == nil {
		t.Fatal("work contract edit without expected revision was accepted")
	}
	revision := uint64(2)
	command.ExpectedRevision = &revision
	if err := command.Validate(); err != nil {
		t.Fatalf("valid work contract edit was rejected: %v", err)
	}
	command.ActorID = uuid.New()
	if err := command.Validate(); err == nil {
		t.Fatal("work contract edit from an actor other than updated_by was accepted")
	}
	command.ActorID = actorID
	runtimeID := uuid.New()
	contract.Status = "approved"
	contract.AgentInstanceID = &runtimeID
	command.Payload, _ = json.Marshal(UpsertWorkContractCommand{
		Type:         CommandUpsertWorkContract,
		WorkContract: contract,
	})
	if err := command.Validate(); err == nil {
		t.Fatal("generic contract edit was allowed to bind a runtime agent")
	}
}

func TestWorkContractRejectsPrematureRuntimeIdentityAndUnsafeScope(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	contract := validWorkContract(workItemID, actorID, uuid.New())
	runtimeID := uuid.New()
	contract.AgentInstanceID = &runtimeID
	if err := validateWorkContract(contract, workItemID); err == nil {
		t.Fatal("proposed work contract with runtime identity was accepted")
	}
	contract.AgentInstanceID = nil
	contract.WriteScope = []string{"../another-project"}
	if err := validateWorkContract(contract, workItemID); err == nil {
		t.Fatal("work contract with unsafe write scope was accepted")
	}
}

func TestWorkContractValidatesOptionalPreferredModelRoute(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	contract := validWorkContract(workItemID, actorID, uuid.New())
	route := "anthropic/claude-review"
	contract.PreferredModelRoute = &route
	if err := validateWorkContract(contract, workItemID); err != nil {
		t.Fatalf("explicit preferred model route was rejected: %v", err)
	}
	for _, invalid := range []string{"", "auto", "not a model"} {
		contract.PreferredModelRoute = &invalid
		if err := validateWorkContract(contract, workItemID); err == nil {
			t.Fatalf("invalid preferred model route %q was accepted", invalid)
		}
	}
}

func TestArtifactUpsertValidatesRevisionScopeAndPath(t *testing.T) {
	workItemID := uuid.New()
	revision := uint64(1)
	payload, err := json.Marshal(UpsertArtifactCommand{
		Type: CommandUpsertArtifact,
		Artifact: ArtifactRef{
			ID:           uuid.New(),
			WorkItemID:   workItemID,
			RelativePath: "docs/requirements.md",
			Kind:         "requirements",
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	envelope := CommandEnvelope{
		CommandID:        uuid.New(),
		SchemaVersion:    SchemaVersion,
		ProjectID:        uuid.New(),
		WorkItemID:       workItemID,
		ActorID:          uuid.New(),
		CorrelationID:    uuid.New(),
		TimestampUnixMS:  1,
		ExpectedRevision: &revision,
		Payload:          payload,
	}
	if err := envelope.Validate(); err != nil {
		t.Fatalf("valid artifact upsert rejected: %v", err)
	}
	envelope.ExpectedRevision = nil
	if err := envelope.Validate(); err == nil {
		t.Fatal("artifact upsert without expected revision was accepted")
	}
	envelope.ExpectedRevision = &revision
	var command UpsertArtifactCommand
	if err := json.Unmarshal(payload, &command); err != nil {
		t.Fatal(err)
	}
	command.Artifact.RelativePath = "../outside.md"
	envelope.Payload, _ = json.Marshal(command)
	if err := envelope.Validate(); err == nil {
		t.Fatal("artifact with unsafe path was accepted")
	}
}

func TestPlanRevisionRequiresAtomicContractLinksAndClearsApproval(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	planID := uuid.New()
	taskID := uuid.New()
	criterionID := uuid.New()
	contract := validWorkContract(workItemID, actorID, criterionID)
	contract.Version = 2
	contract.TaskIDs = []uuid.UUID{taskID}
	contract.Status = "proposed"
	plan := Plan{
		ID:              planID,
		WorkItemID:      workItemID,
		Version:         2,
		Status:          "draft",
		CreatedAtUnixMS: 1,
		UpdatedAtUnixMS: 2,
		Tasks: []PlanTask{{
			ID:                 taskID,
			PlanID:             planID,
			Title:              "Implement revision",
			Description:        "Keep contract links consistent",
			Status:             "proposed",
			AcceptanceCriteria: []uuid.UUID{criterionID},
			AssignedContractID: &contract.ID,
		}},
	}
	payload := RevisePlanCommand{
		Type:          CommandRevisePlan,
		Plan:          plan,
		WorkContracts: []WorkContract{contract},
	}
	raw, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	revision := uint64(4)
	command := CommandEnvelope{
		CommandID:        uuid.New(),
		SchemaVersion:    SchemaVersion,
		ProjectID:        uuid.New(),
		WorkItemID:       workItemID,
		ActorID:          actorID,
		CorrelationID:    uuid.New(),
		TimestampUnixMS:  2,
		ExpectedRevision: &revision,
		Payload:          raw,
	}
	if err := command.Validate(); err != nil {
		t.Fatalf("valid atomic plan revision was rejected: %v", err)
	}

	payload.WorkContracts[0].TaskIDs = nil
	command.Payload, _ = json.Marshal(payload)
	if err := command.Validate(); err == nil {
		t.Fatal("plan revision with stale contract task links was accepted")
	}
	payload.WorkContracts[0].TaskIDs = []uuid.UUID{taskID}
	payload.Plan.Status = "approved"
	payload.Plan.ApprovedBy = &actorID
	approvedAt := int64(2)
	payload.Plan.ApprovedAtUnixMS = &approvedAt
	command.Payload, _ = json.Marshal(payload)
	if err := command.Validate(); err == nil {
		t.Fatal("plan revision retained approval")
	}
}

func TestWorkContractUpdatedEventUsesSameDomainValidation(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	contract := validWorkContract(workItemID, actorID, uuid.New())
	payload, err := json.Marshal(WorkContractUpdatedEvent{
		Type:         EventWorkContractUpdated,
		WorkContract: contract,
	})
	if err != nil {
		t.Fatal(err)
	}
	event := EventEnvelope{
		EventID:         uuid.New(),
		SchemaVersion:   SchemaVersion,
		Sequence:        1,
		ProjectID:       uuid.New(),
		WorkItemID:      workItemID,
		ActorID:         actorID,
		CorrelationID:   uuid.New(),
		TimestampUnixMS: 1,
		Payload:         payload,
	}
	if err := event.Validate(); err != nil {
		t.Fatalf("valid work contract event was rejected: %v", err)
	}
	contract.SuccessCriteria = nil
	event.Payload, _ = json.Marshal(WorkContractUpdatedEvent{
		Type:         EventWorkContractUpdated,
		WorkContract: contract,
	})
	if err := event.Validate(); err == nil {
		t.Fatal("work contract event without success criteria was accepted")
	}
}

func TestAgentProvisioningEventAtomicallyBindsPendingInstance(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	agentInstanceID := uuid.New()
	contract := validWorkContract(workItemID, actorID, uuid.New())
	contract.Status = "approved"
	contract.AgentInstanceID = &agentInstanceID
	agent := AgentInstance{
		ID:                agentInstanceID,
		WorkItemID:        workItemID,
		RoleDefinitionID:  contract.RoleDefinitionID,
		ContractID:        contract.ID,
		Status:            "pending",
		ExecutionLocation: contract.ExecutionLocation,
		ModelRoute:        "auto",
		UpdatedAtUnixMS:   2,
	}
	payload, err := json.Marshal(AgentInstanceProvisionedEvent{
		Type:          EventAgentInstanceProvisioned,
		AgentInstance: agent,
		WorkContract:  contract,
	})
	if err != nil {
		t.Fatal(err)
	}
	event := EventEnvelope{
		EventID:         uuid.New(),
		SchemaVersion:   SchemaVersion,
		Sequence:        1,
		ProjectID:       uuid.New(),
		WorkItemID:      workItemID,
		ActorID:         actorID,
		CorrelationID:   uuid.New(),
		TimestampUnixMS: 2,
		Payload:         payload,
	}
	if err := event.Validate(); err != nil {
		t.Fatalf("valid provisioning event was rejected: %v", err)
	}

	contract.AgentInstanceID = nil
	event.Payload, _ = json.Marshal(AgentInstanceProvisionedEvent{
		Type:          EventAgentInstanceProvisioned,
		AgentInstance: agent,
		WorkContract:  contract,
	})
	if err := event.Validate(); err == nil {
		t.Fatal("provisioning event without an atomic contract binding was accepted")
	}
}

func TestAgentActivationEventBindsLeaseChargingAndActiveContract(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	agentInstanceID := uuid.New()
	leaseID := uuid.New()
	contract := validWorkContract(workItemID, actorID, uuid.New())
	contract.Status = "active"
	contract.AgentInstanceID = &agentInstanceID
	agent := AgentInstance{
		ID:                agentInstanceID,
		WorkItemID:        workItemID,
		RoleDefinitionID:  contract.RoleDefinitionID,
		ContractID:        contract.ID,
		Status:            "ready",
		ExecutionLocation: contract.ExecutionLocation,
		ModelRoute:        "local/default-agent",
		ActiveLeaseID:     &leaseID,
		UpdatedAtUnixMS:   2,
	}
	lease := ExecutionLease{
		ID:               leaseID,
		WorkItemID:       workItemID,
		AgentInstanceID:  agentInstanceID,
		ContractID:       contract.ID,
		Status:           "active",
		Charging:         ChargingAuthorization{OwnerID: actorID, Mode: "local_compute"},
		AcquiredAtUnixMS: 2,
		ExpiresAtUnixMS:  60_002,
		UpdatedAtUnixMS:  2,
	}
	payload, err := json.Marshal(AgentInstanceActivatedEvent{
		Type:          EventAgentInstanceActivated,
		AgentInstance: agent,
		WorkContract:  contract,
		Lease:         lease,
	})
	if err != nil {
		t.Fatal(err)
	}
	event := EventEnvelope{
		EventID:         uuid.New(),
		SchemaVersion:   SchemaVersion,
		Sequence:        1,
		ProjectID:       uuid.New(),
		WorkItemID:      workItemID,
		ActorID:         actorID,
		CorrelationID:   uuid.New(),
		TimestampUnixMS: 2,
		Payload:         payload,
	}
	if err := event.Validate(); err != nil {
		t.Fatalf("valid activation event was rejected: %v", err)
	}

	lease.ContractID = uuid.New()
	event.Payload, _ = json.Marshal(AgentInstanceActivatedEvent{
		Type:          EventAgentInstanceActivated,
		AgentInstance: agent,
		WorkContract:  contract,
		Lease:         lease,
	})
	if err := event.Validate(); err == nil {
		t.Fatal("activation event with a mismatched lease was accepted")
	}
}

func TestAgentExecutionStartedEventIsOneCoherentRuntimeFact(t *testing.T) {
	actorID, workItemID, taskID := uuid.New(), uuid.New(), uuid.New()
	agentInstanceID, leaseID, attemptID, contractID := uuid.New(), uuid.New(), uuid.New(), uuid.New()
	startedAt := int64(2)
	agent := AgentInstance{
		ID: agentInstanceID, WorkItemID: workItemID, RoleDefinitionID: uuid.New(),
		ContractID: contractID, Status: "running", ExecutionLocation: "local",
		ModelRoute: "local/default-agent", ActiveLeaseID: &leaseID,
		StartedAtUnixMS: &startedAt, UpdatedAtUnixMS: startedAt,
	}
	lease := ExecutionLease{
		ID: leaseID, WorkItemID: workItemID, AgentInstanceID: agentInstanceID,
		ContractID: contractID, Status: "active",
		Charging:         ChargingAuthorization{OwnerID: actorID, Mode: "local_compute"},
		AcquiredAtUnixMS: 1, ExpiresAtUnixMS: 60_001, UpdatedAtUnixMS: 1,
	}
	attempt := ExecutionAttempt{
		ID: attemptID, WorkItemID: workItemID, TaskID: taskID,
		AgentInstanceID: agentInstanceID, ContractID: contractID, LeaseID: leaseID,
		AttemptNumber: 1, Status: "running", ModelRoute: agent.ModelRoute,
		StartedAtUnixMS: startedAt, UpdatedAtUnixMS: startedAt,
	}
	plan := validPlan(workItemID, actorID, taskID, "running")
	plan.Tasks[0].AssignedContractID = &contractID
	payload, err := json.Marshal(AgentExecutionStartedEvent{
		Type: EventAgentExecutionStarted, AgentInstance: agent,
		Lease: lease, Attempt: attempt, Plan: plan,
	})
	if err != nil {
		t.Fatal(err)
	}
	event := EventEnvelope{
		EventID: uuid.New(), SchemaVersion: SchemaVersion, Sequence: 1,
		ProjectID: uuid.New(), WorkItemID: workItemID, ActorID: actorID,
		TaskID: &taskID, CorrelationID: uuid.New(), TimestampUnixMS: startedAt,
		Payload: payload,
	}
	if err := event.Validate(); err != nil {
		t.Fatalf("valid execution-start event was rejected: %v", err)
	}

	attempt.LeaseID = uuid.New()
	event.Payload, _ = json.Marshal(AgentExecutionStartedEvent{
		Type: EventAgentExecutionStarted, AgentInstance: agent,
		Lease: lease, Attempt: attempt, Plan: plan,
	})
	if err := event.Validate(); err == nil {
		t.Fatal("execution start with a mismatched attempt lease was accepted")
	}
}

func TestSuccessfulExecutionFinishMovesIntoReviewWithoutCompletingWork(t *testing.T) {
	actorID, workItemID, taskID := uuid.New(), uuid.New(), uuid.New()
	agentInstanceID, leaseID, attemptID, contractID := uuid.New(), uuid.New(), uuid.New(), uuid.New()
	startedAt, finishedAt := int64(2), int64(3)
	agent := AgentInstance{
		ID: agentInstanceID, WorkItemID: workItemID, RoleDefinitionID: uuid.New(),
		ContractID: contractID, Status: "reviewing", ExecutionLocation: "local",
		ModelRoute: "local/default-agent", ActiveLeaseID: &leaseID,
		StartedAtUnixMS: &startedAt, UpdatedAtUnixMS: finishedAt,
	}
	lease := ExecutionLease{
		ID: leaseID, WorkItemID: workItemID, AgentInstanceID: agentInstanceID,
		ContractID: contractID, Status: "active",
		Charging:         ChargingAuthorization{OwnerID: actorID, Mode: "local_compute"},
		AcquiredAtUnixMS: 1, ExpiresAtUnixMS: 60_001, UpdatedAtUnixMS: finishedAt,
	}
	attempt := ExecutionAttempt{
		ID: attemptID, WorkItemID: workItemID, TaskID: taskID,
		AgentInstanceID: agentInstanceID, ContractID: contractID, LeaseID: leaseID,
		AttemptNumber: 1, Status: "succeeded", ModelRoute: agent.ModelRoute,
		StartedAtUnixMS: startedAt, FinishedAtUnixMS: &finishedAt, UpdatedAtUnixMS: finishedAt,
	}
	plan := validPlan(workItemID, actorID, taskID, "reviewing")
	plan.Tasks[0].AssignedContractID = &contractID
	payload, err := json.Marshal(AgentExecutionFinishedEvent{
		Type: EventAgentExecutionFinished, AgentInstance: agent, Lease: lease,
		Attempt: attempt, Plan: plan, Summary: "Ready for independent review.",
	})
	if err != nil {
		t.Fatal(err)
	}
	event := EventEnvelope{
		EventID: uuid.New(), SchemaVersion: SchemaVersion, Sequence: 1,
		ProjectID: uuid.New(), WorkItemID: workItemID, ActorID: actorID,
		TaskID: &taskID, CorrelationID: uuid.New(), TimestampUnixMS: finishedAt,
		Payload: payload,
	}
	if err := event.Validate(); err != nil {
		t.Fatalf("valid execution-finish event was rejected: %v", err)
	}
}

func TestPlanTaskTransitionRequiresRevisionAndTaskScopedEvent(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	taskID := uuid.New()
	revision := uint64(8)
	payload, err := json.Marshal(TransitionPlanTaskCommand{
		Type:   CommandTransitionPlanTask,
		TaskID: taskID,
		Status: "ready",
	})
	if err != nil {
		t.Fatal(err)
	}
	command := CommandEnvelope{
		CommandID:       uuid.New(),
		SchemaVersion:   SchemaVersion,
		ProjectID:       uuid.New(),
		WorkItemID:      workItemID,
		ActorID:         actorID,
		CorrelationID:   uuid.New(),
		TimestampUnixMS: 1,
		Payload:         payload,
	}
	if err := command.Validate(); err == nil {
		t.Fatal("task transition without expected revision was accepted")
	}
	command.ExpectedRevision = &revision
	if err := command.Validate(); err != nil {
		t.Fatalf("valid task transition was rejected: %v", err)
	}

	plan := validPlan(workItemID, actorID, taskID, "ready")
	eventPayload, err := json.Marshal(PlanTaskStatusChangedEvent{
		Type:     EventPlanTaskStatusChanged,
		Plan:     plan,
		TaskID:   taskID,
		Previous: "proposed",
		Current:  "ready",
	})
	if err != nil {
		t.Fatal(err)
	}
	event := EventEnvelope{
		EventID:         uuid.New(),
		SchemaVersion:   SchemaVersion,
		Sequence:        1,
		ProjectID:       command.ProjectID,
		WorkItemID:      workItemID,
		ActorID:         actorID,
		TaskID:          &taskID,
		CorrelationID:   command.CorrelationID,
		TimestampUnixMS: 2,
		Payload:         eventPayload,
	}
	if err := event.Validate(); err != nil {
		t.Fatalf("valid task transition event was rejected: %v", err)
	}
	wrongTaskID := uuid.New()
	event.TaskID = &wrongTaskID
	if err := event.Validate(); err == nil {
		t.Fatal("task transition event with mismatched envelope identity was accepted")
	}
}

func TestPlanValidationRejectsDependencyCycles(t *testing.T) {
	actorID := uuid.New()
	workItemID := uuid.New()
	firstID := uuid.New()
	secondID := uuid.New()
	plan := validPlan(workItemID, actorID, firstID, "proposed")
	plan.Tasks[0].DependsOn = []uuid.UUID{secondID}
	plan.Tasks = append(plan.Tasks, PlanTask{
		ID:        secondID,
		PlanID:    plan.ID,
		Title:     "Second task",
		Status:    "proposed",
		DependsOn: []uuid.UUID{firstID},
	})
	if err := validatePlan(plan, workItemID); err == nil {
		t.Fatal("cyclic plan was accepted")
	}
}

func validPlan(workItemID, actorID, taskID uuid.UUID, taskStatus string) Plan {
	planID := uuid.New()
	approvedAt := int64(1)
	return Plan{
		ID:               planID,
		WorkItemID:       workItemID,
		Version:          1,
		Status:           "approved",
		ApprovedBy:       &actorID,
		ApprovedAtUnixMS: &approvedAt,
		CreatedAtUnixMS:  1,
		UpdatedAtUnixMS:  1,
		Tasks: []PlanTask{{
			ID:     taskID,
			PlanID: planID,
			Title:  "Implement canonical transition",
			Status: taskStatus,
		}},
	}
}

func validWorkContract(workItemID, actorID, criterionID uuid.UUID) WorkContract {
	return WorkContract{
		ID:                uuid.New(),
		WorkItemID:        workItemID,
		RoleDefinitionID:  uuid.New(),
		Version:           1,
		Status:            "proposed",
		Mission:           "Review the implementation and report evidence.",
		SuccessCriteria:   []uuid.UUID{criterionID},
		WriteScope:        []string{"dragent/app/src"},
		Capabilities:      []CapabilityGrant{{Capability: "code_review"}},
		Tools:             []string{"read_file"},
		Permissions:       []PermissionGrant{{Permission: "project_read"}},
		Budget:            WorkBudget{},
		ExecutionLocation: "local",
		CreatedBy:         actorID,
		UpdatedBy:         actorID,
		CreatedAtUnixMS:   1,
		UpdatedAtUnixMS:   1,
	}
}

// Documented single-source gaps: payload types the Rust canonical schemas
// (contracts/workspace/v1/schema, derived from dragent_session_protocol via
// schemars) define but the Go cloud mirror does not yet decode. Each entry
// needs Go decoder + validation + authority handling before cloud clients can
// send it. Anything NOT listed here fails the parity test below in either
// direction, so new drift is loud instead of silent.
var schemaKnownGaps = map[string]string{
	"record_shaping_memory":   "local Principal memory; no cloud decoder yet",
	"shaping_memory_recorded": "local Principal memory event; no cloud projection yet",
}

func schemaPayloadConsts(t *testing.T, schemaFile, defName string) map[string]struct{} {
	t.Helper()
	_, filename, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve test source path")
	}
	path := filepath.Join(filepath.Dir(filename), "..", "..", "..", "..", "contracts", "workspace", "v1", "schema", schemaFile)
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read canonical schema %s: %v", path, err)
	}
	var schema struct {
		Defs map[string]struct {
			OneOf []struct {
				Properties struct {
					Type struct {
						Const string `json:"const"`
					} `json:"type"`
				} `json:"properties"`
			} `json:"oneOf"`
		} `json:"$defs"`
	}
	if err := json.Unmarshal(data, &schema); err != nil {
		t.Fatalf("decode canonical schema: %v", err)
	}
	def, ok := schema.Defs[defName]
	if !ok {
		t.Fatalf("schema %s lacks definition %s", schemaFile, defName)
	}
	out := make(map[string]struct{}, len(def.OneOf))
	for _, branch := range def.OneOf {
		if branch.Properties.Type.Const == "" {
			t.Fatalf("schema %s %s has a variant without a type const", schemaFile, defName)
		}
		out[branch.Properties.Type.Const] = struct{}{}
	}
	t.Logf("schema %s %s: %d payload consts", schemaFile, defName, len(out))
	return out
}

func assertPayloadParity(t *testing.T, kind string, schemaTypes, knownTypes map[string]struct{}) {
	t.Helper()
	for payloadType := range schemaTypes {
		if _, ok := knownTypes[payloadType]; ok {
			continue
		}
		if _, ok := schemaKnownGaps[payloadType]; ok {
			continue
		}
		t.Errorf("canonical schema defines %s payload %q with no Go support (and no documented gap)", kind, payloadType)
	}
	for payloadType := range knownTypes {
		if _, ok := schemaTypes[payloadType]; ok {
			continue
		}
		t.Errorf("Go supports %s payload %q missing from the canonical schema: regenerate schemas from Rust types", kind, payloadType)
	}
	for gap := range schemaKnownGaps {
		_, inSchema := schemaTypes[gap]
		_, inKnown := knownTypes[gap]
		switch {
		case inSchema && !inKnown:
			// Documented gap for this payload kind: the canonical schema
			// defines it, Go does not decode it yet. Judged intentional.
		case !inSchema && !inKnown:
			// Belongs to the other payload kind; judged by that call.
		case inSchema && inKnown:
			t.Errorf("documented gap %q is now supported on both sides; remove it from schemaKnownGaps", gap)
		}
	}
}

func TestCheckedInSchemasMatchSupportedPayloadTypes(t *testing.T) {
	commands := schemaPayloadConsts(t, "workspace-command-envelope.v1.schema.json", "WorkspaceCommandPayload")
	assertPayloadParity(t, "command", commands, knownCommandTypes)
	events := schemaPayloadConsts(t, "workspace-event-envelope.v1.schema.json", "WorkspaceEventPayload")
	assertPayloadParity(t, "event", events, knownEventTypes)
}
