package providers

import (
	"strings"
	"testing"
)

func testFactory() *ProviderFactory {
	return NewProviderFactory(map[string]string{
		"ANTHROPIC_API_KEY": "sk-ant",
		"OPENAI_API_KEY":    "sk-openai",
		"GROQ_API_KEY":      "gsk",
		"CEREBRAS_API_KEY":  "csk",
		"META_API_KEY":      "sk-meta",
		"DEEPSEEK_API_KEY":  "sk-ds",
		"ALIBABA_API_KEY":   "sk-ali",
		"ZHIPU_API_KEY":     "sk-zhipu",
		"MOONSHOT_API_KEY":  "sk-ms",
		"MINIMAX_API_KEY":   "sk-mm",
	})
}

func TestForModelFirstPartyChineseProviders(t *testing.T) {
	f := testFactory()
	cases := []struct {
		model    string
		provider string
		bare     string
	}{
		{"deepseek-chat", "deepseek", "deepseek-chat"},
		{"deepseek-reasoner", "deepseek", "deepseek-reasoner"},
		{"deepseek/deepseek-chat", "deepseek", "deepseek-chat"},
		{"qwen/qwen-max", "alibaba", "qwen-max"},
		{"alibaba/qwen-plus", "alibaba", "qwen-plus"},
		{"glm/glm-4-plus", "zhipu", "glm-4-plus"},
		{"zhipu/glm-4", "zhipu", "glm-4"},
		{"glm-4-plus", "zhipu", "glm-4-plus"},
		{"kimi-k2", "moonshot", "kimi-k2"},
		{"moonshot/kimi-k2", "moonshot", "kimi-k2"},
		{"MiniMax-Text-01", "minimax", "MiniMax-Text-01"},
		{"minimax/MiniMax-Text-01", "minimax", "MiniMax-Text-01"},
		{"mistral/mistral-large-latest", "mistral", "mistral-large-latest"},
		{"mistral-large-latest", "mistral", "mistral-large-latest"},
		{"codestral-latest", "mistral", "codestral-latest"},
		{"together/deepseek-ai/DeepSeek-V3", "together", "deepseek-ai/DeepSeek-V3"},
		{"deepinfra/meta-llama/Llama-4", "deepinfra", "meta-llama/Llama-4"},
		{"sonar-pro", "perplexity", "sonar-pro"},
		{"perplexity/sonar", "perplexity", "sonar"},
		{"venice/llama-3.3-70b", "venice", "llama-3.3-70b"},
		{"nvidia/meta-llama/llama-3.3-70b-instruct", "nvidia", "meta-llama/llama-3.3-70b-instruct"},
		{"fireworks/deepseek-ai/DeepSeek-R1", "fireworks", "deepseek-ai/DeepSeek-R1"},
		{"command-a", "cohere", "command-a"},
		{"cohere/command-r-plus", "cohere", "command-r-plus"},
	}
	for _, c := range cases {
		p, bare, err := f.ForModel(c.model)
		if err != nil {
			t.Fatalf("ForModel(%q) error: %v", c.model, err)
		}
		if p.ProviderName() != c.provider {
			t.Errorf("ForModel(%q) provider = %q, want %q", c.model, p.ProviderName(), c.provider)
		}
		if bare != c.bare {
			t.Errorf("ForModel(%q) bare = %q, want %q", c.model, bare, c.bare)
		}
	}
}

func TestForModelPreservesExistingRouting(t *testing.T) {
	f := testFactory()
	// Bare qwen* stays Cerebras-hosted (managed default, unchanged).
	p, _, err := f.ForModel("qwen2.5-coder")
	if err != nil {
		t.Fatalf("ForModel(qwen2.5-coder) error: %v", err)
	}
	if p.ProviderName() != "cerebras" {
		t.Errorf("bare qwen provider = %q, want cerebras", p.ProviderName())
	}
	// Bare local-style names must not route to a first-party API.
	// Bare hosted-namespace ids stay prefix-only (ambiguous across hosts).
	// Bare llama names are never Meta's: lowercase matches the long-standing
	// Groq-hosted default (preserved below), anything else errors.
	for _, m := range []string{
		"deepseek-r1:8b", "deepseek-r1", "mystery-model-9",
		"meta-llama/Llama-4-Maverick", "Llama-4-Maverick",
	} {
		if _, _, err := f.ForModel(m); err == nil {
			t.Errorf("ForModel(%q) expected error, got provider", m)
		}
	}
	for _, m := range []string{"muse-spark-1.3", "muse-spark-1.1", "meta/muse-spark-1.3"} {
		p, bare, err := f.ForModel(m)
		if err != nil {
			t.Fatalf("ForModel(%q) error: %v", m, err)
		}
		if p.ProviderName() != "meta" {
			t.Errorf("ForModel(%q) provider = %q, want meta", m, p.ProviderName())
		}
		_ = bare
	}
	// Existing families untouched — including lowercase bare llama,
	// which has always meant Groq-hosted on this path.
	for model, want := range map[string]string{
		"claude-sonnet-4-6":       "anthropic",
		"gpt-5.6-luna":            "openai",
		"llama-3.3-70b-versatile": "groq",
	} {
		p, _, err := f.ForModel(model)
		if err != nil {
			t.Fatalf("ForModel(%q) error: %v", model, err)
		}
		if !strings.Contains(strings.ToLower(p.ProviderName()), want) {
			t.Errorf("ForModel(%q) provider = %q, want containing %q", model, p.ProviderName(), want)
		}
	}
}

func TestForModelWithUserKeyMirrorsNewProviders(t *testing.T) {
	f := testFactory()
	for model, want := range map[string]string{
		"deepseek-chat":       "deepseek",
		"qwen/qwen-max":       "alibaba",
		"glm-4-plus":          "zhipu",
		"muse-spark-1.3":      "meta",
		"meta/muse-spark-1.1": "meta",
		"kimi-k2":             "moonshot",
		"minimax/abc":         "minimax",
		"qwen2.5-coder":       "cerebras",
		"deepseek-r1:8b":      "",
	} {
		p, _, err := f.ForModelWithUserKey(model, "user-key", "")
		if want == "" {
			if err == nil {
				t.Errorf("ForModelWithUserKey(%q) expected error, got provider", model)
			}
			continue
		}
		if err != nil {
			t.Fatalf("ForModelWithUserKey(%q) error: %v", model, err)
		}
		if p.ProviderName() != want {
			t.Errorf("ForModelWithUserKey(%q) provider = %q, want %q", model, p.ProviderName(), want)
		}
	}
}
