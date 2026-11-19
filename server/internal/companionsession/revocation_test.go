package companionsession

import (
	"context"
	"errors"
	"os"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"
)

func TestRevocationValidation(t *testing.T) {
	lease := uuid.NewString()
	for _, tc := range []struct {
		event Revocation
		ok    bool
	}{
		{Revocation{Kind: RevokedDevice, UserID: "u", DeviceID: "d"}, true},
		{Revocation{Kind: RevokedDevice, UserID: "u"}, false},
		{Revocation{Kind: RevokedDevice, UserID: "u", DeviceID: "d", LeaseID: lease}, false},
		{Revocation{Kind: RevokedLease, UserID: "u", LeaseID: lease}, true},
		{Revocation{Kind: RevokedLease, UserID: "u", DeviceID: "d", LeaseID: lease}, true},
		{Revocation{Kind: RevokedLease, UserID: "u", LeaseID: "not-a-uuid"}, false},
		{Revocation{Kind: RevokedLease, LeaseID: lease}, false},
		{Revocation{Kind: "other", UserID: "u", DeviceID: "d"}, false},
		{Revocation{Kind: RevokedDevice, UserID: "u u", DeviceID: "d"}, false},
	} {
		if got := tc.event.valid(); got != tc.ok {
			t.Errorf("%+v valid = %v, want %v", tc.event, got, tc.ok)
		}
	}
}

// Publishing is best-effort: a Redis error never fails the revocation, and
// the publish runs even when the caller's context is already done (the
// revocation has been committed by then).
func TestRedisRevokeSucceedsWhenPublishFails(t *testing.T) {
	lease, _ := createLease("account", "desktop", testNow)
	raw := leaseJSON(t, lease)
	published := false
	client := stubRedis(t, func(ctx context.Context, cmd redis.Cmder) error {
		switch cmd.Name() {
		case "get":
			cmd.(*redis.StringCmd).SetVal(raw)
		case "eval":
			cmd.(*redis.Cmd).SetVal(int64(1))
		case "publish":
			published = true
			cmd.SetErr(errors.New("publish refused"))
			return cmd.Err()
		}
		return nil
	})
	s := NewRedisStore(client, func() time.Time { return testNow })
	if err := s.Revoke(context.Background(), lease.ID, "account", "desktop"); err != nil || !published {
		t.Fatalf("revoke: %v, published %v", err, published)
	}
}

func TestPublishRevocationSkipsInvalidAndNil(t *testing.T) {
	PublishRevocation(context.Background(), nil, Revocation{Kind: RevokedDevice, UserID: "u", DeviceID: "d"})
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
		t.Fatalf("invalid event published: %v", cmd.Args())
		return nil
	})
	PublishRevocation(context.Background(), client, Revocation{Kind: RevokedDevice, UserID: "u"})
}

// Real pub/sub round trip; opt in with a dedicated test Redis.
func TestRedisIntegrationRevocationRoundTrip(t *testing.T) {
	store, client, user := integrationRedis(t, time.Now)
	got := make(chan Revocation, 4)
	stop := store.SubscribeRevocations(func(e Revocation) {
		if e.UserID == user {
			got <- e
		}
	}, nil)
	defer stop()
	want := Revocation{Kind: RevokedDevice, UserID: user, DeviceID: uuid.NewString()}
	// The subscription may not be live yet: publish until it is seen.
	deadline := time.After(5 * time.Second)
	for {
		PublishRevocation(context.Background(), client, want)
		select {
		case e := <-got:
			if e != want {
				t.Fatalf("got %+v, want %+v", e, want)
			}
			return
		case <-time.After(100 * time.Millisecond):
		case <-deadline:
			t.Fatal("revocation not delivered")
		}
	}
}

func TestSubscribeRevocationsWithoutClientIsNoop(t *testing.T) {
	(&RedisStore{}).SubscribeRevocations(func(Revocation) {}, nil)()
	if os.Getenv("DRAGENT_TEST_REDIS_ADDR") == "" {
		t.Log("pub/sub round trip skipped without DRAGENT_TEST_REDIS_ADDR")
	}
}
