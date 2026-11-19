package api

import (
	"context"
	"errors"
	"net/http"
	"testing"

	"server/internal/identity/components"
	"server/internal/identity/model"
)

// brokenUsers fails every lookup the way a database outage does.
type brokenUsers struct{ model.UserRepository }

func (brokenUsers) GetByEmail(context.Context, string) (*model.User, error) {
	return nil, errors.New("connection refused")
}

func TestMagicLinkUnknownUserIs404(t *testing.T) {
	h := NewAuthHandlers(components.NewSendMagicLinkUseCase(unknownUsers{}, nil, nil, nil), nil, nil, nil, nil, nil)
	w := serve(h, magicRequest("nobody@example.com", "203.0.113.1:1", nil))
	if w.Code != http.StatusNotFound {
		t.Fatalf("unknown user: got %d, want 404", w.Code)
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
