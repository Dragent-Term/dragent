package organization

import "testing"

func TestIsPublicEmailDomain(t *testing.T) {
	cases := map[string]bool{
		"gmail.com":      true,
		"yahoo.com":      true,
		"outlook.com":    true,
		"icloud.com":     true,
		"protonmail.com": true,
		"acme.com":       false,
		"anthropic.com":  false,
		"":               false,
	}
	for domain, want := range cases {
		if got := isPublicEmailDomain(domain); got != want {
			t.Errorf("isPublicEmailDomain(%q) = %v, want %v", domain, got, want)
		}
	}
}
