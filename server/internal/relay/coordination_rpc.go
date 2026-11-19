// coordination.list / coordination.get: removed after the E2EE cut-over.
// The desktop serves coordination inside the KK channel; the relay never
// sees team goals, tasks, results, or agent messages (§10, F5). These stubs
// keep the old method names compiling for the removed method table; they
// are unreachable (handleRequest answers 410 first).
package relay

import (
	"context"
	"encoding/json"
)

// coordinationCatalog is removed with the server content methods: the
// desktop serves coordination inside the KK channel. Kept as a failing stub
// for the removed tabs.create path (new_tab_rpc.go), which is itself
// unreachable (handleRequest answers 410 first).
func (h *RPCHandler) coordinationCatalog(_ context.Context, _ string) (Catalog, error) {
	return Catalog{}, errTerminalGone
}

// coordinationList is removed: the desktop serves coordination inside the KK
// channel.
func (h *RPCHandler) coordinationList(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}

// coordinationGet is removed: the desktop serves coordination inside the KK
// channel.
func (h *RPCHandler) coordinationGet(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}
