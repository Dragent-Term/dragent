package providers

import (
	"testing"

	"server/internal/platform/netguard"
)

func TestForModelWithUserKeyGuardsCustomBaseURL(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	f := testFactory()
	for _, raw := range []string{"http://169.254.169.254/v1", "https://u:p@example.com/v1", "ftp://example.com"} {
		if _, _, err := f.ForModelWithUserKey("llama3", "sk", raw); err == nil {
			t.Errorf("ForModelWithUserKey accepted %q", raw)
		}
	}
	p, _, err := f.ForModelWithUserKey("llama3", "sk", "https://llm.example.com/v1")
	if err != nil {
		t.Fatal(err)
	}
	if c := p.(*OpenAICompatAdapter).httpClient; c.Transport != netguard.SharedTransport() || c.CheckRedirect == nil {
		t.Fatal("custom base URL adapter does not use the guarded client")
	}
	// ollama/ and lmstudio/ prefixes from a user request default to the
	// server's own localhost, so they must also go through the guard.
	for _, model := range []string{"ollama/llama3", "lmstudio/qwen"} {
		p, _, err := f.ForModelWithUserKey(model, "sk", "")
		if err != nil {
			t.Fatal(err)
		}
		if p.(*OpenAICompatAdapter).httpClient.Transport != netguard.SharedTransport() {
			t.Errorf("%s from user request is not guarded", model)
		}
	}
}

func TestForModelOperatorOllamaURLIsTrusted(t *testing.T) {
	f := NewProviderFactory(map[string]string{
		"OLLAMA_BASE_URL":    "http://10.0.1.20:11434/v1",
		"LM_STUDIO_BASE_URL": "http://10.0.1.21:1234/v1",
	})
	for _, model := range []string{"ollama/llama3", "lmstudio/qwen"} {
		p, _, err := f.ForModel(model)
		if err != nil {
			t.Fatal(err)
		}
		if p.(*OpenAICompatAdapter).httpClient.Transport == netguard.SharedTransport() {
			t.Errorf("%s: operator-configured URL should not be SSRF-guarded", model)
		}
	}
}
