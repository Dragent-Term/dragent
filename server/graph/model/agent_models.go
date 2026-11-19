package model

// AgentToolCall is a tool invocation the LLM wants the client to execute.
type AgentToolCall struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	ArgsJSON string `json:"argsJson"`
}

// AgentStepResult is the per-turn response in the agentic loop.
// Either ToolCalls is non-empty (client should execute them and call agentStep again)
// or Content is set (LLM has produced a final answer).
type AgentStepResult struct {
	ToolCalls []*AgentToolCall `json:"toolCalls"`
	Content   string           `json:"content"`
}

// AgentMessageInput is a single message in the agentic conversation history.
type AgentMessageInput struct {
	Role          string  `json:"role"`
	Content       string  `json:"content"`
	ToolCallID    *string `json:"toolCallId,omitempty"`
	ToolCallsJSON *string `json:"toolCallsJson,omitempty"`
}

// AgentToolInput defines a tool the LLM can call.
type AgentToolInput struct {
	Name           string `json:"name"`
	Description    string `json:"description"`
	ParametersJSON string `json:"parametersJson"`
}

// AgentStepInput is the full per-turn request from the client.
type AgentStepInput struct {
	ModelName string               `json:"modelName"`
	System    string               `json:"system"`
	Messages  []*AgentMessageInput `json:"messages"`
	Tools     []*AgentToolInput    `json:"tools"`
}
