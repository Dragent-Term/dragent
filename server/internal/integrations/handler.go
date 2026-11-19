// Package integrations keeps the desktop app's connections to the services
// work comes from (Linear, Jira, Sentry, Slack, …) on the server, so a key
// someone pasted once serves them on every machine, and a workspace key an
// admin shares serves the whole organization.
//
// The keys never go back to the app. The app builds each request to the
// service itself and hands it here (POST …/request); the server checks it
// goes where that connection's service lives, adds the key, sends it, and
// returns the answer.
//
// Routes (members of the organization in the path only):
//
//	GET    /api/v1/organizations/{orgID}/integrations              shared connections and the caller's own
//	PUT    /api/v1/organizations/{orgID}/integrations              connect (replaces that service's connection)
//	DELETE /api/v1/organizations/{orgID}/integrations/{id}         disconnect
//	POST   /api/v1/organizations/{orgID}/integrations/{id}/request send a request to the service
package integrations

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/organization"
	"server/internal/platform/secretbox"
)

const (
	maxValueChars = 4096
	maxLabelChars = 300
)

// Connection is one saved connection. Times are Unix milliseconds on the
// wire.
type Connection struct {
	ID             uuid.UUID         `json:"id"`
	OrganizationID uuid.UUID         `json:"organization_id"`
	OwnerUserID    uuid.UUID         `json:"owner_user_id"`
	OwnerName      string            `json:"owner_name"`
	Provider       string            `json:"provider"`
	Shared         bool              `json:"shared"`
	Account        *string           `json:"account"`
	Config         map[string]string `json:"config"`
	KeyHint        *string           `json:"key_hint"`
	Status         string            `json:"status"`
	LastUsedAt     *int64            `json:"last_used_at"`
	CreatedAt      int64             `json:"created_at"`
	UpdatedAt      int64             `json:"updated_at"`
	// Whether the caller may replace or remove it (set per request).
	CanManage bool `json:"can_manage"`

	secretsEncrypted string
}

type Repository interface {
	// List returns the organization's shared connections and userID's own.
	List(ctx context.Context, orgID, userID uuid.UUID) ([]Connection, error)
	// Get returns nil when the organization has no connection with this id.
	Get(ctx context.Context, orgID, id uuid.UUID) (*Connection, error)
	// Save puts c in its slot (the organization's shared one for its
	// service, or its owner's personal one), replacing what was there.
	Save(ctx context.Context, c Connection) (Connection, error)
	Delete(ctx context.Context, orgID, id uuid.UUID) error
	// Used records a request through the connection; status is "rejected"
	// once the service refused the key.
	Used(ctx context.Context, id uuid.UUID, status string, at time.Time) error
}

// Roles answers what role a member holds in an organization.
type Roles interface {
	MembershipRole(ctx context.Context, userID, orgID uuid.UUID) (organization.MembershipRole, error)
}

type Handler struct {
	repo  Repository
	roles Roles
	// The passphrase connections are encrypted with; empty when the server
	// isn't set up to keep them.
	sealer secretbox.Sealer
	client *http.Client
	now    func() time.Time
}

// NewHandler: `sealer` encrypts the keys people save; nil means the
// server keeps no connections (the app then uses the computer's keychain).
func NewHandler(repo Repository, roles Roles, sealer secretbox.Sealer) *Handler {
	return &Handler{repo: repo, roles: roles, sealer: sealer, client: serviceClient(), now: time.Now}
}

// RegisterRoutes mounts the routes; `member` must admit only members of
// the organization in the path.
func (h *Handler) RegisterRoutes(r chi.Router, member func(http.Handler) http.Handler) {
	base := "/api/v1/organizations/{orgID}/integrations"
	r.With(member).Get(base, h.list)
	r.With(member).Put(base, h.save)
	r.With(member).Delete(base+"/{connectionID}", h.remove)
	r.With(member).Post(base+"/{connectionID}/request", h.request)
}

func (h *Handler) list(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	if h.sealer == nil {
		writeJSON(w, http.StatusOK, map[string]any{"connections": []Connection{}, "available": false})
		return
	}
	ctx := r.Context()
	items, err := h.repo.List(ctx, orgID, userID)
	if err != nil {
		slog.Error("integrations: list failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load connections")
		return
	}
	admin := h.isAdmin(ctx, userID, orgID)
	for i := range items {
		items[i].CanManage = items[i].OwnerUserID == userID || (items[i].Shared && admin)
	}
	if items == nil {
		items = []Connection{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"connections": items, "available": true, "admin": admin})
}

type saveRequest struct {
	Provider string            `json:"provider"`
	Shared   bool              `json:"shared"`
	Account  *string           `json:"account"`
	Config   map[string]string `json:"config"`
	Secrets  map[string]string `json:"secrets"`
}

func (h *Handler) save(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	if h.sealer == nil {
		writeError(w, http.StatusServiceUnavailable, "this server isn't set up to keep connections")
		return
	}
	var req saveRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	p, known := providers[req.Provider]
	if !known {
		writeError(w, http.StatusBadRequest, "unknown service")
		return
	}
	ctx := r.Context()
	if req.Shared {
		if !p.shareable {
			writeError(w, http.StatusBadRequest, "this service's keys belong to one person, so they can't be shared with the team")
			return
		}
		if !h.isAdmin(ctx, userID, orgID) {
			writeError(w, http.StatusForbidden, "only the organization's owners and admins can share a connection with the team")
			return
		}
	}
	secrets := map[string]string{}
	for _, name := range p.secrets {
		value := strings.TrimSpace(req.Secrets[name])
		if value == "" {
			writeError(w, http.StatusBadRequest, "missing "+name)
			return
		}
		if utf8.RuneCountInString(value) > maxValueChars {
			writeError(w, http.StatusBadRequest, name+" is too long")
			return
		}
		secrets[name] = value
	}
	config := map[string]string{}
	for _, name := range p.config {
		value := strings.TrimSpace(req.Config[name])
		if value == "" {
			continue
		}
		if utf8.RuneCountInString(value) > maxValueChars {
			writeError(w, http.StatusBadRequest, name+" is too long")
			return
		}
		config[name] = value
	}
	for _, name := range p.required {
		if config[name] == "" {
			writeError(w, http.StatusBadRequest, "missing "+name)
			return
		}
	}
	base, err := p.base(config)
	if err != nil || (base.Scheme != "https" && base.Scheme != "http") || base.Host == "" || base.User != nil {
		writeError(w, http.StatusBadRequest, "the server address isn't a web address")
		return
	}
	plain, err := json.Marshal(secrets)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not save the connection")
		return
	}
	encrypted, err := h.sealer.Seal(r.Context(), string(plain))
	if err != nil {
		slog.Error("integrations: encrypt failed", "error", err)
		writeError(w, http.StatusInternalServerError, "could not save the connection")
		return
	}
	var account *string
	if req.Account != nil {
		label := strings.TrimSpace(*req.Account)
		if utf8.RuneCountInString(label) > maxLabelChars {
			label = string([]rune(label)[:maxLabelChars])
		}
		if label != "" {
			account = &label
		}
	}
	hint := keyHint(secrets[p.secrets[len(p.secrets)-1]])
	now := h.now()
	saved, err := h.repo.Save(ctx, Connection{
		ID:               uuid.New(),
		OrganizationID:   orgID,
		OwnerUserID:      userID,
		Provider:         p.key,
		Shared:           req.Shared,
		Account:          account,
		Config:           config,
		KeyHint:          &hint,
		Status:           "ok",
		CreatedAt:        now.UnixMilli(),
		UpdatedAt:        now.UnixMilli(),
		secretsEncrypted: encrypted,
	})
	if err != nil {
		slog.Error("integrations: save failed", "organization_id", orgID, "provider", p.key, "error", err)
		writeError(w, http.StatusInternalServerError, "could not save the connection")
		return
	}
	saved.CanManage = true
	writeJSON(w, http.StatusOK, saved)
}

// keyHint is the end of a key, enough to tell two apart.
func keyHint(secret string) string {
	runes := []rune(secret)
	if len(runes) <= 8 {
		return "…"
	}
	return "…" + string(runes[len(runes)-4:])
}

func (h *Handler) remove(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	ctx := r.Context()
	conn, ok := h.connection(w, r, orgID)
	if !ok {
		return
	}
	if conn.OwnerUserID != userID && !(conn.Shared && h.isAdmin(ctx, userID, orgID)) {
		writeError(w, http.StatusForbidden, "only whoever connected it, or an admin, can disconnect it")
		return
	}
	if err := h.repo.Delete(ctx, orgID, conn.ID); err != nil {
		slog.Error("integrations: delete failed", "connection_id", conn.ID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not disconnect it")
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"status": "deleted"})
}

func (h *Handler) request(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	if h.sealer == nil {
		writeError(w, http.StatusServiceUnavailable, "this server isn't set up to keep connections")
		return
	}
	conn, ok := h.connection(w, r, orgID)
	if !ok {
		return
	}
	if !conn.Shared && conn.OwnerUserID != userID {
		// Someone else's personal connection: as if it weren't there.
		writeError(w, http.StatusNotFound, "no such connection")
		return
	}
	var call Call
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxCallBytes)).Decode(&call); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	plain, err := h.sealer.Open(r.Context(), conn.secretsEncrypted)
	var secrets map[string]string
	if err == nil {
		err = json.Unmarshal([]byte(plain), &secrets)
	}
	if err != nil {
		slog.Error("integrations: decrypt failed", "connection_id", conn.ID, "error", err)
		writeError(w, http.StatusInternalServerError, "the connection's key can't be read; connect it again")
		return
	}
	p := providers[conn.Provider]
	answer, err := send(r.Context(), h.client, p, call, secrets, conn.Config)
	var refused *refusedError
	switch {
	case errors.As(err, &refused):
		writeError(w, http.StatusBadRequest, refused.Error())
		return
	case err != nil:
		writeError(w, http.StatusBadGateway, "couldn't reach the service: "+err.Error())
		return
	}
	status := conn.Status
	if answer.Status == http.StatusUnauthorized {
		status = "rejected"
	} else if answer.Status < 400 {
		status = "ok"
	}
	if err := h.repo.Used(context.WithoutCancel(r.Context()), conn.ID, status, h.now()); err != nil {
		slog.Warn("integrations: recording use failed", "connection_id", conn.ID, "error", err)
	}
	writeJSON(w, http.StatusOK, answer)
}

// connection is the one the path names, or a written error.
func (h *Handler) connection(w http.ResponseWriter, r *http.Request, orgID uuid.UUID) (*Connection, bool) {
	id, err := uuid.Parse(chi.URLParam(r, "connectionID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid connection id")
		return nil, false
	}
	conn, err := h.repo.Get(r.Context(), orgID, id)
	if err != nil {
		slog.Error("integrations: get failed", "connection_id", id, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the connection")
		return nil, false
	}
	if conn == nil {
		writeError(w, http.StatusNotFound, "no such connection")
		return nil, false
	}
	return conn, true
}

func (h *Handler) isAdmin(ctx context.Context, userID, orgID uuid.UUID) bool {
	role, err := h.roles.MembershipRole(ctx, userID, orgID)
	return err == nil && role.AtLeast(organization.RoleAdmin)
}

func identify(w http.ResponseWriter, r *http.Request) (uuid.UUID, uuid.UUID, bool) {
	userID := uuid.Nil
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			userID = id
		}
	}
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return uuid.Nil, uuid.Nil, false
	}
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return uuid.Nil, uuid.Nil, false
	}
	return userID, orgID, true
}

func writeJSON(w http.ResponseWriter, code int, body any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(body)
}

func writeError(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]string{"error": msg})
}
