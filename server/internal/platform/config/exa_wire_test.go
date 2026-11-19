package config

import (
	"os"
	"testing"

	"server/internal/agent/webtools"
)

// Guards the full env -> Config -> providerKeys -> webtools.Config chain for the
// Exa key. Before this, EXA_API_KEY was set on the host but never entered the
// key map, so every web_search silently fell back to the model provider.
func TestExaKeyReachesWebtools(t *testing.T) {
	// Load() requires these; set the minimum so it reaches the key parsing.
	os.Setenv("DATABASE_URL", "postgres://localhost/test")
	os.Setenv("JWT_SECRET", "test-secret-test-secret-test-sec")
	defer func() {
		os.Unsetenv("DATABASE_URL")
		os.Unsetenv("JWT_SECRET")
	}()

	os.Setenv("EXA_API_KEY", "exa-sentinel")
	os.Setenv("GEMINI_API_KEY", "gem-sentinel")
	os.Unsetenv("GOOGLE_API_KEY")
	defer func() {
		os.Unsetenv("EXA_API_KEY")
		os.Unsetenv("GEMINI_API_KEY")
	}()

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if cfg.ExaAPIKey != "exa-sentinel" {
		t.Fatalf("Config.ExaAPIKey = %q, want exa-sentinel", cfg.ExaAPIKey)
	}
	// GEMINI_API_KEY is what deployed hosts actually export.
	if cfg.GoogleAPIKey != "gem-sentinel" {
		t.Fatalf("Config.GoogleAPIKey = %q, want gem-sentinel", cfg.GoogleAPIKey)
	}

	// Mirrors the map built in cmd/api/main.go.
	wt := webtools.ConfigFromKeys(map[string]string{
		"EXA_API_KEY":    cfg.ExaAPIKey,
		"GOOGLE_API_KEY": cfg.GoogleAPIKey,
	})
	if wt.ExaKey != "exa-sentinel" {
		t.Fatalf("webtools ExaKey = %q, want exa-sentinel", wt.ExaKey)
	}
	if wt.GoogleKey != "gem-sentinel" {
		t.Fatalf("webtools GoogleKey = %q, want gem-sentinel", wt.GoogleKey)
	}
}
