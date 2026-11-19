// Host-command removal tests: after the E2EE cut-over the desktop serves
// terminal and worktree creation inside the KK channel. The server never
// executes host commands and never reaches the host adapter: every creation
// method answers 410 and the executor is never invoked.
package relay

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

type hostCommandStub struct {
	owners []string
}

func (s *hostCommandStub) CreateTerminal(_ context.Context, owner string) (TabEntry, error) {
	s.owners = append(s.owners, owner)
	return TabEntry{}, errTerminalGone
}
func (s *hostCommandStub) CreateWorktree(_ context.Context, owner string, entry WorkspaceEntry) (WorkspaceEntry, error) {
	s.owners = append(s.owners, owner)
	return WorkspaceEntry{}, errTerminalGone
}

func TestHostCommandsAreNeverReached(t *testing.T) {
	states := NewMemoryHostStore()
	h := NewRPCHandler(states, nil)
	executor := &hostCommandStub{}
	h.SetHostCommands(executor)
	srv := httptest.NewServer(rpcTestRouter(h))
	defer srv.Close()

	for _, tc := range []struct{ id, method, params string }{
		{"a", "tabs.createTerminal", `{"device_id":"device-1"}`},
		{"b", "worktree.create", `{"device_id":"device-1","path":"/repo/feature"}`},
		{"c", "tabs.create", `{"device_id":"device-1","workspace_id":"w","kind":"terminal"}`},
		{"d", "tabs.list", `{"device_id":"device-1"}`},
		{"e", "worktree.list", `{"device_id":"device-1"}`},
	} {
		status, reply := rpcRequest(t, srv, "user-a", tc.id, tc.method, tc.params)
		if status != http.StatusGone || reply.OK {
			t.Fatalf("%s: status=%d reply=%+v, want 410", tc.method, status, reply)
		}
	}
	if len(executor.owners) != 0 {
		t.Fatalf("removed methods reached the host adapter: %v", executor.owners)
	}
	// Anonymous callers get 401 first.
	if status, _ := rpcRequest(t, srv, "", "anon", "worktree.create", `{"device_id":"device-1","path":"/repo/wt"}`); status != http.StatusUnauthorized {
		t.Fatalf("anonymous create status = %d, want 401", status)
	}
	if len(executor.owners) != 0 {
		t.Fatalf("anonymous caller reached the host adapter: %v", executor.owners)
	}
}
