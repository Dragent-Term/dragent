package push

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

// testAPNsKey generates an ephemeral P-256 key and returns its PKCS#8 PEM.
// No real credentials appear in tests.
func testAPNsKey(t *testing.T) (*ecdsa.PrivateKey, string) {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatalf("generate P-256 key: %v", err)
	}
	der, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		t.Fatalf("marshal PKCS#8: %v", err)
	}
	pemBytes := pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: der})
	return key, string(pemBytes)
}

func testAPNSSender(pemKey string) *APNsSender {
	return &APNsSender{
		Config: APNsConfig{
			KeyID:         "TESTKEYID1",
			TeamID:        "TESTTEAMID",
			BundleID:      "dev.dragent.test",
			PrivateKeyPEM: pemKey,
		},
	}
}

func TestAPNsJWTSignVerify(t *testing.T) {
	key, pemKey := testAPNsKey(t)
	now := time.Unix(1_700_000_000, 0).UTC()
	signed, err := mintAPNsJWT(key, "TESTKEYID1", "TESTTEAMID", now)
	if err != nil {
		t.Fatalf("mintAPNsJWT: %v", err)
	}
	parsed, err := jwt.ParseWithClaims(signed, jwt.MapClaims{}, func(tok *jwt.Token) (any, error) {
		if _, ok := tok.Method.(*jwt.SigningMethodECDSA); !ok {
			t.Fatalf("alg = %v, want ECDSA", tok.Header["alg"])
		}
		if kid, _ := tok.Header["kid"].(string); kid != "TESTKEYID1" {
			t.Fatalf("kid = %q", kid)
		}
		return &key.PublicKey, nil
	})
	if err != nil || !parsed.Valid {
		t.Fatalf("verify JWT: %v valid=%v", err, parsed.Valid)
	}
	claims, _ := parsed.Claims.(jwt.MapClaims)
	if claims["iss"] != "TESTTEAMID" {
		t.Fatalf("iss = %v", claims["iss"])
	}
	if int64(claims["iat"].(float64)) != now.Unix() {
		t.Fatalf("iat = %v want %d", claims["iat"], now.Unix())
	}
	if _, err := parseAPNsPrivateKey(pemKey); err != nil {
		t.Fatalf("parse generated PEM: %v", err)
	}
	if _, err := parseAPNsPrivateKey(strings.ReplaceAll(pemKey, "\n", `\n`)); err != nil {
		t.Fatalf("parse PEM written on one line: %v", err)
	}
	if _, err := parseAPNsPrivateKey("test-pem"); err == nil {
		t.Fatal("invalid PEM was accepted")
	}
}

func TestAPNsSendSerializationHeaders(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	blob := sealedBlobForTest('A')
	var gotAuth, gotTopic, gotPushType, gotPriority, gotCollapse string
	var gotBody map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Errorf("method = %s, want POST", r.Method)
		}
		if r.URL.Path != "/3/device/test-device-token" {
			t.Errorf("path = %s", r.URL.Path)
		}
		gotAuth, gotTopic, gotPushType, gotPriority, gotCollapse = r.Header.Get("authorization"), r.Header.Get("apns-topic"), r.Header.Get("apns-push-type"), r.Header.Get("apns-priority"), r.Header.Get("apns-collapse-id")
		_ = json.NewDecoder(r.Body).Decode(&gotBody)
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()

	s := testAPNSSender(pemKey)
	s.baseURL = srv.URL
	s.httpClient = srv.Client()
	// Plaintext-shaped fields must never reach the wire, even when set.
	if err := s.Send(context.Background(), PushMessage{
		DeviceID: uuid.New(), UserID: uuid.New(), Platform: "apns",
		Token: "test-device-token", Blob: blob,
		Title: "Join approved canary", Body: "secret body canary",
		Data: map[string]string{"kind": "join_approved", "sender": "desktop-1"},
	}); err != nil {
		t.Fatalf("Send: %v", err)
	}
	if !strings.HasPrefix(gotAuth, "bearer ") || len(gotAuth) < len("bearer x.y.z") {
		t.Fatalf("authorization header missing bearer JWT: %q", gotAuth)
	}
	if gotTopic != "dev.dragent.test" {
		t.Fatalf("apns-topic = %q", gotTopic)
	}
	if gotPushType != "alert" {
		t.Fatalf("apns-push-type = %q", gotPushType)
	}
	if gotPriority != "10" {
		t.Fatalf("apns-priority = %q", gotPriority)
	}
	if len(gotCollapse) != 22 {
		t.Fatalf("apns-collapse-id length = %d, want 22 (opaque, fixed)", len(gotCollapse))
	}
	aps, _ := gotBody["aps"].(map[string]any)
	alert, _ := aps["alert"].(map[string]any)
	if alert["title"] != "Dragent" || alert["body"] != "An agent needs you" {
		t.Fatalf("alert = %v, want the constant placeholder", alert)
	}
	if gotBody["e"] != blob {
		t.Fatal("sealed blob missing from payload")
	}
	raw, _ := json.Marshal(gotBody)
	for _, canary := range []string{"canary", "desktop-1", "join_approved"} {
		if strings.Contains(string(raw), canary) {
			t.Fatalf("payload leaks %q: %s", canary, raw)
		}
	}
	for _, key := range []string{"sender", "desktop", "device", "kind", "title", "body"} {
		if _, ok := gotBody[key]; ok {
			t.Fatalf("payload carries %q", key)
		}
	}
}

func TestAPNsPayloadFixedSize(t *testing.T) {
	a, err := apnsPayload(PushMessage{Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')})
	if err != nil {
		t.Fatalf("payload A: %v", err)
	}
	b, err := apnsPayload(PushMessage{Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('B')})
	if err != nil {
		t.Fatalf("payload B: %v", err)
	}
	if len(a) != len(b) {
		t.Fatalf("payload sizes differ: %d vs %d", len(a), len(b))
	}
	if len(a) > apnsMaxPayloadBytes {
		t.Fatalf("payload size %d exceeds the APNs cap", len(a))
	}
	// A dismissal-shaped seal is just another blob: same size, same shape.
	c, err := apnsPayload(PushMessage{Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('C')})
	if err != nil || len(c) != len(a) {
		t.Fatalf("dismissal-shaped payload differs: %v len=%d want %d", err, len(c), len(a))
	}
}

func TestAPNsCollapseIDStableAndOpaque(t *testing.T) {
	a1 := apnsCollapseID(sealedBlobForTest('A'))
	a2 := apnsCollapseID(sealedBlobForTest('A'))
	b := apnsCollapseID(sealedBlobForTest('B'))
	if a1 != a2 {
		t.Fatal("collapse id is not stable for one blob")
	}
	if a1 == b {
		t.Fatal("collapse id does not vary between blobs")
	}
	if len(a1) != 22 {
		t.Fatalf("collapse id length = %d, want 22", len(a1))
	}
}

func TestAPNsRefusesPlaintext(t *testing.T) {
	if _, err := apnsPayload(PushMessage{Platform: "apns", Token: "test-token", Title: "hi", Body: "there"}); err == nil {
		t.Fatal("title/body without a blob was accepted")
	}
	if _, err := apnsPayload(PushMessage{Platform: "apns", Token: "test-token", Blob: strings.Repeat("A", 100)}); err == nil {
		t.Fatal("short blob was accepted")
	}
}

func TestAPNsJWTCacheReuseAndRefresh(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	var auths []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auths = append(auths, r.Header.Get("authorization"))
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()

	now := time.Unix(1_700_000_000, 0).UTC()
	s := testAPNSSender(pemKey)
	s.baseURL = srv.URL
	s.httpClient = srv.Client()
	s.nowFn = func() time.Time { return now }
	msg := PushMessage{DeviceID: uuid.New(), Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("first send: %v", err)
	}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("second send: %v", err)
	}
	if len(auths) != 2 || auths[0] != auths[1] {
		t.Fatalf("JWT was not reused: %q", auths)
	}
	now = now.Add(apnsTokenReuse + time.Minute)
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("post-expiry send: %v", err)
	}
	if len(auths) != 3 || auths[2] == auths[0] {
		t.Fatal("expired JWT was not refreshed")
	}
}

func TestAPNsFailureClassification(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	cases := []struct {
		name       string
		status     int
		reason     string
		check      func(error) bool
		wantErrNil bool
	}{
		{"bad device token", 400, "BadDeviceToken", IsInvalidToken, false},
		{"unregistered gone", 410, "Unregistered", IsInvalidToken, false},
		{"invalid provider token", 403, "InvalidProviderToken", IsAuthError, false},
		{"rate limited", 429, "TooManyRequests", IsRetryable, false},
		{"server error", 500, "InternalError", IsRetryable, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("content-type", "application/json")
				w.WriteHeader(tc.status)
				_ = json.NewEncoder(w).Encode(map[string]string{"reason": tc.reason})
			}))
			defer srv.Close()
			s := testAPNSSender(pemKey)
			s.baseURL = srv.URL
			s.httpClient = srv.Client()
			err := s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')})
			if tc.wantErrNil {
				if err != nil {
					t.Fatalf("Send: %v", err)
				}
				return
			}
			if err == nil {
				t.Fatal("expected an error")
			}
			if !tc.check(err) {
				t.Fatalf("error %v failed classification", err)
			}
			if strings.Contains(err.Error(), "test-token") || strings.Contains(err.Error(), "test-pem") {
				t.Fatalf("error leaks sensitive input: %v", err)
			}
		})
	}
}

func TestAPNsExpiredProviderTokenRefreshesOnce(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	var calls atomic.Int64
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			w.WriteHeader(http.StatusForbidden)
			_ = json.NewEncoder(w).Encode(map[string]string{"reason": "ExpiredProviderToken"})
			return
		}
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()
	s := testAPNSSender(pemKey)
	s.baseURL = srv.URL
	s.httpClient = srv.Client()
	if err := s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')}); err != nil {
		t.Fatalf("Send after refresh: %v", err)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2 (one refresh retry)", calls.Load())
	}
}

func TestAPNsContextCancellation(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		t.Error("server should not be hit after cancellation")
	}))
	defer srv.Close()
	s := testAPNSSender(pemKey)
	s.baseURL = srv.URL
	s.httpClient = srv.Client()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := s.Send(ctx, PushMessage{DeviceID: uuid.New(), Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
		t.Fatal("cancelled send succeeded")
	}
}

func TestAPNsPayloadConstantSizeEnforced(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	s := testAPNSSender(pemKey)
	s.baseURL = "http://127.0.0.1:1"
	s.httpClient = &http.Client{}
	// The payload is constant-size by construction: blobs that differ only
	// in content are accepted and stay far under the APNs cap.
	for _, fill := range []byte{'A', 'B', 'C'} {
		raw, err := apnsPayload(PushMessage{Platform: "apns", Token: "test-token", Blob: sealedBlobForTest(fill)})
		if err != nil {
			t.Fatalf("payload %c: %v", fill, err)
		}
		if len(raw) > apnsMaxPayloadBytes {
			t.Fatalf("payload size %d exceeds the APNs cap", len(raw))
		}
	}
}

func TestAPNsRedirectNotFollowed(t *testing.T) {
	_, pemKey := testAPNsKey(t)
	var evilHits atomic.Int64
	evil := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		evilHits.Add(1)
		w.WriteHeader(http.StatusOK)
	}))
	defer evil.Close()
	sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("location", evil.URL)
		w.WriteHeader(http.StatusFound)
	}))
	defer sendSrv.Close()
	s := testAPNSSender(pemKey)
	// Default client (nil injection) enforces the fixed-host no-redirect rule.
	s.baseURL = sendSrv.URL
	if err := s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
		t.Fatal("redirect was accepted")
	}
	if evilHits.Load() != 0 {
		t.Fatal("redirect target was followed")
	}
}
