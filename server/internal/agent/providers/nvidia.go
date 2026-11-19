package providers

// NewNvidiaAdapter creates a streaming adapter for NVIDIA NIM via its
// OpenAI-compatible endpoint.
//
// Model examples: meta-llama/llama-3.3-70b-instruct
// (always qualified: "nvidia/<model>").
// Docs: https://docs.nvidia.com/nim/
func NewNvidiaAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "nvidia",
		BaseURL: "https://integrate.api.nvidia.com/v1",
		APIKey:  apiKey,
	})
}
