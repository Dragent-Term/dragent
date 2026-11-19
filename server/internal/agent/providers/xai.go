package providers

// NewXAIAdapter creates a streaming adapter for xAI's Grok API.
// xAI exposes a fully OpenAI-compatible Chat Completions endpoint.
//
// Model examples: grok-4.3, grok-4
// Docs: https://docs.x.ai/developers
func NewXAIAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "xai",
		BaseURL: "https://api.x.ai/v1",
		APIKey:  apiKey,
	})
}
