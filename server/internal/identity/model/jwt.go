package model

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"log/slog"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
	"server/internal/identity/types"
)

type TokenService struct {
	secret []byte
	// signedOut reports a device whose session was ended (logout). Nil in
	// tests and tools that don't sign anyone out.
	signedOut func(ctx context.Context, deviceID string) (bool, error)
}

// ErrSignedOut is a valid token from a device that has since signed out.
var ErrSignedOut = errors.New("session signed out")

// SetSignedOutCheck makes every verification refuse tokens from devices that
// signed out, so logout takes effect everywhere a token is checked (API,
// relay, pairing), not only where the refresh token is used.
func (s *TokenService) SetSignedOutCheck(check func(ctx context.Context, deviceID string) (bool, error)) {
	s.signedOut = check
}

func NewTokenService(secret string) *TokenService {
	return &TokenService{secret: []byte(secret)}
}

func (s *TokenService) GenerateAccessToken(userID, deviceID uuid.UUID) (string, error) {
	return s.GenerateScopedAccessToken(userID, deviceID, "")
}

// GenerateScopedAccessToken mints the same relay-accepted access token shape
// with an additional scope claim (e.g. "companion" for paired phones).
// Verifiers that do not understand the scope ignore it.
func (s *TokenService) GenerateScopedAccessToken(userID, deviceID uuid.UUID, scope string) (string, error) {
	claims := types.TokenClaims{
		UserID:   userID,
		DeviceID: deviceID,
		Scope:    scope,
		RegisteredClaims: jwt.RegisteredClaims{
			ExpiresAt: jwt.NewNumericDate(time.Now().Add(24 * time.Hour)),
			IssuedAt:  jwt.NewNumericDate(time.Now()),
			NotBefore: jwt.NewNumericDate(time.Now()),
			Issuer:    "dragent",
		},
	}

	token := jwt.NewWithClaims(jwt.SigningMethodHS256, claims)
	return token.SignedString(s.secret)
}

func (s *TokenService) GenerateSecureRandomToken() (string, error) {
	bytes := make([]byte, 32)
	if _, err := rand.Read(bytes); err != nil {
		return "", fmt.Errorf("failed to generate random token: %w", err)
	}
	return hex.EncodeToString(bytes), nil
}

func (s *TokenService) VerifyAccessToken(tokenString string) (*types.TokenClaims, error) {
	token, err := jwt.ParseWithClaims(tokenString, &types.TokenClaims{}, func(token *jwt.Token) (interface{}, error) {
		if _, ok := token.Method.(*jwt.SigningMethodHMAC); !ok {
			return nil, fmt.Errorf("unexpected signing method: %v", token.Header["alg"])
		}
		return s.secret, nil
	})

	if err != nil {
		return nil, err
	}

	claims, ok := token.Claims.(*types.TokenClaims)
	if !ok || !token.Valid {
		return nil, errors.New("invalid token claims")
	}

	if s.signedOut != nil && claims.DeviceID != uuid.Nil {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		out, err := s.signedOut(ctx, claims.DeviceID.String())
		if err != nil {
			// Redis down: let the token through rather than signing every
			// user out at once. A signed-out device's refresh token is
			// revoked in the database either way.
			slog.Warn("signed-out check unavailable", "error", err)
		} else if out {
			return nil, ErrSignedOut
		}
	}

	return claims, nil
}
