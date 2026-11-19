package tasks

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

type Handler struct {
	repo Repository
	now  func() time.Time
}

func NewHandler(repo Repository) *Handler {
	return &Handler{repo: repo, now: time.Now}
}

// RegisterRoutes mounts the tasks API; `member` must admit only members of
// the organization in the path.
func (h *Handler) RegisterRoutes(r chi.Router, member func(http.Handler) http.Handler) {
	r.With(member).Get("/api/v1/organizations/{orgID}/tasks", h.list)
	r.With(member).Post("/api/v1/organizations/{orgID}/tasks", h.create)
	r.With(member).Get("/api/v1/organizations/{orgID}/task-labels", h.labels)
	r.With(member).Get("/api/v1/organizations/{orgID}/tasks/{task}", h.detail)
	r.With(member).Patch("/api/v1/organizations/{orgID}/tasks/{task}", h.patch)
	r.With(member).Delete("/api/v1/organizations/{orgID}/tasks/{task}", h.remove)
	r.With(member).Post("/api/v1/organizations/{orgID}/tasks/{task}/restore", h.restore)
	r.With(member).Post("/api/v1/organizations/{orgID}/tasks/{task}/comments", h.createComment)
	r.With(member).Patch("/api/v1/organizations/{orgID}/tasks/{task}/comments/{commentID}", h.updateComment)
	r.With(member).Delete("/api/v1/organizations/{orgID}/tasks/{task}/comments/{commentID}", h.deleteComment)
	r.With(member).Post("/api/v1/organizations/{orgID}/tasks/{task}/links", h.createLink)
	r.With(member).Patch("/api/v1/organizations/{orgID}/tasks/{task}/links/{linkID}", h.updateLink)
	r.With(member).Delete("/api/v1/organizations/{orgID}/tasks/{task}/links/{linkID}", h.deleteLink)
}

// restoreWindow is how long a deleted task can come back.
const restoreWindow = 30 * 24 * time.Hour

func (h *Handler) caller(w http.ResponseWriter, r *http.Request) (*Member, uuid.UUID, bool) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return nil, uuid.Nil, false
	}
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return nil, uuid.Nil, false
	}
	m, err := h.repo.Member(r.Context(), orgID, userID)
	if err != nil {
		slog.Error("tasks: member lookup failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the task")
		return nil, uuid.Nil, false
	}
	if m == nil {
		writeError(w, http.StatusForbidden, "not a member of this organization")
		return nil, uuid.Nil, false
	}
	return m, orgID, true
}

func (h *Handler) liveTask(w http.ResponseWriter, r *http.Request, orgID uuid.UUID) (*Task, bool) {
	t, err := h.repo.GetTask(r.Context(), orgID, chi.URLParam(r, "task"))
	if err != nil {
		slog.Error("tasks: get failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the task")
		return nil, false
	}
	if t == nil {
		writeError(w, http.StatusNotFound, "task not found")
		return nil, false
	}
	return t, true
}

func actorKind(byAgent bool) string {
	if byAgent {
		return "agent"
	}
	return "person"
}

func (h *Handler) activity(orgID, taskID uuid.UUID, caller *Member, byAgent bool, kind string, from, to any, atMs int64) Activity {
	return Activity{
		ID: uuid.New(), OrgID: orgID, TaskID: taskID,
		Actor: &caller.Person, ActorID: &caller.UserID, ActorKind: actorKind(byAgent),
		Kind: kind, From: from, To: to, AtMs: atMs,
	}
}

type newTaskRequest struct {
	Title       *string         `json:"title"`
	Description *string         `json:"description"`
	Status      *string         `json:"status"`
	Priority    *string         `json:"priority"`
	AssigneeID  *string         `json:"assignee_id"`
	Project     *projectRequest `json:"project"`
	Labels      []string        `json:"labels"`
	DueAtMs     *int64          `json:"due_at_ms"`
	ByAgent     bool            `json:"by_agent"`
}

type projectRequest struct {
	Key   string `json:"key"`
	Label string `json:"label"`
}

func (h *Handler) create(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	var req newTaskRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Title == nil {
		writeError(w, http.StatusBadRequest, "title is required")
		return
	}
	title, err := validateTitle(*req.Title)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	description := ""
	if req.Description != nil {
		description = *req.Description
	}
	if err := validateDescription(description); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	status := StatusTodo
	if req.Status != nil {
		status, err = parseStatusOr400(w, *req.Status)
		if err != nil {
			return
		}
	}
	priority := PriorityNone
	if req.Priority != nil {
		priority, err = parsePriorityOr400(w, *req.Priority)
		if err != nil {
			return
		}
	}
	assignee, assigneeID, ok := h.resolveAssignee(w, r, orgID, req.AssigneeID)
	if !ok {
		return
	}
	var project *ProjectRef
	if req.Project != nil {
		project, err = parseProjectOr400(w, req.Project)
		if err != nil {
			return
		}
	}
	labels, err := validateLabels(req.Labels)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	now := h.now().UnixMilli()
	t := Task{
		ID: uuid.New(), OrgID: orgID, Title: title, Description: description,
		Status: status, Priority: priority, Assignee: assignee, AssigneeID: assigneeID,
		Creator: &caller.Person, CreatorID: &caller.UserID, Project: project,
		Labels: labels, DueAtMs: req.DueAtMs,
		CreatedAtMs: now, UpdatedAtMs: now, Links: []TaskLinkRow{},
	}
	if status == StatusInProgress || status == StatusInReview {
		t.StartedAtMs = &now
	}
	if status == StatusDone || status == StatusCanceled {
		t.CompletedAtMs = &now
	}
	created, err := h.repo.CreateTask(r.Context(), t, h.activity(orgID, t.ID, caller, req.ByAgent, "created", nil, nil, now))
	if err != nil {
		slog.Error("tasks: create failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not create the task")
		return
	}
	writeJSON(w, http.StatusCreated, created.wire())
}

func parseStatusOr400(w http.ResponseWriter, raw string) (string, error) {
	s, ok := parseStatus(raw)
	if !ok {
		writeError(w, http.StatusBadRequest, "unknown status")
		return "", errors.New("unknown status")
	}
	return s, nil
}

func parsePriorityOr400(w http.ResponseWriter, raw string) (string, error) {
	p, ok := parsePriority(raw)
	if !ok {
		writeError(w, http.StatusBadRequest, "unknown priority")
		return "", errors.New("unknown priority")
	}
	return p, nil
}

func parseProjectOr400(w http.ResponseWriter, req *projectRequest) (*ProjectRef, error) {
	key := strings.TrimSpace(req.Key)
	if key == "" {
		writeError(w, http.StatusBadRequest, "project key is required")
		return nil, errors.New("project key is required")
	}
	return &ProjectRef{Key: key, Label: strings.TrimSpace(req.Label)}, nil
}

// resolveAssignee turns the wire's assignee_id into a person. A nil pointer
// means absent (leave it alone); callers map an explicit null to clearing.
func (h *Handler) resolveAssignee(w http.ResponseWriter, r *http.Request, orgID uuid.UUID, raw *string) (*Person, *uuid.UUID, bool) {
	if raw == nil {
		return nil, nil, true
	}
	id, err := uuid.Parse(strings.TrimSpace(*raw))
	if err != nil {
		writeError(w, http.StatusBadRequest, "unknown assignee")
		return nil, nil, false
	}
	m, err := h.repo.Member(r.Context(), orgID, id)
	if err != nil {
		slog.Error("tasks: member lookup failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the task")
		return nil, nil, false
	}
	if m == nil {
		writeError(w, http.StatusBadRequest, "assignee must be a member of this organization")
		return nil, nil, false
	}
	p := m.Person
	return &p, &id, true
}

func (h *Handler) detail(w http.ResponseWriter, r *http.Request) {
	_, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return
	}
	ctx := r.Context()
	comments, err := h.repo.TaskComments(ctx, orgID, t.ID)
	if err != nil {
		slog.Error("tasks: comments failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the task")
		return
	}
	activity, err := h.repo.TaskActivity(ctx, orgID, t.ID)
	if err != nil {
		slog.Error("tasks: activity failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the task")
		return
	}
	wireComments := make([]any, 0, len(comments))
	for _, c := range comments {
		wireComments = append(wireComments, c.wire())
	}
	wireActivity := make([]any, 0, len(activity))
	for _, a := range activity {
		wireActivity = append(wireActivity, a.wire())
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"task": t.wire(), "comments": wireComments, "activity": wireActivity,
	})
}

func (h *Handler) patch(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	var raw map[string]json.RawMessage
	if err := json.NewDecoder(r.Body).Decode(&raw); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	byAgent := false
	if v, present := raw["by_agent"]; present {
		if err := json.Unmarshal(v, &byAgent); err != nil {
			writeError(w, http.StatusBadRequest, "invalid request body")
			return
		}
	}
	now := h.now().UnixMilli()
	var activities []Activity
	add := func(kind string, from, to any) {
		activities = append(activities, h.activity(orgID, t.ID, caller, byAgent, kind, from, to, now))
	}
	if v, present := raw["title"]; present && string(v) != "null" {
		var title string
		if err := json.Unmarshal(v, &title); err != nil {
			writeError(w, http.StatusBadRequest, "invalid title")
			return
		}
		title, err := validateTitle(title)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		if title != t.Title {
			add("title", t.Title, title)
			t.Title = title
		}
	}
	if v, present := raw["description"]; present && string(v) != "null" {
		var description string
		if err := json.Unmarshal(v, &description); err != nil {
			writeError(w, http.StatusBadRequest, "invalid description")
			return
		}
		if err := validateDescription(description); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		if description != t.Description {
			// A description change carries no values, only the fact.
			add("description", nil, nil)
			t.Description = description
		}
	}
	if v, present := raw["status"]; present && string(v) != "null" {
		var s string
		if err := json.Unmarshal(v, &s); err != nil {
			writeError(w, http.StatusBadRequest, "invalid status")
			return
		}
		status, err := parseStatusOr400(w, s)
		if err != nil {
			return
		}
		if status != t.Status {
			add("status", t.Status, status)
			t.Status = status
			if (status == StatusInProgress || status == StatusInReview) && t.StartedAtMs == nil {
				t.StartedAtMs = &now
			}
			if status == StatusDone || status == StatusCanceled {
				t.CompletedAtMs = &now
			} else {
				t.CompletedAtMs = nil
			}
		}
	}
	if v, present := raw["priority"]; present && string(v) != "null" {
		var p string
		if err := json.Unmarshal(v, &p); err != nil {
			writeError(w, http.StatusBadRequest, "invalid priority")
			return
		}
		priority, err := parsePriorityOr400(w, p)
		if err != nil {
			return
		}
		if priority != t.Priority {
			add("priority", t.Priority, priority)
			t.Priority = priority
		}
	}
	if v, present := raw["assignee_id"]; present {
		if string(v) == "null" {
			if t.Assignee != nil {
				from := *t.Assignee
				add("assignee", from, nil)
				t.Assignee, t.AssigneeID = nil, nil
			}
		} else {
			var idRaw string
			if err := json.Unmarshal(v, &idRaw); err != nil {
				writeError(w, http.StatusBadRequest, "invalid assignee")
				return
			}
			assignee, assigneeID, ok := h.resolveAssignee(w, r, orgID, &idRaw)
			if !ok {
				return
			}
			if !personPtrEqual(t.Assignee, assignee) {
				var from any
				if t.Assignee != nil {
					from = *t.Assignee
				}
				add("assignee", from, *assignee)
				t.Assignee, t.AssigneeID = assignee, assigneeID
			}
		}
	}
	if v, present := raw["project"]; present {
		if string(v) == "null" {
			if t.Project != nil {
				from := *t.Project
				add("project", from, nil)
				t.Project = nil
			}
		} else {
			var req projectRequest
			if err := json.Unmarshal(v, &req); err != nil {
				writeError(w, http.StatusBadRequest, "invalid project")
				return
			}
			project, err := parseProjectOr400(w, &req)
			if err != nil {
				return
			}
			if !projectsEqual(t.Project, project) {
				var from any
				if t.Project != nil {
					from = *t.Project
				}
				add("project", from, *project)
				t.Project = project
			}
		}
	}
	if v, present := raw["labels"]; present && string(v) != "null" {
		var labels []string
		if err := json.Unmarshal(v, &labels); err != nil {
			writeError(w, http.StatusBadRequest, "invalid labels")
			return
		}
		labels, err := validateLabels(labels)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		if !labelsEqual(t.Labels, labels) {
			from := append([]string{}, t.Labels...)
			add("labels", from, append([]string{}, labels...))
			t.Labels = labels
		}
	}
	if v, present := raw["due_at_ms"]; present {
		if string(v) == "null" {
			if t.DueAtMs != nil {
				from := *t.DueAtMs
				add("due", from, nil)
				t.DueAtMs = nil
			}
		} else {
			var due int64
			if err := json.Unmarshal(v, &due); err != nil {
				writeError(w, http.StatusBadRequest, "invalid due date")
				return
			}
			if !int64PtrEqual(t.DueAtMs, &due) {
				var from any
				if t.DueAtMs != nil {
					from = *t.DueAtMs
				}
				add("due", from, due)
				t.DueAtMs = &due
			}
		}
	}
	if len(activities) == 0 {
		writeJSON(w, http.StatusOK, t.wire())
		return
	}
	t.UpdatedAtMs = now
	if err := h.repo.UpdateTask(r.Context(), *t, activities); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "task not found")
			return
		}
		slog.Error("tasks: update failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not update the task")
		return
	}
	writeJSON(w, http.StatusOK, t.wire())
}

func (h *Handler) remove(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return
	}
	if t.CreatorID == nil || *t.CreatorID != caller.UserID {
		if !caller.IsAdmin() {
			writeError(w, http.StatusForbidden, "only the creator or an admin can delete this task")
			return
		}
	}
	now := h.now().UnixMilli()
	if err := h.repo.SoftDelete(r.Context(), orgID, t.ID, now,
		h.activity(orgID, t.ID, caller, false, "deleted", nil, nil, now)); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "task not found")
			return
		}
		slog.Error("tasks: delete failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not delete the task")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) restore(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, err := h.repo.GetTaskRaw(r.Context(), orgID, chi.URLParam(r, "task"))
	if err != nil {
		slog.Error("tasks: get failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the task")
		return
	}
	if t == nil || t.DeletedAtMs == nil {
		if t == nil {
			writeError(w, http.StatusNotFound, "task not found")
			return
		}
		writeJSON(w, http.StatusOK, t.wire())
		return
	}
	now := h.now().UnixMilli()
	if now-*t.DeletedAtMs > restoreWindow.Milliseconds() {
		writeError(w, http.StatusGone, "this task was deleted more than 30 days ago")
		return
	}
	if err := h.repo.Restore(r.Context(), orgID, t.ID, now,
		h.activity(orgID, t.ID, caller, false, "restored", nil, nil, now)); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "task not found")
			return
		}
		slog.Error("tasks: restore failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not restore the task")
		return
	}
	t.DeletedAtMs = nil
	t.UpdatedAtMs = now
	writeJSON(w, http.StatusOK, t.wire())
}

func (h *Handler) list(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	q := r.URL.Query()
	var opts ListOptions
	if raw := strings.TrimSpace(q.Get("status")); raw != "" {
		switch strings.ToLower(raw) {
		case "open":
			opts.Statuses = append([]string{}, openStatuses...)
		case "closed":
			opts.Statuses = append([]string{}, closedStatuses...)
		case "all":
			opts.All = true
		default:
			var set []string
			for _, part := range strings.Split(raw, ",") {
				if s, ok := parseStatus(part); ok {
					set = append(set, s)
				}
			}
			if len(set) == 0 {
				opts.NoMatch = true
			} else {
				opts.Statuses = set
			}
		}
	} else if strings.TrimSpace(q.Get("updated_after_ms")) != "" {
		// An incremental refresh wants every status; deletions come
		// back separately in deleted_ids.
		opts.All = true
	} else {
		opts.Statuses = append([]string{}, openStatuses...)
	}
	if raw := strings.TrimSpace(q.Get("assignee")); raw != "" {
		switch strings.ToLower(raw) {
		case "me":
			id := caller.UserID
			opts.AssigneeID = &id
		case "none":
			opts.AssigneeNone = true
		default:
			id, err := uuid.Parse(raw)
			if err != nil {
				writeError(w, http.StatusBadRequest, "unknown assignee")
				return
			}
			opts.AssigneeID = &id
		}
	}
	if raw := strings.TrimSpace(q.Get("label")); raw != "" {
		opts.Label = strings.ToLower(raw)
	}
	if raw := strings.TrimSpace(q.Get("project")); raw != "" {
		opts.Project = raw
	}
	if raw := strings.TrimSpace(q.Get("q")); raw != "" {
		opts.Words = strings.Fields(raw)
	}
	if raw := strings.TrimSpace(q.Get("updated_after_ms")); raw != "" {
		ms, err := strconv.ParseInt(raw, 10, 64)
		if err != nil {
			writeError(w, http.StatusBadRequest, "invalid updated_after_ms")
			return
		}
		opts.UpdatedAfterMs = &ms
	}
	limit := 200
	if raw := strings.TrimSpace(q.Get("limit")); raw != "" {
		n, err := strconv.Atoi(raw)
		if err != nil || n < 1 {
			writeError(w, http.StatusBadRequest, "invalid limit")
			return
		}
		limit = n
		if limit > 500 {
			limit = 500
		}
	}
	opts.Limit = limit
	if raw := strings.TrimSpace(q.Get("cursor")); raw != "" {
		c, err := decodeCursor(raw)
		if err != nil {
			writeError(w, http.StatusBadRequest, "invalid cursor")
			return
		}
		opts.Cursor = &c
	}
	res, err := h.repo.ListTasks(r.Context(), orgID, opts)
	if err != nil {
		slog.Error("tasks: list failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load tasks")
		return
	}
	prefix, err := h.repo.EnsurePrefix(r.Context(), orgID)
	if err != nil {
		slog.Error("tasks: prefix failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load tasks")
		return
	}
	wireTasks := make([]any, 0, len(res.Tasks))
	for _, t := range res.Tasks {
		wireTasks = append(wireTasks, t.wire())
	}
	var nextCursor any
	if res.NextCursor != nil {
		nextCursor = *res.NextCursor
	}
	if res.DeletedIDs == nil {
		res.DeletedIDs = []string{}
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"tasks": wireTasks, "deleted_ids": res.DeletedIDs,
		"next_cursor": nextCursor, "prefix": prefix,
	})
}

func (h *Handler) labels(w http.ResponseWriter, r *http.Request) {
	_, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	labels, err := h.repo.TaskLabels(r.Context(), orgID)
	if err != nil {
		slog.Error("tasks: labels failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load labels")
		return
	}
	if labels == nil {
		labels = []string{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"labels": labels})
}

type newCommentRequest struct {
	Body    *string `json:"body"`
	ByAgent bool    `json:"by_agent"`
}

func (h *Handler) createComment(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	var req newCommentRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Body == nil {
		writeError(w, http.StatusBadRequest, "body is required")
		return
	}
	body, err := validateCommentBody(*req.Body)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	now := h.now().UnixMilli()
	c := Comment{
		ID: uuid.New(), OrgID: orgID, TaskID: t.ID,
		Author: &caller.Person, AuthorID: &caller.UserID,
		ActorKind: actorKind(req.ByAgent), Body: body, CreatedAtMs: now,
	}
	created, err := h.repo.CreateComment(r.Context(), c)
	if err != nil {
		slog.Error("tasks: comment failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not add the comment")
		return
	}
	writeJSON(w, http.StatusCreated, created.wire())
}

func (h *Handler) commentTarget(w http.ResponseWriter, r *http.Request, orgID uuid.UUID) (*Task, *Comment, bool) {
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return nil, nil, false
	}
	commentID, err := uuid.Parse(chi.URLParam(r, "commentID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid comment id")
		return nil, nil, false
	}
	c, err := h.repo.GetComment(r.Context(), orgID, t.ID, commentID)
	if err != nil {
		slog.Error("tasks: comment get failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the comment")
		return nil, nil, false
	}
	if c == nil {
		writeError(w, http.StatusNotFound, "comment not found")
		return nil, nil, false
	}
	return t, c, true
}

func (h *Handler) updateComment(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	_, c, ok := h.commentTarget(w, r, orgID)
	if !ok {
		return
	}
	if c.AuthorID == nil || *c.AuthorID != caller.UserID {
		writeError(w, http.StatusForbidden, "only the author can edit this comment")
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	var req struct {
		Body *string `json:"body"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Body == nil {
		writeError(w, http.StatusBadRequest, "body is required")
		return
	}
	body, err := validateCommentBody(*req.Body)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	now := h.now().UnixMilli()
	if err := h.repo.UpdateComment(r.Context(), orgID, c.TaskID, c.ID, body, now, now); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "comment not found")
			return
		}
		slog.Error("tasks: comment update failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not update the comment")
		return
	}
	c.Body = body
	c.EditedAtMs = &now
	writeJSON(w, http.StatusOK, c.wire())
}

func (h *Handler) deleteComment(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	_, c, ok := h.commentTarget(w, r, orgID)
	if !ok {
		return
	}
	mine := c.AuthorID != nil && *c.AuthorID == caller.UserID
	if !mine && !caller.IsAdmin() {
		writeError(w, http.StatusForbidden, "only the author or an admin can delete this comment")
		return
	}
	if err := h.repo.DeleteComment(r.Context(), orgID, c.TaskID, c.ID, h.now().UnixMilli()); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "comment not found")
			return
		}
		slog.Error("tasks: comment delete failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not delete the comment")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

type newLinkRequest struct {
	Kind      *string `json:"kind"`
	Title     *string `json:"title"`
	URL       *string `json:"url"`
	Reference *string `json:"reference"`
	State     *string `json:"state"`
	ByAgent   bool    `json:"by_agent"`
}

func (h *Handler) createLink(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	var req newLinkRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Kind == nil || req.Title == nil || req.Reference == nil {
		writeError(w, http.StatusBadRequest, "kind, title and reference are required")
		return
	}
	kind, err := validateLinkKind(*req.Kind)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	title, err := validateLinkTitle(*req.Title)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	reference, err := validateReference(*req.Reference)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	url, err := validateURL(req.URL)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	var state *string
	if req.State != nil {
		s := strings.TrimSpace(*req.State)
		if s != "" {
			state = &s
		}
	}
	now := h.now().UnixMilli()
	existing, err := h.repo.FindLink(r.Context(), orgID, t.ID, kind, reference)
	if err != nil {
		slog.Error("tasks: link lookup failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not save the link")
		return
	}
	if existing != nil {
		// The same kind + reference links once: refresh it in place,
		// with no history entry.
		updated, err := h.repo.UpdateLink(r.Context(), orgID, t.ID, existing.ID,
			&title, NullableString{Set: true, Value: url}, NullableString{Set: true, Value: state}, now)
		if err != nil {
			if errors.Is(err, ErrNotFound) {
				writeError(w, http.StatusNotFound, "link not found")
				return
			}
			slog.Error("tasks: link update failed", "organization_id", orgID, "error", err)
			writeError(w, http.StatusInternalServerError, "could not save the link")
			return
		}
		writeJSON(w, http.StatusOK, updated.wire())
		return
	}
	row := TaskLinkRow{
		ID: uuid.New(), OrgID: orgID, TaskID: t.ID, Kind: kind, Title: title,
		URL: url, Reference: reference, State: state,
		AddedBy: &caller.Person, AddedByID: &caller.UserID, CreatedAtMs: now,
	}
	created, err := h.repo.CreateLink(r.Context(), row,
		h.activity(orgID, t.ID, caller, req.ByAgent, "link_added", nil, row.wire(), now), now)
	if err != nil {
		slog.Error("tasks: link create failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not save the link")
		return
	}
	created.AddedBy = &caller.Person
	writeJSON(w, http.StatusCreated, created.wire())
}

func (h *Handler) linkTarget(w http.ResponseWriter, r *http.Request, orgID uuid.UUID) (*Task, *TaskLinkRow, bool) {
	t, ok := h.liveTask(w, r, orgID)
	if !ok {
		return nil, nil, false
	}
	linkID, err := uuid.Parse(chi.URLParam(r, "linkID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid link id")
		return nil, nil, false
	}
	l, err := h.repo.GetLink(r.Context(), orgID, t.ID, linkID)
	if err != nil {
		slog.Error("tasks: link get failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not load the link")
		return nil, nil, false
	}
	if l == nil {
		writeError(w, http.StatusNotFound, "link not found")
		return nil, nil, false
	}
	return t, l, true
}

func (h *Handler) updateLink(w http.ResponseWriter, r *http.Request) {
	_, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, l, ok := h.linkTarget(w, r, orgID)
	if !ok {
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 1<<20)
	var raw map[string]json.RawMessage
	if err := json.NewDecoder(r.Body).Decode(&raw); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	var title *string
	if v, present := raw["title"]; present && string(v) != "null" {
		var s string
		if err := json.Unmarshal(v, &s); err != nil {
			writeError(w, http.StatusBadRequest, "invalid title")
			return
		}
		validated, err := validateLinkTitle(s)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		title = &validated
	}
	url := NullableString{}
	if v, present := raw["url"]; present {
		url.Set = true
		if string(v) != "null" {
			var s string
			if err := json.Unmarshal(v, &s); err != nil {
				writeError(w, http.StatusBadRequest, "invalid url")
				return
			}
			validated, err := validateURL(&s)
			if err != nil {
				writeError(w, http.StatusBadRequest, err.Error())
				return
			}
			url.Value = validated
		}
	}
	state := NullableString{}
	if v, present := raw["state"]; present {
		state.Set = true
		if string(v) != "null" {
			var s string
			if err := json.Unmarshal(v, &s); err != nil {
				writeError(w, http.StatusBadRequest, "invalid state")
				return
			}
			s = strings.TrimSpace(s)
			if s != "" {
				state.Value = &s
			}
		}
	}
	updated, err := h.repo.UpdateLink(r.Context(), orgID, t.ID, l.ID, title, url, state, h.now().UnixMilli())
	if err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "link not found")
			return
		}
		slog.Error("tasks: link update failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not update the link")
		return
	}
	writeJSON(w, http.StatusOK, updated.wire())
}

func (h *Handler) deleteLink(w http.ResponseWriter, r *http.Request) {
	caller, orgID, ok := h.caller(w, r)
	if !ok {
		return
	}
	t, l, ok := h.linkTarget(w, r, orgID)
	if !ok {
		return
	}
	now := h.now().UnixMilli()
	if err := h.repo.DeleteLink(r.Context(), orgID, t.ID, l.ID,
		h.activity(orgID, t.ID, caller, false, "link_removed", l.wire(), nil, now), now); err != nil {
		if errors.Is(err, ErrNotFound) {
			writeError(w, http.StatusNotFound, "link not found")
			return
		}
		slog.Error("tasks: link delete failed", "organization_id", orgID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not delete the link")
		return
	}
	w.WriteHeader(http.StatusNoContent)
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
