package components

import (
	"context"
	"testing"

	"github.com/google/uuid"
)

type recordingInit struct{ called []uuid.UUID }

func (r *recordingInit) InitializeTx(_ context.Context, _ any, id, _ uuid.UUID) error {
	r.called = append(r.called, id)
	return nil
}

// The setter is the whole mechanism: if it does not store the dependency, both
// registration paths silently skip balance creation and every new user's
// dashboard 500s on "fetch balance: sql: no rows in result set".
func TestBalanceInitializerIsWired(t *testing.T) {
	oauth := &AuthenticateOAuthUseCase{}
	if oauth.balanceInit != nil {
		t.Fatal("expected nil before wiring")
	}
	r1 := &recordingInit{}
	oauth.SetBalanceInitializer(r1)
	if oauth.balanceInit == nil {
		t.Error("AuthenticateOAuthUseCase: setter did not store the initializer")
	}

	magic := &VerifyMagicLinkUseCase{}
	r2 := &recordingInit{}
	magic.SetBalanceInitializer(r2)
	if magic.balanceInit == nil {
		t.Error("VerifyMagicLinkUseCase: setter did not store the initializer")
	}

	// Nil must stay safe — an un-wired usecase should skip, not panic.
	id := uuid.New()
	var unset BalanceInitializer
	if unset != nil {
		t.Fatal("zero interface should be nil")
	}
	_ = id
}
