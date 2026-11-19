package model

import "testing"

func TestVerifiedGitHubEmailNeverReturnsAnUnverifiedAddress(t *testing.T) {
	cases := []struct {
		name    string
		profile string
		emails  []gitHubEmail
		want    string
		wantErr bool
	}{
		{"verified profile address", "a@x.com", []gitHubEmail{{Email: "a@x.com", Verified: true}}, "a@x.com", false},
		{"unverified profile address is not trusted", "victim@x.com", []gitHubEmail{{Email: "victim@x.com"}, {Email: "me@y.com", Primary: true, Verified: true}}, "me@y.com", false},
		{"profile address matched case-insensitively", "A@X.com", []gitHubEmail{{Email: "a@x.com", Verified: true}}, "A@X.com", false},
		{"verified primary when no profile address", "", []gitHubEmail{{Email: "b@x.com", Verified: true}, {Email: "p@x.com", Primary: true, Verified: true}}, "p@x.com", false},
		{"any verified when the primary isn't", "", []gitHubEmail{{Email: "p@x.com", Primary: true}, {Email: "v@x.com", Verified: true}}, "v@x.com", false},
		{"nothing verified", "victim@x.com", []gitHubEmail{{Email: "victim@x.com", Primary: true}}, "", true},
		{"no addresses", "", nil, "", true},
	}
	for _, c := range cases {
		got, err := verifiedGitHubEmail(c.profile, c.emails)
		if (err != nil) != c.wantErr || got != c.want {
			t.Errorf("%s: got %q, %v; want %q, error %v", c.name, got, err, c.want, c.wantErr)
		}
	}
}
