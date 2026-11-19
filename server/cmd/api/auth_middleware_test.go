package main

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/google/uuid"

	"server/internal/identity/model"
)

func testTokenService(t *testing.T) (*model.TokenService, string) {
	t.Helper()
	svc := model.NewTokenService("e10-test-secret")
	token, err := svc.GenerateAccessToken(uuid.New(), uuid.New())
	if err != nil {
		t.Fatalf("mint test token: %v", err)
	}
	return svc, token
}

// URL tokens are refused (ignored) on the relay, pairing and push prefixes
// (H2): a request carrying only ?access_token there must stay anonymous.
func TestAuthMiddlewareIgnoresQueryTokenOnE2EEPrefixes(t *testing.T) {
	svc, token := testTokenService(t)
	next := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		uid, _ := r.Context().Value("user_id").(string)
		if uid == "" {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		w.WriteHeader(http.StatusOK)
	})
	h := AuthMiddleware(svc)(next)

	for _, target := range []string{
		"/v1/relay/socket?access_token=" + token,
		"/v1/relay/host?access_token=" + token,
		"/v1/relay/catalog?access_token=" + token,
		"/v1/relay/rpc?access_token=" + token,
		"/v1/relay/host/state?access_token=" + token,
		"/v1/companion/pairings?access_token=" + token,
		"/v1/companion/pairings/redeem?access_token=" + token,
		"/v1/companion/devices/token?access_token=" + token,
		"/v1/companion/devices?access_token=" + token,
		"/v1/push/register?access_token=" + token,
		"/v1/push/rotate?access_token=" + token,
		"/v1/push/unregister?access_token=" + token,
		"/v1/push/registrations?access_token=" + token,
	} {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, target, nil))
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("GET %s with only a query token: got %d, want 401 (token must be ignored)", target, rec.Code)
		}
	}
}

// The Authorization header still authenticates on those prefixes, and
// /realtime (deferred, §14) plus every other route keep today's behaviour.
func TestAuthMiddlewareQueryTokenElsewhereUnchanged(t *testing.T) {
	svc, token := testTokenService(t)
	next := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		uid, _ := r.Context().Value("user_id").(string)
		if uid == "" {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		w.WriteHeader(http.StatusOK)
	})
	h := AuthMiddleware(svc)(next)

	// Header bearer works on the E2EE prefixes.
	for _, target := range []string{
		"/v1/relay/catalog",
		"/v1/companion/devices",
		"/v1/push/registrations",
	} {
		req := httptest.NewRequest(http.MethodGet, target, nil)
		req.Header.Set("Authorization", "Bearer "+token)
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		if rec.Code != http.StatusOK {
			t.Errorf("GET %s with header token: got %d, want 200", target, rec.Code)
		}
	}

	// Query tokens still authenticate outside the E2EE prefixes.
	for _, target := range []string{
		"/realtime?access_token=" + token,
		"/realtime/socket?access_token=" + token,
		"/query?access_token=" + token,
	} {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, target, nil))
		if rec.Code != http.StatusOK {
			t.Errorf("GET %s with query token: got %d, want 200 (behaviour unchanged)", target, rec.Code)
		}
	}
}
