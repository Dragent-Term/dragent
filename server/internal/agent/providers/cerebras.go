package providers

// NewCerebrasAdapter creates a streaming adapter for the Cerebras inference API.
// Cerebras runs large models at extremely high token throughput on its wafer-scale chips.
//
// Model examples: llama3.1-8b, llama3.1-70b, llama3.3-70b
// Docs: https://inference-docs.cerebras.ai/introduction
func NewCerebrasAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "cerebras",
		BaseURL: "https://api.cerebras.ai/v1",
		APIKey:  apiKey,
	})
}
