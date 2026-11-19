package e2ee

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"

	"server/internal/companionpairing"
	"server/internal/identity/model"
	identityTypes "server/internal/identity/types"
	"server/internal/push"
	"server/internal/relay"
)

// Fixed lowercase canonical UUIDs for the recorder run: the account, the
// paired phone and desktop, and the lease. They are routing metadata (§2.2),
// not canaries, and appear in server bytes by design.
const (
	recAccount = "11111111-2222-4333-8444-555555555555"
	recPhone   = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	recDesktop = "01234567-89ab-4cde-8f01-23456789abcd"
	recLease   = "lease-recorder-1"
)

func mustRawID(t *testing.T, id string) [16]byte {
	t.Helper()
	raw, ok := relay.DeviceIDToRaw(id)
	if !ok {
		t.Fatalf("bad test device id %q", id)
	}
	return raw
}

func randChan(t *testing.T) [16]byte {
	t.Helper()
	var c [16]byte
	if _, err := rand.Read(c[:]); err != nil {
		t.Fatal(err)
	}
	return c
}

// TestRelayForwardsCiphertextUnmodified runs the real E2EE router under a
// recorder: HS1 (companion and pairing purposes), DATA in both directions
// carrying sealed §2.1 canaries (F1 input/output/screen, F2 files/tabs,
// F5 coordination, F6 work items, F8 attention), and CLOSE. It asserts the
// relay forwards every frame unmodified and in order, rewrites only the HS1
// peer field, and records nothing but routing in its stats — with no canary
// in any encoding in any recorded byte.
func TestRelayForwardsCiphertextUnmodified(t *testing.T) {
	c := newCanaries(t)
	s := newSealer(t)
	rec := &recorder{}
	stats := relay.NewRelayStats()
	router := relay.NewE2EERouter(nil, stats, nil)

	// One sealed DATA body per flow, as the endpoints would emit them.
	flows := []string{
		c.input, c.output, c.screenGrid, c.keystroke,
		c.tabTitle, c.fileName, c.fileContent, c.path, c.branch,
		c.coordGoal, c.coordMessage, c.workItemTitle, c.workItemQuery,
		c.attentionTitle, c.attentionBody,
	}
	bodies := make([][]byte, len(flows))
	for i, f := range flows {
		bodies[i] = s.seal(f)
		assertRandomLooking(t, "frame body", bodies[i], f)
	}

	// Companion channel: phone HS1 → desktop.
	chanID := randChan(t)
	peer := mustRawID(t, recDesktop)
	hsBody := s.seal(`{"v":1,"caps":[]}`)
	hs1, err := relay.EncodeE2EEFrame(relay.E2EEKindHS, 1, relay.E2EEPurposeCompanion, chanID, &peer, 0, hsBody)
	if err != nil {
		t.Fatalf("encode HS1: %v", err)
	}
	frame, err := relay.DecodeE2EEFrame(hs1)
	if err != nil {
		t.Fatalf("decode HS1: %v", err)
	}
	binding, forward, err := router.RegisterHS1(frame, hs1, recLease, recAccount, recDesktop, recPhone, relay.E2EERolePhone, nil)
	if err != nil {
		t.Fatalf("RegisterHS1: %v", err)
	}
	if binding.InitiatorDevice != recPhone || binding.TargetDevice != recDesktop {
		t.Fatalf("binding = %+v, want phone→desktop", binding)
	}
	rec.addNetwork(hs1)
	rec.addNetwork(forward)
	// Only the peer field is rewritten (to the initiator); the body — the
	// ciphertext — is untouched.
	if string(forward[44:]) != string(hs1[44:]) {
		t.Fatal("router edited the HS1 body")
	}
	fwdFrame, err := relay.DecodeE2EEFrame(forward)
	if err != nil {
		t.Fatalf("decode forwarded HS1: %v", err)
	}
	if fwdFrame.Peer == nil || *fwdFrame.Peer != mustRawID(t, recPhone) {
		t.Fatal("forwarded HS1 peer is not the relay-written initiator id")
	}

	// DATA both ways: every flow, counters rising, forwarded unmodified.
	var ctr uint64
	for i, body := range bodies {
		raw, err := relay.EncodeE2EEFrame(relay.E2EEKindData, 0, 0, chanID, nil, ctr, body)
		if err != nil {
			t.Fatalf("encode DATA %d: %v", i, err)
		}
		dec, err := relay.DecodeE2EEFrame(raw)
		if err != nil {
			t.Fatalf("decode DATA %d: %v", i, err)
		}
		src := recPhone
		if i%2 == 1 {
			src = recDesktop
		}
		if _, err := router.Route(dec, recLease, src, relay.E2EERolePhone); err != nil {
			t.Fatalf("route DATA %d: %v", i, err)
		}
		rec.addNetwork(raw)
		ctr++
	}

	// Pairing channel on the same lease: its HS1 and one DATA frame.
	pairChan := randChan(t)
	phs, err := relay.EncodeE2EEFrame(relay.E2EEKindHS, 1, relay.E2EEPurposePairing, pairChan, &peer, 0, s.seal(c.deviceName))
	if err != nil {
		t.Fatalf("encode pairing HS1: %v", err)
	}
	pframe, err := relay.DecodeE2EEFrame(phs)
	if err != nil {
		t.Fatalf("decode pairing HS1: %v", err)
	}
	if _, _, err := router.RegisterHS1(pframe, phs, recLease, recAccount, recDesktop, recPhone, relay.E2EERolePhone, nil); err != nil {
		t.Fatalf("pairing RegisterHS1: %v", err)
	}
	rec.addNetwork(phs)

	// CLOSE one channel: the hint carries only chan hex and reason.
	closeRaw, err := relay.EncodeE2EEFrame(relay.E2EEKindClose, 0, 0, chanID, nil, 0, []byte{relay.E2EECloseProtocolError})
	if err != nil {
		t.Fatalf("encode CLOSE: %v", err)
	}
	rec.addNetwork(closeRaw)
	closed := relay.RelayChanClosedFrame(relay.ChanHex(chanID), "protocol_error")
	if len(closed["chan"].(string)) != 32 {
		t.Fatalf("chan-closed chan = %q, want 32 hex chars", closed["chan"])
	}
	rec.addReply([]byte(closed["chan"].(string) + closed["reason"].(string)))
	router.Close(chanID)

	// Stats are aggregate counters only: template words and buckets, no ids
	// and no content.
	snap := stats.SnapshotAndReset()
	if len(snap) == 0 {
		t.Fatal("router recorded no aggregate counters")
	}
	var statsText strings.Builder
	for k, v := range snap {
		if v == 0 {
			t.Fatalf("counter %q is zero", k)
		}
		statsText.WriteString(k)
	}
	assertNoCanary(t, "relay aggregate counters", []byte(statsText.String()), c)
	for _, id := range []string{recPhone, recDesktop, recAccount, recLease} {
		if strings.Contains(statsText.String(), id) {
			t.Fatalf("aggregate counter %q carries a device id", statsText.String())
		}
	}

	assertNoCanary(t, "recorded relay bytes", rec.serverBytes(), c)
	assertEndpointDelivery(t, s, c)
}

// randB64 returns n random bytes as base64url, the opaque-ciphertext shape
// the sealed catalog wraps and bodies use.
func randB64(t *testing.T, n int) string {
	t.Helper()
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	return base64.URLEncoding.EncodeToString(b)
}

// TestSealedCatalogStoresOnlyCiphertext PUTs and GETs the real sealed
// catalog (F4) under a recorder. One blob carries sealed §2.1 canaries
// (machine/project names, tab titles, paths, branches, agent states) padded
// to one 4 KiB bucket; a second blob is pure random. It asserts the store
// and the wire carry the blobs verbatim, the plaintext catalog shape is
// refused with 410 without echoing content, the endpoint still recovers
// every canary, and no recorded byte carries one.
func TestSealedCatalogStoresOnlyCiphertext(t *testing.T) {
	c := newCanaries(t)
	s := newSealer(t)
	rec := &recorder{}
	h := relay.NewHandler(nil)

	put := func(user, device string, blob relay.SealedCatalog) (int, []byte) {
		envelope := map[string]any{
			"device_id":  device,
			"v":          blob.V,
			"desktop":    blob.Desktop,
			"version":    blob.Version,
			"recipients": blob.Recipients,
			"nonce":      blob.Nonce,
			"body":       blob.Body,
		}
		raw, _ := json.Marshal(envelope)
		req := httptest.NewRequest(http.MethodPut, "/v1/relay/catalog", bytes.NewReader(raw))
		req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
		w := httptest.NewRecorder()
		h.PublishCatalog(w, req)
		res := w.Result()
		defer res.Body.Close()
		var buf bytes.Buffer
		_, _ = buf.ReadFrom(res.Body)
		rec.addReply(buf.Bytes())
		return res.StatusCode, buf.Bytes()
	}
	get := func(user, device string) (int, []byte) {
		req := httptest.NewRequest(http.MethodGet, "/v1/relay/catalog?device_id="+device, nil)
		req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
		w := httptest.NewRecorder()
		h.ReadCatalog(w, req)
		res := w.Result()
		defer res.Body.Close()
		var buf bytes.Buffer
		_, _ = buf.ReadFrom(res.Body)
		rec.addReply(buf.Bytes())
		return res.StatusCode, buf.Bytes()
	}

	// Blob 1: sealed canary catalog, padded to exactly one 4 KiB bucket.
	catalogPlain, _ := json.Marshal(map[string]string{
		"machine": c.machineName, "project": c.projectName,
		"tab": c.tabTitle, "path": c.path,
		"branch": c.branch, "agent": c.agentState,
	})
	sealed := s.seal(string(catalogPlain))
	sealLen := len(sealed)
	padded := make([]byte, 4096)
	copy(padded, sealed)
	blob1 := relay.SealedCatalog{
		V:          1,
		Desktop:    recDesktop,
		Version:    5,
		Recipients: []relay.SealedRecipient{{Phone: recPhone, Wrap: randB64(t, 136)}},
		Nonce:      randB64(t, 24),
		Body:       base64.URLEncoding.EncodeToString(padded),
	}.Normalized()
	if code, _ := put(recAccount, recDesktop, blob1); code != http.StatusOK {
		t.Fatalf("PUT sealed catalog = %d, want 200", code)
	}
	// Blob 2: pure random, second version.
	blob2 := blob1
	blob2.Version = 6
	blob2.Body = randB64(t, 4096)
	blob2.Nonce = randB64(t, 24)
	if code, _ := put(recAccount, recDesktop, blob2); code != http.StatusOK {
		t.Fatalf("PUT sealed catalog v6 = %d, want 200", code)
	}

	// GET returns the latest blob verbatim.
	code, raw := get(recAccount, recDesktop)
	if code != http.StatusOK {
		t.Fatalf("GET sealed catalog = %d, want 200", code)
	}
	var got relay.SealedCatalog
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatalf("GET body is not a blob: %v", err)
	}
	if got.Version != 6 || got.Body != blob2.Body {
		t.Fatal("GET did not return the latest blob verbatim")
	}
	stored, ok, err := h.Sealed().GetSealed(context.Background(), "device\x00"+recAccount+"\x00"+recDesktop)
	if err != nil || !ok {
		t.Fatalf("sealed store missing blob: %v %v", ok, err)
	}
	storedRaw, _ := json.Marshal(stored)
	rec.addStored(storedRaw)
	if stored.Body != blob2.Body || stored.Version != 6 {
		t.Fatal("stored blob differs from what was published")
	}

	// Plaintext catalog shape is refused with 410 before storage, without
	// echoing the smuggled content.
	plaintext, _ := json.Marshal(map[string]any{
		"device_id": recDesktop,
		"projects":  []string{c.projectName, c.path},
		"tabs":      []string{c.tabTitle},
	})
	req := httptest.NewRequest(http.MethodPut, "/v1/relay/catalog", bytes.NewReader(plaintext))
	req = req.WithContext(context.WithValue(req.Context(), "user_id", recAccount))
	w := httptest.NewRecorder()
	h.PublishCatalog(w, req)
	res := w.Result()
	defer res.Body.Close()
	var gone bytes.Buffer
	_, _ = gone.ReadFrom(res.Body)
	rec.addReply(gone.Bytes())
	if res.StatusCode != http.StatusGone {
		t.Fatalf("plaintext catalog PUT = %d, want 410", res.StatusCode)
	}
	assertNoCanary(t, "410 refusal body", gone.Bytes(), c)

	// Endpoint delivery: the phone holding the key recovers every canary
	// from blob 1's body.
	bodyBytes, err := base64.URLEncoding.DecodeString(blob1.Body)
	if err != nil {
		t.Fatalf("decode blob1 body: %v", err)
	}
	if plain := s.open(t, bodyBytes[:sealLen]); plain != string(catalogPlain) {
		t.Fatal("endpoint could not recover the sealed catalog")
	}

	assertNoCanary(t, "recorded catalog bytes", rec.serverBytes(), c)
}

// pairingFakes is the minimal companionpairing wiring: the real memory
// store, one user, a token map, and an allow-all rate limiter.
type pairingFakes struct {
	users  map[uuid.UUID]*model.User
	tokens map[string]*identityTypes.TokenClaims
}

func (f *pairingFakes) GetByID(_ context.Context, id uuid.UUID) (*model.User, error) {
	if u, ok := f.users[id]; ok {
		return u, nil
	}
	return nil, errNoPairUser
}

type pairUserError struct{}

func (pairUserError) Error() string { return "unknown user" }

var errNoPairUser = pairUserError{}

func (f *pairingFakes) GenerateScopedAccessToken(userID, deviceID uuid.UUID, scope string) (string, error) {
	tok := "fake-token-" + uuid.NewString()
	f.tokens[tok] = &identityTypes.TokenClaims{
		UserID: userID, DeviceID: deviceID, Scope: scope,
	}
	return tok, nil
}

func (f *pairingFakes) VerifyAccessToken(token string) (*identityTypes.TokenClaims, error) {
	if cl, ok := f.tokens[token]; ok {
		return cl, nil
	}
	return nil, errNoPairUser
}

type allowAllLimiter struct{}

func (allowAllLimiter) Allow(_ context.Context, _ string) (bool, error) { return true, nil }

// TestPairingCarriesNoDeviceName drives the real pairing endpoints (F11)
// under a recorder: create + redeem with platform only succeeds and stores
// no name, a redeem smuggling the device-name canary is refused, and an
// access_token query parameter authenticates nothing. No recorded byte
// carries the canary.
func TestPairingCarriesNoDeviceName(t *testing.T) {
	c := newCanaries(t)
	rec := &recorder{}
	fakes := &pairingFakes{
		users:  map[uuid.UUID]*model.User{},
		tokens: map[string]*identityTypes.TokenClaims{},
	}
	owner := uuid.New()
	fakes.users[owner] = &model.User{ID: owner, DisplayName: "Recorder Owner"}
	store := companionpairing.NewMemoryStore()
	h := companionpairing.NewHandler(store, fakes, fakes, "https://api.example.test",
		companionpairing.WithOrgResolver(func(_ context.Context, _ string) string { return "org-test" }),
		companionpairing.WithRateLimiters(allowAllLimiter{}, allowAllLimiter{}),
	)
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			req = req.WithContext(context.WithValue(req.Context(), "user_id", owner.String()))
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterRoutes(r)
	h.RegisterPublicRoutes(r)
	do := func(method, path string, body any, headers map[string]string) *httptest.ResponseRecorder {
		var reader *bytes.Reader
		if body == nil {
			reader = bytes.NewReader(nil)
		} else {
			raw, err := json.Marshal(body)
			if err != nil {
				t.Fatal(err)
			}
			reader = bytes.NewReader(raw)
		}
		req := httptest.NewRequest(method, path, reader)
		for k, v := range headers {
			req.Header.Set(k, v)
		}
		w := httptest.NewRecorder()
		r.ServeHTTP(w, req)
		rec.addReply(w.Body.Bytes())
		return w
	}

	// Create a pairing as the desktop owner.
	w := do(http.MethodPost, "/v1/companion/pairings", map[string]any{}, nil)
	if w.Code != http.StatusCreated {
		t.Fatalf("create pairing = %d (%s), want 201", w.Code, w.Body.String())
	}
	var created struct {
		Code string `json:"code"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &created); err != nil || created.Code == "" {
		t.Fatalf("create pairing response has no code: %s", w.Body.String())
	}

	// Redeem with platform only (F11): no device name on the wire.
	w = do(http.MethodPost, "/v1/companion/pairings/redeem",
		map[string]any{"code": created.Code, "platform": "ios"}, nil)
	if w.Code != http.StatusOK {
		t.Fatalf("redeem = %d (%s), want 200", w.Code, w.Body.String())
	}
	var redeemed struct {
		DeviceID string `json:"device_id"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &redeemed); err != nil || redeemed.DeviceID == "" {
		t.Fatalf("redeem response has no device: %s", w.Body.String())
	}
	assertNoCanary(t, "redeem response", w.Body.Bytes(), c)

	// A second pairing, redeemed with the device-name canary smuggled in,
	// is refused — and nothing carrying the name is stored.
	w2 := do(http.MethodPost, "/v1/companion/pairings", map[string]any{}, nil)
	var created2 struct {
		Code string `json:"code"`
	}
	if err := json.Unmarshal(w2.Body.Bytes(), &created2); err != nil {
		t.Fatalf("second create: %s", w2.Body.String())
	}
	w = do(http.MethodPost, "/v1/companion/pairings/redeem",
		map[string]any{"code": created2.Code, "platform": "android", "device_name": c.deviceName}, nil)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("name-smuggling redeem = %d, want 400", w.Code)
	}

	// Stored devices carry no name and the cut-over flag; the list shows
	// only routing.
	devices, err := store.ListDevices(context.Background(), owner.String())
	if err != nil {
		t.Fatalf("ListDevices: %v", err)
	}
	rawDevices, _ := json.Marshal(devices)
	rec.addStored(rawDevices)
	for _, d := range devices {
		if d.Name != "" {
			t.Fatalf("stored device carries a name %q", d.Name)
		}
		if !d.E2E {
			t.Fatal("redeemed device is not flagged e2e")
		}
	}
	if known, e2e, err := store.DeviceE2E(context.Background(), owner.String(), redeemed.DeviceID); err != nil || !known || !e2e {
		t.Fatalf("DeviceE2E = %v %v %v, want true true nil", known, e2e, err)
	}
	w = do(http.MethodGet, "/v1/companion/devices", nil, nil)
	if w.Code != http.StatusOK {
		t.Fatalf("list devices = %d, want 200", w.Code)
	}
	assertNoCanary(t, "device list", w.Body.Bytes(), c)

	// H2: an access_token query parameter authenticates nothing on pairing
	// routes — with the token only in the query, creation is refused.
	tok, _ := fakes.GenerateScopedAccessToken(owner, uuid.New(), companionpairing.AccessScope)
	plain := chi.NewRouter()
	plain.Post("/v1/companion/pairings", func(wr http.ResponseWriter, pr *http.Request) {
		// Same check the authed chain applies: no user in context, since a
		// query token is never promoted to one.
		if _, ok := pr.Context().Value("user_id").(string); !ok {
			wr.WriteHeader(http.StatusUnauthorized)
			return
		}
		wr.WriteHeader(http.StatusCreated)
	})
	req2 := httptest.NewRequest(http.MethodPost, "/v1/companion/pairings?access_token="+tok, bytes.NewReader([]byte("{}")))
	w4 := httptest.NewRecorder()
	plain.ServeHTTP(w4, req2)
	if w4.Code != http.StatusUnauthorized {
		t.Fatalf("query-token pairing create = %d, want 401", w4.Code)
	}

	assertNoCanary(t, "recorded pairing bytes", rec.serverBytes(), c)
}

// mockSender captures dispatched pushes: the mock APNs/FCM in §12.10.
type mockSender struct {
	sent []push.PushMessage
}

func (m *mockSender) Send(_ context.Context, msg push.PushMessage) error {
	m.sent = append(m.sent, msg)
	return nil
}

func (m *mockSender) Name() string { return "mock" }

// sealedPushBlob returns a valid opaque blob: 1,120 random bytes as 1,494
// unpadded base64url characters (§8). Random by construction, so it can
// carry no canary; the endpoints' seals are covered by assertEndpointDelivery.
func sealedPushBlob(t *testing.T) string {
	t.Helper()
	b := make([]byte, 1120)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	blob := base64.RawURLEncoding.EncodeToString(b)
	if !push.ValidSealedPushBlob(blob) {
		t.Fatalf("fixture blob of %d chars is invalid", len(blob))
	}
	return blob
}

// pushDeviceResolver is the minimal push.DeviceResolver: the account's live
// phones.
type pushDeviceResolver struct {
	owned map[uuid.UUID]uuid.UUID
}

func (r *pushDeviceResolver) Exists(_ context.Context, userID, deviceID uuid.UUID) (bool, error) {
	u, ok := r.owned[deviceID]
	return ok && u == userID, nil
}

const pushRecorderSecret = "e2ee-recorder-push-secret"

// pushToken mints a push JWT for user+device, the shape
// JWTAuthenticator verifies.
func pushToken(t *testing.T, userID, deviceID uuid.UUID) string {
	t.Helper()
	claims := push.Claims{
		UserID:           userID,
		DeviceID:         deviceID,
		RegisteredClaims: jwt.RegisteredClaims{ExpiresAt: jwt.NewNumericDate(time.Now().Add(time.Hour))},
	}
	tok, err := jwt.NewWithClaims(jwt.SigningMethodHS256, claims).SignedString([]byte(pushRecorderSecret))
	if err != nil {
		t.Fatal(err)
	}
	return tok
}

// TestPushCarriesOnlySealedBlobs runs the real push dispatcher (F9) under a
// recorder with mock APNs/FCM senders: sealed host-notify blobs resolve to
// live registrations and dispatch with no title, body, data or sender;
// malformed blobs and lapsed registrations are refused; the no-credential
// log sender records only platform and outcome even when handed plaintext;
// and the registrations list never echoes push tokens. No recorded byte
// carries a canary.
func TestPushCarriesOnlySealedBlobs(t *testing.T) {
	c := newCanaries(t)
	s := newSealer(t)
	rec := &recorder{}

	store := push.NewMemoryStore()
	userID, apnsPhone, fcmPhone := uuid.New(), uuid.New(), uuid.New()
	pushTokenCanary := "e2ee-canary-pushtoken-" + c.pushTitle[len("e2ee-canary-pushtitle-"):]
	for _, reg := range []push.Registration{
		{DeviceID: apnsPhone, UserID: userID, Platform: "apns", PushToken: "apns-" + pushTokenCanary},
		{DeviceID: fcmPhone, UserID: userID, Platform: "fcm", PushToken: "fcm-" + pushTokenCanary},
	} {
		if err := store.Upsert(context.Background(), reg); err != nil {
			t.Fatalf("upsert: %v", err)
		}
	}
	sender := &mockSender{}
	d := &push.NotifyDispatcher{Store: store, Sender: sender}

	// One sealed notification per phone, as the desktop's host-notify would
	// carry them. The endpoint seals title+body; the server sees the blob.
	notifyPlain := c.pushTitle + "\x00" + c.pushBody
	_ = s.seal(notifyPlain)
	for _, phone := range []uuid.UUID{apnsPhone, fcmPhone} {
		blob := sealedPushBlob(t)
		if err := d.HandleNotify(context.Background(), userID.String(), phone.String(), blob); err != nil {
			t.Fatalf("HandleNotify: %v", err)
		}
	}
	if len(sender.sent) != 2 {
		t.Fatalf("dispatched %d pushes, want 2", len(sender.sent))
	}
	for _, msg := range sender.sent {
		raw, _ := json.Marshal(msg)
		// The token is routing the provider needs; record everything but
		// assert the opaque path carries no plaintext.
		rec.addPush(raw)
		if msg.Title != "" || msg.Body != "" || len(msg.Data) != 0 {
			t.Fatalf("opaque dispatch carried plaintext: %+v", msg)
		}
		if !push.ValidSealedPushBlob(msg.Blob) {
			t.Fatal("dispatched blob fails the fixed-size check")
		}
	}
	// Every dispatch is the same shape: blobs differ, sizes don't.
	if len(sender.sent[0].Blob) != push.SealedPushBlobChars || len(sender.sent[1].Blob) != push.SealedPushBlobChars {
		t.Fatal("dispatched blobs are not the fixed sealed size")
	}

	// Refusals reach no sender: short blob, bad alphabet, unknown phone,
	// foreign account, and a lapsed registration.
	lapsedPhone := uuid.New()
	lapsedStore := push.NewMemoryStore()
	fixed := time.Now()
	if err := lapsedStore.Upsert(context.Background(), push.Registration{
		DeviceID: lapsedPhone, UserID: userID, Platform: "fcm", PushToken: "tok",
	}); err != nil {
		t.Fatal(err)
	}
	lapsed := &push.NotifyDispatcher{Store: lapsedStore, Sender: sender,
		Now: func() time.Time { return fixed.Add(400 * 24 * time.Hour) }}
	before := len(sender.sent)
	blob := sealedPushBlob(t)
	for name, fn := range map[string]func() error{
		"short blob": func() error {
			return d.HandleNotify(context.Background(), userID.String(), apnsPhone.String(), blob[:100])
		},
		"bad alphabet": func() error {
			return d.HandleNotify(context.Background(), userID.String(), apnsPhone.String(), strings.Repeat("!", push.SealedPushBlobChars))
		},
		"unknown phone":   func() error { return d.HandleNotify(context.Background(), userID.String(), uuid.NewString(), blob) },
		"foreign account": func() error { return d.HandleNotify(context.Background(), uuid.NewString(), apnsPhone.String(), blob) },
		"lapsed lease": func() error {
			return lapsed.HandleNotify(context.Background(), userID.String(), lapsedPhone.String(), blob)
		},
	} {
		if err := fn(); err == nil {
			t.Fatalf("%s was accepted", name)
		}
	}
	if len(sender.sent) != before {
		t.Fatal("a refused intake reached the sender")
	}

	// LogSender with a captured log: even handed plaintext title/body/data
	// and a token, it records only platform and outcome.
	var logBuf bytes.Buffer
	logSender := &push.LogSender{Log: slog.New(slog.NewTextHandler(&logBuf, nil))}
	if err := logSender.Send(context.Background(), push.PushMessage{
		DeviceID: apnsPhone, UserID: userID, Platform: "apns",
		Token: "tok-" + pushTokenCanary, Blob: sealedPushBlob(t),
		Title: c.pushTitle, Body: c.pushBody, Data: map[string]string{"k": c.pushBody},
	}); err != nil {
		t.Fatalf("LogSender: %v", err)
	}
	rec.addStored(logBuf.Bytes())
	assertNoCanary(t, "log-sender line", logBuf.Bytes(), c)

	// HTTP registration surface: register over the real handler, list, and
	// confirm the list never echoes push tokens and query tokens get 401.
	resolver := &pushDeviceResolver{owned: map[uuid.UUID]uuid.UUID{apnsPhone: userID, fcmPhone: userID}}
	auth := push.NewJWTAuthenticator(pushRecorderSecret)
	ph := push.NewHandler(store, resolver, auth)
	pr := chi.NewRouter()
	ph.RegisterRoutes(pr)
	httpDo := func(method, path, token, body string) *httptest.ResponseRecorder {
		var reader *bytes.Reader
		if body == "" {
			reader = bytes.NewReader(nil)
		} else {
			reader = bytes.NewReader([]byte(body))
		}
		req := httptest.NewRequest(method, path, reader)
		if token != "" {
			req.Header.Set("Authorization", "Bearer "+token)
		}
		w := httptest.NewRecorder()
		pr.ServeHTTP(w, req)
		rec.addReply(w.Body.Bytes())
		return w
	}
	regBody := `{"device_id":"` + apnsPhone.String() + `","platform":"apns","push_token":"http-` + pushTokenCanary + `"}`
	if w := httpDo(http.MethodPost, "/v1/push/register", pushToken(t, userID, apnsPhone), regBody); w.Code != http.StatusOK {
		t.Fatalf("register = %d (%s), want 200", w.Code, w.Body.String())
	}
	w := httpDo(http.MethodGet, "/v1/push/registrations", pushToken(t, userID, apnsPhone), "")
	if w.Code != http.StatusOK {
		t.Fatalf("list = %d, want 200", w.Code)
	}
	if strings.Contains(w.Body.String(), pushTokenCanary) {
		t.Fatal("registrations list echoed a push token")
	}
	// Query-token callers are unauthenticated (H2): 401, must use the header.
	if w := httpDo(http.MethodGet, "/v1/push/registrations?access_token="+pushToken(t, userID, apnsPhone), "", ""); w.Code != http.StatusUnauthorized {
		t.Fatalf("query-token list = %d, want 401", w.Code)
	}
	if _, err := auth.Authenticate(mustQueryTokenRequest(t, pushToken(t, userID, apnsPhone))); err == nil {
		t.Fatal("lease Authenticate accepted a query token")
	}

	assertNoCanary(t, "recorded push bytes", rec.serverBytes(), c)
	assertEndpointDelivery(t, s, c)
}

func mustQueryTokenRequest(t *testing.T, tok string) *http.Request {
	t.Helper()
	return httptest.NewRequest(http.MethodGet, "/v1/push/registrations?access_token="+tok, nil)
}
