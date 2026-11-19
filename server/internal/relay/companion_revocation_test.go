package relay

import (
	"context"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	"server/internal/companionsession"
)

// fakeRevocations stands in for the Redis subscription: tests deliver
// events and reconnects by hand.
type fakeRevocations struct {
	mu            sync.Mutex
	onEvent       func(companionsession.Revocation)
	onResubscribe func()
	subscribes    int
	stopped       int
}

func (f *fakeRevocations) SubscribeRevocations(onEvent func(companionsession.Revocation), onResubscribe func()) func() {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.onEvent, f.onResubscribe = onEvent, onResubscribe
	f.subscribes++
	return func() { f.mu.Lock(); f.stopped++; f.mu.Unlock() }
}

func (f *fakeRevocations) publish(event companionsession.Revocation) {
	f.mu.Lock()
	fn := f.onEvent
	f.mu.Unlock()
	fn(event)
}

func (f *fakeRevocations) reconnect() {
	f.mu.Lock()
	fn := f.onResubscribe
	f.mu.Unlock()
	fn()
}

// A session store that is also a revocation source, like the Redis store.
type sourcedSessions struct {
	*companionsession.MemoryStore
	*fakeRevocations
}

// A device revocation served on ANOTHER instance reaches this one through
// the published event: only that phone's socket ends, at once, with no poll.
func TestPublishedDeviceRevocationEndsOnlyThatSocket(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	var checks atomic.Int32
	h.SetDeviceAuthorizer(func(context.Context, string, string) (bool, error) { checks.Add(1); return true, nil })
	src := &fakeRevocations{}
	h.SetRevocationSource(src)

	c1, _ := openSocket(t, srv)
	admitPhone(t, c1, "test-account-token")
	c2, _ := openSocket(t, srv)
	admitPhone(t, c2, "test-account-token-b")

	src.publish(companionsession.Revocation{Kind: companionsession.RevokedDevice, UserID: "account-a", DeviceID: "phone-1"})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if code := readCloseCode(t, ctx, c1); code != companionSessionEndedCode {
		t.Fatalf("revoked phone close code %d, want %d", code, companionSessionEndedCode)
	}
	socketWrite(t, ctx, c2, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, ctx, c2); got["type"] != "relay-pong" {
		t.Fatalf("other phone lost: %v", got)
	}
	if n := checks.Load(); n != 0 {
		t.Fatalf("revocation needed %d device reads; want event-only", n)
	}
}

// A lease revoked elsewhere ends every socket on that lease.
func TestPublishedLeaseRevocationEndsSocket(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, fixedAuth("account-a", now.Add(time.Hour)))
	src := &fakeRevocations{}
	h.SetRevocationSource(src)
	c, _ := openSocket(t, srv)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token", "device_id": "desktop"})
	ready := socketRead(t, ctx, c)
	leaseID, _ := ready["session_id"].(string)
	if leaseID == "" {
		t.Fatalf("admission: %v", ready)
	}
	// Another lease's revocation leaves this socket alone.
	src.publish(companionsession.Revocation{Kind: companionsession.RevokedLease, UserID: "account-a", LeaseID: "00000000-0000-4000-8000-000000000001"})
	socketWrite(t, ctx, c, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, ctx, c); got["type"] != "relay-pong" {
		t.Fatalf("unrelated lease revocation closed socket: %v", got)
	}
	src.publish(companionsession.Revocation{Kind: companionsession.RevokedLease, UserID: "account-a", DeviceID: "desktop", LeaseID: leaseID})
	if code := readCloseCode(t, ctx, c); code != companionSessionEndedCode {
		t.Fatalf("lease revocation close code %d, want %d", code, companionSessionEndedCode)
	}
}

// A revocation that lands while a socket is mid-admission (after the device
// check, before registration) still ends it as soon as it registers.
func TestRevocationJustBeforeRegistrationEndsSocket(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	h.CloseDeviceSockets("account-a", "phone-1")

	c, ctx := openSocket(t, srv)
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": "test-account-token"})
	waitCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if code := readCloseCode(t, waitCtx, c); code != companionSessionEndedCode {
		t.Fatalf("tombstoned phone close code %d, want %d", code, companionSessionEndedCode)
	}
}

func TestRevocationTombstoneExpires(t *testing.T) {
	r := newDeviceConnRegistry()
	clock := time.Now()
	r.now = func() time.Time { return clock }
	r.closeScope("s", companionSessionEndedCode, "x")
	clock = clock.Add(revocationTombstoneTTL)
	ended := make(chan struct{}, 1)
	untrack := r.track("s", newLiveSocket(func(websocket.StatusCode, string) { ended <- struct{}{} }))
	defer untrack()
	select {
	case <-ended:
		t.Fatal("expired tombstone ended a new socket")
	case <-time.After(50 * time.Millisecond):
	}
	// A later revocation prunes the stale tombstone.
	r.closeScope("other", companionSessionEndedCode, "x")
	r.mu.Lock()
	_, kept := r.tombstones["s"]
	r.mu.Unlock()
	if kept {
		t.Fatal("expired tombstone not pruned")
	}
}

// After the subscription reconnects (events may have been lost) each socket
// re-checks soon instead of waiting for the slow safety poll.
func TestResubscribeRechecksSockets(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	var revoked atomic.Bool
	h.SetDeviceAuthorizer(func(_ context.Context, _, deviceID string) (bool, error) {
		return !(revoked.Load() && deviceID == "phone-1"), nil
	})
	h.sessionPollInterval = time.Hour // the safety poll won't fire in this test
	h.recheckSpread = 50 * time.Millisecond
	src := &fakeRevocations{}
	h.SetRevocationSource(src)

	c, _ := openSocket(t, srv)
	admitPhone(t, c, "test-account-token")
	revoked.Store(true) // the event for this was lost
	src.reconnect()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if code := readCloseCode(t, ctx, c); code != companionSessionEndedCode {
		t.Fatalf("recheck close code %d, want %d", code, companionSessionEndedCode)
	}
}

// Idle sockets no longer read Redis every 10 s: at the default cadence a
// freshly admitted socket makes no lease or device reads for minutes.
func TestDefaultSafetyPollIsSlow(t *testing.T) {
	if companionSafetyPollInterval < 5*time.Minute {
		t.Fatalf("safety poll %v: per-socket Redis reads are back on a hot cadence", companionSafetyPollInterval)
	}
	if h := NewRPCHandler(nil, nil); h.sessionPollInterval != companionSafetyPollInterval {
		t.Fatalf("default poll interval %v", h.sessionPollInterval)
	}
	for i := 0; i < 1000; i++ {
		d := jittered(companionSafetyPollInterval)
		if d < companionSafetyPollInterval*4/5 || d > companionSafetyPollInterval*6/5 {
			t.Fatalf("jitter out of range: %v", d)
		}
	}
	now := time.Now()
	srv, h, sessions := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	var reads atomic.Int32
	h.SetDeviceAuthorizer(func(context.Context, string, string) (bool, error) { reads.Add(1); return true, nil })
	h.sessionPollInterval = 0 // the production default applies
	counting := &countingSessions{MemoryStore: sessions}
	h.SetCompanionSessions(counting)
	c, _ := openSocket(t, srv)
	admitPhone(t, c, "test-account-token")
	time.Sleep(300 * time.Millisecond)
	if n, m := reads.Load(), counting.resumes.Load(); n != 0 || m != 0 {
		t.Fatalf("idle socket made %d device and %d lease reads", n, m)
	}
}

type countingSessions struct {
	*companionsession.MemoryStore
	resumes atomic.Int32
}

func (c *countingSessions) Resume(ctx context.Context, id, userID, deviceID string) (companionsession.Lease, error) {
	c.resumes.Add(1)
	return c.MemoryStore.Resume(ctx, id, userID, deviceID)
}

// A session store that also delivers revocations (the Redis store) is
// subscribed once by SetCompanionSessions; replacing it unsubscribes.
func TestSetCompanionSessionsSubscribesSourceOnce(t *testing.T) {
	h := NewRPCHandler(nil, nil)
	src := &fakeRevocations{}
	h.SetCompanionSessions(sourcedSessions{MemoryStore: companionsession.NewMemoryStore(), fakeRevocations: src})
	if src.subscribes != 1 {
		t.Fatalf("subscribes = %d", src.subscribes)
	}
	h.SetCompanionSessions(companionsession.NewMemoryStore())
	if src.stopped != 0 {
		t.Fatal("plain store replaced the subscription")
	}
	h.SetRevocationSource(nil)
	if src.stopped != 1 {
		t.Fatalf("stopped = %d", src.stopped)
	}
}
