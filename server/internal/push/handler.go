package push

import (
	"encoding/json"
	"errors"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

// Handler serves the device registration API. Every route requires an
// authenticated token lease (see lease.go); the device claim in the JWT must
// equal the device being registered.
type Handler struct {
	regs    Store
	devices DeviceResolver
	auth    Authenticator
}

// NewHandler wires the registration API to its store, device resolver, and
// JWT authenticator.
func NewHandler(regs Store, devices DeviceResolver, auth Authenticator) *Handler {
	return &Handler{regs: regs, devices: devices, auth: auth}
}

// RegisterRoutes mounts the push API. Mount under RequireAuth (which
// guarantees a user) — the handler additionally verifies the JWT itself to
// recover the device claim RequireAuth does not propagate.
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/v1/push/register", h.register)
	r.Post("/v1/push/rotate", h.rotate)
	r.Post("/v1/push/unregister", h.unregister)
	r.Get("/v1/push/registrations", h.list)
}

// resolveDeviceID infers the target device from the verified lease when the
// caller omits it (mobile never decodes its JWT). An explicit device_id that
// differs from the lease still fails closed; ownership is always DB-checked.
func resolveDeviceID(bodyDeviceID string, lease Lease) (uuid.UUID, error) {
	trimmed := strings.TrimSpace(bodyDeviceID)
	if trimmed == "" {
		return lease.DeviceID, nil
	}
	return uuid.Parse(trimmed)
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

// leaseOr401 verifies the caller's token lease.
func (h *Handler) leaseOr401(w http.ResponseWriter, r *http.Request) (Lease, bool) {
	lease, err := h.auth.Authenticate(r)
	if err != nil {
		writeErr(w, http.StatusUnauthorized, "authentication required")
		return Lease{}, false
	}
	return lease, true
}

// ownedOrError enforces the foreign-device rule (403) and the known-device
// rule (404). deviceID has already been parsed.
func (h *Handler) ownedOrError(w http.ResponseWriter, r *http.Request, lease Lease, deviceID uuid.UUID) bool {
	if lease.DeviceID != deviceID {
		writeErr(w, http.StatusForbidden, "device mismatch")
		return false
	}
	ok, err := h.devices.Exists(r.Context(), lease.UserID, deviceID)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not verify device")
		return false
	}
	if !ok {
		writeErr(w, http.StatusNotFound, "unknown device")
		return false
	}
	return true
}

type registerRequest struct {
	DeviceID   string `json:"device_id"`
	Platform   string `json:"platform"`
	PushToken  string `json:"push_token"`
	Foreground *bool  `json:"foreground,omitempty"`
}

type registrationResponse struct {
	Registered bool   `json:"registered"`
	DeviceID   string `json:"device_id"`
	Platform   string `json:"platform"`
	ExpiresAt  string `json:"expires_at"`
}

// register creates or renews a device's push lease. Registration is renewable:
// re-registering (e.g. on every app start) extends the expiry, so active
// devices never lapse.
func (h *Handler) register(w http.ResponseWriter, r *http.Request) {
	lease, ok := h.leaseOr401(w, r)
	if !ok {
		return
	}
	var body registerRequest
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	deviceID, err := resolveDeviceID(body.DeviceID, lease)
	if err != nil || deviceID == uuid.Nil {
		writeErr(w, http.StatusBadRequest, "device_id must be a UUID")
		return
	}
	if !h.ownedOrError(w, r, lease, deviceID) {
		return
	}
	reg := Registration{
		DeviceID:   deviceID,
		UserID:     lease.UserID,
		Platform:   normalizePlatform(body.Platform),
		PushToken:  strings.TrimSpace(body.PushToken),
		Foreground: body.Foreground != nil && *body.Foreground,
	}
	if err := ValidateRegistration(reg); err != nil {
		writeErr(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := h.regs.Upsert(r.Context(), reg); err != nil {
		writeErr(w, http.StatusInternalServerError, "could not store registration")
		return
	}
	stored, found, err := h.regs.Get(r.Context(), deviceID)
	if err != nil || !found {
		writeErr(w, http.StatusInternalServerError, "could not load registration")
		return
	}
	writeJSON(w, registrationResponse{
		Registered: true,
		DeviceID:   stored.DeviceID.String(),
		Platform:   stored.Platform,
		ExpiresAt:  stored.ExpiresAt.UTC().Format("2006-01-02T15:04:05Z07:00"),
	})
}

type rotateRequest struct {
	DeviceID   string `json:"device_id"`
	OldToken   string `json:"old_token"`
	NewToken   string `json:"new_token"`
	Foreground *bool  `json:"foreground,omitempty"`
}

// rotate swaps the push token with foreground compare-and-swap semantics:
// the swap only lands when old_token equals the stored token, so a stale
// client cannot clobber a newer rotation. It also renews the lease.
func (h *Handler) rotate(w http.ResponseWriter, r *http.Request) {
	lease, ok := h.leaseOr401(w, r)
	if !ok {
		return
	}
	var body rotateRequest
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	deviceID, err := resolveDeviceID(body.DeviceID, lease)
	if err != nil || deviceID == uuid.Nil {
		writeErr(w, http.StatusBadRequest, "device_id must be a UUID")
		return
	}
	if !h.ownedOrError(w, r, lease, deviceID) {
		return
	}
	foreground := body.Foreground != nil && *body.Foreground
	err = h.regs.Rotate(r.Context(), lease.UserID, deviceID, body.OldToken, strings.TrimSpace(body.NewToken), foreground)
	switch {
	case err == nil:
		writeJSON(w, map[string]bool{"rotated": true})
	case errors.Is(err, ErrNotFound):
		writeErr(w, http.StatusNotFound, "no registration for this device")
	case errors.Is(err, ErrTokenMismatch):
		writeErr(w, http.StatusConflict, "stale token: re-register this device")
	default:
		writeErr(w, http.StatusBadRequest, err.Error())
	}
}

type unregisterRequest struct {
	DeviceID string `json:"device_id"`
}

// unregister removes the push lease durably. The devices row itself is left
// alone (auth owns it); only push delivery stops.
func (h *Handler) unregister(w http.ResponseWriter, r *http.Request) {
	lease, ok := h.leaseOr401(w, r)
	if !ok {
		return
	}
	var body unregisterRequest
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	deviceID, err := resolveDeviceID(body.DeviceID, lease)
	if err != nil || deviceID == uuid.Nil {
		writeErr(w, http.StatusBadRequest, "device_id must be a UUID")
		return
	}
	if lease.DeviceID != deviceID {
		writeErr(w, http.StatusForbidden, "device mismatch")
		return
	}
	if err := h.regs.Delete(r.Context(), lease.UserID, deviceID); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeErr(w, http.StatusNotFound, "no registration for this device")
			return
		}
		writeErr(w, http.StatusInternalServerError, "could not remove registration")
		return
	}
	writeJSON(w, map[string]bool{"unregistered": true})
}

// list returns the caller's live registrations (user-scoped, so a settings
// screen can show every device that would ring).
func (h *Handler) list(w http.ResponseWriter, r *http.Request) {
	lease, ok := h.leaseOr401(w, r)
	if !ok {
		return
	}
	regs, err := h.regs.ListForUser(r.Context(), lease.UserID)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not list registrations")
		return
	}
	type item struct {
		DeviceID   string `json:"device_id"`
		Platform   string `json:"platform"`
		Foreground bool   `json:"foreground"`
		ExpiresAt  string `json:"expires_at"`
	}
	out := make([]item, 0, len(regs))
	for _, reg := range regs {
		if reg.DeviceID == uuid.Nil {
			continue
		}
		out = append(out, item{
			DeviceID:   reg.DeviceID.String(),
			Platform:   reg.Platform,
			Foreground: reg.Foreground,
			ExpiresAt:  reg.ExpiresAt.UTC().Format("2006-01-02T15:04:05Z07:00"),
		})
	}
	writeJSON(w, map[string]any{"registrations": out})
}
