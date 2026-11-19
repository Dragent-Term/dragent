package execution

import (
	"context"
	"crypto/ed25519"
	"encoding/json"
	"log"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	billingModel "server/internal/billing/model"
)

// BalanceSource is the subset of the billing use case needed to resolve a
// user's tier for capability assignment. Mirrors the byok handler's minimal
// interface (deliberately narrow: the grant path must never bypass
// GetOrInitBalance's active-org resolution).
type BalanceSource interface {
	GetOrInitBalance(ctx context.Context, userID uuid.UUID) (*billingModel.UserBalance, error)
}

// Handler issues execution grants and serves the verification public key.
type Handler struct {
	priv      ed25519.PrivateKey
	ephemeral bool
	balances  BalanceSource
}

// NewHandler builds the grant handler. signingSeedB64 is a base64 32-byte
// Ed25519 seed (EXECUTION_GRANT_KEY); empty = ephemeral dev key.
func NewHandler(signingSeedB64 string, balances BalanceSource) (*Handler, error) {
	priv, ephemeral, err := LoadSigningKey(signingSeedB64)
	if err != nil {
		return nil, err
	}
	if ephemeral {
		log.Println("[execution] EXECUTION_GRANT_KEY unset: using an ephemeral grant key; cached grants expire on restart")
	}
	return &Handler{priv: priv, ephemeral: ephemeral, balances: balances}, nil
}

func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/v1/execution/authorize", h.authorize)
	r.Get("/v1/execution/pubkey", h.pubkey)
}

// ── POST /v1/execution/authorize ────────────────────────────────────────────

type authorizeRequest struct {
	// Client-supplied device identifier for grant binding (optional).
	Device string `json:"device_id"`
	// Client build version, recorded for future minimum-version pinning.
	// Accepted but not enforced in v1.
	ClientVersion string `json:"client_version"`
}

type authorizeResponse struct {
	ExecutionToken string   `json:"execution_token"`
	ExpiresAt      int64    `json:"expires_at"`
	UserID         string   `json:"user_id"`
	Capabilities   []string `json:"capabilities"`
}

func (h *Handler) authorize(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	var req authorizeRequest
	if r.Body != nil {
		_ = json.NewDecoder(r.Body).Decode(&req)
	}
	if len(req.Device) > 128 {
		req.Device = req.Device[:128]
	}

	// Resolve tier for capability assignment. Failure here fails closed:
	// no tier, no grant.
	bal, err := h.balances.GetOrInitBalance(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "fetch balance")
		return
	}
	caps := []string{CapabilityAgent, CapabilityManaged}
	// BYOK capability mirrors the existing product rule for cloud-stored
	// keys (byok handler): Pro+ tiers. Local execution costs us nothing
	// per use, but the entitlement boundary stays identical on purpose —
	// no product change smuggled in through a new endpoint.
	if bal.Tier != billingModel.TierFree {
		caps = append(caps, CapabilityBYOK)
	}

	token, claims, err := IssueGrant(h.priv, userID, bal.Tier, caps, req.Device)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "issue grant")
		return
	}
	writeJSON(w, http.StatusOK, authorizeResponse{
		ExecutionToken: token,
		ExpiresAt:      claims.ExpiresAt.Unix(),
		UserID:         userID.String(),
		Capabilities:   claims.Capabilities,
	})
}

// ── GET /v1/execution/pubkey ────────────────────────────────────────────────

type pubkeyResponse struct {
	Algorithm string `json:"algorithm"`
	PublicKey string `json:"public_key"`
	// True when the key is ephemeral (dev): clients must not expect cached
	// grants to survive a server restart.
	Ephemeral bool `json:"ephemeral"`
}

func (h *Handler) pubkey(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, pubkeyResponse{
		Algorithm: "Ed25519",
		PublicKey: PublicKeyB64(h.priv),
		Ephemeral: h.ephemeral,
	})
}

// ── helpers (unexported) ────────────────────────────────────────────────────

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(strings.TrimSpace(s)); err == nil {
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

func writeError(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]string{"error": msg})
}
