package providers

// NewVeniceAdapter creates a streaming adapter for Venice (private
// inference) via its OpenAI-compatible endpoint.
//
// Model examples: llama-3.3-70b (always qualified: "venice/<model>").
// Docs: https://docs.venice.ai/
func NewVeniceAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "venice",
		BaseURL: "https://api.venice.ai/api/v1",
		APIKey:  apiKey,
	})
}
