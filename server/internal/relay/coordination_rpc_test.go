// coordination.list / coordination.get are removed after the E2EE
// cut-over (F5): the desktop serves coordination inside the KK channel.
// Every method answers 410 and nothing reaches the desktop.
package relay

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/coder/websocket"
)

func coordinationFixture(t *testing.T) (*terminalFixture, *hostFixture, *websocket.Conn, context.Context, map[string]any) {
	t.Helper()
	f := newTerminalFixture(t)
	// Host admission requires a known catalog owner: record a content-free
	// routing stub. No content is stored or served.
	owner, _ := ownerKey("account-a", "desktop")
	if err := f.catalogs.Put(context.Background(), owner, Catalog{}.Normalized()); err != nil {
		t.Fatal(err)
	}
	host := f.host(t, "account-a", "desktop")
	phone, ctx, ready := f.phone(t, "account-a", "desktop", "")
	return f, host, phone, ctx, ready
}

func TestCoordinationMethodsAreGone(t *testing.T) {
	f, host, phone, ctx, _ := coordinationFixture(t)
	_ = f
	for _, tc := range []struct{ method, params string }{
		{"coordination.list", `{"device_id":"desktop"}`},
		{"coordination.get", `{"device_id":"desktop","team_id":"team-1"}`},
	} {
		sendTerminalRPC(t, phone, ctx, tc.method, "desktop", map[string]any{})
		reply := socketRead(t, ctx, phone)
		if reply["ok"] != false {
			t.Fatalf("%s admitted: %v", tc.method, reply)
		}
	}
	assertNothingReachedDesktop(t, host)
}

func TestCoordinationHTTPIsGone(t *testing.T) {
	f, _, _, _, _ := coordinationFixture(t)
	raw := `{"type":"rpc-request","v":1,"id":"list","method":"coordination.list","params":{"device_id":"desktop"}}`
	req := httptest.NewRequest(http.MethodPost, "/v1/relay/rpc", strings.NewReader(raw))
	req = req.WithContext(context.WithValue(req.Context(), "user_id", "account-a"))
	response := httptest.NewRecorder()
	f.handler.HandleFrame(response, req)
	if response.Code != http.StatusGone {
		t.Fatal(response.Code, response.Body.String())
	}
}
