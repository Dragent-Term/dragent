// E2EE channel routing (§4.1, §4.2): the relay is a blind router.
//
// At HS1 the relay records chan → (initiator, responder), validates the HS1
// target (the lease's desktop, or a same-account desktop for a discovery
// lease), enforces chan uniqueness and the channel caps (limits.go), rewrites
// the peer field for the responder side, and forwards later frames in order
// and unmodified. It never reorders, merges, or edits frames. Under
// backpressure it closes the channel with a relay-chan-closed {chan, reason}
// text frame instead of dropping. CLOSE reasons are unauthenticated hints.
package relay

import (
	"context"
	"encoding/hex"
	"errors"
	"sync"
	"sync/atomic"
	"time"

	"github.com/google/uuid"
)

// Roles on one E2EE channel. The phone typically initiates; the desktop host
// responds. Pairing uses the same roles.
type e2eeRole int

const (
	// E2EERolePhone is the companion socket (/v1/relay/socket) side.
	E2EERolePhone e2eeRole = iota
	// E2EERoleHost is the desktop socket (/v1/relay/host) side.
	E2EERoleHost
)

// Router errors. Messages are routing-only: no handles, methods, or content.
var (
	ErrE2EEUnknownChannel = errors.New("unknown channel")
	ErrE2EEChannelInUse   = errors.New("channel id already in use")
	ErrE2EEChannelCap     = errors.New("too many channels")
	ErrE2EEWrongLease     = errors.New("channel belongs to another session")
	ErrE2EEWrongPeer      = errors.New("sender is not a channel participant")
	ErrE2EEBadTarget      = errors.New("handshake target is not this session's desktop")
)

// E2EEBinding records one channel's participants. InitiatorDevice and
// TargetDevice are lowercase canonical UUID strings (§5).
type E2EEBinding struct {
	Chan            [16]byte
	LeaseID         string
	AccountID       string
	InitiatorDevice string
	TargetDevice    string
	Initiator       e2eeRole
	// initiatorEP is the exact socket that sent HS1. Replies go back to it,
	// not to whichever socket of that device registered last: one phone can
	// hold several sockets (discovery, a machine screen, a reconnect still
	// draining), and a reply on the wrong one strands the handshake.
	initiatorEP atomic.Pointer[e2eeEndpoint]
	// targetEP is the socket the HS1 was forwarded to: the channel lives on
	// that pair of sockets and dies with either of them.
	targetEP atomic.Pointer[e2eeEndpoint]
	// createdAt and established bound a handshake that is never answered:
	// the phone gives up after a few seconds without a CLOSE, and an
	// unreleased slot would count against the channel caps forever.
	createdAt   time.Time
	established atomic.Bool
}

// PendingHandshakeTTL is how long an unanswered HS1 holds its channel slot.
const PendingHandshakeTTL = 30 * time.Second

// E2EERouter is the chan → (initiator, responder) table plus caps. All
// methods are safe for concurrent use. Frame bodies are never inspected.
type E2EERouter struct {
	mu       sync.Mutex
	bindings map[[16]byte]*E2EEBinding
	caps     *ChannelCaps
	stats    *RelayStats
	limiter  *ThroughputLimiter
}

// NewE2EERouter returns a router with fresh caps. Stats and limiter may be
// nil (counting and throughput checks are then skipped); tests inject fakes.
func NewE2EERouter(caps *ChannelCaps, stats *RelayStats, limiter *ThroughputLimiter) *E2EERouter {
	if caps == nil {
		caps = NewChannelCaps()
	}
	return &E2EERouter{bindings: make(map[[16]byte]*E2EEBinding), caps: caps, stats: stats, limiter: limiter}
}

// pairKey joins phone+desktop for the per-pair cap. Order-independent: the
// pair is the same whichever side initiated.
func e2eePairKey(a, b string) string {
	if a < b {
		return a + "\x00" + b
	}
	return b + "\x00" + a
}

// DeviceIDToRaw parses a lowercase canonical UUID device id to its 16 bytes.
func DeviceIDToRaw(id string) ([16]byte, bool) {
	parsed, err := uuid.Parse(id)
	if err != nil || parsed.String() != id {
		return [16]byte{}, false
	}
	return parsed, true
}

// ChanHex renders a channel id as 32 lowercase hex chars (the chan-closed
// text form carries no dashes).
func ChanHex(chanID [16]byte) string { return hex.EncodeToString(chanID[:]) }

// RegisterHS1 records a new channel from its HS1. frame must already decode
// (frames.go). leaseID/accountID come from the admission lease; leaseDevice
// is the lease's desktop ("" for a discovery lease); srcDevice is the
// authenticated sender's device id; isSameAccount reports whether target is
// another of the account's desktops (discovery leases only).
//
// It returns the binding and the HS1 bytes to forward to the responder with
// the peer field rewritten to the initiator's device id (relay-written, so
// the responder never trusts the initiator's claim about who is calling).
// The returned bytes are a copy; the input is never mutated.
func (r *E2EERouter) RegisterHS1(frame *E2EEFrame, raw []byte, leaseID, accountID, leaseDevice, srcDevice string, src e2eeRole, isSameAccount func(target string) bool) (*E2EEBinding, []byte, error) {
	if frame == nil || !frame.IsHS() || frame.Step != 1 {
		return nil, nil, ErrFrameBadStep
	}
	if frame.Peer == nil {
		return nil, nil, ErrFrameMissingPeer
	}
	targetUUID, err := uuid.FromBytes(frame.Peer[:])
	if err != nil {
		return nil, nil, ErrE2EEBadTarget
	}
	target := targetUUID.String()
	if leaseDevice != "" {
		if target != leaseDevice {
			r.record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "bad-target")
			return nil, nil, ErrE2EEBadTarget
		}
	} else {
		if isSameAccount == nil || !isSameAccount(target) {
			r.record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "bad-target")
			return nil, nil, ErrE2EEBadTarget
		}
	}
	srcRaw, ok := DeviceIDToRaw(srcDevice)
	if !ok {
		r.record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "bad-device")
		return nil, nil, ErrE2EEBadTarget
	}
	// Pair participants are phone+desktop regardless of who initiated.
	phone, desktop := srcDevice, target
	if src == E2EERoleHost {
		phone, desktop = target, srcDevice
	}
	pair := e2eePairKey(phone, desktop)
	r.mu.Lock()
	if _, dup := r.bindings[frame.Chan]; dup {
		r.mu.Unlock()
		r.record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "dup-chan")
		return nil, nil, ErrE2EEChannelInUse
	}
	r.mu.Unlock()
	r.expirePending(time.Now())
	if r.caps.Known(frame.Chan) {
		r.record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "dup-chan")
		return nil, nil, ErrE2EEChannelInUse
	}
	reserved := r.caps.Reserve(frame.Chan, leaseID, pair)
	// Only a full pair evicts (§4.6); the per-lease cap still refuses.
	if !reserved && r.caps.LeaseCount(leaseID) < MaxE2EEChannelsPerLease &&
		r.caps.PairCount(pair) >= MaxE2EEChannelsPerPair && r.evictOldest(pair) {
		// The pair was full: per the cap's contract, a new HS1 evicts the
		// pair's oldest channel (rekey/reconnect holds old + new).
		reserved = r.caps.Reserve(frame.Chan, leaseID, pair)
	}
	if !reserved {
		r.record("e2ee-hs1", "refused", SizeBucket(len(frame.Body)), "chan-cap")
		return nil, nil, ErrE2EEChannelCap
	}
	binding := &E2EEBinding{
		Chan:            frame.Chan,
		LeaseID:         leaseID,
		AccountID:       accountID,
		InitiatorDevice: srcDevice,
		TargetDevice:    target,
		Initiator:       src,
		createdAt:       time.Now(),
	}
	r.mu.Lock()
	r.bindings[frame.Chan] = binding
	r.mu.Unlock()
	// Rewrite peer → initiator device id for the responder side. Copy so the
	// sender's bytes (and any replay) are never mutated.
	forward := append([]byte(nil), raw...)
	if len(forward) >= E2EEHeaderLen {
		copy(forward[20:36], srcRaw[:])
	}
	r.record("e2ee-hs1", "ok", SizeBucket(len(frame.Body)), "none")
	return binding, forward, nil
}

// Route checks one non-HS1 frame against its binding: the channel must exist,
// the lease must match, and the sender must be a participant. Bodies are
// never inspected; the caller forwards raw unmodified on success. Throughput
// over-cap is refused so the relay never silently drops (the caller closes
// the channel with relay-chan-closed instead).
func (r *E2EERouter) Route(frame *E2EEFrame, leaseID, srcDevice string, src e2eeRole) (*E2EEBinding, error) {
	if frame == nil {
		return nil, ErrE2EEUnknownChannel
	}
	if frame.IsHS() && frame.Step == 1 {
		return nil, ErrFrameBadStep
	}
	r.mu.Lock()
	binding, ok := r.bindings[frame.Chan]
	r.mu.Unlock()
	if !ok {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "unknown-chan")
		return nil, ErrE2EEUnknownChannel
	}
	if binding.LeaseID != leaseID {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "wrong-lease")
		return nil, ErrE2EEWrongLease
	}
	if srcDevice != binding.InitiatorDevice && srcDevice != binding.TargetDevice {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "wrong-peer")
		return nil, ErrE2EEWrongPeer
	}
	_ = src // both participants may send either direction after HS1
	if srcDevice == binding.TargetDevice {
		binding.established.Store(true)
	}
	if r.limiter != nil && !r.limiter.AllowFrame(leaseID, len(frame.Body)) {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "rate-cap")
		return nil, ErrE2EEChannelCap
	}
	r.record("e2ee-route", "ok", SizeBucket(len(frame.Body)), "none")
	return binding, nil
}

// Close releases one channel. Unknown chans are ignored. The caller sends
// RelayChanClosedFrame to both sides; CLOSE reasons are hints only.
func (r *E2EERouter) Close(chanID [16]byte) {
	r.mu.Lock()
	delete(r.bindings, chanID)
	r.mu.Unlock()
	r.caps.Release(chanID)
}

// expirePending releases handshakes nobody answered within
// PendingHandshakeTTL.
func (r *E2EERouter) expirePending(now time.Time) {
	var stale [][16]byte
	r.mu.Lock()
	for id, b := range r.bindings {
		if !b.established.Load() && now.Sub(b.createdAt) > PendingHandshakeTTL {
			stale = append(stale, id)
		}
	}
	r.mu.Unlock()
	for _, id := range stale {
		r.record("e2ee-route", "expired", "none", "pending-handshake")
		r.Close(id)
	}
}

// evictOldest closes the oldest channel of one (phone, desktop) pair. It
// reports whether one was closed.
func (r *E2EERouter) evictOldest(pair string) bool {
	var oldest *E2EEBinding
	r.mu.Lock()
	for _, b := range r.bindings {
		phone, desktop := b.InitiatorDevice, b.TargetDevice
		if b.Initiator == E2EERoleHost {
			phone, desktop = b.TargetDevice, b.InitiatorDevice
		}
		if e2eePairKey(phone, desktop) != pair {
			continue
		}
		if oldest == nil || b.createdAt.Before(oldest.createdAt) {
			oldest = b
		}
	}
	r.mu.Unlock()
	if oldest == nil {
		return false
	}
	r.record("e2ee-hs1", "evicted", "none", "pair-cap")
	r.Close(oldest.Chan)
	return true
}

// CloseEndpoint releases every channel carried by ep (either side). Called
// when a socket ends, so its channels never hold cap slots afterwards.
func (r *E2EERouter) CloseEndpoint(ep *e2eeEndpoint) int {
	if ep == nil {
		return 0
	}
	var ids [][16]byte
	r.mu.Lock()
	for id, b := range r.bindings {
		if b.initiatorEP.Load() == ep || b.targetEP.Load() == ep {
			ids = append(ids, id)
		}
	}
	r.mu.Unlock()
	for _, id := range ids {
		r.Close(id)
	}
	return len(ids)
}

// Lookup returns the binding for chan, if any.
func (r *E2EERouter) Lookup(chanID [16]byte) (*E2EEBinding, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	b, ok := r.bindings[chanID]
	return b, ok
}

// Count returns live channel count (tests).
func (r *E2EERouter) Count() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.bindings)
}

func (r *E2EERouter) record(route, status, bucket, errClass string) {
	if r.stats != nil {
		r.stats.Record(route, status, bucket, errClass)
	}
}

// ── E2EE endpoint mesh ───────────────────────────────────────────────────
// The mesh tracks live binary endpoints so HS1 and later frames forward to
// the other side's socket. RPCHandler's struct lives in rpc.go (owned by
// E2b), so per-handler E2EE state hangs off this sync.Map instead of new
// struct fields. Keys are account+device only — never content.

// E2EDevicePolicy reports whether a paired phone is E2EE-capable. known is
// false for devices this checker knows nothing about (other checks — JWT,
// revocation — still apply); known && !e2e is a pre-cut-over e2e:false
// device, refused with repairRequired + 4403 (§10). A nil policy skips the
// check (tests and single-instance dev); E2b wires it to the pairing store.
type E2EDevicePolicy func(ctx context.Context, userID, deviceID string) (known bool, e2e bool)

// NotifyIntakeFunc receives one validated host-notify blob for push
// dispatch. accountID routes to the phone; phoneID and blob are opaque.
// E9b wires the real dispatcher; nil accepts and counts (drop).
type NotifyIntakeFunc func(ctx context.Context, accountID, phoneID, blob string) error

// e2eeEndpoint is one live binary socket side.
type e2eeEndpoint struct {
	account string
	device  string
	role    e2eeRole
	write   func(context.Context, []byte) error
}

// e2eeDomain is one handler's E2EE runtime: routing, mesh, policy, intake.
type e2eeDomain struct {
	router *E2EERouter
	stats  *RelayStats
	mu     sync.Mutex
	phones map[string]*e2eeEndpoint
	hosts  map[string]*e2eeEndpoint

	policyMu sync.Mutex
	policy   E2EDevicePolicy
	notify   NotifyIntakeFunc
}

var e2eeDomains sync.Map // *RPCHandler -> *e2eeDomain

func e2eeFor(h *RPCHandler) *e2eeDomain {
	if d, ok := e2eeDomains.Load(h); ok {
		return d.(*e2eeDomain)
	}
	stats := NewRelayStats()
	d := &e2eeDomain{
		router: NewE2EERouter(nil, stats, nil),
		stats:  stats,
		phones: make(map[string]*e2eeEndpoint),
		hosts:  make(map[string]*e2eeEndpoint),
	}
	if actual, loaded := e2eeDomains.LoadOrStore(h, d); loaded {
		return actual.(*e2eeDomain)
	}
	startRelayStatsLog()
	return d
}

var relayStatsLogOnce sync.Once

// startRelayStatsLog logs the E2EE routing counters (handshakes forwarded or
// refused, and why) once a minute. They were counted but never written out,
// so a phone that couldn't reach its desktop left no trace.
func startRelayStatsLog() {
	relayStatsLogOnce.Do(func() {
		go func() {
			ticker := time.NewTicker(time.Minute)
			defer ticker.Stop()
			for range ticker.C {
				e2eeDomains.Range(func(_, value any) bool {
					value.(*e2eeDomain).stats.LogAggregates(nil)
					return true
				})
			}
		}()
	})
}

// SetE2EDevicePolicy installs the pre-cut-over device check. Nil disables it.
func (h *RPCHandler) SetE2EDevicePolicy(fn E2EDevicePolicy) {
	d := e2eeFor(h)
	d.policyMu.Lock()
	d.policy = fn
	d.policyMu.Unlock()
}

// SetE2ENotifyIntake installs the push-dispatch hook for host-notify blobs.
func (h *RPCHandler) SetE2ENotifyIntake(fn NotifyIntakeFunc) {
	d := e2eeFor(h)
	d.policyMu.Lock()
	d.notify = fn
	d.policyMu.Unlock()
}

func e2eeKey(account, device string) string { return account + "\x00" + device }

func (d *e2eeDomain) put(role e2eeRole, account, device string, ep *e2eeEndpoint) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if role == E2EERolePhone {
		d.phones[e2eeKey(account, device)] = ep
	} else {
		d.hosts[e2eeKey(account, device)] = ep
	}
}

func (d *e2eeDomain) drop(role e2eeRole, account, device string, ep *e2eeEndpoint) {
	d.mu.Lock()
	defer d.mu.Unlock()
	k := e2eeKey(account, device)
	if role == E2EERolePhone {
		if d.phones[k] == ep {
			delete(d.phones, k)
		}
	} else {
		if d.hosts[k] == ep {
			delete(d.hosts, k)
		}
	}
}

// otherSide returns the live endpoint for the far side of binding, or nil
// when that desktop/phone is offline.
func (d *e2eeDomain) otherSide(binding *E2EEBinding, srcDevice string) *e2eeEndpoint {
	d.mu.Lock()
	defer d.mu.Unlock()
	if srcDevice != binding.InitiatorDevice {
		// Back to the initiator: the socket that opened this channel.
		if ep := binding.initiatorEP.Load(); ep != nil {
			return ep
		}
	} else if ep := binding.targetEP.Load(); ep != nil {
		// On to the socket that received the HS1.
		return ep
	}
	if srcDevice == binding.InitiatorDevice {
		if binding.Initiator == E2EERolePhone {
			return d.hosts[e2eeKey(binding.AccountID, binding.TargetDevice)]
		}
		return d.phones[e2eeKey(binding.AccountID, binding.TargetDevice)]
	}
	if binding.Initiator == E2EERolePhone {
		return d.phones[e2eeKey(binding.AccountID, binding.InitiatorDevice)]
	}
	return d.hosts[e2eeKey(binding.AccountID, binding.InitiatorDevice)]
}

// hostFor returns the live desktop endpoint for one account+desktop.
func (d *e2eeDomain) hostFor(account, desktop string) *e2eeEndpoint {
	d.mu.Lock()
	defer d.mu.Unlock()
	return d.hosts[e2eeKey(account, desktop)]
}

// phoneFor returns the live phone endpoint for one account+phone.
func (d *e2eeDomain) phoneFor(account, phone string) *e2eeEndpoint {
	d.mu.Lock()
	defer d.mu.Unlock()
	return d.phones[e2eeKey(account, phone)]
}

// RouteHost checks one host-side frame: the desktop never initiates (HS1 is
// phone-only per §4.2), so HS step 1 is refused here. The host carries no
// lease; the binding's account must match instead, and throughput draws from
// the binding lease's budget so both directions share one cap.
func (r *E2EERouter) RouteHost(frame *E2EEFrame, accountID, srcDevice string) (*E2EEBinding, error) {
	if frame == nil {
		return nil, ErrE2EEUnknownChannel
	}
	if frame.IsHS() && frame.Step == 1 {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "hs1-from-host")
		return nil, ErrFrameBadStep
	}
	r.mu.Lock()
	binding, ok := r.bindings[frame.Chan]
	r.mu.Unlock()
	if !ok {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "unknown-chan")
		return nil, ErrE2EEUnknownChannel
	}
	if binding.AccountID != accountID {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "wrong-account")
		return nil, ErrE2EEWrongLease
	}
	if srcDevice != binding.InitiatorDevice && srcDevice != binding.TargetDevice {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "wrong-peer")
		return nil, ErrE2EEWrongPeer
	}
	if srcDevice == binding.TargetDevice {
		binding.established.Store(true)
	}
	if r.limiter != nil && !r.limiter.AllowFrame(binding.LeaseID, len(frame.Body)) {
		r.record("e2ee-route", "refused", SizeBucket(len(frame.Body)), "rate-cap")
		return nil, ErrE2EEChannelCap
	}
	r.record("e2ee-route", "ok", SizeBucket(len(frame.Body)), "none")
	return binding, nil
}
