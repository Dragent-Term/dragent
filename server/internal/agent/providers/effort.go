package providers

import "strings"

// Effort is the user-selected reasoning depth for a task. It is deliberately
// limited to three levels: they are the only ones every effort-capable provider
// in this codebase understands, and they map cleanly onto each provider's own
// vocabulary (see effortWireValue).
//
// Providers spell this parameter differently — Anthropic nests it under
// output_config.effort, OpenAI-compatible APIs take a top-level
// reasoning_effort — so the wire mapping lives in each adapter's buildBody.
// What lives HERE is the one question every call site needs answered: is this
// level valid for this model at all?
type Effort string

const (
	EffortLow    Effort = "low"
	EffortMedium Effort = "medium"
	EffortHigh   Effort = "high"
)

// Valid reports whether e is one of the three levels (empty = unset, which is
// always valid and means "send nothing, take the provider default").
func (e Effort) Valid() bool {
	switch e {
	case "", EffortLow, EffortMedium, EffortHigh:
		return true
	}
	return false
}

// ModelSupportsEffort reports whether the model accepts a reasoning-effort
// parameter AT ALL under the request shape this pipeline actually sends.
//
// The second clause matters: the agent pipeline always sends function tools, so
// a model that only accepts effort on tool-less requests counts as unsupported
// here. Sending it anyway is not a no-op — several providers hard-reject it:
//
//   - claude-haiku-4-5 errors on the effort parameter (it is an Opus/Sonnet
//     4.6+ feature; Sonnet 4.5 and Haiku 4.5 reject it).
//   - gpt-5.6-* rejects any reasoning_effort other than "none" once function
//     tools are present on Chat Completions — the OpenAI adapter pins "none"
//     for exactly this reason, and a user-chosen level must not override it.
//
// Keep this the ONLY place that answers the question. The HTTP handler, the
// /api/models capability payload, and the adapters all route through it, so a
// model added to one list can't silently disagree with another.
func ModelSupportsEffort(modelName string) bool {
	m := strings.ToLower(strings.TrimSpace(modelName))

	switch {
	// ── Anthropic: output_config.effort ──────────────────────────────────
	// GA on Opus 4.5+ and Sonnet 4.6+. Opus 4.5 takes low/medium/high (it
	// only lacks xhigh/max, which we don't offer). Haiku 4.5 and Sonnet 4.5
	// REJECT the parameter outright.
	case strings.HasPrefix(m, "claude-opus-"),
		strings.HasPrefix(m, "claude-fable-"),
		strings.HasPrefix(m, "claude-mythos-"):
		return true
	case strings.HasPrefix(m, "claude-sonnet-4-6"),
		strings.HasPrefix(m, "claude-sonnet-5"):
		return true

	// ── OpenAI: reasoning_effort ─────────────────────────────────────────
	// gpt-5.6 is the one exclusion: it rejects any value but "none" once
	// function tools are present, and this pipeline always sends tools.
	case strings.HasPrefix(m, "gpt-5.6-"):
		return false
	case strings.HasPrefix(m, "gpt-5."):
		return true

	// ── Google: generationConfig.thinkingConfig.thinkingLevel ────────────
	// Gemini 3.x accepts low/medium/high. Gemini 2.5 predates thinkingLevel
	// (it only has the legacy thinkingBudget), and mixing the two is a 400.
	case strings.HasPrefix(m, "gemini-3"):
		return true

	// ── xAI: reasoning_effort (OpenAI-compatible) ────────────────────────
	// grok-4.3 and grok-4.5 accept low/medium/high. Plain grok-4 ERRORS on
	// the parameter, so this must stay an explicit allowlist rather than a
	// "grok-4" prefix match. grok-build-0.1 is unverified.
	case strings.HasPrefix(m, "grok-4.3"), strings.HasPrefix(m, "grok-4.5"):
		return true

	// ── Meta Muse Spark: reasoning_effort (OpenAI-compatible) ────────────
	// Reasoning family; low/medium/high apply. Llama names are NOT Meta's
	// (retired preview, third-party hosts) and stay unsupported here.
	case strings.HasPrefix(m, "muse-spark"):
		return true
	}

	// Anything else (Gemini 2.5, plain grok-4, Groq/Llama, Ollama, LM Studio,
	// custom OpenAI-compatible endpoints) has no verified mapping. Default to
	// unsupported: a wrong guess is a hard request failure at generation time,
	// not a degraded response.
	return false
}

// ResolveEffort returns the effort to send for a model, dropping it when the
// model can't accept one. Adapters call this instead of reading req.Effort
// directly, so an unsupported pairing degrades to the provider default rather
// than failing the request.
func ResolveEffort(modelName string, e Effort) Effort {
	if e == "" || !e.Valid() || !ModelSupportsEffort(modelName) {
		return ""
	}
	return e
}
