// Package voice mints short-lived Deepgram API keys for paid-tier users so
// the desktop app can stream dictation audio directly to Deepgram without the
// master key ever leaving the server.
//
//	POST /api/v1/voice/token  →  {"key": "...", "expires_in": 900, "model": "nova-3"}
//
// Free-tier users get 403 (they use the on-device OS engines); a missing
// server-side Deepgram configuration returns 503 — in both cases the client
// silently falls back to the OS engine, so this endpoint failing can never
// break dictation.
package voice

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	billingModel "server/internal/billing/model"
)

const (
	// Temp keys live long enough for one dictation session plus reconnects.
	keyTTLSeconds = 900
	// Streaming model the client should request.
	model = "nova-3"

	deepgramKeysURL = "https://api.deepgram.com/v1/projects/%s/keys"
)

// BalanceReader is the subset of BillingUseCase needed by this handler.
// GetOrInitBalance, not a raw repository read, is deliberate: a user whose
// currently active org has never had a balance row created for it must
// still get a real (free-tier) affordability answer, not a 500 —
// GetOrInitBalance already resolves the active org internally and lazily
// creates that row on first read.
type BalanceReader interface {
	GetOrInitBalance(ctx context.Context, userID uuid.UUID) (*billingModel.UserBalance, error)
}

type Handler struct {
	balances BalanceReader
	// Injected for tests; defaults to the real Deepgram call.
	mintKey func(ctx context.Context) (string, error)
}

func NewHandler(balances BalanceReader) *Handler {
	h := &Handler{balances: balances}
	h.mintKey = h.mintDeepgramKey
	return h
}

func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/api/v1/voice/token", h.createToken)
}

func (h *Handler) createToken(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeJSONError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	if os.Getenv("DEEPGRAM_API_KEY") == "" || os.Getenv("DEEPGRAM_PROJECT_ID") == "" {
		writeJSONError(w, http.StatusServiceUnavailable, "voice transcription is not configured")
		return
	}

	// Cloud dictation is a paid-tier feature; free tier uses the OS engines.
	bal, err := h.balances.GetOrInitBalance(r.Context(), userID)
	if err != nil || bal == nil {
		writeJSONError(w, http.StatusForbidden, "cloud dictation requires a paid plan")
		return
	}
	if bal.Tier == billingModel.TierFree {
		writeJSONError(w, http.StatusForbidden, "cloud dictation requires a paid plan")
		return
	}

	key, err := h.mintKey(r.Context())
	if err != nil {
		log.Printf("[voice] deepgram key mint failed for user %s: %v", userID, err)
		writeJSONError(w, http.StatusBadGateway, "could not provision transcription key")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"key":        key,
		"expires_in": keyTTLSeconds,
		"model":      model,
	})
}

// mintDeepgramKey creates a short-lived, usage-scoped key in our Deepgram
// project. Scope usage:write only — the temp key can transcribe but cannot
// manage the project or create further keys.
func (h *Handler) mintDeepgramKey(ctx context.Context) (string, error) {
	body, _ := json.Marshal(map[string]any{
		"comment":                 "dragent-voice-session",
		"scopes":                  []string{"usage:write"},
		"time_to_live_in_seconds": keyTTLSeconds,
	})
	url := fmt.Sprintf(deepgramKeysURL, os.Getenv("DEEPGRAM_PROJECT_ID"))
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Token "+os.Getenv("DEEPGRAM_API_KEY"))
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", fmt.Errorf("deepgram keys API returned %d", resp.StatusCode)
	}
	var out struct {
		Key string `json:"key"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", err
	}
	if out.Key == "" {
		return "", fmt.Errorf("deepgram keys API returned no key")
	}
	return out.Key, nil
}

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}

func writeJSONError(w http.ResponseWriter, status int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}
