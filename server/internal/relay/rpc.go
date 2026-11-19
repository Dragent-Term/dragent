// RPC transport for the R2 relay host-data methods.
//
// The R0 envelope (RPC_ENVELOPE.md) defines the wire frames the phone and the
// relay speak: rpc-request → rpc-reply for calls, rpc-subscribe → rpc-stream
// for the runtime event stream. This file implements that envelope on the
// server side over HTTP, so the frames stay byte-identical to a future socket
// transport:
//
//	POST /v1/relay/rpc with {"type":"rpc-request",...} → {"type":"rpc-reply",...}
//	POST /v1/relay/rpc with {"type":"rpc-subscribe",...} → SSE of rpc-stream frames
//	POST /v1/relay/rpc with {"type":"rpc-unsubscribe",...} → {"type":"rpc-reply",...}
//
// Authorization follows the spec's server rules: every method and stream
// checks the same-account controller first. The routes must be mounted under
// RequireAuth; the handler additionally rejects requests whose context carries
// no user id (anonymous → 401 semantics with loginRequired:true). A device id
// that another account published under reads as foreign (403,
// loginRequired:false); a device id nobody published reads as 404, never as
// an oracle for other accounts' machines. Owner scoping reuses ownerKey, so a
// caller can only ever address their own account's state.
//
// Payload caps (spec): params JSON ≤ 64KB; list results paginated (limit
// default 50, max 200); file content truncated with an explicit flag (see
// files.go).
package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/go-chi/chi/v5"

	"server/internal/companionsession"
)

// Envelope frame types and version (R0 spec § wire frames).
const (
	rpcTypeRequest     = "rpc-request"
	rpcTypeReply       = "rpc-reply"
	rpcTypeSubscribe   = "rpc-subscribe"
	rpcTypeUnsubscribe = "rpc-unsubscribe"
	rpcTypeStream      = "rpc-stream"

	rpcVersion = 1
)

// Caps from the spec's server rules.
const (
	// maxParamsBytes bounds the params object of one RPC frame (spec: 64KB).
	maxParamsBytes = 64 * 1024
	// maxFrameBodyBytes bounds the whole HTTP body: params plus envelope
	// slack. Anything larger cannot be a legal frame.
	maxFrameBodyBytes = 128 * 1024
	// defaultLimit / maxLimit paginate every list result (spec).
	defaultLimit = 50
	maxLimit     = 200
	// maxIDLength bounds client-supplied correlation ids and stream handles.
	maxIDLength = 128
)

// RPCHandler serves the relay RPC surface. After the E2EE cut-over (§10)
// the server is a blind router: every companion/desktop method except
// routing-only discovery travels inside the KK channel, served by the
// desktop. States holds the removed host mirror (unwritten; PublishState is
// 410); Events is the neutered attention bus (record drops, subscribe is
// empty); catalogs is the legacy admission store (content-free stubs only);
// sealed carries the opaque catalog blobs machines.list reads.
type RPCHandler struct {
	bridge       *HostBridge
	states       HostStore
	events       *EventBus
	commands     HostCommandExecutor
	catalogs     Store
	sealed       SealedStore
	providers    HostProviderExecutor
	authenticate SocketAuthenticator
	// Logical companion access is distinct from public terminal share links.
	sessions            companionsession.Store
	sessionPollInterval time.Duration
	// Paired-phone device and lease tracking: revoking a phone or lease
	// closes exactly its controller sockets (see companion_devices.go). Nil
	// authorizer keeps the safety poll to the companion lease only.
	pairedDevices    *deviceConnRegistry
	deviceAuthorizer DeviceAuthorizer
	// The process-wide revocation subscription (SetRevocationSource).
	revocationMu    sync.Mutex
	stopRevocations func()
	// recheckSpread overrides companionRecheckSpread (tests).
	recheckSpread time.Duration
}

// NewRPCHandler returns an RPCHandler backed by states (or an in-memory
// store) and events (or a fresh bus).
func NewRPCHandler(states HostStore, events *EventBus) *RPCHandler {
	if states == nil {
		states = NewMemoryHostStore()
	}
	if events == nil {
		events = NewEventBus()
	}
	return &RPCHandler{bridge: NewHostBridge(), states: states, events: events, sealed: NewMemorySealedStore(), sessions: companionsession.NewMemoryStore(), sessionPollInterval: companionSafetyPollInterval, pairedDevices: newDeviceConnRegistry()}
}

// SetSealedStore installs the opaque catalog store machines.list reads.
// Main wires the same store the catalog Handler publishes to; tests do the
// same. Nil keeps the previous store.
func (h *RPCHandler) SetSealedStore(s SealedStore) {
	if s != nil {
		h.sealed = s
	}
}

// RegisterRPCRoutes mounts the removed plaintext endpoints. E2EE cut-over
// (§10.5): POST /v1/relay/rpc (and its SSE subscribe path) and
// PUT /v1/relay/host/state return 410 before the body is read. Mount under
// RequireAuth: the 410 still requires authentication so the route is not an
// oracle.
func (h *RPCHandler) RegisterRPCRoutes(r chi.Router) {
	r.Post("/v1/relay/rpc", h.HandleFrame)
	r.Put("/v1/relay/host/state", h.PublishState)
}

// goneRPC answers a removed plaintext relay route: 410 without reading the
// body. §10.5 requires the status before any body byte is consumed.
func goneRPC(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusGone)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": "relay rpc is end-to-end encrypted: update the app"})
}

// rpcFrame is the shared envelope header of every inbound frame.
type rpcFrame struct {
	Type   string          `json:"type"`
	V      int             `json:"v"`
	ID     string          `json:"id"`
	Method string          `json:"method,omitempty"`
	Stream string          `json:"stream,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
}

// rpcError is the user-safe error payload of an rpc-reply. Message never
// carries tokens, credentialed URLs, or internal errors.
type rpcError struct {
	Message       string `json:"message"`
	LoginRequired bool   `json:"loginRequired"`
}

// writeReply answers an rpc-request (or unsubscribe) with an rpc-reply frame.
// The HTTP status mirrors the outcome so mobile status mapping (401/403/404)
// keeps working; the body is always a well-formed reply frame.
func writeReply(w http.ResponseWriter, status int, id string, ok bool, result any, rpcErr *rpcError) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	body := map[string]any{"type": rpcTypeReply, "v": rpcVersion, "id": id, "ok": ok}
	if ok {
		if result == nil {
			result = map[string]any{}
		}
		body["result"] = result
	} else {
		body["error"] = rpcErr
	}
	_ = json.NewEncoder(w).Encode(body)
}

func replyOK(w http.ResponseWriter, id string, result any) {
	writeReply(w, http.StatusOK, id, true, result, nil)
}

func replyErr(w http.ResponseWriter, status int, id, msg string, loginRequired bool) {
	writeReply(w, status, id, false, nil, &rpcError{Message: msg, LoginRequired: loginRequired})
}

// transportErr is for frames that cannot be answered with a reply (no usable
// id): malformed JSON, missing id, oversize body. The shape still carries
// loginRequired so mobile maps it like any other auth failure.
func transportErr(w http.ResponseWriter, status int, msg string, loginRequired bool) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{"error": msg, "loginRequired": loginRequired})
}

// callerUserID returns the authenticated account id, following the existing
// relay pattern (context "user_id" injected by auth middleware / tests).
func callerUserID(r *http.Request) (string, bool) {
	userID, _ := r.Context().Value("user_id").(string)
	if userID == "" {
		return "", false
	}
	return userID, true
}

// deviceParams extracts the routing device_id shared by every R2 method's
// params. Method-specific params ride alongside it. Absent params decode as
// an empty object (every method table entry shows `{}` as valid).
func deviceParams(raw json.RawMessage) (deviceID string, rest map[string]json.RawMessage, err error) {
	if len(bytes.TrimSpace(raw)) == 0 {
		return "", map[string]json.RawMessage{}, nil
	}
	var params map[string]json.RawMessage
	dec := json.NewDecoder(bytes.NewReader(raw))
	if err := dec.Decode(&params); err != nil {
		return "", nil, err
	}
	if params == nil {
		params = map[string]json.RawMessage{}
	}
	var device string
	if rawDevice, ok := params["device_id"]; ok {
		if err := json.Unmarshal(rawDevice, &device); err != nil {
			return "", nil, err
		}
		delete(params, "device_id")
	}
	return strings.TrimSpace(device), params, nil
}

// resolveOwner gates one RPC call on same-account device ownership. It
// returns the caller's device-scoped owner for method handlers, or writes the
// reply itself and returns ok=false.
func (h *RPCHandler) resolveOwner(w http.ResponseWriter, r *http.Request, id, deviceID string) (owner string, ok bool) {
	if deviceID == "" || len(deviceID) > 128 {
		replyErr(w, http.StatusBadRequest, id, "a device is required", false)
		return "", false
	}
	userID, authed := callerUserID(r)
	if !authed {
		// Unreachable under RequireAuth; fail closed regardless.
		replyErr(w, http.StatusUnauthorized, id, "login required for relay", true)
		return "", false
	}
	owner, valid := ownerKey(userID, deviceID)
	if !valid {
		replyErr(w, http.StatusBadRequest, id, "invalid device", false)
		return "", false
	}
	users, err := h.states.DeviceUsers(r.Context(), deviceID)
	if err != nil {
		replyErr(w, http.StatusInternalServerError, id, "could not load relay state", false)
		return "", false
	}
	if len(users) == 0 {
		replyErr(w, http.StatusNotFound, id, "this machine has not shared its state", false)
		return "", false
	}
	for _, u := range users {
		if u == userID {
			return owner, true
		}
	}
	replyErr(w, http.StatusForbidden, id, "this machine belongs to another account", false)
	return "", false
}

// methodFunc implements one R2 method: owner is the caller's device-scoped
// owner (already authorized); params are the method params minus device_id.
// On failure it returns the HTTP status for the reply plus a user-safe
// message.
type methodFunc func(ctx context.Context, owner string, params map[string]json.RawMessage) (result any, status int, errMsg string)

// methods is the post-cut-over method table: only routing-only discovery
// is served by the relay. Every companion content method (tabs, files,
// worktrees, terminal, coordination, providers, catalog.read) travels
// inside the KK channel, served by the desktop — the server answers 410.
func (h *RPCHandler) methods() map[string]methodFunc {
	return map[string]methodFunc{
		"machines.list": h.machinesList,
	}
}

// goneMethod answers a removed content method over an already-parsed
// frame (socket transport): 410 with a user-safe repair message. The HTTP
// transport never reaches here — HandleFrame is 410 before the body.
func goneMethod() (any, int, string) {
	return nil, http.StatusGone, "this relay method is end-to-end encrypted: update the app"
}

// HandleFrame answers the removed HTTP RPC transport: 410 before the body
// is read (§10.5). No frame byte is consumed, parsed, or logged.
func (h *RPCHandler) HandleFrame(w http.ResponseWriter, r *http.Request) {
	if _, authed := callerUserID(r); !authed {
		transportErr(w, http.StatusUnauthorized, "login required for relay", true)
		return
	}
	goneRPC(w, nil)
}

func (h *RPCHandler) handleRequest(w http.ResponseWriter, r *http.Request, frame rpcFrame) {
	method := strings.TrimSpace(frame.Method)
	if method == "" {
		replyErr(w, http.StatusBadRequest, frame.ID, "a method is required", false)
		return
	}
	if len(frame.Params) > maxParamsBytes {
		replyErr(w, http.StatusBadRequest, frame.ID, "request params are too large", false)
		return
	}
	// Removed content methods fail closed with 410 over already-parsed
	// socket frames. The HTTP transport never reaches here (HandleFrame is
	// 410 before the body). machines.list stays: routing-only discovery.
	if method != "machines.list" {
		if method == eventSubscribeMethod || method == "catalog.read" ||
			isTerminalRPC(method) || method == "tabs.list" ||
			method == "tabs.createTerminal" || method == "tabs.create" ||
			method == "worktree.list" || method == "worktree.create" ||
			method == "files.list" || method == "files.search" ||
			method == "files.open" || method == "coordination.list" ||
			method == "coordination.get" || method == "github.listWorkItems" ||
			method == "gitlab.listWorkItems" || method == "linear.listWorkItems" {
			_, status, msg := goneMethod()
			replyErr(w, status, frame.ID, msg, false)
			return
		}
		replyErr(w, http.StatusNotFound, frame.ID, "unknown method", false)
		return
	}
	if _, authed := callerUserID(r); !authed {
		replyErr(w, http.StatusUnauthorized, frame.ID, "login required for relay", true)
		return
	}
	// machines.list is account-scoped discovery: no device selection.
	fn := h.methods()["machines.list"]
	result, status, errMsg := fn(r.Context(), "", nil)
	if errMsg != "" {
		replyErr(w, status, frame.ID, errMsg, false)
		return
	}
	replyOK(w, frame.ID, result)
}

// limitParam parses the shared pagination limit: default 50, max 200 (spec).
// Non-numeric limits are malformed params.
func limitParam(params map[string]json.RawMessage) (int, string) {
	raw, ok := params["limit"]
	if !ok || len(bytes.TrimSpace(raw)) == 0 || string(bytes.TrimSpace(raw)) == "null" {
		return defaultLimit, ""
	}
	var limit int
	if err := json.Unmarshal(raw, &limit); err != nil || limit <= 0 {
		return 0, "invalid limit"
	}
	if limit > maxLimit {
		limit = maxLimit
	}
	return limit, ""
}

// paginate caps a result list at limit (spec: limit default 50, max 200).
func paginate[T any](items []T, limit int) []T {
	if len(items) > limit {
		return items[:limit]
	}
	return items
}
