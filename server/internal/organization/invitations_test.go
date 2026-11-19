package organization

import (
	"testing"
	"time"
)

func TestInvitationIsExpired(t *testing.T) {
	cases := []struct {
		name      string
		expiresAt time.Time
		want      bool
	}{
		{"in the future", time.Now().Add(time.Hour), false},
		{"in the past", time.Now().Add(-time.Hour), true},
		{"exactly now, a moment ago by the time it's checked", time.Now().Add(-time.Millisecond), true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			inv := Invitation{ExpiresAt: c.expiresAt}
			if got := inv.IsExpired(); got != c.want {
				t.Errorf("IsExpired() with ExpiresAt %v = %v, want %v", c.expiresAt, got, c.want)
			}
		})
	}
}
