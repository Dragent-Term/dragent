// Package api serves the realtime WebSocket gateway for shared/remote sessions.
//
// A host `initialize`s a session (authenticated). To invite others the host
// creates a role-scoped share link (`POST /v1/sessions/share`) or emails one
// (`POST /v1/sessions/invite`): each link carries its own capability secret that
// resolves to a role — View or Edit. A joiner opens the link's URL and `join`s
// the WebSocket with `{session, secret}`; the gateway maps the secret to a role.
//
//   - View  → read-only, admitted anonymously (no login required).
//   - Edit  → may run commands (no per-command approval), and REQUIRES the
//     connection to be authenticated so every command is attributed + audited.
//
// The `/realtime` socket is therefore mounted OUTSIDE RequireAuth (anonymous
// viewers must reach it); the soft global AuthMiddleware still populates the
// user id when a bearer token is present. Host `reconnect` is validated against a
// per-session token.
package api

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/session/components"
	"server/internal/session/types"
)

// 8 MiB. The 32 KiB default is far too small for output-chunk deltas, and even
// 1 MiB was overrun by a block snapshot from a command that dumps a lot of output
// (a failed `yarn dev`, a build log) — which dropped the HOST's connection and
// made viewers see "the host ended the session" mid-session. The host also caps
// its snapshots (SHARE_MAX_LINES); this is the belt to that braces.
const readLimitBytes = 8 << 20

// Inviter sends a session-invite email. Implemented by the SMTP mailer; may be
// nil (in tests, or if email is unconfigured) — the link is still returned so the
// host can share it manually.
type Inviter interface {
	SendSessionInvite(ctx context.Context, email, joinURL, access string) error
}

// UserLookup resolves an account id to the display name + avatar used in the
// session participant list. Nil is fine (tests); lookups then no-op.
type UserLookup interface {
	SessionProfile(ctx context.Context, userID string) (displayName, avatarURL string, ok bool)
	SessionProfileByEmail(ctx context.Context, email string) (displayName, avatarURL string, ok bool)
}

type Handler struct {
	hub            *components.Hub
	store          components.SessionStore
	forwards       *components.ForwardRegistry
	forwardTunnels *forwardTunnelRegistry
	webOrigin      string // for building share/join URLs
	forwardDomain  string
	inviter        Inviter
	users          UserLookup
}

func NewHandler(hub *components.Hub, store components.SessionStore, webOrigin string, inviter Inviter, users UserLookup) *Handler {
	return &Handler{hub: hub, store: store, forwards: components.NewForwardRegistry(), forwardTunnels: newForwardTunnelRegistry(), webOrigin: strings.TrimRight(webOrigin, "/"), inviter: inviter, users: users}
}

// SetForwardDomain enables hostname-based public forwards such as
// https://<forward-id>.dragent-preview.dev/. It is deliberately configured
// separately from webOrigin. Forwards serve user-controlled scripts, so the
// domain must be its own registrable domain, never a subdomain of the portal's
// or API's site: otherwise a forward is same-site with them and can set cookies
// for it.
func (h *Handler) SetForwardDomain(domain string) {
	h.forwardDomain = strings.TrimSuffix(strings.ToLower(strings.TrimSpace(domain)), ".")
	if h.forwardDomain == "" {
		return
	}
	if web, err := url.Parse(h.webOrigin); err == nil && web.Hostname() != "" &&
		siteOf(h.forwardDomain) == siteOf(web.Hostname()) {
		slog.Warn("forward domain shares a site with the web origin; forwarded apps can set cookies for it. Use a separate registrable domain",
			"forward_domain", h.forwardDomain, "web_origin", h.webOrigin)
	}
}

// siteOf approximates a host's registrable domain as its last two labels. It
// is only used for the startup warning above, so multi-label public suffixes
// such as co.uk merely make the warning more eager.
func siteOf(host string) string {
	labels := strings.Split(strings.TrimSuffix(strings.ToLower(host), "."), ".")
	if len(labels) <= 2 {
		return strings.Join(labels, ".")
	}
	return strings.Join(labels[len(labels)-2:], ".")
}

// IsForwardHost reports whether a request host belongs to the public forward
// namespace. The API router uses this before registering its ordinary routes;
// otherwise paths such as "/" and "/query" would be claimed by GraphQL before
// the forwarding catch-all could see them.
func (h *Handler) IsForwardHost(rawHost string) bool {
	_, ok := h.forwardIDFromHost(rawHost)
	return ok
}

// profileFor returns the account's display name + avatar when known.
func (h *Handler) profileFor(ctx context.Context, userID string) (name, avatar string) {
	if h.users == nil || userID == "" {
		return "", ""
	}
	name, avatar, ok := h.users.SessionProfile(ctx, userID)
	if !ok {
		return "", ""
	}
	return name, avatar
}

// profileForEmail returns the existing account profile for an invited email.
func (h *Handler) profileForEmail(ctx context.Context, email string) (name, avatar string) {
	if h.users == nil || email == "" {
		return "", ""
	}
	name, avatar, ok := h.users.SessionProfileByEmail(ctx, email)
	if !ok {
		return "", ""
	}
	return name, avatar
}

// joinURL builds the shareable link a viewer opens. The secret (== the link
// token) is the capability; anyone with this URL can join at the link's role.
func (h *Handler) joinURL(sessionID, secret string) string {
	return h.webOrigin + "/join?session=" + sessionID + "&secret=" + secret
}

// RegisterPublicRoutes mounts the long-lived WebSocket. It must NOT sit under
// RequireAuth (anonymous View joiners connect here) nor under a request timeout.
func (h *Handler) RegisterPublicRoutes(r chi.Router) {
	r.Get("/realtime", h.ServeWS)
	r.Get("/realtime/forward", h.ServeForwardTunnel)
	r.Get("/internal/forward-tls-ask", h.ForwardTLSAsk)
	// The portal fallback proxies a forward as /forward/<id>/... when a
	// wildcard forward domain is not configured (for local development and
	// deployments that do not own one). Keep this explicit route in addition
	// to the host-based catch-all below: the proxy cannot select a forward by
	// replacing the backend request's Host header.
	r.HandleFunc("/forward/{forwardID}", h.ServeForwardPath)
	r.HandleFunc("/forward/{forwardID}/*", h.ServeForwardPath)
	// Host-based forwards must receive every path under the forward hostname,
	// including root-relative Vite assets and WebSocket handshake paths.
	r.HandleFunc("/*", h.ServeForwardHost)
}

// RegisterRoutes mounts the authenticated REST surface: only a signed-in host may
// create, email, or revoke the links that grant access to their session.
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/v1/sessions/share", h.CreateShareLink)
	r.Post("/v1/sessions/invite", h.InviteByEmail)
	r.Post("/v1/sessions/share/revoke", h.RevokeShareLink)
	r.Get("/v1/forwards", h.ListForwards)
	r.Post("/v1/forwards", h.CreateForward)
	r.Post("/v1/forwards/close", h.CloseForward)
}

func forwardOwnerID(userID, deviceID string) (string, bool) {
	deviceID = strings.TrimSpace(deviceID)
	if userID == "" || deviceID == "" || len(deviceID) > 128 {
		return "", false
	}
	return "device\x00" + userID + "\x00" + deviceID, true
}

func (h *Handler) publicForwardURL(f components.PortForward) string {
	publicURL := h.webOrigin + "/api/forward/" + f.ID + "?secret=" + url.QueryEscape(f.Token)
	if h.forwardDomain != "" {
		publicURL = "https://" + f.ID + "." + h.forwardDomain + "?secret=" + url.QueryEscape(f.Token)
	}
	return publicURL
}

type forwardAPIResponse struct {
	ID        string `json:"id"`
	Port      uint16 `json:"port"`
	Label     string `json:"label"`
	PublicURL string `json:"public_url"`
}

func (h *Handler) forwardAPIResponse(f components.PortForward) forwardAPIResponse {
	return forwardAPIResponse{ID: f.ID, Port: f.Port, Label: f.Label, PublicURL: h.publicForwardURL(f)}
}

// CreateForward is device-scoped and independent from terminal sharing. The
// device's binary data plane must be connected before a public capability is
// issued, so the first request cannot race an unavailable desktop tunnel.
func (h *Handler) CreateForward(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	var body struct {
		DeviceID string `json:"device_id"`
		Port     uint16 `json:"port"`
		Label    string `json:"label"`
	}
	if json.NewDecoder(r.Body).Decode(&body) != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	ownerID, ok := forwardOwnerID(userID, body.DeviceID)
	if !ok {
		writeErr(w, http.StatusBadRequest, "invalid device")
		return
	}
	readyCtx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
	ready := h.forwardTunnels.wait(readyCtx, ownerID)
	cancel()
	if ready == nil {
		writeErr(w, http.StatusServiceUnavailable, "forward tunnel is still connecting")
		return
	}
	f, err := h.forwards.Create(ownerID, body.Port, strings.TrimSpace(body.Label))
	if err != nil {
		writeErr(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSONResp(w, h.forwardAPIResponse(f))
}

func (h *Handler) ListForwards(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	ownerID, ok := forwardOwnerID(userID, r.URL.Query().Get("device_id"))
	if !ok {
		writeErr(w, http.StatusBadRequest, "invalid device")
		return
	}
	forwards := h.forwards.List(ownerID)
	sort.Slice(forwards, func(i, j int) bool { return forwards[i].CreatedAt.Before(forwards[j].CreatedAt) })
	result := make([]forwardAPIResponse, 0, len(forwards))
	for _, f := range forwards {
		result = append(result, h.forwardAPIResponse(f))
	}
	writeJSONResp(w, result)
}

func (h *Handler) CloseForward(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	var body struct {
		DeviceID  string `json:"device_id"`
		ForwardID string `json:"forward_id"`
	}
	if json.NewDecoder(r.Body).Decode(&body) != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	ownerID, ok := forwardOwnerID(userID, body.DeviceID)
	if !ok {
		writeErr(w, http.StatusBadRequest, "invalid device")
		return
	}
	if err := h.forwards.Close(strings.TrimSpace(body.ForwardID), ownerID); err != nil {
		writeErr(w, http.StatusNotFound, err.Error())
		return
	}
	writeJSONResp(w, map[string]bool{"closed": true})
}

// ServeForwardHost serves a forward selected by the request hostname. It is
// intentionally a no-op for normal API hosts so the catch-all route does not
// change existing application behavior.
func (h *Handler) ServeForwardHost(w http.ResponseWriter, r *http.Request) {
	forwardID, ok := h.forwardIDFromHost(r.Host)
	if !ok {
		http.NotFound(w, r)
		return
	}
	h.serveForward(w, r, forwardID, forwardHostSandboxPolicy, hostOnlyForwardCookies)
}

// ServeForwardPath serves a portal-proxied forward. It is intentionally kept
// separate from ServeForwardHost because the portal's server-to-server fetch
// reaches the normal API hostname, not the forward's wildcard hostname.
func (h *Handler) ServeForwardPath(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Security-Policy", forwardSandboxPolicy)
	forwardID := chi.URLParam(r, "forwardID")
	if !components.IsForwardID(forwardID) {
		http.NotFound(w, r)
		return
	}

	// Remove the gateway-only routing prefix before sending the request to the
	// local service. For example, the portal asks the API for
	// /forward/<id>/src/main.tsx, while Vite must receive /src/main.tsx.
	prefix := "/forward/" + forwardID
	escapedPath := strings.TrimPrefix(r.URL.EscapedPath(), prefix)
	if escapedPath == "" {
		escapedPath = "/"
	}
	path, err := url.PathUnescape(escapedPath)
	if err != nil || !strings.HasPrefix(path, "/") {
		http.NotFound(w, r)
		return
	}
	forwarded := r.Clone(r.Context())
	forwardedURL := *r.URL
	forwardedURL.Path = path
	if escapedPath == path {
		forwardedURL.RawPath = ""
	} else {
		forwardedURL.RawPath = escapedPath
	}
	forwarded.URL = &forwardedURL
	h.serveForward(w, forwarded, forwardID, forwardSandboxPolicy, dropAllForwardCookies)
}

// ForwardTLSAsk is the local Caddy on-demand TLS authorization hook. It only
// approves UUID hosts that currently refer to a live forward; Caddy never
// obtains certificates for arbitrary attacker-controlled subdomains.
func (h *Handler) ForwardTLSAsk(w http.ResponseWriter, r *http.Request) {
	forwardID, ok := h.forwardIDFromHost(r.URL.Query().Get("domain"))
	if !ok {
		w.WriteHeader(http.StatusForbidden)
		return
	}
	if _, ok := h.forwards.Get(forwardID); !ok {
		w.WriteHeader(http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusOK)
}

// serveForward authorizes and relays one forwarded request. policy is the CSP
// sandbox applied to the forwarded response and cookies the upstream
// Set-Cookie filter (see forward_headers.go).
func (h *Handler) serveForward(w http.ResponseWriter, r *http.Request, forwardID, policy string, cookies forwardCookieFilter) {
	// The gateway's own replies on forward routes (errors, the bootstrap
	// redirect) carry the sandbox too. Streamed responses hijack the
	// connection and get it from the response rewriter instead.
	w.Header().Set("Content-Security-Policy", policy)
	queryToken := strings.TrimSpace(r.URL.Query().Get("secret"))
	token := queryToken
	if token == "" {
		if cookie, err := r.Cookie(forwardCookieName); err == nil {
			token = cookie.Value
		}
	}
	f, ok := h.forwards.Authorize(forwardID, token)
	if !ok {
		writeErr(w, http.StatusNotFound, "forward not found")
		return
	}
	if queryToken != "" {
		http.SetCookie(w, &http.Cookie{
			Name: forwardCookieName, Value: token, Path: "/",
			Secure: true, HttpOnly: true, SameSite: http.SameSiteLaxMode,
			MaxAge: int((12 * time.Hour).Seconds()),
		})
		// The forward token is a bootstrap capability. Store it in a host-only
		// cookie then remove it from the address bar before opening the tunnel,
		// so root-relative assets, fetches, and WebSockets all authenticate
		// without leaking the token to the local application.
		redirectURL := *r.URL
		query := redirectURL.Query()
		query.Del("secret")
		redirectURL.RawQuery = query.Encode()
		http.Redirect(w, r, redirectURL.RequestURI(), http.StatusFound)
		return
	}
	h.serveForwardStream(w, r, f, policy, cookies)
}

// serveForwardStream opens one duplex byte stream to the host's loopback
// socket. Unlike the previous request/response relay, it does not wait for a
// complete HTTP body, so browser streams, server-sent events, WebSockets, and
// other long-lived HTTP connections retain their native behavior.
func (h *Handler) serveForwardStream(w http.ResponseWriter, r *http.Request, f components.PortForward, policy string, cookies forwardCookieFilter) {
	if r.ContentLength > 8<<20 {
		writeErr(w, http.StatusRequestEntityTooLarge, "forwarded request is too large")
		return
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, 8<<20+1))
	if err != nil || len(body) > 8<<20 {
		writeErr(w, http.StatusRequestEntityTooLarge, "forwarded request is too large")
		return
	}
	if tunnel := h.forwardTunnels.get(f.OwnerID); tunnel != nil {
		h.serveDedicatedForwardStream(w, r, f, body, tunnel, policy, cookies)
		return
	}
	if strings.HasPrefix(f.OwnerID, "device\x00") {
		// A desktop data-plane reconnect normally completes within a fraction of
		// a second. Give it a small grace window so browser asset bursts arriving
		// during the handoff do not become intermittent 502 responses.
		waitCtx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
		tunnel := h.forwardTunnels.wait(waitCtx, f.OwnerID)
		cancel()
		if tunnel != nil {
			h.serveDedicatedForwardStream(w, r, f, body, tunnel, policy, cookies)
			return
		}
		writeErr(w, http.StatusBadGateway, "forwarding device is reconnecting")
		return
	}
	hijacker, ok := w.(http.Hijacker)
	if !ok {
		writeErr(w, http.StatusNotImplemented, "forwarded streams require HTTP/1.1")
		return
	}
	conn, buffered, err := hijacker.Hijack()
	if err != nil {
		return
	}
	defer conn.Close()
	browser := newForwardResponseRewriter(conn, policy, cookies, isUpgradeRequest(r.Header))

	requestID := uuid.NewString()
	headers := forwardStreamHeaders(r.Header)
	events, err := h.hub.RequestForwardWebSocket(r.Context(), f.OwnerID, types.ServerForwardWebSocketOpen{
		Type: types.TypeForwardWebSocketOpen, ForwardID: f.ID, Port: f.Port, RequestID: requestID,
		Method: r.Method, Path: forwardedRequestURI(r.URL), Headers: headers, Body: base64.StdEncoding.EncodeToString(body),
	})
	if err != nil {
		return
	}
	openTimer := time.NewTimer(10 * time.Second)
	defer openTimer.Stop()
	select {
	case event, ok := <-events:
		if !ok || !event.Opened {
			slog.Warn("forward stream closed before opening", "forward_id", f.ID, "request_id", requestID)
			return
		}
	case <-openTimer.C:
		slog.Warn("forward stream open timed out", "forward_id", f.ID, "request_id", requestID)
		return
	case <-r.Context().Done():
		return
	}

	writeDone := make(chan struct{})
	go func() {
		defer close(writeDone)
		defer conn.Close()
		for event := range events {
			if event.Closed {
				return
			}
			data, decodeErr := base64.StdEncoding.DecodeString(event.Data)
			if decodeErr != nil {
				return
			}
			if _, err := browser.Write(data); err != nil {
				return
			}
		}
	}()
	defer func() {
		h.hub.CloseForwardWebSocket(context.Background(), f.OwnerID, requestID)
		select {
		case <-writeDone:
		case <-time.After(time.Second):
		}
	}()
	buffer := make([]byte, 32*1024)
	for {
		size, readErr := buffered.Read(buffer)
		if size > 0 {
			h.hub.ForwardWebSocketData(r.Context(), f.OwnerID, requestID, "data", base64.StdEncoding.EncodeToString(buffer[:size]))
		}
		if readErr != nil {
			return
		}
	}
}

// serveDedicatedForwardStream uses the host's binary data-plane socket. Stream
// bytes never enter the terminal-sharing hub, Redis bus, JSON codec, or UI
// actor; the session socket remains a control plane only.
func (h *Handler) serveDedicatedForwardStream(w http.ResponseWriter, r *http.Request, f components.PortForward, body []byte, tunnel *forwardTunnelHost, policy string, cookies forwardCookieFilter) {
	openCtx, cancel := context.WithTimeout(r.Context(), 10*time.Second)
	stream, err := tunnel.open(openCtx, forwardTunnelOpen{
		Port: f.Port, Method: r.Method, Path: forwardedRequestURI(r.URL), Headers: forwardStreamHeaders(r.Header),
	}, body)
	cancel()
	if err != nil {
		slog.Warn("dedicated forward stream open failed", "forward_id", f.ID, "error", err)
		writeErr(w, http.StatusBadGateway, "local port is unavailable")
		return
	}
	defer stream.close()

	hijacker, ok := w.(http.Hijacker)
	if !ok {
		writeErr(w, http.StatusNotImplemented, "forwarded streams require HTTP/1.1")
		return
	}
	conn, buffered, err := hijacker.Hijack()
	if err != nil {
		return
	}
	defer conn.Close()
	browser := newForwardResponseRewriter(conn, policy, cookies, isUpgradeRequest(r.Header))

	writeDone := make(chan struct{})
	go func() {
		defer close(writeDone)
		defer conn.Close()
		for {
			select {
			case event := <-stream.events:
				switch event.kind {
				case tunnelFrameData:
					if _, err := browser.Write(event.data); err != nil {
						return
					}
				case tunnelFrameFin, tunnelFrameReset:
					return
				}
			case <-tunnel.done:
				return
			case <-r.Context().Done():
				return
			}
		}
	}()

	buffer := make([]byte, 32*1024)
	for {
		size, readErr := buffered.Read(buffer)
		if size > 0 {
			if err := stream.send(r.Context(), tunnelFrameData, buffer[:size]); err != nil {
				return
			}
		}
		if readErr != nil {
			_ = stream.send(context.Background(), tunnelFrameFin, nil)
			select {
			case <-writeDone:
			case <-time.After(time.Second):
			}
			return
		}
	}
}

func forwardStreamHeaders(headers http.Header) [][2]string {
	forwarded := make([][2]string, 0, 16)
	for name, values := range headers {
		// These headers describe the public gateway hop. They must not reach
		// the loopback service: Caddy adds X-Forwarded-Host for the public
		// forward hostname, and dev servers such as Vite use it when applying
		// their host allow-list. The desktop writes the sole local Host header.
		if strings.EqualFold(name, "host") ||
			strings.EqualFold(name, "proxy-connection") ||
			strings.EqualFold(name, "forwarded") ||
			strings.HasPrefix(strings.ToLower(name), "x-forwarded-") ||
			strings.EqualFold(name, "x-real-ip") {
			continue
		}
		for _, value := range values {
			if len(forwarded) == 64 {
				return forwarded
			}
			forwarded = append(forwarded, [2]string{name, value})
		}
	}
	return forwarded
}

// forwardedRequestURI preserves the local application's path and query while
// removing the gateway-only share capability. The capability must never be
// exposed to the developer service, including on WebSocket upgrades.
func forwardedRequestURI(raw *url.URL) string {
	u := *raw
	q := u.Query()
	q.Del("secret")
	u.RawQuery = q.Encode()
	return u.RequestURI()
}

func (h *Handler) forwardIDFromHost(rawHost string) (string, bool) {
	if h.forwardDomain == "" {
		return "", false
	}
	host := strings.ToLower(rawHost)
	if parsed, err := url.Parse("//" + rawHost); err == nil && parsed.Hostname() != "" {
		host = strings.ToLower(parsed.Hostname())
	}
	suffix := "." + h.forwardDomain
	if !strings.HasSuffix(host, suffix) {
		return "", false
	}
	id := strings.TrimSuffix(host, suffix)
	if !components.IsForwardID(id) {
		return "", false
	}
	return id, true
}

// wsSink buffers frames for one connection; a writer goroutine drains it. Send
// applies bounded backpressure when the peer is slow and is safe after close
// (selects on `done`), so the hub's dispatch goroutine can push concurrently
// with teardown without silently losing forwarding control frames.
type wsSink struct {
	ch   chan []byte
	done chan struct{}
	once sync.Once
}

func newWSSink() *wsSink {
	// A forwarded Vite page can request hundreds of modules in parallel. Each
	// module opens its own rewritten tunnel stream, so the host must absorb that
	// short burst without silently dropping open frames and leaving Cloudflare
	// to return a 502. This remains bounded while comfortably covering normal
	// frontend dependency graphs.
	return &wsSink{ch: make(chan []byte, 1024), done: make(chan struct{})}
}

func (s *wsSink) Send(data []byte) {
	select {
	case s.ch <- data:
	case <-s.done:
	}
}

func (s *wsSink) close() { s.once.Do(func() { close(s.done) }) }

func writeErr(w http.ResponseWriter, status int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}

func writeJSONResp(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

// ── Teammate share links (REST) ──────────────────────────────────────────────

// normalizeAccess maps a requested access string to a known role, defaulting to
// View. Only "control" grants Edit.
func normalizeAccess(requested string) string {
	if requested == components.AccessControl {
		return components.AccessControl
	}
	return components.AccessViewOnly
}

// createLink mints a fresh per-link secret (which is also the link token), stores
// it against the session at the given role, and returns the secret + join URL.
// The caller must have already confirmed the requester owns the session.
func (h *Handler) createLink(ctx context.Context, sessionID, access string, opts ...func(*components.ShareLink)) (secret, url string, err error) {
	secret = uuid.NewString()
	link := components.ShareLink{
		SessionID: sessionID, Secret: secret, Access: access,
	}
	for _, opt := range opts {
		opt(&link)
	}
	if err = h.store.PutShareLink(ctx, secret, link); err != nil {
		return "", "", err
	}
	return secret, h.joinURL(sessionID, secret), nil
}

// requireHost loads the session and confirms the caller owns it. It writes the
// appropriate error response and returns ok=false on any failure.
func (h *Handler) requireHost(w http.ResponseWriter, r *http.Request, sessionID string) bool {
	userID, _ := r.Context().Value("user_id").(string)
	rec, ok, err := h.store.GetSession(r.Context(), sessionID)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not load session")
		return false
	}
	if !ok {
		writeErr(w, http.StatusNotFound, "session not found")
		return false
	}
	if rec.HostUser != userID {
		writeErr(w, http.StatusForbidden, "only the host can share this session")
		return false
	}
	return true
}

// CreateShareLink issues a revocable, role-scoped invite link. Only the session's
// host may create one. The URL's secret IS the capability — a View link admits
// anonymously; an Edit link requires the joiner to be signed in.
func (h *Handler) CreateShareLink(w http.ResponseWriter, r *http.Request) {
	var body struct {
		SessionID string `json:"session_id"`
		Access    string `json:"access"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if !h.requireHost(w, r, body.SessionID) {
		return
	}
	access := normalizeAccess(body.Access)
	token, url, err := h.createLink(r.Context(), body.SessionID, access)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not create share link")
		return
	}
	writeJSONResp(w, map[string]string{"token": token, "join_url": url, "access": access})
}

// InviteByEmail creates a role-scoped link and emails it to the address. The link
// is returned regardless of whether the email send succeeds, so the host can
// always copy it manually.
func (h *Handler) InviteByEmail(w http.ResponseWriter, r *http.Request) {
	var body struct {
		SessionID string `json:"session_id"`
		Email     string `json:"email"`
		Access    string `json:"access"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	email := strings.TrimSpace(body.Email)
	if email == "" || !strings.Contains(email, "@") {
		writeErr(w, http.StatusBadRequest, "a valid email is required")
		return
	}
	if !h.requireHost(w, r, body.SessionID) {
		return
	}
	access := normalizeAccess(body.Access)
	invitedName, invitedAvatar := h.profileForEmail(r.Context(), email)
	token, url, err := h.createLink(r.Context(), body.SessionID, access, func(link *components.ShareLink) {
		link.InvitedEmail = email
		link.InvitedDisplayName = invitedName
		link.InvitedAvatarURL = invitedAvatar
	})
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not create invite")
		return
	}
	emailed := false
	if h.inviter != nil {
		if err := h.inviter.SendSessionInvite(r.Context(), email, url, access); err != nil {
			slog.Warn("realtime: invite email failed", "email", email, "error", err)
		} else {
			emailed = true
		}
	}
	writeJSONResp(w, map[string]any{"token": token, "join_url": url, "access": access, "emailed": emailed})
}

// RevokeShareLink deletes a share link. Only the session's host may revoke it.
func (h *Handler) RevokeShareLink(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	var body struct {
		Token string `json:"token"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	token := strings.TrimSpace(body.Token)
	link, ok, _ := h.store.ResolveShareLink(r.Context(), token)
	if !ok {
		writeErr(w, http.StatusNotFound, "link invalid or expired")
		return
	}
	rec, ok, _ := h.store.GetSession(r.Context(), link.SessionID)
	if !ok || rec.HostUser != userID {
		writeErr(w, http.StatusForbidden, "only the host can revoke this link")
		return
	}
	_ = h.store.DeleteShareLink(r.Context(), token)
	writeJSONResp(w, map[string]bool{"revoked": true})
}

// ── WebSocket ────────────────────────────────────────────────────────────────

func (h *Handler) ServeWS(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string) // RequireAuth guarantees this

	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{
		OriginPatterns: []string{"*"}, // auth is by Bearer token, not Origin
	})
	if err != nil {
		slog.Warn("realtime: accept failed", "user", userID, "error", err)
		return
	}
	conn.SetReadLimit(readLimitBytes)

	ctx := r.Context()
	sink := newWSSink()
	writerDone := make(chan struct{})
	go func() {
		h.writeLoop(ctx, conn, sink)
		close(writerDone)
	}()
	// Graceful shutdown: stop the writer, let it FLUSH any queued frames (e.g. a
	// final "ended"/"terminated" sent just before returning), then close. Without
	// waiting for the flush, the close races the writer and the client sees a
	// bare close frame instead of the message.
	defer func() {
		sink.close()
		<-writerDone
		conn.Close(websocket.StatusNormalClosure, "")
	}()

	// First frame decides the role: initialize/reconnect = host, join = viewer.
	firstCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	typ, data, err := conn.Read(firstCtx)
	cancel()
	if err != nil || typ != websocket.MessageText {
		return
	}
	var env types.Envelope
	if json.Unmarshal(data, &env) != nil {
		return
	}

	switch env.Type {
	case types.TypeInitialize, types.TypeReconnect:
		h.serveHost(ctx, conn, sink, userID, env.Type, data)
	case types.TypeJoin:
		h.serveViewer(ctx, conn, sink, userID, data)
	default:
		slog.Warn("realtime: unexpected first frame", "user", userID, "type", env.Type)
	}
}

// ServeForwardTunnel accepts the desktop host's dedicated forwarding data
// plane. Authentication identifies the account; the session lookup proves that
// account owns the requested session. Browser traffic is carried only as binary
// frames on this socket and cannot mix with shared-terminal control messages.
func (h *Handler) ServeForwardTunnel(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	sessionID := strings.TrimSpace(r.URL.Query().Get("session"))
	deviceID := strings.TrimSpace(r.URL.Query().Get("device"))
	if userID == "" || (sessionID == "" && deviceID == "") {
		writeErr(w, http.StatusUnauthorized, "login required for forward tunnel")
		return
	}
	ownerID := sessionID
	if deviceID != "" {
		var ok bool
		ownerID, ok = forwardOwnerID(userID, deviceID)
		if !ok {
			writeErr(w, http.StatusBadRequest, "invalid device")
			return
		}
	} else {
		rec, ok, err := h.store.GetSession(r.Context(), sessionID)
		if err != nil || !ok || rec.HostUser != userID {
			writeErr(w, http.StatusForbidden, "session is not owned by this host")
			return
		}
	}
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{OriginPatterns: []string{"*"}})
	if err != nil {
		slog.Warn("forward tunnel accept failed", "owner", ownerID, "error", err)
		return
	}
	conn.SetReadLimit(forwardTunnelReadLimit)
	host := newForwardTunnelHost(conn)
	h.forwardTunnels.register(ownerID, host)
	defer func() {
		h.forwardTunnels.unregister(ownerID, host)
		_ = conn.Close(websocket.StatusNormalClosure, "")
	}()

	writerDone := make(chan struct{})
	go func() {
		defer close(writerDone)
		for {
			select {
			case frame := <-host.writes:
				if err := conn.Write(r.Context(), websocket.MessageBinary, frame); err != nil {
					host.close()
					return
				}
			case <-host.done:
				return
			case <-r.Context().Done():
				return
			}
		}
	}()

	for {
		messageType, frame, err := conn.Read(r.Context())
		if err != nil {
			slog.Info("forward tunnel disconnected", "owner", ownerID, "error", err)
			break
		}
		if messageType != websocket.MessageBinary {
			continue
		}
		kind, streamID, payload, err := decodeTunnelFrame(frame)
		if err != nil || (kind != tunnelFrameOpened && kind != tunnelFrameData && kind != tunnelFrameFin && kind != tunnelFrameReset) {
			_ = conn.Close(websocket.StatusPolicyViolation, "invalid forward tunnel frame")
			break
		}
		host.dispatch(kind, streamID, payload)
	}
	host.close()
	<-writerDone
}

func (h *Handler) serveHost(ctx context.Context, conn *websocket.Conn, sink *wsSink, userID, firstType string, first []byte) {
	// Hosting a session requires a signed-in account — it owns the session and
	// every invite link is issued against it.
	if userID == "" {
		sink.Send(mustJSON(types.Terminated("login required to host a session")))
		return
	}
	hostName, hostAvatar := h.profileFor(ctx, userID)
	var sessionID string
	switch firstType {
	case types.TypeReconnect:
		var m types.HostReconnect
		_ = json.Unmarshal(first, &m)
		sessionID = m.Session.String()
		rec, ok, _ := h.store.GetSession(context.Background(), sessionID)
		if !ok || rec.ReconnectToken != m.Token || rec.HostUser != userID {
			sink.Send(mustJSON(types.Terminated("cannot reconnect: unknown or expired session")))
			return
		}
		h.hub.RegisterHost(sessionID, userID, hostName, hostAvatar, sink)
		sink.Send(mustJSON(types.Reconnected()))
		slog.Info("realtime: session reconnected", "user", userID, "session", sessionID)
	default: // initialize
		var m types.HostInitialize
		_ = json.Unmarshal(first, &m)
		sessionID = uuid.NewString()
		reconnectToken := uuid.NewString()
		rec := components.SessionRecord{ID: sessionID, ReconnectToken: reconnectToken, HostUser: userID}
		if err := h.store.CreateSession(context.Background(), rec); err != nil {
			sink.Send(mustJSON(types.Terminated("could not create session")))
			return
		}
		h.hub.RegisterHost(sessionID, userID, hostName, hostAvatar, sink)
		// No pairing code: access is granted only through role-scoped share links.
		sink.Send(mustJSON(types.Initialized(uuid.MustParse(sessionID), nil, reconnectToken)))
		slog.Info("realtime: session opened", "user", userID, "session", sessionID, "device", m.DeviceID)
	}
	defer h.hub.UnregisterHost(context.Background(), sessionID)

	for {
		typ, data, err := conn.Read(ctx)
		if err != nil {
			slog.Info("realtime: host disconnected", "user", userID, "session", sessionID, "error", err)
			return
		}
		if typ != websocket.MessageText {
			continue
		}
		var env types.Envelope
		if json.Unmarshal(data, &env) != nil {
			continue
		}
		switch env.Type {
		case types.TypeDelta:
			h.hub.BroadcastDelta(ctx, sessionID, data)
		case types.TypeSetParticipantRole:
			var m types.HostSetParticipantRole
			_ = json.Unmarshal(data, &m)
			h.hub.SetParticipantAccess(ctx, sessionID, m.Participant.String(), m.Access)
		case types.TypeDenyAccessRequest:
			var m types.HostDenyAccessRequest
			_ = json.Unmarshal(data, &m)
			h.hub.SendToViewer(ctx, sessionID, m.Participant.String(), mustJSON(types.AccessDenied()))
		case types.TypeRemoveParticipant:
			var m types.HostRemoveParticipant
			_ = json.Unmarshal(data, &m)
			h.hub.RemoveParticipant(ctx, sessionID, m.Participant.String())
		case types.TypeEndSession:
			var m types.HostEndSession
			_ = json.Unmarshal(data, &m)
			// Always record a reason: it carries the host's message (e.g. the
			// inactivity auto-end) to viewers AND marks this as an EXPLICIT end,
			// so UnregisterHost tears down immediately instead of waiting out the
			// reconnect grace period meant for unexpected drops.
			reason := m.Reason
			if reason == "" {
				reason = "The host ended this session."
			}
			h.hub.SetEndReason(sessionID, reason)
			_ = h.store.DeleteSession(context.Background(), sessionID)
			h.forwards.CloseOwner(sessionID)
			sink.Send(mustJSON(types.Terminated("ended by host")))
			return
		case types.TypeForwardCreate:
			var m types.HostForwardCreate
			if json.Unmarshal(data, &m) != nil {
				continue
			}
			// New clients must have their dedicated data plane registered before
			// the capability URL is exposed. Older clients omit tunnel_version and
			// continue to use the compatibility relay during a rolling deployment.
			if m.TunnelVersion >= 1 {
				readyCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
				ready := h.forwardTunnels.wait(readyCtx, sessionID)
				cancel()
				if ready == nil {
					sink.Send(mustJSON(types.ServerForwardError{Type: types.TypeForwardError, Reason: "forward tunnel is still connecting; try again"}))
					continue
				}
			}
			f, err := h.forwards.Create(sessionID, m.Port, strings.TrimSpace(m.Label))
			if err != nil {
				sink.Send(mustJSON(types.ServerForwardError{Type: types.TypeForwardError, Reason: err.Error()}))
				continue
			}
			h.hub.RegisterForward(sessionID, f)
			// A forward has its own bearer capability; it is independent from a
			// terminal-share link and is carried only in the bootstrap URL.
			publicURL := h.publicForwardURL(f)
			sink.Send(mustJSON(types.ServerForwardCreated{Type: types.TypeForwardCreated, ForwardID: f.ID, Port: f.Port, PublicURL: publicURL}))
		case types.TypeForwardClose:
			var m types.HostForwardClose
			if json.Unmarshal(data, &m) == nil {
				if err := h.forwards.Close(m.ForwardID, sessionID); err == nil {
					h.hub.UnregisterForward(sessionID, m.ForwardID)
					sink.Send(mustJSON(types.ServerForwardClosed{Type: types.TypeForwardClosed, ForwardID: m.ForwardID}))
				}
			}
		case types.TypeForwardResponse:
			var m types.HostForwardResponse
			if json.Unmarshal(data, &m) == nil {
				h.hub.ForwardResponse(ctx, sessionID, types.ServerForwardResponse{
					Type: types.TypeForwardResponse, RequestID: m.RequestID, Status: m.Status,
					Headers: m.Headers, Body: m.Body,
				})
			}
		case types.TypeForwardWebSocketOpened:
			var m types.HostForwardWebSocketOpen
			if json.Unmarshal(data, &m) == nil {
				h.hub.ForwardWebSocketEvent(ctx, sessionID, types.ServerForwardWebSocketEvent{Opened: true, RequestID: m.RequestID})
			}
		case types.TypeForwardWebSocketMessage:
			var m types.HostForwardWebSocketData
			if json.Unmarshal(data, &m) == nil {
				h.hub.ForwardWebSocketEvent(ctx, sessionID, types.ServerForwardWebSocketEvent{RequestID: m.RequestID, MessageType: m.MessageType, Data: m.Data})
			}
		case types.TypeForwardWebSocketClosed:
			var m types.HostForwardWebSocketClose
			if json.Unmarshal(data, &m) == nil {
				h.hub.ForwardWebSocketEvent(ctx, sessionID, types.ServerForwardWebSocketEvent{Closed: true, RequestID: m.RequestID})
			}
		case types.TypePing:
			sink.Send(mustJSON(types.Pong()))
		case types.TypeResolveCommand:
			var m types.HostResolveCommand
			_ = json.Unmarshal(data, &m)
			h.hub.ResolveCommand(context.Background(), sessionID, m.RequestID.String(), m.Approved)
		default:
			slog.Debug("realtime: host frame ignored", "session", sessionID, "type", env.Type)
		}
	}
}

func (h *Handler) serveViewer(ctx context.Context, conn *websocket.Conn, sink *wsSink, userID string, first []byte) {
	var m types.ViewerJoin
	if json.Unmarshal(first, &m) != nil {
		return
	}
	sessionID := m.Session.String()

	// The presented secret IS a share-link token; it resolves to the session and
	// the role the host granted for this link.
	link, ok, err := h.store.ResolveShareLink(context.Background(), m.Secret)
	if err != nil || !ok || link.SessionID != sessionID {
		sink.Send(mustJSON(types.Ended("invalid or expired link")))
		slog.Info("realtime: viewer join rejected", "user", userID, "session", sessionID)
		return
	}

	// Share links outlive the session (24h TTL), so a link can resolve after the
	// host has stopped/ended the session (its record is deleted on end). Reject
	// the join with a clear reason so a viewer who refreshes sees "session ended"
	// rather than a dead, output-less live view.
	sessionRec, exists, _ := h.store.GetSession(context.Background(), sessionID)
	if !exists {
		sink.Send(mustJSON(types.Ended("This session has ended.")))
		slog.Info("realtime: viewer join rejected — session ended", "user", userID, "session", sessionID)
		return
	}
	// The joiner is the host's OWN account (e.g. their second device) when signed
	// in as the same user that owns the session. Only such a controller may drive
	// the host's terminal size.
	isHostAccount := userID != "" && userID == sessionRec.HostUser

	// View links admit anyone anonymously; Edit links require a signed-in account
	// so every command is attributed and audited.
	role := types.RoleViewer
	if link.Access == components.AccessControl {
		if userID == "" {
			sink.Send(mustJSON(types.Ended("login required to edit this session")))
			slog.Info("realtime: edit join rejected — not signed in", "session", sessionID)
			return
		}
		role = types.RoleController
	}

	participant := types.Participant{
		ID:          uuid.New(),
		DisplayName: m.DisplayName,
		Role:        role,
		Present:     true,
	}
	// Signed-in joiners use their account profile (full name + avatar) when the
	// email/user already exists in our DB — prefer that over the client-sent name.
	if name, avatar := h.profileFor(ctx, userID); name != "" || avatar != "" {
		if name != "" {
			participant.DisplayName = name
		}
		participant.AvatarURL = avatar
	} else if link.InvitedDisplayName != "" || link.InvitedAvatarURL != "" {
		if link.InvitedDisplayName != "" {
			participant.DisplayName = link.InvitedDisplayName
		}
		participant.AvatarURL = link.InvitedAvatarURL
	}
	sink.Send(mustJSON(types.Joined(participant.ID, participant.Role, isHostAccount)))
	capabilities := types.ViewerCapabilities{}
	if m.Capabilities != nil {
		capabilities = *m.Capabilities
	}
	h.hub.RegisterViewerWithCapabilities(ctx, sessionID, participant, userID, sink, capabilities)
	defer h.hub.UnregisterViewer(context.Background(), sessionID, participant)
	slog.Info("realtime: viewer joined", "user", userID, "session", sessionID, "participant", participant.ID)

	for {
		typ, data, err := conn.Read(ctx)
		if err != nil {
			slog.Info("realtime: viewer disconnected", "user", userID, "session", sessionID, "error", err)
			return
		}
		if typ != websocket.MessageText {
			continue
		}
		var env types.Envelope
		if json.Unmarshal(data, &env) != nil {
			continue
		}
		switch env.Type {
		case types.TypeExecuteCommand:
			var c types.ViewerExecuteCommand
			_ = json.Unmarshal(data, &c)
			h.hub.RequestCommand(ctx, sessionID, participant.ID.String(), c.Command)
		case types.TypeAgentQuery:
			var q types.ViewerAgentQuery
			if json.Unmarshal(data, &q) == nil {
				h.hub.RequestAgentQuery(ctx, sessionID, participant.ID.String(), q.RequestID.String(), q.Query)
			}
		case types.TypeAgentStop:
			h.hub.RequestAgentStop(ctx, sessionID, participant.ID.String())
		case types.TypeInputBytes:
			var in types.ViewerInputBytes
			_ = json.Unmarshal(data, &in)
			h.hub.ForwardInput(ctx, sessionID, participant.ID.String(), in.DataBase64)
		case types.TypeResize:
			var rz types.ViewerResize
			_ = json.Unmarshal(data, &rz)
			h.hub.ForwardResize(ctx, sessionID, participant.ID.String(), rz.Rows, rz.Cols)
		case types.TypeControl:
			var ctrl types.ViewerControl
			_ = json.Unmarshal(data, &ctrl)
			h.hub.ForwardControl(ctx, sessionID, participant.ID.String(), ctrl.Surface)
		case types.TypePointer:
			var pointer types.ViewerPointer
			if json.Unmarshal(data, &pointer) == nil {
				h.hub.ForwardPointer(ctx, sessionID, participant.ID.String(), pointer.Position)
			}
		case types.TypeRequestControl:
			slog.Info("realtime: control requested", "session", sessionID, "participant", participant.ID)
			h.hub.RequestAccess(ctx, sessionID, participant.ID.String())
		case types.TypePing:
			sink.Send(mustJSON(types.Pong()))
		default:
			slog.Debug("realtime: viewer frame ignored", "session", sessionID, "type", env.Type)
		}
	}
}

func (h *Handler) writeLoop(ctx context.Context, conn *websocket.Conn, sink *wsSink) {
	for {
		select {
		case data := <-sink.ch:
			if err := conn.Write(ctx, websocket.MessageText, data); err != nil {
				return
			}
		case <-sink.done:
			// Drain and flush any frames queued before shutdown (best effort),
			// then exit so the caller can close the socket.
			for {
				select {
				case data := <-sink.ch:
					if err := conn.Write(ctx, websocket.MessageText, data); err != nil {
						return
					}
				default:
					return
				}
			}
		case <-ctx.Done():
			return
		}
	}
}

func mustJSON(v any) []byte {
	data, _ := json.Marshal(v)
	return data
}
