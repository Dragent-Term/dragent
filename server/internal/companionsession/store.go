// Package companionsession owns fixed-duration mobile companion leases.
// A lease is separate from a shared-terminal link or an account access token:
// callers must authenticate the account before attempting to resume it.
package companionsession

import (
	"context"
	"errors"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/google/uuid"
)

const (
	Duration         = 24 * time.Hour
	MaxPerUser       = 32
	maxIdentityBytes = 128
)

// Lookup failures intentionally share one message. An id must not reveal
// whether it exists, expired, was revoked, or belongs to another caller.
var (
	ErrInvalid     = errors.New("the companion session request is invalid")
	ErrUnavailable = errors.New("this companion session is unavailable")
	ErrLimit       = errors.New("too many companion sessions are active")
	ErrStorage     = errors.New("companion session storage is unavailable")
)

// Lease's deadline is fixed at creation. Neither resume nor socket admission
// refreshes ExpiresAt. DeviceID may be empty for account-wide discovery only.
type Lease struct {
	ID        string    `json:"id"`
	UserID    string    `json:"user_id"`
	DeviceID  string    `json:"device_id"`
	CreatedAt time.Time `json:"created_at"`
	ExpiresAt time.Time `json:"expires_at"`
}

type Store interface {
	Create(ctx context.Context, userID, deviceID string) (Lease, error)
	Resume(ctx context.Context, id, userID, deviceID string) (Lease, error)
	Revoke(ctx context.Context, id, userID, deviceID string) error
}

func storeClock(clocks []func() time.Time) func() time.Time {
	if len(clocks) > 0 && clocks[0] != nil {
		return clocks[0]
	}
	return time.Now
}

func validIdentity(value string, allowEmpty bool) bool {
	if value == "" {
		return allowEmpty
	}
	if len(value) > maxIdentityBytes || !utf8.ValidString(value) || strings.TrimSpace(value) != value {
		return false
	}
	for _, ch := range value {
		if unicode.IsControl(ch) || unicode.IsSpace(ch) {
			return false
		}
	}
	return true
}

func validOwner(userID, deviceID string) bool {
	return validIdentity(userID, false) && validIdentity(deviceID, true)
}

func validID(id string) bool {
	if len(id) != 36 {
		return false
	}
	parsed, err := uuid.Parse(id)
	return err == nil && parsed != uuid.Nil && parsed.String() == id
}

func createLease(userID, deviceID string, now time.Time) (Lease, error) {
	id, err := uuid.NewRandom()
	if err != nil {
		return Lease{}, ErrStorage
	}
	now = now.UTC()
	return Lease{ID: id.String(), UserID: userID, DeviceID: deviceID, CreatedAt: now, ExpiresAt: now.Add(Duration)}, nil
}

// matches checks persisted data as well as ownership. A corrupt record must
// never turn a fixed 24-hour deadline into an unbounded session.
func matches(lease Lease, id, userID, deviceID string, now time.Time) bool {
	return lease.ID == id && validID(lease.ID) && validOwner(lease.UserID, lease.DeviceID) &&
		lease.UserID == userID && lease.DeviceID == deviceID && !lease.CreatedAt.IsZero() &&
		lease.ExpiresAt.Equal(lease.CreatedAt.Add(Duration)) && now.Before(lease.ExpiresAt)
}
