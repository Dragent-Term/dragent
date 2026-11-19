package model

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
	"server/internal/identity/types"
)

type TokenService struct {
	secret []byte
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

	return claims, nil
}
