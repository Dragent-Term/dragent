// worktree.list / worktree.create: removed after the E2EE cut-over. The
// desktop serves worktrees inside the KK channel; the relay mirror is gone
// (§10, F2, F3). These stubs keep the old method names compiling for the
// removed method table; they are unreachable (handleRequest answers 410
// first).
package relay

import (
	"context"
	"encoding/json"
)

// worktreeList is removed: the desktop serves worktrees inside the KK channel.
func (h *RPCHandler) worktreeList(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}

// worktreeCreate is removed: the desktop serves worktree creation inside the
// KK channel.
func (h *RPCHandler) worktreeCreate(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}
