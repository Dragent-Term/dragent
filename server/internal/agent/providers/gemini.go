package providers

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"time"
)

const geminiBaseURL = "https://generativelanguage.googleapis.com/v1beta"

// GeminiAdapter implements LLMProvider for Google Gemini via the generateContent API.
type GeminiAdapter struct {
	apiKey     string
	httpClient *http.Client
}

func NewGeminiAdapter(apiKey string) *GeminiAdapter {
	return &GeminiAdapter{
		apiKey:     apiKey,
		httpClient: &http.Client{Timeout: 120 * time.Second},
	}
}

func (g *GeminiAdapter) ProviderName() string { return "gemini" }

// SupportedTools returns nil: web tooling is served by the pipeline's
// server-side webtools executor, not injected inline by the adapter.
func (g *GeminiAdapter) SupportedTools() []ToolDefinition { return nil }

func (g *GeminiAdapter) Capabilities() ProviderCapabilities {
	return ProviderCapabilities{
		Streaming:        false,
		ToolCalling:      true,
		Thinking:         false,
		ImageInput:       true,
		StructuredOutput: false,
		Caching:          false,
	}
}

func (g *GeminiAdapter) StreamStep(ctx context.Context, req StepRequest) (<-chan StreamEvent, error) {
	ch := make(chan StreamEvent, 8)
	go func() {
		defer close(ch)
		result, err := g.singleStep(ctx, req)
		if err != nil {
			ch <- StreamEvent{Kind: EventError, Err: err}
			return
		}
		if len(result.ToolCalls) > 0 {
			for i := range result.ToolCalls {
				ch <- StreamEvent{Kind: EventToolStart, ToolCall: &result.ToolCalls[i]}
			}
		} else {
			ch <- StreamEvent{Kind: EventTextDelta, Text: result.Content}
		}
		ch <- StreamEvent{Kind: EventDone, Result: result}
	}()
	return ch, nil
}

// buildBody assembles the generateContent request. Extracted from singleStep so
// the wire shape (notably generationConfig.thinkingConfig) is unit-testable,
// matching the buildBody method the other adapters expose.
func (g *GeminiAdapter) buildBody(req StepRequest) ([]byte, error) {
	contents := make([]map[string]interface{}, 0, len(req.Messages))
	for _, m := range req.Messages {
		if m.Role == RoleSystem {
			continue
		}
		role := "user"
		if m.Role == RoleAssistant {
			role = "model"
		}
		if m.Role == RoleTool {
			contents = append(contents, map[string]interface{}{
				"role": "user",
				"parts": []map[string]interface{}{{
					"functionResponse": map[string]interface{}{
						"name":     cleanToolName(m.ToolCallID),
						"response": map[string]interface{}{"content": m.Content},
					},
				}},
			})
			continue
		}
		if len(m.ToolCalls) > 0 {
			parts := make([]map[string]interface{}, len(m.ToolCalls))
			for i, tc := range m.ToolCalls {
				var args interface{}
				_ = json.Unmarshal([]byte(tc.ArgsJSON), &args)
				parts[i] = map[string]interface{}{
					"functionCall": map[string]interface{}{"name": tc.Name, "args": args},
				}
			}
			contents = append(contents, map[string]interface{}{"role": role, "parts": parts})
		} else if len(m.Images) > 0 {
			// Multimodal user turn: inline_data image parts, then the text.
			parts := make([]map[string]interface{}, 0, len(m.Images)+1)
			for _, img := range m.Images {
				parts = append(parts, map[string]interface{}{
					"inlineData": map[string]string{
						"mimeType": img.MediaType,
						"data":     img.DataB64,
					},
				})
			}
			if m.Content != "" {
				parts = append(parts, map[string]interface{}{"text": m.Content})
			}
			contents = append(contents, map[string]interface{}{"role": role, "parts": parts})
		} else {
			contents = append(contents, map[string]interface{}{
				"role":  role,
				"parts": []map[string]string{{"text": m.Content}},
			})
		}
	}

	generationConfig := map[string]interface{}{}
	if req.MaxTokens > 0 {
		generationConfig["maxOutputTokens"] = req.MaxTokens
	}
	if req.Temperature != nil {
		generationConfig["temperature"] = *req.Temperature
	}
	if req.TopP != nil {
		generationConfig["topP"] = *req.TopP
	}
	if len(req.StopSequences) > 0 {
		generationConfig["stopSequences"] = req.StopSequences
	}
	// Reasoning depth. On the generateContent API this is
	// generationConfig.thinkingConfig.thinkingLevel — NOT the flat
	// generation_config.thinking_level shape, which belongs to the separate
	// Interactions API. ResolveEffort limits this to Gemini 3.x: 2.5 only has
	// the legacy thinkingBudget, and sending both in one request is a 400.
	if effort := ResolveEffort(req.ModelName, req.Effort); effort != "" {
		generationConfig["thinkingConfig"] = map[string]interface{}{
			"thinkingLevel": string(effort),
		}
	}

	body := map[string]interface{}{"contents": contents}
	if len(generationConfig) > 0 {
		body["generationConfig"] = generationConfig
	}
	if req.System != "" {
		body["system_instruction"] = map[string]interface{}{
			"parts": []map[string]string{{"text": req.System}},
		}
	}
	if len(req.Tools) > 0 {
		var toolEntries []map[string]interface{}
		var functionDecls []map[string]interface{}
		// web_search stays a plain function declaration — the pipeline executes
		// it server-side (Google grounding on a Flash sub-call via webtools).
		for _, t := range req.Tools {
			functionDecls = append(functionDecls, map[string]interface{}{
				"name":        t.Name,
				"description": t.Description,
				"parameters":  t.Parameters,
			})
		}
		if len(functionDecls) > 0 {
			toolEntries = append(toolEntries, map[string]interface{}{
				"functionDeclarations": functionDecls,
			})
		}
		if len(toolEntries) > 0 {
			body["tools"] = toolEntries
		}
	}
	// ProviderOptions override any field above — applied last.
	for k, v := range req.ProviderOptions {
		body[k] = v
	}

	return json.Marshal(body)
}

func (g *GeminiAdapter) singleStep(ctx context.Context, req StepRequest) (*StepResult, error) {
	data, err := g.buildBody(req)
	if err != nil {
		return nil, err
	}
	url := fmt.Sprintf("%s/models/%s:generateContent?key=%s", geminiBaseURL, req.ModelName, g.apiKey)
	httpReq, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := g.httpClient.Do(httpReq)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		b, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("gemini: HTTP %d: %s", resp.StatusCode, b)
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
			GroundingMetadata *struct {
				GroundingChunks []struct {
					Web *struct {
						URI   string `json:"uri"`
						Title string `json:"title"`
					} `json:"web"`
				} `json:"groundingChunks"`
			} `json:"groundingMetadata"`
		} `json:"candidates"`
		UsageMetadata struct {
			PromptTokenCount        int64 `json:"promptTokenCount"`
			CandidatesTokenCount    int64 `json:"candidatesTokenCount"`
			CachedContentTokenCount int64 `json:"cachedContentTokenCount"`
			// ThoughtsTokenCount is the reasoning ("thinking") tokens for
			// thinking-capable models (Gemini 2.5+). Google counts it SEPARATELY
			// from candidatesTokenCount (totalTokenCount = prompt + thoughts +
			// candidates) and bills it as OUTPUT — so it MUST be added to the
			// output count or thinking-model turns are undercharged.
			ThoughtsTokenCount int64 `json:"thoughtsTokenCount"`
			// ToolUsePromptTokenCount and TotalTokenCount are read only to
			// reconcile our accounting against Google's own total (below).
			// toolUsePromptTokenCount is documented as a breakdown already inside
			// promptTokenCount (NOT additive) — the reconciliation guard verifies
			// that assumption on real responses.
			ToolUsePromptTokenCount int64 `json:"toolUsePromptTokenCount"`
			TotalTokenCount         int64 `json:"totalTokenCount"`
		} `json:"usageMetadata"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&raw); err != nil {
		return nil, fmt.Errorf("gemini: decode: %w", err)
	}

	// Gemini's promptTokenCount INCLUDES cached content — split so cached is
	// billed at its own rate.
	cached := raw.UsageMetadata.CachedContentTokenCount
	nonCachedInput := raw.UsageMetadata.PromptTokenCount - cached
	if nonCachedInput < 0 {
		nonCachedInput = 0
	}
	// Output = visible response + thinking tokens (both billed at the output
	// rate; Google reports thoughts separately from candidates).
	result := &StepResult{
		InputTokens:       nonCachedInput,
		CachedInputTokens: cached,
		OutputTokens:      raw.UsageMetadata.CandidatesTokenCount + raw.UsageMetadata.ThoughtsTokenCount,
	}

	// Reconciliation guard. Google documents totalTokenCount = prompt + thoughts
	// + candidates, with toolUsePromptTokenCount a breakdown ALREADY inside
	// promptTokenCount (so we must not add it — that would double-count). If that
	// identity ever fails on a real response — a field becoming additive, a new
	// billable field appearing — our token accounting has drifted and billing is
	// wrong. Log it loudly so we catch it from production traffic instead of
	// guessing from prose docs. Only checked when Google actually returns a total.
	if um := raw.UsageMetadata; um.TotalTokenCount > 0 {
		accounted := um.PromptTokenCount + um.ThoughtsTokenCount + um.CandidatesTokenCount
		if accounted != um.TotalTokenCount {
			log.Printf(
				"[usage] gemini token reconciliation MISMATCH model=%s: prompt=%d cached=%d candidates=%d thoughts=%d toolUse=%d accounted=%d total=%d delta=%d — a usage field may be additive/unaccounted; billing may be off, review gemini.go extraction",
				req.ModelName,
				um.PromptTokenCount, um.CachedContentTokenCount, um.CandidatesTokenCount,
				um.ThoughtsTokenCount, um.ToolUsePromptTokenCount,
				accounted, um.TotalTokenCount, um.TotalTokenCount-accounted,
			)
		}
	}
	if len(raw.Candidates) > 0 {
		cand := raw.Candidates[0]
		for _, part := range cand.Content.Parts {
			if part.FunctionCall != nil {
				argsJSON, _ := json.Marshal(part.FunctionCall.Args)
				result.ToolCalls = append(result.ToolCalls, ToolCall{
					ID:       fmt.Sprintf("%s_%d", part.FunctionCall.Name, len(result.ToolCalls)),
					Name:     part.FunctionCall.Name,
					ArgsJSON: string(argsJSON),
				})
			} else if part.Text != "" {
				result.Content += part.Text
			}
		}
		// Append grounding sources when Google Search was used.
		if cand.GroundingMetadata != nil && len(cand.GroundingMetadata.GroundingChunks) > 0 {
			result.Content += "\n\n**Sources:**\n"
			for i, chunk := range cand.GroundingMetadata.GroundingChunks {
				if chunk.Web != nil {
					title := chunk.Web.Title
					if title == "" {
						title = chunk.Web.URI
					}
					result.Content += fmt.Sprintf("%d. [%s](%s)\n", i+1, title, chunk.Web.URI)
				}
			}
		}
	}
	return finalizeUsage("gemini", req.ModelName, result, EstimateRequestTokens(req.System, req.Messages, req.Tools)), nil
}

func cleanToolName(id string) string {
	// Strip the trailing _<digits> suffix if present
	for i := len(id) - 1; i >= 0; i-- {
		if id[i] == '_' {
			// Check if all characters after i are digits
			isDigits := true
			for j := i + 1; j < len(id); j++ {
				if id[j] < '0' || id[j] > '9' {
					isDigits = false
					break
				}
			}
			if isDigits && i < len(id)-1 {
				return id[:i]
			}
			break
		}
	}
	return id
}
