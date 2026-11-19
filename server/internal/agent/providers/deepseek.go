package providers

// NewDeepSeekAdapter creates a streaming adapter for the DeepSeek API.
// DeepSeek serves an OpenAI-compatible Chat Completions surface (streaming,
// tool calls). Model examples: deepseek-chat, deepseek-reasoner.
//
// Note: the reasoner streams `reasoning_content` deltas alongside content;
// the shared Chat Completions reader surfaces the final content (thinking
// stays invisible, the answer is intact).
// Docs: https://api-docs.deepseek.com/
func NewDeepSeekAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "deepseek",
		BaseURL: "https://api.deepseek.com/v1",
		APIKey:  apiKey,
	})
}
