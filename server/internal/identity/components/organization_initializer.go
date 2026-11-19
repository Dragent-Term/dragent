package components

import (
	"context"

	"github.com/google/uuid"
)

// OrganizationInitializer creates a personal organization and owner
// membership for a newly registered user. Declared here as a small
// interface, rather than importing the organization package, so identity
// does not take a dependency on organization internals — the concrete
// organization.PostgresRepository already satisfies it. Mirrors
// BalanceInitializer's dependency-inversion shape in authenticate_oauth.go.
//
// Every account gets an organization; this is not opt-in. A user without one has nowhere for org-scoped features to attach.
//
// Returns the organization id the user landed in (auto-join target or new
// personal org) — the caller needs it to initialize that user's per-org
// balance row in the same transaction, since organization_id is NOT NULL
// there.
type OrganizationInitializer interface {
	InitializeTx(ctx context.Context, tx any, userID uuid.UUID, displayName, email string) (uuid.UUID, error)
}
