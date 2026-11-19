// Package automations keeps the organization-wide registry of desktop
// automations. The desktop app defines and runs each automation on its
// owner's machine and mirrors it here, so teammates can see what the
// organization automates and so the free plan's allowance (FreeLimit
// automations per account, unlimited on a paid plan) holds across every
// machine the account uses.
package automations

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
)

// FreeLimit is how many automations an account on the free plan can have
// in one organization.
const FreeLimit = 4

const (
	maxNameChars   = 200
	maxPromptChars = 50_000
	maxShortChars  = 200
)

// Automation is one registry entry. Times are Unix milliseconds on the
// wire and in this struct.
type Automation struct {
	ID             uuid.UUID `json:"id"`
	OrganizationID uuid.UUID `json:"organization_id"`
	OwnerUserID    uuid.UUID `json:"owner_user_id"`
	OwnerName      string    `json:"owner_name"`
	OwnerEmail     string    `json:"owner_email"`
	Name           string    `json:"name"`
	Prompt         string    `json:"prompt"`
	Schedule       string    `json:"schedule"`
	ScheduleLabel  string    `json:"schedule_label"`
	Timezone       string    `json:"timezone"`
	Agent          string    `json:"agent"`
	// Nil for a session automation (no project).
	ProjectLabel  *string `json:"project_label"`
	Enabled       bool    `json:"enabled"`
	NextRunAt     *int64  `json:"next_run_at"`
	LastRunStatus *string `json:"last_run_status"`
	LastRunAt     *int64  `json:"last_run_at"`
	CreatedAt     int64   `json:"created_at"`
	UpdatedAt     int64   `json:"updated_at"`
}

// Allowance is the requesting account's standing against its plan.
type Allowance struct {
	// Nil on a paid plan: unlimited.
	Limit *int   `json:"limit"`
	Owned int    `json:"owned"`
	Tier  string `json:"tier"`
}

type Repository interface {
	List(ctx context.Context, orgID uuid.UUID) ([]Automation, error)
	// Get returns nil when no automation has this id in the organization.
	Get(ctx context.Context, orgID, id uuid.UUID) (*Automation, error)
	// Exists reports whether the id is taken in any organization.
	Exists(ctx context.Context, id uuid.UUID) (bool, error)
	Upsert(ctx context.Context, a Automation) error
	Delete(ctx context.Context, orgID, id uuid.UUID) error
	CountOwned(ctx context.Context, orgID, userID uuid.UUID) (int, error)
	// Tier is the best of the account's own tier in the organization and
	// the organization's plan.
	Tier(ctx context.Context, orgID, userID uuid.UUID) (string, error)
}

type Handler struct {
	repo Repository
	now  func() time.Time
}

func NewHandler(repo Repository) *Handler {
	return &Handler{repo: repo, now: time.Now}
}

// RegisterRoutes mounts the registry; `member` must admit only members of
// the organization in the path.
func (h *Handler) RegisterRoutes(r chi.Router, member func(http.Handler) http.Handler) {
	r.With(member).Get("/api/v1/organizations/{orgID}/automations", h.list)
	r.With(member).Put("/api/v1/organizations/{orgID}/automations/{automationID}", h.save)
	r.With(member).Delete("/api/v1/organizations/{orgID}/automations/{automationID}", h.remove)
}

func isPaid(tier string) bool {
	return tier != "" && tier != "free"
}

func (h *Handler) allowance(ctx context.Context, orgID, userID uuid.UUID) (Allowance, error) {
	tier, err := h.repo.Tier(ctx, orgID, userID)
	if err != nil {
		return Allowance{}, err
	}
	owned, err := h.repo.CountOwned(ctx, orgID, userID)
	if err != nil {
		return Allowance{}, err
	}
	allowance := Allowance{Owned: owned, Tier: tier}
	if !isPaid(tier) {
		limit := FreeLimit
		allowance.Limit = &limit
	}
	return allowance, nil
}

func (h *Handler) list(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	ctx := r.Context()
	items, err := h.repo.List(ctx, orgID)
	if err != nil {
		slog.Error("automations: list failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load automations")
		return
	}
	allowance, err := h.allowance(ctx, orgID, userID)
	if err != nil {
		slog.Error("automations: allowance failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load automations")
		return
	}
	if items == nil {
		items = []Automation{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"automations": items, "allowance": allowance})
}

type saveRequest struct {
	Name          string  `json:"name"`
	Prompt        string  `json:"prompt"`
	Schedule      string  `json:"schedule"`
	ScheduleLabel string  `json:"schedule_label"`
	Timezone      string  `json:"timezone"`
	Agent         string  `json:"agent"`
	ProjectLabel  *string `json:"project_label"`
	Enabled       bool    `json:"enabled"`
	NextRunAt     *int64  `json:"next_run_at"`
	LastRunStatus *string `json:"last_run_status"`
	LastRunAt     *int64  `json:"last_run_at"`
	CreatedAt     int64   `json:"created_at"`
}

func (req *saveRequest) validate() error {
	req.Name = strings.TrimSpace(req.Name)
	req.Schedule = strings.TrimSpace(req.Schedule)
	req.Agent = strings.TrimSpace(req.Agent)
	switch {
	case req.Name == "":
		return errors.New("name is required")
	case req.Schedule == "":
		return errors.New("schedule is required")
	case req.Agent == "":
		return errors.New("agent is required")
	case utf8.RuneCountInString(req.Name) > maxNameChars:
		return errors.New("name is too long")
	case utf8.RuneCountInString(req.Prompt) > maxPromptChars:
		return errors.New("prompt is too long")
	}
	for _, field := range []string{req.Schedule, req.ScheduleLabel, req.Timezone, req.Agent} {
		if utf8.RuneCountInString(field) > maxShortChars {
			return errors.New("a field is too long")
		}
	}
	if req.ProjectLabel != nil && utf8.RuneCountInString(*req.ProjectLabel) > maxShortChars {
		return errors.New("project label is too long")
	}
	if req.LastRunStatus != nil && utf8.RuneCountInString(*req.LastRunStatus) > 40 {
		return errors.New("run status is too long")
	}
	return nil
}

func (h *Handler) save(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	id, err := uuid.Parse(chi.URLParam(r, "automationID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid automation id")
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 256<<10)
	var req saveRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if err := req.validate(); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	ctx := r.Context()
	existing, err := h.repo.Get(ctx, orgID, id)
	if err != nil {
		slog.Error("automations: get failed", "automation_id", id, "error", err)
		writeError(w, http.StatusInternalServerError, "could not save the automation")
		return
	}
	if existing != nil && existing.OwnerUserID != userID {
		writeError(w, http.StatusForbidden, "only the owner can change this automation")
		return
	}
	if existing == nil {
		// An id from another organization is never taken over.
		taken, err := h.repo.Exists(ctx, id)
		if err != nil {
			slog.Error("automations: exists failed", "automation_id", id, "error", err)
			writeError(w, http.StatusInternalServerError, "could not save the automation")
			return
		}
		if taken {
			writeError(w, http.StatusConflict, "this automation belongs to another organization")
			return
		}
		allowance, err := h.allowance(ctx, orgID, userID)
		if err != nil {
			slog.Error("automations: allowance failed", "organization_id", orgID, "error", err)
			writeError(w, http.StatusInternalServerError, "could not save the automation")
			return
		}
		if allowance.Limit != nil && allowance.Owned >= *allowance.Limit {
			writeJSON(w, http.StatusPaymentRequired, map[string]any{
				"error": "The free plan includes 4 automations. Upgrade to Pro for unlimited automations.",
				"code":  "automation_limit",
				"limit": *allowance.Limit,
			})
			return
		}
	}

	now := h.now().UnixMilli()
	created := req.CreatedAt
	if existing != nil {
		created = existing.CreatedAt
	} else if created <= 0 || created > now {
		created = now
	}
	item := Automation{
		ID:             id,
		OrganizationID: orgID,
		OwnerUserID:    userID,
		Name:           req.Name,
		Prompt:         req.Prompt,
		Schedule:       req.Schedule,
		ScheduleLabel:  strings.TrimSpace(req.ScheduleLabel),
		Timezone:       strings.TrimSpace(req.Timezone),
		Agent:          req.Agent,
		ProjectLabel:   req.ProjectLabel,
		Enabled:        req.Enabled,
		NextRunAt:      req.NextRunAt,
		LastRunStatus:  req.LastRunStatus,
		LastRunAt:      req.LastRunAt,
		CreatedAt:      created,
		UpdatedAt:      now,
	}
	if err := h.repo.Upsert(ctx, item); err != nil {
		slog.Error("automations: upsert failed", "automation_id", id, "error", err)
		writeError(w, http.StatusInternalServerError, "could not save the automation")
		return
	}
	status := http.StatusOK
	if existing == nil {
		status = http.StatusCreated
	}
	writeJSON(w, status, item)
}

func (h *Handler) remove(w http.ResponseWriter, r *http.Request) {
	userID, orgID, ok := identify(w, r)
	if !ok {
		return
	}
	id, err := uuid.Parse(chi.URLParam(r, "automationID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid automation id")
		return
	}
	ctx := r.Context()
	existing, err := h.repo.Get(ctx, orgID, id)
	if err != nil {
		slog.Error("automations: get failed", "automation_id", id, "error", err)
		writeError(w, http.StatusInternalServerError, "could not delete the automation")
		return
	}
	if existing == nil {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	if existing.OwnerUserID != userID {
		writeError(w, http.StatusForbidden, "only the owner can delete this automation")
		return
	}
	if err := h.repo.Delete(ctx, orgID, id); err != nil {
		slog.Error("automations: delete failed", "automation_id", id, "error", err)
		writeError(w, http.StatusInternalServerError, "could not delete the automation")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func identify(w http.ResponseWriter, r *http.Request) (uuid.UUID, uuid.UUID, bool) {
	userID := userIDFromContext(r)
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

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}

func writeJSON(w http.ResponseWriter, code int, body any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(body)
}

func writeError(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]string{"error": msg})
}
