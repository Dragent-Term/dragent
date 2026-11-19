// Store copy-semantics tests after the cut-over: the sealed store deep
// copies on write and read, and the removed host-state mirror accepts no
// writes (PublishState is 410).
package relay

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
)

func TestSealedSnapshotsDoNotAliasPublisherOrReader(t *testing.T) {
	ctx := context.Background()
	store := NewMemorySealedStore()
	owner, _ := ownerKey("user-a", sealedTestDesktop)
	blob := makeSealedBlob(t, sealedTestDesktop, 1)
	if err := store.PutSealed(ctx, owner, blob); err != nil {
		t.Fatal(err)
	}
	blob.Recipients[0].Phone = sealedTestPhone2
	blob.Body = sealedB64(t, sealedBodyBucket)
	first, found, err := store.GetSealed(ctx, owner)
	if err != nil || !found {
		t.Fatalf("GetSealed = %+v, %v, %v", first, found, err)
	}
	if first.Recipients[0].Phone != sealedTestPhone {
		t.Fatal("publisher changed stored blob")
	}
	first.Recipients[0].Wrap = sealedB64(t, 136)
	second, _, err := store.GetSealed(ctx, owner)
	if err != nil {
		t.Fatal(err)
	}
	if second.Recipients[0].Wrap == first.Recipients[0].Wrap {
		t.Fatal("reader changed stored blob")
	}
}

func TestHostStateMirrorAcceptsNoWrites(t *testing.T) {
	h := NewRPCHandler(NewMemoryHostStore(), NewEventBus())
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterRPCRoutes(r)
	srv := httptest.NewServer(r)
	defer srv.Close()

	// The removed mirror route is 410 without reading the body — even for a
	// huge plaintext mirror that the old code would have stored.
	body := `{"device_id":"device-1","tabs":[],"worktrees":[],"listings":[],"contents":[]}`
	req, _ := http.NewRequest(http.MethodPut, srv.URL+"/v1/relay/host/state", strings.NewReader(body))
	req.Header.Set("X-Test-User", "user-a")
	req.Header.Set("X-Test-User", "user-a")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusGone {
		t.Fatalf("host state status = %d, want 410", resp.StatusCode)
	}
	// Anonymous callers get 401 first.
	req, _ = http.NewRequest(http.MethodPut, srv.URL+"/v1/relay/host/state", http.NoBody)
	resp, err = http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous host state status = %d, want 401", resp.StatusCode)
	}
}
