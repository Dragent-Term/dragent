package companionpairing

import (
	"context"
	"crypto/subtle"
	"sort"
	"strings"
	"sync"
	"time"
)

// MemoryStore is the single-instance/test Store. Every mutating path holds
// one mutex, so concurrent redeems of one code and concurrent rotations
// are linearizable: exactly one redeem wins, rotation reuse is detected.
type MemoryStore struct {
	mu       sync.Mutex
	now      func() time.Time
	pairings map[string]*pairingEntry // by pairing ID
	byCode   map[string]string        // code hash -> pairing ID
	gone     map[string]time.Time     // code hash -> tombstone expiry
	devices  map[string]*deviceRecord // by device ID
	byUser   map[string]map[string]bool
}

type pairingEntry struct {
	Pairing
	CodeHash string
}

// NewMemoryStore returns an empty store. Production uses RedisStore.
func NewMemoryStore(now ...func() time.Time) *MemoryStore {
	return &MemoryStore{
		now:      storeClock(now),
		pairings: make(map[string]*pairingEntry),
		byCode:   make(map[string]string),
		gone:     make(map[string]time.Time),
		devices:  make(map[string]*deviceRecord),
		byUser:   make(map[string]map[string]bool),
	}
}

func (s *MemoryStore) CreatePairing(ctx context.Context, userID, orgID string) (Pairing, string, error) {
	if !validOwner(userID, orgID) {
		return Pairing{}, "", ErrInvalid
	}
	if err := ctx.Err(); err != nil {
		return Pairing{}, "", err
	}
	for attempt := 0; attempt < 3; attempt++ {
		code, err := newCode()
		if err != nil {
			return Pairing{}, "", ErrStorage
		}
		hash := hashSecret(code)
		s.mu.Lock()
		if _, exists := s.byCode[hash]; exists {
			s.mu.Unlock()
			continue // code-hash collision: retry with fresh entropy
		}
		now := s.now().UTC()
		s.prunePairingsLocked(userID, now)
		s.dropOldestLocked(userID)
		p := newPairing(userID, orgID, now)
		s.pairings[p.ID] = &pairingEntry{Pairing: p, CodeHash: hash}
		s.byCode[hash] = p.ID
		s.gone[hash] = now.Add(TombstoneTTL)
		s.mu.Unlock()
		return p, code, nil
	}
	return Pairing{}, "", ErrStorage
}

func (s *MemoryStore) RedeemPairing(ctx context.Context, code, platform string) (Device, string, string, string, error) {
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
	hash := hashSecret(normalized)
	token, err := newDeviceToken()
	if err != nil {
		return Device{}, "", "", "", ErrStorage
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.now().UTC()
	id, exists := s.byCode[hash]
	entry := s.pairings[id]
	if !exists || entry == nil || !now.Before(entry.ExpiresAt) || entry.CodeHash != hash {
		if expiry, marked := s.gone[hash]; marked && now.Before(expiry) {
			return Device{}, "", "", "", ErrGone
		}
		return Device{}, "", "", "", ErrInvalid
	}
	rec := newDeviceRecord(entry.UserID, entry.OrgID, plat, hashSecret(token), now)
	delete(s.byCode, hash)
	delete(s.pairings, id)
	if s.byUser[rec.UserID] == nil {
		s.byUser[rec.UserID] = make(map[string]bool)
	}
	s.byUser[rec.UserID][rec.ID] = true
	stored := rec
	s.devices[rec.ID] = &stored
	return rec.Device, rec.UserID, rec.OrgID, token, nil
}

func (s *MemoryStore) RotateDeviceToken(ctx context.Context, deviceID, presentedToken string) (string, Device, error) {
	if !validDeviceID(deviceID) || presentedToken == "" {
		return "", Device{}, ErrUnauthorized
	}
	if err := ctx.Err(); err != nil {
		return "", Device{}, err
	}
	next, err := newDeviceToken()
	if err != nil {
		return "", Device{}, ErrStorage
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.now().UTC()
	rec, ok := s.devices[deviceID]
	if !ok || rec.expired(now) {
		return "", Device{}, ErrUnauthorized
	}
	presented := hashSecret(strings.TrimSpace(presentedToken))
	if subtle.ConstantTimeCompare([]byte(rec.TokenHash), []byte(presented)) == 1 {
		rec.PrevTokenHash = rec.TokenHash
		rec.TokenHash = hashSecret(next)
		rec.LastSeenAt = now
		rec.ExpiresAt = now.Add(DeviceLifetime)
		return next, rec.Device, nil
	}
	if rec.PrevTokenHash != "" && subtle.ConstantTimeCompare([]byte(rec.PrevTokenHash), []byte(presented)) == 1 {
		device := rec.Device
		s.deleteDeviceLocked(rec)
		return "", device, ErrRevoked
	}
	return "", Device{}, ErrUnauthorized
}

func (s *MemoryStore) ListDevices(ctx context.Context, userID string) ([]Device, error) {
	if !validIdentity(userID, false) {
		return nil, ErrInvalid
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.now().UTC()
	out := make([]Device, 0, len(s.byUser[userID]))
	for id := range s.byUser[userID] {
		rec, ok := s.devices[id]
		if !ok || rec.UserID != userID || rec.expired(now) {
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

func (s *MemoryStore) RevokeDevice(ctx context.Context, userID, deviceID string) error {
	if !validIdentity(userID, false) || !validDeviceID(deviceID) {
		return ErrNotFound
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	rec, ok := s.devices[deviceID]
	if !ok || rec.UserID != userID {
		return ErrNotFound
	}
	s.deleteDeviceLocked(rec)
	return nil
}

func (s *MemoryStore) IsDeviceValid(ctx context.Context, userID, deviceID string) (bool, error) {
	if !validIdentity(userID, false) || !validDeviceID(deviceID) {
		return false, nil
	}
	if err := ctx.Err(); err != nil {
		return false, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	rec, ok := s.devices[deviceID]
	if !ok || rec.UserID != userID || rec.expired(s.now().UTC()) {
		return false, nil
	}
	return true, nil
}

func (s *MemoryStore) DeviceE2E(ctx context.Context, userID, deviceID string) (bool, bool, error) {
	if !validIdentity(userID, false) || !validDeviceID(deviceID) {
		return false, false, nil
	}
	if err := ctx.Err(); err != nil {
		return false, false, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	rec, ok := s.devices[deviceID]
	if !ok || rec.UserID != userID || rec.expired(s.now().UTC()) {
		return false, false, nil
	}
	return true, rec.E2E, nil
}

func (s *MemoryStore) deleteDeviceLocked(rec *deviceRecord) {
	delete(s.devices, rec.ID)
	if set := s.byUser[rec.UserID]; set != nil {
		delete(set, rec.ID)
		if len(set) == 0 {
			delete(s.byUser, rec.UserID)
		}
	}
}

// prunePairingsLocked drops expired pairings of one user. Tombstones stay
// so expired codes still read as gone until TombstoneTTL lapses.
func (s *MemoryStore) prunePairingsLocked(userID string, now time.Time) {
	for id, entry := range s.pairings {
		if entry.UserID != userID || now.Before(entry.ExpiresAt) {
			continue
		}
		delete(s.pairings, id)
		delete(s.byCode, entry.CodeHash)
	}
}

// dropOldestLocked enforces MaxLivePairingsPerUser by removing the oldest
// live pairing. Its tombstone stays, so the dropped code reads as gone.
func (s *MemoryStore) dropOldestLocked(userID string) {
	var oldest *pairingEntry
	count := 0
	for _, entry := range s.pairings {
		if entry.UserID != userID {
			continue
		}
		count++
		if oldest == nil || entry.CreatedAt.Before(oldest.CreatedAt) {
			oldest = entry
		}
	}
	if count < MaxLivePairingsPerUser || oldest == nil {
		return
	}
	delete(s.pairings, oldest.ID)
	delete(s.byCode, oldest.CodeHash)
}

var _ Store = (*MemoryStore)(nil)
