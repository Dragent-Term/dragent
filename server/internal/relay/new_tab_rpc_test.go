// tabs.create is removed after the E2EE cut-over: the desktop opens tabs
// inside the KK channel. Socket and HTTP callers answer 410 and nothing
// reaches the desktop.
package relay

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/coder/websocket"
)

func newTabFixture(t *testing.T) (*terminalFixture, *hostFixture, *websocket.Conn, context.Context) {
	t.Helper()
	f := newTerminalFixture(t)
	// Host admission requires a known catalog owner: record a content-free
	// routing stub. No content is stored or served.
	owner, _ := ownerKey("account-a", "desktop")
	if err := f.catalogs.Put(context.Background(), owner, Catalog{}.Normalized()); err != nil {
		t.Fatal(err)
	}
	host := f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")
	return f, host, phone, ctx
}

func assertNothingReachedDesktop(t *testing.T, host *hostFixture) {
	t.Helper()
	select {
	case request := <-host.requests:
		t.Fatalf("request reached desktop: %v", request)
	default:
	}
}

func TestNewTabIsGone(t *testing.T) {
	_, host, phone, ctx := newTabFixture(t)
	for _, kind := range []string{"terminal", "browser", "dragent_cli"} {
		sendTerminalRPC(t, phone, ctx, "tabs.create", "desktop", map[string]any{"workspace_id": "w0w3", "kind": kind})
		reply := socketRead(t, ctx, phone)
		if reply["ok"] != false {
			t.Fatalf("%s admitted: %v", kind, reply)
		}
		if msg, _ := reply["error"].(map[string]any)["message"].(string); !strings.Contains(msg, "end-to-end encrypted") {
			t.Fatalf("%s wrong message: %v", kind, reply)
		}
	}
	assertNothingReachedDesktop(t, host)
}

func TestNewTabRejectsOtherDeviceAndHTTPWithoutSession(t *testing.T) {
	f, host, phone, ctx := newTabFixture(t)
	sendTerminalRPC(t, phone, ctx, "tabs.create", "other", map[string]any{"workspace_id": "w0w3", "kind": "terminal"})
	if reply := socketRead(t, ctx, phone); reply["ok"] != false {
		t.Fatal(reply)
	}
	raw := `{"type":"rpc-request","v":1,"id":"new","method":"tabs.create","params":{"device_id":"desktop","workspace_id":"w0w3","kind":"terminal"}}`
	req := httptest.NewRequest(http.MethodPost, "/v1/relay/rpc", strings.NewReader(raw))
	req = req.WithContext(context.WithValue(req.Context(), "user_id", "account-a"))
	response := httptest.NewRecorder()
	f.handler.HandleFrame(response, req)
	if response.Code != http.StatusGone {
		t.Fatal(response.Code, response.Body.String())
	}
	assertNothingReachedDesktop(t, host)
}
