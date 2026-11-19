package companionsession

import (
	"context"
	"errors"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"
)

// Opt in against a dedicated test Redis. No flush or global key scan is used:
// cleanup deletes only keys tagged with the UUID generated for this test.
func integrationRedis(t *testing.T, clock func() time.Time) (*RedisStore, *redis.Client, string) {
	t.Helper()
	addr := os.Getenv("DRAGENT_TEST_REDIS_ADDR")
	if addr == "" {
		t.Skip("set DRAGENT_TEST_REDIS_ADDR to run real Redis/Lua integration")
	}
	client := redis.NewClient(&redis.Options{Addr: addr, MaxRetries: -1, DialTimeout: time.Second})
	t.Cleanup(func() { _ = client.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if client.Ping(ctx).Err() != nil {
		t.Fatal("dedicated test Redis is unavailable")
	}
	userID := "test-companion-" + uuid.NewString()
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		ids, err := client.ZRange(ctx, indexKey(userID), 0, -1).Result()
		if err != nil {
			t.Error("could not clean test lease index")
			return
		}
		keys := []string{indexKey(userID)}
		for _, id := range ids {
			keys = append(keys, leaseKey(userID, id))
		}
		if client.Del(ctx, keys...).Err() != nil {
			t.Error("could not clean isolated test leases")
		}
	})
	return NewRedisStore(client, clock), client, userID
}

func TestRedisIntegrationCapacityAndRevoke(t *testing.T) {
	store, _, user := integrationRedis(t, time.Now)
	ctx := context.Background()
	var first Lease
	for i := 0; i < MaxPerUser; i++ {
		lease, err := store.Create(ctx, user, "desktop")
		if err != nil {
			t.Fatal(err)
		}
		if i == 0 {
			first = lease
		}
	}
	if _, err := store.Create(ctx, user, "other-desktop"); !errors.Is(err, ErrLimit) {
		t.Fatalf("real Redis did not enforce cap: %v", err)
	}
	if err := store.Revoke(ctx, first.ID, user, first.DeviceID); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Resume(ctx, first.ID, user, first.DeviceID); !errors.Is(err, ErrUnavailable) {
		t.Fatal("revoked Redis lease resumed")
	}
	if _, err := store.Create(ctx, user, "desktop"); err != nil {
		t.Fatal("revoke did not release Redis account capacity", err)
	}
}

func TestRedisIntegrationResumeDoesNotRenewTTL(t *testing.T) {
	now := time.Now().UTC()
	store, client, user := integrationRedis(t, func() time.Time { return now })
	ctx := context.Background()
	lease, err := store.Create(ctx, user, "desktop")
	if err != nil {
		t.Fatal(err)
	}
	before, err := client.PTTL(ctx, leaseKey(user, lease.ID)).Result()
	if err != nil || before <= 0 || before > Duration {
		t.Fatal("new lease has incorrect Redis TTL")
	}
	now = now.Add(23 * time.Hour)
	resumed, err := store.Resume(ctx, lease.ID, user, "desktop")
	if err != nil || !resumed.ExpiresAt.Equal(lease.ExpiresAt) {
		t.Fatal("resume changed lease deadline", err)
	}
	after, err := client.PTTL(ctx, leaseKey(user, lease.ID)).Result()
	if err != nil || after > before {
		t.Fatal("resume extended Redis TTL")
	}
	now = lease.ExpiresAt
	if _, err := store.Resume(ctx, lease.ID, user, "desktop"); !errors.Is(err, ErrUnavailable) {
		t.Fatal("expired lease admitted while Redis cleanup pending")
	}
}

func TestRedisIntegrationConcurrentAdmissionCap(t *testing.T) {
	store, _, user := integrationRedis(t, time.Now)
	ctx := context.Background()
	var wg sync.WaitGroup
	results := make(chan error, MaxPerUser*2)
	for i := 0; i < MaxPerUser*2; i++ {
		wg.Add(1)
		go func() { defer wg.Done(); _, err := store.Create(ctx, user, "desktop"); results <- err }()
	}
	wg.Wait()
	close(results)
	admitted, denied := 0, 0
	for err := range results {
		if err == nil {
			admitted++
		} else if errors.Is(err, ErrLimit) {
			denied++
		} else {
			t.Fatal(err)
		}
	}
	if admitted != MaxPerUser || denied != MaxPerUser {
		t.Fatalf("atomic Redis cap failed: %d admitted, %d denied", admitted, denied)
	}
}
