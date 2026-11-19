package providers

// NewOllamaAdapter creates a streaming adapter for a local Ollama server.
// Ollama exposes an OpenAI-compatible API; no API key is required.
// baseURL defaults to http://localhost:11434/v1 if empty.
//
// Model examples: llama3.2, deepseek-r1, qwen2.5-coder, phi4
// Docs: https://github.com/ollama/ollama/blob/main/docs/openai.md
func NewOllamaAdapter(baseURL string) *OpenAICompatAdapter {
	return NewOllamaAdapterWithTrust(baseURL, false)
}
func NewOllamaAdapterWithTrust(baseURL string, userSupplied bool) *OpenAICompatAdapter {
	if baseURL == "" {
		baseURL = "http://localhost:11434/v1"
	}
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:         "ollama",
		BaseURL:      baseURL,
		APIKey:       "ollama", // Ollama ignores the key but the header must be present
		UserSupplied: userSupplied,
	})
}
