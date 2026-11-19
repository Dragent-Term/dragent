package payments

import (
	"context"
	"database/sql"
	"testing"

	"github.com/google/uuid"
)

// fakeRepo embeds a nil Repository so only the methods a given test actually
// exercises need overriding — a call to anything else panics loudly rather
// than silently returning a zero value.
type fakeRepo struct {
	Repository

	endSubscriptionCalls   []uuid.UUID
	userIDBySubscriptionID func(ctx context.Context, subscriptionID string) (uuid.UUID, error)

	addDepositCreditsCalls []struct{ userID, orgID uuid.UUID }
}

func (f *fakeRepo) EndSubscription(ctx context.Context, userID uuid.UUID, status string) error {
	f.endSubscriptionCalls = append(f.endSubscriptionCalls, userID)
	return nil
}

func (f *fakeRepo) UserIDBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
	return f.userIDBySubscriptionID(ctx, subscriptionID)
}

func (f *fakeRepo) GetCreditPackByVariant(ctx context.Context, variantID string) (*CreditPack, error) {
	return &CreditPack{ID: "pack-1", CreditsMc: 1000, PriceCents: 1000, Currency: "USD", DisplayName: "$10 credits"}, nil
}

func (f *fakeRepo) AddDepositCredits(ctx context.Context, userID, orgID uuid.UUID, creditsMc int64, description string) error {
	f.addDepositCreditsCalls = append(f.addDepositCreditsCalls, struct{ userID, orgID uuid.UUID }{userID, orgID})
	return nil
}

func (f *fakeRepo) RecordReceipt(ctx context.Context, r Receipt) error { return nil }

// fakeOrgApplier records calls and lets each test script its resolution/apply
// behavior independently — the two are deliberately separate calls on the
// real interface, so the fake mirrors that.
type fakeOrgApplier struct {
	orgIDBySubID func(ctx context.Context, subscriptionID string) (uuid.UUID, error)
	applyCalls   []*WebhookEvent
	applyErr     error
}

func (f *fakeOrgApplier) ApplyOrgEvent(ctx context.Context, ev *WebhookEvent) error {
	f.applyCalls = append(f.applyCalls, ev)
	return f.applyErr
}

func (f *fakeOrgApplier) OrganizationIDBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
	return f.orgIDBySubID(ctx, subscriptionID)
}

func TestApplyEvent_OrgAttributedFastPathSkipsSubscriptionLookup(t *testing.T) {
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			t.Fatal("fast path (organization_id already set) must not call the subscription-id fallback")
			return uuid.Nil, nil
		},
	}
	svc := &Service{repo: &fakeRepo{}, orgApplier: org}
	ev := &WebhookEvent{Type: EventSubscriptionEnded, OrganizationID: uuid.New().String(), SubscriptionID: "sub_1"}

	if err := svc.applyEvent(context.Background(), ev, nil); err != nil {
		t.Fatal(err)
	}
	if len(org.applyCalls) != 1 {
		t.Fatalf("ApplyOrgEvent calls=%d want 1", len(org.applyCalls))
	}
}

func TestApplyEvent_UserAttributedFastPath(t *testing.T) {
	uid := uuid.New()
	repo := &fakeRepo{}
	svc := &Service{repo: repo}
	ev := &WebhookEvent{Type: EventSubscriptionEnded, SubscriptionID: "sub_1"}

	if err := svc.applyEvent(context.Background(), ev, &uid); err != nil {
		t.Fatal(err)
	}
	if len(repo.endSubscriptionCalls) != 1 || repo.endSubscriptionCalls[0] != uid {
		t.Fatalf("EndSubscription calls=%v want [%s]", repo.endSubscriptionCalls, uid)
	}
}

// The scenario this whole fallback exists for: a subscription_payment_*
// webhook arrives with no user_id/organization_id in custom_data (unproven
// but unconfirmed-safe per LemonSqueezy's own docs), and the subscription
// turns out to belong to an org.
func TestApplyEvent_FallbackResolvesOrgBySubscriptionID(t *testing.T) {
	orgID := uuid.New()
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			if subscriptionID != "sub_42" {
				t.Fatalf("subscriptionID=%q want sub_42", subscriptionID)
			}
			return orgID, nil
		},
	}
	repo := &fakeRepo{
		userIDBySubscriptionID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			t.Fatal("org resolution succeeded — must not also try the individual-user fallback")
			return uuid.Nil, nil
		},
	}
	svc := &Service{repo: repo, orgApplier: org}
	ev := &WebhookEvent{Type: EventSubscriptionEnded, SubscriptionID: "sub_42"}

	if err := svc.applyEvent(context.Background(), ev, nil); err != nil {
		t.Fatal(err)
	}
	if len(org.applyCalls) != 1 {
		t.Fatalf("ApplyOrgEvent calls=%d want 1", len(org.applyCalls))
	}
	if org.applyCalls[0].OrganizationID != orgID.String() {
		t.Fatalf("ApplyOrgEvent's ev.OrganizationID=%q want %s (resolved org must be written back onto the event)",
			org.applyCalls[0].OrganizationID, orgID)
	}
}

// Same scenario, but the subscription belongs to an individual, not an org.
func TestApplyEvent_FallbackResolvesUserBySubscriptionIDWhenNotAnOrg(t *testing.T) {
	uid := uuid.New()
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			return uuid.Nil, sql.ErrNoRows
		},
	}
	repo := &fakeRepo{
		userIDBySubscriptionID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			if subscriptionID != "sub_7" {
				t.Fatalf("subscriptionID=%q want sub_7", subscriptionID)
			}
			return uid, nil
		},
	}
	svc := &Service{repo: repo, orgApplier: org}
	ev := &WebhookEvent{Type: EventSubscriptionEnded, SubscriptionID: "sub_7"}

	if err := svc.applyEvent(context.Background(), ev, nil); err != nil {
		t.Fatal(err)
	}
	if len(repo.endSubscriptionCalls) != 1 || repo.endSubscriptionCalls[0] != uid {
		t.Fatalf("EndSubscription calls=%v want [%s]", repo.endSubscriptionCalls, uid)
	}
	if len(org.applyCalls) != 0 {
		t.Fatalf("ApplyOrgEvent must not be called once org resolution came back sql.ErrNoRows")
	}
}

// Neither lookup matches anything — must be a benign no-op (recorded for
// audit via ClaimEvent already, at the HandleWebhook layer), not an error a
// retry could ever fix.
func TestApplyEvent_FallbackNoMatchIsBenign(t *testing.T) {
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			return uuid.Nil, sql.ErrNoRows
		},
	}
	repo := &fakeRepo{
		userIDBySubscriptionID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			return uuid.Nil, sql.ErrNoRows
		},
	}
	svc := &Service{repo: repo, orgApplier: org}
	ev := &WebhookEvent{Type: EventSubscriptionEnded, SubscriptionID: "sub_unknown"}

	if err := svc.applyEvent(context.Background(), ev, nil); err != nil {
		t.Fatalf("unattributable event must not error: %v", err)
	}
}

// A real failure resolving the org (not "no match") must propagate as an
// error the provider will retry — and must NOT be misread as "not an org
// subscription" and silently redirected to the individual-user fallback,
// which is exactly the ambiguity a plain ApplyOrgEvent-error check would
// have introduced.
func TestApplyEvent_FallbackOrgResolutionRealErrorPropagates(t *testing.T) {
	boom := context.DeadlineExceeded
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			return uuid.Nil, boom
		},
	}
	repo := &fakeRepo{
		userIDBySubscriptionID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			t.Fatal("a real org-resolution error must not fall through to the individual fallback")
			return uuid.Nil, nil
		},
	}
	svc := &Service{repo: repo, orgApplier: org}
	ev := &WebhookEvent{Type: EventSubscriptionEnded, SubscriptionID: "sub_1"}

	if err := svc.applyEvent(context.Background(), ev, nil); err == nil {
		t.Fatal("expected an error to propagate")
	}
}

func TestApplyEvent_NoSubscriptionIDAndUnattributedIsBenign(t *testing.T) {
	svc := &Service{repo: &fakeRepo{}}
	ev := &WebhookEvent{Type: EventSubscriptionEnded}

	if err := svc.applyEvent(context.Background(), ev, nil); err != nil {
		t.Fatalf("no subscription id to fall back on must not error: %v", err)
	}
}

func TestApplyEvent_IgnoredEventSkipsEverything(t *testing.T) {
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			t.Fatal("EventIgnored must short-circuit before any attribution attempt")
			return uuid.Nil, nil
		},
	}
	svc := &Service{repo: &fakeRepo{}, orgApplier: org}
	ev := &WebhookEvent{Type: EventIgnored, SubscriptionID: "sub_1"}

	if err := svc.applyEvent(context.Background(), ev, nil); err != nil {
		t.Fatal(err)
	}
}

// A credit-pack purchase carries ev.OrganizationID too (which org's balance
// to credit), a different meaning from every other event type (where its
// presence means "this whole event belongs to org billing instead"). It
// must stay routed to the user-attributed path, not get redirected into
// applyOrgEvent, which has no concept of credit packs at all.
func TestApplyEvent_OrderPaidStaysUserAttributedDespiteOrganizationIDBeingSet(t *testing.T) {
	uid := uuid.New()
	orgID := uuid.New()
	org := &fakeOrgApplier{
		orgIDBySubID: func(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
			t.Fatal("EventOrderPaid must never reach org attribution")
			return uuid.Nil, nil
		},
		applyErr: nil,
	}
	repo := &fakeRepo{}
	svc := &Service{repo: repo, orgApplier: org}
	ev := &WebhookEvent{Type: EventOrderPaid, OrganizationID: orgID.String(), ProviderVariantID: "v1"}

	if err := svc.applyEvent(context.Background(), ev, &uid); err != nil {
		t.Fatal(err)
	}
	if len(org.applyCalls) != 0 {
		t.Fatalf("ApplyOrgEvent must not be called for a credit-pack purchase, got %d calls", len(org.applyCalls))
	}
	if len(repo.addDepositCreditsCalls) != 1 {
		t.Fatalf("AddDepositCredits calls=%d want 1", len(repo.addDepositCreditsCalls))
	}
	got := repo.addDepositCreditsCalls[0]
	if got.userID != uid || got.orgID != orgID {
		t.Fatalf("AddDepositCredits called with (user=%s, org=%s), want (user=%s, org=%s)", got.userID, got.orgID, uid, orgID)
	}
}
