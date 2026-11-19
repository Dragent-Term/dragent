package providers

// NewMoonshotAdapter creates a streaming adapter for the Moonshot API
// (Kimi family) via its OpenAI-compatible endpoint. Uses the international
// endpoint; mainland callers point a custom endpoint at
// https://api.moonshot.cn/v1 instead.
//
// Model examples: kimi-k2, moonshot-v1-8k.
// Docs: https://platform.moonshot.ai/docs
func NewMoonshotAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "moonshot",
		BaseURL: "https://api.moonshot.ai/v1",
		APIKey:  apiKey,
	})
}
