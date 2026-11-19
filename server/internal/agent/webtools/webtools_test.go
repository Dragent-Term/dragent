package webtools

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestFamilyForModel(t *testing.T) {
	cases := map[string]string{
		"claude-opus-4-8":              "anthropic",
		"claude-haiku-4-5":             "anthropic",
		"gpt-5.4-mini":                 "openai",
		"o3-mini":                      "openai",
		"codex-latest":                 "openai",
		"gemini-2.5-pro":               "gemini",
		"google/gemini-2.5-flash":      "gemini",
		"grok-4":                       "xai",
		"groq/llama-3.3-70b-versatile": "",
		"openrouter/some/model":        "",
		"ollama/llama3":                "",
		"qwen-2.5-coder":               "",
	}
	for model, want := range cases {
		if got := FamilyForModel(model); got != want {
			t.Errorf("FamilyForModel(%q) = %q, want %q", model, got, want)
		}
	}
}

func TestResolvePrefersActiveFamilyThenFallsBack(t *testing.T) {
	cfg := Config{AnthropicKey: "sk-ant", OpenAIKey: "sk-oai"}

	// Active family keyed by the platform.
	e := NewExecutor(cfg, "claude-opus-4-8", "", "", true)
	if fam, key, byok := e.resolve(); fam != "anthropic" || key != "sk-ant" || byok {
		t.Fatalf("expected anthropic/platform key, got %s/%s byok=%v", fam, key, byok)
	}

	// BYOK key beats the platform key for the active family — and is flagged
	// so billing skips it (the user pays their provider directly).
	e = NewExecutor(cfg, "claude-opus-4-8", "sk-user", "", true)
	if fam, key, byok := e.resolve(); fam != "anthropic" || key != "sk-user" || !byok {
		t.Fatalf("expected anthropic/user key, got %s/%s byok=%v", fam, key, byok)
	}

	// Family without web tooling falls back through platform keys.
	e = NewExecutor(cfg, "groq/llama-3.3-70b-versatile", "gsk-user", "", true)
	if fam, key, byok := e.resolve(); fam != "anthropic" || key != "sk-ant" || byok {
		t.Fatalf("expected anthropic fallback, got %s/%s byok=%v", fam, key, byok)
	}

	// Custom endpoint: never send its key to the big providers.
	e = NewExecutor(cfg, "some-local-model", "local-key", "http://localhost:11434/v1", true)
	if fam, key, byok := e.resolve(); fam != "anthropic" || key != "sk-ant" || byok {
		t.Fatalf("expected anthropic fallback for custom endpoint, got %s/%s byok=%v", fam, key, byok)
	}

	// No keys anywhere → no routing.
	e = NewExecutor(Config{}, "groq/llama-3.3-70b-versatile", "", "", true)
	if fam, _, _ := e.resolve(); fam != "" {
		t.Fatalf("expected no family with no keys, got %s", fam)
	}
}

func TestExecutorNeverUsesPlatformKeysWhenFallbackIsDisabled(t *testing.T) {
	e := NewExecutor(Config{AnthropicKey: "platform", ExaKey: "platform-exa"},
		"claude-opus-4-8", "", "", false)
	if e.UsesPlatform() {
		t.Fatal("platform resolution must be disabled without user permission")
	}
	output, isErr, usage := e.Execute(context.Background(), "web_search", `{"query":"test"}`)
	if !isErr || usage != (Usage{}) {
		t.Fatalf("output=%q isErr=%v usage=%+v", output, isErr, usage)
	}
	if !strings.Contains(output, "not available") {
		t.Fatalf("unexpected error: %q", output)
	}
}

func TestBYOKIsInitialRouteEvenWhenExaIsConfigured(t *testing.T) {
	e := NewExecutor(Config{OpenAIKey: "platform-openai", ExaKey: "platform-exa"},
		"gpt-5.6-luna", "user-openai", "", true)
	if e.UsesPlatform() {
		t.Fatal("BYOK must be attempted before Exa or another platform credential")
	}
}

func TestBYOKAuthFallbackRequiresBillingAuthorization(t *testing.T) {
	e := NewExecutor(Config{ExaKey: "platform-exa"}, "gpt-5.6-luna", "bad-user-key", "", true)
	requests := 0
	e.client = &http.Client{Transport: roundTripFunc(func(req *http.Request) (*http.Response, error) {
		requests++
		if req.URL.Host != "api.openai.com" {
			t.Fatalf("platform provider was reached before billing authorization: %s", req.URL)
		}
		return &http.Response{
			StatusCode: http.StatusUnauthorized,
			Status:     "401 Unauthorized",
			Header:     make(http.Header),
			Body:       io.NopCloser(strings.NewReader(`{"error":{"message":"invalid api key"}}`)),
			Request:    req,
		}, nil
	})}

	gateCalls := 0
	out, isErr, usage := e.ExecuteGated(context.Background(), "web_search", `{"query":"test"}`, func() error {
		gateCalls++
		return errors.New("plan does not permit platform credits")
	})
	if !isErr || usage != (Usage{}) {
		t.Fatalf("output=%q isErr=%v usage=%+v", out, isErr, usage)
	}
	if gateCalls != 1 || requests != 1 {
		t.Fatalf("gate calls=%d provider requests=%d, want 1 BYOK request and no platform request", gateCalls, requests)
	}
	if !strings.Contains(out, "platform fallback was not authorized") {
		t.Fatalf("unexpected output: %q", out)
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(req *http.Request) (*http.Response, error) { return f(req) }

func TestCollectorsCountBillableSearches(t *testing.T) {
	// Anthropic: explicit server_tool_use counter.
	_, u := anthropicCollect(map[string]interface{}{
		"content": []interface{}{map[string]interface{}{"type": "text", "text": "hi"}},
		"usage": map[string]interface{}{
			"input_tokens": float64(100), "output_tokens": float64(50),
			"server_tool_use": map[string]interface{}{"web_search_requests": float64(2)},
		},
	})
	if u.SearchCount != 2 || u.InputTokens != 100 || u.OutputTokens != 50 {
		t.Fatalf("anthropic usage = %+v, want 2 searches, 100 in, 50 out", u)
	}

	// OpenAI: one web_search_call output item per billable invocation.
	_, u = openaiCollect(map[string]interface{}{
		"output": []interface{}{
			map[string]interface{}{"type": "web_search_call"},
			map[string]interface{}{"type": "web_search_call"},
			map[string]interface{}{"type": "message", "content": []interface{}{
				map[string]interface{}{"type": "output_text", "text": "hi"},
			}},
		},
		"usage": map[string]interface{}{"input_tokens": float64(10), "output_tokens": float64(5)},
	})
	if u.SearchCount != 2 {
		t.Fatalf("openai SearchCount = %d, want 2", u.SearchCount)
	}

	// Gemini: grounding metadata present → one grounded prompt billed.
	_, u = geminiCollect(map[string]interface{}{
		"candidates": []interface{}{map[string]interface{}{
			"content": map[string]interface{}{"parts": []interface{}{map[string]interface{}{"text": "hi"}}},
			"groundingMetadata": map[string]interface{}{
				"groundingChunks": []interface{}{
					map[string]interface{}{"web": map[string]interface{}{"uri": "https://x.dev"}},
				},
			},
		}},
	})
	if u.SearchCount != 1 {
		t.Fatalf("gemini SearchCount = %d, want 1", u.SearchCount)
	}

	// Gemini without grounding (e.g. url_context fetch) bills no search.
	_, u = geminiCollect(map[string]interface{}{
		"candidates": []interface{}{map[string]interface{}{
			"content": map[string]interface{}{"parts": []interface{}{map[string]interface{}{"text": "hi"}}},
		}},
	})
	if u.SearchCount != 0 {
		t.Fatalf("gemini fetch SearchCount = %d, want 0", u.SearchCount)
	}
}

func TestExecuteValidatesArgs(t *testing.T) {
	e := NewExecutor(Config{AnthropicKey: "sk-ant"}, "claude-opus-4-8", "", "", true)

	out, isErr, _ := e.Execute(context.Background(), "web_search", `{"query": ""}`)
	if !isErr || !strings.Contains(out, "query") {
		t.Fatalf("expected query-required error, got %q (err=%v)", out, isErr)
	}
	out, isErr, _ = e.Execute(context.Background(), "fetch_url", `{"url": "ftp://x"}`)
	if !isErr || !strings.Contains(out, "http") {
		t.Fatalf("expected scheme error, got %q (err=%v)", out, isErr)
	}
	out, isErr, _ = e.Execute(context.Background(), "web_search", `not json`)
	if !isErr {
		t.Fatalf("expected parse error, got %q", out)
	}
}

func TestExecuteWithNoKeysReturnsActionableError(t *testing.T) {
	e := NewExecutor(Config{}, "claude-opus-4-8", "", "", true)
	out, isErr, _ := e.Execute(context.Background(), "web_search", `{"query": "anything"}`)
	if !isErr || !strings.Contains(out, "server") {
		t.Fatalf("expected server-config error, got %q (err=%v)", out, isErr)
	}
}

func TestHandles(t *testing.T) {
	e := NewExecutor(Config{}, "claude-opus-4-8", "", "", true)
	if !e.Handles("web_search") || !e.Handles("fetch_url") || e.Handles("read_file") {
		t.Fatal("Handles() misclassifies tools")
	}
}

func TestClampToTokenBudgetMiddleElides(t *testing.T) {
	long := strings.Repeat("a", 100_000)
	clamped := clampToTokenBudget(long, 1_000)
	if len(clamped) >= 5_000 {
		t.Fatalf("clamped output too long: %d", len(clamped))
	}
	if !strings.Contains(clamped, "trimmed to ~1000 tokens") {
		t.Fatalf("missing elision marker: %q", clamped[:100])
	}
	if got := clampToTokenBudget("hello", 1_000); got != "hello" {
		t.Fatalf("short text must pass through, got %q", got)
	}
	// Multi-byte input must not be split mid-rune.
	uni := strings.Repeat("héllo wörld ", 10_000)
	for _, r := range clampToTokenBudget(uni, 500) {
		if r == '�' {
			t.Fatal("clamp split a multi-byte rune")
		}
	}
}

func TestFetchCacheTTL(t *testing.T) {
	c := &ttlCache{entries: make(map[string]ttlEntry)}
	c.put("k", "v")
	if v, ok := c.get("k"); !ok || v != "v" {
		t.Fatal("expected cache hit")
	}
	if _, ok := c.get("missing"); ok {
		t.Fatal("expected cache miss")
	}
}

// Exa serves search whenever its key is present, regardless of the task's model
// family — search quality must not depend on the user's model choice. It bills a
// flat fee per REQUEST with no model in the loop, so it reports no tokens.
func TestExaSearchServesAnyFamilyAndBillsPerRequest(t *testing.T) {
	var gotPath, gotKey string
	var gotBody map[string]interface{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path
		gotKey = r.Header.Get("x-api-key")
		_ = json.NewDecoder(r.Body).Decode(&gotBody)
		_, _ = w.Write([]byte(`{"results":[
			{"title":"Result One","url":"https://example.com/a","highlights":["first excerpt"]},
			{"title":"Result Two","url":"https://example.com/b","highlights":["second excerpt"]}
		]}`))
	}))
	defer srv.Close()

	e := NewExecutor(Config{ExaKey: "test-key"}, "claude-opus-4-8", "", "", true)
	text, usage, err := e.exaSearchAt(context.Background(), srv.URL, "some query", 5)
	if err != nil {
		t.Fatalf("exaSearch: %v", err)
	}
	if gotPath != "/" || gotKey != "test-key" {
		t.Fatalf("request path=%q key=%q", gotPath, gotKey)
	}
	if gotBody["type"] != exaSearchType {
		t.Errorf("type = %v, want %q", gotBody["type"], exaSearchType)
	}
	if c, ok := gotBody["contents"].(map[string]interface{}); !ok || c["highlights"] != true {
		t.Errorf("contents = %v, want highlights:true", gotBody["contents"])
	}
	for _, want := range []string{"Result One", "https://example.com/a", "first excerpt", "second excerpt"} {
		if !strings.Contains(text, want) {
			t.Errorf("output missing %q:\n%s", want, text)
		}
	}
	// One request = one billable unit, and no model ran.
	if usage.SearchCount != 1 || usage.Family != "exa" {
		t.Errorf("usage = %+v, want 1 search on family exa", usage)
	}
	if usage.InputTokens != 0 || usage.OutputTokens != 0 {
		t.Errorf("usage tokens = %d/%d, want 0/0 (no model in the loop)",
			usage.InputTokens, usage.OutputTokens)
	}
}

// numResults is clamped: Exa bills per request, but an unbounded count would
// still bloat the digest handed back to the conversation.
func TestExaSearchClampsResultCount(t *testing.T) {
	var got map[string]interface{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewDecoder(r.Body).Decode(&got)
		_, _ = w.Write([]byte(`{"results":[]}`))
	}))
	defer srv.Close()
	e := NewExecutor(Config{ExaKey: "k"}, "gpt-5.6-luna", "", "", true)

	for _, tc := range []struct{ in, want float64 }{{0, 5}, {99, 10}, {7, 7}} {
		if _, _, err := e.exaSearchAt(context.Background(), srv.URL, "q", int(tc.in)); err != nil {
			t.Fatalf("exaSearch(%v): %v", tc.in, err)
		}
		if got["numResults"] != tc.want {
			t.Errorf("numResults for %v = %v, want %v", tc.in, got["numResults"], tc.want)
		}
	}
}

// fetch_url goes through Exa's /contents when a key is present. Unlike /search,
// the content options are TOP-LEVEL there, and `text` is requested rather than
// highlights — a fetch has no query to excerpt against.
func TestExaFetchRequestsTopLevelText(t *testing.T) {
	var got map[string]interface{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewDecoder(r.Body).Decode(&got)
		_, _ = w.Write([]byte(`{"results":[{"title":"Doc","url":"https://example.com/x","text":"page body"}]}`))
	}))
	defer srv.Close()

	e := NewExecutor(Config{ExaKey: "k"}, "claude-opus-4-8", "", "", true)
	text, usage, err := e.exaFetchAt(context.Background(), srv.URL, "https://example.com/x")
	if err != nil {
		t.Fatalf("exaFetch: %v", err)
	}
	// `text` must be top-level on /contents, NOT nested under "contents".
	if _, nested := got["contents"]; nested {
		t.Error("sent a nested contents object; /contents takes text at the top level")
	}
	txt, ok := got["text"].(map[string]interface{})
	if !ok {
		t.Fatalf("text option = %v, want an object with maxCharacters", got["text"])
	}
	if txt["maxCharacters"] != float64(fetchResultTokenBudget*4) {
		t.Errorf("maxCharacters = %v, want %d", txt["maxCharacters"], fetchResultTokenBudget*4)
	}
	if urls, _ := got["urls"].([]interface{}); len(urls) != 1 || urls[0] != "https://example.com/x" {
		t.Errorf("urls = %v", got["urls"])
	}
	for _, want := range []string{"Doc", "page body", "https://example.com/x"} {
		if !strings.Contains(text, want) {
			t.Errorf("output missing %q:\n%s", want, text)
		}
	}
	// Billed per request, same as search, and no model ran.
	if usage.SearchCount != 1 || usage.Family != "exa" || usage.InputTokens != 0 {
		t.Errorf("usage = %+v, want 1 request on exa with no tokens", usage)
	}
}
