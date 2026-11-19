package companionsession

import (
	"context"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
)

var testNow = time.Date(2026, time.November, 2, 10, 0, 0, 0, time.UTC)

func TestMemoryResumeKeepsOriginalDeadline(t *testing.T) {
	now := testNow
	s := NewMemoryStore(func() time.Time { return now })
	lease, err := s.Create(context.Background(), "account-a", "desktop-a")
	if err != nil {
		t.Fatal(err)
	}
	if !validID(lease.ID) || !lease.CreatedAt.Equal(testNow) || !lease.ExpiresAt.Equal(testNow.Add(Duration)) {
		t.Fatalf("invalid new lease: %+v", lease)
	}
	now = now.Add(23 * time.Hour)
	resumed, err := s.Resume(context.Background(), lease.ID, lease.UserID, lease.DeviceID)
	if err != nil || resumed != lease {
		t.Fatalf("resume changed original lease: %+v, %v", resumed, err)
	}
	now = lease.ExpiresAt
	if _, err = s.Resume(context.Background(), lease.ID, lease.UserID, lease.DeviceID); !errors.Is(err, ErrUnavailable) {
		t.Fatalf("expired at exact deadline: %v", err)
	}
}

func TestMemoryOwnerDeviceAndDiscoveryIsolation(t *testing.T) {
	s := NewMemoryStore(func() time.Time { return testNow })
	lease, _ := s.Create(context.Background(), "account-a", "desktop-a")
	for _, input := range []struct{ id, user, device string }{
		{lease.ID, "account-b", "desktop-a"},
		{lease.ID, "account-a", "desktop-b"},
		{lease.ID, "account-a", ""},
		{uuid.NewString(), "account-a", "desktop-a"},
		{"not-a-session", "account-a", "desktop-a"},
		{lease.ID, "", "desktop-a"},
	} {
		if _, err := s.Resume(context.Background(), input.id, input.user, input.device); !errors.Is(err, ErrUnavailable) {
			t.Fatalf("unsafe lookup result: %v", err)
		}
		if err := s.Revoke(context.Background(), input.id, input.user, input.device); !errors.Is(err, ErrUnavailable) {
			t.Fatalf("unsafe revoke result: %v", err)
		}
	}
	if _, err := s.Resume(context.Background(), lease.ID, lease.UserID, lease.DeviceID); err != nil {
		t.Fatal("foreign revoke changed original lease", err)
	}
	discovery, err := s.Create(context.Background(), "account-a", "")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.Resume(context.Background(), discovery.ID, "account-a", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Resume(context.Background(), discovery.ID, "account-a", "desktop-a"); !errors.Is(err, ErrUnavailable) {
		t.Fatal("discovery lease expanded device scope")
	}
}

func TestMemoryRevokeAndExpiredCapacityRecovery(t *testing.T) {
	now := testNow
	s := NewMemoryStore(func() time.Time { return now })
	var first Lease
	for i := 0; i < MaxPerUser; i++ {
		lease, err := s.Create(context.Background(), "account-a", "desktop")
		if err != nil {
			t.Fatal(err)
		}
		if i == 0 {
			first = lease
		}
	}
	if _, err := s.Create(context.Background(), "account-a", "other-device"); !errors.Is(err, ErrLimit) {
		t.Fatalf("per-account cap bypassed: %v", err)
	}
	if _, err := s.Create(context.Background(), "account-b", "desktop"); err != nil {
		t.Fatal("unrelated account exhausted", err)
	}
	if err := s.Revoke(context.Background(), first.ID, first.UserID, first.DeviceID); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Resume(context.Background(), first.ID, first.UserID, first.DeviceID); !errors.Is(err, ErrUnavailable) {
		t.Fatal("revoked lease resumed")
	}
	if _, err := s.Create(context.Background(), "account-a", "desktop"); err != nil {
		t.Fatal("revoke did not free capacity", err)
	}
	now = now.Add(Duration)
	fresh, err := s.Create(context.Background(), "account-a", "desktop")
	if err != nil {
		t.Fatal("expired entries consumed capacity", err)
	}
	if _, err := s.Resume(context.Background(), fresh.ID, fresh.UserID, fresh.DeviceID); err != nil {
		t.Fatal("new lease lost when pruning its old account bucket", err)
	}
	if len(s.byUser["account-a"]) != 1 {
		t.Fatal("expired entries remain")
	}
}

func TestMemoryConcurrentCreateBoundAndRevoke(t *testing.T) {
	s := NewMemoryStore(func() time.Time { return testNow })
	var wg sync.WaitGroup
	created := make(chan Lease, MaxPerUser*2)
	failures := make(chan error, MaxPerUser*2)
	for i := 0; i < MaxPerUser*2; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			lease, err := s.Create(context.Background(), "account", "desktop")
			if err != nil {
				failures <- err
			} else {
				created <- lease
			}
		}()
	}
	wg.Wait()
	close(created)
	close(failures)
	if len(created) != MaxPerUser || len(failures) != MaxPerUser {
		t.Fatalf("concurrent cap: %d created/%d rejected", len(created), len(failures))
	}
	for err := range failures {
		if !errors.Is(err, ErrLimit) {
			t.Fatal(err)
		}
	}
	lease := <-created
	wg.Add(2)
	go func() {
		defer wg.Done()
		_, err := s.Resume(context.Background(), lease.ID, lease.UserID, lease.DeviceID)
		if err != nil && !errors.Is(err, ErrUnavailable) {
			t.Error(err)
		}
	}()
	go func() {
		defer wg.Done()
		if err := s.Revoke(context.Background(), lease.ID, lease.UserID, lease.DeviceID); err != nil {
			t.Error(err)
		}
	}()
	wg.Wait()
	if _, err := s.Resume(context.Background(), lease.ID, lease.UserID, lease.DeviceID); !errors.Is(err, ErrUnavailable) {
		t.Fatal("resume after completed revoke succeeded")
	}
}

func TestIdentityValidationAndCancelledContext(t *testing.T) {
	s := NewMemoryStore()
	for _, input := range []struct{ user, device string }{
		{"", ""}, {strings.Repeat("x", 129), "desktop"}, {"account", strings.Repeat("x", 129)}, {" account", "desktop"}, {"account", "desktop\x00"},
	} {
		if _, err := s.Create(context.Background(), input.user, input.device); !errors.Is(err, ErrInvalid) {
			t.Fatalf("invalid identity accepted: %v", err)
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := s.Create(ctx, "account", "desktop"); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
}
