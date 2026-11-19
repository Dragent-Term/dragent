package companionsession

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"strings"
	"testing"
	"time"

	"github.com/redis/go-redis/v9"
)

// Intercept real go-redis commands rather than adding a Redis emulator
// dependency. These tests assert command construction and Redis-result handling;
// they do not claim to execute the Lua script against a Redis server.
type commandHook struct {
	process func(context.Context, redis.Cmder) error
}

func (h commandHook) DialHook(next redis.DialHook) redis.DialHook {
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

func leaseJSON(t *testing.T, lease Lease) string {
	t.Helper()
	raw, err := json.Marshal(lease)
	if err != nil {
		t.Fatal(err)
	}
	return string(raw)
}

func TestRedisCreateUsesNXAndRemainingOriginalLifetime(t *testing.T) {
	calls := 0
	now := testNow
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
		calls++
		args := cmd.Args()
		if cmd.Name() != "eval" || len(args) != 11 || args[1] != createScript || args[2] != 2 {
			t.Fatalf("unexpected lease creation command: %v", args)
		}
		var lease Lease
		if err := json.Unmarshal(args[8].([]byte), &lease); err != nil {
			t.Fatal(err)
		}
		if args[3] != leaseKey("account", lease.ID) || args[4] != indexKey("account") || args[5] != testNow.Add(500*time.Millisecond).UnixMilli() || args[6] != testNow.Add(Duration).UnixMilli() || args[7] != MaxPerUser || args[9] != int64((Duration-500*time.Millisecond)/time.Millisecond) || args[10] != lease.ID {
			t.Fatalf("incorrect fixed-expiry/cap params: %v", args)
		}
		if !strings.Contains(createScript, "'NX'") || !strings.Contains(createScript, "'PX'") || !strings.Contains(createScript, "'ZREMRANGEBYSCORE'") || !strings.Contains(createScript, "'ZCARD'") {
			t.Fatal("admission must atomically prune/cap/SET NX")
		}
		cmd.(*redis.Cmd).SetVal(int64(1))
		return nil
	})
	clockCalls := 0
	store := NewRedisStore(client, func() time.Time {
		clockCalls++
		if clockCalls > 1 {
			now = testNow.Add(500 * time.Millisecond)
		}
		return now
	})
	lease, err := store.Create(context.Background(), "account", "desktop")
	if err != nil || calls != 1 || !lease.CreatedAt.Equal(testNow) {
		t.Fatalf("create: %+v, %v", lease, err)
	}
}

func TestRedisAdmissionCapacityAndCollisionDeadline(t *testing.T) {
	for _, result := range []int64{-1, -2, 0} {
		calls := 0
		client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error { calls++; cmd.(*redis.Cmd).SetVal(result); return nil })
		_, err := NewRedisStore(client, func() time.Time { return testNow }).Create(context.Background(), "account", "desktop")
		want := ErrStorage
		if result == -1 {
			want = ErrLimit
		}
		if result == -2 {
			want = ErrUnavailable
		}
		if !errors.Is(err, want) {
			t.Fatalf("admission %d: %v", result, err)
		}
		if result == 0 && calls != 3 {
			t.Fatal("UUID collision did not stop after bounded retries")
		}
	}
}

func TestRedisResumeOnlyReadsAndCannotExtendExpiry(t *testing.T) {
	lease, _ := createLease("account", "desktop", testNow)
	raw := leaseJSON(t, lease)
	now := testNow.Add(23 * time.Hour)
	calls := 0
	expectedUser := lease.UserID
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
		calls++
		if cmd.Name() != "get" || cmd.Args()[1] != leaseKey(expectedUser, lease.ID) {
			t.Fatalf("resume wrote/renewed lease: %v", cmd.Args())
		}
		cmd.(*redis.StringCmd).SetVal(raw)
		return nil
	})
	s := NewRedisStore(client, func() time.Time { return now })
	resumed, err := s.Resume(context.Background(), lease.ID, "account", "desktop")
	if err != nil || resumed != lease || calls != 1 {
		t.Fatalf("resume changed deadline: %+v %v", resumed, err)
	}
	for _, input := range []struct{ user, device string }{{"foreign", "desktop"}, {"account", "foreign"}} {
		expectedUser = input.user
		if _, err := s.Resume(context.Background(), lease.ID, input.user, input.device); !errors.Is(err, ErrUnavailable) {
			t.Fatal(err)
		}
	}
	now = lease.ExpiresAt
	expectedUser = lease.UserID
	if _, err := s.Resume(context.Background(), lease.ID, "account", "desktop"); !errors.Is(err, ErrUnavailable) {
		t.Fatal("expired record admitted before Redis cleanup")
	}
}

func TestRedisRevokeComparesValidatedRecordBeforeDeletion(t *testing.T) {
	lease, _ := createLease("account", "", testNow)
	raw := leaseJSON(t, lease)
	calls := 0
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
		calls++
		switch cmd.Name() {
		case "get":
			cmd.(*redis.StringCmd).SetVal(raw)
		case "eval":
			args := cmd.Args()
			if args[1] != revokeScript || args[2] != 2 || args[3] != leaseKey(lease.UserID, lease.ID) || args[4] != indexKey(lease.UserID) || args[5] != raw || args[6] != lease.ID {
				t.Fatalf("revoke failed exact record comparison: %v", args)
			}
			if !strings.Contains(revokeScript, "redis.call('GET', KEYS[1]) == ARGV[1]") || !strings.Contains(revokeScript, "redis.call('DEL', KEYS[1])") {
				t.Fatal("revoke must compare before deleting")
			}
			cmd.(*redis.Cmd).SetVal(int64(1))
		case "publish":
			// Other instances close sockets on this lease from the event.
			args := cmd.Args()
			var event Revocation
			if args[1] != RevocationChannel || json.Unmarshal(args[2].([]byte), &event) != nil ||
				event != (Revocation{Kind: RevokedLease, UserID: "account", LeaseID: lease.ID}) {
				t.Fatalf("unexpected revocation event: %v", args)
			}
			cmd.(*redis.IntCmd).SetVal(1)
		default:
			t.Fatalf("unexpected revoke command: %v", cmd.Args())
		}
		return nil
	})
	s := NewRedisStore(client, func() time.Time { return testNow })
	if err := s.Revoke(context.Background(), lease.ID, "account", ""); err != nil || calls != 3 {
		t.Fatalf("revoke: %v, %d calls", err, calls)
	}
}

func TestRedisFailuresStaySanitizedAndMissingRecordsAreUnavailable(t *testing.T) {
	lease, _ := createLease("account", "desktop", testNow)
	for _, input := range []struct {
		name, raw string
		err       error
		want      error
	}{
		{"missing", "", redis.Nil, ErrUnavailable},
		{"corrupt", "not JSON bearer=private", nil, ErrUnavailable},
		{"storage", "", errors.New("dial redis://private-secret@host failed"), ErrStorage},
	} {
		t.Run(input.name, func(t *testing.T) {
			client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
				cmd.(*redis.StringCmd).SetVal(input.raw)
				cmd.SetErr(input.err)
				return input.err
			})
			s := NewRedisStore(client)
			if _, err := s.Resume(context.Background(), lease.ID, "account", "desktop"); !errors.Is(err, input.want) || strings.Contains(err.Error(), "private") {
				t.Fatalf("unsafe error: %v", err)
			}
		})
	}
	if _, err := NewRedisStore(nil).Create(context.Background(), "account", "desktop"); !errors.Is(err, ErrStorage) {
		t.Fatal("nil client did not fail closed", err)
	}
}

func TestRedisConcurrentRevokeCannotDeleteChangedRecord(t *testing.T) {
	lease, _ := createLease("account", "desktop", testNow)
	raw := leaseJSON(t, lease)
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error {
		switch cmd.Name() {
		case "get":
			cmd.(*redis.StringCmd).SetVal(raw)
		case "eval":
			cmd.(*redis.Cmd).SetVal(int64(0)) // Deleted or replaced since GET.
		default:
			t.Fatalf("unexpected command %v", cmd.Args())
		}
		return nil
	})
	if err := NewRedisStore(client, func() time.Time { return testNow }).Revoke(context.Background(), lease.ID, "account", "desktop"); !errors.Is(err, ErrUnavailable) {
		t.Fatal("lost revoke race reported success", err)
	}
}

func TestStoresRejectPersistedDeadlineExtension(t *testing.T) {
	ctx := context.Background()
	memory := NewMemoryStore(func() time.Time { return testNow })
	lease, err := memory.Create(ctx, "account", "desktop")
	if err != nil {
		t.Fatal(err)
	}
	lease.ExpiresAt = lease.ExpiresAt.Add(time.Hour)
	memory.byUser[lease.UserID][lease.ID] = lease
	if _, err := memory.Resume(ctx, lease.ID, lease.UserID, lease.DeviceID); !errors.Is(err, ErrUnavailable) {
		t.Fatal("extended memory deadline accepted", err)
	}

	raw := leaseJSON(t, lease)
	client := stubRedis(t, func(_ context.Context, cmd redis.Cmder) error { cmd.(*redis.StringCmd).SetVal(raw); return nil })
	if _, err := NewRedisStore(client, func() time.Time { return testNow }).Resume(ctx, lease.ID, lease.UserID, lease.DeviceID); !errors.Is(err, ErrUnavailable) {
		t.Fatal("extended Redis deadline accepted", err)
	}
	if accountTag("account") == accountTag("foreign") || !strings.Contains(leaseKey(lease.UserID, lease.ID), accountTag(lease.UserID)) || !strings.Contains(indexKey(lease.UserID), accountTag(lease.UserID)) {
		t.Fatal("account keys are not isolated in their own Redis slot")
	}
}
