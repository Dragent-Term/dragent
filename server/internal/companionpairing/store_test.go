package companionpairing

import (
	"context"
	"errors"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
)

var testNow = time.Date(2026, time.November, 3, 10, 0, 0, 0, time.UTC)

func testStore(now *time.Time) *MemoryStore {
	return NewMemoryStore(func() time.Time { return *now })
}

func mustPair(t *testing.T, s *MemoryStore, user, org string) (Pairing, string) {
	t.Helper()
	p, code, err := s.CreatePairing(context.Background(), user, org)
	if err != nil {
		t.Fatal(err)
	}
	if len(code) != CodeLength {
		t.Fatalf("code length %d, want %d", len(code), CodeLength)
	}
	if p.ExpiresAt.Sub(p.CreatedAt) != PairingTTL {
		t.Fatalf("pairing TTL %v, want %v", p.ExpiresAt.Sub(p.CreatedAt), PairingTTL)
	}
	return p, code
}

func TestCreateBindsUserAndOrg(t *testing.T) {
	now := testNow
	s := testStore(&now)
	p, code, err := s.CreatePairing(context.Background(), "user-a", "org-a")
	if err != nil {
		t.Fatal(err)
	}
	if p.UserID != "user-a" || p.OrgID != "org-a" {
		t.Fatalf("pairing not bound: %+v", p)
	}
	if !validDeviceID(p.ID) {
		t.Fatalf("pairing id is not a UUID: %q", p.ID)
	}
	if _, ok := normalizeCode(code); !ok {
		t.Fatalf("code fails validation: %q", code)
	}
	if _, _, err := s.CreatePairing(context.Background(), "", "org-a"); !errors.Is(err, ErrInvalid) {
		t.Fatalf("empty user accepted: %v", err)
	}
}

func TestMaxLivePairingsDropsOldest(t *testing.T) {
	now := testNow
	s := testStore(&now)
	var firstCode string
	for i := 0; i < MaxLivePairingsPerUser+1; i++ {
		now = now.Add(time.Second)
		_, code, err := s.CreatePairing(context.Background(), "user-a", "org-a")
		if err != nil {
			t.Fatal(err)
		}
		if i == 0 {
			firstCode = code
		}
		if i == MaxLivePairingsPerUser {
			if _, _, _, _, err := s.RedeemPairing(context.Background(), firstCode, "ios"); !errors.Is(err, ErrGone) {
				t.Fatalf("dropped oldest redeems: %v", err)
			}
		}
	}
	// Other users are unaffected by the cap.
	if _, _, err := s.CreatePairing(context.Background(), "user-b", "org-a"); err != nil {
		t.Fatal(err)
	}
}

func TestRedeemSingleUseRace(t *testing.T) {
	now := testNow
	s := testStore(&now)
	_, code := mustPair(t, s, "user-a", "org-a")
	const racers = 16
	var won atomic.Int64
	var gone atomic.Int64
	var wg sync.WaitGroup
	for i := 0; i < racers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, _, _, _, err := s.RedeemPairing(context.Background(), code, "android")
			switch {
			case err == nil:
				won.Add(1)
			case errors.Is(err, ErrGone):
				gone.Add(1)
			default:
				t.Errorf("unexpected redeem error: %v", err)
			}
		}()
	}
	wg.Wait()
	if won.Load() != 1 || gone.Load() != racers-1 {
		t.Fatalf("race: %d won, %d gone (want 1/%d)", won.Load(), gone.Load(), racers-1)
	}
}

func TestRedeemExpiryAndTombstone(t *testing.T) {
	now := testNow
	s := testStore(&now)
	_, code := mustPair(t, s, "user-a", "org-a")
	now = testNow.Add(PairingTTL).Add(time.Second)
	if _, _, _, _, err := s.RedeemPairing(context.Background(), code, "ios"); !errors.Is(err, ErrGone) {
		t.Fatalf("expired redeem: %v", err)
	}
	// Consumed codes read as gone while the tombstone lives.
	now = testNow
	_, code2 := mustPair(t, s, "user-a", "org-a")
	redeemedAt := now
	if _, _, _, _, err := s.RedeemPairing(context.Background(), code2, "ios"); err != nil {
		t.Fatal(err)
	}
	now = redeemedAt.Add(TombstoneTTL).Add(-time.Minute)
	if _, _, _, _, err := s.RedeemPairing(context.Background(), code2, "ios"); !errors.Is(err, ErrGone) {
		t.Fatalf("consumed redeem inside tombstone window: %v", err)
	}
	now = redeemedAt.Add(TombstoneTTL).Add(2 * time.Minute)
	if _, _, _, _, err := s.RedeemPairing(context.Background(), code2, "ios"); !errors.Is(err, ErrInvalid) {
		t.Fatalf("ancient code should be invalid: %v", err)
	}
}

func TestRedeemValidation(t *testing.T) {
	now := testNow
	s := testStore(&now)
	_, code := mustPair(t, s, "user-a", "org-a")
	for _, input := range []struct {
		name     string
		code     string
		platform string
	}{
		{"short code", "ABC", "ios"},
		{"bad charset", strings.Repeat("0", CodeLength), "ios"},
		{"bad platform", code, "windows"},
		{"web platform removed", code, "web"},
		{"empty platform", code, ""},
	} {
		if _, _, _, _, err := s.RedeemPairing(context.Background(), input.code, input.platform); !errors.Is(err, ErrInvalid) {
			t.Fatalf("%s accepted: %v", input.name, err)
		}
	}
	// Lowercase codes and platform case are normalized, not rejected.
	// No device name is stored: names live in the encrypted HS1 payload.
	device, _, _, token, err := s.RedeemPairing(context.Background(), strings.ToLower(code), "IOS")
	if err != nil {
		t.Fatalf("normalized redeem rejected: %v", err)
	}
	if device.Platform != "ios" || device.Name != "" || !device.E2E {
		t.Fatalf("redeem not normalized: %+v", device)
	}
	if len(token) != 64 {
		t.Fatalf("device token length %d, want 64 hex chars", len(token))
	}
}

func TestRotationRenewsAndReuseRevokes(t *testing.T) {
	now := testNow
	s := testStore(&now)
	_, code := mustPair(t, s, "user-a", "org-a")
	device, _, _, token, err := s.RedeemPairing(context.Background(), code, "ios")
	if err != nil {
		t.Fatal(err)
	}
	if device.ExpiresAt.Sub(device.CreatedAt) != DeviceLifetime {
		t.Fatalf("device lifetime %v, want %v", device.ExpiresAt.Sub(device.CreatedAt), DeviceLifetime)
	}
	now = now.Add(time.Hour)
	s.now = func() time.Time { return now }
	next, rotated, err := s.RotateDeviceToken(context.Background(), device.ID, token)
	if err != nil {
		t.Fatal(err)
	}
	if next == token {
		t.Fatal("rotation did not change the token")
	}
	if !rotated.LastSeenAt.Equal(now) {
		t.Fatalf("last_seen not updated: %v", rotated.LastSeenAt)
	}
	if rotated.ExpiresAt.Sub(now) != DeviceLifetime {
		t.Fatal("rotation did not renew the lifetime")
	}
	// Reusing the rotated-out token revokes the device.
	if _, _, err := s.RotateDeviceToken(context.Background(), device.ID, token); !errors.Is(err, ErrRevoked) {
		t.Fatalf("reuse did not revoke: %v", err)
	}
	// The device is gone: even the fresh token fails, and the id is invalid.
	if _, _, err := s.RotateDeviceToken(context.Background(), device.ID, next); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("revoked device still rotates: %v", err)
	}
	if valid, err := s.IsDeviceValid(context.Background(), "user-a", device.ID); err != nil || valid {
		t.Fatalf("revoked device still valid: %v %v", valid, err)
	}
	if _, _, err := s.RotateDeviceToken(context.Background(), device.ID, "wrong"); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("wrong token: %v", err)
	}
}

func TestRevokeAndList(t *testing.T) {
	now := testNow
	s := testStore(&now)
	_, codeA := mustPair(t, s, "user-a", "org-a")
	deviceA, _, _, _, err := s.RedeemPairing(context.Background(), codeA, "ios")
	if err != nil {
		t.Fatal(err)
	}
	_, codeB := mustPair(t, s, "user-a", "org-a")
	deviceB, _, _, _, err := s.RedeemPairing(context.Background(), codeB, "android")
	if err != nil {
		t.Fatal(err)
	}
	devices, err := s.ListDevices(context.Background(), "user-a")
	if err != nil || len(devices) != 2 {
		t.Fatalf("list: %+v %v", devices, err)
	}
	// Both devices share CreatedAt, so their IDs break the tie.
	first, second := deviceA.ID, deviceB.ID
	if first > second {
		first, second = second, first
	}
	if devices[0].ID != first || devices[1].ID != second {
		t.Fatalf("list order: %+v", devices)
	}
	if err := s.RevokeDevice(context.Background(), "user-a", deviceA.ID); err != nil {
		t.Fatal(err)
	}
	if err := s.RevokeDevice(context.Background(), "user-a", deviceA.ID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("double revoke: %v", err)
	}
	if _, _, err := s.RotateDeviceToken(context.Background(), deviceA.ID, "whatever"); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("revoked device rotates: %v", err)
	}
	devices, err = s.ListDevices(context.Background(), "user-a")
	if err != nil || len(devices) != 1 || devices[0].ID != deviceB.ID || devices[0].Name != "" || !devices[0].E2E {
		t.Fatalf("list after revoke: %+v %v", devices, err)
	}
}

func TestCrossUserIsolation(t *testing.T) {
	now := testNow
	s := testStore(&now)
	_, code := mustPair(t, s, "user-a", "org-a")
	device, _, _, token, err := s.RedeemPairing(context.Background(), code, "ios")
	if err != nil {
		t.Fatal(err)
	}
	if valid, _ := s.IsDeviceValid(context.Background(), "user-b", device.ID); valid {
		t.Fatal("foreign device validates")
	}
	if err := s.RevokeDevice(context.Background(), "user-b", device.ID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("foreign revoke: %v", err)
	}
	if valid, err := s.IsDeviceValid(context.Background(), "user-a", device.ID); err != nil || !valid {
		t.Fatalf("own device invalid after foreign attempts: %v %v", valid, err)
	}
	if _, _, err := s.RotateDeviceToken(context.Background(), device.ID, token); err != nil {
		t.Fatalf("own rotation failed after foreign attempts: %v", err)
	}
	if devices, err := s.ListDevices(context.Background(), "user-b"); err != nil || len(devices) != 0 {
		t.Fatalf("foreign list leaks: %+v %v", devices, err)
	}
	if devices, err := s.ListDevices(context.Background(), "user-unknown"); err != nil || len(devices) != 0 {
		t.Fatalf("unknown user list should be empty: %+v %v", devices, err)
	}
	if _, err := s.ListDevices(context.Background(), "not a user id"); !errors.Is(err, ErrInvalid) {
		t.Fatalf("malformed user list: %v", err)
	}
	// Malformed ids never validate and never revoke.
	if valid, _ := s.IsDeviceValid(context.Background(), "user-a", "not-a-uuid"); valid {
		t.Fatal("malformed id validates")
	}
	if err := s.RevokeDevice(context.Background(), "user-a", "not-a-uuid"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("malformed revoke: %v", err)
	}
	if err := s.RevokeDevice(context.Background(), "user-a", uuid.NewString()); !errors.Is(err, ErrNotFound) {
		t.Fatalf("unknown revoke: %v", err)
	}
}

func TestStoreCancelledContext(t *testing.T) {
	now := testNow
	s := testStore(&now)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, _, err := s.CreatePairing(ctx, "user-a", "org-a"); !errors.Is(err, context.Canceled) {
		t.Fatalf("create: %v", err)
	}
	if _, _, _, _, err := s.RedeemPairing(ctx, strings.Repeat("A", CodeLength), "ios"); !errors.Is(err, context.Canceled) {
		t.Fatalf("redeem: %v", err)
	}
	if _, _, err := s.RotateDeviceToken(ctx, uuid.NewString(), "token"); !errors.Is(err, context.Canceled) {
		t.Fatalf("rotate: %v", err)
	}
	if _, err := s.ListDevices(ctx, "user-a"); !errors.Is(err, context.Canceled) {
		t.Fatalf("list: %v", err)
	}
	if err := s.RevokeDevice(ctx, "user-a", uuid.NewString()); !errors.Is(err, context.Canceled) {
		t.Fatalf("revoke: %v", err)
	}
	if _, err := s.IsDeviceValid(ctx, "user-a", uuid.NewString()); !errors.Is(err, context.Canceled) {
		t.Fatalf("valid: %v", err)
	}
}
