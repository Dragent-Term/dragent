package organization

import (
	"context"
	"database/sql"
	"errors"
	"fmt"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// OrganizationWithRole is an organization joined with the calling user's
// role in it — what "list my organizations" actually needs, one row per
// membership rather than a separate lookup per org.
type OrganizationWithRole struct {
	Organization
	Role MembershipRole
}

// FullOrganization is the combined view get-full-organization provides in
// the reference plugin: the organization plus its current roster and team
// list in one response, instead of three round trips.
type FullOrganization struct {
	Organization Organization
	Members      []MemberWithUser
	Teams        []Team
}

// GetOrganization fetches a single organization by id.
func (r *PostgresRepository) GetOrganization(ctx context.Context, orgID uuid.UUID) (Organization, error) {
	var org Organization
	err := r.pool.QueryRow(ctx, `
		SELECT id, name, slug, created_at, updated_at FROM organizations WHERE id = $1
	`, orgID).Scan(&org.ID, &org.Name, &org.Slug, &org.CreatedAt, &org.UpdatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Organization{}, sql.ErrNoRows
		}
		return Organization{}, err
	}
	return org, nil
}

// GetFullOrganization combines GetOrganization, ListMembers, and ListTeams —
// a convenience aggregation, not new data.
func (r *PostgresRepository) GetFullOrganization(ctx context.Context, orgID uuid.UUID) (FullOrganization, error) {
	org, err := r.GetOrganization(ctx, orgID)
	if err != nil {
		return FullOrganization{}, err
	}
	members, err := r.ListMembers(ctx, orgID)
	if err != nil {
		return FullOrganization{}, fmt.Errorf("list members for full organization: %w", err)
	}
	teams, err := r.ListTeams(ctx, orgID)
	if err != nil {
		return FullOrganization{}, fmt.Errorf("list teams for full organization: %w", err)
	}
	return FullOrganization{Organization: org, Members: members, Teams: teams}, nil
}

// UpdateOrganization changes an organization's name and/or slug. Returns
// ErrSlugTaken on collision, sql.ErrNoRows if the organization doesn't
// exist.
func (r *PostgresRepository) UpdateOrganization(ctx context.Context, orgID uuid.UUID, name, slug string) (Organization, error) {
	var org Organization
	err := r.pool.QueryRow(ctx, `
		UPDATE organizations SET name = $2, slug = $3, updated_at = NOW()
		WHERE id = $1
		RETURNING id, name, slug, created_at, updated_at
	`, orgID, name, slug).Scan(&org.ID, &org.Name, &org.Slug, &org.CreatedAt, &org.UpdatedAt)
	if err != nil {
		if isUniqueViolation(err) {
			return Organization{}, ErrSlugTaken
		}
		if errors.Is(err, pgx.ErrNoRows) {
			return Organization{}, sql.ErrNoRows
		}
		return Organization{}, fmt.Errorf("update organization: %w", err)
	}
	return org, nil
}

// DeleteOrganization removes an organization and everything scoped to it
// (memberships, teams, domains, invitations — all declared ON DELETE
// CASCADE in their own migrations, so this is the one statement that needs
// to run). Returns sql.ErrNoRows if it doesn't exist.
func (r *PostgresRepository) DeleteOrganization(ctx context.Context, orgID uuid.UUID) error {
	tag, err := r.pool.Exec(ctx, `DELETE FROM organizations WHERE id = $1`, orgID)
	if err != nil {
		return fmt.Errorf("delete organization: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return sql.ErrNoRows
	}
	return nil
}

// IsSlugAvailable reports whether a slug is free to use — the preflight
// check before Create/Update so a client can validate as someone types
// instead of finding out only on submit.
func (r *PostgresRepository) IsSlugAvailable(ctx context.Context, slug string) (bool, error) {
	var exists bool
	err := r.pool.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM organizations WHERE slug = $1)`, slug).Scan(&exists)
	if err != nil {
		return false, fmt.Errorf("check slug availability: %w", err)
	}
	return !exists, nil
}

// ListOrganizationsForUser returns every organization a user belongs to,
// with their role in each, oldest membership first — so the first result
// is typically their original (often personal) organization.
func (r *PostgresRepository) ListOrganizationsForUser(ctx context.Context, userID uuid.UUID) ([]OrganizationWithRole, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT o.id, o.name, o.slug, o.created_at, o.updated_at, m.role
		FROM memberships m
		JOIN organizations o ON o.id = m.organization_id
		WHERE m.user_id = $1
		ORDER BY m.created_at ASC
	`, userID)
	if err != nil {
		return nil, fmt.Errorf("list organizations for user: %w", err)
	}
	defer rows.Close()

	var orgs []OrganizationWithRole
	for rows.Next() {
		var o OrganizationWithRole
		if err := rows.Scan(&o.ID, &o.Name, &o.Slug, &o.CreatedAt, &o.UpdatedAt, &o.Role); err != nil {
			return nil, fmt.Errorf("scan organization: %w", err)
		}
		orgs = append(orgs, o)
	}
	return orgs, rows.Err()
}

// ErrNotAMember is returned by SetActiveOrganization/SetActiveTeam when the
// caller isn't actually a member of the organization/team they're trying
// to make active.
var ErrNotAMember = errors.New("not a member")

// SetActiveOrganization records which organization a user is currently
// working in. Refuses (ErrNotAMember) unless they're actually a member —
// otherwise a user could "activate" an organization they have no access to
// and have every active-org-scoped read quietly resolve to it.
func (r *PostgresRepository) SetActiveOrganization(ctx context.Context, userID, orgID uuid.UUID) error {
	var isMember bool
	if err := r.pool.QueryRow(ctx, `
		SELECT EXISTS(SELECT 1 FROM memberships WHERE user_id = $1 AND organization_id = $2)
	`, userID, orgID).Scan(&isMember); err != nil {
		return fmt.Errorf("check membership before activating: %w", err)
	}
	if !isMember {
		return ErrNotAMember
	}
	if _, err := r.pool.Exec(ctx, `
		UPDATE users SET active_organization_id = $2 WHERE id = $1
	`, userID, orgID); err != nil {
		return fmt.Errorf("set active organization: %w", err)
	}
	return nil
}

// GetActiveOrganization resolves a user's current organization: their
// stored active_organization_id if it's still a real membership, otherwise
// their oldest membership as a fallback — a user with at least one
// organization (everyone, since signup always creates one — see
// InitializeTx) always resolves to something, never a hard "no active
// org" error. Same fallback spirit as the reference product's
// resolveSessionOrganizationState, adapted to dragent having no session
// table to hang the pointer off (see 00051's migration comment).
func (r *PostgresRepository) GetActiveOrganization(ctx context.Context, userID uuid.UUID) (OrganizationWithRole, error) {
	var org OrganizationWithRole
	err := r.pool.QueryRow(ctx, `
		SELECT o.id, o.name, o.slug, o.created_at, o.updated_at, m.role
		FROM users u
		JOIN memberships m ON m.user_id = u.id AND m.organization_id = u.active_organization_id
		JOIN organizations o ON o.id = m.organization_id
		WHERE u.id = $1
	`, userID).Scan(&org.ID, &org.Name, &org.Slug, &org.CreatedAt, &org.UpdatedAt, &org.Role)
	if err == nil {
		return org, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return OrganizationWithRole{}, fmt.Errorf("get active organization: %w", err)
	}

	// No stored active org, or it points at a membership that no longer
	// exists (left, removed, org deleted). Fall back to the oldest one.
	orgs, err := r.ListOrganizationsForUser(ctx, userID)
	if err != nil {
		return OrganizationWithRole{}, err
	}
	if len(orgs) == 0 {
		return OrganizationWithRole{}, sql.ErrNoRows
	}
	return orgs[0], nil
}

// ActiveOrganizationID is GetActiveOrganization narrowed to just the id —
// for a caller outside this package's domain that has no legitimate need
// for role/name/slug (e.g. billing.OrgResolver, which only ever needs an id
// to key a balance row). Keeping this here, rather than letting another
// package import OrganizationWithRole and extract .ID itself, is what keeps
// that package decoupled from organization's own domain shape: it depends
// on a one-method interface it defines, and this satisfies it structurally.
func (r *PostgresRepository) ActiveOrganizationID(ctx context.Context, userID uuid.UUID) (uuid.UUID, error) {
	org, err := r.GetActiveOrganization(ctx, userID)
	if err != nil {
		return uuid.Nil, err
	}
	return org.ID, nil
}
