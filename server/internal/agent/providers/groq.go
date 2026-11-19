package providers

// NewGroqAdapter creates a streaming adapter for Groq's inference API.
// Groq supports the full Chat Completions streaming format with tool calls.
//
// Model examples: llama-3.3-70b-versatile, llama-3.1-8b-instant, mixtral-8x7b-32768
// Docs: https://console.groq.com/docs/openai
func NewGroqAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "groq",
		BaseURL: "https://api.groq.com/openai/v1",
		APIKey:  apiKey,
	})
}
