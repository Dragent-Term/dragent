package ratelimit

import (
	"net"
	"net/http"
	"net/netip"
	"strings"
)

// cloudflareRanges are Cloudflare's edge addresses (https://www.cloudflare.com/ips/).
// Only a request whose peer is one of these can carry a CF-Connecting-IP we
// believe: Cloudflare overwrites that header, but anyone reaching the origin
// directly can set it.
var cloudflareRanges = mustPrefixes(
	"173.245.48.0/20", "103.21.244.0/22", "103.22.200.0/22", "103.31.4.0/22",
	"141.101.64.0/18", "108.162.192.0/18", "190.93.240.0/20", "188.114.96.0/20",
	"197.234.240.0/22", "198.41.128.0/17", "162.158.0.0/15", "104.16.0.0/13",
	"104.24.0.0/14", "172.64.0.0/13", "131.0.72.0/22",
	"2400:cb00::/32", "2606:4700::/32", "2803:f800::/32", "2405:b500::/32",
	"2405:8100::/32", "2a06:98c0::/29", "2c0f:f248::/32",
)

func mustPrefixes(cidrs ...string) []netip.Prefix {
	out := make([]netip.Prefix, len(cidrs))
	for i, c := range cidrs {
		out[i] = netip.MustParsePrefix(c)
	}
	return out
}

func isCloudflare(ip netip.Addr) bool {
	ip = ip.Unmap()
	for _, p := range cloudflareRanges {
		if p.Contains(ip) {
			return true
		}
	}
	return false
}

func parseAddr(s string) (netip.Addr, bool) {
	s = strings.TrimSpace(s)
	if host, _, err := net.SplitHostPort(s); err == nil {
		s = host
	}
	ip, err := netip.ParseAddr(strings.Trim(s, "[]"))
	if err != nil {
		return netip.Addr{}, false
	}
	return ip.Unmap(), true
}

// ClientIP is the address to count a request against. It only believes what
// a proxy we run, or Cloudflare, wrote; never a header the caller can set.
//
//   - The peer is RemoteAddr.
//   - A loopback peer is Caddy on the same host (it runs with host networking
//     and no trusted_proxies), which replaces any client-sent X-Forwarded-For
//     with the peer it saw: its last entry is that peer.
//   - A peer on Cloudflare's edge is Cloudflare, whose CF-Connecting-IP is the
//     visitor.
//
// X-Real-IP, True-Client-IP and earlier X-Forwarded-For entries are ignored.
func ClientIP(r *http.Request) string {
	peer, ok := parseAddr(r.RemoteAddr)
	if !ok {
		return strings.TrimSpace(r.RemoteAddr)
	}
	if peer.IsLoopback() {
		if fwd := r.Header.Values("X-Forwarded-For"); len(fwd) > 0 {
			entries := strings.Split(fwd[len(fwd)-1], ",")
			if ip, ok := parseAddr(entries[len(entries)-1]); ok {
				peer = ip
			}
		}
	}
	if isCloudflare(peer) {
		if ip, ok := parseAddr(r.Header.Get("CF-Connecting-IP")); ok {
			return ip.String()
		}
	}
	return peer.String()
}

// RealIP replaces chi's middleware.RealIP, which believes X-Real-IP,
// True-Client-IP and X-Forwarded-For from anyone: it sets RemoteAddr to
// ClientIP so every later reader of RemoteAddr sees the trusted address.
func RealIP(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ip := ClientIP(r)
		port := "0"
		if _, p, err := net.SplitHostPort(r.RemoteAddr); err == nil {
			port = p
		}
		r.RemoteAddr = net.JoinHostPort(ip, port)
		next.ServeHTTP(w, r)
	})
}
