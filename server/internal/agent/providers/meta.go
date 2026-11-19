package providers

// NewMetaAdapter creates a streaming adapter for the Meta Model API
// (Muse Spark family) via its OpenAI-compatible Chat Completions surface.
// Muse Spark models only — the retired Llama API preview is unsupported.
//
// Model examples: muse-spark-1.3, muse-spark-1.1.
// Docs: https://dev.meta.ai/docs
func NewMetaAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "meta",
		BaseURL: "https://api.meta.ai/v1",
		APIKey:  apiKey,
	})
}
