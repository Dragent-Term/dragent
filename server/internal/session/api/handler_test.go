package api

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/session/components"
	"server/internal/session/types"
)

func testServer(t *testing.T) (*httptest.Server, string) {
	t.Helper()
	return testServerWithLookup(t, nil)
}

func testServerWithLookup(t *testing.T, lookup UserLookup) (*httptest.Server, string) {
	t.Helper()
	h := NewHandler(
		components.NewHub(components.NewMemoryBus(), components.AllowAll{}, components.NoopAuditor{}),
		components.NewMemoryStore(),
		"https://app.test",
		nil, // no inviter — email is exercised separately
		lookup,
	)
	r := chi.NewRouter()
	// Public WS: soft auth — a user id is set only when X-Test-User is present, so
	// an anonymous dial (no header) stays anonymous (mirrors the real gateway).
	r.Group(func(rr chi.Router) {
		rr.Use(func(next http.Handler) http.Handler {
			return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
				if user := req.Header.Get("X-Test-User"); user != "" {
					req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
				}
				next.ServeHTTP(w, req)
			})
		})
		h.RegisterPublicRoutes(rr)
	})
	// Authed REST: X-Test-User acts as the signed-in account (defaults to "test-user").
	r.Group(func(rr chi.Router) {
		rr.Use(func(next http.Handler) http.Handler {
			return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
				user := req.Header.Get("X-Test-User")
				if user == "" {
					user = "test-user"
				}
				ctx := context.WithValue(req.Context(), "user_id", user)
				next.ServeHTTP(w, req.WithContext(ctx))
			})
		})
		h.RegisterRoutes(rr)
	})
	srv := httptest.NewServer(r)
	wsURL := "ws" + strings.TrimPrefix(srv.URL, "http") + "/realtime"
	return srv, wsURL
}

func TestForwardIDFromHost(t *testing.T) {
	h := &Handler{}
	h.SetForwardDomain("dragent.dev")
	want := "calm-a1b2c3"

	for _, host := range []string{want + ".dragent.dev", want + ".dragent.dev:443"} {
		got, ok := h.forwardIDFromHost(host)
		if !ok || got != want {
			t.Fatalf("forwardIDFromHost(%q) = %q, %v; want %q, true", host, got, ok, want)
		}
	}
	if _, ok := h.forwardIDFromHost("beta.dragent.dev"); ok {
		t.Fatal("non-UUID host should not be treated as a forward")
	}
}

func TestWSSinkBuffersFrontendModuleBurst(t *testing.T) {
	sink := newWSSink()
	for i := 0; i < cap(sink.ch); i++ {
		sink.Send([]byte("forward-open"))
	}
	if got := len(sink.ch); got != cap(sink.ch) {
		t.Fatalf("forward burst queued %d frames, want %d", got, cap(sink.ch))
	}
}

func TestForwardedRequestURIRemovesCapability(t *testing.T) {
	u, err := url.Parse("/__vite_ping?secret=private&token=hmr%201&x=1")
	if err != nil {
		t.Fatal(err)
	}
	if got, want := forwardedRequestURI(u), "/__vite_ping?token=hmr+1&x=1"; got != want {
		t.Fatalf("forwardedRequestURI() = %q, want %q", got, want)
	}
}

func TestForwardStreamHeadersPreserveUpgrade(t *testing.T) {
	headers := forwardStreamHeaders(http.Header{
		"Connection":             {"Upgrade"},
		"Upgrade":                {"websocket"},
		"Sec-WebSocket-Key":      {"test-key"},
		"Sec-WebSocket-Protocol": {"vite-hmr"},
		"Host":                   {"public.example"},
		"Proxy-Connection":       {"keep-alive"},
		"Forwarded":              {"host=public.example;proto=https"},
		"X-Forwarded-For":        {"203.0.113.1"},
		"X-Forwarded-Host":       {"public.example"},
		"X-Forwarded-Proto":      {"https"},
		"X-Real-IP":              {"203.0.113.1"},
	})
	got := make(map[string]string, len(headers))
	for _, header := range headers {
		got[header[0]] = header[1]
	}
	for _, name := range []string{"Connection", "Upgrade", "Sec-WebSocket-Key", "Sec-WebSocket-Protocol"} {
		if got[name] == "" {
			t.Errorf("forward stream omitted %q", name)
		}
	}
	for _, name := range []string{
		"Host", "Proxy-Connection", "Forwarded", "X-Forwarded-For",
		"X-Forwarded-Host", "X-Forwarded-Proto", "X-Real-IP",
	} {
		if got[name] != "" {
			t.Errorf("forward stream retained %q", name)
		}
	}
}

func TestForwardStreamRelaysChunkedResponse(t *testing.T) {
	hub := components.NewHub(components.NewMemoryBus(), components.AllowAll{}, components.NoopAuditor{})
	h := NewHandler(hub, components.NewMemoryStore(), "https://app.test", nil, nil)
	router := chi.NewRouter()
	router.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			user := req.Header.Get("X-Test-User")
			if user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterPublicRoutes(router)
	srv := httptest.NewServer(router)
	defer srv.Close()
	wsURL := "ws" + strings.TrimPrefix(srv.URL, "http") + "/realtime"

	host, ctx, initialized := openHost(t, wsURL)
	defer host.Close(websocket.StatusNormalClosure, "")
	forward, err := h.forwards.Create(initialized.Session.String(), 5173, "Vite")
	if err != nil {
		t.Fatal(err)
	}
	h.hub.RegisterForward(initialized.Session.String(), forward)

	type result struct {
		response *http.Response
		err      error
	}
	resultCh := make(chan result, 1)
	go func() {
		req, err := http.NewRequest(http.MethodGet, srv.URL+"/forward/"+forward.ID+"/", nil)
		if err == nil {
			req.AddCookie(&http.Cookie{Name: "__Host-dragent_forward", Value: forward.Token})
			response, requestErr := http.DefaultClient.Do(req)
			resultCh <- result{response: response, err: requestErr}
			return
		}
		resultCh <- result{err: err}
	}()

	typ, data := recvType(t, ctx, host)
	if typ != types.TypeForwardWebSocketOpen {
		t.Fatalf("want forward_web_socket_open, got %q", typ)
	}
	var opened types.ServerForwardWebSocketOpen
	if err := json.Unmarshal(data, &opened); err != nil {
		t.Fatalf("decode forward stream open: %v", err)
	}
	if opened.Method != http.MethodGet || opened.Path != "/" {
		t.Fatalf("forwarded request = %s %s, want GET /", opened.Method, opened.Path)
	}
	send(t, ctx, host, types.HostForwardWebSocketOpen{
		Type: types.TypeForwardWebSocketOpened, RequestID: opened.RequestID,
	})
	for _, chunk := range []string{
		"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n",
		"6\r\n world\r\n0\r\n\r\n",
	} {
		send(t, ctx, host, types.HostForwardWebSocketData{
			Type: types.TypeForwardWebSocketMessage, RequestID: opened.RequestID,
			MessageType: "data", Data: base64.StdEncoding.EncodeToString([]byte(chunk)),
		})
	}
	send(t, ctx, host, types.HostForwardWebSocketClose{
		Type: types.TypeForwardWebSocketClosed, RequestID: opened.RequestID,
	})

	select {
	case got := <-resultCh:
		if got.err != nil {
			t.Fatalf("forward stream request: %v", got.err)
		}
		defer got.response.Body.Close()
		body, err := io.ReadAll(got.response.Body)
		if err != nil {
			t.Fatalf("read streamed response: %v", err)
		}
		if text := string(body); text != "hello world" {
			t.Fatalf("streamed response body = %q, want hello world", text)
		}
		if got := got.response.Header.Get("Content-Security-Policy"); got != forwardSandboxPolicy {
			t.Fatalf("Content-Security-Policy = %q, want %q", got, forwardSandboxPolicy)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("forward stream response timed out")
	}
}

func TestDedicatedForwardTunnelRelaysBinaryResponse(t *testing.T) {
	hub := components.NewHub(components.NewMemoryBus(), components.AllowAll{}, components.NoopAuditor{})
	h := NewHandler(hub, components.NewMemoryStore(), "https://app.test", nil, nil)
	router := chi.NewRouter()
	router.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterPublicRoutes(router)
	srv := httptest.NewServer(router)
	defer srv.Close()
	wsBase := "ws" + strings.TrimPrefix(srv.URL, "http")

	host, ctx, initialized := openHost(t, wsBase+"/realtime")
	defer host.Close(websocket.StatusNormalClosure, "")
	tunnel, _, err := websocket.Dial(ctx, wsBase+"/realtime/forward?session="+initialized.Session.String(), &websocket.DialOptions{
		HTTPHeader: http.Header{"X-Test-User": []string{"test-user"}},
	})
	if err != nil {
		t.Fatalf("dial forward tunnel: %v", err)
	}
	defer tunnel.Close(websocket.StatusNormalClosure, "")
	forward, err := h.forwards.Create(initialized.Session.String(), 5173, "Vite")
	if err != nil {
		t.Fatal(err)
	}

	type result struct {
		response *http.Response
		err      error
	}
	resultCh := make(chan result, 1)
	go func() {
		req, _ := http.NewRequest(http.MethodGet, srv.URL+"/forward/"+forward.ID+"/app.js", nil)
		req.AddCookie(&http.Cookie{Name: "__Host-dragent_forward", Value: forward.Token})
		response, requestErr := http.DefaultClient.Do(req)
		resultCh <- result{response: response, err: requestErr}
	}()

	messageType, frame, err := tunnel.Read(ctx)
	if err != nil || messageType != websocket.MessageBinary {
		t.Fatalf("read binary tunnel open: type %v, error %v", messageType, err)
	}
	kind, streamID, payload, err := decodeTunnelFrame(frame)
	if err != nil || kind != tunnelFrameOpen {
		t.Fatalf("decode tunnel open: kind %d, error %v", kind, err)
	}
	metadataSize := int(binary.BigEndian.Uint32(payload[:4]))
	var opened forwardTunnelOpen
	if err := json.Unmarshal(payload[4:4+metadataSize], &opened); err != nil {
		t.Fatal(err)
	}
	if opened.Port != 5173 || opened.Method != http.MethodGet || opened.Path != "/app.js" {
		t.Fatalf("tunnel open = %#v", opened)
	}
	for _, responseFrame := range [][]byte{
		encodeTunnelFrame(tunnelFrameOpened, streamID, nil),
		encodeTunnelFrame(tunnelFrameData, streamID, []byte("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")),
		encodeTunnelFrame(tunnelFrameFin, streamID, nil),
	} {
		if err := tunnel.Write(ctx, websocket.MessageBinary, responseFrame); err != nil {
			t.Fatal(err)
		}
	}

	select {
	case got := <-resultCh:
		if got.err != nil {
			t.Fatal(got.err)
		}
		defer got.response.Body.Close()
		body, err := io.ReadAll(got.response.Body)
		if err != nil || string(body) != "hello" {
			t.Fatalf("body = %q, error %v", body, err)
		}
		if got := got.response.Header.Get("Content-Security-Policy"); got != forwardSandboxPolicy {
			t.Fatalf("Content-Security-Policy = %q, want %q", got, forwardSandboxPolicy)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("dedicated forward response timed out")
	}
}

func TestDeviceForwardLifecycleDoesNotRequireSharedSession(t *testing.T) {
	h := NewHandler(
		components.NewHub(components.NewMemoryBus(), components.AllowAll{}, components.NoopAuditor{}),
		components.NewMemoryStore(), "https://app.test", nil, nil,
	)
	router := chi.NewRouter()
	router.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterRoutes(router)
	h.RegisterPublicRoutes(router)
	srv := httptest.NewServer(router)
	defer srv.Close()
	ctx := context.Background()
	tunnel, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(srv.URL, "http")+"/realtime/forward?device=device-1", &websocket.DialOptions{
		HTTPHeader: http.Header{"X-Test-User": []string{"test-user"}},
	})
	if err != nil {
		t.Fatalf("dial device tunnel: %v", err)
	}
	defer tunnel.Close(websocket.StatusNormalClosure, "")

	createdResp := postAs(t, srv, "/v1/forwards", "test-user", map[string]any{
		"device_id": "device-1", "port": 3000, "label": "Frontend",
	})
	defer createdResp.Body.Close()
	if createdResp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(createdResp.Body)
		t.Fatalf("create status %d: %s", createdResp.StatusCode, body)
	}
	var created forwardAPIResponse
	if err := json.NewDecoder(createdResp.Body).Decode(&created); err != nil {
		t.Fatal(err)
	}
	if created.Port != 3000 || created.ID == "" || !strings.Contains(created.PublicURL, "secret=") {
		t.Fatalf("created forward = %#v", created)
	}

	listReq, _ := http.NewRequest(http.MethodGet, srv.URL+"/v1/forwards?device_id=device-1", nil)
	listReq.Header.Set("X-Test-User", "test-user")
	listResp, err := http.DefaultClient.Do(listReq)
	if err != nil {
		t.Fatal(err)
	}
	defer listResp.Body.Close()
	var listed []forwardAPIResponse
	if err := json.NewDecoder(listResp.Body).Decode(&listed); err != nil || len(listed) != 1 || listed[0].ID != created.ID {
		t.Fatalf("listed forwards = %#v, error %v", listed, err)
	}
	otherClose := postAs(t, srv, "/v1/forwards/close", "other-user", map[string]any{
		"device_id": "device-1", "forward_id": created.ID,
	})
	otherClose.Body.Close()
	if otherClose.StatusCode != http.StatusNotFound {
		t.Fatalf("different account closed device forward: status %d", otherClose.StatusCode)
	}

	closedResp := postAs(t, srv, "/v1/forwards/close", "test-user", map[string]any{
		"device_id": "device-1", "forward_id": created.ID,
	})
	defer closedResp.Body.Close()
	if closedResp.StatusCode != http.StatusOK {
		t.Fatalf("close status = %d", closedResp.StatusCode)
	}
}

type fakeUserLookup struct {
	byID    map[string][2]string
	byEmail map[string][2]string
}

func (f fakeUserLookup) SessionProfile(_ context.Context, userID string) (string, string, bool) {
	profile, ok := f.byID[userID]
	return profile[0], profile[1], ok
}

func (f fakeUserLookup) SessionProfileByEmail(_ context.Context, email string) (string, string, bool) {
	profile, ok := f.byEmail[email]
	return profile[0], profile[1], ok
}

func dial(t *testing.T, wsURL string) (*websocket.Conn, context.Context) {
	return dialAs(t, wsURL, "")
}

// dialAs connects as a specific test user (empty = default "test-user").
func dialAs(t *testing.T, wsURL, user string) (*websocket.Conn, context.Context) {
	t.Helper()
	ctx := context.Background()
	var opts *websocket.DialOptions
	if user != "" {
		opts = &websocket.DialOptions{HTTPHeader: http.Header{"X-Test-User": []string{user}}}
	}
	conn, _, err := websocket.Dial(ctx, wsURL, opts)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	return conn, ctx
}

// postAs sends an authenticated JSON POST as a specific test user.
func postAs(t *testing.T, srv *httptest.Server, path, user string, body any) *http.Response {
	t.Helper()
	b, _ := json.Marshal(body)
	req, _ := http.NewRequest(http.MethodPost, srv.URL+path, bytes.NewReader(b))
	req.Header.Set("Content-Type", "application/json")
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("post %s: %v", path, err)
	}
	return resp
}

func send(t *testing.T, ctx context.Context, conn *websocket.Conn, v any) {
	t.Helper()
	data, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if err := conn.Write(ctx, websocket.MessageText, data); err != nil {
		t.Fatalf("write: %v", err)
	}
}

func recvType(t *testing.T, ctx context.Context, conn *websocket.Conn) (string, []byte) {
	t.Helper()
	rctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	_, data, err := conn.Read(rctx)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	var env types.Envelope
	if err := json.Unmarshal(data, &env); err != nil {
		t.Fatalf("decode envelope: %v", err)
	}
	return env.Type, data
}

// openHost initializes a host (as the default signed-in user) and returns its
// session. Hosting requires auth, so it dials as "test-user".
func openHost(t *testing.T, wsURL string) (*websocket.Conn, context.Context, types.ServerHostInitialized) {
	return openHostAs(t, wsURL, "test-user")
}

// openHostAs initializes a host as a specific user.
func openHostAs(t *testing.T, wsURL, user string) (*websocket.Conn, context.Context, types.ServerHostInitialized) {
	t.Helper()
	conn, ctx := dialAs(t, wsURL, user)
	send(t, ctx, conn, types.HostInitialize{
		Type: types.TypeInitialize, ProtocolVersion: types.ProtocolVersion,
		DeviceID: "dev-1", TabTitle: "zsh",
	})
	typ, data := recvType(t, ctx, conn)
	if typ != types.TypeInitialized {
		t.Fatalf("want initialized, got %q", typ)
	}
	var m types.ServerHostInitialized
	if err := json.Unmarshal(data, &m); err != nil {
		t.Fatalf("decode initialized: %v", err)
	}
	return conn, ctx, m
}

func TestHostInitializeHandshake(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	conn, _, m := openHost(t, wsURL)
	defer conn.Close(websocket.StatusNormalClosure, "")
	if m.ReconnectToken == "" {
		t.Fatal("missing reconnect token")
	}
	if m.PairingCode != nil {
		t.Fatalf("no pairing code expected in the link-based model, got %v", *m.PairingCode)
	}
}

func TestHostRequiresLogin(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	// Dial anonymously (no X-Test-User) and try to host — must be refused.
	conn, ctx := dial(t, wsURL)
	defer conn.Close(websocket.StatusNormalClosure, "")
	send(t, ctx, conn, types.HostInitialize{
		Type: types.TypeInitialize, ProtocolVersion: types.ProtocolVersion,
		DeviceID: "dev-1", TabTitle: "zsh",
	})
	if typ, _ := recvType(t, ctx, conn); typ != types.TypeTerminated {
		t.Fatalf("want terminated (login required), got %q", typ)
	}
}

func decodeMap(t *testing.T, resp *http.Response) map[string]string {
	t.Helper()
	defer resp.Body.Close()
	out := map[string]string{}
	_ = json.NewDecoder(resp.Body).Decode(&out)
	return out
}

// share creates a role-scoped link for a session and returns {token, join_url,
// access}. The token doubles as the join secret.
func share(t *testing.T, srv *httptest.Server, user, sessionID, access string) map[string]string {
	t.Helper()
	resp := postAs(t, srv, "/v1/sessions/share", user,
		map[string]string{"session_id": sessionID, "access": access})
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("share create status = %d", resp.StatusCode)
	}
	return decodeMap(t, resp)
}

func invite(t *testing.T, srv *httptest.Server, user, sessionID, email, access string) map[string]any {
	t.Helper()
	resp := postAs(t, srv, "/v1/sessions/invite", user,
		map[string]string{"session_id": sessionID, "email": email, "access": access})
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("invite status = %d", resp.StatusCode)
	}
	out := map[string]any{}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		t.Fatalf("decode invite: %v", err)
	}
	return out
}

// joinResult sends a join and returns the server's reply type.
func joinResult(t *testing.T, wsURL, user string, session uuid.UUID, secret string) string {
	t.Helper()
	viewer, vctx := dialAs(t, wsURL, user)
	defer viewer.Close(websocket.StatusNormalClosure, "")
	send(t, vctx, viewer, types.ViewerJoin{
		Type: types.TypeJoin, ProtocolVersion: types.ProtocolVersion,
		Session: session, Secret: secret, DeviceID: "dev-2", DisplayName: "Joiner",
	})
	typ, _ := recvType(t, vctx, viewer)
	return typ
}

func TestViewLinkAnonymousJoin(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	host, _, init := openHost(t, wsURL)
	defer host.Close(websocket.StatusNormalClosure, "")

	link := share(t, srv, "test-user", init.Session.String(), "view_only")
	// No user — a View link admits anonymously.
	if typ := joinResult(t, wsURL, "", init.Session, link["token"]); typ != types.TypeJoined {
		t.Fatalf("anonymous view join: want joined, got %q", typ)
	}
}

func TestJoinBadSecretRejected(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	host, _, init := openHost(t, wsURL)
	defer host.Close(websocket.StatusNormalClosure, "")

	if typ := joinResult(t, wsURL, "", init.Session, "not-a-real-secret"); typ != types.TypeEnded {
		t.Fatalf("bad secret: want ended (rejected), got %q", typ)
	}
}

func TestJoinRejectedAfterSessionEnded(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	host, ctx, init := openHost(t, wsURL)
	defer host.Close(websocket.StatusNormalClosure, "")

	link := share(t, srv, "test-user", init.Session.String(), "view_only")

	// Host ends the session (deletes its store record); read the Terminated ack
	// so the delete has completed before we try to (re)join via the still-valid link.
	send(t, ctx, host, types.Simple{Type: types.TypeEndSession})
	if typ, _ := recvType(t, ctx, host); typ != types.TypeTerminated {
		t.Fatalf("end session: want terminated ack, got %q", typ)
	}

	if typ := joinResult(t, wsURL, "", init.Session, link["token"]); typ != types.TypeEnded {
		t.Fatalf("join after session ended: want ended, got %q", typ)
	}
}

func TestEditLinkRequiresLogin(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	host, _, init := openHost(t, wsURL)
	defer host.Close(websocket.StatusNormalClosure, "")

	link := share(t, srv, "test-user", init.Session.String(), "control")
	// Anonymous → refused; signed-in → admitted.
	if typ := joinResult(t, wsURL, "", init.Session, link["token"]); typ != types.TypeEnded {
		t.Fatalf("anonymous edit join: want ended, got %q", typ)
	}
	if typ := joinResult(t, wsURL, "bob", init.Session, link["token"]); typ != types.TypeJoined {
		t.Fatalf("signed-in edit join: want joined, got %q", typ)
	}
}

func TestShareLinkCrossUserJoin(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()

	// Alice hosts and creates a view link.
	host, _, init := openHostAs(t, wsURL, "alice")
	defer host.Close(websocket.StatusNormalClosure, "")

	link := share(t, srv, "alice", init.Session.String(), "view_only")
	if link["token"] == "" || !strings.Contains(link["join_url"], link["token"]) {
		t.Fatalf("bad share response: %v", link)
	}

	// Bob (a DIFFERENT user) joins directly with the link's secret.
	if typ := joinResult(t, wsURL, "bob", init.Session, link["token"]); typ != types.TypeJoined {
		t.Fatalf("teammate join: want joined, got %q", typ)
	}
}

func TestEmailInviteUsesExistingUserProfile(t *testing.T) {
	srv, wsURL := testServerWithLookup(t, fakeUserLookup{
		byEmail: map[string][2]string{
			"sam@example.com": {"Sam Existing", "https://cdn.test/sam.png"},
		},
	})
	defer srv.Close()

	host, hctx, init := openHostAs(t, wsURL, "alice")
	defer host.Close(websocket.StatusNormalClosure, "")

	link := invite(t, srv, "alice", init.Session.String(), "sam@example.com", "view_only")
	token, _ := link["token"].(string)
	if token == "" {
		t.Fatalf("bad invite response: %v", link)
	}

	viewer, vctx := dial(t, wsURL)
	defer viewer.Close(websocket.StatusNormalClosure, "")
	send(t, vctx, viewer, types.ViewerJoin{
		Type: types.TypeJoin, ProtocolVersion: types.ProtocolVersion,
		Session: init.Session, Secret: token, DeviceID: "dev-2", DisplayName: "Joiner",
	})
	if typ, _ := recvType(t, vctx, viewer); typ != types.TypeJoined {
		t.Fatalf("invite join: want joined, got %q", typ)
	}

	typ, data := recvType(t, hctx, host)
	if typ != types.TypeParticipantsUpdated {
		t.Fatalf("host participants: want participants_updated, got %q", typ)
	}
	var list types.ServerParticipantsUpdated
	if err := json.Unmarshal(data, &list); err != nil {
		t.Fatalf("decode participants: %v", err)
	}
	if len(list.Participants) != 1 {
		t.Fatalf("want one participant, got %+v", list.Participants)
	}
	got := list.Participants[0]
	if got.DisplayName != "Sam Existing" || got.AvatarURL != "https://cdn.test/sam.png" {
		t.Fatalf("participant profile not enriched: %+v", got)
	}
}

func TestViewerPresenceUsesHostProfile(t *testing.T) {
	srv, wsURL := testServerWithLookup(t, fakeUserLookup{
		byID: map[string][2]string{
			"alice": {"Alice Host", "https://cdn.test/alice.png"},
		},
	})
	defer srv.Close()

	host, _, init := openHostAs(t, wsURL, "alice")
	defer host.Close(websocket.StatusNormalClosure, "")

	link := share(t, srv, "alice", init.Session.String(), "view_only")
	viewer, vctx := dial(t, wsURL)
	defer viewer.Close(websocket.StatusNormalClosure, "")
	send(t, vctx, viewer, types.ViewerJoin{
		Type: types.TypeJoin, ProtocolVersion: types.ProtocolVersion,
		Session: init.Session, Secret: link["token"], DeviceID: "dev-2", DisplayName: "Joiner",
	})
	if typ, _ := recvType(t, vctx, viewer); typ != types.TypeJoined {
		t.Fatalf("view join: want joined, got %q", typ)
	}

	typ, data := recvType(t, vctx, viewer)
	if typ != types.TypeParticipantsUpdated {
		t.Fatalf("viewer participants: want participants_updated, got %q", typ)
	}
	var list types.ServerParticipantsUpdated
	if err := json.Unmarshal(data, &list); err != nil {
		t.Fatalf("decode participants: %v", err)
	}
	for _, part := range list.Participants {
		if part.ID == types.HostParticipantID {
			if part.DisplayName != "Alice Host" || part.AvatarURL != "https://cdn.test/alice.png" {
				t.Fatalf("host profile not enriched: %+v", part)
			}
			return
		}
	}
	t.Fatalf("viewer presence list missing host entry: %+v", list.Participants)
}

func TestShareLinkOnlyHostCanCreate(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	host, _, init := openHostAs(t, wsURL, "alice")
	defer host.Close(websocket.StatusNormalClosure, "")

	// Bob is not the host — must be refused.
	resp := postAs(t, srv, "/v1/sessions/share", "bob",
		map[string]string{"session_id": init.Session.String()})
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("want 403 for non-host share, got %d", resp.StatusCode)
	}
}

func TestShareLinkRevoke(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	host, _, init := openHostAs(t, wsURL, "alice")
	defer host.Close(websocket.StatusNormalClosure, "")

	link := share(t, srv, "alice", init.Session.String(), "view_only")

	// A non-host cannot revoke.
	if resp := postAs(t, srv, "/v1/sessions/share/revoke", "bob",
		map[string]string{"token": link["token"]}); resp.StatusCode != http.StatusForbidden {
		resp.Body.Close()
		t.Fatalf("want 403 for non-host revoke, got %d", resp.StatusCode)
	}

	// The host revokes it.
	resp := postAs(t, srv, "/v1/sessions/share/revoke", "alice", map[string]string{"token": link["token"]})
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("want 200 on revoke, got %d", resp.StatusCode)
	}

	// The link no longer admits anyone.
	if typ := joinResult(t, wsURL, "bob", init.Session, link["token"]); typ != types.TypeEnded {
		t.Fatalf("want ended after revoke, got %q", typ)
	}
}

func TestPingPong(t *testing.T) {
	srv, wsURL := testServer(t)
	defer srv.Close()
	conn, ctx, _ := openHost(t, wsURL)
	defer conn.Close(websocket.StatusNormalClosure, "")

	send(t, ctx, conn, types.Simple{Type: types.TypePing})
	if typ, _ := recvType(t, ctx, conn); typ != types.TypePong {
		t.Fatalf("want pong, got %q", typ)
	}
}
