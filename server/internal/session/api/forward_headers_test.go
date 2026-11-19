package api

import (
	"bufio"
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"

	"server/internal/session/components"
)

// writeInPieces feeds the rewriter in small chunks so head detection is
// exercised across Write boundaries, as tunnel frames arrive in practice.
func writeInPieces(t *testing.T, w io.Writer, data string, size int) {
	t.Helper()
	for len(data) > 0 {
		n := min(size, len(data))
		if _, err := w.Write([]byte(data[:n])); err != nil {
			t.Fatal(err)
		}
		data = data[n:]
	}
}

// rewrite runs data through a fresh rewriter in small pieces and returns the
// output and the first write error.
func rewrite(policy string, cookies forwardCookieFilter, upgrade bool, data string) (string, error) {
	var out bytes.Buffer
	w := newForwardResponseRewriter(&out, policy, cookies, upgrade)
	for len(data) > 0 {
		n := min(7, len(data))
		if _, err := w.Write([]byte(data[:n])); err != nil {
			return out.String(), err
		}
		data = data[n:]
	}
	return out.String(), nil
}

// readForwardResponse parses rewriter output the way Go clients (and Caddy)
// do, skipping interim 1xx responses.
func readForwardResponse(t *testing.T, raw string) (*http.Response, string) {
	t.Helper()
	reader := bufio.NewReader(strings.NewReader(raw))
	for {
		response, err := http.ReadResponse(reader, nil)
		if err != nil {
			t.Fatalf("read response from %q: %v", raw, err)
		}
		if response.StatusCode >= 200 || response.StatusCode == http.StatusSwitchingProtocols {
			body, err := io.ReadAll(response.Body)
			if err != nil {
				t.Fatalf("read body from %q: %v", raw, err)
			}
			return response, string(body)
		}
	}
}

func assertSandboxed(t *testing.T, response *http.Response, policy string) {
	t.Helper()
	if csp := response.Header.Values("Content-Security-Policy"); len(csp) != 1 || csp[0] != policy {
		t.Fatalf("Content-Security-Policy = %q, want only %q", csp, policy)
	}
	if !response.Close {
		t.Fatal("forwarded response should close the connection")
	}
}

func TestForwardResponseRewriterSandboxesFinalResponse(t *testing.T) {
	got, err := rewrite(forwardSandboxPolicy, dropAllForwardCookies, false, "HTTP/1.1 100 Continue\r\n\r\n"+
		"HTTP/1.1 200 OK\r\n"+
		"Content-Type: text/html\r\n"+
		"content-security-policy: script-src *;\r\n"+
		"  frame-ancestors *\r\n"+
		"Content-Security-Policy-Report-Only: default-src *\r\n"+
		"Set-Cookie: a=b\r\n"+
		"Connection: keep-alive\r\n"+
		"Keep-Alive: timeout=5\r\n"+
		"Content-Length: 31\r\n\r\n"+
		"<script>alert(1)</script>\r\n\r\nok")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(got, "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n") {
		t.Fatalf("interim response was not passed through: %q", got)
	}
	response, body := readForwardResponse(t, got)
	if body != "<script>alert(1)</script>\r\n\r\nok" {
		t.Fatalf("body = %q", body)
	}
	assertSandboxed(t, response, forwardSandboxPolicy)
	for _, unwanted := range []string{"Keep-Alive", "Set-Cookie", "Content-Security-Policy-Report-Only"} {
		if v := response.Header.Values(unwanted); len(v) != 0 {
			t.Fatalf("%s survived: %q", unwanted, v)
		}
	}
	if response.Header.Get("Content-Type") != "text/html" || response.ContentLength != 31 {
		t.Fatalf("framing headers changed: %v", response.Header)
	}
	if strings.Contains(forwardSandboxPolicy, "allow-same-origin") {
		t.Fatal("path-based forwards must not keep their origin")
	}
}

// TestForwardResponseRewriterHandlesBareLFHeads covers heads that end lines
// with a bare LF, which browsers and net/http accept. The CSP and Connection
// header must land in the head, not in the body.
func TestForwardResponseRewriterHandlesBareLFHeads(t *testing.T) {
	for name, raw := range map[string]string{
		"lf-only": "HTTP/1.1 200 OK\nContent-Type: text/html\nContent-Security-Policy: default-src *\nConnection: keep-alive\nContent-Length: 29\n\n<script>alert(1)</script>\r\n\r\n",
		"mixed":   "HTTP/1.1 200 OK\r\nContent-Type: text/html\nContent-Length: 29\r\n\n<script>alert(1)</script>\r\n\r\n",
		"mixed2":  "HTTP/1.1 200 OK\nContent-Type: text/html\r\nContent-Length: 29\n\r\n<script>alert(1)</script>\r\n\r\n",
		"interim": "HTTP/1.1 103 Early Hints\nLink: </a.css>\n\nHTTP/1.1 200 OK\nContent-Length: 29\n\n<script>alert(1)</script>\r\n\r\n",
	} {
		t.Run(name, func(t *testing.T) {
			for _, policy := range []string{forwardSandboxPolicy, forwardHostSandboxPolicy} {
				got, err := rewrite(policy, hostOnlyForwardCookies, false, raw)
				if err != nil {
					t.Fatal(err)
				}
				response, body := readForwardResponse(t, got)
				assertSandboxed(t, response, policy)
				if body != "<script>alert(1)</script>\r\n\r\n" {
					t.Fatalf("body = %q", body)
				}
			}
		})
	}
}

func TestForwardResponseRewriterRejectsMalformedHeads(t *testing.T) {
	for name, raw := range map[string]string{
		"leading blank line": "\r\nHTTP/1.1 200 OK\r\n\r\n<script></script>",
		"not http":           "<script>alert(1)</script>\n\n",
		"bad status":         "HTTP/1.1 2x0 OK\r\n\r\n",
		"no colon":           "HTTP/1.1 200 OK\r\nContent-Type text/html\r\n\r\n",
		"bare cr":            "HTTP/1.1 200 OK\r\nX-A: b\rContent-Security-Policy: x\r\n\r\n",
		"leading fold":       "HTTP/1.1 200 OK\r\n X-A: b\r\n\r\n",
		"nul":                "HTTP/1.1 200 OK\r\nX-A: \x00\r\n\r\n",
	} {
		t.Run(name, func(t *testing.T) {
			got, err := rewrite(forwardSandboxPolicy, dropAllForwardCookies, false, raw)
			if err == nil {
				t.Fatalf("malformed head accepted: %q", got)
			}
			if got != "" {
				t.Fatalf("malformed head leaked output: %q", got)
			}
		})
	}
}

func TestForwardResponseRewriterOnlyRewritesFirstResponse(t *testing.T) {
	second := "HTTP/1.1 200 OK\r\nX-Test: raw\r\n\r\n"
	got, err := rewrite(forwardSandboxPolicy, dropAllForwardCookies, false, "HTTP/1.1 204 No Content\r\n\r\n"+second)
	if err != nil {
		t.Fatal(err)
	}
	response, _ := readForwardResponse(t, got)
	assertSandboxed(t, response, forwardSandboxPolicy)
	if !strings.HasSuffix(got, "\r\n\r\n"+second) {
		t.Fatalf("output = %q", got)
	}
}

func TestForwardResponseRewriterPassesWebSocketUpgrade(t *testing.T) {
	frames := "\x81\x05hello\r\n\r\n\x81\x02hi"
	got, err := rewrite(forwardHostSandboxPolicy, hostOnlyForwardCookies, true,
		"HTTP/1.1 101 Switching Protocols\nUpgrade: websocket\nConnection: Upgrade\nSec-WebSocket-Accept: abc\n\n"+frames)
	if err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(strings.NewReader(got))
	response, err := http.ReadResponse(reader, nil)
	if err != nil || response.StatusCode != http.StatusSwitchingProtocols {
		t.Fatalf("response %v, error %v", response, err)
	}
	if response.Header.Get("Upgrade") != "websocket" || response.Header.Get("Sec-WebSocket-Accept") != "abc" {
		t.Fatalf("upgrade headers changed: %v", response.Header)
	}
	rest, _ := io.ReadAll(reader)
	if string(rest) != frames {
		t.Fatalf("upgrade stream altered: %q", rest)
	}
}

// TestForwardResponseRewriterRejectsUnrequestedUpgrade checks that a 101 to a
// plain request cannot be used to skip the rewrite of what follows.
func TestForwardResponseRewriterRejectsUnrequestedUpgrade(t *testing.T) {
	got, err := rewrite(forwardSandboxPolicy, dropAllForwardCookies, false,
		"HTTP/1.1 101 Switching Protocols\r\nUpgrade: x\r\n\r\nHTTP/1.1 200 OK\r\n\r\n<script>alert(1)</script>")
	if err != errForwardUnexpectedUpgrade || got != "" {
		t.Fatalf("output %q, error %v", got, err)
	}
}

func TestIsUpgradeRequest(t *testing.T) {
	for _, tc := range []struct {
		header http.Header
		want   bool
	}{
		{http.Header{"Connection": {"keep-alive, Upgrade"}, "Upgrade": {"websocket"}}, true},
		{http.Header{"Connection": {"keep-alive"}, "Upgrade": {"websocket"}}, false},
		{http.Header{"Connection": {"upgrade"}}, false},
		{http.Header{}, false},
	} {
		if got := isUpgradeRequest(tc.header); got != tc.want {
			t.Fatalf("isUpgradeRequest(%v) = %v", tc.header, got)
		}
	}
}

func TestForwardResponseRewriterFiltersHostCookies(t *testing.T) {
	got, err := rewrite(forwardHostSandboxPolicy, hostOnlyForwardCookies, false, "HTTP/1.1 200 OK\r\n"+
		"Set-Cookie: app=1; Path=/; HttpOnly\r\n"+
		"Set-Cookie: wide=1; Path=/; domain=.dragent.dev\r\n"+
		"Set-Cookie: __host-dragent_forward=stolen; Path=/; Secure\r\n"+
		"Content-Length: 0\r\n\r\n")
	if err != nil {
		t.Fatal(err)
	}
	response, _ := readForwardResponse(t, got)
	if cookies := response.Header.Values("Set-Cookie"); len(cookies) != 1 || cookies[0] != "app=1; Path=/; HttpOnly" {
		t.Fatalf("Set-Cookie = %q", cookies)
	}
}

func TestForwardResponseRewriterRejectsUnboundedHead(t *testing.T) {
	w := newForwardResponseRewriter(io.Discard, forwardSandboxPolicy, dropAllForwardCookies, false)
	line := "X-Filler: " + strings.Repeat("a", 1000) + "\r\n"
	var err error
	_, err = w.Write([]byte("HTTP/1.1 200 OK\r\n"))
	for i := 0; i < 100 && err == nil; i++ {
		_, err = w.Write([]byte(line))
	}
	if err != errForwardResponseHeadTooLarge {
		t.Fatalf("err = %v, want %v", err, errForwardResponseHeadTooLarge)
	}
}

func TestForwardGatewayRepliesAreSandboxed(t *testing.T) {
	hub := components.NewHub(components.NewMemoryBus(), components.AllowAll{}, components.NoopAuditor{})
	h := NewHandler(hub, components.NewMemoryStore(), "https://app.test", nil, nil)
	h.SetForwardDomain("preview.test")
	router := chi.NewRouter()
	h.RegisterPublicRoutes(router)
	forward, err := h.forwards.Create("session", 5173, "Vite")
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		host, path, policy string
	}{
		{"api.test", "/forward/" + forward.ID + "/", forwardSandboxPolicy},
		{"api.test", "/forward/" + forward.ID + "/?secret=" + forward.Token, forwardSandboxPolicy},
		{forward.ID + ".preview.test", "/", forwardHostSandboxPolicy},
		{forward.ID + ".preview.test", "/?secret=" + forward.Token, forwardHostSandboxPolicy},
	} {
		req := httptest.NewRequest(http.MethodGet, tc.path, nil)
		req.Host = tc.host
		rec := httptest.NewRecorder()
		router.ServeHTTP(rec, req)
		if rec.Code == http.StatusOK {
			t.Fatalf("%s%s unexpectedly streamed", tc.host, tc.path)
		}
		if csp := rec.Header().Get("Content-Security-Policy"); csp != tc.policy {
			t.Fatalf("%s%s (%d): Content-Security-Policy = %q", tc.host, tc.path, rec.Code, csp)
		}
	}
}

func TestSiteOf(t *testing.T) {
	for host, want := range map[string]string{
		"dragent.dev": "dragent.dev", "x.preview.dragent.dev": "dragent.dev", "dragent-preview.dev.": "dragent-preview.dev",
	} {
		if got := siteOf(host); got != want {
			t.Fatalf("siteOf(%q) = %q", host, got)
		}
	}
}

// TestHostForwardResponseIsSandboxed drives a hostname-based forward end to
// end over the dedicated tunnel and checks the browser-facing response.
func TestHostForwardResponseIsSandboxed(t *testing.T) {
	hub := components.NewHub(components.NewMemoryBus(), components.AllowAll{}, components.NoopAuditor{})
	h := NewHandler(hub, components.NewMemoryStore(), "https://app.test", nil, nil)
	h.SetForwardDomain("preview.test")
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
		req, _ := http.NewRequest(http.MethodGet, srv.URL+"/src/main.tsx", nil)
		req.Host = forward.ID + ".preview.test"
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
	if opened.Path != "/src/main.tsx" {
		t.Fatalf("tunnel open path = %q", opened.Path)
	}
	response := "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\n" +
		"Content-Security-Policy: default-src *\r\nConnection: keep-alive\r\nContent-Length: 2\r\n\r\nok"
	for _, responseFrame := range [][]byte{
		encodeTunnelFrame(tunnelFrameOpened, streamID, nil),
		encodeTunnelFrame(tunnelFrameData, streamID, []byte(response)),
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
		if err != nil || string(body) != "ok" {
			t.Fatalf("body = %q, error %v", body, err)
		}
		if csp := got.response.Header.Values("Content-Security-Policy"); len(csp) != 1 || csp[0] != forwardHostSandboxPolicy {
			t.Fatalf("Content-Security-Policy = %q, want only %q", csp, forwardHostSandboxPolicy)
		}
		if !got.response.Close {
			t.Fatal("forwarded response should close the connection")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("host forward response timed out")
	}
}

// readForwardHeads parses every head in rewriter output, interim ones
// included, stopping after the first final or 101 head. It returns the heads
// and whatever bytes follow the last one.
func readForwardHeads(t *testing.T, raw string) ([]*http.Response, string) {
	t.Helper()
	reader := bufio.NewReader(strings.NewReader(raw))
	var heads []*http.Response
	for {
		response, err := http.ReadResponse(reader, nil)
		if err != nil {
			t.Fatalf("read response from %q: %v", raw, err)
		}
		heads = append(heads, response)
		if response.StatusCode >= 200 || response.StatusCode == http.StatusSwitchingProtocols {
			rest, _ := io.ReadAll(reader)
			return heads, string(rest)
		}
	}
}

const domainCookieHeaders = "Set-Cookie: app=1; Path=/\r\n" +
	"Set-Cookie: wide=1; Path=/; Domain=dragent.dev\r\n" +
	"Set-Cookie: __Host-dragent_forward=stolen; Path=/; Secure\r\n"

// TestForwardResponseRewriterFiltersInterimHeadCookies checks that 101 and
// 103 heads cannot set Domain= cookies (or overwrite the forward cookie) on
// either route, and that they are not given a CSP or Connection: close.
func TestForwardResponseRewriterFiltersInterimHeadCookies(t *testing.T) {
	routes := []struct {
		name    string
		policy  string
		cookies forwardCookieFilter
		want    []string
	}{
		{"path", forwardSandboxPolicy, dropAllForwardCookies, nil},
		{"host", forwardHostSandboxPolicy, hostOnlyForwardCookies, []string{"app=1; Path=/"}},
	}
	for _, route := range routes {
		t.Run(route.name+"/101", func(t *testing.T) {
			frames := "\x81\x02hi"
			got, err := rewrite(route.policy, route.cookies, true,
				"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"+
					"Sec-WebSocket-Accept: abc\r\n"+domainCookieHeaders+"\r\n"+frames)
			if err != nil {
				t.Fatal(err)
			}
			heads, rest := readForwardHeads(t, got)
			response := heads[len(heads)-1]
			if response.StatusCode != http.StatusSwitchingProtocols {
				t.Fatalf("status = %d", response.StatusCode)
			}
			assertCookies(t, response.Header, route.want)
			if response.Header.Get("Upgrade") != "websocket" || response.Header.Get("Connection") != "Upgrade" ||
				response.Header.Get("Sec-WebSocket-Accept") != "abc" {
				t.Fatalf("upgrade headers changed: %v", response.Header)
			}
			if csp := response.Header.Values("Content-Security-Policy"); len(csp) != 0 {
				t.Fatalf("101 got a CSP: %q", csp)
			}
			if rest != frames {
				t.Fatalf("upgrade stream altered: %q", rest)
			}
		})
		t.Run(route.name+"/103", func(t *testing.T) {
			got, err := rewrite(route.policy, route.cookies, false,
				"HTTP/1.1 103 Early Hints\r\nLink: </app.js>; rel=preload\r\n"+domainCookieHeaders+"\r\n"+
					"HTTP/1.1 200 OK\r\n"+domainCookieHeaders+"Content-Length: 2\r\n\r\nok")
			if err != nil {
				t.Fatal(err)
			}
			heads, rest := readForwardHeads(t, got)
			if len(heads) != 2 || heads[0].StatusCode != http.StatusEarlyHints || heads[1].StatusCode != http.StatusOK {
				t.Fatalf("heads = %v", heads)
			}
			hints := heads[0].Header
			assertCookies(t, hints, route.want)
			if hints.Get("Link") != "</app.js>; rel=preload" {
				t.Fatalf("Link changed: %v", hints)
			}
			if len(hints.Values("Content-Security-Policy")) != 0 || len(hints.Values("Connection")) != 0 {
				t.Fatalf("103 got final-head headers: %v", hints)
			}
			assertCookies(t, heads[1].Header, route.want)
			if rest != "ok" {
				t.Fatalf("body = %q", rest)
			}
		})
	}
}

func assertCookies(t *testing.T, h http.Header, want []string) {
	t.Helper()
	got := h.Values("Set-Cookie")
	if len(got) != len(want) {
		t.Fatalf("Set-Cookie = %q, want %q", got, want)
	}
	for i := range got {
		if got[i] != want[i] {
			t.Fatalf("Set-Cookie = %q, want %q", got, want)
		}
	}
}
