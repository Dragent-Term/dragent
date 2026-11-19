package providers

// NewZhipuAdapter creates a streaming adapter for the Zhipu BigModel API
// (GLM family) via its OpenAI-compatible endpoint.
//
// Model examples: glm-4-plus, glm-4, glm-4-flash.
// Docs: https://docs.bigmodel.org/
func NewZhipuAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "zhipu",
		BaseURL: "https://open.bigmodel.cn/api/paas/v4/",
		APIKey:  apiKey,
	})
}
