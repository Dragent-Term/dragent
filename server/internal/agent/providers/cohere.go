package providers

// NewCohereAdapter creates a streaming adapter for Cohere via its
// OpenAI-compatibility endpoint.
//
// Model examples: command-a, command-r-plus.
// Docs: https://docs.cohere.com/docs/openai-compatibility
func NewCohereAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "cohere",
		BaseURL: "https://api.cohere.com/compatibility/v1",
		APIKey:  apiKey,
	})
}
