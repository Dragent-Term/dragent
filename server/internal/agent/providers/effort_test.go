package providers

import (
	"encoding/json"
	"testing"
)

func TestModelSupportsEffort(t *testing.T) {
	cases := []struct {
		model string
		want  bool
		why   string
	}{
		// ── Anthropic: output_config.effort ──
		{"claude-opus-4-8", true, "Opus 4.8"},
		{"claude-opus-4-7", true, "Opus 4.7"},
		// Opus 4.5 DOES take low/medium/high — it only lacks xhigh/max.
		{"claude-opus-4-5", true, "Opus 4.5 supports low/medium/high"},
		{"claude-sonnet-4-6", true, "Sonnet 4.6"},
		{"claude-fable-5", true, "Fable 5"},
		// Haiku 4.5 / Sonnet 4.5 hard-reject the parameter.
		{"claude-haiku-4-5", false, "Haiku 4.5 rejects effort"},
		{"claude-sonnet-4-5", false, "Sonnet 4.5 rejects effort"},

		// ── OpenAI: reasoning_effort ──
		{"gpt-5.6-luna", false, "gpt-5.6 + tools pins reasoning_effort=none"},
		{"gpt-5.6-terra", false, "gpt-5.6 + tools pins reasoning_effort=none"},
		{"gpt-5.6-sol", false, "gpt-5.6 + tools pins reasoning_effort=none"},
		{"gpt-5.5", true, "gpt-5.5"},
		{"gpt-5.4-mini", true, "gpt-5.4-mini"},

		// ── Google: generationConfig.thinkingConfig.thinkingLevel ──
		{"gemini-3.1-pro", true, "Gemini 3.1 Pro takes low/medium/high"},
		{"gemini-3.5-flash", true, "Gemini 3.5 Flash"},
		// 2.5 predates thinkingLevel (legacy thinkingBudget only).
		{"gemini-2.5-flash", false, "Gemini 2.5 has no thinkingLevel"},

		// ── xAI: reasoning_effort ──
		{"grok-4.3", true, "grok-4.3 takes none/low/medium/high"},
		{"grok-4.5", true, "grok-4.5 takes low/medium/high"},
		// Plain grok-4 ERRORS on the parameter — the allowlist must not widen
		// to a bare "grok-4" prefix.
		{"grok-4", false, "plain grok-4 errors on reasoning_effort"},
		{"grok-build-0.1", false, "unverified"},

		// ── Meta Muse Spark: reasoning_effort ──
		{"muse-spark-1.3", true, "Muse Spark takes low/medium/high"},
		{"muse-spark-1.1", true, "Muse Spark takes low/medium/high"},

		// ── Unverified / no mapping ──
		{"llama-4-scout-17b-16e-instruct", false, "Groq/Llama"},
		{"", false, "empty model name"},
	}
	for _, c := range cases {
		if got := ModelSupportsEffort(c.model); got != c.want {
			t.Errorf("ModelSupportsEffort(%q) = %v, want %v (%s)", c.model, got, c.want, c.why)
		}
	}
}

func TestModelSupportsEffortIsCaseAndSpaceInsensitive(t *testing.T) {
	if !ModelSupportsEffort("  Claude-Opus-4-8  ") {
		t.Error("expected trimming + lowercasing to normalise the model name")
	}
}

func TestResolveEffortDropsUnsupportedPairings(t *testing.T) {
	cases := []struct {
		model string
		in    Effort
		want  Effort
	}{
		{"claude-opus-4-8", EffortLow, EffortLow},
		{"claude-opus-4-8", EffortHigh, EffortHigh},
		{"claude-opus-4-8", "", ""},
		// Unsupported model → dropped, NOT an error: the task still runs at the
		// provider default instead of failing.
		{"claude-haiku-4-5", EffortHigh, ""},
		{"gpt-5.6-luna", EffortLow, ""},
		// Garbage level → dropped.
		{"claude-opus-4-8", Effort("ultra"), ""},
		{"claude-opus-4-8", Effort("xhigh"), ""},
	}
	for _, c := range cases {
		if got := ResolveEffort(c.model, c.in); got != c.want {
			t.Errorf("ResolveEffort(%q, %q) = %q, want %q", c.model, c.in, got, c.want)
		}
	}
}

// Anthropic nests effort under output_config — a top-level "effort" key is
// silently ignored by the API, so assert the exact nesting.
func TestAnthropicBodyNestsEffortUnderOutputConfig(t *testing.T) {
	a := NewAnthropicAdapter("test-key")
	raw, err := a.buildBody(StepRequest{
		ModelName: "claude-opus-4-8",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Effort:    EffortLow,
	})
	if err != nil {
		t.Fatalf("buildBody: %v", err)
	}
	var body map[string]interface{}
	if err := json.Unmarshal(raw, &body); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if _, topLevel := body["effort"]; topLevel {
		t.Error("effort must NOT be a top-level field on Anthropic requests")
	}
	oc, ok := body["output_config"].(map[string]interface{})
	if !ok {
		t.Fatalf("output_config missing or wrong type: %#v", body["output_config"])
	}
	if oc["effort"] != "low" {
		t.Errorf("output_config.effort = %v, want low", oc["effort"])
	}
}

func TestAnthropicBodyOmitsEffortForHaiku(t *testing.T) {
	a := NewAnthropicAdapter("test-key")
	raw, err := a.buildBody(StepRequest{
		ModelName: "claude-haiku-4-5",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Effort:    EffortHigh,
	})
	if err != nil {
		t.Fatalf("buildBody: %v", err)
	}
	var body map[string]interface{}
	if err := json.Unmarshal(raw, &body); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if _, present := body["output_config"]; present {
		t.Error("Haiku 4.5 rejects effort — output_config must be absent")
	}
}

// The gpt-5.6 "none" pin must survive a user-selected effort, otherwise every
// tool-using task on that family fails.
func TestOpenAIBodyKeepsNonePinForGPT56WithTools(t *testing.T) {
	o := NewOpenAIAdapter("test-key")
	raw, err := o.buildBody(StepRequest{
		ModelName: "gpt-5.6-luna",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Tools:     []ToolDefinition{{Name: "t", Description: "d", Parameters: map[string]interface{}{}}},
		Effort:    EffortHigh,
	})
	if err != nil {
		t.Fatalf("buildBody: %v", err)
	}
	var body map[string]interface{}
	if err := json.Unmarshal(raw, &body); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if body["reasoning_effort"] != "none" {
		t.Errorf("reasoning_effort = %v, want none (user effort must not override the pin)", body["reasoning_effort"])
	}
}

func TestOpenAIBodySetsEffortForSupportedModel(t *testing.T) {
	o := NewOpenAIAdapter("test-key")
	raw, err := o.buildBody(StepRequest{
		ModelName: "gpt-5.5",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Effort:    EffortMedium,
	})
	if err != nil {
		t.Fatalf("buildBody: %v", err)
	}
	var body map[string]interface{}
	if err := json.Unmarshal(raw, &body); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if body["reasoning_effort"] != "medium" {
		t.Errorf("reasoning_effort = %v, want medium", body["reasoning_effort"])
	}
}

// Gemini's generateContent API nests this under generationConfig.thinkingConfig.
// The flat generation_config.thinking_level shape belongs to the separate
// Interactions API and is silently ignored here — assert the exact path.
func TestGeminiBodyNestsThinkingLevel(t *testing.T) {
	g := NewGeminiAdapter("test-key")
	raw, err := g.buildBody(StepRequest{
		ModelName: "gemini-3.1-pro",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Effort:    EffortMedium,
	})
	if err != nil {
		t.Fatalf("buildBody: %v", err)
	}
	var body map[string]interface{}
	if err := json.Unmarshal(raw, &body); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	gc, ok := body["generationConfig"].(map[string]interface{})
	if !ok {
		t.Fatalf("generationConfig missing: %#v", body)
	}
	tc, ok := gc["thinkingConfig"].(map[string]interface{})
	if !ok {
		t.Fatalf("generationConfig.thinkingConfig missing: %#v", gc)
	}
	if tc["thinkingLevel"] != "medium" {
		t.Errorf("thinkingLevel = %v, want medium", tc["thinkingLevel"])
	}
	// Mixing thinkingLevel with the legacy thinkingBudget is a 400.
	if _, bad := tc["thinkingBudget"]; bad {
		t.Error("thinkingBudget must not accompany thinkingLevel")
	}
}

// Gemini 2.5 has no thinkingLevel — sending one alongside its legacy budget
// surface is a 400, so it must be dropped.
func TestGeminiBodyOmitsThinkingLevelFor25(t *testing.T) {
	g := NewGeminiAdapter("test-key")
	raw, _ := g.buildBody(StepRequest{
		ModelName: "gemini-2.5-flash",
		Messages:  []Message{{Role: RoleUser, Content: "hi"}},
		Effort:    EffortHigh,
	})
	var body map[string]interface{}
	_ = json.Unmarshal(raw, &body)
	if gc, ok := body["generationConfig"].(map[string]interface{}); ok {
		if _, present := gc["thinkingConfig"]; present {
			t.Error("Gemini 2.5 must not receive thinkingConfig.thinkingLevel")
		}
	}
}

// xAI rides the OpenAI-compatible adapter, which also backs Ollama/LM Studio/
// Groq — none of which understand reasoning_effort. Verify it lands for a
// verified Grok model and stays off for the shared-adapter siblings.
func TestOpenAICompatEffortIsScopedToVerifiedModels(t *testing.T) {
	adapter := NewXAIAdapter("test-key")
	for _, c := range []struct {
		model string
		want  interface{}
	}{
		{"grok-4.3", "high"},
		{"grok-4.5", "high"},
		// Plain grok-4 errors on the parameter.
		{"grok-4", nil},
		{"llama-4-scout-17b-16e-instruct", nil},
		{"qwen2.5-coder", nil},
	} {
		raw, err := adapter.buildBody(StepRequest{
			ModelName: c.model,
			Messages:  []Message{{Role: RoleUser, Content: "hi"}},
			Effort:    EffortHigh,
		})
		if err != nil {
			t.Fatalf("%s: %v", c.model, err)
		}
		var body map[string]interface{}
		_ = json.Unmarshal(raw, &body)
		if body["reasoning_effort"] != c.want {
			t.Errorf("%s: reasoning_effort = %v, want %v", c.model, body["reasoning_effort"], c.want)
		}
	}
}
