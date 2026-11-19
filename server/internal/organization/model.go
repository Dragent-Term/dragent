package organization

import (
	"time"

	"github.com/google/uuid"
)

type Organization struct {
	ID        uuid.UUID
	Name      string
	Slug      string
	CreatedAt time.Time
	UpdatedAt time.Time
}

type MembershipRole string

const (
	RoleOwner  MembershipRole = "owner"
	RoleAdmin  MembershipRole = "admin"
	RoleMember MembershipRole = "member"
)

// roleRank orders roles from least to most privileged so authorization can
// ask "at least X" without hardcoding the comparison at every call site.
var roleRank = map[MembershipRole]int{
	RoleMember: 1,
	RoleAdmin:  2,
	RoleOwner:  3,
}

// AtLeast reports whether r has at least the privilege of min. An unknown
// role ranks below every known role, so it never satisfies any requirement.
func (r MembershipRole) AtLeast(min MembershipRole) bool {
	got, ok := roleRank[r]
	if !ok {
		return false
	}
	want, ok := roleRank[min]
	if !ok {
		return false
	}
	return got >= want
}

type Membership struct {
	ID             uuid.UUID
	UserID         uuid.UUID
	OrganizationID uuid.UUID
	Role           MembershipRole
	CreatedAt      time.Time
}

// MemberWithUser is a membership row joined with the identifying fields of
// the user it belongs to — what a member-roster listing actually needs.
type MemberWithUser struct {
	UserID    uuid.UUID
	Email     string
	Role      MembershipRole
	CreatedAt time.Time
}

// OrganizationDomain is a domain an admin has submitted for verification.
// VerifiedAt is nil until a DNS lookup confirms the challenge token; only a
// verified domain may have AutoJoinEnabled turned on (11.A.7 enforces that,
// not this package's schema).
type OrganizationDomain struct {
	ID                uuid.UUID
	OrganizationID    uuid.UUID
	Domain            string
	VerificationToken string
	VerifiedAt        *time.Time
	AutoJoinEnabled   bool
	CreatedAt         time.Time
}

// VerificationHost is the DNS name a domain's challenge TXT record must be
// published at — a dedicated subdomain, not the domain's apex, so this
// never collides with a domain's existing SPF/DKIM/other TXT records.
func (d OrganizationDomain) VerificationHost() string {
	return "_dragent-challenge." + d.Domain
}
