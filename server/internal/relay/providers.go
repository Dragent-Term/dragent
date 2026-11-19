// R3 provider proxy RPCs: github.listWorkItems and gitlab.listWorkItems.
//
// The phone sends only search parameters ({query?, repo?} / {query?,
// project?}); provider tokens and credentials never cross the relay. Search
// parameters are executed through the host's own CLIs and credentials — the
// same `gh` and `glab` binaries the desktop's pull_requests modules
// (dragent/crates/dragent_host/src/pull_requests.rs + pull_requests/gitlab.rs)
// shell out to — so authentication stays in the CLI credential stores on the
// project machine, exactly as the host's local and SSH screens do.
//
// Wire shape follows the R0 envelope (server/internal/relay/RPC_ENVELOPE.md):
// correlated rpc-request in, rpc-reply out, user-safe error messages, params
// JSON capped at 64KB, results bounded. Every call checks same-account
// controller authorization first: anonymous callers get 401 semantics with
// loginRequired:true, foreign devices get 403 with loginRequired:false.
//
// Same-account scoping for the data path uses the same device-owner
// construction as the catalog (ownerKey embeds the caller id), so a caller
// can only ever address their own account's host — no method may publish
// another account's data. The transport routes the search to the caller's
// own host; this file never sees tokens, and CLI failures are classified
// into fixed user-safe messages (raw CLI output, which could contain
// credentials or internal errors, is never returned and never logged).
//
// Linear is explicitly out of scope: no desktop integration exists, so any
// linear.* method is rejected with a user-safe message.
package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
)

// Relay provider method names (R0 provider table, R3).
const (
	ProviderMethodGitHubListWorkItems = "github.listWorkItems"
	ProviderMethodGitLabListWorkItems = "gitlab.listWorkItems"
)

// Provider bounds. Params JSON follows the 64KB server cap from the R0
// spec; list results are bounded like the other relay listings (default 50,
// hard max 200); CLI output is capped like the host's 8MB response limit.
const (
	providerMaxParamsBytes = 64 * 1024
	providerMaxQueryLength = 512
	providerMaxScopeLength = 256
	providerDefaultLimit   = 50
	providerMaxItems       = 200
	providerMaxOutputBytes = 8 * 1024 * 1024
	providerCLITimeout     = 50 * time.Second
)

// Fixed user-safe provider messages. Nothing returned to the phone may
// contain tokens, credentialed URLs, or internal errors.
const (
	providerMsgLoginRequired    = "login required for relay providers"
	providerMsgDeviceMismatch   = "this device is not authorized for relay providers"
	providerMsgVerifyFailed     = "could not verify device"
	providerMsgBadRequest       = "invalid provider request"
	providerMsgTooLarge         = "provider request is too large"
	providerMsgUnavailable      = "provider lookup is unavailable on this host"
	providerMsgAuthRequired     = "provider authentication required: sign in with the CLI on the host (gh auth login or glab auth login)"
	providerMsgNotFound         = "repository or project not found on the provider"
	providerMsgTimeout          = "provider request timed out: check the host connection and try again"
	providerMsgTooBig           = "provider response was too large: narrow the search and try again"
	providerMsgFailed           = "provider request failed: try again"
	providerMsgLinearOutOfScope = "linear is out of scope for relay providers: no desktop integration exists"
	providerMsgUnknownMethod    = "unknown provider method"
)

// ProviderWorkItem is one inbox row from a provider: an issue or a merge
// request / pull request. JSON names match the R0 provider table exactly.
type ProviderWorkItem struct {
	ID     string `json:"id"`
	Number int64  `json:"number"`
	Title  string `json:"title"`
	URL    string `json:"url"`
	State  string `json:"state"`
	Repo   string `json:"repo"`
}

// Validate checks the wire shape of one emitted item. States are the
// canonical lowercase set (normalizers map opened->open etc. before this).
func (w ProviderWorkItem) Validate() error {
	if strings.TrimSpace(w.ID) == "" {
		return fmt.Errorf("work item id is required")
	}
	if len(w.ID) > providerMaxScopeLength {
		return fmt.Errorf("work item id is too long")
	}
	if w.Number <= 0 {
		return fmt.Errorf("work item number is required")
	}
	if strings.TrimSpace(w.Title) == "" {
		return fmt.Errorf("work item title is required")
	}
	if len(w.Title) > 1024 {
		return fmt.Errorf("work item title is too long")
	}
	if err := validateProviderURL(w.URL); err != nil {
		return err
	}
	switch strings.ToLower(strings.TrimSpace(w.State)) {
	case "open", "closed", "merged":
	default:
		return fmt.Errorf("work item state must be open, closed, or merged")
	}
	if strings.TrimSpace(w.Repo) == "" {
		return fmt.Errorf("work item repo is required")
	}
	if len(w.Repo) > providerMaxScopeLength {
		return fmt.Errorf("work item repo is too long")
	}
	return nil
}

func validateProviderURL(raw string) error {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return fmt.Errorf("work item url is required")
	}
	if len(raw) > 2048 {
		return fmt.Errorf("work item url is too long")
	}
	u, err := url.Parse(raw)
	if err != nil {
		return fmt.Errorf("work item url is invalid")
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return fmt.Errorf("work item url is invalid")
	}
	if u.Host == "" {
		return fmt.Errorf("work item url is invalid")
	}
	// Credentialed URLs must never be emitted: tokens and passwords must not
	// cross the relay in either direction.
	if u.User != nil {
		return fmt.Errorf("work item url is invalid")
	}
	return nil
}

// providerGitHubParams is the {query?, repo?} search for github.listWorkItems.
type providerGitHubParams struct {
	Query string `json:"query,omitempty"`
	Repo  string `json:"repo,omitempty"`
}

// providerGitLabParams is the {query?, project?} search for gitlab.listWorkItems.
type providerGitLabParams struct {
	Query   string `json:"query,omitempty"`
	Project string `json:"project,omitempty"`
}

func (p providerGitHubParams) normalized() (query, repo string, err error) {
	query, err = normalizeProviderQuery(p.Query)
	if err != nil {
		return "", "", err
	}
	repo, err = normalizeProviderScope(p.Repo)
	if err != nil {
		return "", "", err
	}
	return query, repo, nil
}

func (p providerGitLabParams) normalized() (query, project string, err error) {
	query, err = normalizeProviderQuery(p.Query)
	if err != nil {
		return "", "", err
	}
	project, err = normalizeProviderScope(p.Project)
	if err != nil {
		return "", "", err
	}
	return query, project, nil
}

// normalizeProviderQuery trims the free-text search. Blank means absent.
// Newlines and NULs are rejected (log splitting / argv confusion); everything
// else travels as a single argv element (no shell), with `--`/`--flag=`
// forms used at the CLI boundary so leading dashes stay literal.
func normalizeProviderQuery(q string) (string, error) {
	q = strings.TrimSpace(q)
	if len(q) > providerMaxQueryLength {
		return "", fmt.Errorf("query is too long")
	}
	if strings.ContainsAny(q, "\x00\n\r") {
		return "", fmt.Errorf("query is invalid")
	}
	return q, nil
}

// normalizeProviderScope validates the repo/project selector: either a short
// OWNER/REPO (GROUP/.../REPO) path or an https URL without credentials.
// Blank means absent (search everywhere the CLI credential allows). The
// whitelist keeps CLI metacharacters and credentialed URLs out; values are
// passed as `--flag=value` argv elements, never a shell string.
func normalizeProviderScope(s string) (string, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return "", nil
	}
	if len(s) > providerMaxScopeLength {
		return "", fmt.Errorf("repository scope is too long")
	}
	if strings.ContainsAny(s, "\x00\n\r \t") {
		return "", fmt.Errorf("repository scope is invalid")
	}
	if strings.HasPrefix(s, "-") {
		return "", fmt.Errorf("repository scope is invalid")
	}
	if strings.Contains(s, "://") {
		u, err := url.Parse(s)
		if err != nil {
			return "", fmt.Errorf("repository scope is invalid")
		}
		if u.Scheme != "http" && u.Scheme != "https" {
			return "", fmt.Errorf("repository scope is invalid")
		}
		if u.Host == "" {
			return "", fmt.Errorf("repository scope is invalid")
		}
		if u.User != nil {
			return "", fmt.Errorf("repository scope must not contain credentials")
		}
		return s, nil
	}
	for _, r := range s {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' ||
			r == '_' || r == '.' || r == '-' || r == '/' || r == ':' {
			continue
		}
		return "", fmt.Errorf("repository scope is invalid")
	}
	return s, nil
}

// ProviderItemsResult is the {items:[...]} result for both provider methods.
// Items is never nil on the wire: empty results encode as [].
type ProviderItemsResult struct {
	Items []ProviderWorkItem `json:"items"`
}

// normalizedProviderResult bounds and sanitizes executor output before it
// reaches the phone: invalid rows are skipped (one malformed provider row
// must not fail the whole inbox), the list is truncated to the server cap,
// and nil becomes [].
func normalizedProviderResult(items []ProviderWorkItem) ProviderItemsResult {
	out := make([]ProviderWorkItem, 0, len(items))
	for _, w := range items {
		w.State = strings.ToLower(strings.TrimSpace(w.State))
		if err := w.Validate(); err != nil {
			continue
		}
		out = append(out, w)
		if len(out) >= providerMaxItems {
			break
		}
	}
	return ProviderItemsResult{Items: out}
}

// ProviderRPCRequest is one R0 rpc-request for a provider method.
type ProviderRPCRequest struct {
	Type   string          `json:"type"`
	V      int             `json:"v"`
	ID     string          `json:"id"`
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
}

// ProviderRPCError is the user-safe {message, loginRequired} error. Status
// carries the HTTP mapping for REST handlers and is never serialized.
type ProviderRPCError struct {
	Message       string `json:"message"`
	LoginRequired bool   `json:"loginRequired"`
	Status        int    `json:"-"`
}

// ProviderRPCReply is one R0 rpc-reply answering the request id.
type ProviderRPCReply struct {
	Type   string               `json:"type"`
	V      int                  `json:"v"`
	ID     string               `json:"id"`
	OK     bool                 `json:"ok"`
	Result *ProviderItemsResult `json:"result,omitempty"`
	Error  *ProviderRPCError    `json:"error,omitempty"`
}

func providerFail(id string, status int, msg string, loginRequired bool) ProviderRPCReply {
	return ProviderRPCReply{
		Type: "rpc-reply", V: 1, ID: id, OK: false,
		Error: &ProviderRPCError{Message: msg, LoginRequired: loginRequired, Status: status},
	}
}

// ProviderDeviceResolver answers whether a caller device belongs to a user.
// Production wires this to the devices table (cf. the push package's device
// resolver); tests use a stub. A nil resolver disables the ownership check
// (user check still applies) for single-instance dev.
type ProviderDeviceResolver interface {
	DeviceOwnedBy(ctx context.Context, userID, deviceID string) (bool, error)
}

// authorizeProviderCaller enforces the R0 server rules: anonymous callers
// are rejected with 401 semantics (loginRequired:true); a caller device that
// is not owned by the caller is rejected with 403 (loginRequired:false).
func authorizeProviderCaller(ctx context.Context, resolver ProviderDeviceResolver) (string, *ProviderRPCError) {
	userID, _ := ctx.Value("user_id").(string)
	if strings.TrimSpace(userID) == "" {
		return "", &ProviderRPCError{Message: providerMsgLoginRequired, LoginRequired: true, Status: http.StatusUnauthorized}
	}
	deviceID, _ := ctx.Value("device_id").(string)
	deviceID = strings.TrimSpace(deviceID)
	if deviceID == "" || resolver == nil {
		return userID, nil
	}
	owned, err := resolver.DeviceOwnedBy(ctx, userID, deviceID)
	if err != nil {
		return "", &ProviderRPCError{Message: providerMsgVerifyFailed, Status: http.StatusInternalServerError}
	}
	if !owned {
		return "", &ProviderRPCError{Message: providerMsgDeviceMismatch, Status: http.StatusForbidden}
	}
	return userID, nil
}

// ProviderExecutor runs a provider search through the host's CLIs and
// credentials. Only search parameters cross the boundary: implementations
// must never accept, return, or log tokens.
type ProviderExecutor interface {
	ListGitHubItems(ctx context.Context, query, repo string) ([]ProviderWorkItem, error)
	ListGitLabItems(ctx context.Context, query, project string) ([]ProviderWorkItem, error)
}

// DispatchProviderRPC answers one R0 rpc-request for the R3 provider table.
// It performs authorization first, then param validation, then the executor
// call; CLI failures are classified into user-safe messages (Status 502).
// Linear is rejected as out of scope; unknown methods get a user-safe error.
func DispatchProviderRPC(ctx context.Context, req ProviderRPCRequest, exec ProviderExecutor, resolver ProviderDeviceResolver) ProviderRPCReply {
	if req.Type != "" && req.Type != "rpc-request" {
		return providerFail(req.ID, http.StatusBadRequest, providerMsgBadRequest, false)
	}
	if req.V != 1 {
		return providerFail(req.ID, http.StatusBadRequest, providerMsgBadRequest, false)
	}
	if _, rpcErr := authorizeProviderCaller(ctx, resolver); rpcErr != nil {
		return ProviderRPCReply{Type: "rpc-reply", V: 1, ID: req.ID, OK: false, Error: rpcErr}
	}
	if len(req.Params) > providerMaxParamsBytes {
		return providerFail(req.ID, http.StatusBadRequest, providerMsgTooLarge, false)
	}

	var items []ProviderWorkItem
	switch req.Method {
	case ProviderMethodGitHubListWorkItems:
		var p providerGitHubParams
		if err := decodeProviderParams(req.Params, &p); err != nil {
			return providerFail(req.ID, http.StatusBadRequest, providerMsgBadRequest, false)
		}
		query, repo, err := p.normalized()
		if err != nil {
			return providerFail(req.ID, http.StatusBadRequest, err.Error(), false)
		}
		if exec == nil {
			return providerFail(req.ID, http.StatusBadGateway, providerMsgUnavailable, false)
		}
		items, err = exec.ListGitHubItems(ctx, query, repo)
		if err != nil {
			return providerFail(req.ID, http.StatusBadGateway, sanitizeProviderError(err.Error()), false)
		}
	case ProviderMethodGitLabListWorkItems:
		var p providerGitLabParams
		if err := decodeProviderParams(req.Params, &p); err != nil {
			return providerFail(req.ID, http.StatusBadRequest, providerMsgBadRequest, false)
		}
		query, project, err := p.normalized()
		if err != nil {
			return providerFail(req.ID, http.StatusBadRequest, err.Error(), false)
		}
		if exec == nil {
			return providerFail(req.ID, http.StatusBadGateway, providerMsgUnavailable, false)
		}
		items, err = exec.ListGitLabItems(ctx, query, project)
		if err != nil {
			return providerFail(req.ID, http.StatusBadGateway, sanitizeProviderError(err.Error()), false)
		}
	case "linear.listWorkItems", "linear.search", "linear.listIssues":
		return providerFail(req.ID, http.StatusBadRequest, providerMsgLinearOutOfScope, false)
	default:
		if strings.HasPrefix(req.Method, "linear.") {
			return providerFail(req.ID, http.StatusBadRequest, providerMsgLinearOutOfScope, false)
		}
		return providerFail(req.ID, http.StatusBadRequest, providerMsgUnknownMethod, false)
	}

	result := normalizedProviderResult(items)
	return ProviderRPCReply{Type: "rpc-reply", V: 1, ID: req.ID, OK: true, Result: &result}
}

func decodeProviderParams(raw json.RawMessage, v any) error {
	if len(bytes.TrimSpace(raw)) == 0 {
		return nil
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	if err := dec.Decode(v); err != nil {
		return err
	}
	return nil
}

// sanitizeProviderError maps raw CLI failures to fixed user-safe messages.
// The raw text — which may contain tokens, credentialed URLs, or internal
// errors — is only ever classified in memory: it is never returned and
// never logged.
func sanitizeProviderError(raw string) string {
	lowered := strings.ToLower(redactProviderCredentials(raw))
	switch {
	case strings.Contains(lowered, "auth") ||
		strings.Contains(lowered, "login") ||
		strings.Contains(lowered, "sign in") ||
		strings.Contains(lowered, "token") ||
		strings.Contains(lowered, "credential") ||
		strings.Contains(lowered, "401") ||
		strings.Contains(lowered, "403") ||
		strings.Contains(lowered, "forbidden") ||
		strings.Contains(lowered, "unauthorized"):
		return providerMsgAuthRequired
	case strings.Contains(lowered, "timeout") ||
		strings.Contains(lowered, "timed out") ||
		strings.Contains(lowered, "deadline") ||
		strings.Contains(lowered, "network") ||
		strings.Contains(lowered, "connection") ||
		strings.Contains(lowered, "offline") ||
		strings.Contains(lowered, "no route to host") ||
		strings.Contains(lowered, "could not resolve host"):
		return providerMsgTimeout
	case strings.Contains(lowered, "not found") ||
		strings.Contains(lowered, "404") ||
		strings.Contains(lowered, "no such") ||
		strings.Contains(lowered, "could not resolve") ||
		strings.Contains(lowered, "unknown repo") ||
		strings.Contains(lowered, "unknown project"):
		return providerMsgNotFound
	case strings.Contains(lowered, "too large") ||
		strings.Contains(lowered, "exceeds"):
		return providerMsgTooBig
	default:
		return providerMsgFailed
	}
}

// redactProviderCredentials strips credential-shaped substrings (userinfo in
// URLs, token assignments) before classification, as defense in depth. The
// result is still only classified, never surfaced.
func redactProviderCredentials(s string) string {
	if i := strings.Index(s, "://"); i >= 0 {
		if at := strings.Index(s[i+3:], "@"); at >= 0 {
			if slash := strings.Index(s[i+3:], "/"); slash < 0 || at < slash {
				s = s[:i+3] + "***@" + s[i+3+at+1:]
			}
		}
	}
	for _, key := range []string{"token=", "token:", "bearer ", "password=", "password:"} {
		searchFrom := 0
		for searchFrom < len(s) {
			i := strings.Index(strings.ToLower(s[searchFrom:]), key)
			if i < 0 {
				break
			}
			i += searchFrom
			end := i + len(key)
			for end < len(s) && s[end] != ' ' && s[end] != '\n' && s[end] != '"' && s[end] != '\'' {
				end++
			}
			s = s[:i+len(key)] + "***" + s[end:]
			// Continue past the redacted marker: the key prefix itself
			// remains, so re-scanning from the same offset would loop.
			searchFrom = i + len(key) + len("***")
		}
	}
	return s
}

// cliProviderRunner executes one CLI (argv only, no shell) with the host's
// own credentials. It mirrors the host modules' non-interactive environment
// (prompts and pagers off) and caps output. Raw output is returned for
// parsing/classification only — callers must sanitize before surfacing.
type cliProviderRunner func(ctx context.Context, program string, args ...string) (string, error)

// CLIProviderExecutor lists work items through the host's gh/glab CLIs,
// following the existing host-side provider modules: `gh search issues`
// (with --include-prs, like the desktop's PR overview) and `glab issue list`
// + `glab mr list` merged. Auth comes from the CLI stores on the machine;
// only search parameters are passed.
type CLIProviderExecutor struct {
	run cliProviderRunner
}

// NewCLIProviderExecutor returns an executor that shells out to gh/glab.
func NewCLIProviderExecutor() *CLIProviderExecutor {
	return &CLIProviderExecutor{run: runProviderCLI}
}

func runProviderCLI(ctx context.Context, program string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, providerCLITimeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, program, args...)
	cmd.Env = providerCLIEnv()
	var stdout, stderr bytes.Buffer
	// Cap each stream; the +1 detects overflow without buffering the excess.
	cmd.Stdout = &limitedProviderBuffer{buf: &stdout, limit: providerMaxOutputBytes + 1}
	cmd.Stderr = &limitedProviderBuffer{buf: &stderr, limit: 64 * 1024}
	if err := cmd.Run(); err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return "", fmt.Errorf("provider CLI timed out")
		}
		msg := strings.TrimSpace(stderr.String())
		if msg == "" {
			msg = strings.TrimSpace(stdout.String())
		}
		if msg == "" {
			msg = "provider CLI failed"
		}
		return "", fmt.Errorf("%s", msg)
	}
	if stdout.Len() > providerMaxOutputBytes {
		return "", fmt.Errorf("provider response too large")
	}
	return stdout.String(), nil
}

// providerCLIEnv mirrors the host modules' non-interactive CLI environment
// (dragent pull_requests.rs run_process): prompts and pagers off, and the
// repo-forcing variables removed so -R/--repo selections apply. Tokens are
// neither added nor removed — the CLIs authenticate from their own stores.
func providerCLIEnv() []string {
	env := os.Environ()
	out := make([]string, 0, len(env)+8)
	for _, kv := range env {
		if strings.HasPrefix(kv, "GH_REPO=") || strings.HasPrefix(kv, "GH_FORCE_TTY=") {
			continue
		}
		out = append(out, kv)
	}
	out = append(out,
		"NO_COLOR=1",
		"GH_PROMPT_DISABLED=1",
		"GLAB_NO_PROMPT=1",
		"GLAB_PAGER=cat",
		"GIT_TERMINAL_PROMPT=0",
		"GH_PAGER=",
		"PAGER=cat",
	)
	return out
}

type limitedProviderBuffer struct {
	buf   *bytes.Buffer
	limit int
}

func (l *limitedProviderBuffer) Write(p []byte) (int, error) {
	remaining := l.limit - l.buf.Len()
	if remaining <= 0 {
		return len(p), nil
	}
	if len(p) > remaining {
		p = p[:remaining]
	}
	return l.buf.Write(p)
}

// ListGitHubItems is removed after the E2EE cut-over (F6): the desktop
// answers provider searches inside the KK channel with its own CLIs. The
// relay never shells out.
func (e *CLIProviderExecutor) ListGitHubItems(_ context.Context, _, _ string) ([]ProviderWorkItem, error) {
	return nil, errors.New(providerMsgUnavailable)
}

// ListGitLabItems is removed after the E2EE cut-over (F6): the desktop
// answers provider searches inside the KK channel with its own CLIs. The
// relay never shells out.
func (e *CLIProviderExecutor) ListGitLabItems(_ context.Context, _, _ string) ([]ProviderWorkItem, error) {
	return nil, errors.New(providerMsgUnavailable)
}

// providerJSONValue reads tolerant JSON: objects decode into maps, missing or
// malformed documents become empty (empty-result handling, never an error
// for shape drift the normalizers already tolerate).
func providerJSONDocs(raw []byte) []map[string]any {
	var docs []map[string]any
	dec := json.NewDecoder(bytes.NewReader(bytes.TrimSpace(raw)))
	if err := dec.Decode(&docs); err != nil {
		return nil
	}
	if docs == nil {
		return []map[string]any{}
	}
	return docs
}

func providerString(m map[string]any, keys ...string) string {
	for _, k := range keys {
		if s, ok := m[k].(string); ok && strings.TrimSpace(s) != "" {
			return strings.TrimSpace(s)
		}
	}
	return ""
}

func providerInt(m map[string]any, keys ...string) int64 {
	for _, k := range keys {
		switch v := m[k].(type) {
		case float64:
			if v > 0 {
				return int64(v)
			}
		case int64:
			if v > 0 {
				return v
			}
		case json.Number:
			if n, err := v.Int64(); err == nil && n > 0 {
				return n
			}
		}
	}
	return 0
}

// providerRepoName accepts the repository/project reference in whatever
// shape the CLI emitted: a plain string, or an object carrying a
// nameWithOwner/name/full_name/fullName/path_with_namespace value.
func providerRepoName(v any, fallback string) string {
	switch t := v.(type) {
	case string:
		if strings.TrimSpace(t) != "" {
			return strings.TrimSpace(t)
		}
	case map[string]any:
		for _, k := range []string{"nameWithOwner", "path_with_namespace", "full_name", "fullName", "name"} {
			if s, ok := t[k].(string); ok && strings.TrimSpace(s) != "" {
				return strings.TrimSpace(s)
			}
		}
	}
	return fallback
}

// normalizeGitHubState maps provider state spellings to the canonical set;
// unknown states report false and the row is skipped.
func normalizeGitHubState(s string) (string, bool) {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "open", "opened":
		return "open", true
	case "closed":
		return "closed", true
	case "merged":
		return "merged", true
	default:
		return "", false
	}
}

// normalizeGitHubSearchItems converts `gh search issues --json
// id,number,title,url,state,repository` rows to work items. Rows missing
// number/title/url, or with unknown states, are skipped; an empty or
// unparseable document yields an empty (non-nil) list.
func normalizeGitHubSearchItems(raw []byte, fallbackRepo string) ([]ProviderWorkItem, error) {
	docs := providerJSONDocs(raw)
	items := make([]ProviderWorkItem, 0, len(docs))
	for _, d := range docs {
		number := providerInt(d, "number")
		title := providerString(d, "title")
		u := providerString(d, "url", "web_url", "html_url")
		state, ok := normalizeGitHubState(providerString(d, "state"))
		if number <= 0 || title == "" || u == "" || !ok {
			continue
		}
		id := providerString(d, "id")
		if id == "" {
			id = fmt.Sprintf("github:%s#%d", strings.TrimSpace(fallbackRepo), number)
			if strings.TrimSpace(fallbackRepo) == "" {
				id = fmt.Sprintf("github#%d", number)
			}
		}
		repo := providerRepoName(d["repository"], fallbackRepo)
		items = append(items, ProviderWorkItem{
			ID: id, Number: number, Title: title, URL: u, State: state, Repo: repo,
		})
	}
	return items, nil
}

// normalizeGitLabRefsRepo derives owner/repo from a references.full value
// like "group/sub/project#12".
func normalizeGitLabRefsRepo(full, fallback string) string {
	if i := strings.LastIndex(full, "#"); i >= 0 {
		full = full[:i]
	}
	if strings.TrimSpace(full) != "" {
		return strings.TrimSpace(full)
	}
	return fallback
}

// normalizeGitLabIssueItems converts `glab issue list -O json` rows (iid,
// title, web_url, state opened/closed, project references) to work items.
func normalizeGitLabIssueItems(raw []byte, fallbackProject string) ([]ProviderWorkItem, error) {
	docs := providerJSONDocs(raw)
	items := make([]ProviderWorkItem, 0, len(docs))
	for _, d := range docs {
		number := providerInt(d, "iid", "number")
		title := providerString(d, "title")
		u := providerString(d, "web_url", "url", "html_url")
		state, ok := normalizeGitHubState(providerString(d, "state"))
		if number <= 0 || title == "" || u == "" || !ok {
			continue
		}
		id := providerString(d, "id")
		if id == "" {
			id = fmt.Sprintf("gitlab:%s#%d", strings.TrimSpace(fallbackProject), number)
			if strings.TrimSpace(fallbackProject) == "" {
				id = fmt.Sprintf("gitlab#%d", number)
			}
		}
		repo := fallbackProject
		if refs, ok := d["references"].(map[string]any); ok {
			repo = normalizeGitLabRefsRepo(providerString(refs, "full"), fallbackProject)
		}
		if repo == "" {
			repo = providerRepoName(d["project"], fallbackProject)
		}
		items = append(items, ProviderWorkItem{
			ID: id, Number: number, Title: title, URL: u, State: state, Repo: repo,
		})
	}
	return items, nil
}

// normalizeGitLabMRItems converts `glab mr list -F json` rows the same way;
// merge-request states (opened/closed/merged) map through the same table.
func normalizeGitLabMRItems(raw []byte, fallbackProject string) ([]ProviderWorkItem, error) {
	docs := providerJSONDocs(raw)
	items := make([]ProviderWorkItem, 0, len(docs))
	for _, d := range docs {
		number := providerInt(d, "iid", "number")
		title := providerString(d, "title")
		u := providerString(d, "web_url", "url", "html_url")
		state, ok := normalizeGitHubState(providerString(d, "state"))
		if number <= 0 || title == "" || u == "" || !ok {
			continue
		}
		id := providerString(d, "id")
		if id == "" {
			id = fmt.Sprintf("gitlab:%s!%d", strings.TrimSpace(fallbackProject), number)
			if strings.TrimSpace(fallbackProject) == "" {
				id = fmt.Sprintf("gitlab!%d", number)
			}
		}
		repo := fallbackProject
		if refs, ok := d["references"].(map[string]any); ok {
			repo = normalizeGitLabRefsRepo(providerString(refs, "full"), fallbackProject)
		}
		if repo == "" {
			repo = providerRepoName(d["project"], fallbackProject)
		}
		items = append(items, ProviderWorkItem{
			ID: id, Number: number, Title: title, URL: u, State: state, Repo: repo,
		})
	}
	return items, nil
}

// ProviderHandler serves the provider searches over REST for hosts that have
// not yet moved to the relay socket: same routes the socket dispatcher
// covers, same auth, same user-safe errors. Mount under RequireAuth; each
// handler additionally rejects requests without a user id.
type ProviderHandler struct {
	Exec     ProviderExecutor
	Resolver ProviderDeviceResolver
}

// RegisterProviderRoutes mounts the provider work-item endpoints.
func (h *ProviderHandler) RegisterProviderRoutes(r chi.Router) {
	r.Post("/v1/relay/providers/github/work-items", h.serveGitHubWorkItems)
	r.Post("/v1/relay/providers/gitlab/work-items", h.serveGitLabWorkItems)
}

func (h *ProviderHandler) callerError(w http.ResponseWriter, r *http.Request) (string, bool) {
	if _, rpcErr := authorizeProviderCaller(r.Context(), h.Resolver); rpcErr != nil {
		writeErr(w, rpcErr.Status, rpcErr.Message)
		return "", false
	}
	userID, _ := r.Context().Value("user_id").(string)
	return userID, true
}

func (h *ProviderHandler) serveGitHubWorkItems(w http.ResponseWriter, r *http.Request) {
	// E2EE cut-over, F6 (§10.5): the REST provider helpers are removed. The
	// desktop answers provider searches inside the KK channel with its own
	// CLIs. Authenticated callers answer 410 before the body is read (no
	// device distinction, no oracle); anonymous callers answer 401.
	if r.Context().Value("user_id") == nil {
		writeErr(w, http.StatusUnauthorized, providerMsgLoginRequired)
		return
	}
	writeErr(w, http.StatusGone, "relay providers are end-to-end encrypted: update the app")
}

func (h *ProviderHandler) serveGitLabWorkItems(w http.ResponseWriter, r *http.Request) {
	// E2EE cut-over, F6 (§10.5): the REST provider helpers are removed. The
	// desktop answers provider searches inside the KK channel with its own
	// CLIs. Authenticated callers answer 410 before the body is read (no
	// device distinction, no oracle); anonymous callers answer 401.
	if r.Context().Value("user_id") == nil {
		writeErr(w, http.StatusUnauthorized, providerMsgLoginRequired)
		return
	}
	writeErr(w, http.StatusGone, "relay providers are end-to-end encrypted: update the app")
}

func decodeProviderBody(w http.ResponseWriter, r *http.Request, v any) bool {
	r.Body = http.MaxBytesReader(w, r.Body, providerMaxParamsBytes)
	if r.Body == nil {
		writeErr(w, http.StatusBadRequest, providerMsgBadRequest)
		return false
	}
	dec := json.NewDecoder(r.Body)
	// An empty body means "no filters": list with defaults.
	if err := dec.Decode(v); err != nil {
		// io.EOF (empty body) is valid; anything else is malformed.
		if !errors.Is(err, io.EOF) {
			writeErr(w, http.StatusBadRequest, providerMsgBadRequest)
			return false
		}
	}
	return true
}
