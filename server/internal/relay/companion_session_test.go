package relay

import (
	"context"
	"encoding/json"
	"errors"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"

	"server/internal/companionsession"
)

// companionSocketServer builds a socket server with the 24h lease store
// wired (memory store, injectable clock) and a token authenticator. It
// exercises the real admission path: first-frame auth, lease mint/resume,
// ready fields, deadlines, and revocation polling.
func companionSocketServer(t *testing.T, now func() time.Time, auth SocketAuthenticator) (*httptest.Server, *RPCHandler, *companionsession.MemoryStore) {
	t.Helper()
	h := NewRPCHandler(nil, nil)
	h.ConfigureSocket(auth, NewMemoryStore(), nil)
	sessions := companionsession.NewMemoryStore(now)
	h.SetCompanionSessions(sessions)
	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	srv := httptest.NewServer(r)
	t.Cleanup(srv.Close)
	return srv, h, sessions
}

func fixedAuth(user string, expiresAt time.Time) SocketAuthenticator {
	return func(_ context.Context, token string) (SocketIdentity, error) {
		if token != "test-account-token" && token != "test-account-token-b" && token != "test-account-token-refresh" {
			return SocketIdentity{}, errors.New("bad token")
		}
		u := user
		if token == "test-account-token-b" {
			u = "account-b"
		}
		return SocketIdentity{UserID: u, ExpiresAt: expiresAt}, nil
	}
}

// socketReadRaw reads one frame without failing on close errors, so tests
// can assert the close code that ended the socket.
func socketReadRaw(t *testing.T, ctx context.Context, c *websocket.Conn) (websocket.MessageType, []byte, error) {
	t.Helper()
	return c.Read(ctx)
}

func decodeFrame(t *testing.T, raw []byte) map[string]any {
	t.Helper()
	var frame map[string]any
	if err := json.Unmarshal(raw, &frame); err != nil {
		t.Fatal(err)
	}
	return frame
}

// readCloseCode drains frames until the socket closes and returns the close
// code. Ready/error frames before the close are skipped.
func readCloseCode(t *testing.T, ctx context.Context, c *websocket.Conn) websocket.StatusCode {
	t.Helper()
	for {
		_, _, err := socketReadRaw(t, ctx, c)
		if err == nil {
			continue
		}
		var closeErr websocket.CloseError
		if !errors.As(err, &closeErr) {
			t.Fatalf("socket ended without a close frame: %v", err)
		}
		return closeErr.Code
	}
}

func TestCompanionAdmissionMintsFixedSession(t *testing.T) {
	now := time.Now()
	srv, _, _ := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	ready := socketRead(t, ctx, c)
	if ready["type"] != "relay-ready" {
		t.Fatalf("admission: %v", ready)
	}
	sessionID, _ := ready["session_id"].(string)
	if len(sessionID) != 36 {
		t.Fatalf("ready missing opaque session_id: %v", ready)
	}
	sessionExp, _ := ready["session_expires_at"].(float64)
	accessExp, _ := ready["access_expires_at"].(float64)
	if sessionExp <= 0 || accessExp <= 0 {
		t.Fatalf("ready missing expiries: %v", ready)
	}
	// Fixed 24h logical session from creation, independent of the JWT.
	if int64(sessionExp) != now.Add(24*time.Hour).UnixMilli() {
		t.Fatalf("session_expires_at = %v, want creation+24h", sessionExp)
	}
	if int64(accessExp) != now.Add(time.Hour).UnixMilli() {
		t.Fatalf("access_expires_at = %v, want JWT expiry", accessExp)
	}
}

func TestCompanionResumeKeepsFixedExpiry(t *testing.T) {
	now := time.Now()
	srv, _, _ := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	first := socketRead(t, ctx, c)
	sessionID := first["session_id"].(string)
	sessionExp := first["session_expires_at"].(float64)

	// Physical reconnect on the same lease: identical id and absolute expiry.
	c2, ctx2 := openSocket(t, srv)
	socketWrite(t, ctx2, c2, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token-refresh", "device_id": "desktop", "resume_session_id": sessionID})
	second := socketRead(t, ctx2, c2)
	if second["type"] != "relay-ready" {
		t.Fatalf("resume: %v", second)
	}
	if second["session_id"] != sessionID || second["session_expires_at"] != sessionExp {
		t.Fatalf("resume renewed the lease: %v vs %v", first, second)
	}
}

func TestCompanionResumeForeignRejectedWithoutReplacement(t *testing.T) {
	now := time.Now()
	srv, _, sessions := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	first := socketRead(t, ctx, c)
	sessionID := first["session_id"].(string)

	for name, frame := range map[string]map[string]any{
		"foreign user":   {"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token-b", "device_id": "desktop", "resume_session_id": sessionID},
		"foreign device": {"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "other", "resume_session_id": sessionID},
		"malformed id":   {"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop", "resume_session_id": "not-a-uuid"},
	} {
		cc, cctx := openSocket(t, srv)
		socketWrite(t, cctx, cc, frame)
		got := decodeFrame(t, mustReadRaw(t, cctx, cc))
		if got["type"] != "relay-error" {
			t.Fatalf("%s: expected relay-error, got %v", name, got)
		}
		if code := readCloseCode(t, cctx, cc); code != websocket.StatusCode(4408) {
			t.Fatalf("%s: close code = %v, want 4408", name, code)
		}
	}
	// The original lease is untouched by the foreign attempts.
	lease, err := sessions.Resume(context.Background(), sessionID, "account-a", "desktop")
	if err != nil || lease.ID != sessionID {
		t.Fatalf("original lease disturbed: %v %v", lease, err)
	}
}

func mustReadRaw(t *testing.T, ctx context.Context, c *websocket.Conn) []byte {
	t.Helper()
	_, raw, err := c.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

func TestCompanionResumeExpiredRejected(t *testing.T) {
	var current atomic.Int64
	current.Store(time.Now().UnixNano())
	now := func() time.Time { return time.Unix(0, current.Load()) }
	srv, _, _ := companionSocketServer(t, now, fixedAuth("account-a", now().Add(48*time.Hour)))
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	first := socketRead(t, ctx, c)
	sessionID := first["session_id"].(string)

	// Past the fixed 24h deadline the same lease must not resume.
	current.Add(int64(25 * time.Hour))
	cc, cctx := openSocket(t, srv)
	socketWrite(t, cctx, cc, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop", "resume_session_id": sessionID})
	got := decodeFrame(t, mustReadRaw(t, cctx, cc))
	if got["type"] != "relay-error" {
		t.Fatalf("expected relay-error, got %v", got)
	}
	if code := readCloseCode(t, cctx, cc); code != websocket.StatusCode(4408) {
		t.Fatalf("close code = %v, want 4408", code)
	}
}

func TestCompanionSessionDeadlineEndsSocket(t *testing.T) {
	// Lease created 24h-150ms ago: the session deadline fires in ~150ms of wall time.
	frozen := time.Now().Add(-24*time.Hour + 150*time.Millisecond)
	srv, _, _ := companionSocketServer(t, func() time.Time { return frozen }, fixedAuth("account-a", time.Now().Add(time.Hour)))
	c, _ := openSocket(t, srv)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token"})
	if got := socketRead(t, ctx, c); got["type"] != "relay-ready" {
		t.Fatalf("admission: %v", got)
	}
	if code := readCloseCode(t, ctx, c); code != websocket.StatusCode(4408) {
		t.Fatalf("close code = %v, want 4408 at session deadline", code)
	}
}

func TestCompanionJWTDeadlineStillBinds(t *testing.T) {
	now := time.Now()
	// The JWT dies in ~150ms while the lease runs 24h: the credential bound wins.
	srv, _, _ := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(150*time.Millisecond)))
	c, _ := openSocket(t, srv)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token"})
	if got := socketRead(t, ctx, c); got["type"] != "relay-ready" || got["session_id"] == nil {
		t.Fatalf("admission: %v", got)
	}
	if code := readCloseCode(t, ctx, c); code != websocket.StatusCode(4401) {
		t.Fatalf("close code = %v, want 4401 at JWT expiry", code)
	}
}

func TestCompanionRevokeEndsSocketAtBoundedInterval(t *testing.T) {
	now := time.Now()
	srv, h, sessions := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	h.sessionPollInterval = 20 * time.Millisecond
	c, _ := openSocket(t, srv)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	ready := socketRead(t, ctx, c)
	sessionID := ready["session_id"].(string)
	if err := sessions.Revoke(context.Background(), sessionID, "account-a", "desktop"); err != nil {
		t.Fatal(err)
	}
	if code := readCloseCode(t, ctx, c); code != websocket.StatusCode(4408) {
		t.Fatalf("close code = %v, want 4408 after revoke", code)
	}
}

func TestCompanionLegacyAuthCreatesDiscoveryLease(t *testing.T) {
	// Existing auth frames remain accepted, with additive lifetime fields.
	srv, h, _ := socketServer(t)
	h.SetCompanionSessions(nil) // nil cannot disable the lifetime boundary.
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token"})
	ready := socketRead(t, ctx, c)
	id, ok := ready["session_id"].(string)
	if ready["type"] != "relay-ready" || !ok {
		t.Fatalf("legacy admission: %v", ready)
	}
	lease, err := h.sessions.Resume(context.Background(), id, "account-a", "")
	if err != nil || lease.DeviceID != "" {
		t.Fatalf("legacy discovery lease: %+v %v", lease, err)
	}
}

// A session store that stops answering once switched.
type flakySessions struct {
	*companionsession.MemoryStore
	failing atomic.Bool
}

func (f *flakySessions) Resume(ctx context.Context, id, userID, deviceID string) (companionsession.Lease, error) {
	if f.failing.Load() {
		return companionsession.Lease{}, companionsession.ErrStorage
	}
	return f.MemoryStore.Resume(ctx, id, userID, deviceID)
}

// Storage not answering keeps the phone connected: requests in the
// meantime are refused, the socket isn't closed (it used to be, on the next
// 10 s check, so phones kept reconnecting).
func TestSessionStorageErrorKeepsSocket(t *testing.T) {
	now := time.Now()
	srv, h, sessions := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	flaky := &flakySessions{MemoryStore: sessions}
	h.SetCompanionSessions(flaky)
	h.sessionPollInterval = 20 * time.Millisecond

	c, _ := openSocket(t, srv)
	admitPhone(t, c, "test-account-token")
	flaky.failing.Store(true)
	time.Sleep(200 * time.Millisecond) // several failed checks

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, socketRequest("tabs.list", "phone-1"))
	if got := socketRead(t, ctx, c); got["ok"] != false {
		t.Fatalf("request served without a confirmed session: %v", got)
	}
	socketWrite(t, ctx, c, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, ctx, c); got["type"] != "relay-pong" {
		t.Fatalf("phone dropped on a storage error: %v", got)
	}
}
