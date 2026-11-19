package providers

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const anthropicBaseURL = "https://api.anthropic.com/v1"

// AnthropicAdapter implements LLMProvider for the Anthropic Messages API
// with native streaming (SSE), prompt caching, web search, and extended thinking.
type AnthropicAdapter struct {
	apiKey     string
	httpClient *http.Client
}

func NewAnthropicAdapter(apiKey string) *AnthropicAdapter {
	return &AnthropicAdapter{
		apiKey:     apiKey,
		httpClient: &http.Client{Timeout: 120 * time.Second},
	}
}

func (a *AnthropicAdapter) ProviderName() string { return "anthropic" }

// SupportedTools returns nil: web tooling is served by the pipeline's
// server-side webtools executor, not injected inline by the adapter.
func (a *AnthropicAdapter) SupportedTools() []ToolDefinition { return nil }

func (a *AnthropicAdapter) Capabilities() ProviderCapabilities {
	return ProviderCapabilities{
		Streaming:        true,
		ToolCalling:      true,
		Thinking:         true,
		ImageInput:       true,
		StructuredOutput: false,
		Caching:          true,
	}
}

func (a *AnthropicAdapter) StreamStep(ctx context.Context, req StepRequest) (<-chan StreamEvent, error) {
	body, err := a.buildBody(req)
	if err != nil {
		return nil, fmt.Errorf("anthropic: build request body: %w", err)
	}

	httpReq, err := http.NewRequestWithContext(ctx, "POST", anthropicBaseURL+"/messages", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Accept", "text/event-stream")
	httpReq.Header.Set("x-api-key", a.apiKey)
	httpReq.Header.Set("anthropic-version", "2023-06-01")
	httpReq.Header.Set("anthropic-beta", "prompt-caching-2024-07-31,interleaved-thinking-2025-05-14")

	resp, err := a.httpClient.Do(httpReq)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		return nil, fmt.Errorf("anthropic: HTTP %d: %s", resp.StatusCode, body)
	}

	ch := make(chan StreamEvent, 32)
	go a.readStream(resp.Body, "anthropic", req.ModelName, EstimateRequestTokens(req.System, req.Messages, req.Tools), ch)
	return ch, nil
}

// ── Request builder ───────────────────────────────────────────────────────────

func (a *AnthropicAdapter) buildBody(req StepRequest) ([]byte, error) {
	messages := make([]map[string]interface{}, 0, len(req.Messages))
	for _, m := range req.Messages {
		switch m.Role {
		case RoleSystem:
			continue // top-level field
		case RoleAssistant:
			if len(m.ToolCalls) > 0 {
				content := make([]interface{}, 0, len(m.ToolCalls)+1)
				if m.Content != "" {
					content = append(content, map[string]string{"type": "text", "text": m.Content})
				}
				for _, tc := range m.ToolCalls {
					var args interface{}
					_ = json.Unmarshal([]byte(tc.ArgsJSON), &args)
					content = append(content, map[string]interface{}{
						"type":  "tool_use",
						"id":    tc.ID,
						"name":  tc.Name,
						"input": args,
					})
				}
				messages = append(messages, map[string]interface{}{"role": "assistant", "content": content})
			} else {
				messages = append(messages, map[string]interface{}{"role": "assistant", "content": m.Content})
			}
		case RoleTool:
			messages = append(messages, map[string]interface{}{
				"role": "user",
				"content": []map[string]interface{}{{
					"type":        "tool_result",
					"tool_use_id": m.ToolCallID,
					"content":     m.Content,
				}},
			})
		default:
			if len(m.Images) > 0 {
				// Multimodal user turn: image blocks first, then the text —
				// Anthropic's docs recommend images before the question.
				content := make([]interface{}, 0, len(m.Images)+1)
				for _, img := range m.Images {
					content = append(content, map[string]interface{}{
						"type": "image",
						"source": map[string]interface{}{
							"type":       "base64",
							"media_type": img.MediaType,
							"data":       img.DataB64,
						},
					})
				}
				if m.Content != "" {
					content = append(content, map[string]string{"type": "text", "text": m.Content})
				}
				messages = append(messages, map[string]interface{}{"role": "user", "content": content})
			} else {
				messages = append(messages, map[string]interface{}{"role": "user", "content": m.Content})
			}
		}
	}

	maxTokens := req.MaxTokens
	if maxTokens == 0 {
		maxTokens = 8192
	}

	body := map[string]interface{}{
		"model":      req.ModelName,
		"max_tokens": maxTokens,
		"stream":     true,
		"messages":   messages,
	}
	if req.System != "" {
		body["system"] = buildCachedSystemBlocks(req.System)
	}
	if req.Temperature != nil {
		body["temperature"] = *req.Temperature
	}
	if req.TopP != nil {
		body["top_p"] = *req.TopP
	}
	if len(req.StopSequences) > 0 {
		body["stop_sequences"] = req.StopSequences
	}

	// Reasoning depth. Anthropic nests this under output_config (NOT top-level),
	// and it is GA on Opus 4.6+/Sonnet 4.6+ — no beta header. ResolveEffort
	// drops it for models that reject the parameter (e.g. Haiku 4.5).
	if effort := ResolveEffort(req.ModelName, req.Effort); effort != "" {
		body["output_config"] = map[string]interface{}{"effort": string(effort)}
	}

	tools := a.buildTools(req.Tools)
	if len(tools) > 0 {
		body["tools"] = tools
	}

	// ProviderOptions override any field above — applied last.
	for k, v := range req.ProviderOptions {
		body[k] = v
	}

	return json.Marshal(body)
}

// buildCachedSystemBlocks splits the system prompt at the static/dynamic boundary
// and marks the stable prefix with cache_control so Anthropic caches it across turns.
func buildCachedSystemBlocks(system string) []map[string]interface{} {
	const boundary = "\n---\nEnvironment:"
	idx := strings.Index(system, boundary)
	if idx < 0 {
		return []map[string]interface{}{{"type": "text", "text": system}}
	}
	staticPart := system[:idx]
	dynamicPart := system[idx:]
	return []map[string]interface{}{
		{
			"type":          "text",
			"text":          staticPart,
			"cache_control": map[string]string{"type": "ephemeral"},
		},
		{
			"type": "text",
			"text": dynamicPart,
		},
	}
}

// buildTools passes the caller's tool list through unchanged. web_search and
// fetch_url are plain function tools here — the pipeline intercepts them and
// executes via the webtools package (a Haiku sub-call with Anthropic's native
// search/fetch server tools), so the main model never carries raw search
// results in its context.
func (a *AnthropicAdapter) buildTools(callerTools []ToolDefinition) []map[string]interface{} {
	result := make([]map[string]interface{}, 0, len(callerTools))
	for _, t := range callerTools {
		result = append(result, map[string]interface{}{
			"name":         t.Name,
			"description":  t.Description,
			"input_schema": t.Parameters,
		})
	}
	return result
}

// ── SSE stream reader ─────────────────────────────────────────────────────────

type anthropicSSEData struct {
	Type  string `json:"type"`
	Index int    `json:"index"`

	ContentBlock *struct {
		Type  string      `json:"type"`
		ID    string      `json:"id"`
		Name  string      `json:"name"`
		Input interface{} `json:"input"`
	} `json:"content_block"`
	Delta *struct {
		Type        string `json:"type"`
		Text        string `json:"text"`
		PartialJSON string `json:"partial_json"`
		Thinking    string `json:"thinking"`
	} `json:"delta"`

	Usage *struct {
		OutputTokens int64 `json:"output_tokens"`
	} `json:"usage"`

	Message *struct {
		Usage struct {
			InputTokens              int64 `json:"input_tokens"`
			OutputTokens             int64 `json:"output_tokens"`
			CacheReadInputTokens     int64 `json:"cache_read_input_tokens"`
			CacheCreationInputTokens int64 `json:"cache_creation_input_tokens"`
		} `json:"usage"`
	} `json:"message"`
}

func (a *AnthropicAdapter) readStream(body io.ReadCloser, provider, model string, estInput int64, ch chan<- StreamEvent) {
	defer body.Close()
	defer close(ch)

	scanner := bufio.NewScanner(body)
	scanner.Buffer(make([]byte, 64*1024), 64*1024)

	type pendingTool struct {
		id       string
		name     string
		argsJSON strings.Builder
	}
	// Client-side tool calls (type: "tool_use") — delegated to the client for execution.
	pending := map[int]*pendingTool{}
	// Server-side tool blocks (type: "server_tool_use", "web_search_tool_result") —
	// executed by Anthropic's infrastructure, never sent to the client.
	serverBlocks := map[int]bool{}
	result := &StepResult{}

	for scanner.Scan() {
		line := scanner.Text()
		if !strings.HasPrefix(line, "data: ") {
			continue
		}
		payload := line[6:]
		if payload == "[DONE]" {
			break
		}

		var ev anthropicSSEData
		if err := json.Unmarshal([]byte(payload), &ev); err != nil {
			continue
		}

		switch ev.Type {
		case "message_start":
			if ev.Message != nil {
				// Anthropic reports input_tokens, cache_read, and cache_creation
				// as SEPARATE fields (input_tokens excludes the cached ones).
				// Cache-creation (writes) are billed at a surcharge, so fold
				// them into the non-cached input bucket rather than losing them.
				result.InputTokens = ev.Message.Usage.InputTokens + ev.Message.Usage.CacheCreationInputTokens
				result.CachedInputTokens = ev.Message.Usage.CacheReadInputTokens
			}

		case "content_block_start":
			if ev.ContentBlock == nil {
				continue
			}
			switch ev.ContentBlock.Type {
			case "tool_use":
				// Client-delegated tool — accumulate args, emit start event.
				pending[ev.Index] = &pendingTool{
					id:   ev.ContentBlock.ID,
					name: ev.ContentBlock.Name,
				}
				ch <- StreamEvent{
					Kind: EventToolStart,
					ToolCall: &ToolCall{
						ID:   ev.ContentBlock.ID,
						Name: ev.ContentBlock.Name,
					},
				}
			case "server_tool_use":
				// Anthropic executes this server-side (e.g. web_search).
				// Emit a UI event so the client can show a spinner, but do NOT
				// add to pending — no client execution is needed.
				serverBlocks[ev.Index] = true
				if ev.ContentBlock.Name != "" {
					ch <- StreamEvent{
						Kind: EventToolStart,
						ToolCall: &ToolCall{
							ID:   ev.ContentBlock.ID,
							Name: ev.ContentBlock.Name,
						},
					}
				}
			case "web_search_tool_result":
				// Search results returned by Anthropic's infrastructure.
				// These are consumed internally; mark as server block to skip.
				serverBlocks[ev.Index] = true
			}

		case "content_block_delta":
			if ev.Delta == nil {
				continue
			}
			if serverBlocks[ev.Index] {
				continue // server block — no client-side accumulation
			}
			switch ev.Delta.Type {
			case "text_delta":
				result.Content += ev.Delta.Text
				ch <- StreamEvent{Kind: EventTextDelta, Text: ev.Delta.Text}
			case "thinking_delta":
				ch <- StreamEvent{Kind: EventThinking, Text: ev.Delta.Thinking}
			case "input_json_delta":
				if p, ok := pending[ev.Index]; ok {
					p.argsJSON.WriteString(ev.Delta.PartialJSON)
				}
			}

		case "content_block_stop":
			if serverBlocks[ev.Index] {
				delete(serverBlocks, ev.Index)
				continue
			}
			if p, ok := pending[ev.Index]; ok {
				result.ToolCalls = append(result.ToolCalls, ToolCall{
					ID:       p.id,
					Name:     p.name,
					ArgsJSON: p.argsJSON.String(),
				})
				delete(pending, ev.Index)
			}

		case "message_delta":
			if ev.Usage != nil {
				result.OutputTokens = ev.Usage.OutputTokens
			}

		case "message_stop":
			ch <- StreamEvent{Kind: EventDone, Result: finalizeUsage(provider, model, result, estInput)}
			return

		case "error":
			ch <- StreamEvent{Kind: EventError, Err: fmt.Errorf("anthropic stream error: %s", payload)}
			return
		}
	}

	if err := scanner.Err(); err != nil {
		ch <- StreamEvent{Kind: EventError, Err: fmt.Errorf("anthropic stream read: %w", err)}
		return
	}

	ch <- StreamEvent{Kind: EventDone, Result: finalizeUsage(provider, model, result, estInput)}
}
