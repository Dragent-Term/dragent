package push

import (
	"context"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

const testJWTSecret = "test-push-jwt-secret-not-a-real-credential"

// mintToken signs an access token with the same claims shape the identity
// service uses. Tokens here are test-only fixtures, never real credentials.
func mintToken(t *testing.T, secret string, userID, deviceID uuid.UUID, ttl time.Duration) string {
	t.Helper()
	now := time.Now()
	claims := Claims{
		UserID:   userID,
		DeviceID: deviceID,
		RegisteredClaims: jwt.RegisteredClaims{
			ExpiresAt: jwt.NewNumericDate(now.Add(ttl)),
			IssuedAt:  jwt.NewNumericDate(now),
			NotBefore: jwt.NewNumericDate(now),
			Issuer:    "dragent-test",
		},
	}
	token, err := jwt.NewWithClaims(jwt.SigningMethodHS256, claims).SignedString([]byte(secret))
	if err != nil {
		t.Fatalf("sign test token: %v", err)
	}
	return token
}

func TestVerifyTokenValid(t *testing.T) {
	a := NewJWTAuthenticator(testJWTSecret)
	userID, deviceID := uuid.New(), uuid.New()
	lease, err := a.VerifyToken(mintToken(t, testJWTSecret, userID, deviceID, time.Hour))
	if err != nil {
		t.Fatalf("VerifyToken: %v", err)
	}
	if lease.UserID != userID || lease.DeviceID != deviceID {
		t.Fatalf("lease = %+v, want user %v device %v", lease, userID, deviceID)
	}
}

func TestVerifyTokenRejections(t *testing.T) {
	a := NewJWTAuthenticator(testJWTSecret)
	userID, deviceID := uuid.New(), uuid.New()
	valid := mintToken(t, testJWTSecret, userID, deviceID, time.Hour)

	cases := []struct {
		name  string
		token string
	}{
		{"empty", ""},
		{"blank", "   "},
		{"garbage", "not-a-jwt"},
		{"wrong secret", mintToken(t, "another-test-secret", userID, deviceID, time.Hour)},
		{"expired", mintToken(t, testJWTSecret, userID, deviceID, -time.Hour)},
		{"missing device claim", mintToken(t, testJWTSecret, userID, uuid.Nil, time.Hour)},
		{"missing user claim", mintToken(t, testJWTSecret, uuid.Nil, deviceID, time.Hour)},
		{"tampered", valid[:len(valid)-2] + "xx"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := a.VerifyToken(tc.token); err == nil {
				t.Fatal("expected rejection")
			}
		})
	}
}

func TestVerifyTokenRejectsNoneAlgorithm(t *testing.T) {
	a := NewJWTAuthenticator(testJWTSecret)
	claims := Claims{UserID: uuid.New(), DeviceID: uuid.New()}
	unsigned, err := jwt.NewWithClaims(jwt.SigningMethodNone, claims).SignedString(jwt.UnsafeAllowNoneSignatureType)
	if err != nil {
		t.Fatalf("sign none token: %v", err)
	}
	if _, err := a.VerifyToken(unsigned); err == nil {
		t.Fatal("unsigned token was accepted")
	}
}

func TestAuthenticateHeaderOnly(t *testing.T) {
	a := NewJWTAuthenticator(testJWTSecret)
	userID, deviceID := uuid.New(), uuid.New()
	token := mintToken(t, testJWTSecret, userID, deviceID, time.Hour)

	req := httptest.NewRequest("POST", "/v1/push/register", nil)
	req.Header.Set("Authorization", "Bearer "+token)
	lease, err := a.Authenticate(req)
	if err != nil {
		t.Fatalf("header Authenticate: %v", err)
	}
	if lease.DeviceID != deviceID {
		t.Fatalf("lease device = %v, want %v", lease.DeviceID, deviceID)
	}

	// Query-param tokens are refused (H2, §2.3): tokens never travel in URLs.
	req = httptest.NewRequest("POST", "/v1/push/register?access_token="+token, nil)
	if _, err := a.Authenticate(req); err == nil {
		t.Fatal("query-param token was accepted")
	}

	req = httptest.NewRequest("POST", "/v1/push/register", nil)
	if _, err := a.Authenticate(req); err == nil {
		t.Fatal("unauthenticated request was accepted")
	}
}

func TestLeaseFromContext(t *testing.T) {
	userID, deviceID := uuid.New(), uuid.New()
	ctx := context.WithValue(context.Background(), "user_id", userID.String())
	ctx = context.WithValue(ctx, "device_id", deviceID.String())
	lease, ok := LeaseFromContext(ctx)
	if !ok {
		t.Fatal("expected lease from context")
	}
	if lease.UserID != userID || lease.DeviceID != deviceID {
		t.Fatalf("lease = %+v", lease)
	}
	if _, ok := LeaseFromContext(context.Background()); ok {
		t.Fatal("empty context produced a lease")
	}
}
