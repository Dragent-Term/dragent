package api

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"net"
	"net/http"
	"strings"

	"server/internal/platform/ratelimit"
)

// Headers the auth portal sets when it proxies POST /auth/magic/request.
// The portal calls this API server-to-server, so without them every visitor
// would share the portal's own IP and one IP bucket.
const (
	PortalClientIPHeader = "X-Dragent-Client-IP"
	PortalSecretHeader   = "X-Dragent-Portal-Secret"
)

// MagicLinkLimits bounds magic-link requests. Per email stops one inbox
// being flooded; per IP stops one client spraying many inboxes (and the
// email quota) and slows account enumeration.
type MagicLinkLimits struct {
	PerEmail ratelimit.Limiter
	PerIP    ratelimit.Limiter
	// PortalSecret, when set, lets a request carrying it in
	// PortalSecretHeader name the visitor's IP in PortalClientIPHeader.
	// When empty the per-IP limit is skipped: every portal request would
	// otherwise count against the portal's single IP and lock out sign-in
	// for everyone.
	PortalSecret string
}

// SetMagicLinkLimits installs the magic-link rate limits.
func (h *AuthHandlers) SetMagicLinkLimits(l MagicLinkLimits) {
	h.magicLimits = l
}

// allowMagicLink applies the limits, writing a 429/503 itself when refused.
// The IP rule runs first so a refused sprayer does not also spend the
// victim inbox's budget.
func (h *AuthHandlers) allowMagicLink(w http.ResponseWriter, r *http.Request, email string) bool {
	l := h.magicLimits
	var rules []ratelimit.Rule
	if l.PerIP != nil && l.PortalSecret != "" {
		rules = append(rules, ratelimit.Rule{Limiter: l.PerIP, Key: "rl:magic:ip:" + magicClientIP(r, l.PortalSecret)})
	}
	if l.PerEmail != nil {
		// Hashed: Redis keys show up in tooling and dumps; the address must not.
		sum := sha256.Sum256([]byte(strings.ToLower(strings.TrimSpace(email))))
		rules = append(rules, ratelimit.Rule{Limiter: l.PerEmail, Key: "rl:magic:email:" + hex.EncodeToString(sum[:])})
	}
	return ratelimit.Check(w, r, rules...)
}

// magicClientIP trusts the portal's forwarded visitor IP only when the
// request proves it came from the portal; anything else is counted against
// the connecting address.
func magicClientIP(r *http.Request, secret string) string {
	given := r.Header.Get(PortalSecretHeader)
	if given != "" && subtle.ConstantTimeCompare([]byte(given), []byte(secret)) == 1 {
		if ip := net.ParseIP(strings.TrimSpace(r.Header.Get(PortalClientIPHeader))); ip != nil {
			return ip.String()
		}
	}
	return ratelimit.ClientIP(r)
}
