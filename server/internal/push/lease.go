package push

import (
	"context"
	"errors"
	"net/http"
	"strings"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

// ErrUnauthenticated is returned when a registration call carries no usable
// access token. Handlers map it to 401.
var ErrUnauthenticated = errors.New("push: authentication required")

// Lease is the authenticated identity a registration call acts under: the
// user plus the exact device the access JWT was issued to.
type Lease struct {
	UserID   uuid.UUID
	DeviceID uuid.UUID
}

// Claims mirrors the access-JWT claims the identity service signs
// (server/internal/identity/types.TokenClaims): same JSON names, same HMAC.
// Keeping the shape here (rather than importing the identity package) keeps
// push decoupled from login flows while staying wire-compatible.
type Claims struct {
	UserID   uuid.UUID `json:"user_id"`
	DeviceID uuid.UUID `json:"device_id"`
	jwt.RegisteredClaims
}

// Authenticator verifies a request's bearer access token and returns its
// lease. Registration handlers reject whatever it rejects.
type Authenticator interface {
	Authenticate(r *http.Request) (Lease, error)
}

// JWTAuthenticator verifies HMAC-signed access tokens with the API's JWT
// secret — the same secret identity's TokenService signs with.
type JWTAuthenticator struct {
	secret []byte
}

// NewJWTAuthenticator builds an authenticator for the given JWT secret.
func NewJWTAuthenticator(secret string) *JWTAuthenticator {
	return &JWTAuthenticator{secret: []byte(secret)}
}

// VerifyToken parses and validates a raw access-token string.
func (a *JWTAuthenticator) VerifyToken(tokenString string) (Lease, error) {
	tokenString = strings.TrimSpace(tokenString)
	if tokenString == "" {
		return Lease{}, ErrUnauthenticated
	}
	token, err := jwt.ParseWithClaims(tokenString, &Claims{}, func(token *jwt.Token) (any, error) {
		if _, ok := token.Method.(*jwt.SigningMethodHMAC); !ok {
			return nil, errors.New("push: unexpected signing method")
		}
		return a.secret, nil
	})
	if err != nil {
		return Lease{}, ErrUnauthenticated
	}
	claims, ok := token.Claims.(*Claims)
	if !ok || !token.Valid {
		return Lease{}, ErrUnauthenticated
	}
	// The device claim is load-bearing for push: without it the server cannot
	// tell which device is registering, so a token without one is as useless
	// as no token. (The global AuthMiddleware currently discards this claim;
	// registration calls verify the JWT themselves for exactly this reason.)
	if claims.UserID == uuid.Nil || claims.DeviceID == uuid.Nil {
		return Lease{}, ErrUnauthenticated
	}
	return Lease{UserID: claims.UserID, DeviceID: claims.DeviceID}, nil
}

// Authenticate extracts the bearer access token from the Authorization
// header and verifies it. Query parameters are never accepted: access
// tokens in URLs would leak into logs and history (H2, §2.3). Callers that
// used ?access_token get 401 and must send the header instead.
func (a *JWTAuthenticator) Authenticate(r *http.Request) (Lease, error) {
	token := ""
	if h := r.Header.Get("Authorization"); h != "" {
		parts := strings.SplitN(h, " ", 2)
		if len(parts) == 2 && strings.EqualFold(parts[0], "bearer") {
			token = parts[1]
		}
	}
	return a.VerifyToken(token)
}

// LeaseFromContext reads a lease previously injected into the request
// context under the conventional "user_id"/"device_id" keys. It exists for
// the day the global AuthMiddleware propagates the device claim; handlers
// below verify the JWT directly and do not depend on it.
func LeaseFromContext(ctx context.Context) (Lease, bool) {
	uidStr, _ := ctx.Value("user_id").(string)
	devStr, _ := ctx.Value("device_id").(string)
	uid, err := uuid.Parse(uidStr)
	if err != nil || uid == uuid.Nil {
		return Lease{}, false
	}
	dev, err := uuid.Parse(devStr)
	if err != nil || dev == uuid.Nil {
		return Lease{}, false
	}
	return Lease{UserID: uid, DeviceID: dev}, true
}

var _ Authenticator = (*JWTAuthenticator)(nil)
