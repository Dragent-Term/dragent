package components

import (
	cryptorand "crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"
)

const (
	MinForwardPort = 1
	MaxForwardPort = 65535
	ForwardTTL     = 12 * time.Hour
)

var ErrForwardNotFound = errors.New("port forward not found")

var forwardNames = []string{
	"amber", "calm", "cedar", "coral", "dawn", "ember", "frost", "hazel",
	"lunar", "maple", "misty", "ocean", "pearl", "quiet", "river", "solar",
}

// PortForward is the gateway's control-plane record for one host-approved
// loopback service. OwnerID is normally an authenticated user+device key; older
// session-scoped clients use their session id during the rolling migration.
type PortForward struct {
	ID        string    `json:"id"`
	Token     string    `json:"-"`
	OwnerID   string    `json:"owner_id"`
	Port      uint16    `json:"port"`
	Label     string    `json:"label"`
	CreatedAt time.Time `json:"created_at"`
	ExpiresAt time.Time `json:"expires_at"`
}

type ForwardRegistry struct {
	mu   sync.Mutex
	byID map[string]PortForward
	now  func() time.Time
}

func NewForwardRegistry() *ForwardRegistry {
	return &ForwardRegistry{byID: make(map[string]PortForward), now: time.Now}
}

func (r *ForwardRegistry) Create(ownerID string, port uint16, label string) (PortForward, error) {
	if port < MinForwardPort || port > MaxForwardPort {
		return PortForward{}, errors.New("forward port is out of range")
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	now := r.now()
	for id, existing := range r.byID {
		if !now.Before(existing.ExpiresAt) {
			delete(r.byID, id)
			continue
		}
		if existing.OwnerID == ownerID && existing.Port == port {
			return existing, nil
		}
	}
	id, err := newForwardID(r.byID)
	if err != nil {
		return PortForward{}, err
	}
	token, err := newForwardToken()
	if err != nil {
		return PortForward{}, err
	}
	f := PortForward{
		ID: id, Token: token, OwnerID: ownerID, Port: port, Label: label,
		CreatedAt: now, ExpiresAt: now.Add(ForwardTTL),
	}
	r.byID[f.ID] = f
	return f, nil
}

func newForwardToken() (string, error) {
	var random [32]byte
	if _, err := cryptorand.Read(random[:]); err != nil {
		return "", fmt.Errorf("generate forward token: %w", err)
	}
	return hex.EncodeToString(random[:]), nil
}

func newForwardID(existing map[string]PortForward) (string, error) {
	var random [4]byte
	for attempt := 0; attempt < 5; attempt++ {
		if _, err := cryptorand.Read(random[:]); err != nil {
			return "", fmt.Errorf("generate forward id: %w", err)
		}
		name := forwardNames[int(random[0])%len(forwardNames)]
		id := name + "-" + hex.EncodeToString(random[1:])
		if _, exists := existing[id]; !exists {
			return id, nil
		}
	}
	return "", errors.New("could not generate a unique forward id")
}

// IsForwardID accepts only the public slug format: name- plus six lowercase
// hexadecimal characters. This prevents ordinary hosts such as beta from being
// mistaken for forwarded applications.
func IsForwardID(id string) bool {
	parts := strings.Split(id, "-")
	if len(parts) != 2 || parts[0] == "" || len(parts[1]) != 6 {
		return false
	}
	if strings.ToLower(parts[1]) != parts[1] {
		return false
	}
	_, err := hex.DecodeString(parts[1])
	return err == nil
}

func (r *ForwardRegistry) Get(id string) (PortForward, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	f, ok := r.byID[id]
	if ok && !r.now().Before(f.ExpiresAt) {
		delete(r.byID, id)
		return PortForward{}, false
	}
	return f, ok
}

// Authorize returns a live forward only when its independent bearer token
// matches. Port forwards deliberately do not inherit a terminal-share link:
// sharing a tab and exposing a local service are separate capabilities.
func (r *ForwardRegistry) Authorize(id, token string) (PortForward, bool) {
	f, ok := r.Get(id)
	if !ok || len(token) != len(f.Token) || subtle.ConstantTimeCompare([]byte(token), []byte(f.Token)) != 1 {
		return PortForward{}, false
	}
	return f, true
}

func (r *ForwardRegistry) Close(id, ownerID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	f, ok := r.byID[id]
	if !ok || f.OwnerID != ownerID {
		return ErrForwardNotFound
	}
	delete(r.byID, id)
	return nil
}

func (r *ForwardRegistry) CloseOwner(ownerID string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for id, f := range r.byID {
		if f.OwnerID == ownerID {
			delete(r.byID, id)
		}
	}
}

func (r *ForwardRegistry) List(ownerID string) []PortForward {
	r.mu.Lock()
	defer r.mu.Unlock()
	now := r.now()
	for id, f := range r.byID {
		if !now.Before(f.ExpiresAt) {
			delete(r.byID, id)
		}
	}
	result := make([]PortForward, 0)
	for _, f := range r.byID {
		if f.OwnerID == ownerID {
			result = append(result, f)
		}
	}
	return result
}
