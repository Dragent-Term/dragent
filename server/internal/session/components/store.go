package components

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
	"time"

	"github.com/redis/go-redis/v9"
)

// How long a session record and a share link live.
const (
	SessionTTL   = 12 * time.Hour
	ShareLinkTTL = 24 * time.Hour
)

// Access levels a share link may grant on join.
const (
	AccessViewOnly = "view_only"
	AccessControl  = "control"
)

// ShareLink is a revocable, session-scoped capability handed to a teammate. The
// token in the URL resolves (server-side) to the session + secret needed to join.
type ShareLink struct {
	SessionID          string `json:"session_id"`
	Secret             string `json:"secret"`
	Access             string `json:"access"`
	InvitedEmail       string `json:"invited_email,omitempty"`
	InvitedDisplayName string `json:"invited_display_name,omitempty"`
	InvitedAvatarURL   string `json:"invited_avatar_url,omitempty"`
}

// SessionRecord is the server's authoritative record of a shared session.
type SessionRecord struct {
	ID             string `json:"id"`
	Secret         string `json:"secret"`          // capability required to join
	ReconnectToken string `json:"reconnect_token"` // host-only reattach token
	HostUser       string `json:"host_user"`       // owning account
}

// SessionStore persists session records and revocable share links. Backed by
// Redis in production and an in-memory map in tests / single-instance dev.
type SessionStore interface {
	CreateSession(ctx context.Context, rec SessionRecord) error
	GetSession(ctx context.Context, sessionID string) (SessionRecord, bool, error)
	DeleteSession(ctx context.Context, sessionID string) error
	PutShareLink(ctx context.Context, token string, link ShareLink) error
	ResolveShareLink(ctx context.Context, token string) (ShareLink, bool, error)
	DeleteShareLink(ctx context.Context, token string) error
}

// ── Redis-backed store ───────────────────────────────────────────────────────

type RedisStore struct {
	rdb *redis.Client
}

func NewRedisStore(rdb *redis.Client) *RedisStore { return &RedisStore{rdb: rdb} }

func sessionKey(id string) string  { return "session:" + id + ":rec" }
func shareKey(token string) string { return "sharelink:" + token }

func (s *RedisStore) CreateSession(ctx context.Context, rec SessionRecord) error {
	data, err := json.Marshal(rec)
	if err != nil {
		return err
	}
	return s.rdb.Set(ctx, sessionKey(rec.ID), data, SessionTTL).Err()
}

func (s *RedisStore) GetSession(ctx context.Context, sessionID string) (SessionRecord, bool, error) {
	data, err := s.rdb.Get(ctx, sessionKey(sessionID)).Bytes()
	if errors.Is(err, redis.Nil) {
		return SessionRecord{}, false, nil
	}
	if err != nil {
		return SessionRecord{}, false, err
	}
	var rec SessionRecord
	if err := json.Unmarshal(data, &rec); err != nil {
		return SessionRecord{}, false, err
	}
	return rec, true, nil
}

func (s *RedisStore) DeleteSession(ctx context.Context, sessionID string) error {
	return s.rdb.Del(ctx, sessionKey(sessionID)).Err()
}

func (s *RedisStore) PutShareLink(ctx context.Context, token string, link ShareLink) error {
	data, err := json.Marshal(link)
	if err != nil {
		return err
	}
	return s.rdb.Set(ctx, shareKey(token), data, ShareLinkTTL).Err()
}

func (s *RedisStore) ResolveShareLink(ctx context.Context, token string) (ShareLink, bool, error) {
	data, err := s.rdb.Get(ctx, shareKey(token)).Bytes()
	if errors.Is(err, redis.Nil) {
		return ShareLink{}, false, nil
	}
	if err != nil {
		return ShareLink{}, false, err
	}
	var link ShareLink
	if err := json.Unmarshal(data, &link); err != nil {
		return ShareLink{}, false, err
	}
	return link, true, nil
}

func (s *RedisStore) DeleteShareLink(ctx context.Context, token string) error {
	return s.rdb.Del(ctx, shareKey(token)).Err()
}

// ── In-memory store (tests / single-instance) ────────────────────────────────

type MemoryStore struct {
	mu       sync.Mutex
	sessions map[string]SessionRecord
	links    map[string]linkEntry
	now      func() time.Time
}

type linkEntry struct {
	link    ShareLink
	expires time.Time
}

func NewMemoryStore() *MemoryStore {
	return &MemoryStore{
		sessions: make(map[string]SessionRecord),
		links:    make(map[string]linkEntry),
		now:      time.Now,
	}
}

func (s *MemoryStore) CreateSession(_ context.Context, rec SessionRecord) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.sessions[rec.ID] = rec
	return nil
}

func (s *MemoryStore) GetSession(_ context.Context, sessionID string) (SessionRecord, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rec, ok := s.sessions[sessionID]
	return rec, ok, nil
}

func (s *MemoryStore) DeleteSession(_ context.Context, sessionID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.sessions, sessionID)
	return nil
}

func (s *MemoryStore) PutShareLink(_ context.Context, token string, link ShareLink) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.links[token] = linkEntry{link: link, expires: s.now().Add(ShareLinkTTL)}
	return nil
}

func (s *MemoryStore) ResolveShareLink(_ context.Context, token string) (ShareLink, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	e, ok := s.links[token]
	if !ok {
		return ShareLink{}, false, nil
	}
	if s.now().After(e.expires) {
		delete(s.links, token)
		return ShareLink{}, false, nil
	}
	return e.link, true, nil
}

func (s *MemoryStore) DeleteShareLink(_ context.Context, token string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.links, token)
	return nil
}
