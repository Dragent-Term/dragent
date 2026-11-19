// files.list / files.search / files.open: removed after the E2EE cut-over.
// The desktop serves listings and file contents inside the KK channel; the
// relay mirror is gone (§10, F2, F3). These stubs keep the old method names
// compiling for the removed method table; they are unreachable
// (handleRequest answers 410 first).
package relay

import (
	"context"
	"encoding/json"
)

// filesList is removed: the desktop serves listings inside the KK channel.
func (h *RPCHandler) filesList(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}

// filesSearch is removed: the desktop serves search inside the KK channel.
func (h *RPCHandler) filesSearch(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}

// filesOpen is removed: the desktop serves file contents inside the KK channel.
func (h *RPCHandler) filesOpen(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	return goneMethod()
}
