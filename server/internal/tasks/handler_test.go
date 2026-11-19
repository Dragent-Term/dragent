package tasks

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/organization"
)

// fakeRepo mirrors the Postgres semantics in memory: per-organisation
// numbering, soft deletes, stable page order, touches on every change.
type fakeRepo struct {
	mu       sync.Mutex
	orgNames map[uuid.UUID]string
	prefixes map[uuid.UUID]string
	counters map[uuid.UUID]int64
	members  map[uuid.UUID]map[uuid.UUID]*Member
	tasks    map[uuid.UUID]*Task
	comments map[uuid.UUID][]Comment
	activity map[uuid.UUID][]Activity
	links    map[uuid.UUID][]TaskLinkRow
}

func newFakeRepo() *fakeRepo {
	return &fakeRepo{
		orgNames: map[uuid.UUID]string{},
		prefixes: map[uuid.UUID]string{},
		counters: map[uuid.UUID]int64{},
		members:  map[uuid.UUID]map[uuid.UUID]*Member{},
		tasks:    map[uuid.UUID]*Task{},
		comments: map[uuid.UUID][]Comment{},
		activity: map[uuid.UUID][]Activity{},
		links:    map[uuid.UUID][]TaskLinkRow{},
	}
}

func (f *fakeRepo) addMember(org, user uuid.UUID, name, email string, role organization.MembershipRole) *Member {
	m := &Member{UserID: user, Person: Person{ID: user.String(), Name: name, Email: email}, Role: role}
	if f.members[org] == nil {
		f.members[org] = map[uuid.UUID]*Member{}
	}
	f.members[org][user] = m
	return m
}

func (f *fakeRepo) EnsurePrefix(_ context.Context, orgID uuid.UUID) (string, error) {
	if p, ok := f.prefixes[orgID]; ok {
		return p, nil
	}
	p := DerivePrefix(f.orgNames[orgID])
	f.prefixes[orgID] = p
	if _, ok := f.counters[orgID]; !ok {
		f.counters[orgID] = 1
	}
	return p, nil
}

func (f *fakeRepo) Member(_ context.Context, orgID, userID uuid.UUID) (*Member, error) {
	return f.members[orgID][userID], nil
}

func cloneTask(t Task) *Task {
	c := t
	c.Labels = append([]string{}, t.Labels...)
	c.Links = append([]TaskLinkRow{}, t.Links...)
	return &c
}

func (f *fakeRepo) CreateTask(_ context.Context, t Task, created Activity) (Task, error) {
	prefix, _ := f.EnsurePrefix(context.Background(), t.OrgID)
	t.Number = f.counters[t.OrgID]
	f.counters[t.OrgID]++
	t.Key = prefix + "-" + itoa(t.Number)
	if t.Labels == nil {
		t.Labels = []string{}
	}
	t.Links = []TaskLinkRow{}
	f.tasks[t.ID] = cloneTask(t)
	f.activity[t.ID] = append(f.activity[t.ID], created)
	return t, nil
}

func itoa(n int64) string { return strconv.FormatInt(n, 10) }

func (f *fakeRepo) find(orgID uuid.UUID, idOrKey string, includeDeleted bool) *Task {
	if id, err := uuid.Parse(idOrKey); err == nil {
		if t, ok := f.tasks[id]; ok && t.OrgID == orgID && (includeDeleted || t.DeletedAtMs == nil) {
			return t
		}
		return nil
	}
	for _, t := range f.tasks {
		if t.OrgID == orgID && strings.EqualFold(t.Key, idOrKey) && (includeDeleted || t.DeletedAtMs == nil) {
			return t
		}
	}
	return nil
}

func (f *fakeRepo) assemble(t *Task) *Task {
	c := cloneTask(*t)
	c.Links = append([]TaskLinkRow{}, f.links[t.ID]...)
	c.CommentCount = len(f.comments[t.ID])
	return c
}

func (f *fakeRepo) GetTask(_ context.Context, orgID uuid.UUID, idOrKey string) (*Task, error) {
	if t := f.find(orgID, idOrKey, false); t != nil {
		return f.assemble(t), nil
	}
	return nil, nil
}

func (f *fakeRepo) GetTaskRaw(_ context.Context, orgID uuid.UUID, idOrKey string) (*Task, error) {
	if t := f.find(orgID, idOrKey, true); t != nil {
		return f.assemble(t), nil
	}
	return nil, nil
}

func (f *fakeRepo) UpdateTask(_ context.Context, t Task, activities []Activity) error {
	cur := f.tasks[t.ID]
	if cur == nil || cur.OrgID != t.OrgID || cur.DeletedAtMs != nil {
		return ErrNotFound
	}
	t.Links = cur.Links
	f.tasks[t.ID] = cloneTask(t)
	f.activity[t.ID] = append(f.activity[t.ID], activities...)
	return nil
}

func (f *fakeRepo) SoftDelete(_ context.Context, orgID, taskID uuid.UUID, atMs int64, activity Activity) error {
	t := f.tasks[taskID]
	if t == nil || t.OrgID != orgID || t.DeletedAtMs != nil {
		return ErrNotFound
	}
	t.DeletedAtMs = &atMs
	t.UpdatedAtMs = atMs
	f.activity[taskID] = append(f.activity[taskID], activity)
	return nil
}

func (f *fakeRepo) Restore(_ context.Context, orgID, taskID uuid.UUID, atMs int64, activity Activity) error {
	t := f.tasks[taskID]
	if t == nil || t.OrgID != orgID || t.DeletedAtMs == nil {
		return ErrNotFound
	}
	t.DeletedAtMs = nil
	t.UpdatedAtMs = atMs
	f.activity[taskID] = append(f.activity[taskID], activity)
	return nil
}

func (f *fakeRepo) ListTasks(_ context.Context, orgID uuid.UUID, opts ListOptions) (ListResult, error) {
	var out ListResult
	out.DeletedIDs = []string{}
	if opts.NoMatch {
		out.Tasks = []Task{}
		return out, nil
	}
	for _, t := range f.tasks {
		if t.OrgID != orgID || t.DeletedAtMs != nil {
			continue
		}
		if !opts.All && len(opts.Statuses) > 0 && !contains(opts.Statuses, t.Status) {
			continue
		}
		if opts.AssigneeNone {
			if t.AssigneeID != nil {
				continue
			}
		} else if opts.AssigneeID != nil {
			if t.AssigneeID == nil || *t.AssigneeID != *opts.AssigneeID {
				continue
			}
		}
		if opts.Label != "" && !contains(t.Labels, opts.Label) {
			continue
		}
		if opts.Project != "" && (t.Project == nil || t.Project.Key != opts.Project) {
			continue
		}
		if !matchWords(t, opts.Words) {
			continue
		}
		if opts.UpdatedAfterMs != nil && t.UpdatedAtMs <= *opts.UpdatedAfterMs {
			continue
		}
		if opts.Cursor != nil {
			if t.UpdatedAtMs > opts.Cursor.UpdatedMs {
				continue
			}
			if t.UpdatedAtMs == opts.Cursor.UpdatedMs && t.ID.String() <= opts.Cursor.ID {
				continue
			}
		}
		out.Tasks = append(out.Tasks, *f.assemble(t))
	}
	sort.Slice(out.Tasks, func(i, j int) bool {
		if out.Tasks[i].UpdatedAtMs != out.Tasks[j].UpdatedAtMs {
			return out.Tasks[i].UpdatedAtMs > out.Tasks[j].UpdatedAtMs
		}
		return out.Tasks[i].ID.String() < out.Tasks[j].ID.String()
	})
	limit := opts.Limit
	if limit <= 0 {
		limit = 200
	}
	if limit > 500 {
		limit = 500
	}
	if len(out.Tasks) > limit {
		out.Tasks = out.Tasks[:limit]
		last := out.Tasks[len(out.Tasks)-1]
		c := encodeCursor(pageCursor{UpdatedMs: last.UpdatedAtMs, ID: last.ID.String()})
		out.NextCursor = &c
	}
	if out.Tasks == nil {
		out.Tasks = []Task{}
	}
	if opts.UpdatedAfterMs != nil {
		var deleted []*Task
		for _, t := range f.tasks {
			if t.OrgID == orgID && t.DeletedAtMs != nil && t.UpdatedAtMs > *opts.UpdatedAfterMs {
				deleted = append(deleted, t)
			}
		}
		sort.Slice(deleted, func(i, j int) bool { return deleted[i].UpdatedAtMs > deleted[j].UpdatedAtMs })
		for _, t := range deleted {
			out.DeletedIDs = append(out.DeletedIDs, t.ID.String())
		}
	}
	return out, nil
}

func contains(set []string, s string) bool {
	for _, v := range set {
		if v == s {
			return true
		}
	}
	return false
}

func matchWords(t *Task, words []string) bool {
	hay := strings.ToLower(t.Key + "\n" + t.Title + "\n" + t.Description)
	for _, w := range words {
		if !strings.Contains(hay, strings.ToLower(w)) {
			return false
		}
	}
	return true
}

func (f *fakeRepo) TaskLabels(_ context.Context, orgID uuid.UUID) ([]string, error) {
	seen := map[string]bool{}
	var out []string
	for _, t := range f.tasks {
		if t.OrgID != orgID || t.DeletedAtMs != nil {
			continue
		}
		for _, l := range t.Labels {
			if !seen[l] {
				seen[l] = true
				out = append(out, l)
			}
		}
	}
	sort.Strings(out)
	if out == nil {
		out = []string{}
	}
	return out, nil
}

func (f *fakeRepo) TaskComments(_ context.Context, _, taskID uuid.UUID) ([]Comment, error) {
	out := append([]Comment{}, f.comments[taskID]...)
	if out == nil {
		out = []Comment{}
	}
	return out, nil
}

func (f *fakeRepo) CreateComment(_ context.Context, c Comment) (Comment, error) {
	f.comments[c.TaskID] = append(f.comments[c.TaskID], c)
	f.tasks[c.TaskID].UpdatedAtMs = c.CreatedAtMs
	return c, nil
}

func (f *fakeRepo) GetComment(_ context.Context, _, taskID, commentID uuid.UUID) (*Comment, error) {
	for _, c := range f.comments[taskID] {
		if c.ID == commentID {
			c := c
			return &c, nil
		}
	}
	return nil, nil
}

func (f *fakeRepo) UpdateComment(_ context.Context, _, taskID, commentID uuid.UUID, body string, editedAtMs, touchedAtMs int64) error {
	for i, c := range f.comments[taskID] {
		if c.ID == commentID {
			f.comments[taskID][i].Body = body
			f.comments[taskID][i].EditedAtMs = &editedAtMs
			f.tasks[taskID].UpdatedAtMs = touchedAtMs
			return nil
		}
	}
	return ErrNotFound
}

func (f *fakeRepo) DeleteComment(_ context.Context, _, taskID, commentID uuid.UUID, touchedAtMs int64) error {
	for i, c := range f.comments[taskID] {
		if c.ID == commentID {
			f.comments[taskID] = append(f.comments[taskID][:i], f.comments[taskID][i+1:]...)
			f.tasks[taskID].UpdatedAtMs = touchedAtMs
			return nil
		}
	}
	return ErrNotFound
}

func (f *fakeRepo) TaskActivity(_ context.Context, _, taskID uuid.UUID) ([]Activity, error) {
	out := append([]Activity{}, f.activity[taskID]...)
	if out == nil {
		out = []Activity{}
	}
	return out, nil
}

func (f *fakeRepo) GetLink(_ context.Context, _, taskID, linkID uuid.UUID) (*TaskLinkRow, error) {
	for _, l := range f.links[taskID] {
		if l.ID == linkID {
			l := l
			return &l, nil
		}
	}
	return nil, nil
}

func (f *fakeRepo) FindLink(_ context.Context, _, taskID uuid.UUID, kind, reference string) (*TaskLinkRow, error) {
	for _, l := range f.links[taskID] {
		if l.Kind == kind && l.Reference == reference {
			l := l
			return &l, nil
		}
	}
	return nil, nil
}

func (f *fakeRepo) CreateLink(_ context.Context, link TaskLinkRow, activity Activity, touchedAtMs int64) (TaskLinkRow, error) {
	f.links[link.TaskID] = append(f.links[link.TaskID], link)
	f.activity[link.TaskID] = append(f.activity[link.TaskID], activity)
	f.tasks[link.TaskID].UpdatedAtMs = touchedAtMs
	return link, nil
}

func (f *fakeRepo) UpdateLink(_ context.Context, orgID, taskID, linkID uuid.UUID, title *string, url, state NullableString, touchedAtMs int64) (*TaskLinkRow, error) {
	for i, l := range f.links[taskID] {
		if l.ID == linkID && l.OrgID == orgID {
			if title != nil {
				l.Title = *title
			}
			if url.Set {
				l.URL = url.Value
			}
			if state.Set {
				l.State = state.Value
			}
			f.links[taskID][i] = l
			f.tasks[taskID].UpdatedAtMs = touchedAtMs
			updated := l
			return &updated, nil
		}
	}
	return nil, ErrNotFound
}

func (f *fakeRepo) DeleteLink(_ context.Context, _, taskID, linkID uuid.UUID, activity Activity, touchedAtMs int64) error {
	for i, l := range f.links[taskID] {
		if l.ID == linkID {
			f.links[taskID] = append(f.links[taskID][:i], f.links[taskID][i+1:]...)
			f.activity[taskID] = append(f.activity[taskID], activity)
			f.tasks[taskID].UpdatedAtMs = touchedAtMs
			return nil
		}
	}
	return ErrNotFound
}

// ── HTTP harness ────────────────────────────────────────────────────────────

type fixture struct {
	repo *fakeRepo
	h    *Handler
	org  uuid.UUID
	me   uuid.UUID
	now  int64
}

func newFixture(t *testing.T, orgName string) *fixture {
	t.Helper()
	repo := newFakeRepo()
	org, me := uuid.New(), uuid.New()
	repo.orgNames[org] = orgName
	repo.addMember(org, me, "Ada", "ada@example.com", organization.RoleMember)
	fx := &fixture{repo: repo, org: org, me: me, now: 1_700_000_000_000}
	fx.h = NewHandler(repo)
	fx.h.now = func() time.Time { return time.UnixMilli(fx.now) }
	return fx
}

func (fx *fixture) advance(ms int64) { fx.now += ms }

func (fx *fixture) router() http.Handler {
	r := chi.NewRouter()
	fx.h.RegisterRoutes(r, func(next http.Handler) http.Handler { return next })
	return r
}

func (fx *fixture) call(t *testing.T, user uuid.UUID, method, path string, body any) *httptest.ResponseRecorder {
	t.Helper()
	var buf bytes.Buffer
	if body != nil {
		if err := json.NewEncoder(&buf).Encode(body); err != nil {
			t.Fatal(err)
		}
	}
	req := httptest.NewRequest(method, path, &buf)
	if user != uuid.Nil {
		req = req.WithContext(context.WithValue(req.Context(), "user_id", user.String()))
	}
	rec := httptest.NewRecorder()
	fx.router().ServeHTTP(rec, req)
	return rec
}

func (fx *fixture) tasksPath() string {
	return "/api/v1/organizations/" + fx.org.String() + "/tasks"
}

func decode(t *testing.T, rec *httptest.ResponseRecorder) map[string]any {
	t.Helper()
	var v map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &v); err != nil {
		t.Fatalf("bad json %q: %v", rec.Body.String(), err)
	}
	return v
}

func (fx *fixture) createTask(t *testing.T, user uuid.UUID, body map[string]any) map[string]any {
	t.Helper()
	rec := fx.call(t, user, http.MethodPost, fx.tasksPath(), body)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create: want 201, got %d %s", rec.Code, rec.Body.String())
	}
	return decode(t, rec)
}

func TestCreateNumbersPrefix(t *testing.T) {
	fx := newFixture(t, "Dragent")
	a := fx.createTask(t, fx.me, map[string]any{"title": "First"})
	if a["key"] != "DRA-1" || a["number"] != float64(1) {
		t.Fatalf("first task: %v", a)
	}
	if a["status"] != "todo" || a["priority"] != "none" {
		t.Fatalf("defaults: %v", a)
	}
	if a["assignee"] != nil || a["creator"] == nil {
		t.Fatalf("people: %v", a)
	}
	creator := a["creator"].(map[string]any)
	if creator["id"] != fx.me.String() || creator["name"] != "Ada" || creator["email"] != "ada@example.com" {
		t.Fatalf("creator: %v", creator)
	}
	fx.advance(1)
	b := fx.createTask(t, fx.me, map[string]any{"title": "Second"})
	if b["key"] != "DRA-2" {
		t.Fatalf("second task: %v", b)
	}
	// Renaming the organisation never changes the prefix.
	fx.repo.orgNames[fx.org] = "Renamed Inc"
	fx.advance(1)
	c := fx.createTask(t, fx.me, map[string]any{"title": "Third"})
	if c["key"] != "DRA-3" {
		t.Fatalf("prefix changed: %v", c)
	}
	// Another organisation counts from its own prefix.
	other := uuid.New()
	fx.repo.orgNames[other] = "Acme Corporation"
	fx.repo.addMember(other, fx.me, "Ada", "ada@example.com", organization.RoleMember)
	opath := "/api/v1/organizations/" + other.String() + "/tasks"
	rec := fx.call(t, fx.me, http.MethodPost, opath, map[string]any{"title": "Theirs"})
	if rec.Code != http.StatusCreated {
		t.Fatalf("other org create: %d", rec.Code)
	}
	if got := decode(t, rec)["key"]; got != "ACM-1" {
		t.Fatalf("other org key: %v", got)
	}
	// A name without letters falls back to TSK.
	blank := uuid.New()
	fx.repo.orgNames[blank] = "123"
	fx.repo.addMember(blank, fx.me, "Ada", "ada@example.com", organization.RoleMember)
	rec = fx.call(t, fx.me, http.MethodPost, "/api/v1/organizations/"+blank.String()+"/tasks", map[string]any{"title": "X"})
	if got := decode(t, rec)["key"]; got != "TSK-1" {
		t.Fatalf("blank name key: %v", got)
	}
	// Address by id, and by key case-insensitively.
	id := a["id"].(string)
	for _, addr := range []string{id, "DRA-1", "dra-1"} {
		rec := fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+addr, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("get %s: want 200, got %d", addr, rec.Code)
		}
		detail := decode(t, rec)
		if detail["task"].(map[string]any)["key"] != "DRA-1" {
			t.Fatalf("detail key: %v", detail)
		}
		if _, ok := detail["comments"]; !ok {
			t.Fatalf("detail has no comments: %v", detail)
		}
		if _, ok := detail["activity"]; !ok {
			t.Fatalf("detail has no activity: %v", detail)
		}
	}
}

func TestCreateWithExplicitTodoAndNoLabels(t *testing.T) {
	fx := newFixture(t, "Dragent")
	// The exact payload the desktop sends when Todo is selected: explicit
	// "todo", priority "none", no labels key at all. It must create a Todo
	// task, never reject it and never store a null status.
	body := map[string]any{"title": "Explicit todo", "status": "todo", "priority": "none"}
	rec := fx.call(t, fx.me, http.MethodPost, fx.tasksPath(), body)
	if rec.Code != http.StatusCreated {
		t.Fatalf("explicit todo: want 201, got %d %s", rec.Code, rec.Body.String())
	}
	got := decode(t, rec)
	if got["status"] != "todo" {
		t.Fatalf("explicit todo keeps its status: %v", got)
	}
	if labels, ok := got["labels"].([]any); !ok || len(labels) != 0 {
		t.Fatalf("explicit todo has empty labels, not null: %v", got)
	}
}

func TestListFiltersAndStatusGroups(t *testing.T) {
	fx := newFixture(t, "Dragent")
	teammate := uuid.New()
	fx.repo.addMember(fx.org, teammate, "Bob", "bob@example.com", organization.RoleMember)
	mk := func(title, status string, extra map[string]any) {
		body := map[string]any{"title": title, "status": status}
		for k, v := range extra {
			body[k] = v
		}
		fx.createTask(t, fx.me, body)
		fx.advance(1)
	}
	mk("Fix login", "todo", map[string]any{"assignee_id": fx.me.String(), "labels": []string{"Bug", "bug", " UI "}, "project": map[string]any{"key": "github.com/acme/app", "label": "App"}})
	mk("Write docs", "backlog", map[string]any{"labels": []string{"docs"}})
	mk("Ship it", "done", map[string]any{"assignee_id": teammate.String()})
	mk("Rollback", "canceled", nil)
	mk("Review PR", "in_review", map[string]any{"description": "login flow check"})

	list := func(query string) map[string]any {
		rec := fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+query, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("list %s: %d %s", query, rec.Code, rec.Body.String())
		}
		return decode(t, rec)
	}
	keys := func(v map[string]any) []string {
		var out []string
		for _, item := range v["tasks"].([]any) {
			out = append(out, item.(map[string]any)["key"].(string))
		}
		return out
	}
	// Default is open, most recently updated first.
	got := list("")
	if len(keys(got)) != 3 {
		t.Fatalf("default open: %v", keys(got))
	}
	if k := keys(got); k[0] != "DRA-5" {
		t.Fatalf("order: %v", k)
	}
	if got["prefix"] != "DRA" {
		t.Fatalf("prefix: %v", got)
	}
	if got["next_cursor"] != nil {
		t.Fatalf("single page should have no cursor: %v", got)
	}
	if len(keys(list("?status=closed"))) != 2 {
		t.Fatalf("closed: %v", keys(list("?status=closed")))
	}
	if len(keys(list("?status=all"))) != 5 {
		t.Fatalf("all: %v", keys(list("?status=all")))
	}
	if k := keys(list("?status=todo,in_progress")); len(k) != 1 || k[0] != "DRA-1" {
		t.Fatalf("status list: %v", k)
	}
	// Assignee filters.
	if k := keys(list("?status=all&assignee=me")); len(k) != 1 || k[0] != "DRA-1" {
		t.Fatalf("assignee=me: %v", k)
	}
	if k := keys(list("?assignee=none")); len(k) != 2 {
		t.Fatalf("assignee=none: %v", k)
	}
	if k := keys(list("?status=all&assignee=" + teammate.String())); len(k) != 1 || k[0] != "DRA-3" {
		t.Fatalf("assignee=id: %v", k)
	}
	// Label is stored lowercase; project matches on key.
	if k := keys(list("?label=bug")); len(k) != 1 || k[0] != "DRA-1" {
		t.Fatalf("label: %v", k)
	}
	if k := keys(list("?project=github.com/acme/app")); len(k) != 1 {
		t.Fatalf("project: %v", k)
	}
	// Search needs every word, case-insensitively.
	if k := keys(list("?status=all&q=login")); len(k) != 2 {
		t.Fatalf("q login: %v", k)
	}
	if k := keys(list("?status=all&q=LOGIN%20flow")); len(k) != 1 || k[0] != "DRA-5" {
		t.Fatalf("q multiword: %v", k)
	}
	// LIKE wildcards in q match literally.
	mk("100% done_ish", "todo", nil)
	if k := keys(list("?status=all&q=100%25%20done_ish")); len(k) != 1 {
		t.Fatalf("q wildcards: %v", k)
	}
	// Pagination: stable order across pages.
	page1 := list("?status=all&limit=2")
	if len(keys(page1)) != 2 || page1["next_cursor"] == nil {
		t.Fatalf("page1: %v", page1)
	}
	cursor := page1["next_cursor"].(string)
	page2 := list("?status=all&limit=2&cursor=" + cursor)
	if len(keys(page2)) != 2 || page2["next_cursor"] == nil {
		t.Fatalf("page2: %v", page2)
	}
	seen := append(keys(page1), keys(page2)...)
	page3 := list("?status=all&limit=2&cursor=" + page2["next_cursor"].(string))
	seen = append(seen, keys(page3)...)
	if len(seen) != 6 || page3["next_cursor"] != nil {
		t.Fatalf("pages: %v last=%v", seen, page3["next_cursor"])
	}
	// Incremental refresh: only newer tasks, plus deletions.
	snap := fx.now
	fx.advance(10)
	fx.createTask(t, fx.me, map[string]any{"title": "Fresh"})
	del := fx.call(t, fx.me, http.MethodDelete, fx.tasksPath()+"/DRA-2", nil)
	if del.Code != http.StatusNoContent {
		t.Fatalf("delete DRA-2: %d", del.Code)
	}
	inc := list("?updated_after_ms=" + strconv.FormatInt(snap, 10))
	if k := keys(inc); len(k) != 1 || k[0] != "DRA-7" {
		t.Fatalf("incremental tasks: %v", k)
	}
	deleted, _ := inc["deleted_ids"].([]any)
	if len(deleted) != 1 {
		t.Fatalf("incremental deleted: %v", inc["deleted_ids"])
	}
	// updated_after_ms without a status still sees every status.
	incAll := list("?status=all&updated_after_ms=0")
	if len(keys(incAll)) != 6 {
		t.Fatalf("updated_after all statuses: %v", keys(incAll))
	}
}

func TestPatchHistoryAndStatusTimes(t *testing.T) {
	fx := newFixture(t, "Dragent")
	teammate := uuid.New()
	fx.repo.addMember(fx.org, teammate, "Bob", "bob@example.com", organization.RoleMember)
	created := fx.createTask(t, fx.me, map[string]any{"title": "Fix it", "labels": []string{"bug"}})
	id := created["id"].(string)

	patch := func(body map[string]any) map[string]any {
		fx.advance(5)
		rec := fx.call(t, fx.me, http.MethodPatch, fx.tasksPath()+"/"+id, body)
		if rec.Code != http.StatusOK {
			t.Fatalf("patch %v: want 200, got %d %s", body, rec.Code, rec.Body.String())
		}
		return decode(t, rec)
	}
	// Repeating the same values changes nothing and writes no history.
	got := patch(map[string]any{"title": "Fix it", "status": "todo"})
	if got["updated_at_ms"] != created["updated_at_ms"] {
		t.Fatalf("no-op patch moved updated_at_ms")
	}
	// Real changes: one entry per field, with wire values.
	fx.advance(5)
	got = patch(map[string]any{
		"title": "Fix it now", "status": "in_progress", "priority": "high",
		"assignee_id": teammate.String(), "labels": []string{"Bug", "urgent"},
		"due_at_ms":   float64(1_800_000_000_000),
		"project":     map[string]any{"key": "github.com/acme/app", "label": "App"},
		"description": "Details here",
		"by_agent":    true,
	})
	if got["started_at_ms"] == nil {
		t.Fatalf("started_at_ms not set on in_progress: %v", got)
	}
	started := got["started_at_ms"]
	rec := fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil)
	activity := decode(t, rec)["activity"].([]any)
	// created + 8 changed fields.
	if len(activity) != 9 {
		t.Fatalf("want 9 history entries, got %d: %v", len(activity), activity)
	}
	byKind := map[string]map[string]any{}
	for _, a := range activity[1:] {
		m := a.(map[string]any)
		byKind[m["kind"].(string)] = m
		if m["actor_kind"] != "agent" {
			t.Fatalf("by_agent should mark agent: %v", m)
		}
		actor := m["actor"].(map[string]any)
		if actor["id"] != fx.me.String() {
			t.Fatalf("actor: %v", actor)
		}
	}
	if byKind["title"]["from"] != "Fix it" || byKind["title"]["to"] != "Fix it now" {
		t.Fatalf("title entry: %v", byKind["title"])
	}
	if byKind["status"]["from"] != "todo" || byKind["status"]["to"] != "in_progress" {
		t.Fatalf("status entry: %v", byKind["status"])
	}
	if byKind["description"]["from"] != nil || byKind["description"]["to"] != nil {
		t.Fatalf("description carries no values: %v", byKind["description"])
	}
	if byKind["labels"]["to"].([]any)[0] != "bug" {
		t.Fatalf("labels stored lowercase: %v", byKind["labels"])
	}
	if byKind["assignee"]["to"].(map[string]any)["id"] != teammate.String() {
		t.Fatalf("assignee entry: %v", byKind["assignee"])
	}
	// Leaving review keeps started_at; done sets completed_at.
	fx.advance(5)
	got = patch(map[string]any{"status": "in_review"})
	if got["started_at_ms"] != started {
		t.Fatalf("started_at_ms moved: %v", got)
	}
	fx.advance(5)
	got = patch(map[string]any{"status": "done"})
	if got["completed_at_ms"] == nil {
		t.Fatalf("completed_at_ms not set on done")
	}
	// Reopening clears completed_at but keeps started_at.
	fx.advance(5)
	got = patch(map[string]any{"status": "todo"})
	if got["completed_at_ms"] != nil || got["started_at_ms"] != started {
		t.Fatalf("reopen times: %v", got)
	}
}

func TestNullClearing(t *testing.T) {
	fx := newFixture(t, "Dragent")
	created := fx.createTask(t, fx.me, map[string]any{
		"title": "T", "assignee_id": fx.me.String(),
		"project":   map[string]any{"key": "github.com/acme/app", "label": "App"},
		"due_at_ms": float64(1_800_000_000_000),
	})
	id := created["id"].(string)
	fx.advance(5)
	rec := fx.call(t, fx.me, http.MethodPatch, fx.tasksPath()+"/"+id, map[string]any{
		"assignee_id": nil, "project": nil, "due_at_ms": nil, "labels": nil,
	})
	if rec.Code != http.StatusOK {
		t.Fatalf("clear: %d %s", rec.Code, rec.Body.String())
	}
	got := decode(t, rec)
	if got["assignee"] != nil || got["project"] != nil || got["due_at_ms"] != nil {
		t.Fatalf("not cleared: %v", got)
	}
	detail := decode(t, fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil))
	activity := detail["activity"].([]any)
	// created + assignee + project + due (labels:null is ignored like the desktop shape).
	if len(activity) != 4 {
		t.Fatalf("want 4 entries, got %d", len(activity))
	}
}

func TestLinkUpsertIdempotency(t *testing.T) {
	fx := newFixture(t, "Dragent")
	created := fx.createTask(t, fx.me, map[string]any{"title": "T"})
	id := created["id"].(string)
	post := func(body map[string]any) *httptest.ResponseRecorder {
		fx.advance(5)
		return fx.call(t, fx.me, http.MethodPost, fx.tasksPath()+"/"+id+"/links", body)
	}
	link := map[string]any{"kind": "pull_request", "title": "Fix PR", "reference": "https://git.example.com/acme/app/pull/1", "url": "https://git.example.com/acme/app/pull/1"}
	rec := post(link)
	if rec.Code != http.StatusCreated {
		t.Fatalf("new link: want 201, got %d %s", rec.Code, rec.Body.String())
	}
	first := decode(t, rec)
	link["title"] = "Fix PR (updated)"
	link["state"] = "merged"
	rec = post(link)
	if rec.Code != http.StatusOK {
		t.Fatalf("relink: want 200, got %d %s", rec.Code, rec.Body.String())
	}
	second := decode(t, rec)
	if second["id"] != first["id"] {
		t.Fatalf("upsert made a second link: %v vs %v", first, second)
	}
	if second["title"] != "Fix PR (updated)" || second["state"] != "merged" {
		t.Fatalf("upsert did not update: %v", second)
	}
	detail := decode(t, fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil))
	if links := detail["task"].(map[string]any)["links"].([]any); len(links) != 1 {
		t.Fatalf("task links: %v", links)
	}
	var kinds []string
	for _, a := range detail["activity"].([]any) {
		kinds = append(kinds, a.(map[string]any)["kind"].(string))
	}
	if len(kinds) != 2 || kinds[1] != "link_added" {
		t.Fatalf("upsert must add no history: %v", kinds)
	}
	added := detail["activity"].([]any)[1].(map[string]any)
	if added["to"] == nil || added["from"] != nil {
		t.Fatalf("link_added entry: %v", added)
	}
	// Patching the link edits it without history; deleting records removal.
	linkID := first["id"].(string)
	fx.advance(5)
	rec = fx.call(t, fx.me, http.MethodPatch, fx.tasksPath()+"/"+id+"/links/"+linkID, map[string]any{"state": "closed", "url": nil})
	if rec.Code != http.StatusOK {
		t.Fatalf("link patch: %d %s", rec.Code, rec.Body.String())
	}
	patched := decode(t, rec)
	if patched["state"] != "closed" || patched["url"] != nil {
		t.Fatalf("patched link: %v", patched)
	}
	fx.advance(5)
	rec = fx.call(t, fx.me, http.MethodDelete, fx.tasksPath()+"/"+id+"/links/"+linkID, nil)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("link delete: %d", rec.Code)
	}
	detail = decode(t, fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil))
	if links := detail["task"].(map[string]any)["links"].([]any); len(links) != 0 {
		t.Fatalf("link not removed: %v", links)
	}
	last := detail["activity"].([]any)
	if last[len(last)-1].(map[string]any)["kind"] != "link_removed" {
		t.Fatalf("no link_removed entry: %v", last)
	}
}

func TestCommentPermissions(t *testing.T) {
	fx := newFixture(t, "Dragent")
	teammate := uuid.New()
	fx.repo.addMember(fx.org, teammate, "Bob", "bob@example.com", organization.RoleMember)
	admin := uuid.New()
	fx.repo.addMember(fx.org, admin, "Root", "root@example.com", organization.RoleAdmin)
	created := fx.createTask(t, fx.me, map[string]any{"title": "T"})
	id := created["id"].(string)

	post := func(user uuid.UUID, body string) *httptest.ResponseRecorder {
		fx.advance(5)
		return fx.call(t, user, http.MethodPost, fx.tasksPath()+"/"+id+"/comments", map[string]any{"body": body, "by_agent": true})
	}
	rec := post(fx.me, "First!")
	if rec.Code != http.StatusCreated {
		t.Fatalf("comment: %d %s", rec.Code, rec.Body.String())
	}
	comment := decode(t, rec)
	if comment["actor_kind"] != "agent" || comment["edited_at_ms"] != nil {
		t.Fatalf("comment shape: %v", comment)
	}
	if comment["author"].(map[string]any)["id"] != fx.me.String() {
		t.Fatalf("author: %v", comment)
	}
	commentID := comment["id"].(string)
	cpath := fx.tasksPath() + "/" + id + "/comments/" + commentID
	// A teammate cannot edit or delete; an admin can delete but not edit.
	if rec := fx.call(t, teammate, http.MethodPatch, cpath, map[string]any{"body": "hijack"}); rec.Code != http.StatusForbidden {
		t.Fatalf("teammate edit: want 403, got %d", rec.Code)
	}
	if rec := fx.call(t, teammate, http.MethodDelete, cpath, nil); rec.Code != http.StatusForbidden {
		t.Fatalf("teammate delete: want 403, got %d", rec.Code)
	}
	if rec := fx.call(t, admin, http.MethodPatch, cpath, map[string]any{"body": "hijack"}); rec.Code != http.StatusForbidden {
		t.Fatalf("admin edit: want 403, got %d", rec.Code)
	}
	if rec := fx.call(t, admin, http.MethodDelete, cpath, nil); rec.Code != http.StatusNoContent {
		t.Fatalf("admin delete: want 204, got %d", rec.Code)
	}
	// The author edits their own; the count follows live comments.
	rec = post(fx.me, "Second")
	secondID := decode(t, rec)["id"].(string)
	spath := fx.tasksPath() + "/" + id + "/comments/" + secondID
	fx.advance(5)
	rec = fx.call(t, fx.me, http.MethodPatch, spath, map[string]any{"body": "Second (edited)"})
	if rec.Code != http.StatusOK {
		t.Fatalf("author edit: %d", rec.Code)
	}
	if edited := decode(t, rec); edited["body"] != "Second (edited)" || edited["edited_at_ms"] == nil {
		t.Fatalf("edited comment: %v", edited)
	}
	detail := decode(t, fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil))
	if n := detail["task"].(map[string]any)["comment_count"]; n != float64(1) {
		t.Fatalf("comment_count: %v", n)
	}
	if comments := detail["comments"].([]any); len(comments) != 1 || comments[0].(map[string]any)["id"] != secondID {
		t.Fatalf("comments: %v", comments)
	}
	// Every comment moves the task's updated time.
	before := created["updated_at_ms"]
	_ = post(fx.me, "Third")
	after := decode(t, fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil))["task"].(map[string]any)["updated_at_ms"]
	if after == before {
		t.Fatalf("comment did not touch the task")
	}
}

func TestDeleteRestore(t *testing.T) {
	fx := newFixture(t, "Dragent")
	teammate := uuid.New()
	fx.repo.addMember(fx.org, teammate, "Bob", "bob@example.com", organization.RoleMember)
	admin := uuid.New()
	fx.repo.addMember(fx.org, admin, "Root", "root@example.com", organization.RoleAdmin)
	created := fx.createTask(t, fx.me, map[string]any{"title": "T"})
	id := created["id"].(string)
	key := created["key"].(string)

	// A stranger to the task cannot delete it.
	if rec := fx.call(t, teammate, http.MethodDelete, fx.tasksPath()+"/"+id, nil); rec.Code != http.StatusForbidden {
		t.Fatalf("stranger delete: want 403, got %d", rec.Code)
	}
	// Anyone restores, but only within 30 days.
	fx.advance(5)
	if rec := fx.call(t, teammate, http.MethodDelete, fx.tasksPath()+"/"+id, nil); rec.Code != http.StatusForbidden {
		t.Fatalf("teammate delete: want 403, got %d", rec.Code)
	}
	fx.advance(5)
	if rec := fx.call(t, admin, http.MethodDelete, fx.tasksPath()+"/"+id, nil); rec.Code != http.StatusNoContent {
		t.Fatalf("admin delete: %d", rec.Code)
	}
	if rec := fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"/"+id, nil); rec.Code != http.StatusNotFound {
		t.Fatalf("deleted detail: want 404, got %d", rec.Code)
	}
	if rec := fx.call(t, fx.me, http.MethodGet, fx.tasksPath()+"?status=all", nil); rec.Code != http.StatusOK {
		t.Fatalf("list: %d", rec.Code)
	} else if tasks := decode(t, rec)["tasks"].([]any); len(tasks) != 0 {
		t.Fatalf("deleted task listed: %v", tasks)
	}
	fx.advance(5)
	rec := fx.call(t, teammate, http.MethodPost, fx.tasksPath()+"/"+id+"/restore", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("restore: want 200, got %d %s", rec.Code, rec.Body.String())
	}
	// The number stays taken: the next task moves on.
	fx.advance(5)
	next := fx.createTask(t, fx.me, map[string]any{"title": "Next"})
	if next["key"] != "DRA-2" {
		t.Fatalf("number reused: %v", next)
	}
	if rec := fx.call(t, fx.me, http.MethodDelete, fx.tasksPath()+"/"+key, nil); rec.Code != http.StatusNoContent {
		t.Fatalf("creator delete by key: %d", rec.Code)
	}
	// After 30 days the restore window closes.
	fx.advance(30*24*60*60*1000 + 1)
	if rec := fx.call(t, fx.me, http.MethodPost, fx.tasksPath()+"/"+key+"/restore", nil); rec.Code != http.StatusGone {
		t.Fatalf("late restore: want 410, got %d", rec.Code)
	}
}

func TestValidationErrors(t *testing.T) {
	fx := newFixture(t, "Dragent")
	bad := func(method, path string, body any) *httptest.ResponseRecorder {
		return fx.call(t, fx.me, method, path, body)
	}
	cases := []struct {
		name string
		call func() *httptest.ResponseRecorder
	}{
		{"blank title", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "  "})
		}},
		{"missing title", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{})
		}},
		{"title too long", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": strings.Repeat("x", 501)})
		}},
		{"unknown status", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "T", "status": "later"})
		}},
		{"unknown priority", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "T", "priority": "yesterday"})
		}},
		{"stranger assignee", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "T", "assignee_id": uuid.New().String()})
		}},
		{"bad assignee", func() *httptest.ResponseRecorder {
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "T", "assignee_id": "nobody"})
		}},
		{"too many labels", func() *httptest.ResponseRecorder {
			labels := make([]string, 21)
			for i := range labels {
				labels[i] = "label-" + strconv.Itoa(i)
			}
			return bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "T", "labels": labels})
		}},
		{"blank comment", func() *httptest.ResponseRecorder {
			created := fx.createTask(t, fx.me, map[string]any{"title": "T"})
			return bad(http.MethodPost, fx.tasksPath()+"/"+created["id"].(string)+"/comments", map[string]any{"body": " "})
		}},
		{"bad link url", func() *httptest.ResponseRecorder {
			created := fx.createTask(t, fx.me, map[string]any{"title": "T"})
			return bad(http.MethodPost, fx.tasksPath()+"/"+created["id"].(string)+"/links",
				map[string]any{"kind": "item", "title": "X", "reference": "linear:ENG-1", "url": "ftp://x"})
		}},
		{"unknown link kind", func() *httptest.ResponseRecorder {
			created := fx.createTask(t, fx.me, map[string]any{"title": "T"})
			return bad(http.MethodPost, fx.tasksPath()+"/"+created["id"].(string)+"/links",
				map[string]any{"kind": "portal", "title": "X", "reference": "x"})
		}},
		{"bad limit", func() *httptest.ResponseRecorder {
			return bad(http.MethodGet, fx.tasksPath()+"?limit=nope", nil)
		}},
		{"bad cursor", func() *httptest.ResponseRecorder {
			return bad(http.MethodGet, fx.tasksPath()+"?cursor=nope", nil)
		}},
		{"bad updated_after", func() *httptest.ResponseRecorder {
			return bad(http.MethodGet, fx.tasksPath()+"?updated_after_ms=nope", nil)
		}},
	}
	for _, c := range cases {
		if rec := c.call(); rec.Code != http.StatusBadRequest {
			t.Fatalf("%s: want 400, got %d %s", c.name, rec.Code, rec.Body.String())
		}
	}
	// Unknown fields are ignored, not refused.
	rec := bad(http.MethodPost, fx.tasksPath(), map[string]any{"title": "Fine", "nonsense": 1})
	if rec.Code != http.StatusCreated {
		t.Fatalf("unknown fields: want 201, got %d", rec.Code)
	}
}

func TestTaskLabels(t *testing.T) {
	fx := newFixture(t, "Dragent")
	empty := decode(t, fx.call(t, fx.me, http.MethodGet, "/api/v1/organizations/"+fx.org.String()+"/task-labels", nil))
	if labels := empty["labels"].([]any); len(labels) != 0 {
		t.Fatalf("empty labels: %v", labels)
	}
	fx.createTask(t, fx.me, map[string]any{"title": "A", "labels": []string{"Bug", "UI"}})
	fx.advance(1)
	fx.createTask(t, fx.me, map[string]any{"title": "B", "labels": []string{"bug", "docs"}})
	got := decode(t, fx.call(t, fx.me, http.MethodGet, "/api/v1/organizations/"+fx.org.String()+"/task-labels", nil))
	var labels []string
	for _, l := range got["labels"].([]any) {
		labels = append(labels, l.(string))
	}
	if len(labels) != 3 || labels[0] != "bug" || labels[1] != "docs" || labels[2] != "ui" {
		t.Fatalf("labels: %v", labels)
	}
}

func TestCrossOrgIsolation(t *testing.T) {
	fx := newFixture(t, "Dragent")
	created := fx.createTask(t, fx.me, map[string]any{"title": "Secret"})
	id := created["id"].(string)
	key := created["key"].(string)

	other := uuid.New()
	fx.repo.orgNames[other] = "Other"
	fx.repo.addMember(other, fx.me, "Ada", "ada@example.com", organization.RoleMember)
	opath := "/api/v1/organizations/" + other.String() + "/tasks/" + id
	for _, tc := range []struct {
		method, path string
		body         any
	}{
		{http.MethodGet, opath, nil},
		{http.MethodPatch, opath, map[string]any{"title": "Steal"}},
		{http.MethodDelete, opath, nil},
		{http.MethodPost, opath + "/restore", nil},
		{http.MethodPost, opath + "/comments", map[string]any{"body": "Hi"}},
		{http.MethodPost, opath + "/links", map[string]any{"kind": "item", "title": "X", "reference": "linear:ENG-1"}},
		{http.MethodGet, "/api/v1/organizations/" + other.String() + "/tasks/" + key, nil},
	} {
		if rec := fx.call(t, fx.me, tc.method, tc.path, tc.body); rec.Code != http.StatusNotFound {
			t.Fatalf("%s %s: want 404, got %d %s", tc.method, tc.path, rec.Code, rec.Body.String())
		}
	}
	// And the other organisation's list is empty.
	rec := fx.call(t, fx.me, http.MethodGet, "/api/v1/organizations/"+other.String()+"/tasks?status=all", nil)
	if rec.Code != http.StatusOK || len(decode(t, rec)["tasks"].([]any)) != 0 {
		t.Fatalf("other org list leaks: %d %s", rec.Code, rec.Body.String())
	}
}
