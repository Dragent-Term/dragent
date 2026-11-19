package providers

// NewMiniMaxAdapter creates a streaming adapter for the MiniMax API via its
// OpenAI-compatible endpoint (international platform).
//
// Model examples: MiniMax-Text-01.
// Docs: https://www.minimaxi.com/document/guides/chat-service
func NewMiniMaxAdapter(apiKey string) *OpenAICompatAdapter {
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:    "minimax",
		BaseURL: "https://api.minimax.io/v1",
		APIKey:  apiKey,
	})
}
