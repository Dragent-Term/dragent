package companionpairing

import (
	"context"
	"errors"
	"os"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"
)

// Opt in against a dedicated test Redis. No flush or global key scan is
// used: cleanup deletes only keys tagged with the UUID generated for this
// test plus the code-hash keys this test created.
func integrationRedis(t *testing.T) (*RedisStore, *redis.Client, string, *[]string) {
	t.Helper()
	addr := os.Getenv("DRAGENT_TEST_REDIS_ADDR")
	if addr == "" {
		t.Skip("set DRAGENT_TEST_REDIS_ADDR to run real Redis/Lua integration")
	}
	client := redis.NewClient(&redis.Options{Addr: addr, MaxRetries: -1, DialTimeout: time.Second})
	t.Cleanup(func() { _ = client.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := client.Ping(ctx).Err(); err != nil {
		t.Skip("dedicated test Redis is unavailable")
	}
	userID := "test-pairing-" + uuid.NewString()
	var codeHashes []string
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		keys := []string{pairingIndexKey(userID), deviceIndexKey(userID)}
		for _, h := range codeHashes {
			keys = append(keys, pairingCodeKey(h), pairingGoneKey(h))
		}
		if ids, err := client.SMembers(ctx, deviceIndexKey(userID)).Result(); err == nil {
			for _, id := range ids {
				keys = append(keys, deviceKey(id))
			}
		}
		if members, err := client.ZRange(ctx, pairingIndexKey(userID), 0, -1).Result(); err == nil {
			for _, m := range members {
				if len(m) > 37 {
					keys = append(keys, pairingCodeKey(m[len(m)-64:]))
				}
			}
		}
		_ = client.Del(ctx, keys...).Err()
	})
	return NewRedisStore(client), client, userID, &codeHashes
}

func TestRedisIntegrationRoundTrip(t *testing.T) {
	store, _, user, hashes := integrationRedis(t)
	ctx := context.Background()

	pairing, code, err := store.CreatePairing(ctx, user, "org-redis")
	if err != nil {
		t.Fatal(err)
	}
	*hashes = append(*hashes, hashSecret(code))
	if pairing.UserID != user || pairing.OrgID != "org-redis" {
		t.Fatalf("pairing not bound: %+v", pairing)
	}

	device, owner, org, token, err := store.RedeemPairing(ctx, code, "ios")
	if err != nil {
		t.Fatal(err)
	}
	if owner != user || org != "org-redis" || device.Name != "" || device.Platform != "ios" || !device.E2E {
		t.Fatalf("device not bound: %+v owner=%s org=%s", device, owner, org)
	}
	if valid, err := store.IsDeviceValid(ctx, user, device.ID); err != nil || !valid {
		t.Fatalf("device invalid: %v %v", valid, err)
	}

	next, _, err := store.RotateDeviceToken(ctx, device.ID, token)
	if err != nil || next == token {
		t.Fatalf("rotate: %v", err)
	}
	if _, _, err := store.RotateDeviceToken(ctx, device.ID, token); !errors.Is(err, ErrRevoked) {
		t.Fatalf("reuse did not revoke: %v", err)
	}
	if valid, _ := store.IsDeviceValid(ctx, user, device.ID); valid {
		t.Fatal("revoked device still valid")
	}

	// Fresh pairing revokes cleanly through the HTTP-facing path.
	_, code2, err := store.CreatePairing(ctx, user, "org-redis")
	if err != nil {
		t.Fatal(err)
	}
	*hashes = append(*hashes, hashSecret(code2))
	device2, _, _, token2, err := store.RedeemPairing(ctx, code2, "android")
	if err != nil {
		t.Fatal(err)
	}
	devices, err := store.ListDevices(ctx, user)
	if err != nil || len(devices) != 1 || devices[0].ID != device2.ID {
		t.Fatalf("list: %+v %v", devices, err)
	}
	if err := store.RevokeDevice(ctx, user, device2.ID); err != nil {
		t.Fatal(err)
	}
	if _, _, err := store.RotateDeviceToken(ctx, device2.ID, token2); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("revoked device rotates: %v", err)
	}
}

func TestRedisIntegrationSingleUseRace(t *testing.T) {
	store, _, user, hashes := integrationRedis(t)
	ctx := context.Background()

	_, code, err := store.CreatePairing(ctx, user, "org-redis")
	if err != nil {
		t.Fatal(err)
	}
	*hashes = append(*hashes, hashSecret(code))

	const racers = 8
	var won atomic.Int64
	var gone atomic.Int64
	var wg sync.WaitGroup
	for i := 0; i < racers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, _, _, _, err := store.RedeemPairing(ctx, code, "ios")
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
		t.Fatalf("race: %d won, %d gone", won.Load(), gone.Load())
	}
}

func TestRedisIntegrationCapDropsOldest(t *testing.T) {
	store, _, user, hashes := integrationRedis(t)
	ctx := context.Background()

	var firstCode string
	for i := 0; i < MaxLivePairingsPerUser+1; i++ {
		_, code, err := store.CreatePairing(ctx, user, "org-redis")
		if err != nil {
			t.Fatal(err)
		}
		*hashes = append(*hashes, hashSecret(code))
		if i == 0 {
			firstCode = code
		}
	}
	if _, _, _, _, err := store.RedeemPairing(ctx, firstCode, "ios"); !errors.Is(err, ErrGone) {
		t.Fatalf("dropped oldest redeems: %v", err)
	}
}
