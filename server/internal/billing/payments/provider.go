// Package payments implements subscription billing on top of the credit engine.
//
// Providers (LemonSqueezy today; Paystack later) sit behind the PaymentProvider
// interface so the service layer never depends on a specific processor. Each
// provider normalises its own checkout flow and webhook payloads into the
// shared types below.
package payments

import (
	"time"

	"github.com/google/uuid"
)

// EventType is the normalised classification of a provider webhook.
type EventType string

const (
	// EventSubscriptionActive: a subscription was created/renewed/updated to an
	// active, paying state. Grant the plan's entitlement.
	EventSubscriptionActive EventType = "subscription_active"
	// EventSubscriptionEnded: cancelled, expired, unpaid, or paused — the user
	// no longer has an active paid subscription. Downgrade to free.
	EventSubscriptionEnded EventType = "subscription_ended"
	// EventOrderPaid: a one-time credit-pack purchase completed. Credit the
	// user's durable deposit balance. ProviderVariantID identifies the pack.
	EventOrderPaid EventType = "order_paid"
	// EventOrderRefunded: a one-time credit-pack purchase was refunded. Claw the
	// granted credits back off the deposit balance (floored at 0). Provider-
	// VariantID identifies the pack whose credits to reverse.
	EventOrderRefunded EventType = "order_refunded"
	// EventPaymentSucceeded: a subscription invoice (initial or renewal) was
	// paid. Entitlement is already granted via EventSubscriptionActive on the
	// accompanying subscription webhook — this is for receipts/notifications,
	// not access.
	EventPaymentSucceeded EventType = "payment_succeeded"
	// EventPaymentFailed: a subscription renewal charge failed and the
	// provider has entered its dunning/retry flow. Access continues during
	// retries (see activeStatuses' "past_due") — this event exists for
	// dunning notifications, not access.
	EventPaymentFailed EventType = "payment_failed"
	// EventPaymentRecovered: a previously failed subscription payment
	// succeeded on retry. For notifications, not access — the accompanying
	// subscription webhook already restores status to active.
	EventPaymentRecovered EventType = "payment_recovered"
	// EventIgnored: a webhook we don't act on.
	EventIgnored EventType = "ignored"
)

// WebhookEvent is a provider-agnostic view of an incoming webhook.
type WebhookEvent struct {
	Type EventType

	// UserID is our user's UUID, round-tripped through the provider's custom
	// checkout data. Empty if the provider didn't return it (a webhook we can't
	// attribute — logged and skipped).
	UserID string

	// OrganizationID is set instead of UserID for an org-billing checkout
	// (custom_data carries organization_id, not user_id) — mutually
	// exclusive with UserID in practice. A non-empty value routes the event
	// to Service's OrgApplier instead of the per-user switch.
	OrganizationID string

	// ProviderVariantID identifies which plan was purchased; the service maps it
	// to a plans row to resolve tier + allowance.
	ProviderVariantID string

	SubscriptionID string
	// SubscriptionItemID is the subscription-item id org billing later PATCHes
	// to sync seat quantity. Only populated when the provider includes it
	// (LemonSqueezy: subscription_created's first_subscription_item.id).
	SubscriptionItemID string
	CustomerID         string
	Status             string     // provider's raw status, stored for display
	RenewsAt           *time.Time // next renewal, when the provider reports one
	PortalURL          string     // provider-hosted manage/cancel link, when present

	// The following are only populated on invoice-shaped events
	// (EventPaymentSucceeded/Failed/Recovered, and the refund case of
	// EventSubscriptionEnded) — a subscription-invoice carries its own
	// amount/currency/receipt link, distinct from the subscription itself.
	AmountCents   int64  // invoice total, in the provider's smallest currency unit
	Currency      string // e.g. "USD"
	InvoiceURL    string // provider-hosted invoice/receipt PDF link
	BillingReason string // provider's reason for the invoice, e.g. "initial"/"renewal"

	RawEventName string // provider's event name, for the audit log
}

// PaymentProvider abstracts a payment processor. Implementations are stateless
// and safe for concurrent use.
type PaymentProvider interface {
	// Name identifies the provider in logs and the payment_events table.
	Name() string

	// CreditPackCheckout returns a hosted checkout URL for a one-time
	// credit-pack purchase, tagging the session with BOTH the buyer
	// (`userID`) and which org's balance row to credit (`orgID` — the
	// buyer's active org, captured now; see payments.OrgResolver's doc
	// comment for why it must be resolved at checkout time, not later).
	// Subscriptions have no individual checkout at all — every subscription
	// is an org subscription (organization.BillingProvider.OrgCheckout),
	// personal orgs included.
	CreditPackCheckout(userID, orgID uuid.UUID, variantID, redirectURL string) (string, error)

	// VerifyWebhook reports whether `signature` authenticates the raw `body`.
	// Must run before the body is trusted or parsed.
	VerifyWebhook(body []byte, signature string) bool

	// ParseWebhook normalises a verified raw body into a WebhookEvent.
	ParseWebhook(body []byte) (*WebhookEvent, error)

	// PortalURL returns the provider-hosted management link for a subscription,
	// or "" if the provider surfaces it only via webhooks.
	PortalURL(subscriptionID string) (string, error)
}
