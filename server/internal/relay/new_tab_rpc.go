package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"time"
)

// tabs.create: the phone's "+ New tab" asks the connected desktop to open a
// terminal, browser or Dragent CLI tab in one of its published workspaces.
// Only the command crosses the relay; the desktop answers the new tab's
// catalog wire id, which the phone then finds in the live catalog.

const maxNewTabWorkspaceID = 64

func validNewTabKind(kind string) bool {
	return kind == "terminal" || kind == "browser" || kind == "dragent_cli"
}

// Catalog workspace wire ids are short and alphanumeric ("w0w3").
func validNewTabWorkspaceID(id string) bool {
	if id == "" || len(id) > maxNewTabWorkspaceID {
		return false
	}
	for i := 0; i < len(id); i++ {
		c := id[i]
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') {
			return false
		}
	}
	return true
}

func newTabFailure(err error) (any, int, string) {
	switch {
	case errors.Is(err, errTerminalInvalid):
		return nil, http.StatusBadRequest, "the new tab request is invalid"
	case errors.Is(err, errTerminalGone):
		return nil, http.StatusGone, "this workspace is no longer open on the desktop"
	case errors.Is(err, errTerminalResponse):
		return nil, http.StatusBadGateway, "the desktop returned an invalid tab"
	default:
		return nil, http.StatusServiceUnavailable, "connect the desktop to open a tab"
	}
}

func newTabWorkspacePublished(catalog Catalog, id string) bool {
	for _, workspace := range catalog.Workspaces {
		if workspace.ID != nil && *workspace.ID == id {
			return true
		}
	}
	return false
}

func (h *RPCHandler) tabsCreate(ctx context.Context, owner string, params map[string]json.RawMessage) (any, int, string) {
	var input struct {
		WorkspaceID string `json:"workspace_id"`
		Kind        string `json:"kind"`
	}
	if !terminalParams(params, &input) || !validNewTabWorkspaceID(input.WorkspaceID) || !validNewTabKind(input.Kind) {
		return newTabFailure(errTerminalInvalid)
	}
	access, err := h.terminalScope(ctx, owner)
	if err != nil {
		return newTabFailure(err)
	}
	// Only a workspace this desktop published may be named; anything else
	// never reaches the desktop.
	catalog, err := h.coordinationCatalog(ctx, owner)
	if err != nil {
		return newTabFailure(err)
	}
	if !newTabWorkspacePublished(catalog, input.WorkspaceID) {
		return newTabFailure(errTerminalGone)
	}
	h.bridge.mu.Lock()
	peer := h.bridge.peers[owner]
	h.bridge.mu.Unlock()
	if peer == nil {
		return newTabFailure(errTerminalUnavailable)
	}
	raw, err := peer.call(ctx, "workspace.newTab", map[string]any{"workspace_id": input.WorkspaceID, "kind": input.Kind})
	if err != nil {
		return newTabFailure(err)
	}
	if ctx.Err() != nil || !h.bridge.current(peer) || !time.Now().Before(access.lease.ExpiresAt) {
		return newTabFailure(errTerminalGone)
	}
	var result struct {
		ID *uint64 `json:"id"`
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&result) != nil || result.ID == nil || *result.ID > maxSafeJSInteger {
		return newTabFailure(errTerminalResponse)
	}
	return map[string]any{"id": *result.ID}, 0, ""
}
