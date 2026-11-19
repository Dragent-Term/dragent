package providers

import (
	"encoding/json"
	"testing"
)

// The adapter must pass caller tools through unchanged — web_search is a plain
// function tool executed server-side by the pipeline's webtools executor, and
// no native server tool may be injected inline (inline search results would
// land in the frontier model's context and stay there every turn).
func TestBuildBody_NoNativeToolInjection(t *testing.T) {
	a := NewAnthropicAdapter("test-key")
	body, err := a.buildBody(StepRequest{
		ModelName: "claude-opus-4-8",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Tools: []ToolDefinition{{
			Name:        "web_search",
			Description: "Search the web.",
			Parameters:  map[string]interface{}{"type": "object"},
		}},
	})
	if err != nil {
		t.Fatalf("buildBody: %v", err)
	}
	var parsed struct {
		Tools []map[string]interface{} `json:"tools"`
	}
	if err := json.Unmarshal(body, &parsed); err != nil {
		t.Fatalf("unmarshal body: %v", err)
	}
	if len(parsed.Tools) != 1 {
		t.Fatalf("expected exactly the caller's tool, got %d tools", len(parsed.Tools))
	}
	tool := parsed.Tools[0]
	if tool["name"] != "web_search" {
		t.Fatalf("expected web_search passthrough, got %v", tool["name"])
	}
	if _, hasType := tool["type"]; hasType {
		t.Fatalf("web_search must be a plain function tool, got server-tool type %v", tool["type"])
	}
	if _, hasSchema := tool["input_schema"]; !hasSchema {
		t.Fatal("expected input_schema on the passthrough tool")
	}
}
