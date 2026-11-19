package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
)

type terminalFixture struct {
	srv      *httptest.Server
	handler  *RPCHandler
	catalogs *MemoryStore
}

func newTerminalFixture(t *testing.T) *terminalFixture {
	t.Helper()
	h := NewRPCHandler(nil, nil)
	catalogs := NewMemoryStore()
	h.sessionPollInterval = 20 * time.Millisecond
	h.ConfigureSocket(func(_ context.Context, token string) (SocketIdentity, error) {
		if token != "account-a-token" && token != "account-b-token" {
			return SocketIdentity{}, errTerminalInvalid
		}
		return SocketIdentity{UserID: strings.TrimSuffix(token, "-token"), ExpiresAt: time.Now().Add(time.Minute)}, nil
	}, catalogs, nil)
	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	h.RegisterHostRoutes(r)
	srv := httptest.NewServer(r)
	t.Cleanup(srv.Close)
	return &terminalFixture{srv: srv, handler: h, catalogs: catalogs}
}
func (f *terminalFixture) publish(t *testing.T, user, device string) {
	t.Helper()
	owner, _ := ownerKey(user, device)
	err := f.catalogs.Put(context.Background(), owner, Catalog{Tabs: []TabEntry{{ID: 1, Kind: TabKindTerminal}, {ID: 2, Kind: TabKindAgent}, {ID: 3, Kind: TabKindBrowser}, {ID: 4, Kind: TabKindSettings}}})
	if err != nil {
		t.Fatal(err)
	}
}

type hostFixture struct {
	conn     *websocket.Conn
	ctx      context.Context
	requests chan map[string]any
	done     chan struct{}
}

func hostAuth(t *testing.T, f *terminalFixture, user, device string, expected string) (*websocket.Conn, context.Context, map[string]any, error) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	t.Cleanup(cancel)
	c, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(f.srv.URL, "http")+"/v1/relay/host", nil)
	if err != nil {
		return nil, ctx, nil, err
	}
	t.Cleanup(func() { c.CloseNow() })
	socketWrite(t, ctx, c, map[string]any{"type": "relay-host-auth", "v": 1, "e2e": 1, "access_token": user + "-token", "device_id": device, "expected_user_id": expected})
	_, raw, err := c.Read(ctx)
	if err != nil {
		return c, ctx, nil, err
	}
	var ready map[string]any
	err = json.Unmarshal(raw, &ready)
	return c, ctx, ready, err
}
func (f *terminalFixture) host(t *testing.T, user, device string) *hostFixture {
	t.Helper()
	c, ctx, ready, err := hostAuth(t, f, user, device, user)
	if err != nil || ready["type"] != "relay-host-ready" {
		t.Fatalf("host admission: %v %v", ready, err)
	}
	fixture := &hostFixture{conn: c, ctx: ctx, requests: make(chan map[string]any, maxHostPending+8), done: make(chan struct{})}
	go func() {
		defer close(fixture.done)
		for {
			_, raw, err := c.Read(ctx)
			if err != nil {
				return
			}
			var frame map[string]any
			if json.Unmarshal(raw, &frame) == nil && frame["type"] == "host-request" {
				select {
				case fixture.requests <- frame:
				case <-ctx.Done():
					return
				}
			}
		}
	}()
	return fixture
}
func (f *hostFixture) next(t *testing.T) map[string]any {
	t.Helper()
	select {
	case frame := <-f.requests:
		return frame
	case <-f.done:
		t.Fatal("desktop connection ended before request")
	case <-time.After(3 * time.Second):
		t.Fatal("no host request")
	}
	return nil
}
func (f *hostFixture) reply(t *testing.T, request map[string]any, result any) {
	t.Helper()
	socketWrite(t, f.ctx, f.conn, map[string]any{"type": "host-reply", "v": 1, "id": request["id"], "ok": true, "result": result})
}
func (f *terminalFixture) phone(t *testing.T, user, device, resume string) (*websocket.Conn, context.Context, map[string]any) {
	t.Helper()
	c, ctx := openSocket(t, f.srv)
	auth := map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": user + "-token", "device_id": device}
	if resume != "" {
		auth["resume_session_id"] = resume
	}
	socketWrite(t, ctx, c, auth)
	ready := socketRead(t, ctx, c)
	if ready["type"] != "relay-ready" {
		t.Fatal(ready)
	}
	return c, ctx, ready
}
func sendTerminalRPC(t *testing.T, c *websocket.Conn, ctx context.Context, method, device string, params map[string]any) map[string]any {
	t.Helper()
	frame := socketRequest(method, device)
	params["device_id"] = device
	frame["params"] = params
	socketWrite(t, ctx, c, frame)
	return frame
}
func (f *terminalFixture) attach(t *testing.T, host *hostFixture, c *websocket.Conn, ctx context.Context, device string, tab uint64) string {
	t.Helper()
	sendTerminalRPC(t, c, ctx, "tabs.attach", device, map[string]any{"tab_id": tab})
	request := host.next(t)
	if request["method"] != "terminal.attach" {
		t.Fatal(request)
	}
	host.reply(t, request, map[string]any{"cols": 80, "rows": 24})
	reply := socketRead(t, ctx, c)
	if reply["ok"] != true {
		t.Fatal(reply)
	}
	return reply["result"].(map[string]any)["handle"].(string)
}
func waitBridge(t *testing.T, test func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if test() {
			return
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatal("bridge state did not settle")
}

func TestTerminalBridgeRealSocketReadInputResizeScreenDetach(t *testing.T) {
	// E2EE cut-over (F1): the terminal proxy is removed. Attach, read,
	// input, resize, screen and detach answer 410 and nothing reaches the
	// desktop — the stream travels as binary TERM records inside the KK
	// channel.
	f := newTerminalFixture(t)
	f.publish(t, "account-a", "desktop")
	host := f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")
	for _, tc := range []struct {
		method string
		params map[string]any
	}{
		{"tabs.attach", map[string]any{"tab_id": 1}},
		{"tabs.read", map[string]any{"handle": "00000000-0000-4000-8000-000000000000", "offset": 0}},
		{"tabs.input", map[string]any{"handle": "00000000-0000-4000-8000-000000000000", "text": "x"}},
		{"tabs.resize", map[string]any{"handle": "00000000-0000-4000-8000-000000000000", "rows": 30, "cols": 100}},
		{"tabs.screen", map[string]any{"handle": "00000000-0000-4000-8000-000000000000"}},
		{"tabs.detach", map[string]any{"handle": "00000000-0000-4000-8000-000000000000"}},
	} {
		sendTerminalRPC(t, phone, ctx, tc.method, "desktop", tc.params)
		if got := socketRead(t, ctx, phone); got["ok"] != false {
			t.Fatalf("%s admitted: %v", tc.method, got)
		}
	}
	assertNothingReachedDesktop(t, host)
}

func TestHostBridgeAdmitsDesktopPublishedWithZeroRecipients(t *testing.T) {
	// First-pairing deadlock (§6.5): a new desktop with no paired phones
	// publishes a sealed catalog with 0 recipients. That publish must
	// succeed and the host socket must be admitted, so HS1/HS2 can ride it.
	f := newTerminalFixture(t)
	sealed := NewMemorySealedStore()
	catalogHandler := NewHandler(f.catalogs)
	catalogHandler.SetSealedStore(sealed)
	f.handler.SetSealedStore(sealed)

	blob := makeSealedBlob(t, sealedTestDesktop, 1, sealedTestPhone)
	blob.Recipients = []SealedRecipient{}
	raw, _ := json.Marshal(sealedPutBody(sealedTestDesktop, blob))
	req := httptest.NewRequest(http.MethodPut, "/v1/relay/catalog", bytes.NewReader(raw))
	req = req.WithContext(context.WithValue(req.Context(), "user_id", "account-a"))
	publishRec := httptest.NewRecorder()
	catalogHandler.PublishCatalog(publishRec, req)
	if publishRec.Code != http.StatusOK {
		t.Fatalf("empty-recipient publish status = %d (%s), want 200", publishRec.Code, publishRec.Body.String())
	}

	c, ctx, ready, err := hostAuth(t, f, "account-a", sealedTestDesktop, "account-a")
	if err != nil || ready["type"] != "relay-host-ready" {
		t.Fatalf("host admission after empty-recipient publish: %v %v", ready, err)
	}
	c.CloseNow()
	_ = ctx
}

func TestHostBridgeRejectsUnknownForeignAndExpectedUserMismatch(t *testing.T) {
	for _, test := range []struct{ name, user, device, expected string }{{"unknown", "account-a", "unknown", "account-a"}, {"foreign", "account-b", "desktop", "account-b"}, {"wrong expected user", "account-a", "desktop", "account-b"}, {"missing expected user", "account-a", "desktop", ""}} {
		t.Run(test.name, func(t *testing.T) {
			f := newTerminalFixture(t)
			f.publish(t, "account-a", "desktop")
			_, _, ready, err := hostAuth(t, f, test.user, test.device, test.expected)
			if err == nil && ready["type"] == "relay-host-ready" {
				t.Fatal("unowned host admitted")
			}
			f.handler.bridge.mu.Lock()
			count := len(f.handler.bridge.peers)
			f.handler.bridge.mu.Unlock()
			if count != 0 {
				t.Fatal("unauthorized peer registered")
			}
		})
	}
}

func TestHostBridgePeerCountRemainsBoundedAcrossCatalogueEviction(t *testing.T) {
	f := newTerminalFixture(t)
	for i := 0; i < maxMachinesPerAccount; i++ {
		device := fmt.Sprintf("desktop-%d", i)
		f.publish(t, "account-a", device)
		f.host(t, "account-a", device)
	}
	f.publish(t, "account-a", "new-desktop")
	_, _, ready, err := hostAuth(t, f, "account-a", "new-desktop", "account-a")
	if err == nil && ready["type"] == "relay-host-ready" {
		t.Fatal("peer capacity bypassed by catalog eviction")
	}
	f.handler.bridge.mu.Lock()
	count := len(f.handler.bridge.peers)
	f.handler.bridge.mu.Unlock()
	if count != maxMachinesPerAccount {
		t.Fatalf("unexpected peer count %d", count)
	}
}

func TestHostBridgeReplacementFailsPendingInputAndNeverReplays(t *testing.T) {
	// E2EE cut-over (F1): server-side input proxying is removed. Input
	// travels inside the KK channel; the relay answers 410 and nothing
	// reaches either host generation.
	f := newTerminalFixture(t)
	f.publish(t, "account-a", "desktop")
	old := f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")
	sendTerminalRPC(t, phone, ctx, "tabs.input", "desktop", map[string]any{"handle": "00000000-0000-4000-8000-000000000000", "text": "run exactly once"})
	if got := socketRead(t, ctx, phone); got["ok"] != false {
		t.Fatalf("input admitted: %v", got)
	}
	replacement := f.host(t, "account-a", "desktop")
	select {
	case request := <-old.requests:
		t.Fatal("input reached the old generation", request)
	default:
	}
	select {
	case request := <-replacement.requests:
		t.Fatal("input replayed on successor", request)
	default:
	}
}

func TestHostBridgePendingCallsAreBoundedAndCanceledWithoutReplay(t *testing.T) {
	bridge := NewHostBridge()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	peer := &hostPeer{bridge: bridge, owner: "device\x00account\x00desktop", expiresAt: time.Now().Add(time.Minute), ctx: ctx, cancel: cancel, writes: make(chan hostOutbound, maxHostPending), pending: make(map[string]chan hostResult)}
	bridge.peers[peer.owner] = peer
	var wg sync.WaitGroup
	for i := 0; i < maxHostPending; i++ {
		wg.Add(1)
		go func() { defer wg.Done(); _, _ = peer.call(ctx, "terminal.read", map[string]any{"handle": "test"}) }()
	}
	waitBridge(t, func() bool {
		peer.mu.Lock()
		defer peer.mu.Unlock()
		return len(peer.pending) == maxHostPending && len(peer.writes) == maxHostPending
	})
	if _, err := peer.call(ctx, "terminal.input", map[string]any{"text": "no queue"}); err == nil {
		t.Fatal("pending cap exceeded")
	}
	cancel()
	wg.Wait()
	peer.mu.Lock()
	left := len(peer.pending)
	peer.mu.Unlock()
	if left != 0 {
		t.Fatal("canceled pending calls retained")
	}
	for len(peer.writes) > 0 {
		queued := <-peer.writes
		if queued.ctx.Err() == nil {
			t.Fatal("canceled queued operation could still send")
		}
	}
}

func TestHostBridgeDoesNotExposeCredentialInURL(t *testing.T) {
	f := newTerminalFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	c, response, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(f.srv.URL, "http")+"/v1/relay/host?access_token=private", nil)
	if c != nil {
		c.CloseNow()
	}
	if err == nil || response == nil || response.StatusCode != http.StatusBadRequest {
		t.Fatal("query credentials accepted")
	}
}
