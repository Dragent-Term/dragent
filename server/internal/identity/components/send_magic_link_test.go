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

func TestSendMagicLinkOnlyRealMissIsUserNotFound(t *testing.T) {
	cmd := SendMagicLinkCommand{Email: "a@example.com", StateID: "s"}

	err := NewSendMagicLinkUseCase(lookupUsers{err: sql.ErrNoRows}, nil, nil, nil).Execute(context.Background(), cmd)
	if !errors.Is(err, ErrUserNotFound) {
		t.Fatalf("no rows: got %v, want ErrUserNotFound", err)
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
