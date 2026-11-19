package providers

// NewLMStudioAdapter creates a streaming adapter for a local LM Studio server.
// LM Studio exposes an OpenAI-compatible API on localhost.
// baseURL defaults to http://localhost:1234/v1 if empty.
//
// Docs: https://lmstudio.ai/docs/api
func NewLMStudioAdapter(baseURL string) *OpenAICompatAdapter {
	return NewLMStudioAdapterWithTrust(baseURL, false)
}
func NewLMStudioAdapterWithTrust(baseURL string, userSupplied bool) *OpenAICompatAdapter {
	if baseURL == "" {
		baseURL = "http://localhost:1234/v1"
	}
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:         "lmstudio",
		BaseURL:      baseURL,
		UserSupplied: userSupplied,
		APIKey:       "lmstudio",
	})
}
