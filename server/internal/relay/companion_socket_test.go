package relay

import (
	"context"
	"encoding/json"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"server/internal/companionsession"
)

type countedCompanionStore struct {
	companionsession.Store
	creates atomic.Int64
	resumes atomic.Int64
}

func (s *countedCompanionStore) Create(ctx context.Context, user, device string) (companionsession.Lease, error) {
	s.creates.Add(1)
	return s.Store.Create(ctx, user, device)
}
func (s *countedCompanionStore) Resume(ctx context.Context, id, user, device string) (companionsession.Lease, error) {
	s.resumes.Add(1)
	return s.Store.Resume(ctx, id, user, device)
}

func TestCompanionJWTPrecedesAllLeaseAccess(t *testing.T) {
	now := time.Now()
	srv, h, sessions := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	store := &countedCompanionStore{Store: sessions}
	h.SetCompanionSessions(store)
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "invalid-token", "resume_session_id": uuid.NewString(), "device_id": "desktop"})
	got := socketRead(t, ctx, c)
	if got["loginRequired"] != true || readCloseCode(t, ctx, c) != websocket.StatusCode(4401) {
		t.Fatal(got)
	}
	if store.creates.Load() != 0 || store.resumes.Load() != 0 {
		t.Fatal("lease store reached before JWT verification")
	}
}

func TestCompanionInvalidResumeNeverMintsReplacement(t *testing.T) {
	for name, value := range map[string]any{"null": nil, "empty": "", "wrong type": 42, "unknown": uuid.NewString()} {
		t.Run(name, func(t *testing.T) {
			now := time.Now()
			srv, h, sessions := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
			store := &countedCompanionStore{Store: sessions}
			h.SetCompanionSessions(store)
			c, ctx := openSocket(t, srv)
			socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "resume_session_id": value})
			got := socketRead(t, ctx, c)
			if got["type"] != "relay-error" || readCloseCode(t, ctx, c) != companionSessionEndedCode {
				t.Fatal(got)
			}
			if store.creates.Load() != 0 {
				t.Fatal("invalid resume minted a replacement lease")
			}
		})
	}
}

func TestCompanionBoundDeviceCannotChangeThroughRPCOrStream(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	for _, device := range []string{"selected", "other"} {
		owner, _ := ownerKey("account-a", device)
		_ = h.states.Publish(context.Background(), owner, HostState{Tabs: []TabEntry{{ID: 1, Kind: TabKindTerminal}}})
	}
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "selected"})
	if socketRead(t, ctx, c)["type"] != "relay-ready" {
		t.Fatal("not admitted")
	}
	for _, frame := range []map[string]any{
		socketRequest("tabs.list", "other"),
		{"type": "rpc-subscribe", "v": 1, "id": uuid.NewString(), "stream": "catalog", "params": map[string]any{"device_id": "other"}},
	} {
		socketWrite(t, ctx, c, frame)
		reply := socketRead(t, ctx, c)
		if reply["ok"] != false || reply["id"] != frame["id"] {
			t.Fatalf("cross-device request admitted: %v", reply)
		}
	}
	socketWrite(t, ctx, c, socketRequest("tabs.list", "selected"))
	if got := socketRead(t, ctx, c); got["ok"] != false {
		t.Fatalf("removed method admitted: %v", got)
	} else if got["error"].(map[string]any)["message"] != "this relay method is end-to-end encrypted: update the app" {
		t.Fatalf("wrong gone message: %v", got)
	}
}

func TestCompanionRevokeUsesCurrentLeaseAndAcceptsInjectedDevice(t *testing.T) {
	now := time.Now()
	srv, _, sessions := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	other, err := sessions.Create(context.Background(), "account-a", "other")
	if err != nil {
		t.Fatal(err)
	}
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	ready := socketRead(t, ctx, c)
	id := ready["session_id"].(string)
	frame := socketRequest("companion.session.revoke", "desktop")
	frame["params"] = map[string]any{"device_id": "desktop", "session_id": other.ID}
	socketWrite(t, ctx, c, frame)
	if got := socketRead(t, ctx, c); got["ok"] != false {
		t.Fatalf("caller-selected session accepted: %v", got)
	}
	if _, err := sessions.Resume(context.Background(), id, "account-a", "desktop"); err != nil {
		t.Fatal("invalid revoke disturbed active lease", err)
	}
	frame = socketRequest("companion.session.revoke", "desktop")
	socketWrite(t, ctx, c, frame)
	reply := socketRead(t, ctx, c)
	if reply["ok"] != true || reply["id"] != frame["id"] {
		t.Fatalf("revoke failed: %v", reply)
	}
	if readCloseCode(t, ctx, c) != companionSessionEndedCode {
		t.Fatal("revoke did not close with4408")
	}
	if _, err := sessions.Resume(context.Background(), id, "account-a", "desktop"); err == nil {
		t.Fatal("revoked session resumed")
	}
	if _, err := sessions.Resume(context.Background(), other.ID, "account-a", "other"); err != nil {
		t.Fatal("revoke disturbed another lease", err)
	}
}

func TestCompanionPollingCancellationIsLocalAndDoesNotDetachStoreContext(t *testing.T) {
	h := NewRPCHandler(nil, nil)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	stop := h.watchCompanionAccess(ctx, companionsession.Lease{}, SocketIdentity{}, func(websocket.StatusCode, string) { t.Error("canceled poll ended access") })
	stop()
}

func TestCompanionOptionalFieldPresenceDistinguishesMissingFromInvalid(t *testing.T) {
	if got, err := companionAuthField(nil, false); err != nil || got != "" {
		t.Fatal(got, err)
	}
	for _, raw := range []string{`null`, `""`, `{}`, `123`} {
		if _, err := companionAuthField(json.RawMessage(raw), false); err == nil {
			t.Fatalf("invalid resume admitted: %s", raw)
		}
	}
}
