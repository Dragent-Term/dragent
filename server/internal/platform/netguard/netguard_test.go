package netguard

import (
	"net/http"
	"net/http/httptest"
	"net/netip"
	"net/url"
	"strings"
	"testing"
	"time"
)

func TestValidateURL(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	for _, raw := range []string{
		"http://example.com",
		"http://localhost:11434/v1",
		"https://user:pass@example.com",
		"https://example.com?token=x",
		"https://example.com#frag",
		"file:///etc/passwd",
		"gopher://example.com",
		"https://",
		"not a url",
	} {
		if err := ValidateURL(raw); err == nil {
			t.Errorf("production: ValidateURL(%q) accepted", raw)
		}
	}
	if err := ValidateURL("https://user:secret@example.com"); err != nil && strings.Contains(err.Error(), "secret") {
		t.Errorf("error leaks credentials: %v", err)
	}
	if err := ValidateURL("https://example.com/v1"); err != nil {
		t.Fatalf("https URL rejected: %v", err)
	}
	for _, env := range []string{"local", "dev"} {
		t.Setenv("APP_ENV", env)
		if err := ValidateURL("http://localhost:11434/v1"); err != nil {
			t.Errorf("%s: http rejected: %v", env, err)
		}
	}
}

func TestIsPublic(t *testing.T) {
	blocked := []string{
		"127.0.0.1", "169.254.169.254", "10.0.0.1", "172.16.0.1", "192.168.1.1", "100.64.0.1",
		"0.0.0.0", "0.1.2.3", "192.0.0.1", "192.88.99.1", "198.18.0.1", "240.0.0.1", "255.255.255.255", "224.0.0.1",
		"::", "::1", "fc00::1", "fd00::1", "fe80::1", "fec0::1", "ff02::1", "100::1", "2001:db8::1",
		"::ffff:127.0.0.1", "::ffff:169.254.169.254", "::ffff:10.0.0.1", // IPv4-mapped
		"::127.0.0.1", "::a9fe:a9fe", // IPv4-compatible
		"::ffff:0:0:1",                                          // SIIT
		"64:ff9b::a9fe:a9fe", "64:ff9b::7f00:1", "64:ff9b:1::1", // NAT64
		"2002:a9fe:a9fe::1", "2002:7f00:1::1", // 6to4
		"2001::1", "2001:0:4136:e378:8000:63bf:3fff:fdd2", // Teredo
	}
	for _, raw := range blocked {
		if isPublic(netip.MustParseAddr(raw)) {
			t.Errorf("%s accepted as public", raw)
		}
	}
	for _, raw := range []string{"8.8.8.8", "1.1.1.1", "2606:4700:4700::1111", "2a00:1450:4001::1"} {
		if !isPublic(netip.MustParseAddr(raw)) {
			t.Errorf("%s rejected as public", raw)
		}
	}
}

func TestControl(t *testing.T) {
	cases := []struct {
		addr      string
		prod, dev bool // allowed?
	}{
		{"8.8.8.8:443", true, true},
		{"[2606:4700:4700::1111]:443", true, true},
		{"127.0.0.1:11434", false, true},
		{"127.1.2.3:80", false, true},
		{"[::1]:1234", false, true},
		{"[::ffff:127.0.0.1]:80", false, true},
		{"169.254.169.254:80", false, false},
		{"10.0.0.5:80", false, false},
		{"192.168.0.1:80", false, false},
		{"0.0.0.0:80", false, false},
		{"[64:ff9b::a9fe:a9fe]:80", false, false},
		{"[2002:a9fe:a9fe::1]:80", false, false},
		{"[::ffff:169.254.169.254]:80", false, false},
		{"[fe80::1]:80", false, false},
		{"[fd00::1]:80", false, false},
		{"localhost:80", false, false}, // Control only ever sees resolved IPs
		{"garbage", false, false},
	}
	for _, env := range []string{"production", "local"} {
		t.Setenv("APP_ENV", env)
		for _, c := range cases {
			want := c.prod
			if env == "local" {
				want = c.dev
			}
			if got := Control("tcp", c.addr, nil) == nil; got != want {
				t.Errorf("APP_ENV=%s Control(%s) allowed=%v, want %v", env, c.addr, got, want)
			}
		}
	}
}

func TestCheckRedirect(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	from := []*http.Request{{URL: mustURL("https://api.example.com/v1")}}
	if err := checkRedirect(&http.Request{URL: mustURL("https://api2.example.com/v1?page=2")}, from); err != nil {
		t.Errorf("https hop with query refused: %v", err)
	}
	// A private literal passes the redirect check; the dial Control refuses it.
	if err := checkRedirect(&http.Request{URL: mustURL("https://169.254.169.254/latest")}, from); err != nil {
		t.Errorf("https hop refused before dial: %v", err)
	}
	if Control("tcp", "169.254.169.254:443", nil) == nil {
		t.Error("dial to redirect target 169.254.169.254 allowed")
	}
	if err := checkRedirect(&http.Request{URL: mustURL("http://example.com/")}, from); err == nil {
		t.Error("https->http downgrade allowed in production")
	}
	if err := checkRedirect(&http.Request{URL: mustURL("https://u:p@example.com/")}, from); err == nil {
		t.Error("redirect with credentials allowed")
	}
	if err := checkRedirect(&http.Request{URL: mustURL("https://example.com/")}, make([]*http.Request, 4)); err != nil {
		t.Errorf("5th hop refused: %v", err)
	}
	if err := checkRedirect(&http.Request{URL: mustURL("https://example.com/")}, make([]*http.Request, 5)); err == nil {
		t.Error("6th hop allowed")
	}
}

// In dev mode the loopback httptest server is reachable, so a real redirect
// from it to a private address must fail at dial time.
func TestClientRedirectToPrivateFailsAtDial(t *testing.T) {
	t.Setenv("APP_ENV", "local")
	for _, target := range []string{"http://169.254.169.254/latest/meta-data/", "http://10.255.255.1/", "http://[64:ff9b::a9fe:a9fe]/"} {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			http.Redirect(w, r, target, http.StatusFound)
		}))
		client := NewClient(5 * time.Second)
		resp, err := client.Get(srv.URL)
		srv.Close()
		if err == nil {
			resp.Body.Close()
			t.Fatalf("redirect to %s followed", target)
		}
		if !strings.Contains(err.Error(), "not public") {
			t.Errorf("redirect to %s: want dial refusal, got %v", target, err)
		}
	}
}

func TestClientRefusesLoopbackInProduction(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	defer srv.Close()
	t.Setenv("APP_ENV", "production")
	if resp, err := NewClient(5 * time.Second).Get(srv.URL); err == nil {
		resp.Body.Close()
		t.Fatal("production client reached loopback")
	} else if !strings.Contains(err.Error(), "not public") {
		t.Errorf("want dial refusal, got %v", err)
	}
	t.Setenv("APP_ENV", "local")
	resp, err := NewClient(5 * time.Second).Get(srv.URL)
	if err != nil {
		t.Fatalf("dev client could not reach loopback: %v", err)
	}
	resp.Body.Close()
}

func TestTransportIsSharedAndIgnoresProxy(t *testing.T) {
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:9")
	t.Setenv("HTTP_PROXY", "http://127.0.0.1:9")
	a, b := NewClient(time.Second), NewClient(2*time.Second)
	if a.Transport != b.Transport {
		t.Fatal("NewClient builds a new Transport per call")
	}
	tr := SharedTransport()
	if tr.Proxy != nil {
		t.Fatal("guarded transport honours env proxies")
	}
	if tr.TLSHandshakeTimeout == 0 || tr.ResponseHeaderTimeout == 0 || tr.IdleConnTimeout == 0 {
		t.Fatal("guarded transport is missing timeouts")
	}
}

func mustURL(raw string) *url.URL {
	u, err := url.Parse(raw)
	if err != nil {
		panic(err)
	}
	return u
}
