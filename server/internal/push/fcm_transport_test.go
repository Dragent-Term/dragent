package push

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

// testFCMKey generates an ephemeral RSA key and returns its PKCS#8 PEM.
// No real credentials appear in tests.
func testFCMKey(t *testing.T) (*rsa.PrivateKey, string) {
	t.Helper()
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatalf("generate RSA key: %v", err)
	}
	der, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		t.Fatalf("marshal PKCS#8: %v", err)
	}
	return key, string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: der}))
}

func testFCMAccountJSON(project, email, keyID, pemKey, tokenURI string) string {
	raw, _ := json.Marshal(map[string]string{
		"type": "service_account", "project_id": project,
		"private_key_id": keyID, "private_key": pemKey,
		"client_email": email, "token_uri": tokenURI,
	})
	return string(raw)
}

func testFCMSender(project, creds string) *FCMSender {
	return &FCMSender{Config: FCMConfig{ProjectID: project, CredentialsJSON: creds}}
}

func TestFCMAssertionSignVerify(t *testing.T) {
	key, pemKey := testFCMKey(t)
	acct := &fcmServiceAccount{PrivateKey: pemKey, PrivateKeyX: "test-key-id", ClientEmail: "test@test-project.iam.gserviceaccount.com"}
	signer, alg, err := parseFCMPrivateKey(acct.PrivateKey)
	if err != nil {
		t.Fatalf("parse generated key: %v", err)
	}
	now := time.Now().UTC().Truncate(time.Second)
	assertion, err := mintFCMAssertion(acct, signer, alg, fcmTokenURL, now)
	if err != nil {
		t.Fatalf("mintFCMAssertion: %v", err)
	}
	parsed, err := jwt.ParseWithClaims(assertion, jwt.MapClaims{}, func(tok *jwt.Token) (any, error) {
		if _, ok := tok.Method.(*jwt.SigningMethodRSA); !ok {
			t.Fatalf("alg = %v, want RSA", tok.Header["alg"])
		}
		return &key.PublicKey, nil
	})
	if err != nil || !parsed.Valid {
		t.Fatalf("verify assertion: %v valid=%v", err, parsed.Valid)
	}
	claims, _ := parsed.Claims.(jwt.MapClaims)
	if claims["iss"] != acct.ClientEmail || claims["scope"] != fcmOAuthScope || claims["aud"] != fcmTokenURL {
		t.Fatalf("claims = %v", claims)
	}
	if int64(claims["exp"].(float64))-int64(claims["iat"].(float64)) != 3600 {
		t.Fatalf("ttl = %v", claims)
	}
	if _, _, err := parseFCMPrivateKey("test-pem"); err == nil {
		t.Fatal("invalid PEM was accepted")
	}
	// Non-official token endpoints are rejected outright.
	bad := *acct
	bad.TokenURI = "https://evil.example/token"
	if _, err := parseFCMAccount(mustJSON(t, bad)); err == nil {
		t.Fatal("arbitrary token endpoint was accepted")
	}
}

func mustJSON(t *testing.T, v any) string {
	t.Helper()
	raw, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return string(raw)
}

func TestFCMSendSerializationTokenCache(t *testing.T) {
	_, pemKey := testFCMKey(t)
	var tokenHits atomic.Int64
	tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		tokenHits.Add(1)
		if err := r.ParseForm(); err != nil {
			t.Errorf("parse token form: %v", err)
		}
		if r.Form.Get("grant_type") != fcmTokenGrant || r.Form.Get("assertion") == "" {
			t.Errorf("token form = %v", r.Form)
		}
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "test-access-token", "expires_in": 3600, "token_type": "Bearer"})
	}))
	defer tokenSrv.Close()

	var gotAuth, gotContentType string
	var gotBody map[string]any
	var sendHits atomic.Int64
	sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sendHits.Add(1)
		if r.URL.Path != "/v1/projects/test-project/messages:send" {
			t.Errorf("send path = %s", r.URL.Path)
		}
		gotAuth, gotContentType = r.Header.Get("authorization"), r.Header.Get("content-type")
		_ = json.NewDecoder(r.Body).Decode(&gotBody)
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"name": "projects/test-project/messages/test-id"})
	}))
	defer sendSrv.Close()

	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	s := testFCMSender("test-project", creds)
	s.tokenURL = tokenSrv.URL
	s.sendURL = sendSrv.URL + "/v1/projects/test-project/messages:send"
	s.httpClient = sendSrv.Client()
	msg := PushMessage{DeviceID: uuid.New(), UserID: uuid.New(), Platform: "fcm", Token: "test-device-token", Blob: sealedBlobForTest('A'),
		Title: "Agent done canary", Body: "run finished canary", Data: map[string]string{"kind": "agent_completed", "sender": "desktop-1"}}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("first Send: %v", err)
	}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("second Send: %v", err)
	}
	if tokenHits.Load() != 1 {
		t.Fatalf("token exchange hits = %d, want 1 (cached)", tokenHits.Load())
	}
	if sendHits.Load() != 2 {
		t.Fatalf("send hits = %d, want 2", sendHits.Load())
	}
	if gotAuth != "Bearer test-access-token" {
		t.Fatalf("authorization = %q", gotAuth)
	}
	if !strings.HasPrefix(gotContentType, "application/json") {
		t.Fatalf("content-type = %q", gotContentType)
	}
	message, _ := gotBody["message"].(map[string]any)
	if message["token"] != "test-device-token" {
		t.Fatalf("message token = %v", message)
	}
	// Data-only, high priority: no notification block, so the system never
	// shows server-provided text.
	if _, ok := message["notification"]; ok {
		t.Fatalf("message carries a notification block: %v", message)
	}
	data, _ := message["data"].(map[string]any)
	if data["e"] != sealedBlobForTest('A') {
		t.Fatalf("sealed blob missing from data: %v", data)
	}
	if len(data) != 1 {
		t.Fatalf("data carries extra keys: %v", data)
	}
	android, _ := message["android"].(map[string]any)
	if android["priority"] != "HIGH" {
		t.Fatalf("android priority = %v, want HIGH", android)
	}
	for _, key := range []string{"sender", "desktop", "device", "kind"} {
		if _, ok := message[key]; ok {
			t.Fatalf("message carries %q", key)
		}
	}
	raw, _ := json.Marshal(gotBody)
	for _, canary := range []string{"canary", "desktop-1", "agent_completed"} {
		if strings.Contains(string(raw), canary) {
			t.Fatalf("payload leaks %q: %s", canary, raw)
		}
	}
}

func TestFCMPayloadFixedSizeAndRefusals(t *testing.T) {
	a, err := fcmPayload(PushMessage{Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')})
	if err != nil {
		t.Fatalf("payload A: %v", err)
	}
	// A dismissal-shaped seal is just another blob: same size, same shape.
	b, err := fcmPayload(PushMessage{Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('B')})
	if err != nil {
		t.Fatalf("payload B: %v", err)
	}
	if len(a) != len(b) {
		t.Fatalf("payload sizes differ: %d vs %d", len(a), len(b))
	}
	if len(a) > fcmMaxPayloadBytes {
		t.Fatalf("payload size %d exceeds the FCM cap", len(a))
	}
	for _, bad := range []PushMessage{
		{Platform: "fcm", Token: "test-token", Title: "hi"},
		{Platform: "fcm", Token: "test-token", Blob: strings.Repeat("A", 100)},
		{Platform: "fcm", Token: "test-token", Blob: ""},
	} {
		if _, err := fcmPayload(bad); err == nil {
			t.Fatalf("plaintext/short payload was accepted: %+v", bad.Blob)
		}
	}
}

func TestFCMTokenExpiryRefreshes(t *testing.T) {
	_, pemKey := testFCMKey(t)
	var tokenHits atomic.Int64
	tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		tokenHits.Add(1)
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "test-access-token", "expires_in": 3600, "token_type": "Bearer"})
	}))
	defer tokenSrv.Close()
	sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"name": "projects/test-project/messages/test-id"})
	}))
	defer sendSrv.Close()

	now := time.Unix(1_700_000_000, 0).UTC()
	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	s := testFCMSender("test-project", creds)
	s.tokenURL, s.sendURL, s.httpClient = tokenSrv.URL, sendSrv.URL, sendSrv.Client()
	s.nowFn = func() time.Time { return now }
	msg := PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("first send: %v", err)
	}
	now = now.Add(time.Hour)
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("post-expiry send: %v", err)
	}
	if tokenHits.Load() != 2 {
		t.Fatalf("token hits = %d, want 2 after expiry", tokenHits.Load())
	}
}

func TestFCMFailureClassification(t *testing.T) {
	_, pemKey := testFCMKey(t)
	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	cases := []struct {
		name       string
		status     int
		body       string
		check      func(error) bool
		notInvalid bool
	}{
		{"unregistered detail", 404, `{"error":{"code":404,"message":"Requested entity was not found.","status":"NOT_FOUND","details":[{"@type":"type.googleapis.com/google.firebase.fcm.v1.FcmError","errorCode":"UNREGISTERED"}]}}`, IsInvalidToken, false},
		// A bare 404/NOT_FOUND without the UNREGISTERED detail (e.g. a missing
		// project) must NOT invalidate the registration.
		{"missing project", 404, `{"error":{"code":404,"message":"Project not found.","status":"NOT_FOUND"}}`, nil, true},
		// INVALID_ARGUMENT without UNREGISTERED detail must not delete either;
		// no message-text heuristics are used.
		{"invalid argument no detail", 400, `{"error":{"code":400,"message":"Registration token is invalid.","status":"INVALID_ARGUMENT"}}`, nil, true},
		{"rate limited", 429, `{"error":{"code":429,"message":"Quota exceeded.","status":"RESOURCE_EXHAUSTED"}}`, IsRetryable, false},
		{"server error", 500, `{"error":{"code":500,"message":"Internal.","status":"INTERNAL"}}`, IsRetryable, false},
		{"permission denied", 403, `{"error":{"code":403,"message":"Denied.","status":"PERMISSION_DENIED"}}`, IsAuthError, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("content-type", "application/json")
				_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "test-access-token", "expires_in": 3600, "token_type": "Bearer"})
			}))
			defer tokenSrv.Close()
			sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("content-type", "application/json")
				w.WriteHeader(tc.status)
				_, _ = w.Write([]byte(tc.body))
			}))
			defer sendSrv.Close()
			s := testFCMSender("test-project", creds)
			s.tokenURL, s.sendURL, s.httpClient = tokenSrv.URL, sendSrv.URL, sendSrv.Client()
			err := s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')})
			if err == nil {
				t.Fatal("expected an error")
			}
			if tc.notInvalid {
				if IsInvalidToken(err) {
					t.Fatalf("error %v must not invalidate the registration", err)
				}
				return
			}
			if !tc.check(err) {
				t.Fatalf("error %v failed classification", err)
			}
			if strings.Contains(err.Error(), "test-token") {
				t.Fatalf("error leaks device token: %v", err)
			}
		})
	}
}

func TestFCMContextCancellation(t *testing.T) {
	_, pemKey := testFCMKey(t)
	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	s := testFCMSender("test-project", creds)
	s.tokenURL, s.sendURL = "http://127.0.0.1:1", "http://127.0.0.1:1"
	s.httpClient = sendOnlyClient()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := s.Send(ctx, PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
		t.Fatal("cancelled send succeeded")
	}
}

func TestFCMTokenResponseFailClosed(t *testing.T) {
	_, pemKey := testFCMKey(t)
	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	cases := []struct {
		name string
		body string
	}{
		{"missing token", `{"expires_in":3600,"token_type":"Bearer"}`},
		{"non bearer", `{"access_token":"test-access-token","expires_in":3600,"token_type":"mac"}`},
		{"zero ttl", `{"access_token":"test-access-token","expires_in":0,"token_type":"Bearer"}`},
		{"negative ttl", `{"access_token":"test-access-token","expires_in":-5,"token_type":"Bearer"}`},
		{"overflow ttl", `{"access_token":"test-access-token","expires_in":99999999999,"token_type":"Bearer"}`},
		{"malformed", `not-json`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("content-type", "application/json")
				_, _ = w.Write([]byte(tc.body))
			}))
			defer tokenSrv.Close()
			sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				t.Error("send must not be reached with an invalid token response")
			}))
			defer sendSrv.Close()
			s := testFCMSender("test-project", creds)
			s.tokenURL, s.sendURL, s.httpClient = tokenSrv.URL, sendSrv.URL, sendSrv.Client()
			if err := s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
				t.Fatal("invalid token response was accepted")
			}
		})
	}
}

func TestFCMRedirectNotFollowed(t *testing.T) {
	_, pemKey := testFCMKey(t)
	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	var evilHits atomic.Int64
	evil := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		evilHits.Add(1)
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"name": "projects/test-project/messages/evil"})
	}))
	defer evil.Close()
	tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "test-access-token", "expires_in": 3600, "token_type": "Bearer"})
	}))
	defer tokenSrv.Close()
	sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("location", evil.URL)
		w.WriteHeader(http.StatusFound)
	}))
	defer sendSrv.Close()
	s := testFCMSender("test-project", creds)
	// Default client (nil injection) enforces the fixed-host no-redirect rule.
	s.tokenURL = tokenSrv.URL
	s.sendURL = sendSrv.URL
	if err := s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
		t.Fatal("redirect was accepted")
	}
	if evilHits.Load() != 0 {
		t.Fatal("redirect target was followed")
	}
}

func TestFCMConcurrentCacheMissSingleflight(t *testing.T) {
	_, pemKey := testFCMKey(t)
	var tokenHits atomic.Int64
	tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		tokenHits.Add(1)
		time.Sleep(100 * time.Millisecond)
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "test-access-token", "expires_in": 3600, "token_type": "Bearer"})
	}))
	defer tokenSrv.Close()
	sendSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"name": "projects/test-project/messages/test-id"})
	}))
	defer sendSrv.Close()
	creds := testFCMAccountJSON("test-project", "test@test-project.iam.gserviceaccount.com", "test-key-id", pemKey, "")
	s := testFCMSender("test-project", creds)
	s.tokenURL, s.sendURL, s.httpClient = tokenSrv.URL, sendSrv.URL, sendSrv.Client()
	const workers = 8
	errs := make([]error, workers)
	var wg sync.WaitGroup
	for i := range errs {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			errs[i] = s.Send(context.Background(), PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')})
		}(i)
	}
	wg.Wait()
	for _, err := range errs {
		if err != nil {
			t.Fatalf("concurrent send: %v", err)
		}
	}
	if tokenHits.Load() != 1 {
		t.Fatalf("token exchanges = %d, want 1 (singleflight)", tokenHits.Load())
	}
}

func sendOnlyClient() *http.Client { return &http.Client{} }
