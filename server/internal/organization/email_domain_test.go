package organization

import "testing"

func TestEmailDomain(t *testing.T) {
	cases := map[string]string{
		"person@Acme.COM":  "acme.com",
		"person@sub.co.uk": "sub.co.uk",
		"noatsign":         "",
		"trailing@":        "",
		"@leading.com":     "leading.com",
		"":                 "",
	}
	for email, want := range cases {
		if got := emailDomain(email); got != want {
			t.Errorf("emailDomain(%q) = %q, want %q", email, got, want)
		}
	}
}
