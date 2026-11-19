// Package usage exposes GET /api/v1/usage, returning the caller's credit pool
// state (paid + free) and rolling window positions.
package usage

import (
	"context"
	"encoding/json"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	billingModel "server/internal/billing/model"
)

// BalanceSource is the subset of BillingUseCase needed by this handler.
// GetOrInitBalance, not a raw repository read, is deliberate: a user whose
// currently active org has never had a balance row created for it (e.g.
// they just switched into a team they hold no paid seat in) must see a
// real free-tier balance, not a 500 — GetOrInitBalance already resolves the
// active org internally and lazily creates that row on first read.
type BalanceSource interface {
	GetOrInitBalance(ctx context.Context, userID uuid.UUID) (*billingModel.UserBalance, error)
}

type Handler struct {
	balances BalanceSource
}

func NewHandler(balances BalanceSource) *Handler {
	return &Handler{balances: balances}
}

func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Get("/api/v1/usage", h.getUsage)
}

// UsageResponse is the JSON body returned by GET /api/v1/usage.
type UsageResponse struct {
	Tier string    `json:"tier"`
	Paid PaidUsage `json:"paid"`
	Free FreeUsage `json:"free"`
}

// PaidUsage is the monthly subscription allowance — one pool, no windows.
type PaidUsage struct {
	IsPaid    bool  `json:"is_paid"`
	CapMc     int64 `json:"cap_mc"`
	UsedMc    int64 `json:"used_mc"`
	BalanceMc int64 `json:"balance_mc"`
	ResetsAt  int64 `json:"resets_at"`
}

// FreeUsage is the monthly free allowance — one pool, no sub-windows.
type FreeUsage struct {
	CapMc    int64 `json:"cap_mc"`
	UsedMc   int64 `json:"used_mc"`
	ResetsAt int64 `json:"resets_at"`
}

func (h *Handler) getUsage(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	bal, err := h.balances.GetOrInitBalance(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "fetch balance: "+err.Error())
		return
	}

	now := time.Now().UTC()

	// The free MONTHLY allowance renews on a calendar month; if it has rolled
	// since the last charge its used-counter is logically zero. Reads never
	// persist a new window.
	monthActive := bal.FreeMonthlyResetAt != nil && now.Before(*bal.FreeMonthlyResetAt)
	freeUsed := int64(0)
	if monthActive {
		freeUsed = bal.FreeMonthlyUsed
	}

	resp := UsageResponse{
		Tier: bal.Tier,
		Paid: PaidUsage{
			IsPaid:    bal.IsPaid,
			CapMc:     bal.PaidMonthlyCap,
			UsedMc:    bal.PaidMonthlyCap - bal.PaidBalanceMc,
			BalanceMc: bal.PaidBalanceMc,
		},
		Free: FreeUsage{
			CapMc:  bal.FreeMonthlyCap,
			UsedMc: freeUsed,
		},
	}
	if bal.PaidMonthlyResetAt != nil {
		resp.Paid.ResetsAt = bal.PaidMonthlyResetAt.Unix()
	}
	resp.Free.ResetsAt = bal.NextFreeResetTime().Unix()

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

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
