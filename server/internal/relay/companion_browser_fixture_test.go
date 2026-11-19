package relay

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
)

// Explicit loopback-only browser fixture; never mounted in the API binary.
func TestCompanionBrowserFixture(t *testing.T) {
	if os.Getenv("DRAGENT_TEST_BROWSER_FIXTURE") != "1" {
		t.Skip("opt-in local browser fixture")
	}
	h := NewRPCHandler(nil, nil)
	catalogs := NewMemoryStore()
	now := time.Now()
	catalogs.SetClock(func() time.Time { return now })
	h.ConfigureSocket(func(_ context.Context, token string) (SocketIdentity, error) {
		if token != "test-token" {
			return SocketIdentity{}, errors.New("fixture token rejected")
		}
		return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(time.Hour)}, nil
	}, catalogs, nil)
	owner, _ := ownerKey("account-a", "desktop")
	projectID := "/repo/100%2F literal"
	workspaceID := "/workspace/feature branch"
	// E2EE cut-over: the relay stores no catalog content. The fixture
	// records a content-free routing stub so the desktop host socket passes
	// admission; ids below are local labels only, never published.
	err := catalogs.Put(context.Background(), owner, Catalog{}.Normalized())
	if err != nil {
		t.Fatal(err)
	}
	stopped := make(chan struct{})
	var once sync.Once
	router := chi.NewRouter()
	router.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			origin := r.Header.Get("Origin")
			if origin == "http://localhost:8083" || origin == "http://127.0.0.1:8083" {
				w.Header().Set("Access-Control-Allow-Origin", origin)
				w.Header().Set("Access-Control-Allow-Credentials", "true")
			}
			w.Header().Set("Access-Control-Allow-Headers", "Authorization, Content-Type")
			w.Header().Set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
			if r.Method == http.MethodOptions {
				w.WriteHeader(204)
				return
			}
			next.ServeHTTP(w, r)
		})
	})
	h.RegisterSocketRoutes(router)
	h.RegisterHostRoutes(router)
	var server *httptest.Server
	respond := func(w http.ResponseWriter, value any) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(value)
	}
	router.Get("/api/session/config", func(w http.ResponseWriter, _ *http.Request) {
		respond(w, map[string]any{"gateway": "ws" + strings.TrimPrefix(server.URL, "http") + "/realtime"})
	})
	router.Get("/api/auth/session", func(w http.ResponseWriter, _ *http.Request) {
		respond(w, map[string]any{"refresh_token": "local-fixture-refresh"})
	})
	router.Post("/api/auth/refresh", func(w http.ResponseWriter, _ *http.Request) {
		respond(w, map[string]any{"accessToken": "test-token", "refreshToken": "local-fixture-refresh", "expiresIn": 3600, "user": map[string]any{"id": "account-a", "email": "fixture@example.invalid", "displayName": "Local QA"}})
	})
	router.Get("/login", func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("Local fixture sign-in is ready. Return to the test app."))
	})
	router.Post("/_test/stop", func(w http.ResponseWriter, _ *http.Request) { once.Do(func() { close(stopped) }); w.WriteHeader(204) })
	server = httptest.NewServer(router)
	defer server.Close()
	startMobileInteropHost(t, server)
	path := os.Getenv("DRAGENT_TEST_BROWSER_INFO")
	if path == "" {
		t.Fatal("private fixture info path required")
	}
	data, _ := json.Marshal(map[string]any{"portal": server.URL, "projectId": projectID, "workspaceId": workspaceID})
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
	defer os.Remove(path)
	select {
	case <-stopped:
	case <-time.After(10 * time.Minute):
		t.Fatal("browser fixture timed out")
	}
}
