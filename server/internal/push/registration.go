package push

import (
	"context"
	"errors"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

// Platforms a registration may target.
const (
	PlatformAPNs = "apns"
	PlatformFCM  = "fcm"
)

// RegistrationTTL is how long a push registration stays live. Every register
// and every rotation renews the lease to now+TTL, so an actively used device
// never lapses while an uninstalled/forgotten one ages out and is cleaned up.
const RegistrationTTL = 90 * 24 * time.Hour

// maxPushTokenBytes bounds stored tokens. FCM tokens are ~163 chars and APNs
// device tokens 64 bytes hex; 4 KiB leaves headroom without inviting abuse.
const maxPushTokenBytes = 4096

var (
	// ErrNotFound is returned when no registration exists for a device.
	ErrNotFound = errors.New("push: registration not found")
	// ErrTokenMismatch is returned when a rotation's old token does not match
	// the stored token — the caller holds a stale token and must re-register.
	ErrTokenMismatch = errors.New("push: token mismatch")
	// ErrUnknownDevice is returned when the device is not in the devices table
	// (or belongs to another user).
	ErrUnknownDevice = errors.New("push: unknown device")
)

// Registration is one device's push lease. DeviceID is the companion
// phone's pairing device id (verified against the companion pairing store
// at registration, not a Postgres FK since migration 00063); unregister
// removes only the push lease.
type Registration struct {
	DeviceID   uuid.UUID
	UserID     uuid.UUID
	Platform   string
	PushToken  string
	Foreground bool
	UpdatedAt  time.Time
	ExpiresAt  time.Time
}

// Expired reports whether the lease has lapsed as of now.
func (r Registration) Expired(now time.Time) bool {
	return !r.ExpiresAt.After(now)
}

func normalizePlatform(p string) string {
	return strings.ToLower(strings.TrimSpace(p))
}

// ValidateRegistration checks a registration's fields before storage.
func ValidateRegistration(r Registration) error {
	if r.DeviceID == uuid.Nil {
		return errors.New("push: device_id is required")
	}
	if r.UserID == uuid.Nil {
		return errors.New("push: user_id is required")
	}
	switch normalizePlatform(r.Platform) {
	case PlatformAPNs, PlatformFCM:
	default:
		return errors.New("push: platform must be apns or fcm")
	}
	token := strings.TrimSpace(r.PushToken)
	if token == "" {
		return errors.New("push: push_token is required")
	}
	if len(token) > maxPushTokenBytes {
		return errors.New("push: push_token is too long")
	}
	return nil
}

// Store persists push registrations.
type Store interface {
	// Upsert creates or renews a registration, extending its expiry.
	Upsert(ctx context.Context, r Registration) error
	// Get returns the registration for a device.
	Get(ctx context.Context, deviceID uuid.UUID) (Registration, bool, error)
	// ListForUser returns the user's live (unexpired) registrations.
	ListForUser(ctx context.Context, userID uuid.UUID) ([]Registration, error)
	// Delete removes a registration durably. It returns ErrNotFound when
	// there is nothing to remove (so double-unregister is observable).
	Delete(ctx context.Context, userID, deviceID uuid.UUID) error
	// DeleteExpired removes lapsed leases and reports how many went away.
	// Run it from a periodic cleanup job.
	DeleteExpired(ctx context.Context, now time.Time) (int64, error)
	// Rotate atomically swaps the push token iff the stored token equals
	// oldToken (foreground rotation compare-and-swap) and renews the lease.
	// It returns ErrNotFound when no registration exists and ErrTokenMismatch
	// when the caller holds a stale token.
	Rotate(ctx context.Context, userID, deviceID uuid.UUID, oldToken, newToken string, foreground bool) error
}

// DeviceResolver answers whether a device row exists and belongs to a user.
// Registrations are only accepted for known, owned devices.
type DeviceResolver interface {
	Exists(ctx context.Context, userID, deviceID uuid.UUID) (bool, error)
}

// ── In-memory store (tests / single-instance dev) ────────────────────────────

type memoryDeviceResolver struct {
	mu    sync.Mutex
	owned map[uuid.UUID]uuid.UUID // device -> user
}

func newMemoryDeviceResolver() *memoryDeviceResolver {
	return &memoryDeviceResolver{owned: make(map[uuid.UUID]uuid.UUID)}
}

func (r *memoryDeviceResolver) add(userID, deviceID uuid.UUID) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.owned[deviceID] = userID
}

func (r *memoryDeviceResolver) Exists(_ context.Context, userID, deviceID uuid.UUID) (bool, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	owner, ok := r.owned[deviceID]
	return ok && owner == userID, nil
}

// MemoryStore is an in-memory Store with the same semantics as PostgresStore.
type MemoryStore struct {
	mu   sync.Mutex
	regs map[uuid.UUID]Registration
	now  func() time.Time
}

// NewMemoryStore returns an empty store using the real clock.
func NewMemoryStore() *MemoryStore {
	return &MemoryStore{regs: make(map[uuid.UUID]Registration), now: time.Now}
}

func (s *MemoryStore) Upsert(_ context.Context, r Registration) error {
	if err := ValidateRegistration(r); err != nil {
		return err
	}
	now := s.now()
	r.Platform = normalizePlatform(r.Platform)
	r.PushToken = strings.TrimSpace(r.PushToken)
	r.UpdatedAt = now
	r.ExpiresAt = now.Add(RegistrationTTL)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.regs[r.DeviceID] = r
	return nil
}

func (s *MemoryStore) Get(_ context.Context, deviceID uuid.UUID) (Registration, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.regs[deviceID]
	return r, ok, nil
}

func (s *MemoryStore) ListForUser(_ context.Context, userID uuid.UUID) ([]Registration, error) {
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []Registration
	for _, r := range s.regs {
		if r.UserID == userID && !r.Expired(now) {
			out = append(out, r)
		}
	}
	return out, nil
}

func (s *MemoryStore) Delete(_ context.Context, userID, deviceID uuid.UUID) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.regs[deviceID]
	if !ok || r.UserID != userID {
		return ErrNotFound
	}
	delete(s.regs, deviceID)
	return nil
}

func (s *MemoryStore) DeleteExpired(_ context.Context, now time.Time) (int64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var n int64
	for id, r := range s.regs {
		if r.Expired(now) {
			delete(s.regs, id)
			n++
		}
	}
	return n, nil
}

func (s *MemoryStore) Rotate(_ context.Context, userID, deviceID uuid.UUID, oldToken, newToken string, foreground bool) error {
	newToken = strings.TrimSpace(newToken)
	if newToken == "" || len(newToken) > maxPushTokenBytes {
		return errors.New("push: new push_token is invalid")
	}
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.regs[deviceID]
	if !ok || r.UserID != userID {
		return ErrNotFound
	}
	if r.PushToken != oldToken {
		return ErrTokenMismatch
	}
	r.PushToken = newToken
	r.Foreground = foreground
	r.UpdatedAt = now
	r.ExpiresAt = now.Add(RegistrationTTL)
	s.regs[deviceID] = r
	return nil
}

// ── Postgres store (production) ──────────────────────────────────────────────

type pgConn interface {
	Exec(ctx context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

// PostgresStore persists registrations in device_push_registrations — see
// migrations/00061_device_push_registry.sql, plus 00063 which drops the
// devices(id) FK (companion phones have no devices row; the pairing store
// owns them and the pairing handler cleans their registrations on revoke).
type PostgresStore struct {
	db pgConn
}

// NewPostgresStore wires the store to a pool (or transaction).
func NewPostgresStore(pool *pgxpool.Pool) *PostgresStore {
	return &PostgresStore{db: pool}
}

func scanRegistration(row pgx.Row) (Registration, error) {
	var r Registration
	err := row.Scan(&r.DeviceID, &r.UserID, &r.Platform, &r.PushToken, &r.Foreground, &r.UpdatedAt, &r.ExpiresAt, new(time.Time))
	return r, err
}

func (s *PostgresStore) Upsert(ctx context.Context, r Registration) error {
	if err := ValidateRegistration(r); err != nil {
		return err
	}
	now := time.Now()
	_, err := s.db.Exec(ctx, `
		INSERT INTO device_push_registrations
			(device_id, user_id, platform, push_token, foreground, expires_at, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $6)
		ON CONFLICT (device_id) DO UPDATE SET
			user_id = EXCLUDED.user_id,
			platform = EXCLUDED.platform,
			push_token = EXCLUDED.push_token,
			foreground = EXCLUDED.foreground,
			expires_at = EXCLUDED.expires_at,
			updated_at = EXCLUDED.updated_at
	`,
		r.DeviceID, r.UserID, normalizePlatform(r.Platform), strings.TrimSpace(r.PushToken),
		r.Foreground, now.Add(RegistrationTTL),
	)
	return err
}

func (s *PostgresStore) Get(ctx context.Context, deviceID uuid.UUID) (Registration, bool, error) {
	r, err := scanRegistration(s.db.QueryRow(ctx, `
		SELECT device_id, user_id, platform, push_token, foreground,
			updated_at, expires_at, created_at
		FROM device_push_registrations WHERE device_id = $1
	`, deviceID))
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Registration{}, false, nil
		}
		return Registration{}, false, err
	}
	return r, true, nil
}

func (s *PostgresStore) ListForUser(ctx context.Context, userID uuid.UUID) ([]Registration, error) {
	rows, err := s.db.Query(ctx, `
		SELECT device_id, user_id, platform, push_token, foreground,
			updated_at, expires_at, created_at
		FROM device_push_registrations
		WHERE user_id = $1 AND expires_at > NOW()
		ORDER BY updated_at DESC
	`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Registration
	for rows.Next() {
		var r Registration
		var created time.Time
		if err := rows.Scan(&r.DeviceID, &r.UserID, &r.Platform, &r.PushToken, &r.Foreground, &r.UpdatedAt, &r.ExpiresAt, &created); err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

func (s *PostgresStore) Delete(ctx context.Context, userID, deviceID uuid.UUID) error {
	tag, err := s.db.Exec(ctx, `
		DELETE FROM device_push_registrations WHERE device_id = $1 AND user_id = $2
	`, deviceID, userID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *PostgresStore) DeleteExpired(ctx context.Context, now time.Time) (int64, error) {
	tag, err := s.db.Exec(ctx, `DELETE FROM device_push_registrations WHERE expires_at <= $1`, now)
	if err != nil {
		return 0, err
	}
	return tag.RowsAffected(), nil
}

func (s *PostgresStore) Rotate(ctx context.Context, userID, deviceID uuid.UUID, oldToken, newToken string, foreground bool) error {
	newToken = strings.TrimSpace(newToken)
	if newToken == "" || len(newToken) > maxPushTokenBytes {
		return errors.New("push: new push_token is invalid")
	}
	now := time.Now()
	tag, err := s.db.Exec(ctx, `
		UPDATE device_push_registrations
		SET push_token = $4, foreground = $5,
			expires_at = $6, updated_at = $6
		WHERE device_id = $1 AND user_id = $2 AND push_token = $3
	`, deviceID, userID, oldToken, newToken, foreground, now.Add(RegistrationTTL))
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 1 {
		return nil
	}
	// Nothing swapped: distinguish "no such registration" from "stale token".
	_, found, err := s.Get(ctx, deviceID)
	if err != nil {
		return err
	}
	if !found {
		return ErrNotFound
	}
	// A row exists but the token (or owner) did not match. Ownership is
	// enforced by the resolver before this point; surface stale-token here.
	return ErrTokenMismatch
}

// PostgresDeviceResolver checks the existing devices table: the row must
// exist and belong to the registering user.
//
// Superseded for companion phones (FIX1, §8): paired phones live only in
// the companion pairing store, never in devices, so use
// CompanionDeviceResolver instead (wired in server/cmd/api/main.go). Kept
// for compatibility; not wired to any route.
type PostgresDeviceResolver struct {
	db pgConn
}

// NewPostgresDeviceResolver wires the resolver to a pool (or transaction).
func NewPostgresDeviceResolver(pool *pgxpool.Pool) *PostgresDeviceResolver {
	return &PostgresDeviceResolver{db: pool}
}

func (r *PostgresDeviceResolver) Exists(ctx context.Context, userID, deviceID uuid.UUID) (bool, error) {
	var one int
	err := r.db.QueryRow(ctx, `SELECT 1 FROM devices WHERE id = $1 AND user_id = $2`, deviceID, userID).Scan(&one)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return false, nil
		}
		return false, err
	}
	return true, nil
}

var (
	_ Store          = (*MemoryStore)(nil)
	_ Store          = (*PostgresStore)(nil)
	_ DeviceResolver = (*PostgresDeviceResolver)(nil)
)
