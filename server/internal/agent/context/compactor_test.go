package context

import (
	"context"
	"strings"
	"testing"

	"server/internal/agent/providers"
)

type summaryProvider struct{}

func (summaryProvider) StreamStep(context.Context, providers.StepRequest) (<-chan providers.StreamEvent, error) {
	stream := make(chan providers.StreamEvent, 2)
	stream <- providers.StreamEvent{Kind: providers.EventTextDelta, Text: "dense summary"}
	stream <- providers.StreamEvent{Kind: providers.EventDone, Result: &providers.StepResult{InputTokens: 5, OutputTokens: 2}}
	close(stream)
	return stream, nil
}
func (summaryProvider) SupportedTools() []providers.ToolDefinition { return nil }
func (summaryProvider) ProviderName() string                       { return "summary" }
func (summaryProvider) Capabilities() providers.ProviderCapabilities {
	return providers.ProviderCapabilities{Streaming: true}
}

func TestCompactionIncludesDeterministicToolLedger(t *testing.T) {
	messages := []providers.Message{
		{Role: providers.RoleUser, Content: "fix the upload flow"},
		{Role: providers.RoleUser, Content: "never delete existing fixtures"},
		{Role: providers.RoleAssistant, ToolCalls: []providers.ToolCall{{ID: "read-1", Name: "read_file", ArgsJSON: `{"path":"/workspace/upload.go"}`}}},
		{Role: providers.RoleTool, ToolCallID: "read-1", Content: "package upload\nfunc Save() {}"},
		{Role: providers.RoleAssistant, Content: "I found the handler."},
		{Role: providers.RoleUser, Content: "continue"},
		{Role: providers.RoleAssistant, ToolCalls: []providers.ToolCall{
			{ID: "test-1", Name: "run_command", ArgsJSON: `{"command":"go test ./..."}`},
			{ID: "read-2", Name: "read_file", ArgsJSON: `{"path":"/workspace/test.go"}`},
		}},
		{Role: providers.RoleTool, ToolCallID: "test-1", Content: "ok"},
		{Role: providers.RoleTool, ToolCallID: "read-2", Content: "fixture contents"},
		{Role: providers.RoleUser, Content: "finish it"},
		{Role: providers.RoleAssistant, Content: "working"},
	}
	result, err := Compact(context.Background(), summaryProvider{}, "test", "system", messages, nil, ModelLimits{MaxInputTokens: 1000, MaxOutputTokens: 100})
	if err != nil {
		t.Fatal(err)
	}
	joined := ""
	for _, message := range result.Messages {
		joined += message.Content
	}
	if !strings.Contains(joined, "<deterministic_execution_ledger>") ||
		!strings.Contains(joined, "call_id=read-1") ||
		!strings.Contains(joined, "never delete existing fixtures") {
		t.Fatalf("structured ledger missing from compacted history: %s", joined)
	}
	// Tail adjustment must retain the assistant call before its two tool results.
	for i, message := range result.Messages {
		if message.Role == providers.RoleTool && (i == 0 || len(result.Messages[i-1].ToolCalls) == 0 && result.Messages[i-1].Role != providers.RoleTool) {
			t.Fatalf("orphan tool result at compacted index %d: %+v", i, result.Messages)
		}
	}
}

func TestLedgerTruncationPreservesUTF8(t *testing.T) {
	value := strings.Repeat("界", 600)
	got := truncateLedger(value, 1_000)
	if !strings.HasSuffix(got, "…") || strings.ContainsRune(got, '\uFFFD') {
		t.Fatalf("invalid UTF-8 truncation: %q", got[len(got)-12:])
	}
}
