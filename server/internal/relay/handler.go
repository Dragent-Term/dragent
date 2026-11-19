package relay

import (
	"context"
	"encoding/json"
	"net/http"
	"unicode/utf8"

	"github.com/go-chi/chi/v5"
)

// Handler serves the sealed relay catalog (§6.5). Mount RegisterRoutes under
// RequireAuth: both endpoints additionally reject requests whose context
// carries no user id, so a mis-mounted router still exposes nothing.
//
// The relay is a blind router: PUT stores the desktop's sealed blob
// opaquely (routing validated only), GET returns it verbatim. Plaintext
// catalog shapes (projects / workspaces / tabs / machine) are gone: bodies
// carrying them answer 410 without being stored.
type Handler struct {
	store  Store
	sealed SealedStore
}

// NewHandler returns a Handler backed by store. A nil store uses an
// in-memory one (tests / single-instance dev).
func NewHandler(store Store) *Handler {
	if store == nil {
		store = NewMemoryStore()
	}
	return &Handler{store: store, sealed: NewMemorySealedStore()}
}

// Sealed returns the opaque blob store (tests and the socket catalog path).
func (h *Handler) Sealed() SealedStore { return h.sealed }

// SetSealedStore installs the opaque store (production wiring).
func (h *Handler) SetSealedStore(s SealedStore) {
	if s != nil {
		h.sealed = s
	}
}

// RegisterRoutes mounts the catalog endpoints on an authenticated router.
// No route here is reachable anonymously: each handler re-checks the user.
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Put("/v1/relay/catalog", h.PublishCatalog)
	r.Get("/v1/relay/catalog", h.ReadCatalog)
}

// writeGone answers a removed plaintext route. §10.5 requires 410 before the
// body is read; callers must invoke this before touching r.Body.
func writeGone(w http.ResponseWriter, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusGone)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}

// PublishCatalog stores the calling account's sealed catalog blob for one
// of its devices. Same-account only: the owner key embeds the caller's user
// id, so a host can never publish into another account's catalog.
//
// E2EE cut-over (§6.5, §10): the body must be the sealed blob
// {v, desktop, version, recipients, nonce, body}. Plaintext shapes
// (projects / workspaces / tabs / machine) are removed: they answer 410
// and are never stored. Unknown devices read as 404, never 403.
func (h *Handler) PublishCatalog(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	if userID == "" {
		writeErr(w, http.StatusUnauthorized, "login required for relay catalog")
		return
	}
	// Bind retries to the account that collected the snapshot, before reading
	// the body. Older publishers without an assertion remain compatible.
	if expected := r.Header.Get("X-Dragent-Publisher-User"); expected != "" && expected != userID {
		writeErr(w, http.StatusForbidden, "publisher account changed")
		return
	}
	raw, err := readLimited(r, 4*1024*1024)
	if err != nil {
		writeErr(w, http.StatusRequestEntityTooLarge, "catalog body is too large")
		return
	}
	if !utf8.Valid(raw) {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	// Plaintext catalog shapes are gone (§10.5): refuse before storing.
	if isPlaintextCatalog(raw) {
		writeGone(w, "relay catalog is end-to-end encrypted: update Dragent on this desktop")
		return
	}
	// device_id plus the sealed blob's exact fields; anything else is 400.
	var envelope map[string]json.RawMessage
	if err := json.Unmarshal(raw, &envelope); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	for k := range envelope {
		switch k {
		case "device_id", "v", "desktop", "version", "recipients", "nonce", "body":
		default:
			writeErr(w, http.StatusBadRequest, "invalid request body")
			return
		}
	}
	var device struct {
		DeviceID string `json:"device_id"`
	}
	if err := json.Unmarshal(raw, &device); err != nil || device.DeviceID == "" {
		writeErr(w, http.StatusBadRequest, "invalid device")
		return
	}
	var blob SealedCatalog
	if err := json.Unmarshal(raw, &blob); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	blob = blob.Normalized()
	if err := blob.Validate(); err != nil {
		writeErr(w, http.StatusBadRequest, err.Error())
		return
	}
	owner, ok := ownerKey(userID, device.DeviceID)
	if !ok {
		writeErr(w, http.StatusBadRequest, "invalid device")
		return
	}
	sealed := h.sealed
	if sealed == nil {
		writeErr(w, http.StatusInternalServerError, "could not store catalog")
		return
	}
	if err := sealed.PutSealed(r.Context(), owner, blob); err != nil {
		if err.Error() == "blob desktop does not match device" {
			writeErr(w, http.StatusBadRequest, "invalid device")
			return
		}
		writeErr(w, http.StatusInternalServerError, "could not store catalog")
		return
	}
	// Admission compatibility: the desktop host socket requires a known
	// catalog owner before connecting (E2a host_bridge.go). Record a
	// content-free routing stub in the legacy store so admission keeps
	// working without ever storing content.
	_ = h.store.Put(context.Background(), owner, Catalog{}.Normalized())
	writeJSON(w, map[string]bool{"stored": true})
}

// isPlaintextCatalog reports whether a publish body carries any removed
// plaintext catalog shape. Checked before parsing so removed fields are
// never stored.
func isPlaintextCatalog(raw []byte) bool {
	var keys map[string]json.RawMessage
	if json.Unmarshal(raw, &keys) != nil {
		return false
	}
	for _, k := range []string{"projects", "workspaces", "tabs", "machine"} {
		if _, present := keys[k]; present {
			return true
		}
	}
	return false
}

// ReadCatalog returns the calling account's sealed blob for one of its
// devices, verbatim and opaque. Same-account controller authorization by
// construction: the owner key embeds the caller's user id.
// Unknown devices — including another account's — read as 404, never 403,
// so the endpoint is not an oracle for other accounts' machines.
func (h *Handler) ReadCatalog(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	if userID == "" {
		writeErr(w, http.StatusUnauthorized, "login required for relay catalog")
		return
	}
	owner, ok := ownerKey(userID, r.URL.Query().Get("device_id"))
	if !ok {
		writeErr(w, http.StatusBadRequest, "invalid device")
		return
	}
	sealed := h.sealed
	if sealed == nil {
		writeErr(w, http.StatusInternalServerError, "could not load catalog")
		return
	}
	blob, ok, err := sealed.GetSealed(r.Context(), owner)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not load catalog")
		return
	}
	if !ok {
		writeErr(w, http.StatusNotFound, "catalog not available")
		return
	}
	writeJSON(w, blob)
}

func writeErr(w http.ResponseWriter, status int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}
