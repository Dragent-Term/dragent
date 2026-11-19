// Package tasks is the organisation's shared record of work to do, kept
// on the server so everyone in it sees, assigns and follows the same
// tasks. The wire shapes here match the desktop's dragent_tasks crate
// exactly (snake_case fields, times as Unix milliseconds); see API.md in
// that crate for the routes and rules.
package tasks

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"

	"server/internal/organization"
)

// Statuses and priorities are the fixed sets the desktop defines; the keys
// below are their wire forms.
const (
	StatusBacklog    = "backlog"
	StatusTodo       = "todo"
	StatusInProgress = "in_progress"
	StatusInReview   = "in_review"
	StatusDone       = "done"
	StatusCanceled   = "canceled"
)

var openStatuses = []string{StatusBacklog, StatusTodo, StatusInProgress, StatusInReview}
var closedStatuses = []string{StatusDone, StatusCanceled}
var allStatuses = []string{StatusBacklog, StatusTodo, StatusInProgress, StatusInReview, StatusDone, StatusCanceled}

const (
	PriorityNone   = "none"
	PriorityUrgent = "urgent"
	PriorityHigh   = "high"
	PriorityMedium = "medium"
	PriorityLow    = "low"
)

// parseStatus reads a status the way the desktop does: `in_progress`,
// `in-progress`, `In progress` and `inprogress` alike.
func parseStatus(raw string) (string, bool) {
	folded := strings.Map(func(r rune) rune {
		if r >= 'A' && r <= 'Z' {
			return r + ('a' - 'A')
		}
		if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') {
			return r
		}
		return -1
	}, strings.TrimSpace(raw))
	for _, s := range allStatuses {
		if strings.ReplaceAll(s, "_", "") == folded {
			return s, true
		}
	}
	return "", false
}

func isOpenStatus(s string) bool {
	return s == StatusBacklog || s == StatusTodo || s == StatusInProgress || s == StatusInReview
}

func parsePriority(raw string) (string, bool) {
	switch key := strings.ToLower(strings.TrimSpace(raw)); key {
	case "", "none", "no priority":
		return PriorityNone, true
	case "1":
		return PriorityUrgent, true
	case "2":
		return PriorityHigh, true
	case "3":
		return PriorityMedium, true
	case "4":
		return PriorityLow, true
	case PriorityUrgent, PriorityHigh, PriorityMedium, PriorityLow:
		return key, true
	}
	return "", false
}

// Person is a member of the organisation as it appears on the wire.
type Person struct {
	ID    string `json:"id"`
	Name  string `json:"name"`
	Email string `json:"email"`
}

// ProjectRef names a repository the same way on every machine.
type ProjectRef struct {
	Key   string `json:"key"`
	Label string `json:"label"`
}

// TaskLink is a workspace, pull request or tracker item linked to a task.
type TaskLink struct {
	ID        string  `json:"id"`
	Kind      string  `json:"kind"`
	Title     string  `json:"title"`
	URL       *string `json:"url"`
	Reference string  `json:"reference"`
	State     *string `json:"state"`
	AddedBy   *Person `json:"added_by"`
	CreatedAt int64   `json:"created_at_ms"`
}

// Task is one task on the wire. AssigneeUserID and friends stay internal;
// only the Person shapes cross it.
type Task struct {
	ID            uuid.UUID
	OrgID         uuid.UUID
	Number        int64
	Key           string
	Title         string
	Description   string
	Status        string
	Priority      string
	Assignee      *Person
	AssigneeID    *uuid.UUID
	Creator       *Person
	CreatorID     *uuid.UUID
	Project       *ProjectRef
	Labels        []string
	DueAtMs       *int64
	CreatedAtMs   int64
	UpdatedAtMs   int64
	StartedAtMs   *int64
	CompletedAtMs *int64
	DeletedAtMs   *int64
	Links         []TaskLinkRow
	CommentCount  int
}

func (t Task) wire() map[string]any {
	labels := t.Labels
	if labels == nil {
		labels = []string{}
	}
	links := make([]TaskLink, 0, len(t.Links))
	for _, l := range t.Links {
		links = append(links, l.wire())
	}
	return map[string]any{
		"id": t.ID.String(), "key": t.Key, "number": t.Number,
		"title": t.Title, "description": t.Description,
		"status": t.Status, "priority": t.Priority,
		"assignee": personOrNil(t.Assignee), "creator": personOrNil(t.Creator),
		"project": projectOrNil(t.Project), "labels": labels,
		"due_at_ms":     intOrNil(t.DueAtMs),
		"created_at_ms": t.CreatedAtMs, "updated_at_ms": t.UpdatedAtMs,
		"started_at_ms": intOrNil(t.StartedAtMs), "completed_at_ms": intOrNil(t.CompletedAtMs),
		"links": links, "comment_count": t.CommentCount,
	}
}

func personOrNil(p *Person) any {
	if p == nil {
		return nil
	}
	return *p
}

func projectOrNil(p *ProjectRef) any {
	if p == nil {
		return nil
	}
	return *p
}

func intOrNil(v *int64) any {
	if v == nil {
		return nil
	}
	return *v
}

// Comment is one task comment on the wire.
type Comment struct {
	ID          uuid.UUID
	OrgID       uuid.UUID
	TaskID      uuid.UUID
	Author      *Person
	AuthorID    *uuid.UUID
	ActorKind   string
	Body        string
	CreatedAtMs int64
	EditedAtMs  *int64
}

func (c Comment) wire() map[string]any {
	return map[string]any{
		"id": c.ID.String(), "task_id": c.TaskID.String(),
		"author": personOrNil(c.Author), "actor_kind": c.ActorKind,
		"body": c.Body, "created_at_ms": c.CreatedAtMs,
		"edited_at_ms": intOrNil(c.EditedAtMs),
	}
}

// Activity is one history entry on the wire. From and To hold the values
// in their wire form (a status key, a person, a link); a description
// change carries none.
type Activity struct {
	ID        uuid.UUID
	OrgID     uuid.UUID
	TaskID    uuid.UUID
	Actor     *Person
	ActorID   *uuid.UUID
	ActorKind string
	Kind      string
	From      any
	To        any
	AtMs      int64
}

func (a Activity) wire() map[string]any {
	return map[string]any{
		"id": a.ID.String(), "task_id": a.TaskID.String(),
		"actor": personOrNil(a.Actor), "actor_kind": a.ActorKind,
		"kind": a.Kind, "from": a.From, "to": a.To, "at_ms": a.AtMs,
	}
}

// Member is an organisation member as the handler needs them: how to show
// them, and whether they may do admin-only things.
type Member struct {
	UserID uuid.UUID
	Person Person
	Role   organization.MembershipRole
}

func (m Member) IsAdmin() bool { return m.Role.AtLeast(organization.RoleAdmin) }

// Repository is what the handler needs stored. Numbering happens inside
// CreateTask's transaction: numbers are given out atomically and never
// reused.
type Repository interface {
	// EnsurePrefix returns the organisation's key prefix, making it once
	// from the organisation's name. It never changes afterwards.
	EnsurePrefix(ctx context.Context, orgID uuid.UUID) (string, error)
	// Member returns nil when the user is not in the organisation.
	Member(ctx context.Context, orgID, userID uuid.UUID) (*Member, error)
	// CreateTask stores t (ID, times and people already set), gives out its
	// number and key, and records the created activity.
	CreateTask(ctx context.Context, t Task, created Activity) (Task, error)
	// GetTask finds a live task by id or key (case-insensitive key). It
	// returns (nil, nil) when there is none, including soft-deleted ones.
	GetTask(ctx context.Context, orgID uuid.UUID, idOrKey string) (*Task, error)
	// GetTaskRaw is GetTask including soft-deleted tasks, for restores.
	GetTaskRaw(ctx context.Context, orgID uuid.UUID, idOrKey string) (*Task, error)
	// UpdateTask writes t's fields and records activities, all in one
	// transaction. It reports ErrNotFound for a missing or deleted task.
	UpdateTask(ctx context.Context, t Task, activities []Activity) error
	// SoftDelete marks the task deleted and records the activity.
	SoftDelete(ctx context.Context, orgID, taskID uuid.UUID, atMs int64, activity Activity) error
	// Restore clears a soft delete and records the activity.
	Restore(ctx context.Context, orgID, taskID uuid.UUID, atMs int64, activity Activity) error
	// ListTasks answers a filtered, most-recently-updated-first page.
	ListTasks(ctx context.Context, orgID uuid.UUID, opts ListOptions) (ListResult, error)
	// TaskLabels lists every label in use on live tasks, sorted.
	TaskLabels(ctx context.Context, orgID uuid.UUID) ([]string, error)
	// TaskComments lists a task's comments, oldest first.
	TaskComments(ctx context.Context, orgID, taskID uuid.UUID) ([]Comment, error)
	CreateComment(ctx context.Context, c Comment) (Comment, error)
	// GetComment returns (nil, nil) when the comment is not on this task.
	GetComment(ctx context.Context, orgID, taskID, commentID uuid.UUID) (*Comment, error)
	UpdateComment(ctx context.Context, orgID, taskID, commentID uuid.UUID, body string, editedAtMs, touchedAtMs int64) error
	DeleteComment(ctx context.Context, orgID, taskID, commentID uuid.UUID, touchedAtMs int64) error
	// TaskActivity lists a task's history, oldest first.
	TaskActivity(ctx context.Context, orgID, taskID uuid.UUID) ([]Activity, error)
	// GetLink returns a task's link by id, or (nil, nil) when missing.
	GetLink(ctx context.Context, orgID, taskID, linkID uuid.UUID) (*TaskLinkRow, error)
	// FindLink returns the task's link with this kind + reference, if any.
	FindLink(ctx context.Context, orgID, taskID uuid.UUID, kind, reference string) (*TaskLinkRow, error)
	CreateLink(ctx context.Context, link TaskLinkRow, activity Activity, touchedAtMs int64) (TaskLinkRow, error)
	UpdateLink(ctx context.Context, orgID, taskID, linkID uuid.UUID, title *string, url, state NullableString, touchedAtMs int64) (*TaskLinkRow, error)
	DeleteLink(ctx context.Context, orgID, taskID, linkID uuid.UUID, activity Activity, touchedAtMs int64) error
}

// TaskLinkRow is a link with the ids the handler needs to assemble it.
type TaskLinkRow struct {
	ID          uuid.UUID
	OrgID       uuid.UUID
	TaskID      uuid.UUID
	Kind        string
	Title       string
	URL         *string
	Reference   string
	State       *string
	AddedBy     *Person
	AddedByID   *uuid.UUID
	CreatedAtMs int64
}

func (l TaskLinkRow) wire() TaskLink {
	return TaskLink{ID: l.ID.String(), Kind: l.Kind, Title: l.Title, URL: l.URL,
		Reference: l.Reference, State: l.State, AddedBy: l.AddedBy, CreatedAt: l.CreatedAtMs}
}

// ListOptions is a parsed GET /tasks query.
type ListOptions struct {
	Statuses []string // empty with neither All nor NoMatch means the default set
	// All disables the default open-only filter.
	All bool
	// NoMatch was an explicit filter that resolves to nothing.
	NoMatch        bool
	AssigneeID     *uuid.UUID
	AssigneeNone   bool
	Label          string
	Project        string
	Words          []string
	UpdatedAfterMs *int64
	Limit          int
	Cursor         *pageCursor
}

// ListResult is one page of tasks.
type ListResult struct {
	Tasks      []Task
	DeletedIDs []string
	NextCursor *string
}

// pageCursor resumes a page: tasks updated before (u, i) in
// (updated_at DESC, id ASC) order.
type pageCursor struct {
	UpdatedMs int64  `json:"u"`
	ID        string `json:"i"`
}

func encodeCursor(c pageCursor) string {
	raw, _ := json.Marshal(c)
	return base64.RawURLEncoding.EncodeToString(raw)
}

func decodeCursor(s string) (pageCursor, error) {
	var c pageCursor
	raw, err := base64.RawURLEncoding.DecodeString(s)
	if err != nil {
		return c, err
	}
	if err := json.Unmarshal(raw, &c); err != nil {
		return c, err
	}
	id, err := uuid.Parse(c.ID)
	if err != nil {
		return c, err
	}
	// Canonical form, so the text comparison in the page query matches
	// the database's own ordering.
	c.ID = id.String()
	return c, nil
}

// ErrNotFound reports a task that is missing, deleted, or in another
// organisation (all three look the same from outside).
var ErrNotFound = errors.New("task not found")

// DerivePrefix makes an organisation's key prefix once, from its name: its
// first three letters A–Z, uppercase; TSK when there are none.
func DerivePrefix(name string) string {
	var letters []rune
	for _, r := range name {
		if (r >= 'A' && r <= 'Z') || (r >= 'a' && r <= 'z') {
			letters = append(letters, r)
			if len(letters) == 3 {
				break
			}
		}
	}
	if len(letters) == 0 {
		return "TSK"
	}
	return strings.ToUpper(string(letters))
}

// NormalizeLabels trims, lowercases, drops empties and duplicates, and
// sorts, so stored labels compare and list deterministically.
func NormalizeLabels(labels []string) []string {
	seen := map[string]bool{}
	out := make([]string, 0, len(labels))
	for _, l := range labels {
		l = strings.ToLower(strings.TrimSpace(l))
		if l == "" || seen[l] {
			continue
		}
		seen[l] = true
		out = append(out, l)
	}
	sort.Strings(out)
	return out
}

func labelsEqual(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func projectsEqual(a, b *ProjectRef) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.Key == b.Key && a.Label == b.Label
}

func int64PtrEqual(a, b *int64) bool {
	if a == nil || b == nil {
		return a == b
	}
	return *a == *b
}

func personPtrEqual(a, b *Person) bool {
	if a == nil || b == nil {
		return a == b
	}
	return a.ID == b.ID
}

// EscapeLike escapes LIKE wildcards so a search word matches literally.
func EscapeLike(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `%`, `\%`)
	s = strings.ReplaceAll(s, `_`, `\_`)
	return s
}

const (
	maxTitleRunes       = 500
	maxDescriptionRunes = 100_000
	maxLabels           = 20
	maxLabelRunes       = 40
	maxCommentRunes     = 20_000
	maxLinkTitleRunes   = 300
	maxReferenceRunes   = 1000
)

func validateTitle(title string) (string, error) {
	title = strings.TrimSpace(title)
	if title == "" {
		return "", errors.New("title is required")
	}
	if utf8.RuneCountInString(title) > maxTitleRunes {
		return "", errors.New("title is too long")
	}
	return title, nil
}

func validateDescription(d string) error {
	if utf8.RuneCountInString(d) > maxDescriptionRunes {
		return errors.New("description is too long")
	}
	return nil
}

func validateLabels(labels []string) ([]string, error) {
	out := NormalizeLabels(labels)
	if len(out) > maxLabels {
		return nil, errors.New("too many labels")
	}
	for _, l := range out {
		if utf8.RuneCountInString(l) > maxLabelRunes {
			return nil, fmt.Errorf("label %q is too long", l)
		}
	}
	return out, nil
}

func validateCommentBody(body string) (string, error) {
	body = strings.TrimSpace(body)
	if body == "" {
		return "", errors.New("body is required")
	}
	if utf8.RuneCountInString(body) > maxCommentRunes {
		return "", errors.New("body is too long")
	}
	return body, nil
}

func validateLinkTitle(title string) (string, error) {
	title = strings.TrimSpace(title)
	if title == "" {
		return "", errors.New("title is required")
	}
	if utf8.RuneCountInString(title) > maxLinkTitleRunes {
		return "", errors.New("title is too long")
	}
	return title, nil
}

func validateReference(ref string) (string, error) {
	ref = strings.TrimSpace(ref)
	if ref == "" {
		return "", errors.New("reference is required")
	}
	if utf8.RuneCountInString(ref) > maxReferenceRunes {
		return "", errors.New("reference is too long")
	}
	return ref, nil
}

func validateURL(raw *string) (*string, error) {
	if raw == nil {
		return nil, nil
	}
	u := strings.TrimSpace(*raw)
	if u == "" {
		return nil, nil
	}
	lower := strings.ToLower(u)
	if !strings.HasPrefix(lower, "http://") && !strings.HasPrefix(lower, "https://") {
		return nil, errors.New("url must start with http:// or https://")
	}
	return &u, nil
}

func validateLinkKind(kind string) (string, error) {
	switch strings.ToLower(strings.TrimSpace(kind)) {
	case "workspace":
		return "workspace", nil
	case "pull_request":
		return "pull_request", nil
	case "item":
		return "item", nil
	}
	return "", errors.New("unknown link kind")
}

// NullableString is a PATCH field that can be left out (Set false),
// cleared (Set, nil value) or set.
type NullableString struct {
	Set   bool
	Value *string
}

func millisToTime(ms int64) time.Time { return time.UnixMilli(ms).UTC() }
