package api

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"testing"

	"server/internal/identity/components"
	"server/internal/identity/model"
)

// brokenUsers fails every lookup the way a database outage does.
type brokenUsers struct{ model.UserRepository }

func (brokenUsers) GetByEmail(context.Context, string) (*model.User, error) {
	return nil, errors.New("connection refused")
}

// An address with no account gets the same reply as one with an account,
// so the form can't be used to find out who has one. The inbox gets a link
// that creates the account.
func TestMagicLinkUnknownUserLooksLikeKnownUser(t *testing.T) {
	h, store, mail := newMagicHandlers(unknownUsers{})
	w := serve(h, magicRequest("nobody@example.com", "203.0.113.1:1", nil))
	if w.Code != http.StatusOK || !strings.Contains(w.Body.String(), `"success":true`) {
		t.Fatalf("unknown user: got %d %q, want 200 success", w.Code, w.Body.String())
	}
	if len(mail.to) != 1 || len(store.registrations) != 1 || !store.registrations[0] {
		t.Fatalf("want one sign-up link sent, got mail %v registrations %v", mail.to, store.registrations)
	}
}

// A lookup failure must not tell a registered user they have no account
// (the portal would switch them to sign-up): it is a server error.
func TestMagicLinkLookupFailureIs500(t *testing.T) {
	h := NewAuthHandlers(components.NewSendMagicLinkUseCase(brokenUsers{}, nil, nil, nil), nil, nil, nil, nil, nil)
	w := serve(h, magicRequest("someone@example.com", "203.0.113.1:1", nil))
	if w.Code != http.StatusInternalServerError {
		t.Fatalf("lookup failure: got %d, want 500 (body %q)", w.Code, w.Body.String())
	}
}
