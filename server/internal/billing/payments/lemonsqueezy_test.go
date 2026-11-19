package payments

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
)

// roundTripperFunc lets a test stand in for the real LemonSqueezy API
// without a network call — captures the outgoing request for inspection.
type roundTripperFunc func(*http.Request) (*http.Response, error)

func (f roundTripperFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func sign(secret string, body []byte) string {
	m := hmac.New(sha256.New, []byte(secret))
	m.Write(body)
	return hex.EncodeToString(m.Sum(nil))
}

func TestVerifyWebhook(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "whsec_test")
	body := []byte(`{"meta":{"event_name":"subscription_created"}}`)

	if !ls.VerifyWebhook(body, sign("whsec_test", body)) {
		t.Fatal("valid signature must verify")
	}
	if ls.VerifyWebhook(body, sign("wrong-secret", body)) {
		t.Fatal("signature under a different secret must fail")
	}
	if ls.VerifyWebhook(body, "deadbeef") {
		t.Fatal("garbage signature must fail")
	}
	if ls.VerifyWebhook(body, "") {
		t.Fatal("empty signature must fail")
	}
	// A body tampered with after signing must not verify (sig is over the raw body).
	if ls.VerifyWebhook([]byte(`{"tampered":true}`), sign("whsec_test", body)) {
		t.Fatal("tampered body must fail")
	}
}

// UpdateSeatQuantity must charge/credit the proration immediately rather
// than silently deferring to next renewal — LemonSqueezy has no dry-run
// preview for this endpoint (checked directly against its docs), so
// invoice_immediately is what makes a seat change's cost show up when the
// membership change actually happens, not weeks later.
func TestUpdateSeatQuantity_ChargesImmediately(t *testing.T) {
	var captured map[string]any
	ls := &LemonSqueezy{
		apiKey:  "k",
		storeID: "1",
		http: &http.Client{Transport: roundTripperFunc(func(r *http.Request) (*http.Response, error) {
			if r.Method != http.MethodPatch {
				t.Fatalf("method=%s want PATCH", r.Method)
			}
			body, _ := io.ReadAll(r.Body)
			if err := json.Unmarshal(body, &captured); err != nil {
				t.Fatal(err)
			}
			return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader("{}"))}, nil
		})},
	}
	if err := ls.UpdateSeatQuantity("item_123", 3); err != nil {
		t.Fatal(err)
	}
	attrs, _ := captured["data"].(map[string]any)["attributes"].(map[string]any)
	if attrs["invoice_immediately"] != true {
		t.Fatalf("attributes=%+v want invoice_immediately=true", attrs)
	}
	if attrs["quantity"] != float64(3) {
		t.Fatalf("attributes=%+v want quantity=3", attrs)
	}
}

func TestVerifyWebhook_NoSecretAlwaysFails(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "") // no webhook secret configured
	body := []byte(`{}`)
	if ls.VerifyWebhook(body, sign("", body)) {
		t.Fatal("with no configured secret, verification must always fail")
	}
}

func TestParseWebhook_ActiveSubscription(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_created", "custom_data": {"user_id": "u-123"}},
		"data": {"id": "sub_99", "attributes": {
			"status": "active", "variant_id": 555, "customer_id": 777,
			"renews_at": "2026-09-01T00:00:00.000000Z",
			"urls": {"customer_portal": "https://portal.example/abc"}
		}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventSubscriptionActive {
		t.Fatalf("type=%s want active", ev.Type)
	}
	if ev.UserID != "u-123" || ev.SubscriptionID != "sub_99" ||
		ev.ProviderVariantID != "555" || ev.CustomerID != "777" {
		t.Fatalf("bad fields: %+v", ev)
	}
	if ev.PortalURL != "https://portal.example/abc" {
		t.Fatalf("portal url=%q", ev.PortalURL)
	}
	if ev.RenewsAt == nil {
		t.Fatal("renews_at should parse")
	}
}

func TestParseWebhook_CancelledIsEnded(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_cancelled", "custom_data": {"user_id": "u-1"}},
		"data": {"id": "sub_1", "attributes": {"status": "cancelled", "variant_id": 1}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventSubscriptionEnded {
		t.Fatalf("cancelled must map to ended, got %s", ev.Type)
	}
}

func TestParseWebhook_UpdatedButInactiveIsEnded(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	// subscription_updated with a non-active status → treat as ended (access lost).
	body := []byte(`{
		"meta": {"event_name": "subscription_updated", "custom_data": {"user_id": "u-1"}},
		"data": {"id": "sub_1", "attributes": {"status": "expired", "variant_id": 1}}
	}`)
	ev, _ := ls.ParseWebhook(body)
	if ev.Type != EventSubscriptionEnded {
		t.Fatalf("updated+expired must be ended, got %s", ev.Type)
	}
}

func TestParseWebhook_OrderIgnored(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{"meta":{"event_name":"order_created"},"data":{"id":"ord_1","attributes":{}}}`)
	ev, _ := ls.ParseWebhook(body)
	if ev.Type != EventIgnored {
		t.Fatalf("order_created must be ignored in phase 1, got %s", ev.Type)
	}
}

func TestNewLemonSqueezy_DisabledWhenUnconfigured(t *testing.T) {
	if NewLemonSqueezy("", "1", "s") != nil {
		t.Fatal("no api key → nil provider")
	}
	if NewLemonSqueezy("k", "", "s") != nil {
		t.Fatal("no store id → nil provider")
	}
}

func TestParseWebhook_OrderPaidCreditsPack(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	// order_created carries the purchased variant in first_order_item, and the
	// paid status gates crediting.
	body := []byte(`{
		"meta": {"event_name": "order_created", "custom_data": {"user_id": "u-9"}},
		"data": {"id": "ord_5", "attributes": {"status": "paid", "first_order_item": {"variant_id": 888}}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventOrderPaid {
		t.Fatalf("type=%s want order_paid", ev.Type)
	}
	if ev.ProviderVariantID != "888" || ev.UserID != "u-9" || ev.SubscriptionID != "ord_5" {
		t.Fatalf("bad order fields: %+v", ev)
	}
}

func TestParseWebhook_OrgAttributedSubscription(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	// An org checkout tags organization_id instead of user_id, and
	// subscription_created carries first_subscription_item.id — the id an
	// org's later seat-quantity PATCH needs.
	body := []byte(`{
		"meta": {"event_name": "subscription_created", "custom_data": {"organization_id": "org-abc"}},
		"data": {"id": "sub_org_1", "attributes": {
			"status": "active", "variant_id": 555, "customer_id": 777,
			"first_subscription_item": {"id": 42, "quantity": 3}
		}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventSubscriptionActive {
		t.Fatalf("type=%s want active", ev.Type)
	}
	if ev.OrganizationID != "org-abc" {
		t.Fatalf("organization_id=%q want org-abc", ev.OrganizationID)
	}
	if ev.UserID != "" {
		t.Fatalf("user_id should be empty for an org-attributed event, got %q", ev.UserID)
	}
	if ev.SubscriptionItemID != "42" {
		t.Fatalf("subscription_item_id=%q want 42", ev.SubscriptionItemID)
	}
}

func TestParseWebhook_UserSubscriptionHasNoOrganizationID(t *testing.T) {
	// Regression: an ordinary individual-billing event must never pick up an
	// organization id (would misroute it to org billing in applyEvent).
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_created", "custom_data": {"user_id": "u-123"}},
		"data": {"id": "sub_99", "attributes": {"status": "active", "variant_id": 555}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.OrganizationID != "" {
		t.Fatalf("organization_id should be empty for a user-attributed event, got %q", ev.OrganizationID)
	}
	if ev.SubscriptionItemID != "" {
		t.Fatalf("subscription_item_id should be empty when the payload omits it, got %q", ev.SubscriptionItemID)
	}
}

func TestParseWebhook_SubscriptionItemIDPreservedWhenOmittedLater(t *testing.T) {
	// subscription_updated (a later lifecycle event, e.g. a dunning
	// transition) typically omits first_subscription_item — the org repo's
	// COALESCE(NULLIF($x,''), ...) is what keeps the previously-recorded id,
	// but ParseWebhook itself must report "" (not a stale/fabricated value)
	// so that COALESCE has something correct to guard.
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_updated", "custom_data": {"organization_id": "org-abc"}},
		"data": {"id": "sub_org_1", "attributes": {"status": "past_due", "variant_id": 555}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.SubscriptionItemID != "" {
		t.Fatalf("subscription_item_id=%q want empty when payload omits first_subscription_item", ev.SubscriptionItemID)
	}
}

func TestParseWebhook_OrderPendingIgnored(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{"meta":{"event_name":"order_created"},"data":{"id":"ord_1","attributes":{"status":"pending","first_order_item":{"variant_id":1}}}}`)
	ev, _ := ls.ParseWebhook(body)
	if ev.Type != EventIgnored {
		t.Fatalf("unpaid order must be ignored, got %s", ev.Type)
	}
}

// subscription_payment_* events deliver a "subscription-invoices" object,
// not a "subscriptions" one: data.id is the INVOICE's own id, the real link
// back to the subscription is attributes.subscription_id. Getting this wrong
// silently breaks the webhook-attribution fallback (Service.applyEvent),
// since it keys entirely on ev.SubscriptionID.
func TestParseWebhook_PaymentFailedUsesInvoiceSubscriptionID(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_payment_failed", "custom_data": {"user_id": "u-1"}},
		"data": {"id": "invoice_999", "attributes": {
			"status": "pending", "subscription_id": 42, "total": 999, "currency": "USD",
			"billing_reason": "renewal", "urls": {"invoice_url": "https://ls.example/invoice/999"}
		}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventPaymentFailed {
		t.Fatalf("type=%s want payment_failed", ev.Type)
	}
	if ev.SubscriptionID != "42" {
		t.Fatalf("subscription_id=%q want the invoice's subscription_id attribute (42), not data.id", ev.SubscriptionID)
	}
	if ev.AmountCents != 999 || ev.Currency != "USD" || ev.BillingReason != "renewal" {
		t.Fatalf("bad invoice fields: %+v", ev)
	}
	if ev.InvoiceURL != "https://ls.example/invoice/999" {
		t.Fatalf("invoice_url=%q", ev.InvoiceURL)
	}
}

func TestParseWebhook_PaymentSucceededAndRecovered(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	for _, tc := range []struct {
		event string
		want  EventType
	}{
		{"subscription_payment_success", EventPaymentSucceeded},
		{"subscription_payment_recovered", EventPaymentRecovered},
	} {
		body := []byte(`{
			"meta": {"event_name": "` + tc.event + `", "custom_data": {"user_id": "u-1"}},
			"data": {"id": "invoice_1", "attributes": {"status": "paid", "subscription_id": 7}}
		}`)
		ev, err := ls.ParseWebhook(body)
		if err != nil {
			t.Fatal(err)
		}
		if ev.Type != tc.want {
			t.Fatalf("%s: type=%s want %s", tc.event, ev.Type, tc.want)
		}
		if ev.SubscriptionID != "7" {
			t.Fatalf("%s: subscription_id=%q want 7", tc.event, ev.SubscriptionID)
		}
	}
}

// A refunded subscription payment still ends access, but the fix must not
// regress the subscription-id extraction while doing it — the previous
// implementation used data.id (the invoice's id) for every subscription_*
// event uniformly, which happened to be correct only for the non-invoice
// ones.
func TestParseWebhook_SubscriptionPaymentRefundedUsesInvoiceSubscriptionID(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_payment_refunded", "custom_data": {"organization_id": "org-1"}},
		"data": {"id": "invoice_5", "attributes": {"status": "refunded", "subscription_id": 88}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventSubscriptionEnded {
		t.Fatalf("type=%s want ended", ev.Type)
	}
	if ev.SubscriptionID != "88" {
		t.Fatalf("subscription_id=%q want 88 (the invoice's subscription_id attribute)", ev.SubscriptionID)
	}
}

// subscription_plan_changed doesn't appear on LemonSqueezy's documented
// event list — plan changes are delivered via subscription_updated instead
// (which already carries the new variant_id). Confirms it now falls to
// EventIgnored rather than being treated as a real event.
func TestParseWebhook_UnknownSubscriptionPlanChangedIgnored(t *testing.T) {
	ls := NewLemonSqueezy("k", "1", "s")
	body := []byte(`{
		"meta": {"event_name": "subscription_plan_changed", "custom_data": {"user_id": "u-1"}},
		"data": {"id": "sub_1", "attributes": {"status": "active", "variant_id": 2}}
	}`)
	ev, err := ls.ParseWebhook(body)
	if err != nil {
		t.Fatal(err)
	}
	if ev.Type != EventIgnored {
		t.Fatalf("type=%s want ignored (not a real LemonSqueezy event)", ev.Type)
	}
}

// The 5-hour session cap must be real: a subscriber cannot spend the whole
// month in one window. Activation used to write the monthly allowance into the
// session cap, which made the window unable to bind at all.
