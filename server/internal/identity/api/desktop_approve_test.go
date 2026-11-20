package api

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"server/internal/identity/model"
)

// parkedStates is an in-memory stand-in for the poll:* keys.
type parkedStates struct {
	model.TokenStore
	user     map[string]string
	approved map[string]bool
}

func newParkedStates() *parkedStates {
	return &parkedStates{user: map[string]string{}, approved: map[string]bool{}}
}

func (p *parkedStates) AuthorizeState(_ context.Context, state, userID, _, _ string) error {
	p.user[state] = userID
	return nil
}

func (p *parkedStates) ApproveState(_ context.Context, state, userID string) error {
	owner, ok := p.user[state]
	if !ok {
		return errors.New("expired")
	}
	if owner != userID {
		return model.ErrStateWrongUser
	}
	p.approved[state] = true
	return nil
}

func (p *parkedStates) GetAuthorizedState(_ context.Context, state string) (string, string, error) {
	if _, ok := p.user[state]; !ok {
		return "", "", errors.New("expired")
	}
	if !p.approved[state] {
		return "", "", model.ErrStateNotApproved
	}
	return "access", "refresh", nil
}

func poll(h *AuthHandlers, state string) int {
	w := httptest.NewRecorder()
	h.PollDesktopSession(w, httptest.NewRequest(http.MethodGet, "/auth/desktop/session?state="+state, nil))
	return w.Code
}

func approve(h *AuthHandlers, state, asUser string) int {
	r := httptest.NewRequest(http.MethodPost, "/auth/desktop/approve", strings.NewReader(`{"state":"`+state+`"}`))
	if asUser != "" {
		r = r.WithContext(context.WithValue(r.Context(), "user_id", asUser))
	}
	w := httptest.NewRecorder()
	h.ApproveDesktopSession(w, r)
	return w.Code
}

// Knowing a sign-in's state isn't enough to collect it: whoever started it
// chose the state. It's handed out only after the signed-in user approves.
func TestDesktopSessionNeedsApprovalByTheSignedInUser(t *testing.T) {
	store := newParkedStates()
	h := NewAuthHandlers(nil, nil, nil, store, nil, nil)
	_ = store.AuthorizeState(context.Background(), "s1", "victim", "a", "r")

	if got := poll(h, "s1"); got != http.StatusNoContent {
		t.Fatalf("before approval: poll %d, want 204", got)
	}
	if got := approve(h, "s1", ""); got != http.StatusUnauthorized {
		t.Fatalf("anonymous approval: %d, want 401", got)
	}
	if got := approve(h, "s1", "attacker"); got != http.StatusNotFound {
		t.Fatalf("another user's approval: %d, want 404", got)
	}
	if got := poll(h, "s1"); got != http.StatusNoContent {
		t.Fatalf("after a refused approval: poll %d, want 204", got)
	}
	if got := approve(h, "s1", "victim"); got != http.StatusOK {
		t.Fatalf("owner's approval: %d, want 200", got)
	}
	if got := poll(h, "s1"); got != http.StatusOK {
		t.Fatalf("after approval: poll %d, want 200", got)
	}
}
