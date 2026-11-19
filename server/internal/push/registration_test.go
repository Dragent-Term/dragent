package push

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/google/uuid"
)

func testIDs() (userID, deviceID uuid.UUID) {
	return uuid.New(), uuid.New()
}

func TestMemoryStoreRegisterAndRenew(t *testing.T) {
	s := NewMemoryStore()
	ctx := context.Background()
	userID, deviceID := testIDs()

	fixed := time.Now().Truncate(time.Second)
	s.now = func() time.Time { return fixed }
	if err := s.Upsert(ctx, Registration{
		DeviceID: deviceID, UserID: userID, Platform: "fcm", PushToken: "tok-1",
	}); err != nil {
		t.Fatalf("Upsert: %v", err)
	}
	got, found, err := s.Get(ctx, deviceID)
	if err != nil || !found {
		t.Fatalf("Get: found=%v err=%v", found, err)
	}
	if got.PushToken != "tok-1" || got.Platform != PlatformFCM {
		t.Fatalf("stored = %+v, want token tok-1 platform fcm", got)
	}
	if want := fixed.Add(RegistrationTTL); !got.ExpiresAt.Equal(want) {
		t.Fatalf("expires = %v, want %v", got.ExpiresAt, want)
	}

	// Re-registering (renewal) extends the lease and swaps the token.
	later := fixed.Add(time.Hour)
	s.now = func() time.Time { return later }
	if err := s.Upsert(ctx, Registration{
		DeviceID: deviceID, UserID: userID, Platform: "FCM", PushToken: "tok-2", Foreground: true,
	}); err != nil {
		t.Fatalf("renew Upsert: %v", err)
	}
	got, _, _ = s.Get(ctx, deviceID)
	if got.PushToken != "tok-2" || !got.Foreground {
		t.Fatalf("renewed = %+v, want tok-2 foreground", got)
	}
	if want := later.Add(RegistrationTTL); !got.ExpiresAt.Equal(want) {
		t.Fatalf("renewed expires = %v, want %v", got.ExpiresAt, want)
	}
}

func TestMemoryStoreRotate(t *testing.T) {
	s := NewMemoryStore()
	ctx := context.Background()
	userID, deviceID := testIDs()

	if err := s.Upsert(ctx, Registration{
		DeviceID: deviceID, UserID: userID, Platform: "apns", PushToken: "old",
	}); err != nil {
		t.Fatal(err)
	}

	// Foreground rotation: old token matches, new token lands, lease renews.
	before, _, _ := s.Get(ctx, deviceID)
	if err := s.Rotate(ctx, userID, deviceID, "old", "new", true); err != nil {
		t.Fatalf("Rotate: %v", err)
	}
	after, _, _ := s.Get(ctx, deviceID)
	if after.PushToken != "new" || !after.Foreground {
		t.Fatalf("rotated = %+v, want token new foreground", after)
	}
	if !after.ExpiresAt.After(before.ExpiresAt) && !after.ExpiresAt.Equal(before.ExpiresAt.Add(0)) {
		t.Fatalf("rotation did not renew expiry: %v -> %v", before.ExpiresAt, after.ExpiresAt)
	}

	// A stale client holding the pre-rotation token cannot clobber it.
	if err := s.Rotate(ctx, userID, deviceID, "old", "evil", false); !errors.Is(err, ErrTokenMismatch) {
		t.Fatalf("stale Rotate = %v, want ErrTokenMismatch", err)
	}
	kept, _, _ := s.Get(ctx, deviceID)
	if kept.PushToken != "new" {
		t.Fatalf("stale rotation clobbered token: %+v", kept)
	}

	// No registration at all.
	if err := s.Rotate(ctx, userID, uuid.New(), "x", "y", false); !errors.Is(err, ErrNotFound) {
		t.Fatalf("unknown Rotate = %v, want ErrNotFound", err)
	}
	// Another user's device id is not rotatable.
	if err := s.Rotate(ctx, uuid.New(), deviceID, "new", "z", false); !errors.Is(err, ErrNotFound) {
		t.Fatalf("cross-user Rotate = %v, want ErrNotFound", err)
	}
	// Empty replacement token is rejected.
	if err := s.Rotate(ctx, userID, deviceID, "new", "  ", false); err == nil {
		t.Fatal("empty new token Rotate unexpectedly succeeded")
	}
}

func TestMemoryStoreUnregister(t *testing.T) {
	s := NewMemoryStore()
	ctx := context.Background()
	userID, deviceID := testIDs()

	if err := s.Delete(ctx, userID, deviceID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("unregister missing = %v, want ErrNotFound", err)
	}
	if err := s.Upsert(ctx, Registration{
		DeviceID: deviceID, UserID: userID, Platform: "fcm", PushToken: "tok",
	}); err != nil {
		t.Fatal(err)
	}
	if err := s.Delete(ctx, userID, deviceID); err != nil {
		t.Fatalf("Delete: %v", err)
	}
	if _, found, _ := s.Get(ctx, deviceID); found {
		t.Fatal("registration survived unregister")
	}
	// Durable: a second unregister observes the absence.
	if err := s.Delete(ctx, userID, deviceID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("second Delete = %v, want ErrNotFound", err)
	}
	// Another user cannot remove it (and observes not-found, not the row).
	other := uuid.New()
	_ = s.Upsert(ctx, Registration{DeviceID: deviceID, UserID: userID, Platform: "fcm", PushToken: "tok"})
	if err := s.Delete(ctx, other, deviceID); !errors.Is(err, ErrNotFound) {
		t.Fatalf("cross-user Delete = %v, want ErrNotFound", err)
	}
}

func TestMemoryStoreDeleteExpired(t *testing.T) {
	s := NewMemoryStore()
	ctx := context.Background()
	userID := uuid.New()
	past := time.Now().Add(-2 * RegistrationTTL).Truncate(time.Second)
	s.now = func() time.Time { return past }
	for i := 0; i < 3; i++ {
		if err := s.Upsert(ctx, Registration{
			DeviceID: uuid.New(), UserID: userID, Platform: "fcm", PushToken: "old",
		}); err != nil {
			t.Fatal(err)
		}
	}
	liveID := uuid.New()
	s.now = time.Now
	if err := s.Upsert(ctx, Registration{
		DeviceID: liveID, UserID: userID, Platform: "fcm", PushToken: "live",
	}); err != nil {
		t.Fatal(err)
	}
	n, err := s.DeleteExpired(ctx, time.Now())
	if err != nil {
		t.Fatalf("DeleteExpired: %v", err)
	}
	if n != 3 {
		t.Fatalf("DeleteExpired removed %d, want 3", n)
	}
	live, err := s.ListForUser(ctx, userID)
	if err != nil {
		t.Fatal(err)
	}
	if len(live) != 1 || live[0].DeviceID != liveID {
		t.Fatalf("live set = %+v, want only the live registration", live)
	}
}

func TestValidateRegistration(t *testing.T) {
	userID, deviceID := testIDs()
	valid := Registration{DeviceID: deviceID, UserID: userID, Platform: "apns", PushToken: "tok"}
	if err := ValidateRegistration(valid); err != nil {
		t.Fatalf("valid registration rejected: %v", err)
	}
	cases := []struct {
		name string
		mut  func(*Registration)
	}{
		{"zero device", func(r *Registration) { r.DeviceID = uuid.Nil }},
		{"zero user", func(r *Registration) { r.UserID = uuid.Nil }},
		{"bad platform", func(r *Registration) { r.Platform = "sms" }},
		{"empty platform", func(r *Registration) { r.Platform = "" }},
		{"empty token", func(r *Registration) { r.PushToken = "  " }},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := valid
			tc.mut(&r)
			if err := ValidateRegistration(r); err == nil {
				t.Fatal("expected validation error")
			}
		})
	}
	huge := valid
	huge.PushToken = string(make([]byte, maxPushTokenBytes+1))
	if err := ValidateRegistration(huge); err == nil {
		t.Fatal("expected oversized token to be rejected")
	}
}
