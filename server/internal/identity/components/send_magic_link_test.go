package components

import (
	"context"
	"database/sql"
	"errors"
	"testing"

	"server/internal/identity/model"
)

type lookupUsers struct {
	model.UserRepository
	err error
}

func (u lookupUsers) GetByEmail(context.Context, string) (*model.User, error) {
	return nil, u.err
}

type recordMagic struct {
	model.TokenStore
	registration bool
}

func (s *recordMagic) SaveMagicToken(_ context.Context, _, _, _, _, _ string, isRegistration bool) error {
	s.registration = isRegistration
	return nil
}

type noMail struct{}

func (noMail) SendMagicLink(context.Context, string, string, string) error { return nil }

// A missing account gets a sign-up link, not an error the caller could tell
// apart; an outage is still an error.
func TestSendMagicLinkMissSendsSignUpOutageFails(t *testing.T) {
	cmd := SendMagicLinkCommand{Email: "a@example.com", StateID: "s"}

	store := &recordMagic{}
	err := NewSendMagicLinkUseCase(lookupUsers{err: sql.ErrNoRows}, store, noMail{}, model.NewTokenService("k")).Execute(context.Background(), cmd)
	if err != nil || !store.registration {
		t.Fatalf("no rows: got err %v registration %v, want a sign-up link", err, store.registration)
	}

	outage := errors.New("dial tcp: connection refused")
	err = NewSendMagicLinkUseCase(lookupUsers{err: outage}, nil, nil, nil).Execute(context.Background(), cmd)
	if err == nil || errors.Is(err, ErrUserNotFound) {
		t.Fatalf("db outage: got %v, want a non-not-found error", err)
	}
	if !errors.Is(err, outage) {
		t.Fatalf("db outage: cause not wrapped: %v", err)
	}
}
