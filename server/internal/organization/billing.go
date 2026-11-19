package organization

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"log"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"server/internal/billing/payments"
)

// OrgBilling mirrors the billing columns 00052_organization_billing.sql adds
// to organizations. A blank PlanID means the org is still on Free — no
// LemonSqueezy subscription has ever been created for it.
type OrgBilling struct {
	OrganizationID                 uuid.UUID
	PlanID                         string
	Tier                           string
	MonthlyAllowanceMc             int64
	LemonSqueezyCustomerID         string
	LemonSqueezySubscriptionID     string
	LemonSqueezySubscriptionItemID string
	SubscriptionStatus             string
	SubscriptionRenewsAt           *time.Time
}

// IsPaid reports whether the org currently has an active tier selected.
// This — not presence of a subscription id — is the source of truth: an
// ended subscription clears PlanID but keeps the provider ids for history
// (see endOrgSubscription), so checking PlanID is what actually
// distinguishes "paid now" from "was paid, no longer is."
func (b OrgBilling) IsPaid() bool { return b.PlanID != "" }

// LifecycleState is the billing history + current-status classification a
// dunning UI or a notification's wording needs — distinct from IsPaid()'s
// simple yes/no, which collapses "never subscribed" and "subscription
// lapsed" into the same false.
type LifecycleState string

const (
	// LifecycleNeverSubscribed: always been Free — no subscription has ever
	// existed for this org.
	LifecycleNeverSubscribed LifecycleState = "never_subscribed"
	// LifecycleActive: currently paid and healthy (includes on_trial).
	LifecycleActive LifecycleState = "active"
	// LifecyclePastDue: currently paid — LemonSqueezy is mid-retry after a
	// failed renewal charge. Access continues (see lemonsqueezy.go's
	// activeStatuses, which treats "past_due" as still active), but the org
	// should be warned before the retries run out.
	LifecyclePastDue LifecycleState = "past_due"
	// LifecycleLapsed: WAS paid, no longer is — retries exhausted, the
	// subscription was cancelled, or a payment was refunded.
	LifecycleLapsed LifecycleState = "lapsed"
)

// LifecycleState classifies the org's billing history and current status.
// Every input is a column GetOrgBilling already reads — no schema change
// needed, this is pure derived logic.
//
// Mirrors a distinction the reference billing UI makes explicitly
// (packages/trpc/src/router/billing/billing.ts's "lapsed" flag, checked
// directly against their source 2026-09-21): its dunning banner
// (usePaymentFailedCard.ts) carries a comment documenting a real incident
// where keying the banner on "past_due" alone made it vanish the instant a
// subscription was fully cancelled — silently dropping the warning for 87
// orgs instead of switching it to a "resubscribe" message. Checking
// LemonSqueezySubscriptionID != "" (an ended subscription's id is kept for
// history, see endOrgSubscription) — not just IsPaid() — is what makes
// LifecycleLapsed distinguishable from LifecycleNeverSubscribed here.
func (b OrgBilling) LifecycleState() LifecycleState {
	switch {
	case b.IsPaid() && b.SubscriptionStatus == "past_due":
		return LifecyclePastDue
	case b.IsPaid():
		return LifecycleActive
	case b.LemonSqueezySubscriptionID != "":
		return LifecycleLapsed
	default:
		return LifecycleNeverSubscribed
	}
}

// ErrOrgNotPaid is returned by any team-collaboration action (invite a
// member, create a team, claim a domain) attempted on an organization still
// on Free. Per ORGANIZATION_GAPS_AND_OPEN_QUESTIONS.md §3.1: Free is
// local-only, with no remote sharing, automations, mobile app usage,
// integrations, or TEAM COLLABORATION at all — every paid tier (Pro/Max/
// Ultra) gets identical collaboration features, so the only distinction
// that matters here is free vs. paid, never tier vs. tier.
var ErrOrgNotPaid = errors.New("organization must be on a paid plan to use team collaboration features")

// requireOrgPaid enforces ErrOrgNotPaid. Deliberately a repository-layer
// check, not just a handler-side one — the same defense-in-depth already
// used for ownership protection (RemoveMember/ChangeMemberRole's WHERE
// role != 'owner') — so no future caller can add a collaboration feature
// that accidentally skips the gate.
func (r *PostgresRepository) requireOrgPaid(ctx context.Context, orgID uuid.UUID) error {
	billing, err := r.GetOrgBilling(ctx, orgID)
	if err != nil {
		return fmt.Errorf("check organization plan: %w", err)
	}
	if !billing.IsPaid() {
		return ErrOrgNotPaid
	}
	return nil
}

// GetOrgBilling reads an organization's current billing state.
func (r *PostgresRepository) GetOrgBilling(ctx context.Context, orgID uuid.UUID) (OrgBilling, error) {
	b := OrgBilling{OrganizationID: orgID}
	err := r.pool.QueryRow(ctx, `
		SELECT COALESCE(plan_id, ''), tier, monthly_allowance_mc,
		       COALESCE(lemonsqueezy_customer_id, ''), COALESCE(lemonsqueezy_subscription_id, ''),
		       COALESCE(lemonsqueezy_subscription_item_id, ''), subscription_status, subscription_renews_at
		FROM organizations WHERE id = $1
	`, orgID).Scan(&b.PlanID, &b.Tier, &b.MonthlyAllowanceMc, &b.LemonSqueezyCustomerID,
		&b.LemonSqueezySubscriptionID, &b.LemonSqueezySubscriptionItemID, &b.SubscriptionStatus, &b.SubscriptionRenewsAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return OrgBilling{}, sql.ErrNoRows
		}
		return OrgBilling{}, err
	}
	return b, nil
}

// setOrgPlan records the tier an admin chose, ahead of (or independent of)
// any checkout completing — lets a fast subscription_created webhook, or the
// optimistic seat grant in BillingService.Subscribe, resolve tier/allowance
// immediately rather than waiting on a round trip.
func (r *PostgresRepository) setOrgPlan(ctx context.Context, orgID uuid.UUID, planID, tier string, monthlyAllowanceMc int64) error {
	tag, err := r.pool.Exec(ctx, `
		UPDATE organizations SET plan_id = $2, tier = $3, monthly_allowance_mc = $4, updated_at = NOW()
		WHERE id = $1
	`, orgID, planID, tier, monthlyAllowanceMc)
	if err != nil {
		return fmt.Errorf("set org plan: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return sql.ErrNoRows
	}
	return nil
}

// activateOrgSubscription records a LemonSqueezy subscription against an
// org — called from the subscription_created/updated webhook path. Customer
// id and subscription-item id are COALESCEd rather than overwritten: only
// subscription_created's payload reliably carries first_subscription_item,
// so a later subscription_updated (card change, dunning) must not blank an
// id already recorded.
func (r *PostgresRepository) activateOrgSubscription(ctx context.Context, orgID uuid.UUID, planID, tier string, monthlyAllowanceMc int64, subscriptionID, customerID, subscriptionItemID, status string, renewsAt *time.Time) error {
	_, err := r.pool.Exec(ctx, `
		UPDATE organizations SET
			plan_id                           = $2,
			tier                               = $3,
			monthly_allowance_mc               = $4,
			lemonsqueezy_customer_id           = COALESCE(NULLIF($5, ''), lemonsqueezy_customer_id),
			lemonsqueezy_subscription_id       = $6,
			lemonsqueezy_subscription_item_id  = COALESCE(NULLIF($7, ''), lemonsqueezy_subscription_item_id),
			subscription_status                = $8,
			subscription_renews_at             = $9,
			-- A genuine rollover to a NEW renewal date clears the reminder
			-- flag so the next cycle gets its own reminder; a webhook that
			-- doesn't change renews_at (e.g. a dunning status flip) leaves
			-- whatever value is already there untouched.
			renewal_reminder_sent_at          = CASE WHEN subscription_renews_at IS DISTINCT FROM $9 THEN NULL ELSE renewal_reminder_sent_at END,
			updated_at                         = NOW()
		WHERE id = $1
	`, orgID, planID, tier, monthlyAllowanceMc, customerID, subscriptionID, subscriptionItemID, status, renewsAt)
	if err != nil {
		return fmt.Errorf("activate org subscription: %w", err)
	}
	return nil
}

// endOrgSubscription downgrades an org to Free. Provider ids are kept (not
// cleared) for portal/history, mirroring EndSubscription's same choice for
// individual billing — only the plan/tier/allowance that grant access are
// reset.
func (r *PostgresRepository) endOrgSubscription(ctx context.Context, orgID uuid.UUID, status string) error {
	_, err := r.pool.Exec(ctx, `
		UPDATE organizations SET
			plan_id = NULL, tier = 'free', monthly_allowance_mc = 0,
			subscription_status = $2, updated_at = NOW()
		WHERE id = $1
	`, orgID, status)
	if err != nil {
		return fmt.Errorf("end org subscription: %w", err)
	}
	return nil
}

// getOrgBySubscriptionID resolves an org from a LemonSqueezy subscription
// id — a defensive fallback for ApplyOrgEvent if a webhook ever arrives
// without organization_id in custom_data (shouldn't happen: LemonSqueezy
// round-trips the original checkout's custom_data on every subsequent event
// for the same subscription, the same mechanism individual billing already
// depends on).
func (r *PostgresRepository) getOrgBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
	var id uuid.UUID
	err := r.pool.QueryRow(ctx, `
		SELECT id FROM organizations WHERE lemonsqueezy_subscription_id = $1
	`, subscriptionID).Scan(&id)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return uuid.Nil, sql.ErrNoRows
		}
		return uuid.Nil, err
	}
	return id, nil
}

// memberCount is the org's current billed seat count — every membership,
// including the owner, counts as a billed seat in a paid org (no free
// riders inside a paid org, per §3.1's "org membership requires a paid
// seat").
func (r *PostgresRepository) memberCount(ctx context.Context, orgID uuid.UUID) (int, error) {
	var n int
	err := r.pool.QueryRow(ctx, `SELECT count(*) FROM memberships WHERE organization_id = $1`, orgID).Scan(&n)
	if err != nil {
		return 0, fmt.Errorf("count org members: %w", err)
	}
	return n, nil
}

// listMemberUserIDs returns every current member's user id — what a
// subscription-wide grant/revoke (initial Subscribe, a renewal, or
// EventSubscriptionEnded) needs to apply to.
func (r *PostgresRepository) listMemberUserIDs(ctx context.Context, orgID uuid.UUID) ([]uuid.UUID, error) {
	rows, err := r.pool.Query(ctx, `SELECT user_id FROM memberships WHERE organization_id = $1`, orgID)
	if err != nil {
		return nil, fmt.Errorf("list org member ids: %w", err)
	}
	defer rows.Close()
	var ids []uuid.UUID
	for rows.Next() {
		var id uuid.UUID
		if err := rows.Scan(&id); err != nil {
			return nil, fmt.Errorf("scan org member id: %w", err)
		}
		ids = append(ids, id)
	}
	return ids, rows.Err()
}

// ── Billing service ─────────────────────────────────────────────────────────

// PlanLookup resolves a purchasable plan (tier, monthly allowance, provider
// variant) — implemented by billing/payments' PostgresRepository, which
// already has both methods for individual billing.
type PlanLookup interface {
	GetPlanByID(ctx context.Context, id string) (*payments.Plan, error)
	GetPlanByVariant(ctx context.Context, variantID string) (*payments.Plan, error)
}

// SeatBalances provisions/revokes one member's individual AI-credit
// entitlement at an org's tier — implemented by billing/payments'
// PostgresRepository (GrantOrgSeat/RevokeOrgSeat/DeleteOrgSeat).
type SeatBalances interface {
	GrantOrgSeat(ctx context.Context, userID, orgID uuid.UUID, tier string, monthlyAllowanceMc int64, status string, renewsAt *time.Time) error
	RevokeOrgSeat(ctx context.Context, userID, orgID uuid.UUID) error
	DeleteOrgSeat(ctx context.Context, userID, orgID uuid.UUID) error
}

// ReceiptWriter records a permanent payment receipt — implemented by
// billing/payments' PostgresRepository. See payments.Receipt's doc comment
// for what belongs here.
type ReceiptWriter interface {
	RecordReceipt(ctx context.Context, r payments.Receipt) error
}

// BillingProvider is the subset of the payment provider org billing needs:
// checkout for an org's first paid seats, a quantity sync for every seat
// after that, and the hosted portal link. Implemented by billing/payments'
// LemonSqueezy client.
type BillingProvider interface {
	Name() string
	OrgCheckout(organizationID uuid.UUID, variantID string, quantity int, redirectURL string) (string, error)
	UpdateSeatQuantity(subscriptionItemID string, quantity int) error
	PortalURL(subscriptionID string) (string, error)
}

var (
	// ErrOrgBillingDisabled means no payment provider is configured — mirrors
	// payments.ErrDisabled for the individual path.
	ErrOrgBillingDisabled = errors.New("organization billing is not configured")
	// ErrOrgAlreadySubscribed is returned by Subscribe on a paid org — plan
	// changes (upgrade/downgrade an already-subscribed org) are a separate,
	// not-yet-built path; not needed for the routing this pass implements.
	ErrOrgAlreadySubscribed = errors.New("organization already has an active plan; changing tier is not yet supported")
)

// BillingService orchestrates org-level per-seat billing: choosing a tier,
// routing seat additions to checkout (the org's first paid seats) or a
// quantity PATCH (every seat after), applying subscription webhooks, and
// granting/revoking individual members' entitlements.
type BillingService struct {
	repo       *PostgresRepository
	plans      PlanLookup
	seats      SeatBalances
	provider   BillingProvider // nil when payments are disabled
	successURL string
	mailer     Mailer        // nil-safe: no notification sent if unset
	receipts   ReceiptWriter // nil-safe: no receipt written if unset
}

func NewBillingService(repo *PostgresRepository, plans PlanLookup, seats SeatBalances, provider BillingProvider, successURL string) *BillingService {
	return &BillingService{repo: repo, plans: plans, seats: seats, provider: provider, successURL: successURL}
}

// SetMailer wires billing lifecycle notification emails in. Optional,
// setter-after-construct, same pattern as Handler.SetMailer.
func (s *BillingService) SetMailer(m Mailer) { s.mailer = m }

// SetReceiptWriter wires permanent payment-receipt recording in. Optional,
// setter-after-construct, same pattern as SetMailer.
func (s *BillingService) SetReceiptWriter(r ReceiptWriter) { s.receipts = r }

// recordReceipt is best-effort — see payments.Service.recordReceipt's
// identical reasoning (a receipts-table failure must never turn an
// otherwise-successful charge into something worth retrying the whole
// webhook over).
func (s *BillingService) recordReceipt(ctx context.Context, r payments.Receipt) {
	if s.receipts == nil {
		return
	}
	if err := s.receipts.RecordReceipt(ctx, r); err != nil {
		log.Printf("[organization billing] record receipt (kind=%s, ref=%s): %v", r.Kind, r.ProviderReference, err)
	}
}

// Enabled reports whether a payment provider is configured. Nil-safe so a
// caller holding a possibly-nil *BillingService doesn't need its own check.
func (s *BillingService) Enabled() bool { return s != nil && s.provider != nil }

// Subscribe starts an org's paid subscription at planID. Only valid while
// the org isn't already paid (see ErrOrgAlreadySubscribed) — tier changes on
// an already-subscribed org are a separate path, not built in this pass.
// Every CURRENT member becomes a billed seat immediately: the quantity sent
// to checkout is today's member count, and each one is granted their
// entitlement optimistically before the checkout even completes — matching
// LemonSqueezy's own documented access guidance (§3.1) rather than waiting
// on a webhook.
// PortalURL returns the provider-hosted management link for an org's
// subscription — the org-billing equivalent of the individual billing path's
// (now retired) portal endpoint. Every subscription is an org subscription
// now, personal included (a personal org is just a 1-member org), so this is
// the only portal endpoint that exists going forward.
func (s *BillingService) PortalURL(ctx context.Context, orgID uuid.UUID) (string, error) {
	if !s.Enabled() {
		return "", ErrOrgBillingDisabled
	}
	billing, err := s.repo.GetOrgBilling(ctx, orgID)
	if err != nil {
		return "", fmt.Errorf("load org billing state: %w", err)
	}
	if billing.LemonSqueezySubscriptionID == "" {
		return "", fmt.Errorf("no active subscription")
	}
	return s.provider.PortalURL(billing.LemonSqueezySubscriptionID)
}

func (s *BillingService) Subscribe(ctx context.Context, orgID uuid.UUID, planID string) (checkoutURL string, err error) {
	if !s.Enabled() {
		return "", ErrOrgBillingDisabled
	}
	billing, err := s.repo.GetOrgBilling(ctx, orgID)
	if err != nil {
		return "", fmt.Errorf("load org billing state: %w", err)
	}
	if billing.IsPaid() {
		return "", ErrOrgAlreadySubscribed
	}
	plan, err := s.plans.GetPlanByID(ctx, planID)
	if err != nil {
		return "", fmt.Errorf("look up plan: %w", err)
	}
	if plan.LemonSqueezyVariantID == "" {
		return "", fmt.Errorf("plan %q is not available for purchase (no provider variant configured)", planID)
	}

	quantity, err := s.repo.memberCount(ctx, orgID)
	if err != nil {
		return "", err
	}
	if quantity < 1 {
		quantity = 1
	}

	if err := s.repo.setOrgPlan(ctx, orgID, plan.ID, plan.Tier, plan.MonthlyAllowanceMc); err != nil {
		return "", fmt.Errorf("record chosen plan: %w", err)
	}

	s.grantAllMembers(ctx, orgID, plan.Tier, plan.MonthlyAllowanceMc, "active", nil)

	return s.provider.OrgCheckout(orgID, plan.LemonSqueezyVariantID, quantity, s.successURL)
}

// GrantSeatIfPaid provisions one member's individual entitlement at the
// org's current tier, if the org is paid, and syncs the seat quantity with
// the provider. Called right after a membership is created (an invitation
// accepted). Never returns an error to its caller: access is granted
// optimistically and a failure here — balance or provider sync — is logged,
// not surfaced, so it can never block the membership itself succeeding
// (§3.1: "Access and billing are deliberately decoupled").
func (s *BillingService) GrantSeatIfPaid(ctx context.Context, orgID, userID uuid.UUID) {
	if !s.Enabled() {
		return
	}
	billing, err := s.repo.GetOrgBilling(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] load billing state for org %s: %v", orgID, err)
		return
	}
	if !billing.IsPaid() {
		return
	}
	if err := s.seats.GrantOrgSeat(ctx, userID, orgID, billing.Tier, billing.MonthlyAllowanceMc, "active", billing.SubscriptionRenewsAt); err != nil {
		log.Printf("[organization billing] grant seat for user %s in org %s: %v", userID, orgID, err)
	}
	s.syncSeatQuantity(ctx, orgID, billing)
}

// RemoveMemberEntitlement deletes exactly the (user, org) balance row when a
// member leaves or is removed from the org (isolation plan §4.1). Unlike
// GrantSeatIfPaid/the org-subscription-ended path, this is unconditional on
// paid status: even a Free-tier org's member can have accrued a
// lazily-created row for free-quota tracking, and that row is entitlement
// residue the instant the membership itself ends — a clean, unambiguous
// "you're out" state, not a lingering free balance that could confuse a
// later rejoin. If the org happens to be paid, also syncs the provider seat
// quantity down to the new member count.
func (s *BillingService) RemoveMemberEntitlement(ctx context.Context, orgID, userID uuid.UUID) {
	if s == nil {
		return
	}
	if err := s.seats.DeleteOrgSeat(ctx, userID, orgID); err != nil {
		log.Printf("[organization billing] delete entitlement for user %s in org %s: %v", userID, orgID, err)
	}
	if !s.Enabled() {
		return
	}
	billing, err := s.repo.GetOrgBilling(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] load billing state for org %s: %v", orgID, err)
		return
	}
	if !billing.IsPaid() {
		return
	}
	s.syncSeatQuantity(ctx, orgID, billing)
}

// NotifyMemberAddedBilling emails billing recipients (owners+admins) the
// estimated cost impact of a new paid seat. memberName is supplied by the
// caller (which already resolved the new member's account for its own
// plain member-added email) rather than looked up again here —
// BillingService has no UserLookup dependency, deliberately, to avoid a
// second, redundant user-repository dependency for one string.
// No-op for a free org (nothing costs anything) or once no mailer is wired.
func (s *BillingService) NotifyMemberAddedBilling(ctx context.Context, orgID uuid.UUID, memberName string) {
	if s.mailer == nil {
		return
	}
	_, plan, org, seatCount, recipients, ok := s.billingNotificationContext(ctx, orgID, "member-added")
	if !ok {
		return
	}
	newMonthlyCents := int64(seatCount) * int64(plan.PriceCents)
	for _, recipient := range recipients {
		if err := s.mailer.SendMemberAddedBilling(ctx, recipient.Email, org.Name, memberName, seatCount,
			int64(plan.PriceCents), newMonthlyCents, plan.Currency); err != nil {
			log.Printf("[organization billing] send member-added billing email to %s: %v", recipient.Email, err)
		}
	}
}

// NotifyMemberRemovedBilling is NotifyMemberAddedBilling's counterpart for a
// departing paid seat.
func (s *BillingService) NotifyMemberRemovedBilling(ctx context.Context, orgID uuid.UUID, memberName string) {
	if s.mailer == nil {
		return
	}
	_, plan, org, seatCount, recipients, ok := s.billingNotificationContext(ctx, orgID, "member-removed")
	if !ok {
		return
	}
	newMonthlyCents := int64(seatCount) * int64(plan.PriceCents)
	for _, recipient := range recipients {
		if err := s.mailer.SendMemberRemovedBilling(ctx, recipient.Email, org.Name, memberName, seatCount,
			int64(plan.PriceCents), newMonthlyCents, plan.Currency); err != nil {
			log.Printf("[organization billing] send member-removed billing email to %s: %v", recipient.Email, err)
		}
	}
}

// billingNotificationContext gathers the data both seat-change billing
// emails need — org billing state (and whether it's even paid), the current
// plan's list price, the org's name, its current seat count, and its
// billing recipients — so the two near-identical notify methods above don't
// each repeat five lookups. ok is false (nothing to send) for a free org or
// if any lookup fails; each failure is logged with `what` for context.
func (s *BillingService) billingNotificationContext(ctx context.Context, orgID uuid.UUID, what string) (
	billing OrgBilling, plan *payments.Plan, org Organization, seatCount int, recipients []MemberWithUser, ok bool,
) {
	billing, err := s.repo.GetOrgBilling(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] %s billing email: load billing state for org %s: %v", what, orgID, err)
		return
	}
	if !billing.IsPaid() {
		return // free org: no cost impact to report
	}
	plan, err = s.plans.GetPlanByID(ctx, billing.PlanID)
	if err != nil {
		log.Printf("[organization billing] %s billing email: load plan %q: %v", what, billing.PlanID, err)
		return
	}
	org, err = s.repo.GetOrganization(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] %s billing email: load org %s: %v", what, orgID, err)
		return
	}
	seatCount, err = s.repo.memberCount(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] %s billing email: count members for org %s: %v", what, orgID, err)
		return
	}
	recipients, err = s.repo.ListMembersByRole(ctx, orgID, RoleOwner, RoleAdmin)
	if err != nil {
		log.Printf("[organization billing] %s billing email: list recipients for org %s: %v", what, orgID, err)
		return
	}
	ok = true
	return
}

// syncSeatQuantity PATCHes the stored subscription-item's quantity to match
// the org's current member count. Best-effort: never rolls back the balance
// grant/revoke that triggered it (access and billing are decoupled).
func (s *BillingService) syncSeatQuantity(ctx context.Context, orgID uuid.UUID, billing OrgBilling) {
	if billing.LemonSqueezySubscriptionItemID == "" {
		// The subscription_created webhook hasn't landed yet (or this org
		// isn't yet subscribed by webhook confirmation) — nothing to PATCH.
		// The next grant/revoke, or the webhook's own activation pass, will
		// bring quantity in sync once the item id is known.
		return
	}
	count, err := s.repo.memberCount(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] count members for org %s: %v", orgID, err)
		return
	}
	if err := s.provider.UpdateSeatQuantity(billing.LemonSqueezySubscriptionItemID, count); err != nil {
		log.Printf("[organization billing] sync seat quantity for org %s: %v", orgID, err)
	}
}

// grantAllMembers applies a grant to every current member — used by
// Subscribe (initial optimistic grant) and ApplyOrgEvent (webhook-driven
// activation/renewal). Idempotent per member (GrantOrgSeat's own CASE WHEN),
// so re-running this on every webhook is always safe, including a renewal
// refreshing everyone's monthly allowance at once.
func (s *BillingService) grantAllMembers(ctx context.Context, orgID uuid.UUID, tier string, monthlyAllowanceMc int64, status string, renewsAt *time.Time) {
	members, err := s.repo.listMemberUserIDs(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] list members for org %s: %v", orgID, err)
		return
	}
	for _, userID := range members {
		if err := s.seats.GrantOrgSeat(ctx, userID, orgID, tier, monthlyAllowanceMc, status, renewsAt); err != nil {
			log.Printf("[organization billing] grant seat for user %s in org %s: %v", userID, orgID, err)
		}
	}
}

// revokeAllMembers is grantAllMembers' counterpart for
// EventSubscriptionEnded: the org's own subscription ended but every current
// member is still a member, so each row is downgraded to free via
// RevokeOrgSeat — not deleted. Deletion only happens on actual departure,
// see RemoveMemberEntitlement.
func (s *BillingService) revokeAllMembers(ctx context.Context, orgID uuid.UUID) {
	members, err := s.repo.listMemberUserIDs(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] list members for org %s: %v", orgID, err)
		return
	}
	for _, userID := range members {
		if err := s.seats.RevokeOrgSeat(ctx, userID, orgID); err != nil {
			log.Printf("[organization billing] revoke seat for user %s in org %s: %v", userID, orgID, err)
		}
	}
}

// ApplyOrgEvent implements payments.OrgApplier — the org half of the SAME
// webhook dispatch individual billing uses (payments.Service.applyEvent
// branches to this when WebhookEvent.OrganizationID is set). Errors here
// make the provider retry (the handler replies 5xx), same contract as
// individual billing's applyEvent.
func (s *BillingService) ApplyOrgEvent(ctx context.Context, ev *payments.WebhookEvent) error {
	orgID, err := s.resolveOrgID(ctx, ev)
	if err != nil {
		return err
	}

	switch ev.Type {
	case payments.EventSubscriptionActive:
		plan, perr := s.plans.GetPlanByVariant(ctx, ev.ProviderVariantID)
		if perr != nil {
			return fmt.Errorf("org webhook %s: variant %q has no plan: %w", ev.RawEventName, ev.ProviderVariantID, perr)
		}
		// Read BEFORE activating: EventSubscriptionActive fires on every
		// renewal/plan-change too (see lemonsqueezy.go), not just a genuine
		// first subscribe — the started-email must only fire on the latter,
		// matching the reference SubscriptionStartedEmail trigger
		// (onSubscriptionComplete, a one-time checkout-completed event).
		wasPaidBefore := false
		if before, berr := s.repo.GetOrgBilling(ctx, orgID); berr == nil {
			wasPaidBefore = before.IsPaid()
		}
		if err := s.repo.activateOrgSubscription(ctx, orgID, plan.ID, plan.Tier, plan.MonthlyAllowanceMc,
			ev.SubscriptionID, ev.CustomerID, ev.SubscriptionItemID, ev.Status, ev.RenewsAt); err != nil {
			return fmt.Errorf("activate org subscription: %w", err)
		}
		s.grantAllMembers(ctx, orgID, plan.Tier, plan.MonthlyAllowanceMc, ev.Status, ev.RenewsAt)
		if !wasPaidBefore {
			s.notifySubscriptionStarted(ctx, orgID, plan.DisplayName)
		}
		return nil
	case payments.EventSubscriptionEnded:
		// Read BEFORE ending: endOrgSubscription clears Tier/PlanID, and the
		// cancellation email needs to name what just ended.
		planName := "your subscription"
		if before, berr := s.repo.GetOrgBilling(ctx, orgID); berr == nil && before.Tier != "" {
			planName = before.Tier
		}
		if err := s.repo.endOrgSubscription(ctx, orgID, ev.Status); err != nil {
			return fmt.Errorf("end org subscription: %w", err)
		}
		s.revokeAllMembers(ctx, orgID)
		// "expired" is LemonSqueezy's own event name for "dunning retries
		// exhausted" — the nearest available signal to the reference implementation's
		// live-status check for dueToPaymentFailure (LS's webhook payload
		// carries no richer reason code than the event name itself).
		s.notifySubscriptionCancelled(ctx, orgID, planName, ev.RawEventName == "subscription_expired")
		// subscription_payment_refunded is the one EventSubscriptionEnded
		// cause that's ALSO an invoice-shaped event carrying a real charged
		// amount — the others (cancelled/expired/paused) are pure
		// entitlement-lifecycle events with nothing to receipt.
		if ev.RawEventName == "subscription_payment_refunded" {
			s.recordReceipt(ctx, payments.Receipt{
				OrganizationID: &orgID, AmountCents: ev.AmountCents, Currency: ev.Currency,
				Provider: "lemonsqueezy", ProviderReference: payments.ReceiptReference(ev),
				Description: "Subscription payment refund", Kind: payments.ReceiptSubscriptionRefund,
			})
		}
		return nil
	case payments.EventPaymentFailed:
		s.notifyPaymentFailed(ctx, orgID, ev)
		return nil
	case payments.EventPaymentSucceeded, payments.EventPaymentRecovered:
		s.recordReceipt(ctx, payments.Receipt{
			OrganizationID: &orgID, AmountCents: ev.AmountCents, Currency: ev.Currency,
			Provider: "lemonsqueezy", ProviderReference: payments.ReceiptReference(ev),
			Description: payments.SubscriptionChargeDescription(ev.BillingReason), Kind: payments.ReceiptSubscriptionCharge,
		})
		return nil
	}
	return nil
}

// notifySubscriptionStarted emails the org's owners — matches the reference behavior
// SubscriptionStartedEmail audience exactly (owners only, not every billing
// recipient).
func (s *BillingService) notifySubscriptionStarted(ctx context.Context, orgID uuid.UUID, planName string) {
	if s.mailer == nil {
		return
	}
	org, err := s.repo.GetOrganization(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] subscription-started email: load org %s: %v", orgID, err)
		return
	}
	owners, err := s.repo.ListMembersByRole(ctx, orgID, RoleOwner)
	if err != nil {
		log.Printf("[organization billing] subscription-started email: list owners for org %s: %v", orgID, err)
		return
	}
	for _, owner := range owners {
		if err := s.mailer.SendSubscriptionStarted(ctx, owner.Email, org.Name, planName); err != nil {
			log.Printf("[organization billing] send subscription-started email to %s: %v", owner.Email, err)
		}
	}
}

// notifySubscriptionCancelled emails billing recipients (owners+admins) —
// The reference SubscriptionCancelledEmail audience.
func (s *BillingService) notifySubscriptionCancelled(ctx context.Context, orgID uuid.UUID, planName string, dueToPaymentFailure bool) {
	if s.mailer == nil {
		return
	}
	org, err := s.repo.GetOrganization(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] subscription-cancelled email: load org %s: %v", orgID, err)
		return
	}
	recipients, err := s.repo.ListMembersByRole(ctx, orgID, RoleOwner, RoleAdmin)
	if err != nil {
		log.Printf("[organization billing] subscription-cancelled email: list recipients for org %s: %v", orgID, err)
		return
	}
	for _, recipient := range recipients {
		if err := s.mailer.SendSubscriptionCancelled(ctx, recipient.Email, org.Name, planName, dueToPaymentFailure); err != nil {
			log.Printf("[organization billing] send subscription-cancelled email to %s: %v", recipient.Email, err)
		}
	}
}

// notifyPaymentFailed emails billing recipients (owners+admins) — the reference
// PaymentFailedEmail audience.
func (s *BillingService) notifyPaymentFailed(ctx context.Context, orgID uuid.UUID, ev *payments.WebhookEvent) {
	if s.mailer == nil {
		return
	}
	org, err := s.repo.GetOrganization(ctx, orgID)
	if err != nil {
		log.Printf("[organization billing] payment-failed email: load org %s: %v", orgID, err)
		return
	}
	recipients, err := s.repo.ListMembersByRole(ctx, orgID, RoleOwner, RoleAdmin)
	if err != nil {
		log.Printf("[organization billing] payment-failed email: list recipients for org %s: %v", orgID, err)
		return
	}
	for _, recipient := range recipients {
		if err := s.mailer.SendPaymentFailed(ctx, recipient.Email, org.Name, ev.AmountCents, ev.Currency, ev.InvoiceURL); err != nil {
			log.Printf("[organization billing] send payment-failed email to %s: %v", recipient.Email, err)
		}
	}
}

// RenewalReminderCandidate is one annual-plan org whose subscription is
// renewing soon and hasn't been reminded yet this cycle.
type RenewalReminderCandidate struct {
	OrgID       uuid.UUID
	OrgName     string
	PlanName    string
	RenewsAt    time.Time
	SeatCount   int
	AmountCents int64
	Currency    string
}

// ListAnnualRenewalsDue returns paid orgs on an ANNUAL plan whose
// subscription renews within `within` and haven't had a reminder sent for
// this cycle yet. Mirrors the reference RenewalUpcomingEmail restriction to
// annual plans (monthly/seat-change renewals are deliberately excluded
// there too — a reminder before every single frequent renewal would be
// spam, checked directly against their source 2026-09-21).
func (r *PostgresRepository) ListAnnualRenewalsDue(ctx context.Context, within time.Duration) ([]RenewalReminderCandidate, error) {
	deadline := time.Now().Add(within)
	rows, err := r.pool.Query(ctx, `
		SELECT o.id, o.name, p.display_name, o.subscription_renews_at,
		       (SELECT count(*) FROM memberships m WHERE m.organization_id = o.id),
		       p.price_cents, p.currency
		FROM organizations o
		JOIN plans p ON p.id = o.plan_id
		WHERE o.plan_id IS NOT NULL
		  AND p.billing_interval = 'annual'
		  AND o.subscription_renews_at IS NOT NULL
		  AND o.subscription_renews_at BETWEEN NOW() AND $1
		  AND o.renewal_reminder_sent_at IS NULL
	`, deadline)
	if err != nil {
		return nil, fmt.Errorf("list annual renewals due: %w", err)
	}
	defer rows.Close()

	var candidates []RenewalReminderCandidate
	for rows.Next() {
		var c RenewalReminderCandidate
		var seatCount int
		var priceCents int
		if err := rows.Scan(&c.OrgID, &c.OrgName, &c.PlanName, &c.RenewsAt, &seatCount, &priceCents, &c.Currency); err != nil {
			return nil, fmt.Errorf("scan renewal candidate: %w", err)
		}
		c.SeatCount = seatCount
		c.AmountCents = int64(priceCents) * int64(seatCount)
		candidates = append(candidates, c)
	}
	return candidates, rows.Err()
}

// MarkRenewalReminderSent records that this org's reminder has gone out for
// the current cycle — cleared again on the next genuine rollover, see
// activateOrgSubscription.
func (r *PostgresRepository) MarkRenewalReminderSent(ctx context.Context, orgID uuid.UUID) error {
	_, err := r.pool.Exec(ctx, `UPDATE organizations SET renewal_reminder_sent_at = NOW() WHERE id = $1`, orgID)
	if err != nil {
		return fmt.Errorf("mark renewal reminder sent: %w", err)
	}
	return nil
}

// SendRenewalReminders emails billing recipients (owners+admins) for every
// annual-plan org renewing within `within` that hasn't been reminded yet
// this cycle. Best-effort per org — one org's failure doesn't block the
// rest. Returns how many orgs were successfully reminded, for the caller's
// own logging; nil error unless the initial candidate query itself failed.
func (s *BillingService) SendRenewalReminders(ctx context.Context, within time.Duration) (int, error) {
	if s.mailer == nil {
		return 0, nil
	}
	candidates, err := s.repo.ListAnnualRenewalsDue(ctx, within)
	if err != nil {
		return 0, fmt.Errorf("list annual renewals due: %w", err)
	}
	sent := 0
	for _, c := range candidates {
		recipients, err := s.repo.ListMembersByRole(ctx, c.OrgID, RoleOwner, RoleAdmin)
		if err != nil {
			log.Printf("[organization billing] renewal reminder: list recipients for org %s: %v", c.OrgID, err)
			continue
		}
		anySent := false
		for _, recipient := range recipients {
			if err := s.mailer.SendRenewalUpcoming(ctx, recipient.Email, c.OrgName, c.PlanName, c.RenewsAt, c.AmountCents, c.Currency, c.SeatCount); err != nil {
				log.Printf("[organization billing] send renewal reminder to %s: %v", recipient.Email, err)
				continue
			}
			anySent = true
		}
		// Marked sent once ANY recipient succeeded — a single failed
		// delivery to one admin shouldn't cause the whole org to be
		// re-attempted (and re-mailed to everyone else) on every future
		// sweep for the rest of this cycle.
		if anySent {
			if err := s.repo.MarkRenewalReminderSent(ctx, c.OrgID); err != nil {
				log.Printf("[organization billing] mark renewal reminder sent for org %s: %v", c.OrgID, err)
			}
			sent++
		}
	}
	return sent, nil
}

// resolveOrgID prefers OrganizationID straight off the event (the normal
// case — LemonSqueezy round-trips checkout custom_data on every subsequent
// webhook for the same subscription). Falls back to a subscription-id
// lookup only as defense-in-depth for a delivery that somehow arrives
// without it.
func (s *BillingService) resolveOrgID(ctx context.Context, ev *payments.WebhookEvent) (uuid.UUID, error) {
	if ev.OrganizationID != "" {
		id, err := uuid.Parse(ev.OrganizationID)
		if err != nil {
			return uuid.Nil, fmt.Errorf("org webhook %s: invalid organization_id %q: %w", ev.RawEventName, ev.OrganizationID, err)
		}
		return id, nil
	}
	id, err := s.repo.getOrgBySubscriptionID(ctx, ev.SubscriptionID)
	if err != nil {
		return uuid.Nil, fmt.Errorf("org webhook %s: no organization_id and no org found for subscription %q: %w", ev.RawEventName, ev.SubscriptionID, err)
	}
	return id, nil
}

// OrganizationIDBySubscriptionID resolves an org from a LemonSqueezy
// subscription id. Exposed publicly (not just used internally by
// resolveOrgID) so payments.Service can call it directly as a webhook-
// attribution fallback — see payments.OrgApplier's doc comment for why this
// is kept separate from ApplyOrgEvent itself.
func (s *BillingService) OrganizationIDBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
	return s.repo.getOrgBySubscriptionID(ctx, subscriptionID)
}

var _ PlanLookup = (*payments.PostgresRepository)(nil)
var _ SeatBalances = (*payments.PostgresRepository)(nil)
var _ BillingProvider = (*payments.LemonSqueezy)(nil)
