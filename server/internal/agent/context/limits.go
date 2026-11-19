package context

import "strings"

// ModelLimits describes the token budget for one model.
type ModelLimits struct {
	// MaxInputTokens is the total context window (input + output combined for some
	// providers, input-only for others). We treat it conservatively as input-only.
	MaxInputTokens int
	// MaxOutputTokens is the largest completion the model can produce.
	MaxOutputTokens int
}

// defaultLimits is the fallback when the model isn't recognised.
var defaultLimits = ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 8_192}

// limitsTable maps model name prefixes to their limits.
// Ordered from most-specific to least-specific; first match wins.
var limitsTable = []struct {
	prefix string
	limits ModelLimits
}{
	// ── Anthropic ─────────────────────────────────────────────────────────────
	// All current Claude 3/4 models: 200k input, 8192 output
	{"claude-", ModelLimits{MaxInputTokens: 200_000, MaxOutputTokens: 8_192}},

	// ── OpenAI ────────────────────────────────────────────────────────────────
	{"o4-mini", ModelLimits{MaxInputTokens: 200_000, MaxOutputTokens: 100_000}},
	{"o3-mini", ModelLimits{MaxInputTokens: 200_000, MaxOutputTokens: 100_000}},
	{"o3", ModelLimits{MaxInputTokens: 200_000, MaxOutputTokens: 100_000}},
	{"o1", ModelLimits{MaxInputTokens: 200_000, MaxOutputTokens: 100_000}},
	{"gpt-4o", ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 16_384}},
	{"gpt-4", ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 8_192}},
	{"gpt-3.5", ModelLimits{MaxInputTokens: 16_385, MaxOutputTokens: 4_096}},

	// ── Google ────────────────────────────────────────────────────────────────
	{"gemini-2.0-flash-thinking", ModelLimits{MaxInputTokens: 1_048_576, MaxOutputTokens: 65_536}},
	{"gemini-2.0", ModelLimits{MaxInputTokens: 1_048_576, MaxOutputTokens: 8_192}},
	{"gemini-1.5-pro", ModelLimits{MaxInputTokens: 2_097_152, MaxOutputTokens: 8_192}},
	{"gemini-1.5-flash", ModelLimits{MaxInputTokens: 1_048_576, MaxOutputTokens: 8_192}},
	{"gemini-", ModelLimits{MaxInputTokens: 1_048_576, MaxOutputTokens: 8_192}},

	// ── Meta / Groq ───────────────────────────────────────────────────────────
	{"llama-3.3", ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 8_192}},
	{"llama-3.2", ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 8_192}},
	{"llama-3.1", ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 8_192}},
	{"llama-3", ModelLimits{MaxInputTokens: 8_192, MaxOutputTokens: 8_192}},
	{"mixtral", ModelLimits{MaxInputTokens: 32_768, MaxOutputTokens: 4_096}},

	// ── Cerebras ──────────────────────────────────────────────────────────────
	{"llama3.3-70b-cerebras", ModelLimits{MaxInputTokens: 128_000, MaxOutputTokens: 8_192}},
}

// For returns the context limits for the given bare model name.
func For(modelName string) ModelLimits {
	lower := strings.ToLower(modelName)
	for _, entry := range limitsTable {
		if strings.HasPrefix(lower, entry.prefix) {
			return entry.limits
		}
	}
	return defaultLimits
}

// WithContextWindow applies the database-owned model context window while
// retaining the adapter-safe output reserve. The DB catalog is already what
// drives model availability and user-facing meters; using it here prevents a
// third, stale source of truth inside the runner.
func WithContextWindow(modelName string, contextWindow int64) ModelLimits {
	limits := For(modelName)
	if contextWindow > 0 && contextWindow <= int64(^uint(0)>>1) {
		limits.MaxInputTokens = int(contextWindow)
	}
	if limits.MaxOutputTokens >= limits.MaxInputTokens {
		limits.MaxOutputTokens = max(1, limits.MaxInputTokens/8)
	}
	return limits
}

// InputBudget returns how many input tokens are available after reserving space
// for the model's maximum output. This is the real ceiling for the history.
func (m ModelLimits) InputBudget() int {
	return m.MaxInputTokens - m.MaxOutputTokens
}

// CompactionThreshold returns the token count at which we should compact
// history before the next LLM call (80% of input budget).
func (m ModelLimits) CompactionThreshold() int {
	return int(float64(m.InputBudget()) * 0.80)
}
