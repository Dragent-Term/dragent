package payments

import (
	"context"
	"time"

	"github.com/google/uuid"
)

// Plan is a purchasable subscription (a plans row). Tier + MonthlyAllowanceMc
// are the entitlement; monthly and annual variants of a tier share both and
// differ only in BillingInterval and PriceCents.
type Plan struct {
	ID                    string
	Tier                  string
	BillingInterval       string // 'monthly' | 'annual'
	MonthlyAllowanceMc    int64
	PriceCents            int
	Currency              string
	DisplayName           string
	LemonSqueezyVariantID string
	IsActive              bool
}

// ReceiptKind classifies what kind of money movement a Receipt records.
type ReceiptKind string

const (
	ReceiptCreditPackPurchase ReceiptKind = "credit_pack_purchase"
	ReceiptCreditPackRefund   ReceiptKind = "credit_pack_refund"
	ReceiptSubscriptionCharge ReceiptKind = "subscription_charge"
	ReceiptSubscriptionRefund ReceiptKind = "subscription_refund"
)

// Receipt is a permanent, immutable record of money that actually moved —
// distinct from credit_transactions (internal balance-ledger movements, no
// currency/provider reference) and payment_events (webhook idempotency log,
// not structured as history). Written only for events that represent an
// actual charge or refund, never for an entitlement-lifecycle event alone
// (a subscription_created webhook changes access; the accompanying
// subscription_payment_success invoice event is what represents the real
// charge — see ORGANIZATION_BILLING_ISOLATION_AND_SHARING_PLAN.md §5).
// Never updated or deleted by any code path once written, including
// 11.F.3's entitlement-row deletion on leave/remove.
type Receipt struct {
	UserID            *uuid.UUID // nil for an org-attributed receipt
	OrganizationID    *uuid.UUID // nil for a personal receipt
	AmountCents       int64
	Currency          string
	Provider          string // e.g. "lemonsqueezy"
	ProviderReference string // the provider's own reference for this transaction — an invoice URL when the provider gives one, otherwise its id
	Description       string // human-readable: plan/product name
	Kind              ReceiptKind
}

// CreditPack is a one-time top-up purchase (a credit_packs row). CreditsMc is
// the durable spend budget granted to the deposit balance; larger packs bake in
// a bulk bonus.
type CreditPack struct {
	ID                    string
	PriceCents            int
	CreditsMc             int64
	Currency              string
	DisplayName           string
	LemonSqueezyVariantID string
	IsActive              bool
}

// Repository persists plans, packs, the webhook idempotency log, and
// subscription/deposit state on user_balances + users.
type Repository interface {
	GetPlanByID(ctx context.Context, id string) (*Plan, error)
	GetPlanByVariant(ctx context.Context, variantID string) (*Plan, error)
	ListActivePlans(ctx context.Context) ([]*Plan, error)

	GetCreditPackByID(ctx context.Context, id string) (*CreditPack, error)
	GetCreditPackByVariant(ctx context.Context, variantID string) (*CreditPack, error)
	ListActivePacks(ctx context.Context) ([]*CreditPack, error)

	// AddDepositCredits adds durable credits to the (userID, orgID) balance
	// row and records an auditable credit_transaction. Upserts the row.
	AddDepositCredits(ctx context.Context, userID, orgID uuid.UUID, creditsMc int64, description string) error

	// DeductDepositCredits claws credits back off the (userID, orgID) balance
	// row after a refund (floored at 0, never negative) and records an
	// auditable negative credit_transaction.
	DeductDepositCredits(ctx context.Context, userID, orgID uuid.UUID, creditsMc int64, description string) error

	// ClaimEvent reserves the webhook for processing, keyed by `id` (a hash of
	// the raw body). Returns true when the caller owns the apply: either the
	// row is new, or a previous attempt claimed it and never finished. Returns
	// false only when a prior attempt completed — that is the sole terminal
	// state, so a provider retry after a failed apply can still succeed.
	ClaimEvent(ctx context.Context, id, provider, eventName, externalID string, userID *uuid.UUID, payload []byte) (bool, error)
	// WithEventLock runs fn while holding an exclusive lock for this event id,
	// serialising concurrent deliveries of the SAME event across the whole
	// claim→apply→mark sequence. Returns ErrEventInFlight when another
	// delivery already holds it.
	//
	// The claim alone cannot provide this: its row lock is released when its
	// statement commits, long before the apply runs, so two deliveries could
	// both observe an unprocessed claim and both grant the credits.
	WithEventLock(ctx context.Context, id string, fn func() error) error
	// MarkEventProcessed closes the claim. Called ONLY after the apply
	// succeeded; skipping it is what leaves the event retryable.
	MarkEventProcessed(ctx context.Context, id string) error

	// ActivateSubscription grants the plan's entitlement: sets the user's tier,
	// paid monthly cap, and a fresh balance, and records the provider ids on the
	// user. Upserts the balance row (a user may subscribe before first agent use).
	ActivateSubscription(ctx context.Context, userID uuid.UUID, plan *Plan, subscriptionID, customerID, status string, renewsAt *time.Time) error

	// EndSubscription downgrades the user to the free tier (zero paid caps) and
	// records the ended status. Provider ids are kept for portal/history.
	EndSubscription(ctx context.Context, userID uuid.UUID, status string) error

	// SubscriptionID returns the user's provider subscription id (for the portal
	// link), or "" if none.
	SubscriptionID(ctx context.Context, userID uuid.UUID) (string, error)

	// UserIDBySubscriptionID is SubscriptionID's reverse: resolves which user
	// owns a provider subscription id. Used only as a webhook-attribution
	// fallback when a delivery arrives without user_id/organization_id in
	// custom_data (see Service.applyEvent) — the common path resolves
	// straight off custom_data and never calls this. Returns sql.ErrNoRows
	// if no user's stored subscription id matches.
	UserIDBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error)

	// GrantOrgSeat provisions a member's individual AI-credit entitlement in
	// their (userID, orgID) balance row — the same idempotent, grant-only-
	// on-genuine-new-entitlement pattern as ActivateSubscription, but
	// targeting the org's own row instead of the buyer's personal one.
	GrantOrgSeat(ctx context.Context, userID, orgID uuid.UUID, tier string, monthlyAllowanceMc int64, status string, renewsAt *time.Time) error

	// RevokeOrgSeat downgrades exactly the (userID, orgID) row to free — a
	// no-op if no such row exists. Used when the ORG's own subscription ends
	// but membership continues (every current member keeps their row, just
	// downgraded) — not when a member leaves or is removed, see
	// DeleteOrgSeat for that.
	RevokeOrgSeat(ctx context.Context, userID, orgID uuid.UUID) error

	// DeleteOrgSeat removes the (userID, orgID) balance row entirely — a
	// no-op if no such row exists. Used when a member leaves or is removed
	// from the org: they have no remaining relationship to it, so no balance
	// state — paid or free — should be left behind (isolation plan §4.1).
	DeleteOrgSeat(ctx context.Context, userID, orgID uuid.UUID) error

	// RecordReceipt writes a permanent payment-receipt row. See Receipt's
	// own doc comment for what belongs here vs. credit_transactions/
	// payment_events.
	RecordReceipt(ctx context.Context, r Receipt) error
}
