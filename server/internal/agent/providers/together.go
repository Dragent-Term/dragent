package providers

// NewTogetherAdapter creates a streaming adapter for Together AI
// (open-model hosting) via its OpenAI-compatible endpoint.
//
// Model examples: deepseek-ai/DeepSeek-V3, Qwen/Qwen3-235B-A22B-fp8-tput
// (always qualified: "together/<model>" — bare vendor namespaces are
// ambiguous across hosts).
// Docs: https://docs.together.ai/
func NewTogetherAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "together",
		BaseURL: "https://api.together.xyz/v1",
		APIKey:  apiKey,
	})
}
