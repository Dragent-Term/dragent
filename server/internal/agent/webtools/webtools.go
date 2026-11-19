// Package webtools executes the agent's web_search and fetch_url tools ON THE
// SERVER, routing through the active LLM provider's own web tooling — never a
// third-party search engine and never the client's machine:
//
//	| Provider  | Search                            | Fetch                       |
//	|-----------|-----------------------------------|-----------------------------|
//	| Anthropic | web_search_20250305               | web_fetch_20250910 (beta)   |
//	| OpenAI    | Responses web_search_preview      | Responses browse-the-URL    |
//	| Gemini    | google_search grounding           | url_context tool            |
//	| xAI/Grok  | Live Search (search_parameters)   | → Live Search on the URL    |
//
// Every call runs on the provider's SMALLEST fast model (Haiku / gpt-4o-mini /
// Flash / grok-3-mini), not the user's selected model — search/fetch digestion
// doesn't need a frontier model, and the digest is all that enters the main
// conversation. Results are clamped to a hard token budget (middle-elided) and
// each sub-call reports its own token usage so billing can settle it.
//
// Credentials: the active model's BYOK key is always attempted first. A
// platform-funded provider (including Exa) may only be used when fallback is
// enabled and the caller's just-in-time billing gate authorizes that request.
package webtools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"
)

// ── Token budgets ─────────────────────────────────────────────────────────────

// searchResultTokenBudget caps tokens a search result may inject into the conversation.
const searchResultTokenBudget = 2_000

// fetchResultTokenBudget caps tokens a fetched page may inject into the conversation.
const fetchResultTokenBudget = 6_000

// searchMaxTokens is the completion budget requested from the provider for a search.
const searchMaxTokens = 2_048

// fetchMaxTokens is the completion budget requested from the provider for a fetch.
const fetchMaxTokens = 6_144

const httpTimeout = 60 * time.Second

// approxTokens is the standard ≈4-chars/token cheap estimate, biased high.
func approxTokens(text string) int { return (len(text) + 3) / 4 }

// clampToTokenBudget trims text to ~budget tokens, middle-eliding so both the
// head and the tail survive (error messages and conclusions live at the ends).
func clampToTokenBudget(text string, budget int) string {
	maxBytes := budget * 4
	if len(text) <= maxBytes {
		return text
	}
	half := maxBytes / 2
	head := text[:half]
	tail := text[len(text)-half:]
	// Trim to valid UTF-8 boundaries.
	for len(head) > 0 && !isCharBoundary(text, len(head)) {
		head = head[:len(head)-1]
	}
	tailStart := len(text) - len(tail)
	for tailStart < len(text) && !isCharBoundary(text, tailStart) {
		tailStart++
	}
	return fmt.Sprintf("%s\n\n[… trimmed to ~%d tokens …]\n\n%s", head, budget, text[tailStart:])
}

func isCharBoundary(s string, i int) bool {
	return i == 0 || i == len(s) || (s[i]&0xC0) != 0x80
}

// ── Executor ──────────────────────────────────────────────────────────────────

// Config holds the platform's provider keys for web tooling.
type Config struct {
	AnthropicKey string
	OpenAIKey    string
	GoogleKey    string
	XAIKey       string
	// ExaKey enables the dedicated platform search provider. BYOK still takes
	// precedence; Exa is used for platform-funded calls and fallbacks.
	ExaKey string
}

// ConfigFromKeys builds a Config from the factory's flat key map.
func ConfigFromKeys(keys map[string]string) Config {
	return Config{
		AnthropicKey: keys["ANTHROPIC_API_KEY"],
		OpenAIKey:    keys["OPENAI_API_KEY"],
		GoogleKey:    keys["GOOGLE_API_KEY"],
		XAIKey:       keys["XAI_API_KEY"],
		ExaKey:       keys["EXA_API_KEY"],
	}
}

// Usage is the spend of one web-tool sub-call, reported for billing.
type Usage struct {
	InputTokens  int64
	OutputTokens int64
	// SearchCount is the number of billable provider searches the sub-call
	// performed (providers charge a flat per-search fee on top of tokens).
	// 0 for fetches and for providers that report no search activity.
	SearchCount int64
	// Family is the provider family that actually served the call (may differ
	// from the task's model family when the fallback chain kicked in) — it
	// decides which per-search fee applies.
	Family string
	// UserKeyUsed is true when the user's own BYOK key served the sub-call:
	// the user pays their provider directly, so the platform must not also
	// charge credits for it.
	UserKeyUsed bool
}

// Executor serves web_search / fetch_url calls for one task.
type Executor struct {
	cfg             Config
	family          string // provider family of the task's active model ("" = no native web tooling)
	userKey         string // BYOK key for that family; "" = use the platform key
	platformAllowed bool
	client          *http.Client
}

// NewExecutor builds an executor for a task. modelName is the task's model
// (routing-prefixed or bare); userKey is the BYOK key from X-User-Api-Key (may
// be empty); customBaseURL non-empty means an OpenAI-compatible custom endpoint
// whose key must not be sent to the big providers, so the family is treated as
// having no native web tooling.
func NewExecutor(cfg Config, modelName, userKey, customBaseURL string, platformAllowed bool) *Executor {
	family := FamilyForModel(modelName)
	if customBaseURL != "" {
		family = ""
		userKey = ""
	}
	return &Executor{
		cfg:             cfg,
		family:          family,
		userKey:         userKey,
		platformAllowed: platformAllowed,
		client:          &http.Client{Timeout: httpTimeout},
	}
}

// FamilyForModel maps a model name to its web-capable provider family, or ""
// when the provider has no native web tooling. Mirrors the factory's routing.
func FamilyForModel(modelName string) string {
	if idx := strings.Index(modelName, "/"); idx > 0 {
		if modelName[:idx] == "google" {
			return "gemini"
		}
		return "" // groq / openrouter / ollama / lmstudio / cerebras / azure
	}
	switch {
	case strings.HasPrefix(modelName, "claude-"):
		return "anthropic"
	case strings.HasPrefix(modelName, "gpt-"),
		strings.HasPrefix(modelName, "o1-"),
		strings.HasPrefix(modelName, "o3-"),
		strings.HasPrefix(modelName, "o4-"),
		strings.HasPrefix(modelName, "codex-"):
		return "openai"
	case strings.HasPrefix(modelName, "gemini-"):
		return "gemini"
	case strings.HasPrefix(modelName, "grok-"):
		return "xai"
	default:
		return ""
	}
}

// Handles reports whether the executor serves this tool name.
func (e *Executor) Handles(name string) bool {
	return name == "web_search" || name == "fetch_url"
}

// UsesPlatform reports whether the executor's initial route requires platform
// credentials. A BYOK call can still fall back later, subject to its gate.
func (e *Executor) UsesPlatform() bool {
	if e.family != "" && e.userKey != "" {
		return false
	}
	family, key, userKeyUsed := e.resolve()
	return e.platformAllowed && (e.cfg.ExaKey != "" || (family != "" && key != "" && !userKeyUsed))
}

// serverKeyFor returns the platform key for a family.
func (e *Executor) serverKeyFor(family string) string {
	switch family {
	case "anthropic":
		return e.cfg.AnthropicKey
	case "openai":
		return e.cfg.OpenAIKey
	case "gemini":
		return e.cfg.GoogleKey
	case "xai":
		return e.cfg.XAIKey
	}
	return ""
}

// resolve picks the provider family + key that should serve a web call:
// the active model's family first (BYOK key preferred, then platform key),
// then the platform fallback chain. userKeyUsed reports whether the BYOK key
// was chosen — those calls are paid by the user's own provider account and
// must not also be charged as credits.
func (e *Executor) resolve() (family, key string, userKeyUsed bool) {
	if e.family != "" {
		if e.userKey != "" {
			return e.family, e.userKey, true
		}
		if e.platformAllowed {
			if key := e.serverKeyFor(e.family); key != "" {
				return e.family, key, false
			}
		}
	}
	if !e.platformAllowed {
		return "", "", false
	}
	for _, fam := range []string{"anthropic", "openai", "gemini", "xai"} {
		if key := e.serverKeyFor(fam); key != "" {
			return fam, key, false
		}
	}
	return "", "", false
}

const noKeyMsg = "Web tools are not available: the server has no provider credentials " +
	"configured for web search/fetch. This is a server configuration issue, not something " +
	"the user can fix from the app."

// Execute runs a web tool call. The output is already clamped to its token
// budget; isError follows the tool-result convention (in-band error text).
func (e *Executor) Execute(ctx context.Context, name, argsJSON string) (string, bool, Usage) {
	return e.ExecuteGated(ctx, name, argsJSON, nil)
}

// ExecuteGated runs a web tool and calls authorizePlatform immediately before
// any platform-funded request. BYOK calls do not invoke the gate. If a BYOK
// authentication failure triggers fallback, a rejected gate prevents that
// fallback from reaching the provider.
func (e *Executor) ExecuteGated(ctx context.Context, name, argsJSON string, authorizePlatform func() error) (string, bool, Usage) {
	var args struct {
		Query      string `json:"query"`
		URL        string `json:"url"`
		MaxResults int    `json:"max_results"`
	}
	if err := json.Unmarshal([]byte(argsJSON), &args); err != nil {
		return fmt.Sprintf("%s: invalid arguments: %v", name, err), true, Usage{}
	}
	switch name {
	case "web_search":
		if strings.TrimSpace(args.Query) == "" {
			return "web_search: 'query' is required", true, Usage{}
		}
		return e.webSearch(ctx, args.Query, args.MaxResults, authorizePlatform)
	case "fetch_url":
		if !strings.HasPrefix(args.URL, "http://") && !strings.HasPrefix(args.URL, "https://") {
			return "fetch_url: URL must start with http:// or https://", true, Usage{}
		}
		return e.fetchURL(ctx, args.URL, authorizePlatform)
	}
	return fmt.Sprintf("unknown web tool: %s", name), true, Usage{}
}

func (e *Executor) webSearch(ctx context.Context, query string, maxResults int, authorizePlatform func() error) (string, bool, Usage) {
	// The user's provider tool is the first route. Only authentication failures
	// may cross from BYOK to platform credentials; ordinary provider errors are
	// returned as-is so fallback does not hide outages or spend credits silently.
	if e.family != "" && e.userKey != "" {
		text, usage, err := e.providerSearch(ctx, e.family, query, e.userKey)
		usage.UserKeyUsed = true
		if err == nil || !isProviderAuthError(err) || !e.platformAllowed {
			return finish("web_search", e.family, text, usage, err, searchResultTokenBudget, "Web search returned no content.")
		}
		log.Printf("[web-tools] BYOK %s authentication failed; requesting authorized platform fallback", e.family)
	}
	if !e.platformAllowed {
		return noKeyMsg, true, Usage{}
	}
	family, key, _ := e.resolvePlatform()
	if e.cfg.ExaKey == "" && family == "" {
		return noKeyMsg, true, Usage{}
	}
	if err := authorizePlatformCall(authorizePlatform); err != nil {
		return "web_search: platform fallback was not authorized: " + err.Error(), true, Usage{}
	}
	if e.cfg.ExaKey != "" {
		text, usage, err := e.exaSearch(ctx, query, maxResults)
		return finish("web_search", "exa", text, usage, err, searchResultTokenBudget, "Web search returned no results.")
	}
	text, usage, err := e.providerSearch(ctx, family, query, key)
	return finish("web_search", family, text, usage, err, searchResultTokenBudget, "Web search returned no content.")
}

func (e *Executor) providerSearch(ctx context.Context, family, query, key string) (string, Usage, error) {
	var text string
	var usage Usage
	var err error
	switch family {
	case "anthropic":
		text, usage, err = e.anthropicSearch(ctx, query, key)
	case "openai":
		text, usage, err = e.openaiSearch(ctx, query, key)
	case "gemini":
		text, usage, err = e.geminiSearch(ctx, query, key)
	case "xai":
		text, usage, err = e.xaiSearch(ctx, query, key)
	}
	return text, usage, err
}

func (e *Executor) fetchURL(ctx context.Context, url string, authorizePlatform func() error) (string, bool, Usage) {
	if hit, ok := fetchCache.get(url); ok {
		log.Printf("[web-tools] fetch_url cache hit — ~%d tokens", approxTokens(hit))
		return "[cached]\n" + hit, false, Usage{}
	}
	if e.family != "" && e.userKey != "" {
		text, usage, err := e.providerFetch(ctx, e.family, url, e.userKey)
		usage.UserKeyUsed = true
		if err == nil || !isProviderAuthError(err) || !e.platformAllowed {
			out, isErr, usage := finishFetch(e.family, text, usage, err, url)
			if !isErr {
				fetchCache.put(url, out)
			}
			return out, isErr, usage
		}
		log.Printf("[web-tools] BYOK %s authentication failed; requesting authorized platform fallback", e.family)
	}
	if !e.platformAllowed {
		return noKeyMsg, true, Usage{}
	}
	family, key, _ := e.resolvePlatform()
	if e.cfg.ExaKey == "" && family == "" {
		return noKeyMsg, true, Usage{}
	}
	if err := authorizePlatformCall(authorizePlatform); err != nil {
		return "fetch_url: platform fallback was not authorized: " + err.Error(), true, Usage{}
	}
	if e.cfg.ExaKey != "" {
		text, usage, err := e.exaFetch(ctx, url)
		out, isErr, u := finishFetch("exa", text, usage, err, url)
		if !isErr {
			fetchCache.put(url, out)
		}
		return out, isErr, u
	}
	text, usage, err := e.providerFetch(ctx, family, url, key)
	out, isErr, usage := finishFetch(family, text, usage, err, url)
	if !isErr {
		fetchCache.put(url, out)
	}
	return out, isErr, usage
}

func (e *Executor) providerFetch(ctx context.Context, family, url, key string) (string, Usage, error) {
	var text string
	var usage Usage
	var err error
	switch family {
	case "anthropic":
		text, usage, err = e.anthropicFetch(ctx, url, key)
	case "openai":
		text, usage, err = e.openaiFetch(ctx, url, key)
	case "gemini":
		text, usage, err = e.geminiFetch(ctx, url, key)
	case "xai":
		text, usage, err = e.xaiSearch(ctx, "Contents of the page at "+url, key)
	}
	return text, usage, err
}

func (e *Executor) resolvePlatform() (family, key string, userKeyUsed bool) {
	if e.family != "" {
		if key := e.serverKeyFor(e.family); key != "" {
			return e.family, key, false
		}
	}
	for _, fam := range []string{"anthropic", "openai", "gemini", "xai"} {
		if key := e.serverKeyFor(fam); key != "" {
			return fam, key, false
		}
	}
	return "", "", false
}

func authorizePlatformCall(gate func() error) error {
	if gate == nil {
		return nil
	}
	return gate()
}

func isProviderAuthError(err error) bool {
	if err == nil {
		return false
	}
	msg := strings.ToLower(err.Error())
	for _, marker := range []string{" 401", " 403", "unauthorized", "forbidden", "authentication", "invalid api key", "invalid_api_key", "incorrect api key"} {
		if strings.Contains(msg, marker) {
			return true
		}
	}
	return false
}

func finishFetch(family, text string, usage Usage, err error, url string) (string, bool, Usage) {
	return finish("fetch_url", family, text, usage, err, fetchResultTokenBudget,
		fmt.Sprintf("Fetch of %s returned no content.", url))
}

// finish applies the empty-result check, token clamp, and per-call log line.
func finish(tool, family, text string, usage Usage, err error, budget int, emptyMsg string) (string, bool, Usage) {
	usage.Family = family
	if err != nil {
		log.Printf("[web-tools] %s via %s FAILED: %v", tool, family, err)
		return err.Error(), true, usage
	}
	if strings.TrimSpace(text) == "" {
		return emptyMsg, true, usage
	}
	clamped := clampToTokenBudget(text, budget)
	log.Printf("[web-tools] %s via %s — ~%d tokens injected (sub-call in=%d out=%d searches=%d)",
		tool, family, approxTokens(clamped), usage.InputTokens, usage.OutputTokens, usage.SearchCount)
	return clamped, false, usage
}

// ── Fetch cache ───────────────────────────────────────────────────────────────
// Server-wide in-memory TTL cache so repeated fetches of the same page within
// a session (or across users hitting the same docs) don't re-spend sub-calls.

const fetchCacheTTL = 15 * time.Minute
const fetchCacheMaxEntries = 256

type ttlCache struct {
	mu      sync.Mutex
	entries map[string]ttlEntry
}

type ttlEntry struct {
	value    string
	storedAt time.Time
}

var fetchCache = &ttlCache{entries: make(map[string]ttlEntry)}

func (c *ttlCache) get(key string) (string, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	e, ok := c.entries[key]
	if !ok || time.Since(e.storedAt) > fetchCacheTTL {
		delete(c.entries, key)
		return "", false
	}
	return e.value, true
}

func (c *ttlCache) put(key, value string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.entries) >= fetchCacheMaxEntries {
		// Evict expired entries first; if none, evict the oldest.
		oldestKey, oldestAt := "", time.Now()
		for k, e := range c.entries {
			if time.Since(e.storedAt) > fetchCacheTTL {
				delete(c.entries, k)
				continue
			}
			if e.storedAt.Before(oldestAt) {
				oldestKey, oldestAt = k, e.storedAt
			}
		}
		if len(c.entries) >= fetchCacheMaxEntries && oldestKey != "" {
			delete(c.entries, oldestKey)
		}
	}
	c.entries[key] = ttlEntry{value: value, storedAt: time.Now()}
}

// ── HTTP plumbing ─────────────────────────────────────────────────────────────

func (e *Executor) postJSON(ctx context.Context, url string, headers map[string]string, body map[string]interface{}, providerLabel string) (map[string]interface{}, error) {
	payload, err := json.Marshal(body)
	if err != nil {
		return nil, fmt.Errorf("%s marshal error: %w", providerLabel, err)
	}
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(payload))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := e.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("%s network error: %w", providerLabel, err)
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return nil, fmt.Errorf("%s read error: %w", providerLabel, err)
	}
	if resp.StatusCode != http.StatusOK {
		msg := string(raw)
		if len(msg) > 400 {
			msg = msg[:400]
		}
		return nil, fmt.Errorf("%s error %d: %s", providerLabel, resp.StatusCode, msg)
	}
	var data map[string]interface{}
	if err := json.Unmarshal(raw, &data); err != nil {
		return nil, fmt.Errorf("%s parse error: %w", providerLabel, err)
	}
	return data, nil
}

// jsonPath walks nested maps/arrays: strings index maps, ints index arrays.
func jsonPath(v interface{}, path ...interface{}) interface{} {
	for _, p := range path {
		switch key := p.(type) {
		case string:
			m, ok := v.(map[string]interface{})
			if !ok {
				return nil
			}
			v = m[key]
		case int:
			arr, ok := v.([]interface{})
			if !ok || key >= len(arr) {
				return nil
			}
			v = arr[key]
		}
	}
	return v
}

func jsonString(v interface{}, path ...interface{}) string {
	s, _ := jsonPath(v, path...).(string)
	return s
}

func jsonInt64(v interface{}, path ...interface{}) int64 {
	f, _ := jsonPath(v, path...).(float64)
	return int64(f)
}

func appendSources(text string, sources []string) string {
	if len(sources) == 0 {
		return text
	}
	var sb strings.Builder
	sb.WriteString(text)
	sb.WriteString("\n\nSources:\n")
	for i, s := range sources {
		if i >= 8 {
			break
		}
		sb.WriteString("- " + s + "\n")
	}
	return sb.String()
}

func addUnique(list []string, item string) []string {
	for _, s := range list {
		if s == item {
			return list
		}
	}
	return append(list, item)
}

// ── Anthropic ─────────────────────────────────────────────────────────────────

// anthropicToolModel is the cheap, fast model for tool-serving sub-calls —
// the user's selected model may be a large one; search/fetch digestion
// doesn't need it.
const exaContentsURL = "https://api.exa.ai/contents"

// exaFetch retrieves one URL's cleaned page text through Exa's /contents.
//
// NOTE the shape difference from /search: on /contents the content options are
// TOP-LEVEL (`text`), whereas on /search they nest inside `contents`. Exa's own
// docs call this out as a common mistake.
//
// `text` (not `highlights`) is right here: fetch_url means "give me this page",
// so the caller wants the body, not query-relevant excerpts — there is no query.
// maxCharacters is set from the same token budget the result is clamped to, so
// we don't pay to transfer text that is about to be trimmed away.
func (e *Executor) exaFetch(ctx context.Context, url string) (string, Usage, error) {
	return e.exaFetchAt(ctx, exaContentsURL, url)
}

// exaFetchAt is exaFetch with the endpoint injected, for tests.
func (e *Executor) exaFetchAt(ctx context.Context, endpoint, url string) (string, Usage, error) {
	body := map[string]interface{}{
		"urls": []string{url},
		"text": map[string]interface{}{
			"maxCharacters": fetchResultTokenBudget * 4, // ≈4 chars/token
		},
	}
	data, err := e.postJSON(ctx, endpoint, map[string]string{"x-api-key": e.cfg.ExaKey}, body, "exa")
	if err != nil {
		return "", Usage{}, err
	}
	results, _ := jsonPath(data, "results").([]interface{})
	var out strings.Builder
	for _, raw := range results {
		item, ok := raw.(map[string]interface{})
		if !ok {
			continue
		}
		if title := jsonString(item, "title"); title != "" {
			fmt.Fprintf(&out, "# %s\n\n", title)
		}
		out.WriteString(jsonString(item, "text"))
	}
	// Billed per REQUEST, like search — see billing.WebSearchFeeMc("exa").
	usage := Usage{SearchCount: 1, Family: "exa"}
	return appendSources(strings.TrimSpace(out.String()), []string{url}), usage, nil
}

// ── Exa (dedicated search provider) ──────────────────────────────────────────
//
// Exa is a search API, not an LLM: it returns ranked results with
// query-relevant `highlights` directly. That removes the round trip the
// provider-native path needs — those ask a small model to run a search tool and
// digest the outcome, costing tokens and latency on every call. Here there is
// no model in the loop, so `Usage` carries no tokens: the whole cost is the
// flat per-request fee (see billing.WebSearchFeeMc, "exa").
//
// Reference: https://docs.exa.ai/reference/search-api-guide-for-coding-agents

const exaSearchURL = "https://api.exa.ai/search"

// exaSearchType is the balanced relevance/speed default (~1s). `fast` and
// `instant` trade relevance for latency; the `deep*` variants cost far more and
// take seconds — neither is right for an inline agent tool.
const exaSearchType = "auto"

// exaSearch runs one query against Exa and formats the ranked results.
//
// `numResults` is what we ask for; Exa bills per REQUEST, not per result, so
// asking for a few more costs nothing extra. The token budget applied by the
// caller is what actually bounds how much reaches the conversation.
func (e *Executor) exaSearch(ctx context.Context, query string, numResults int) (string, Usage, error) {
	return e.exaSearchAt(ctx, exaSearchURL, query, numResults)
}

// exaSearchAt is exaSearch with the endpoint injected, so tests can point it at
// a local server without reaching the network.
func (e *Executor) exaSearchAt(ctx context.Context, endpoint, query string, numResults int) (string, Usage, error) {
	if numResults <= 0 {
		numResults = 5
	}
	if numResults > 10 {
		numResults = 10
	}
	body := map[string]interface{}{
		"query":      query,
		"type":       exaSearchType,
		"numResults": numResults,
		// Highlights are query-relevant excerpts — the token-efficient content
		// mode. Full `text` would blow the budget on a single page.
		"contents": map[string]interface{}{"highlights": true},
	}
	data, err := e.postJSON(ctx, endpoint, map[string]string{"x-api-key": e.cfg.ExaKey}, body, "exa")
	if err != nil {
		return "", Usage{}, err
	}

	results, _ := jsonPath(data, "results").([]interface{})
	var out strings.Builder
	var sources []string
	for i, raw := range results {
		item, ok := raw.(map[string]interface{})
		if !ok {
			continue
		}
		title := jsonString(item, "title")
		url := jsonString(item, "url")
		if title == "" {
			title = url
		}
		fmt.Fprintf(&out, "%d. %s\n", i+1, title)
		if url != "" {
			fmt.Fprintf(&out, "   %s\n", url)
			sources = addUnique(sources, url)
		}
		if hs, ok := jsonPath(item, "highlights").([]interface{}); ok {
			for _, h := range hs {
				if text, ok := h.(string); ok && strings.TrimSpace(text) != "" {
					fmt.Fprintf(&out, "   %s\n", strings.TrimSpace(text))
				}
			}
		}
		out.WriteString("\n")
	}

	// One request = one billable unit, whatever the result count.
	usage := Usage{SearchCount: 1, Family: "exa"}
	return appendSources(strings.TrimSpace(out.String()), sources), usage, nil
}

const anthropicToolModel = "claude-haiku-4-5-20251001"

func (e *Executor) anthropicRequest(ctx context.Context, key string, body map[string]interface{}, beta string) (map[string]interface{}, error) {
	headers := map[string]string{
		"x-api-key":         key,
		"anthropic-version": "2023-06-01",
	}
	if beta != "" {
		headers["anthropic-beta"] = beta
	}
	return e.postJSON(ctx, "https://api.anthropic.com/v1/messages", headers, body, "Anthropic")
}

// anthropicCollect pulls the text blocks (and any cited URLs) out of a
// Messages response, plus the sub-call's token usage.
func anthropicCollect(data map[string]interface{}) (string, Usage) {
	var text strings.Builder
	var sources []string
	if content, ok := data["content"].([]interface{}); ok {
		for _, block := range content {
			if jsonString(block, "type") != "text" {
				continue
			}
			text.WriteString(jsonString(block, "text"))
			if cits, ok := jsonPath(block, "citations").([]interface{}); ok {
				for _, c := range cits {
					if u := jsonString(c, "url"); u != "" {
						sources = addUnique(sources, u)
					}
				}
			}
		}
	}
	usage := Usage{
		InputTokens:  jsonInt64(data, "usage", "input_tokens"),
		OutputTokens: jsonInt64(data, "usage", "output_tokens"),
		// Anthropic reports flat-fee search invocations explicitly; web_fetch
		// requests are free beyond tokens so they are not counted.
		SearchCount: jsonInt64(data, "usage", "server_tool_use", "web_search_requests"),
	}
	return appendSources(text.String(), sources), usage
}

func (e *Executor) anthropicSearch(ctx context.Context, query, key string) (string, Usage, error) {
	body := map[string]interface{}{
		"model":      anthropicToolModel,
		"max_tokens": searchMaxTokens,
		"messages": []map[string]interface{}{{
			"role":    "user",
			"content": "Search the web and answer concisely with the key facts and their sources: " + query,
		}},
		"tools": []map[string]interface{}{{
			"type": "web_search_20250305", "name": "web_search", "max_uses": 3,
		}},
	}
	data, err := e.anthropicRequest(ctx, key, body, "")
	if err != nil {
		return "", Usage{}, err
	}
	text, usage := anthropicCollect(data)
	return text, usage, nil
}

func (e *Executor) anthropicFetch(ctx context.Context, url, key string) (string, Usage, error) {
	body := map[string]interface{}{
		"model":      anthropicToolModel,
		"max_tokens": fetchMaxTokens,
		"messages": []map[string]interface{}{{
			"role": "user",
			"content": "Fetch " + url + " and reproduce its main content faithfully (code blocks and " +
				"tables intact). Do not summarize away details.",
		}},
		"tools": []map[string]interface{}{{
			"type": "web_fetch_20250910", "name": "web_fetch",
			"max_uses": 2, "max_content_tokens": fetchMaxTokens,
		}},
	}
	data, err := e.anthropicRequest(ctx, key, body, "web-fetch-2025-09-10")
	if err != nil {
		return "", Usage{}, err
	}
	text, usage := anthropicCollect(data)
	return text, usage, nil
}

// ── OpenAI ────────────────────────────────────────────────────────────────────

const openaiToolModel = "gpt-4o-mini"

func (e *Executor) openaiResponses(ctx context.Context, key, input string) (map[string]interface{}, error) {
	body := map[string]interface{}{
		"model":             openaiToolModel,
		"input":             input,
		"max_output_tokens": fetchMaxTokens,
		"tools":             []map[string]interface{}{{"type": "web_search_preview"}},
	}
	return e.postJSON(ctx, "https://api.openai.com/v1/responses",
		map[string]string{"Authorization": "Bearer " + key}, body, "OpenAI")
}

func openaiCollect(data map[string]interface{}) (string, Usage) {
	var text strings.Builder
	var sources []string
	var searches int64
	if output, ok := data["output"].([]interface{}); ok {
		for _, item := range output {
			// Each web_search_call output item is one flat-fee tool invocation.
			if jsonString(item, "type") == "web_search_call" {
				searches++
			}
			if jsonString(item, "type") != "message" {
				continue
			}
			if content, ok := jsonPath(item, "content").([]interface{}); ok {
				for _, part := range content {
					text.WriteString(jsonString(part, "text"))
					if anns, ok := jsonPath(part, "annotations").([]interface{}); ok {
						for _, a := range anns {
							if u := jsonString(a, "url"); u != "" {
								sources = addUnique(sources, u)
							}
						}
					}
				}
			}
		}
	}
	usage := Usage{
		InputTokens:  jsonInt64(data, "usage", "input_tokens"),
		OutputTokens: jsonInt64(data, "usage", "output_tokens"),
		SearchCount:  searches,
	}
	return appendSources(text.String(), sources), usage
}

func (e *Executor) openaiSearch(ctx context.Context, query, key string) (string, Usage, error) {
	data, err := e.openaiResponses(ctx, key,
		"Search the web and answer concisely with the key facts and their sources: "+query)
	if err != nil {
		return "", Usage{}, err
	}
	text, usage := openaiCollect(data)
	return text, usage, nil
}

func (e *Executor) openaiFetch(ctx context.Context, url, key string) (string, Usage, error) {
	data, err := e.openaiResponses(ctx, key,
		"Open "+url+" and reproduce its main content faithfully (code blocks and "+
			"tables intact). Do not summarize away details.")
	if err != nil {
		return "", Usage{}, err
	}
	text, usage := openaiCollect(data)
	return text, usage, nil
}

// ── Gemini ────────────────────────────────────────────────────────────────────

const geminiToolModel = "gemini-2.5-flash"

func (e *Executor) geminiRequest(ctx context.Context, key, prompt string, tool map[string]interface{}) (map[string]interface{}, error) {
	url := fmt.Sprintf(
		"https://generativelanguage.googleapis.com/v1beta/models/%s:generateContent?key=%s",
		geminiToolModel, key)
	body := map[string]interface{}{
		"contents":         []map[string]interface{}{{"parts": []map[string]interface{}{{"text": prompt}}}},
		"tools":            []map[string]interface{}{tool},
		"generationConfig": map[string]interface{}{"maxOutputTokens": fetchMaxTokens},
	}
	return e.postJSON(ctx, url, nil, body, "Gemini")
}

func geminiCollect(data map[string]interface{}) (string, Usage) {
	var text strings.Builder
	if parts, ok := jsonPath(data, "candidates", 0, "content", "parts").([]interface{}); ok {
		for _, p := range parts {
			text.WriteString(jsonString(p, "text"))
		}
	}
	var sources []string
	var searches int64
	if chunks, ok := jsonPath(data, "candidates", 0, "groundingMetadata", "groundingChunks").([]interface{}); ok {
		for _, c := range chunks {
			if u := jsonString(c, "web", "uri"); u != "" {
				sources = addUnique(sources, u)
			}
		}
		// Google bills grounding per grounded prompt, not per internal query —
		// grounding metadata present means this prompt was grounded once.
		searches = 1
	}
	usage := Usage{
		InputTokens:  jsonInt64(data, "usageMetadata", "promptTokenCount"),
		OutputTokens: jsonInt64(data, "usageMetadata", "candidatesTokenCount"),
		SearchCount:  searches,
	}
	return appendSources(text.String(), sources), usage
}

func (e *Executor) geminiSearch(ctx context.Context, query, key string) (string, Usage, error) {
	data, err := e.geminiRequest(ctx, key,
		"Search the web and answer concisely with the key facts: "+query,
		map[string]interface{}{"google_search": map[string]interface{}{}})
	if err != nil {
		return "", Usage{}, err
	}
	text, usage := geminiCollect(data)
	return text, usage, nil
}

func (e *Executor) geminiFetch(ctx context.Context, url, key string) (string, Usage, error) {
	data, err := e.geminiRequest(ctx, key,
		"Read "+url+" and reproduce its main content faithfully (code blocks and "+
			"tables intact). Do not summarize away details.",
		map[string]interface{}{"url_context": map[string]interface{}{}})
	if err != nil {
		return "", Usage{}, err
	}
	text, usage := geminiCollect(data)
	return text, usage, nil
}

// ── xAI / Grok ────────────────────────────────────────────────────────────────

const xaiToolModel = "grok-3-mini"

func (e *Executor) xaiSearch(ctx context.Context, query, key string) (string, Usage, error) {
	body := map[string]interface{}{
		"model":      xaiToolModel,
		"max_tokens": searchMaxTokens,
		"messages": []map[string]interface{}{{
			"role":    "user",
			"content": "Search the web and answer concisely with the key facts and their sources: " + query,
		}},
		"search_parameters": map[string]interface{}{"mode": "on", "return_citations": true},
	}
	data, err := e.postJSON(ctx, "https://api.x.ai/v1/chat/completions",
		map[string]string{"Authorization": "Bearer " + key}, body, "xAI")
	if err != nil {
		return "", Usage{}, err
	}
	text := jsonString(data, "choices", 0, "message", "content")
	var sources []string
	if cits, ok := jsonPath(data, "citations").([]interface{}); ok {
		for _, c := range cits {
			if u, ok := c.(string); ok {
				sources = addUnique(sources, u)
			}
		}
	}
	usage := Usage{
		InputTokens:  jsonInt64(data, "usage", "prompt_tokens"),
		OutputTokens: jsonInt64(data, "usage", "completion_tokens"),
		// xAI bills Live Search per source consulted; the citation list is the
		// closest observable proxy (min 1 — mode "on" always searches).
		SearchCount: max64(1, int64(len(sources))),
	}
	return appendSources(text, sources), usage, nil
}

func max64(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}
