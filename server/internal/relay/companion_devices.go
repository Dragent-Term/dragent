package relay

import (
	"context"
	"math/rand/v2"
	"sync"
	"time"

	"github.com/coder/websocket"
	"server/internal/companionsession"
)

// Phone socket revocation.
//
// Controller sockets admitted with a companion-scoped access token carry the
// paired phone's device id (SocketIdentity.DeviceID) and a companion lease.
// Revoking the phone, or the lease, must end exactly those sockets:
// same-account desktops and other phones stay connected.
//
// Revocation is event-driven, so idle sockets cost no Redis reads:
//
//   - CloseDeviceSockets closes this instance's live sockets immediately
//     (the DELETE device handler calls it).
//   - The revoking store PUBLISHes the revocation; each API instance holds
//     ONE subscription (SetRevocationSource) that closes its matching local
//     sockets, so revocation reaches instances that did not serve it.
//   - A revocation that lands between admission and registration is kept
//     briefly as a tombstone, so a socket registering just after it still
//     ends at once.
//   - A slow, jittered safety poll per socket (companionSafetyPollInterval)
//     re-reads the lease and device, so a lost message still ends the
//     socket. When the subscription reconnects (events may have been
//     missed), every local socket re-checks within a few seconds.
//
// Admission stays fail closed: the socket authenticator refuses a device it
// can't confirm, and the lease must resolve before relay-ready.

// DeviceAuthorizer answers whether the device is still paired, or an error
// when that couldn't be read (storage slow or down): an error is not a
// revocation.
type DeviceAuthorizer func(ctx context.Context, userID, deviceID string) (bool, error)

const (
	// companionSafetyPollInterval is the per-socket backstop cadence. Two
	// GETs per 5 min is ~17K Redis commands per always-connected phone per
	// month (it was two GETs per 10 s, ~518K).
	companionSafetyPollInterval = 5 * time.Minute
	// companionRetryInterval is the re-check cadence while storage isn't
	// answering, until companionStorageGrace runs out.
	companionRetryInterval = 10 * time.Second
	// companionRecheckSpread spreads the burst of re-checks after the
	// revocation subscription reconnects.
	companionRecheckSpread = 10 * time.Second
	// revocationTombstoneTTL keeps a revoked scope long enough to catch a
	// socket that was mid-admission when the event arrived.
	revocationTombstoneTTL = 2 * time.Minute
)

// SetDeviceAuthorizer installs the paired-device validity check consulted by
// the safety poll. Nil (the default) keeps polling only the companion lease.
func (h *RPCHandler) SetDeviceAuthorizer(fn DeviceAuthorizer) {
	h.deviceAuthorizer = fn
}

// SetRevocationSource subscribes this instance to cross-instance
// revocations (one subscription per process, replacing any previous one).
// SetCompanionSessions calls it when the session store is a source, so
// production wiring needs nothing extra. Nil unsubscribes.
func (h *RPCHandler) SetRevocationSource(src companionsession.RevocationSource) {
	h.revocationMu.Lock()
	defer h.revocationMu.Unlock()
	if h.stopRevocations != nil {
		h.stopRevocations()
		h.stopRevocations = nil
	}
	if src == nil {
		return
	}
	h.stopRevocations = src.SubscribeRevocations(h.applyRevocation, h.recheckAllSockets)
}

// applyRevocation closes this instance's sockets matching one event.
func (h *RPCHandler) applyRevocation(event companionsession.Revocation) {
	switch event.Kind {
	case companionsession.RevokedDevice:
		h.CloseDeviceSockets(event.UserID, event.DeviceID)
	case companionsession.RevokedLease:
		if h.pairedDevices != nil {
			h.pairedDevices.closeScope(leaseSocketScope(event.LeaseID), companionSessionEndedCode, "companion session revoked")
		}
	}
}

// recheckAllSockets asks every local socket to re-read its lease and device
// soon; run after the revocation subscription reconnects.
func (h *RPCHandler) recheckAllSockets() {
	if h.pairedDevices != nil {
		h.pairedDevices.recheckAll()
	}
}

// CloseDeviceSockets ends every live controller socket admitted for one
// paired device. Unknown scopes close nothing. Nil-safe.
func (h *RPCHandler) CloseDeviceSockets(userID, deviceID string) {
	if h == nil || h.pairedDevices == nil {
		return
	}
	h.pairedDevices.closeScope(deviceSocketScope(userID, deviceID), companionSessionEndedCode, "companion device was revoked")
}

func deviceSocketScope(userID, deviceID string) string {
	return "device\x00" + userID + "\x00" + deviceID
}

func leaseSocketScope(leaseID string) string {
	return "lease\x00" + leaseID
}

// deviceConnRegistry tracks each admitted phone socket's end-of-access func
// by device and lease scope, so a revocation runs the same shutdown path as
// expiry.
type deviceConnRegistry struct {
	mu         sync.Mutex
	ends       map[string]map[*liveSocket]struct{}
	tombstones map[string]time.Time
	now        func() time.Time
}

type liveSocket struct {
	end     func(websocket.StatusCode, string)
	recheck chan struct{}
}

func newLiveSocket(end func(websocket.StatusCode, string)) *liveSocket {
	return &liveSocket{end: end, recheck: make(chan struct{}, 1)}
}

func newDeviceConnRegistry() *deviceConnRegistry {
	return &deviceConnRegistry{
		ends:       make(map[string]map[*liveSocket]struct{}),
		tombstones: make(map[string]time.Time),
		now:        time.Now,
	}
}

// track registers entry under scope and returns the matching untrack func.
// A scope revoked moments ago ends the socket at once.
func (r *deviceConnRegistry) track(scope string, entry *liveSocket) func() {
	if r == nil || entry == nil || entry.end == nil {
		return func() {}
	}
	r.mu.Lock()
	set := r.ends[scope]
	if set == nil {
		set = make(map[*liveSocket]struct{})
		r.ends[scope] = set
	}
	set[entry] = struct{}{}
	revokedAt, revoked := r.tombstones[scope]
	revoked = revoked && r.now().Sub(revokedAt) < revocationTombstoneTTL
	r.mu.Unlock()
	if revoked {
		go entry.end(companionSessionEndedCode, "companion access was revoked")
	}
	return func() {
		r.mu.Lock()
		defer r.mu.Unlock()
		if set := r.ends[scope]; set != nil {
			delete(set, entry)
			if len(set) == 0 {
				delete(r.ends, scope)
			}
		}
	}
}

func (r *deviceConnRegistry) closeScope(scope string, code websocket.StatusCode, reason string) {
	if r == nil {
		return
	}
	r.mu.Lock()
	now := r.now()
	for key, at := range r.tombstones {
		if now.Sub(at) >= revocationTombstoneTTL {
			delete(r.tombstones, key)
		}
	}
	r.tombstones[scope] = now
	set := r.ends[scope]
	entries := make([]*liveSocket, 0, len(set))
	for e := range set {
		entries = append(entries, e)
	}
	r.mu.Unlock()
	// Closing waits for the peer's close frame; never hold the revoking
	// request on a slow or dead phone.
	for _, e := range entries {
		go e.end(code, reason)
	}
}

func (r *deviceConnRegistry) recheckAll() {
	if r == nil {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, set := range r.ends {
		for e := range set {
			select {
			case e.recheck <- struct{}{}:
			default:
			}
		}
	}
}

// jittered spreads per-socket checks so sockets admitted together don't
// hit storage together: d ± 20%.
func jittered(d time.Duration) time.Duration {
	if d <= 0 {
		return d
	}
	spread := int64(d) * 2 / 5
	if spread <= 0 {
		return d
	}
	return d - time.Duration(spread/2) + time.Duration(rand.Int64N(spread))
}

// watchCompanionAccess registers the socket for event-driven revocation and
// runs its safety poll. The returned func untracks and stops it.
func (h *RPCHandler) watchCompanionAccess(ctx context.Context, lease companionsession.Lease, identity SocketIdentity, end func(websocket.StatusCode, string)) func() {
	entry := newLiveSocket(end)
	untrackDevice := h.pairedDevices.track(deviceSocketScope(identity.UserID, identity.DeviceID), entry)
	untrackLease := h.pairedDevices.track(leaseSocketScope(lease.ID), entry)
	pollCtx, stop := context.WithCancel(ctx)
	done := make(chan struct{})
	interval := h.sessionPollInterval
	if interval <= 0 {
		interval = companionSafetyPollInterval
	}
	retry := min(interval, companionRetryInterval)
	spread := companionRecheckSpread
	if h.recheckSpread > 0 {
		spread = h.recheckSpread
	}
	go func() {
		defer close(done)
		timer := time.NewTimer(jittered(interval))
		defer timer.Stop()
		var failingSince time.Time
		for {
			select {
			case <-pollCtx.Done():
				return
			case <-entry.recheck:
				// Reconnected subscription: re-check soon, spread out.
				timer.Reset(time.Duration(rand.Int64N(int64(spread) + 1)))
				continue
			case <-timer.C:
			}
			code, reason, err := h.checkCompanionAccess(pollCtx, lease, identity)
			if pollCtx.Err() != nil {
				return
			}
			switch {
			case reason != "":
				end(code, reason)
				return
			case err != nil:
				// Storage didn't answer: keep the phone connected, retry
				// sooner, and only give up when it hasn't for a while.
				if failingSince.IsZero() {
					failingSince = time.Now()
				}
				if time.Since(failingSince) > companionStorageGrace {
					end(code, "companion access check unavailable")
					return
				}
				timer.Reset(jittered(retry))
			default:
				failingSince = time.Time{}
				timer.Reset(jittered(interval))
			}
		}
	}()
	return func() {
		stop()
		<-done
		untrackLease()
		untrackDevice()
	}
}

// checkCompanionAccess re-reads the lease and (for paired phones) the
// device. A non-empty reason means access really ended; err alone means
// storage didn't answer, with code the close code to use if that persists.
func (h *RPCHandler) checkCompanionAccess(ctx context.Context, lease companionsession.Lease, identity SocketIdentity) (websocket.StatusCode, string, error) {
	if err := h.checkCompanionSession(ctx, lease); err != nil {
		if companionSessionGone(err) {
			return companionFailureCode(err), "companion session unavailable", err
		}
		return companionFailureCode(err), "", err
	}
	if h.deviceAuthorizer == nil || identity.UserID == "" || identity.DeviceID == "" {
		return 0, "", nil
	}
	checkCtx, cancel := context.WithTimeout(ctx, companionStoreTimeout)
	defer cancel()
	valid, err := h.deviceAuthorizer(checkCtx, identity.UserID, identity.DeviceID)
	switch {
	case err != nil:
		return websocket.StatusInternalError, "", err
	case !valid:
		return companionSessionEndedCode, "companion device was revoked", nil
	}
	return 0, "", nil
}
