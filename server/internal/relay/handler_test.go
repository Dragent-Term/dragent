// Sealed catalog endpoint tests (§6.5, §10): PUT stores the desktop's
// sealed blob opaquely, GET returns it verbatim. Plaintext shapes are gone.
package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
)

// testRouter mounts the relay routes the way the gateway will: behind an
// auth layer that injects "user_id" only when the caller presents one
// (X-Test-User), so missing credentials stay missing.
func testRouter(h *Handler) http.Handler {
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterRoutes(r)
	return r
}

func publish(t *testing.T, srv *httptest.Server, user string, body any) *http.Response {
	t.Helper()
	raw, _ := json.Marshal(body)
	req, _ := http.NewRequest(http.MethodPut, srv.URL+"/v1/relay/catalog", bytes.NewReader(raw))
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func read(t *testing.T, srv *httptest.Server, user, device string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/v1/relay/catalog?device_id="+device, nil)
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func TestPublishAndReadSealedRoundTrip(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	blob := makeSealedBlob(t, sealedTestDesktop, 7, sealedTestPhone)
	published := publish(t, srv, "user-a", sealedPutBody(sealedTestDesktop, blob))
	defer published.Body.Close()
	if published.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(published.Body)
		t.Fatalf("publish status = %d: %s", published.StatusCode, body)
	}

	status, raw := readSealed(t, srv, "user-a", sealedTestDesktop)
	if status != http.StatusOK {
		t.Fatalf("read status = %d: %s", status, raw)
	}
	got := decodeSealed(t, raw)
	if got.Desktop != sealedTestDesktop || got.Version != 7 || len(got.Recipients) != 1 ||
		got.Recipients[0].Phone != sealedTestPhone || got.Body != blob.Body || got.Nonce != blob.Nonce {
		t.Fatalf("blob mismatch: %+v", got)
	}
	// The wire carries no catalog content: no titles, paths, or names.
	for _, needle := range []string{"dragent", "title", "projects", "workspaces", "tabs", "machine"} {
		if strings.Contains(strings.ToLower(string(raw)), needle) {
			t.Fatalf("sealed read leaked %q: %s", needle, raw)
		}
	}
}

// A new desktop with no paired phones seals for nobody: 0 recipients must
// store and return verbatim (first-pairing deadlock fix, §6.5).
func TestPublishAndReadSealedEmptyRecipients(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	blob := makeSealedBlob(t, sealedTestDesktop, 1, sealedTestPhone)
	blob.Recipients = []SealedRecipient{}
	published := publish(t, srv, "user-a", sealedPutBody(sealedTestDesktop, blob))
	defer published.Body.Close()
	if published.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(published.Body)
		t.Fatalf("empty-recipient publish status = %d: %s", published.StatusCode, body)
	}

	status, raw := readSealed(t, srv, "user-a", sealedTestDesktop)
	if status != http.StatusOK {
		t.Fatalf("read status = %d: %s", status, raw)
	}
	got := decodeSealed(t, raw)
	if got.Desktop != sealedTestDesktop || got.Version != 1 || len(got.Recipients) != 0 ||
		got.Body != blob.Body || got.Nonce != blob.Nonce {
		t.Fatalf("empty-recipient blob mismatch: %+v", got)
	}
	// The wire must carry an explicit empty array, not null, so phone
	// decoders see an array.
	var wire map[string]any
	if err := json.Unmarshal(raw, &wire); err != nil {
		t.Fatalf("sealed body is not JSON: %v", err)
	}
	recipients, present := wire["recipients"]
	if !present {
		t.Fatal("sealed read omits recipients")
	}
	list, ok := recipients.([]any)
	if !ok || len(list) != 0 {
		t.Fatalf("recipients = %#v, want empty array", recipients)
	}
}

func TestUnauthenticatedCallsRejected(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	published := publish(t, srv, "", sealedPutBody(sealedTestDesktop, makeSealedBlob(t, sealedTestDesktop, 1)))
	published.Body.Close()
	if published.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous publish status = %d, want 401", published.StatusCode)
	}
	resp := read(t, srv, "", sealedTestDesktop)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous read status = %d, want 401", resp.StatusCode)
	}
}

func TestCrossAccountSealedIsUnaddressable(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	if status := publishSealed(t, srv, "user-a", sealedTestDesktop, makeSealedBlob(t, sealedTestDesktop, 3)); status != http.StatusOK {
		t.Fatalf("publish status = %d", status)
	}

	// Another account's same device id is a different owner: 404, never the
	// first account's blob and never a distinguishable forbidden.
	status, body := readSealed(t, srv, "user-b", sealedTestDesktop)
	if status != http.StatusNotFound {
		t.Fatalf("cross-account read status = %d (%s), want 404", status, body)
	}

	// Writing under the same device id from the other account stores into
	// that account's own owner — the first account's blob is untouched.
	other := makeSealedBlob(t, sealedTestDesktop, 9, sealedTestPhone2)
	if status := publishSealed(t, srv, "user-b", sealedTestDesktop, other); status != http.StatusOK {
		t.Fatalf("second account publish status = %d", status)
	}
	status, raw := readSealed(t, srv, "user-a", sealedTestDesktop)
	if status != http.StatusOK {
		t.Fatalf("first account read status = %d", status)
	}
	if got := decodeSealed(t, raw); got.Version != 3 || got.Recipients[0].Phone != sealedTestPhone {
		t.Fatalf("first account blob clobbered: %+v", got)
	}
}

// Plaintext catalog shapes are gone (§10.5): bodies carrying them answer 410
// and store nothing.
func TestPlaintextCatalogShapesAreGone(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	for name, body := range map[string]any{
		"projects":   map[string]any{"device_id": sealedTestDesktop, "projects": []any{map[string]any{"name": "dragent", "path": "/Users/me/dragent"}}},
		"workspaces": map[string]any{"device_id": sealedTestDesktop, "workspaces": []any{map[string]any{"name": "w", "path": "/x"}}},
		"tabs":       map[string]any{"device_id": sealedTestDesktop, "tabs": []any{map[string]any{"id": 1, "kind": "terminal"}}},
		"machine":    map[string]any{"device_id": sealedTestDesktop, "machine": map[string]any{"name": "mbp", "platform": "darwin"}},
	} {
		resp := publish(t, srv, "user-a", body)
		resp.Body.Close()
		if resp.StatusCode != http.StatusGone {
			t.Fatalf("%s: publish status = %d, want 410", name, resp.StatusCode)
		}
	}
	status, _ := readSealed(t, srv, "user-a", sealedTestDesktop)
	if status != http.StatusNotFound {
		t.Fatalf("refused plaintext stored a blob: status = %d", status)
	}
}

func TestInvalidSealedRejected(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	good := makeSealedBlob(t, sealedTestDesktop, 1)
	mutate := func(f func(map[string]any)) map[string]any {
		body := sealedPutBody(sealedTestDesktop, good)
		f(body)
		return body
	}
	for name, body := range map[string]any{
		"missing device":   map[string]any{},
		"blank device":     map[string]any{"device_id": "  "},
		"oversized device": map[string]any{"device_id": strings.Repeat("d", 129)},
		"bad version":      mutate(func(b map[string]any) { b["v"] = 2 }),
		"zero version":     mutate(func(b map[string]any) { b["version"] = 0 }),
		"bad desktop":      mutate(func(b map[string]any) { b["desktop"] = "not-a-uuid" }),
		"desktop mismatch": mutate(func(b map[string]any) { b["desktop"] = sealedTestDesktop2 }),
		"bad recipient": mutate(func(b map[string]any) {
			b["recipients"] = []any{map[string]any{"phone": "x", "wrap": sealedB64(t, 136)}}
		}),
		"bad nonce":      mutate(func(b map[string]any) { b["nonce"] = sealedB64(t, 16) }),
		"thin body":      mutate(func(b map[string]any) { b["body"] = sealedB64(t, 100) }),
		"unknown field":  mutate(func(b map[string]any) { b["title"] = "hello" }),
		"malformed json": "not-an-object",
	} {
		resp := publish(t, srv, "user-a", body)
		resp.Body.Close()
		if resp.StatusCode != http.StatusBadRequest {
			t.Fatalf("%s: publish status = %d, want 400", name, resp.StatusCode)
		}
	}

	resp := read(t, srv, "user-a", "")
	resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("blank device read status = %d, want 400", resp.StatusCode)
	}

	status, _ := readSealed(t, srv, "user-a", "never-published")
	if status != http.StatusNotFound {
		t.Fatalf("unpublished device read status = %d, want 404", status)
	}
}

func TestPublishKeepsAuthAndSizeCap(t *testing.T) {
	srv := httptest.NewServer(testRouter(NewHandler(nil)))
	defer srv.Close()

	anon := publish(t, srv, "", sealedPutBody(sealedTestDesktop, makeSealedBlob(t, sealedTestDesktop, 1)))
	anon.Body.Close()
	if anon.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous publish status = %d, want 401", anon.StatusCode)
	}

	// Oversize bodies are rejected before parsing.
	big := sealedPutBody(sealedTestDesktop, makeSealedBlob(t, sealedTestDesktop, 1))
	big["body"] = strings.Repeat("A", 4*1024*1024)
	resp := publish(t, srv, "user-a", big)
	resp.Body.Close()
	if resp.StatusCode != http.StatusRequestEntityTooLarge {
		t.Fatalf("oversize publish status = %d, want 413", resp.StatusCode)
	}
}

// A refreshed credential must not relabel a previous account's snapshot.
func TestPublishRejectsChangedPublisherAccountBeforeDecode(t *testing.T) {
	sealed := NewMemorySealedStore()
	h := NewHandler(nil)
	h.SetSealedStore(sealed)
	handler := testRouter(h)
	blob := sealedPutBody(sealedTestDesktop, makeSealedBlob(t, sealedTestDesktop, 1))
	raw, _ := json.Marshal(blob)
	for _, body := range []string{string(raw), `invalid json`} {
		req := httptest.NewRequest(http.MethodPut, "/v1/relay/catalog", strings.NewReader(body))
		req.Header.Set("X-Test-User", "new-account")
		req.Header.Set("X-Dragent-Publisher-User", "old-account")
		response := httptest.NewRecorder()
		handler.ServeHTTP(response, req)
		if response.Code != http.StatusForbidden {
			t.Fatalf("status = %d, want 403", response.Code)
		}
	}
	if _, found, _ := sealed.GetSealed(context.Background(), "device\x00new-account\x00"+sealedTestDesktop); found {
		t.Fatal("rejected publish stored a blob")
	}
}
