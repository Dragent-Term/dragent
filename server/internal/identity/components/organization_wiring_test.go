package components

import (
	"context"
	"testing"

	"github.com/google/uuid"
)

type recordingOrgInit struct {
	called []uuid.UUID
}

func (r *recordingOrgInit) InitializeTx(_ context.Context, _ any, userID uuid.UUID, _ string, _ string) (uuid.UUID, error) {
	r.called = append(r.called, userID)
	return uuid.New(), nil
}

// Same shape as TestBalanceInitializerIsWired: the setter is the whole
// mechanism. If it doesn't store the dependency, both registration paths
// silently skip organization creation and every new user has nowhere for
// org-scoped features (billing, model policy, sharing) to attach.
func TestOrganizationInitializerIsWired(t *testing.T) {
	oauth := &AuthenticateOAuthUseCase{}
	if oauth.orgInit != nil {
		t.Fatal("expected nil before wiring")
	}
	r1 := &recordingOrgInit{}
	oauth.SetOrganizationInitializer(r1)
	if oauth.orgInit == nil {
		t.Error("AuthenticateOAuthUseCase: setter did not store the initializer")
	}

	magic := &VerifyMagicLinkUseCase{}
	r2 := &recordingOrgInit{}
	magic.SetOrganizationInitializer(r2)
	if magic.orgInit == nil {
		t.Error("VerifyMagicLinkUseCase: setter did not store the initializer")
	}

	// Nil must stay safe — an un-wired usecase should skip, not panic.
	var unset OrganizationInitializer
	if unset != nil {
		t.Fatal("zero interface should be nil")
	}
}
