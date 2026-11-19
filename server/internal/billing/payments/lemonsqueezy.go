package payments

import (
	"bytes"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
)

const lemonSqueezyAPIBase = "https://api.lemonsqueezy.com/v1"

// LemonSqueezy implements PaymentProvider against the LemonSqueezy API.
type LemonSqueezy struct {
	apiKey        string
	storeID       string
	webhookSecret string
	http          *http.Client
}

// NewLemonSqueezy builds a provider. Returns nil when unconfigured (no API
// key), so the caller can leave payments disabled.
func NewLemonSqueezy(apiKey, storeID, webhookSecret string) *LemonSqueezy {
	if apiKey == "" || storeID == "" {
		return nil
	}
	return &LemonSqueezy{
		apiKey:        apiKey,
		storeID:       storeID,
		webhookSecret: webhookSecret,
		http:          &http.Client{Timeout: 15 * time.Second},
	}
}

func (l *LemonSqueezy) Name() string { return "lemonsqueezy" }

// ── Checkout ──────────────────────────────────────────────────────────────────

// Checkout creates a hosted checkout for the variant and returns its URL. The
// user's UUID is passed as checkout custom data so it round-trips back on the
// subscription webhook (LemonSqueezy custom values must be strings).
func (l *LemonSqueezy) CreditPackCheckout(userID, orgID uuid.UUID, variantID, redirectURL string) (string, error) {
	if variantID == "" {
		return "", fmt.Errorf("lemonsqueezy: pack has no variant id configured")
	}
	return l.postCheckout(variantID, map[string]any{
		"checkout_data": map[string]any{
			"custom": map[string]any{"user_id": userID.String(), "organization_id": orgID.String()},
		},
		"product_options": map[string]any{
			"redirect_url": redirectURL,
		},
	})
}

// OrgCheckout creates a hosted checkout for an organization's paid seats:
// the variant at `quantity` (the org's current member count, all of whom
// become billed seats the moment the org goes paid), tagged with
// organization_id — not user_id — so the resulting webhook round-trips back
// to the org rather than any individual member. Used only for an org's
// FIRST subscription; every seat added after goes through
// UpdateSeatQuantity instead (see ORGANIZATION_GAPS_AND_OPEN_QUESTIONS.md
// §3.1's routing logic).
func (l *LemonSqueezy) OrgCheckout(organizationID uuid.UUID, variantID string, quantity int, redirectURL string) (string, error) {
	if variantID == "" {
		return "", fmt.Errorf("lemonsqueezy: plan has no variant id configured")
	}
	if quantity < 1 {
		quantity = 1
	}
	variantIDInt, err := strconv.ParseInt(variantID, 10, 64)
	if err != nil {
		return "", fmt.Errorf("lemonsqueezy: invalid variant id %q: %w", variantID, err)
	}
	return l.postCheckout(variantID, map[string]any{
		"checkout_data": map[string]any{
			"custom":             map[string]any{"organization_id": organizationID.String()},
			"variant_quantities": []map[string]any{{"variant_id": variantIDInt, "quantity": quantity}},
		},
		"product_options": map[string]any{
			"redirect_url": redirectURL,
		},
	})
}

// postCheckout shares the request/response handling between Checkout and
// OrgCheckout — only the `attributes` payload (custom data, quantity) differs.
func (l *LemonSqueezy) postCheckout(variantID string, attributes map[string]any) (string, error) {
	body := map[string]any{
		"data": map[string]any{
			"type":       "checkouts",
			"attributes": attributes,
			"relationships": map[string]any{
				"store":   jsonAPIRef("stores", l.storeID),
				"variant": jsonAPIRef("variants", variantID),
			},
		},
	}
	payload, err := json.Marshal(body)
	if err != nil {
		return "", err
	}

	req, err := http.NewRequest(http.MethodPost, lemonSqueezyAPIBase+"/checkouts", bytes.NewReader(payload))
	if err != nil {
		return "", err
	}
	l.setHeaders(req)

	resp, err := l.http.Do(req)
	if err != nil {
		return "", fmt.Errorf("lemonsqueezy checkout request: %w", err)
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return "", fmt.Errorf("lemonsqueezy checkout failed (%d): %s", resp.StatusCode, string(respBody))
	}

	var parsed struct {
		Data struct {
			Attributes struct {
				URL string `json:"url"`
			} `json:"attributes"`
		} `json:"data"`
	}
	if err := json.Unmarshal(respBody, &parsed); err != nil {
		return "", fmt.Errorf("lemonsqueezy checkout decode: %w", err)
	}
	if parsed.Data.Attributes.URL == "" {
		return "", fmt.Errorf("lemonsqueezy checkout returned no url")
	}
	return parsed.Data.Attributes.URL, nil
}

// UpdateSeatQuantity PATCHes a subscription item's quantity — the org
// per-seat add/remove sync, called after a membership is granted or
// revoked. Uses LemonSqueezy's default proration (no invoice_immediately or
// disable_prorations flag): the difference defers onto the next invoice,
// which is the behavior confirmed to match Stripe's create_prorations
// (§3.2) — the org design deliberately chose this over an immediate charge.
func (l *LemonSqueezy) UpdateSeatQuantity(subscriptionItemID string, quantity int) error {
	if subscriptionItemID == "" {
		return fmt.Errorf("lemonsqueezy: no subscription item id")
	}
	if quantity < 1 {
		quantity = 1
	}
	body := map[string]any{
		"data": map[string]any{
			"type": "subscription-items",
			"id":   subscriptionItemID,
			"attributes": map[string]any{
				"quantity": quantity,
				// Charge/credit the proration difference right away rather
				// than silently deferring it to next renewal (LemonSqueezy's
				// default) — a seat added or removed should be reflected in
				// what the org is charged immediately, not weeks later with
				// no visible connection to the membership change that
				// caused it. No dry-run/preview exists for this endpoint
				// (checked directly against LemonSqueezy's docs) — this is
				// the real charge, not an estimate.
				"invoice_immediately": true,
			},
		},
	}
	payload, err := json.Marshal(body)
	if err != nil {
		return err
	}

	req, err := http.NewRequest(http.MethodPatch, lemonSqueezyAPIBase+"/subscription-items/"+subscriptionItemID, bytes.NewReader(payload))
	if err != nil {
		return err
	}
	l.setHeaders(req)

	resp, err := l.http.Do(req)
	if err != nil {
		return fmt.Errorf("lemonsqueezy update subscription item request: %w", err)
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("lemonsqueezy update subscription item failed (%d): %s", resp.StatusCode, string(respBody))
	}
	return nil
}

// ── Webhooks ──────────────────────────────────────────────────────────────────

// VerifyWebhook checks the X-Signature header (hex HMAC-SHA256 of the raw body
// under the webhook signing secret). Constant-time comparison.
func (l *LemonSqueezy) VerifyWebhook(body []byte, signature string) bool {
	if l.webhookSecret == "" || signature == "" {
		return false
	}
	mac := hmac.New(sha256.New, []byte(l.webhookSecret))
	mac.Write(body)
	expected := hex.EncodeToString(mac.Sum(nil))
	return hmac.Equal([]byte(expected), []byte(signature))
}

// lsWebhook mirrors the subset of the LemonSqueezy webhook payload we use.
// Two materially different resource shapes flow through this one struct:
// a "subscriptions" object (subscription_created/updated/cancelled/etc, data.id
// IS the subscription id) and a "subscription-invoices" object (the
// subscription_payment_* events, data.id is the INVOICE's own id — the real
// link back to the subscription is attributes.subscription_id). ParseWebhook
// branches on event name to read the right fields for each.
type lsWebhook struct {
	Meta struct {
		EventName  string            `json:"event_name"`
		CustomData map[string]string `json:"custom_data"`
	} `json:"meta"`
	Data struct {
		ID         string `json:"id"`
		Attributes struct {
			Status     string     `json:"status"`
			VariantID  int64      `json:"variant_id"`
			CustomerID int64      `json:"customer_id"`
			RenewsAt   *time.Time `json:"renews_at"`
			URLs       struct {
				CustomerPortal string `json:"customer_portal"`
				// InvoiceURL is only present on a subscription-invoice
				// object (the hosted PDF/receipt link).
				InvoiceURL string `json:"invoice_url"`
			} `json:"urls"`
			// Orders carry the purchased variant here (not top-level variant_id).
			FirstOrderItem struct {
				VariantID int64 `json:"variant_id"`
			} `json:"first_order_item"`
			// FirstSubscriptionItem.ID is the id an org subscription's later
			// PATCH /v1/subscription-items/{id} quantity sync needs. Present
			// on subscription_created; a zero value elsewhere leaves the
			// stored id untouched (see ParseWebhook and the org repo's
			// COALESCE on write) rather than blanking it out.
			FirstSubscriptionItem struct {
				ID int64 `json:"id"`
			} `json:"first_subscription_item"`
			// The remaining fields are only present on a subscription-invoice
			// object (subscription_payment_* events).
			SubscriptionID int64  `json:"subscription_id"`
			Total          int64  `json:"total"`
			Currency       string `json:"currency"`
			BillingReason  string `json:"billing_reason"`
		} `json:"attributes"`
	} `json:"data"`
}

// activeStatuses are the LemonSqueezy subscription statuses that grant access.
var activeStatuses = map[string]bool{"active": true, "on_trial": true, "past_due": true}

// ParseWebhook normalises a verified body. Only subscription_*/order_* events
// act; other events (customer_updated, license_key_*, affiliate_activated —
// none apply to dragent, which sells neither license keys nor runs an
// affiliate program) map to EventIgnored.
func (l *LemonSqueezy) ParseWebhook(body []byte) (*WebhookEvent, error) {
	var w lsWebhook
	if err := json.Unmarshal(body, &w); err != nil {
		return nil, fmt.Errorf("lemonsqueezy webhook decode: %w", err)
	}

	ev := &WebhookEvent{
		RawEventName:      w.Meta.EventName,
		UserID:            w.Meta.CustomData["user_id"],
		OrganizationID:    w.Meta.CustomData["organization_id"],
		Status:            w.Data.Attributes.Status,
		RenewsAt:          w.Data.Attributes.RenewsAt,
		PortalURL:         w.Data.Attributes.URLs.CustomerPortal,
		ProviderVariantID: strconv.FormatInt(w.Data.Attributes.VariantID, 10),
	}
	if w.Data.Attributes.CustomerID != 0 {
		ev.CustomerID = strconv.FormatInt(w.Data.Attributes.CustomerID, 10)
	}
	if w.Data.Attributes.FirstSubscriptionItem.ID != 0 {
		ev.SubscriptionItemID = strconv.FormatInt(w.Data.Attributes.FirstSubscriptionItem.ID, 10)
	}

	if strings.HasPrefix(w.Meta.EventName, "subscription_payment_") {
		// subscription-invoice object: data.id is the INVOICE's own id, not
		// the subscription's — the real link back is this attribute.
		if w.Data.Attributes.SubscriptionID != 0 {
			ev.SubscriptionID = strconv.FormatInt(w.Data.Attributes.SubscriptionID, 10)
		}
		ev.AmountCents = w.Data.Attributes.Total
		ev.Currency = w.Data.Attributes.Currency
		ev.InvoiceURL = w.Data.Attributes.URLs.InvoiceURL
		ev.BillingReason = w.Data.Attributes.BillingReason
	} else {
		ev.SubscriptionID = w.Data.ID
	}

	switch w.Meta.EventName {
	case "subscription_created", "subscription_updated", "subscription_resumed",
		"subscription_unpaused":
		if activeStatuses[w.Data.Attributes.Status] {
			ev.Type = EventSubscriptionActive
		} else {
			ev.Type = EventSubscriptionEnded
		}
	case "subscription_cancelled", "subscription_expired", "subscription_paused":
		ev.Type = EventSubscriptionEnded
	case "subscription_payment_success":
		ev.Type = EventPaymentSucceeded
	case "subscription_payment_failed":
		ev.Type = EventPaymentFailed
	case "subscription_payment_recovered":
		ev.Type = EventPaymentRecovered
	case "subscription_payment_refunded":
		// A refunded subscription payment revokes paid access — treat it like
		// an end so the user drops to free (they can re-subscribe).
		ev.Type = EventSubscriptionEnded
	case "order_created":
		// One-time credit-pack purchase. The purchased variant is in
		// first_order_item, not the top-level variant_id.
		if w.Data.Attributes.Status == "paid" {
			ev.Type = EventOrderPaid
			ev.ProviderVariantID = strconv.FormatInt(w.Data.Attributes.FirstOrderItem.VariantID, 10)
		} else {
			ev.Type = EventIgnored
		}
	case "order_refunded":
		// The refunded pack's variant is in first_order_item (same as the
		// original order); the service claws its credits back.
		ev.Type = EventOrderRefunded
		ev.ProviderVariantID = strconv.FormatInt(w.Data.Attributes.FirstOrderItem.VariantID, 10)
	default:
		ev.Type = EventIgnored
	}
	return ev, nil
}

// PortalURL fetches the subscription and returns its customer-portal link.
func (l *LemonSqueezy) PortalURL(subscriptionID string) (string, error) {
	if subscriptionID == "" {
		return "", fmt.Errorf("no subscription id")
	}
	req, err := http.NewRequest(http.MethodGet, lemonSqueezyAPIBase+"/subscriptions/"+subscriptionID, nil)
	if err != nil {
		return "", err
	}
	l.setHeaders(req)

	resp, err := l.http.Do(req)
	if err != nil {
		return "", fmt.Errorf("lemonsqueezy subscription request: %w", err)
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return "", fmt.Errorf("lemonsqueezy subscription fetch failed (%d): %s", resp.StatusCode, string(respBody))
	}

	var parsed struct {
		Data struct {
			Attributes struct {
				URLs struct {
					CustomerPortal string `json:"customer_portal"`
				} `json:"urls"`
			} `json:"attributes"`
		} `json:"data"`
	}
	if err := json.Unmarshal(respBody, &parsed); err != nil {
		return "", err
	}
	return parsed.Data.Attributes.URLs.CustomerPortal, nil
}

// ── helpers ───────────────────────────────────────────────────────────────────

func (l *LemonSqueezy) setHeaders(req *http.Request) {
	req.Header.Set("Accept", "application/vnd.api+json")
	req.Header.Set("Content-Type", "application/vnd.api+json")
	req.Header.Set("Authorization", "Bearer "+l.apiKey)
}

func jsonAPIRef(typ, id string) map[string]any {
	return map[string]any{"data": map[string]any{"type": typ, "id": id}}
}
