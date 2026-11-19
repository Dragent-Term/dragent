// Package companionpairing implements phone pairing: a signed-in desktop
// shows a one-time code as a QR, the phone redeems it once for a device
// credential, and the credential rotates into short-lived access tokens.
// Transport is always the relay; same-Wi-Fi direct is a later phase.
//
// Storage mirrors server/internal/companionsession: Redis in production,
// an in-memory store for tests. Error codes intentionally stay inside the
// contract's set: invalid (400), unauthorized (401), gone (410),
// rate-limited (429).
package companionpairing

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base32"
	"encoding/hex"
	"errors"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
)

const (
	// PairingTTL bounds one pairing code's life. The desktop re-issues on
	// expiry; the phone redeems at most once inside it.
	PairingTTL = 10 * time.Minute
	// MaxLivePairingsPerUser caps live codes per account. Creating beyond
	// the cap drops the oldest live pairing first.
	MaxLivePairingsPerUser = 5
	// DeviceLifetime bounds a paired phone credential. Every token rotation
	// renews it; a phone that never refreshes lapses on its own.
	DeviceLifetime = 30 * 24 * time.Hour
	// CodeLength is the pairing code length: 12 crypto-random bytes as
	// base32 without padding (no 0/O/1/I ambiguity for manual entry).
	CodeLength = 20
	// TombstoneTTL keeps consumed/expired code markers so a second redeem
	// reports gone (410) instead of invalid (400) within a day. Markers
	// are tiny and keyed by code hash only.
	TombstoneTTL = 24 * time.Hour
)

var (
	// ErrInvalid covers malformed bodies, unknown codes and bad
	// credentials. Unknown and foreign ids share it so existence never
	// leaks across accounts.
	ErrInvalid = errors.New("the companion pairing request is invalid")
	// ErrGone covers consumed or expired codes. The phone must ask the
	// desktop for a fresh pairing.
	ErrGone = errors.New("this companion pairing is gone")
	// ErrUnauthorized covers bad device credentials on the token endpoint.
	ErrUnauthorized = errors.New("companion device credentials are invalid")
	// ErrRevoked covers reuse of an already-rotated device token. The
	// device is revoked as compromised; the phone must pair again.
	ErrRevoked = errors.New("this companion device was revoked")
	// ErrRateLimited covers redeem throttling per IP and per code.
	ErrRateLimited = errors.New("too many companion pairing attempts")
	// ErrNotFound covers list/revoke of an unknown device id. Handlers map
	// it to 404; it never escapes as an oracle because foreign ids map
	// the same way.
	ErrNotFound = errors.New("companion device not found")
	// ErrStorage covers unreachable backing storage.
	ErrStorage = errors.New("companion pairing storage is unavailable")
)

// Pairing is a live, unredeemed desktop code. Code itself is never stored;
// only its hash leaves this package's boundary toward the store.
type Pairing struct {
	ID        string    `json:"pairing_id"`
	UserID    string    `json:"user_id"`
	OrgID     string    `json:"org_id"`
	CreatedAt time.Time `json:"created_at"`
	ExpiresAt time.Time `json:"expires_at"`
}

// Device is a paired phone. Token hashes never leave the store.
// E2EE cut-over (§10): the device name stays inside the encrypted pairing
// payload and never reaches the server — Name is always empty. Platform is
// the routing class only ("ios" or "android"). E2E marks devices redeemed
// after the cut-over; pre-cut-over devices are e2e:false and must pair again.
type Device struct {
	ID         string    `json:"device_id"`
	UserID     string    `json:"user_id"`
	OrgID      string    `json:"org_id"`
	Name       string    `json:"name"`
	Platform   string    `json:"platform"`
	E2E        bool      `json:"e2e"`
	CreatedAt  time.Time `json:"created_at"`
	ExpiresAt  time.Time `json:"expires_at"`
	LastSeenAt time.Time `json:"last_seen_at"`
}

// Platforms accepted on redeem. Anything else is a 400.
// E2EE cut-over (§6.2): the phone sends only its platform class, ios or
// android. The web platform is gone.
var validPlatforms = map[string]bool{"ios": true, "android": true}

// Store owns pairing codes and paired-device credentials.
type Store interface {
	// CreatePairing mints one single-use code bound to user+org, dropping
	// the oldest live pairing past MaxLivePairingsPerUser. It returns the
	// pairing and the plaintext code (shown once to the desktop).
	CreatePairing(ctx context.Context, userID, orgID string) (Pairing, string, error)
	// RedeemPairing atomically consumes a code and creates the phone's
	// device credential. Exactly one concurrent redeem wins; the rest see
	// ErrGone. It returns the device, its owner, and the plaintext device
	// token (shown once to the phone).
	// E2EE cut-over (§6.2): the phone sends only its platform class. There
	// is no device-name parameter: names live in the encrypted HS1 payload.
	RedeemPairing(ctx context.Context, code, platform string) (Device, string, string, string, error)
	// RotateDeviceToken swaps the device token for a fresh one (returned
	// plaintext once) and renews the device lifetime. Presenting the
	// immediately-previous token revokes the device: it returns ErrRevoked
	// together with the revoked Device identity so callers can close its
	// relay sockets. Anything else is ErrUnauthorized.
	RotateDeviceToken(ctx context.Context, deviceID, presentedToken string) (string, Device, error)
	// ListDevices returns the account's live devices, oldest first.
	// Devices with equal creation times are ordered by device ID ascending.
	ListDevices(ctx context.Context, userID string) ([]Device, error)
	// RevokeDevice deletes the account's device. Unknown or foreign ids
	// report ErrNotFound so callers map both to 404.
	RevokeDevice(ctx context.Context, userID, deviceID string) error
	// IsDeviceValid reports whether the device may still authenticate:
	// known, owned by userID, unrevoked and unexpired.
	IsDeviceValid(ctx context.Context, userID, deviceID string) (bool, error)
	// DeviceE2E reports the cut-over flag for admission (§10): known
	// reports whether the device exists for this user; e2e reports whether
	// it was redeemed after the cut-over. Unknown devices report
	// known=false. Pre-cut-over devices (known, e2e=false) must pair again.
	DeviceE2E(ctx context.Context, userID, deviceID string) (known bool, e2e bool, err error)
}

func storeClock(clocks []func() time.Time) func() time.Time {
	if len(clocks) > 0 && clocks[0] != nil {
		return clocks[0]
	}
	return time.Now
}

// validOwner mirrors companionsession identity rules: no empty user, no
// whitespace/control, bounded bytes. Org may be empty (users predate org
// resolution); when present it follows the same rules.
func validOwner(userID, orgID string) bool {
	if !validIdentity(userID, false) {
		return false
	}
	return validIdentity(orgID, true)
}

func validIdentity(value string, allowEmpty bool) bool {
	if value == "" {
		return allowEmpty
	}
	if len(value) > 128 || !utf8.ValidString(value) || strings.TrimSpace(value) != value {
		return false
	}
	for _, ch := range value {
		if ch <= 32 || ch == 127 {
			return false
		}
	}
	return true
}

func validDeviceID(id string) bool {
	if len(id) != 36 {
		return false
	}
	parsed, err := uuid.Parse(id)
	return err == nil && parsed != uuid.Nil && parsed.String() == id
}

// normalizePlatform lowercases and validates the phone platform.
func normalizePlatform(raw string) (string, bool) {
	p := strings.ToLower(strings.TrimSpace(raw))
	return p, validPlatforms[p]
}

// normalizeCode trims and validates pasted/scanned codes. Codes are
// uppercase base32; lowercase input is accepted and normalized.
func normalizeCode(raw string) (string, bool) {
	code := strings.ToUpper(strings.TrimSpace(raw))
	if len(code) != CodeLength {
		return "", false
	}
	for _, ch := range code {
		if !(ch >= 'A' && ch <= 'Z' || ch >= '2' && ch <= '7') {
			return "", false
		}
	}
	return code, true
}

var codeEncoding = base32.StdEncoding.WithPadding(base32.NoPadding)

// newCode generates one 20-char base32 crypto-random code (12 bytes).
func newCode() (string, error) {
	var entropy [12]byte
	if _, err := rand.Read(entropy[:]); err != nil {
		return "", err
	}
	code := codeEncoding.EncodeToString(entropy[:])
	if len(code) != CodeLength {
		return "", ErrStorage
	}
	return code, nil
}

// newDeviceToken generates one opaque 256-bit credential, hex-encoded.
func newDeviceToken() (string, error) {
	var entropy [32]byte
	if _, err := rand.Read(entropy[:]); err != nil {
		return "", err
	}
	return hex.EncodeToString(entropy[:]), nil
}

func hashSecret(secret string) string {
	sum := sha256.Sum256([]byte(secret))
	return hex.EncodeToString(sum[:])
}

// deviceRecord is the stored device row: public fields plus token hashes.
// The previous-token hash exists only between a rotation and its first
// use; presenting it revokes the device as compromised.
type deviceRecord struct {
	Device
	TokenHash     string `json:"token_hash"`
	PrevTokenHash string `json:"prev_token_hash,omitempty"`
}

func (r deviceRecord) expired(now time.Time) bool {
	return !now.Before(r.ExpiresAt)
}

func newPairing(userID, orgID string, now time.Time) Pairing {
	id := uuid.NewString()
	now = now.UTC()
	return Pairing{
		ID:        id,
		UserID:    userID,
		OrgID:     orgID,
		CreatedAt: now,
		ExpiresAt: now.Add(PairingTTL),
	}
}

func newDeviceRecord(userID, orgID, platform, tokenHash string, now time.Time) deviceRecord {
	id := uuid.NewString()
	now = now.UTC()
	return deviceRecord{
		Device: Device{
			ID:         id,
			UserID:     userID,
			OrgID:      orgID,
			Name:       "",
			Platform:   platform,
			E2E:        true,
			CreatedAt:  now,
			ExpiresAt:  now.Add(DeviceLifetime),
			LastSeenAt: now,
		},
		TokenHash: tokenHash,
	}
}
