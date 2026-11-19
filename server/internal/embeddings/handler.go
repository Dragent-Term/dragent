package embeddings

import (
	"bytes"
	"errors"
	"io"
	"log/slog"
	"net/http"

	"server/internal/platform/ratelimit"
)

const voyageEmbeddingsURL = "https://api.voyageai.com/v1/embeddings"

// MaxBodyBytes caps one embeddings request. Voyage accepts at most 1,000
// inputs and 120k tokens per call (well under this); anything larger is
// not a real request.
const MaxBodyBytes = 4 << 20

// Default limits: per user, every request is paid for with the server key.
const (
	DefaultPerMinute = 60
	DefaultPerDay    = 2000
)

type Handler struct {
	voyageAPIKey string
	limits       Limits
	upstreamURL  string
	client       *http.Client
}

// Limits meter each user's use of the server's Voyage key.
type Limits struct {
	PerMinute ratelimit.Limiter
	PerDay    ratelimit.Limiter
}

func NewHandler(voyageAPIKey string) *Handler {
	return &Handler{voyageAPIKey: voyageAPIKey, upstreamURL: voyageEmbeddingsURL, client: http.DefaultClient}
}

// SetLimits installs the per-user rate limits.
func (h *Handler) SetLimits(l Limits) {
	h.limits = l
}

// Embed proxies POST /v1/embeddings to the Voyage API.
// The request body is forwarded unchanged — Voyage uses the same
// OpenAI-compatible format the Rust client already sends.
// Requires a valid user JWT (injected by AuthMiddleware as "user_id").
func (h *Handler) Embed(w http.ResponseWriter, r *http.Request) {
	userID, ok := r.Context().Value("user_id").(string)
	if !ok || userID == "" {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}

	if h.voyageAPIKey == "" {
		http.Error(w, "embedding service not configured", http.StatusServiceUnavailable)
		return
	}
	if !ratelimit.Check(w, r,
		ratelimit.Rule{Limiter: h.limits.PerMinute, Key: "rl:embeddings:minute:" + userID, Message: "embedding rate limit exceeded"},
		ratelimit.Rule{Limiter: h.limits.PerDay, Key: "rl:embeddings:day:" + userID, Message: "daily embedding limit reached"},
	) {
		return
	}

	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, MaxBodyBytes))
	if err != nil {
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
			return
		}
		http.Error(w, "failed to read request body", http.StatusBadRequest)
		return
	}

	req, err := http.NewRequestWithContext(r.Context(), http.MethodPost, h.upstreamURL, bytes.NewReader(body))
	if err != nil {
		slog.Error("embeddings: failed to build upstream request", "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}
	req.Header.Set("Authorization", "Bearer "+h.voyageAPIKey)
	req.Header.Set("Content-Type", "application/json")

	resp, err := h.client.Do(req)
	if err != nil {
		slog.Error("embeddings: upstream request failed", "error", err, "user_id", userID)
		http.Error(w, "upstream error", http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(resp.StatusCode)
	if _, err := io.Copy(w, resp.Body); err != nil {
		slog.Warn("embeddings: failed to copy upstream response", "error", err, "user_id", userID)
	}
}
