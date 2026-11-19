package api

import (
	"context"
	"database/sql"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"server/internal/identity/components"
	"server/internal/identity/model"
	"server/internal/platform/ratelimit"
)

// unknownUsers answers every lookup with "not found", so requests stop at
// the 404 path without needing a token store or mailer.
type unknownUsers struct{ model.UserRepository }

func (unknownUsers) GetByEmail(context.Context, string) (*model.User, error) {
	return nil, sql.ErrNoRows
}

func newLimitedHandlers(l MagicLinkLimits) *AuthHandlers {
	h := NewAuthHandlers(components.NewSendMagicLinkUseCase(unknownUsers{}, nil, nil, nil), nil, nil, nil, nil, nil)
	h.SetMagicLinkLimits(l)
	return h
}

func magicRequest(email, remote string, headers map[string]string) *http.Request {
	r := httptest.NewRequest(http.MethodPost, "/auth/magic/request",
		strings.NewReader(fmt.Sprintf(`{"email":%q,"state":"s"}`, email)))
	r.RemoteAddr = remote
	for k, v := range headers {
		r.Header.Set(k, v)
	}
	return r
}

func serve(h *AuthHandlers, r *http.Request) *httptest.ResponseRecorder {
	w := httptest.NewRecorder()
	h.RequestMagicLink(w, r)
	return w
}

func TestMagicLinkPerEmailLimit(t *testing.T) {
	h := newLimitedHandlers(MagicLinkLimits{PerEmail: ratelimit.NewMemoryRateLimiter(2, 15*time.Minute, nil)})
	for i, email := range []string{"a@example.com", " A@Example.com "} {
		if w := serve(h, magicRequest(email, "198.51.100.1:1", nil)); w.Code != http.StatusNotFound {
			t.Fatalf("request %d: status %d, want 404", i+1, w.Code)
		}
	}
	w := serve(h, magicRequest("a@example.com", "198.51.100.2:1", nil))
	if w.Code != http.StatusTooManyRequests || w.Header().Get("Retry-After") == "" {
		t.Fatalf("third request for the same inbox: status %d Retry-After %q, want 429 with Retry-After", w.Code, w.Header().Get("Retry-After"))
	}
	if w := serve(h, magicRequest("b@example.com", "198.51.100.2:1", nil)); w.Code != http.StatusNotFound {
		t.Fatalf("other inbox: status %d, want 404", w.Code)
	}
}

func TestMagicLinkPerIPLimitUsesPortalForwardedIP(t *testing.T) {
	h := newLimitedHandlers(MagicLinkLimits{
		PerIP:        ratelimit.NewMemoryRateLimiter(1, 15*time.Minute, nil),
		PortalSecret: "s3cret",
	})
	portal := "10.0.0.5:443"
	visitor := func(ip string) map[string]string {
		return map[string]string{PortalSecretHeader: "s3cret", PortalClientIPHeader: ip}
	}
	if w := serve(h, magicRequest("a@example.com", portal, visitor("203.0.113.7"))); w.Code != http.StatusNotFound {
		t.Fatalf("first visitor: status %d, want 404", w.Code)
	}
	// A different visitor behind the same portal is not throttled.
	if w := serve(h, magicRequest("b@example.com", portal, visitor("203.0.113.8"))); w.Code != http.StatusNotFound {
		t.Fatalf("second visitor: status %d, want 404", w.Code)
	}
	if w := serve(h, magicRequest("c@example.com", portal, visitor("203.0.113.7"))); w.Code != http.StatusTooManyRequests {
		t.Fatalf("repeat visitor: status %d, want 429", w.Code)
	}
}

func TestMagicLinkIgnoresForwardedIPWithoutPortalSecret(t *testing.T) {
	h := newLimitedHandlers(MagicLinkLimits{
		PerIP:        ratelimit.NewMemoryRateLimiter(1, 15*time.Minute, nil),
		PortalSecret: "s3cret",
	})
	spoof := func(ip string) map[string]string {
		return map[string]string{PortalSecretHeader: "wrong", PortalClientIPHeader: ip}
	}
	serve(h, magicRequest("a@example.com", "192.0.2.9:1", spoof("203.0.113.1")))
	if w := serve(h, magicRequest("b@example.com", "192.0.2.9:1", spoof("203.0.113.2"))); w.Code != http.StatusTooManyRequests {
		t.Fatalf("rotating a spoofed client IP must not escape the limit: status %d", w.Code)
	}
}

func TestMagicLinkCloudflareIPPreferred(t *testing.T) {
	h := newLimitedHandlers(MagicLinkLimits{
		PerIP:        ratelimit.NewMemoryRateLimiter(1, 15*time.Minute, nil),
		PortalSecret: "s3cret",
	})
	cf := func(ip string) map[string]string { return map[string]string{"CF-Connecting-IP": ip} }
	serve(h, magicRequest("a@example.com", "172.64.0.1:1", cf("203.0.113.1")))
	if w := serve(h, magicRequest("b@example.com", "172.64.0.1:1", cf("203.0.113.2"))); w.Code != http.StatusNotFound {
		t.Fatalf("different Cloudflare visitor: status %d, want 404", w.Code)
	}
}

func TestMagicLinkSkipsIPLimitWithoutPortalSecret(t *testing.T) {
	h := newLimitedHandlers(MagicLinkLimits{PerIP: ratelimit.NewMemoryRateLimiter(1, 15*time.Minute, nil)})
	for i := 0; i < 3; i++ {
		if w := serve(h, magicRequest(fmt.Sprintf("u%d@example.com", i), "10.0.0.5:443", nil)); w.Code != http.StatusNotFound {
			t.Fatalf("request %d: status %d, want 404 (per-IP limit off without the portal secret)", i+1, w.Code)
		}
	}
}

// A caller that isn't Cloudflare (or our Caddy) can't pick its own address
// with CF-Connecting-IP, X-Real-IP or True-Client-IP.
func TestMagicLinkSpoofedProxyHeadersDoNotEscapeIPLimit(t *testing.T) {
	for _, header := range []string{"CF-Connecting-IP", "X-Real-IP", "True-Client-IP"} {
		h := newLimitedHandlers(MagicLinkLimits{
			PerIP:        ratelimit.NewMemoryRateLimiter(1, 15*time.Minute, nil),
			PortalSecret: "s3cret",
		})
		spoof := func(ip string) map[string]string { return map[string]string{header: ip} }
		serve(h, magicRequest("a@example.com", "198.51.100.20:1", spoof("203.0.113.1")))
		if w := serve(h, magicRequest("b@example.com", "198.51.100.20:1", spoof("203.0.113.2"))); w.Code != http.StatusTooManyRequests {
			t.Fatalf("%s: rotating a spoofed address must not escape the limit: status %d", header, w.Code)
		}
	}
}
