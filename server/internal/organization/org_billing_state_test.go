package organization

import "testing"

func TestOrgBillingIsPaid(t *testing.T) {
	if (OrgBilling{}).IsPaid() {
		t.Fatal("zero-value OrgBilling (no plan) must not be paid")
	}
	if !(OrgBilling{PlanID: "pro-monthly"}).IsPaid() {
		t.Fatal("a set plan id must be paid")
	}
}

func TestOrgBillingLifecycleState(t *testing.T) {
	cases := []struct {
		name string
		b    OrgBilling
		want LifecycleState
	}{
		{"zero value", OrgBilling{}, LifecycleNeverSubscribed},
		{"active paid", OrgBilling{PlanID: "pro-monthly", SubscriptionStatus: "active"}, LifecycleActive},
		{"on trial counts as active, not a distress state",
			OrgBilling{PlanID: "pro-monthly", SubscriptionStatus: "on_trial"}, LifecycleActive},
		{"paid but retrying a failed renewal charge",
			OrgBilling{PlanID: "pro-monthly", SubscriptionStatus: "past_due"}, LifecyclePastDue},
		{"retries exhausted / cancelled: plan cleared, subscription id kept for history",
			OrgBilling{LemonSqueezySubscriptionID: "sub_123", SubscriptionStatus: "expired"}, LifecycleLapsed},
		{"refunded subscription payment: same shape as any other ended subscription",
			OrgBilling{LemonSqueezySubscriptionID: "sub_123", SubscriptionStatus: "refunded"}, LifecycleLapsed},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := tc.b.LifecycleState(); got != tc.want {
				t.Fatalf("LifecycleState()=%s want %s", got, tc.want)
			}
		})
	}
}

// The distinction this whole type exists for: a never-subscribed org and a
// lapsed one both fail IsPaid(), but must not be confused with each other —
// only LemonSqueezySubscriptionID's presence tells them apart.
func TestOrgBillingLifecycleState_NeverSubscribedVsLapsedAreDistinguishable(t *testing.T) {
	neverSubscribed := OrgBilling{}
	lapsed := OrgBilling{LemonSqueezySubscriptionID: "sub_123"}

	if neverSubscribed.IsPaid() || lapsed.IsPaid() {
		t.Fatal("both must be unpaid — that's exactly the ambiguity LifecycleState resolves")
	}
	if neverSubscribed.LifecycleState() == lapsed.LifecycleState() {
		t.Fatal("never-subscribed and lapsed must map to different states")
	}
}
