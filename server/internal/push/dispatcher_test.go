package push

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

type fakeSender struct {
	sent []PushMessage
	err  error
	name string
}

func (f *fakeSender) Send(_ context.Context, msg PushMessage) error {
	if f.err != nil {
		return f.err
	}
	f.sent = append(f.sent, msg)
	return nil
}

func (f *fakeSender) Name() string {
	if f.name != "" {
		return f.name
	}
	return "fake"
}

// sealedBlobForTest returns a valid opaque blob fixture. The bytes are
// meaningless ("A"/"B" runs); only length and alphabet matter, since the
// server never opens the seal.
func sealedBlobForTest(fill byte) string {
	return strings.Repeat(string([]byte{fill}), SealedPushBlobChars)
}

func notifyRig() (*MemoryStore, *fakeSender, *NotifyDispatcher, uuid.UUID, uuid.UUID) {
	store := NewMemoryStore()
	userID, deviceID := uuid.New(), uuid.New()
	sender := &fakeSender{}
	d := &NotifyDispatcher{Store: store, Sender: sender}
	if err := store.Upsert(context.Background(), Registration{
		DeviceID:  deviceID,
		UserID:    userID,
		Platform:  "fcm",
		PushToken: "test-token",
	}); err != nil {
		panic(err)
	}
	return store, sender, d, userID, deviceID
}

func TestValidSealedPushBlob(t *testing.T) {
	if !ValidSealedPushBlob(sealedBlobForTest('A')) {
		t.Fatal("all-A blob of exact length must be valid")
	}
	for _, bad := range []string{
		"",
		strings.Repeat("A", 100),
		strings.Repeat("A", SealedPushBlobChars-1),
		strings.Repeat("A", SealedPushBlobChars+1),
		strings.Repeat("!", SealedPushBlobChars),
		sealedBlobForTest('A')[:100] + "!" + sealedBlobForTest('A')[101:],
		sealedBlobForTest('A')[:SealedPushBlobChars-2] + "==",
	} {
		if ValidSealedPushBlob(bad) {
			t.Fatalf("invalid blob of length %d was accepted", len(bad))
		}
	}
	// base64url alphabet without padding is accepted in full.
	all := "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
	for len(all) < SealedPushBlobChars {
		all += all
	}
	if !ValidSealedPushBlob(all[:SealedPushBlobChars]) {
		t.Fatal("full base64url alphabet was rejected")
	}
}

func TestHandleNotifyDispatchesOpaqueBlob(t *testing.T) {
	_, sender, d, userID, deviceID := notifyRig()
	blob := sealedBlobForTest('A')
	if err := d.HandleNotify(context.Background(), userID.String(), deviceID.String(), blob); err != nil {
		t.Fatalf("HandleNotify: %v", err)
	}
	if len(sender.sent) != 1 {
		t.Fatalf("sent %d messages, want 1", len(sender.sent))
	}
	msg := sender.sent[0]
	if msg.Blob != blob {
		t.Fatal("dispatched blob differs from the desktop's seal")
	}
	if msg.Token != "test-token" || msg.DeviceID != deviceID {
		t.Fatalf("message routed to the wrong registration: %+v", msg)
	}
	// The opaque path carries no plaintext: even deprecated fields stay empty.
	if msg.Title != "" || msg.Body != "" || len(msg.Data) != 0 {
		t.Fatalf("opaque dispatch carried plaintext: %+v", msg)
	}
}

func TestHandleNotifyRefusals(t *testing.T) {
	_, sender, d, userID, deviceID := notifyRig()
	blob := sealedBlobForTest('A')
	other := uuid.New()

	cases := []struct {
		name    string
		account string
		phone   string
		blob    string
	}{
		{"short blob", userID.String(), deviceID.String(), strings.Repeat("A", 100)},
		{"bad alphabet", userID.String(), deviceID.String(), strings.Repeat("!", SealedPushBlobChars)},
		{"empty blob", userID.String(), deviceID.String(), ""},
		{"unknown phone", userID.String(), uuid.NewString(), blob},
		{"foreign account", other.String(), deviceID.String(), blob},
		{"bad phone id", userID.String(), "not-a-uuid", blob},
		{"bad account id", "not-a-uuid", deviceID.String(), blob},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if err := d.HandleNotify(context.Background(), tc.account, tc.phone, tc.blob); err == nil {
				t.Fatal("expected a refusal")
			}
		})
	}
	if len(sender.sent) != 0 {
		t.Fatalf("refused intake reached the sender: %+v", sender.sent)
	}
}

func TestHandleNotifySkipsLapsedRegistration(t *testing.T) {
	store := NewMemoryStore()
	userID, deviceID := uuid.New(), uuid.New()
	fixed := time.Now()
	store.now = func() time.Time { return fixed }
	if err := store.Upsert(context.Background(), Registration{
		DeviceID: deviceID, UserID: userID, Platform: "apns", PushToken: "test-token",
	}); err != nil {
		t.Fatalf("upsert: %v", err)
	}
	sender := &fakeSender{}
	d := &NotifyDispatcher{Store: store, Sender: sender,
		Now: func() time.Time { return fixed.Add(RegistrationTTL + time.Hour) }}
	if err := d.HandleNotify(context.Background(), userID.String(), deviceID.String(), sealedBlobForTest('A')); err == nil {
		t.Fatal("lapsed registration was accepted")
	}
	if len(sender.sent) != 0 {
		t.Fatal("lapsed registration reached the sender")
	}
}

func TestHandleNotifyNotWired(t *testing.T) {
	d := &NotifyDispatcher{}
	if err := d.HandleNotify(context.Background(), uuid.NewString(), uuid.NewString(), sealedBlobForTest('A')); err == nil {
		t.Fatal("unwired dispatcher was accepted")
	}
}
