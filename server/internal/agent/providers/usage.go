package providers

import (
	"log"
	"unicode/utf8"
)

// ── Token usage normalization & validation ────────────────────────────────────
//
// Billing accuracy depends on every provider reporting input / cached / output
// tokens correctly. Providers differ in what they include, sometimes omit usage
// entirely on truncated streams, and occasionally emit garbage. This module is
// the single choke point that guarantees a StepResult carries sane, billable
// token counts before it leaves an adapter.

// maxSaneTokens caps a single turn's token count. Real turns never approach
// this; a value above it means a provider bug or malformed payload, which we
// clamp (and log) rather than bill.
const maxSaneTokens int64 = 50_000_000

// estimateTokens approximates token count from text length (~4 chars/token).
// Only used as a last resort when a provider returns no usage at all, so a
// real call is never billed as zero (a silent revenue leak).
func estimateTokens(s string) int64 {
	if s == "" {
		return 0
	}
	return int64(utf8.RuneCountInString(s)+3) / 4
}

// clampToken forces a single token field into [0, maxSaneTokens], logging when
// it has to correct an out-of-range value so provider bugs are visible.
func clampToken(field string, v int64, model string) int64 {
	if v < 0 {
		log.Printf("[usage] %s negative (%d) from %s — clamped to 0", field, v, model)
		return 0
	}
	if v > maxSaneTokens {
		log.Printf("[usage] %s absurd (%d) from %s — clamped to %d", field, v, model, maxSaneTokens)
		return maxSaneTokens
	}
	return v
}

// finalizeUsage validates a StepResult's token counts and, when a provider
// reported none at all, falls back to estimates so the call is still billed.
// `estInputTokens` is a length-based estimate of the request the caller sent
// (system + messages + tool schemas); `model` is for logs.
//
// Never returns an error: billing must not fail because a provider
// under-reported — it bills a safe estimate instead of dropping the charge.
func finalizeUsage(provider, model string, r *StepResult, estInputTokens int64) *StepResult {
	if r == nil {
		return &StepResult{}
	}
	r.InputTokens = clampToken("input_tokens", r.InputTokens, model)
	r.CachedInputTokens = clampToken("cached_input_tokens", r.CachedInputTokens, model)
	r.OutputTokens = clampToken("output_tokens", r.OutputTokens, model)

	producedWork := r.Content != "" || len(r.ToolCalls) > 0
	// Track whether each field is the provider's real count or a fallback
	// estimate, so the log is unambiguous about the source.
	inputSource := "provider"
	outputSource := "provider"
	if producedWork && r.InputTokens == 0 && r.CachedInputTokens == 0 && estInputTokens > 0 {
		log.Printf("[usage] %s reported no input tokens — estimated %d from request length", model, estInputTokens)
		r.InputTokens = estInputTokens
		inputSource = "estimated"
	}
	if producedWork && r.OutputTokens == 0 {
		out := r.Content
		for _, tc := range r.ToolCalls {
			out += tc.Name + tc.ArgsJSON
		}
		if est := estimateTokens(out); est > 0 {
			log.Printf("[usage] %s reported no output tokens — estimated %d from output length", model, est)
			r.OutputTokens = est
			outputSource = "estimated"
		}
	}

	// Per-provider token log — one line per LLM call, for EVERY provider, so
	// the exact input/cached/output the provider returned is auditable. The
	// source tags make it explicit whether each count came straight from the
	// provider or fell back to a length estimate (only when the provider
	// reported nothing).
	log.Printf(
		"[provider-tokens] provider=%s model=%s input=%d cached_input=%d output=%d total=%d input_src=%s output_src=%s",
		provider, model,
		r.InputTokens, r.CachedInputTokens, r.OutputTokens,
		r.InputTokens+r.CachedInputTokens+r.OutputTokens,
		inputSource, outputSource,
	)
	return r
}

// EstimateRequestTokens approximates the input token count of a request from
// its system prompt, message contents, and tool schema names. Used only as the
// zero-usage fallback input estimate — never overrides a real provider count.
func EstimateRequestTokens(system string, messages []Message, tools []ToolDefinition) int64 {
	total := estimateTokens(system)
	for _, m := range messages {
		total += estimateTokens(m.Content)
		// Images are billed as vision tokens; use the conservative per-image
		// ceiling so the pre-flight gate can't under-lock capacity.
		total += int64(len(m.Images)) * EstImageTokens
	}
	for _, t := range tools {
		total += estimateTokens(t.Name) + estimateTokens(t.Description)
	}
	return total
}
