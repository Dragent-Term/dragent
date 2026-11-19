package main

import (
	"net/http"
	"strings"

	"server/internal/embeddings"
	"server/internal/feedback"
)

// defaultMaxBodyBytes caps every request body that no rule below raises.
const defaultMaxBodyBytes int64 = 1 << 20

// agentMaxBodyBytes caps agent requests. The desktop resends the whole
// conversation each step, image attachments included (up to 5 images of
// ~7 MB base64 in one message), so 8 MiB would break real sessions;
// Anthropic itself refuses requests over 32 MB, so nothing larger can work.
const agentMaxBodyBytes int64 = 32 << 20

// bodyRule raises (or, with limit 0, removes) the body cap for a path prefix.
type bodyRule struct {
	prefix string
	limit  int64 // 0: the route enforces its own cap
}

// bodyRules are checked in order; the first matching prefix wins.
var bodyRules = []bodyRule{
	// Webhooks read their own bounded body and must see it whole to verify
	// the signature.
	{prefix: "/api/v1/billing/webhook/", limit: 0},
	// Forwarded apps stream through the session gateway, which caps
	// buffered request bodies itself (8 MiB).
	{prefix: "/forward/", limit: 0},
	{prefix: "/api/v1/agent/", limit: agentMaxBodyBytes},
	{prefix: "/api/v1/haiku/", limit: agentMaxBodyBytes},
	{prefix: "/api/v1/feedback", limit: feedback.MaxPayloadBytes + 1<<20},
	{prefix: "/v1/embeddings", limit: embeddings.MaxBodyBytes},
	// GraphQL agentStep / reportBlockCompleted carry PR diffs and failed
	// command output as text.
	{prefix: "/query", limit: 8 << 20},
	// Relay catalog publish takes up to 4 MiB, host state up to 2 MiB.
	{prefix: "/v1/relay/", limit: 4 << 20},
}

// maxBodyLimit returns the body cap for a path; 0 means none at this layer.
func maxBodyLimit(path string) int64 {
	for _, rule := range bodyRules {
		if strings.HasPrefix(path, rule.prefix) {
			return rule.limit
		}
	}
	// Integration calls proxy up to 2 MiB to the connected service.
	if strings.HasPrefix(path, "/api/v1/organizations/") && strings.Contains(path, "/integrations/") {
		return 2 << 20
	}
	return defaultMaxBodyBytes
}

// limitRequestBodies caps request bodies so no handler can be made to
// buffer an unbounded upload. WebSocket upgrades are left alone (their
// traffic is not the request body), as are bodiless GET/HEAD requests,
// which covers the SSE and WebSocket GET endpoints. POST endpoints that
// answer with a stream still have their request body capped; the cap does
// not touch the response.
func limitRequestBodies(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet || r.Method == http.MethodHead || isWebSocketUpgrade(r) {
			next.ServeHTTP(w, r)
			return
		}
		if limit := maxBodyLimit(r.URL.Path); limit > 0 {
			if r.ContentLength > limit {
				http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
				return
			}
			r.Body = http.MaxBytesReader(w, r.Body, limit)
		}
		next.ServeHTTP(w, r)
	})
}

func isWebSocketUpgrade(r *http.Request) bool {
	return strings.EqualFold(r.Header.Get("Upgrade"), "websocket")
}
