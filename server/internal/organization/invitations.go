package organization

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

type InvitationStatus string

const (
	InvitationPending  InvitationStatus = "pending"
	InvitationAccepted InvitationStatus = "accepted"
	InvitationRejected InvitationStatus = "rejected"
	InvitationCanceled InvitationStatus = "canceled"
)

// invitationExpiry matches the reference product's own configured window
// (packages/auth/src/server.ts: invitationExpiresIn: 60*60*24*7) — a
// product decision, not copyrightable expression, fine to match exactly
// per the plan doc §3.1.
const invitationExpiry = 7 * 24 * time.Hour

// Invitation is a pending offer to join an organization at a given role.
// Unlike the earlier immediate-add model this replaces, the target does not
// need an account yet and nothing joins until they explicitly accept.
type Invitation struct {
	ID             uuid.UUID
	OrganizationID uuid.UUID
	Email          string
	Role           MembershipRole
	Status         InvitationStatus
	InviterID      uuid.UUID
	ExpiresAt      time.Time
	CreatedAt      time.Time
}

// IsExpired is computed, not a stored status. dragent has no scheduler yet
// (CLOUD_FLEET_AND_CROSS_AGENT_MESH_PLAN.md Phase 11.D, not started), so
// there is nothing to sweep "pending" rows into an "expired" status on a
// timer — every action that would operate on an invitation checks this
// instead, at the moment it matters.
func (i Invitation) IsExpired() bool {
	return time.Now().After(i.ExpiresAt)
}

var (
	// ErrInvitationPending is returned when a pending invitation already
	// exists for this (organization, email) — see the partial unique index
	// in 00050_invitations.sql.
	ErrInvitationPending = errors.New("an invitation to this email is already pending")
	// ErrInvitationNotPending is returned when accepting, rejecting, or
	// canceling an invitation that isn't (or is no longer) pending.
	ErrInvitationNotPending = errors.New("invitation is no longer pending")
	// ErrInvitationExpired is returned when acting on a pending invitation
	// whose window (invitationExpiry) has passed.
	ErrInvitationExpired = errors.New("invitation has expired")
	// ErrInvitationEmailMismatch is returned when the account accepting or
	// rejecting an invitation doesn't own the email it was sent to.
	ErrInvitationEmailMismatch = errors.New("this invitation was not sent to your account's email")
)

// CreateInvitation records a pending invitation. Returns ErrInvitationPending
// if one is already open for this (organization, email), or ErrOrgNotPaid if
// the organization is still on Free — inviting anyone is a team-collaboration
// action, gated to paid orgs (see requireOrgPaid).
func (r *PostgresRepository) CreateInvitation(ctx context.Context, orgID uuid.UUID, email string, role MembershipRole, inviterID uuid.UUID) (Invitation, error) {
	if err := r.requireOrgPaid(ctx, orgID); err != nil {
		return Invitation{}, err
	}
	now := time.Now()
	inv := Invitation{
		ID:             uuid.New(),
		OrganizationID: orgID,
		Email:          email,
		Role:           role,
		Status:         InvitationPending,
		InviterID:      inviterID,
		ExpiresAt:      now.Add(invitationExpiry),
		CreatedAt:      now,
	}
	_, err := r.pool.Exec(ctx, `
		INSERT INTO invitations (id, organization_id, email, role, status, inviter_id, expires_at, created_at, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
	`, inv.ID, inv.OrganizationID, inv.Email, inv.Role, inv.Status, inv.InviterID, inv.ExpiresAt, inv.CreatedAt)
	if err != nil {
		if isUniqueViolation(err) {
			return Invitation{}, ErrInvitationPending
		}
		return Invitation{}, fmt.Errorf("create invitation: %w", err)
	}
	return inv, nil
}

// GetInvitation fetches an invitation by id regardless of organization —
// the accepting/rejecting user isn't a member of the organization it
// belongs to (that's the whole point), so this can't be scoped the way
// GetDomain is. Returns sql.ErrNoRows if it doesn't exist.
func (r *PostgresRepository) GetInvitation(ctx context.Context, invitationID uuid.UUID) (Invitation, error) {
	var inv Invitation
	err := r.pool.QueryRow(ctx, `
		SELECT id, organization_id, email, role, status, inviter_id, expires_at, created_at
		FROM invitations WHERE id = $1
	`, invitationID).Scan(&inv.ID, &inv.OrganizationID, &inv.Email, &inv.Role, &inv.Status, &inv.InviterID, &inv.ExpiresAt, &inv.CreatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Invitation{}, sql.ErrNoRows
		}
		return Invitation{}, err
	}
	return inv, nil
}

// ListInvitations returns every invitation an organization has ever sent
// (all statuses), newest first — the admin-facing view.
func (r *PostgresRepository) ListInvitations(ctx context.Context, orgID uuid.UUID) ([]Invitation, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT id, organization_id, email, role, status, inviter_id, expires_at, created_at
		FROM invitations WHERE organization_id = $1
		ORDER BY created_at DESC
	`, orgID)
	if err != nil {
		return nil, fmt.Errorf("list invitations: %w", err)
	}
	defer rows.Close()
	return scanInvitations(rows)
}

// ListInvitationsForEmail returns the still-pending invitations addressed
// to an email — "invitations waiting for me to act on," the caller's-own
// view. Deliberately excludes accepted/rejected/canceled ones; nothing
// currently needs that history from this side.
func (r *PostgresRepository) ListInvitationsForEmail(ctx context.Context, email string) ([]Invitation, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT id, organization_id, email, role, status, inviter_id, expires_at, created_at
		FROM invitations WHERE email = $1 AND status = 'pending'
		ORDER BY created_at DESC
	`, email)
	if err != nil {
		return nil, fmt.Errorf("list invitations for email: %w", err)
	}
	defer rows.Close()
	return scanInvitations(rows)
}

func scanInvitations(rows pgx.Rows) ([]Invitation, error) {
	var invs []Invitation
	for rows.Next() {
		var inv Invitation
		if err := rows.Scan(&inv.ID, &inv.OrganizationID, &inv.Email, &inv.Role, &inv.Status, &inv.InviterID, &inv.ExpiresAt, &inv.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan invitation: %w", err)
		}
		invs = append(invs, inv)
	}
	return invs, rows.Err()
}

// CancelInvitation withdraws a pending invitation. Admin-side action, scoped
// to the organization so one org can't cancel another's invitation by id.
func (r *PostgresRepository) CancelInvitation(ctx context.Context, orgID, invitationID uuid.UUID) error {
	tag, err := r.pool.Exec(ctx, `
		UPDATE invitations SET status = 'canceled', updated_at = NOW()
		WHERE id = $1 AND organization_id = $2 AND status = 'pending'
	`, invitationID, orgID)
	if err != nil {
		return fmt.Errorf("cancel invitation: %w", err)
	}
	if tag.RowsAffected() == 0 {
		inv, getErr := r.GetInvitation(ctx, invitationID)
		if errors.Is(getErr, sql.ErrNoRows) || inv.OrganizationID != orgID {
			return sql.ErrNoRows
		}
		return ErrInvitationNotPending
	}
	return nil
}

// RejectInvitation records that the invited person declined. accountEmail
// is the authenticated caller's own account email, checked against the
// invitation's target — an account can only reject its own invitations.
func (r *PostgresRepository) RejectInvitation(ctx context.Context, invitationID uuid.UUID, accountEmail string) error {
	inv, err := r.GetInvitation(ctx, invitationID)
	if err != nil {
		return err
	}
	if !strings.EqualFold(inv.Email, accountEmail) {
		return ErrInvitationEmailMismatch
	}
	if inv.Status != InvitationPending {
		return ErrInvitationNotPending
	}
	if inv.IsExpired() {
		return ErrInvitationExpired
	}
	if _, err := r.pool.Exec(ctx, `
		UPDATE invitations SET status = 'rejected', updated_at = NOW() WHERE id = $1
	`, invitationID); err != nil {
		return fmt.Errorf("reject invitation: %w", err)
	}
	return nil
}

// AcceptInvitation creates the membership and marks the invitation accepted,
// atomically. Same email-ownership and pending/expiry checks as
// RejectInvitation. If the accepting account is somehow already a member
// (a race, or an invitation sent after they joined some other way), the
// membership insert is a no-op — accepting still succeeds rather than
// erroring on a harmless duplicate.
//
// Re-checks ErrOrgNotPaid at accept time, not just at CreateInvitation time:
// an org's subscription can lapse between sending an invite and it being
// accepted, and "org membership requires a paid seat" (§3.1) must hold at
// the moment the seat is actually granted, not just when the invite was
// sent.
func (r *PostgresRepository) AcceptInvitation(ctx context.Context, invitationID, acceptingUserID uuid.UUID, accountEmail string) (Invitation, error) {
	inv, err := r.GetInvitation(ctx, invitationID)
	if err != nil {
		return Invitation{}, err
	}
	if !strings.EqualFold(inv.Email, accountEmail) {
		return Invitation{}, ErrInvitationEmailMismatch
	}
	if inv.Status != InvitationPending {
		return Invitation{}, ErrInvitationNotPending
	}
	if inv.IsExpired() {
		return Invitation{}, ErrInvitationExpired
	}
	if err := r.requireOrgPaid(ctx, inv.OrganizationID); err != nil {
		return Invitation{}, err
	}

	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return Invitation{}, err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Exec(ctx, `
		INSERT INTO memberships (id, user_id, organization_id, role, created_at)
		VALUES ($1, $2, $3, $4, NOW())
		ON CONFLICT (user_id, organization_id) DO NOTHING
	`, uuid.New(), acceptingUserID, inv.OrganizationID, inv.Role); err != nil {
		return Invitation{}, fmt.Errorf("create membership from invitation: %w", err)
	}

	if _, err := tx.Exec(ctx, `
		UPDATE invitations SET status = 'accepted', updated_at = NOW() WHERE id = $1
	`, invitationID); err != nil {
		return Invitation{}, fmt.Errorf("mark invitation accepted: %w", err)
	}

	if err := tx.Commit(ctx); err != nil {
		return Invitation{}, err
	}
	inv.Status = InvitationAccepted
	return inv, nil
}
