package companionpairing

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"

	"server/internal/companionsession"
)

// RedisStore is the production Store. Pairing consume and token rotation
// are Lua scripts, so concurrent redeems and rotation-reuse revocation
// stay atomic across API instances.
type RedisStore struct {
	client *redis.Client
	now    func() time.Time
}

// NewRedisStore returns a Redis-backed Store.
func NewRedisStore(client *redis.Client, now ...func() time.Time) *RedisStore {
	return &RedisStore{client: client, now: storeClock(now)}
}

func pairingUserTag(userID string) string {
	sum := sha256.Sum256([]byte(userID))
	return "{" + hex.EncodeToString(sum[:]) + "}"
}

func pairingCodeKey(codeHash string) string { return "pairing:code:v1:" + codeHash }
func pairingGoneKey(codeHash string) string { return "pairing:gone:v1:" + codeHash }
func pairingIndexKey(userID string) string  { return "pairing:index:v1:" + pairingUserTag(userID) }
func deviceKey(deviceID string) string      { return "pairdevice:v1:" + deviceID }
func deviceIndexKey(userID string) string   { return "pairdevice:index:v1:" + pairingUserTag(userID) }

// Create prunes expired index entries, drops the oldest live pairing past
// the cap, and stores the new code exactly once (NX guards a code-hash
// collision). The dropped member's code key is deleted by the caller: Lua
// cannot derive it from the member alone without trusting client input,
// so the member carries both ids (pairingID:codeHash) and Go deletes the
// exact code key the script names back.
const pairingCreateScript = `
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', tonumber(ARGV[3]))
local dropped = ''
local droppedScore = ''
if redis.call('ZCARD', KEYS[2]) >= tonumber(ARGV[4]) then
  local old = redis.call('ZPOPMIN', KEYS[2], 1)
  if #old >= 1 then dropped, droppedScore = old[1], old[2] end
end
local stored = redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[5], 'NX')
if not stored then
  if dropped ~= '' then redis.call('ZADD', KEYS[2], droppedScore, dropped) end
  return {0, ''}
end
redis.call('ZADD', KEYS[2], tonumber(ARGV[6]), ARGV[2])
redis.call('PEXPIRE', KEYS[2], ARGV[5])
redis.call('SET', KEYS[3], '1', 'PX', ARGV[7])
return {1, dropped}
`

func (s *RedisStore) CreatePairing(ctx context.Context, userID, orgID string) (Pairing, string, error) {
	if !validOwner(userID, orgID) {
		return Pairing{}, "", ErrInvalid
	}
	if err := ctx.Err(); err != nil {
		return Pairing{}, "", err
	}
	if s.client == nil {
		return Pairing{}, "", ErrStorage
	}
	for attempt := 0; attempt < 3; attempt++ {
		code, err := newCode()
		if err != nil {
			return Pairing{}, "", ErrStorage
		}
		codeHash := hashSecret(code)
		now := s.now().UTC()
		p := newPairing(userID, orgID, now)
		record, err := json.Marshal(pairingPersisted{Pairing: p, CodeHash: codeHash})
		if err != nil {
			return Pairing{}, "", ErrStorage
		}
		member := p.ID + ":" + codeHash
		ttlMs := p.ExpiresAt.Sub(now).Milliseconds()
		if ttlMs <= 0 {
			return Pairing{}, "", ErrStorage
		}
		res, err := s.client.Eval(ctx, pairingCreateScript,
			[]string{pairingCodeKey(codeHash), pairingIndexKey(userID), pairingGoneKey(codeHash)},
			string(record), member, now.Add(-PairingTTL).UnixMilli(), MaxLivePairingsPerUser,
			ttlMs, now.UnixMilli(), TombstoneTTL.Milliseconds(),
		).Result()
		if err != nil {
			return Pairing{}, "", storageError(ctx, err)
		}
		parts, ok := res.([]interface{})
		if !ok || len(parts) != 2 {
			return Pairing{}, "", ErrStorage
		}
		status, _ := parts[0].(int64)
		if status != 1 {
			continue // code-hash collision: retry with fresh entropy
		}
		if dropped, _ := parts[1].(string); dropped != "" {
			if ids := strings.SplitN(dropped, ":", 2); len(ids) == 2 && ids[0] != "" && ids[1] != "" {
				_ = s.client.Del(ctx, pairingCodeKey(ids[1])).Err()
			}
		}
		return p, code, nil
	}
	return Pairing{}, "", ErrStorage
}

// pairingPersisted is the code-key row: the pairing plus its code hash so
// redeem can verify the lookup key matches the stored row.
type pairingPersisted struct {
	Pairing
	CodeHash string `json:"code_hash"`
}

// Redeem consumes the code and tombstones it in one script, then creates
// the device credential. The second concurrent redeem sees the tombstone
// and reports gone.
const pairingRedeemScript = `
local rec = redis.call('GET', KEYS[1])
if not rec then
  if redis.call('EXISTS', KEYS[3]) == 1 then return -1 end
  return 0
end
redis.call('DEL', KEYS[1])
redis.call('ZREM', KEYS[2], ARGV[1])
redis.call('SET', KEYS[3], '1', 'PX', ARGV[2])
return rec
`

func (s *RedisStore) RedeemPairing(ctx context.Context, code, platform string) (Device, string, string, string, error) {
	plat, ok := normalizePlatform(platform)
	if !ok {
		return Device{}, "", "", "", ErrInvalid
	}
	normalized, ok := normalizeCode(code)
	if !ok {
		return Device{}, "", "", "", ErrInvalid
	}
	if err := ctx.Err(); err != nil {
		return Device{}, "", "", "", err
	}
	if s.client == nil {
		return Device{}, "", "", "", ErrStorage
	}
	codeHash := hashSecret(normalized)
	res, err := s.client.Eval(ctx, pairingRedeemScript,
		[]string{pairingCodeKey(codeHash), "", pairingGoneKey(codeHash)},
		"", TombstoneTTL.Milliseconds(),
	).Result()
	// The index key needs the owner, which is known only after reading the
	// record — so redeem runs in two steps: consume by code (atomic
	// above), then drop the index entry by owner. A lost race on the
	// second step only leaves a stale index member that prune-on-create
	// and list filtering already tolerate.
	if err != nil {
		return Device{}, "", "", "", storageError(ctx, err)
	}
	recordJSON, ok := res.(string)
	if !ok {
		status, _ := res.(int64)
		if status == -1 {
			return Device{}, "", "", "", ErrGone
		}
		return Device{}, "", "", "", ErrInvalid
	}
	var stored pairingPersisted
	if json.Unmarshal([]byte(recordJSON), &stored) != nil ||
		stored.CodeHash != codeHash || !validOwner(stored.UserID, stored.OrgID) {
		return Device{}, "", "", "", ErrGone
	}
	_ = s.client.ZRem(ctx, pairingIndexKey(stored.UserID), stored.ID+":"+codeHash).Err()
	now := s.now().UTC()
	if !now.Before(stored.ExpiresAt) {
		return Device{}, "", "", "", ErrGone
	}
	token, err := newDeviceToken()
	if err != nil {
		return Device{}, "", "", "", ErrStorage
	}
	rec := newDeviceRecord(stored.UserID, stored.OrgID, plat, hashSecret(token), now)
	data, err := json.Marshal(rec)
	if err != nil {
		return Device{}, "", "", "", ErrStorage
	}
	remaining := rec.ExpiresAt.Sub(now).Milliseconds()
	if remaining <= 0 {
		return Device{}, "", "", "", ErrGone
	}
	for attempt := 0; attempt < 3; attempt++ {
		if attempt > 0 {
			rec.ID = uuid.NewString()
			if data, err = json.Marshal(rec); err != nil {
				return Device{}, "", "", "", ErrStorage
			}
		}
		storedOK, err := s.client.SetNX(ctx, deviceKey(rec.ID), data, time.Duration(remaining)*time.Millisecond).Result()
		if err != nil {
			return Device{}, "", "", "", storageError(ctx, err)
		}
		if !storedOK {
			continue // UUID collision: retry with a fresh id
		}
		if err := s.client.SAdd(ctx, deviceIndexKey(rec.UserID), rec.ID).Err(); err != nil {
			return Device{}, "", "", "", storageError(ctx, err)
		}
		return rec.Device, rec.UserID, rec.OrgID, token, nil
	}
	return Device{}, "", "", "", ErrStorage
}

// Rotation compares the presented hash inside Lua: match swaps in the new
// record (renewing the lifetime), previous-token match revokes the device,
// anything else fails without touching state.
const deviceRotateScript = `
local rec = redis.call('GET', KEYS[1])
if not rec then return 0 end
local obj = cjson.decode(rec)
if obj.token_hash == ARGV[1] then
  redis.call('SET', KEYS[1], ARGV[2], 'PX', ARGV[3])
  return 1
end
if obj.prev_token_hash and obj.prev_token_hash == ARGV[1] then
  redis.call('DEL', KEYS[1])
  redis.call('SREM', KEYS[2], ARGV[4])
  return -1
end
return 0
`

func (s *RedisStore) RotateDeviceToken(ctx context.Context, deviceID, presentedToken string) (string, Device, error) {
	if !validDeviceID(deviceID) || strings.TrimSpace(presentedToken) == "" {
		return "", Device{}, ErrUnauthorized
	}
	if err := ctx.Err(); err != nil {
		return "", Device{}, err
	}
	if s.client == nil {
		return "", Device{}, ErrStorage
	}
	now := s.now().UTC()
	raw, err := s.client.Get(ctx, deviceKey(deviceID)).Result()
	if errors.Is(err, redis.Nil) {
		return "", Device{}, ErrUnauthorized
	}
	if err != nil {
		return "", Device{}, storageError(ctx, err)
	}
	var rec deviceRecord
	if json.Unmarshal([]byte(raw), &rec) != nil || rec.ID != deviceID || !validOwner(rec.UserID, rec.OrgID) || rec.expired(now) {
		return "", Device{}, ErrUnauthorized
	}
	next, err := newDeviceToken()
	if err != nil {
		return "", Device{}, ErrStorage
	}
	updated := rec
	updated.PrevTokenHash = rec.TokenHash
	updated.TokenHash = hashSecret(next)
	updated.LastSeenAt = now
	updated.ExpiresAt = now.Add(DeviceLifetime)
	data, err := json.Marshal(updated)
	if err != nil {
		return "", Device{}, ErrStorage
	}
	remaining := updated.ExpiresAt.Sub(now).Milliseconds()
	if remaining <= 0 {
		return "", Device{}, ErrUnauthorized
	}
	res, err := s.client.Eval(ctx, deviceRotateScript,
		[]string{deviceKey(deviceID), deviceIndexKey(rec.UserID)},
		hashSecret(strings.TrimSpace(presentedToken)), string(data), remaining, rec.ID,
	).Int64()
	if err != nil {
		return "", Device{}, storageError(ctx, err)
	}
	switch res {
	case 1:
		return next, updated.Device, nil
	case -1:
		publishDeviceRevocation(ctx, s.client, rec.UserID, rec.ID)
		return "", rec.Device, ErrRevoked
	default:
		return "", Device{}, ErrUnauthorized
	}
}

// Revoke deletes only the exact validated record; a concurrent rotation
// cannot make this delete an unvalidated newer row.
const deviceRevokeScript = `
if redis.call('GET', KEYS[1]) == ARGV[1] then
  redis.call('DEL', KEYS[1])
  redis.call('SREM', KEYS[2], ARGV[2])
  return 1
end
return 0
`

func (s *RedisStore) RevokeDevice(ctx context.Context, userID, deviceID string) error {
	if !validIdentity(userID, false) || !validDeviceID(deviceID) {
		return ErrNotFound
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if s.client == nil {
		return ErrStorage
	}
	raw, err := s.client.Get(ctx, deviceKey(deviceID)).Result()
	if errors.Is(err, redis.Nil) {
		return ErrNotFound
	}
	if err != nil {
		return storageError(ctx, err)
	}
	var rec deviceRecord
	if json.Unmarshal([]byte(raw), &rec) != nil || rec.ID != deviceID || rec.UserID != userID {
		return ErrNotFound
	}
	removed, err := s.client.Eval(ctx, deviceRevokeScript,
		[]string{deviceKey(deviceID), deviceIndexKey(rec.UserID)}, raw, rec.ID).Int64()
	if err != nil {
		return storageError(ctx, err)
	}
	if removed != 1 {
		return ErrNotFound
	}
	publishDeviceRevocation(ctx, s.client, rec.UserID, rec.ID)
	return nil
}

// publishDeviceRevocation tells every API instance to close the revoked
// phone's relay sockets now (one PUBLISH per revocation, instead of every
// socket polling the device record). Best-effort: the relay's slow safety
// poll still ends the sockets if the message is lost.
func publishDeviceRevocation(ctx context.Context, client *redis.Client, userID, deviceID string) {
	companionsession.PublishRevocation(ctx, client, companionsession.Revocation{
		Kind: companionsession.RevokedDevice, UserID: userID, DeviceID: deviceID,
	})
}

func (s *RedisStore) ListDevices(ctx context.Context, userID string) ([]Device, error) {
	if !validIdentity(userID, false) {
		return nil, ErrInvalid
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if s.client == nil {
		return nil, ErrStorage
	}
	ids, err := s.client.SMembers(ctx, deviceIndexKey(userID)).Result()
	if err != nil {
		return nil, storageError(ctx, err)
	}
	if len(ids) == 0 {
		return []Device{}, nil
	}
	keys := make([]string, 0, len(ids))
	seen := make(map[string]bool, len(ids))
	for _, id := range ids {
		if !validDeviceID(id) || seen[id] {
			continue
		}
		seen[id] = true
		keys = append(keys, deviceKey(id))
	}
	if len(keys) == 0 {
		return []Device{}, nil
	}
	raws, err := s.client.MGet(ctx, keys...).Result()
	if err != nil {
		return nil, storageError(ctx, err)
	}
	now := s.now().UTC()
	out := make([]Device, 0, len(raws))
	for _, raw := range raws {
		text, _ := raw.(string)
		if text == "" {
			continue
		}
		var rec deviceRecord
		if json.Unmarshal([]byte(text), &rec) != nil || rec.UserID != userID || rec.expired(now) {
			continue
		}
		out = append(out, rec.Device)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].CreatedAt.Equal(out[j].CreatedAt) {
			return out[i].ID < out[j].ID
		}
		return out[i].CreatedAt.Before(out[j].CreatedAt)
	})
	return out, nil
}

func (s *RedisStore) IsDeviceValid(ctx context.Context, userID, deviceID string) (bool, error) {
	if !validIdentity(userID, false) || !validDeviceID(deviceID) {
		return false, nil
	}
	if err := ctx.Err(); err != nil {
		return false, err
	}
	if s.client == nil {
		return false, ErrStorage
	}
	raw, err := s.client.Get(ctx, deviceKey(deviceID)).Result()
	if errors.Is(err, redis.Nil) {
		return false, nil
	}
	if err != nil {
		return false, storageError(ctx, err)
	}
	var rec deviceRecord
	if json.Unmarshal([]byte(raw), &rec) != nil || rec.UserID != userID || rec.expired(s.now().UTC()) {
		return false, nil
	}
	return true, nil
}

func (s *RedisStore) DeviceE2E(ctx context.Context, userID, deviceID string) (bool, bool, error) {
	if !validIdentity(userID, false) || !validDeviceID(deviceID) {
		return false, false, nil
	}
	if err := ctx.Err(); err != nil {
		return false, false, err
	}
	if s.client == nil {
		return false, false, ErrStorage
	}
	raw, err := s.client.Get(ctx, deviceKey(deviceID)).Result()
	if errors.Is(err, redis.Nil) {
		return false, false, nil
	}
	if err != nil {
		return false, false, storageError(ctx, err)
	}
	var rec deviceRecord
	if json.Unmarshal([]byte(raw), &rec) != nil || rec.UserID != userID || rec.expired(s.now().UTC()) {
		return false, false, nil
	}
	return true, rec.E2E, nil
}

func storageError(ctx context.Context, err error) error {
	if ctxErr := ctx.Err(); ctxErr != nil {
		return ctxErr
	}
	return ErrStorage
}

var _ Store = (*RedisStore)(nil)
