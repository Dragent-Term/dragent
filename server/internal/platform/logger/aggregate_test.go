package logger

import (
	"bytes"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
)

func testRouter(agg *Aggregator) *chi.Mux {
	r := chi.NewRouter()
	r.Use(agg.Middleware)
	r.Get("/v1/relay/catalog", func(w http.ResponseWriter, req *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	})
	r.Get("/v1/companion/devices", func(w http.ResponseWriter, req *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write(make([]byte, 2048))
	})
	r.Get("/boom", func(w http.ResponseWriter, req *http.Request) {
		panic("secret-content-should-never-be-logged")
	})
	return r
}

func findBucket(buckets []Bucket, route, status, size, errClass string) *Bucket {
	for i := range buckets {
		b := &buckets[i]
		if b.Route == route && b.StatusClass == status && b.SizeBucket == size && b.ErrorClass == errClass {
			return b
		}
	}
	return nil
}

// The aggregate key carries the route template, never the request URI: a
// request with a query string (tokens, ids) must land in the same bucket as
// one without, and the bucket key must contain no trace of the query.
func TestAggregateCountsByRouteTemplate(t *testing.T) {
	agg := NewAggregator(time.Minute)
	r := testRouter(agg)

	for _, target := range []string{
		"/v1/relay/catalog?access_token=secret-token&device=some-id",
		"/v1/relay/catalog",
	} {
		rec := httptest.NewRecorder()
		r.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, target, nil))
		if rec.Code != http.StatusOK {
			t.Fatalf("GET %s: got %d, want 200", target, rec.Code)
		}
	}

	snap := agg.Snapshot()
	if len(snap) != 1 {
		t.Fatalf("want exactly 1 bucket, got %d: %+v", len(snap), snap)
	}
	b := snap[0]
	if b.Route != "/v1/relay/catalog" || b.StatusClass != "2xx" || b.SizeBucket != "<1KB" || b.ErrorClass != "none" {
		t.Fatalf("unexpected bucket: %+v", b)
	}
	if b.Count != 2 {
		t.Fatalf("want count 2, got %d", b.Count)
	}
}

// Flush output must match the §2.3 allowlist: templates, classes and counts
// only — no query strings, tokens or ids.
func TestAggregateFlushEmitsNoSensitiveData(t *testing.T) {
	var buf bytes.Buffer
	h := slog.NewJSONHandler(&buf, nil)
	prev := slog.Default()
	slog.SetDefault(slog.New(h))
	defer slog.SetDefault(prev)

	agg := NewAggregator(time.Minute)
	r := testRouter(agg)

	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/v1/companion/devices?access_token=secret-token-value&user=u-123", nil))

	agg.Flush()
	out := buf.String()
	if !strings.Contains(out, "/v1/companion/devices") {
		t.Fatalf("flush should name the route template, got: %s", out)
	}
	for _, forbidden := range []string{"secret-token-value", "u-123", "access_token"} {
		if strings.Contains(out, forbidden) {
			t.Fatalf("flush leaked %q: %s", forbidden, out)
		}
	}
	// The window is consumed by the flush.
	if got := agg.Snapshot(); len(got) != 0 {
		t.Fatalf("flush must reset counters, still have %+v", got)
	}
}

// Panics are reduced to the route template and the panic type: the panic
// value (which could carry content) must never reach the log.
func TestAggregatePanicReduced(t *testing.T) {
	var buf bytes.Buffer
	prev := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&buf, nil)))
	defer slog.SetDefault(prev)

	agg := NewAggregator(time.Hour)
	r := testRouter(agg)

	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/boom", nil))
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("panicking handler: got %d, want 500", rec.Code)
	}

	out := buf.String()
	if strings.Contains(out, "secret-content-should-never-be-logged") {
		t.Fatalf("panic log leaked the panic value: %s", out)
	}
	if !strings.Contains(out, "/boom") {
		t.Fatalf("panic log should name the route template, got: %s", out)
	}
	if !strings.Contains(out, `"panic_type":"string"`) {
		t.Fatalf("panic log should name the panic type, got: %s", out)
	}

	b := findBucket(agg.Snapshot(), "/boom", "5xx", "<1KB", "string")
	if b == nil || b.Count != 1 {
		t.Fatalf("want one 5xx/string bucket, got %+v", agg.Snapshot())
	}
}

// Health probes are excluded so platform polling does not drown the
// aggregates.
func TestAggregateSkipsProbes(t *testing.T) {
	agg := NewAggregator(time.Minute)
	r := chi.NewRouter()
	r.Use(agg.Middleware)
	r.Get("/health", func(w http.ResponseWriter, req *http.Request) { _, _ = w.Write([]byte("OK")) })
	r.Get("/ready", func(w http.ResponseWriter, req *http.Request) { _, _ = w.Write([]byte("OK")) })

	for _, target := range []string{"/health", "/ready"} {
		rec := httptest.NewRecorder()
		r.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, target, nil))
		if rec.Code != http.StatusOK {
			t.Fatalf("GET %s: got %d, want 200", target, rec.Code)
		}
	}
	if got := agg.Snapshot(); len(got) != 0 {
		t.Fatalf("probes must not be counted, got %+v", got)
	}
}

func TestAggregateStatusAndSizeBuckets(t *testing.T) {
	if got := statusClass(200); got != "2xx" {
		t.Fatalf("statusClass(200) = %q", got)
	}
	if got := statusClass(404); got != "4xx" {
		t.Fatalf("statusClass(404) = %q", got)
	}
	if got := statusClass(500); got != "5xx" {
		t.Fatalf("statusClass(500) = %q", got)
	}
	cases := map[int]string{0: "<1KB", 1023: "<1KB", 1024: "<10KB", 2048: "<10KB", 100 * 1024: "<1MB", 2 * 1024 * 1024: ">=1MB"}
	for n, want := range cases {
		if got := sizeBucket(n); got != want {
			t.Fatalf("sizeBucket(%d) = %q, want %q", n, got, want)
		}
	}
}

// Unmatched requests count under a fixed label, never under the raw path.
func TestAggregateUnmatchedHasNoURI(t *testing.T) {
	agg := NewAggregator(time.Minute)
	r := chi.NewRouter()
	r.Use(agg.Middleware)
	r.Get("/x", func(w http.ResponseWriter, req *http.Request) {})

	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/no/such/thing?access_token=x&user=y", nil))
	if rec.Code != http.StatusNotFound {
		t.Fatalf("got %d, want 404", rec.Code)
	}
	snap := agg.Snapshot()
	if len(snap) != 1 || snap[0].Route != "unmatched" {
		t.Fatalf("want single unmatched bucket, got %+v", snap)
	}
}

// The wrapper must stay transparent to websocket upgrades (hijack) and SSE
// streams (flush) mounted below it.
func TestStatusRecorderPassthrough(t *testing.T) {
	agg := NewAggregator(time.Minute)
	r := chi.NewRouter()
	r.Use(agg.Middleware)
	r.Get("/x", func(w http.ResponseWriter, req *http.Request) {
		if _, ok := w.(http.Hijacker); !ok {
			t.Error("wrapped writer must implement http.Hijacker")
		}
		if _, ok := w.(http.Flusher); !ok {
			t.Error("wrapped writer must implement http.Flusher")
		}
		w.WriteHeader(http.StatusTeapot)
		_, _ = w.Write([]byte("hi"))
	})
	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/x", nil))
	if rec.Code != http.StatusTeapot {
		t.Fatalf("got %d, want 418", rec.Code)
	}
	if body := rec.Body.String(); body != "hi" {
		t.Fatalf("body = %q, want %q", body, "hi")
	}
	b := findBucket(agg.Snapshot(), "/x", "4xx", "<1KB", "none")
	if b == nil || b.Count != 1 {
		t.Fatalf("want one 4xx bucket, got %+v", agg.Snapshot())
	}
}
