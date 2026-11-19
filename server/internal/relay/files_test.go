// files.list / files.search / files.open are removed after the E2EE
// cut-over (F2): the desktop serves files inside the KK channel. Every
// method answers 410; the host-state mirror accepts no writes.
package relay

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func removedMethodsGone(t *testing.T, srv *httptest.Server, user, id, method, params string) {
	t.Helper()
	status, reply := rpcRequest(t, srv, user, id, method, params)
	if status != http.StatusGone || reply.OK {
		t.Fatalf("%s: status=%d reply=%+v, want 410", method, status, reply)
	}
}

func TestFilesMethodsAreGone(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()

	removedMethodsGone(t, srv, "user-a", "1", "files.list", `{"device_id":"device-1","path":"/x"}`)
	removedMethodsGone(t, srv, "user-a", "2", "files.search", `{"device_id":"device-1","query":"main"}`)
	removedMethodsGone(t, srv, "user-a", "3", "files.open", `{"device_id":"device-1","path":"/x/f"}`)
	// Anonymous callers get 401 first.
	status, reply := rpcRequest(t, srv, "", "4", "files.list", `{"device_id":"device-1","path":"/x"}`)
	if status != http.StatusUnauthorized || reply.OK || reply.Error == nil || !reply.Error.LoginRequired {
		t.Fatalf("anonymous files.list: status=%d reply=%+v", status, reply)
	}
}
