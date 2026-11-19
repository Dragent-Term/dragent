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

	"server/internal/platform/netguard"
)

// chatCompletionChunk is the SSE event format shared by ALL Chat Completions providers:
// OpenAI, Groq, OpenRouter, Ollama, LM Studio, Cerebras, Azure OpenAI, and more.
type chatCompletionChunk struct {
	Choices []struct {
		Delta struct {
			Content   *string `json:"content"`
			ToolCalls []struct {
				Index    int    `json:"index"`
				ID       string `json:"id"`
				Function struct {
					Name      string `json:"name"`
					Arguments string `json:"arguments"`
				} `json:"function"`
			} `json:"tool_calls"`
		} `json:"delta"`
		FinishReason *string `json:"finish_reason"`
	} `json:"choices"`
	Usage *struct {
		PromptTokens        int64 `json:"prompt_tokens"`
		CompletionTokens    int64 `json:"completion_tokens"`
		PromptTokensDetails *struct {
			CachedTokens int64 `json:"cached_tokens"`
		} `json:"prompt_tokens_details"`
	} `json:"usage"`
}

// readChatCompletionsStream parses a streaming Chat Completions SSE body and emits
// events on ch. Called by OpenAIAdapter and OpenAICompatAdapter — any provider that
// follows the Chat Completions streaming format can share this reader.
//
// Tool call lifecycle:
//   - EventToolStart  emitted as soon as the LLM outputs the call ID + name (early,
//     before args have fully streamed) — runner uses this for immediate UI feedback.
//   - Tool args accumulated via delta chunks.
//   - On finish_reason:"tool_calls" (or [DONE] fallback) args are finalized and
//     attached to result.ToolCalls for the runner to dispatch.
func readChatCompletionsStream(body io.ReadCloser, provider, model string, estInput int64, ch chan<- StreamEvent) {
	defer body.Close()
	defer close(ch)

	scanner := bufio.NewScanner(body)
	scanner.Buffer(make([]byte, 64*1024), 64*1024)

	type pendingCall struct {
		id      string
		name    string
		args    strings.Builder
		started bool // true once EventToolStart has been emitted for this call
	}
	byIndex := map[int]*pendingCall{}
	result := &StepResult{}

	// finalizePending converts all accumulated calls into result.ToolCalls.
	// Called on finish_reason:"tool_calls" and again at [DONE] as a safety net.
	finalizePending := func() {
		if len(byIndex) == 0 {
			return
		}
		// Collect indices in order — OpenAI always uses sequential 0,1,2,…
		indices := make([]int, 0, len(byIndex))
		for idx := range byIndex {
			indices = append(indices, idx)
		}
		// Sort ascending without importing sort: simple insertion sort (N is tiny).
		for i := 1; i < len(indices); i++ {
			for j := i; j > 0 && indices[j] < indices[j-1]; j-- {
				indices[j], indices[j-1] = indices[j-1], indices[j]
			}
		}
		for _, idx := range indices {
			p := byIndex[idx]
			if p.id == "" && p.name == "" {
				continue // skip if we somehow got a delta with no identifying info
			}
			argsStr := p.args.String()
			if argsStr == "" {
				argsStr = "{}" // ensure valid JSON for zero-arg tool calls
			}
			result.ToolCalls = append(result.ToolCalls, ToolCall{
				ID:       p.id,
				Name:     p.name,
				ArgsJSON: argsStr,
			})
		}
		byIndex = map[int]*pendingCall{} // clear so a second call is a no-op
	}

	for scanner.Scan() {
		line := scanner.Text()
		if !strings.HasPrefix(line, "data: ") {
			continue
		}
		payload := line[6:]
		if payload == "[DONE]" {
			// Safety net: finalize any pending calls that finish_reason may have missed
			// (e.g. stream cut short or provider omitted finish_reason).
			finalizePending()
			break
		}

		var chunk chatCompletionChunk
		if err := json.Unmarshal([]byte(payload), &chunk); err != nil {
			continue
		}

		// Usage arrives in a trailing chunk where choices is empty.
		if chunk.Usage != nil {
			cached := int64(0)
			if chunk.Usage.PromptTokensDetails != nil {
				cached = chunk.Usage.PromptTokensDetails.CachedTokens
			}
			// prompt_tokens INCLUDES cached tokens in the OpenAI schema — split
			// them so each is billed at its own rate (cached is discounted).
			result.CachedInputTokens = cached
			result.InputTokens = chunk.Usage.PromptTokens - cached
			if result.InputTokens < 0 {
				result.InputTokens = 0
			}
			result.OutputTokens = chunk.Usage.CompletionTokens
		}

		if len(chunk.Choices) == 0 {
			continue
		}

		choice := chunk.Choices[0]

		// Text delta.
		if choice.Delta.Content != nil && *choice.Delta.Content != "" {
			result.Content += *choice.Delta.Content
			ch <- StreamEvent{Kind: EventTextDelta, Text: *choice.Delta.Content}
		}

		// Tool call deltas — accumulate args and emit EventToolStart early.
		for _, tc := range choice.Delta.ToolCalls {
			p, ok := byIndex[tc.Index]
			if !ok {
				p = &pendingCall{}
				byIndex[tc.Index] = p
			}
			if tc.ID != "" {
				p.id = tc.ID
			}
			if tc.Function.Name != "" {
				p.name = tc.Function.Name
			}
			p.args.WriteString(tc.Function.Arguments)

			// Emit EventToolStart as soon as we know both the call ID and name —
			// gives the runner an early signal so the client can show a pending
			// indicator immediately, before args have fully streamed.
			if !p.started && p.id != "" && p.name != "" {
				ch <- StreamEvent{Kind: EventToolStart, ToolCall: &ToolCall{ID: p.id, Name: p.name}}
				p.started = true
			}
		}

		// finish_reason:"tool_calls" is the authoritative signal that all deltas arrived.
		if choice.FinishReason != nil && *choice.FinishReason == "tool_calls" {
			finalizePending()
		}
	}

	if err := scanner.Err(); err != nil {
		ch <- StreamEvent{Kind: EventError, Err: fmt.Errorf("stream read: %w", err)}
		return
	}

	ch <- StreamEvent{Kind: EventDone, Result: finalizeUsage(provider, model, result, estInput)}
}

// ── OpenAICompatAdapter ───────────────────────────────────────────────────────

// OpenAICompatConfig holds connection details for any OpenAI-compatible provider.
type OpenAICompatConfig struct {
	Name         string // display name used in ProviderName() and logs
	BaseURL      string
	APIKey       string
	AuthHeader   string // default: "Authorization"
	AuthPrefix   string // default: "Bearer "
	UserSupplied bool
}

// OpenAICompatAdapter implements LLMProvider using Chat Completions streaming.
// Works with any provider that follows the OpenAI Chat Completions API format.
// Use the dedicated per-provider constructors (NewGroqAdapter, NewOpenRouterAdapter,
// etc.) rather than this directly — they set the correct URL and auth.
type OpenAICompatAdapter struct {
	cfg        OpenAICompatConfig
	httpClient *http.Client
}

func NewOpenAICompatAdapter(cfg OpenAICompatConfig) *OpenAICompatAdapter {
	if cfg.AuthHeader == "" {
		cfg.AuthHeader = "Authorization"
	}
	if cfg.AuthPrefix == "" {
		cfg.AuthPrefix = "Bearer "
	}
	client := &http.Client{Timeout: 120 * time.Second}
	if cfg.UserSupplied {
		client = netguard.NewClient(120 * time.Second)
	}
	return &OpenAICompatAdapter{
		cfg:        cfg,
		httpClient: client,
	}
}

func (c *OpenAICompatAdapter) ProviderName() string {
	if c.cfg.Name != "" {
		return c.cfg.Name
	}
	return "openai-compat:" + c.cfg.BaseURL
}

func (c *OpenAICompatAdapter) SupportedTools() []ToolDefinition { return nil }

func (c *OpenAICompatAdapter) Capabilities() ProviderCapabilities {
	return ProviderCapabilities{
		Streaming:        true,
		ToolCalling:      true,
		Thinking:         false,
		ImageInput:       false,
		StructuredOutput: true,
		Caching:          false,
	}
}

func (c *OpenAICompatAdapter) StreamStep(ctx context.Context, req StepRequest) (<-chan StreamEvent, error) {
	body, err := c.buildBody(req)
	if err != nil {
		return nil, fmt.Errorf("%s: build body: %w", c.ProviderName(), err)
	}

	url := resolveEndpointURL(c.cfg.BaseURL)
	ch := make(chan StreamEvent, 32)
	name := c.ProviderName()
	go func() {
		resp, err := doStreamRequest(ctx, c.httpClient, func() (*http.Request, error) {
			r, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
			if err != nil {
				return nil, err
			}
			r.Header.Set("Content-Type", "application/json")
			r.Header.Set("Accept", "text/event-stream")
			r.Header.Set(c.cfg.AuthHeader, c.cfg.AuthPrefix+c.cfg.APIKey)
			return r, nil
		}, name)
		if err != nil {
			ch <- StreamEvent{Kind: EventError, Err: err}
			close(ch)
			return
		}
		if resp.StatusCode != http.StatusOK {
			b, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			ch <- StreamEvent{Kind: EventError, Err: fmt.Errorf("%s: HTTP %d: %s", name, resp.StatusCode, b)}
			close(ch)
			return
		}
		readChatCompletionsStream(resp.Body, c.ProviderName(), req.ModelName, EstimateRequestTokens(req.System, req.Messages, req.Tools), ch)
	}()
	return ch, nil
}

// resolveEndpointURL returns the URL as-is. The user supplies the full endpoint
// URL (e.g. https://api.example.com/v1/chat/completions or
// https://api.example.com/v1/completions) — we never append a path suffix
// because different providers use different endpoints.
func resolveEndpointURL(baseURL string) string {
	return strings.TrimRight(strings.TrimSpace(baseURL), "/")
}

func (c *OpenAICompatAdapter) buildBody(req StepRequest) ([]byte, error) {
	messages := buildChatCompletionsMessages(req)

	tools := make([]map[string]interface{}, 0, len(req.Tools))
	for _, t := range req.Tools {
		tools = append(tools, map[string]interface{}{
			"type": "function",
			"function": map[string]interface{}{
				"name":        t.Name,
				"description": t.Description,
				"parameters":  t.Parameters,
			},
		})
	}

	body := map[string]interface{}{
		"model":    req.ModelName,
		"messages": messages,
		"stream":   true,
		"stream_options": map[string]interface{}{
			"include_usage": true,
		},
	}
	if len(tools) > 0 {
		body["tools"] = tools
	}
	if req.MaxTokens > 0 {
		body["max_tokens"] = req.MaxTokens
	}
	if req.Temperature != nil {
		body["temperature"] = *req.Temperature
	}
	if req.TopP != nil {
		body["top_p"] = *req.TopP
	}
	if len(req.StopSequences) > 0 {
		body["stop"] = req.StopSequences
	}
	// Reasoning depth. This adapter backs every OpenAI-compatible endpoint
	// (xAI, Groq, Ollama, LM Studio, OpenRouter, Azure, custom), most of which
	// do NOT understand reasoning_effort — ResolveEffort is what keeps this to
	// the models verified to accept it (currently grok-4.3 / grok-4.5).
	if effort := ResolveEffort(req.ModelName, req.Effort); effort != "" {
		body["reasoning_effort"] = string(effort)
	}
	// ProviderOptions override any field above — applied last.
	for k, v := range req.ProviderOptions {
		body[k] = v
	}

	return json.Marshal(body)
}

// buildChatCompletionsMessages converts our internal Message slice to the
// Chat Completions messages array format. Shared by OpenAIAdapter and
// OpenAICompatAdapter since both use identical message structure.
func buildChatCompletionsMessages(req StepRequest) []map[string]interface{} {
	messages := make([]map[string]interface{}, 0, len(req.Messages)+1)

	if req.System != "" {
		messages = append(messages, map[string]interface{}{
			"role":    "system",
			"content": req.System,
		})
	}

	for _, m := range req.Messages {
		switch m.Role {
		case RoleSystem:
			// Already handled above; skip any system message in history.
			continue
		case RoleAssistant:
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
				msg := map[string]interface{}{
					"role":       "assistant",
					"tool_calls": calls,
				}
				if m.Content != "" {
					msg["content"] = m.Content
				}
				messages = append(messages, msg)
			} else {
				messages = append(messages, map[string]interface{}{
					"role":    "assistant",
					"content": m.Content,
				})
			}
		case RoleTool:
			messages = append(messages, map[string]interface{}{
				"role":         "tool",
				"tool_call_id": m.ToolCallID,
				"content":      m.Content,
			})
		default:
			if len(m.Images) > 0 {
				// Multimodal user turn (OpenAI vision format): image parts as
				// data URLs, then the text part.
				parts := make([]map[string]interface{}, 0, len(m.Images)+1)
				for _, img := range m.Images {
					parts = append(parts, map[string]interface{}{
						"type": "image_url",
						"image_url": map[string]string{
							"url": "data:" + img.MediaType + ";base64," + img.DataB64,
						},
					})
				}
				if m.Content != "" {
					parts = append(parts, map[string]interface{}{"type": "text", "text": m.Content})
				}
				messages = append(messages, map[string]interface{}{
					"role":    string(m.Role),
					"content": parts,
				})
			} else {
				messages = append(messages, map[string]interface{}{
					"role":    string(m.Role),
					"content": m.Content,
				})
			}
		}
	}

	return messages
}
