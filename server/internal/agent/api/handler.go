// Package api exposes the agent pipeline over HTTP with Server-Sent Events (SSE).
//
// Public routes:
//
//	POST   /api/v1/agent/tasks                       — create a task, returns {"task_id": "..."}
//	GET    /api/v1/agent/tasks/{id}/stream            — SSE stream for a task
//	POST   /api/v1/agent/tasks/{id}/tool_results      — submit a local tool execution result
//	POST   /api/v1/agent/tasks/{id}/resume            — resume from a durable checkpoint
//	DELETE /api/v1/agent/tasks/{id}                   — cancel a running task
package api

import (
	"bytes"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/agent/pipeline"
	"server/internal/agent/providers"
	"server/internal/agent/tasks"
	"server/internal/platform/netguard"
)

// Handler holds dependencies for the agent HTTP routes.
type Handler struct {
	manager      *tasks.Manager
	providerKeys map[string]string // server-side provider API keys, used by /api/models
}

func NewHandler(manager *tasks.Manager, providerKeys map[string]string) *Handler {
	return &Handler{manager: manager, providerKeys: providerKeys}
}

// RegisterRoutes mounts the agent routes on a chi router.
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/api/v1/agent/tasks", h.createTask)
	// Compatibility path for released clients whose complete loop is server-owned.
	r.Post("/api/v1/agent/tasks/stream", h.createAndStreamTask)
	// Normal desktop path: one stateless provider step per streamed response.
	r.Post("/api/v1/agent/steps/stream", h.createAndStreamDesktopStep)
	r.Get("/api/v1/agent/tasks/{id}/stream", h.streamTask)
	r.Post("/api/v1/agent/tasks/{id}/tool_results", h.submitToolResult)
	r.Post("/api/v1/agent/tasks/{id}/resume", h.resumeTask)
	r.Delete("/api/v1/agent/tasks/{id}", h.cancelTask)
	r.Delete("/api/v1/agent/tasks/{id}/data", h.deleteTaskData)
	r.Post("/api/v1/agent/compact", h.compactHistory)
	r.Post("/api/v1/agent/extract", h.extractMemories)
	r.Post("/api/v1/agent/consolidate", h.consolidateMemories)
	// Haiku extraction proxy — platform key stays server-side.
	r.Post("/api/v1/haiku/extract", h.haikuExtract)
	// Returns which providers have server-side keys configured.
	r.Get("/api/models", h.availableModels)
}

// ── Create task ───────────────────────────────────────────────────────────────

type createTaskToolDef struct {
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	Parameters  map[string]interface{} `json:"parameters"`
}

type createTaskRequest struct {
	ModelName string `json:"model_name"`
	System    string `json:"system"`
	Query     string `json:"query"`
	// Attachments ride with the current Query turn (images: screenshots,
	// design exports). History messages carry their own attachments.
	Attachments []taskAttachment `json:"attachments,omitempty"`
	// Optional: supply a full message history (for follow-up turns).
	Messages []createTaskMessage `json:"messages,omitempty"`
	// AgentMode controls which tools are visible to the LLM.
	// "coordinator" → no write/exec tools; "worker" → no spawn_subagent; "" → all tools.
	AgentMode string `json:"agent_mode,omitempty"`
	// Effort is the user-selected reasoning depth: "low" | "medium" | "high".
	// Omitted or unrecognised = provider default. Silently dropped for models
	// that reject the parameter, so an out-of-date client can't fail a task.
	Effort string `json:"effort,omitempty"`
	// ExtraTools are MCP tool definitions provided by the client to be added to
	// the LLM's tool list for this task.
	ExtraTools []createTaskToolDef `json:"extra_tools,omitempty"`
	// AllowedTools, when present, is a fail-closed capability allowlist. A
	// present empty array intentionally exposes no tools. This is distinct from
	// omission, which preserves the legacy unrestricted terminal-agent flow.
	AllowedTools *[]string `json:"allowed_tools,omitempty"`
	// ExecutionOwner="desktop" requests a non-durable single provider step.
	// No prompt, event, checkpoint, or tool result is written to PostgreSQL.
	ExecutionOwner string `json:"execution_owner,omitempty"`
	// ClientRunID/ClientRound provide content-free idempotency for desktop-owned
	// provider steps. They contain no conversation or filesystem data.
	ClientRunID string `json:"client_run_id,omitempty"`
	ClientRound int    `json:"client_round,omitempty"`
}

// taskAttachment is a user-attached file on a message. Only images reach the
// LLM as native multimodal content; the client inlines text files into the
// message body itself.
type taskAttachment struct {
	Type      string `json:"type"`       // "image"
	MediaType string `json:"media_type"` // image/png, image/jpeg, image/webp, image/gif
	Data      string `json:"data"`       // base64, no data: prefix
}

type createTaskToolCall struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Args string `json:"args"` // raw JSON args string
}

type createTaskMessage struct {
	Role       string `json:"role"`
	Content    string `json:"content"`
	ToolCallID string `json:"tool_call_id,omitempty"`
	// ToolCalls must be included for role:"assistant" messages that requested tool calls.
	// Omitting them causes orphan role:"tool" messages that the LLM API rejects with 400.
	ToolCalls   []createTaskToolCall `json:"tool_calls,omitempty"`
	Attachments []taskAttachment     `json:"attachments,omitempty"`
}

// Attachment limits. Anthropic caps images at 5 MB decoded; base64 inflates
// ~4/3, so cap the encoded payload accordingly. The per-message count keeps a
// single request from ballooning past provider limits and our own body cap.
const (
	maxImagesPerMessage = 6
	maxImageB64Bytes    = 7 * 1024 * 1024 // ≈5 MB decoded
)

var allowedImageTypes = map[string]bool{
	"image/png":  true,
	"image/jpeg": true,
	"image/webp": true,
	"image/gif":  true,
}

// convertAttachments validates and converts wire attachments to provider
// image parts. Returns a human-readable error for the 400 response.
func convertAttachments(atts []taskAttachment) ([]providers.ImageAttachment, error) {
	if len(atts) == 0 {
		return nil, nil
	}
	if len(atts) > maxImagesPerMessage {
		return nil, fmt.Errorf("too many attachments on one message (max %d)", maxImagesPerMessage)
	}
	images := make([]providers.ImageAttachment, 0, len(atts))
	for _, a := range atts {
		if a.Type != "image" {
			return nil, fmt.Errorf("unsupported attachment type %q (only images reach the model; send text files inline)", a.Type)
		}
		if !allowedImageTypes[a.MediaType] {
			return nil, fmt.Errorf("unsupported image media type %q", a.MediaType)
		}
		if len(a.Data) == 0 {
			return nil, fmt.Errorf("empty image attachment")
		}
		if len(a.Data) > maxImageB64Bytes {
			return nil, fmt.Errorf("image too large (max ~5 MB); resize or compress it")
		}
		images = append(images, providers.ImageAttachment{MediaType: a.MediaType, DataB64: a.Data})
	}
	return images, nil
}

type createTaskResponse struct {
	TaskID string `json:"task_id"`
	Status string `json:"status"`
}

type createTaskHTTPError struct {
	status  int
	message string
}

func (e *createTaskHTTPError) Error() string { return e.message }

func (h *Handler) createTaskForRequest(r *http.Request, requiredExecutionOwner string) (*tasks.Task, error) {
	var req createTaskRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		return nil, &createTaskHTTPError{http.StatusBadRequest, "invalid request body: " + err.Error()}
	}
	if req.Query == "" && len(req.Messages) == 0 {
		return nil, &createTaskHTTPError{http.StatusBadRequest, "query or messages required"}
	}

	// Build provider message list — preserve ToolCalls so multi-turn history that
	// includes assistant tool-call turns can be reconstructed correctly.
	// Without this, the API sees role:"tool" messages with no matching tool_calls
	// on the preceding assistant message and returns a 400 error.
	messages := make([]providers.Message, 0, len(req.Messages)+1)
	for _, m := range req.Messages {
		msg := providers.Message{
			Role:       providers.AgentRole(m.Role),
			Content:    m.Content,
			ToolCallID: m.ToolCallID,
		}
		if len(m.ToolCalls) > 0 {
			msg.ToolCalls = make([]providers.ToolCall, len(m.ToolCalls))
			for i, tc := range m.ToolCalls {
				msg.ToolCalls[i] = providers.ToolCall{
					ID:       tc.ID,
					Name:     tc.Name,
					ArgsJSON: tc.Args,
				}
			}
		}
		imgs, err := convertAttachments(m.Attachments)
		if err != nil {
			return nil, &createTaskHTTPError{http.StatusBadRequest, err.Error()}
		}
		msg.Images = imgs
		messages = append(messages, msg)
	}
	if req.Query != "" {
		imgs, err := convertAttachments(req.Attachments)
		if err != nil {
			return nil, &createTaskHTTPError{http.StatusBadRequest, err.Error()}
		}
		messages = append(messages, providers.Message{
			Role:    providers.RoleUser,
			Content: req.Query,
			Images:  imgs,
		})
	}

	// Extract user ID from context (set by auth middleware).
	// RequireAuth in main.go ensures this is never nil for agent routes, but we
	// guard here too so the billing layer never operates on uuid.Nil.
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		return nil, &createTaskHTTPError{http.StatusUnauthorized, "authentication required"}
	}
	if requiredExecutionOwner != "" && req.ExecutionOwner != requiredExecutionOwner {
		return nil, &createTaskHTTPError{
			http.StatusUpgradeRequired,
			"this endpoint requires a desktop-owned agent run; update the desktop app",
		}
	}

	// Convert MCP extra tools from the request into provider.ToolDefinition entries.
	extraTools := make([]providers.ToolDefinition, 0, len(req.ExtraTools))
	for _, t := range req.ExtraTools {
		extraTools = append(extraTools, providers.ToolDefinition{
			Name:        t.Name,
			Description: t.Description,
			Parameters:  t.Parameters,
		})
	}
	allowedTools := []string(nil)
	restrictTools := req.AllowedTools != nil
	if restrictTools {
		seen := make(map[string]struct{}, len(*req.AllowedTools))
		allowedTools = make([]string, 0, len(*req.AllowedTools))
		for _, raw := range *req.AllowedTools {
			name := strings.TrimSpace(raw)
			if name == "" {
				return nil, &createTaskHTTPError{http.StatusBadRequest, "allowed_tools cannot contain an empty tool name"}
			}
			if _, exists := seen[name]; exists {
				continue
			}
			seen[name] = struct{}{}
			allowedTools = append(allowedTools, name)
		}
	}

	// Extract optional BYOK credentials. AuthMiddleware copies these into the
	// context, but read the headers directly as a defensive fallback too. This
	// keeps the credential attached when a proxy/router wraps the request context
	// before it reaches the streaming handler. Never log or persist the secret.
	userApiKey, _ := r.Context().Value("user_api_key").(string)
	if strings.TrimSpace(userApiKey) == "" {
		userApiKey = r.Header.Get("X-User-Api-Key")
	}
	userApiKey = strings.TrimSpace(userApiKey)
	customBaseURL, _ := r.Context().Value("custom_base_url").(string)
	if strings.TrimSpace(customBaseURL) == "" {
		customBaseURL = r.Header.Get("X-Custom-Base-Url")
	}
	customBaseURL = strings.TrimSpace(customBaseURL)
	if customBaseURL != "" {
		if err := netguard.ValidateURL(customBaseURL); err != nil {
			return nil, &createTaskHTTPError{http.StatusBadRequest, err.Error()}
		}
	}
	customModel, _ := r.Context().Value("custom_model").(string)
	if strings.TrimSpace(customModel) == "" {
		customModel = r.Header.Get("X-Custom-Model")
	}
	customModel = strings.TrimSpace(customModel)
	fallbackAllowed, _ := r.Context().Value("fallback_allowed").(bool)
	if !fallbackAllowed {
		fallbackAllowed = strings.EqualFold(strings.TrimSpace(r.Header.Get("X-Fallback-Allowed")), "true")
	}

	// For OpenAI-compatible custom endpoints all three come from headers:
	// X-User-Api-Key, X-Custom-Base-Url, X-Custom-Model. When a custom base URL
	// is set and the client also sends X-Custom-Model, that model name takes
	// precedence over whatever model_name was set in the request body.
	modelName := req.ModelName
	if customBaseURL != "" && customModel != "" {
		modelName = customModel
	}
	if userApiKey != "" {
		fmt.Printf("[agent] BYOK credential received (model=%q fallback_allowed=%t)\n", modelName, fallbackAllowed)
	}

	// Reasoning depth. An unrecognised value is dropped rather than rejected:
	// a stale client sending a level this server doesn't know should fall back
	// to the provider default, not fail the whole task.
	effort := providers.Effort(strings.ToLower(strings.TrimSpace(req.Effort)))
	if !effort.Valid() {
		effort = ""
	}

	task, err := h.manager.Create(r.Context(), tasks.CreateRequest{
		UserID:          userID,
		ModelName:       modelName,
		System:          req.System,
		Messages:        messages,
		Mode:            tasks.AgentMode(req.AgentMode),
		Effort:          effort,
		ExtraTools:      extraTools,
		AllowedTools:    allowedTools,
		RestrictTools:   restrictTools,
		UserApiKey:      userApiKey,
		CustomBaseURL:   customBaseURL,
		FallbackAllowed: fallbackAllowed,
		DesktopOwned:    req.ExecutionOwner == "desktop",
		ClientRunID:     req.ClientRunID,
		ClientRound:     req.ClientRound,
	})
	if err != nil {
		if errors.Is(err, tasks.ErrUnauthenticated) {
			return nil, &createTaskHTTPError{http.StatusUnauthorized, err.Error()}
		}
		// Insufficient credits → 402 Payment Required
		if errors.Is(err, tasks.ErrInsufficientCredits) {
			return nil, &createTaskHTTPError{http.StatusPaymentRequired, err.Error()}
		}
		// Monthly allowance exhausted (free or paid) → 402 with reset date.
		if errors.Is(err, tasks.ErrFreeCreditsExhausted) ||
			errors.Is(err, tasks.ErrPaidCreditsExhausted) {
			return nil, &createTaskHTTPError{http.StatusPaymentRequired, err.Error()}
		}
		// Exhausted but purchased credits are available and just switched off →
		// 402 carrying the "enable credits in Settings" message. Without this it
		// fell through to a generic 500 and the user saw "an error occurred".
		if errors.Is(err, tasks.ErrCreditsNotEnabled) {
			return nil, &createTaskHTTPError{http.StatusPaymentRequired, err.Error()}
		}
		return nil, &createTaskHTTPError{http.StatusInternalServerError, "create task: " + err.Error()}
	}
	return task, nil
}

func writeCreateTaskError(w http.ResponseWriter, err error) {
	var httpErr *createTaskHTTPError
	if errors.As(err, &httpErr) {
		writeError(w, httpErr.status, httpErr.message)
		return
	}
	writeError(w, http.StatusInternalServerError, "create task: "+err.Error())
}

// Deprecated: legacy two-request server-owned task creation. Current clients
// drive desktop-owned steps via createAndStreamDesktopStep instead; this
// endpoint stays only for released clients still migrating. Do not build new
// flows on it.
func (h *Handler) createTask(w http.ResponseWriter, r *http.Request) {
	task, err := h.createTaskForRequest(r, "")
	if err != nil {
		writeCreateTaskError(w, err)
		return
	}

	writeJSON(w, http.StatusCreated, createTaskResponse{
		TaskID: task.ID.String(),
		Status: string(task.Status),
	})
}

// resumeTask creates a linked attempt from the source task's durable working
// state. BYOK credentials are intentionally supplied again by the client and
// are never loaded from storage.
func (h *Handler) resumeTask(w http.ResponseWriter, r *http.Request) {
	sourceID, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid task id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	userAPIKey := strings.TrimSpace(r.Header.Get("X-User-Api-Key"))
	customBaseURL := strings.TrimSpace(r.Header.Get("X-Custom-Base-Url"))
	if customBaseURL != "" {
		if err := netguard.ValidateURL(customBaseURL); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
	}
	fallbackAllowed := strings.EqualFold(strings.TrimSpace(r.Header.Get("X-Fallback-Allowed")), "true")
	var body struct {
		RuntimeContext string `json:"runtime_context"`
	}
	if r.Body != nil {
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil && !errors.Is(err, io.EOF) {
			writeError(w, http.StatusBadRequest, "invalid resume request")
			return
		}
	}
	if len(body.RuntimeContext) > 32_000 {
		writeError(w, http.StatusBadRequest, "resume context is too large")
		return
	}

	task, err := h.manager.Resume(r.Context(), sourceID, userID, tasks.CreateRequest{
		UserApiKey: userAPIKey, CustomBaseURL: customBaseURL, FallbackAllowed: fallbackAllowed,
		ResumeRuntimeContext: body.RuntimeContext,
	})
	if err != nil {
		// Ownership and existence are intentionally indistinguishable.
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "task not found")
			return
		}
		writeError(w, http.StatusConflict, err.Error())
		return
	}
	writeJSON(w, http.StatusCreated, createTaskResponse{TaskID: task.ID.String(), Status: string(task.Status)})
}

// Deprecated: legacy server-owned streaming entry. It keeps released clients
// working while they migrate from server-owned tasks. New desktop builds use
// createAndStreamDesktopStep. Do not build new flows on it.
func (h *Handler) createAndStreamTask(w http.ResponseWriter, r *http.Request) {
	h.createAndStream(w, r, "")
}

// createAndStreamDesktopStep is exclusively desktop-owned and non-durable:
// provider-ready history remains local, and a dropped step is retried from the
// desktop checkpoint. Durable execution is reserved for a future cloud path.
func (h *Handler) createAndStreamDesktopStep(w http.ResponseWriter, r *http.Request) {
	h.createAndStream(w, r, "desktop")
}

func (h *Handler) createAndStream(w http.ResponseWriter, r *http.Request, requiredOwner string) {
	task, err := h.createTaskForRequest(r, requiredOwner)
	if err != nil {
		writeCreateTaskError(w, err)
		return
	}
	w.Header().Set("X-Dragent-Task-ID", task.ID.String())
	if task.DesktopOwned {
		w.Header().Set("X-Dragent-Execution-Owner", "desktop")
	}
	h.serveTaskStream(w, r, task, 0)
}

// ── Stream task events ────────────────────────────────────────────────────────

func (h *Handler) streamTask(w http.ResponseWriter, r *http.Request) {
	taskID, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid task id")
		return
	}

	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	task, ok := h.manager.GetForUser(taskID, userID)
	if !ok {
		// Deliberately do not reveal whether a task owned by another user exists.
		writeError(w, http.StatusNotFound, "task not found")
		return
	}

	fromSeq := parseLastEventID(r.Header.Get("Last-Event-ID"))
	h.serveTaskStream(w, r, task, fromSeq)
}

// serveTaskStream attaches a subscriber, replays any requested gap, and then
// forwards live task events. It is shared by the initial streaming POST and the
// reconnect GET so both paths have identical ordering and keepalive semantics.
func (h *Handler) serveTaskStream(w http.ResponseWriter, r *http.Request, task *tasks.Task, fromSeq uint64) {
	backlog, sub, resumable := task.Subscribe(fromSeq)
	if !resumable {
		// Check this before committing the SSE 200 response; otherwise net/http
		// cannot change the already-written status to 409.
		writeError(w, http.StatusConflict, "stream not resumable from that position")
		return
	}
	defer task.Unsubscribe(sub)

	// Set SSE headers only after all failures that require an HTTP status.
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.Header().Set("X-Accel-Buffering", "no") // disable nginx buffering
	w.WriteHeader(http.StatusOK)

	flusher, canFlush := w.(http.Flusher)

	// Start the runner now that a subscriber is attached (deferred in
	// Manager.Create) so the entire response streams to this client instead of
	// racing ahead and arriving all at once. Idempotent (startOnce), so a
	// reconnect does not restart it.
	task.StartRunner()

	// Replay missed events first, in order, before going live.
	lastSeq := fromSeq
	for _, se := range backlog {
		if se.Seq <= lastSeq {
			continue
		}
		writeSSEWithID(w, se.Seq, se.Ev)
		lastSeq = se.Seq
		if se.Ev.Kind == pipeline.KindDone || se.Ev.Kind == pipeline.KindStepDone || se.Ev.Kind == pipeline.KindError {
			// The turn already finished during the gap — deliver the terminal
			// event and stop; there is nothing live to wait for.
			if canFlush {
				flusher.Flush()
			}
			return
		}
	}
	if canFlush {
		flusher.Flush()
	}

	ping := time.NewTicker(20 * time.Second)
	defer ping.Stop()
	poll := time.NewTicker(500 * time.Millisecond)
	defer poll.Stop()

	for {
		select {
		case <-r.Context().Done():
			return

		case <-ping.C:
			fmt.Fprintf(w, ": ping\n\n")
			if canFlush {
				flusher.Flush()
			}

		case <-poll.C:
			// Durable polling bridges API replicas: the stream may be attached to
			// a different process from the executor that appended the event.
			events, err := task.EventsAfter(r.Context(), lastSeq)
			if err != nil {
				continue
			}
			terminal := false
			for _, se := range events {
				if se.Seq <= lastSeq {
					continue
				}
				writeSSEWithID(w, se.Seq, se.Ev)
				lastSeq = se.Seq
				terminal = se.Ev.Kind == pipeline.KindDone || se.Ev.Kind == pipeline.KindStepDone || se.Ev.Kind == pipeline.KindError
			}
			if len(events) > 0 && canFlush {
				flusher.Flush()
			}
			if terminal {
				return
			}
			status, statusErr := task.DurableStatus(r.Context())
			if statusErr == nil && status == tasks.StatusCancelled {
				return
			}

		case se, ok := <-sub:
			if !ok {
				return
			}
			if se.Seq <= lastSeq {
				continue
			}
			writeSSEWithID(w, se.Seq, se.Ev)
			lastSeq = se.Seq
			if canFlush {
				flusher.Flush()
			}
			if se.Ev.Kind == pipeline.KindDone || se.Ev.Kind == pipeline.KindStepDone || se.Ev.Kind == pipeline.KindError {
				return
			}
		}
	}
}

// parseLastEventID reads the SSE resume cursor; anything unparseable is a fresh
// connect (seq 0).
func parseLastEventID(h string) uint64 {
	if h == "" {
		return 0
	}
	n, err := strconv.ParseUint(strings.TrimSpace(h), 10, 64)
	if err != nil {
		return 0
	}
	return n
}

// ── Tool result submission ─────────────────────────────────────────────────────
// The client executes tool calls locally and POSTs results here so the server
// can continue the LLM loop with the actual output.

type toolResultRequest struct {
	CallID  string `json:"call_id"`
	Output  string `json:"output"`
	IsError bool   `json:"is_error"`
}

func (h *Handler) submitToolResult(w http.ResponseWriter, r *http.Request) {
	taskID, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid task id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	task, ok := h.manager.GetForUser(taskID, userID)
	if !ok {
		writeError(w, http.StatusNotFound, "task not found")
		return
	}

	var req toolResultRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return
	}
	if req.CallID == "" {
		writeError(w, http.StatusBadRequest, "call_id is required")
		return
	}

	if !task.ResolveToolResult(req.CallID, req.Output, req.IsError) {
		writeError(w, http.StatusGone, "tool call not found or already resolved")
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
}

// ── Cancel task ───────────────────────────────────────────────────────────────

func (h *Handler) cancelTask(w http.ResponseWriter, r *http.Request) {
	taskID, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid task id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	task, ok := h.manager.GetForUser(taskID, userID)
	if !ok {
		writeError(w, http.StatusNotFound, "task not found")
		return
	}
	task.Cancel()
	writeJSON(w, http.StatusOK, map[string]string{"status": "cancelled"})
}

// deleteTaskData permanently removes the server-side recovery record and all
// dependent checkpoints, tool results, and stream events. Conversation history
// stored by the desktop is a separate, local lifecycle.
func (h *Handler) deleteTaskData(w http.ResponseWriter, r *http.Request) {
	taskID, err := uuid.Parse(chi.URLParam(r, "id"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid task id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	if err := h.manager.DeleteForUser(r.Context(), taskID, userID); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "task not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not delete task data")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// ── Compact history ───────────────────────────────────────────────────────────
// Allows the client to request LLM-based compaction of its conversation history
// before sending the next turn, replacing middle messages with a dense summary.

type compactHistoryRequest struct {
	ModelName string              `json:"model_name"`
	System    string              `json:"system"`
	Messages  []createTaskMessage `json:"messages"`
}

type compactHistoryResponse struct {
	Messages []createTaskMessage `json:"messages"`
	Summary  string              `json:"summary,omitempty"`
}

func (h *Handler) compactHistory(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	var req compactHistoryRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return
	}
	if len(req.Messages) == 0 {
		writeError(w, http.StatusBadRequest, "messages required")
		return
	}

	msgs := make([]providers.Message, len(req.Messages))
	for i, m := range req.Messages {
		msg := providers.Message{
			Role:       providers.AgentRole(m.Role),
			Content:    m.Content,
			ToolCallID: m.ToolCallID,
		}
		if len(m.ToolCalls) > 0 {
			msg.ToolCalls = make([]providers.ToolCall, len(m.ToolCalls))
			for j, tc := range m.ToolCalls {
				msg.ToolCalls[j] = providers.ToolCall{ID: tc.ID, Name: tc.Name, ArgsJSON: tc.Args}
			}
		}
		msgs[i] = msg
	}

	result, err := h.manager.Compact(r.Context(), tasks.CompactHistoryRequest{
		UserID:    userID,
		ModelName: req.ModelName,
		System:    req.System,
		Messages:  msgs,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "compact: "+err.Error())
		return
	}

	respMsgs := make([]createTaskMessage, len(result.Messages))
	for i, m := range result.Messages {
		respMsgs[i] = createTaskMessage{
			Role:       string(m.Role),
			Content:    m.Content,
			ToolCallID: m.ToolCallID,
		}
	}

	writeJSON(w, http.StatusOK, compactHistoryResponse{
		Messages: respMsgs,
		Summary:  result.SummaryText,
	})
}

// ── Extract memories ──────────────────────────────────────────────────────────
// Extracts memorable facts from a conversation history via LLM.

type memoryItemJSON struct {
	Name        string  `json:"name"`
	Description string  `json:"description"`
	Type        string  `json:"type"`
	Body        string  `json:"body"`
	Evidence    string  `json:"evidence,omitempty"`
	Confidence  float64 `json:"confidence,omitempty"`
}

type extractMemoriesRequest struct {
	ModelName string              `json:"model_name"`
	Messages  []createTaskMessage `json:"messages"`
}

type extractMemoriesResponse struct {
	Memories []memoryItemJSON `json:"memories"`
}

func (h *Handler) extractMemories(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	var req extractMemoriesRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return
	}
	if len(req.Messages) == 0 {
		writeError(w, http.StatusBadRequest, "messages required")
		return
	}

	msgs := make([]providers.Message, len(req.Messages))
	for i, m := range req.Messages {
		msg := providers.Message{
			Role:       providers.AgentRole(m.Role),
			Content:    m.Content,
			ToolCallID: m.ToolCallID,
		}
		if len(m.ToolCalls) > 0 {
			msg.ToolCalls = make([]providers.ToolCall, len(m.ToolCalls))
			for j, tc := range m.ToolCalls {
				msg.ToolCalls[j] = providers.ToolCall{ID: tc.ID, Name: tc.Name, ArgsJSON: tc.Args}
			}
		}
		msgs[i] = msg
	}

	result, err := h.manager.Extract(r.Context(), tasks.ExtractRequest{
		UserID:    userID,
		ModelName: req.ModelName,
		Messages:  msgs,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "extract: "+err.Error())
		return
	}

	out := make([]memoryItemJSON, len(result.Memories))
	for i, mem := range result.Memories {
		out[i] = memoryItemJSON{
			Name:        mem.Name,
			Description: mem.Description,
			Type:        mem.Type,
			Body:        mem.Body,
			Evidence:    mem.Evidence,
			Confidence:  mem.Confidence,
		}
	}
	writeJSON(w, http.StatusOK, extractMemoriesResponse{Memories: out})
}

// ── Consolidate memories ──────────────────────────────────────────────────────
// Consolidates an existing memory list via LLM, deduplicating and merging.

type consolidateMemoriesRequest struct {
	ModelName string           `json:"model_name"`
	Memories  []memoryItemJSON `json:"memories"`
}

type consolidateMemoriesResponse struct {
	Memories []memoryItemJSON `json:"memories"`
}

func (h *Handler) consolidateMemories(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	var req consolidateMemoriesRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return
	}
	if len(req.Memories) == 0 {
		writeError(w, http.StatusBadRequest, "memories required")
		return
	}

	mems := make([]tasks.MemoryItem, len(req.Memories))
	for i, m := range req.Memories {
		mems[i] = tasks.MemoryItem{
			Name:        m.Name,
			Description: m.Description,
			Type:        m.Type,
			Body:        m.Body,
			Evidence:    m.Evidence,
			Confidence:  m.Confidence,
		}
	}

	result, err := h.manager.Consolidate(r.Context(), tasks.ConsolidateRequest{
		UserID:    userID,
		ModelName: req.ModelName,
		Memories:  mems,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "consolidate: "+err.Error())
		return
	}

	out := make([]memoryItemJSON, len(result.Memories))
	for i, mem := range result.Memories {
		out[i] = memoryItemJSON{
			Name:        mem.Name,
			Description: mem.Description,
			Type:        mem.Type,
			Body:        mem.Body,
			Evidence:    mem.Evidence,
			Confidence:  mem.Confidence,
		}
	}
	writeJSON(w, http.StatusOK, consolidateMemoriesResponse{Memories: out})
}

// ── Haiku extraction proxy ────────────────────────────────────────────────────
// Proxies HTML content to Claude Haiku for semantic extraction, keeping the
// platform API key entirely server-side. The client sends its session bearer
// token; the server uses its own ANTHROPIC_API_KEY.
//
// POST /api/v1/haiku/extract
//   Body:  {"html": "...", "url": "..."}
//   200:   {"content": "..."}
//   503:   platform key not configured
//   402:   user has no credits / free tier exhausted

const haikuMaxHTMLBytes = 20_000

type haikuExtractRequest struct {
	HTML string `json:"html"`
	URL  string `json:"url"`
}

type haikuExtractResponse struct {
	Content string `json:"content"`
}

func (h *Handler) haikuExtract(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	var req haikuExtractRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return
	}
	if req.HTML == "" {
		writeError(w, http.StatusBadRequest, "html field required")
		return
	}

	apiKey := h.providerKeys["ANTHROPIC_API_KEY"]
	if apiKey == "" {
		writeError(w, http.StatusServiceUnavailable, "Haiku extraction unavailable: platform API key not configured")
		return
	}

	// Truncate to keep prompt size reasonable.
	html := req.HTML
	if len(html) > haikuMaxHTMLBytes {
		html = html[:haikuMaxHTMLBytes]
	}

	prompt := fmt.Sprintf(
		"Extract the main content of this webpage as clean plain text. "+
			"Focus on the primary article body, documentation, or main content. "+
			"Omit navigation menus, ads, footers, cookie banners, and boilerplate. "+
			"URL: %s\n\nHTML:\n%s",
		req.URL, html,
	)

	// Model is DB-configurable (config web_extract_model) so it can be swapped
	// when deprecated. It must stay an Anthropic model — this calls the
	// Anthropic API directly.
	extractModel := h.manager.WebExtractModel(r.Context())

	// Gate before spending. This endpoint calls Anthropic on the PLATFORM key
	// with user-supplied HTML and was previously ungated and unmetered — any
	// authenticated account could loop it and bill our provider account
	// without limit.
	if err := h.manager.GateInternal(r.Context(), userID, extractModel); err != nil {
		writeError(w, http.StatusPaymentRequired, err.Error())
		return
	}
	anthropicBody := map[string]interface{}{
		"model":      extractModel,
		"max_tokens": 2000,
		"messages": []map[string]string{
			{"role": "user", "content": prompt},
		},
	}
	bodyBytes, err := json.Marshal(anthropicBody)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "marshal error: "+err.Error())
		return
	}

	httpReq, err := http.NewRequestWithContext(r.Context(), http.MethodPost,
		"https://api.anthropic.com/v1/messages", bytes.NewReader(bodyBytes))
	if err != nil {
		writeError(w, http.StatusInternalServerError, "build request: "+err.Error())
		return
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("x-api-key", apiKey)
	httpReq.Header.Set("anthropic-version", "2023-06-01")

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(httpReq)
	if err != nil {
		writeError(w, http.StatusBadGateway, "Anthropic request failed: "+err.Error())
		return
	}
	defer resp.Body.Close()

	var anthropicResp struct {
		Content []struct {
			Text string `json:"text"`
		} `json:"content"`
		// Usage was not parsed at all before, which is why this call could
		// never be billed even in principle.
		Usage struct {
			InputTokens              int64 `json:"input_tokens"`
			OutputTokens             int64 `json:"output_tokens"`
			CacheReadInputTokens     int64 `json:"cache_read_input_tokens"`
			CacheCreationInputTokens int64 `json:"cache_creation_input_tokens"`
		} `json:"usage"`
		Error *struct {
			Message string `json:"message"`
			Type    string `json:"type"`
		} `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&anthropicResp); err != nil {
		writeError(w, http.StatusInternalServerError, "parse Anthropic response: "+err.Error())
		return
	}
	// Settle as soon as usage is known, BEFORE the error branches below: the
	// tokens were spent whether or not the answer turned out to be usable.
	// Cache-creation tokens bill at the input rate (see CalculateCost).
	h.manager.SettleInternal(userID, extractModel,
		anthropicResp.Usage.InputTokens+anthropicResp.Usage.CacheCreationInputTokens,
		anthropicResp.Usage.CacheReadInputTokens,
		anthropicResp.Usage.OutputTokens)
	if anthropicResp.Error != nil {
		writeError(w, http.StatusBadGateway, "Anthropic error: "+anthropicResp.Error.Message)
		return
	}
	if len(anthropicResp.Content) == 0 || anthropicResp.Content[0].Text == "" {
		writeError(w, http.StatusInternalServerError, "empty response from Anthropic")
		return
	}

	writeJSON(w, http.StatusOK, haikuExtractResponse{Content: anthropicResp.Content[0].Text})
}

// ── SSE helpers ───────────────────────────────────────────────────────────────

func writeSSE(w http.ResponseWriter, ev pipeline.AgentEvent) {
	data, _ := json.Marshal(ev)
	fmt.Fprintf(w, "event: %s\ndata: %s\n\n", ev.Kind, data)
}

// writeSSEWithID emits an event with its seq as the SSE id, so the client can
// resume from it after a dropped connection (Last-Event-ID).
func writeSSEWithID(w http.ResponseWriter, seq uint64, ev pipeline.AgentEvent) {
	data, _ := json.Marshal(ev)
	fmt.Fprintf(w, "id: %d\nevent: %s\ndata: %s\n\n", seq, ev.Kind, data)
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

func writeJSON(w http.ResponseWriter, code int, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

func writeError(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]string{"error": msg})
}

// ── Auth context ──────────────────────────────────────────────────────────────

func userIDFromContext(r *http.Request) uuid.UUID {
	// The AuthMiddleware in main.go injects the user_id as a plain string under
	// the "user_id" key. Parse it back to a UUID here.
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}
