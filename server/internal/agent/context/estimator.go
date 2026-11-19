package context

import (
	"encoding/json"

	"server/internal/agent/providers"
)

// EstimateTokens returns a conservative token estimate for a string.
// Uses chars/3 — this over-estimates compared to real tokenizers,
// which means we trigger compaction earlier than strictly necessary (safe).
func EstimateTokens(s string) int {
	n := len(s) / 3
	if n < 1 && len(s) > 0 {
		return 1
	}
	return n
}

// EstimateMessageTokens returns the token estimate for a single message,
// including role, content, and any embedded tool call JSON.
func EstimateMessageTokens(m providers.Message) int {
	total := EstimateTokens(string(m.Role)) + EstimateTokens(m.Content)
	for _, tc := range m.ToolCalls {
		total += EstimateTokens(tc.Name) + EstimateTokens(tc.ArgsJSON)
	}
	if m.ToolCallID != "" {
		total += EstimateTokens(m.ToolCallID)
	}
	total += len(m.Images) * providers.EstImageTokens
	// Per-message overhead (role formatting, separators)
	total += 4
	return total
}

// EstimateHistoryTokens returns the cumulative token estimate for a message slice.
func EstimateHistoryTokens(messages []providers.Message) int {
	total := 0
	for _, m := range messages {
		total += EstimateMessageTokens(m)
	}
	return total
}

// EstimateToolDefsTokens returns the token estimate for the tool definition list.
func EstimateToolDefsTokens(tools []providers.ToolDefinition) int {
	total := 0
	for _, t := range tools {
		total += EstimateTokens(t.Name) + EstimateTokens(t.Description)
		if t.Parameters != nil {
			if b, err := json.Marshal(t.Parameters); err == nil {
				total += EstimateTokens(string(b))
			}
		}
	}
	return total
}

// BudgetUsed returns the estimated total tokens consumed by a full request
// (system prompt + history + tool defs). Compare against ModelLimits.InputBudget().
func BudgetUsed(system string, messages []providers.Message, tools []providers.ToolDefinition) int {
	return EstimateTokens(system) +
		EstimateHistoryTokens(messages) +
		EstimateToolDefsTokens(tools)
}
