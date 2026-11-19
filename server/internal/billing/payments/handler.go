package payments

import (
	"encoding/json"
	"errors"
	"io"
	"log"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

const maxWebhookBytes = 1 << 20 // 1 MiB

type Handler struct {
	svc *Service
}

func NewHandler(svc *Service) *Handler {
	return &Handler{svc: svc}
}

// RegisterRoutes mounts the authenticated billing routes (require a user).
// Subscription checkout and the subscription-portal link are NOT here —
// every subscription is an org subscription now, personal orgs included
// (see organization.Handler's own /billing/subscribe and /billing/portal
// routes). Only one-time credit-pack purchases stay here: they credit
// whichever org the buyer is currently acting as, resolved via OrgResolver,
// which is a per-purchase choice rather than an org-level subscription
// action.
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Get("/api/v1/billing/plans", h.listPlans)
	r.Get("/api/v1/billing/packs", h.listPacks)
	r.Post("/api/v1/billing/topup", h.createTopup)
}

// RegisterWebhook mounts the public webhook route (authenticated by signature,
// NOT by user token — the provider calls it). Register OUTSIDE RequireAuth.
func (h *Handler) RegisterWebhook(r chi.Router) {
	r.Post("/api/v1/billing/webhook/lemonsqueezy", h.webhook)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

func (h *Handler) listPlans(w http.ResponseWriter, r *http.Request) {
	if userIDFromContext(r) == uuid.Nil {
		writeErr(w, http.StatusUnauthorized, "authentication required")
		return
	}
	plans, err := h.svc.ListPlans(r.Context())
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "list plans: "+err.Error())
		return
	}
	out := make([]planJSON, 0, len(plans))
	for _, p := range plans {
		out = append(out, planJSON{
			ID:              p.ID,
			Tier:            p.Tier,
			BillingInterval: p.BillingInterval,
			PriceCents:      p.PriceCents,
			Currency:        p.Currency,
			DisplayName:     p.DisplayName,
			Purchasable:     p.LemonSqueezyVariantID != "",
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"plans": out, "enabled": h.svc.Enabled()})
}

func (h *Handler) listPacks(w http.ResponseWriter, r *http.Request) {
	if userIDFromContext(r) == uuid.Nil {
		writeErr(w, http.StatusUnauthorized, "authentication required")
		return
	}
	packs, err := h.svc.ListPacks(r.Context())
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "list packs: "+err.Error())
		return
	}
	out := make([]packJSON, 0, len(packs))
	for _, p := range packs {
		out = append(out, packJSON{
			ID:          p.ID,
			PriceCents:  p.PriceCents,
			CreditsMc:   p.CreditsMc,
			Currency:    p.Currency,
			DisplayName: p.DisplayName,
			Purchasable: p.LemonSqueezyVariantID != "",
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"packs": out, "enabled": h.svc.Enabled()})
}

type topupRequest struct {
	PackID string `json:"pack_id"`
}

func (h *Handler) createTopup(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeErr(w, http.StatusUnauthorized, "authentication required")
		return
	}
	var req topupRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.PackID == "" {
		writeErr(w, http.StatusBadRequest, "pack_id is required")
		return
	}
	url, err := h.svc.CreateTopupCheckout(r.Context(), userID, req.PackID)
	if err != nil {
		if errors.Is(err, ErrDisabled) {
			writeErr(w, http.StatusServiceUnavailable, "payments are not available")
			return
		}
		if errors.Is(err, ErrPlanNotFound) {
			writeErr(w, http.StatusNotFound, "pack not found")
			return
		}
		writeErr(w, http.StatusBadGateway, "topup: "+err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"url": url})
}

func (h *Handler) webhook(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(io.LimitReader(r.Body, maxWebhookBytes))
	if err != nil {
		writeErr(w, http.StatusBadRequest, "read body")
		return
	}
	signature := r.Header.Get("X-Signature")

	if err := h.svc.HandleWebhook(r.Context(), body, signature); err != nil {
		if errors.Is(err, ErrInvalidSignature) {
			// Don't leak detail; 401 so the provider doesn't keep retrying a
			// forged/misconfigured delivery.
			writeErr(w, http.StatusUnauthorized, "invalid signature")
			return
		}
		// Genuine processing failure — 500 so the provider retries.
		log.Printf("[payments] webhook processing error: %v", err)
		writeErr(w, http.StatusInternalServerError, "processing error")
		return
	}
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte(`{"ok":true}`))
}

// ── helpers ───────────────────────────────────────────────────────────────────

type planJSON struct {
	ID              string `json:"id"`
	Tier            string `json:"tier"`
	BillingInterval string `json:"billing_interval"`
	PriceCents      int    `json:"price_cents"`
	Currency        string `json:"currency"`
	DisplayName     string `json:"display_name"`
	Purchasable     bool   `json:"purchasable"`
}

type packJSON struct {
	ID          string `json:"id"`
	PriceCents  int    `json:"price_cents"`
	CreditsMc   int64  `json:"credits_mc"`
	Currency    string `json:"currency"`
	DisplayName string `json:"display_name"`
	Purchasable bool   `json:"purchasable"`
}

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]string{"error": msg})
}
