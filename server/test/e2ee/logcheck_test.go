package e2ee

import (
	"bytes"
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"

	"server/internal/platform/logger"
	"server/internal/relay"
)

// captureSlog installs a text slog handler writing to buf as the process
// default for the duration of f. Tests in this package never run parallel,
// so swapping the default is safe.
func captureSlog(t *testing.T, buf *bytes.Buffer, f func()) {
	t.Helper()
	prev := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(buf, nil)))
	defer slog.SetDefault(prev)
	f()
}

// moduleRoot returns the server module directory (the parent of
// test/e2ee), so static source checks read the real product code.
func moduleRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	return filepath.Dir(filepath.Dir(filepath.Dir(file)))
}

func readSource(t *testing.T, rel string) string {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(moduleRoot(t), rel))
	if err != nil {
		t.Fatalf("read %s: %v", rel, err)
	}
	return string(raw)
}

// TestAggregateLogsCarryNoContent (§12.9) drives the real HTTP aggregate
// middleware with canaries in the query string (including access_token),
// the body and the response, plus a panicking handler. It asserts the only
// emitted log line is the aggregate bucket line — carrying route templates,
// status classes, size buckets and error classes, never URIs, queries,
// tokens, bodies or ids — and that no access_token appears in any log.
func TestAggregateLogsCarryNoContent(t *testing.T) {
	c := newCanaries(t)
	agg := logger.NewAggregator(0)
	r := chi.NewRouter()
	r.Use(agg.Middleware)
	r.Post("/v1/relay/catalog", func(w http.ResponseWriter, req *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("stored " + c.machineName))
	})
	r.Get("/v1/push/registrations", func(w http.ResponseWriter, req *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{"registrations":[]}`))
	})
	r.Get("/v1/relay/panic", func(w http.ResponseWriter, req *http.Request) {
		panic(&testPanicValue{msg: c.pushBody})
	})

	var logs bytes.Buffer
	captureSlog(t, &logs, func() {
		serve := func(method, target, body string) {
			var reader *bytes.Reader
			if body == "" {
				reader = bytes.NewReader(nil)
			} else {
				reader = bytes.NewReader([]byte(body))
			}
			req := httptest.NewRequest(method, target, reader)
			w := httptest.NewRecorder()
			r.ServeHTTP(w, req)
		}
		serve(http.MethodPost, "/v1/relay/catalog?access_token="+c.pushTitle+"&name="+c.deviceName, c.fileContent)
		serve(http.MethodGet, "/v1/push/registrations?access_token="+c.pushBody, "")
		serve(http.MethodGet, "/v1/relay/panic?access_token="+c.input, "")
		serve(http.MethodGet, "/ready?access_token="+c.output, "")
		agg.Flush()
	})

	out := logs.Bytes()
	assertNoCanary(t, "http aggregate log", out, c)
	if bytes.Contains(out, []byte("access_token")) {
		t.Fatalf("aggregate log mentions access_token:\n%s", out)
	}
	if !bytes.Contains(out, []byte("http request aggregates")) {
		t.Fatalf("expected one aggregate line, got:\n%s", out)
	}
	for _, b := range agg.Snapshot() {
		joined := b.Route + b.StatusClass + b.SizeBucket + b.ErrorClass
		assertNoCanary(t, "aggregate bucket", []byte(joined), c)
		if strings.Contains(joined, "access_token") || strings.Contains(joined, "?") {
			t.Fatalf("aggregate bucket carries URI material: %q", joined)
		}
	}
	// The panic line is reduced to route template + panic type (§2.3): the
	// canary inside the panicking value must not appear.
	if bytes.Contains(out, []byte(c.pushBody)) {
		t.Fatal("panic log carried the panic value")
	}
}

type testPanicValue struct{ msg string }

// TestRelayAggregatesCarryNoContent (§12.9) runs canary ciphertext through
// the real router, then emits RelayStats aggregates into a captured log,
// asserting the lines carry counters only.
func TestRelayAggregatesCarryNoContent(t *testing.T) {
	c := newCanaries(t)
	s := newSealer(t)
	var logs bytes.Buffer
	stats := relay.NewRelayStats()
	router := relay.NewE2EERouter(nil, stats, nil)

	chanID := randChan(t)
	peer := mustRawID(t, recDesktop)
	hs1, err := relay.EncodeE2EEFrame(relay.E2EEKindHS, 1, relay.E2EEPurposeCompanion, chanID, &peer, 0, s.seal(c.input))
	if err != nil {
		t.Fatal(err)
	}
	frame, err := relay.DecodeE2EEFrame(hs1)
	if err != nil {
		t.Fatal(err)
	}
	if _, _, err := router.RegisterHS1(frame, hs1, recLease, recAccount, recDesktop, recPhone, relay.E2EERolePhone, nil); err != nil {
		t.Fatal(err)
	}
	bad, _ := relay.EncodeE2EEFrame(relay.E2EEKindData, 0, 0, randChan(t), nil, 0, s.seal(c.output))
	badFrame, _ := relay.DecodeE2EEFrame(bad)
	_, _ = router.Route(badFrame, recLease, recPhone, relay.E2EERolePhone)

	captureSlog(t, &logs, func() {
		stats.LogAggregates(slog.Default())
	})
	assertNoCanary(t, "relay aggregate log", logs.Bytes(), c)
	for _, id := range []string{recPhone, recDesktop, recAccount} {
		if bytes.Contains(logs.Bytes(), []byte(id)) {
			t.Fatalf("relay aggregate log carries device id %s", id)
		}
	}
}

// TestLoggingAllowlistStatic (§12.9, static half) greps the product code for
// the known §2.3 violations: chi's per-request access log, URL access
// tokens accepted on relay/pairing/push paths, plaintext in push payload
// builders, and per-request id/token logging.
func TestLoggingAllowlistStatic(t *testing.T) {
	main := readSource(t, "cmd/api/main.go")
	if strings.Contains(main, "middleware.Logger") {
		t.Fatal("main.go still mounts chi middleware.Logger (H1)")
	}
	for _, prefix := range []string{"/v1/relay/", "/v1/companion/", "/v1/push/"} {
		if !strings.Contains(main, `"`+prefix+`"`) {
			t.Fatalf("main.go queryToken does not refuse %s (H2)", prefix)
		}
	}

	// Push and pairing authenticate from the Authorization header only: no
	// access_token query fallback in non-test code.
	for _, f := range []string{"internal/push/lease.go", "internal/companionpairing/handler.go"} {
		src := readSource(t, f)
		if strings.Contains(src, `Query().Get("access_token")`) {
			t.Fatalf("%s accepts access_token in the query string (H2)", f)
		}
	}

	// Push payload builders never reference notification text, kind data or
	// sender fields: the constant placeholder and the opaque blob are all
	// that goes on the wire (§8).
	for _, f := range []string{"internal/push/apns_transport.go", "internal/push/fcm_transport.go"} {
		src := readSource(t, f)
		for _, ref := range []string{"msg.Title", "msg.Body", "msg.Data", "SenderID", "senderID", "DesktopID", "desktopID"} {
			if strings.Contains(src, ref) {
				t.Fatalf("%s references %s in a payload path", f, ref)
			}
		}
	}
	// The no-credential sender logs platform and outcome only.
	provider := readSource(t, "internal/push/provider.go")
	sendFn := funcBody(t, provider, "func (s *LogSender) Send")
	for _, ref := range []string{"Title", "Body", "Token", "Blob", "UserID", "DeviceID"} {
		if strings.Contains(sendFn, ref) {
			t.Fatalf("LogSender references %s", ref)
		}
	}

	// Plaintext relay routes are gone with 410 before the body is read
	// (§10.5): catalog, host-state mirror, RPC/SSE, providers.
	for _, f := range []string{
		"internal/relay/handler.go",
		"internal/relay/hoststate.go",
		"internal/relay/rpc.go",
		"internal/relay/events.go",
		"internal/relay/providers.go",
	} {
		src := readSource(t, f)
		// goneRPC in rpc.go is the shared 410 writer; callers reference
		// it instead of spelling StatusGone themselves.
		if !strings.Contains(src, "StatusGone") && !strings.Contains(src, "goneRPC") {
			t.Fatalf("%s has no 410 cut-over refusal", f)
		}
	}

	// No per-request slog line in relay/push/pairing carries ids, tokens,
	// titles or fingerprints (§2.3).
	for _, dir := range []string{"internal/relay", "internal/push", "internal/companionpairing"} {
		entries, err := os.ReadDir(filepath.Join(moduleRoot(t), dir))
		if err != nil {
			t.Fatal(err)
		}
		for _, e := range entries {
			if strings.HasSuffix(e.Name(), "_test.go") || !strings.HasSuffix(e.Name(), ".go") {
				continue
			}
			src := readSource(t, dir+"/"+e.Name())
			for _, line := range strings.Split(src, "\n") {
				trimmed := strings.TrimSpace(line)
				if strings.HasPrefix(trimmed, "//") || !strings.Contains(line, "slog.") {
					continue
				}
				for _, forbidden := range []string{"user_id", "device_id", "access_token", "push_token", "title", "Title", "fingerprint", "query"} {
					if strings.Contains(line, `"`+forbidden+`"`) {
						t.Fatalf("%s/%s logs %q in a per-request line: %s", dir, e.Name(), forbidden, trimmed)
					}
				}
			}
		}
	}

	// The access_token query parameter appears in non-test code only at the
	// refusal points (main middleware, pairing and push comments/handlers).
	var _ = context.Background
}

// funcBody extracts the source of the named top-level function (up to the
// next line starting at column 0 with "func " or the end of file).
func funcBody(t *testing.T, src, name string) string {
	t.Helper()
	idx := strings.Index(src, name)
	if idx < 0 {
		t.Fatalf("function %s not found", name)
	}
	rest := src[idx:]
	lines := strings.Split(rest, "\n")
	end := len(lines)
	for i := 1; i < len(lines); i++ {
		if strings.HasPrefix(lines[i], "func ") {
			end = i
			break
		}
	}
	return strings.Join(lines[:end], "\n")
}
