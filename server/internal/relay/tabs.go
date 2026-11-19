// tabs.list / tabs.createTerminal: removed after the E2EE cut-over. The
// desktop serves tabs inside the KK channel; the relay mirror is gone (§10,
// F2, F3). These stubs keep the old method names compiling for the removed
// method table; they are unreachable (handleRequest answers 410 first).
package relay

import (
	"context"
	"encoding/json"
)

// tabsList is removed: the desktop serves tabs inside the KK channel.
func (h *RPCHandler) tabsList(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}

// tabsCreateTerminal is removed: the desktop serves tab creation inside the
// KK channel.
func (h *RPCHandler) tabsCreateTerminal(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}
