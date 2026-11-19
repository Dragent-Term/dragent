// Package account exposes GET /api/v1/account, a single endpoint that returns
// the authenticated user's profile, credit-pool usage, and the full list of
// models they can select from — all the data the account screen needs.
package account

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	billingModel "server/internal/billing/model"
	identityModel "server/internal/identity/model"
)

// UserRepository is the subset of identity UserRepository needed here.
type UserRepository interface {
	GetByID(ctx context.Context, id uuid.UUID) (*identityModel.User, error)
}

// PriceCatalog is the subset of PostgresBillingRepository needed here — the
// model list only. Balance reads go through BalanceSource instead (below),
// not a repository interface, so this handler never bypasses
// GetOrInitBalance's active-org resolution and lazy row creation with a raw
// repository read.
type PriceCatalog interface {
	billingModel.PriceRepository
}

// BalanceSource is the subset of BillingUseCase this screen needs: the
// caller's balance (GetOrInitBalance resolves the active org internally and
// lazily creates that row on first read — a user whose currently active
// org has never had one must still see a real free-tier balance, not a
// 500) and the free-model gate the same balance decides.
type BalanceSource interface {
	GetOrInitBalance(ctx context.Context, userID uuid.UUID) (*billingModel.UserBalance, error)
	RestrictedToFreeModel(ctx context.Context, bal *billingModel.UserBalance) bool
	SetAllowCreditUsage(ctx context.Context, userID uuid.UUID, allow bool) error
}

type Handler struct {
	users    UserRepository
	prices   PriceCatalog
	balances BalanceSource
}

func NewHandler(users UserRepository, prices PriceCatalog, balances BalanceSource) *Handler {
	return &Handler{users: users, prices: prices, balances: balances}
}

func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Get("/api/v1/account", h.getAccount)
	r.Patch("/api/v1/account/preferences", h.patchPreferences)
}

// ── Response types ────────────────────────────────────────────────────────────

type AccountResponse struct {
	User   UserInfo    `json:"user"`
	Usage  UsageInfo   `json:"usage"`
	Models []ModelInfo `json:"models"`
}

type UserInfo struct {
	ID          string `json:"id"`
	Email       string `json:"email"`
	FirstName   string `json:"first_name"`
	LastName    string `json:"last_name"`
	DisplayName string `json:"display_name"`
	AvatarURL   string `json:"avatar_url"`
	Tier        string `json:"tier"`
	CreatedAt   int64  `json:"created_at"`
}

type UsageInfo struct {
	// Paid is the monthly allowance (paid tiers), usable at any pace.
	Paid PaidUsage `json:"paid"`
	// Free is the monthly allowance every user gets, usable at any pace.
	Free FreeUsage `json:"free"`
	// DepositBalanceMc is the durable purchased-credit balance (top-ups).
	// Spent after the paid allowance and before the free allowance, and only
	// with consent.
	DepositBalanceMc int64 `json:"deposit_balance_mc"`
	// AllowCreditUsage is the user's opt-in for spending that balance.
	AllowCreditUsage bool `json:"allow_credit_usage"`
	// ModelLockedToFree is true when the user has no premium-capable funds (no
	// paid allowance and no consented credits) and is therefore restricted to
	// the free-tier model. The client uses it to LOCK the model selector — the
	// server enforces the same restriction on the agent path regardless.
	ModelLockedToFree bool `json:"model_locked_to_free"`
}

// PaidUsage is the monthly subscription allowance — a single pool, no windows.
type PaidUsage struct {
	IsPaid    bool  `json:"is_paid"`
	CapMc     int64 `json:"cap_mc"`     // monthly allowance total
	UsedMc    int64 `json:"used_mc"`    // cap - balance
	BalanceMc int64 `json:"balance_mc"` // remaining this month
	ResetsAt  int64 `json:"resets_at"`  // unix; monthly renewal
}

// FreeUsage is the monthly free allowance — a single pool, no sub-windows.
type FreeUsage struct {
	CapMc    int64 `json:"cap_mc"`
	UsedMc   int64 `json:"used_mc"`
	ResetsAt int64 `json:"resets_at"` // unix; monthly refresh
}

type ModelInfo struct {
	ID            string `json:"id"`
	Name          string `json:"name"`
	Provider      string `json:"provider"`
	ContextWindow int64  `json:"context_window"`
	MinTier       string `json:"min_tier"` // 'free' or 'pro'
	IsActive      bool   `json:"is_active"`
}

// ── Handler ───────────────────────────────────────────────────────────────────

func (h *Handler) getAccount(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	ctx := r.Context()

	// Fetch user profile and balance in parallel. GetOrInitBalance resolves
	// the caller's active org internally and lazily creates that org's
	// balance row on first read — a new user, or one whose active org has
	// never had a row of its own, gets a real balance back, never
	// sql.ErrNoRows, so there is no "treat a missing row as zero" fallback
	// to handle here.
	type userResult struct {
		u   *identityModel.User
		err error
	}
	type balResult struct {
		b   *billingModel.UserBalance
		err error
	}
	userCh := make(chan userResult, 1)
	balCh := make(chan balResult, 1)

	go func() {
		u, err := h.users.GetByID(ctx, userID)
		userCh <- userResult{u, err}
	}()
	go func() {
		b, err := h.balances.GetOrInitBalance(ctx, userID)
		balCh <- balResult{b, err}
	}()

	ur := <-userCh
	if ur.err != nil {
		writeError(w, http.StatusInternalServerError, "fetch user: "+ur.err.Error())
		return
	}

	br := <-balCh
	if br.err != nil {
		writeError(w, http.StatusInternalServerError, "fetch balance: "+br.err.Error())
		return
	}

	// Fetch user-facing models.
	dbModels, err := h.prices.ListUserFacingModels(ctx)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "fetch models: "+err.Error())
		return
	}

	u := ur.u
	bal := br.b

	displayName := u.DisplayName
	if displayName == "" {
		displayName = strings.TrimSpace(u.FirstName + " " + u.LastName)
	}
	if displayName == "" {
		displayName = u.Email
	}

	// Build usage section: monthly paid allowance + monthly free allowance.
	usage := UsageInfo{
		Paid: PaidUsage{
			IsPaid:    bal.IsPaid,
			CapMc:     bal.PaidMonthlyCap,
			UsedMc:    bal.PaidMonthlyCap - bal.PaidBalanceMc,
			BalanceMc: bal.PaidBalanceMc,
		},
		Free: FreeUsage{
			CapMc:  bal.FreeMonthlyCap,
			UsedMc: bal.FreeMonthlyUsed,
		},
		DepositBalanceMc:  bal.DepositBalanceMc,
		AllowCreditUsage:  bal.AllowCreditUsage,
		ModelLockedToFree: h.balances.RestrictedToFreeModel(ctx, bal),
	}
	if bal.PaidMonthlyResetAt != nil {
		usage.Paid.ResetsAt = bal.PaidMonthlyResetAt.Unix()
	}
	if bal.FreeMonthlyResetAt != nil {
		usage.Free.ResetsAt = bal.FreeMonthlyResetAt.Unix()
	}

	// Build model list, enriched with provider + display name + tier gate.
	models := make([]ModelInfo, 0, len(dbModels))
	for _, m := range dbModels {
		models = append(models, ModelInfo{
			ID:            m.ModelName,
			Name:          displayNameFor(m.ModelName),
			Provider:      providerFor(m.ModelName),
			ContextWindow: m.ContextWindowTokens,
			MinTier:       minTierFor(m.ModelName),
			IsActive:      m.IsActive,
		})
	}

	resp := AccountResponse{
		User: UserInfo{
			ID:          u.ID.String(),
			Email:       u.Email,
			FirstName:   u.FirstName,
			LastName:    u.LastName,
			DisplayName: displayName,
			AvatarURL:   u.AvatarURL,
			Tier:        bal.Tier,
			CreatedAt:   u.CreatedAt.Unix(),
		},
		Usage:  usage,
		Models: models,
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

// ── Model metadata helpers ────────────────────────────────────────────────────

// providerFor maps a model ID to its provider slug.
func providerFor(id string) string {
	switch {
	case strings.HasPrefix(id, "claude-"):
		return "anthropic"
	case strings.HasPrefix(id, "gpt-"):
		return "openai"
	case strings.HasPrefix(id, "gemini-"):
		return "google"
	case strings.HasPrefix(id, "grok-"):
		return "xai"
	case strings.HasPrefix(id, "groq/"):
		return "groq"
	default:
		return "unknown"
	}
}

// displayNameFor returns a human-readable model name.
var modelDisplayNames = map[string]string{
	"claude-fable-5":    "Claude Fable 5",
	"claude-sonnet-5":   "Claude Sonnet 5",
	"claude-opus-4-8":   "Claude Opus 4.8",
	"claude-sonnet-4-6": "Claude Sonnet 4.6",
	"claude-haiku-4-5":  "Claude Haiku 4.5",
	"gpt-5.6-sol":       "GPT-5.6 Sol",
	"gpt-5.6-terra":     "GPT-5.6 Terra",
	"gpt-5.6-luna":      "GPT-5.6 Luna",
	"gpt-5.5":           "GPT-5.5",
	"gpt-5.4":           "GPT-5.4",
	"gpt-5.4-mini":      "GPT-5.4 mini",
	"gemini-2.5-flash":  "Gemini 2.5 Flash",
	"gemini-3.5-flash":  "Gemini 3.5 Flash",
	"gemini-3.1-pro":    "Gemini 3.1 Pro",
	"grok-4.5":          "Grok 4.5",
	"grok-4.3":          "Grok 4.3",
	"grok-build-0.1":    "Grok Build 0.1",
}

func displayNameFor(id string) string {
	if name, ok := modelDisplayNames[id]; ok {
		return name
	}
	return id
}

// minTierFor returns the minimum subscription tier required to use the model.
// Currently only gpt-5.6-luna is accessible on the free tier; everything else
// requires at least Pro.
func minTierFor(id string) string {
	if id == "gpt-5.6-luna" {
		return billingModel.TierFree
	}
	return billingModel.TierPro
}

// ── Context helpers ───────────────────────────────────────────────────────────

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}

func writeError(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}

// ── Preferences ───────────────────────────────────────────────────────────────

type preferencesRequest struct {
	// Pointer so an omitted field is "leave unchanged" rather than "set false".
	AllowCreditUsage *bool `json:"allow_credit_usage"`
}

// patchPreferences updates user billing preferences. Currently only the
// credit-usage opt-in, which is the difference between a request being refused
// and the user's purchased credits being spent — so it is deliberately an
// explicit, authenticated write rather than anything inferred.
func (h *Handler) patchPreferences(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	var req preferencesRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.AllowCreditUsage == nil {
		writeError(w, http.StatusBadRequest, "no preference supplied")
		return
	}
	if err := h.balances.SetAllowCreditUsage(r.Context(), userID, *req.AllowCreditUsage); err != nil {
		writeError(w, http.StatusInternalServerError, "could not save preference")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]bool{"allow_credit_usage": *req.AllowCreditUsage})
}
