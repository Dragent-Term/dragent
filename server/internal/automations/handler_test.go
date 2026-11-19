package automations

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

type fakeRepo struct {
	items map[uuid.UUID]Automation
	tier  string
}

func newFakeRepo(tier string) *fakeRepo {
	return &fakeRepo{items: map[uuid.UUID]Automation{}, tier: tier}
}

func (f *fakeRepo) List(_ context.Context, orgID uuid.UUID) ([]Automation, error) {
	var out []Automation
	for _, a := range f.items {
		if a.OrganizationID == orgID {
			out = append(out, a)
		}
	}
	return out, nil
}

func (f *fakeRepo) Get(_ context.Context, orgID, id uuid.UUID) (*Automation, error) {
	a, ok := f.items[id]
	if !ok || a.OrganizationID != orgID {
		return nil, nil
	}
	return &a, nil
}

func (f *fakeRepo) Exists(_ context.Context, id uuid.UUID) (bool, error) {
	_, ok := f.items[id]
	return ok, nil
}

func (f *fakeRepo) Upsert(_ context.Context, a Automation) error {
	f.items[a.ID] = a
	return nil
}

func (f *fakeRepo) Delete(_ context.Context, _ uuid.UUID, id uuid.UUID) error {
	delete(f.items, id)
	return nil
}

func (f *fakeRepo) CountOwned(_ context.Context, orgID, userID uuid.UUID) (int, error) {
	n := 0
	for _, a := range f.items {
		if a.OrganizationID == orgID && a.OwnerUserID == userID {
			n++
		}
	}
	return n, nil
}

func (f *fakeRepo) Tier(context.Context, uuid.UUID, uuid.UUID) (string, error) {
	return f.tier, nil
}

func router(h *Handler) http.Handler {
	r := chi.NewRouter()
	h.RegisterRoutes(r, func(next http.Handler) http.Handler { return next })
	return r
}

func call(t *testing.T, handler http.Handler, user uuid.UUID, method, path string, body any) *httptest.ResponseRecorder {
	t.Helper()
	var buf bytes.Buffer
	if body != nil {
		if err := json.NewEncoder(&buf).Encode(body); err != nil {
			t.Fatal(err)
		}
	}
	req := httptest.NewRequest(method, path, &buf)
	req = req.WithContext(context.WithValue(req.Context(), "user_id", user.String()))
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)
	return rec
}

func body(name string) map[string]any {
	return map[string]any{
		"name": name, "prompt": "Review yesterday", "schedule": "0 9 * * *",
		"schedule_label": "Every day at 9:00 AM", "agent": "claude", "enabled": true,
	}
}

func path(org, id uuid.UUID) string {
	return "/api/v1/organizations/" + org.String() + "/automations/" + id.String()
}

func TestFreePlanAllowsFourAutomations(t *testing.T) {
	repo := newFakeRepo("free")
	h := router(NewHandler(repo))
	org, me := uuid.New(), uuid.New()
	for i := 0; i < FreeLimit; i++ {
		if rec := call(t, h, me, http.MethodPut, path(org, uuid.New()), body("A")); rec.Code != http.StatusCreated {
			t.Fatalf("create %d: %d %s", i, rec.Code, rec.Body)
		}
	}
	rec := call(t, h, me, http.MethodPut, path(org, uuid.New()), body("Fifth"))
	if rec.Code != http.StatusPaymentRequired {
		t.Fatalf("fifth: want 402, got %d", rec.Code)
	}
	var refused map[string]any
	_ = json.Unmarshal(rec.Body.Bytes(), &refused)
	if refused["code"] != "automation_limit" {
		t.Fatalf("want automation_limit code, got %v", refused)
	}
	// Updating an existing one is still fine at the limit.
	var anyID uuid.UUID
	for id := range repo.items {
		anyID = id
		break
	}
	if rec := call(t, h, me, http.MethodPut, path(org, anyID), body("Renamed")); rec.Code != http.StatusOK {
		t.Fatalf("update at limit: %d", rec.Code)
	}
	// A teammate has their own allowance.
	if rec := call(t, h, uuid.New(), http.MethodPut, path(org, uuid.New()), body("Theirs")); rec.Code != http.StatusCreated {
		t.Fatalf("teammate create: %d", rec.Code)
	}
}

func TestPaidPlanIsUnlimited(t *testing.T) {
	repo := newFakeRepo("pro")
	h := router(NewHandler(repo))
	org, me := uuid.New(), uuid.New()
	for i := 0; i < FreeLimit+2; i++ {
		if rec := call(t, h, me, http.MethodPut, path(org, uuid.New()), body("A")); rec.Code != http.StatusCreated {
			t.Fatalf("create %d: %d", i, rec.Code)
		}
	}
	rec := call(t, h, me, http.MethodGet, "/api/v1/organizations/"+org.String()+"/automations", nil)
	var listed struct {
		Automations []Automation `json:"automations"`
		Allowance   Allowance    `json:"allowance"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &listed); err != nil {
		t.Fatal(err)
	}
	if len(listed.Automations) != FreeLimit+2 || listed.Allowance.Limit != nil || listed.Allowance.Owned != FreeLimit+2 {
		t.Fatalf("unexpected list: %+v", listed.Allowance)
	}
}

func TestOnlyTheOwnerChangesOrDeletes(t *testing.T) {
	repo := newFakeRepo("free")
	h := router(NewHandler(repo))
	org, owner, teammate, id := uuid.New(), uuid.New(), uuid.New(), uuid.New()
	if rec := call(t, h, owner, http.MethodPut, path(org, id), body("Mine")); rec.Code != http.StatusCreated {
		t.Fatalf("create: %d", rec.Code)
	}
	if rec := call(t, h, teammate, http.MethodPut, path(org, id), body("Stolen")); rec.Code != http.StatusForbidden {
		t.Fatalf("teammate update: want 403, got %d", rec.Code)
	}
	if rec := call(t, h, teammate, http.MethodDelete, path(org, id), nil); rec.Code != http.StatusForbidden {
		t.Fatalf("teammate delete: want 403, got %d", rec.Code)
	}
	// The same id can't be claimed in another organization.
	if rec := call(t, h, owner, http.MethodPut, path(uuid.New(), id), body("Moved")); rec.Code != http.StatusConflict {
		t.Fatalf("cross-org: want 409, got %d", rec.Code)
	}
	if rec := call(t, h, owner, http.MethodDelete, path(org, id), nil); rec.Code != http.StatusNoContent {
		t.Fatalf("owner delete: %d", rec.Code)
	}
	if len(repo.items) != 0 {
		t.Fatal("automation not deleted")
	}
}

func TestInvalidBodiesAreRefused(t *testing.T) {
	h := router(NewHandler(newFakeRepo("free")))
	org, me := uuid.New(), uuid.New()
	bad := body(" ")
	if rec := call(t, h, me, http.MethodPut, path(org, uuid.New()), bad); rec.Code != http.StatusBadRequest {
		t.Fatalf("blank name: want 400, got %d", rec.Code)
	}
	if rec := call(t, h, me, http.MethodPut, "/api/v1/organizations/"+org.String()+"/automations/nope", body("A")); rec.Code != http.StatusBadRequest {
		t.Fatalf("bad id: want 400, got %d", rec.Code)
	}
}
