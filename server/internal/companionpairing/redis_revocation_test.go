package companionpairing

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"testing"
	"time"

	"github.com/redis/go-redis/v9"

	"server/internal/companionsession"
)

// Intercept go-redis commands (no network) to assert that every device
// revocation is published for the relay instances to act on.
type commandHook struct {
	process func(context.Context, redis.Cmder) error
}

func (h commandHook) DialHook(redis.DialHook) redis.DialHook {
	return func(context.Context, string, string) (net.Conn, error) {
		return nil, errors.New("unexpected network access")
	}
}
func (h commandHook) ProcessHook(redis.ProcessHook) redis.ProcessHook { return h.process }
func (h commandHook) ProcessPipelineHook(next redis.ProcessPipelineHook) redis.ProcessPipelineHook {
	return next
}

func stubRedis(t *testing.T, process func(context.Context, redis.Cmder) error) *redis.Client {
	t.Helper()
	client := redis.NewClient(&redis.Options{Addr: "127.0.0.1:1", MaxRetries: -1})
	client.AddHook(commandHook{process: process})
	t.Cleanup(func() { _ = client.Close() })
	return client
}

func stubDevice(t *testing.T) (deviceRecord, string) {
	t.Helper()
	rec := newDeviceRecord("account-a", "", "ios", hashSecret("current"), time.Now())
	rec.PrevTokenHash = hashSecret("previous")
	raw, err := json.Marshal(rec)
	if err != nil {
		t.Fatal(err)
	}
	return rec, string(raw)
}

// publishedEvents runs fn against a stub where GET returns raw and every
// EVAL returns evalResult, and collects the revocations it published.
func publishedEvents(t *testing.T, raw string, evalResult int64, fn func(*RedisStore)) []companionsession.Revocation {
	t.Helper()
	var events []companionsession.Revocation
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
		switch cmd.Name() {
		case "get":
			cmd.(*redis.StringCmd).SetVal(raw)
		case "eval":
			cmd.(*redis.Cmd).SetVal(evalResult)
		case "publish":
			args := cmd.Args()
			if args[1] != companionsession.RevocationChannel {
				t.Fatalf("published on %v", args[1])
			}
			var event companionsession.Revocation
			if err := json.Unmarshal(args[2].([]byte), &event); err != nil {
				t.Fatal(err)
			}
			events = append(events, event)
			cmd.(*redis.IntCmd).SetVal(1)
		default:
			t.Fatalf("unexpected command: %v", cmd.Args())
		}
		return nil
	})
	fn(NewRedisStore(client))
	return events
}

func TestRedisRevokeDevicePublishesRevocation(t *testing.T) {
	rec, raw := stubDevice(t)
	want := companionsession.Revocation{Kind: companionsession.RevokedDevice, UserID: rec.UserID, DeviceID: rec.ID}
	events := publishedEvents(t, raw, 1, func(s *RedisStore) {
		if err := s.RevokeDevice(context.Background(), rec.UserID, rec.ID); err != nil {
			t.Fatal(err)
		}
	})
	if len(events) != 1 || events[0] != want {
		t.Fatalf("events = %+v, want %+v", events, want)
	}
	// A revoke that deleted nothing announces nothing.
	if events := publishedEvents(t, raw, 0, func(s *RedisStore) {
		_ = s.RevokeDevice(context.Background(), rec.UserID, rec.ID)
	}); len(events) != 0 {
		t.Fatalf("lost revoke published %+v", events)
	}
}

func TestRedisTokenReusePublishesRevocation(t *testing.T) {
	rec, raw := stubDevice(t)
	want := companionsession.Revocation{Kind: companionsession.RevokedDevice, UserID: rec.UserID, DeviceID: rec.ID}
	events := publishedEvents(t, raw, -1, func(s *RedisStore) {
		if _, _, err := s.RotateDeviceToken(context.Background(), rec.ID, "previous"); !errors.Is(err, ErrRevoked) {
			t.Fatalf("reuse: %v", err)
		}
	})
	if len(events) != 1 || events[0] != want {
		t.Fatalf("events = %+v, want %+v", events, want)
	}
	// A normal rotation announces nothing.
	if events := publishedEvents(t, raw, 1, func(s *RedisStore) {
		if _, _, err := s.RotateDeviceToken(context.Background(), rec.ID, "current"); err != nil {
			t.Fatal(err)
		}
	}); len(events) != 0 {
		t.Fatalf("rotation published %+v", events)
	}
}
