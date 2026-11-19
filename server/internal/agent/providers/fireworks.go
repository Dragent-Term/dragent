package providers

// NewFireworksAdapter creates a streaming adapter for Fireworks
// (open-model hosting) via its OpenAI-compatible endpoint.
//
// Model examples: accounts/fireworks/models/llama-v3p3-70b-instruct
// (always qualified: "fireworks/<model>").
// Docs: https://docs.fireworks.ai/
func NewFireworksAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "fireworks",
		BaseURL: "https://api.fireworks.ai/inference/v1",
		APIKey:  apiKey,
	})
}
