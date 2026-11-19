package model

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"

	"server/internal/platform/netguard"
)

var (
	ErrCircuitOpen = errors.New("llm provider circuit is open (too many recent failures), using fallback analysis")
)

// ── Circuit Breaker ───────────────────────────────────────────────────────────

type CircuitBreaker struct {
	mu           sync.RWMutex
	failures     int
	lastFailure  time.Time
	state        string // "CLOSED", "OPEN"
	openDuration time.Duration
}

func NewCircuitBreaker(openDuration time.Duration) *CircuitBreaker {
	return &CircuitBreaker{state: "CLOSED", openDuration: openDuration}
}

func (cb *CircuitBreaker) Allow() bool {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	if cb.state == "OPEN" {
		if time.Since(cb.lastFailure) > cb.openDuration {
			cb.state = "CLOSED"
			cb.failures = 0
			return true
		}
		return false
	}
	return true
}

func (cb *CircuitBreaker) RecordSuccess() {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.failures = 0
	cb.state = "CLOSED"
}

func (cb *CircuitBreaker) RecordFailure() {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.failures++
	cb.lastFailure = time.Now()
	if cb.failures >= 5 {
		cb.state = "OPEN"
	}
}

// ── Provider kinds ────────────────────────────────────────────────────────────

type providerKind int

const (
	providerOpenAICompat providerKind = iota // OpenAI, OpenRouter, Groq, Ollama, Azure, Vercel, Cerebras, LM Studio
	providerAnthropic
	providerGemini
)

type providerConfig struct {
	kind    providerKind
	baseURL string
	apiKey  string
	// authHeader is the HTTP header name used to send the API key (default: Authorization)
	authHeader string
	// authPrefix is the value prefix, e.g. "Bearer " or "" (default: "Bearer ")
	authPrefix   string
	userSupplied bool
}

// resolveProvider maps a model name to a providerConfig. Model names may carry an
// explicit routing prefix separated by "/" (e.g. "openrouter/claude-3.5-sonnet")
// or use conventional prefixes (e.g. "gpt-4o" → OpenAI, "claude-*" → Anthropic).
func resolveProvider(modelName string, keys map[string]string) (providerConfig, string, error) {
	if idx := strings.Index(modelName, "/"); idx > 0 {
		prefix := modelName[:idx]
		bare := modelName[idx+1:]
		switch prefix {
		case "openrouter":
			return providerConfig{
				kind:       providerOpenAICompat,
				baseURL:    "https://openrouter.ai/api/v1",
				apiKey:     keys["OPENROUTER_API_KEY"],
				authHeader: "Authorization",
				authPrefix: "Bearer ",
			}, bare, nil
		case "groq":
			return providerConfig{
				kind:    providerOpenAICompat,
				baseURL: "https://api.groq.com/openai/v1",
				apiKey:  keys["GROQ_API_KEY"],
			}, bare, nil
		case "ollama":
			baseURL := keys["OLLAMA_BASE_URL"]
			if baseURL == "" {
				baseURL = "http://localhost:11434/v1"
			}
			return providerConfig{
				kind:    providerOpenAICompat,
				baseURL: baseURL,
				apiKey:  "ollama", // Ollama doesn't require a real key
			}, bare, nil
		case "lmstudio":
			baseURL := keys["LM_STUDIO_BASE_URL"]
			if baseURL == "" {
				baseURL = "http://localhost:1234/v1"
			}
			return providerConfig{
				kind:    providerOpenAICompat,
				baseURL: baseURL,
				apiKey:  "lmstudio",
			}, bare, nil
		case "azure":
			// azure/gpt-4o → Azure OpenAI, requires AZURE_OPENAI_ENDPOINT + AZURE_OPENAI_API_KEY
			endpoint := keys["AZURE_OPENAI_ENDPOINT"]
			apiVersion := keys["AZURE_OPENAI_API_VERSION"]
			if apiVersion == "" {
				apiVersion = "2024-02-01"
			}
			return providerConfig{
				kind:       providerOpenAICompat,
				baseURL:    fmt.Sprintf("%s/openai/deployments/%s?api-version=%s", endpoint, bare, apiVersion),
				apiKey:     keys["AZURE_OPENAI_API_KEY"],
				authHeader: "api-key",
				authPrefix: "",
			}, bare, nil
		case "bedrock":
			// AWS Bedrock requires AWS SDK — route through OpenRouter as proxy for now
			return providerConfig{
				kind:    providerOpenAICompat,
				baseURL: "https://openrouter.ai/api/v1",
				apiKey:  keys["OPENROUTER_API_KEY"],
			}, "anthropic/" + bare, nil
		case "vertex":
			return providerConfig{
				kind:    providerOpenAICompat,
				baseURL: "https://openrouter.ai/api/v1",
				apiKey:  keys["OPENROUTER_API_KEY"],
			}, "google/" + bare, nil
		case "cerebras":
			return providerConfig{
				kind:    providerOpenAICompat,
				baseURL: "https://api.cerebras.ai/v1",
				apiKey:  keys["CEREBRAS_API_KEY"],
			}, bare, nil
		case "google":
			return providerConfig{
				kind:    providerGemini,
				baseURL: "https://generativelanguage.googleapis.com/v1beta",
				apiKey:  keys["GOOGLE_API_KEY"],
			}, bare, nil
		}
	}

	// No prefix — use naming conventions
	switch {
	case strings.HasPrefix(modelName, "gpt-") ||
		strings.HasPrefix(modelName, "o1-") ||
		strings.HasPrefix(modelName, "o3-") ||
		strings.HasPrefix(modelName, "o4-"):
		return providerConfig{
			kind:    providerOpenAICompat,
			baseURL: "https://api.openai.com/v1",
			apiKey:  keys["OPENAI_API_KEY"],
		}, modelName, nil

	case strings.HasPrefix(modelName, "claude-"):
		return providerConfig{
			kind:       providerAnthropic,
			baseURL:    "https://api.anthropic.com/v1",
			apiKey:     keys["ANTHROPIC_API_KEY"],
			authHeader: "x-api-key",
			authPrefix: "",
		}, modelName, nil

	case strings.HasPrefix(modelName, "gemini-"):
		return providerConfig{
			kind:    providerGemini,
			baseURL: "https://generativelanguage.googleapis.com/v1beta",
			apiKey:  keys["GOOGLE_API_KEY"],
		}, modelName, nil

	case strings.HasPrefix(modelName, "llama") || strings.HasPrefix(modelName, "mixtral") || strings.HasPrefix(modelName, "qwen"):
		// Default fast-inference models → Groq
		return providerConfig{
			kind:    providerOpenAICompat,
			baseURL: "https://api.groq.com/openai/v1",
			apiKey:  keys["GROQ_API_KEY"],
		}, modelName, nil

	default:
		// Fallback: OpenAI
		return providerConfig{
			kind:    providerOpenAICompat,
			baseURL: "https://api.openai.com/v1",
			apiKey:  keys["OPENAI_API_KEY"],
		}, modelName, nil
	}
}

// ── MultiLLMGateway ───────────────────────────────────────────────────────────

type MultiLLMGateway struct {
	keys          map[string]string // env-like key store: OPENAI_API_KEY, ANTHROPIC_API_KEY, etc.
	cb            *CircuitBreaker
	httpClient    *http.Client
	guardedClient *http.Client
}

func NewMultiLLMGateway(openaiKey, anthropicKey string) *MultiLLMGateway {
	return &MultiLLMGateway{
		keys: map[string]string{
			"OPENAI_API_KEY":    openaiKey,
			"ANTHROPIC_API_KEY": anthropicKey,
		},
		cb:            NewCircuitBreaker(30 * time.Second),
		httpClient:    &http.Client{Timeout: 30 * time.Second},
		guardedClient: netguard.NewClient(30 * time.Second),
	}
}

// NewMultiLLMGatewayFromKeys creates a gateway with all supported provider keys.
func NewMultiLLMGatewayFromKeys(keys map[string]string) *MultiLLMGateway {
	return &MultiLLMGateway{
		keys:          keys,
		cb:            NewCircuitBreaker(30 * time.Second),
		httpClient:    &http.Client{Timeout: 30 * time.Second},
		guardedClient: netguard.NewClient(30 * time.Second),
	}
}

// ── AnalyzeFailure (single-shot, backwards-compatible) ───────────────────────

func (g *MultiLLMGateway) AnalyzeFailure(ctx context.Context, modelName string, command string, cwd string, outputText string) (*AnalysisResult, error) {
	if modelName == "" {
		// Safety net only — callers resolve the model from config
		// (selfheal_model) before reaching here. Must be a cataloged model:
		// unknown models are refused at billing time (no fallback pricing).
		modelName = "gpt-5.4-mini"
	}

	if !g.cb.Allow() {
		return g.fallbackMockResult(command, outputText), nil
	}

	if len(g.keys) == 0 || (g.keys["OPENAI_API_KEY"] == "" && g.keys["ANTHROPIC_API_KEY"] == "") {
		return g.fallbackMockResult(command, outputText), nil
	}

	prov, bareModel, err := resolveProvider(modelName, g.keys)
	if err != nil {
		return g.fallbackMockResult(command, outputText), nil
	}

	system := "You are a terminal command failure diagnostic agent. Analyze the error and return a JSON payload matching this schema: {\"explanation\": \"string: why the command failed and what fixes it\", \"suggested_command\": \"string or null\"}. " +
		"CLASSIFY THE FIX. Set 'suggested_command' to the EXACT shell command the user should re-run ONLY when re-running a corrected command resolves the failure — a typo, a missing flag or argument, a wrong path, a missing dependency to install, a wrong directory. " +
		"Set 'suggested_command' to null when fixing the failure requires EDITING source code or configuration files (a bug in the code, a syntax error in a file, a wrong value in a config) — in that case put the concrete change needed into 'explanation'. " +
		"Never invent a command that only papers over a code bug. Output only valid JSON."
	userMsg := fmt.Sprintf("Command failed: %s\nWorking Directory: %s\nOutput:\n%s", command, cwd, outputText)

	var result *AnalysisResult
	switch prov.kind {
	case providerAnthropic:
		result, err = g.callAnthropicSimple(ctx, prov, bareModel, system, userMsg)
	case providerGemini:
		result, err = g.callGeminiSimple(ctx, prov, bareModel, system, userMsg)
	default:
		result, err = g.callOpenAICompatSimple(ctx, prov, bareModel, system, userMsg)
	}

	if err != nil {
		g.cb.RecordFailure()
		if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, context.Canceled) {
			return nil, err
		}
		return g.fallbackMockResult(command, outputText), nil
	}

	g.cb.RecordSuccess()
	return result, nil
}

// ── AgentStep (agentic tool-calling loop) ────────────────────────────────────

func (g *MultiLLMGateway) AgentStep(ctx context.Context, req AgentStepRequest) (*AgentStepResponse, error) {
	if req.ModelName == "" {
		req.ModelName = "claude-sonnet-4-6"
	}

	// Merge gateway keys with user-provided key: the user's key takes precedence
	// over the server's shared key for whichever provider the model belongs to.
	keys := g.keys
	if req.UserApiKey != "" {
		keys = make(map[string]string, len(g.keys)+2)
		for k, v := range g.keys {
			keys[k] = v
		}
		// Determine which provider key to override based on model name prefix.
		// This mirrors the logic in resolveProvider below.
		name := req.ModelName
		switch {
		case strings.HasPrefix(name, "claude-"):
			keys["ANTHROPIC_API_KEY"] = req.UserApiKey
		case strings.HasPrefix(name, "gpt-") ||
			strings.HasPrefix(name, "o1-") ||
			strings.HasPrefix(name, "o3-") ||
			strings.HasPrefix(name, "o4-"):
			keys["OPENAI_API_KEY"] = req.UserApiKey
		case strings.HasPrefix(name, "gemini-") || strings.HasPrefix(name, "google/"):
			keys["GOOGLE_API_KEY"] = req.UserApiKey
		case strings.HasPrefix(name, "groq/"):
			keys["GROQ_API_KEY"] = req.UserApiKey
		case strings.HasPrefix(name, "cerebras/"):
			keys["CEREBRAS_API_KEY"] = req.UserApiKey
		case strings.HasPrefix(name, "openrouter/"):
			keys["OPENROUTER_API_KEY"] = req.UserApiKey
		default:
			// Generic override — set all common key slots so the resolved
			// provider picks it up regardless of routing.
			keys["OPENAI_API_KEY"] = req.UserApiKey
			keys["ANTHROPIC_API_KEY"] = req.UserApiKey
		}
	}

	// If the client sent a custom base URL, route immediately to an OpenAI-compat endpoint.
	if req.CustomBaseUrl != "" {
		if err := netguard.ValidateURL(req.CustomBaseUrl); err != nil {
			return nil, err
		}
		prov := providerConfig{
			kind:         providerOpenAICompat,
			baseURL:      req.CustomBaseUrl,
			apiKey:       req.UserApiKey,
			userSupplied: true,
		}
		return g.agentStepOpenAICompat(ctx, prov, req.ModelName, req)
	}

	prov, bareModel, err := resolveProvider(req.ModelName, keys)
	if err != nil {
		return nil, fmt.Errorf("unknown model provider: %w", err)
	}

	switch prov.kind {
	case providerAnthropic:
		return g.agentStepAnthropic(ctx, prov, bareModel, req)
	case providerGemini:
		return g.agentStepGemini(ctx, prov, bareModel, req)
	default:
		return g.agentStepOpenAICompat(ctx, prov, bareModel, req)
	}
}

// ── OpenAI-compatible (covers OpenAI, OpenRouter, Groq, Ollama, Azure, Vercel, Cerebras) ──

func (g *MultiLLMGateway) callOpenAICompatSimple(ctx context.Context, prov providerConfig, model, system, userMsg string) (*AnalysisResult, error) {
	body := map[string]interface{}{
		"model": model,
		"messages": []map[string]string{
			{"role": "system", "content": system},
			{"role": "user", "content": userMsg},
		},
		"response_format": map[string]string{"type": "json_object"},
	}
	data, _ := json.Marshal(body)
	resp, err := g.doRequest(ctx, prov, "POST", "/chat/completions", data)
	if err != nil {
		return nil, err
	}

	var raw struct {
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
		Usage struct {
			PromptTokens     int64 `json:"prompt_tokens"`
			CompletionTokens int64 `json:"completion_tokens"`
		} `json:"usage"`
	}
	if err := json.Unmarshal(resp, &raw); err != nil {
		return nil, fmt.Errorf("parse openai response: %w", err)
	}
	if len(raw.Choices) == 0 {
		return nil, errors.New("empty choices from openai")
	}

	var parsed struct {
		Explanation      string  `json:"explanation"`
		SuggestedCommand *string `json:"suggested_command"`
	}
	if err := json.Unmarshal([]byte(raw.Choices[0].Message.Content), &parsed); err != nil {
		return nil, fmt.Errorf("parse openai json body: %w", err)
	}
	return &AnalysisResult{
		Explanation:      parsed.Explanation,
		SuggestedCommand: parsed.SuggestedCommand,
		InputTokens:      raw.Usage.PromptTokens,
		OutputTokens:     raw.Usage.CompletionTokens,
	}, nil
}

func (g *MultiLLMGateway) agentStepOpenAICompat(ctx context.Context, prov providerConfig, model string, req AgentStepRequest) (*AgentStepResponse, error) {
	// Build messages array
	messages := make([]map[string]interface{}, 0, len(req.Messages)+1)
	if req.System != "" {
		messages = append(messages, map[string]interface{}{"role": "system", "content": req.System})
	}
	for _, m := range req.Messages {
		msg := map[string]interface{}{"role": string(m.Role)}
		switch m.Role {
		case AgentRoleAssistant:
			if len(m.ToolCalls) > 0 {
				calls := make([]map[string]interface{}, len(m.ToolCalls))
				for i, tc := range m.ToolCalls {
					calls[i] = map[string]interface{}{
						"id":   tc.ID,
						"type": "function",
						"function": map[string]interface{}{
							"name":      tc.Name,
							"arguments": tc.ArgsJSON,
						},
					}
				}
				msg["tool_calls"] = calls
				if m.Content != "" {
					msg["content"] = m.Content
				} else {
					msg["content"] = nil
				}
			} else {
				msg["content"] = m.Content
			}
		case AgentRoleTool:
			msg["tool_call_id"] = m.ToolCallID
			msg["content"] = m.Content
		default:
			msg["content"] = m.Content
		}
		messages = append(messages, msg)
	}

	// Build tools array
	tools := make([]map[string]interface{}, len(req.Tools))
	for i, t := range req.Tools {
		tools[i] = map[string]interface{}{
			"type": "function",
			"function": map[string]interface{}{
				"name":        t.Name,
				"description": t.Description,
				"parameters":  t.Parameters,
			},
		}
	}

	body := map[string]interface{}{
		"model":    model,
		"messages": messages,
	}
	if len(tools) > 0 {
		body["tools"] = tools
	}

	data, _ := json.Marshal(body)
	respBytes, err := g.doRequest(ctx, prov, "POST", "/chat/completions", data)
	if err != nil {
		return nil, err
	}

	var raw struct {
		Choices []struct {
			Message struct {
				Content   *string `json:"content"`
				ToolCalls []struct {
					ID       string `json:"id"`
					Function struct {
						Name      string `json:"name"`
						Arguments string `json:"arguments"`
					} `json:"function"`
				} `json:"tool_calls"`
			} `json:"message"`
		} `json:"choices"`
		Usage struct {
			PromptTokens     int64 `json:"prompt_tokens"`
			CompletionTokens int64 `json:"completion_tokens"`
		} `json:"usage"`
	}
	if err := json.Unmarshal(respBytes, &raw); err != nil {
		return nil, fmt.Errorf("parse openai agent step response: %w", err)
	}
	if len(raw.Choices) == 0 {
		return nil, errors.New("empty choices from openai agent step")
	}

	msg := raw.Choices[0].Message
	out := &AgentStepResponse{
		InputTokens:  raw.Usage.PromptTokens,
		OutputTokens: raw.Usage.CompletionTokens,
	}
	if len(msg.ToolCalls) > 0 {
		out.ToolCalls = make([]AgentToolCall, len(msg.ToolCalls))
		for i, tc := range msg.ToolCalls {
			out.ToolCalls[i] = AgentToolCall{
				ID:       tc.ID,
				Name:     tc.Function.Name,
				ArgsJSON: tc.Function.Arguments,
			}
		}
	} else if msg.Content != nil {
		out.Content = *msg.Content
	}
	return out, nil
}

// ── Anthropic ─────────────────────────────────────────────────────────────────

func (g *MultiLLMGateway) callAnthropicSimple(ctx context.Context, prov providerConfig, model, system, userMsg string) (*AnalysisResult, error) {
	body := map[string]interface{}{
		"model":      model,
		"max_tokens": 1024,
		"system":     system,
		"messages":   []map[string]string{{"role": "user", "content": userMsg}},
	}
	data, _ := json.Marshal(body)
	resp, err := g.doRequest(ctx, prov, "POST", "/messages", data)
	if err != nil {
		return nil, err
	}

	var raw struct {
		Content []struct {
			Text string `json:"text"`
		} `json:"content"`
		Usage struct {
			InputTokens  int64 `json:"input_tokens"`
			OutputTokens int64 `json:"output_tokens"`
		} `json:"usage"`
	}
	if err := json.Unmarshal(resp, &raw); err != nil {
		return nil, fmt.Errorf("parse anthropic response: %w", err)
	}
	if len(raw.Content) == 0 {
		return nil, errors.New("empty content from anthropic")
	}

	var parsed struct {
		Explanation      string  `json:"explanation"`
		SuggestedCommand *string `json:"suggested_command"`
	}
	if err := json.Unmarshal([]byte(raw.Content[0].Text), &parsed); err != nil {
		return nil, fmt.Errorf("parse anthropic json body: %w", err)
	}
	return &AnalysisResult{
		Explanation:      parsed.Explanation,
		SuggestedCommand: parsed.SuggestedCommand,
		InputTokens:      raw.Usage.InputTokens,
		OutputTokens:     raw.Usage.OutputTokens,
	}, nil
}

func (g *MultiLLMGateway) agentStepAnthropic(ctx context.Context, prov providerConfig, model string, req AgentStepRequest) (*AgentStepResponse, error) {
	// Build Anthropic messages
	messages := make([]map[string]interface{}, 0, len(req.Messages))
	for _, m := range req.Messages {
		switch m.Role {
		case AgentRoleSystem:
			continue // system is a top-level field
		case AgentRoleAssistant:
			if len(m.ToolCalls) > 0 {
				content := make([]map[string]interface{}, len(m.ToolCalls))
				for i, tc := range m.ToolCalls {
					var args map[string]interface{}
					_ = json.Unmarshal([]byte(tc.ArgsJSON), &args)
					content[i] = map[string]interface{}{
						"type":  "tool_use",
						"id":    tc.ID,
						"name":  tc.Name,
						"input": args,
					}
				}
				if m.Content != "" {
					full := []interface{}{map[string]string{"type": "text", "text": m.Content}}
					for _, c := range content {
						full = append(full, c)
					}
					messages = append(messages, map[string]interface{}{"role": "assistant", "content": full})
				} else {
					messages = append(messages, map[string]interface{}{"role": "assistant", "content": content})
				}
			} else {
				messages = append(messages, map[string]interface{}{"role": "assistant", "content": m.Content})
			}
		case AgentRoleTool:
			messages = append(messages, map[string]interface{}{
				"role": "user",
				"content": []map[string]interface{}{{
					"type":        "tool_result",
					"tool_use_id": m.ToolCallID,
					"content":     m.Content,
				}},
			})
		default:
			messages = append(messages, map[string]interface{}{"role": "user", "content": m.Content})
		}
	}

	// Build Anthropic tools
	tools := make([]map[string]interface{}, len(req.Tools))
	for i, t := range req.Tools {
		tools[i] = map[string]interface{}{
			"name":         t.Name,
			"description":  t.Description,
			"input_schema": t.Parameters,
		}
	}

	body := map[string]interface{}{
		"model":      model,
		"max_tokens": 4096,
		"system":     req.System,
		"messages":   messages,
	}
	if len(tools) > 0 {
		body["tools"] = tools
	}

	data, _ := json.Marshal(body)
	respBytes, err := g.doRequest(ctx, prov, "POST", "/messages", data)
	if err != nil {
		return nil, err
	}

	var raw struct {
		Content []struct {
			Type  string          `json:"type"`
			Text  string          `json:"text"`
			ID    string          `json:"id"`
			Name  string          `json:"name"`
			Input json.RawMessage `json:"input"`
		} `json:"content"`
		Usage struct {
			InputTokens  int64 `json:"input_tokens"`
			OutputTokens int64 `json:"output_tokens"`
		} `json:"usage"`
	}
	if err := json.Unmarshal(respBytes, &raw); err != nil {
		return nil, fmt.Errorf("parse anthropic agent step: %w", err)
	}

	out := &AgentStepResponse{
		InputTokens:  raw.Usage.InputTokens,
		OutputTokens: raw.Usage.OutputTokens,
	}
	for _, block := range raw.Content {
		switch block.Type {
		case "tool_use":
			argsJSON, _ := json.Marshal(block.Input)
			out.ToolCalls = append(out.ToolCalls, AgentToolCall{
				ID:       block.ID,
				Name:     block.Name,
				ArgsJSON: string(argsJSON),
			})
		case "text":
			out.Content += block.Text
		}
	}
	return out, nil
}

// ── Google Gemini ─────────────────────────────────────────────────────────────

func (g *MultiLLMGateway) callGeminiSimple(ctx context.Context, prov providerConfig, model, system, userMsg string) (*AnalysisResult, error) {
	body := map[string]interface{}{
		"system_instruction": map[string]interface{}{
			"parts": []map[string]string{{"text": system}},
		},
		"contents": []map[string]interface{}{{
			"role":  "user",
			"parts": []map[string]string{{"text": userMsg}},
		}},
		"generationConfig": map[string]interface{}{
			"responseMimeType": "application/json",
		},
	}
	data, _ := json.Marshal(body)
	// Gemini uses ?key=... for auth
	url := fmt.Sprintf("%s/models/%s:generateContent?key=%s", prov.baseURL, model, prov.apiKey)
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewBuffer(data))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := g.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("gemini returned %d", resp.StatusCode)
	}

	var raw struct {
		Candidates []struct {
			Content struct {
				Parts []struct {
					Text string `json:"text"`
				} `json:"parts"`
			} `json:"content"`
		} `json:"candidates"`
		UsageMetadata struct {
			PromptTokenCount     int64 `json:"promptTokenCount"`
			CandidatesTokenCount int64 `json:"candidatesTokenCount"`
		} `json:"usageMetadata"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&raw); err != nil {
		return nil, fmt.Errorf("parse gemini: %w", err)
	}
	if len(raw.Candidates) == 0 || len(raw.Candidates[0].Content.Parts) == 0 {
		return nil, errors.New("empty gemini response")
	}

	var parsed struct {
		Explanation      string  `json:"explanation"`
		SuggestedCommand *string `json:"suggested_command"`
	}
	if err := json.Unmarshal([]byte(raw.Candidates[0].Content.Parts[0].Text), &parsed); err != nil {
		return nil, fmt.Errorf("parse gemini json body: %w", err)
	}
	return &AnalysisResult{
		Explanation:      parsed.Explanation,
		SuggestedCommand: parsed.SuggestedCommand,
		InputTokens:      raw.UsageMetadata.PromptTokenCount,
		OutputTokens:     raw.UsageMetadata.CandidatesTokenCount,
	}, nil
}

func (g *MultiLLMGateway) agentStepGemini(ctx context.Context, prov providerConfig, model string, req AgentStepRequest) (*AgentStepResponse, error) {
	// Build Gemini contents
	contents := make([]map[string]interface{}, 0, len(req.Messages))
	for _, m := range req.Messages {
		if m.Role == AgentRoleSystem {
			continue
		}
		role := "user"
		if m.Role == AgentRoleAssistant {
			role = "model"
		}
		if m.Role == AgentRoleTool {
			// Tool response → functionResponse part
			contents = append(contents, map[string]interface{}{
				"role": "user",
				"parts": []map[string]interface{}{{
					"functionResponse": map[string]interface{}{
						"name": m.ToolCallID,
						"response": map[string]interface{}{
							"content": m.Content,
						},
					},
				}},
			})
			continue
		}
		if len(m.ToolCalls) > 0 {
			parts := make([]map[string]interface{}, len(m.ToolCalls))
			for i, tc := range m.ToolCalls {
				var args map[string]interface{}
				_ = json.Unmarshal([]byte(tc.ArgsJSON), &args)
				parts[i] = map[string]interface{}{
					"functionCall": map[string]interface{}{
						"name": tc.Name,
						"args": args,
					},
				}
			}
			contents = append(contents, map[string]interface{}{"role": role, "parts": parts})
		} else {
			contents = append(contents, map[string]interface{}{
				"role":  role,
				"parts": []map[string]string{{"text": m.Content}},
			})
		}
	}

	// Build Gemini tools
	var tools interface{}
	if len(req.Tools) > 0 {
		decls := make([]map[string]interface{}, len(req.Tools))
		for i, t := range req.Tools {
			decls[i] = map[string]interface{}{
				"name":        t.Name,
				"description": t.Description,
				"parameters":  t.Parameters,
			}
		}
		tools = []map[string]interface{}{{"functionDeclarations": decls}}
	}

	body := map[string]interface{}{
		"contents": contents,
	}
	if req.System != "" {
		body["system_instruction"] = map[string]interface{}{
			"parts": []map[string]string{{"text": req.System}},
		}
	}
	if tools != nil {
		body["tools"] = tools
	}

	data, _ := json.Marshal(body)
	url := fmt.Sprintf("%s/models/%s:generateContent?key=%s", prov.baseURL, model, prov.apiKey)
	httpReq, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewBuffer(data))
	if err != nil {
		return nil, err
	}
	httpReq.Header.Set("Content-Type", "application/json")
	resp, err := g.httpClient.Do(httpReq)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("gemini agent step returned %d", resp.StatusCode)
	}

	var raw struct {
		Candidates []struct {
			Content struct {
				Parts []struct {
					Text         string `json:"text"`
					FunctionCall *struct {
						Name string          `json:"name"`
						Args json.RawMessage `json:"args"`
					} `json:"functionCall"`
				} `json:"parts"`
			} `json:"content"`
		} `json:"candidates"`
		UsageMetadata struct {
			PromptTokenCount     int64 `json:"promptTokenCount"`
			CandidatesTokenCount int64 `json:"candidatesTokenCount"`
		} `json:"usageMetadata"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&raw); err != nil {
		return nil, fmt.Errorf("parse gemini agent step: %w", err)
	}

	out := &AgentStepResponse{
		InputTokens:  raw.UsageMetadata.PromptTokenCount,
		OutputTokens: raw.UsageMetadata.CandidatesTokenCount,
	}
	if len(raw.Candidates) > 0 {
		for _, part := range raw.Candidates[0].Content.Parts {
			if part.FunctionCall != nil {
				argsJSON, _ := json.Marshal(part.FunctionCall.Args)
				out.ToolCalls = append(out.ToolCalls, AgentToolCall{
					ID:       part.FunctionCall.Name + "_" + fmt.Sprintf("%d", len(out.ToolCalls)),
					Name:     part.FunctionCall.Name,
					ArgsJSON: string(argsJSON),
				})
			} else if part.Text != "" {
				out.Content += part.Text
			}
		}
	}
	return out, nil
}

// ── HTTP helper ───────────────────────────────────────────────────────────────

func (g *MultiLLMGateway) doRequest(ctx context.Context, prov providerConfig, method, path string, body []byte) ([]byte, error) {
	url := prov.baseURL + path
	req, err := http.NewRequestWithContext(ctx, method, url, bytes.NewBuffer(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")

	authHeader := prov.authHeader
	if authHeader == "" {
		authHeader = "Authorization"
	}
	authPrefix := prov.authPrefix
	if authPrefix == "" && prov.kind != providerAnthropic {
		authPrefix = "Bearer "
	}
	if prov.apiKey != "" {
		req.Header.Set(authHeader, authPrefix+prov.apiKey)
	}
	// Anthropic-specific headers
	if prov.kind == providerAnthropic {
		req.Header.Set("anthropic-version", "2023-06-01")
	}

	client := g.httpClient
	if prov.userSupplied {
		client = g.guardedClient
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("provider returned %d for %s%s", resp.StatusCode, prov.baseURL, path)
	}

	var buf bytes.Buffer
	if _, err := buf.ReadFrom(resp.Body); err != nil {
		return nil, fmt.Errorf("read response body: %w", err)
	}
	return buf.Bytes(), nil
}

// ── Fallback (offline / dev mode) ─────────────────────────────────────────────

func (g *MultiLLMGateway) fallbackMockResult(command, outputText string) *AnalysisResult {
	explanation := "The command execution failed. It appears to be a typo or configuration discrepancy."
	var suggested string

	cmdLower := strings.ToLower(command)
	if strings.Contains(cmdLower, "gti") {
		explanation = "TYPO DETECTED: You typed 'gti' instead of 'git'."
		suggested = strings.Replace(command, "gti", "git", 1)
	} else if strings.Contains(cmdLower, "docker") && strings.Contains(outputText, "cannot connect") {
		explanation = "DOCKER DAEMON NOT RUNNING: The command failed because Docker is not started."
		suggested = "open -a Docker"
	} else if strings.Contains(cmdLower, "npm") && strings.Contains(outputText, "package.json") {
		explanation = "MISSING PACKAGE.JSON: You ran npm but there is no package.json in this directory."
	}

	var suggestedPtr *string
	if suggested != "" {
		suggestedPtr = &suggested
	}
	return &AnalysisResult{
		Explanation:      explanation,
		SuggestedCommand: suggestedPtr,
		InputTokens:      150,
		OutputTokens:     50,
	}
}

var _ LLMGateway = (*MultiLLMGateway)(nil)
