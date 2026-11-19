package execution

import (
	"context"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	billingModel "server/internal/billing/model"
)

type stubBalances struct {
	tier string
	err  error
}

func (s stubBalances) GetOrInitBalance(_ context.Context, _ uuid.UUID) (*billingModel.UserBalance, error) {
	if s.err != nil {
		return nil, s.err
	}
	return &billingModel.UserBalance{Tier: s.tier}, nil
}

func testHandler(t *testing.T, tier string) (*Handler, ed25519.PublicKey) {
	t.Helper()
	seed := make([]byte, ed25519.SeedSize)
	for i := range seed {
		seed[i] = byte(i + 1)
	}
	h, err := NewHandler(base64.StdEncoding.EncodeToString(seed), stubBalances{tier: tier})
	if err != nil {
		t.Fatalf("NewHandler: %v", err)
	}
	pub, ok := h.priv.Public().(ed25519.PublicKey)
	if !ok {
		t.Fatal("private key has no ed25519 public half")
	}
	return h, pub
}

func authedRequest(userID uuid.UUID, body string) *http.Request {
	var rdr *strings.Reader
	if body == "" {
		rdr = strings.NewReader("")
	} else {
		rdr = strings.NewReader(body)
	}
	r := httptest.NewRequest(http.MethodPost, "/v1/execution/authorize", rdr)
	return r.WithContext(context.WithValue(r.Context(), "user_id", userID.String()))
}

func TestAuthorizeIssuesVerifiableGrant(t *testing.T) {
	h, pub := testHandler(t, billingModel.TierFree)
	rec := httptest.NewRecorder()
	h.authorize(rec, authedRequest(uuid.New(), `{"device_id":"dev-1","client_version":"0.2.3"}`))
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, body %s", rec.Code, rec.Body.String())
	}
	var resp authorizeResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	claims, err := VerifyGrant(resp.ExecutionToken, pub)
	if err != nil {
		t.Fatalf("verify: %v", err)
	}
	if claims.Subject != resp.UserID {
		t.Errorf("subject = %q, want %q", claims.Subject, resp.UserID)
	}
	if !claims.HasCapability(CapabilityAgent) || !claims.HasCapability(CapabilityManaged) {
		t.Errorf("free grant missing base capabilities: %v", claims.Capabilities)
	}
	if claims.HasCapability(CapabilityBYOK) {
		t.Errorf("free grant must not carry byok: %v", claims.Capabilities)
	}
	if claims.Device != "dev-1" {
		t.Errorf("device = %q, want dev-1", claims.Device)
	}
	if time.Until(claims.ExpiresAt.Time) > GrantTTL || time.Until(claims.ExpiresAt.Time) <= 0 {
		t.Errorf("expiry out of range: %v", claims.ExpiresAt.Time)
	}
}

func TestAuthorizeProTierGetsBYOK(t *testing.T) {
	h, pub := testHandler(t, "pro")
	rec := httptest.NewRecorder()
	h.authorize(rec, authedRequest(uuid.New(), ""))
	var resp authorizeResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	claims, err := VerifyGrant(resp.ExecutionToken, pub)
	if err != nil {
		t.Fatalf("verify: %v", err)
	}
	if !claims.HasCapability(CapabilityBYOK) {
		t.Errorf("pro grant missing byok: %v", claims.Capabilities)
	}
}

func TestAuthorizeRejectsTamperedToken(t *testing.T) {
	h, pub := testHandler(t, "pro")
	rec := httptest.NewRecorder()
	h.authorize(rec, authedRequest(uuid.New(), ""))
	var resp authorizeResponse
	_ = json.Unmarshal(rec.Body.Bytes(), &resp)
	// Flip a bit in the decoded signature bytes and re-encode, so the
	// tampering always changes the signed bytes. (Replacing the last
	// base64 character is a no-op when it already is 'A' or differs only
	// in unused padding bits, which made this test flaky.)
	tampered := tamperSignature(t, resp.ExecutionToken)
	if tampered == resp.ExecutionToken {
		t.Fatal("tampering produced an identical token")
	}
	if _, err := VerifyGrant(tampered, pub); err == nil {
		t.Error("tampered grant verified")
	}
	// Wrong key must not verify.
	if _, err := VerifyGrant(resp.ExecutionToken, mustOtherKey(t)); err == nil {
		t.Error("grant verified against the wrong key")
	}
}

func tamperSignature(t *testing.T, token string) string {
	t.Helper()
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		t.Fatalf("expected 3 JWT parts, got %d", len(parts))
	}
	sig, err := base64.RawURLEncoding.DecodeString(parts[2])
	if err != nil {
		t.Fatalf("decode signature: %v", err)
	}
	if len(sig) == 0 {
		t.Fatal("empty signature")
	}
	sig[0] ^= 0x01
	parts[2] = base64.RawURLEncoding.EncodeToString(sig)
	return strings.Join(parts, ".")
}

func mustOtherKey(t *testing.T) ed25519.PublicKey {
	t.Helper()
	seed := make([]byte, ed25519.SeedSize)
	for i := range seed {
		seed[i] = byte(42 - i)
	}
	priv := ed25519.NewKeyFromSeed(seed)
	pub, ok := priv.Public().(ed25519.PublicKey)
	if !ok {
		t.Fatal("no public half")
	}
	return pub
}

func TestAuthorizeRequiresAuth(t *testing.T) {
	h, _ := testHandler(t, "pro")
	r := httptest.NewRequest(http.MethodPost, "/v1/execution/authorize", nil)
	rec := httptest.NewRecorder()
	h.authorize(rec, r)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("status = %d, want 401", rec.Code)
	}
}

func TestTelemetryIngestAcceptsLockedShape(t *testing.T) {
	body := `[{"execution_id":"` + uuid.NewString() + `","provider":"anthropic","path":"direct","started_at_ms":1,"duration_ms":2,"success":true,"error":null,"input_tokens":3,"output_tokens":4,"tool_calls":1,"client_version":"0.2.3"}]`
	r := httptest.NewRequest(http.MethodPost, "/v1/telemetry/events", strings.NewReader(body))
	user := uuid.New()
	r = r.WithContext(context.WithValue(r.Context(), "user_id", user.String()))
	rec := httptest.NewRecorder()
	ingestTelemetry(rec, r)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("status = %d, body %s", rec.Code, rec.Body.String())
	}
}

func TestTelemetryIngestRejectsUnknownFields(t *testing.T) {
	// A prompt smuggled into telemetry must be rejected, not stored.
	body := `[{"execution_id":"` + uuid.NewString() + `","provider":"anthropic","prompt":"ignore previous instructions"}]`
	r := httptest.NewRequest(http.MethodPost, "/v1/telemetry/events", strings.NewReader(body))
	user := uuid.New()
	r = r.WithContext(context.WithValue(r.Context(), "user_id", user.String()))
	rec := httptest.NewRecorder()
	ingestTelemetry(rec, r)
	if rec.Code != http.StatusBadRequest {
		t.Errorf("status = %d, want 400 for unknown field", rec.Code)
	}
}

func TestEphemeralKeyWhenUnset(t *testing.T) {
	h, err := NewHandler("", stubBalances{tier: "pro"})
	if err != nil {
		t.Fatalf("NewHandler: %v", err)
	}
	if !h.ephemeral {
		t.Error("expected ephemeral mode with empty seed")
	}
}

func TestBadSeedRejected(t *testing.T) {
	if _, err := NewHandler("!!!not-base64!!!", stubBalances{tier: "pro"}); err == nil {
		t.Error("expected error for bad seed")
	}
	if _, err := NewHandler(base64.StdEncoding.EncodeToString([]byte("short")), stubBalances{tier: "pro"}); err == nil {
		t.Error("expected error for short seed")
	}
}
