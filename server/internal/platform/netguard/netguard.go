// Package netguard validates user supplied destination URLs and prevents
// outbound connections to non-public IP addresses.
package netguard

import (
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"os"
	"strings"
	"sync"
	"syscall"
	"time"
)

// ValidateURL accepts HTTPS everywhere and HTTP only in explicit local/dev environments.
func ValidateURL(raw string) error {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u == nil || u.Hostname() == "" {
		return fmt.Errorf("invalid custom base URL")
	}
	if u.User != nil {
		return fmt.Errorf("custom base URL must not contain credentials")
	}
	if u.Scheme != "https" && !(u.Scheme == "http" && isDevEnv()) {
		return fmt.Errorf("custom base URL must use https")
	}
	if u.RawQuery != "" || u.Fragment != "" {
		return fmt.Errorf("custom base URL must not contain a query or fragment")
	}
	return nil
}

func isDevEnv() bool {
	switch strings.ToLower(strings.TrimSpace(os.Getenv("APP_ENV"))) {
	case "dev", "local":
		return true
	default:
		return false
	}
}

// NewClient returns a client for a user-supplied destination. It shares one
// guarded Transport (so connections are pooled, not leaked per call), checks
// every resolved address at dial time, and re-checks each redirect hop.
func NewClient(timeout time.Duration) *http.Client {
	return &http.Client{Timeout: timeout, Transport: SharedTransport(), CheckRedirect: checkRedirect}
}

var (
	sharedOnce      sync.Once
	sharedTransport *http.Transport
)

// SharedTransport returns the process-wide guarded Transport. Prefer NewClient,
// which also limits redirects.
func SharedTransport() *http.Transport {
	sharedOnce.Do(func() {
		dialer := &net.Dialer{Timeout: 10 * time.Second, KeepAlive: 30 * time.Second, Control: Control}
		sharedTransport = &http.Transport{
			// Never honour HTTP_PROXY/HTTPS_PROXY: a proxy would do the DNS
			// lookup and connect for us, skipping the Control check below.
			Proxy:                 nil,
			DialContext:           dialer.DialContext,
			ForceAttemptHTTP2:     true,
			TLSHandshakeTimeout:   10 * time.Second,
			ResponseHeaderTimeout: 60 * time.Second, // headers only; SSE bodies may stream longer
			IdleConnTimeout:       90 * time.Second,
			MaxIdleConns:          100,
			MaxIdleConnsPerHost:   10,
		}
	})
	return sharedTransport
}

// Control runs after DNS resolution, just before each connect, so it sees the
// real IP (covering DNS rebinding and redirects). Only public addresses are
// allowed; loopback is also allowed when APP_ENV is dev/local, so a local
// Ollama or LM Studio works there, never in production.
func Control(_, address string, _ syscall.RawConn) error {
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		return fmt.Errorf("invalid destination")
	}
	ip, err := netip.ParseAddr(host)
	if err != nil {
		return fmt.Errorf("invalid destination")
	}
	if isPublic(ip) || (isDevEnv() && ip.Unmap().IsLoopback()) {
		return nil
	}
	return fmt.Errorf("destination address is not public")
}

// checkRedirect caps redirects and refuses credentials or an http hop outside
// dev/local. The destination address itself is checked by Control at dial.
func checkRedirect(req *http.Request, via []*http.Request) error {
	if len(via) >= 5 {
		return fmt.Errorf("too many redirects")
	}
	if req.URL.User != nil || req.URL.Scheme != "https" && !(req.URL.Scheme == "http" && isDevEnv()) {
		return fmt.Errorf("invalid redirect destination")
	}
	return nil
}

func isPublic(ip netip.Addr) bool {
	if !ip.IsValid() {
		return false
	}
	ip = ip.Unmap()
	if !ip.Is4() && !netip.MustParsePrefix("2000::/3").Contains(ip) {
		return false
	}
	if !ip.IsGlobalUnicast() || ip.IsPrivate() || ip.IsLoopback() || ip.IsLinkLocalUnicast() || ip.IsMulticast() || ip.IsUnspecified() {
		return false
	}
	blocked := []string{"0.0.0.0/8", "100.64.0.0/10", "192.0.0.0/24", "192.0.2.0/24", "192.88.99.0/24", "198.18.0.0/15", "198.51.100.0/24", "203.0.113.0/24", "240.0.0.0/4", "2001:db8::/32", "fc00::/7", "fe80::/10", "64:ff9b::/96", "64:ff9b:1::/48", "2002::/16", "2001::/32", "::/96", "::ffff:0:0:0/96", "fec0::/10", "100::/64"}
	for _, cidr := range blocked {
		if netip.MustParsePrefix(cidr).Contains(ip) {
			return false
		}
	}
	return true
}
