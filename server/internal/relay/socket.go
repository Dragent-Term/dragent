package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/companionsession"
)

// SocketIdentity comes only from a verified account token, never a frame's
// claimed user/device. Expiry bounds a socket even when it stays connected.
// DeviceID is the token's device claim (the paired phone for companion
// scopes, empty for tokens without one) and keys revocation tracking.
type SocketIdentity struct {
	UserID    string
	DeviceID  string
	ExpiresAt time.Time
}
type SocketAuthenticator func(context.Context, string) (SocketIdentity, error)

// Provider credentials live on the selected authorized desktop. The owner
// adapter must select that host; a process-wide cloud CLI is never a fallback.
type HostProviderExecutor interface {
	ForOwner(context.Context, string) (ProviderExecutor, error)
}

// ConfigureSocket is called once, before serving. Providers and commands are
// optional authenticated HOST adapters. A nil adapter never runs cloud CLIs or
// simulates creating something on the desktop.
func (h *RPCHandler) ConfigureSocket(auth SocketAuthenticator, catalogs Store, providers HostProviderExecutor) {
	h.authenticate, h.catalogs, h.providers = auth, catalogs, providers
}

// parseE2EMarker accepts only the exact JSON number 1. Missing markers,
// zeros, strings, bools, and floats all fail closed (§10: stripping e2e:1
// from a new client's auth frame fails closed).
func parseE2EMarker(raw json.RawMessage) bool {
	if len(raw) == 0 {
		return false
	}
	var n int
	if err := json.Unmarshal(raw, &n); err != nil || n != 1 {
		return false
	}
	return true
}

// E2EE admission refusal (§10): 4403 is the cut-over refusal code, distinct
// from 4401 (bad/expired credentials) and 4408 (session ended). The messages
// are the §9 user-facing strings; the flags drive the client's repair UX.
// Nothing sensitive is included: no ids, tokens, or device names.
const e2eeRefusedCode = websocket.StatusCode(4403)

const (
	e2eeUpdatePhoneMessage   = "Update the Dragent app on this phone."
	e2eeRepairPhoneMessage   = "Pair this phone again to keep using it. Phone access is now end-to-end encrypted."
	e2eeUpdateDesktopMessage = "Update Dragent on this desktop to connect."
)

// writeE2EERefusal sends one relay-error and closes 4403. It never sends
// relay-ready: refused builds latch authRejected and stop retrying.
func writeE2EERefusal(ctx context.Context, conn *websocket.Conn, write func(context.Context, any) error, message string, updateRequired, repairRequired bool) {
	_ = write(ctx, map[string]any{
		"type": "relay-error", "v": 1, "message": message,
		"loginRequired": false, "updateRequired": updateRequired, "repairRequired": repairRequired,
	})
	closePhoneSocket(conn, e2eeRefusedCode, "e2ee required")
}

// e2eeChanClosedReason maps a routing failure to the relay-chan-closed
// reason hint: "refused" for validation/cap failures, "unavailable" when the
// other side is offline, "backpressure" for rate caps and slow readers.
// CLOSE reasons are unauthenticated hints; a forged one only reconnects.
func e2eeChanClosedReason(err error) string {
	if err == nil {
		return "closed"
	}
	if errors.Is(err, ErrE2EEChannelCap) {
		return "backpressure"
	}
	if errors.Is(err, ErrE2EEUnknownChannel) || errors.Is(err, ErrE2EEWrongLease) || errors.Is(err, ErrE2EEWrongPeer) {
		if errors.Is(err, ErrE2EEUnknownChannel) {
			return "unavailable"
		}
		return "refused"
	}
	return "refused"
}

// socketWriteBinaryFunc writes raw E2EE frames as binary messages, bounded by
// timeout. Concurrent use is safe: coder/websocket serializes writes, so
// per-channel order is preserved across forwarding goroutines.
func socketWriteBinaryFunc(conn *websocket.Conn, wireCtx context.Context, timeout time.Duration) func(context.Context, []byte) error {
	return func(ctx context.Context, raw []byte) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		deadline, cancel := context.WithTimeout(wireCtx, timeout)
		defer cancel()
		return conn.Write(deadline, websocket.MessageBinary, raw)
	}
}

// socketWriteFunc serializes frames as JSON and writes them as text
// messages, bounded by timeout. Concurrent use is safe: coder/websocket
// serializes writes internally, which the catalog and events streams
// already rely on.
func socketWriteFunc(conn *websocket.Conn, wireCtx context.Context, timeout time.Duration) func(context.Context, any) error {
	return func(ctx context.Context, frame any) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		data, err := json.Marshal(frame)
		if err != nil {
			return err
		}
		deadline, cancel := context.WithTimeout(wireCtx, timeout)
		defer cancel()
		return conn.Write(deadline, websocket.MessageText, data)
	}
}

// RegisterSocketRoutes lives outside HTTP request timeouts and RequireAuth:
// browsers cannot set upgrade headers. The first frame MUST authenticate before
// any RPC is dispatched. Share-session capabilities grant no access here.
func (h *RPCHandler) RegisterSocketRoutes(r chi.Router) {
	r.Get("/v1/relay/socket", h.ServeSocket)
}

func (h *RPCHandler) readCatalogRPC(ctx context.Context, owner string, _ map[string]json.RawMessage) (any, int, string) {
	if h.catalogs == nil {
		return nil, http.StatusServiceUnavailable, "this machine has not shared its catalog"
	}
	catalog, ok, err := h.catalogs.Get(ctx, owner)
	if err != nil {
		return nil, http.StatusInternalServerError, "could not load relay catalog"
	}
	if !ok {
		return nil, http.StatusNotFound, "this machine has not shared its catalog"
	}
	return catalog.Normalized(), 0, ""
}

func (h *RPCHandler) providerMethod(method string) methodFunc {
	return func(ctx context.Context, owner string, params map[string]json.RawMessage) (any, int, string) {
		body, err := json.Marshal(params)
		if err != nil {
			return nil, http.StatusBadRequest, "invalid request params"
		}
		// The caller/device have already passed resolveOwner. No provider
		// credentials cross this boundary and production has no cloud CLI fallback.
		var provider ProviderExecutor
		if h.providers != nil {
			provider, err = h.providers.ForOwner(ctx, owner)
			if err != nil {
				return nil, http.StatusBadGateway, "connect the desktop to search provider work items"
			}
		}
		reply := DispatchProviderRPC(ctx, ProviderRPCRequest{Type: rpcTypeRequest, V: rpcVersion, ID: "dispatch", Method: method, Params: body}, provider, nil)
		if !reply.OK {
			return nil, reply.Error.Status, reply.Error.Message
		}
		return reply.Result, 0, ""
	}
}

// replyBuffer adapts the existing HTTP method dispatch to one socket reply, so
// authorization, validation, result readers and error classification stay shared.
type replyBuffer struct {
	body   bytes.Buffer
	header http.Header
}

func newReplyBuffer() *replyBuffer                 { return &replyBuffer{header: make(http.Header)} }
func (w *replyBuffer) Header() http.Header         { return w.header }
func (w *replyBuffer) WriteHeader(int)             {}
func (w *replyBuffer) Write(p []byte) (int, error) { return w.body.Write(p) }

func (h *RPCHandler) ServeSocket(w http.ResponseWriter, r *http.Request) {
	if r.URL.RawQuery != "" || h.authenticate == nil {
		writeErr(w, http.StatusBadRequest, "relay socket unavailable or invalid address")
		return
	}
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{OriginPatterns: []string{"*"}})
	if err != nil {
		return
	}
	defer conn.CloseNow()
	conn.SetReadLimit(maxFrameBodyBytes)
	wireCtx, cancelWire := context.WithCancel(r.Context())
	defer cancelWire()
	// Stream frames ride a longer write timeout than dispatch replies: a
	// phone that stalls for a few seconds (tunnel, handoff, loaded test
	// box) must not silently kill a live terminal stream with no error to
	// the phone. The timeout still bounds a dead connection; stream and
	// connection contexts end the writer first.
	write := socketWriteFunc(conn, wireCtx, 5*time.Second)
	terminalWrite := socketWriteFunc(conn, wireCtx, terminalStreamWriteTimeout)
	authCtx, cancelAuth := context.WithTimeout(r.Context(), 10*time.Second)
	kind, raw, err := conn.Read(authCtx)
	var auth struct {
		Type     string          `json:"type"`
		V        int             `json:"v"`
		Token    string          `json:"access_token"`
		E2E      json.RawMessage `json:"e2e,omitempty"`
		DeviceID json.RawMessage `json:"device_id,omitempty"`
		ResumeID json.RawMessage `json:"resume_session_id,omitempty"`
	}
	var identity SocketIdentity
	if err == nil && kind == websocket.MessageText && json.Unmarshal(raw, &auth) == nil && auth.Type == "relay-auth" && auth.V == 1 && len(auth.Token) > 0 && len(auth.Token) <= 16*1024 {
		identity, err = h.authenticate(authCtx, auth.Token)
	} else {
		err = errors.New("invalid auth")
	}
	cancelAuth()
	if err != nil || identity.UserID == "" || !identity.ExpiresAt.After(time.Now()) {
		_ = write(r.Context(), map[string]any{"type": "relay-error", "v": 1, "message": "Sign in to connect to this machine.", "loginRequired": true})
		closePhoneSocket(conn, websocket.StatusCode(4401), "authentication required")
		return
	}
	// E2EE cut-over (§10): old phone builds without e2e:1 are refused before
	// admission with relay-error + 4403. They never get relay-ready, and the
	// shipped client latches 4403 as authRejected (no silent retry).
	// Stripping e2e:1 (missing, zero, or wrong-typed) fails closed the same way.
	if !parseE2EMarker(auth.E2E) {
		writeE2EERefusal(r.Context(), conn, write, e2eeUpdatePhoneMessage, true, false)
		return
	}
	// Pre-cut-over e2e:false devices pair again (§10): same refusal shape
	// with repairRequired. Unknown devices skip this check — JWT validity
	// and revocation still apply — E2b wires the pairing-store policy.
	if identity.DeviceID != "" {
		if dom := e2eeFor(h); dom != nil {
			dom.policyMu.Lock()
			policy := dom.policy
			dom.policyMu.Unlock()
			if policy != nil {
				if known, e2e := policy(r.Context(), identity.UserID, identity.DeviceID); known && !e2e {
					writeE2EERefusal(r.Context(), conn, write, e2eeRepairPhoneMessage, false, true)
					return
				}
			}
		}
	}
	// Optional scope is interpreted only after the account JWT was verified.
	auth.Token = ""
	deviceID, scopeErr := companionAuthField(auth.DeviceID, true)
	resumeID, resumeErr := companionAuthField(auth.ResumeID, false)
	if scopeErr != nil || resumeErr != nil {
		writeCompanionAdmissionError(wireCtx, conn, write, companionsession.ErrUnavailable)
		return
	}
	lease, leaseErr := h.admitCompanionSession(wireCtx, identity, deviceID, resumeID)
	if !identity.ExpiresAt.After(time.Now()) {
		_ = write(wireCtx, map[string]any{"type": "relay-error", "v": 1, "message": "Sign in to connect to this machine.", "loginRequired": true})
		closePhoneSocket(conn, websocket.StatusCode(4401), "authentication expired")
		return
	}
	if leaseErr != nil {
		writeCompanionAdmissionError(wireCtx, conn, write, leaseErr)
		return
	}
	if !lease.ExpiresAt.After(time.Now()) {
		writeCompanionAdmissionError(wireCtx, conn, write, companionsession.ErrUnavailable)
		return
	}
	deadline, deadlineCode := identity.ExpiresAt, websocket.StatusCode(4401)
	if !lease.ExpiresAt.After(deadline) {
		deadline, deadlineCode = lease.ExpiresAt, companionSessionEndedCode
	}
	workCtx, cancelWork := context.WithDeadline(wireCtx, deadline)
	defer cancelWork()
	ctx := context.WithValue(workCtx, "user_id", identity.UserID)
	connectionID := uuid.NewString()
	ctx = context.WithValue(ctx, terminalAccessKey{}, terminalAccess{lease: lease, connectionID: connectionID})
	defer h.bridge.detachConnection(connectionID)
	var closeOnce sync.Once
	endAccess := func(code websocket.StatusCode, reason string) {
		closeOnce.Do(func() {
			// Cancel host work immediately. Close on the wire context so the
			// credential/session close code survives operation cancellation.
			cancelWork()
			closePhoneSocket(conn, code, reason)
			cancelWire()
		})
	}
	// endAccess runs on other goroutines too (revocation, expiry, poll) and
	// cancels work before it writes the close frame, so this handler can
	// return early on that cancellation (e.g. the relay-ready write) while
	// the close is still in flight. Wait for it here, ahead of the deferred
	// CloseNow, so the phone always gets its close code instead of EOF; once
	// the handler is leaving, later ends are no-ops.
	defer closeOnce.Do(func() {})
	expiry := time.AfterFunc(time.Until(deadline), func() { endAccess(deadlineCode, "companion access expired") })
	defer expiry.Stop()
	// Revocation of this phone or lease: event-driven close (local DELETE or
	// a published revocation from any instance) plus a slow safety poll.
	defer h.watchCompanionAccess(ctx, lease, identity, endAccess)()
	request := r.WithContext(ctx)
	ready := map[string]any{"type": "relay-ready", "v": 1, "session_id": lease.ID,
		"session_expires_at": lease.ExpiresAt.UnixMilli(), "access_expires_at": identity.ExpiresAt.UnixMilli()}
	if write(ctx, ready) != nil {
		return
	}
	// E2EE binary endpoint: registered when the token carries a UUID phone
	// device, so a desktop HS1 forward (or host-initiated check) can find
	// this socket. Sockets without a device claim stay initiate-capable but
	// unreachable — discovery phones open channels, they don't receive them.
	dom := e2eeFor(h)
	writeBinary := socketWriteBinaryFunc(conn, wireCtx, 5*time.Second)
	// This socket's own endpoint: channels it opens send their replies here.
	self := &e2eeEndpoint{account: identity.UserID, device: identity.DeviceID, role: E2EERolePhone, write: writeBinary}
	if _, ok := DeviceIDToRaw(identity.DeviceID); ok && identity.DeviceID != "" {
		dom.put(E2EERolePhone, identity.UserID, identity.DeviceID, self)
		defer dom.drop(E2EERolePhone, identity.UserID, identity.DeviceID, self)
	}
	// Channels this socket opened end with it (and free their cap slots).
	defer dom.router.CloseEndpoint(self)
	streams := make(map[string]func())
	defer func() {
		for _, stop := range streams {
			stop()
		}
	}()
	// Keep reading while a host RPC is pending, so a lost phone cancels the
	// operation and discards queued input promptly. Dispatch stays ordered.
	type inboundFrame struct {
		kind websocket.MessageType
		data []byte
	}
	inbound := make(chan inboundFrame, maxHostPending)
	go func() {
		for {
			kind, data, err := conn.Read(wireCtx)
			if err != nil {
				// The phone closed it, or the network did (a close the
				// relay started was logged where it was made).
				if wireCtx.Err() == nil {
					logPhoneSocketEnd("phone", int(websocket.CloseStatus(err)), readEndClass(err))
				}
				cancelWire()
				return
			}
			select {
			case inbound <- inboundFrame{kind: kind, data: data}:
			case <-wireCtx.Done():
				return
			default:
				endAccess(websocket.StatusPolicyViolation, "too many queued relay requests")
				return
			}
		}
	}()
	for {
		var kind websocket.MessageType
		var data []byte
		select {
		case frame := <-inbound:
			kind, data = frame.kind, frame.data
		case <-wireCtx.Done():
			return
		}
		if ctx.Err() != nil {
			endAccess(deadlineCode, "companion access expired")
			return
		}
		if kind == websocket.MessageBinary {
			if h.handlePhoneBinary(ctx, wireCtx, conn, write, data, lease, identity, dom, self) {
				return
			}
			continue
		}
		if kind != websocket.MessageText {
			closePhoneSocket(conn, websocket.StatusUnsupportedData, "text frames required")
			return
		}
		var frame rpcFrame
		if json.Unmarshal(data, &frame) != nil {
			closePhoneSocket(conn, websocket.StatusInvalidFramePayloadData, "invalid relay frame")
			return
		}
		if frame.Type == "relay-ping" && frame.V == rpcVersion {
			if write(ctx, map[string]any{"type": "relay-pong", "v": 1}) != nil {
				return
			}
			continue
		}
		id, idErr := uuid.Parse(frame.ID)
		if frame.V != rpcVersion || idErr != nil || id == uuid.Nil || len(frame.ID) != 36 || len(frame.Params) > maxParamsBytes {
			closePhoneSocket(conn, websocket.StatusPolicyViolation, "invalid relay envelope")
			return
		}
		response := newReplyBuffer()
		// A selected-desktop lease cannot be repurposed by changing params
		// on a later frame. Discovery leases retain the legacy per-call scope.
		if lease.DeviceID != "" && (frame.Type == rpcTypeRequest || frame.Type == rpcTypeSubscribe) {
			device, _, err := deviceParams(frame.Params)
			if err != nil || device != lease.DeviceID {
				replyErr(response, http.StatusForbidden, frame.ID, "this companion session belongs to another desktop", false)
				if writeRawRelayReply(wireCtx, conn, response) != nil {
					return
				}
				continue
			}
		}
		if err := h.checkCompanionSession(ctx, lease); err != nil {
			if !companionSessionGone(err) && time.Now().Before(deadline) {
				// Storage didn't answer: refuse this request, keep the
				// socket (the poll gives up if it stays unanswered).
				replyErr(response, http.StatusServiceUnavailable, frame.ID, "the companion service is temporarily unavailable", false)
				if writeRawRelayReply(wireCtx, conn, response) != nil {
					return
				}
				continue
			}
			code := companionFailureCode(err)
			if !time.Now().Before(deadline) {
				code = deadlineCode
			}
			endAccess(code, "companion session unavailable")
			return
		}
		revoked := false
		switch frame.Type {
		case rpcTypeRequest:
			if frame.Method == "companion.session.revoke" {
				revoked = h.revokeSocketCompanion(response, ctx, frame, lease)
			} else {
				h.handleRequest(response, request, frame)
			}
		case rpcTypeSubscribe:
			if frame.Stream == "catalog" {
				device, _, err := deviceParams(frame.Params)
				if err != nil {
					replyErr(response, http.StatusBadRequest, frame.ID, "invalid request params", false)
					break
				}
				initial, status, message := h.machineCatalogSnapshot(ctx, identity.UserID, device)
				if message != "" {
					replyErr(response, status, frame.ID, message, false)
					break
				}
				if stop := streams[frame.Stream]; stop != nil {
					stop()
				}
				streams[frame.Stream] = h.startSocketCatalog(ctx, identity.UserID, device, frame.ID, initial, write)
				break
			}
			if strings.HasPrefix(frame.Stream, terminalStreamPrefix) {
				h.subscribeTerminalSocket(ctx, response, frame, identity.UserID, terminalWrite, streams)
				break
			}
			if frame.Stream != eventStreamName {
				replyErr(response, http.StatusNotFound, frame.ID, "unknown stream", false)
				break
			}
			device, _, err := deviceParams(frame.Params)
			if err != nil {
				replyErr(response, http.StatusBadRequest, frame.ID, "invalid request params", false)
				break
			}
			if _, ok := h.resolveOwner(response, request, frame.ID, device); !ok {
				break
			}
			if stop := streams[frame.Stream]; stop != nil {
				stop()
			}
			streams[frame.Stream] = h.startSocketEvents(ctx, identity.UserID, write)
		case rpcTypeUnsubscribe:
			if stop := streams[frame.Stream]; stop != nil {
				stop()
				delete(streams, frame.Stream)
			}
			replyOK(response, frame.ID, map[string]bool{"unsubscribed": true})
		default:
			replyErr(response, http.StatusBadRequest, frame.ID, "unknown frame type", false)
		}
		if response.body.Len() > 0 {
			if ctx.Err() != nil {
				endAccess(deadlineCode, "companion access expired")
				return
			}
			if writeRawRelayReply(wireCtx, conn, response) != nil {
				return
			}
		}
		if revoked {
			endAccess(companionSessionEndedCode, "companion session revoked")
			return
		}
	}
}

func (h *RPCHandler) startSocketEvents(ctx context.Context, user string, write func(context.Context, any) error) func() {
	streamCtx, cancel := context.WithCancel(ctx)
	handle, snapshot, events, unsubscribe := h.events.Subscribe(user)
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer unsubscribe()
		send := func(seq uint64, payload any) error {
			if streamCtx.Err() != nil {
				return streamCtx.Err()
			}
			// Canceling an in-flight coder/websocket Write closes the entire
			// connection. Unsubscribing only stops this stream: let its current
			// bounded write finish before stopping or replacing its snapshot.
			return write(ctx, map[string]any{"type": rpcTypeStream, "v": rpcVersion, "stream": eventStreamName, "seq": seq, "payload": payload})
		}
		if send(1, map[string]any{"events": snapshot}) != nil {
			return
		}
		for {
			select {
			case <-streamCtx.Done():
				return
			case <-h.events.doneFor(handle, user):
				return
			case event := <-events:
				if streamCtx.Err() != nil {
					return
				}
				if send(event.seq, event.event) != nil {
					return
				}
			}
		}
	}()
	return func() { cancel(); <-done }
}

// handlePhoneBinary routes one binary E2EE frame from a companion socket.
// It returns true when the socket itself is dead (framing violation) and the
// caller must stop. Channel-level failures send relay-chan-closed and keep
// the socket open for other channels. Frames forward unmodified, except HS1
// whose peer field the relay rewrites to the initiator id (see RegisterHS1).

func (h *RPCHandler) handlePhoneBinary(ctx, wireCtx context.Context, conn *websocket.Conn, write func(context.Context, any) error, raw []byte, lease companionsession.Lease, identity SocketIdentity, dom *e2eeDomain, self *e2eeEndpoint) bool {
	frame, err := DecodeE2EEFrame(raw)
	if err != nil {
		dom.stats.Record("e2ee-binary", "refused", SizeBucket(len(raw)), "bad-header")
		closePhoneSocket(conn, websocket.StatusInvalidFramePayloadData, "invalid e2ee frame")
		return true
	}
	srcDevice := identity.DeviceID
	if frame.IsHS() && frame.Step == 1 {
		if _, ok := DeviceIDToRaw(srcDevice); !ok || srcDevice == "" {
			dom.stats.Record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "bad-device")
			_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), "refused"))
			return false
		}
		isSame := func(target string) bool {
			if h.catalogs == nil {
				return false
			}
			owner, ok := ownerKey(identity.UserID, target)
			if !ok {
				return false
			}
			_, known, _ := h.catalogs.Get(ctx, owner)
			return known
		}
		binding, forward, regErr := dom.router.RegisterHS1(frame, raw, lease.ID, identity.UserID, lease.DeviceID, srcDevice, E2EERolePhone, isSame)
		if regErr != nil {
			_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), e2eeChanClosedReason(regErr)))
			return false
		}
		binding.initiatorEP.Store(self)
		if dst := dom.otherSide(binding, srcDevice); dst != nil {
			binding.targetEP.Store(dst)
			if werr := dst.write(wireCtx, forward); werr != nil {
				_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), "backpressure"))
				dom.router.Close(frame.Chan)
			} else if frame.IsClose() {
				dom.router.Close(frame.Chan)
			}
		} else {
			_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), "unavailable"))
			dom.router.Close(frame.Chan)
		}
		return false
	}
	binding, routeErr := dom.router.Route(frame, lease.ID, srcDevice, E2EERolePhone)
	if routeErr != nil {
		_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), e2eeChanClosedReason(routeErr)))
		return false
	}
	if dst := dom.otherSide(binding, srcDevice); dst != nil {
		if werr := dst.write(wireCtx, raw); werr != nil {
			_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), "backpressure"))
			dom.router.Close(frame.Chan)
		} else if frame.IsClose() {
			dom.router.Close(frame.Chan)
		}
	} else {
		_ = write(ctx, RelayChanClosedFrame(ChanHex(frame.Chan), "unavailable"))
		dom.router.Close(frame.Chan)
	}
	return false
}

// closePhoneSocket closes a phone's socket and logs why, as aggregates do:
// the code and the fixed reason string only, never ids or content. Without
// this, every ended phone socket showed only as a 5xx in the request log.
func closePhoneSocket(conn *websocket.Conn, code websocket.StatusCode, reason string) {
	logPhoneSocketEnd("relay", int(code), reason)
	_ = conn.Close(code, reason)
}

func logPhoneSocketEnd(side string, code int, reason string) {
	slog.Info("relay phone socket ended", "side", side, "code", code, "reason", reason)
}

// readEndClass names how a phone's read ended without exposing its text.
func readEndClass(err error) string {
	switch {
	case websocket.CloseStatus(err) != -1:
		return "close frame"
	case errors.Is(err, context.Canceled), errors.Is(err, context.DeadlineExceeded):
		return "context ended"
	default:
		return "network"
	}
}
