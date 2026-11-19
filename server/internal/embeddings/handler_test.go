package embeddings

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"server/internal/platform/ratelimit"
)

func newTestHandler(t *testing.T, limits Limits) (*Handler, *atomic.Int32) {
	t.Helper()
	var calls atomic.Int32
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		_, _ = w.Write([]byte(`{"data":[]}`))
	}))
	t.Cleanup(upstream.Close)
	h := NewHandler("test-key")
	h.upstreamURL = upstream.URL
	h.client = upstream.Client()
	h.SetLimits(limits)
	return h, &calls
}

func embedRequest(user, body string) *http.Request {
	r := httptest.NewRequest(http.MethodPost, "/v1/embeddings", strings.NewReader(body))
	return r.WithContext(context.WithValue(r.Context(), "user_id", user)) //nolint:staticcheck // matches AuthMiddleware's key
}

func TestEmbedRejectsOversizedBody(t *testing.T) {
	h, calls := newTestHandler(t, Limits{})
	w := httptest.NewRecorder()
	h.Embed(w, embedRequest("user-1", strings.Repeat("x", MaxBodyBytes+1)))
	if w.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("status = %d, want 413", w.Code)
	}
	if calls.Load() != 0 {
		t.Fatal("oversized body must not reach Voyage")
	}
}

func TestEmbedPerUserMinuteLimit(t *testing.T) {
	h, calls := newTestHandler(t, Limits{PerMinute: ratelimit.NewMemoryRateLimiter(2, time.Minute, nil)})
	for i := 0; i < 2; i++ {
		w := httptest.NewRecorder()
		h.Embed(w, embedRequest("user-1", `{"input":["a"]}`))
		if w.Code != http.StatusOK {
			t.Fatalf("request %d: status %d, want 200", i+1, w.Code)
		}
	}
	w := httptest.NewRecorder()
	h.Embed(w, embedRequest("user-1", `{"input":["a"]}`))
	if w.Code != http.StatusTooManyRequests || w.Header().Get("Retry-After") == "" {
		t.Fatalf("over limit: status %d Retry-After %q, want 429 with Retry-After", w.Code, w.Header().Get("Retry-After"))
	}
	w = httptest.NewRecorder()
	h.Embed(w, embedRequest("user-2", `{"input":["a"]}`))
	if w.Code != http.StatusOK {
		t.Fatalf("other user: status %d, want 200", w.Code)
	}
	if calls.Load() != 3 {
		t.Fatalf("upstream calls = %d, want 3", calls.Load())
	}
}

func TestEmbedDailyCap(t *testing.T) {
	h, calls := newTestHandler(t, Limits{
		PerMinute: ratelimit.NewMemoryRateLimiter(100, time.Minute, nil),
		PerDay:    ratelimit.NewMemoryRateLimiter(1, 24*time.Hour, nil),
	})
	h.Embed(httptest.NewRecorder(), embedRequest("user-1", `{}`))
	w := httptest.NewRecorder()
	h.Embed(w, embedRequest("user-1", `{}`))
	if w.Code != http.StatusTooManyRequests {
		t.Fatalf("status = %d, want 429", w.Code)
	}
	if calls.Load() != 1 {
		t.Fatalf("upstream calls = %d, want 1", calls.Load())
	}
}
