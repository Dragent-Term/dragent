package config

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	billingModel "server/internal/billing/model"
)

type Handler struct {
	repo Repo
}

func NewHandler(repo Repo) *Handler {
	return &Handler{repo: repo}
}

// RegisterRoutes mounts the admin config API. Every route is additionally
// gated by RequireAdmin middleware at the router level.
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Get("/api/v1/admin/models/{name}", h.getModel)
	r.Put("/api/v1/admin/models", h.upsertModel)
	r.Post("/api/v1/admin/models/{name}/active", h.setModelActive)
	r.Get("/api/v1/admin/config", h.listConfig)
	r.Put("/api/v1/admin/config/{key}", h.setConfig)
}

// ── shared helpers ────────────────────────────────────────────────────────────

func adminID(r *http.Request) (uuid.UUID, bool) {
	s, _ := r.Context().Value("user_id").(string)
	id, err := uuid.Parse(s)
	if err != nil {
		return uuid.Nil, false
	}
	return id, true
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

type apiError struct {
	Error   string   `json:"error"`
	Details []string `json:"details,omitempty"`
}

func writeErr(w http.ResponseWriter, status int, msg string, details ...string) {
	writeJSON(w, status, apiError{Error: msg, Details: details})
}

// decodeStrict rejects unknown fields so a typo in a payload is a 400, not a
// silently-ignored field on a billing-critical object.
func decodeStrict(r *http.Request, dst any) error {
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	return dec.Decode(dst)
}

// ── model pricing ─────────────────────────────────────────────────────────────

// ModelPriceRequest is the typed payload for creating/updating a model's rates.
// Pointers distinguish "omitted" from "zero" so an update can't accidentally
// zero out a rate. All rate fields are micro-cents per million tokens.
type ModelPriceRequest struct {
	ModelName                            string `json:"model_name"`
	InputRatePerMillion                  *int64 `json:"input_rate_per_million"`
	OutputRatePerMillion                 *int64 `json:"output_rate_per_million"`
	CacheWriteRatePerMillion             *int64 `json:"cache_write_rate_per_million"`
	CachedInputRatePerMillion            *int64 `json:"cached_input_rate_per_million"`
	LongContextThreshold                 *int64 `json:"long_context_threshold"`
	LongContextInputRatePerMillion       *int64 `json:"long_context_input_rate_per_million"`
	LongContextCachedInputRatePerMillion *int64 `json:"long_context_cached_input_rate_per_million"`
	LongContextOutputRatePerMillion      *int64 `json:"long_context_output_rate_per_million"`
	ContextWindowTokens                  *int64 `json:"context_window_tokens"`
	MinBalanceMc                         *int64 `json:"min_balance_mc"`
	IsActive                             *bool  `json:"is_active"`
	IsUserFacing                         *bool  `json:"is_user_facing"`
}

func (h *Handler) upsertModel(w http.ResponseWriter, r *http.Request) {
	admin, ok := adminID(r)
	if !ok {
		writeErr(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	var req ModelPriceRequest
	if err := decodeStrict(r, &req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON body", err.Error())
		return
	}

	// Validation — reject anything that would corrupt billing.
	var problems []string
	name := strings.TrimSpace(req.ModelName)
	if name == "" {
		problems = append(problems, "model_name is required")
	}
	if req.InputRatePerMillion == nil {
		problems = append(problems, "input_rate_per_million is required")
	}
	if req.OutputRatePerMillion == nil {
		problems = append(problems, "output_rate_per_million is required")
	}
	nonNeg := func(field string, v *int64) {
		if v != nil && *v < 0 {
			problems = append(problems, field+" must be >= 0")
		}
	}
	nonNeg("input_rate_per_million", req.InputRatePerMillion)
	nonNeg("output_rate_per_million", req.OutputRatePerMillion)
	nonNeg("cache_write_rate_per_million", req.CacheWriteRatePerMillion)
	nonNeg("cached_input_rate_per_million", req.CachedInputRatePerMillion)
	nonNeg("long_context_threshold", req.LongContextThreshold)
	nonNeg("long_context_input_rate_per_million", req.LongContextInputRatePerMillion)
	nonNeg("long_context_cached_input_rate_per_million", req.LongContextCachedInputRatePerMillion)
	nonNeg("long_context_output_rate_per_million", req.LongContextOutputRatePerMillion)
	nonNeg("context_window_tokens", req.ContextWindowTokens)
	nonNeg("min_balance_mc", req.MinBalanceMc)
	if len(problems) > 0 {
		writeErr(w, http.StatusUnprocessableEntity, "validation failed", problems...)
		return
	}

	// Merge onto the existing row (or a zero value for a brand-new model) so a
	// partial update leaves untouched fields intact.
	existing, err := h.repo.GetModelPrice(r.Context(), name)
	base := &billingModel.ModelPrice{ModelName: name, IsActive: true}
	if err == nil && existing != nil && existing.ModelName == name {
		base = existing
	}
	before, _ := json.Marshal(base)

	setI := func(dst *int64, v *int64) {
		if v != nil {
			*dst = *v
		}
	}
	setB := func(dst *bool, v *bool) {
		if v != nil {
			*dst = *v
		}
	}
	setI(&base.InputRatePerMillion, req.InputRatePerMillion)
	setI(&base.OutputRatePerMillion, req.OutputRatePerMillion)
	setI(&base.CacheWriteRatePerMillion, req.CacheWriteRatePerMillion)
	setI(&base.CachedInputRatePerMillion, req.CachedInputRatePerMillion)
	setI(&base.LongContextThreshold, req.LongContextThreshold)
	setI(&base.LongContextInputRatePerMillion, req.LongContextInputRatePerMillion)
	setI(&base.LongContextCachedInputRatePerMillion, req.LongContextCachedInputRatePerMillion)
	setI(&base.LongContextOutputRatePerMillion, req.LongContextOutputRatePerMillion)
	setI(&base.ContextWindowTokens, req.ContextWindowTokens)
	setI(&base.MinBalanceMc, req.MinBalanceMc)
	setB(&base.IsActive, req.IsActive)
	setB(&base.IsUserFacing, req.IsUserFacing)

	if err := h.repo.UpdateModelPrice(r.Context(), base); err != nil {
		writeErr(w, http.StatusInternalServerError, "failed to save model price", err.Error())
		return
	}
	after, _ := json.Marshal(base)
	_ = h.repo.RecordAudit(r.Context(), admin, "model_price", name, before, after)
	writeJSON(w, http.StatusOK, base)
}

func (h *Handler) getModel(w http.ResponseWriter, r *http.Request) {
	name := chi.URLParam(r, "name")
	mp, err := h.repo.GetModelPrice(r.Context(), name)
	if err != nil || mp == nil {
		writeErr(w, http.StatusNotFound, "model not found")
		return
	}
	writeJSON(w, http.StatusOK, mp)
}

type setActiveRequest struct {
	IsActive *bool `json:"is_active"`
}

func (h *Handler) setModelActive(w http.ResponseWriter, r *http.Request) {
	admin, ok := adminID(r)
	if !ok {
		writeErr(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	name := chi.URLParam(r, "name")
	var req setActiveRequest
	if err := decodeStrict(r, &req); err != nil || req.IsActive == nil {
		writeErr(w, http.StatusBadRequest, "is_active (bool) is required")
		return
	}
	if err := h.repo.SetActive(r.Context(), name, *req.IsActive); err != nil {
		writeErr(w, http.StatusInternalServerError, "failed to set active", err.Error())
		return
	}
	nv, _ := json.Marshal(map[string]any{"model_name": name, "is_active": *req.IsActive})
	_ = h.repo.RecordAudit(r.Context(), admin, "model_price", name, nil, nv)
	writeJSON(w, http.StatusOK, map[string]any{"model_name": name, "is_active": *req.IsActive})
}

// ── system constants ──────────────────────────────────────────────────────────

func (h *Handler) listConfig(w http.ResponseWriter, r *http.Request) {
	rows, err := h.repo.ListConfigs(r.Context())
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "failed to list config", err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"configs": rows})
}

type setConfigRequest struct {
	Value any `json:"value"`
}

func (h *Handler) setConfig(w http.ResponseWriter, r *http.Request) {
	admin, ok := adminID(r)
	if !ok {
		writeErr(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	key := chi.URLParam(r, "key")

	meta, err := h.repo.GetConfigMeta(r.Context(), key)
	if err != nil || meta == nil {
		// Only known keys are settable — prevents typos creating dead config.
		writeErr(w, http.StatusNotFound, "unknown config key: "+key)
		return
	}

	var req setConfigRequest
	if err := decodeStrict(r, &req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON body", err.Error())
		return
	}
	normalized, verr := validateConfigValue(meta.ValueType, req.Value)
	if verr != nil {
		writeErr(w, http.StatusUnprocessableEntity, "validation failed", verr.Error())
		return
	}
	// Markup has a semantic invariant the type check cannot see: the ratio must
	// exceed 1, or every request is sold at or below provider cost. Validate the
	// resulting PAIR here so a bad value is refused at write time rather than
	// taking billing down at the next charge (or at the next restart, since the
	// service now refuses to boot without a valid markup).
	if verr := h.validateMarkupChange(r.Context(), key, normalized); verr != nil {
		writeErr(w, http.StatusUnprocessableEntity, "validation failed", verr.Error())
		return
	}

	before, _ := json.Marshal(map[string]string{"value": meta.Value})
	if err := h.repo.Set(r.Context(), key, normalized); err != nil {
		writeErr(w, http.StatusInternalServerError, "failed to save config", err.Error())
		return
	}
	after, _ := json.Marshal(map[string]string{"value": normalized})
	_ = h.repo.RecordAudit(r.Context(), admin, "system_config", key, before, after)
	writeJSON(w, http.StatusOK, billingModel.SystemConfigRow{
		Key: key, Value: normalized, ValueType: meta.ValueType, Description: meta.Description,
	})
}

// validateConfigValue enforces the stored value_type and returns the canonical
// string form to persist. This is the type-enforcement guarantee: a config
// declared 'int' can never be set to "abc" or 3.5.
func validateConfigValue(valueType string, raw any) (string, error) {
	switch valueType {
	case "int", "duration_sec":
		f, ok := raw.(float64) // JSON numbers decode to float64
		if !ok {
			return "", fmt.Errorf("expected an integer for a %s value", valueType)
		}
		if f != float64(int64(f)) {
			return "", errors.New("expected a whole number")
		}
		iv := int64(f)
		if iv < 0 {
			return "", errors.New("must be >= 0")
		}
		return strconv.FormatInt(iv, 10), nil
	case "float":
		f, ok := raw.(float64)
		if !ok {
			return "", errors.New("expected a number")
		}
		return strconv.FormatFloat(f, 'g', -1, 64), nil
	case "bool":
		b, ok := raw.(bool)
		if !ok {
			return "", errors.New("expected true or false")
		}
		return strconv.FormatBool(b), nil
	case "string":
		s, ok := raw.(string)
		if !ok {
			return "", errors.New("expected a string")
		}
		if strings.TrimSpace(s) == "" {
			return "", errors.New("must not be empty")
		}
		return s, nil
	default:
		return "", fmt.Errorf("unsupported value_type %q", valueType)
	}
}

var _ = context.Background

// validateMarkupChange checks a proposed markup_numerator/markup_denominator
// write against its counterpart, so the pair is always valid on disk.
func (h *Handler) validateMarkupChange(ctx context.Context, key, normalized string) error {
	if key != "markup_numerator" && key != "markup_denominator" {
		return nil
	}
	other := "markup_denominator"
	if key == "markup_denominator" {
		other = "markup_numerator"
	}
	otherRaw, err := h.repo.Get(ctx, other)
	if err != nil {
		return fmt.Errorf("cannot validate markup: %q is missing from system_configs", other)
	}
	proposed, err := strconv.ParseInt(normalized, 10, 64)
	if err != nil {
		return fmt.Errorf("markup values must be whole numbers")
	}
	current, err := strconv.ParseInt(strings.TrimSpace(otherRaw), 10, 64)
	if err != nil {
		return fmt.Errorf("cannot validate markup: %q = %q is not an integer", other, otherRaw)
	}
	num, den := proposed, current
	if key == "markup_denominator" {
		num, den = current, proposed
	}
	return billingModel.ValidateMarkup(num, den)
}
