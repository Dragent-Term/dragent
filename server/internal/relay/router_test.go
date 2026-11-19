package relay

import (
	"encoding/hex"
	"fmt"
	"testing"
	"time"

	"github.com/google/uuid"
)

func mustUUID(t *testing.T, s string) [16]byte {
	t.Helper()
	id := uuid.MustParse(s)
	return [16]byte(id)
}

func hs1Raw(t *testing.T, chanHex, peerUUID string, body []byte) []byte {
	t.Helper()
	var chanID [16]byte
	raw, err := hex.DecodeString(chanHex)
	if err != nil {
		t.Fatal(err)
	}
	copy(chanID[:], raw)
	peer := uuid.MustParse(peerUUID)
	raw2, err := EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, chanID, (*[16]byte)(&peer), 0, body)
	if err != nil {
		t.Fatal(err)
	}
	return raw2
}

func TestRouterHS1PeerRewrite(t *testing.T) {
	r := NewE2EERouter(nil, NewRelayStats(), nil)
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	raw := hs1Raw(t, "00112233445566778899aabbccddeeff", desktop, []byte("hs1body"))
	frame, err := DecodeE2EEFrame(raw)
	if err != nil {
		t.Fatal(err)
	}
	binding, forward, err := r.RegisterHS1(frame, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil)
	if err != nil {
		t.Fatal(err)
	}
	if binding.InitiatorDevice != phone || binding.TargetDevice != desktop {
		t.Fatalf("binding %+v", binding)
	}
	// Peer rewritten to the initiator (phone) id; everything else unmodified.
	ff, err := DecodeE2EEFrame(forward)
	if err != nil {
		t.Fatal(err)
	}
	if ff.Peer == nil || uuid.UUID(*ff.Peer).String() != phone {
		t.Fatalf("rewritten peer = %v, want %s", ff.Peer, phone)
	}
	if ff.Chan != frame.Chan || string(ff.Body) != "hs1body" || ff.Kind != E2EEKindHS || ff.Step != 1 {
		t.Fatal("forward mutated more than peer")
	}
	// Input bytes untouched (no aliasing of the replayable HS1).
	orig, _ := DecodeE2EEFrame(raw)
	if orig.Peer == nil || uuid.UUID(*orig.Peer).String() != desktop {
		t.Fatal("input HS1 mutated")
	}
}

func TestRouterHS1TargetEnforcement(t *testing.T) {
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	other := "11111111-2222-4333-8444-555555555555"
	r := NewE2EERouter(nil, NewRelayStats(), nil)
	raw := hs1Raw(t, "00112233445566778899aabbccddeeff", other, []byte("x"))
	frame, _ := DecodeE2EEFrame(raw)
	if _, _, err := r.RegisterHS1(frame, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil); err == nil {
		t.Fatal("HS1 to foreign desktop admitted")
	}
	// Discovery lease: same-account desktop ok, foreign refused.
	r2 := NewE2EERouter(nil, NewRelayStats(), nil)
	same := func(target string) bool { return target == desktop }
	raw2 := hs1Raw(t, "00112233445566778899aabbccddeeff", desktop, []byte("x"))
	f2, _ := DecodeE2EEFrame(raw2)
	if _, _, err := r2.RegisterHS1(f2, raw2, "lease-d", "account-a", "", phone, E2EERolePhone, same); err != nil {
		t.Fatalf("discovery HS1 refused: %v", err)
	}
	raw3 := hs1Raw(t, "ffeeddccbbaa99887766554433221100", other, []byte("x"))
	f3, _ := DecodeE2EEFrame(raw3)
	if _, _, err := r2.RegisterHS1(f3, raw3, "lease-d", "account-a", "", phone, E2EERolePhone, same); err == nil {
		t.Fatal("discovery HS1 to foreign desktop admitted")
	}
}

func TestRouterCapsAndOrdering(t *testing.T) {
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	r := NewE2EERouter(nil, NewRelayStats(), nil)
	// Per-pair cap: 2 channels per (phone, desktop).
	var chans [][]byte
	for i := 0; i < MaxE2EEChannelsPerPair; i++ {
		chanHex := uuid.NewString()
		id := uuid.MustParse(chanHex)
		var chanID [16]byte = id
		raw, err := EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, chanID, (*[16]byte)(&id), 0, []byte("b"))
		_ = raw
		_ = err
		// Build with proper peer=desktop instead.
		raw, err = EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, chanID, (*[16]byte)(func() *[16]byte { d := uuid.MustParse(desktop); return (*[16]byte)(&d) }()), 0, []byte("b"))
		if err != nil {
			t.Fatal(err)
		}
		f, _ := DecodeE2EEFrame(raw)
		if _, _, err := r.RegisterHS1(f, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil); err != nil {
			t.Fatalf("chan %d: %v", i, err)
		}
		chans = append(chans, raw)
	}
	// §4.6: a third channel per pair evicts the oldest (never refused).
	extraID := uuid.New()
	extraRaw, _ := EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, [16]byte(extraID), (*[16]byte)(func() *[16]byte { d := uuid.MustParse(desktop); return (*[16]byte)(&d) }()), 0, []byte("b"))
	ef, _ := DecodeE2EEFrame(extraRaw)
	if _, _, err := r.RegisterHS1(ef, extraRaw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil); err != nil {
		t.Fatalf("third channel per pair refused instead of evicting the oldest: %v", err)
	}
	if f0, _ := DecodeE2EEFrame(chans[0]); func() bool { _, ok := r.Lookup(f0.Chan); return ok }() {
		t.Fatal("the oldest channel of the pair was not evicted")
	}
	if got := r.caps.PairCount(e2eePairKey(phone, desktop)); got != MaxE2EEChannelsPerPair {
		t.Fatalf("pair holds %d channels, want %d", got, MaxE2EEChannelsPerPair)
	}
	// The evicted id is free again; keep the duplicate check below meaningful.
	chans[0] = chans[1]
	// Duplicate chan id refused.
	f0, _ := DecodeE2EEFrame(chans[0])
	if _, _, err := r.RegisterHS1(f0, chans[0], "lease-1", "account-a", desktop, phone, E2EERolePhone, nil); err == nil {
		t.Fatal("duplicate chan admitted")
	}
	// Later frames forward unmodified and in order: DATA from either side.
	fst, _ := DecodeE2EEFrame(chans[0])
	binding, ok := r.Lookup(fst.Chan)
	if !ok {
		t.Fatal("missing binding")
	}
	for ctr := uint64(0); ctr < 3; ctr++ {
		body := []byte{byte(ctr), 0xAA, 0xBB}
		raw, err := EncodeE2EEFrame(E2EEKindData, 0, 0, binding.Chan, nil, ctr, body)
		if err != nil {
			t.Fatal(err)
		}
		df, _ := DecodeE2EEFrame(raw)
		got, err := r.Route(df, "lease-1", phone, E2EERolePhone)
		if err != nil || got != binding {
			t.Fatalf("route ctr %d: %v", ctr, err)
		}
		// Unmodified: re-encode equals input.
		re, _ := EncodeE2EEFrame(df.Kind, df.Step, df.Purpose, df.Chan, df.Peer, df.Ctr, df.Body)
		if hex.EncodeToString(re) != hex.EncodeToString(raw) {
			t.Fatal("router mutated DATA")
		}
	}
	// Wrong lease / wrong peer refused.
	dfRaw, _ := EncodeE2EEFrame(E2EEKindData, 0, 0, binding.Chan, nil, 9, []byte("z"))
	df, _ := DecodeE2EEFrame(dfRaw)
	if _, err := r.Route(df, "lease-other", phone, E2EERolePhone); err == nil {
		t.Fatal("cross-lease route admitted")
	}
	if _, err := r.Route(df, "lease-1", "99999999-9999-4999-8999-999999999999", E2EERolePhone); err == nil {
		t.Fatal("foreign sender admitted")
	}
	// Close frees caps.
	r.Close(binding.Chan)
	if _, ok := r.Lookup(binding.Chan); ok {
		t.Fatal("close kept binding")
	}
}

func TestRouterPerLeaseCap(t *testing.T) {
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	r := NewE2EERouter(nil, NewRelayStats(), nil)
	desktops := []string{
		"01234567-89ab-4cde-8f01-23456789abcd",
		"11111111-2222-4333-8444-555555555555",
		"22222222-3333-4444-8555-666666666666",
		"33333333-4444-4555-8666-777777777777",
		"44444444-5555-4666-8777-888888888888",
	}
	made := 0
	for i := 0; made < MaxE2EEChannelsPerLease; i++ {
		desktop := desktops[i%len(desktops)]
		chanID := uuid.New()
		peer := uuid.MustParse(desktop)
		raw, _ := EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, [16]byte(chanID), (*[16]byte)(&peer), 0, []byte("b"))
		f, _ := DecodeE2EEFrame(raw)
		_, _, err := r.RegisterHS1(f, raw, "lease-1", "account-a", "", phone, E2EERolePhone, func(target string) bool {
			for _, d := range desktops {
				if d == target {
					return true
				}
			}
			return false
		})
		if err != nil {
			t.Fatalf("chan %d: %v", made, err)
		}
		made++
	}
	// Ninth refused.
	chanID := uuid.New()
	peer := uuid.MustParse(desktops[0])
	raw, _ := EncodeE2EEFrame(E2EEKindHS, 1, E2EEPurposeCompanion, [16]byte(chanID), (*[16]byte)(&peer), 0, []byte("b"))
	f, _ := DecodeE2EEFrame(raw)
	if _, _, err := r.RegisterHS1(f, raw, "lease-1", "account-a", "", phone, E2EERolePhone, func(string) bool { return true }); err == nil {
		t.Fatal("ninth channel per lease admitted")
	}
}

// One phone can hold several sockets at once (discovery, a machine screen, a
// reconnect still draining). The desktop's reply must go back to the socket
// that opened the channel, not to whichever socket registered last — or the
// handshake is stranded and the relay closes the channel (live bug, 2026-11-10).
func TestRepliesGoToTheSocketThatOpenedTheChannel(t *testing.T) {
	h := &RPCHandler{}
	dom := e2eeFor(h)
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	first := &e2eeEndpoint{account: "account-a", device: phone, role: E2EERolePhone}
	second := &e2eeEndpoint{account: "account-a", device: phone, role: E2EERolePhone}
	host := &e2eeEndpoint{account: "account-a", device: desktop, role: E2EERoleHost}
	dom.put(E2EERolePhone, "account-a", phone, first)
	dom.put(E2EERoleHost, "account-a", desktop, host)

	raw := hs1Raw(t, "00112233445566778899aabbccddeeff", desktop, []byte("hs1"))
	frame, err := DecodeE2EEFrame(raw)
	if err != nil {
		t.Fatal(err)
	}
	binding, _, err := dom.router.RegisterHS1(frame, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil)
	if err != nil {
		t.Fatal(err)
	}
	binding.initiatorEP.Store(first)
	// A second socket of the same phone registers after the HS1 went out.
	dom.put(E2EERolePhone, "account-a", phone, second)

	if got := dom.otherSide(binding, phone); got != host {
		t.Fatalf("phone → desktop routed to %+v", got)
	}
	if got := dom.otherSide(binding, desktop); got != first {
		t.Fatalf("desktop reply went to the newer socket, not the one that opened the channel")
	}
	// Without a recorded socket (older bindings), the device lookup still works.
	var legacy E2EEBinding
	legacy.AccountID, legacy.InitiatorDevice, legacy.TargetDevice, legacy.Initiator = "account-a", phone, desktop, E2EERolePhone
	if got := dom.otherSide(&legacy, desktop); got != second {
		t.Fatalf("fallback lookup broken")
	}
}

func numberedHS1(t *testing.T, i int, desktop string) ([]byte, *E2EEFrame) {
	t.Helper()
	raw := hs1Raw(t, fmt.Sprintf("%032x", i+1), desktop, []byte("hs1"))
	frame, err := DecodeE2EEFrame(raw)
	if err != nil {
		t.Fatal(err)
	}
	return raw, frame
}

// A phone gives up on an unanswered handshake without sending CLOSE. Those
// channels must not hold cap slots forever, or the phone is locked out with
// "backpressure" on every new attempt (live bug, 2026-11-10).
func TestAbandonedHandshakesDoNotLockThePhoneOut(t *testing.T) {
	r := NewE2EERouter(NewChannelCaps(), NewRelayStats(), nil)
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	// Far more attempts than the pair cap (2): every new HS1 must get in,
	// evicting the pair's oldest channel, never "too many channels".
	var first *E2EEBinding
	for i := 0; i < 20; i++ {
		raw, frame := numberedHS1(t, i, desktop)
		b, _, err := r.RegisterHS1(frame, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil)
		if err != nil {
			t.Fatalf("attempt %d locked out: %v", i, err)
		}
		if first == nil {
			first = b
		}
	}
	if _, ok := r.Lookup(first.Chan); ok {
		t.Fatal("the oldest channel was not evicted")
	}
	if got := r.caps.PairCount(e2eePairKey(phone, desktop)); got > MaxE2EEChannelsPerPair {
		t.Fatalf("pair holds %d channels, cap %d", got, MaxE2EEChannelsPerPair)
	}
	// Unanswered handshakes also expire on their own.
	r.mu.Lock()
	for _, b := range r.bindings {
		b.createdAt = b.createdAt.Add(-2 * PendingHandshakeTTL)
	}
	r.mu.Unlock()
	r.expirePending(time.Now())
	if r.Count() != 0 {
		t.Fatalf("%d abandoned handshakes still held", r.Count())
	}
}

// An answered (established) channel is never expired by the sweep.
func TestEstablishedChannelsAreNotExpired(t *testing.T) {
	r := NewE2EERouter(NewChannelCaps(), NewRelayStats(), nil)
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	raw, frame := numberedHS1(t, 1, desktop)
	b, _, err := r.RegisterHS1(frame, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil)
	if err != nil {
		t.Fatal(err)
	}
	b.established.Store(true)
	b.createdAt = b.createdAt.Add(-2 * PendingHandshakeTTL)
	r.expirePending(time.Now())
	if _, ok := r.Lookup(b.Chan); !ok {
		t.Fatal("an established channel was expired")
	}
}

// A socket's channels end with it, on either side, freeing their slots.
func TestClosingASocketReleasesItsChannels(t *testing.T) {
	r := NewE2EERouter(NewChannelCaps(), NewRelayStats(), nil)
	phone := "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	desktop := "01234567-89ab-4cde-8f01-23456789abcd"
	phoneSock := &e2eeEndpoint{device: phone, role: E2EERolePhone}
	hostSock := &e2eeEndpoint{device: desktop, role: E2EERoleHost}
	for i := 0; i < MaxE2EEChannelsPerPair; i++ {
		raw, frame := numberedHS1(t, i, desktop)
		b, _, err := r.RegisterHS1(frame, raw, "lease-1", "account-a", desktop, phone, E2EERolePhone, nil)
		if err != nil {
			t.Fatal(err)
		}
		b.initiatorEP.Store(phoneSock)
		b.targetEP.Store(hostSock)
	}
	if n := r.CloseEndpoint(hostSock); n != MaxE2EEChannelsPerPair {
		t.Fatalf("closed %d channels, want %d", n, MaxE2EEChannelsPerPair)
	}
	if r.Count() != 0 || r.caps.LeaseCount("lease-1") != 0 {
		t.Fatalf("slots still held: bindings=%d lease=%d", r.Count(), r.caps.LeaseCount("lease-1"))
	}
}
