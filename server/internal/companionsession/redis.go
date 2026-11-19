package companionsession

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"time"

	"github.com/redis/go-redis/v9"
)

// RedisStore gives every record an expiry at creation. Resume performs only
// GET; it never issues SET/EXPIRE and therefore cannot renew a lease.
type RedisStore struct {
	client *redis.Client
	now    func() time.Time
}

func NewRedisStore(client *redis.Client, now ...func() time.Time) *RedisStore {
	return &RedisStore{client: client, now: storeClock(now)}
}

// Both script keys share a hash slot without putting raw account ids in keys.
func accountTag(userID string) string {
	sum := sha256.Sum256([]byte(userID))
	return "{" + hex.EncodeToString(sum[:]) + "}"
}
func leaseKey(userID, id string) string { return "companion:lease:v1:" + accountTag(userID) + ":" + id }
func indexKey(userID string) string     { return "companion:index:v1:" + accountTag(userID) }

// Prune/count/create is atomic. NX also protects an existing lease against a
// UUID collision. Only the ACCOUNT INDEX may extend for a separately created
// lease; no individual lease ever changes its original deadline.
const createScript = `
local now = tonumber(ARGV[1])
local deadline = tonumber(ARGV[2])
if deadline <= now then return -2 end
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', now)
if redis.call('ZCARD', KEYS[2]) >= tonumber(ARGV[3]) then return -1 end
local stored = redis.call('SET', KEYS[1], ARGV[4], 'PX', ARGV[5], 'NX')
if not stored then return 0 end
redis.call('ZADD', KEYS[2], deadline, ARGV[6])
local latest = redis.call('ZREVRANGE', KEYS[2], 0, 0, 'WITHSCORES')
redis.call('PEXPIREAT', KEYS[2], latest[2])
return 1
`

func (s *RedisStore) Create(ctx context.Context, userID, deviceID string) (Lease, error) {
	if !validOwner(userID, deviceID) {
		return Lease{}, ErrInvalid
	}
	if err := ctx.Err(); err != nil {
		return Lease{}, err
	}
	if s.client == nil {
		return Lease{}, ErrStorage
	}
	now := s.now()
	for attempt := 0; attempt < 3; attempt++ {
		lease, err := createLease(userID, deviceID, now)
		if err != nil {
			return Lease{}, err
		}
		data, err := json.Marshal(lease)
		if err != nil {
			return Lease{}, ErrStorage
		}
		admissionTime := s.now()
		remaining := lease.ExpiresAt.Sub(admissionTime).Milliseconds()
		if remaining <= 0 {
			return Lease{}, ErrUnavailable
		}
		result, err := s.client.Eval(ctx, createScript,
			[]string{leaseKey(userID, lease.ID), indexKey(userID)},
			admissionTime.UnixMilli(), lease.ExpiresAt.UnixMilli(), MaxPerUser, data, remaining, lease.ID,
		).Int64()
		if err != nil {
			return Lease{}, storageError(ctx)
		}
		switch result {
		case 1:
			return lease, nil
		case -1:
			return Lease{}, ErrLimit
		case -2:
			return Lease{}, ErrUnavailable
		case 0: // Collision: retry a fresh UUID without extending the deadline.
		default:
			return Lease{}, ErrStorage
		}
	}
	return Lease{}, ErrStorage
}

func (s *RedisStore) Resume(ctx context.Context, id, userID, deviceID string) (Lease, error) {
	lease, _, err := s.load(ctx, id, userID, deviceID)
	return lease, err
}

// Compare the exact validated record before deleting. A concurrent revoke or
// replacement cannot make this call delete an unvalidated newer record.
const revokeScript = `
if redis.call('GET', KEYS[1]) == ARGV[1] then
  redis.call('DEL', KEYS[1])
  redis.call('ZREM', KEYS[2], ARGV[2])
  if redis.call('ZCARD', KEYS[2]) == 0 then redis.call('DEL', KEYS[2]) end
  return 1
end
return 0
`

func (s *RedisStore) Revoke(ctx context.Context, id, userID, deviceID string) error {
	_, raw, err := s.load(ctx, id, userID, deviceID)
	if err != nil {
		return err
	}
	removed, err := s.client.Eval(ctx, revokeScript, []string{leaseKey(userID, id), indexKey(userID)}, raw, id).Int64()
	if err != nil {
		return storageError(ctx)
	}
	if removed != 1 {
		return ErrUnavailable
	}
	// Sockets resuming this lease on other API instances end on the event.
	PublishRevocation(ctx, s.client, Revocation{Kind: RevokedLease, UserID: userID, DeviceID: deviceID, LeaseID: id})
	return nil
}

func (s *RedisStore) load(ctx context.Context, id, userID, deviceID string) (Lease, string, error) {
	if !validOwner(userID, deviceID) || !validID(id) {
		return Lease{}, "", ErrUnavailable
	}
	if err := ctx.Err(); err != nil {
		return Lease{}, "", err
	}
	if s.client == nil {
		return Lease{}, "", ErrStorage
	}
	raw, err := s.client.Get(ctx, leaseKey(userID, id)).Result()
	if errors.Is(err, redis.Nil) {
		return Lease{}, "", ErrUnavailable
	}
	if err != nil {
		return Lease{}, "", storageError(ctx)
	}
	var lease Lease
	if json.Unmarshal([]byte(raw), &lease) != nil || !matches(lease, id, userID, deviceID, s.now()) {
		return Lease{}, "", ErrUnavailable
	}
	return lease, raw, nil
}

func storageError(ctx context.Context) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	return ErrStorage
}

var _ Store = (*RedisStore)(nil)
