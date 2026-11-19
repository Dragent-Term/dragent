package providers

// NewDeepInfraAdapter creates a streaming adapter for DeepInfra
// (open-model hosting) via its OpenAI-compatible endpoint.
//
// Model examples: meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8
// (always qualified: "deepinfra/<model>").
// Docs: https://deepinfra.com/docs
func NewDeepInfraAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "deepinfra",
		BaseURL: "https://api.deepinfra.net/v1/openai",
		APIKey:  apiKey,
	})
}
