package push

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/companionpairing"
)

// stubCompanionStore implements the resolver's narrow store slice with
// scripted answers (e.g. a pre-cut-over e2e:false device, which live stores
// never mint anymore).
type stubCompanionStore struct {
	valid bool
	known bool
	e2e   bool
	err   error
}

func (s *stubCompanionStore) IsDeviceValid(context.Context, string, string) (bool, error) {
	return s.valid, s.err
}

func (s *stubCompanionStore) DeviceE2E(context.Context, string, string) (bool, bool, error) {
	return s.known, s.e2e, s.err
}

// pairPhone redeems one E2E companion phone for userID in a memory pairing
// store and returns its device UUID.
func pairPhone(t *testing.T, store *companionpairing.MemoryStore, userID uuid.UUID) uuid.UUID {
	t.Helper()
	ctx := context.Background()
	_, code, err := store.CreatePairing(ctx, userID.String(), "")
	if err != nil {
		t.Fatalf("CreatePairing: %v", err)
	}
	device, _, _, _, err := store.RedeemPairing(ctx, code, "ios")
	if err != nil {
		t.Fatalf("RedeemPairing: %v", err)
	}
	deviceID, err := uuid.Parse(device.ID)
	if err != nil {
		t.Fatalf("device id is not a UUID: %v", err)
	}
	return deviceID
}

func TestCompanionResolverAcceptsOnlyLiveE2EPhones(t *testing.T) {
	ctx := context.Background()
	userID := uuid.New()
	store := companionpairing.NewMemoryStore()
	deviceID := pairPhone(t, store, userID)
	resolver := NewCompanionDeviceResolver(store)

	if ok, err := resolver.Exists(ctx, userID, deviceID); err != nil || !ok {
		t.Fatalf("paired E2E phone Exists = %v, %v; want true, nil", ok, err)
	}

	// Unknown device: false, no error (the handler answers 404).
	if ok, err := resolver.Exists(ctx, userID, uuid.New()); err != nil || ok {
		t.Fatalf("unknown device Exists = %v, %v; want false, nil", ok, err)
	}
	// Foreign device: a phone paired under another user is not this user's.
	other := uuid.New()
	foreignDevice := pairPhone(t, store, other)
	if ok, err := resolver.Exists(ctx, userID, foreignDevice); err != nil || ok {
		t.Fatalf("foreign device Exists = %v, %v; want false, nil", ok, err)
	}

	// Revoked: gone from the pairing store reads the same as unknown.
	if err := store.RevokeDevice(ctx, userID.String(), deviceID.String()); err != nil {
		t.Fatalf("RevokeDevice: %v", err)
	}
	if ok, err := resolver.Exists(ctx, userID, deviceID); err != nil || ok {
		t.Fatalf("revoked device Exists = %v, %v; want false, nil", ok, err)
	}

	// Storage errors propagate (the handler answers 500, never 404).
	broken := &CompanionDeviceResolver{Store: &stubCompanionStore{err: errors.New("redis down")}}
	if _, err := broken.Exists(ctx, userID, deviceID); err == nil {
		t.Fatal("storage error swallowed")
	}

	// Unwired resolver fails closed.
	if ok, err := (&CompanionDeviceResolver{}).Exists(ctx, userID, deviceID); err != nil || ok {
		t.Fatalf("nil-store Exists = %v, %v; want false, nil", ok, err)
	}
}

func TestCompanionResolverRefusesPreCutoverDevices(t *testing.T) {
	ctx := context.Background()
	userID, deviceID := uuid.New(), uuid.New()

	// A known device without the E2E flag (redeemed before the cut-over,
	// kept from an old store snapshot) must pair again: no push.
	stale := &CompanionDeviceResolver{Store: &stubCompanionStore{valid: true, known: true, e2e: false}}
	if ok, err := stale.Exists(ctx, userID, deviceID); err != nil || ok {
		t.Fatalf("e2e:false device Exists = %v, %v; want false, nil", ok, err)
	}

	// A valid lease that the cut-over lookup no longer knows reads the same.
	gone := &CompanionDeviceResolver{Store: &stubCompanionStore{valid: true, known: false}}
	if ok, err := gone.Exists(ctx, userID, deviceID); err != nil || ok {
		t.Fatalf("unknown-to-cutover device Exists = %v, %v; want false, nil", ok, err)
	}
}

func TestCompanionResolverRefusesExpiredPhones(t *testing.T) {
	now := time.Now()
	clock := now
	store := companionpairing.NewMemoryStore(func() time.Time { return clock })
	userID := uuid.New()
	deviceID := pairPhone(t, store, userID)
	resolver := NewCompanionDeviceResolver(store)

	if ok, err := resolver.Exists(context.Background(), userID, deviceID); err != nil || !ok {
		t.Fatalf("fresh phone Exists = %v, %v; want true, nil", ok, err)
	}
	// Past the 30-day device lifetime the phone must pair again.
	clock = now.Add(companionpairing.DeviceLifetime + time.Hour)
	if ok, err := resolver.Exists(context.Background(), userID, deviceID); err != nil || ok {
		t.Fatalf("expired phone Exists = %v, %v; want false, nil", ok, err)
	}
}

// companionPushRig wires the real push handler to a memory pairing store:
// register/rotate/unregister resolve companion phones through it.
type companionPushRig struct {
	pairing *companionpairing.MemoryStore
	push    *MemoryStore
	router  chi.Router
}

func newCompanionPushRig() *companionPushRig {
	pairing := companionpairing.NewMemoryStore()
	pushStore := NewMemoryStore()
	h := NewHandler(pushStore, NewCompanionDeviceResolver(pairing), NewJWTAuthenticator(testJWTSecret))
	r := chi.NewRouter()
	h.RegisterRoutes(r)
	return &companionPushRig{pairing: pairing, push: pushStore, router: r}
}

func (rig *companionPushRig) do(t *testing.T, method, path, token, body string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, path, bytes.NewReader([]byte(body)))
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	rec := httptest.NewRecorder()
	rig.router.ServeHTTP(rec, req)
	return rec
}

func TestRegisterThroughCompanionPairing(t *testing.T) {
	rig := newCompanionPushRig()
	userID := uuid.New()
	deviceID := pairPhone(t, rig.pairing, userID)
	token := mintToken(t, testJWTSecret, userID, deviceID, time.Hour)
	body := `{"device_id":"` + deviceID.String() + `","platform":"fcm","push_token":"companion-token"}`

	if rec := rig.do(t, "POST", "/v1/push/register", token, body); rec.Code != http.StatusOK {
		t.Fatalf("paired-phone register = %d (%s), want 200", rec.Code, rec.Body.String())
	}

	// Unknown, foreign and revoked phones share one 404 answer (no oracle).
	// The device_id is inferred from the lease (mobile never decodes its
	// JWT), so the body carries no id.
	other := uuid.New()
	foreignDevice := pairPhone(t, rig.pairing, other)
	cases := map[string]string{
		"unknown": mintToken(t, testJWTSecret, userID, uuid.New(), time.Hour),
		"foreign": mintToken(t, testJWTSecret, userID, foreignDevice, time.Hour),
	}
	var reference string
	for name, token := range cases {
		rec := rig.do(t, "POST", "/v1/push/register", token, `{"platform":"fcm","push_token":"x"}`)
		if rec.Code != http.StatusNotFound {
			t.Fatalf("%s register = %d (%s), want 404", name, rec.Code, rec.Body.String())
		}
		if reference == "" {
			reference = rec.Body.String()
		} else if rec.Body.String() != reference {
			t.Fatalf("%s body %q differs from %q: oracle", name, rec.Body.String(), reference)
		}
	}

	// Revoked: the phone registered fine above; after revocation it reads
	// exactly like unknown.
	if err := rig.pairing.RevokeDevice(context.Background(), userID.String(), deviceID.String()); err != nil {
		t.Fatalf("RevokeDevice: %v", err)
	}
	rec := rig.do(t, "POST", "/v1/push/register", token, body)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("revoked register = %d (%s), want 404", rec.Code, rec.Body.String())
	}
	if rec.Body.String() != reference {
		t.Fatalf("revoked body %q differs from %q: oracle", rec.Body.String(), reference)
	}
	var decoded map[string]string
	if err := json.Unmarshal([]byte(reference), &decoded); err != nil || decoded["error"] != "unknown device" {
		t.Fatalf("404 body = %q, want the unknown-device answer", reference)
	}
}

// TestRevokeRemovesPushRegistration drives the real pairing + push stores
// through both HTTP handlers: after DELETE /v1/companion/devices/{id} the
// push lease is gone and the phone cannot re-register.
func TestRevokeRemovesPushRegistration(t *testing.T) {
	pairing := companionpairing.NewMemoryStore()
	pushStore := NewMemoryStore()
	pushHandler := NewHandler(pushStore, NewCompanionDeviceResolver(pairing), NewJWTAuthenticator(testJWTSecret))
	pushRouter := chi.NewRouter()
	pushHandler.RegisterRoutes(pushRouter)

	cleanup := func(ctx context.Context, userID, deviceID string) {
		userUUID, err := uuid.Parse(userID)
		deviceUUID, err2 := uuid.Parse(deviceID)
		if err != nil || err2 != nil {
			t.Errorf("cleanup got non-UUID ids %q %q", userID, deviceID)
			return
		}
		if err := pushStore.Delete(ctx, userUUID, deviceUUID); err != nil && !errors.Is(err, ErrNotFound) {
			t.Errorf("cleanup Delete: %v", err)
		}
	}
	pairingHandler := companionpairing.NewHandler(pairing, nil, nil, "",
		companionpairing.WithPushCleanup(cleanup))
	userID := uuid.New()
	pairingRouter := chi.NewRouter()
	pairingRouter.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			req = req.WithContext(context.WithValue(req.Context(), "user_id", userID.String()))
			next.ServeHTTP(w, req)
		})
	})
	pairingHandler.RegisterRoutes(pairingRouter)

	deviceID := pairPhone(t, pairing, userID)
	token := mintToken(t, testJWTSecret, userID, deviceID, time.Hour)

	pushDo := func(method, path, body string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(method, path, bytes.NewReader([]byte(body)))
		req.Header.Set("Authorization", "Bearer "+token)
		rec := httptest.NewRecorder()
		pushRouter.ServeHTTP(rec, req)
		return rec
	}
	if rec := pushDo("POST", "/v1/push/register",
		`{"device_id":"`+deviceID.String()+`","platform":"apns","push_token":"revoke-me"}`); rec.Code != http.StatusOK {
		t.Fatalf("register = %d (%s), want 200", rec.Code, rec.Body.String())
	}

	req := httptest.NewRequest("DELETE", "/v1/companion/devices/"+deviceID.String(), nil)
	req = req.WithContext(context.WithValue(req.Context(), "user_id", userID.String()))
	// Route through chi so the {id} param is populated.
	rec := httptest.NewRecorder()
	pairingRouter.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("revoke = %d (%s), want 200", rec.Code, rec.Body.String())
	}

	if _, found, _ := pushStore.Get(context.Background(), deviceID); found {
		t.Fatal("push registration survived companion revoke")
	}
	if rec := pushDo("POST", "/v1/push/register",
		`{"device_id":"`+deviceID.String()+`","platform":"apns","push_token":"again"}`); rec.Code != http.StatusNotFound {
		t.Fatalf("re-register after revoke = %d, want 404", rec.Code)
	}
}
