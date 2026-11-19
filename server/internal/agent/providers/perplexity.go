package providers

// NewPerplexityAdapter creates a streaming adapter for the Perplexity API.
// OpenAI-compatible Chat Completions; search is built into every model, so
// no separate search backend is needed on this path.
//
// Model examples: sonar, sonar-pro, sonar-reasoning-pro.
// Docs: https://docs.perplexity.ai/
func NewPerplexityAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "perplexity",
		BaseURL: "https://api.perplexity.ai",
		APIKey:  apiKey,
	})
}
