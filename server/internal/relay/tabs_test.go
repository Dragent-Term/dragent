// tabs.list / tabs.createTerminal are removed after the E2EE cut-over
// (F2): the desktop serves tabs inside the KK channel. Every method
// answers 410.
package relay

import (
	"net/http"
	"testing"
)

func TestTabsMethodsAreGone(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()

	removedMethodsGone(t, srv, "user-a", "1", "tabs.list", `{"device_id":"device-1"}`)
	removedMethodsGone(t, srv, "user-a", "2", "tabs.createTerminal", `{"device_id":"device-1"}`)
	removedMethodsGone(t, srv, "user-a", "3", "tabs.create", `{"device_id":"device-1"}`)
	status, reply := rpcRequest(t, srv, "", "a", "tabs.list", `{"device_id":"device-1"}`)
	if status != http.StatusUnauthorized || reply.Error == nil || !reply.Error.LoginRequired {
		t.Fatalf("anonymous list: status=%d reply=%+v", status, reply)
	}
}
