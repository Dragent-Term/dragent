// runtime.events after the E2EE cut-over (§8, F8): the attention mirror is
// removed. Attention travels inside the KK channel; the server keeps no
// titles, bodies, kinds, or workspace ids. EventBus stays as a neutered
// type so the socket streaming path compiles: Record drops everything and
// Subscribe returns an empty snapshot with no live deliveries. The HTTP
// subscribe/unsubscribe transports are 410 (see rpc.go HandleFrame).
// MirrorSender is deleted: push content never touches the relay.
package relay

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"
)

// Stream names accepted for the attention mirror. "runtime.events" is
// canonical; the method-table spelling is aliased.
const (
	eventStreamName      = "runtime.events"
	eventSubscribeMethod = "runtime.events.subscribe"
)

const (
	// maxEventLog caps stored events per account (read-only mirror, not storage).
	maxEventLog = 100
	// eventSnapshotLimit caps one snapshot's events.
	eventSnapshotLimit = 50
	// eventChannelSize buffers live delivery per subscription. On overflow
	// the event is dropped but the subscription seq still advances, so the
	// client observes a seq gap and re-subscribes for a fresh snapshot
	// (the spec's gap rule).
	eventChannelSize = 64
)

// AttentionEvent is one mirrored attention decision: what the push
// dispatcher sent (never tokens, never secrets).
type AttentionEvent struct {
	Kind       string            `json:"kind"`
	SessionID  string            `json:"session_id,omitempty"`
	Title      string            `json:"title"`
	Body       string            `json:"body"`
	Data       map[string]string `json:"data,omitempty"`
	OccurredAt time.Time         `json:"occurred_at"`
	Delivered  int               `json:"delivered"`
}

// streamPayload is one queued live delivery with its per-stream seq.
type streamPayload struct {
	seq   uint64
	event AttentionEvent
}

type subscription struct {
	handle string
	userID string
	ch     chan streamPayload
	done   chan struct{}
	seq    uint64 // last assigned seq; guarded by EventBus.mu
}

// EventBus is the per-account attention mirror: a bounded log plus live
// fan-out to that account's subscriptions.
type EventBus struct {
	mu   sync.Mutex
	log  map[string][]AttentionEvent
	subs map[string]*subscription
}

// NewEventBus returns an empty EventBus.
func NewEventBus() *EventBus {
	return &EventBus{log: make(map[string][]AttentionEvent), subs: make(map[string]*subscription)}
}

// Record drops every attention decision. The content mirror is removed
// (§8, F8): titles, bodies, kinds and workspace ids never touch the relay.
// It never blocks.
func (b *EventBus) Record(_ string, _ AttentionEvent) {
}

// Subscribe registers a neutered subscription: the snapshot is always empty
// and no live event is ever delivered (Record drops everything). Kept so
// the socket streaming path compiles and idles without content.
func (b *EventBus) Subscribe(userID string) (handle string, snapshot []AttentionEvent, ch <-chan streamPayload, cancel func()) {
	b.mu.Lock()
	defer b.mu.Unlock()
	handle = newStreamHandle()
	sub := &subscription{handle: handle, userID: userID, ch: make(chan streamPayload, eventChannelSize), done: make(chan struct{}), seq: 1}
	b.subs[handle] = sub
	return handle, []AttentionEvent{}, sub.ch, func() { b.unsubscribe(handle, userID) }
}

func (b *EventBus) unsubscribe(handle, userID string) bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	sub, ok := b.subs[handle]
	if !ok || sub.userID != userID {
		return false
	}
	delete(b.subs, handle)
	close(sub.done)
	return true
}

// doneFor returns the subscription's done channel (nil when gone).
func (b *EventBus) doneFor(handle, userID string) chan struct{} {
	b.mu.Lock()
	defer b.mu.Unlock()
	sub, ok := b.subs[handle]
	if !ok || sub.userID != userID {
		return nil
	}
	return sub.done
}

func newStreamHandle() string {
	var bytes [16]byte
	if _, err := rand.Read(bytes[:]); err != nil {
		return fmt.Sprintf("stream-%d", time.Now().UnixNano())
	}
	return "stream-" + hex.EncodeToString(bytes[:])
}

// writeStreamFrame emits one SSE data event carrying an rpc-stream frame.
func writeStreamFrame(w http.ResponseWriter, handle string, seq uint64, payload any) error {
	frame := map[string]any{"type": rpcTypeStream, "v": rpcVersion, "stream": handle, "seq": seq, "payload": payload}
	raw, err := json.Marshal(frame)
	if err != nil {
		return err
	}
	if _, err := fmt.Fprintf(w, "data: %s\n\n", raw); err != nil {
		return err
	}
	return http.NewResponseController(w).Flush()
}

// handleSubscribe answers the removed SSE subscribe transport: 410.
// Attention now travels inside the KK channel.
func (h *RPCHandler) handleSubscribe(w http.ResponseWriter, _ *http.Request, _ rpcFrame) {
	transportErr(w, http.StatusGone, "relay rpc is end-to-end encrypted: update the app", false)
}

// handleUnsubscribe serves rpc-unsubscribe: it closes the named stream and
// confirms with an rpc-reply. Unknown handles (including
// unsubscribe-before-first-snapshot) confirm as unsubscribed — idempotent,
// never an oracle.
func (h *RPCHandler) handleUnsubscribe(w http.ResponseWriter, r *http.Request, frame rpcFrame) {
	userID, authed := callerUserID(r)
	if !authed {
		replyErr(w, http.StatusUnauthorized, frame.ID, "login required for relay", true)
		return
	}
	handle := strings.TrimSpace(frame.Stream)
	if handle != "" {
		h.events.unsubscribe(handle, userID)
	}
	replyOK(w, frame.ID, map[string]any{"unsubscribed": true})
}
