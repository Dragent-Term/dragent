package ratelimit

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestClientIPTrustsOnlyOurProxyAndCloudflare(t *testing.T) {
	cases := []struct {
		name    string
		remote  string
		headers map[string][]string
		want    string
	}{
		{"direct peer", "198.51.100.7:4000", nil, "198.51.100.7"},
		{"direct peer ignores spoofed headers", "198.51.100.7:4000", map[string][]string{
			"X-Real-Ip": {"1.1.1.1"}, "True-Client-Ip": {"2.2.2.2"},
			"X-Forwarded-For": {"3.3.3.3"}, "Cf-Connecting-Ip": {"4.4.4.4"},
		}, "198.51.100.7"},
		{"caddy: last forwarded entry is the peer it saw", "127.0.0.1:5000", map[string][]string{
			"X-Forwarded-For": {"6.6.6.6, 203.0.113.9"}, "X-Real-Ip": {"1.1.1.1"},
		}, "203.0.113.9"},
		{"caddy: several header lines, last one wins", "127.0.0.1:5000", map[string][]string{
			"X-Forwarded-For": {"6.6.6.6", "203.0.113.9"},
		}, "203.0.113.9"},
		{"caddy: client CF header ignored when peer isn't Cloudflare", "127.0.0.1:5000", map[string][]string{
			"X-Forwarded-For": {"203.0.113.9"}, "Cf-Connecting-Ip": {"4.4.4.4"},
		}, "203.0.113.9"},
		{"cloudflare through caddy: CF-Connecting-IP is the visitor", "127.0.0.1:5000", map[string][]string{
			"X-Forwarded-For": {"162.158.1.2"}, "Cf-Connecting-Ip": {"203.0.113.50"},
		}, "203.0.113.50"},
		{"cloudflare direct, IPv6 edge", "[2606:4700::1]:443", map[string][]string{
			"Cf-Connecting-Ip": {"2001:db8::5"},
		}, "2001:db8::5"},
		{"cloudflare peer with junk CF header keeps the peer", "127.0.0.1:5000", map[string][]string{
			"X-Forwarded-For": {"162.158.1.2"}, "Cf-Connecting-Ip": {"not-an-ip"},
		}, "162.158.1.2"},
		{"loopback with no forwarded header (local dev)", "127.0.0.1:5000", nil, "127.0.0.1"},
		{"loopback with junk forwarded entry keeps loopback", "127.0.0.1:5000", map[string][]string{
			"X-Forwarded-For": {"garbage"},
		}, "127.0.0.1"},
		{"mapped IPv4 peer", "[::ffff:198.51.100.7]:1", nil, "198.51.100.7"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := httptest.NewRequest(http.MethodGet, "/", nil)
			r.RemoteAddr = tc.remote
			for k, vs := range tc.headers {
				for _, v := range vs {
					r.Header.Add(k, v)
				}
			}
			if got := ClientIP(r); got != tc.want {
				t.Fatalf("ClientIP = %q, want %q", got, tc.want)
			}
		})
	}
}

// Rotating client-set headers must not produce new addresses: this is the
// bypass that let one caller dodge the per-visitor sign-in limit.
func TestClientIPRotatingSpoofedHeadersStaySame(t *testing.T) {
	seen := map[string]bool{}
	for _, h := range []string{"X-Real-Ip", "True-Client-Ip", "Cf-Connecting-Ip"} {
		for i := 0; i < 5; i++ {
			r := httptest.NewRequest(http.MethodGet, "/", nil)
			r.RemoteAddr = "127.0.0.1:5000"
			r.Header.Set("X-Forwarded-For", "10.9.8.7, 203.0.113.9") // Caddy wrote the last entry
			r.Header.Set(h, "198.51.100."+string(rune('1'+i)))
			seen[ClientIP(r)] = true
		}
	}
	if len(seen) != 1 || !seen["203.0.113.9"] {
		t.Fatalf("spoofed headers changed the address: %v", seen)
	}
}

func TestRealIPMiddlewareRewritesRemoteAddr(t *testing.T) {
	var got string
	h := RealIP(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got = r.RemoteAddr
		if again := ClientIP(r); again != "203.0.113.9" {
			t.Errorf("ClientIP after RealIP = %q", again)
		}
	}))
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.RemoteAddr = "127.0.0.1:5000"
	r.Header.Set("X-Forwarded-For", "203.0.113.9")
	r.Header.Set("X-Real-Ip", "1.1.1.1")
	h.ServeHTTP(httptest.NewRecorder(), r)
	if got != "203.0.113.9:5000" {
		t.Fatalf("RemoteAddr = %q", got)
	}
}
