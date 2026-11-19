package types

import (
	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

type MagicRequest struct {
	Email          string `json:"email"`
	State          string `json:"state"`
	FirstName      string `json:"first_name"`
	LastName       string `json:"last_name"`
	IsRegistration bool   `json:"is_registration"`
}

type AuthPayload struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	UserID       string `json:"user_id"`
}

type TokenClaims struct {
	UserID   uuid.UUID `json:"user_id"`
	DeviceID uuid.UUID `json:"device_id"`
	// Scope marks tokens minted for a narrow purpose (e.g. "companion"
	// for paired phones). Empty on portal login tokens; verifiers that do
	// not understand a scope ignore it, so the shape stays compatible.
	Scope string `json:"scope,omitempty"`
	jwt.RegisteredClaims
}
