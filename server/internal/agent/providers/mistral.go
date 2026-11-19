package providers

// NewMistralAdapter creates a streaming adapter for Mistral La Plateforme.
// OpenAI-compatible Chat Completions with tool calls.
//
// Model examples: mistral-large-latest, mistral-medium-latest, codestral-latest.
// Docs: https://docs.mistral.ai/
func NewMistralAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "mistral",
		BaseURL: "https://api.mistral.ai/v1",
		APIKey:  apiKey,
	})
}
