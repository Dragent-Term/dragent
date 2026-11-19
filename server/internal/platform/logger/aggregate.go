// Aggregate-only HTTP telemetry for the E2EE phone relay (§2.3).
//
// There is deliberately no per-request access log anywhere on this server:
// request or response bodies, query strings, tokens, titles or names, full
// key fingerprints, push tokens, and user or device ids must never appear in
// per-request lines. What is logged is, once per minute per process:
//
//   - per-minute aggregate counters keyed by route template, status class,
//     response size bucket and error class;
//   - startup and configuration lines (elsewhere, via slog at boot);
//   - panics, reduced to the route template and the panic type.
//
// Health probes (/health, /ready) are excluded from the counters so the
// platform's polling does not drown real traffic in the aggregates.
package logger

import (
	"bufio"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"sort"
	"sync"
	"time"

	"github.com/go-chi/chi/v5"
)

// aggKey is one counter bucket. Every field is a fixed template or class —
// never a URI, query string, token or id.
type aggKey struct {
	route       string
	statusClass string
	sizeBucket  string
	errorClass  string
}

// Bucket is the wire shape of one emitted counter.
type Bucket struct {
	Route       string `json:"route"`
	StatusClass string `json:"status_class"`
	SizeBucket  string `json:"size_bucket"`
	ErrorClass  string `json:"error_class"`
	Count       uint64 `json:"count"`
}

// Aggregator counts requests in memory and emits the counters once per
// interval. The zero value is not usable; construct with NewAggregator.
type Aggregator struct {
	interval time.Duration

	mu     sync.Mutex
	counts map[aggKey]uint64

	stopOnce sync.Once
	done     chan struct{}
}

// NewAggregator returns an Aggregator that flushes every interval. A
// non-positive interval falls back to one minute.
func NewAggregator(interval time.Duration) *Aggregator {
	if interval <= 0 {
		interval = time.Minute
	}
	return &Aggregator{interval: interval, counts: make(map[aggKey]uint64), done: make(chan struct{})}
}

// Start begins the background flush loop. The returned function stops the
// loop and emits a final flush. Callers that never Start can still Flush
// manually (tests do this).
func (a *Aggregator) Start() (stop func()) {
	ticker := time.NewTicker(a.interval)
	go func() {
		for {
			select {
			case <-ticker.C:
				a.Flush()
			case <-a.done:
				ticker.Stop()
				return
			}
		}
	}()
	return func() {
		a.stopOnce.Do(func() { close(a.done) })
		a.Flush()
	}
}

// Snapshot returns a copy of the current counters, oldest-first by nothing
// in particular — tests use it to assert bucketing without flushing.
func (a *Aggregator) Snapshot() []Bucket {
	a.mu.Lock()
	defer a.mu.Unlock()
	return flatten(a.counts)
}

// Flush emits the current counters as a single slog line and resets them.
// An empty window emits nothing.
func (a *Aggregator) Flush() {
	a.mu.Lock()
	counts := a.counts
	a.counts = make(map[aggKey]uint64, len(counts))
	a.mu.Unlock()
	if len(counts) == 0 {
		return
	}
	slog.Info("http request aggregates",
		"window_seconds", int64(a.interval/time.Second),
		"buckets", flatten(counts),
	)
}

func flatten(counts map[aggKey]uint64) []Bucket {
	out := make([]Bucket, 0, len(counts))
	for k, n := range counts {
		out = append(out, Bucket{
			Route:       k.route,
			StatusClass: k.statusClass,
			SizeBucket:  k.sizeBucket,
			ErrorClass:  k.errorClass,
			Count:       n,
		})
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Route != out[j].Route {
			return out[i].Route < out[j].Route
		}
		if out[i].StatusClass != out[j].StatusClass {
			return out[i].StatusClass < out[j].StatusClass
		}
		if out[i].SizeBucket != out[j].SizeBucket {
			return out[i].SizeBucket < out[j].SizeBucket
		}
		return out[i].ErrorClass < out[j].ErrorClass
	})
	return out
}

func (a *Aggregator) record(route string, status, bytes int, errorClass string) {
	k := aggKey{
		route:       route,
		statusClass: statusClass(status),
		sizeBucket:  sizeBucket(bytes),
		errorClass:  errorClass,
	}
	a.mu.Lock()
	a.counts[k]++
	a.mu.Unlock()
}

func statusClass(status int) string {
	switch {
	case status >= 200 && status < 300:
		return "2xx"
	case status >= 300 && status < 400:
		return "3xx"
	case status >= 400 && status < 500:
		return "4xx"
	default:
		return "5xx"
	}
}

func sizeBucket(n int) string {
	switch {
	case n < 1024:
		return "<1KB"
	case n < 10*1024:
		return "<10KB"
	case n < 100*1024:
		return "<100KB"
	case n < 1024*1024:
		return "<1MB"
	default:
		return ">=1MB"
	}
}

// routeTemplate returns the matched chi route pattern (e.g.
// "/v1/relay/rpc"), never the request URI. Unmatched requests — 404s and
// anything answered before routing — count under "unmatched".
func routeTemplate(r *http.Request) string {
	if rctx := chi.RouteContext(r.Context()); rctx != nil {
		if p := rctx.RoutePattern(); p != "" {
			return p
		}
	}
	return "unmatched"
}

// Middleware counts one request per bucket and recovers panics with the
// §2.3-reduced log line (route template and panic type only — no stack, no
// values that could carry content). It replaces chi's per-request Logger and
// its full-stack Recoverer on the routes it wraps.
func (a *Aggregator) Middleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Health probes are noise, not traffic.
		if r.URL.Path == "/health" || r.URL.Path == "/ready" {
			next.ServeHTTP(w, r)
			return
		}
		rec := &statusRecorder{ResponseWriter: w}
		defer func() {
			if v := recover(); v != nil {
				if v == http.ErrAbortHandler {
					panic(v)
				}
				tmpl := routeTemplate(r)
				ptype := fmt.Sprintf("%T", v)
				a.record(tmpl, http.StatusInternalServerError, rec.bytes, ptype)
				slog.Error("http handler panic", "route", tmpl, "panic_type", ptype)
				if r.Header.Get("Connection") != "Upgrade" && rec.status == 0 {
					rec.ResponseWriter.WriteHeader(http.StatusInternalServerError)
				}
				return
			}
			status := rec.status
			if status == 0 {
				status = http.StatusOK
			}
			a.record(routeTemplate(r), status, rec.bytes, "none")
		}()
		next.ServeHTTP(rec, r)
	})
}

// statusRecorder captures the status code and body size without touching
// the URI, headers or body contents. It passes through Hijacker (websocket
// upgrades), Flusher (SSE streams) and Unwrap (ResponseController) so
// wrapping is transparent to every handler below it.
type statusRecorder struct {
	http.ResponseWriter
	status int
	bytes  int
}

func (w *statusRecorder) WriteHeader(status int) {
	if w.status == 0 {
		w.status = status
	}
	w.ResponseWriter.WriteHeader(status)
}

func (w *statusRecorder) Write(b []byte) (int, error) {
	if w.status == 0 {
		w.status = http.StatusOK
	}
	n, err := w.ResponseWriter.Write(b)
	w.bytes += n
	return n, err
}

func (w *statusRecorder) Flush() {
	if w.status == 0 {
		w.status = http.StatusOK
	}
	if f, ok := w.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

func (w *statusRecorder) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	if h, ok := w.ResponseWriter.(http.Hijacker); ok {
		return h.Hijack()
	}
	return nil, nil, errors.New("logger: underlying writer does not support hijacking")
}

func (w *statusRecorder) Unwrap() http.ResponseWriter {
	return w.ResponseWriter
}
