package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
)

// providerFakeExecutor records the search parameters it receives (the only
// thing the phone may send) and replays canned items or errors.
type providerFakeExecutor struct {
	githubQuery, githubRepo    string
	gitlabQuery, gitlabProject string
	githubItems                []ProviderWorkItem
	gitlabItems                []ProviderWorkItem
	githubErr, gitlabErr       error
}

func (f *providerFakeExecutor) ListGitHubItems(_ context.Context, query, repo string) ([]ProviderWorkItem, error) {
	f.githubQuery, f.githubRepo = query, repo
	if f.githubErr != nil {
		return nil, f.githubErr
	}
	return f.githubItems, nil
}

func (f *providerFakeExecutor) ListGitLabItems(_ context.Context, query, project string) ([]ProviderWorkItem, error) {
	f.gitlabQuery, f.gitlabProject = query, project
	if f.gitlabErr != nil {
		return nil, f.gitlabErr
	}
	return f.gitlabItems, nil
}

// providerStubResolver is a fake device-ownership check.
type providerStubResolver struct {
	owned map[string]bool
	err   error
}

func (s *providerStubResolver) DeviceOwnedBy(_ context.Context, userID, deviceID string) (bool, error) {
	if s.err != nil {
		return false, s.err
	}
	return s.owned[userID+"\x00"+deviceID], nil
}

func providerCtx(user, device string) context.Context {
	ctx := context.Background()
	if user != "" {
		ctx = context.WithValue(ctx, "user_id", user)
	}
	if device != "" {
		ctx = context.WithValue(ctx, "device_id", device)
	}
	return ctx
}

func providerValidItem() ProviderWorkItem {
	return ProviderWorkItem{
		ID:     "github:acme/app#7",
		Number: 7,
		Title:  "Fix login redirect",
		URL:    "https://github.com/acme/app/pull/7",
		State:  "open",
		Repo:   "acme/app",
	}
}

func TestProviderWorkItemShapeValidation(t *testing.T) {
	if err := providerValidItem().Validate(); err != nil {
		t.Fatalf("valid item rejected: %v", err)
	}
	// Canonical states, any case.
	for _, state := range []string{"open", "OPEN", "Closed", "MERGED", "merged"} {
		w := providerValidItem()
		w.State = state
		if err := w.Validate(); err != nil {
			t.Fatalf("state %q rejected: %v", state, err)
		}
	}
	bad := map[string]func(*ProviderWorkItem){
		"empty id":     func(w *ProviderWorkItem) { w.ID = "  " },
		"id too long":  func(w *ProviderWorkItem) { w.ID = strings.Repeat("i", providerMaxScopeLength+1) },
		"zero number":  func(w *ProviderWorkItem) { w.Number = 0 },
		"empty title":  func(w *ProviderWorkItem) { w.Title = "  " },
		"empty url":    func(w *ProviderWorkItem) { w.URL = "" },
		"non-http url": func(w *ProviderWorkItem) { w.URL = "gopher://example.com/x" },
		"relative url": func(w *ProviderWorkItem) { w.URL = "/acme/app/pull/7" },
		"bad state":    func(w *ProviderWorkItem) { w.State = "draft" },
		"empty state":  func(w *ProviderWorkItem) { w.State = "" },
		"empty repo":   func(w *ProviderWorkItem) { w.Repo = "" },
		"credentialed url": func(w *ProviderWorkItem) {
			w.URL = "https://user:s3cret@github.com/acme/app/pull/7"
		},
	}
	for name, mutate := range bad {
		w := providerValidItem()
		mutate(&w)
		if err := w.Validate(); err == nil {
			t.Fatalf("%s accepted", name)
		}
	}
}

func TestProviderParamsValidation(t *testing.T) {
	if _, _, err := (providerGitHubParams{Query: "login bug", Repo: "acme/app"}).normalized(); err != nil {
		t.Fatalf("valid github params rejected: %v", err)
	}
	if _, _, err := (providerGitLabParams{Query: "auth", Project: "group/sub/project"}).normalized(); err != nil {
		t.Fatalf("valid gitlab params rejected: %v", err)
	}
	// Blank filters mean absent.
	q, repo, err := (providerGitHubParams{Query: "  ", Repo: ""}).normalized()
	if err != nil || q != "" || repo != "" {
		t.Fatalf("blank filters should normalize to absent, got %q %q err=%v", q, repo, err)
	}
	// Full https URLs without credentials are accepted as scopes.
	if _, repo, err := (providerGitHubParams{Repo: "https://github.com/acme/app"}).normalized(); err != nil || repo == "" {
		t.Fatalf("https scope rejected: %v", err)
	}
	for name, p := range map[string]any{
		"query too long":         providerGitHubParams{Query: strings.Repeat("q", providerMaxQueryLength+1)},
		"query with newline":     providerGitHubParams{Query: "a\nb"},
		"query with NUL":         providerGitLabParams{Query: "a\x00b"},
		"scope too long":         providerGitHubParams{Repo: strings.Repeat("r", providerMaxScopeLength+1)},
		"scope leading dash":     providerGitHubParams{Repo: "--repo=x"},
		"scope with space":       providerGitHubParams{Repo: "acme/app x"},
		"scope with semicolon":   providerGitHubParams{Repo: "acme/app;rm"},
		"scope with pipe":        providerGitHubParams{Repo: "a|b"},
		"scope with dollars":     providerGitLabParams{Project: "$(touch /tmp/pwned)"},
		"scope with backtick":    providerGitLabParams{Project: "`whoami`"},
		"scope credentialed url": providerGitHubParams{Repo: "https://user:s3cret@github.com/acme/app"},
		"scope non-http url":     providerGitLabParams{Project: "gopher://example.com/x"},
	} {
		var err error
		switch v := p.(type) {
		case providerGitHubParams:
			_, _, err = v.normalized()
		case providerGitLabParams:
			_, _, err = v.normalized()
		}
		if err == nil {
			t.Fatalf("%s accepted", name)
		}
	}
}

func providerDispatch(t *testing.T, ctx context.Context, method string, params any, exec ProviderExecutor, resolver ProviderDeviceResolver) ProviderRPCReply {
	t.Helper()
	raw, _ := json.Marshal(params)
	return DispatchProviderRPC(ctx, ProviderRPCRequest{
		Type: "rpc-request", V: 1, ID: "req-1", Method: method, Params: raw,
	}, exec, resolver)
}

func TestProviderAnonymousRejected(t *testing.T) {
	exec := &providerFakeExecutor{githubItems: []ProviderWorkItem{providerValidItem()}}
	for _, method := range []string{ProviderMethodGitHubListWorkItems, ProviderMethodGitLabListWorkItems} {
		reply := providerDispatch(t, providerCtx("", ""), method, map[string]any{}, exec, nil)
		if reply.OK || reply.Error == nil {
			t.Fatalf("%s: anonymous call succeeded", method)
		}
		if !reply.Error.LoginRequired || reply.Error.Status != http.StatusUnauthorized {
			t.Fatalf("%s: anonymous error = %+v, want 401 loginRequired", method, reply.Error)
		}
		if reply.Type != "rpc-reply" || reply.V != 1 || reply.ID != "req-1" {
			t.Fatalf("%s: reply envelope = %+v", method, reply)
		}
	}
}

func TestProviderForeignDeviceRejected(t *testing.T) {
	exec := &providerFakeExecutor{githubItems: []ProviderWorkItem{providerValidItem()}}
	resolver := &providerStubResolver{owned: map[string]bool{}}
	reply := providerDispatch(t, providerCtx("user-a", "phone-1"), ProviderMethodGitHubListWorkItems, map[string]any{}, exec, resolver)
	if reply.OK || reply.Error == nil {
		t.Fatal("foreign device call succeeded")
	}
	if reply.Error.LoginRequired || reply.Error.Status != http.StatusForbidden {
		t.Fatalf("foreign device error = %+v, want 403 without loginRequired", reply.Error)
	}
	// A device the user owns passes.
	owned := &providerStubResolver{owned: map[string]bool{"user-a\x00phone-1": true}}
	reply = providerDispatch(t, providerCtx("user-a", "phone-1"), ProviderMethodGitHubListWorkItems, map[string]any{}, exec, owned)
	if !reply.OK {
		t.Fatalf("owned device rejected: %+v", reply.Error)
	}
	// A resolver outage is user-safe and never 401-shaped.
	broken := &providerStubResolver{err: fmt.Errorf("db down")}
	reply = providerDispatch(t, providerCtx("user-a", "phone-1"), ProviderMethodGitHubListWorkItems, map[string]any{}, exec, broken)
	if reply.OK || reply.Error.Message != providerMsgVerifyFailed || reply.Error.LoginRequired {
		t.Fatalf("resolver outage error = %+v", reply.Error)
	}
}

func TestProviderLinearOutOfScope(t *testing.T) {
	exec := &providerFakeExecutor{}
	for _, method := range []string{"linear.listWorkItems", "linear.search", "linear.anything"} {
		reply := providerDispatch(t, providerCtx("user-a", ""), method, map[string]any{}, exec, nil)
		if reply.OK || reply.Error == nil {
			t.Fatalf("%s accepted", method)
		}
		if !strings.Contains(strings.ToLower(reply.Error.Message), "linear") {
			t.Fatalf("%s: error %q does not name linear", method, reply.Error.Message)
		}
		if reply.Error.LoginRequired {
			t.Fatalf("%s: out-of-scope must not be loginRequired", method)
		}
	}
	unknown := providerDispatch(t, providerCtx("user-a", ""), "github.deleteEverything", map[string]any{}, exec, nil)
	if unknown.OK || unknown.Error.Message != providerMsgUnknownMethod {
		t.Fatalf("unknown method reply = %+v", unknown)
	}
}

func TestProviderEnvelopeValidation(t *testing.T) {
	exec := &providerFakeExecutor{githubItems: []ProviderWorkItem{providerValidItem()}}
	ctx := providerCtx("user-a", "")

	badType := DispatchProviderRPC(ctx, ProviderRPCRequest{Type: "stream", V: 1, ID: "x", Method: ProviderMethodGitHubListWorkItems}, exec, nil)
	if badType.OK {
		t.Fatal("bad frame type accepted")
	}
	badVersion := DispatchProviderRPC(ctx, ProviderRPCRequest{Type: "rpc-request", V: 2, ID: "x", Method: ProviderMethodGitHubListWorkItems}, exec, nil)
	if badVersion.OK {
		t.Fatal("bad version accepted")
	}
	malformed := DispatchProviderRPC(ctx, ProviderRPCRequest{Type: "rpc-request", V: 1, ID: "x", Method: ProviderMethodGitHubListWorkItems, Params: json.RawMessage(`{oops`)}, exec, nil)
	if malformed.OK {
		t.Fatal("malformed params accepted")
	}
	// Params over the 64KB server cap are rejected before the executor runs.
	huge := DispatchProviderRPC(ctx, ProviderRPCRequest{
		Type: "rpc-request", V: 1, ID: "x", Method: ProviderMethodGitHubListWorkItems,
		Params: json.RawMessage(`{"query":"` + strings.Repeat("q", providerMaxParamsBytes) + `"}`),
	}, exec, nil)
	if huge.OK || huge.Error.Message != providerMsgTooLarge {
		t.Fatalf("oversized params reply = %+v", huge.Error)
	}
	// Param validation failures are user-safe (invalid scopes rejected).
	invalid := providerDispatch(t, ctx, ProviderMethodGitHubListWorkItems, map[string]any{"repo": "--repo=x"}, exec, nil)
	if invalid.OK {
		t.Fatal("flag-injection scope accepted")
	}
	// A missing executor is a user-safe 502, never a panic.
	missing := providerDispatch(t, ctx, ProviderMethodGitHubListWorkItems, map[string]any{}, nil, nil)
	if missing.OK || missing.Error.Message != providerMsgUnavailable || missing.Error.Status != http.StatusBadGateway {
		t.Fatalf("nil executor reply = %+v", missing.Error)
	}
	// Replies always answer the request id.
	if missing.ID != "req-1" || missing.Type != "rpc-reply" || missing.V != 1 {
		t.Fatalf("reply envelope = %+v", missing)
	}
}

func TestProviderCLIFailureMapping(t *testing.T) {
	ctx := providerCtx("user-a", "")
	cases := []struct {
		raw  string
		want string
	}{
		{"gh: authentication required: Run gh auth login", providerMsgAuthRequired},
		{"HTTP 401: Bad credentials (token expired)", providerMsgAuthRequired},
		{"error: forbidden: check your credentials", providerMsgAuthRequired},
		{"repository not found (404)", providerMsgNotFound},
		{"glab: 404 Not Found: project unknown", providerMsgNotFound},
		{"could not resolve host git.example.com", providerMsgTimeout},
		{"network is unreachable", providerMsgTimeout},
		{"response too large: exceeds 8 MB", providerMsgTooBig},
		{"something utterly unexpected", providerMsgFailed},
	}
	for _, tc := range cases {
		exec := &providerFakeExecutor{githubErr: fmt.Errorf("%s", tc.raw)}
		reply := providerDispatch(t, ctx, ProviderMethodGitHubListWorkItems, map[string]any{}, exec, nil)
		if reply.OK {
			t.Fatalf("raw %q succeeded", tc.raw)
		}
		if reply.Error.Message != tc.want {
			t.Fatalf("raw %q mapped to %q, want %q", tc.raw, reply.Error.Message, tc.want)
		}
		if reply.Error.Status != http.StatusBadGateway || reply.Error.LoginRequired {
			t.Fatalf("raw %q status/login = %+v", tc.raw, reply.Error)
		}
	}
}

func TestProviderSecretsNeverCrossTheRelay(t *testing.T) {
	ctx := providerCtx("user-a", "")
	secrets := []string{
		"ghp_supersecret123",
		"glpat-sekret-token",
		"https://oauth:s3cret@github.com/acme/app",
		"token=abc123 password=hunter2",
	}
	for _, secret := range secrets {
		exec := &providerFakeExecutor{
			githubErr: fmt.Errorf("request failed with %s (internal code 7)", secret),
			gitlabErr: fmt.Errorf("boom %s", secret),
		}
		for _, method := range []string{ProviderMethodGitHubListWorkItems, ProviderMethodGitLabListWorkItems} {
			reply := providerDispatch(t, ctx, method, map[string]any{}, exec, nil)
			raw, _ := json.Marshal(reply)
			for _, s := range []string{secret, "s3cret", "hunter2", "abc123"} {
				if strings.Contains(string(raw), s) {
					t.Fatalf("%s: reply leaks secret material: %s", method, raw)
				}
			}
		}
		// Direct classification never echoes either.
		if strings.Contains(sanitizeProviderError("token="+secrets[0]), secrets[0]) {
			t.Fatal("sanitizer echoes token material")
		}
	}
}

func TestProviderEmptyResultsEncodeAsArrays(t *testing.T) {
	ctx := providerCtx("user-a", "")
	for _, tc := range []struct {
		method string
		exec   *providerFakeExecutor
	}{
		{ProviderMethodGitHubListWorkItems, &providerFakeExecutor{githubItems: nil}},
		{ProviderMethodGitLabListWorkItems, &providerFakeExecutor{gitlabItems: []ProviderWorkItem{}}},
	} {
		reply := providerDispatch(t, ctx, tc.method, map[string]any{"query": "nothing matches this"}, tc.exec, nil)
		if !reply.OK || reply.Result == nil {
			t.Fatalf("%s: empty result not ok: %+v", tc.method, reply)
		}
		if reply.Result.Items == nil || len(reply.Result.Items) != 0 {
			t.Fatalf("%s: items = %#v, want empty non-nil", tc.method, reply.Result.Items)
		}
		raw, _ := json.Marshal(reply.Result)
		if !strings.Contains(string(raw), `"items":[]`) {
			t.Fatalf("%s: wire result = %s, want items:[]", tc.method, raw)
		}
	}
}

func TestProviderResultBoundedAndSkipsBadRows(t *testing.T) {
	ctx := providerCtx("user-a", "")
	items := []ProviderWorkItem{providerValidItem()}
	// One malformed row (no URL) must be skipped, not fail the call.
	items = append(items, ProviderWorkItem{ID: "bad", Number: 8, Title: "Broken", State: "open", Repo: "acme/app"})
	// Flood past the server cap.
	for i := int64(9); i < int64(providerMaxItems+50); i++ {
		w := providerValidItem()
		w.Number = i
		w.ID = fmt.Sprintf("github:acme/app#%d", i)
		items = append(items, w)
	}
	exec := &providerFakeExecutor{githubItems: items}
	reply := providerDispatch(t, ctx, ProviderMethodGitHubListWorkItems, map[string]any{}, exec, nil)
	if !reply.OK {
		t.Fatalf("bounded call failed: %+v", reply.Error)
	}
	if len(reply.Result.Items) != providerMaxItems {
		t.Fatalf("items = %d, want truncation at %d", len(reply.Result.Items), providerMaxItems)
	}
	for _, w := range reply.Result.Items {
		if err := w.Validate(); err != nil {
			t.Fatalf("emitted invalid item %+v: %v", w, err)
		}
	}
}

func TestProviderOnlySearchParamsReachExecutor(t *testing.T) {
	ctx := providerCtx("user-a", "")
	exec := &providerFakeExecutor{githubItems: []ProviderWorkItem{}, gitlabItems: []ProviderWorkItem{}}
	providerDispatch(t, ctx, ProviderMethodGitHubListWorkItems, map[string]any{"query": "login", "repo": "acme/app"}, exec, nil)
	if exec.githubQuery != "login" || exec.githubRepo != "acme/app" {
		t.Fatalf("github executor got %q %q", exec.githubQuery, exec.githubRepo)
	}
	providerDispatch(t, ctx, ProviderMethodGitLabListWorkItems, map[string]any{"query": "auth", "project": "group/proj"}, exec, nil)
	if exec.gitlabQuery != "auth" || exec.gitlabProject != "group/proj" {
		t.Fatalf("gitlab executor got %q %q", exec.gitlabQuery, exec.gitlabProject)
	}
}

func TestProviderNormalizers(t *testing.T) {
	gh := []byte(`[
		{"id":"GH_1","number":7,"title":"Fix login","url":"https://github.com/acme/app/pull/7","state":"OPEN","repository":{"nameWithOwner":"acme/app"}},
		{"number":8,"title":"Docs","url":"https://github.com/acme/app/issues/8","state":"closed","repository":"acme/app"},
		{"number":9,"title":"No state","url":"https://github.com/acme/app/issues/9","state":"weird"},
		{"number":0,"title":"No number","url":"https://github.com/acme/app/issues/0","state":"open"},
		{"number":10,"title":"","url":"https://github.com/acme/app/issues/10","state":"open"}
	]`)
	items, err := normalizeGitHubSearchItems(gh, "acme/app")
	if err != nil {
		t.Fatalf("github normalize failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("github items = %+v, want the 2 valid rows", items)
	}
	if items[0].ID != "GH_1" || items[0].State != "open" || items[0].Repo != "acme/app" {
		t.Fatalf("github row 0 = %+v", items[0])
	}
	if items[1].ID != "github:acme/app#8" {
		t.Fatalf("github synthesized id = %q", items[1].ID)
	}
	// Empty / drifted documents are empty results, not errors.
	for _, raw := range []string{``, `null`, `[]`, `{"oops":1}`} {
		got, err := normalizeGitHubSearchItems([]byte(raw), "acme/app")
		if err != nil || len(got) != 0 {
			t.Fatalf("raw %q -> %+v, %v", raw, got, err)
		}
	}

	glIssues := []byte(`[
		{"id":101,"iid":3,"title":"Bug","web_url":"https://git.example.com/g/p/-/issues/3","state":"opened","references":{"full":"g/p#3"}},
		{"iid":4,"title":"Old","web_url":"https://git.example.com/g/p/-/issues/4","state":"closed","references":{"full":"g/p#4"}}
	]`)
	issues, err := normalizeGitLabIssueItems(glIssues, "g/p")
	if err != nil || len(issues) != 2 {
		t.Fatalf("gitlab issues = %+v, %v", issues, err)
	}
	if issues[0].State != "open" || issues[0].Repo != "g/p" {
		t.Fatalf("gitlab issue row = %+v", issues[0])
	}
	glMRs := []byte(`[
		{"id":202,"iid":9,"title":"MR!","web_url":"https://git.example.com/g/p/-/merge_requests/9","state":"merged","references":{"full":"g/p!9"}}
	]`)
	mrs, err := normalizeGitLabMRItems(glMRs, "g/p")
	if err != nil || len(mrs) != 1 || mrs[0].State != "merged" {
		t.Fatalf("gitlab mrs = %+v, %v", mrs, err)
	}
}

// providerTestRouter mounts the provider REST routes the way the gateway
// will: behind an auth layer that injects "user_id"/"device_id" only when
// the caller presents them, so missing credentials stay missing.
func providerTestRouter(h *ProviderHandler) http.Handler {
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			if dev := req.Header.Get("X-Test-Device"); dev != "" {
				req = req.WithContext(context.WithValue(req.Context(), "device_id", dev))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterProviderRoutes(r)
	return r
}

func providerDoPost(t *testing.T, srv string, path, user, device string, body any) *http.Response {
	t.Helper()
	raw, _ := json.Marshal(body)
	req, _ := http.NewRequest(http.MethodPost, srv+path, bytes.NewReader(raw))
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	if device != "" {
		req.Header.Set("X-Test-Device", device)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func TestProviderHTTPHandlers(t *testing.T) {
	exec := &providerFakeExecutor{githubItems: []ProviderWorkItem{providerValidItem()}}
	resolver := &providerStubResolver{owned: map[string]bool{"user-a\x00phone-1": true}}
	srv := httptest.NewServer(providerTestRouter(&ProviderHandler{Exec: exec, Resolver: resolver}))
	defer srv.Close()

	// Anonymous -> 401.
	resp := providerDoPost(t, srv.URL, "/v1/relay/providers/github/work-items", "", "", map[string]any{})
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous status = %d, want 401", resp.StatusCode)
	}
	// The REST provider helpers are removed (F6, §10.5): every shape answers
	// 410 before the body is read — happy paths, foreign devices, and
	// credentialed scopes alike, with nothing echoed.
	for name, tt := range map[string]struct {
		path, user, device string
		body               map[string]any
	}{
		"happy path": {"/v1/relay/providers/github/work-items", "user-a", "phone-1", map[string]any{"query": "login"}},
		"foreign":    {"/v1/relay/providers/github/work-items", "user-a", "someone-else", map[string]any{}},
		"bad scope":  {"/v1/relay/providers/github/work-items", "user-a", "phone-1", map[string]any{"repo": "https://user:s3cret@github.com/acme/app"}},
		"gitlab":     {"/v1/relay/providers/gitlab/work-items", "user-a", "", map[string]any{}},
	} {
		resp := providerDoPost(t, srv.URL, tt.path, tt.user, tt.device, tt.body)
		raw, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		if resp.StatusCode != http.StatusGone {
			t.Fatalf("%s status = %d, want 410", name, resp.StatusCode)
		}
		if strings.Contains(string(raw), "s3cret") {
			t.Fatalf("%s echoes credentials: %s", name, raw)
		}
	}
	if exec.githubQuery != "" {
		t.Fatalf("removed helpers reached the executor: %q", exec.githubQuery)
	}
}
