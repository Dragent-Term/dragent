package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

func socketServer(t *testing.T) (*httptest.Server, *RPCHandler, *MemoryHostStore) {
	t.Helper()
	states := NewMemoryHostStore()
	h := NewRPCHandler(states, nil)
	h.ConfigureSocket(func(_ context.Context, token string) (SocketIdentity, error) {
		if token != "test-account-token" {
			return SocketIdentity{}, errors.New("private detail must not escape")
		}
		return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(time.Minute)}, nil
	}, NewMemoryStore(), nil)
	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	srv := httptest.NewServer(r)
	t.Cleanup(srv.Close)
	return srv, h, states
}
func openSocket(t *testing.T, srv *httptest.Server) (*websocket.Conn, context.Context) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	c, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(srv.URL, "http")+"/v1/relay/socket", nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.CloseNow() })
	return c, ctx
}
func socketWrite(t *testing.T, ctx context.Context, c *websocket.Conn, frame any) {
	t.Helper()
	raw, err := json.Marshal(frame)
	if err != nil {
		t.Fatal(err)
	}
	if err = c.Write(ctx, websocket.MessageText, raw); err != nil {
		t.Fatal(err)
	}
}
func socketRead(t *testing.T, ctx context.Context, c *websocket.Conn) map[string]any {
	t.Helper()
	_, raw, err := c.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	var frame map[string]any
	if err = json.Unmarshal(raw, &frame); err != nil {
		t.Fatal(err)
	}
	return frame
}
func admitSocket(t *testing.T, ctx context.Context, c *websocket.Conn) {
	t.Helper()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token"})
	if got := socketRead(t, ctx, c); got["type"] != "relay-ready" {
		t.Fatalf("admission: %v", got)
	}
}
func socketRequest(method, device string) map[string]any {
	return map[string]any{"type": "rpc-request", "v": 1, "id": uuid.NewString(), "method": method, "params": map[string]any{"device_id": device}}
}

func TestRelaySocketRequiresFirstFrameAuth(t *testing.T) {
	srv, _, _ := socketServer(t)
	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, socketRequest("tabs.list", "desktop"))
	got := socketRead(t, ctx, c)
	if got["type"] != "relay-error" || got["loginRequired"] != true {
		t.Fatalf("unauthenticated RPC admitted: %v", got)
	}
	if strings.Contains(got["message"].(string), "private") {
		t.Fatal("internal auth error leaked")
	}
}
func TestRelaySocketRejectsCredentialsInQuery(t *testing.T) {
	srv, _, _ := socketServer(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	c, resp, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(srv.URL, "http")+"/v1/relay/socket?access_token=test-account-token", nil)
	if c != nil {
		c.CloseNow()
	}
	if err == nil || resp.StatusCode != 400 {
		t.Fatal("query credentials admitted")
	}
}
func TestRelaySocketCorrelatedReadAndOwnerIsolation(t *testing.T) {
	srv, _, _ := socketServer(t)
	c, ctx := openSocket(t, srv)
	admitSocket(t, ctx, c)
	// Content methods are gone after the cut-over: the reply correlates by
	// id and answers 410, for own and foreign devices alike (no oracle).
	request := socketRequest("tabs.list", "desktop")
	socketWrite(t, ctx, c, request)
	reply := socketRead(t, ctx, c)
	if reply["id"] != request["id"] || reply["ok"] != false {
		t.Fatalf("uncorrelated read: %v", reply)
	}
	if reply["error"].(map[string]any)["message"] != "this relay method is end-to-end encrypted: update the app" {
		t.Fatalf("wrong gone message: %v", reply)
	}
	socketWrite(t, ctx, c, socketRequest("catalog.read", "desktop"))
	if got := socketRead(t, ctx, c); got["ok"] != false {
		t.Fatalf("catalog.read admitted: %v", got)
	}
	socketWrite(t, ctx, c, socketRequest("tabs.list", "foreign"))
	got := socketRead(t, ctx, c)
	if got["ok"] != false {
		t.Fatalf("foreign read admitted: %v", got)
	}
	socketWrite(t, ctx, c, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, ctx, c); got["type"] != "relay-pong" {
		t.Fatal(got)
	}
}
func TestRelaySocketSnapshotThenLiveAndResubscribe(t *testing.T) {
	srv, h, states := socketServer(t)
	// The socket events path resolves the device through the (now empty)
	// mirror registry: record a content-free stub so the subscription is
	// admitted. No content is stored.
	owner, _ := ownerKey("account-a", "desktop")
	_ = states.Publish(context.Background(), owner, HostState{})
	c, ctx := openSocket(t, srv)
	admitSocket(t, ctx, c)
	sub := func() {
		socketWrite(t, ctx, c, map[string]any{"type": "rpc-subscribe", "v": 1, "id": uuid.NewString(), "stream": "runtime.events", "params": map[string]any{"device_id": "desktop"}})
	}
	sub()
	// The attention mirror is removed (§8): the snapshot is empty and
	// records are dropped, so no live delivery ever arrives.
	snapshot := socketRead(t, ctx, c)
	if snapshot["stream"] != "runtime.events" || snapshot["seq"] != float64(1) || len(snapshot["payload"].(map[string]any)["events"].([]any)) != 0 {
		t.Fatal(snapshot)
	}
	h.events.Record("account-a", AttentionEvent{Kind: "control_granted", Title: "Live"})
	sub()
	if got := socketRead(t, ctx, c); got["seq"] != float64(1) {
		t.Fatal(got)
	}
	socketWrite(t, ctx, c, map[string]any{"type": "rpc-unsubscribe", "v": 1, "id": uuid.NewString(), "stream": "runtime.events"})
	if got := socketRead(t, ctx, c); got["ok"] != true {
		t.Fatal(got)
	}
	h.events.mu.Lock()
	left := len(h.events.subs)
	h.events.mu.Unlock()
	if left != 0 {
		t.Fatalf("leaked %d subscriptions", left)
	}
}

// Opt-in because the Go server's ordinary test suite needs no Node installation.
// This crosses the real socket using the shipping TypeScript client/readers.
func TestRelaySocketMobileInterop(t *testing.T) {
	if os.Getenv("DRAGENT_TEST_MOBILE_INTEROP") != "1" {
		t.Skip("set DRAGENT_TEST_MOBILE_INTEROP=1 with mobile dependencies installed")
	}
	states := NewMemoryHostStore()
	bus := NewEventBus()
	h := NewRPCHandler(states, bus)
	owner, _ := ownerKey("account-a", "desktop")
	_ = states.Publish(context.Background(), owner, HostState{Tabs: []TabEntry{{ID: 3, Kind: TabKindTerminal, Title: "Desktop shell"}}})
	catalogs := NewMemoryStore()
	fixtureCatalog := Catalog{
		Machine:    &MachineInfo{Name: "Fixture desktop", Platform: "macos"},
		Projects:   []ProjectEntry{{ID: strPtr("fixture-project"), Name: "Dragent", Path: "/repo"}},
		Workspaces: []WorkspaceEntry{{ID: strPtr("fixture-workspace"), ProjectID: strPtr("fixture-project"), Name: "Feature", Path: "/repo-feature", Branch: strPtr("feat/mobile")}},
		Tabs:       []TabEntry{{ID: 3, Kind: TabKindTerminal, Title: "Desktop shell", WorkspaceID: strPtr("fixture-workspace"), ProjectID: strPtr("fixture-project")}},
	}
	_ = catalogs.Put(context.Background(), owner, fixtureCatalog)
	h.ConfigureSocket(func(_ context.Context, token string) (SocketIdentity, error) {
		if token != "test-token" {
			return SocketIdentity{}, errors.New("invalid")
		}
		return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(time.Minute)}, nil
	}, catalogs, nil)
	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	r.Post("/_test/event", func(w http.ResponseWriter, _ *http.Request) {
		bus.Record("account-a", AttentionEvent{Kind: "control_granted", Title: "Edit available", Body: "Host granted edit"})
		w.WriteHeader(http.StatusNoContent)
	})
	h.RegisterHostRoutes(r)
	r.Post("/_test/catalog", func(w http.ResponseWriter, req *http.Request) {
		updated := fixtureCatalog.Normalized()
		updated.Projects[0].Name = "Updated Dragent"
		if err := catalogs.Put(req.Context(), owner, updated); err != nil {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	})
	srv := httptest.NewServer(r)
	defer srv.Close()
	startMobileInteropHost(t, srv)
	mobile, err := filepath.Abs("../../../mobile")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, filepath.Join(mobile, "node_modules/.bin/tsx"), "scripts/relay-server-interop.ts")
	cmd.Dir = mobile
	cmd.Env = append(os.Environ(), "DRAGENT_TEST_RELAY_URL=ws"+strings.TrimPrefix(srv.URL, "http")+"/realtime", "DRAGENT_TEST_RELAY_TOKEN=test-token")
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("mobile interoperability: %v\n%s", err, output)
	} else {
		t.Log(string(output))
	}
}

func TestStoppingStreamDoesNotCancelConnectionWrite(t *testing.T) {
	h := NewRPCHandler(nil, nil)
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	started := make(chan struct{})
	release := make(chan struct{})
	interrupted := make(chan struct{}, 1)
	stop := h.startSocketEvents(ctx, "account-a", func(writeCtx context.Context, _ any) error {
		close(started)
		select {
		case <-writeCtx.Done():
			interrupted <- struct{}{}
			return writeCtx.Err()
		case <-release:
			return nil
		}
	})
	<-started
	stopped := make(chan struct{})
	go func() { stop(); close(stopped) }()
	select {
	case <-interrupted:
		t.Fatal("unsubscribing canceled the connection's in-flight write")
	case <-time.After(50 * time.Millisecond):
	}
	close(release)
	select {
	case <-stopped:
	case <-ctx.Done():
		t.Fatal("unsubscribe did not drain its bounded write")
	}
	if ctx.Err() != nil {
		t.Fatal("stopping a stream canceled the shared connection")
	}
}

func TestCatalogPublishBoundsBodyBeforeDecode(t *testing.T) {
	h := NewHandler(nil)
	raw := bytes.Repeat([]byte(" "), 4*1024*1024+1)
	req := httptest.NewRequest(http.MethodPut, "/v1/relay/catalog", bytes.NewReader(raw))
	req = req.WithContext(context.WithValue(req.Context(), "user_id", "account-a"))
	w := httptest.NewRecorder()
	h.PublishCatalog(w, req)
	if w.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("unbounded catalog accepted: %d", w.Code)
	}
}

func TestRelaySocketExpiresAuthenticatedConnection(t *testing.T) {
	h := NewRPCHandler(nil, nil)
	h.ConfigureSocket(func(context.Context, string) (SocketIdentity, error) {
		return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(250 * time.Millisecond)}, nil
	}, nil, nil)
	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	srv := httptest.NewServer(r)
	defer srv.Close()
	c, ctx := openSocket(t, srv)
	admitSocket(t, ctx, c)
	_, _, err := c.Read(ctx)
	if websocket.CloseStatus(err) != websocket.StatusCode(4401) {
		t.Fatalf("expired JWT left socket authorized: %v", err)
	}
}
