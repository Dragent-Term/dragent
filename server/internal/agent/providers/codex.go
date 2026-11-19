package providers

// NewCodexAdapter creates an adapter for OpenAI Codex models.
// Codex models (e.g. codex-mini-latest) use the OpenAI API key and the same
// Chat Completions endpoint as other OpenAI models.
//
// Model examples: codex-mini-latest
func NewCodexAdapter(apiKey string) *OpenAIAdapter {
	return NewOpenAIAdapter(apiKey)
}
