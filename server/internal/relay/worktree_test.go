// worktree.list / worktree.create are removed after the E2EE cut-over
// (F2): the desktop serves worktrees inside the KK channel. Every method
// answers 410.
package relay

import (
	"net/http"
	"testing"
)

func TestWorktreeMethodsAreGone(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()

	removedMethodsGone(t, srv, "user-a", "1", "worktree.list", `{"device_id":"device-1"}`)
	removedMethodsGone(t, srv, "user-a", "2", "worktree.create", `{"device_id":"device-1","path":"/x","name":"wt-x"}`)
	status, reply := rpcRequest(t, srv, "", "a", "worktree.list", `{"device_id":"device-1"}`)
	if status != http.StatusUnauthorized || reply.Error == nil || !reply.Error.LoginRequired {
		t.Fatalf("anonymous list: status=%d reply=%+v", status, reply)
	}
}
