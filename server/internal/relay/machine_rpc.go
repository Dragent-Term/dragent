package relay

import (
	"context"
	"encoding/json"
	"net/http"
	"time"
)

// machinesList is routing-only discovery (§2.2): desktop id, online,
// last seen, blob version, padded size. No names, no platforms, no
// content. Served from the opaque sealed store.
func (h *RPCHandler) machinesList(ctx context.Context, _ string, params map[string]json.RawMessage) (any, int, string) {
	if len(params) != 0 {
		return nil, http.StatusBadRequest, "invalid machine discovery params"
	}
	user, _ := ctx.Value("user_id").(string)
	if user == "" {
		// Socket path carries the account in the lease, not ctx; fall back
		// to the caller-supplied context the socket sets. HTTP passes
		// user_id via middleware (checked by handleRequest before dispatch).
		return nil, http.StatusUnauthorized, "sign in to view your machines"
	}
	sealed := h.sealed
	if sealed == nil {
		return nil, http.StatusServiceUnavailable, "machine discovery is unavailable"
	}
	machines, err := sealed.ListSealed(ctx, user)
	if err != nil {
		return nil, http.StatusInternalServerError, "could not load machines"
	}
	if machines == nil {
		machines = []SealedMachineSummary{}
	}
	return map[string]any{"machines": machines}, 0, ""
}

// machineCatalogSnapshot serves the opaque sealed blob over the socket
// catalog stream (routing + ciphertext, never content). The live catalog
// travels inside the KK channel; this stream carries the at-rest blob for
// offline phones. Unknown devices read as not-shared, never as an oracle.
type machineCatalogSnapshot struct {
	Catalog SealedCatalog        `json:"catalog"`
	Machine SealedMachineSummary `json:"machine"`
}

func (h *RPCHandler) machineCatalogSnapshot(ctx context.Context, user, device string) (machineCatalogSnapshot, int, string) {
	var result machineCatalogSnapshot
	owner, ok := ownerKey(user, device)
	if !ok {
		return result, http.StatusBadRequest, "a device is required"
	}
	sealed := h.sealed
	if sealed == nil {
		return result, http.StatusServiceUnavailable, "machine catalog updates are unavailable"
	}
	blob, found, err := sealed.GetSealed(ctx, owner)
	if err != nil {
		return result, http.StatusInternalServerError, "could not load machine catalog"
	}
	if !found {
		return result, http.StatusNotFound, "this machine has not shared its catalog"
	}
	body, _ := decodeSealedB64(blob.Body)
	_ = body
	machines, err := sealed.ListSealed(ctx, user)
	if err != nil {
		return result, http.StatusInternalServerError, "could not load machine catalog"
	}
	var summary SealedMachineSummary
	for _, m := range machines {
		if m.DeviceID == device {
			summary = m
		}
	}
	if summary.DeviceID == "" {
		return result, http.StatusNotFound, "this machine has not shared its catalog"
	}
	return machineCatalogSnapshot{Catalog: blob, Machine: summary}, 0, ""
}

// Catalog frames replace the complete snapshot. Polling also publishes lease
// expiry while the desktop is offline; UI never keeps treating stale state as live.
// Stream cancellation stops production, not the connection's in-flight writes.
func (h *RPCHandler) startSocketCatalog(ctx context.Context, user, device, requestID string, initial machineCatalogSnapshot, write func(context.Context, any) error) func() {
	streamCtx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		ticker := time.NewTicker(2 * time.Second)
		defer ticker.Stop()
		current := initial
		var previous string
		var seq uint64
		for {
			encoded, err := json.Marshal(current)
			if err != nil || streamCtx.Err() != nil {
				return
			}
			if string(encoded) != previous {
				seq++
				if write(ctx, map[string]any{"type": rpcTypeStream, "v": rpcVersion, "stream": "catalog", "seq": seq, "payload": current}) != nil {
					return
				}
				previous = string(encoded)
			}
			select {
			case <-streamCtx.Done():
				return
			case <-ticker.C:
				updated, _, message := h.machineCatalogSnapshot(streamCtx, user, device)
				if message != "" {
					if streamCtx.Err() == nil {
						_ = write(ctx, map[string]any{"type": rpcTypeReply, "v": rpcVersion, "id": requestID, "ok": false, "error": rpcError{Message: message}})
					}
					return
				}
				current = updated
			}
		}
	}()
	return func() { cancel(); <-done }
}
