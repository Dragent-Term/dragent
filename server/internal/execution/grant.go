// Package execution implements the control-plane authority for local
// execution: short-lived signed execution grants (M3).
//
// Model: the cloud answers "is this user allowed to do this?" by issuing a
// JWT (EdDSA/Ed25519) the local engine verifies OFFLINE against a cached
// public key. No database access from local processes, ever; no per-turn
// authorization round trips. Revocation is via short TTL (24h), not live
// checks — a bounded authorization window in exchange for local
// availability during cloud outages.
package execution

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

// Capability strings carried in grants. Clients enforce membership per
// path (direct needs "byok", cloud needs "managed"); everything needs
// "agent".
const (
	CapabilityAgent   = "agent"
	CapabilityBYOK    = "byok"
	CapabilityManaged = "managed"
)

// GrantTTL bounds the authorization window (revocation strategy).
const GrantTTL = 24 * time.Hour

// GrantClaims is the signed authorization object.
type GrantClaims struct {
	jwt.RegisteredClaims
	// Plan name at issuance (e.g. "free", "pro") — informational; the
	// capabilities below are what clients enforce.
	Plan string `json:"plan"`
	// Capabilities granted (subset of agent/byok/managed).
	Capabilities []string `json:"capabilities"`
	// Device that requested the grant (client-supplied, informational
	// binding; clients enforce equality when they have a stable id).
	Device string `json:"device,omitempty"`
	// Unique grant id for idempotency/debugging.
	GrantID string `json:"grant_id"`
}

// HasCapability reports whether the grant carries a capability.
func (c *GrantClaims) HasCapability(cap string) bool {
	for _, got := range c.Capabilities {
		if got == cap {
			return true
		}
	}
	return false
}

// LoadSigningKey decodes a base64 32-byte Ed25519 seed. An empty seed
// generates an EPHEMERAL key (dev convenience): grants die with the
// process restart, and the handler advertises this. Production must set
// EXECUTION_GRANT_KEY.
func LoadSigningKey(seedB64 string) (priv ed25519.PrivateKey, ephemeral bool, err error) {
	if seedB64 == "" {
		_, priv, err = ed25519.GenerateKey(rand.Reader)
		if err != nil {
			return nil, false, fmt.Errorf("generate ephemeral grant key: %w", err)
		}
		return priv, true, nil
	}
	seed, err := base64.StdEncoding.DecodeString(seedB64)
	if err != nil {
		seed, err = base64.URLEncoding.DecodeString(seedB64)
		if err != nil {
			return nil, false, fmt.Errorf("decode grant key: %w", err)
		}
	}
	if len(seed) != ed25519.SeedSize {
		return nil, false, fmt.Errorf("grant key must be %d bytes, got %d", ed25519.SeedSize, len(seed))
	}
	return ed25519.NewKeyFromSeed(seed), false, nil
}

// PublicKeyB64 returns the base64-encoded Ed25519 public key for clients.
func PublicKeyB64(priv ed25519.PrivateKey) string {
	pub, ok := priv.Public().(ed25519.PublicKey)
	if !ok {
		return ""
	}
	return base64.StdEncoding.EncodeToString(pub)
}

// IssueGrant signs a grant for userID with the given plan/capabilities.
func IssueGrant(priv ed25519.PrivateKey, userID uuid.UUID, plan string, capabilities []string, device string) (string, *GrantClaims, error) {
	now := time.Now()
	claims := &GrantClaims{
		RegisteredClaims: jwt.RegisteredClaims{
			Subject:   userID.String(),
			ExpiresAt: jwt.NewNumericDate(now.Add(GrantTTL)),
			IssuedAt:  jwt.NewNumericDate(now),
			NotBefore: jwt.NewNumericDate(now),
			ID:        uuid.NewString(),
		},
		Plan:         plan,
		Capabilities: append([]string(nil), capabilities...),
		Device:       device,
		GrantID:      uuid.NewString(),
	}
	token := jwt.NewWithClaims(jwt.SigningMethodEdDSA, claims)
	signed, err := token.SignedString(priv)
	if err != nil {
		return "", nil, fmt.Errorf("sign grant: %w", err)
	}
	return signed, claims, nil
}

// VerifyGrant parses and verifies a grant token against a public key.
// Shared logic with clients (which reimplement it locally); covered by test.
func VerifyGrant(tokenString string, pub ed25519.PublicKey) (*GrantClaims, error) {
	claims := &GrantClaims{}
	token, err := jwt.ParseWithClaims(tokenString, claims, func(token *jwt.Token) (interface{}, error) {
		if _, ok := token.Method.(*jwt.SigningMethodEd25519); !ok {
			return nil, fmt.Errorf("unexpected signing method %v", token.Header["alg"])
		}
		return pub, nil
	})
	if err != nil {
		return nil, err
	}
	if !token.Valid {
		return nil, fmt.Errorf("invalid grant")
	}
	return claims, nil
}
