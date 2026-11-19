package organization

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

type dbConn interface {
	Exec(ctx context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

type PostgresRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresRepository(pool *pgxpool.Pool) *PostgresRepository {
	return &PostgresRepository{pool: pool}
}

func (r *PostgresRepository) db(tx any) dbConn {
	if pgTx, ok := tx.(pgx.Tx); ok {
		return pgTx
	}
	return r.pool
}

// personalSlug derives a short, effectively-collision-free slug from a user
// id — first 8 hex characters, no dashes. The 00048 backfill migration uses
// the identical scheme in SQL, so a signup-created org and a backfilled one
// are indistinguishable.
func personalSlug(userID uuid.UUID) string {
	return strings.ReplaceAll(userID.String(), "-", "")[:8] + "-team"
}

// personalName mirrors the reference organization plugin's default naming
// ("{name}'s Team"), falling back to the email's local part when a user has
// no display name yet (magic-link signups can register with no name typed).
func personalName(displayName, email string) string {
	name := strings.TrimSpace(displayName)
	if name == "" {
		if at := strings.IndexByte(email, '@'); at > 0 {
			name = email[:at]
		} else {
			name = email
		}
	}
	return name + "'s Team"
}

// InitializeTx creates a personal organization and an owner membership for a
// newly registered user, inside the caller's transaction. Satisfies
// identity/components.OrganizationInitializer.
//
// Every account gets an organization — not opt-in. A user without
// one has nowhere for org-scoped features (billing, model policy, sharing)
// to attach.
// InitializeTx returns the id of the organization the user landed in
// (the auto-join target, or their newly created personal org) — needed by
// the caller to initialize that user's per-org balance row in the same
// transaction (organization_id is NOT NULL, so balance init can never run before the
// org it belongs to is known).
func (r *PostgresRepository) InitializeTx(ctx context.Context, tx any, userID uuid.UUID, displayName, email string) (uuid.UUID, error) {
	db := r.db(tx)

	// A verified, auto-join-enabled domain match takes priority over
	// creating a personal org — same order the reference organization
	// plugin uses (domain match first, personal org as fallback), see plan
	// §3.1. Never checked for public-provider domains: those can never be
	// claimed (11.A.6), so a lookup for gmail.com would only ever waste a
	// query, never match.
	if domain := emailDomain(email); domain != "" && !isPublicEmailDomain(domain) {
		var autoJoinOrgID uuid.UUID
		err := db.QueryRow(ctx, `
			SELECT organization_id FROM organization_domains
			WHERE domain = $1 AND verified_at IS NOT NULL AND auto_join_enabled = TRUE
			LIMIT 1
		`, domain).Scan(&autoJoinOrgID)
		if err == nil {
			if err := insertMembershipTx(ctx, db, userID, autoJoinOrgID, RoleMember); err != nil {
				return uuid.Nil, err
			}
			return autoJoinOrgID, nil
		}
		if !errors.Is(err, pgx.ErrNoRows) {
			return uuid.Nil, fmt.Errorf("check domain auto-join: %w", err)
		}
		// pgx.ErrNoRows: no matching verified+enabled domain, fall through
		// to the personal-org path below.
	}

	orgID := uuid.New()
	name := personalName(displayName, email)
	slug := personalSlug(userID)

	if _, err := db.Exec(ctx, `
		INSERT INTO organizations (id, name, slug, created_at, updated_at)
		VALUES ($1, $2, $3, NOW(), NOW())
	`, orgID, name, slug); err != nil {
		return uuid.Nil, fmt.Errorf("create personal organization: %w", err)
	}

	if err := insertMembershipTx(ctx, db, userID, orgID, RoleOwner); err != nil {
		return uuid.Nil, err
	}
	return orgID, nil
}

// insertMembershipTx is the shared membership-row insert used by both the
// personal-org-owner path and the domain-auto-join-member path above.
func insertMembershipTx(ctx context.Context, db dbConn, userID, orgID uuid.UUID, role MembershipRole) error {
	if _, err := db.Exec(ctx, `
		INSERT INTO memberships (id, user_id, organization_id, role, created_at)
		VALUES ($1, $2, $3, $4, NOW())
	`, uuid.New(), userID, orgID, role); err != nil {
		return fmt.Errorf("create membership: %w", err)
	}
	return nil
}

// emailDomain returns the part of email after "@", or "" if email has no
// "@". Not validated beyond that — callers already have a real user's
// stored email, not untrusted input.
func emailDomain(email string) string {
	at := strings.IndexByte(email, '@')
	if at < 0 || at == len(email)-1 {
		return ""
	}
	return strings.ToLower(email[at+1:])
}

// ErrSlugTaken is returned when a chosen organization slug collides with
// an existing one.
var ErrSlugTaken = errors.New("organization slug is already taken")

func isUniqueViolation(err error) bool {
	var pgErr *pgconn.PgError
	return errors.As(err, &pgErr) && pgErr.Code == "23505"
}

// Create makes a new organization with the given user as its owner. Not the
// signup path (see InitializeTx) — this is for a user deliberately creating
// an additional organization.
func (r *PostgresRepository) Create(ctx context.Context, name, slug string, ownerUserID uuid.UUID) (Organization, error) {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return Organization{}, err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	org := Organization{ID: uuid.New(), Name: name, Slug: slug}
	if _, err := tx.Exec(ctx, `
		INSERT INTO organizations (id, name, slug, created_at, updated_at)
		VALUES ($1, $2, $3, NOW(), NOW())
	`, org.ID, org.Name, org.Slug); err != nil {
		if isUniqueViolation(err) {
			return Organization{}, ErrSlugTaken
		}
		return Organization{}, fmt.Errorf("create organization: %w", err)
	}

	if _, err := tx.Exec(ctx, `
		INSERT INTO memberships (id, user_id, organization_id, role, created_at)
		VALUES ($1, $2, $3, $4, NOW())
	`, uuid.New(), ownerUserID, org.ID, RoleOwner); err != nil {
		return Organization{}, fmt.Errorf("create owner membership: %w", err)
	}

	if err := tx.Commit(ctx); err != nil {
		return Organization{}, err
	}
	return org, nil
}

// ErrCannotModifyOwner is returned by RemoveMember and ChangeMemberRole when
// the target's current role is owner. Ownership can only be set at creation
// (Create, InitializeTx) — removing or demoting an owner through generic
// member management would risk leaving an organization ownerless, and a
// deliberate ownership-transfer flow is a different, more sensitive
// operation than this endpoint, not something to fold in here.
var ErrCannotModifyOwner = errors.New("ownership cannot be changed through member management")

// RemoveMember removes a non-owner member from an organization. Returns
// sql.ErrNoRows if the target isn't a member, ErrCannotModifyOwner if they
// are but their role is owner.
func (r *PostgresRepository) RemoveMember(ctx context.Context, orgID, targetUserID uuid.UUID) error {
	tag, err := r.pool.Exec(ctx, `
		DELETE FROM memberships WHERE organization_id = $1 AND user_id = $2 AND role != 'owner'
	`, orgID, targetUserID)
	if err != nil {
		return fmt.Errorf("remove member: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return r.disambiguateMemberWriteFailure(ctx, orgID, targetUserID)
	}
	return nil
}

// ErrLastOwner is returned by LeaveOrganization when the caller is the
// organization's only owner — leaving would make it ownerless.
var ErrLastOwner = errors.New("cannot leave: you are the last owner of this organization")

// LeaveOrganization removes the caller's own membership — the self-service
// counterpart to RemoveMember, which flatly refuses any owner target.
// Leaving needs the more nuanced rule: fine for an owner unless they're the
// LAST one, so it takes a transaction with a row lock to count the others
// safely rather than a single guarded statement.
func (r *PostgresRepository) LeaveOrganization(ctx context.Context, orgID, userID uuid.UUID) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	var role MembershipRole
	err = tx.QueryRow(ctx, `
		SELECT role FROM memberships WHERE organization_id = $1 AND user_id = $2 FOR UPDATE
	`, orgID, userID).Scan(&role)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return sql.ErrNoRows
		}
		return fmt.Errorf("find membership: %w", err)
	}

	if role == RoleOwner {
		var otherOwners int
		if err := tx.QueryRow(ctx, `
			SELECT count(*) FROM memberships WHERE organization_id = $1 AND role = 'owner' AND user_id != $2
		`, orgID, userID).Scan(&otherOwners); err != nil {
			return fmt.Errorf("count other owners: %w", err)
		}
		if otherOwners == 0 {
			return ErrLastOwner
		}
	}

	if _, err := tx.Exec(ctx, `
		DELETE FROM memberships WHERE organization_id = $1 AND user_id = $2
	`, orgID, userID); err != nil {
		return fmt.Errorf("leave organization: %w", err)
	}

	return tx.Commit(ctx)
}

// GetActiveMember returns the caller's own membership row in their active
// organization (GetActiveOrganization) — "who am I, in the org I'm
// currently working in."
func (r *PostgresRepository) GetActiveMember(ctx context.Context, userID uuid.UUID) (MemberWithUser, error) {
	activeOrg, err := r.GetActiveOrganization(ctx, userID)
	if err != nil {
		return MemberWithUser{}, err
	}
	var m MemberWithUser
	err = r.pool.QueryRow(ctx, `
		SELECT m.user_id, u.email, m.role, m.created_at
		FROM memberships m
		JOIN users u ON u.id = m.user_id
		WHERE m.user_id = $1 AND m.organization_id = $2
	`, userID, activeOrg.ID).Scan(&m.UserID, &m.Email, &m.Role, &m.CreatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return MemberWithUser{}, sql.ErrNoRows
		}
		return MemberWithUser{}, err
	}
	return m, nil
}

// ChangeMemberRole updates a non-owner member's role. newRole must not be
// RoleOwner — the handler validates that before calling this, but the WHERE
// clause below refuses it regardless, so this method is safe even if a
// future caller forgets to check. Same not-found/owner-target errors as
// RemoveMember.
func (r *PostgresRepository) ChangeMemberRole(ctx context.Context, orgID, targetUserID uuid.UUID, newRole MembershipRole) error {
	tag, err := r.pool.Exec(ctx, `
		UPDATE memberships SET role = $3
		WHERE organization_id = $1 AND user_id = $2 AND role != 'owner' AND $3 != 'owner'
	`, orgID, targetUserID, newRole)
	if err != nil {
		return fmt.Errorf("change member role: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return r.disambiguateMemberWriteFailure(ctx, orgID, targetUserID)
	}
	return nil
}

// disambiguateMemberWriteFailure distinguishes "not a member at all" from
// "is a member, but their role is owner" after a zero-rows-affected write,
// so RemoveMember/ChangeMemberRole can report a useful error instead of a
// generic failure.
func (r *PostgresRepository) disambiguateMemberWriteFailure(ctx context.Context, orgID, targetUserID uuid.UUID) error {
	var role MembershipRole
	err := r.pool.QueryRow(ctx, `
		SELECT role FROM memberships WHERE organization_id = $1 AND user_id = $2
	`, orgID, targetUserID).Scan(&role)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return sql.ErrNoRows
		}
		return err
	}
	return ErrCannotModifyOwner
}

// ListMembers returns an organization's roster, oldest membership first.
func (r *PostgresRepository) ListMembers(ctx context.Context, orgID uuid.UUID) ([]MemberWithUser, error) {
	rows, err := r.pool.Query(ctx, `
		SELECT m.user_id, u.email, m.role, m.created_at
		FROM memberships m
		JOIN users u ON u.id = m.user_id
		WHERE m.organization_id = $1
		ORDER BY m.created_at ASC
	`, orgID)
	if err != nil {
		return nil, fmt.Errorf("list members: %w", err)
	}
	defer rows.Close()

	var members []MemberWithUser
	for rows.Next() {
		var m MemberWithUser
		if err := rows.Scan(&m.UserID, &m.Email, &m.Role, &m.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan member: %w", err)
		}
		members = append(members, m)
	}
	return members, rows.Err()
}

// ListMembersByRole is ListMembers narrowed to a set of roles — the single
// primitive both "owners" (who should hold a billing-portal link) and
// "billing recipients" (owners+admins, who should be *told* about a billing
// change but not handed a portal session — the reference implementation's distinction,
// packages/auth/src/utils/billing.ts) are expressed with, rather than two
// near-duplicate queries.
func (r *PostgresRepository) ListMembersByRole(ctx context.Context, orgID uuid.UUID, roles ...MembershipRole) ([]MemberWithUser, error) {
	if len(roles) == 0 {
		return nil, nil
	}
	roleStrs := make([]string, len(roles))
	for i, role := range roles {
		roleStrs[i] = string(role)
	}
	rows, err := r.pool.Query(ctx, `
		SELECT m.user_id, u.email, m.role, m.created_at
		FROM memberships m
		JOIN users u ON u.id = m.user_id
		WHERE m.organization_id = $1 AND m.role = ANY($2)
		ORDER BY m.created_at ASC
	`, orgID, roleStrs)
	if err != nil {
		return nil, fmt.Errorf("list members by role: %w", err)
	}
	defer rows.Close()

	var members []MemberWithUser
	for rows.Next() {
		var m MemberWithUser
		if err := rows.Scan(&m.UserID, &m.Email, &m.Role, &m.CreatedAt); err != nil {
			return nil, fmt.Errorf("scan member: %w", err)
		}
		members = append(members, m)
	}
	return members, rows.Err()
}

// ErrDomainTaken is returned when a domain is already submitted (by this
// organization or another one) — domains are globally unique, see 00049.
var ErrDomainTaken = errors.New("domain is already submitted for verification")

// generateVerificationToken returns a 32-byte random token, hex-encoded.
// Not derived from anything guessable (org id, domain, time) — a token an
// attacker could predict would defeat the point of verification.
func generateVerificationToken() (string, error) {
	buf := make([]byte, 32)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("generate verification token: %w", err)
	}
	return hex.EncodeToString(buf), nil
}

// AddDomain submits a domain for verification. Returns ErrDomainTaken if the
// domain is already claimed by any organization (including this one).
// AddDomain returns ErrOrgNotPaid if the organization is still on Free —
// domain claiming exists to enable company-wide auto-join collaboration,
// gated to paid orgs the same as inviting and teams.
func (r *PostgresRepository) AddDomain(ctx context.Context, orgID uuid.UUID, domain string) (OrganizationDomain, error) {
	if err := r.requireOrgPaid(ctx, orgID); err != nil {
		return OrganizationDomain{}, err
	}
	token, err := generateVerificationToken()
	if err != nil {
		return OrganizationDomain{}, err
	}

	d := OrganizationDomain{
		ID:                uuid.New(),
		OrganizationID:    orgID,
		Domain:            domain,
		VerificationToken: token,
	}

	_, err = r.pool.Exec(ctx, `
		INSERT INTO organization_domains (id, organization_id, domain, verification_token, created_at, updated_at)
		VALUES ($1, $2, $3, $4, NOW(), NOW())
	`, d.ID, d.OrganizationID, d.Domain, d.VerificationToken)
	if err != nil {
		if isUniqueViolation(err) {
			return OrganizationDomain{}, ErrDomainTaken
		}
		return OrganizationDomain{}, fmt.Errorf("add domain: %w", err)
	}
	return d, nil
}

// GetDomain fetches a domain row scoped to the given organization — a
// domain id from a different organization returns sql.ErrNoRows, the same
// as one that doesn't exist, so this can't be used to probe other
// organizations' pending domains.
func (r *PostgresRepository) GetDomain(ctx context.Context, orgID, domainID uuid.UUID) (OrganizationDomain, error) {
	var d OrganizationDomain
	err := r.pool.QueryRow(ctx, `
		SELECT id, organization_id, domain, verification_token, verified_at, auto_join_enabled, created_at
		FROM organization_domains
		WHERE id = $1 AND organization_id = $2
	`, domainID, orgID).Scan(&d.ID, &d.OrganizationID, &d.Domain, &d.VerificationToken, &d.VerifiedAt, &d.AutoJoinEnabled, &d.CreatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return OrganizationDomain{}, sql.ErrNoRows
		}
		return OrganizationDomain{}, err
	}
	return d, nil
}

// DNSLookupTXT resolves the TXT records for a host. Production wiring uses
// net.LookupTXT; tests inject a fake so the match logic below is provable
// without owning a real domain to publish a challenge record under.
type DNSLookupTXT func(host string) ([]string, error)

// domainTokenMatches reports whether token appears among records — pulled
// out of VerifyDomain as a pure function so the matching decision is
// unit-testable without a database connection.
func domainTokenMatches(records []string, token string) bool {
	for _, rec := range records {
		if rec == token {
			return true
		}
	}
	return false
}

// VerifyDomain checks whether d's challenge token is published as a TXT
// record at d.VerificationHost(), and if so marks it verified. Returns
// (false, nil) — not an error — when the record isn't there yet or doesn't
// match; that's the normal "not verified yet" outcome, not a failure.
func (r *PostgresRepository) VerifyDomain(ctx context.Context, d OrganizationDomain, lookup DNSLookupTXT) (bool, error) {
	records, err := lookup(d.VerificationHost())
	if err != nil {
		// A lookup error (NXDOMAIN, no TXT records, timeout) means "not
		// verified yet," not a server error — the admin hasn't published
		// the record, or hasn't finished propagating, which is routine.
		return false, nil
	}

	if !domainTokenMatches(records, d.VerificationToken) {
		return false, nil
	}

	_, err = r.pool.Exec(ctx, `
		UPDATE organization_domains SET verified_at = NOW(), updated_at = NOW()
		WHERE id = $1
	`, d.ID)
	if err != nil {
		return false, fmt.Errorf("mark domain verified: %w", err)
	}
	return true, nil
}

// ErrDomainNotVerified is returned when enabling auto-join is attempted on
// a domain that hasn't passed DNS verification yet.
var ErrDomainNotVerified = errors.New("domain must be verified before auto-join can be enabled")

// SetAutoJoin enables or disables domain auto-join. Enabling is refused
// (ErrDomainNotVerified) unless the domain has already passed verification
// — off by default, and never enabled by the same request that submits an
// unverified domain, matching the plan's safeguard list (§10).
func (r *PostgresRepository) SetAutoJoin(ctx context.Context, orgID, domainID uuid.UUID, enabled bool) error {
	if !enabled {
		tag, err := r.pool.Exec(ctx, `
			UPDATE organization_domains SET auto_join_enabled = FALSE, updated_at = NOW()
			WHERE id = $1 AND organization_id = $2
		`, domainID, orgID)
		if err != nil {
			return fmt.Errorf("disable auto-join: %w", err)
		}
		if tag.RowsAffected() == 0 {
			return sql.ErrNoRows
		}
		return nil
	}

	tag, err := r.pool.Exec(ctx, `
		UPDATE organization_domains SET auto_join_enabled = TRUE, updated_at = NOW()
		WHERE id = $1 AND organization_id = $2 AND verified_at IS NOT NULL
	`, domainID, orgID)
	if err != nil {
		return fmt.Errorf("enable auto-join: %w", err)
	}
	if tag.RowsAffected() == 0 {
		// Distinguish "doesn't exist/wrong org" from "exists but
		// unverified" for a useful error, without a second round-trip in
		// the common (already verified) case above.
		if _, getErr := r.GetDomain(ctx, orgID, domainID); errors.Is(getErr, sql.ErrNoRows) {
			return sql.ErrNoRows
		}
		return ErrDomainNotVerified
	}
	return nil
}

// IsDomainClaimed reports whether any organization has already submitted
// this domain — verified or still pending, since the schema's UNIQUE
// constraint on domain means a second submission is impossible either way.
// A pending claim from someone else is reason enough not to invite a
// different person to start a competing one.
func (r *PostgresRepository) IsDomainClaimed(ctx context.Context, domain string) (bool, error) {
	var exists bool
	err := r.pool.QueryRow(ctx, `
		SELECT EXISTS(SELECT 1 FROM organization_domains WHERE domain = $1)
	`, domain).Scan(&exists)
	if err != nil {
		return false, fmt.Errorf("check domain claimed: %w", err)
	}
	return exists, nil
}

// MembershipRole returns the caller's role in the given organization, or
// sql.ErrNoRows if they are not a member — the same not-found convention
// identity.PostgresUserRepository uses, so callers can check with
// errors.Is(err, sql.ErrNoRows) regardless of which package they're calling.
func (r *PostgresRepository) MembershipRole(ctx context.Context, userID, orgID uuid.UUID) (MembershipRole, error) {
	var role MembershipRole
	err := r.pool.QueryRow(ctx, `
		SELECT role FROM memberships WHERE user_id = $1 AND organization_id = $2
	`, userID, orgID).Scan(&role)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return "", sql.ErrNoRows
		}
		return "", err
	}
	return role, nil
}
