package api

import (
	"bufio"
	"bytes"
	"errors"
	"io"
	"net/http"
	"net/textproto"
	"strings"
)

// Forwarded applications are arbitrary, user-controlled HTML and JavaScript.
// Every forwarded response carries a CSP sandbox so it can never act with the
// origin it is served from.
//
// forwardSandboxPolicy deliberately omits allow-same-origin: the document gets
// an opaque origin and cannot read cookies, localStorage or same-origin
// responses. It is used on the path-based route, which the sign-in portal
// proxies under its own origin.
//
// forwardHostSandboxPolicy is used on the dedicated forward hostname
// (<id>.<DRAGENT_FORWARD_DOMAIN>). That hostname is already its own origin and,
// deployed as documented, its own registrable domain, so isolation from the
// portal comes from the domain. It keeps allow-same-origin because module
// scripts, fetch() and WebSockets from an opaque origin are cross-origin
// requests that never carry the forward cookie, which would break Vite and
// every app that calls its own API.
const (
	forwardSandboxPolicy     = "sandbox allow-scripts allow-forms allow-popups allow-modals allow-downloads"
	forwardHostSandboxPolicy = forwardSandboxPolicy + " allow-same-origin"
)

// forwardCookieName is the host-only cookie that carries a forward's share
// capability after the ?secret= bootstrap redirect.
const forwardCookieName = "__Host-dragent_forward"

// maxForwardResponseHead bounds how much of a response head is buffered while
// looking for its end. Real heads are a few KiB.
const maxForwardResponseHead = 64 << 10

var (
	errForwardResponseHeadTooLarge = errors.New("forwarded response head is too large")
	errForwardResponseHeadInvalid  = errors.New("forwarded response head is malformed")
	errForwardUnexpectedUpgrade    = errors.New("forwarded response switched protocols without an upgrade request")
)

// forwardCookieFilter decides which upstream Set-Cookie headers survive.
type forwardCookieFilter int

const (
	// dropAllForwardCookies removes every upstream Set-Cookie. Used on the
	// path-based route, which is served on the API origin (and proxied by the
	// portal), where a forward must not be able to set any cookie.
	dropAllForwardCookies forwardCookieFilter = iota
	// hostOnlyForwardCookies keeps cookies scoped to the forward's own host
	// but drops any with a Domain attribute (which could reach a parent
	// domain) and any that would overwrite the gateway's forward cookie.
	hostOnlyForwardCookies
)

// forwardResponseRewriter sits between the local application's raw HTTP/1.x
// byte stream and the browser-facing connection. It parses the head of the
// first final response and re-serializes it:
//   - any upstream Content-Security-Policy is replaced by the sandbox policy;
//   - upstream Set-Cookie headers are filtered (see forwardCookieFilter);
//   - Connection is forced to close, so the client cannot send a second
//     request on this stream whose response would bypass the rewrite.
//
// Heads are accepted with CRLF, bare LF or mixed line endings, as browsers and
// net/http accept them, and are always re-emitted with CRLF. A head that does
// not parse is never forwarded: Write returns an error and the caller drops
// the connection.
//
// Interim 1xx and 101 heads get the same Set-Cookie filter but keep their
// other headers (no CSP, no Connection: close). Interim 1xx responses are
// re-emitted while it waits for the final head. A
// 101 Switching Protocols response is relayed, followed by the raw stream,
// only when the browser's request asked for an upgrade; otherwise the stream
// is rejected. After the first final head the rest of the stream (body) is
// relayed unchanged.
type forwardResponseRewriter struct {
	dst     io.Writer
	policy  string
	cookies forwardCookieFilter
	upgrade bool
	buf     []byte
	done    bool
	err     error
}

func newForwardResponseRewriter(dst io.Writer, policy string, cookies forwardCookieFilter, upgradeRequested bool) *forwardResponseRewriter {
	return &forwardResponseRewriter{dst: dst, policy: policy, cookies: cookies, upgrade: upgradeRequested}
}

// isUpgradeRequest reports whether a browser request asks to switch protocols
// (Connection: upgrade plus an Upgrade header), e.g. a WebSocket handshake.
func isUpgradeRequest(header http.Header) bool {
	if strings.TrimSpace(header.Get("Upgrade")) == "" {
		return false
	}
	for _, value := range header.Values("Connection") {
		for _, token := range strings.Split(value, ",") {
			if strings.EqualFold(strings.TrimSpace(token), "upgrade") {
				return true
			}
		}
	}
	return false
}

func (w *forwardResponseRewriter) Write(p []byte) (int, error) {
	if w.err != nil {
		return 0, w.err
	}
	if w.done {
		return w.dst.Write(p)
	}
	w.buf = append(w.buf, p...)
	for !w.done {
		end, found, err := forwardResponseHeadEnd(w.buf)
		if err != nil {
			w.err = err
			return 0, err
		}
		if !found {
			if len(w.buf) > maxForwardResponseHead {
				w.err = errForwardResponseHeadTooLarge
				return 0, w.err
			}
			return len(p), nil
		}
		if end > maxForwardResponseHead {
			w.err = errForwardResponseHeadTooLarge
			return 0, w.err
		}
		out, final, err := w.rewriteHead(w.buf[:end])
		if err != nil {
			w.err = err
			return 0, err
		}
		if _, err := w.dst.Write(out); err != nil {
			w.err = err
			return 0, err
		}
		w.buf = append([]byte(nil), w.buf[end:]...)
		w.done = final
	}
	if len(w.buf) > 0 {
		if _, err := w.dst.Write(w.buf); err != nil {
			w.err = err
			return 0, err
		}
	}
	w.buf = nil
	return len(p), nil
}

// forwardResponseHeadEnd finds the end of the response head in buf: the byte
// after the first empty line, where a line ends in "\n" with an optional
// preceding "\r". An empty first line is malformed.
func forwardResponseHeadEnd(buf []byte) (int, bool, error) {
	start := 0
	for {
		i := bytes.IndexByte(buf[start:], '\n')
		if i < 0 {
			return 0, false, nil
		}
		line := bytes.TrimSuffix(buf[start:start+i], []byte("\r"))
		if len(line) == 0 {
			if start == 0 {
				return 0, false, errForwardResponseHeadInvalid
			}
			return start + i + 1, true, nil
		}
		start += i + 1
	}
}

// rewriteHead parses one complete response head and re-serializes it. final
// reports whether the head ends header processing for the stream.
func (w *forwardResponseRewriter) rewriteHead(head []byte) ([]byte, bool, error) {
	// A bare CR or NUL is read differently by different clients; refuse it
	// rather than guess which reading the browser will use.
	if bytes.IndexByte(head, 0) >= 0 || bytes.Contains(bytes.ReplaceAll(head, []byte("\r\n"), nil), []byte("\r")) {
		return nil, false, errForwardResponseHeadInvalid
	}
	reader := textproto.NewReader(bufio.NewReader(bytes.NewReader(head)))
	statusLine, err := reader.ReadLine()
	if err != nil {
		return nil, false, errForwardResponseHeadInvalid
	}
	status, ok := parseForwardStatusLine(statusLine)
	if !ok {
		return nil, false, errForwardResponseHeadInvalid
	}
	header, err := reader.ReadMIMEHeader()
	if err != nil {
		return nil, false, errForwardResponseHeadInvalid
	}

	h := http.Header(header)
	// Browsers store Set-Cookie from a 101 WebSocket handshake, and may from
	// interim heads such as 103 Early Hints, so the per-route cookie filter
	// applies to every head, not only the final one.
	w.filterCookies(h)

	var out bytes.Buffer
	out.WriteString(statusLine)
	out.WriteString("\r\n")
	switch {
	case status == http.StatusSwitchingProtocols:
		if !w.upgrade {
			return nil, false, errForwardUnexpectedUpgrade
		}
		// Upgrade and Connection must pass untouched for the switch to work;
		// the stream that follows is no longer HTTP, so no CSP either.
		if err := h.Write(&out); err != nil {
			return nil, false, err
		}
		out.WriteString("\r\n")
		return out.Bytes(), true, nil
	case status < 200:
		if err := h.Write(&out); err != nil {
			return nil, false, err
		}
		out.WriteString("\r\n")
		return out.Bytes(), false, nil
	}

	h.Del("Content-Security-Policy")
	h.Del("Content-Security-Policy-Report-Only")
	h.Del("Connection")
	h.Del("Keep-Alive")
	h.Del("Upgrade")
	h.Del("Proxy-Connection")
	h.Set("Content-Security-Policy", w.policy)
	h.Set("Connection", "close")
	if err := h.Write(&out); err != nil {
		return nil, false, err
	}
	out.WriteString("\r\n")
	return out.Bytes(), true, nil
}

// filterCookies applies the route's forwardCookieFilter to h's Set-Cookie
// headers in place.
func (w *forwardResponseRewriter) filterCookies(h http.Header) {
	cookies := h.Values("Set-Cookie")
	h.Del("Set-Cookie")
	if w.cookies != hostOnlyForwardCookies {
		return
	}
	for _, cookie := range cookies {
		if forwardCookieAllowed(cookie) {
			h.Add("Set-Cookie", cookie)
		}
	}
}

// parseForwardStatusLine accepts "HTTP/1.x NNN[ reason]" and returns NNN.
func parseForwardStatusLine(line string) (int, bool) {
	proto, rest, ok := strings.Cut(line, " ")
	if !ok || (proto != "HTTP/1.1" && proto != "HTTP/1.0") || len(rest) < 3 {
		return 0, false
	}
	if len(rest) > 3 && rest[3] != ' ' {
		return 0, false
	}
	if strings.ContainsAny(line, "\r\n\x00") {
		return 0, false
	}
	code := 0
	for _, c := range rest[:3] {
		if c < '0' || c > '9' {
			return 0, false
		}
		code = code*10 + int(c-'0')
	}
	if code < 100 {
		return 0, false
	}
	return code, true
}

// forwardCookieAllowed reports whether an upstream Set-Cookie may reach the
// browser on a hostname-based forward: host-only cookies are fine, but not
// ones with a Domain attribute (they could be scoped to a parent domain) or
// ones that would overwrite the gateway's own forward cookie.
func forwardCookieAllowed(setCookie string) bool {
	parts := strings.Split(setCookie, ";")
	name, _, _ := strings.Cut(parts[0], "=")
	if strings.EqualFold(strings.TrimSpace(name), forwardCookieName) {
		return false
	}
	for _, attr := range parts[1:] {
		key, _, _ := strings.Cut(attr, "=")
		if strings.EqualFold(strings.TrimSpace(key), "domain") {
			return false
		}
	}
	return true
}
