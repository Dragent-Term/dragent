package main

// E2EE push-intake wiring test (E9c, §8 steps 2-3): a host-notify blob
// arriving on the desktop socket must reach the push dispatcher, which
// delivers the opaque seal to the phone's live registration. Unconfigured
// (no provider credentials) installs nothing, keeping today's drop-and-count
// behaviour, and the intake constructor never yields a panicking func.

import (
	"context"
	"encoding/json"
	"errors"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/push"
	"server/internal/relay"
)

type recordingPushSender struct {
	mu   sync.Mutex
	sent []push.PushMessage
}

func (s *recordingPushSender) Send(_ context.Context, msg push.PushMessage) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.sent = append(s.sent, msg)
	return nil
}

func (s *recordingPushSender) Name() string { return "recording" }

func (s *recordingPushSender) count() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.sent)
}

func (s *recordingPushSender) last() push.PushMessage {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.sent[len(s.sent)-1]
}

func wsWriteJSON(t *testing.T, ctx context.Context, c *websocket.Conn, v any) {
	t.Helper()
	raw, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	if err := c.Write(ctx, websocket.MessageText, raw); err != nil {
		t.Fatal(err)
	}
}

func wsReadJSON(t *testing.T, ctx context.Context, c *websocket.Conn) map[string]any {
	t.Helper()
	kind, raw, err := c.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if kind != websocket.MessageText {
		t.Fatalf("frame kind = %v, want text", kind)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatal(err)
	}
	return out
}

func TestE2ENotifyIntakeReachesDispatcher(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	store := push.NewMemoryStore()
	accountID, phoneID, desktopID := uuid.New(), uuid.New(), uuid.New()
	if err := store.Upsert(ctx, push.Registration{
		DeviceID: phoneID, UserID: accountID, Platform: "fcm", PushToken: "test-token",
	}); err != nil {
		t.Fatalf("upsert registration: %v", err)
	}
	sender := &recordingPushSender{}
	intake := newE2ENotifyIntake(store, sender)
	if intake == nil {
		t.Fatal("configured intake is nil")
	}

	h := relay.NewRPCHandler(nil, nil)
	catalogs := relay.NewMemoryStore()
	owner := "device\x00" + accountID.String() + "\x00" + desktopID.String()
	if err := catalogs.Put(ctx, owner, relay.Catalog{Tabs: []relay.TabEntry{{ID: 1, Kind: relay.TabKindTerminal}}}); err != nil {
		t.Fatalf("put catalog: %v", err)
	}
	h.ConfigureSocket(func(_ context.Context, token string) (relay.SocketIdentity, error) {
		if token != "host-token" {
			return relay.SocketIdentity{}, errors.New("bad token")
		}
		return relay.SocketIdentity{UserID: accountID.String(), ExpiresAt: time.Now().Add(time.Hour)}, nil
	}, catalogs, nil)
	h.SetE2ENotifyIntake(intake)

	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	h.RegisterHostRoutes(r)
	srv := httptest.NewServer(r)
	t.Cleanup(srv.Close)

	conn, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(srv.URL, "http")+"/v1/relay/host", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.CloseNow()
	wsWriteJSON(t, ctx, conn, map[string]any{
		"type": "relay-host-auth", "v": 1, "e2e": 1,
		"access_token": "host-token", "device_id": desktopID.String(), "expected_user_id": accountID.String(),
	})
	if got := wsReadJSON(t, ctx, conn); got["type"] != "relay-host-ready" {
		t.Fatalf("host admission: %v", got)
	}

	blob := strings.Repeat("A", relay.MaxE2EENotifyBlobChars)
	wsWriteJSON(t, ctx, conn, map[string]any{"type": "host-notify", "v": 1, "phone_id": phoneID.String(), "blob": blob})

	deadline := time.Now().Add(2 * time.Second)
	for sender.count() == 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if sender.count() != 1 {
		t.Fatalf("dispatcher received %d messages, want 1", sender.count())
	}
	msg := sender.last()
	if msg.Blob != blob {
		t.Fatal("dispatched blob differs from the desktop's seal")
	}
	if msg.Token != "test-token" || msg.DeviceID != phoneID || msg.UserID != accountID {
		t.Fatalf("message routed to the wrong registration: %+v", msg)
	}

	// Invalid intake never reaches dispatch and never fails the socket: a
	// live relay-pong proves the desktop socket survived.
	wsWriteJSON(t, ctx, conn, map[string]any{"type": "host-notify", "v": 1, "phone_id": phoneID.String(), "blob": "too-short"})
	wsWriteJSON(t, ctx, conn, map[string]any{"type": "relay-ping", "v": 1})
	if got := wsReadJSON(t, ctx, conn); got["type"] != "relay-pong" {
		t.Fatalf("host socket died on invalid intake: %v", got)
	}
	if n := sender.count(); n != 1 {
		t.Fatalf("invalid intake reached dispatch: %d messages", n)
	}
}

func TestNewE2ENotifyIntakeUnconfigured(t *testing.T) {
	// A nil store or sender never yields an intake, so installing it cannot
	// panic on an unwired dispatcher.
	if newE2ENotifyIntake(nil, &recordingPushSender{}) != nil {
		t.Fatal("nil store must yield no intake")
	}
	if newE2ENotifyIntake(push.NewMemoryStore(), nil) != nil {
		t.Fatal("nil sender must yield no intake")
	}
	// No credentials: unconfigured, so main.go installs nothing and the
	// relay keeps today's drop-and-count behaviour.
	if e2ePushConfigured(push.Config{}) {
		t.Fatal("empty push config must count as unconfigured")
	}
	apnsOnly := push.Config{APNs: push.APNsConfig{KeyID: "k", TeamID: "t", BundleID: "b", PrivateKeyPEM: "p"}}
	if !e2ePushConfigured(apnsOnly) {
		t.Fatal("APNs credentials must count as configured")
	}
	fcmOnly := push.Config{FCM: push.FCMConfig{ProjectID: "p", CredentialsJSON: "{}"}}
	if !e2ePushConfigured(fcmOnly) {
		t.Fatal("FCM credentials must count as configured")
	}
	// No-credential mode builds the aggregate-only log sender, which main.go
	// never installs as an intake.
	if got := push.SenderFromConfig(push.Config{}).Name(); got != "log" {
		t.Fatalf("unconfigured sender = %q, want log mode", got)
	}
}
