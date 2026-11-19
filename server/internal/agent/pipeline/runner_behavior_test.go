package pipeline

import (
	"context"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"server/internal/agent/permissions"
	"server/internal/agent/providers"
)

type scriptedProvider struct {
	mu       sync.Mutex
	results  []*providers.StepResult
	requests []providers.StepRequest
}

func (p *scriptedProvider) StreamStep(_ context.Context, request providers.StepRequest) (<-chan providers.StreamEvent, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.requests = append(p.requests, request)
	if len(p.results) == 0 {
		return nil, errors.New("script exhausted")
	}
	result := p.results[0]
	p.results = p.results[1:]
	stream := make(chan providers.StreamEvent, 1)
	stream <- providers.StreamEvent{Kind: providers.EventDone, Result: result}
	close(stream)
	return stream, nil
}

func (*scriptedProvider) SupportedTools() []providers.ToolDefinition { return nil }
func (*scriptedProvider) ProviderName() string                       { return "scripted" }
func (*scriptedProvider) Capabilities() providers.ProviderCapabilities {
	return providers.ProviderCapabilities{Streaming: true, ToolCalling: true}
}

func TestAllowedToolsRestrictsProviderVisibleCatalog(t *testing.T) {
	runner := NewRunner(
		nil,
		"test-model",
		permissions.PermissivePolicy(),
		AgentModeNormal,
		func(AgentEvent) {},
		[]providers.ToolDefinition{
			{Name: "mcp__issue_read"},
			{Name: "mcp__issue_write"},
		},
	)
	runner.SetAllowedTools([]string{"read_file", "web_search", "mcp__issue_read"})

	tools := runner.buildToolDefs()
	names := make(map[string]bool, len(tools))
	for _, tool := range tools {
		names[tool.Name] = true
	}
	for _, expected := range []string{"read_file", "web_search", "mcp__issue_read"} {
		if !names[expected] {
			t.Fatalf("allowed tool %q was not advertised: %v", expected, names)
		}
	}
	for _, denied := range []string{"edit_file", "fetch_url", "mcp__issue_write"} {
		if names[denied] {
			t.Fatalf("denied tool %q was advertised: %v", denied, names)
		}
	}
}

func TestExplicitEmptyAllowedToolsAdvertisesNoTools(t *testing.T) {
	runner := NewRunner(
		nil,
		"test-model",
		permissions.PermissivePolicy(),
		AgentModeNormal,
		func(AgentEvent) {},
		[]providers.ToolDefinition{{Name: "mcp__issue_read"}},
	)
	runner.SetAllowedTools(nil)

	if tools := runner.buildToolDefs(); len(tools) != 0 {
		t.Fatalf("explicit empty allowlist advertised %d tools: %+v", len(tools), tools)
	}
}

func TestLocalToolResultDoesNotTriggerWebBillingAbort(t *testing.T) {
	runner := NewRunner(nil, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(AgentEvent) {}, nil)
	aborted := 0
	runner.onWebAborted = func(string, string) { aborted++ }
	call := providers.ToolCall{ID: "read-1", Name: "read_file", ArgsJSON: `{"path":"/workspace/app.ts"}`}
	done := make(chan bool, 1)
	go func() {
		_, wasAborted := runner.delegateTools(context.Background(), []providers.ToolCall{call}, nil, 1)
		done <- wasAborted
	}()

	deadline := time.Now().Add(time.Second)
	for !runner.Gate().Resolve(call.ID, "file contents", false) {
		if time.Now().After(deadline) {
			t.Fatal("client result gate was not registered")
		}
		time.Sleep(time.Millisecond)
	}
	if <-done {
		t.Fatal("local tool delegation aborted")
	}
	if aborted != 0 {
		t.Fatalf("local tool triggered %d web billing abort callbacks", aborted)
	}
}

func TestDesktopOwnedRunnerReturnsAfterOneProviderStep(t *testing.T) {
	call := providers.ToolCall{ID: "read-1", Name: "read_file", ArgsJSON: `{"path":"/workspace/app.ts"}`}
	provider := &scriptedProvider{results: []*providers.StepResult{
		{Content: "I will inspect it.", ToolCalls: []providers.ToolCall{call}, InputTokens: 11, OutputTokens: 3},
		{Content: "this second step must remain client-owned"},
	}}
	var events []AgentEvent
	runner := NewRunner(provider, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(event AgentEvent) {
		events = append(events, event)
	}, nil)
	runner.SetDesktopOwned(true)
	runner.Run(context.Background(), "system", []providers.Message{{Role: providers.RoleUser, Content: "inspect it"}})

	if len(provider.requests) != 1 {
		t.Fatalf("desktop-owned runner made %d provider calls, want exactly 1", len(provider.requests))
	}
	var sawCall, sawStepDone bool
	for _, event := range events {
		sawCall = sawCall || event.Kind == KindToolCall && event.ToolID == call.ID
		if event.Kind == KindStepDone {
			sawStepDone = true
			if event.AssistantContent != "I will inspect it." || event.TotalInputTokens != 11 || event.TotalOutputTokens != 3 {
				t.Fatalf("step terminal lost provider state: %+v", event)
			}
		}
	}
	if !sawCall || !sawStepDone {
		t.Fatalf("desktop step events incomplete: %+v", events)
	}
}

func TestRunCommandRequiresVisibleReason(t *testing.T) {
	for _, tool := range clientToolDefinitions() {
		if tool.Name != "run_command" {
			continue
		}
		required, ok := tool.Parameters["required"].([]string)
		if !ok {
			t.Fatalf("run_command required schema has type %T", tool.Parameters["required"])
		}
		for _, field := range required {
			if field == "reason" {
				return
			}
		}
		t.Fatal("run_command does not require reason")
	}
	t.Fatal("run_command tool definition not found")
}

func TestManageTaskDoesNotExposeStatusPolling(t *testing.T) {
	for _, tool := range clientToolDefinitions() {
		if tool.Name != "manage_task" {
			continue
		}
		properties, ok := tool.Parameters["properties"].(map[string]interface{})
		if !ok {
			t.Fatalf("manage_task properties schema has type %T", tool.Parameters["properties"])
		}
		action, ok := properties["action"].(map[string]interface{})
		if !ok {
			t.Fatalf("manage_task action schema has type %T", properties["action"])
		}
		values, ok := action["enum"].([]interface{})
		if !ok {
			t.Fatalf("manage_task action enum has type %T", action["enum"])
		}

		seen := map[string]bool{}
		for _, value := range values {
			name, ok := value.(string)
			if !ok {
				t.Fatalf("manage_task action has non-string value %T", value)
			}
			seen[name] = true
		}
		if seen["status"] {
			t.Fatal("manage_task still advertises status polling")
		}
		if !seen["send_input"] || !seen["kill"] {
			t.Fatalf("manage_task controls incomplete: %v", seen)
		}
		return
	}
	t.Fatal("manage_task tool definition not found")
}

func TestRunnerCheckpointsBeforeAndAfterToolExecution(t *testing.T) {
	call := providers.ToolCall{ID: "read-1", Name: "read_file", ArgsJSON: `{"path":"/workspace/app.ts"}`}
	provider := &scriptedProvider{results: []*providers.StepResult{
		{Content: "I will inspect it.", ToolCalls: []providers.ToolCall{call}, InputTokens: 10, OutputTokens: 3},
		{Content: "Implemented.", InputTokens: 15, OutputTokens: 2},
	}}
	var checkpoints []RunCheckpoint
	var checkpointMu sync.Mutex
	done := make(chan struct{})
	var runner *Runner
	// Resolve only after KindToolCall is emitted. The preceding awaiting_tools
	// checkpoint must therefore already be durable.
	runner = NewRunner(provider, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(ev AgentEvent) {
		if ev.Kind == KindToolCall {
			go func() {
				deadline := time.Now().Add(time.Second)
				for !runner.Gate().Resolve(call.ID, "export const value = 1", false) {
					if time.Now().After(deadline) {
						return
					}
					time.Sleep(time.Millisecond)
				}
			}()
		}
		if ev.Kind == KindDone || ev.Kind == KindError {
			select {
			case <-done:
			default:
				close(done)
			}
		}
	}, nil)
	runner.SetCheckpointHook(func(cp RunCheckpoint) error {
		checkpointMu.Lock()
		defer checkpointMu.Unlock()
		checkpoints = append(checkpoints, cp)
		return nil
	})
	go runner.Run(context.Background(), "system", []providers.Message{{Role: providers.RoleUser, Content: "fix it"}})

	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("runner did not finish")
	}
	// KindDone is emitted before the completed checkpoint projection by design:
	// the terminal stream event must be durable first. Wait for the runner's
	// final projection instead of racing it immediately after observing Done.
	deadline := time.Now().Add(time.Second)
	for {
		checkpointMu.Lock()
		projected := len(checkpoints) > 0 && checkpoints[len(checkpoints)-1].Phase == RunPhaseCompleted
		checkpointMu.Unlock()
		if projected {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("completed checkpoint was not projected")
		}
		time.Sleep(time.Millisecond)
	}
	checkpointMu.Lock()
	defer checkpointMu.Unlock()
	awaiting := -1
	readyAfterTool := -1
	for i, cp := range checkpoints {
		if cp.Phase == RunPhaseAwaitingTools {
			awaiting = i
			if len(cp.PendingToolCalls) != 1 || cp.PendingToolCalls[0].ID != call.ID {
				t.Fatalf("awaiting checkpoint pending calls = %+v", cp.PendingToolCalls)
			}
		}
		if awaiting >= 0 && i > awaiting && cp.Phase == RunPhaseReadyModel && cp.Round == 1 {
			readyAfterTool = i
			if got := cp.Messages[len(cp.Messages)-1]; got.Role != providers.RoleTool || got.Content != "export const value = 1" {
				t.Fatalf("tool result not persisted in checkpoint: %+v", got)
			}
		}
	}
	if awaiting < 0 || readyAfterTool <= awaiting {
		t.Fatalf("checkpoint phases do not bracket tool execution: %+v", checkpoints)
	}
	last := checkpoints[len(checkpoints)-1]
	if last.Phase != RunPhaseCompleted || last.Messages[len(last.Messages)-1].Content != "Implemented." {
		t.Fatalf("final checkpoint incomplete: %+v", last)
	}
}

func TestCheckpointFailureStopsBeforeToolSideEffect(t *testing.T) {
	call := providers.ToolCall{ID: "write-1", Name: "write_file", ArgsJSON: `{"path":"/workspace/app.ts"}`}
	provider := &scriptedProvider{results: []*providers.StepResult{{ToolCalls: []providers.ToolCall{call}}}}
	toolEmitted := false
	done := make(chan struct{})
	runner := NewRunner(provider, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(ev AgentEvent) {
		if ev.Kind == KindToolCall {
			toolEmitted = true
		}
		if ev.Kind == KindError {
			close(done)
		}
	}, nil)
	runner.SetCheckpointHook(func(cp RunCheckpoint) error {
		if cp.Phase == RunPhaseAwaitingTools {
			return errors.New("database unavailable")
		}
		return nil
	})
	go runner.Run(context.Background(), "system", []providers.Message{{Role: providers.RoleUser, Content: "edit it"}})
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("runner did not report checkpoint failure")
	}
	if toolEmitted {
		t.Fatal("tool was emitted after its safety checkpoint failed")
	}
}

func TestExactDuplicateReadIsReusedWithoutAnotherClientExecution(t *testing.T) {
	first := providers.ToolCall{ID: "read-1", Name: "read_file", ArgsJSON: `{"path":"/workspace/app.ts"}`}
	second := providers.ToolCall{ID: "read-2", Name: "read_file", ArgsJSON: first.ArgsJSON}
	provider := &scriptedProvider{results: []*providers.StepResult{
		{ToolCalls: []providers.ToolCall{first}},
		{ToolCalls: []providers.ToolCall{second}},
		{Content: "done"},
	}}
	done := make(chan struct{})
	clientExecutions := 0
	var trajectory []TrajectoryEvent
	var runner *Runner
	runner = NewRunner(provider, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(ev AgentEvent) {
		if ev.Kind == KindToolCall && !ev.ServerExecuted {
			clientExecutions++
			callID := ev.ToolID
			go func() {
				deadline := time.Now().Add(time.Second)
				for !runner.Gate().Resolve(callID, "same file contents", false) {
					if time.Now().After(deadline) {
						return
					}
					time.Sleep(time.Millisecond)
				}
			}()
		}
		if ev.Kind == KindDone || ev.Kind == KindError {
			close(done)
		}
	}, nil)
	runner.SetTrajectoryObserver(func(event TrajectoryEvent) { trajectory = append(trajectory, event) })
	// The runner keeps tracing (run.completed) after it emits KindDone, so the
	// trajectory is only complete, and safe to read, once Run has returned.
	runReturned := make(chan struct{})
	go func() {
		defer close(runReturned)
		runner.Run(context.Background(), "system", []providers.Message{{Role: providers.RoleUser, Content: "inspect it"}})
	}()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("runner did not finish")
	}
	select {
	case <-runReturned:
	case <-time.After(2 * time.Second):
		t.Fatal("runner did not return after KindDone")
	}
	if clientExecutions != 1 {
		t.Fatalf("client executed identical read %d times, want 1", clientExecutions)
	}
	grade := GradeTrajectory(trajectory)
	if grade.ExecutedTools != 1 || grade.ReusedReads != 1 || !grade.Completed {
		t.Fatalf("trajectory grade=%+v events=%+v", grade, trajectory)
	}
	provider.mu.Lock()
	defer provider.mu.Unlock()
	if len(provider.requests) != 3 {
		t.Fatalf("provider requests=%d, want 3", len(provider.requests))
	}
	lastHistory := provider.requests[2].Messages
	if got := lastHistory[len(lastHistory)-1].Content; !strings.Contains(got, "prior result was reused") {
		t.Fatalf("duplicate-read controller signal missing: %q", got)
	}
}

func TestResumeRetriesInterruptedProviderAsSameLogicalRound(t *testing.T) {
	provider := &scriptedProvider{results: []*providers.StepResult{{
		Content: "recovered", InputTokens: 7, OutputTokens: 2,
	}}}
	done := make(chan AgentEvent, 1)
	runner := NewRunner(provider, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(event AgentEvent) {
		if event.Kind == KindDone || event.Kind == KindError {
			done <- event
		}
	}, nil)
	runner.SetCheckpointHook(func(RunCheckpoint) error { return nil })
	runner.Resume(context.Background(), RunCheckpoint{
		SchemaVersion:     RunCheckpointSchemaVersion,
		Phase:             RunPhaseCallingModel,
		Round:             3,
		System:            "system",
		Messages:          []providers.Message{{Role: providers.RoleUser, Content: "continue"}},
		TotalInputTokens:  20,
		TotalOutputTokens: 5,
	})
	event := <-done
	if event.Kind != KindDone {
		t.Fatalf("resume ended with %s: %s", event.Kind, event.Error)
	}
	if event.Rounds != 3 {
		t.Fatalf("resumed round=%d, want same logical round 3", event.Rounds)
	}
	if event.TotalInputTokens != 27 || event.TotalOutputTokens != 7 {
		t.Fatalf("usage totals were reset: %+v", event)
	}
}

func TestFinalizingResumeDoesNotCallProviderAgain(t *testing.T) {
	provider := &scriptedProvider{}
	done := make(chan AgentEvent, 1)
	var projected RunCheckpoint
	runner := NewRunner(provider, "test-model", permissions.PermissivePolicy(), AgentModeNormal, func(event AgentEvent) {
		if event.Kind == KindDone || event.Kind == KindError {
			done <- event
		}
	}, nil)
	runner.SetCheckpointHook(func(checkpoint RunCheckpoint) error {
		projected = checkpoint
		return nil
	})
	runner.Resume(context.Background(), RunCheckpoint{
		SchemaVersion: RunCheckpointSchemaVersion,
		Phase:         RunPhaseFinalizing,
		Round:         2,
		System:        "system",
		Messages:      []providers.Message{{Role: providers.RoleAssistant, Content: "finished"}},
	})
	if event := <-done; event.Kind != KindDone || event.Rounds != 2 {
		t.Fatalf("finalizing resume=%+v", event)
	}
	provider.mu.Lock()
	requests := len(provider.requests)
	provider.mu.Unlock()
	if requests != 0 {
		t.Fatalf("finalizing resume called provider %d times", requests)
	}
	if projected.Phase != RunPhaseCompleted {
		t.Fatalf("projected phase=%s, want completed", projected.Phase)
	}
}
