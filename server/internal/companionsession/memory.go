package companionsession

import (
	"context"
	"sync"
	"time"
)

// MemoryStore bounds active leases per account. Expired leases are removed
// whenever that account creates or resumes a lease; it is intended for tests
// and single-instance development. Production should use RedisStore.
type MemoryStore struct {
	mu     sync.Mutex
	byUser map[string]map[string]Lease
	now    func() time.Time
}

func NewMemoryStore(now ...func() time.Time) *MemoryStore {
	return &MemoryStore{byUser: make(map[string]map[string]Lease), now: storeClock(now)}
}

func (s *MemoryStore) Create(ctx context.Context, userID, deviceID string) (Lease, error) {
	if !validOwner(userID, deviceID) {
		return Lease{}, ErrInvalid
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return Lease{}, err
	}
	now := s.now()
	leases := s.prune(userID, now)
	if len(leases) >= MaxPerUser {
		return Lease{}, ErrLimit
	}
	lease, err := createLease(userID, deviceID, now)
	if err != nil {
		return Lease{}, err
	}
	if leases == nil {
		leases = make(map[string]Lease)
		s.byUser[userID] = leases
	}
	leases[lease.ID] = lease
	return lease, nil
}

func (s *MemoryStore) Resume(ctx context.Context, id, userID, deviceID string) (Lease, error) {
	if !validOwner(userID, deviceID) || !validID(id) {
		return Lease{}, ErrUnavailable
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return Lease{}, err
	}
	now := s.now()
	lease, ok := s.prune(userID, now)[id]
	if !ok || !matches(lease, id, userID, deviceID, now) {
		return Lease{}, ErrUnavailable
	}
	return lease, nil
}

func (s *MemoryStore) Revoke(ctx context.Context, id, userID, deviceID string) error {
	if !validOwner(userID, deviceID) || !validID(id) {
		return ErrUnavailable
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return err
	}
	now := s.now()
	leases := s.prune(userID, now)
	lease, ok := leases[id]
	if !ok || !matches(lease, id, userID, deviceID, now) {
		return ErrUnavailable
	}
	delete(leases, id)
	if len(leases) == 0 {
		delete(s.byUser, userID)
	}
	return nil
}

// Called under mu, so Resume and Revoke have one ordering even when concurrent.
func (s *MemoryStore) prune(userID string, now time.Time) map[string]Lease {
	leases := s.byUser[userID]
	for id, lease := range leases {
		if !now.Before(lease.ExpiresAt) {
			delete(leases, id)
		}
	}
	if len(leases) == 0 {
		delete(s.byUser, userID)
		return nil
	}
	return leases
}

var _ Store = (*MemoryStore)(nil)
