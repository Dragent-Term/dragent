package push

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

type testRig struct {
	store    *MemoryStore
	resolver *memoryDeviceResolver
	auth     *JWTAuthenticator
	router   chi.Router
	userID   uuid.UUID
	deviceID uuid.UUID
}

func newTestRig(t *testing.T) *testRig {
	t.Helper()
	store := NewMemoryStore()
	resolver := newMemoryDeviceResolver()
	auth := NewJWTAuthenticator(testJWTSecret)
	h := NewHandler(store, resolver, auth)
	r := chi.NewRouter()
	h.RegisterRoutes(r)
	rig := &testRig{store: store, resolver: resolver, auth: auth, router: r,
		userID: uuid.New(), deviceID: uuid.New()}
	resolver.add(rig.userID, rig.deviceID)
	return rig
}

func (rig *testRig) token(t *testing.T, userID, deviceID uuid.UUID) string {
	t.Helper()
	return mintToken(t, testJWTSecret, userID, deviceID, time.Hour)
}

func (rig *testRig) do(t *testing.T, method, path, token, body string) *httptest.ResponseRecorder {
	t.Helper()
	var reader *bytes.Reader
	if body == "" {
		reader = bytes.NewReader(nil)
	} else {
		reader = bytes.NewReader([]byte(body))
	}
	req := httptest.NewRequest(method, path, reader)
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	rec := httptest.NewRecorder()
	rig.router.ServeHTTP(rec, req)
	return rec
}

func TestRegisterHappyPath(t *testing.T) {
	rig := newTestRig(t)
	rec := rig.do(t, "POST", "/v1/push/register", rig.token(t, rig.userID, rig.deviceID),
		`{"device_id":"`+rig.deviceID.String()+`","platform":"fcm","push_token":"test-token-1","foreground":true}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, body %s", rec.Code, rec.Body.String())
	}
	var resp registrationResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if !resp.Registered || resp.DeviceID != rig.deviceID.String() || resp.Platform != "fcm" {
		t.Fatalf("response = %+v", resp)
	}
	if resp.ExpiresAt == "" {
		t.Fatal("missing expires_at")
	}
	// Renewable: a second register extends the lease.
	first := resp.ExpiresAt
	rec = rig.do(t, "POST", "/v1/push/register", rig.token(t, rig.userID, rig.deviceID),
		`{"device_id":"`+rig.deviceID.String()+`","platform":"fcm","push_token":"test-token-2"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("renew status = %d, body %s", rec.Code, rec.Body.String())
	}
	var renewed registrationResponse
	_ = json.Unmarshal(rec.Body.Bytes(), &renewed)
	if first == "" || renewed.ExpiresAt == "" {
		t.Fatal("renewal must return an expiry")
	}
}

func TestRegisterLeaseRejections(t *testing.T) {
	rig := newTestRig(t)
	body := `{"device_id":"` + rig.deviceID.String() + `","platform":"fcm","push_token":"test-token"}`
	userID := rig.userID

	// No token at all: 401.
	if rec := rig.do(t, "POST", "/v1/push/register", "", body); rec.Code != http.StatusUnauthorized {
		t.Fatalf("unauthenticated status = %d, want 401", rec.Code)
	}
	// Bad signature: 401.
	bad := mintToken(t, "wrong-test-secret", userID, rig.deviceID, time.Hour)
	if rec := rig.do(t, "POST", "/v1/push/register", bad, body); rec.Code != http.StatusUnauthorized {
		t.Fatalf("forged status = %d, want 401", rec.Code)
	}
	// Foreign device: this token belongs to another device of the same user.
	other := uuid.New()
	rig.resolver.add(userID, other)
	foreignBody := `{"device_id":"` + other.String() + `","platform":"fcm","push_token":"test-token"}`
	if rec := rig.do(t, "POST", "/v1/push/register", rig.token(t, userID, rig.deviceID), foreignBody); rec.Code != http.StatusForbidden {
		t.Fatalf("foreign status = %d, want 403", rec.Code)
	}
	// Token for an unknown device id: 404 (registered devices only).
	ghost := uuid.New()
	ghostToken := rig.token(t, userID, ghost)
	ghostBody := `{"device_id":"` + ghost.String() + `","platform":"fcm","push_token":"test-token"}`
	if rec := rig.do(t, "POST", "/v1/push/register", ghostToken, ghostBody); rec.Code != http.StatusNotFound {
		t.Fatalf("unknown-device status = %d, want 404", rec.Code)
	}
	// Bad platform: 400.
	badPlatform := `{"device_id":"` + rig.deviceID.String() + `","platform":"sms","push_token":"test-token"}`
	if rec := rig.do(t, "POST", "/v1/push/register", rig.token(t, userID, rig.deviceID), badPlatform); rec.Code != http.StatusBadRequest {
		t.Fatalf("bad-platform status = %d, want 400", rec.Code)
	}
}

func TestRotateAndUnregister(t *testing.T) {
	rig := newTestRig(t)
	token := rig.token(t, rig.userID, rig.deviceID)
	id := rig.deviceID.String()

	if rec := rig.do(t, "POST", "/v1/push/register", token,
		`{"device_id":"`+id+`","platform":"apns","push_token":"tok-a","foreground":true}`); rec.Code != http.StatusOK {
		t.Fatalf("register: %d %s", rec.Code, rec.Body.String())
	}

	// Foreground rotation swaps the token.
	if rec := rig.do(t, "POST", "/v1/push/rotate", token,
		`{"device_id":"`+id+`","old_token":"tok-a","new_token":"tok-b","foreground":true}`); rec.Code != http.StatusOK {
		t.Fatalf("rotate: %d %s", rec.Code, rec.Body.String())
	}
	// Stale old token: 409, and the stored token is untouched.
	if rec := rig.do(t, "POST", "/v1/push/rotate", token,
		`{"device_id":"`+id+`","old_token":"tok-a","new_token":"tok-evil"}`); rec.Code != http.StatusConflict {
		t.Fatalf("stale rotate status = %d, want 409", rec.Code)
	}
	// Rotating a device with no registration: 404.
	other := uuid.New()
	rig.resolver.add(rig.userID, other)
	otherToken := rig.token(t, rig.userID, other)
	if rec := rig.do(t, "POST", "/v1/push/rotate", otherToken,
		`{"device_id":"`+other.String()+`","old_token":"x","new_token":"y"}`); rec.Code != http.StatusNotFound {
		t.Fatalf("missing rotate status = %d, want 404", rec.Code)
	}

	// Unregister is durable: gone afterwards, and repeatable only as 404.
	if rec := rig.do(t, "POST", "/v1/push/unregister", token,
		`{"device_id":"`+id+`"`+`}`); rec.Code != http.StatusOK {
		t.Fatalf("unregister: %d %s", rec.Code, rec.Body.String())
	}
	if rec := rig.do(t, "POST", "/v1/push/unregister", token,
		`{"device_id":"`+id+`"`+`}`); rec.Code != http.StatusNotFound {
		t.Fatalf("second unregister status = %d, want 404", rec.Code)
	}
	// A foreign device cannot unregister this device (already gone, but the
	// lease check runs first).
	if rec := rig.do(t, "POST", "/v1/push/unregister", otherToken,
		`{"device_id":"`+id+`"`+`}`); rec.Code != http.StatusForbidden {
		t.Fatalf("foreign unregister status = %d, want 403", rec.Code)
	}
}

func TestListRegistrations(t *testing.T) {
	rig := newTestRig(t)
	other := uuid.New()
	rig.resolver.add(rig.userID, other)

	for _, tc := range []struct{ device, platform, token string }{
		{rig.deviceID.String(), "fcm", "test-token-a"},
		{other.String(), "apns", "test-token-b"},
	} {
		tok := rig.token(t, rig.userID, uuid.MustParse(tc.device))
		rec := rig.do(t, "POST", "/v1/push/register", tok,
			`{"device_id":"`+tc.device+`","platform":"`+tc.platform+`","push_token":"`+tc.token+`"}`)
		if rec.Code != http.StatusOK {
			t.Fatalf("register %s: %d %s", tc.device, rec.Code, rec.Body.String())
		}
	}

	rec := rig.do(t, "GET", "/v1/push/registrations", rig.token(t, rig.userID, rig.deviceID), "")
	if rec.Code != http.StatusOK {
		t.Fatalf("list: %d %s", rec.Code, rec.Body.String())
	}
	var payload struct {
		Registrations []map[string]any `json:"registrations"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &payload); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if len(payload.Registrations) != 2 {
		t.Fatalf("registrations = %d, want 2", len(payload.Registrations))
	}
	// Push tokens are secrets: the list must never echo them.
	if strings.Contains(rec.Body.String(), "test-token-a") || strings.Contains(rec.Body.String(), "test-token-b") {
		t.Fatal("list leaked push tokens")
	}

	// Unauthenticated list: 401.
	if rec := rig.do(t, "GET", "/v1/push/registrations", "", ""); rec.Code != http.StatusUnauthorized {
		t.Fatalf("unauthenticated list status = %d, want 401", rec.Code)
	}
}

func TestMissingDeviceIDInfersVerifiedLease(t *testing.T) {
	rig := newTestRig(t)
	token := rig.token(t, rig.userID, rig.deviceID)

	// Mobile omits device_id (it never decodes its JWT): the verified lease
	// supplies it. Explicit different device_id still 403, ownership checked.
	if rec := rig.do(t, "POST", "/v1/push/register", token,
		`{"platform":"fcm","push_token":"test-token-inferred"}`); rec.Code != http.StatusOK {
		t.Fatalf("inferred register: %d %s", rec.Code, rec.Body.String())
	}
	var resp registrationResponse
	if err := json.Unmarshal(rig.do(t, "POST", "/v1/push/register", token,
		`{"platform":"fcm","push_token":"test-token-inferred-2"}`).Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if !resp.Registered || resp.DeviceID != rig.deviceID.String() {
		t.Fatalf("inferred response = %+v, want device %s", resp, rig.deviceID)
	}

	// Rotate without device_id renews the inferred lease.
	if rec := rig.do(t, "POST", "/v1/push/rotate", token,
		`{"old_token":"test-token-inferred-2","new_token":"test-token-inferred-3"}`); rec.Code != http.StatusOK {
		t.Fatalf("inferred rotate: %d %s", rec.Code, rec.Body.String())
	}

	// Unregister without device_id removes the inferred lease.
	if rec := rig.do(t, "POST", "/v1/push/unregister", token, `{}`); rec.Code != http.StatusOK {
		t.Fatalf("inferred unregister: %d %s", rec.Code, rec.Body.String())
	}

	// Explicit different device_id still 403 (lease check runs first).
	other := uuid.New()
	rig.resolver.add(rig.userID, other)
	if rec := rig.do(t, "POST", "/v1/push/unregister", token,
		`{"device_id":"`+other.String()+`"}`); rec.Code != http.StatusForbidden {
		t.Fatalf("foreign unregister status = %d, want 403", rec.Code)
	}
}
