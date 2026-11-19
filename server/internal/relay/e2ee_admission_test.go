// E2EE admission (§10) and binary routing (§4.1) tests: the §12.7 server
// cases E2a owns — old phone builds, e2e:false devices, old desktops,
// stripped e2e:1 — plus frame forwarding (unmodified, in order), caps, the
// relay-chan-closed backpressure path, and host-notify intake.
package relay

import (
	"context"
	"errors"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

const (
	e2eTestPhone    = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	e2eTestPhone2   = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"
	e2eTestDesktop  = "01234567-89ab-4cde-8f01-23456789abcd"
	e2eTestDesktop2 = "33333333-4444-4555-8666-777777777777"
)

func e2eeAuth(token string) SocketAuthenticator {
	return func(_ context.Context, tokenIn string) (SocketIdentity, error) {
		switch tokenIn {
		case "phone-token":
			return SocketIdentity{UserID: "account-a", DeviceID: e2eTestPhone, ExpiresAt: time.Now().Add(time.Hour)}, nil
		case "phone2-token":
			return SocketIdentity{UserID: "account-a", DeviceID: e2eTestPhone2, ExpiresAt: time.Now().Add(time.Hour)}, nil
		case "host-token":
			return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(time.Hour)}, nil
		case token:
			return SocketIdentity{UserID: "account-a", ExpiresAt: time.Now().Add(time.Hour)}, nil
		}
		return SocketIdentity{}, errors.New("bad token")
	}
}

func e2eeServer(t *testing.T) (*httptest.Server, *RPCHandler) {
	t.Helper()
	h := NewRPCHandler(nil, nil)
	catalogs := NewMemoryStore()
	h.ConfigureSocket(e2eeAuth(""), catalogs, nil)
	r := chi.NewRouter()
	h.RegisterSocketRoutes(r)
	h.RegisterHostRoutes(r)
	srv := httptest.NewServer(r)
	t.Cleanup(srv.Close)
	owner, _ := ownerKey("account-a", e2eTestDesktop)
	if err := catalogs.Put(context.Background(), owner, Catalog{Tabs: []TabEntry{{ID: 1, Kind: TabKindTerminal}}}); err != nil {
		t.Fatal(err)
	}
	return srv, h
}

func dialPath(t *testing.T, srv *httptest.Server, path string) (*websocket.Conn, context.Context) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	t.Cleanup(cancel)
	c, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(srv.URL, "http")+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.CloseNow() })
	return c, ctx
}

func writeBinary(t *testing.T, ctx context.Context, c *websocket.Conn, raw []byte) {
	t.Helper()
	if err := c.Write(ctx, websocket.MessageBinary, raw); err != nil {
		t.Fatal(err)
	}
}

func readBinary(t *testing.T, ctx context.Context, c *websocket.Conn) []byte {
	t.Helper()
	kind, raw, err := c.Read(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if kind != websocket.MessageBinary {
		t.Fatalf("kind = %v, want binary", kind)
	}
	return raw
}

func hs1For(t *testing.T, chanHex, peerUUID string, body []byte) ([]byte, [16]byte) {
	t.Helper()
	chanRaw, err := uuid.Parse(chanHex)
	if err != nil {
		// chanHex may be dashless from ChanHex; re-add dashes.
		chanRaw, err = uuid.Parse(chanHex[:8] + "-" + chanHex[8:12] + "-" + chanHex[12:16] + "-" + chanHex[16:20] + "-" + chanHex[20:])
		if err != nil {
			t.Fatal(err)
		}
	}
	var chanID [16]byte = chanRaw
	peer := uuid.MustParse(peerUUID)
	raw, err := EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, chanID, (*[16]byte)(&peer), 0, body)
	if err != nil {
		t.Fatal(err)
	}
	return raw, chanID
}

// Admit one phone; on refusal returns the relay-error frame (never ready).
func admitPhoneRaw(t *testing.T, ctx context.Context, c *websocket.Conn, auth map[string]any) map[string]any {
	t.Helper()
	socketWrite(t, ctx, c, auth)
	return socketRead(t, ctx, c)
}

func TestE2EOldPhoneBuildRefused4403(t *testing.T) {
	srv, _ := e2eeServer(t)
	variants := []map[string]any{
		{"type": "relay-auth", "v": 1, "access_token": "phone-token", "device_id": e2eTestDesktop},
		{"type": "relay-auth", "v": 1, "access_token": "phone-token", "device_id": e2eTestDesktop, "e2e": 0},
		{"type": "relay-auth", "v": 1, "access_token": "phone-token", "device_id": e2eTestDesktop, "e2e": "1"},
		{"type": "relay-auth", "v": 1, "access_token": "phone-token", "device_id": e2eTestDesktop, "e2e": true},
	}
	for i, auth := range variants {
		c, ctx := dialPath(t, srv, "/v1/relay/socket")
		got := admitPhoneRaw(t, ctx, c, auth)
		if got["type"] != "relay-error" {
			t.Fatalf("variant %d: admitted without e2e:1: %v", i, got)
		}
		if got["updateRequired"] != true {
			t.Fatalf("variant %d: missing updateRequired: %v", i, got)
		}
		if !strings.Contains(got["message"].(string), "Update the Dragent app on this phone.") {
			t.Fatalf("variant %d: wrong message: %v", i, got)
		}
		if code := readCloseCode(t, ctx, c); code != e2eeRefusedCode {
			t.Fatalf("variant %d: close = %v, want 4403", i, code)
		}
	}
}

func TestE2EPreCutoverDeviceRefused4403(t *testing.T) {
	srv, h := e2eeServer(t)
	h.SetE2EDevicePolicy(func(context.Context, string, string) (bool, bool) { return true, false })
	c, ctx := dialPath(t, srv, "/v1/relay/socket")
	got := admitPhoneRaw(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "phone-token", "device_id": e2eTestDesktop})
	if got["type"] != "relay-error" || got["repairRequired"] != true {
		t.Fatalf("e2e:false device not refused with repairRequired: %v", got)
	}
	if !strings.Contains(got["message"].(string), "Pair this phone again") {
		t.Fatalf("wrong message: %v", got)
	}
	if code := readCloseCode(t, ctx, c); code != e2eeRefusedCode {
		t.Fatalf("close = %v, want 4403", code)
	}
}

func TestE2EAdmittedDevicesPass(t *testing.T) {
	srv, h := e2eeServer(t)
	// e2e:true device admitted.
	h.SetE2EDevicePolicy(func(context.Context, string, string) (bool, bool) { return true, true })
	c, ctx := dialPath(t, srv, "/v1/relay/socket")
	if got := admitPhoneRaw(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "phone-token", "device_id": e2eTestDesktop}); got["type"] != "relay-ready" {
		t.Fatalf("e2e:true refused: %v", got)
	}
	// Unknown device (policy silent) still admitted: JWT + revocation apply.
	h.SetE2EDevicePolicy(func(context.Context, string, string) (bool, bool) { return false, false })
	c2, ctx2 := dialPath(t, srv, "/v1/relay/socket")
	if got := admitPhoneRaw(t, ctx2, c2, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "phone-token", "device_id": e2eTestDesktop}); got["type"] != "relay-ready" {
		t.Fatalf("unknown device refused: %v", got)
	}
	// Nil policy (dev default) admitted.
	h.SetE2EDevicePolicy(nil)
	c3, ctx3 := dialPath(t, srv, "/v1/relay/socket")
	if got := admitPhoneRaw(t, ctx3, c3, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "phone-token", "device_id": e2eTestDesktop}); got["type"] != "relay-ready" {
		t.Fatalf("nil policy refused: %v", got)
	}
}

func TestE2EOldDesktopRefused4403(t *testing.T) {
	srv, _ := e2eeServer(t)
	// Without e2e:1: refused, never relay-host-ready.
	c, ctx := dialPath(t, srv, "/v1/relay/host")
	socketWrite(t, ctx, c, map[string]any{"type": "relay-host-auth", "v": 1, "access_token": "host-token", "device_id": e2eTestDesktop, "expected_user_id": "account-a"})
	got := socketRead(t, ctx, c)
	if got["type"] != "relay-error" || got["updateRequired"] != true {
		t.Fatalf("old desktop not refused: %v", got)
	}
	if code := readCloseCode(t, ctx, c); code != e2eeRefusedCode {
		t.Fatalf("close = %v, want 4403", code)
	}
	// Stripped (e2e:0) fails closed the same way.
	c2, ctx2 := dialPath(t, srv, "/v1/relay/host")
	socketWrite(t, ctx2, c2, map[string]any{"type": "relay-host-auth", "v": 1, "e2e": 0, "access_token": "host-token", "device_id": e2eTestDesktop, "expected_user_id": "account-a"})
	if got := socketRead(t, ctx2, c2); got["type"] != "relay-error" {
		t.Fatalf("stripped host e2e admitted: %v", got)
	}
	if code := readCloseCode(t, ctx2, c2); code != e2eeRefusedCode {
		t.Fatalf("close = %v, want 4403", code)
	}
	// With e2e:1: admitted.
	c3, ctx3 := dialPath(t, srv, "/v1/relay/host")
	socketWrite(t, ctx3, c3, map[string]any{"type": "relay-host-auth", "v": 1, "e2e": 1, "access_token": "host-token", "device_id": e2eTestDesktop, "expected_user_id": "account-a"})
	if got := socketRead(t, ctx3, c3); got["type"] != "relay-host-ready" {
		t.Fatalf("e2ee host refused: %v", got)
	}
}

func admitE2EPhone(t *testing.T, srv *httptest.Server, token, desktop string) (*websocket.Conn, context.Context) {
	t.Helper()
	c, ctx := dialPath(t, srv, "/v1/relay/socket")
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": token, "device_id": desktop})
	if got := socketRead(t, ctx, c); got["type"] != "relay-ready" {
		t.Fatalf("phone admission: %v", got)
	}
	return c, ctx
}

func admitE2EHost(t *testing.T, srv *httptest.Server) (*websocket.Conn, context.Context) {
	t.Helper()
	c, ctx := dialPath(t, srv, "/v1/relay/host")
	socketWrite(t, ctx, c, map[string]any{"type": "relay-host-auth", "v": 1, "e2e": 1, "access_token": "host-token", "device_id": e2eTestDesktop, "expected_user_id": "account-a"})
	if got := socketRead(t, ctx, c); got["type"] != "relay-host-ready" {
		t.Fatalf("host admission: %v", got)
	}
	return c, ctx
}

func TestE2EBinaryForwardsUnmodifiedAndInOrder(t *testing.T) {
	srv, _ := e2eeServer(t)
	phone, pctx := admitE2EPhone(t, srv, "phone-token", e2eTestDesktop)
	host, hctx := admitE2EHost(t, srv)

	chanUUID := uuid.NewString()
	hs1, chanID := hs1For(t, chanUUID, e2eTestDesktop, []byte("handshake-bytes"))
	writeBinary(t, pctx, phone, hs1)

	// Host receives the HS1 with peer rewritten to the initiator (phone) id.
	got := readBinary(t, hctx, host)
	hf, err := DecodeE2EEFrame(got)
	if err != nil {
		t.Fatal(err)
	}
	if hf.Peer == nil || uuid.UUID(*hf.Peer).String() != e2eTestPhone {
		t.Fatalf("peer not rewritten to initiator: %v", hf.Peer)
	}
	if hf.Chan != chanID || string(hf.Body) != "handshake-bytes" {
		t.Fatal("HS1 mutated beyond peer")
	}

	// Host DATA forwards to the phone unmodified, in order.
	for ctr := uint64(0); ctr < 3; ctr++ {
		body := []byte{byte(ctr), 0xDE, 0xAD}
		raw, err := EncodeE2EEFrame(E2EEKindData, 0, 0, chanID, nil, ctr, body)
		if err != nil {
			t.Fatal(err)
		}
		writeBinary(t, hctx, host, raw)
	}
	for ctr := uint64(0); ctr < 3; ctr++ {
		raw := readBinary(t, pctx, phone)
		df, err := DecodeE2EEFrame(raw)
		if err != nil {
			t.Fatal(err)
		}
		if df.Ctr != ctr || len(df.Body) != 3 || df.Body[0] != byte(ctr) {
			t.Fatalf("order/content changed at %d: %+v", ctr, df)
		}
	}

	// Phone DATA forwards to the host unmodified.
	payload, err := EncodeE2EEFrame(E2EEKindData, 0, 0, chanID, nil, 7, []byte("keystroke"))
	if err != nil {
		t.Fatal(err)
	}
	writeBinary(t, pctx, phone, payload)
	if got := readBinary(t, hctx, host); string(got) != string(payload) {
		t.Fatal("phone DATA mutated")
	}

	// CLOSE forwards, then releases the channel: later DATA gets chan-closed
	// while the socket stays open for other channels.
	closeRaw, err := EncodeE2EEFrame(E2EEKindClose, 0, 0, chanID, nil, 0, []byte{E2EECloseProtocolError})
	if err != nil {
		t.Fatal(err)
	}
	writeBinary(t, pctx, phone, closeRaw)
	if got := readBinary(t, hctx, host); string(got) != string(closeRaw) {
		t.Fatal("CLOSE mutated")
	}
	stray, err := EncodeE2EEFrame(E2EEKindData, 0, 0, chanID, nil, 8, []byte("late"))
	if err != nil {
		t.Fatal(err)
	}
	writeBinary(t, pctx, phone, stray)
	notice := socketRead(t, pctx, phone)
	if notice["type"] != TextRelayChanClosed {
		t.Fatalf("missing chan-closed after CLOSE: %v", notice)
	}
	// Socket still live: heartbeat answers.
	socketWrite(t, pctx, phone, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, pctx, phone); got["type"] != "relay-pong" {
		t.Fatalf("socket died with the channel: %v", got)
	}
}

func TestE2EBadTargetAndOfflineDesktopChanClosed(t *testing.T) {
	srv, h := e2eeServer(t)
	phone, pctx := admitE2EPhone(t, srv, "phone-token", e2eTestDesktop)
	_ = phone

	// HS1 to a desktop outside the lease: refused, socket stays open.
	hs1, _ := hs1For(t, uuid.NewString(), e2eTestDesktop2, []byte("x"))
	writeBinary(t, pctx, phone, hs1)
	if got := socketRead(t, pctx, phone); got["type"] != TextRelayChanClosed || got["reason"] != "refused" {
		t.Fatalf("bad target: %v", got)
	}

	// HS1 matching a lease whose desktop is offline: unavailable, no state kept.
	phone2, pctx2 := admitE2EPhone(t, srv, "phone2-token", e2eTestDesktop2)
	hs1b, chanB := hs1For(t, uuid.NewString(), e2eTestDesktop2, []byte("x"))
	writeBinary(t, pctx2, phone2, hs1b)
	if got := socketRead(t, pctx2, phone2); got["type"] != TextRelayChanClosed || got["reason"] != "unavailable" {
		t.Fatalf("offline desktop: %v", got)
	}
	if _, ok := e2eeFor(h).router.Lookup(chanB); ok {
		t.Fatal("unavailable channel kept router state")
	}
	// Malformed binary kills the socket, not just the channel.
	writeBinary(t, pctx2, phone2, []byte{0x01, 0x02})
	if code := readCloseCode(t, pctx2, phone2); code != websocket.StatusInvalidFramePayloadData {
		t.Fatalf("framing violation close = %v", code)
	}
}

func TestE2EHostNotifyIntake(t *testing.T) {
	srv, h := e2eeServer(t)
	var mu sync.Mutex
	var received []struct{ account, phone, blob string }
	intakeLen := func() int {
		mu.Lock()
		defer mu.Unlock()
		return len(received)
	}
	intakeSnapshot := func() []struct{ account, phone, blob string } {
		mu.Lock()
		defer mu.Unlock()
		out := make([]struct{ account, phone, blob string }, len(received))
		copy(out, received)
		return out
	}
	h.SetE2ENotifyIntake(func(_ context.Context, account, phone, blob string) error {
		mu.Lock()
		received = append(received, struct{ account, phone, blob string }{account, phone, blob})
		mu.Unlock()
		return nil
	})
	host, hctx := admitE2EHost(t, srv)

	blob := strings.Repeat("A", MaxE2EENotifyBlobChars)
	socketWrite(t, hctx, host, map[string]any{"type": "host-notify", "v": 1, "phone_id": e2eTestPhone, "blob": blob})
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && intakeLen() == 0 {
		time.Sleep(10 * time.Millisecond)
	}
	if got := intakeSnapshot(); len(got) != 1 || got[0].account != "account-a" || got[0].phone != e2eTestPhone || got[0].blob != blob {
		t.Fatalf("intake: %+v", got)
	}

	// Invalid blobs are dropped without intake and without failing the socket:
	// a live relay-pong proves the desktop socket survived.
	for _, bad := range []string{strings.Repeat("A", 100), strings.Repeat("!", MaxE2EENotifyBlobChars), ""} {
		socketWrite(t, hctx, host, map[string]any{"type": "host-notify", "v": 1, "phone_id": e2eTestPhone, "blob": bad})
	}
	socketWrite(t, hctx, host, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, hctx, host); got["type"] != "relay-pong" {
		t.Fatalf("host socket died on invalid intake: %v", got)
	}
	if got := intakeSnapshot(); len(got) != 1 {
		t.Fatalf("invalid intake reached dispatch: %+v", got)
	}
}

func TestE2EStatsCarryNoIdentifiers(t *testing.T) {
	srv, h := e2eeServer(t)
	phone, pctx := admitE2EPhone(t, srv, "phone-token", e2eTestDesktop)
	host, _ := admitE2EHost(t, srv)
	hs1, _ := hs1For(t, uuid.NewString(), e2eTestDesktop, []byte("x"))
	writeBinary(t, pctx, phone, hs1)
	_ = readBinary(t, context.Background(), host)
	dom := e2eeFor(h)
	snap := dom.stats.SnapshotAndReset()
	if len(snap) == 0 {
		t.Fatal("no aggregate counters recorded")
	}
	for k := range snap {
		for _, secret := range []string{e2eTestPhone, e2eTestDesktop, "account-a", "token"} {
			if strings.Contains(k, secret) {
				t.Fatalf("aggregate key leaks identifier: %q", k)
			}
		}
	}
}
