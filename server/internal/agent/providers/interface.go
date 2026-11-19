package providers

import (
	"context"
	"encoding/json"
)

type AgentRole string

const (
	RoleSystem    AgentRole = "system"
	RoleUser      AgentRole = "user"
	RoleAssistant AgentRole = "assistant"
	RoleTool      AgentRole = "tool"
)

// Message is one turn in a conversation.
type Message struct {
	Role       AgentRole         `json:"role"`
	Content    string            `json:"content"`
	ToolCallID string            `json:"tool_call_id,omitempty"` // non-empty for role=tool
	ToolCalls  []ToolCall        `json:"tool_calls,omitempty"`   // non-empty for role=assistant with tool use
	Images     []ImageAttachment `json:"attachments,omitempty"`  // user-supplied images (screenshots, design exports)
}

// ImageAttachment is an image the user attached to a message (screenshot,
// Figma export, photo). Providers encode it as a multimodal content part.
type ImageAttachment struct {
	MediaType string `json:"media_type"` // image/png, image/jpeg, image/webp, image/gif
	DataB64   string `json:"data"`       // raw base64 payload (no data: URL prefix)
}

// MarshalJSON keeps provider-internal image state compatible with the desktop
// task wire format when compacted history is streamed back to the local owner.
func (a ImageAttachment) MarshalJSON() ([]byte, error) {
	return json.Marshal(struct {
		Type      string `json:"type"`
		MediaType string `json:"media_type"`
		Data      string `json:"data"`
	}{Type: "image", MediaType: a.MediaType, Data: a.DataB64})
}

// EstImageTokens is the per-image token estimate used by pre-flight cost
// checks. Anthropic charges ~(w×h)/750; a full-size 1568×1568 image is
// ~1600 tokens, so this is the conservative ceiling. Actual billing uses
// provider-reported usage, so this only gates the capacity check.
const EstImageTokens = 1600

// ToolCall is a single tool invocation the LLM requested.
type ToolCall struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	ArgsJSON string `json:"args"` // JSON-encoded args matching the tool's parameter schema
}

// ToolDefinition describes a tool available to the LLM.
type ToolDefinition struct {
	Name        string
	Description string
	Parameters  map[string]interface{} // JSON Schema object
}

// StepRequest is the full input for one LLM turn.
type StepRequest struct {
	ModelName string
	System    string
	Messages  []Message
	Tools     []ToolDefinition

	// MaxTokens controls the max completion size (0 = provider default).
	MaxTokens int

	// Effort is the user-selected reasoning depth ("" = provider default).
	// Adapters must map it through ResolveEffort, which drops it for models
	// that reject the parameter — see effort.go.
	Effort Effort

	// Sampling parameters — nil means use the provider's default.
	Temperature *float64
	TopP        *float64

	// StopSequences halts generation when any sequence is produced.
	StopSequences []string

	// ProviderOptions passes extra body fields directly to the provider API.
	// Keys here override any field set by buildBody, so use with care.
	// Example: map[string]interface{}{"response_format": map[string]string{"type": "json_object"}}
	ProviderOptions map[string]interface{}

	// DisableServerTools is retained for call-site compatibility but adapters
	// no longer inject provider-native server tools inline — web search/fetch
	// run through the pipeline's server-side webtools executor instead, so
	// there is nothing left to suppress.
	DisableServerTools bool
}

// ── Streaming event types ─────────────────────────────────────────────────────

type StreamEventKind string

const (
	EventThinking  StreamEventKind = "thinking"
	EventTextDelta StreamEventKind = "text_delta"
	EventToolStart StreamEventKind = "tool_start"
	EventDone      StreamEventKind = "done"
	EventError     StreamEventKind = "error"
)

// StreamEvent is one item emitted by LLMProvider.StreamStep.
type StreamEvent struct {
	Kind StreamEventKind

	// EventTextDelta / EventThinking
	Text string

	// EventToolStart
	ToolCall *ToolCall

	// EventDone
	Result *StepResult

	// EventError
	Err error
}

// StepResult is the final aggregate from one LLM turn.
type StepResult struct {
	ToolCalls         []ToolCall
	Content           string // non-empty = the LLM's final text answer
	InputTokens       int64
	CachedInputTokens int64 // tokens served from prompt cache (charged at lower rate)
	OutputTokens      int64
}

// ── Provider capabilities ─────────────────────────────────────────────────────

// ProviderCapabilities declares what a provider adapter supports.
// Callers use this to decide how to format requests and what features to expose.
type ProviderCapabilities struct {
	Streaming        bool // supports SSE streaming
	ToolCalling      bool // supports function/tool calling
	Thinking         bool // supports extended reasoning / thinking tokens
	ImageInput       bool // accepts image content in messages
	StructuredOutput bool // supports JSON mode / structured output
	Caching          bool // supports prompt caching (e.g. Anthropic cache_control)
}

// ── Provider interface ────────────────────────────────────────────────────────

// LLMProvider is the common interface every provider adapter must satisfy.
// StreamStep streams events on the returned channel until a done or error event.
// The channel is closed after the terminal event is sent.
// Callers must drain the channel and cancel ctx to free resources.
type LLMProvider interface {
	// StreamStep initiates one streaming LLM turn.
	StreamStep(ctx context.Context, req StepRequest) (<-chan StreamEvent, error)

	// SupportedTools returns tools natively injected by this provider
	// (e.g. web_search). These are merged with the caller's tool list.
	SupportedTools() []ToolDefinition

	// ProviderName returns a stable name used for logging / metrics.
	ProviderName() string

	// Capabilities declares which features this provider supports.
	// Callers check this before using optional features like tool calling.
	Capabilities() ProviderCapabilities
}
