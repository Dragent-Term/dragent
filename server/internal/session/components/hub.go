package components

import (
	"context"
	"encoding/json"
	"log/slog"
	"math"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	"server/internal/session/types"
)

// Sink receives protocol frames destined for one connection. The WebSocket
// handler implements it; tests use a fake. Send must not block the caller.
type Sink interface {
	Send(data []byte)
}

// channelFor is the bus channel carrying all traffic for one session.
func channelFor(sessionID string) string { return "session:" + sessionID }

// busFrame is the internal envelope carried on the bus. Either a protocol frame
// to deliver (Ctrl == "") or a control notice about membership.
type busFrame struct {
	// Destination for protocol frames: "host", "viewers", or "viewer".
	To string `json:"to,omitempty"`
	// Participant id when To == "viewer".
	Target string `json:"target,omitempty"`
	// "", "join", or "leave".
	Ctrl string `json:"ctrl,omitempty"`
	// The protocol frame (when Ctrl == "").
	Data json.RawMessage `json:"data,omitempty"`
	// Membership payload (when Ctrl == "join"/"leave").
	Part *types.Participant `json:"part,omitempty"`
	// Exec request (when Ctrl == "exec"): the requesting participant + command.
	From    string `json:"from,omitempty"`
	Command string `json:"cmd,omitempty"`
	// Agent request (when Ctrl == "agent_query"/"agent_stop"). RequestID is
	// supplied by the viewer so reconnect retries can be deduplicated.
	RequestID string `json:"rid,omitempty"`
	Query     string `json:"query,omitempty"`
	// Raw input bytes, base64 (when Ctrl == "input"): keystrokes to forward to
	// the host's PTY. The requesting participant travels in From.
	Input string `json:"in,omitempty"`
	// PTY resize request (when Ctrl == "resize"): the controller's grid. The
	// requesting participant travels in From.
	Rows uint16 `json:"rows,omitempty"`
	Cols uint16 `json:"cols,omitempty"`
	// Controller input/scroll surface, opaque (when Ctrl == "control"): the
	// exact `surface` payload the viewer sent, forwarded verbatim to the host.
	// The requesting participant travels in From.
	Surface json.RawMessage `json:"surf,omitempty"`
	// Normalized pointer presence (when Ctrl == "pointer"). Unlike Surface,
	// this is accepted from view-only participants as well as controllers.
	Pointer *types.PointerPosition `json:"pointer,omitempty"`
	// Owning account of a joining participant (Ctrl == "join"), so the host
	// instance can auto-approve commands from its own user's other devices.
	User string `json:"u,omitempty"`
}

// pendingCmd is a remote command awaiting the host's approval.
type pendingCmd struct {
	from    string
	command string
}

// room is one session's local presence on this instance.
type room struct {
	mu         sync.Mutex
	host       Sink                         // local host sink, if the host is here
	hostUser   string                       // owning account (host instance only)
	hostName   string                       // host display name for viewer presence
	hostAvatar string                       // host avatar URL for viewer presence
	viewers    map[string]Sink              // participantID -> local viewer sink
	people     map[string]types.Participant // authoritative list (host instance only)
	userBy     map[string]string            // participantID -> owning account (host instance only)
	pending    map[string]pendingCmd        // request id -> pending command (host instance only)
	agentSeen  map[string]time.Time         // viewer request id -> accepted time (host instance only)
	history    [][]byte                     // recent viewer frames for late-joining mirrors
	sub        Subscription
	refs       int
	endReason  string // reason the host ended (e.g. inactivity); shown to viewers
	// Bumped whenever the host attaches or detaches. A pending reconnect-grace
	// timer compares against the value it captured: if they differ, the host came
	// back (or left again) and that timer must not end the session.
	hostEpoch uint64
	forwards  map[string]PortForward
	waiters   map[string]chan types.ServerForwardResponse
	wsWaiters map[string]chan types.ServerForwardWebSocketEvent
}

// Hub registers connections and bridges them via the Bus, enforcing remote-
// command roles and rate limits and recording an audit trail.
type Hub struct {
	bus     Bus
	limiter RateLimiter
	auditor Auditor
	mu      sync.Mutex
	rooms   map[string]*room
}

func marshalForward(v any) []byte {
	b, _ := json.Marshal(v)
	return b
}

func NewHub(bus Bus, limiter RateLimiter, auditor Auditor) *Hub {
	return &Hub{bus: bus, limiter: limiter, auditor: auditor, rooms: make(map[string]*room)}
}

// roomFor returns the room for a session, creating it (and its subscription +
// dispatch loop) on first use. Caller must not hold r.mu.
func (h *Hub) roomFor(sessionID string) *room {
	h.mu.Lock()
	defer h.mu.Unlock()
	if r, ok := h.rooms[sessionID]; ok {
		return r
	}
	sub, _ := h.bus.Subscribe(channelFor(sessionID))
	r := &room{
		viewers:   make(map[string]Sink),
		people:    make(map[string]types.Participant),
		userBy:    make(map[string]string),
		pending:   make(map[string]pendingCmd),
		agentSeen: make(map[string]time.Time),
		history:   make([][]byte, 0, 256),
		forwards:  make(map[string]PortForward),
		waiters:   make(map[string]chan types.ServerForwardResponse),
		wsWaiters: make(map[string]chan types.ServerForwardWebSocketEvent),
		sub:       sub,
	}
	h.rooms[sessionID] = r
	go h.dispatch(sessionID, r)
	return r
}

// dispatch delivers inbound bus messages to local connections.
func (h *Hub) dispatch(sessionID string, r *room) {
	for data := range r.sub.C() {
		var f busFrame
		if json.Unmarshal(data, &f) != nil {
			continue
		}
		switch f.Ctrl {
		case "join", "leave":
			h.applyMembership(sessionID, r, f)
		case "exec":
			h.handleExec(sessionID, r, f.From, f.Command)
		case "agent_query":
			h.handleAgentQuery(sessionID, r, f.From, f.RequestID, f.Query)
		case "agent_stop":
			h.handleAgentStop(sessionID, r, f.From)
		case "input":
			h.handleInput(sessionID, r, f.From, f.Input)
		case "resize":
			h.handleResize(sessionID, r, f.From, f.Rows, f.Cols)
		case "control":
			h.handleControl(sessionID, r, f.From, f.Surface)
		case "pointer":
			h.handlePointer(sessionID, r, f.From, f.Pointer)
		case "forward_request":
			h.handleForwardRequest(sessionID, r, f)
		case "forward_response":
			h.handleForwardResponse(r, f)
		case "forward_ws_open", "forward_ws_data", "forward_ws_close":
			h.handleForwardWebSocketToHost(r, f)
		case "forward_ws_event":
			h.handleForwardWebSocketEvent(r, f)
		case "access":
			h.handleAccess(sessionID, r, f.From)
		default:
			r.mu.Lock()
			if f.To == "viewers" && len(f.Data) > 0 {
				r.history = append(r.history, append([]byte(nil), f.Data...))
				if len(r.history) > 2048 {
					r.history = r.history[len(r.history)-2048:]
				}
			}
			switch f.To {
			case "viewers":
				for _, v := range r.viewers {
					v.Send(f.Data)
				}
			case "viewer":
				if v, ok := r.viewers[f.Target]; ok {
					v.Send(f.Data)
				}
			case "host":
				if r.host != nil {
					r.host.Send(f.Data)
				}
			}
			r.mu.Unlock()
		}
	}
}

func (h *Hub) RequestForwardWebSocket(ctx context.Context, sessionID string, req types.ServerForwardWebSocketOpen) (<-chan types.ServerForwardWebSocketEvent, error) {
	r := h.roomFor(sessionID)
	if req.ForwardID == "" || req.RequestID == "" {
		return nil, ErrForwardNotFound
	}
	ch := make(chan types.ServerForwardWebSocketEvent, 32)
	r.mu.Lock()
	r.wsWaiters[req.RequestID] = ch
	r.mu.Unlock()
	h.publish(ctx, sessionID, busFrame{Ctrl: "forward_ws_open", Command: string(marshalForward(req))})
	return ch, nil
}

func (h *Hub) ForwardWebSocketData(ctx context.Context, sessionID, requestID, messageType, data string) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "forward_ws_data", Command: string(marshalForward(types.ServerForwardWebSocketData{Type: types.TypeForwardWebSocketData, RequestID: requestID, MessageType: messageType, Data: data}))})
}

func (h *Hub) SendForwardWebSocketToHost(ctx context.Context, sessionID string, frame []byte) {
	var envelope struct {
		Type string `json:"type"`
	}
	if json.Unmarshal(frame, &envelope) != nil {
		return
	}
	ctrl := "forward_ws_data"
	if envelope.Type == types.TypeForwardWebSocketOpen {
		ctrl = "forward_ws_open"
	} else if envelope.Type == types.TypeForwardWebSocketClose {
		ctrl = "forward_ws_close"
	}
	h.publish(ctx, sessionID, busFrame{Ctrl: ctrl, Command: string(frame)})
}

func (h *Hub) CloseForwardWebSocket(ctx context.Context, sessionID, requestID string) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "forward_ws_close", Command: string(marshalForward(types.ServerForwardWebSocketClose{Type: types.TypeForwardWebSocketClose, RequestID: requestID}))})
}

func (h *Hub) handleForwardWebSocketToHost(r *room, f busFrame) {
	r.mu.Lock()
	host := r.host
	r.mu.Unlock()
	if host != nil {
		host.Send([]byte(f.Command))
	}
}

func (h *Hub) handleForwardWebSocketEvent(r *room, f busFrame) {
	var event types.ServerForwardWebSocketEvent
	if json.Unmarshal([]byte(f.Command), &event) != nil {
		return
	}
	r.mu.Lock()
	ch := r.wsWaiters[event.RequestID]
	if event.Closed {
		delete(r.wsWaiters, event.RequestID)
	}
	r.mu.Unlock()
	if ch != nil {
		ch <- event
		if event.Closed {
			close(ch)
		}
	}
}

func (h *Hub) ForwardWebSocketEvent(ctx context.Context, sessionID string, event types.ServerForwardWebSocketEvent) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "forward_ws_event", Command: string(marshalForward(event))})
}

func (h *Hub) RegisterForward(sessionID string, f PortForward) {
	r := h.roomFor(sessionID)
	r.mu.Lock()
	r.forwards[f.ID] = f
	r.mu.Unlock()
}

func (h *Hub) UnregisterForward(sessionID, forwardID string) {
	if r := h.room(sessionID); r != nil {
		r.mu.Lock()
		delete(r.forwards, forwardID)
		r.mu.Unlock()
	}
}

func (h *Hub) RequestForward(ctx context.Context, sessionID string, req types.ViewerForwardRequest) (<-chan types.ServerForwardResponse, error) {
	r := h.roomFor(sessionID)
	if req.ForwardID == "" || req.RequestID == "" {
		return nil, ErrForwardNotFound
	}
	ch := make(chan types.ServerForwardResponse, 1)
	r.mu.Lock()
	r.waiters[req.RequestID] = ch
	r.mu.Unlock()
	h.publish(ctx, sessionID, busFrame{Ctrl: "forward_request", Command: string(marshalForward(req))})
	return ch, nil
}

func (h *Hub) ForwardResponse(ctx context.Context, sessionID string, response types.ServerForwardResponse) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "forward_response", Command: string(marshalForward(response))})
}

func (h *Hub) handleForwardRequest(_ string, r *room, f busFrame) {
	var req types.ViewerForwardRequest
	if json.Unmarshal([]byte(f.Command), &req) != nil {
		return
	}
	r.mu.Lock()
	host := r.host
	forward, ok := r.forwards[req.ForwardID]
	r.mu.Unlock()
	if host == nil || !ok {
		return
	}
	host.Send(marshalForward(types.ServerForwardRequest{
		Type: types.TypeForwardRequest, ForwardID: forward.ID, Port: forward.Port,
		RequestID: req.RequestID, Method: req.Method, Path: req.Path,
		Headers: req.Headers, Body: req.Body,
	}))
}

func (h *Hub) handleForwardResponse(r *room, f busFrame) {
	var response types.ServerForwardResponse
	if json.Unmarshal([]byte(f.Command), &response) != nil {
		return
	}
	r.mu.Lock()
	ch := r.waiters[response.RequestID]
	delete(r.waiters, response.RequestID)
	r.mu.Unlock()
	if ch != nil {
		ch <- response
	}
}

// applyMembership runs on the host's instance: update the authoritative list and
// broadcast a fresh ParticipantsUpdated to the host and the viewers.
func (h *Hub) applyMembership(sessionID string, r *room, f busFrame) {
	r.mu.Lock()
	if r.host == nil || f.Part == nil {
		r.mu.Unlock()
		return
	}
	key := f.Part.ID.String()
	if f.Ctrl == "join" {
		r.people[key] = *f.Part
		r.userBy[key] = f.User
	} else {
		delete(r.people, key)
		delete(r.userBy, key)
	}
	hostUser := r.hostUser
	list := make([]types.Participant, 0, len(r.people))
	for key, p := range r.people {
		// The host viewing their own shared link joins with their own account —
		// don't list them as a separate participant; the synthetic host entry
		// below already represents them (no duplicate avatar in the stack).
		if hostUser != "" && r.userBy[key] == hostUser {
			continue
		}
		list = append(list, p)
	}
	host := r.host
	hostName := r.hostName
	hostAvatar := r.hostAvatar
	r.mu.Unlock()

	// The host renders itself from its own account, so its list is the viewers
	// only. Viewers also get a synthetic host entry so they can see who they are
	// watching ("who is in the session").
	hostFrame, _ := json.Marshal(types.ParticipantsUpdated(list))
	host.Send(hostFrame)

	viewerList := append([]types.Participant{types.HostParticipant(hostName, hostAvatar)}, list...)
	viewerFrame, _ := json.Marshal(types.ParticipantsUpdated(viewerList))
	h.publish(context.Background(), sessionID, busFrame{To: "viewers", Data: viewerFrame})
}

func (h *Hub) publish(ctx context.Context, sessionID string, f busFrame) {
	b, err := json.Marshal(f)
	if err != nil {
		return
	}
	_ = h.bus.Publish(ctx, channelFor(sessionID), b)
}

// ── Registration ─────────────────────────────────────────────────────────────

// RegisterHost attaches the local host connection for a session. hostName and
// hostAvatar are shown to viewers in the synthetic host presence entry.
func (h *Hub) RegisterHost(sessionID, hostUser, hostName, hostAvatar string, sink Sink) {
	r := h.roomFor(sessionID)
	r.mu.Lock()
	r.host = sink
	r.hostUser = hostUser
	r.hostName = hostName
	r.hostAvatar = hostAvatar
	r.refs++
	// Cancels any pending reconnect-grace teardown from a previous drop.
	r.hostEpoch++
	r.mu.Unlock()
}

// RegisterViewer attaches a local viewer and announces it (with its owning
// account) to the host.
func (h *Hub) RegisterViewer(ctx context.Context, sessionID string, p types.Participant, user string, sink Sink) {
	h.registerViewer(ctx, sessionID, p, user, sink, types.ViewerCapabilities{})
}

// RegisterViewerWithCapabilities opts this connection into additive v2 frames.
// Registration and history replay share the legacy path and its ordering lock.
func (h *Hub) RegisterViewerWithCapabilities(ctx context.Context, sessionID string, p types.Participant, user string, sink Sink, capabilities types.ViewerCapabilities) {
	h.registerViewer(ctx, sessionID, p, user, sink, capabilities)
}

func (h *Hub) registerViewer(ctx context.Context, sessionID string, p types.Participant, user string, sink Sink, capabilities types.ViewerCapabilities) {
	r := h.roomFor(sessionID)
	r.mu.Lock()
	history := make([][]byte, len(r.history))
	copy(history, r.history)
	// Register and replay while holding the room lock.  Dispatch also takes this
	// lock, so no live frame can be delivered between the snapshot and its
	// replay.  Without this ordering a refresh could receive a new agent snapshot
	// first and then stale history, causing blocks to jump or disappear.
	r.viewers[p.ID.String()] = sink
	r.refs++
	for _, frame := range history {
		sink.Send(frame)
	}
	if capabilities.ReplayBoundary {
		boundary, _ := json.Marshal(types.ReplayComplete())
		sink.Send(boundary)
	}
	r.mu.Unlock()
	h.publish(ctx, sessionID, busFrame{Ctrl: "join", Part: &p, User: user})
}

// SetEndReason records the reason the host is ending the session (e.g. an
// inactivity auto-end), so UnregisterHost shows it to viewers instead of the
// generic "host left".
func (h *Hub) SetEndReason(sessionID, reason string) {
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	r.endReason = reason
	r.mu.Unlock()
}

// HostReconnectGrace is how long a session survives after the host's WebSocket
// drops, giving the desktop client (which reconnects with backoff) time to come
// back. Without it ANY transient drop — a network blip, or an oversized frame
// tripping the read limit — permanently told every viewer "the host ended the
// session" while the host was still running.
const HostReconnectGrace = 45 * time.Second

// UnregisterHost detaches the host. An EXPLICIT end (a reason was recorded) tears
// the session down immediately; an unexpected drop starts a reconnect grace
// period and only ends the session if the host doesn't return.
func (h *Hub) UnregisterHost(ctx context.Context, sessionID string) {
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	r.host = nil
	r.refs--
	reason := r.endReason
	r.hostEpoch++
	epoch := r.hostEpoch
	r.mu.Unlock()

	if reason != "" {
		h.endForViewers(ctx, sessionID, reason)
		return
	}

	// Unexpected disconnect — wait for a reconnect before declaring the end.
	time.AfterFunc(HostReconnectGrace, func() {
		r2 := h.room(sessionID)
		if r2 == nil {
			return
		}
		r2.mu.Lock()
		returned := r2.host != nil || r2.hostEpoch != epoch
		r2.mu.Unlock()
		if returned {
			slog.Info("realtime: host reconnected within grace — session continues", "session", sessionID)
			return
		}
		slog.Info("realtime: host did not reconnect — ending session", "session", sessionID)
		h.endForViewers(context.Background(), sessionID, "host left")
	})
}

// endForViewers broadcasts the end frame and closes the room if nothing is left.
func (h *Hub) endForViewers(ctx context.Context, sessionID, reason string) {
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	last := r.refs <= 0
	r.mu.Unlock()

	frame, _ := json.Marshal(types.Ended(reason))
	h.publish(ctx, sessionID, busFrame{To: "viewers", Data: frame})
	if last {
		h.closeRoom(sessionID)
	}
}

// UnregisterViewer detaches a viewer and announces its departure.
func (h *Hub) UnregisterViewer(ctx context.Context, sessionID string, p types.Participant) {
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	delete(r.viewers, p.ID.String())
	r.refs--
	last := r.refs <= 0
	r.mu.Unlock()

	h.publish(ctx, sessionID, busFrame{Ctrl: "leave", Part: &p})
	if last {
		h.closeRoom(sessionID)
	}
}

func (h *Hub) room(sessionID string) *room {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.rooms[sessionID]
}

func (h *Hub) closeRoom(sessionID string) {
	h.mu.Lock()
	r, ok := h.rooms[sessionID]
	if ok {
		delete(h.rooms, sessionID)
	}
	h.mu.Unlock()
	if ok && r.sub != nil {
		r.sub.Close()
	}
}

// ── Message routing (called by the handler on inbound frames) ────────────────

// BroadcastDelta forwards a host's raw `delta` frame to all viewers. The host's
// `{"type":"delta",...}` and the viewer's `{"type":"delta",...}` are identical
// on the wire, so the frame is relayed verbatim.
func (h *Hub) BroadcastDelta(ctx context.Context, sessionID string, deltaFrame []byte) {
	h.publish(ctx, sessionID, busFrame{To: "viewers", Data: append([]byte(nil), deltaFrame...)})
}

// RequestCommand is called on the viewer's instance: it publishes an exec
// request onto the bus. Enforcement (role + rate limit) happens on the HOST's
// instance in handleExec, where the authoritative participant roles live — so a
// viewer cannot bypass it by connecting to a different gateway instance.
func (h *Hub) RequestCommand(ctx context.Context, sessionID, from, command string) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "exec", From: from, Command: command})
}

const maxRemoteAgentQueryBytes = 64 * 1024

// RequestAgentQuery relays a controller's prompt to the host instance. The
// host remains the only process that can contact providers or execute tools.
func (h *Hub) RequestAgentQuery(ctx context.Context, sessionID, from, requestID, query string) {
	h.publish(ctx, sessionID, busFrame{
		Ctrl: "agent_query", From: from, RequestID: requestID, Query: query,
	})
}

func (h *Hub) RequestAgentStop(ctx context.Context, sessionID, from string) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "agent_stop", From: from})
}

func (h *Hub) handleAgentQuery(sessionID string, r *room, from, requestID, query string) {
	r.mu.Lock()
	host := r.host
	p, known := r.people[from]
	r.mu.Unlock()
	if host == nil || !known || p.Role != types.RoleController {
		slog.Info("realtime: agent query denied — participant lacks the edit role", "session", sessionID, "participant", from)
		return
	}
	query = strings.TrimSpace(query)
	if query == "" || len(query) > maxRemoteAgentQueryBytes {
		slog.Info("realtime: agent query denied — invalid size", "session", sessionID, "participant", from, "bytes", len(query))
		return
	}
	id, err := uuid.Parse(requestID)
	if err != nil || id == uuid.Nil {
		return
	}
	allowed, err := h.limiter.Allow(context.Background(), rateKey(sessionID, from)+":agent")
	if err != nil || !allowed {
		slog.Info("realtime: agent query rate-limited", "session", sessionID, "participant", from)
		return
	}

	// Keep a bounded idempotency window. Duplicate frames can occur after a
	// reconnect; accepting them twice would create two separately billed tasks.
	now := time.Now()
	r.mu.Lock()
	for key, seenAt := range r.agentSeen {
		if now.Sub(seenAt) > time.Hour {
			delete(r.agentSeen, key)
		}
	}
	dedupKey := from + ":" + requestID
	if _, duplicate := r.agentSeen[dedupKey]; duplicate {
		r.mu.Unlock()
		return
	}
	r.agentSeen[dedupKey] = now
	r.mu.Unlock()

	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	frame, _ := json.Marshal(types.AgentQueryRequested(id, fromID, query))
	host.Send(frame)
}

func (h *Hub) handleAgentStop(sessionID string, r *room, from string) {
	r.mu.Lock()
	host := r.host
	p, known := r.people[from]
	r.mu.Unlock()
	if host == nil || !known || p.Role != types.RoleController {
		slog.Info("realtime: agent stop denied — participant lacks the edit role", "session", sessionID, "participant", from)
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	frame, _ := json.Marshal(types.AgentStopRequested(fromID))
	host.Send(frame)
}

// handleExec runs on the host's instance. A participant's ROLE is its permission:
// an Edit participant (RoleController, granted by the link/invite they joined
// with) may run commands, which execute immediately — no per-command approval —
// but are still rate-limited and audited. A View participant is denied.
func (h *Hub) handleExec(sessionID string, r *room, from, command string) {
	r.mu.Lock()
	host := r.host
	if host == nil { // not the host's instance — ignore
		r.mu.Unlock()
		return
	}
	p, known := r.people[from]
	hostUser := r.hostUser
	r.mu.Unlock()

	if !known || p.Role != types.RoleController {
		slog.Info("realtime: exec denied — participant lacks the edit role", "session", sessionID, "participant", from)
		return
	}
	allowed, err := h.limiter.Allow(context.Background(), rateKey(sessionID, from))
	if err != nil || !allowed {
		slog.Info("realtime: exec rate-limited", "session", sessionID, "participant", from)
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	reqID := uuid.New()

	// The edit role IS the approval — run it now, but record it in the audit trail.
	_ = h.auditor.RecordCommand(context.Background(), AuditEvent{
		SessionID: sessionID, ParticipantID: from, HostUser: hostUser,
		Command: command, Approved: true, At: time.Now(),
	})
	frame, _ := json.Marshal(types.CommandRequested(reqID, fromID, command, true))
	host.Send(frame)
}

// ForwardInput is called on the viewer's instance: it publishes a raw-input
// frame onto the bus. Enforcement (role) happens on the HOST's instance in
// handleInput, so a viewer cannot bypass it via a different gateway instance.
func (h *Hub) ForwardInput(ctx context.Context, sessionID, from, dataBase64 string) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "input", From: from, Input: dataBase64})
}

// handleInput runs on the host's instance. Like handleExec, the participant's
// ROLE is its permission: only an Edit participant (RoleController) may drive the
// host's PTY. Input is per-keystroke, so it is NOT rate-limited or audited
// line-by-line — the initiating join/link already established the edit grant.
func (h *Hub) handleInput(sessionID string, r *room, from, dataBase64 string) {
	r.mu.Lock()
	host := r.host
	if host == nil { // not the host's instance — ignore
		r.mu.Unlock()
		return
	}
	p, known := r.people[from]
	r.mu.Unlock()

	if !known || p.Role != types.RoleController {
		slog.Debug("realtime: input denied — participant lacks the edit role", "session", sessionID, "participant", from)
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	frame, _ := json.Marshal(types.InputReceived(fromID, dataBase64))
	host.Send(frame)
}

// ForwardResize is called on the viewer's instance: it publishes a resize
// request onto the bus. Enforcement (role) happens on the HOST's instance in
// handleResize.
func (h *Hub) ForwardResize(ctx context.Context, sessionID, from string, rows, cols uint16) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "resize", From: from, Rows: rows, Cols: cols})
}

// handleResize runs on the host's instance. Only an Edit participant may resize
// the host's PTY; the grant is the edit role, same as input.
func (h *Hub) handleResize(sessionID string, r *room, from string, rows, cols uint16) {
	r.mu.Lock()
	host := r.host
	if host == nil { // not the host's instance — ignore
		r.mu.Unlock()
		return
	}
	p, known := r.people[from]
	hostUser := r.hostUser
	fromUser := r.userBy[from]
	r.mu.Unlock()

	if !known || p.Role != types.RoleController {
		slog.Debug("realtime: resize denied — participant lacks the edit role", "session", sessionID, "participant", from)
		return
	}
	// Only the host's OWN account (e.g. a second device) may drive the host's
	// terminal size. A teammate (different account) never resizes the host — they
	// letterbox the host's grid on their side instead — so the host stays
	// authoritative and its desktop layout isn't yanked around by a remote window.
	if hostUser == "" || fromUser != hostUser {
		slog.Debug("realtime: resize ignored — not the host's own account", "session", sessionID, "participant", from)
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	frame, _ := json.Marshal(types.ResizeRequested(fromID, rows, cols))
	host.Send(frame)
}

// ForwardControl is called on the viewer's instance: it publishes a controller's
// input/scroll surface onto the bus. Enforcement (role) happens on the HOST's
// instance in handleControl.
func (h *Hub) ForwardControl(ctx context.Context, sessionID, from string, surface json.RawMessage) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "control", From: from, Surface: surface})
}

// handleControl runs on the host's instance. Only an Edit participant
// (RoleController) may drive the host's editors/scroll; the grant is the edit
// role, same as input/resize. The surface is opaque here — forwarded verbatim.
func (h *Hub) handleControl(sessionID string, r *room, from string, surface json.RawMessage) {
	r.mu.Lock()
	host := r.host
	if host == nil { // not the host's instance — ignore
		r.mu.Unlock()
		return
	}
	p, known := r.people[from]
	r.mu.Unlock()

	if !known || p.Role != types.RoleController {
		slog.Debug("realtime: control denied — participant lacks the edit role", "session", sessionID, "participant", from)
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	frame, _ := json.Marshal(types.ControlReceived(fromID, surface))
	host.Send(frame)
}

// ForwardPointer publishes ephemeral participant presence. It is a separate
// channel from ForwardControl so visibility never grants editing capability.
func (h *Hub) ForwardPointer(ctx context.Context, sessionID, from string, position types.PointerPosition) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "pointer", From: from, Pointer: &position})
}

func (h *Hub) handlePointer(sessionID string, r *room, from string, position *types.PointerPosition) {
	if position == nil {
		return
	}
	x, y := float64(position.X), float64(position.Y)
	if math.IsNaN(x) || math.IsInf(x, 0) || math.IsNaN(y) || math.IsInf(y, 0) ||
		x < 0 || x > 1 || y < 0 || y > 1 {
		return
	}

	r.mu.Lock()
	host := r.host
	_, known := r.people[from]
	r.mu.Unlock()
	if host == nil || !known {
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	frame, _ := json.Marshal(types.PointerMoved(fromID, *position))
	host.Send(frame)
}

// RequestAccess is called on the viewer's instance when a view-only participant
// asks for Edit. It publishes an access request onto the bus; the host instance
// resolves it in handleAccess (where the authoritative participant list lives).
func (h *Hub) RequestAccess(ctx context.Context, sessionID, from string) {
	h.publish(ctx, sessionID, busFrame{Ctrl: "access", From: from})
}

// handleAccess runs on the host's instance: it forwards an Edit request to the
// host so the user can approve (grant control) or decline. The host approves via
// SetParticipantRole; there is no auto-grant.
func (h *Hub) handleAccess(sessionID string, r *room, from string) {
	r.mu.Lock()
	host := r.host
	if host == nil { // not the host's instance — ignore
		r.mu.Unlock()
		return
	}
	p, known := r.people[from]
	r.mu.Unlock()
	if !known {
		return
	}
	fromID, err := uuid.Parse(from)
	if err != nil {
		return
	}
	name := p.DisplayName
	if name == "" {
		name = "A viewer"
	}
	frame, _ := json.Marshal(types.AccessRequested(fromID, name))
	host.Send(frame)
}

// ResolveCommand runs on the host's instance when the host approves or denies a
// pending command: it records the decision in the audit trail. On approval the
// host runs the command locally and its output streams back as deltas.
func (h *Hub) ResolveCommand(ctx context.Context, sessionID, requestID string, approved bool) {
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	pc, ok := r.pending[requestID]
	delete(r.pending, requestID)
	hostUser := r.hostUser
	r.mu.Unlock()
	if !ok {
		return
	}
	if err := h.auditor.RecordCommand(ctx, AuditEvent{
		SessionID:     sessionID,
		ParticipantID: pc.from,
		HostUser:      hostUser,
		Command:       pc.command,
		Approved:      approved,
		At:            time.Now(),
	}); err != nil {
		slog.Warn("realtime: audit write failed", "session", sessionID, "error", err)
	}
}

// SendToViewer delivers a frame to one specific viewer (e.g. RoleChanged).
func (h *Hub) SendToViewer(ctx context.Context, sessionID, participantID string, frame []byte) {
	h.publish(ctx, sessionID, busFrame{To: "viewer", Target: participantID, Data: frame})
}

// SetParticipantAccess updates a viewer's role (control vs view-only), tells that
// viewer via RoleChanged, and refreshes everyone's participant list. Runs on the
// host's instance (that's where the authoritative list lives).
func (h *Hub) SetParticipantAccess(ctx context.Context, sessionID, participantID, access string) {
	role := types.RoleViewer
	if access == types.AccessControl {
		role = types.RoleController
	}
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	if p, ok := r.people[participantID]; ok {
		p.Role = role
		r.people[participantID] = p
	}
	list := make([]types.Participant, 0, len(r.people))
	for _, p := range r.people {
		list = append(list, p)
	}
	host := r.host
	r.mu.Unlock()

	roleChanged, _ := json.Marshal(types.RoleChanged(role))
	h.publish(ctx, sessionID, busFrame{To: "viewer", Target: participantID, Data: roleChanged})

	people, _ := json.Marshal(types.ParticipantsUpdated(list))
	if host != nil {
		host.Send(people)
	}
	h.publish(ctx, sessionID, busFrame{To: "viewers", Data: people})
}

// RemoveParticipant kicks a participant from the session (host revoked access):
// it drops them from the authoritative list, tells them via Removed (their client
// disconnects), and refreshes everyone's participant list. Runs on the host's
// instance.
func (h *Hub) RemoveParticipant(ctx context.Context, sessionID, participantID string) {
	r := h.room(sessionID)
	if r == nil {
		return
	}
	r.mu.Lock()
	delete(r.people, participantID)
	delete(r.userBy, participantID)
	list := make([]types.Participant, 0, len(r.people))
	for _, p := range r.people {
		list = append(list, p)
	}
	host := r.host
	r.mu.Unlock()

	removed, _ := json.Marshal(types.Removed("removed by host"))
	h.publish(ctx, sessionID, busFrame{To: "viewer", Target: participantID, Data: removed})

	people, _ := json.Marshal(types.ParticipantsUpdated(list))
	if host != nil {
		host.Send(people)
	}
	h.publish(ctx, sessionID, busFrame{To: "viewers", Data: people})
}
