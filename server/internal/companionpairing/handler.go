package companionpairing

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	identityModel "server/internal/identity/model"
	identityTypes "server/internal/identity/types"
	"server/internal/platform/ratelimit"
)

// AccessScope marks access tokens minted for paired phones. It is a claim
// inside the same JWT shape the relay already accepts from portal logins
// (user_id, device_id, expiry), so relay admission and the 24-hour
// companion session path need no changes: the relay only additionally
// checks the device is still paired.
const AccessScope = "companion"

// UserLookup resolves the pairing owner for the redeem response.
type UserLookup interface {
	GetByID(ctx context.Context, id uuid.UUID) (*identityModel.User, error)
}

// TokenIssuer mints and verifies the relay-accepted access tokens.
// *identityModel.TokenService satisfies it.
type TokenIssuer interface {
	GenerateScopedAccessToken(userID, deviceID uuid.UUID, scope string) (string, error)
	VerifyAccessToken(tokenString string) (*identityTypes.TokenClaims, error)
}

// OrgResolver binds a pairing to the caller's organization. It returns ""
// when the caller has no resolvable org; the user binding still isolates.
type OrgResolver func(ctx context.Context, userID string) string

// SocketCloser drops live relay sockets of a revoked device. *relay.RPCHandler
// satisfies it. Calls are nil-safe so tests can omit wiring.
type SocketCloser interface {
	CloseDeviceSockets(userID, deviceID string)
}

// PushCleanup removes a revoked device's push registration. It runs
// best-effort after RevokeDevice and after token-reuse revocation (FIX1,
// §8): the push store no longer cascades off the pairing store, so without
// this a revoked phone would keep receiving pushes. The implementation
// lives with the integrator that owns both stores (server/cmd/api/main.go),
// which also logs failures without ids per §2.3; this package never imports
// push, so there is no import cycle. Calls are nil-safe.
type PushCleanup func(ctx context.Context, userID, deviceID string)

// Handler serves the five companion-pairing endpoints. Pairing creation,
// listing and revocation mount under RequireAuth (desktop access token);
// redeem and token rotation are unauthenticated (the code / device token
// in the body is the credential) and redeem is rate-limited per IP + code.
type Handler struct {
	store       Store
	users       UserLookup
	tokens      TokenIssuer
	gatewayURL  string
	orgs        OrgResolver
	sockets     SocketCloser
	pushCleanup PushCleanup
	ipLimiter   RateLimiter
	codeLimiter RateLimiter
	now         func() time.Time
}

// Option customizes a Handler.
type Option func(*Handler)

// WithOrgResolver sets the user-to-org binding used at pairing creation.
func WithOrgResolver(resolver OrgResolver) Option {
	return func(h *Handler) { h.orgs = resolver }
}

// WithSocketCloser wires relay socket closure on device revocation.
func WithSocketCloser(closer SocketCloser) Option {
	return func(h *Handler) { h.sockets = closer }
}

// WithPushCleanup wires push-registration removal on device revocation
// (explicit revoke and token-reuse compromise revocation alike).
func WithPushCleanup(cleanup PushCleanup) Option {
	return func(h *Handler) { h.pushCleanup = cleanup }
}

// WithRateLimiters overrides the default redeem limits (tests).
func WithRateLimiters(ip, code RateLimiter) Option {
	return func(h *Handler) { h.ipLimiter, h.codeLimiter = ip, code }
}

// WithClock injects time (tests).
func WithClock(now func() time.Time) Option {
	return func(h *Handler) { h.now = now }
}

// NewHandler wires the pairing endpoints. gatewayURL is the API origin the
// phone redeems and connects against. When empty, use the API request origin.
// Never pass the web frontend origin: it does not serve these endpoints.
func NewHandler(store Store, users UserLookup, tokens TokenIssuer, gatewayURL string, opts ...Option) *Handler {
	h := &Handler{
		store:       store,
		users:       users,
		tokens:      tokens,
		gatewayURL:  strings.TrimRight(strings.TrimSpace(gatewayURL), "/"),
		ipLimiter:   NewMemoryRateLimiter(defaultRedeemIPLimit, defaultRedeemWindow),
		codeLimiter: NewMemoryRateLimiter(defaultRedeemCodeLimit, defaultRedeemWindow),
		now:         time.Now,
	}
	for _, opt := range opts {
		opt(h)
	}
	return h
}

// gatewayOrigin preserves the public API host through the TLS-terminating
// reverse proxy. The proxy must overwrite X-Forwarded-Proto (as Caddy does).
// Do not use Origin or X-Forwarded-Host: those can name the web frontend.
func (h *Handler) gatewayOrigin(r *http.Request) string {
	if h.gatewayURL != "" {
		return h.gatewayURL
	}
	scheme := "http"
	if r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https" {
		scheme = "https"
	}
	return scheme + "://" + r.Host
}

// RegisterRoutes mounts desktop-authenticated routes (under RequireAuth).
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/v1/companion/pairings", h.createPairing)
	r.Get("/v1/companion/devices", h.listDevices)
	r.Delete("/v1/companion/devices/{id}", h.revokeDevice)
}

// RegisterPublicRoutes mounts unauthenticated routes (rate-limited redeem,
// credential-in-body token rotation).
func (h *Handler) RegisterPublicRoutes(r chi.Router) {
	r.Post("/v1/companion/pairings/redeem", h.redeem)
	r.Post("/v1/companion/devices/token", h.rotateToken)
}

// RequireValidDevice gates relay RPC traffic for paired phones: portal
// tokens pass through untouched, companion-scoped tokens must belong to a
// live device. Mount after RequireAuth on the relay RPC group so revoked
// phones lose HTTP RPC the moment they are revoked.
func (h *Handler) RequireValidDevice(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		uid, ok := userIDFromContext(r)
		if !ok || uid == "" {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		if h.tokens == nil || h.store == nil {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		token := bearerToken(r)
		if token == "" {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		claims, err := h.tokens.VerifyAccessToken(token)
		if err != nil {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		if claims.Scope != AccessScope {
			next.ServeHTTP(w, r)
			return
		}
		valid, err := h.store.IsDeviceValid(r.Context(), claims.UserID.String(), claims.DeviceID.String())
		if err != nil || !valid {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		next.ServeHTTP(w, r)
	})
}

// RejectRevokedDevices runs on every request: a paired phone's token stops
// working everywhere the moment its phone is revoked, not only on the relay.
// Requests without a token, and tokens that aren't a phone's, pass through
// for the routes' own checks.
func (h *Handler) RejectRevokedDevices(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token := bearerToken(r)
		if token == "" || h.tokens == nil || h.store == nil {
			next.ServeHTTP(w, r)
			return
		}
		claims, err := h.tokens.VerifyAccessToken(token)
		if err != nil || claims.Scope != AccessScope {
			next.ServeHTTP(w, r)
			return
		}
		valid, err := h.store.IsDeviceValid(r.Context(), claims.UserID.String(), claims.DeviceID.String())
		if err != nil || !valid {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		next.ServeHTTP(w, r)
	})
}

type createPairingResponse struct {
	PairingID  string `json:"pairing_id"`
	Code       string `json:"code"`
	ExpiresAt  string `json:"expires_at"`
	GatewayURL string `json:"gateway_url"`
}

func (h *Handler) createPairing(w http.ResponseWriter, r *http.Request) {
	uid, ok := userIDFromContext(r)
	if !ok || uid == "" {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	var orgID string
	if h.orgs != nil {
		orgID = h.orgs(r.Context(), uid)
	}
	pairing, code, err := h.store.CreatePairing(r.Context(), uid, orgID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "unavailable")
		return
	}
	writeJSON(w, http.StatusCreated, createPairingResponse{
		PairingID:  pairing.ID,
		Code:       code,
		ExpiresAt:  pairing.ExpiresAt.UTC().Format(time.RFC3339),
		GatewayURL: h.gatewayOrigin(r),
	})
}

type redeemRequest struct {
	Code     string `json:"code"`
	Platform string `json:"platform"`
}

type redeemUser struct {
	ID          string `json:"id"`
	DisplayName string `json:"display_name"`
}

// decodeRedeem decodes the already-read redeem body strictly: unknown fields
// (including a smuggled device_name) are rejected.
func decodeRedeem(raw map[string]json.RawMessage, out *redeemRequest) error {
	for k := range raw {
		if k != "code" && k != "platform" {
			return errors.New("unknown field")
		}
	}
	data, err := json.Marshal(raw)
	if err != nil {
		return err
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	return dec.Decode(out)
}

type redeemResponse struct {
	DeviceID    string     `json:"device_id"`
	DeviceToken string     `json:"device_token"`
	User        redeemUser `json:"user"`
	GatewayURL  string     `json:"gateway_url"`
	ExpiresAt   string     `json:"expires_at"`
}

func (h *Handler) redeem(w http.ResponseWriter, r *http.Request) {
	var raw map[string]json.RawMessage
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 8*1024)).Decode(&raw); err != nil {
		writeError(w, http.StatusBadRequest, "invalid")
		return
	}
	// E2EE cut-over (§6.2): the device name stays in the encrypted HS1
	// payload and never reaches the server. A redeem carrying one is a
	// pre-cut-over client: refuse it so no name is ever stored.
	if v, present := raw["device_name"]; present && len(bytes.TrimSpace(v)) > 0 && string(bytes.TrimSpace(v)) != "null" && string(bytes.TrimSpace(v)) != `""` {
		writeError(w, http.StatusBadRequest, "invalid")
		return
	}
	var body redeemRequest
	if err := decodeRedeem(raw, &body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid")
		return
	}
	code, ok := normalizeCode(body.Code)
	if !ok {
		writeError(w, http.StatusBadRequest, "invalid")
		return
	}
	if allowed, err := h.allowRedeem(r.Context(), r, code); err != nil || !allowed {
		writeError(w, http.StatusTooManyRequests, "rate_limited")
		return
	}
	device, userID, _, token, err := h.store.RedeemPairing(r.Context(), code, body.Platform)
	switch {
	case err == nil:
	case errors.Is(err, ErrGone):
		writeError(w, http.StatusGone, "gone")
		return
	case errors.Is(err, ErrInvalid):
		writeError(w, http.StatusBadRequest, "invalid")
		return
	default:
		writeError(w, http.StatusInternalServerError, "unavailable")
		return
	}
	ownerID, parseErr := uuid.Parse(userID)
	if parseErr != nil {
		writeError(w, http.StatusGone, "gone")
		return
	}
	name := ""
	if h.users != nil {
		if u, lookupErr := h.users.GetByID(r.Context(), ownerID); lookupErr == nil && u != nil {
			name = displayNameOf(u)
		}
	}
	if name == "" {
		// Owner vanished after redeem: the phone must pair again.
		writeError(w, http.StatusGone, "gone")
		return
	}
	writeJSON(w, http.StatusOK, redeemResponse{
		DeviceID:    device.ID,
		DeviceToken: token,
		User:        redeemUser{ID: userID, DisplayName: name},
		GatewayURL:  h.gatewayOrigin(r),
		ExpiresAt:   device.ExpiresAt.UTC().Format(time.RFC3339),
	})
}

func displayNameOf(u *identityModel.User) string {
	if u == nil {
		return ""
	}
	if name := strings.TrimSpace(u.DisplayName); name != "" {
		return name
	}
	if name := strings.TrimSpace(strings.TrimSpace(u.FirstName) + " " + strings.TrimSpace(u.LastName)); name != "" {
		return name
	}
	return strings.TrimSpace(u.Email)
}

// allowRedeem enforces both legs: per-IP and per-code fixed windows.
func (h *Handler) allowRedeem(ctx context.Context, r *http.Request, code string) (bool, error) {
	if h.ipLimiter == nil || h.codeLimiter == nil {
		return false, ErrStorage
	}
	ipOK, err := h.ipLimiter.Allow(ctx, "pairing:redeem:ip:"+clientIP(r))
	if err != nil || !ipOK {
		return false, err
	}
	codeOK, err := h.codeLimiter.Allow(ctx, "pairing:redeem:code:"+hashSecret(code))
	if err != nil || !codeOK {
		return false, err
	}
	return true, nil
}

type rotateRequest struct {
	DeviceID    string `json:"device_id"`
	DeviceToken string `json:"device_token"`
}

type rotateResponse struct {
	AccessToken     string `json:"access_token"`
	AccessExpiresAt string `json:"access_expires_at"`
	DeviceToken     string `json:"device_token"`
	ExpiresAt       string `json:"expires_at"`
}

func (h *Handler) rotateToken(w http.ResponseWriter, r *http.Request) {
	var body rotateRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 8*1024)).Decode(&body); err != nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	deviceID := strings.TrimSpace(body.DeviceID)
	if !validDeviceID(deviceID) || strings.TrimSpace(body.DeviceToken) == "" {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	next, device, err := h.store.RotateDeviceToken(r.Context(), deviceID, body.DeviceToken)
	switch err {
	case nil:
	case ErrRevoked:
		h.closeSockets(device.UserID, device.ID)
		h.cleanupPush(r.Context(), device.UserID, device.ID)
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	default:
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	ownerID, err := uuid.Parse(device.UserID)
	if err != nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	deviceUUID, err := uuid.Parse(device.ID)
	if err != nil {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	access, err := h.tokens.GenerateScopedAccessToken(ownerID, deviceUUID, AccessScope)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "invalid")
		return
	}
	accessExpiry := h.now().Add(24 * time.Hour)
	if claims, verifyErr := h.tokens.VerifyAccessToken(access); verifyErr == nil && claims.ExpiresAt != nil {
		accessExpiry = claims.ExpiresAt.Time
	}
	writeJSON(w, http.StatusOK, rotateResponse{
		AccessToken:     access,
		AccessExpiresAt: accessExpiry.UTC().Format(time.RFC3339),
		DeviceToken:     next,
		ExpiresAt:       device.ExpiresAt.UTC().Format(time.RFC3339),
	})
}

type deviceItem struct {
	DeviceID   string `json:"device_id"`
	Platform   string `json:"platform"`
	E2E        bool   `json:"e2e"`
	CreatedAt  string `json:"created_at"`
	LastSeenAt string `json:"last_seen_at"`
}

func (h *Handler) listDevices(w http.ResponseWriter, r *http.Request) {
	uid, ok := userIDFromContext(r)
	if !ok || uid == "" {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	devices, err := h.store.ListDevices(r.Context(), uid)
	if err != nil {
		if errors.Is(err, ErrInvalid) {
			writeError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		writeError(w, http.StatusInternalServerError, "unavailable")
		return
	}
	items := make([]deviceItem, 0, len(devices))
	for _, d := range devices {
		items = append(items, deviceItem{
			DeviceID:   d.ID,
			Platform:   d.Platform,
			E2E:        d.E2E,
			CreatedAt:  d.CreatedAt.UTC().Format(time.RFC3339),
			LastSeenAt: d.LastSeenAt.UTC().Format(time.RFC3339),
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"devices": items})
}

func (h *Handler) revokeDevice(w http.ResponseWriter, r *http.Request) {
	uid, ok := userIDFromContext(r)
	if !ok || uid == "" {
		writeError(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	id := strings.TrimSpace(chi.URLParam(r, "id"))
	if err := h.store.RevokeDevice(r.Context(), uid, id); err != nil {
		// Unknown, malformed and foreign ids share one answer so device
		// existence never leaks across accounts.
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusBadRequest, "invalid")
			return
		}
		if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			writeError(w, http.StatusBadRequest, "invalid")
			return
		}
		writeError(w, http.StatusInternalServerError, "unavailable")
		return
	}
	h.closeSockets(uid, id)
	h.cleanupPush(r.Context(), uid, id)
	writeJSON(w, http.StatusOK, map[string]bool{"revoked": true})
}

func (h *Handler) closeSockets(userID, deviceID string) {
	if h.sockets == nil || userID == "" || deviceID == "" {
		return
	}
	h.sockets.CloseDeviceSockets(userID, deviceID)
}

// cleanupPush removes the revoked device's push registration best-effort.
// Failure logging (without ids, §2.3) belongs to the integrator's
// implementation; here a nil hook simply means nothing to clean.
func (h *Handler) cleanupPush(ctx context.Context, userID, deviceID string) {
	if h.pushCleanup == nil || userID == "" || deviceID == "" {
		return
	}
	h.pushCleanup(ctx, userID, deviceID)
}

// userIDFromContext reads the account id AuthMiddleware injects (and
// RequireAuth enforces in production).
func userIDFromContext(r *http.Request) (string, bool) {
	uid, _ := r.Context().Value("user_id").(string)
	return uid, uid != ""
}

// bearerToken reads the Authorization header only. E2EE cut-over (H2,
// §10): access_token query parameters are refused on the pairing routes —
// the phone and desktop authenticate in-band (relay-auth / relay-host-auth
// carry the token and e2e:1 inside the first socket message), so no URL
// token is needed. Queries carrying one are treated as absent.
func bearerToken(r *http.Request) string {
	if header := r.Header.Get("Authorization"); header != "" {
		parts := strings.SplitN(header, " ", 2)
		if len(parts) == 2 && strings.EqualFold(strings.TrimSpace(parts[0]), "bearer") {
			return strings.TrimSpace(parts[1])
		}
	}
	return ""
}

// clientIP is the trusted client address (see ratelimit.ClientIP): a
// forwarded header the caller sets can't dodge the per-IP redeem limit.
func clientIP(r *http.Request) string {
	return ratelimit.ClientIP(r)
}

func writeError(w http.ResponseWriter, status int, code string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": code})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
