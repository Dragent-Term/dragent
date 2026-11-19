package pipeline

import "server/internal/agent/providers"

// AgentEventKind identifies the type of a server-sent event in the agent stream.
type AgentEventKind string

const (
	// KindThinking streams reasoning tokens (extended thinking / chain-of-thought).
	KindThinking AgentEventKind = "agent.thinking"

	// KindToolCallStarted is emitted as soon as the LLM begins outputting a tool call
	// (before args have fully streamed). The client shows a pending indicator but does
	// NOT execute yet — args are not available at this point.
	KindToolCallStarted AgentEventKind = "agent.tool_call_started"

	// KindToolCall tells the client to execute a tool locally and POST the result back.
	// This is sent once args are fully assembled and the call is ready to run.
	KindToolCall AgentEventKind = "agent.tool_call"

	// KindToolEnd reports that a tool call is complete (sent after client result received).
	KindToolEnd AgentEventKind = "agent.tool_end"

	// KindTextDelta streams a chunk of the final text answer.
	KindTextDelta AgentEventKind = "agent.text_delta"

	// KindDone closes the stream with usage summary.
	KindDone AgentEventKind = "agent.done"

	// KindStepDone closes a desktop-owned, single-provider-step stream. The
	// desktop persists the returned assistant/tool state locally, executes local
	// tools, and submits a fresh stateless step request. It is never written to
	// the durable server task store.
	KindStepDone AgentEventKind = "agent.step_done"

	// KindError terminates the stream with an error.
	KindError AgentEventKind = "agent.error"
)

// AgentEvent is one item in the SSE stream sent from server → client.
type AgentEvent struct {
	Kind AgentEventKind `json:"kind"`

	// KindThinking / KindTextDelta
	Text string `json:"text,omitempty"`

	// KindToolCallStarted / KindToolCall — tool identity
	ToolID   string `json:"tool_id,omitempty"`
	ToolName string `json:"tool_name,omitempty"`
	ToolIcon string `json:"tool_icon,omitempty"`

	// KindToolCall only — args are not yet available in KindToolCallStarted
	ToolArgs         string `json:"tool_args,omitempty"`         // full JSON args for client execution
	ArgsSummary      string `json:"args_summary,omitempty"`      // human-readable summary for UI
	RequiresApproval bool   `json:"requires_approval,omitempty"` // client must ask user before executing
	// ServerExecuted marks a tool call the SERVER executes itself (web_search /
	// fetch_url via the webtools executor). The client renders the tool card
	// but must NOT execute the tool or post a result — KindToolEnd follows
	// once the server-side execution completes.
	ServerExecuted bool `json:"server_executed,omitempty"`

	// KindToolEnd — sent after client posts the result
	ResultSummary string `json:"result_summary,omitempty"`
	DurationMs    int64  `json:"duration_ms,omitempty"`
	// ToolOutput is returned only for a server-executed tool in desktop-owned
	// mode so the local orchestrator can checkpoint it. It is streamed directly
	// and is not persisted by the server.
	ToolOutput  string `json:"tool_output,omitempty"`
	ToolIsError bool   `json:"tool_is_error,omitempty"`

	// AssistantContent is the exact provider message associated with a
	// KindStepDone event. Text deltas remain the live UI stream.
	AssistantContent string `json:"assistant_content,omitempty"`
	// CompactedMessages returns the exact provider-ready history only when this
	// step compacted it. The desktop replaces its local history before adding
	// the current assistant/tool messages, avoiding repeated compaction. It is
	// delivered live and never written to the desktop-owned task store.
	CompactedMessages []providers.Message `json:"compacted_messages,omitempty"`

	// KindDone
	TotalInputTokens       int64 `json:"total_input_tokens,omitempty"`
	TotalCachedInputTokens int64 `json:"total_cached_input_tokens,omitempty"`
	TotalOutputTokens      int64 `json:"total_output_tokens,omitempty"`
	Rounds                 int   `json:"rounds,omitempty"`

	// ContextTokens is the CURRENT context size: the prompt tokens (input +
	// cached) of the LAST round — everything sent to the model on the final
	// turn, as reported by the provider's usage. This is the authoritative
	// "context used" figure for the client's context-window meter. Unlike the
	// Total* fields (summed across rounds) it is NOT cumulative.
	ContextTokens int64 `json:"context_tokens,omitempty"`

	// KindError
	Error string `json:"error,omitempty"`
}
