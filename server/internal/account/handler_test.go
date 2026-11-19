package account

import (
	"testing"

	billingModel "server/internal/billing/model"
)

func TestGPT56LunaIsTheOnlyFreeTierModel(t *testing.T) {
	if got := minTierFor("gpt-5.6-luna"); got != billingModel.TierFree {
		t.Fatalf("gpt-5.6-luna tier = %q, want %q", got, billingModel.TierFree)
	}
	for _, model := range []string{"gpt-5.4-mini", "gpt-5.6-terra", "gpt-5.6-sol"} {
		if got := minTierFor(model); got != billingModel.TierPro {
			t.Fatalf("%s tier = %q, want %q", model, got, billingModel.TierPro)
		}
	}
}
