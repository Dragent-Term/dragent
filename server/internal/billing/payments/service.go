package payments

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"log"

	"github.com/google/uuid"
)

// ErrInvalidSignature is returned when a webhook fails signature verification.
var ErrInvalidSignature = errors.New("invalid webhook signature")

// ErrDisabled is returned by checkout/portal calls when payments are not
// configured (no provider).
var ErrDisabled = errors.New("payments are not configured")

// OrgApplier receives webhook events attributed to an organization
// (WebhookEvent.OrganizationID set) instead of an individual user.
// Implemented by the organization package's billing service; kept as an
// interface here so payments never imports organization — the same
// dependency-inversion shape used elsewhere in this codebase (e.g.
// identity's BalanceInitializer/OrganizationInitializer). Optional: nil
// means org-attributed webhooks are logged and ignored, individual billing
// is completely unaffected either way.
type OrgApplier interface {
	ApplyOrgEvent(ctx context.Context, ev *WebhookEvent) error

	// OrganizationIDBySubscriptionID resolves which org (if any) owns a
	// provider subscription id. Returns sql.ErrNoRows if none does. Used
	// only to disambiguate org-vs-individual attribution when a webhook
	// arrives without organization_id in custom_data (see
	// Service.applyEvent) — kept as its own call, separate from
	// ApplyOrgEvent, so a real failure applying a resolved org's event can
	// never be misread as "this isn't an org subscription."
	OrganizationIDBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error)
}

// OrgResolver resolves which org a user is currently acting as — used only
// to attribute a credit-pack purchase to the buyer's active org AT CHECKOUT
// TIME (captured into custom_data then, not re-resolved when the webhook
// arrives later: the buyer's active org could have changed in the meantime,
// and re-resolving then would credit the wrong org's balance row).
// Implemented by the organization package; kept as an interface here for the
// same reason as OrgApplier.
type OrgResolver interface {
	ActiveOrganizationID(ctx context.Context, userID uuid.UUID) (uuid.UUID, error)
}

// Service orchestrates a payment provider against the Repository.
type Service struct {
	provider    PaymentProvider
	repo        Repository
	successURL  string // where checkout redirects on success
	orgApplier  OrgApplier
	orgResolver OrgResolver
}

func NewService(provider PaymentProvider, repo Repository, successURL string) *Service {
	return &Service{provider: provider, repo: repo, successURL: successURL}
}

// SetOrgResolver wires active-org resolution into credit-pack checkout.
// Optional — see OrgResolver's doc comment for why this matters.
func (s *Service) SetOrgResolver(r OrgResolver) { s.orgResolver = r }

// SetOrgApplier wires organization billing into webhook handling. Optional —
// call it once at startup (mirrors SetBalanceInitializer/
// SetOrganizationInitializer's setter-after-construct pattern) when org
// billing is built; skip it and org-attributed webhooks are simply logged
// and ignored.
func (s *Service) SetOrgApplier(a OrgApplier) { s.orgApplier = a }

// Enabled reports whether a provider is configured.
func (s *Service) Enabled() bool { return s != nil && s.provider != nil }

// ListPlans returns the active purchasable plans.
func (s *Service) ListPlans(ctx context.Context) ([]*Plan, error) {
	return s.repo.ListActivePlans(ctx)
}

// ListPacks returns the active one-time credit packs.
func (s *Service) ListPacks(ctx context.Context) ([]*CreditPack, error) {
	return s.repo.ListActivePacks(ctx)
}

// CreateTopupCheckout returns a hosted checkout URL for a one-time credit
// pack, tagging both who's buying and which org's balance row to credit —
// the buyer's CURRENT active org, resolved now rather than left for the
// webhook to figure out later (see OrgResolver's doc comment for why that
// distinction matters). Subscriptions no longer have an individual checkout
// at all: every org, personal included, is a real org since a personal org
// is just a 1-member one — see organization.BillingService.Subscribe.
func (s *Service) CreateTopupCheckout(ctx context.Context, userID uuid.UUID, packID string) (string, error) {
	if !s.Enabled() {
		return "", ErrDisabled
	}
	pack, err := s.repo.GetCreditPackByID(ctx, packID)
	if err != nil {
		return "", err
	}
	if pack.LemonSqueezyVariantID == "" {
		return "", fmt.Errorf("pack %q is not available for purchase (no provider variant configured)", packID)
	}
	var orgID uuid.UUID
	if s.orgResolver != nil {
		orgID, err = s.orgResolver.ActiveOrganizationID(ctx, userID)
		if err != nil {
			return "", fmt.Errorf("resolve active organization: %w", err)
		}
	}
	if orgID == uuid.Nil {
		return "", fmt.Errorf("no active organization to credit")
	}
	return s.provider.CreditPackCheckout(userID, orgID, pack.LemonSqueezyVariantID, s.successURL)
}

// HandleWebhook verifies, records (idempotently), and applies a provider
// webhook. Returns ErrInvalidSignature on a bad signature; nil for benign
// no-ops (unknown/ignored event, re-delivery, unattributable); a wrapped error
// for genuine failures the provider should retry.
func (s *Service) HandleWebhook(ctx context.Context, body []byte, signature string) error {
	if !s.Enabled() {
		return ErrDisabled
	}
	if !s.provider.VerifyWebhook(body, signature) {
		return ErrInvalidSignature
	}
	ev, err := s.provider.ParseWebhook(body)
	if err != nil {
		return fmt.Errorf("parse webhook: %w", err)
	}

	var uid *uuid.UUID
	if ev.UserID != "" {
		if parsed, perr := uuid.Parse(ev.UserID); perr == nil {
			uid = &parsed
		}
	}

	// Idempotency: key on a hash of the raw body. The claim is only terminal
	// once the apply below has SUCCEEDED — an event whose apply failed stays
	// claimable so the provider's retry can finish the job. Recording as
	// terminal up-front is what previously turned a transient DB error into a
	// permanently lost, already-paid-for top-up.
	id := hashHex(body)

	// The lock must span claim → apply → mark. The claim's own row lock ends
	// with its statement, so without this two concurrent deliveries of the
	// same order could both see an unprocessed claim and both grant credits —
	// turning the lost-top-up bug this flow was written to fix into a
	// double-credit one.
	err = s.repo.WithEventLock(ctx, id, func() error {
		claimed, cerr := s.repo.ClaimEvent(ctx, id, s.provider.Name(), ev.RawEventName, ev.SubscriptionID, uid, body)
		if cerr != nil {
			return fmt.Errorf("claim event: %w", cerr)
		}
		if !claimed {
			return nil // a previous delivery already applied this event
		}

		if aerr := s.applyEvent(ctx, ev, uid); aerr != nil {
			// Leave processed_at NULL: returning an error makes the handler
			// reply 5xx, the provider retries, and the retry re-claims.
			return aerr
		}
		if merr := s.repo.MarkEventProcessed(ctx, id); merr != nil {
			// The apply DID land. Failing here would make the provider retry
			// and double-apply, which is worse than a stale claim — log and
			// accept.
			log.Printf("[payments] applied %s but could not mark processed (id=%s): %v",
				ev.RawEventName, id, merr)
		}
		return nil
	})
	if errors.Is(err, ErrEventInFlight) {
		// A concurrent delivery of this same event holds the lock and will
		// finish the work. Report success so the provider stops retrying.
		log.Printf("[payments] duplicate delivery of %s ignored (id=%s): already in flight",
			ev.RawEventName, id)
		return nil
	}
	return err
}

// applyEvent performs the actual entitlement change. Returning an error means
// "not applied, safe to retry".
func (s *Service) applyEvent(ctx context.Context, ev *WebhookEvent, uid *uuid.UUID) error {
	if ev.Type == EventIgnored {
		return nil
	}
	// Order events (credit-pack purchases) are always user-attributed — the
	// buyer authorizes the purchase — even though ev.OrganizationID is ALSO
	// set on them (it names which org's balance row to credit, a different
	// meaning than it has on every other event type, where its presence
	// means "route this whole event to org billing instead"). Must be
	// checked before that generic org-attribution branch below or every
	// credit-pack purchase would be misrouted into applyOrgEvent, which has
	// no concept of credit packs at all.
	if ev.Type == EventOrderPaid || ev.Type == EventOrderRefunded {
		if uid == nil {
			return nil // recorded for audit, no buyer to attribute to
		}
		return s.applyUserEvent(ctx, ev, *uid)
	}
	// Org-attributed events never carry a user_id (an org checkout tags
	// organization_id instead) — branch before the uid==nil check below,
	// which would otherwise silently swallow every one of them.
	if ev.OrganizationID != "" {
		return s.applyOrgEvent(ctx, ev)
	}
	if uid != nil {
		return s.applyUserEvent(ctx, ev, *uid)
	}

	// Neither custom_data field was present. The common path never reaches
	// here — LemonSqueezy round-trips custom_data onto every "subscriptions"
	// -shaped event. But its docs make no such guarantee for the
	// "subscription-invoices"-shaped events (subscription_payment_*), so
	// this is a real, not merely defensive, path: resolve by the
	// subscription id itself instead of giving up. Org resolution is a
	// separate call from ApplyOrgEvent specifically so a real downstream
	// failure (e.g. an unrecognised plan variant) can never be misread as
	// "not an org subscription" and silently redirected to the individual
	// fallback below.
	if ev.SubscriptionID == "" {
		return nil // recorded for audit, nothing left to attribute by
	}
	if s.orgApplier != nil {
		orgID, err := s.orgApplier.OrganizationIDBySubscriptionID(ctx, ev.SubscriptionID)
		switch {
		case err == nil:
			ev.OrganizationID = orgID.String()
			return s.applyOrgEvent(ctx, ev)
		case !errors.Is(err, sql.ErrNoRows):
			return fmt.Errorf("resolve organization for subscription %q: %w", ev.SubscriptionID, err)
		}
		// sql.ErrNoRows: this subscription isn't an org's — fall through.
	}
	resolvedUID, err := s.repo.UserIDBySubscriptionID(ctx, ev.SubscriptionID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			log.Printf("[payments] webhook %s has no user_id/organization_id and subscription %q matches no known subscription; ignoring",
				ev.RawEventName, ev.SubscriptionID)
			return nil
		}
		return fmt.Errorf("resolve user for subscription %q: %w", ev.SubscriptionID, err)
	}
	return s.applyUserEvent(ctx, ev, resolvedUID)
}

// applyOrgEvent hands an org-attributed webhook to the wired OrgApplier. When
// ev.OrganizationID is empty (the subscription-id-fallback path above),
// ApplyOrgEvent resolves the org from ev.SubscriptionID itself and returns an
// error wrapping sql.ErrNoRows if no org's stored subscription id matches —
// callers use that to distinguish "not an org event" from a real failure.
func (s *Service) applyOrgEvent(ctx context.Context, ev *WebhookEvent) error {
	if s.orgApplier == nil {
		log.Printf("[payments] org-attributed webhook %s received but no org applier is wired; ignoring", ev.RawEventName)
		return nil
	}
	return s.orgApplier.ApplyOrgEvent(ctx, ev)
}

// applyUserEvent is the per-user entitlement switch, used both by the common
// custom_data-attributed path and the subscription-id-fallback path above.
func (s *Service) applyUserEvent(ctx context.Context, ev *WebhookEvent, uid uuid.UUID) error {
	switch ev.Type {
	case EventSubscriptionActive:
		plan, perr := s.repo.GetPlanByVariant(ctx, ev.ProviderVariantID)
		if perr != nil {
			return fmt.Errorf("webhook %s: variant %q has no plan: %w", ev.RawEventName, ev.ProviderVariantID, perr)
		}
		return s.repo.ActivateSubscription(ctx, uid, plan, ev.SubscriptionID, ev.CustomerID, ev.Status, ev.RenewsAt)
	case EventSubscriptionEnded:
		if err := s.repo.EndSubscription(ctx, uid, ev.Status); err != nil {
			return err
		}
		// subscription_payment_refunded is the one EventSubscriptionEnded
		// cause that's ALSO an invoice-shaped event carrying a real charged
		// amount (see lemonsqueezy.go) — the others (cancelled/expired/
		// paused) are pure entitlement-lifecycle events with nothing to
		// receipt.
		if ev.RawEventName == "subscription_payment_refunded" {
			s.recordReceipt(ctx, Receipt{
				UserID: &uid, AmountCents: ev.AmountCents, Currency: ev.Currency,
				Provider: "lemonsqueezy", ProviderReference: receiptReference(ev),
				Description: "Subscription payment refund", Kind: ReceiptSubscriptionRefund,
			})
		}
		return nil
	case EventOrderPaid:
		pack, perr := s.repo.GetCreditPackByVariant(ctx, ev.ProviderVariantID)
		if perr != nil {
			return fmt.Errorf("webhook %s: variant %q has no credit pack: %w", ev.RawEventName, ev.ProviderVariantID, perr)
		}
		orgID, oerr := uuid.Parse(ev.OrganizationID)
		if oerr != nil {
			return fmt.Errorf("webhook %s: credit-pack purchase missing a valid organization_id: %w", ev.RawEventName, oerr)
		}
		if err := s.repo.AddDepositCredits(ctx, uid, orgID, pack.CreditsMc, "Top-up: "+pack.DisplayName); err != nil {
			return err
		}
		s.recordReceipt(ctx, Receipt{
			UserID: &uid, OrganizationID: &orgID, AmountCents: int64(pack.PriceCents), Currency: pack.Currency,
			Provider: "lemonsqueezy", ProviderReference: ev.SubscriptionID,
			Description: "Top-up: " + pack.DisplayName, Kind: ReceiptCreditPackPurchase,
		})
		return nil
	case EventOrderRefunded:
		pack, perr := s.repo.GetCreditPackByVariant(ctx, ev.ProviderVariantID)
		if perr != nil {
			return fmt.Errorf("webhook %s: variant %q has no credit pack: %w", ev.RawEventName, ev.ProviderVariantID, perr)
		}
		orgID, oerr := uuid.Parse(ev.OrganizationID)
		if oerr != nil {
			return fmt.Errorf("webhook %s: credit-pack refund missing a valid organization_id: %w", ev.RawEventName, oerr)
		}
		if err := s.repo.DeductDepositCredits(ctx, uid, orgID, pack.CreditsMc, "Refund: "+pack.DisplayName); err != nil {
			return err
		}
		s.recordReceipt(ctx, Receipt{
			UserID: &uid, OrganizationID: &orgID, AmountCents: int64(pack.PriceCents), Currency: pack.Currency,
			Provider: "lemonsqueezy", ProviderReference: ev.SubscriptionID,
			Description: "Refund: " + pack.DisplayName, Kind: ReceiptCreditPackRefund,
		})
		return nil
	case EventPaymentSucceeded, EventPaymentRecovered:
		s.recordReceipt(ctx, Receipt{
			UserID: &uid, AmountCents: ev.AmountCents, Currency: ev.Currency,
			Provider: "lemonsqueezy", ProviderReference: receiptReference(ev),
			Description: subscriptionChargeDescription(ev.BillingReason), Kind: ReceiptSubscriptionCharge,
		})
		return nil
	}
	// EventPaymentFailed: parsed and classified, no receipt (nothing was
	// actually charged) and no entitlement change (already handled by the
	// accompanying subscription webhook) — notifications are the org-side's
	// job today (organization.BillingService.notifyPaymentFailed); personal
	// billing has no notification path yet, a known, separately-tracked gap.
	return nil
}

// recordReceipt is best-effort: a receipts-table write failure must never
// turn an otherwise-successful charge/refund into a webhook the provider
// keeps retrying (a retry would redundantly re-run the entitlement change
// too) — logged, not surfaced.
func (s *Service) recordReceipt(ctx context.Context, r Receipt) {
	if err := s.repo.RecordReceipt(ctx, r); err != nil {
		log.Printf("[payments] record receipt (kind=%s, ref=%s): %v", r.Kind, r.ProviderReference, err)
	}
}

// receiptReference prefers the invoice's own hosted URL (a human can click
// straight through to it) over the bare subscription id.
func receiptReference(ev *WebhookEvent) string {
	return ReceiptReference(ev)
}

// ReceiptReference is receiptReference exported for organization.BillingService,
// which builds org-attributed receipts from the same WebhookEvent shape.
func ReceiptReference(ev *WebhookEvent) string {
	if ev.InvoiceURL != "" {
		return ev.InvoiceURL
	}
	return ev.SubscriptionID
}

func subscriptionChargeDescription(billingReason string) string {
	return SubscriptionChargeDescription(billingReason)
}

// SubscriptionChargeDescription is subscriptionChargeDescription exported
// for organization.BillingService, for the same reason as ReceiptReference.
func SubscriptionChargeDescription(billingReason string) string {
	switch billingReason {
	case "initial":
		return "Subscription started"
	case "renewal":
		return "Subscription renewal"
	case "updated":
		return "Subscription updated"
	default:
		return "Subscription charge"
	}
}

func hashHex(b []byte) string {
	sum := sha256.Sum256(b)
	return hex.EncodeToString(sum[:])
}
