package api

import (
	"encoding/json"
	"net/http"

	agentproviders "server/internal/agent/providers"
)

// ModelInfo describes a single model offered by a provider.
type ModelInfo struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	// SupportsEffort reports whether this model accepts a reasoning-effort
	// level under the request shape the agent pipeline sends. The client uses
	// it to show or hide the Low/Medium/High control — the server is the single
	// source of truth so the two can't drift.
	SupportsEffort bool `json:"supports_effort"`
}

// ProviderInfo describes one LLM provider and whether the server has a key for it.
type ProviderInfo struct {
	ID        string      `json:"id"`
	Name      string      `json:"name"`
	Available bool        `json:"available"`
	Models    []ModelInfo `json:"models"`
}

// ModelsResponse is the JSON body returned by GET /api/models.
type ModelsResponse struct {
	Providers []ProviderInfo `json:"providers"`
}

// availableModels returns the list of providers the server can use and which
// have a configured server-side key.  The client uses this to decide which
// provider cards to mark as "Server key".
func (h *Handler) availableModels(w http.ResponseWriter, r *http.Request) {
	// Require authentication.
	if r.Context().Value("user_id") == nil {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}

	type catalogEntry struct {
		id     string
		name   string
		envKey string
		models []ModelInfo
	}

	// Supported model catalog. Users without premium-capable funds are
	// automatically restricted to GPT-5.6 Luna by the server.
	catalog := []catalogEntry{
		{
			id:     "anthropic",
			name:   "Anthropic",
			envKey: "ANTHROPIC_API_KEY",
			models: []ModelInfo{
				{ID: "claude-opus-4-8", Name: "Claude Opus 4.8"},
				{ID: "claude-sonnet-4-6", Name: "Claude Sonnet 4.6"},
				{ID: "claude-haiku-4-5", Name: "Claude Haiku 4.5"},
			},
		},
		{
			id:     "openai",
			name:   "OpenAI",
			envKey: "OPENAI_API_KEY",
			models: []ModelInfo{
				{ID: "gpt-5.6-sol", Name: "GPT-5.6 Sol"},
				{ID: "gpt-5.6-terra", Name: "GPT-5.6 Terra"},
				{ID: "gpt-5.6-luna", Name: "GPT-5.6 Luna (Free tier)"},
				{ID: "gpt-5.5", Name: "GPT-5.5"},
				{ID: "gpt-5.4", Name: "GPT-5.4"},
				{ID: "gpt-5.4-mini", Name: "GPT-5.4 mini"},
			},
		},
		{
			id:     "google",
			name:   "Google Gemini",
			envKey: "GOOGLE_API_KEY",
			models: []ModelInfo{
				{ID: "gemini-2.5-flash", Name: "Gemini 2.5 Flash"},
				{ID: "gemini-3.5-flash", Name: "Gemini 3.5 Flash"},
				{ID: "gemini-3.1-pro", Name: "Gemini 3.1 Pro"},
			},
		},
		{
			id:     "groq",
			name:   "Groq",
			envKey: "GROQ_API_KEY",
			models: []ModelInfo{
				{ID: "groq/llama-4-scout-17b-16e-instruct", Name: "Llama 4 Scout (Groq)"},
			},
		},
		{
			id:     "deepseek",
			name:   "DeepSeek",
			envKey: "DEEPSEEK_API_KEY",
			models: []ModelInfo{
				{ID: "deepseek-chat", Name: "DeepSeek V3"},
				{ID: "deepseek-reasoner", Name: "DeepSeek R1"},
			},
		},
		{
			id:     "qwen",
			name:   "Alibaba Qwen",
			envKey: "ALIBABA_API_KEY",
			models: []ModelInfo{
				{ID: "qwen-max", Name: "Qwen Max"},
				{ID: "qwen-plus", Name: "Qwen Plus"},
			},
		},
		{
			id:     "zhipu",
			name:   "Zhipu GLM",
			envKey: "ZHIPU_API_KEY",
			models: []ModelInfo{
				{ID: "glm-4-plus", Name: "GLM-4 Plus"},
			},
		},
		{
			id:     "moonshot",
			name:   "Moonshot",
			envKey: "MOONSHOT_API_KEY",
			models: []ModelInfo{
				{ID: "kimi-k2", Name: "Kimi K2"},
			},
		},
		{
			id:     "minimax",
			name:   "MiniMax",
			envKey: "MINIMAX_API_KEY",
			models: []ModelInfo{
				{ID: "MiniMax-Text-01", Name: "MiniMax Text 01"},
			},
		},
		{
			id:     "mistral",
			name:   "Mistral",
			envKey: "MISTRAL_API_KEY",
			models: []ModelInfo{
				{ID: "mistral-large-latest", Name: "Mistral Large"},
				{ID: "codestral-latest", Name: "Codestral"},
			},
		},
		{
			id:     "together",
			name:   "Together AI",
			envKey: "TOGETHER_API_KEY",
			models: []ModelInfo{
				{ID: "together/deepseek-ai/DeepSeek-V3", Name: "DeepSeek V3 (Together)"},
			},
		},
		{
			id:     "deepinfra",
			name:   "DeepInfra",
			envKey: "DEEPINFRA_API_KEY",
			models: []ModelInfo{
				{ID: "deepinfra/meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8", Name: "Llama 4 Maverick (DeepInfra)"},
			},
		},
		{
			id:     "perplexity",
			name:   "Perplexity",
			envKey: "PERPLEXITY_API_KEY",
			models: []ModelInfo{
				{ID: "sonar-pro", Name: "Sonar Pro (native search built in)"},
			},
		},
		{
			id:     "venice",
			name:   "Venice",
			envKey: "VENICE_API_KEY",
			models: []ModelInfo{
				{ID: "venice/llama-3.3-70b", Name: "Llama 3.3 70B (Venice)"},
			},
		},
		{
			id:     "nvidia",
			name:   "NVIDIA NIM",
			envKey: "NVIDIA_API_KEY",
			models: []ModelInfo{
				{ID: "nvidia/meta-llama/llama-3.3-70b-instruct", Name: "Llama 3.3 70B (NIM)"},
			},
		},
		{
			id:     "fireworks",
			name:   "Fireworks",
			envKey: "FIREWORKS_API_KEY",
			models: []ModelInfo{
				{ID: "fireworks/deepseek-ai/DeepSeek-R1", Name: "DeepSeek R1 (Fireworks)"},
			},
		},
		{
			id:     "cohere",
			name:   "Cohere",
			envKey: "COHERE_API_KEY",
			models: []ModelInfo{
				{ID: "command-a", Name: "Command A"},
			},
		},
		{
			id:     "meta",
			name:   "Meta Muse Spark",
			envKey: "META_API_KEY",
			models: []ModelInfo{
				{ID: "muse-spark-1.3", Name: "Muse Spark 1.3"},
				{ID: "muse-spark-1.1", Name: "Muse Spark 1.1"},
			},
		},
		{
			id:     "xai",
			name:   "xAI",
			envKey: "XAI_API_KEY",
			models: []ModelInfo{
				{ID: "grok-4.3", Name: "Grok 4.3"},
				{ID: "grok-build-0.1", Name: "Grok Build 0.1"},
			},
		},
	}

	// Resolve effort capability before the `providers` local shadows the
	// package of the same name below.
	for _, p := range catalog {
		for i := range p.models {
			p.models[i].SupportsEffort = agentproviders.ModelSupportsEffort(p.models[i].ID)
		}
	}

	providers := make([]ProviderInfo, 0, len(catalog))
	for _, p := range catalog {
		providers = append(providers, ProviderInfo{
			ID:        p.id,
			Name:      p.name,
			Available: h.providerKeys[p.envKey] != "",
			Models:    p.models,
		})
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(ModelsResponse{Providers: providers})
}
