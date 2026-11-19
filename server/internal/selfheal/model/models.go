package model

import (
	"context"
	"time"

	"github.com/google/uuid"
)

type BlockEvent struct {
	ID              uuid.UUID
	UserID          uuid.UUID
	Command         string
	Cwd             string
	ExitCode        int
	OutputText      string
	OutputTruncated bool
	CreatedAt       time.Time
}

type BlockAnalysis struct {
	ID               uuid.UUID
	BlockEventID     uuid.UUID
	Explanation      string
	SuggestedCommand *string
	CreatedAt        time.Time
}

type SelfHealRepository interface {
	CreateBlockEvent(ctx context.Context, event *BlockEvent) error
	CreateBlockAnalysis(ctx context.Context, analysis *BlockAnalysis) error
	GetBlockEvent(ctx context.Context, id uuid.UUID) (*BlockEvent, error)
	GetBlockAnalysis(ctx context.Context, eventID uuid.UUID) (*BlockAnalysis, error)
}

type AnalysisResult struct {
	Explanation      string
	SuggestedCommand *string
	InputTokens      int64
	OutputTokens     int64
}

type LLMGateway interface {
	AnalyzeFailure(ctx context.Context, modelName string, command string, cwd string, outputText string) (*AnalysisResult, error)
	// AgentStep runs one turn of the agentic tool-calling loop.
	// The caller maintains the messages history and calls again with tool results
	// until the LLM returns Done=true or a final text response.
	AgentStep(ctx context.Context, req AgentStepRequest) (*AgentStepResponse, error)
}

// ── Agentic loop types ────────────────────────────────────────────────────────

type AgentRole string

const (
	AgentRoleSystem    AgentRole = "system"
	AgentRoleUser      AgentRole = "user"
	AgentRoleAssistant AgentRole = "assistant"
	AgentRoleTool      AgentRole = "tool"
)

// AgentMessage is one turn in a conversation.
type AgentMessage struct {
	Role       AgentRole
	Content    string
	ToolCallID string          // non-empty for role=tool
	ToolCalls  []AgentToolCall // non-empty for role=assistant when LLM requested tools
}

// AgentToolCall is a single tool invocation requested by the LLM.
type AgentToolCall struct {
	ID       string
	Name     string
	ArgsJSON string // JSON-encoded arguments matching the tool's parameter schema
}

// AgentTool defines a tool available to the LLM.
type AgentTool struct {
	Name        string
	Description string
	// Parameters is a JSON Schema object describing the tool's arguments.
	Parameters map[string]interface{}
}

// AgentStepRequest is the full input for one LLM turn.
type AgentStepRequest struct {
	ModelName string
	System    string
	Messages  []AgentMessage
	Tools     []AgentTool
	// UserApiKey is an optional user-provided API key forwarded from the client.
	// When non-empty, it overrides the gateway's configured key for this request.
	UserApiKey string
	// CustomBaseUrl, when non-empty, overrides the provider endpoint with an
	// OpenAI-compatible base URL (e.g. for Ollama or LM Studio instances).
	CustomBaseUrl string
}

// AgentStepResponse is the LLM's response for one turn.
// Either ToolCalls is non-empty (LLM wants to call tools) or Content is set (final text).
type AgentStepResponse struct {
	ToolCalls    []AgentToolCall
	Content      string
	InputTokens  int64
	OutputTokens int64
}
