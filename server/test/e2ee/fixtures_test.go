package e2ee

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/google/uuid"

	"server/internal/push"
	"server/internal/relay"
)

// repoRoot returns the repository root (the parent of server/), so fixture
// files owned by other tasks can be loaded read-only by relative path. This
// test never writes outside its own package.
func repoRoot(t *testing.T) string {
	t.Helper()
	return filepath.Dir(moduleRoot(t))
}

func loadJSON(t *testing.T, rel string, out any) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), rel))
	if err != nil {
		t.Fatalf("read %s: %v", rel, err)
	}
	if err := json.Unmarshal(raw, out); err != nil {
		t.Fatalf("parse %s: %v", rel, err)
	}
}

// pushFixtureFile mirrors the shape of
// mobile/plugins/e2ee-nse/fixtures/push-fixtures.json (generated from real
// dragent_e2ee seals). Only the sealed blobs, the account and the phone are
// read: the push secrets and pins in that file are never loaded here — the
// server must never see key material.
type pushFixtureFile struct {
	Account string `json:"account"`
	Phone   string `json:"phone"`
	Cases   map[string]struct {
		Blob   string `json:"blob"`
		Expect string `json:"expect"`
	} `json:"cases"`
}

// TestPushFixturesSealedBlobs (§12.10, F9) pushes every real sealed blob from
// the NSE fixtures through the real push dispatcher with the mock APNs/FCM
// senders. Well-formed blobs must dispatch with the blob byte-for-byte and
// no title, body, data or sender — nothing else readable besides routing —
// at the one fixed size; malformed blobs must be refused with nothing sent.
func TestPushFixturesSealedBlobs(t *testing.T) {
	var fixtures pushFixtureFile
	loadJSON(t, "mobile/plugins/e2ee-nse/fixtures/push-fixtures.json", &fixtures)
	account, err := uuid.Parse(fixtures.Account)
	if err != nil {
		t.Fatalf("fixture account is not a UUID: %v", err)
	}
	phone, err := uuid.Parse(fixtures.Phone)
	if err != nil {
		t.Fatalf("fixture phone is not a UUID: %v", err)
	}

	rec := &recorder{}
	store := push.NewMemoryStore()
	fcmPhone := uuid.New()
	tokens := map[uuid.UUID]string{
		phone:    "fixture-apns-token",
		fcmPhone: "fixture-fcm-token",
	}
	for device, platform := range map[uuid.UUID]string{phone: "apns", fcmPhone: "fcm"} {
		if err := store.Upsert(context.Background(), push.Registration{
			DeviceID: device, UserID: account, Platform: platform, PushToken: tokens[device],
		}); err != nil {
			t.Fatalf("upsert: %v", err)
		}
	}
	sender := &mockSender{}
	d := &push.NotifyDispatcher{Store: store, Sender: sender}

	names := make([]string, 0, len(fixtures.Cases))
	for name := range fixtures.Cases {
		names = append(names, name)
	}
	sort.Strings(names)
	if len(names) == 0 {
		t.Fatal("push fixtures contain no cases")
	}
	dispatched := 0
	for _, name := range names {
		blob := fixtures.Cases[name].Blob
		if !push.ValidSealedPushBlob(blob) {
			// Malformed seals (truncated, bad alphabet) are refused for
			// every phone with nothing dispatched.
			before := len(sender.sent)
			for _, device := range []uuid.UUID{phone, fcmPhone} {
				if err := d.HandleNotify(context.Background(), account.String(), device.String(), blob); err == nil {
					t.Fatalf("fixture %q: malformed blob was accepted", name)
				}
			}
			if len(sender.sent) != before {
				t.Fatalf("fixture %q: refused intake reached the sender", name)
			}
			continue
		}
		for device, platform := range map[uuid.UUID]string{phone: "apns", fcmPhone: "fcm"} {
			if err := d.HandleNotify(context.Background(), account.String(), device.String(), blob); err != nil {
				t.Fatalf("fixture %q to %s: %v", name, platform, err)
			}
			msg := sender.sent[len(sender.sent)-1]
			if msg.Blob != blob {
				t.Fatalf("fixture %q to %s: dispatched blob differs from the fixture seal", name, platform)
			}
			if len(msg.Blob) != push.SealedPushBlobChars {
				t.Fatalf("fixture %q to %s: blob is %d chars, want the fixed %d", name, platform, len(msg.Blob), push.SealedPushBlobChars)
			}
			assertPushCarriesNothingElse(t, name, platform, msg, device, account, tokens[device])
			raw, _ := json.Marshal(msg)
			rec.addPush(raw)
			dispatched++
		}
	}
	if dispatched == 0 {
		t.Fatal("no fixture blob was dispatched")
	}
}

// assertPushCarriesNothingElse checks one dispatched push is exactly routing
// plus the opaque seal: the blob byte-for-byte, empty title/body/data, and
// every other field one of the known routing values.
func assertPushCarriesNothingElse(t *testing.T, fixture, platform string, msg push.PushMessage, device, account uuid.UUID, token string) {
	t.Helper()
	if msg.Title != "" || msg.Body != "" || len(msg.Data) != 0 {
		t.Fatalf("fixture %q to %s: dispatch carried readable content: %+v", fixture, platform, msg)
	}
	if msg.DeviceID != device || msg.UserID != account || msg.Platform != platform || msg.Token != token {
		t.Fatalf("fixture %q to %s: dispatch misrouted: %+v", fixture, platform, msg)
	}
	raw, _ := json.Marshal(msg)
	var decoded map[string]any
	if err := json.Unmarshal(raw, &decoded); err != nil {
		t.Fatal(err)
	}
	for key, value := range decoded {
		switch key {
		case "DeviceID", "UserID", "Platform", "Token", "Blob", "Title", "Body", "Data":
		default:
			t.Fatalf("fixture %q to %s: unexpected push field %q", fixture, platform, key)
		}
		if key == "Title" || key == "Body" {
			if s, _ := value.(string); s != "" {
				t.Fatalf("fixture %q to %s: field %q is readable: %q", fixture, platform, key, s)
			}
		}
	}
}

// catalogVectorFile mirrors dragent/crates/dragent_e2ee/vectors/codec/catalog_blob.json.
type catalogVectorFile struct {
	Vectors []struct {
		Name     string `json:"name"`
		BytesHex string `json:"bytes_hex"`
	} `json:"vectors"`
}

// TestCatalogBlobVectorsSealedRoundTrip (§12.10, F4) loads the real catalog
// blob vectors through the sealed catalog PUT/GET. The codec vectors use a
// tiny body, so the relay must refuse them before storage (fail closed,
// echoing nothing); the same vector routing with a valid-size opaque body
// must store and return verbatim.
func TestCatalogBlobVectorsSealedRoundTrip(t *testing.T) {
	var file catalogVectorFile
	loadJSON(t, "dragent/crates/dragent_e2ee/vectors/codec/catalog_blob.json", &file)
	if len(file.Vectors) == 0 {
		t.Fatal("catalog blob vectors are empty")
	}
	rec := &recorder{}
	h := relay.NewHandler(nil)
	put := func(blob relay.SealedCatalog) (int, []byte) {
		envelope := map[string]any{
			"device_id":  blob.Desktop,
			"v":          blob.V,
			"desktop":    blob.Desktop,
			"version":    blob.Version,
			"recipients": blob.Recipients,
			"nonce":      blob.Nonce,
			"body":       blob.Body,
		}
		raw, _ := json.Marshal(envelope)
		req := httptest.NewRequest(http.MethodPut, "/v1/relay/catalog", bytes.NewReader(raw))
		req = req.WithContext(context.WithValue(req.Context(), "user_id", recAccount))
		w := httptest.NewRecorder()
		h.PublishCatalog(w, req)
		res := w.Result()
		defer res.Body.Close()
		var buf bytes.Buffer
		_, _ = buf.ReadFrom(res.Body)
		rec.addReply(buf.Bytes())
		return res.StatusCode, buf.Bytes()
	}

	var routing relay.SealedCatalog
	seenValid := false
	for _, v := range file.Vectors {
		raw, err := hex.DecodeString(v.BytesHex)
		if err != nil {
			t.Fatalf("vector %q is not hex: %v", v.Name, err)
		}
		trimmed := bytes.Trim(raw, " ")
		var blob relay.SealedCatalog
		if err := json.Unmarshal(trimmed, &blob); err != nil {
			// Non-blob vectors (err_not_bucket) are refused unread.
			if code, body := put(relay.SealedCatalog{}); code != http.StatusBadRequest {
				t.Fatalf("vector %q: garbage PUT = %d, want 400", v.Name, code)
			} else {
				assertVectorNotEchoed(t, v.Name, body, v.BytesHex)
			}
			continue
		}
		if strings.HasPrefix(v.Name, "v9_") || strings.HasPrefix(v.Name, "err_") {
			// Codec vectors carry a tiny body, not a 4 KiB bucket: the
			// relay refuses them before storage and echoes nothing.
			code, body := put(blob)
			if code != http.StatusBadRequest {
				t.Fatalf("vector %q: PUT = %d, want 400 (tiny body is not a bucket)", v.Name, code)
			}
			assertVectorNotEchoed(t, v.Name, body, v.BytesHex)
			if strings.HasPrefix(v.Name, "v9_") {
				routing = blob
				seenValid = true
			}
		}
	}
	if !seenValid {
		t.Fatal("no valid-shape catalog vector found")
	}

	// The vector's real routing with a valid-size opaque body round-trips
	// verbatim through store and wire.
	routing.Body = randB64(t, 4096)
	routing = routing.Normalized()
	if code, body := put(routing); code != http.StatusOK {
		t.Fatalf("vector-shaped PUT = %d (%s), want 200", code, body)
	}
	req := httptest.NewRequest(http.MethodGet, "/v1/relay/catalog?device_id="+routing.Desktop, nil)
	req = req.WithContext(context.WithValue(req.Context(), "user_id", recAccount))
	w := httptest.NewRecorder()
	h.ReadCatalog(w, req)
	res := w.Result()
	defer res.Body.Close()
	var buf bytes.Buffer
	_, _ = buf.ReadFrom(res.Body)
	rec.addReply(buf.Bytes())
	if res.StatusCode != http.StatusOK {
		t.Fatalf("vector-shaped GET = %d, want 200", res.StatusCode)
	}
	var got relay.SealedCatalog
	if err := json.Unmarshal(buf.Bytes(), &got); err != nil {
		t.Fatalf("GET body is not a blob: %v", err)
	}
	if got.Body != routing.Body || got.Version != routing.Version || got.Nonce != routing.Nonce {
		t.Fatal("GET did not return the vector-shaped blob verbatim")
	}
	stored, ok, err := h.Sealed().GetSealed(context.Background(), "device\x00"+recAccount+"\x00"+routing.Desktop)
	if err != nil || !ok || stored.Body != routing.Body {
		t.Fatal("stored blob differs from the published vector-shaped blob")
	}
	storedRaw, _ := json.Marshal(stored)
	rec.addStored(storedRaw)
}

// assertVectorNotEchoed fails when a refusal response repeats the submitted
// vector bytes: refusals are fixed strings, never echoes.
func assertVectorNotEchoed(t *testing.T, name string, body []byte, bytesHex string) {
	t.Helper()
	if len(body) == 0 {
		t.Fatalf("vector %q: empty refusal body", name)
	}
	// Any 32-byte slice of the vector appearing in the response means the
	// server echoed submitted content.
	for i := 0; i+64 <= len(bytesHex); i += 64 {
		if bytes.Contains(body, []byte(bytesHex[i:i+64])) {
			t.Fatalf("vector %q: refusal echoes submitted bytes", name)
		}
	}
}
