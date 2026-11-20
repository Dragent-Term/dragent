package components

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"time"

	"github.com/google/uuid"
	"server/internal/identity/model"
)

// A sign-in hands out two sessions, each its own device: one for the
// browser it happened in, and one parked for the app that's waiting for it
// (collected by deep link, or by polling once approved). Separate devices so
// signing out of one doesn't sign out the other.
const desktopDeviceName = "Dragent app"

// registerDevice records a signed-in device.
func registerDevice(ctx context.Context, repo model.UserRepository, userID uuid.UUID, name string) (uuid.UUID, error) {
	id := uuid.New()
	now := time.Now()
	if err := repo.RegisterDevice(ctx, &model.Device{
		ID:         id,
		UserID:     userID,
		Name:       name,
		Platform:   "WEB",
		LastSeenAt: now,
		CreatedAt:  now,
	}); err != nil {
		return uuid.Nil, fmt.Errorf("failed to register device: %w", err)
	}
	return id, nil
}

// issueRefreshToken stores a new refresh token family for a device and
// returns the raw token.
func issueRefreshToken(ctx context.Context, repo model.UserRepository, tokens *model.TokenService, userID, deviceID uuid.UUID) (string, error) {
	raw, err := tokens.GenerateSecureRandomToken()
	if err != nil {
		return "", err
	}
	hash := sha256.Sum256([]byte(raw))
	now := time.Now()
	if err := repo.SaveRefreshToken(ctx, &model.RefreshToken{
		ID:        uuid.New(),
		UserID:    userID,
		DeviceID:  deviceID,
		TokenHash: hex.EncodeToString(hash[:]),
		FamilyID:  uuid.New(),
		ExpiresAt: now.Add(30 * 24 * time.Hour),
		CreatedAt: now,
	}); err != nil {
		return "", fmt.Errorf("failed to store refresh token: %w", err)
	}
	return raw, nil
}

// signInSessions are the two sessions one sign-in produces.
type signInSessions struct {
	browserDevice  uuid.UUID
	desktopDevice  uuid.UUID
	desktopRefresh string
}

func issueSignInSessions(ctx context.Context, repo model.UserRepository, tokens *model.TokenService, userID uuid.UUID, browserName string) (signInSessions, error) {
	var s signInSessions
	var err error
	if s.browserDevice, err = registerDevice(ctx, repo, userID, browserName); err != nil {
		return s, err
	}
	if s.desktopDevice, err = registerDevice(ctx, repo, userID, desktopDeviceName); err != nil {
		return s, err
	}
	s.desktopRefresh, err = issueRefreshToken(ctx, repo, tokens, userID, s.desktopDevice)
	return s, err
}
