package providers

// NewAlibabaAdapter creates a streaming adapter for Alibaba Model Studio
// (Qwen family) via its OpenAI-compatible endpoint. Uses the international
// endpoint; mainland callers point a custom endpoint at
// https://dashscope.aliyuncs.com/compatible-mode/v1 instead.
//
// Model examples: qwen-max, qwen-plus, qwen-turbo.
// Docs: https://www.alibabacloud.com/help/en/model-studio/
func NewAlibabaAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "alibaba",
		BaseURL: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
		APIKey:  apiKey,
	})
}
