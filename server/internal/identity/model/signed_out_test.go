package model

import (
	"context"
	"errors"
	"testing"

	"github.com/google/uuid"
)

// After logout a device's access tokens are refused, though unexpired;
// other devices' tokens still work.
func TestSignedOutDeviceTokensAreRefused(t *testing.T) {
	svc := NewTokenService("test-secret-test-secret-test-secret")
	out := uuid.New()
	svc.SetSignedOutCheck(func(_ context.Context, deviceID string) (bool, error) {
		return deviceID == out.String(), nil
	})

	signedOut, _ := svc.GenerateAccessToken(uuid.New(), out)
	if _, err := svc.VerifyAccessToken(signedOut); !errors.Is(err, ErrSignedOut) {
		t.Fatalf("signed-out device: got %v, want ErrSignedOut", err)
	}
	other, _ := svc.GenerateAccessToken(uuid.New(), uuid.New())
	if _, err := svc.VerifyAccessToken(other); err != nil {
		t.Fatalf("other device: %v", err)
	}
}
