package organization

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

type Team struct {
	ID             uuid.UUID
	OrganizationID uuid.UUID
	Name           string
	Slug           string
	CreatedAt      time.Time
	UpdatedAt      time.Time
}

// TeamMemberWithUser is a team_memberships row joined with the user it
// belongs to. No role field: team membership doesn't expose a role through
// the API (schema has one for symmetry with org memberships, but nothing
// consumes a "team role" yet, so every add is a plain member — the same
// don't-build-unused-complexity call made elsewhere in this package).
type TeamMemberWithUser struct {
	UserID    uuid.UUID
	Email     string
	CreatedAt time.Time
}

// ErrAlreadyTeamMember is returned when the target is already on the team.
var ErrAlreadyTeamMember = errors.New("user is already on this team")

// CreateTeam makes a new team within an organization. Slug is unique per
// organization (00047's UNIQUE(organization_id, slug)), not globally —
// two different organizations can each have a "design" team.
// CreateTeam returns ErrOrgNotPaid if the organization is still on Free —
// teams are a team-collaboration feature, gated to paid orgs.
func (r *PostgresRepository) CreateTeam(ctx context.Context, orgID uuid.UUID, name, slug string) (Team, error) {
	if err := r.requireOrgPaid(ctx, orgID); err != nil {
		return Team{}, err
	}
	t := Team{ID: uuid.New(), OrganizationID: orgID, Name: name, Slug: slug}
	err := r.pool.QueryRow(ctx, `
		INSERT INTO teams (id, organization_id, name, slug, created_at, updated_at)
		VALUES ($1, $2, $3, $4, NOW(), NOW())
		RETURNING created_at, updated_at
	`, t.ID, t.OrganizationID, t.Name, t.Slug).Scan(&t.CreatedAt, &t.UpdatedAt)
	if err != nil {
		if isUniqueViolation(err) {
			return Team{}, ErrSlugTaken
		}
		return Team{}, fmt.Errorf("create team: %w", err)
	}
	return t, nil
}

// GetTeam fetches a team scoped to an organization — a team id from a
// different organization returns sql.ErrNoRows, the same not-found
// convention GetDomain already uses for the same reason.
func (r *PostgresRepository) GetTeam(ctx context.Context, orgID, teamID uuid.UUID) (Team, error) {
	var t Team
	err := r.pool.QueryRow(ctx, `
		SELECT id, organization_id, name, slug, created_at, updated_at
		FROM teams WHERE id = $1 AND organization_id = $2
	`, teamID, orgID).Scan(&t.ID, &t.OrganizationID, &t.Name, &t.Slug, &t.CreatedAt, &t.UpdatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Team{}, sql.ErrNoRows
		}
		return Team{}, err
	}
	return t, nil
}

// UpdateTeam changes a team's name and/or slug.
func (r *PostgresRepository) UpdateTeam(ctx context.Context, orgID, teamID uuid.UUID, name, slug string) (Team, error) {
	var t Team
	err := r.pool.QueryRow(ctx, `
		UPDATE teams SET name = $3, slug = $4, updated_at = NOW()
		WHERE id = $1 AND organization_id = $2
		RETURNING id, organization_id, name, slug, created_at, updated_at
	`, teamID, orgID, name, slug).Scan(&t.ID, &t.OrganizationID, &t.Name, &t.Slug, &t.CreatedAt, &t.UpdatedAt)
	if err != nil {
		if isUniqueViolation(err) {
			return Team{}, ErrSlugTaken
		}
		if errors.Is(err, pgx.ErrNoRows) {
			return Team{}, sql.ErrNoRows
		}
		return Team{}, fmt.Errorf("update team: %w", err)
	}
	return t, nil
}

// RemoveTeam deletes a team. team_memberships rows cascade (00047).
func (r *PostgresRepository) RemoveTeam(ctx context.Context, orgID, teamID uuid.UUID) error {
	tag, err := r.pool.Exec(ctx, `DELETE FROM teams WHERE id = $1 AND organization_id = $2`, teamID, orgID)
	if err != nil {
		return fmt.Errorf("remove team: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return sql.ErrNoRows
	}
	return nil
}

// ListTeams returns an organization's teams, oldest first.
func (r *PostgresRepository) ListTeams(ctx context.Context, orgID uuid.UUID) ([]Team, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT id, organization_id, name, slug, created_at, updated_at
		FROM teams WHERE organization_id = $1 ORDER BY created_at ASC
	`, orgID)
	if err != nil {
		return nil, fmt.Errorf("list teams: %w", err)
	}
	defer rows.Close()

	var teams []Team
	for rows.Next() {
		var t Team
		if err := rows.Scan(&t.ID, &t.OrganizationID, &t.Name, &t.Slug, &t.CreatedAt, &t.UpdatedAt); err != nil {
			return nil, fmt.Errorf("scan team: %w", err)
		}
		teams = append(teams, t)
	}
	return teams, rows.Err()
}

// ListTeamMembers returns who's on a team, oldest first.
func (r *PostgresRepository) ListTeamMembers(ctx context.Context, orgID, teamID uuid.UUID) ([]TeamMemberWithUser, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT tm.user_id, u.email, tm.created_at
		FROM team_memberships tm
		JOIN teams t ON t.id = tm.team_id
		JOIN users u ON u.id = tm.user_id
		WHERE tm.team_id = $1 AND t.organization_id = $2
		ORDER BY tm.created_at ASC
	`, teamID, orgID)
	if err != nil {
		return nil, fmt.Errorf("list team members: %w", err)
	}
	defer rows.Close()

	var members []TeamMemberWithUser
	for rows.Next() {
		var m TeamMemberWithUser
		if err := rows.Scan(&m.UserID, &m.Email, &m.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan team member: %w", err)
		}
		members = append(members, m)
	}
	return members, rows.Err()
}

// AddTeamMember puts an existing organization member onto a team. Refuses
// (ErrNotAMember) if the target isn't in the organization at all — a team
// is a subgroup of an organization's members, not an independent roster.
func (r *PostgresRepository) AddTeamMember(ctx context.Context, orgID, teamID, userID uuid.UUID) error {
	var isOrgMember bool
	if err := r.pool.QueryRow(ctx, `
		SELECT EXISTS(SELECT 1 FROM memberships WHERE organization_id = $1 AND user_id = $2)
	`, orgID, userID).Scan(&isOrgMember); err != nil {
		return fmt.Errorf("check organization membership: %w", err)
	}
	if !isOrgMember {
		return ErrNotAMember
	}

	_, err := r.pool.Exec(ctx, `
		INSERT INTO team_memberships (id, user_id, team_id, role, created_at)
		VALUES ($1, $2, $3, 'member', NOW())
	`, uuid.New(), userID, teamID)
	if err != nil {
		if isUniqueViolation(err) {
			return ErrAlreadyTeamMember
		}
		return fmt.Errorf("add team member: %w", err)
	}
	return nil
}

// RemoveTeamMember takes someone off a team. Does not touch their
// organization membership — leaving a team is not leaving the org.
func (r *PostgresRepository) RemoveTeamMember(ctx context.Context, teamID, userID uuid.UUID) error {
	tag, err := r.pool.Exec(ctx, `
		DELETE FROM team_memberships WHERE team_id = $1 AND user_id = $2
	`, teamID, userID)
	if err != nil {
		return fmt.Errorf("remove team member: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return sql.ErrNoRows
	}
	return nil
}

// SetActiveTeam records which team a user is currently working in. Refuses
// (ErrNotAMember) unless they're actually on the team — same reasoning as
// SetActiveOrganization.
func (r *PostgresRepository) SetActiveTeam(ctx context.Context, userID, teamID uuid.UUID) error {
	var isMember bool
	if err := r.pool.QueryRow(ctx, `
		SELECT EXISTS(SELECT 1 FROM team_memberships WHERE user_id = $1 AND team_id = $2)
	`, userID, teamID).Scan(&isMember); err != nil {
		return fmt.Errorf("check team membership before activating: %w", err)
	}
	if !isMember {
		return ErrNotAMember
	}
	if _, err := r.pool.Exec(ctx, `UPDATE users SET active_team_id = $2 WHERE id = $1`, userID, teamID); err != nil {
		return fmt.Errorf("set active team: %w", err)
	}
	return nil
}

// ListUserTeams returns every team a user belongs to, across every
// organization they're in.
func (r *PostgresRepository) ListUserTeams(ctx context.Context, userID uuid.UUID) ([]Team, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT t.id, t.organization_id, t.name, t.slug, t.created_at, t.updated_at
		FROM team_memberships tm
		JOIN teams t ON t.id = tm.team_id
		WHERE tm.user_id = $1
		ORDER BY tm.created_at ASC
	`, userID)
	if err != nil {
		return nil, fmt.Errorf("list user teams: %w", err)
	}
	defer rows.Close()

	var teams []Team
	for rows.Next() {
		var t Team
		if err := rows.Scan(&t.ID, &t.OrganizationID, &t.Name, &t.Slug, &t.CreatedAt, &t.UpdatedAt); err != nil {
			return nil, fmt.Errorf("scan team: %w", err)
		}
		teams = append(teams, t)
	}
	return teams, rows.Err()
}
