package providers

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// OpenAIAdapter implements LLMProvider using the OpenAI Chat Completions API
// with native SSE streaming. Correct for gpt-4o and other non-reasoning models.
// The Responses API (/v1/responses) is only for o3/o4/gpt-5+.
type OpenAIAdapter struct {
	apiKey     string
	httpClient *http.Client
}

func NewOpenAIAdapter(apiKey string) *OpenAIAdapter {
	return &OpenAIAdapter{
		apiKey:     apiKey,
		httpClient: &http.Client{Timeout: 120 * time.Second},
	}
}

func (o *OpenAIAdapter) ProviderName() string { return "openai" }

// SupportedTools returns nil: web tooling is served by the pipeline's
// server-side webtools executor, not injected inline by the adapter.
func (o *OpenAIAdapter) SupportedTools() []ToolDefinition { return nil }

func (o *OpenAIAdapter) Capabilities() ProviderCapabilities {
	return ProviderCapabilities{
		Streaming:        true,
		ToolCalling:      true,
		Thinking:         false, // o-series thinking is internal, not exposed via streaming
		ImageInput:       true,
		StructuredOutput: true,
		Caching:          false,
	}
}

func (o *OpenAIAdapter) StreamStep(ctx context.Context, req StepRequest) (<-chan StreamEvent, error) {
	body, err := o.buildBody(req)
	if err != nil {
		return nil, fmt.Errorf("openai: build request body: %w", err)
	}

	ch := make(chan StreamEvent, 32)
	go func() {
		resp, err := doStreamRequest(ctx, o.httpClient, func() (*http.Request, error) {
			r, err := http.NewRequestWithContext(ctx, "POST", "https://api.openai.com/v1/chat/completions", bytes.NewReader(body))
			if err != nil {
				return nil, err
			}
			r.Header.Set("Content-Type", "application/json")
			r.Header.Set("Accept", "text/event-stream")
			r.Header.Set("Authorization", "Bearer "+o.apiKey)
			return r, nil
		}, "openai")
		if err != nil {
			ch <- StreamEvent{Kind: EventError, Err: err}
			close(ch)
			return
		}
		if resp.StatusCode != http.StatusOK {
			b, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			ch <- StreamEvent{Kind: EventError, Err: fmt.Errorf("openai: HTTP %d: %s", resp.StatusCode, b)}
			close(ch)
			return
		}
		readChatCompletionsStream(resp.Body, "openai", req.ModelName, EstimateRequestTokens(req.System, req.Messages, req.Tools), ch)
	}()
	return ch, nil
}

// isStaleConnError detects transport-level errors from reusing a connection the
// server already closed. Retrying once with a fresh connection fixes them.
func isStaleConnError(err error) bool {
	s := err.Error()
	return strings.Contains(s, "bad record MAC") ||
		strings.Contains(s, "connection reset") ||
		strings.Contains(s, "broken pipe") ||
		strings.Contains(s, "EOF")
}

func (o *OpenAIAdapter) buildBody(req StepRequest) ([]byte, error) {
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
	// User-selected reasoning depth. ResolveEffort returns "" for gpt-5.6-*, so
	// this can never fight the "none" pin applied below for that family.
	if effort := ResolveEffort(req.ModelName, req.Effort); effort != "" {
		body["reasoning_effort"] = string(effort)
	}
	if len(tools) > 0 {
		body["tools"] = tools
		// GPT-5.6 variants reject reasoning_effort values other than "none"
		// when function tools are sent through Chat Completions. The pipeline
		// executes tools itself, so disable provider-side reasoning for this
		// request shape. This pin is deliberately unconditional — it must win
		// over any user selection, which is why ModelSupportsEffort excludes
		// this family outright.
		if strings.HasPrefix(req.ModelName, "gpt-5.6-") {
			body["reasoning_effort"] = "none"
		}
	}
	if req.MaxTokens > 0 {
		body["max_completion_tokens"] = req.MaxTokens
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
	// ProviderOptions override any field above — applied last.
	for k, v := range req.ProviderOptions {
		body[k] = v
	}

	return json.Marshal(body)
}
