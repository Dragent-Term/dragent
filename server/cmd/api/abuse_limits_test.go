package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"server/graph"
)

func TestMaxBodyLimitRoutes(t *testing.T) {
	cases := map[string]int64{
		"/auth/magic/request":            defaultMaxBodyBytes,
		"/api/v1/organizations/o1/tasks": defaultMaxBodyBytes,
		"/api/v1/agent/tasks":            agentMaxBodyBytes,
		"/api/v1/agent/steps/stream":     agentMaxBodyBytes,
		"/api/v1/agent/compact":          agentMaxBodyBytes,
		"/api/v1/haiku/extract":          agentMaxBodyBytes,
		"/v1/embeddings":                 4 << 20,
		"/query":                         8 << 20,
		"/v1/relay/catalog":              4 << 20,

		"/api/v1/organizations/o1/integrations/c1/request": 2 << 20,
		"/api/v1/billing/webhook/lemonsqueezy":             0,
		"/forward/abc/upload":                              0,
	}
	for path, want := range cases {
		if got := maxBodyLimit(path); got != want {
			t.Errorf("maxBodyLimit(%q) = %d, want %d", path, got, want)
		}
	}
}

// readAll is a handler that reports whether it could read the whole body.
var readAll = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	if _, err := io.ReadAll(r.Body); err != nil {
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			w.WriteHeader(http.StatusRequestEntityTooLarge)
			return
		}
		w.WriteHeader(http.StatusBadRequest)
		return
	}
	w.WriteHeader(http.StatusOK)
})

func bodyStatus(method, path string, size int64, header map[string]string, chunked bool) int {
	body := io.Reader(strings.NewReader(strings.Repeat("x", int(size))))
	if chunked {
		body = io.MultiReader(body) // hides the length: no Content-Length
	}
	r := httptest.NewRequest(method, path, body)
	if chunked {
		r.ContentLength = -1
	}
	for k, v := range header {
		r.Header.Set(k, v)
	}
	w := httptest.NewRecorder()
	limitRequestBodies(readAll).ServeHTTP(w, r)
	return w.Code
}

func TestLimitRequestBodies(t *testing.T) {
	over := defaultMaxBodyBytes + 1
	if got := bodyStatus(http.MethodPost, "/auth/magic/request", over, nil, false); got != http.StatusRequestEntityTooLarge {
		t.Errorf("over default with Content-Length: %d, want 413", got)
	}
	if got := bodyStatus(http.MethodPost, "/auth/magic/request", over, nil, true); got != http.StatusRequestEntityTooLarge {
		t.Errorf("over default, chunked: %d, want 413", got)
	}
	if got := bodyStatus(http.MethodPost, "/auth/magic/request", defaultMaxBodyBytes, nil, false); got != http.StatusOK {
		t.Errorf("exactly the default: %d, want 200", got)
	}
	if got := bodyStatus(http.MethodPost, "/api/v1/agent/tasks", 8<<20, nil, true); got != http.StatusOK {
		t.Errorf("8 MiB agent task: %d, want 200", got)
	}
	if got := bodyStatus(http.MethodPost, "/api/v1/agent/tasks", agentMaxBodyBytes+1, nil, true); got != http.StatusRequestEntityTooLarge {
		t.Errorf("agent task over its cap: %d, want 413", got)
	}
	if got := bodyStatus(http.MethodPost, "/api/v1/billing/webhook/lemonsqueezy", over, nil, false); got != http.StatusOK {
		t.Errorf("webhook keeps its own cap: %d, want 200 from this layer", got)
	}
	ws := map[string]string{"Upgrade": "websocket", "Connection": "Upgrade"}
	if got := bodyStatus(http.MethodPost, "/v1/relay/rpc", 5<<20, ws, true); got != http.StatusOK {
		t.Errorf("websocket upgrade must not be capped: %d", got)
	}
}

func TestGraphQLDevMode(t *testing.T) {
	for env, want := range map[string]bool{"dev": true, "LOCAL": true, "development": true, "": false, "production": false, "staging": false} {
		if got := graphqlDevMode(env); got != want {
			t.Errorf("graphqlDevMode(%q) = %v, want %v", env, got, want)
		}
	}
}

func graphqlPost(t *testing.T, dev bool, query string) map[string]any {
	t.Helper()
	srv := newGraphQLServer(graph.NewExecutableSchema(graph.Config{Resolvers: &graph.Resolver{}}), dev)
	payload, _ := json.Marshal(map[string]string{"query": query})
	r := httptest.NewRequest(http.MethodPost, "/query", strings.NewReader(string(payload)))
	r.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	srv.ServeHTTP(w, r)
	var out map[string]any
	if err := json.Unmarshal(w.Body.Bytes(), &out); err != nil {
		t.Fatalf("decode %q: %v", w.Body.String(), err)
	}
	return out
}

func TestGraphQLIntrospectionOnlyInDev(t *testing.T) {
	const q = `{ __schema { queryType { name } } }`
	if out := graphqlPost(t, false, q); out["errors"] == nil {
		t.Fatalf("production introspection answered: %v", out)
	}
	out := graphqlPost(t, true, q)
	if out["errors"] != nil {
		t.Fatalf("dev introspection failed: %v", out["errors"])
	}
}

func TestGraphQLComplexityLimit(t *testing.T) {
	var b strings.Builder
	b.WriteString("{")
	for i := 0; i < graphqlComplexityLimit; i++ {
		fmt.Fprintf(&b, " m%d: me { id }", i)
	}
	b.WriteString(" }")
	out := graphqlPost(t, false, b.String())
	errs, _ := json.Marshal(out["errors"])
	if !strings.Contains(string(errs), "complexity") {
		t.Fatalf("over-complex query not refused for complexity: %s", errs)
	}
}
