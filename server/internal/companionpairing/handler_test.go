package companionpairing

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"

	identityModel "server/internal/identity/model"
	identityTypes "server/internal/identity/types"
)

type fakeUsers struct {
	mu    sync.Mutex
	users map[uuid.UUID]*identityModel.User
}

func (f *fakeUsers) GetByID(_ context.Context, id uuid.UUID) (*identityModel.User, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	u, ok := f.users[id]
	if !ok {
		return nil, fmt.Errorf("unknown user")
	}
	return u, nil
}

type fakeTokens struct {
	mu     sync.Mutex
	claims map[string]*identityTypes.TokenClaims
	now    func() time.Time
}

func (f *fakeTokens) GenerateScopedAccessToken(userID, deviceID uuid.UUID, scope string) (string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	token := "fake-token-" + uuid.NewString()
	now := time.Now()
	if f.now != nil {
		now = f.now()
	}
	f.claims[token] = &identityTypes.TokenClaims{
		UserID:   userID,
		DeviceID: deviceID,
		Scope:    scope,
		RegisteredClaims: jwt.RegisteredClaims{
			ExpiresAt: jwt.NewNumericDate(now.Add(24 * time.Hour)),
			IssuedAt:  jwt.NewNumericDate(now),
		},
	}
	return token, nil
}

func (f *fakeTokens) VerifyAccessToken(token string) (*identityTypes.TokenClaims, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	claims, ok := f.claims[token]
	if !ok {
		return nil, fmt.Errorf("bad token")
	}
	return claims, nil
}

type fakeCloser struct {
	mu    sync.Mutex
	calls [][2]string
}

func (f *fakeCloser) CloseDeviceSockets(userID, deviceID string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, [2]string{userID, deviceID})
}

func (f *fakeCloser) closed(userID, deviceID string) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, c := range f.calls {
		if c[0] == userID && c[1] == deviceID {
			return true
		}
	}
	return false
}

func testHandler(store Store, users *fakeUsers, tokens *fakeTokens, closer *fakeCloser, opts ...Option) *Handler {
	base := []Option{
		WithOrgResolver(func(_ context.Context, _ string) string { return "org-test" }),
		WithSocketCloser(closer),
	}
	return NewHandler(store, users, tokens, "https://api.example.test", append(base, opts...)...)
}

func authedRouter(h *Handler, user string) http.Handler {
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterRoutes(r)
	h.RegisterPublicRoutes(r)
	return r
}

func doRequest(t *testing.T, router http.Handler, method, path string, body any, headers map[string]string) *httptest.ResponseRecorder {
	t.Helper()
	var reader *bytes.Reader
	if body == nil {
		reader = bytes.NewReader(nil)
	} else {
		raw, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		reader = bytes.NewReader(raw)
	}
	req := httptest.NewRequest(method, path, reader)
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	router.ServeHTTP(rec, req)
	return rec
}

func decodeBody(t *testing.T, rec *httptest.ResponseRecorder) map[string]any {
	t.Helper()
	var out map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &out); err != nil {
		t.Fatalf("response is not JSON: %v (%q)", err, rec.Body.String())
	}
	return out
}

func TestCreatePairingEndpoint(t *testing.T) {
	now := testNow
	s := testStore(&now)
	h := testHandler(s, &fakeUsers{}, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, &fakeCloser{})
	router := authedRouter(h, "user-a")

	rec := doRequest(t, router, "POST", "/v1/companion/pairings", nil, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("status %d: %s", rec.Code, rec.Body.String())
	}
	body := decodeBody(t, rec)
	code, _ := body["code"].(string)
	if len(code) != CodeLength {
		t.Fatalf("code %q", code)
	}
	if _, ok := normalizeCode(code); !ok {
		t.Fatalf("code fails validation: %q", code)
	}
	if _, err := uuid.Parse(body["pairing_id"].(string)); err != nil {
		t.Fatalf("pairing_id: %v", err)
	}
	expires, err := time.Parse(time.RFC3339, body["expires_at"].(string))
	if err != nil || expires.Sub(now) != PairingTTL {
		t.Fatalf("expires_at %v %v", body["expires_at"], err)
	}
	if body["gateway_url"] != "https://api.example.test" {
		t.Fatalf("gateway_url %v", body["gateway_url"])
	}

	unauth := doRequest(t, authedRouter(h, ""), "POST", "/v1/companion/pairings", nil, nil)
	if unauth.Code != http.StatusUnauthorized {
		t.Fatalf("unauth status %d", unauth.Code)
	}
	if errText := decodeBody(t, unauth)["error"]; errText != "unauthorized" {
		t.Fatalf("unauth error %v", errText)
	}
}

func TestRedeemEndpoint(t *testing.T) {
	now := testNow
	s := testStore(&now)
	users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{}}
	ownerID := uuid.New()
	users.users[ownerID] = &identityModel.User{ID: ownerID, DisplayName: "Ada", Email: "ada@example.test"}
	h := testHandler(s, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, &fakeCloser{})
	router := authedRouter(h, "user-a")

	// Pair as user-a, then point the redeem at the fake owner by swapping
	// the store user: create directly in the store with the owner id.
	_, code, err := s.CreatePairing(context.Background(), ownerID.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	rec := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "ios"}, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d: %s", rec.Code, rec.Body.String())
	}
	body := decodeBody(t, rec)
	if _, err := uuid.Parse(body["device_id"].(string)); err != nil {
		t.Fatalf("device_id: %v", err)
	}
	token, _ := body["device_token"].(string)
	if len(token) != 64 {
		t.Fatalf("device_token length %d", len(token))
	}
	user, _ := body["user"].(map[string]any)
	if user["id"] != ownerID.String() || user["display_name"] != "Ada" {
		t.Fatalf("user %v", user)
	}
	if body["gateway_url"] != "https://api.example.test" {
		t.Fatalf("gateway_url %v", body["gateway_url"])
	}

	// Single use: the second redeem is gone.
	rec = doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "ios"}, nil)
	if rec.Code != http.StatusGone || decodeBody(t, rec)["error"] != "gone" {
		t.Fatalf("second redeem: %d %s", rec.Code, rec.Body.String())
	}

	// Unknown but well-formed code is invalid, not gone.
	rec = doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": strings.Repeat("A", CodeLength), "platform": "ios"}, nil)
	if rec.Code != http.StatusBadRequest || decodeBody(t, rec)["error"] != "invalid" {
		t.Fatalf("unknown code: %d %s", rec.Code, rec.Body.String())
	}

	// Malformed bodies are invalid without consuming anything.
	_, code2, err := s.CreatePairing(context.Background(), ownerID.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	rec = doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code2, "platform": "symbian"}, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("bad platform: %d", rec.Code)
	}
	rec = doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code2, "platform": "ios"}, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("code consumed by invalid redeem: %d", rec.Code)
	}
}

func TestRotateEndpointAndReuseRevocation(t *testing.T) {
	now := testNow
	s := testStore(&now)
	users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{}}
	ownerID := uuid.New()
	users.users[ownerID] = &identityModel.User{ID: ownerID, FirstName: "Ada", LastName: "L", Email: "ada@example.test"}
	tokens := &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}
	closer := &fakeCloser{}
	h := testHandler(s, users, tokens, closer, WithClock(func() time.Time { return now }))
	router := authedRouter(h, "user-a")

	_, code, err := s.CreatePairing(context.Background(), ownerID.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	redeemed := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "android"}, nil)
	redeemBody := decodeBody(t, redeemed)
	deviceID, deviceToken := redeemBody["device_id"].(string), redeemBody["device_token"].(string)

	rotated := doRequest(t, router, "POST", "/v1/companion/devices/token",
		map[string]string{"device_id": deviceID, "device_token": deviceToken}, nil)
	if rotated.Code != http.StatusOK {
		t.Fatalf("rotate status %d: %s", rotated.Code, rotated.Body.String())
	}
	rotatedBody := decodeBody(t, rotated)
	if rotatedBody["access_token"] == "" || rotatedBody["access_token"] == deviceToken {
		t.Fatalf("access token missing: %v", rotatedBody)
	}
	nextToken, _ := rotatedBody["device_token"].(string)
	if nextToken == "" || nextToken == deviceToken {
		t.Fatalf("device token not rotated: %v", rotatedBody)
	}
	if _, err := time.Parse(time.RFC3339, rotatedBody["access_expires_at"].(string)); err != nil {
		t.Fatalf("access_expires_at: %v", err)
	}

	// Reusing the rotated-out token revokes the device: 401 + sockets closed.
	reused := doRequest(t, router, "POST", "/v1/companion/devices/token",
		map[string]string{"device_id": deviceID, "device_token": deviceToken}, nil)
	if reused.Code != http.StatusUnauthorized {
		t.Fatalf("reuse status %d", reused.Code)
	}
	if !closer.closed(ownerID.String(), deviceID) {
		t.Fatal("revoked device sockets were not closed")
	}
	// Even the fresh token is dead after revocation.
	dead := doRequest(t, router, "POST", "/v1/companion/devices/token",
		map[string]string{"device_id": deviceID, "device_token": nextToken}, nil)
	if dead.Code != http.StatusUnauthorized {
		t.Fatalf("revoked rotate status %d", dead.Code)
	}
	// Random credentials are unauthorized without closing anything.
	bogus := doRequest(t, router, "POST", "/v1/companion/devices/token",
		map[string]string{"device_id": uuid.NewString(), "device_token": "nope"}, nil)
	if bogus.Code != http.StatusUnauthorized {
		t.Fatalf("bogus status %d", bogus.Code)
	}
}

func TestDevicesListAndRevoke(t *testing.T) {
	now := testNow
	s := testStore(&now)
	owner := uuid.New()
	other := uuid.New()
	users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{
		owner: {ID: owner, DisplayName: "Owner", Email: "owner@example.test"},
	}}
	h := testHandler(s, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, &fakeCloser{})
	ownerRouter := authedRouter(h, owner.String())
	otherRouter := authedRouter(h, other.String())

	rec := doRequest(t, ownerRouter, "POST", "/v1/companion/pairings", nil, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create status %d: %s", rec.Code, rec.Body.String())
	}
	code := decodeBody(t, rec)["code"].(string)
	redeemed := doRequest(t, ownerRouter, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "android"}, nil)
	if redeemed.Code != http.StatusOK {
		t.Fatalf("redeem status %d: %s", redeemed.Code, redeemed.Body.String())
	}

	listed := doRequest(t, ownerRouter, "GET", "/v1/companion/devices", nil, nil)
	if listed.Code != http.StatusOK {
		t.Fatalf("list status %d", listed.Code)
	}
	var listBody struct {
		Devices []map[string]any `json:"devices"`
	}
	if err := json.Unmarshal(listed.Body.Bytes(), &listBody); err != nil {
		t.Fatal(err)
	}
	if len(listBody.Devices) != 1 {
		t.Fatalf("devices %v", listBody.Devices)
	}
	deviceID := listBody.Devices[0]["device_id"].(string)
	for _, field := range []string{"device_id", "platform", "e2e", "created_at", "last_seen_at"} {
		if _, ok := listBody.Devices[0][field]; !ok {
			t.Fatalf("missing field %s in %v", field, listBody.Devices[0])
		}
	}
	if _, named := listBody.Devices[0]["name"]; named {
		t.Fatalf("device name must not be served: %v", listBody.Devices[0])
	}
	if listBody.Devices[0]["platform"] != "android" || listBody.Devices[0]["e2e"] != true {
		t.Fatalf("device fields %v", listBody.Devices[0])
	}

	// Cross-user: the other account sees nothing and cannot revoke.
	foreign := doRequest(t, otherRouter, "GET", "/v1/companion/devices", nil, nil)
	var foreignBody struct {
		Devices []map[string]any `json:"devices"`
	}
	if err := json.Unmarshal(foreign.Body.Bytes(), &foreignBody); err != nil || len(foreignBody.Devices) != 0 {
		t.Fatalf("foreign list leaks: %s", foreign.Body.String())
	}
	denied := doRequest(t, otherRouter, "DELETE", "/v1/companion/devices/"+deviceID, nil, nil)
	if denied.Code != http.StatusBadRequest || decodeBody(t, denied)["error"] != "invalid" {
		t.Fatalf("foreign revoke: %d %s", denied.Code, denied.Body.String())
	}
	still := doRequest(t, ownerRouter, "GET", "/v1/companion/devices", nil, nil)
	var stillBody struct {
		Devices []map[string]any `json:"devices"`
	}
	if err := json.Unmarshal(still.Body.Bytes(), &stillBody); err != nil || len(stillBody.Devices) != 1 {
		t.Fatalf("device lost to foreign revoke: %s", still.Body.String())
	}

	// Owner revokes: sockets close and the token endpoint 401s.
	closer := &fakeCloser{}
	h2 := testHandler(s, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, closer)
	ownerRouter2 := authedRouter(h2, owner.String())
	revoked := doRequest(t, ownerRouter2, "DELETE", "/v1/companion/devices/"+deviceID, nil, nil)
	if revoked.Code != http.StatusOK || decodeBody(t, revoked)["revoked"] != true {
		t.Fatalf("revoke: %d %s", revoked.Code, revoked.Body.String())
	}
	if !closer.closed(owner.String(), deviceID) {
		t.Fatal("revoke did not close sockets")
	}
	again := doRequest(t, ownerRouter2, "DELETE", "/v1/companion/devices/"+deviceID, nil, nil)
	if again.Code != http.StatusBadRequest {
		t.Fatalf("double revoke: %d", again.Code)
	}
	unknown := doRequest(t, ownerRouter2, "DELETE", "/v1/companion/devices/"+uuid.NewString(), nil, nil)
	if unknown.Code != http.StatusBadRequest {
		t.Fatalf("unknown revoke: %d", unknown.Code)
	}
}

func TestRedeemRateLimit(t *testing.T) {
	now := testNow
	s := testStore(&now)
	newHandler := func(ipLimit, codeLimit int) *Handler {
		return testHandler(s,
			&fakeUsers{},
			&fakeTokens{claims: map[string]*identityTypes.TokenClaims{}},
			&fakeCloser{},
			WithRateLimiters(
				NewMemoryRateLimiter(ipLimit, time.Minute),
				NewMemoryRateLimiter(codeLimit, time.Minute),
			))
	}
	// Per-IP: two attempts allowed, the third is rate-limited.
	router := authedRouter(newHandler(2, 100), "user-a")
	wellFormed := strings.Repeat("B", CodeLength)
	for i := 0; i < 2; i++ {
		rec := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
			map[string]string{"code": wellFormed, "platform": "ios"},
			map[string]string{"X-Forwarded-For": "10.0.0.9"})
		if rec.Code != http.StatusBadRequest {
			t.Fatalf("attempt %d: %d", i, rec.Code)
		}
	}
	limited := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": wellFormed, "platform": "ios"},
		map[string]string{"X-Forwarded-For": "10.0.0.9"})
	if limited.Code != http.StatusTooManyRequests || decodeBody(t, limited)["error"] != "rate_limited" {
		t.Fatalf("per-IP limit: %d %s", limited.Code, limited.Body.String())
	}

	// Rotating X-Forwarded-For from the same caller doesn't reset the
	// per-IP count: only our proxy's address is trusted.
	spoofRouter := authedRouter(newHandler(2, 100), "user-a")
	for i := 0; i < 3; i++ {
		rec := doRequest(t, spoofRouter, "POST", "/v1/companion/pairings/redeem",
			map[string]string{"code": wellFormed, "platform": "ios"},
			map[string]string{"X-Forwarded-For": fmt.Sprintf("198.51.100.%d", i+1), "X-Real-IP": fmt.Sprintf("198.51.100.%d", i+1)})
		if i == 2 && rec.Code != http.StatusTooManyRequests {
			t.Fatalf("spoofed X-Forwarded-For escaped the per-IP limit: %d", rec.Code)
		}
	}

	// Per-code: the same code twice trips the code leg from another IP.
	router2 := authedRouter(newHandler(100, 1), "user-a")
	first := doRequest(t, router2, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": wellFormed, "platform": "ios"},
		map[string]string{"X-Forwarded-For": "10.0.0.10"})
	if first.Code != http.StatusBadRequest {
		t.Fatalf("first attempt: %d", first.Code)
	}
	second := doRequest(t, router2, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": wellFormed, "platform": "ios"},
		map[string]string{"X-Forwarded-For": "10.0.0.11"})
	if second.Code != http.StatusTooManyRequests {
		t.Fatalf("per-code limit: %d", second.Code)
	}
}

func TestRequireValidDevice(t *testing.T) {
	now := testNow
	s := testStore(&now)
	tokens := &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}
	h := testHandler(s, &fakeUsers{}, tokens, &fakeCloser{})
	next := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	})
	guarded := h.RequireValidDevice(next)

	userID, deviceID := uuid.New(), uuid.New()
	portalToken, _ := tokens.GenerateScopedAccessToken(userID, deviceID, "")
	companionToken, _ := tokens.GenerateScopedAccessToken(userID, deviceID, AccessScope)

	call := func(token, user string) int {
		req := httptest.NewRequest("POST", "/v1/relay/rpc", nil)
		if token != "" {
			req.Header.Set("Authorization", "Bearer "+token)
		}
		if user != "" {
			req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
		}
		rec := httptest.NewRecorder()
		guarded.ServeHTTP(rec, req)
		return rec.Code
	}

	if code := call(portalToken, userID.String()); code != http.StatusOK {
		t.Fatalf("portal token gated: %d", code)
	}
	if code := call(companionToken, userID.String()); code != http.StatusUnauthorized {
		t.Fatalf("unknown companion device passes: %d", code)
	}
	if code := call("", userID.String()); code != http.StatusUnauthorized {
		t.Fatalf("missing token passes: %d", code)
	}
	if code := call(portalToken, ""); code != http.StatusUnauthorized {
		t.Fatalf("missing user passes: %d", code)
	}

	// A live device passes; after revoke it is unauthorized.
	_, code, err := s.CreatePairing(context.Background(), userID.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	device, _, _, _, err := s.RedeemPairing(context.Background(), code, "ios")
	if err != nil {
		t.Fatal(err)
	}
	liveToken, _ := tokens.GenerateScopedAccessToken(userID, uuid.MustParse(device.ID), AccessScope)
	if code := call(liveToken, userID.String()); code != http.StatusOK {
		t.Fatalf("live companion device gated: %d", code)
	}
	if err := s.RevokeDevice(context.Background(), userID.String(), device.ID); err != nil {
		t.Fatal(err)
	}
	if code := call(liveToken, userID.String()); code != http.StatusUnauthorized {
		t.Fatalf("revoked companion device passes: %d", code)
	}
}

func TestPairingUsesAPIOriginThroughRedeem(t *testing.T) {
	for _, tc := range []struct{ name, target, forwarded, configured, want string }{
		{"tls", "https://api.example.test", "", "", "https://api.example.test"},
		{"proxy", "http://api.example.test", "https", "", "https://api.example.test"},
		{"local", "http://localhost:8080", "", "", "http://localhost:8080"},
		{"override", "http://internal:8080", "", "https://public.example.test/", "https://public.example.test"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			now := testNow
			store := testStore(&now)
			owner := uuid.New()
			users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{owner: {ID: owner, DisplayName: "Ada"}}}
			h := NewHandler(store, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, tc.configured)
			router := authedRouter(h, owner.String())
			request := func(path, body string) *httptest.ResponseRecorder {
				req := httptest.NewRequest(http.MethodPost, tc.target+path, strings.NewReader(body))
				req.Header.Set("X-Forwarded-Proto", tc.forwarded)
				req.Header.Set("Origin", "https://website.example.test")
				req.Header.Set("X-Forwarded-Host", "website.example.test")
				rec := httptest.NewRecorder()
				router.ServeHTTP(rec, req)
				return rec
			}
			created := request("/v1/companion/pairings", "{}")
			if created.Code != http.StatusCreated {
				t.Fatalf("create status %d", created.Code)
			}
			body := decodeBody(t, created)
			if body["gateway_url"] != tc.want {
				t.Fatalf("QR gateway %v; want %s", body["gateway_url"], tc.want)
			}
			payload, _ := json.Marshal(map[string]any{"code": body["code"], "platform": "ios"})
			redeemed := request("/v1/companion/pairings/redeem", string(payload))
			if redeemed.Code != http.StatusOK {
				t.Fatalf("redeem status %d", redeemed.Code)
			}
			if got := decodeBody(t, redeemed)["gateway_url"]; got != tc.want {
				t.Fatalf("redeem gateway %v; want %s", got, tc.want)
			}
		})
	}
}

// E2EE cut-over (§6.2, H2, §12.5, §12.7): the redeem request carries no
// device name, only ios/android platform; access_token query parameters are
// refused on the pairing routes.
func TestRedeemCutover(t *testing.T) {
	now := testNow
	s := testStore(&now)
	users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{}}
	ownerID := uuid.New()
	users.users[ownerID] = &identityModel.User{ID: ownerID, DisplayName: "Ada", Email: "ada@example.test"}
	h := testHandler(s, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, &fakeCloser{})
	router := authedRouter(h, ownerID.String())

	_, code, err := s.CreatePairing(context.Background(), ownerID.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	// A redeem carrying a device name is a pre-cut-over client: refused,
	// and the code is not consumed.
	rec := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "device_name": "Ada's phone", "platform": "ios"}, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("device_name redeem: %d %s", rec.Code, rec.Body.String())
	}
	// The web platform is gone: only ios/android redeem.
	rec = doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "web"}, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("web redeem: %d %s", rec.Code, rec.Body.String())
	}
	// The same code still redeems cleanly without a name.
	rec = doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "ios"}, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("clean redeem: %d %s", rec.Code, rec.Body.String())
	}
	deviceID, _ := decodeBody(t, rec)["device_id"].(string)
	// The redeemed device carries the cut-over flag and no name.
	devices, err := s.ListDevices(context.Background(), ownerID.String())
	if err != nil || len(devices) != 1 || devices[0].ID != deviceID {
		t.Fatalf("list: %+v %v", devices, err)
	}
	if devices[0].Name != "" || !devices[0].E2E || devices[0].Platform != "ios" {
		t.Fatalf("cut-over device: %+v", devices[0])
	}
	if known, e2e, err := s.DeviceE2E(context.Background(), ownerID.String(), deviceID); err != nil || !known || !e2e {
		t.Fatalf("DeviceE2E: %v %v %v", known, e2e, err)
	}
	if known, _, _ := s.DeviceE2E(context.Background(), ownerID.String(), uuid.NewString()); known {
		t.Fatal("unknown device known")
	}
	// access_token query parameters are refused on the pairing routes (H2):
	// the guarded endpoint without an Authorization header is unauthorized
	// even when the query carries a token.
	next := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	})
	guarded := h.RequireValidDevice(next)
	req := httptest.NewRequest("POST", "/v1/relay/rpc?access_token=sometoken", nil)
	req = req.WithContext(context.WithValue(req.Context(), "user_id", ownerID.String()))
	recorder := httptest.NewRecorder()
	guarded.ServeHTTP(recorder, req)
	if recorder.Code != http.StatusUnauthorized {
		t.Fatalf("query token accepted: %d", recorder.Code)
	}
}

// FIX1 (§8): revoking a companion device must also drop its push
// registration — the registrations table no longer cascades off the devices
// table (migration 00063), so the pairing handler runs the integrator's
// push hook best-effort on every revocation path.
type recordingPushCleanup struct {
	mu    sync.Mutex
	calls [][2]string
}

func (f *recordingPushCleanup) hook(_ context.Context, userID, deviceID string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, [2]string{userID, deviceID})
}

func (f *recordingPushCleanup) cleaned(userID, deviceID string) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, c := range f.calls {
		if c[0] == userID && c[1] == deviceID {
			return true
		}
	}
	return false
}

func (f *recordingPushCleanup) count() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.calls)
}

func TestRevokeRunsPushCleanup(t *testing.T) {
	now := testNow
	s := testStore(&now)
	owner := uuid.New()
	users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{
		owner: {ID: owner, DisplayName: "Owner", Email: "owner@example.test"},
	}}
	cleanup := &recordingPushCleanup{}
	h := testHandler(s, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, &fakeCloser{},
		WithPushCleanup(cleanup.hook))
	router := authedRouter(h, owner.String())

	created := doRequest(t, router, "POST", "/v1/companion/pairings", nil, nil)
	if created.Code != http.StatusCreated {
		t.Fatalf("create status %d: %s", created.Code, created.Body.String())
	}
	code := decodeBody(t, created)["code"].(string)
	redeemed := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "ios"}, nil)
	if redeemed.Code != http.StatusOK {
		t.Fatalf("redeem status %d: %s", redeemed.Code, redeemed.Body.String())
	}
	deviceID := decodeBody(t, redeemed)["device_id"].(string)

	revoked := doRequest(t, router, "DELETE", "/v1/companion/devices/"+deviceID, nil, nil)
	if revoked.Code != http.StatusOK {
		t.Fatalf("revoke: %d %s", revoked.Code, revoked.Body.String())
	}
	if !cleanup.cleaned(owner.String(), deviceID) {
		t.Fatal("revoke did not run the push cleanup hook")
	}

	// Unknown and foreign revokes fail without touching push registrations.
	before := cleanup.count()
	unknown := doRequest(t, router, "DELETE", "/v1/companion/devices/"+uuid.NewString(), nil, nil)
	if unknown.Code != http.StatusBadRequest {
		t.Fatalf("unknown revoke: %d", unknown.Code)
	}
	if cleanup.count() != before {
		t.Fatal("failed revoke ran the push cleanup hook")
	}
}

// Token-reuse revocation (a rotated token presented again deletes the
// device as compromised) must clean the push registration too.
func TestTokenReuseRevocationRunsPushCleanup(t *testing.T) {
	now := testNow
	s := testStore(&now)
	owner := uuid.New()
	users := &fakeUsers{users: map[uuid.UUID]*identityModel.User{
		owner: {ID: owner, DisplayName: "Owner", Email: "owner@example.test"},
	}}
	cleanup := &recordingPushCleanup{}
	h := testHandler(s, users, &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}, &fakeCloser{},
		WithPushCleanup(cleanup.hook))
	router := authedRouter(h, owner.String())

	_, code, err := s.CreatePairing(context.Background(), owner.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	redeemed := doRequest(t, router, "POST", "/v1/companion/pairings/redeem",
		map[string]string{"code": code, "platform": "android"}, nil)
	if redeemed.Code != http.StatusOK {
		t.Fatalf("redeem status %d: %s", redeemed.Code, redeemed.Body.String())
	}
	body := decodeBody(t, redeemed)
	deviceID, _ := body["device_id"].(string)
	firstToken, _ := body["device_token"].(string)

	rotated := doRequest(t, router, "POST", "/v1/companion/devices/token",
		map[string]string{"device_id": deviceID, "device_token": firstToken}, nil)
	if rotated.Code != http.StatusOK {
		t.Fatalf("rotate status %d: %s", rotated.Code, rotated.Body.String())
	}
	if cleanup.count() != 0 {
		t.Fatal("clean rotation ran the push cleanup hook")
	}

	// Presenting the immediately-previous token revokes the device.
	reused := doRequest(t, router, "POST", "/v1/companion/devices/token",
		map[string]string{"device_id": deviceID, "device_token": firstToken}, nil)
	if reused.Code != http.StatusUnauthorized {
		t.Fatalf("reuse status %d, want 401", reused.Code)
	}
	if !cleanup.cleaned(owner.String(), deviceID) {
		t.Fatal("token-reuse revocation did not run the push cleanup hook")
	}
}

// A revoked phone's token is refused on every route; everything else passes
// through to the route's own checks.
func TestRejectRevokedDevicesEverywhere(t *testing.T) {
	now := testNow
	s := testStore(&now)
	tokens := &fakeTokens{claims: map[string]*identityTypes.TokenClaims{}}
	h := testHandler(s, &fakeUsers{}, tokens, &fakeCloser{})
	guarded := h.RejectRevokedDevices(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	call := func(token string) int {
		req := httptest.NewRequest("GET", "/v1/usage", nil)
		if token != "" {
			req.Header.Set("Authorization", "Bearer "+token)
		}
		rec := httptest.NewRecorder()
		guarded.ServeHTTP(rec, req)
		return rec.Code
	}

	userID := uuid.New()
	portalToken, _ := tokens.GenerateScopedAccessToken(userID, uuid.New(), "")
	if code := call(""); code != http.StatusOK {
		t.Fatalf("anonymous request blocked: %d", code)
	}
	if code := call(portalToken); code != http.StatusOK {
		t.Fatalf("portal token blocked: %d", code)
	}
	if code := call("not-a-token"); code != http.StatusOK {
		t.Fatalf("unverifiable token should be left to the route: %d", code)
	}

	_, code, err := s.CreatePairing(context.Background(), userID.String(), "org-test")
	if err != nil {
		t.Fatal(err)
	}
	device, _, _, _, err := s.RedeemPairing(context.Background(), code, "ios")
	if err != nil {
		t.Fatal(err)
	}
	phoneToken, _ := tokens.GenerateScopedAccessToken(userID, uuid.MustParse(device.ID), AccessScope)
	if code := call(phoneToken); code != http.StatusOK {
		t.Fatalf("paired phone blocked: %d", code)
	}
	if err := s.RevokeDevice(context.Background(), userID.String(), device.ID); err != nil {
		t.Fatal(err)
	}
	if code := call(phoneToken); code != http.StatusUnauthorized {
		t.Fatalf("revoked phone still reaches the API: %d", code)
	}
}
