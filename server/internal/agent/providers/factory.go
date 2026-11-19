package providers

import (
	"fmt"
	"strings"

	"server/internal/platform/netguard"
)

// ProviderFactory resolves a model name to the correct LLMProvider adapter.
// It reads from a flat key map — the same shape as the selfheal gateway.
type ProviderFactory struct {
	keys map[string]string
}

// NewProviderFactory creates a factory from a flat key map.
//
// Supported keys:
//
//	ANTHROPIC_API_KEY        — Claude models
//	OPENAI_API_KEY           — gpt-4o, o1, o3, o4 models
//	GOOGLE_API_KEY           — Gemini models
//	GROQ_API_KEY             — Groq-hosted models (llama, mixtral)
//	XAI_API_KEY              — xAI Grok models (grok-4.3, grok-4)
//	OPENROUTER_API_KEY       — OpenRouter (access to any model)
//	CEREBRAS_API_KEY         — Cerebras (llama on wafer-scale chips)
//	OLLAMA_BASE_URL          — local Ollama (default: http://localhost:11434/v1)
//	LM_STUDIO_BASE_URL       — local LM Studio (default: http://localhost:1234/v1)
//	AZURE_OPENAI_ENDPOINT    — Azure OpenAI resource URL
//	AZURE_OPENAI_API_KEY     — Azure OpenAI API key
//	AZURE_OPENAI_API_VERSION — Azure API version (default: 2024-02-01)
func NewProviderFactory(keys map[string]string) *ProviderFactory {
	return &ProviderFactory{keys: keys}
}

// Keys returns a copy of the flat key map for components that need direct
// provider credentials (e.g. the server-side web tools executor).
func (f *ProviderFactory) Keys() map[string]string {
	out := make(map[string]string, len(f.keys))
	for k, v := range f.keys {
		out[k] = v
	}
	return out
}

// ForModelWithUserKey creates a provider using the user's own API key instead of
// the server's key. For OpenAI-compatible custom endpoints (customBaseURL is set)
// the adapter uses baseURL + userKey regardless of model name. For named providers
// (anthropic, openai, google, xai) the routing follows the same convention as
// ForModel but injects the user's key instead of the server's.
//
// The returned bare model name is identical to what ForModel would return for the
// same modelName — use it directly as StepRequest.ModelName.
func (f *ProviderFactory) ForModelWithUserKey(modelName, userKey, customBaseURL string) (LLMProvider, string, error) {
	if modelName == "" {
		modelName = "claude-sonnet-4-6"
	}

	// Custom OpenAI-compatible endpoint: base URL + user key + whatever model the
	// client specifies. This handles Ollama, LM Studio, and any local/proxy endpoint.
	if customBaseURL != "" {
		if err := netguard.ValidateURL(customBaseURL); err != nil {
			return nil, "", err
		}
		return NewOpenAICompatAdapter(OpenAICompatConfig{
			Name:         "byok-compat",
			BaseURL:      customBaseURL,
			APIKey:       userKey,
			UserSupplied: true,
		}), modelName, nil
	}

	// Named provider routing — same prefixes as ForModel but with the user's key.
	if idx := strings.Index(modelName, "/"); idx > 0 {
		prefix := modelName[:idx]
		bare := modelName[idx+1:]
		switch prefix {
		case "groq":
			return NewGroqAdapter(userKey), bare, nil
		case "openrouter":
			return NewOpenRouterAdapter(userKey), bare, nil
		case "ollama":
			return NewOllamaAdapterWithTrust(customBaseURL, true), bare, nil
		case "lmstudio":
			return NewLMStudioAdapterWithTrust(customBaseURL, true), bare, nil
		case "cerebras":
			return NewCerebrasAdapter(userKey), bare, nil
		case "google":
			return NewGeminiAdapter(userKey), bare, nil
		case "deepseek":
			return NewDeepSeekAdapter(userKey), bare, nil
		case "qwen", "alibaba":
			return NewAlibabaAdapter(userKey), bare, nil
		case "glm", "zhipu":
			return NewZhipuAdapter(userKey), bare, nil
		case "kimi", "moonshot":
			return NewMoonshotAdapter(userKey), bare, nil
		case "minimax":
			return NewMiniMaxAdapter(userKey), bare, nil
		case "mistral":
			return NewMistralAdapter(userKey), bare, nil
		case "together":
			return NewTogetherAdapter(userKey), bare, nil
		case "deepinfra":
			return NewDeepInfraAdapter(userKey), bare, nil
		case "perplexity":
			return NewPerplexityAdapter(userKey), bare, nil
		case "venice":
			return NewVeniceAdapter(userKey), bare, nil
		case "nvidia":
			return NewNvidiaAdapter(userKey), bare, nil
		case "fireworks":
			return NewFireworksAdapter(userKey), bare, nil
		case "cohere":
			return NewCohereAdapter(userKey), bare, nil
		case "meta":
			return NewMetaAdapter(userKey), bare, nil
		default:
			return nil, "", fmt.Errorf("unknown provider prefix %q", prefix)
		}
	}

	switch {
	case strings.HasPrefix(modelName, "claude-"):
		return NewAnthropicAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "gpt-") ||
		strings.HasPrefix(modelName, "o1-") ||
		strings.HasPrefix(modelName, "o3-") ||
		strings.HasPrefix(modelName, "o4-"):
		return NewOpenAIAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "codex-"):
		return NewCodexAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "gemini-"):
		return NewGeminiAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "grok-"):
		return NewXAIAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "llama") ||
		strings.HasPrefix(modelName, "mixtral") ||
		strings.HasPrefix(modelName, "gemma"):
		return NewGroqAdapter(userKey), modelName, nil
	// Official DeepSeek API ids only — bare "deepseek-r1*" names belong to
	// local Ollama catalogs and must not route here.
	case modelName == "deepseek-chat" || modelName == "deepseek-reasoner":
		return NewDeepSeekAdapter(userKey), modelName, nil
	// Bare qwen* stays Cerebras-hosted (managed default, unchanged); the
	// "qwen/"/"alibaba/" prefixes above select Alibaba explicitly.
	case strings.HasPrefix(modelName, "qwen"):
		return NewCerebrasAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "glm"):
		return NewZhipuAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "kimi-") || strings.HasPrefix(modelName, "moonshot"):
		return NewMoonshotAdapter(userKey), modelName, nil
	case strings.HasPrefix(strings.ToLower(modelName), "minimax"):
		return NewMiniMaxAdapter(userKey), modelName, nil
	// Bare first-party ids with unambiguous vendor namespaces. Hosted
	// namespaces (together/…, deepinfra/…, …) stay prefix-only on purpose.
	case strings.HasPrefix(modelName, "mistral-") || strings.HasPrefix(modelName, "codestral-"):
		return NewMistralAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "sonar-") || modelName == "sonar":
		return NewPerplexityAdapter(userKey), modelName, nil
	case strings.HasPrefix(modelName, "command-"):
		return NewCohereAdapter(userKey), modelName, nil
	// Meta Muse Spark only — the retired Llama preview is unsupported, and
	// bare llama names belong to third-party hosts, never to Meta.
	case strings.HasPrefix(strings.ToLower(modelName), "muse-spark"):
		return NewMetaAdapter(userKey), modelName, nil
	default:
		return nil, "", fmt.Errorf("no provider matched model %q for user key", modelName)
	}
}

// ForModel returns an LLMProvider for the given model name and the bare model
// name (after stripping any routing prefix).
//
// Routing conventions:
//
//	claude-*                  → Anthropic (streaming Messages API)
//	gpt-* / o1-* / o3-* / o4-* → OpenAI (streaming Chat Completions)
//	gemini-*                  → Gemini
//	grok-*                    → xAI (OpenAI-compat at api.x.ai/v1)
//	groq/<model>              → Groq (streaming Chat Completions)
//	openrouter/<model>        → OpenRouter (streaming Chat Completions)
//	ollama/<model>            → local Ollama (streaming Chat Completions)
//	lmstudio/<model>          → local LM Studio (streaming Chat Completions)
//	azure/<deployment>        → Azure OpenAI (streaming Chat Completions)
//	cerebras/<model>          → Cerebras (streaming Chat Completions)
//	google/<model>            → Gemini via Google AI
func (f *ProviderFactory) ForModel(modelName string) (LLMProvider, string, error) {
	if modelName == "" {
		modelName = "claude-sonnet-4-6"
	}

	// Explicit routing prefix: "provider/model-name"
	if idx := strings.Index(modelName, "/"); idx > 0 {
		prefix := modelName[:idx]
		bare := modelName[idx+1:]
		switch prefix {
		case "groq":
			return NewGroqAdapter(f.keys["GROQ_API_KEY"]), bare, nil
		case "openrouter":
			return NewOpenRouterAdapter(f.keys["OPENROUTER_API_KEY"]), bare, nil
		case "ollama":
			return NewOllamaAdapter(f.keys["OLLAMA_BASE_URL"]), bare, nil
		case "lmstudio":
			return NewLMStudioAdapter(f.keys["LM_STUDIO_BASE_URL"]), bare, nil
		case "cerebras":
			return NewCerebrasAdapter(f.keys["CEREBRAS_API_KEY"]), bare, nil
		case "deepseek":
			return NewDeepSeekAdapter(f.keys["DEEPSEEK_API_KEY"]), bare, nil
		case "qwen", "alibaba":
			return NewAlibabaAdapter(f.keys["ALIBABA_API_KEY"]), bare, nil
		case "glm", "zhipu":
			return NewZhipuAdapter(f.keys["ZHIPU_API_KEY"]), bare, nil
		case "kimi", "moonshot":
			return NewMoonshotAdapter(f.keys["MOONSHOT_API_KEY"]), bare, nil
		case "minimax":
			return NewMiniMaxAdapter(f.keys["MINIMAX_API_KEY"]), bare, nil
		case "mistral":
			return NewMistralAdapter(f.keys["MISTRAL_API_KEY"]), bare, nil
		case "together":
			return NewTogetherAdapter(f.keys["TOGETHER_API_KEY"]), bare, nil
		case "deepinfra":
			return NewDeepInfraAdapter(f.keys["DEEPINFRA_API_KEY"]), bare, nil
		case "perplexity":
			return NewPerplexityAdapter(f.keys["PERPLEXITY_API_KEY"]), bare, nil
		case "venice":
			return NewVeniceAdapter(f.keys["VENICE_API_KEY"]), bare, nil
		case "nvidia":
			return NewNvidiaAdapter(f.keys["NVIDIA_API_KEY"]), bare, nil
		case "fireworks":
			return NewFireworksAdapter(f.keys["FIREWORKS_API_KEY"]), bare, nil
		case "cohere":
			return NewCohereAdapter(f.keys["COHERE_API_KEY"]), bare, nil
		case "meta":
			return NewMetaAdapter(f.keys["META_API_KEY"]), bare, nil
		case "azure":
			return NewAzureAdapter(
				f.keys["AZURE_OPENAI_ENDPOINT"],
				bare,
				f.keys["AZURE_OPENAI_API_KEY"],
				f.keys["AZURE_OPENAI_API_VERSION"],
			), bare, nil
		case "google":
			return NewGeminiAdapter(f.keys["GOOGLE_API_KEY"]), bare, nil
		default:
			return nil, "", fmt.Errorf("unknown provider prefix %q", prefix)
		}
	}

	// Convention-based routing by model name prefix
	switch {
	case strings.HasPrefix(modelName, "claude-"):
		return NewAnthropicAdapter(f.keys["ANTHROPIC_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "gpt-") ||
		strings.HasPrefix(modelName, "o1-") ||
		strings.HasPrefix(modelName, "o3-") ||
		strings.HasPrefix(modelName, "o4-"):
		return NewOpenAIAdapter(f.keys["OPENAI_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "codex-"):
		return NewCodexAdapter(f.keys["OPENAI_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "gemini-"):
		return NewGeminiAdapter(f.keys["GOOGLE_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "grok-"):
		return NewXAIAdapter(f.keys["XAI_API_KEY"]), modelName, nil

	// Groq-hosted open models — route to Groq by default for speed
	case strings.HasPrefix(modelName, "llama") ||
		strings.HasPrefix(modelName, "mixtral") ||
		strings.HasPrefix(modelName, "gemma"):
		return NewGroqAdapter(f.keys["GROQ_API_KEY"]), modelName, nil

	// Qwen and other models available via Cerebras (managed default,
	// unchanged — the "qwen/"/"alibaba/" prefixes select Alibaba explicitly).
	case strings.HasPrefix(modelName, "qwen"):
		return NewCerebrasAdapter(f.keys["CEREBRAS_API_KEY"]), modelName, nil

	// Official DeepSeek API ids only (see the BYOK variant for why r1 is out).
	case modelName == "deepseek-chat" || modelName == "deepseek-reasoner":
		return NewDeepSeekAdapter(f.keys["DEEPSEEK_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "glm"):
		return NewZhipuAdapter(f.keys["ZHIPU_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "kimi-") || strings.HasPrefix(modelName, "moonshot"):
		return NewMoonshotAdapter(f.keys["MOONSHOT_API_KEY"]), modelName, nil

	case strings.HasPrefix(strings.ToLower(modelName), "minimax"):
		return NewMiniMaxAdapter(f.keys["MINIMAX_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "mistral-") || strings.HasPrefix(modelName, "codestral-"):
		return NewMistralAdapter(f.keys["MISTRAL_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "sonar-") || modelName == "sonar":
		return NewPerplexityAdapter(f.keys["PERPLEXITY_API_KEY"]), modelName, nil

	case strings.HasPrefix(modelName, "command-"):
		return NewCohereAdapter(f.keys["COHERE_API_KEY"]), modelName, nil

	case strings.HasPrefix(strings.ToLower(modelName), "muse-spark"):
		return NewMetaAdapter(f.keys["META_API_KEY"]), modelName, nil

	default:
		return nil, "", fmt.Errorf("no provider matched model %q — use a routing prefix (e.g. groq/%s)", modelName, modelName)
	}
}
