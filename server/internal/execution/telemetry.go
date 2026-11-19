package execution

import (
	"encoding/json"
	"log"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

// TelemetryEvent mirrors the client's locked telemetry shape field for
// field — and the decoder rejects anything else. This endpoint can never
// receive prompts, file contents, keys, or streams, because the wire
// format has nowhere to put them.
type TelemetryEvent struct {
	ExecutionID  string  `json:"execution_id"`
	Provider     string  `json:"provider"`
	Path         string  `json:"path"`
	StartedAtMs  int64   `json:"started_at_ms"`
	DurationMs   uint64  `json:"duration_ms"`
	Success      bool    `json:"success"`
	Error        *string `json:"error"`
	InputTokens  uint64  `json:"input_tokens"`
	OutputTokens uint64  `json:"output_tokens"`
	ToolCalls    uint32  `json:"tool_calls"`
	// Client build version for skew analysis.
	ClientVersion string `json:"client_version"`
}

const maxTelemetryBatch = 100

// RegisterTelemetryRoutes mounts the ingest endpoint on an authenticated
// router. v1 accepts, validates, and counts; durable storage is follow-up.
func RegisterTelemetryRoutes(r chi.Router) {
	r.Post("/v1/telemetry/events", ingestTelemetry)
}

func ingestTelemetry(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	var events []TelemetryEvent
	if err := dec.Decode(&events); err != nil {
		writeError(w, http.StatusBadRequest, "invalid telemetry batch")
		return
	}
	if len(events) > maxTelemetryBatch {
		writeError(w, http.StatusBadRequest, "batch too large")
		return
	}
	for i := range events {
		if _, err := uuid.Parse(events[i].ExecutionID); err != nil {
			writeError(w, http.StatusBadRequest, "invalid execution_id")
			return
		}
		if len(events[i].Provider) > 32 || len(events[i].Path) > 16 {
			writeError(w, http.StatusBadRequest, "field too long")
			return
		}
	}
	// v1: accept + count. No content fields exist to persist by construction.
	log.Printf("[telemetry] accepted %d events from user %s", len(events), userID.String())
	writeJSON(w, http.StatusAccepted, map[string]any{"accepted": len(events)})
}
