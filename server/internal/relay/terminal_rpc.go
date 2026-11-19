package relay

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"sync"
	"time"

	"github.com/google/uuid"
	"server/internal/companionsession"
)

type terminalAccessKey struct{}
type terminalAccess struct {
	lease        companionsession.Lease
	connectionID string
}
type terminalAttachment struct {
	handle       string
	owner        string
	leaseID      string
	connectionID string
	expiresAt    time.Time
	tabID        uint64
	peer         *hostPeer
	ready        bool

	// controlTail orders this handle's stream control calls to the desktop
	// (terminal.subscribe / terminal.unsubscribe). See enqueueHostControl.
	controlMu   sync.Mutex
	controlTail chan struct{}
}

// enqueueHostControl runs op after every control call queued before it for
// this handle has finished, including the desktop's reply. Sending in order
// is not enough: the desktop answers requests on parallel workers, and its
// terminal.unsubscribe is keyed by handle alone, so a late unsubscribe for a
// replaced stream would silently cancel the stream that replaced it. Callers
// enqueue from the phone socket's dispatch loop, so the queue order is the
// phone's request order; enqueueing never blocks, and every op is bounded by
// its own host call timeout.
func (a *terminalAttachment) enqueueHostControl(op func()) {
	next := make(chan struct{})
	a.controlMu.Lock()
	prev := a.controlTail
	a.controlTail = next
	a.controlMu.Unlock()
	go func() {
		defer close(next)
		if prev != nil {
			<-prev
		}
		op()
	}()
}

func isTerminalRPC(method string) bool {
	switch method {
	case "tabs.attach", "tabs.read", "tabs.input", "tabs.resize", "tabs.detach", "tabs.screen":
		return true
	}
	return false
}
func validTerminalHandle(value string) bool {
	id, err := uuid.Parse(value)
	return err == nil && id != uuid.Nil && id.String() == value
}
func terminalParams(params map[string]json.RawMessage, out any) bool {
	data, err := json.Marshal(params)
	if err != nil {
		return false
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	return decoder.Decode(out) == nil
}
func terminalFailure(err error) (any, int, string) {
	switch {
	case errors.Is(err, errTerminalInvalid):
		return nil, http.StatusBadRequest, errTerminalInvalid.Error()
	case errors.Is(err, errTerminalGone):
		return nil, http.StatusGone, errTerminalGone.Error()
	case errors.Is(err, errTerminalResponse):
		return nil, http.StatusBadGateway, errTerminalResponse.Error()
	default:
		return nil, http.StatusServiceUnavailable, errTerminalUnavailable.Error()
	}
}
func (h *RPCHandler) terminalScope(ctx context.Context, owner string) (terminalAccess, error) {
	access, ok := ctx.Value(terminalAccessKey{}).(terminalAccess)
	if !ok || access.connectionID == "" || access.lease.ID == "" || ctx.Err() != nil || !time.Now().Before(access.lease.ExpiresAt) {
		return terminalAccess{}, errTerminalGone
	}
	user, device, valid := splitOwner(owner)
	if !valid || user != access.lease.UserID || (access.lease.DeviceID != "" && access.lease.DeviceID != device) {
		return terminalAccess{}, errTerminalGone
	}
	return access, nil
}
func (h *RPCHandler) terminalCatalog(ctx context.Context, owner string, tabID uint64) error {
	store, ok := h.catalogs.(CatalogSnapshotGetter)
	if !ok {
		return errTerminalUnavailable
	}
	catalog, machine, found, err := store.GetCatalogSnapshot(ctx, owner)
	if err != nil || !found || !machine.Online {
		return errTerminalUnavailable
	}
	for _, tab := range catalog.Tabs {
		if tab.ID == tabID && (tab.Kind == TabKindTerminal || tab.Kind == TabKindAgent) {
			return nil
		}
	}
	return errTerminalGone
}
func (h *RPCHandler) terminalOnline(ctx context.Context, owner string) error {
	store, ok := h.catalogs.(MachineGetter)
	if !ok {
		return errTerminalUnavailable
	}
	machine, found, err := store.GetMachine(ctx, owner)
	if err != nil || !found || !machine.Online {
		return errTerminalUnavailable
	}
	return nil
}
func (b *HostBridge) reserve(access terminalAccess, owner string, tabID uint64) (*terminalAttachment, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	peer := b.peers[owner]
	if peer == nil || peer.ctx.Err() != nil || !time.Now().Before(peer.expiresAt) {
		return nil, errTerminalUnavailable
	}
	if b.sessionCounts[access.lease.ID] >= maxTerminalAttachments {
		return nil, errTerminalUnavailable
	}
	a := &terminalAttachment{handle: uuid.NewString(), owner: owner, leaseID: access.lease.ID, connectionID: access.connectionID, expiresAt: access.lease.ExpiresAt, tabID: tabID, peer: peer}
	b.attachments[a.handle] = a
	b.sessionCounts[a.leaseID]++
	return a, nil
}
func (b *HostBridge) removeLocked(handle string) *terminalAttachment {
	a := b.attachments[handle]
	if a == nil {
		return nil
	}
	delete(b.attachments, handle)
	b.sessionCounts[a.leaseID]--
	if b.sessionCounts[a.leaseID] <= 0 {
		delete(b.sessionCounts, a.leaseID)
	}
	// Every attachment removal ends its phone stream with an error:
	// detach, socket close, host replacement or unregistration, and lazy
	// expiry on the next lookup all funnel through here. markReleased never
	// blocks and never takes the bridge lock, so it is safe inline; it also
	// records that a later terminal.unsubscribe would be pointless.
	if sub := b.terminalStreams[handle]; sub != nil {
		delete(b.terminalStreams, handle)
		sub.markReleased(errTerminalGone.Error())
	}
	return a
}
func (b *HostBridge) commit(a *terminalAttachment) bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.attachments[a.handle] != a || b.peers[a.owner] != a.peer || a.peer.ctx.Err() != nil || !time.Now().Before(a.expiresAt) {
		return false
	}
	a.ready = true
	return true
}
func (b *HostBridge) lookup(access terminalAccess, owner, handle string) (*terminalAttachment, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	a := b.attachments[handle]
	if a == nil || a.owner != owner || a.leaseID != access.lease.ID || a.connectionID != access.connectionID || !a.ready {
		return nil, errTerminalGone
	}
	if b.peers[owner] != a.peer || a.peer.ctx.Err() != nil || !time.Now().Before(a.expiresAt) || !time.Now().Before(a.peer.expiresAt) {
		b.removeLocked(handle)
		return nil, errTerminalGone
	}
	return a, nil
}
func (b *HostBridge) active(a *terminalAttachment) bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.attachments[a.handle] == a && b.peers[a.owner] == a.peer && a.peer.ctx.Err() == nil && time.Now().Before(a.expiresAt)
}
func detachHostAttachment(a *terminalAttachment) {
	// Metadata cleanup is bounded and never retries input, replaces a PTY,
	// closes a terminal, or uses an attachment's successor host generation.
	go func() {
		ctx, stop := context.WithTimeout(context.Background(), 2*time.Second)
		defer stop()
		_, _ = a.peer.call(ctx, "terminal.detach", map[string]any{"handle": a.handle, "session_id": a.leaseID})
	}()
}
func (b *HostBridge) release(a *terminalAttachment) {
	b.mu.Lock()
	removed := b.attachments[a.handle] == a
	if removed {
		b.removeLocked(a.handle)
	}
	b.mu.Unlock()
	if removed {
		detachHostAttachment(a)
	}
}
func (b *HostBridge) detachConnection(connectionID string) {
	b.mu.Lock()
	var removed []*terminalAttachment
	for id, a := range b.attachments {
		if a.connectionID == connectionID {
			removed = append(removed, b.removeLocked(id))
		}
	}
	b.mu.Unlock()
	for _, a := range removed {
		detachHostAttachment(a)
	}
}

func (h *RPCHandler) tabsAttach(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	// E2EE cut-over (F1): the terminal stream travels as binary TERM records
	// inside the KK channel. The server JSON terminal proxy is removed.
	return goneMethod()
}
func (h *RPCHandler) terminalHandle(ctx context.Context, owner, handle string) (*terminalAttachment, error) {
	if !validTerminalHandle(handle) {
		return nil, errTerminalInvalid
	}
	access, err := h.terminalScope(ctx, owner)
	if err != nil {
		return nil, err
	}
	if err = h.terminalOnline(ctx, owner); err != nil {
		return nil, err
	}
	return h.bridge.lookup(access, owner, handle)
}
func validTerminalGrid(rows, cols uint64) bool {
	return rows >= 1 && rows <= 240 && cols >= 1 && cols <= 512 && rows*cols <= 65536
}

func (h *RPCHandler) tabsRead(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	// E2EE cut-over (F1): removed. The terminal stream travels as binary
	// TERM records inside the KK channel.
	return goneMethod()
}
func (h *RPCHandler) tabsInput(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	// E2EE cut-over (F1): removed. Input travels inside the KK channel.
	return goneMethod()
}
func (h *RPCHandler) tabsResize(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	// E2EE cut-over (F1): removed. Resize travels inside the KK channel.
	return goneMethod()
}
func (h *RPCHandler) tabsDetach(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	// E2EE cut-over (F1): removed. Detach travels inside the KK channel.
	return goneMethod()
}

type terminalScreenColor []uint16
type terminalScreenSpan struct {
	Text  string              `json:"text"`
	FG    terminalScreenColor `json:"fg,omitempty"`
	BG    terminalScreenColor `json:"bg,omitempty"`
	Flags *uint16             `json:"flags,omitempty"`
}
type terminalScreenLine struct {
	Spans []terminalScreenSpan `json:"spans"`
}
type terminalScreenDelta struct {
	Kind          string               `json:"kind"`
	Rows          *uint64              `json:"rows"`
	Cols          *uint64              `json:"cols"`
	CursorRow     *uint64              `json:"cursor_row"`
	CursorCol     *uint64              `json:"cursor_col"`
	CursorVisible *bool                `json:"cursor_visible"`
	Lines         []terminalScreenLine `json:"lines"`
}

func validScreenColor(color terminalScreenColor) bool {
	if color == nil {
		return true
	}
	if len(color) != 4 {
		return false
	}
	for _, v := range color {
		if v > 255 {
			return false
		}
	}
	return true
}
func validScreenDelta(delta *terminalScreenDelta) bool {
	if delta == nil || delta.Kind != "alt_screen" || delta.Rows == nil || delta.Cols == nil || !validTerminalGrid(*delta.Rows, *delta.Cols) || delta.CursorRow == nil || delta.CursorCol == nil || delta.CursorVisible == nil || *delta.CursorRow >= *delta.Rows || *delta.CursorCol >= *delta.Cols || delta.Lines == nil || uint64(len(delta.Lines)) > *delta.Rows {
		return false
	}
	for _, line := range delta.Lines {
		if line.Spans == nil || uint64(len(line.Spans)) > *delta.Cols {
			return false
		}
		for _, span := range line.Spans {
			if len(span.Text) > int(*delta.Cols)*64 || !validScreenColor(span.FG) || !validScreenColor(span.BG) {
				return false
			}
		}
	}
	return true
}
func (h *RPCHandler) tabsScreen(_ context.Context, _ string, _ map[string]json.RawMessage) (any, int, string) {
	// E2EE cut-over (F1): removed. Screen grids travel inside the KK
	// channel (bounded part protocol).
	return goneMethod()
}

// Terminal output streaming (phone <-> server <-> desktop host).
//
// A phone subscribes to "terminal.<handle>" over the relay socket and
// receives ordered rpc-stream frames (seq from 1) carrying the same payload
// shape tabs.read validates, plus an optional VT seed (reset:true) that
// redraws the current screen. The desktop pushes host-event frames on its
// host socket; this file validates them, buffers them per subscription, and
// forwards them on a dedicated writer goroutine so stream frames are never
// delayed behind the phone socket's serial dispatch loop (which stays ordered
// for tabs.input). Any stream failure is reported once as an rpc-reply error
// on the subscribe id; the phone then resubscribes with its current offset.
// Per-subscription buffer bounds: ~256 frames or ~2 MiB, whichever fills
// first. Overflow ends the stream with an error (never blocks the desktop
// socket, never drops frames silently). Variables only so tests can lower
// them to reach overflow with a handful of frames; production never changes
// them, and a subscription reads them once when it is created.
var (
	maxTerminalStreamFrames = 256
	maxTerminalStreamBytes  = 2 * 1024 * 1024
)

const (
	terminalStreamPrefix = "terminal."

	maxTerminalRawBytes  = 64 * 1024
	maxTerminalSeedBytes = 256 * 1024

	// terminalStreamWriteTimeout bounds one stream frame write. It is
	// deliberately longer than the dispatch socket timeout: a phone that
	// stalls for a few seconds must not silently kill its stream with no
	// error, and the stream and connection contexts still end the writer
	// first on unsubscribe, detach, or socket close.
	terminalStreamWriteTimeout = 30 * time.Second

	terminalStreamOverflowMessage = "the terminal is producing output faster than this connection can receive; resubscribe with the current offset"
	terminalStreamInvalidMessage  = "the desktop sent an invalid terminal update"
	terminalStreamFailedMessage   = "the terminal stream failed on the desktop"
	terminalStreamExitedMessage   = "the terminal has exited"
)

// terminalOutputPayload is a validated terminal.output payload in canonical
// wire form, ready to forward to the phone.
type terminalOutputPayload struct {
	canonical json.RawMessage
	base      uint64
	reset     bool
}

// validateTerminalOutputPayload checks a terminal.output payload exactly the
// way tabs.read validates its response, plus the reset/rows/cols rules: raw
// frames (reset:false) carry <= 64 KiB of base64 PTY bytes with
// base+len <= total and no viewport; seed frames (reset:true) carry <= 256
// KiB of VT bytes that reproduce the screen on a rows x cols terminal with
// base <= total (the next raw frame continues exactly at base, so base+len
// is not required to stay within total). exit_code rides only on exited
// (!alive) frames.
func validateTerminalOutputPayload(raw json.RawMessage) (*terminalOutputPayload, bool) {
	if len(bytes.TrimSpace(raw)) == 0 || string(bytes.TrimSpace(raw)) == "null" {
		return nil, false
	}
	var in struct {
		Base     *uint64 `json:"base"`
		Total    *uint64 `json:"total"`
		Data     *string `json:"data"`
		Alive    *bool   `json:"alive"`
		ExitCode *int32  `json:"exit_code"`
		Reset    *bool   `json:"reset"`
		Rows     *uint64 `json:"rows"`
		Cols     *uint64 `json:"cols"`
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if dec.Decode(&in) != nil || dec.Decode(new(any)) != io.EOF {
		return nil, false
	}
	if in.Base == nil || in.Total == nil || in.Data == nil || in.Alive == nil || in.Reset == nil {
		return nil, false
	}
	if *in.Total > maxSafeJSInteger || *in.Base > *in.Total {
		return nil, false
	}
	cap := maxTerminalRawBytes
	if *in.Reset {
		cap = maxTerminalSeedBytes
		if in.Rows == nil || in.Cols == nil || !validTerminalGrid(*in.Rows, *in.Cols) {
			return nil, false
		}
	} else if in.Rows != nil || in.Cols != nil {
		return nil, false
	}
	if len(*in.Data) > base64.StdEncoding.EncodedLen(cap) {
		return nil, false
	}
	decoded, err := base64.StdEncoding.Strict().DecodeString(*in.Data)
	if err != nil || len(decoded) > cap {
		return nil, false
	}
	if !*in.Reset && *in.Base+uint64(len(decoded)) > *in.Total {
		return nil, false
	}
	if *in.Alive && in.ExitCode != nil {
		return nil, false
	}
	out := map[string]any{"alive": *in.Alive, "base": *in.Base, "data": *in.Data, "reset": *in.Reset, "total": *in.Total}
	if in.ExitCode != nil {
		out["exit_code"] = *in.ExitCode
	}
	if *in.Reset {
		out["rows"] = *in.Rows
		out["cols"] = *in.Cols
	}
	canonical, err := json.Marshal(out)
	if err != nil {
		return nil, false
	}
	return &terminalOutputPayload{canonical: canonical, base: *in.Base, reset: *in.Reset}, true
}

// validateTerminalEndPayload checks a terminal.end payload: exactly one
// reason naming why the desktop's stream stopped.
func validateTerminalEndPayload(raw json.RawMessage) (string, bool) {
	if len(bytes.TrimSpace(raw)) == 0 {
		return "", false
	}
	var in struct {
		Reason *string `json:"reason"`
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if dec.Decode(&in) != nil || dec.Decode(new(any)) != io.EOF {
		return "", false
	}
	if in.Reason == nil {
		return "", false
	}
	switch *in.Reason {
	case "failed", "exited", "gone":
		return *in.Reason, true
	}
	return "", false
}

// decodeTerminalHostEvent strictly decodes a desktop host-event frame. Only
// well-formed JSON reaches here (invalid JSON / oversize frames are rejected
// by the host socket read path); any other deviation reports the stream as
// failed instead of closing the desktop socket.
func decodeTerminalHostEvent(data []byte) (handle, event string, payload json.RawMessage, ok bool) {
	var frame struct {
		Type    string          `json:"type"`
		V       int             `json:"v"`
		Handle  string          `json:"handle"`
		Event   string          `json:"event"`
		Payload json.RawMessage `json:"payload"`
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	if dec.Decode(&frame) != nil || dec.Decode(new(any)) != io.EOF {
		return "", "", nil, false
	}
	if frame.Type != "host-event" || frame.V != 1 || !validTerminalHandle(frame.Handle) {
		return "", "", nil, false
	}
	if frame.Event != "terminal.output" && frame.Event != "terminal.end" {
		return "", "", nil, false
	}
	if len(bytes.TrimSpace(frame.Payload)) == 0 || string(bytes.TrimSpace(frame.Payload)) == "null" {
		return "", "", nil, false
	}
	return frame.Handle, frame.Event, frame.Payload, true
}

// terminalSubscription is one phone's buffered subscription to a terminal
// handle's output. The desktop read loop enqueues validated payloads without
// blocking; a dedicated writer goroutine forwards them in order with seq
// from 1. The first failure wins and is reported once as an rpc-reply error
// on the subscribe id.
type terminalSubscription struct {
	mu               sync.Mutex
	handle           string
	stream           string
	subscribeID      string
	bridge           *HostBridge
	attachment       *terminalAttachment
	queue            chan json.RawMessage
	queuedBytes      int
	maxBytes         int
	failed           bool
	failMessage      string
	failedCh         chan struct{}
	stopped          bool
	hostConfirmed    bool
	hostUnsubscribed bool
	// released is set when the attachment leaves the bridge (detach,
	// socket close, host replacement or unregistration, lazy expiry).
	// The desktop was already told to stop via terminal.detach, or its
	// connection is dead, so a later terminal.unsubscribe would be
	// pointless: detach is the only host call.
	released bool
	// replaced is set when a newer subscribe for the same handle takes
	// over: the desktop's terminal.subscribe replaces its stream for the
	// handle, so this one needs no terminal.unsubscribe (which would only
	// race the replacement) and, if not sent yet, no terminal.subscribe.
	replaced bool
	// supersedes records that this subscription replaced an earlier one
	// whose desktop stream may still be running. If this one never takes
	// over on the desktop (its subscribe is skipped or fails), the earlier
	// stream must still be stopped.
	supersedes bool
	// offset is the output offset the phone asked to start from (nil: a
	// reset seed). started is set by the first frame that matches it; see
	// enqueueOutput.
	offset  *uint64
	started bool
}

func newTerminalSubscription(bridge *HostBridge, a *terminalAttachment, subscribeID string, offset *uint64) *terminalSubscription {
	return &terminalSubscription{
		offset:      offset,
		handle:      a.handle,
		stream:      terminalStreamPrefix + a.handle,
		subscribeID: subscribeID,
		bridge:      bridge,
		attachment:  a,
		queue:       make(chan json.RawMessage, maxTerminalStreamFrames),
		maxBytes:    maxTerminalStreamBytes,
		failedCh:    make(chan struct{}),
	}
}

func (s *terminalSubscription) failLocked(message string) {
	if s.failed {
		return
	}
	s.failed = true
	s.failMessage = message
	close(s.failedCh)
}

// fail ends the stream with an error for the phone. Idempotent: only the
// first failure is reported.
func (s *terminalSubscription) fail(message string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.failLocked(message)
}

// markReleased ends the stream because its attachment left the bridge.
// Unlike fail, it records that the desktop needs no unsubscribe: detach
// already told it to stop, or its connection is gone.
func (s *terminalSubscription) markReleased(message string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.released = true
	s.failLocked(message)
}

// enqueueOutput buffers one validated terminal.output payload, gated on the
// stream having started. Until its first frame, a subscription silently drops
// output that does not match what it asked for: with no offset only a reset
// seed starts it; with an offset, a seed or a raw frame whose base is that
// offset. Such frames are stale output of the stream this subscription
// replaced, still in flight when the desktop switched over (the desktop
// filters by subscription generation, so stale frames only ever precede the
// new stream's first frame). After the start everything is forwarded.
func (s *terminalSubscription) enqueueOutput(out *terminalOutputPayload) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.started {
		if !out.reset && (s.offset == nil || out.base != *s.offset) {
			return
		}
		s.started = true
	}
	s.enqueueLocked(out.canonical)
}

// enqueue buffers one validated payload without blocking. On overflow it
// ends the stream instead: the phone resubscribes with its current offset.
func (s *terminalSubscription) enqueue(payload json.RawMessage) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.enqueueLocked(payload)
}

func (s *terminalSubscription) enqueueLocked(payload json.RawMessage) {
	if s.failed || s.stopped {
		return
	}
	if len(s.queue) >= cap(s.queue) || s.queuedBytes+len(payload) > s.maxBytes {
		s.failLocked(terminalStreamOverflowMessage)
		return
	}
	s.queuedBytes += len(payload)
	s.queue <- payload
}

func (s *terminalSubscription) accounted(size int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.queuedBytes -= size
	if s.queuedBytes < 0 {
		s.queuedBytes = 0
	}
}

func (s *terminalSubscription) failure() (string, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.failMessage, s.failed
}

// settled reports whether the subscription already stopped or failed, so a
// late desktop answer is met with an unsubscribe instead of a stream error.
func (s *terminalSubscription) settled() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.stopped || s.failed
}

// markHostConfirmed records that the desktop accepted the subscription and
// reports whether it already settled, in which case the desktop must be told
// to stop again so no orphan stream is left behind.
func (s *terminalSubscription) markHostConfirmed() (settled bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.hostConfirmed = true
	return s.stopped || s.failed
}

// markReplaced records that a newer subscribe for the same handle took over.
func (s *terminalSubscription) markReplaced() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.replaced = true
}

// takeOver is called on the new subscription when it replaces old, before
// old is stopped.
func (s *terminalSubscription) takeOver(old *terminalSubscription) {
	old.mu.Lock()
	running := !old.released && !old.hostUnsubscribed
	old.mu.Unlock()
	old.markReplaced()
	s.mu.Lock()
	s.supersedes = running
	s.mu.Unlock()
}

func (s *terminalSubscription) markStopped() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.stopped = true
}

// unsubscribeHostIfNeeded tells the desktop to stop streaming, best-effort
// and bounded, at most once. It runs on the handle's control queue, after
// the subscription's own terminal.subscribe has been answered, so the
// confirmation it checks is final. Only confirmed subscriptions need it.
// After a release the desktop already got terminal.detach (or its
// connection is dead), and after a replacement the desktop's own
// terminal.subscribe replaces the stream: either way an unsubscribe would be
// a pointless (for replacement, harmful) second host call.
func (s *terminalSubscription) unsubscribeHostIfNeeded() {
	s.mu.Lock()
	needed := s.hostConfirmed && !s.hostUnsubscribed && !s.released && !s.replaced
	if needed {
		s.hostUnsubscribed = true
	}
	a := s.attachment
	s.mu.Unlock()
	if needed {
		unsubscribeHostAttachment(a)
	}
}

// stopSupersededHostStream stops the desktop stream of the subscription
// this one replaced, when this one never took over on the desktop. Not when
// it was itself replaced (the next subscribe takes over) or released.
func (s *terminalSubscription) stopSupersededHostStream() {
	s.mu.Lock()
	needed := s.supersedes && !s.replaced && !s.released
	s.supersedes = false
	a := s.attachment
	s.mu.Unlock()
	if needed {
		unsubscribeHostAttachment(a)
	}
}

// unsubscribeHostAttachment asks the desktop to stop a terminal stream and
// waits for its answer (bounded and best-effort like detachHostAttachment).
// It runs on the handle's control queue, never on the phone's dispatch loop:
// the phone already got its answer.
func unsubscribeHostAttachment(a *terminalAttachment) {
	ctx, stop := context.WithTimeout(context.Background(), 2*time.Second)
	defer stop()
	_, _ = a.peer.call(ctx, "terminal.unsubscribe", map[string]any{"handle": a.handle, "session_id": a.leaseID})
}

// runTerminalStream forwards queued payloads as rpc-stream frames with seq
// from 1. Writes use the connection context (not the stream context) so an
// unsubscribe lets the in-flight write finish instead of tearing down the
// shared socket, matching the catalog and events streams. When the stream
// fails, queued frames go out first, in order, then a single rpc-reply error
// on the subscribe id. A silent lease/attachment expiry is caught by the
// backstop tick; every explicit removal fails the subscription directly.
func (h *RPCHandler) runTerminalStream(connCtx, streamCtx context.Context, sub *terminalSubscription, write func(context.Context, any) error, done chan struct{}) {
	defer close(done)
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	streamFrame := func(seq uint64, payload json.RawMessage) any {
		return map[string]any{"type": rpcTypeStream, "v": rpcVersion, "stream": sub.stream, "seq": seq, "payload": payload}
	}
	sendError := func(message string) {
		if streamCtx.Err() != nil {
			return
		}
		_ = write(connCtx, map[string]any{"type": rpcTypeReply, "v": rpcVersion, "id": sub.subscribeID, "ok": false, "error": map[string]any{"message": message, "loginRequired": false}})
	}
	var seq uint64
	for {
		select {
		case <-streamCtx.Done():
			return
		case <-sub.failedCh:
			for {
				select {
				case <-streamCtx.Done():
					return
				case item := <-sub.queue:
					sub.accounted(len(item))
					seq++
					if write(connCtx, streamFrame(seq, item)) != nil {
						return
					}
				default:
					message, _ := sub.failure()
					sendError(message)
					return
				}
			}
		case item := <-sub.queue:
			sub.accounted(len(item))
			seq++
			if write(connCtx, streamFrame(seq, item)) != nil {
				return
			}
		case <-ticker.C:
			if !h.bridge.active(sub.attachment) {
				sub.fail(errTerminalGone.Error())
			}
		}
	}
}

// subscribeTerminalSocket serves an rpc-subscribe for "terminal.<handle>".
// Validation mirrors tabs.read: the stream name must equal
// "terminal."+params.handle, params carry exactly the device scope, handle
// and optional offset, and the attachment lookup checks connection, lease,
// owner, readiness and desktop presence. On success no reply is written (the
// first stream frame acknowledges, like the catalog and events streams); the
// subscription is registered before the desktop is told, so events arriving
// before the desktop's reply are buffered, not lost. The desktop round trip
// runs off the dispatch loop so a slow desktop never holds back tabs.input,
// which stays serialized on the loop in order.
func (h *RPCHandler) subscribeTerminalSocket(_ context.Context, response *replyBuffer, frame rpcFrame, _ string, _ func(context.Context, any) error, _ map[string]func()) {
	// E2EE cut-over (F1): the terminal stream travels as binary TERM records
	// inside the KK channel. Server-side stream subscriptions are removed.
	_, status, message := goneMethod()
	replyErr(response, status, frame.ID, message, false)
}

// completeTerminalSubscribe tells the desktop to start streaming. It runs
// on the handle's control queue, off the phone socket's dispatch loop:
// registration and buffering already happened, so a slow desktop delays only
// this handle's stream control, never tabs.input or other streams. A
// subscription that settled before its turn (replaced, unsubscribed, failed)
// is never sent. A failed desktop subscribe ends the stream with an error on
// the subscribe id and drops the registration.
func (h *RPCHandler) completeTerminalSubscribe(sub *terminalSubscription, offset *uint64) {
	a := sub.attachment
	if sub.settled() {
		sub.stopSupersededHostStream()
		return
	}
	params := map[string]any{"handle": a.handle, "session_id": a.leaseID}
	if offset != nil {
		params["offset"] = *offset
	}
	raw, err := a.peer.call(context.Background(), "terminal.subscribe", params)
	if err == nil {
		var result struct {
			Subscribed *bool `json:"subscribed"`
		}
		dec := json.NewDecoder(bytes.NewReader(raw))
		dec.DisallowUnknownFields()
		if dec.Decode(&result) != nil || result.Subscribed == nil || !*result.Subscribed {
			err = errTerminalResponse
		}
	}
	if err != nil {
		_, _, message := terminalFailure(err)
		if !sub.settled() {
			sub.fail(message)
			h.bridge.dropTerminalStream(sub)
		}
		sub.stopSupersededHostStream()
		return
	}
	if sub.markHostConfirmed() {
		// The subscription settled before the desktop answered: stop the
		// orphan stream, unless it was released or replaced (see
		// unsubscribeHostIfNeeded).
		sub.unsubscribeHostIfNeeded()
	}
}
