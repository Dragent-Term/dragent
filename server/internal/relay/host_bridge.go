package relay

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

const (
	maxHostPending         = 128
	maxHostReplyBytes      = 512 * 1024
	hostRequestTimeout     = 10 * time.Second
	maxTerminalAttachments = 4
)

var (
	errTerminalUnavailable = errors.New("connect the desktop to use this terminal")
	errTerminalGone        = errors.New("this terminal attachment is no longer available")
	errTerminalInvalid     = errors.New("the terminal request is invalid")
	errTerminalResponse    = errors.New("the desktop returned an invalid terminal response")
)

type hostResult struct {
	result json.RawMessage
	err    error
}
type hostOutbound struct {
	ctx  context.Context
	data []byte
}
type hostPeer struct {
	bridge    *HostBridge
	owner     string
	conn      *websocket.Conn
	expiresAt time.Time
	ctx       context.Context
	cancel    context.CancelFunc
	writes    chan hostOutbound
	mu        sync.Mutex
	pending   map[string]chan hostResult
	closed    bool
	once      sync.Once
}

// HostBridge is an in-process authenticated desktop broker, not a PTY owner.
// Replacing an outbound connection never replays a request to its successor.
// Multi-instance host routing requires a separate distributed adapter.
type HostBridge struct {
	mu              sync.Mutex
	peers           map[string]*hostPeer
	attachments     map[string]*terminalAttachment
	sessionCounts   map[string]int
	terminalStreams map[string]*terminalSubscription
}

func NewHostBridge() *HostBridge {
	return &HostBridge{peers: make(map[string]*hostPeer), attachments: make(map[string]*terminalAttachment), sessionCounts: make(map[string]int), terminalStreams: make(map[string]*terminalSubscription)}
}
func (b *HostBridge) current(peer *hostPeer) bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.peers[peer.owner] == peer && peer.ctx.Err() == nil && time.Now().Before(peer.expiresAt)
}
func (b *HostBridge) register(peer *hostPeer) bool {
	b.mu.Lock()
	previous := b.peers[peer.owner]
	if previous == nil {
		user, _, _ := splitOwner(peer.owner)
		count := 0
		for owner := range b.peers {
			existingUser, _, _ := splitOwner(owner)
			if existingUser == user {
				count++
			}
		}
		if count >= maxMachinesPerAccount {
			b.mu.Unlock()
			return false
		}
	}
	b.peers[peer.owner] = peer
	for id, a := range b.attachments {
		if a.peer == previous {
			b.removeLocked(id)
		}
	}
	b.mu.Unlock()
	if previous != nil {
		previous.shutdown()
	}
	return true
}
func (b *HostBridge) unregister(peer *hostPeer) {
	b.mu.Lock()
	if b.peers[peer.owner] == peer {
		delete(b.peers, peer.owner)
	}
	for id, a := range b.attachments {
		if a.peer == peer {
			b.removeLocked(id)
		}
	}
	b.mu.Unlock()
	peer.shutdown()
}
func (p *hostPeer) shutdown() {
	p.once.Do(func() {
		p.cancel()
		p.mu.Lock()
		p.closed = true
		pending := p.pending
		p.pending = make(map[string]chan hostResult)
		p.mu.Unlock()
		for _, waiter := range pending {
			waiter <- hostResult{err: errTerminalUnavailable}
		}
		p.conn.CloseNow()
	})
}
func allowedHostMethod(method string) bool {
	switch method {
	case "terminal.attach", "terminal.read", "terminal.input", "terminal.resize", "terminal.detach", "terminal.screen", "terminal.subscribe", "terminal.unsubscribe", "coordination.list", "coordination.get", "workspace.newTab":
		return true
	}
	return false
}
func (p *hostPeer) call(ctx context.Context, method string, params any) (json.RawMessage, error) {
	if !allowedHostMethod(method) || !p.bridge.current(p) {
		return nil, errTerminalUnavailable
	}
	if err := ctx.Err(); err != nil {
		return nil, errTerminalUnavailable
	}
	body, err := json.Marshal(params)
	if err != nil || len(body) > maxParamsBytes {
		return nil, errTerminalInvalid
	}
	callCtx, stop := context.WithTimeout(ctx, hostRequestTimeout)
	defer stop()
	id := uuid.NewString()
	data, err := json.Marshal(map[string]any{"type": "host-request", "v": 1, "id": id, "method": method, "params": json.RawMessage(body)})
	if err != nil {
		return nil, errTerminalInvalid
	}
	reply := make(chan hostResult, 1)
	p.mu.Lock()
	if p.closed || len(p.pending) >= maxHostPending {
		p.mu.Unlock()
		return nil, errTerminalUnavailable
	}
	p.pending[id] = reply
	p.mu.Unlock()
	defer func() { p.mu.Lock(); delete(p.pending, id); p.mu.Unlock() }()
	select {
	case p.writes <- hostOutbound{ctx: callCtx, data: data}:
	default:
		return nil, errTerminalUnavailable
	}
	select {
	case value := <-reply:
		if !p.bridge.current(p) || callCtx.Err() != nil {
			return nil, errTerminalUnavailable
		}
		return value.result, value.err
	case <-callCtx.Done():
		return nil, errTerminalUnavailable
	case <-p.ctx.Done():
		return nil, errTerminalUnavailable
	}
}
func (p *hostPeer) writeLoop() {
	for {
		select {
		case <-p.ctx.Done():
			return
		case outbound := <-p.writes:
			// A canceled caller may have been queued, but is never sent/replayed.
			if outbound.ctx.Err() != nil {
				continue
			}
			if !time.Now().Before(p.expiresAt) {
				return
			}
			deadline, stop := context.WithTimeout(p.ctx, 5*time.Second)
			err := p.conn.Write(deadline, websocket.MessageText, outbound.data)
			stop()
			if err != nil {
				p.shutdown()
				return
			}
		}
	}
}
func (p *hostPeer) receive(data []byte) error {
	if !utf8.Valid(data) || len(data) > maxHostReplyBytes {
		return errTerminalResponse
	}
	var header struct {
		Type string `json:"type"`
		V    int    `json:"v"`
	}
	if json.Unmarshal(data, &header) != nil {
		return errTerminalResponse
	}
	switch header.Type {
	case "host-reply":
		return p.receiveReply(data)
	case "host-event":
		// Push frames never close the desktop socket for payload issues:
		// only frames that are not valid JSON (above) or oversize keep the
		// current strictness. Anything else ends at most one phone stream.
		p.receiveEvent(data)
		return nil
	default:
		return errTerminalResponse
	}
}

// terminalStreamFailureReason maps a desktop terminal.end reason to the
// phone-visible stream error.
func terminalStreamFailureReason(reason string) string {
	switch reason {
	case "exited":
		return terminalStreamExitedMessage
	case "gone":
		return errTerminalGone.Error()
	default:
		return terminalStreamFailedMessage
	}
}

// receiveEvent routes one desktop host-event push frame to its phone stream.
// Events for an unknown handle, or for a handle with no subscription, are
// ignored. Invalid payloads end that stream with an error. It never fails
// the host socket: the caller already rejected non-JSON and oversize frames.
func (p *hostPeer) receiveEvent(data []byte) {
	handle, event, payload, ok := decodeTerminalHostEvent(data)
	if !ok {
		var probe struct {
			Handle string `json:"handle"`
		}
		if json.Unmarshal(data, &probe) == nil {
			if sub := p.bridge.terminalStream(probe.Handle); sub != nil {
				sub.fail(terminalStreamInvalidMessage)
			}
		}
		return
	}
	sub := p.bridge.terminalStreamFor(p, handle)
	if sub == nil {
		return
	}
	switch event {
	case "terminal.output":
		out, valid := validateTerminalOutputPayload(payload)
		if !valid {
			sub.fail(terminalStreamInvalidMessage)
			return
		}
		sub.enqueueOutput(out)
	case "terminal.end":
		reason, valid := validateTerminalEndPayload(payload)
		if !valid {
			sub.fail(terminalStreamInvalidMessage)
			return
		}
		sub.fail(terminalStreamFailureReason(reason))
	}
}

// addTerminalStream registers a phone stream before the desktop is told, so
// events arriving before the desktop's reply are buffered, not lost.
func (b *HostBridge) addTerminalStream(sub *terminalSubscription) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.terminalStreams[sub.handle] = sub
}

func (b *HostBridge) dropTerminalStream(sub *terminalSubscription) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.terminalStreams[sub.handle] == sub {
		delete(b.terminalStreams, sub.handle)
	}
}

func (b *HostBridge) terminalStream(handle string) *terminalSubscription {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.terminalStreams[handle]
}

// terminalStreamFor returns the phone stream for handle only when it still
// belongs to this peer's live attachment. Events from a replaced or foreign
// connection are ignored, never routed across generations.
func (b *HostBridge) terminalStreamFor(peer *hostPeer, handle string) *terminalSubscription {
	b.mu.Lock()
	defer b.mu.Unlock()
	sub := b.terminalStreams[handle]
	if sub == nil {
		return nil
	}
	a := b.attachments[handle]
	if a == nil || a != sub.attachment || a.peer != peer || b.peers[a.owner] != peer {
		return nil
	}
	return sub
}
func (p *hostPeer) receiveReply(data []byte) error {
	var frame struct {
		Type   string          `json:"type"`
		V      int             `json:"v"`
		ID     string          `json:"id"`
		OK     *bool           `json:"ok"`
		Result json.RawMessage `json:"result"`
		Error  struct {
			Code string `json:"code"`
		} `json:"error"`
	}
	if json.Unmarshal(data, &frame) != nil || frame.Type != "host-reply" || frame.V != 1 || !validTerminalHandle(frame.ID) || frame.OK == nil {
		return errTerminalResponse
	}
	value := hostResult{}
	if *frame.OK {
		if len(frame.Result) == 0 || string(frame.Result) == "null" {
			value.err = errTerminalResponse
		} else {
			value.result = append(json.RawMessage(nil), frame.Result...)
		}
	} else {
		switch frame.Error.Code {
		case "invalid":
			value.err = errTerminalInvalid
		case "gone":
			value.err = errTerminalGone
		default:
			value.err = errTerminalUnavailable
		}
	}
	p.mu.Lock()
	waiter := p.pending[frame.ID]
	delete(p.pending, frame.ID)
	p.mu.Unlock()
	if waiter != nil {
		waiter <- value
	} // Late/canceled request replies are ignored.
	return nil
}

// RegisterHostRoutes uses first-frame JWT auth, outside public share admission
// and ordinary HTTP timeouts. It exposes no daemon token, path, or process API.
func (h *RPCHandler) RegisterHostRoutes(r chi.Router) { r.Get("/v1/relay/host", h.ServeHost) }
func (h *RPCHandler) ServeHost(w http.ResponseWriter, r *http.Request) {
	if r.URL.RawQuery != "" || h.authenticate == nil {
		writeErr(w, http.StatusBadRequest, "invalid desktop relay address")
		return
	}
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{OriginPatterns: []string{"*"}})
	if err != nil {
		return
	}
	defer conn.CloseNow()
	conn.SetReadLimit(maxHostReplyBytes)
	authCtx, stopAuth := context.WithTimeout(r.Context(), 10*time.Second)
	kind, data, err := conn.Read(authCtx)
	var auth struct {
		Type         string          `json:"type"`
		V            int             `json:"v"`
		Token        string          `json:"access_token"`
		E2E          json.RawMessage `json:"e2e,omitempty"`
		Device       string          `json:"device_id"`
		ExpectedUser string          `json:"expected_user_id"`
	}
	var identity SocketIdentity
	if err == nil && kind == websocket.MessageText && utf8.Valid(data) && json.Unmarshal(data, &auth) == nil && auth.Type == "relay-host-auth" && auth.V == 1 && len(auth.Token) > 0 && len(auth.Token) <= 16*1024 {
		identity, err = h.authenticate(authCtx, auth.Token)
	} else {
		err = errTerminalInvalid
	}
	stopAuth()
	auth.Token = ""
	owner, valid := ownerKey(identity.UserID, auth.Device)
	if err != nil || !valid || auth.ExpectedUser != identity.UserID || !identity.ExpiresAt.After(time.Now()) || !validHostDevice(auth.Device) {
		deadline, stop := context.WithTimeout(r.Context(), time.Second)
		raw, _ := json.Marshal(map[string]any{"type": "relay-error", "v": 1, "message": "Sign in to connect this desktop.", "loginRequired": true})
		_ = conn.Write(deadline, websocket.MessageText, raw)
		stop()
		_ = conn.Close(websocket.StatusCode(4401), "authentication required")
		return
	}
	// E2EE cut-over (§10): old desktops without e2e:1 are refused before
	// admission. They never get relay-host-ready.
	if !parseE2EMarker(auth.E2E) {
		deadline, stop := context.WithTimeout(r.Context(), time.Second)
		raw, _ := json.Marshal(map[string]any{"type": "relay-error", "v": 1, "message": e2eeUpdateDesktopMessage, "loginRequired": false, "updateRequired": true})
		_ = conn.Write(deadline, websocket.MessageText, raw)
		stop()
		_ = conn.Close(e2eeRefusedCode, "e2ee required")
		return
	}
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	peer := &hostPeer{bridge: h.bridge, owner: owner, conn: conn, expiresAt: identity.ExpiresAt, ctx: ctx, cancel: cancel, writes: make(chan hostOutbound, maxHostPending), pending: make(map[string]chan hostResult)}
	// A known owner publication plus an independent peer cap prevents
	// invented device ids/catalogue eviction from growing live peers forever.
	if h.catalogs == nil {
		_ = conn.Close(websocket.StatusCode(4403), "publish this desktop before connecting")
		return
	}
	_, known, catalogErr := h.catalogs.Get(ctx, owner)
	if catalogErr != nil || !known {
		_ = conn.Close(websocket.StatusCode(4403), "publish this desktop before connecting")
		return
	}
	if !h.bridge.register(peer) {
		_ = conn.Close(websocket.StatusTryAgainLater, "too many desktop connections")
		return
	}
	defer h.bridge.unregister(peer)
	expiry := time.AfterFunc(time.Until(identity.ExpiresAt), func() { _ = conn.Close(websocket.StatusCode(4401), "authentication expired"); peer.shutdown() })
	defer expiry.Stop()
	// E2EE binary endpoint, so phone HS1 forwards can reach this desktop.
	// It's registered before the desktop hears it's ready: a phone frame
	// sent the moment the desktop is admitted must find it, not be dropped.
	// What it carries waits until "ready" has gone out, which the desktop
	// reads first.
	readySent := make(chan struct{})
	dom := e2eeFor(h)
	hep := &e2eeEndpoint{account: identity.UserID, device: auth.Device, role: E2EERoleHost, write: func(c context.Context, p []byte) error {
		deadline, stop := context.WithTimeout(c, 5*time.Second)
		defer stop()
		select {
		case <-readySent:
		case <-deadline.Done():
			return deadline.Err()
		}
		return conn.Write(deadline, websocket.MessageBinary, p)
	}}
	dom.put(E2EERoleHost, identity.UserID, auth.Device, hep)
	defer dom.drop(E2EERoleHost, identity.UserID, auth.Device, hep)
	ready, _ := json.Marshal(map[string]any{"type": "relay-host-ready", "v": 1, "access_expires_at": identity.ExpiresAt.UnixMilli()})
	deadline, stop := context.WithTimeout(ctx, 5*time.Second)
	err = conn.Write(deadline, websocket.MessageText, ready)
	stop()
	if err != nil {
		return
	}
	close(readySent)
	go peer.writeLoop()
	// Channels carried by this desktop socket end with it.
	defer dom.router.CloseEndpoint(hep)
	for {
		kind, data, err := conn.Read(ctx)
		if err != nil {
			return
		}
		if !time.Now().Before(identity.ExpiresAt) {
			_ = conn.Close(websocket.StatusCode(4401), "authentication expired")
			return
		}
		if kind == websocket.MessageBinary {
			if h.handleHostBinary(ctx, conn, peer, data, identity.UserID, dom) {
				return
			}
			continue
		}
		if kind != websocket.MessageText {
			_ = conn.Close(websocket.StatusUnsupportedData, "text frames required")
			return
		}
		// host-notify intake (§8): a validated opaque blob for push dispatch.
		// Invalid intake never fails the desktop socket — only the aggregate
		// counter records it — since blob failures surface as the placeholder.
		if isHostNotifyFrame(data) {
			h.handleHostNotify(ctx, dom, identity.UserID, data)
			continue
		}
		var header struct {
			Type string `json:"type"`
			V    int    `json:"v"`
		}
		if json.Unmarshal(data, &header) == nil && header.Type == "relay-ping" && header.V == 1 {
			pong, _ := json.Marshal(map[string]any{"type": "relay-pong", "v": 1})
			select {
			case peer.writes <- hostOutbound{ctx: ctx, data: pong}:
			default:
				return
			}
			continue
		}
		if peer.receive(data) != nil {
			_ = conn.Close(websocket.StatusPolicyViolation, "invalid desktop relay reply")
			return
		}
	}
}
func validHostDevice(device string) bool {
	if device == "" || len(device) > 128 || !utf8.ValidString(device) {
		return false
	}
	for _, ch := range device {
		if ch <= 32 || ch == 127 {
			return false
		}
	}
	return true
}

// isHostNotifyFrame peeks at a text frame's type without parsing content.
func isHostNotifyFrame(data []byte) bool {
	var header struct {
		Type string `json:"type"`
		V    int    `json:"v"`
	}
	return json.Unmarshal(data, &header) == nil && header.Type == TextHostNotify && header.V == 1
}

// validNotifyBlob accepts only the exact sealed push size (§8: 1,120 bytes
// seal as 1,494 base64url chars). Length and alphabet are checked; the blob
// itself stays opaque to the relay and never reaches logs.
func validNotifyBlob(blob string) bool {
	if len(blob) != MaxE2EENotifyBlobChars {
		return false
	}
	for i := 0; i < len(blob); i++ {
		c := blob[i]
		if c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' || c == '_' {
			continue
		}
		return false
	}
	return true
}

// handleHostNotify validates one host-notify envelope and hands the opaque
// blob to the push intake (E9b). Invalid envelopes are counted and dropped;
// the desktop socket is never failed for intake payload issues.
func (h *RPCHandler) handleHostNotify(ctx context.Context, dom *e2eeDomain, accountID string, data []byte) {
	var frame struct {
		Type    string `json:"type"`
		V       int    `json:"v"`
		PhoneID string `json:"phone_id"`
		Blob    string `json:"blob"`
	}
	if json.Unmarshal(data, &frame) != nil || frame.Type != TextHostNotify || frame.V != 1 ||
		!validHostDevice(frame.PhoneID) || !validNotifyBlob(frame.Blob) {
		dom.stats.Record("e2ee-notify", "refused", "le4k", "bad-notify")
		return
	}
	dom.stats.Record("e2ee-notify", "ok", SizeBucket(len(frame.Blob)), "none")
	dom.policyMu.Lock()
	intake := dom.notify
	dom.policyMu.Unlock()
	if intake != nil {
		_ = intake(ctx, accountID, frame.PhoneID, frame.Blob)
	}
}

// handleHostBinary routes one binary E2EE frame from a desktop socket. The
// desktop never initiates (HS1 is phone-only, §4.2); HS step 1 is refused.
// Channel failures send relay-chan-closed through the host write queue and
// keep the socket open. Returns true when the socket itself is dead.
func (h *RPCHandler) handleHostBinary(ctx context.Context, conn *websocket.Conn, peer *hostPeer, raw []byte, accountID string, dom *e2eeDomain) bool {
	_, desktop, ok := splitOwner(peer.owner)
	if !ok {
		_ = conn.Close(websocket.StatusPolicyViolation, "invalid desktop relay reply")
		return true
	}
	frame, err := DecodeE2EEFrame(raw)
	if err != nil {
		dom.stats.Record("e2ee-binary", "refused", SizeBucket(len(raw)), "bad-header")
		_ = conn.Close(websocket.StatusInvalidFramePayloadData, "invalid e2ee frame")
		return true
	}
	binding, routeErr := dom.router.RouteHost(frame, accountID, desktop)
	if routeErr != nil {
		h.writeHostChanClosed(peer, ctx, frame.Chan, e2eeChanClosedReason(routeErr))
		return false
	}
	if dst := dom.otherSide(binding, desktop); dst != nil {
		if werr := dst.write(ctx, raw); werr != nil {
			h.writeHostChanClosed(peer, ctx, frame.Chan, "backpressure")
			dom.router.Close(frame.Chan)
		} else if frame.IsClose() {
			dom.router.Close(frame.Chan)
		}
	} else {
		h.writeHostChanClosed(peer, ctx, frame.Chan, "unavailable")
		dom.router.Close(frame.Chan)
	}
	return false
}

// writeHostChanClosed queues one relay-chan-closed text frame to the desktop
// without blocking the read loop: a full queue drops the notice (counted in
// router stats) but never fails the socket — other channels stay live.
func (h *RPCHandler) writeHostChanClosed(peer *hostPeer, ctx context.Context, chanID [16]byte, reason string) {
	payload, err := json.Marshal(RelayChanClosedFrame(ChanHex(chanID), reason))
	if err != nil {
		return
	}
	select {
	case peer.writes <- hostOutbound{ctx: ctx, data: payload}:
	default:
	}
}
