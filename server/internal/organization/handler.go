package organization

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	identityModel "server/internal/identity/model"
)

// UserLookup is the subset of identity's UserRepository needed here: GetByEmail
// resolves a manual invite's target to an existing account; GetByID resolves
// the caller's own email for the unclaimed-domain-prompt eligibility check.
// Declared here rather than depending on the full identity repository,
// matching the dependency-inversion shape identity itself uses for
// BalanceInitializer.
type UserLookup interface {
	GetByEmail(ctx context.Context, email string) (*identityModel.User, error)
	GetByID(ctx context.Context, id uuid.UUID) (*identityModel.User, error)
}

// Repository is the subset of PostgresRepository the handler needs — an
// interface so it stays testable without a real database.
type Repository interface {
	Create(ctx context.Context, name, slug string, ownerUserID uuid.UUID) (Organization, error)
	GetOrganization(ctx context.Context, orgID uuid.UUID) (Organization, error)
	GetFullOrganization(ctx context.Context, orgID uuid.UUID) (FullOrganization, error)
	UpdateOrganization(ctx context.Context, orgID uuid.UUID, name, slug string) (Organization, error)
	DeleteOrganization(ctx context.Context, orgID uuid.UUID) error
	IsSlugAvailable(ctx context.Context, slug string) (bool, error)
	ListOrganizationsForUser(ctx context.Context, userID uuid.UUID) ([]OrganizationWithRole, error)
	SetActiveOrganization(ctx context.Context, userID, orgID uuid.UUID) error
	GetActiveOrganization(ctx context.Context, userID uuid.UUID) (OrganizationWithRole, error)
	RemoveMember(ctx context.Context, orgID, targetUserID uuid.UUID) error
	ChangeMemberRole(ctx context.Context, orgID, targetUserID uuid.UUID, newRole MembershipRole) error
	ListMembers(ctx context.Context, orgID uuid.UUID) ([]MemberWithUser, error)
	LeaveOrganization(ctx context.Context, orgID, userID uuid.UUID) error
	GetActiveMember(ctx context.Context, userID uuid.UUID) (MemberWithUser, error)
	AddDomain(ctx context.Context, orgID uuid.UUID, domain string) (OrganizationDomain, error)
	GetDomain(ctx context.Context, orgID, domainID uuid.UUID) (OrganizationDomain, error)
	VerifyDomain(ctx context.Context, d OrganizationDomain, lookup DNSLookupTXT) (bool, error)
	SetAutoJoin(ctx context.Context, orgID, domainID uuid.UUID, enabled bool) error
	IsDomainClaimed(ctx context.Context, domain string) (bool, error)
	CreateInvitation(ctx context.Context, orgID uuid.UUID, email string, role MembershipRole, inviterID uuid.UUID) (Invitation, error)
	GetInvitation(ctx context.Context, invitationID uuid.UUID) (Invitation, error)
	ListInvitations(ctx context.Context, orgID uuid.UUID) ([]Invitation, error)
	ListInvitationsForEmail(ctx context.Context, email string) ([]Invitation, error)
	CancelInvitation(ctx context.Context, orgID, invitationID uuid.UUID) error
	AcceptInvitation(ctx context.Context, invitationID, acceptingUserID uuid.UUID, accountEmail string) (Invitation, error)
	RejectInvitation(ctx context.Context, invitationID uuid.UUID, accountEmail string) error
	CreateTeam(ctx context.Context, orgID uuid.UUID, name, slug string) (Team, error)
	GetTeam(ctx context.Context, orgID, teamID uuid.UUID) (Team, error)
	UpdateTeam(ctx context.Context, orgID, teamID uuid.UUID, name, slug string) (Team, error)
	RemoveTeam(ctx context.Context, orgID, teamID uuid.UUID) error
	ListTeams(ctx context.Context, orgID uuid.UUID) ([]Team, error)
	ListTeamMembers(ctx context.Context, orgID, teamID uuid.UUID) ([]TeamMemberWithUser, error)
	AddTeamMember(ctx context.Context, orgID, teamID, userID uuid.UUID) error
	RemoveTeamMember(ctx context.Context, teamID, userID uuid.UUID) error
	SetActiveTeam(ctx context.Context, userID, teamID uuid.UUID) error
	ListUserTeams(ctx context.Context, userID uuid.UUID) ([]Team, error)
	GetOrgBilling(ctx context.Context, orgID uuid.UUID) (OrgBilling, error)
}

// RoleGate builds middleware requiring at least minRole in the request's
// {orgID} organization. Supplied by main (where RequireAuth/RequireOrgRole
// already live) so this package does not import it back.
type RoleGate func(minRole MembershipRole) func(http.Handler) http.Handler

// Mailer sends organization lifecycle notifications. Declared here as a
// narrow interface, rather than importing identity/model, matching the
// dependency-inversion shape used throughout this codebase (e.g. identity's
// own BalanceInitializer) — the concrete SmtpMailer already satisfies it
// structurally.
type Mailer interface {
	SendOrganizationInvitation(ctx context.Context, email, orgName, inviterName, role, acceptURL string, expiresAt time.Time) error
	SendMemberAdded(ctx context.Context, email, memberName, orgName, role, addedByName string) error
	SendMemberRemoved(ctx context.Context, email, memberName, orgName, removedByName string) error
	SendSubscriptionStarted(ctx context.Context, email, orgName, planName string) error
	SendSubscriptionCancelled(ctx context.Context, email, orgName, planName string, dueToPaymentFailure bool) error
	SendPaymentFailed(ctx context.Context, email, orgName string, amountCents int64, currency, invoiceURL string) error
	SendMemberAddedBilling(ctx context.Context, email, orgName, memberName string, newSeatCount int, perSeatPriceCents, newMonthlyTotalCents int64, currency string) error
	SendMemberRemovedBilling(ctx context.Context, email, orgName, memberName string, newSeatCount int, perSeatPriceCents, newMonthlyTotalCents int64, currency string) error
	SendRenewalUpcoming(ctx context.Context, email, orgName, planName string, renewsAt time.Time, amountCents int64, currency string, seatCount int) error
}

type Handler struct {
	repo       Repository
	users      UserLookup
	gate       RoleGate
	dnsLookup  DNSLookupTXT
	billing    *BillingService // nil when org billing isn't wired (e.g. payments disabled)
	mailer     Mailer          // nil-safe: no notification sent if unset
	appBaseURL string          // for building web deep links (e.g. invitation accept URL)
}

func NewHandler(repo Repository, users UserLookup, gate RoleGate, dnsLookup DNSLookupTXT, appBaseURL string) *Handler {
	return &Handler{repo: repo, users: users, gate: gate, dnsLookup: dnsLookup, appBaseURL: appBaseURL}
}

// SetBillingService wires org billing in. Optional, setter-after-construct —
// same pattern as SetBalanceInitializer/SetOrganizationInitializer — so a
// deployment without payments configured leaves this nil and every billing
// call site (all nil-safe) simply no-ops.
func (h *Handler) SetBillingService(b *BillingService) { h.billing = b }

// SetMailer wires organization notification emails in. Optional,
// setter-after-construct, same pattern as SetBillingService — a nil mailer
// simply skips sending, never blocks the underlying action.
func (h *Handler) SetMailer(m Mailer) { h.mailer = m }

// invitationAcceptURL is the web deep link an invitation email's CTA points
// to. The invitee need not have an account yet — the frontend route is
// responsible for routing an unauthenticated visitor through sign-in/sign-up
// first, then landing them back here to accept.
func (h *Handler) invitationAcceptURL(invitationID uuid.UUID) string {
	return h.appBaseURL + "/invitations/" + invitationID.String()
}

func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/api/v1/organizations", h.createOrganization)
	r.Get("/api/v1/organizations", h.listMyOrganizations)
	r.Get("/api/v1/organizations/check-slug", h.checkSlug)
	r.Get("/api/v1/organizations/domain-prompt-eligibility", h.domainPromptEligibility)
	r.With(h.gate(RoleMember)).Get("/api/v1/organizations/{orgID}", h.getOrganization)
	r.With(h.gate(RoleMember)).Get("/api/v1/organizations/{orgID}/full", h.getFullOrganization)
	r.With(h.gate(RoleAdmin)).Patch("/api/v1/organizations/{orgID}", h.updateOrganization)
	r.With(h.gate(RoleOwner)).Delete("/api/v1/organizations/{orgID}", h.deleteOrganization)
	r.With(h.gate(RoleMember)).Post("/api/v1/organizations/{orgID}/set-active", h.setActiveOrganization)
	r.Get("/api/v1/me/active-organization", h.getActiveOrganization)
	r.With(h.gate(RoleMember)).Get("/api/v1/organizations/{orgID}/members", h.listMembers)
	r.With(h.gate(RoleAdmin)).Delete("/api/v1/organizations/{orgID}/members/{userID}", h.removeMember)
	r.With(h.gate(RoleAdmin)).Patch("/api/v1/organizations/{orgID}/members/{userID}", h.changeMemberRole)
	r.With(h.gate(RoleMember)).Post("/api/v1/organizations/{orgID}/leave", h.leaveOrganization)
	r.Get("/api/v1/me/active-member", h.getActiveMember)
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/domains", h.addDomain)
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/domains/{domainID}/verify", h.verifyDomain)
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/domains/{domainID}/auto-join", h.setAutoJoin)

	// Invitations replace the old immediate-add model: nobody joins an
	// organization without explicitly accepting (decided 2026-09-18).
	// Create/list/cancel are admin-side, org-gated. Accept/reject/list-mine
	// are NOT org-gated — the invited person isn't a member yet, that's the
	// entire point — they only need to be authenticated, which RequireAuth
	// (already applied to this whole route group in main.go) covers.
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/invitations", h.createInvitation)
	r.With(h.gate(RoleAdmin)).Get("/api/v1/organizations/{orgID}/invitations", h.listInvitations)
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/invitations/{invitationID}/cancel", h.cancelInvitation)
	r.Post("/api/v1/invitations/{invitationID}/accept", h.acceptInvitation)
	r.Post("/api/v1/invitations/{invitationID}/reject", h.rejectInvitation)
	r.Get("/api/v1/me/invitations", h.listMyInvitations)

	// Teams: a subgroup of an organization's own members (AddTeamMember
	// refuses anyone who isn't already an org member) — never an
	// independent roster or a second invite path.
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/teams", h.createTeam)
	r.With(h.gate(RoleMember)).Get("/api/v1/organizations/{orgID}/teams", h.listTeams)
	r.With(h.gate(RoleAdmin)).Patch("/api/v1/organizations/{orgID}/teams/{teamID}", h.updateTeam)
	r.With(h.gate(RoleAdmin)).Delete("/api/v1/organizations/{orgID}/teams/{teamID}", h.removeTeam)
	r.With(h.gate(RoleMember)).Get("/api/v1/organizations/{orgID}/teams/{teamID}/members", h.listTeamMembers)
	r.With(h.gate(RoleAdmin)).Post("/api/v1/organizations/{orgID}/teams/{teamID}/members", h.addTeamMember)
	r.With(h.gate(RoleAdmin)).Delete("/api/v1/organizations/{orgID}/teams/{teamID}/members/{userID}", h.removeTeamMember)
	r.With(h.gate(RoleMember)).Post("/api/v1/organizations/{orgID}/teams/{teamID}/set-active", h.setActiveTeam)
	r.Get("/api/v1/me/teams", h.listMyTeams)

	// Billing: owner-gated to subscribe (a financial commitment on behalf of
	// the whole org), member-gated to read status. Seat grant/revoke on
	// membership change is not a separate route — it's hooked directly into
	// acceptInvitation/removeMember/leaveOrganization above.
	r.With(h.gate(RoleOwner)).Post("/api/v1/organizations/{orgID}/billing/subscribe", h.subscribeOrganization)
	r.With(h.gate(RoleMember)).Get("/api/v1/organizations/{orgID}/billing", h.getOrganizationBilling)
	r.With(h.gate(RoleOwner)).Get("/api/v1/organizations/{orgID}/billing/portal", h.organizationBillingPortal)
}

// ── Request/response types ───────────────────────────────────────────────────

type createOrganizationRequest struct {
	Name string `json:"name"`
	Slug string `json:"slug"`
}

type organizationResponse struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Slug string `json:"slug"`
}

func toOrganizationResponse(org Organization) organizationResponse {
	return organizationResponse{ID: org.ID.String(), Name: org.Name, Slug: org.Slug}
}

type organizationWithRoleResponse struct {
	organizationResponse
	Role string `json:"role"`
}

func toOrganizationWithRoleResponse(org OrganizationWithRole) organizationWithRoleResponse {
	return organizationWithRoleResponse{organizationResponse: toOrganizationResponse(org.Organization), Role: string(org.Role)}
}

type fullOrganizationResponse struct {
	Organization organizationResponse `json:"organization"`
	Members      []memberResponse     `json:"members"`
	Teams        []teamResponse       `json:"teams"`
}

type updateOrganizationRequest struct {
	Name string `json:"name"`
	Slug string `json:"slug"`
}

type checkSlugResponse struct {
	Slug      string `json:"slug"`
	Available bool   `json:"available"`
}

type teamResponse struct {
	ID             string `json:"id"`
	OrganizationID string `json:"organization_id"`
	Name           string `json:"name"`
	Slug           string `json:"slug"`
}

func toTeamResponse(t Team) teamResponse {
	return teamResponse{ID: t.ID.String(), OrganizationID: t.OrganizationID.String(), Name: t.Name, Slug: t.Slug}
}

type createTeamRequest struct {
	Name string `json:"name"`
	Slug string `json:"slug"`
}

type teamMemberResponse struct {
	UserID    string `json:"user_id"`
	Email     string `json:"email"`
	CreatedAt int64  `json:"created_at"`
}

type addTeamMemberRequest struct {
	UserID string `json:"user_id"`
}

type memberResponse struct {
	UserID    string `json:"user_id"`
	Email     string `json:"email"`
	Role      string `json:"role"`
	CreatedAt int64  `json:"created_at"`
}

func toMemberResponse(m MemberWithUser) memberResponse {
	return memberResponse{UserID: m.UserID.String(), Email: m.Email, Role: string(m.Role), CreatedAt: m.CreatedAt.Unix()}
}

type changeMemberRoleRequest struct {
	// Never accepts "owner" — same rule as addMemberRequest.Role.
	Role string `json:"role"`
}

type addDomainRequest struct {
	Domain string `json:"domain"`
}

type domainResponse struct {
	ID                string `json:"id"`
	Domain            string `json:"domain"`
	VerificationHost  string `json:"verification_host"`
	VerificationToken string `json:"verification_token"`
	Verified          bool   `json:"verified"`
	AutoJoinEnabled   bool   `json:"auto_join_enabled"`
}

type setAutoJoinRequest struct {
	Enabled bool `json:"enabled"`
}

type createInvitationRequest struct {
	Email string `json:"email"`
	// Optional; defaults to "member". Never accepts "owner" — ownership is
	// set at creation only, same rule as everywhere else in this package.
	Role string `json:"role"`
}

type invitationResponse struct {
	ID             string `json:"id"`
	OrganizationID string `json:"organization_id"`
	Email          string `json:"email"`
	Role           string `json:"role"`
	Status         string `json:"status"`
	Expired        bool   `json:"expired"`
	ExpiresAt      int64  `json:"expires_at"`
	CreatedAt      int64  `json:"created_at"`
}

func toInvitationResponse(inv Invitation) invitationResponse {
	return invitationResponse{
		ID:             inv.ID.String(),
		OrganizationID: inv.OrganizationID.String(),
		Email:          inv.Email,
		Role:           string(inv.Role),
		Status:         string(inv.Status),
		Expired:        inv.IsExpired(),
		ExpiresAt:      inv.ExpiresAt.Unix(),
		CreatedAt:      inv.CreatedAt.Unix(),
	}
}

// domainPattern is deliberately conservative: lowercase letters, digits,
// hyphens, and dots, at least one dot, no leading/trailing hyphen or dot on
// any label. A domain that fails this was never going to have a valid DNS
// record checked against it, so reject it before attempting a lookup.
var domainPattern = regexp.MustCompile(`^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$`)

// slugPattern matches lowercase alphanumeric segments separated by single
// hyphens — a normal URL-safe slug, nothing that could be misread as a path
// segment boundary or need escaping.
var slugPattern = regexp.MustCompile(`^[a-z0-9]+(-[a-z0-9]+)*$`)

// ── Handlers ──────────────────────────────────────────────────────────────────

func (h *Handler) createOrganization(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	var req createOrganizationRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.Name = strings.TrimSpace(req.Name)
	req.Slug = strings.TrimSpace(strings.ToLower(req.Slug))
	if req.Name == "" {
		writeError(w, http.StatusBadRequest, "name is required")
		return
	}
	if !slugPattern.MatchString(req.Slug) {
		writeError(w, http.StatusBadRequest, "slug must be lowercase letters, numbers, and single hyphens")
		return
	}

	org, err := h.repo.Create(r.Context(), req.Name, req.Slug, userID)
	if err != nil {
		if errors.Is(err, ErrSlugTaken) {
			writeError(w, http.StatusConflict, "that slug is already taken")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not create organization")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(w).Encode(toOrganizationResponse(org))
}

func (h *Handler) checkSlug(w http.ResponseWriter, r *http.Request) {
	if userIDFromContext(r) == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	slug := strings.TrimSpace(strings.ToLower(r.URL.Query().Get("slug")))
	if !slugPattern.MatchString(slug) {
		writeError(w, http.StatusBadRequest, "slug must be lowercase letters, numbers, and single hyphens")
		return
	}
	available, err := h.repo.IsSlugAvailable(r.Context(), slug)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not check slug")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(checkSlugResponse{Slug: slug, Available: available})
}

func (h *Handler) listMyOrganizations(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	orgs, err := h.repo.ListOrganizationsForUser(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list organizations")
		return
	}
	resp := make([]organizationWithRoleResponse, 0, len(orgs))
	for _, o := range orgs {
		resp = append(resp, toOrganizationWithRoleResponse(o))
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func (h *Handler) getOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	org, err := h.repo.GetOrganization(r.Context(), orgID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "organization not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not fetch organization")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(toOrganizationResponse(org))
}

func (h *Handler) getFullOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	full, err := h.repo.GetFullOrganization(r.Context(), orgID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "organization not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not fetch organization")
		return
	}

	members := make([]memberResponse, 0, len(full.Members))
	for _, m := range full.Members {
		members = append(members, toMemberResponse(m))
	}
	teams := make([]teamResponse, 0, len(full.Teams))
	for _, t := range full.Teams {
		teams = append(teams, toTeamResponse(t))
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(fullOrganizationResponse{
		Organization: toOrganizationResponse(full.Organization),
		Members:      members,
		Teams:        teams,
	})
}

func (h *Handler) updateOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	var req updateOrganizationRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.Name = strings.TrimSpace(req.Name)
	req.Slug = strings.TrimSpace(strings.ToLower(req.Slug))
	if req.Name == "" {
		writeError(w, http.StatusBadRequest, "name is required")
		return
	}
	if !slugPattern.MatchString(req.Slug) {
		writeError(w, http.StatusBadRequest, "slug must be lowercase letters, numbers, and single hyphens")
		return
	}

	org, err := h.repo.UpdateOrganization(r.Context(), orgID, req.Name, req.Slug)
	if err != nil {
		switch {
		case errors.Is(err, ErrSlugTaken):
			writeError(w, http.StatusConflict, "that slug is already taken")
		case errors.Is(err, sql.ErrNoRows):
			writeError(w, http.StatusNotFound, "organization not found")
		default:
			writeError(w, http.StatusInternalServerError, "could not update organization")
		}
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(toOrganizationResponse(org))
}

func (h *Handler) deleteOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	if err := h.repo.DeleteOrganization(r.Context(), orgID); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "organization not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not delete organization")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) setActiveOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	if err := h.repo.SetActiveOrganization(r.Context(), userID, orgID); err != nil {
		if errors.Is(err, ErrNotAMember) {
			writeError(w, http.StatusForbidden, "not a member of this organization")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not set active organization")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) getActiveOrganization(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	org, err := h.repo.GetActiveOrganization(r.Context(), userID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			// Genuinely no organizations — shouldn't happen (signup always
			// creates one, see InitializeTx), but not a server error either.
			writeError(w, http.StatusNotFound, "no organizations")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not get active organization")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(toOrganizationWithRoleResponse(org))
}

func (h *Handler) createInvitation(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	inviterID := userIDFromContext(r)
	if inviterID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	var req createInvitationRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.Email = strings.TrimSpace(strings.ToLower(req.Email))
	if req.Email == "" {
		writeError(w, http.StatusBadRequest, "email is required")
		return
	}

	role := MembershipRole(strings.TrimSpace(strings.ToLower(req.Role)))
	if role == "" {
		role = RoleMember
	}
	if role == RoleOwner {
		writeError(w, http.StatusBadRequest, "ownership cannot be granted by invitation")
		return
	}
	if role != RoleMember && role != RoleAdmin {
		writeError(w, http.StatusBadRequest, "role must be \"member\" or \"admin\"")
		return
	}

	inv, err := h.repo.CreateInvitation(r.Context(), orgID, req.Email, role, inviterID)
	if err != nil {
		switch {
		case errors.Is(err, ErrInvitationPending):
			writeError(w, http.StatusConflict, "an invitation to this email is already pending")
		case errors.Is(err, ErrOrgNotPaid):
			writeError(w, http.StatusPaymentRequired, "upgrade to a paid plan to invite teammates")
		default:
			writeError(w, http.StatusInternalServerError, "could not create invitation")
		}
		return
	}

	h.sendInvitationEmail(r.Context(), inv, inviterID)

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(w).Encode(toInvitationResponse(inv))
}

// sendInvitationEmail is best-effort: the invitation itself already exists in
// the database and was returned to the caller as a 201 by the time this
// runs, so a slow or failing send must never turn a successful invite into
// an error response — logged, not surfaced.
func (h *Handler) sendInvitationEmail(ctx context.Context, inv Invitation, inviterID uuid.UUID) {
	if h.mailer == nil {
		return
	}
	org, err := h.repo.GetOrganization(ctx, inv.OrganizationID)
	if err != nil {
		slog.Error("invitation email: could not load organization", "org_id", inv.OrganizationID, "error", err)
		return
	}
	inviterName := h.userDisplayName(ctx, inviterID, "A teammate")
	if err := h.mailer.SendOrganizationInvitation(ctx, inv.Email, org.Name, inviterName, string(inv.Role),
		h.invitationAcceptURL(inv.ID), inv.ExpiresAt); err != nil {
		slog.Error("failed to send organization invitation email", "invitation_id", inv.ID, "error", err)
	}
}

// userDisplayName resolves a user's display name for notification copy,
// preferring DisplayName over Email, falling back to `fallback` if the
// lookup itself fails (never blocks the caller's real action over this).
func (h *Handler) userDisplayName(ctx context.Context, userID uuid.UUID, fallback string) string {
	user, err := h.users.GetByID(ctx, userID)
	if err != nil {
		return fallback
	}
	if user.DisplayName != "" {
		return user.DisplayName
	}
	return user.Email
}

// sendMemberAddedEmail is best-effort, same reasoning as sendInvitationEmail
// — the membership itself already exists by the time this runs.
func (h *Handler) sendMemberAddedEmail(ctx context.Context, orgID, memberID, inviterID uuid.UUID, role MembershipRole) {
	if h.mailer == nil {
		return
	}
	org, err := h.repo.GetOrganization(ctx, orgID)
	if err != nil {
		slog.Error("member-added email: could not load organization", "org_id", orgID, "error", err)
		return
	}
	member, err := h.users.GetByID(ctx, memberID)
	if err != nil {
		slog.Error("member-added email: could not load member", "user_id", memberID, "error", err)
		return
	}
	addedByName := h.userDisplayName(ctx, inviterID, "A teammate")
	memberName := member.DisplayName
	if memberName == "" {
		memberName = member.Email
	}
	if err := h.mailer.SendMemberAdded(ctx, member.Email, memberName, org.Name, string(role), addedByName); err != nil {
		slog.Error("failed to send member-added email", "org_id", orgID, "user_id", memberID, "error", err)
	}
}

// sendMemberRemovedEmail is best-effort, same reasoning as
// sendInvitationEmail — the removal itself already succeeded by the time
// this runs. Only for an actual admin/owner-initiated removal, not a
// voluntary self-leave (see removeMember vs leaveOrganization) — a "you were
// removed by yourself" email would just be confusing noise.
func (h *Handler) sendMemberRemovedEmail(ctx context.Context, orgID, memberID, removedByID uuid.UUID) {
	if h.mailer == nil {
		return
	}
	org, err := h.repo.GetOrganization(ctx, orgID)
	if err != nil {
		slog.Error("member-removed email: could not load organization", "org_id", orgID, "error", err)
		return
	}
	member, err := h.users.GetByID(ctx, memberID)
	if err != nil {
		slog.Error("member-removed email: could not load member", "user_id", memberID, "error", err)
		return
	}
	removedByName := h.userDisplayName(ctx, removedByID, "an organization admin")
	memberName := member.DisplayName
	if memberName == "" {
		memberName = member.Email
	}
	if err := h.mailer.SendMemberRemoved(ctx, member.Email, memberName, org.Name, removedByName); err != nil {
		slog.Error("failed to send member-removed email", "org_id", orgID, "user_id", memberID, "error", err)
	}
}

func (h *Handler) listInvitations(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}

	invs, err := h.repo.ListInvitations(r.Context(), orgID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list invitations")
		return
	}

	resp := make([]invitationResponse, 0, len(invs))
	for _, inv := range invs {
		resp = append(resp, toInvitationResponse(inv))
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func (h *Handler) cancelInvitation(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	invitationID, err := uuid.Parse(chi.URLParam(r, "invitationID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid invitation id")
		return
	}

	if err := h.repo.CancelInvitation(r.Context(), orgID, invitationID); err != nil {
		switch {
		case errors.Is(err, sql.ErrNoRows):
			writeError(w, http.StatusNotFound, "invitation not found")
		case errors.Is(err, ErrInvitationNotPending):
			writeError(w, http.StatusBadRequest, "invitation is no longer pending")
		default:
			writeError(w, http.StatusInternalServerError, "could not cancel invitation")
		}
		return
	}

	w.WriteHeader(http.StatusNoContent)
}

// acceptInvitation and rejectInvitation are deliberately NOT org-gated
// (see RegisterRoutes) — the caller isn't a member of the target
// organization yet. Authorization here is "does this account own the
// email the invitation was sent to," checked inside the repo methods via
// accountEmail, not org membership.
func (h *Handler) acceptInvitation(w http.ResponseWriter, r *http.Request) {
	invitationID, err := uuid.Parse(chi.URLParam(r, "invitationID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid invitation id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	account, err := h.users.GetByID(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not look up your account")
		return
	}

	inv, err := h.repo.AcceptInvitation(r.Context(), invitationID, userID, account.Email)
	if err != nil {
		writeInvitationActionError(w, err)
		return
	}
	// Provision the new member's entitlement if the org is paid. Never
	// gated on this succeeding — the membership above already committed,
	// and GrantSeatIfPaid logs rather than errors (§3.1: access and billing
	// are deliberately decoupled).
	if h.billing != nil {
		h.billing.GrantSeatIfPaid(r.Context(), inv.OrganizationID, userID)
		memberName := account.DisplayName
		if memberName == "" {
			memberName = account.Email
		}
		h.billing.NotifyMemberAddedBilling(r.Context(), inv.OrganizationID, memberName)
	}
	h.sendMemberAddedEmail(r.Context(), inv.OrganizationID, userID, inv.InviterID, inv.Role)

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(toInvitationResponse(inv))
}

func (h *Handler) rejectInvitation(w http.ResponseWriter, r *http.Request) {
	invitationID, err := uuid.Parse(chi.URLParam(r, "invitationID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid invitation id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	account, err := h.users.GetByID(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not look up your account")
		return
	}

	if err := h.repo.RejectInvitation(r.Context(), invitationID, account.Email); err != nil {
		writeInvitationActionError(w, err)
		return
	}

	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) listMyInvitations(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	account, err := h.users.GetByID(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not look up your account")
		return
	}

	invs, err := h.repo.ListInvitationsForEmail(r.Context(), account.Email)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list invitations")
		return
	}

	resp := make([]invitationResponse, 0, len(invs))
	for _, inv := range invs {
		resp = append(resp, toInvitationResponse(inv))
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func writeInvitationActionError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, sql.ErrNoRows):
		writeError(w, http.StatusNotFound, "invitation not found")
	case errors.Is(err, ErrInvitationEmailMismatch):
		writeError(w, http.StatusForbidden, "this invitation was not sent to your account's email")
	case errors.Is(err, ErrInvitationNotPending):
		writeError(w, http.StatusBadRequest, "invitation is no longer pending")
	case errors.Is(err, ErrInvitationExpired):
		writeError(w, http.StatusBadRequest, "invitation has expired")
	case errors.Is(err, ErrOrgNotPaid):
		// Only reachable from AcceptInvitation (RejectInvitation never calls
		// requireOrgPaid) — the inviting org's subscription lapsed between
		// the invite being sent and this accept.
		writeError(w, http.StatusPaymentRequired, "this organization is no longer on a paid plan")
	default:
		writeError(w, http.StatusInternalServerError, "could not process invitation")
	}
}

type domainPromptEligibilityResponse struct {
	Domain   string `json:"domain"`
	Eligible bool   `json:"eligible"`
}

// domainPromptEligibility answers whether the calling user should see the
// dismissible "verify your company's domain?" prompt (11.A.9). Backend half
// of that feature only — deciding when to show it, and dismissal state, is
// the desktop client's job. Derives the domain from the caller's OWN email
// via their authenticated user id, never from a client-supplied domain
// parameter — accepting an arbitrary domain here would let anyone probe
// whether a given company domain is already claimed.
func (h *Handler) domainPromptEligibility(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}

	user, err := h.users.GetByID(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not look up user")
		return
	}

	domain := emailDomain(user.Email)
	if domain == "" || isPublicEmailDomain(domain) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(domainPromptEligibilityResponse{Domain: domain, Eligible: false})
		return
	}

	claimed, err := h.repo.IsDomainClaimed(r.Context(), domain)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not check domain")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(domainPromptEligibilityResponse{Domain: domain, Eligible: !claimed})
}

func (h *Handler) addDomain(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}

	var req addDomainRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.Domain = strings.TrimSpace(strings.ToLower(req.Domain))
	if !domainPattern.MatchString(req.Domain) {
		writeError(w, http.StatusBadRequest, "not a valid domain")
		return
	}
	if isPublicEmailDomain(req.Domain) {
		writeError(w, http.StatusBadRequest, "public email providers cannot be claimed")
		return
	}

	d, err := h.repo.AddDomain(r.Context(), orgID, req.Domain)
	if err != nil {
		if errors.Is(err, ErrDomainTaken) {
			writeError(w, http.StatusConflict, "that domain is already submitted")
			return
		}
		if errors.Is(err, ErrOrgNotPaid) {
			writeError(w, http.StatusPaymentRequired, "upgrade to a paid plan to claim a domain")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not add domain")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(w).Encode(domainResponse{
		ID:                d.ID.String(),
		Domain:            d.Domain,
		VerificationHost:  d.VerificationHost(),
		VerificationToken: d.VerificationToken,
		Verified:          false,
		AutoJoinEnabled:   false,
	})
}

func (h *Handler) verifyDomain(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	domainID, err := uuid.Parse(chi.URLParam(r, "domainID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid domain id")
		return
	}

	d, err := h.repo.GetDomain(r.Context(), orgID, domainID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "domain not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not look up domain")
		return
	}

	verified, err := h.repo.VerifyDomain(r.Context(), d, h.dnsLookup)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "verification check failed")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(domainResponse{
		ID:                d.ID.String(),
		Domain:            d.Domain,
		VerificationHost:  d.VerificationHost(),
		VerificationToken: d.VerificationToken,
		Verified:          verified,
		AutoJoinEnabled:   d.AutoJoinEnabled,
	})
}

func (h *Handler) setAutoJoin(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	domainID, err := uuid.Parse(chi.URLParam(r, "domainID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid domain id")
		return
	}

	var req setAutoJoinRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	if err := h.repo.SetAutoJoin(r.Context(), orgID, domainID, req.Enabled); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "domain not found")
			return
		}
		if errors.Is(err, ErrDomainNotVerified) {
			writeError(w, http.StatusBadRequest, "domain must be verified before auto-join can be enabled")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not update auto-join")
		return
	}

	d, err := h.repo.GetDomain(r.Context(), orgID, domainID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "auto-join updated, but could not fetch domain state")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(domainResponse{
		ID:                d.ID.String(),
		Domain:            d.Domain,
		VerificationHost:  d.VerificationHost(),
		VerificationToken: d.VerificationToken,
		Verified:          d.VerifiedAt != nil,
		AutoJoinEnabled:   d.AutoJoinEnabled,
	})
}

func (h *Handler) removeMember(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	targetID, err := uuid.Parse(chi.URLParam(r, "userID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid user id")
		return
	}

	// Removing yourself is a different flow (leaving an organization) with
	// different consequences for the request doing the removing — not built
	// here, and not something this endpoint should do implicitly.
	if targetID == userIDFromContext(r) {
		writeError(w, http.StatusBadRequest, "cannot remove yourself through this endpoint")
		return
	}

	if err := h.repo.RemoveMember(r.Context(), orgID, targetID); err != nil {
		switch {
		case errors.Is(err, sql.ErrNoRows):
			writeError(w, http.StatusNotFound, "not a member of this organization")
		case errors.Is(err, ErrCannotModifyOwner):
			writeError(w, http.StatusBadRequest, "ownership cannot be changed through member management")
		default:
			writeError(w, http.StatusInternalServerError, "could not remove member")
		}
		return
	}
	if h.billing != nil {
		h.billing.RemoveMemberEntitlement(r.Context(), orgID, targetID)
		h.billing.NotifyMemberRemovedBilling(r.Context(), orgID, h.userDisplayName(r.Context(), targetID, "a team member"))
	}
	h.sendMemberRemovedEmail(r.Context(), orgID, targetID, userIDFromContext(r))

	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) changeMemberRole(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	targetID, err := uuid.Parse(chi.URLParam(r, "userID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid user id")
		return
	}
	if targetID == userIDFromContext(r) {
		writeError(w, http.StatusBadRequest, "cannot change your own role through this endpoint")
		return
	}

	var req changeMemberRoleRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	role := MembershipRole(strings.TrimSpace(strings.ToLower(req.Role)))
	if role == RoleOwner {
		writeError(w, http.StatusBadRequest, "ownership cannot be granted by role change")
		return
	}
	if role != RoleMember && role != RoleAdmin {
		writeError(w, http.StatusBadRequest, "role must be \"member\" or \"admin\"")
		return
	}

	if err := h.repo.ChangeMemberRole(r.Context(), orgID, targetID, role); err != nil {
		switch {
		case errors.Is(err, sql.ErrNoRows):
			writeError(w, http.StatusNotFound, "not a member of this organization")
		case errors.Is(err, ErrCannotModifyOwner):
			writeError(w, http.StatusBadRequest, "ownership cannot be changed through member management")
		default:
			writeError(w, http.StatusInternalServerError, "could not change member role")
		}
		return
	}

	target, err := h.users.GetByID(r.Context(), targetID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "role changed, but could not fetch updated member")
		return
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(memberResponse{
		UserID: target.ID.String(),
		Email:  target.Email,
		Role:   string(role),
	})
}

func (h *Handler) leaveOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	if err := h.repo.LeaveOrganization(r.Context(), orgID, userID); err != nil {
		switch {
		case errors.Is(err, sql.ErrNoRows):
			writeError(w, http.StatusNotFound, "not a member of this organization")
		case errors.Is(err, ErrLastOwner):
			writeError(w, http.StatusBadRequest, "you are the last owner — transfer ownership or delete the organization instead")
		default:
			writeError(w, http.StatusInternalServerError, "could not leave organization")
		}
		return
	}
	if h.billing != nil {
		h.billing.RemoveMemberEntitlement(r.Context(), orgID, userID)
		// The cost-impact notification fires here too, unlike the personal
		// "you were removed" email (sendMemberRemovedEmail, removeMember
		// only) — a voluntary leave changes the org's bill exactly the same
		// as an admin-initiated removal, so owners/admins still need to
		// know, even though the departing member doesn't need to be told
		// they removed themselves.
		h.billing.NotifyMemberRemovedBilling(r.Context(), orgID, h.userDisplayName(r.Context(), userID, "a team member"))
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) getActiveMember(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	m, err := h.repo.GetActiveMember(r.Context(), userID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "no active organization membership")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not get active member")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(toMemberResponse(m))
}

func (h *Handler) listMembers(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}

	members, err := h.repo.ListMembers(r.Context(), orgID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list members")
		return
	}

	resp := make([]memberResponse, 0, len(members))
	for _, m := range members {
		resp = append(resp, memberResponse{
			UserID:    m.UserID.String(),
			Email:     m.Email,
			Role:      string(m.Role),
			CreatedAt: m.CreatedAt.Unix(),
		})
	}

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func (h *Handler) createTeam(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	var req createTeamRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.Name = strings.TrimSpace(req.Name)
	req.Slug = strings.TrimSpace(strings.ToLower(req.Slug))
	if req.Name == "" {
		writeError(w, http.StatusBadRequest, "name is required")
		return
	}
	if !slugPattern.MatchString(req.Slug) {
		writeError(w, http.StatusBadRequest, "slug must be lowercase letters, numbers, and single hyphens")
		return
	}

	team, err := h.repo.CreateTeam(r.Context(), orgID, req.Name, req.Slug)
	if err != nil {
		switch {
		case errors.Is(err, ErrSlugTaken):
			writeError(w, http.StatusConflict, "that slug is already taken in this organization")
		case errors.Is(err, ErrOrgNotPaid):
			writeError(w, http.StatusPaymentRequired, "upgrade to a paid plan to create teams")
		default:
			writeError(w, http.StatusInternalServerError, "could not create team")
		}
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(w).Encode(toTeamResponse(team))
}

func (h *Handler) listTeams(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	teams, err := h.repo.ListTeams(r.Context(), orgID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list teams")
		return
	}
	resp := make([]teamResponse, 0, len(teams))
	for _, t := range teams {
		resp = append(resp, toTeamResponse(t))
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func (h *Handler) updateTeam(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	teamID, err := uuid.Parse(chi.URLParam(r, "teamID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid team id")
		return
	}
	var req createTeamRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.Name = strings.TrimSpace(req.Name)
	req.Slug = strings.TrimSpace(strings.ToLower(req.Slug))
	if req.Name == "" {
		writeError(w, http.StatusBadRequest, "name is required")
		return
	}
	if !slugPattern.MatchString(req.Slug) {
		writeError(w, http.StatusBadRequest, "slug must be lowercase letters, numbers, and single hyphens")
		return
	}

	team, err := h.repo.UpdateTeam(r.Context(), orgID, teamID, req.Name, req.Slug)
	if err != nil {
		switch {
		case errors.Is(err, ErrSlugTaken):
			writeError(w, http.StatusConflict, "that slug is already taken in this organization")
		case errors.Is(err, sql.ErrNoRows):
			writeError(w, http.StatusNotFound, "team not found")
		default:
			writeError(w, http.StatusInternalServerError, "could not update team")
		}
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(toTeamResponse(team))
}

func (h *Handler) removeTeam(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	teamID, err := uuid.Parse(chi.URLParam(r, "teamID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid team id")
		return
	}
	if err := h.repo.RemoveTeam(r.Context(), orgID, teamID); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "team not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not remove team")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) listTeamMembers(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	teamID, err := uuid.Parse(chi.URLParam(r, "teamID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid team id")
		return
	}
	members, err := h.repo.ListTeamMembers(r.Context(), orgID, teamID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list team members")
		return
	}
	resp := make([]teamMemberResponse, 0, len(members))
	for _, m := range members {
		resp = append(resp, teamMemberResponse{UserID: m.UserID.String(), Email: m.Email, CreatedAt: m.CreatedAt.Unix()})
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func (h *Handler) addTeamMember(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	teamID, err := uuid.Parse(chi.URLParam(r, "teamID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid team id")
		return
	}
	var req addTeamMemberRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	targetID, err := uuid.Parse(strings.TrimSpace(req.UserID))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid user id")
		return
	}

	if err := h.repo.AddTeamMember(r.Context(), orgID, teamID, targetID); err != nil {
		switch {
		case errors.Is(err, ErrNotAMember):
			writeError(w, http.StatusBadRequest, "user must be an organization member before joining a team")
		case errors.Is(err, ErrAlreadyTeamMember):
			writeError(w, http.StatusConflict, "already on this team")
		default:
			writeError(w, http.StatusInternalServerError, "could not add team member")
		}
		return
	}
	w.WriteHeader(http.StatusCreated)
}

func (h *Handler) removeTeamMember(w http.ResponseWriter, r *http.Request) {
	teamID, err := uuid.Parse(chi.URLParam(r, "teamID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid team id")
		return
	}
	targetID, err := uuid.Parse(chi.URLParam(r, "userID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid user id")
		return
	}
	if err := h.repo.RemoveTeamMember(r.Context(), teamID, targetID); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "not on this team")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not remove team member")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) setActiveTeam(w http.ResponseWriter, r *http.Request) {
	teamID, err := uuid.Parse(chi.URLParam(r, "teamID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid team id")
		return
	}
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	if err := h.repo.SetActiveTeam(r.Context(), userID, teamID); err != nil {
		if errors.Is(err, ErrNotAMember) {
			writeError(w, http.StatusForbidden, "not on this team")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not set active team")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) listMyTeams(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	teams, err := h.repo.ListUserTeams(r.Context(), userID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "could not list teams")
		return
	}
	resp := make([]teamResponse, 0, len(teams))
	for _, t := range teams {
		resp = append(resp, toTeamResponse(t))
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

// ── Billing ───────────────────────────────────────────────────────────────────

type subscribeOrganizationRequest struct {
	PlanID string `json:"plan_id"`
}

type organizationBillingResponse struct {
	Enabled              bool    `json:"enabled"`
	IsPaid               bool    `json:"is_paid"`
	PlanID               string  `json:"plan_id"`
	Tier                 string  `json:"tier"`
	MonthlyAllowanceMc   int64   `json:"monthly_allowance_mc"`
	SubscriptionStatus   string  `json:"subscription_status"`
	SubscriptionRenewsAt *string `json:"subscription_renews_at,omitempty"`
}

func (h *Handler) subscribeOrganization(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	if h.billing == nil {
		writeError(w, http.StatusServiceUnavailable, "organization billing is not available")
		return
	}
	var req subscribeOrganizationRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4<<10)).Decode(&req); err != nil || req.PlanID == "" {
		writeError(w, http.StatusBadRequest, "plan_id is required")
		return
	}
	url, err := h.billing.Subscribe(r.Context(), orgID, req.PlanID)
	if err != nil {
		switch {
		case errors.Is(err, ErrOrgBillingDisabled):
			writeError(w, http.StatusServiceUnavailable, "organization billing is not available")
		case errors.Is(err, ErrOrgAlreadySubscribed):
			writeError(w, http.StatusConflict, err.Error())
		default:
			writeError(w, http.StatusBadGateway, "subscribe: "+err.Error())
		}
		return
	}
	writeJSONBody(w, http.StatusOK, map[string]string{"url": url})
}

func (h *Handler) organizationBillingPortal(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	if h.billing == nil {
		writeError(w, http.StatusServiceUnavailable, "organization billing is not available")
		return
	}
	url, err := h.billing.PortalURL(r.Context(), orgID)
	if err != nil {
		if errors.Is(err, ErrOrgBillingDisabled) {
			writeError(w, http.StatusServiceUnavailable, "organization billing is not available")
			return
		}
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	writeJSONBody(w, http.StatusOK, map[string]string{"url": url})
}

func (h *Handler) getOrganizationBilling(w http.ResponseWriter, r *http.Request) {
	orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid organization id")
		return
	}
	billing, err := h.repo.GetOrgBilling(r.Context(), orgID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			writeError(w, http.StatusNotFound, "organization not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "could not load billing state")
		return
	}
	resp := organizationBillingResponse{
		Enabled:            h.billing.Enabled(),
		IsPaid:             billing.IsPaid(),
		PlanID:             billing.PlanID,
		Tier:               billing.Tier,
		MonthlyAllowanceMc: billing.MonthlyAllowanceMc,
		SubscriptionStatus: billing.SubscriptionStatus,
	}
	if billing.SubscriptionRenewsAt != nil {
		s := billing.SubscriptionRenewsAt.Format(time.RFC3339)
		resp.SubscriptionRenewsAt = &s
	}
	writeJSONBody(w, http.StatusOK, resp)
}

func writeJSONBody(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

// ── Context helpers ───────────────────────────────────────────────────────────

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}

func writeError(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}
