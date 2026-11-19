package providers

// NewOpenRouterAdapter creates a streaming adapter for OpenRouter.
// OpenRouter proxies hundreds of models from Anthropic, OpenAI, Google, Meta, and more
// through a single Chat Completions-compatible endpoint.
//
// Model examples: anthropic/claude-3.5-sonnet, meta-llama/llama-3.3-70b-instruct,
//
//	google/gemini-2.0-flash, openai/gpt-4o
//
// Docs: https://openrouter.ai/docs
func NewOpenRouterAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "openrouter",
		BaseURL: "https://openrouter.ai/api/v1",
		APIKey:  apiKey,
	})
}
