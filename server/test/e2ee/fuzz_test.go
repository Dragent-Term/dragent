package e2ee

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"testing"

	"github.com/google/uuid"

	"server/internal/push"
	"server/internal/relay"
)

// Go native fuzzing (§12.11): the relay header, the router and the push
// intake. Each target must never panic; on success each asserts the
// ciphertext-opacity invariants (bodies unmodified, blobs fixed-size).

func seedFrame(t *testing.T, kind, step, purpose byte, chanID [16]byte, peer *[16]byte, ctr uint64, body []byte) []byte {
	t.Helper()
	raw, err := relay.EncodeE2EEFrame(kind, step, purpose, chanID, peer, ctr, body)
	if err != nil {
		t.Skipf("seed frame not encodable: %v", err)
	}
	return raw
}

// FuzzRelayHeaderDecode feeds arbitrary bytes to the relay header codec:
// decode must never panic, and any accepted frame re-encodes to identical
// bytes (the relay forwards unmodified).
func FuzzRelayHeaderDecode(f *testing.F) {
	peer := mustRawIDForFuzz(recDesktop)
	var ch [16]byte
	copy(ch[:], "0123456789abcdef")
	f.Add([]byte(seedFrameForFuzz(1)))
	f.Add([]byte{0x01, 0x02, 0x00, 0x00})
	f.Add([]byte{})
	f.Add(make([]byte, 44))
	f.Add(append(append([]byte{}, make([]byte, 44)...), 0x01))
	_ = peer
	_ = ch
	f.Fuzz(func(t *testing.T, raw []byte) {
		frame, err := relay.DecodeE2EEFrame(raw)
		if err != nil {
			return
		}
		rebuilt, err := relay.EncodeE2EEFrame(frame.Kind, frame.Step, frame.Purpose, frame.Chan, frame.Peer, frame.Ctr, frame.Body)
		if err != nil {
			t.Fatalf("accepted frame does not re-encode: %v", err)
		}
		if string(rebuilt) != string(raw) {
			t.Fatal("accepted frame does not round-trip byte-identically")
		}
	})
}

func seedFrameForFuzz(fill byte) []byte {
	var ch [16]byte
	for i := range ch {
		ch[i] = fill + byte(i)
	}
	peer, _ := relay.DeviceIDToRaw(recDesktop)
	body := []byte{fill, fill, fill}
	raw, err := relay.EncodeE2EEFrame(relay.E2EEKindData, 0, 0, ch, nil, uint64(fill), body)
	if err != nil {
		return []byte{0x01}
	}
	_ = peer
	return raw
}

func mustRawIDForFuzz(id string) [16]byte {
	raw, ok := relay.DeviceIDToRaw(id)
	if !ok {
		panic("bad fuzz device id")
	}
	return raw
}

// FuzzRouterRouting feeds arbitrary frame bytes plus routing labels to the
// real channel router: it must never panic, never mutate the input, and any
// registered channel must route its raw bytes unmodified.
func FuzzRouterRouting(f *testing.F) {
	f.Add([]byte(seedFrameForFuzz(2)), "lease-1", recPhone, recDesktop)
	f.Add([]byte{0x01}, "lease-1", "not-a-uuid", recDesktop)
	f.Add([]byte{}, "", "", "")
	f.Fuzz(func(t *testing.T, raw []byte, lease, src, target string) {
		before := append([]byte(nil), raw...)
		frame, err := relay.DecodeE2EEFrame(raw)
		if err != nil {
			return
		}
		router := relay.NewE2EERouter(nil, nil, nil)
		if frame.IsHS() && frame.Step == 1 {
			binding, forward, err := router.RegisterHS1(frame, raw, lease, recAccount, target, src, relay.E2EERolePhone, func(string) bool { return true })
			if err != nil {
				return
			}
			if string(raw) != string(before) {
				t.Fatal("RegisterHS1 mutated the input bytes")
			}
			// The forwarded copy differs only in the 16 peer bytes.
			if len(forward) != len(raw) {
				t.Fatal("forwarded HS1 changed length")
			}
			if string(forward[44:]) != string(raw[44:]) {
				t.Fatal("RegisterHS1 edited the HS1 body")
			}
			_ = binding
			return
		}
		binding, err := router.Route(frame, lease, src, relay.E2EERolePhone)
		if err != nil {
			return
		}
		_ = binding
		if string(raw) != string(before) {
			t.Fatal("Route mutated the input bytes")
		}
	})
}

// FuzzSealedCatalogBlob feeds arbitrary JSON to the sealed blob validator:
// it must never panic, and an accepted blob's wire sizes stay bucketed
// (body a 4 KiB multiple) so accepted blobs all look alike in size.
func FuzzSealedCatalogBlob(f *testing.F) {
	valid, _ := json.Marshal(map[string]any{
		"v": 1, "desktop": recDesktop, "version": 3,
		"recipients": []any{map[string]any{"phone": recPhone, "wrap": sealedPushBlobForFuzz()[:100]}},
		"nonce":      "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
		"body":       sealedPushBlobForFuzz()[:100],
	})
	f.Add(valid)
	f.Add([]byte(`{"v":1,"desktop":"x","version":1,"recipients":[],"nonce":"","body":""}`))
	f.Add([]byte(`{"projects":["leak"],"tabs":["leak"]}`))
	f.Add([]byte(`not json`))
	f.Add([]byte{})
	f.Fuzz(func(t *testing.T, raw []byte) {
		var blob relay.SealedCatalog
		if err := json.Unmarshal(raw, &blob); err != nil {
			return
		}
		blob = blob.Normalized()
		if err := blob.Validate(); err != nil {
			return
		}
		body, err := base64.URLEncoding.DecodeString(blob.Body)
		if err != nil {
			body, err = base64.StdEncoding.DecodeString(blob.Body)
			if err != nil {
				t.Fatal("accepted blob body is not base64")
			}
		}
		if len(body) == 0 || len(body)%4096 != 0 {
			t.Fatalf("accepted blob body is %d bytes, not a 4 KiB multiple", len(body))
		}
	})
}

func sealedPushBlobForFuzz() string {
	// 100 chars of base64url alphabet; validity of length is not required
	// for a seed, only that the fuzzer starts from plausible shapes.
	const alpha = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
	out := ""
	for len(out) < 1494 {
		out += alpha
	}
	return out[:1494]
}

// FuzzPushNotifyIntake feeds arbitrary account/phone/blob strings to the
// real push dispatcher: it must never panic, and anything handed to the
// sender must be a fixed-size valid blob for a live registration.
func FuzzPushNotifyIntake(f *testing.F) {
	f.Add(uuid.NewString(), uuid.NewString(), sealedPushBlobForFuzz())
	f.Add("not-a-uuid", "not-a-uuid", "")
	f.Add("", "", "")
	f.Add(uuid.NewString(), uuid.NewString(), "short")
	f.Fuzz(func(t *testing.T, accountID, phoneID, blob string) {
		store := push.NewMemoryStore()
		userID := uuid.New()
		phone := uuid.New()
		if err := store.Upsert(context.Background(), push.Registration{
			DeviceID: phone, UserID: userID, Platform: "fcm", PushToken: "fuzz-token",
		}); err != nil {
			t.Fatal(err)
		}
		sender := &mockSender{}
		d := &push.NotifyDispatcher{Store: store, Sender: sender}
		// Also try the fuzzed ids directly (unknown registrations refuse).
		if err := d.HandleNotify(context.Background(), accountID, phoneID, blob); err == nil {
			if len(sender.sent) != 1 {
				t.Fatal("accepted intake without exactly one dispatch")
			}
			if !push.ValidSealedPushBlob(sender.sent[0].Blob) {
				t.Fatal("dispatcher sent a non-fixed-size blob")
			}
		}
		// And the live registration with the fuzzed blob: acceptance
		// implies the blob passed the fixed-size gate.
		sender2 := &mockSender{}
		d2 := &push.NotifyDispatcher{Store: store, Sender: sender2}
		if err := d2.HandleNotify(context.Background(), userID.String(), phone.String(), blob); err == nil {
			if len(sender2.sent) != 1 || !push.ValidSealedPushBlob(sender2.sent[0].Blob) {
				t.Fatal("live dispatch without a valid sealed blob")
			}
			if sender2.sent[0].Title != "" || sender2.sent[0].Body != "" || len(sender2.sent[0].Data) != 0 {
				t.Fatal("live dispatch carried plaintext")
			}
		}
	})
}
