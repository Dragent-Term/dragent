package model

import (
	"errors"
	"testing"
	"time"
)

func baseBalance() *UserBalance {
	now := time.Now().UTC()
	return &UserBalance{
		Tier:           TierFree,
		FreeMonthlyCap: 375000, FreeMonthlyUsed: 0,
		FreeMonthlyResetAt: &now,
	}
}

func TestCanUsePremiumModel(t *testing.T) {
	const min = 5000
	cases := []struct {
		name string
		bal  UserBalance
		want bool
	}{
		{"paid allowance", UserBalance{IsPaid: true, PaidBalanceMc: 10_000}, true},
		{"stale paid allowance on free account", UserBalance{IsPaid: false, PaidBalanceMc: 10_000}, false},
		{"credits on + funded", UserBalance{AllowCreditUsage: true, DepositBalanceMc: 10_000}, true},
		{"credits off + funded", UserBalance{AllowCreditUsage: false, DepositBalanceMc: 10_000}, false},
		{"credits on + empty", UserBalance{AllowCreditUsage: true, DepositBalanceMc: 100}, false},
		// The free allowance NEVER unlocks a premium model — it only backs free.
		{"only free allowance", UserBalance{FreeMonthlyCap: 1_500_000}, false},
		{"nothing", UserBalance{}, false},
	}
	for _, c := range cases {
		if got := c.bal.CanUsePremiumModel(min); got != c.want {
			t.Errorf("%s: CanUsePremiumModel=%v, want %v", c.name, got, c.want)
		}
	}
}

func TestCalculateCost_SplitsCached(t *testing.T) {
	mp := &ModelPrice{
		ModelName:                 "m",
		InputRatePerMillion:       3_000_000,
		CachedInputRatePerMillion: 300_000, // 10x cheaper
		OutputRatePerMillion:      15_000_000,
		IsActive:                  true,
	}
	// 1M input, 1M cached, 1M output.
	cost, err := mp.CalculateCost(1_000_000, 1_000_000, 1_000_000)
	if err != nil {
		t.Fatal(err)
	}
	want := int64(3_000_000 + 300_000 + 15_000_000)
	if cost != want {
		t.Fatalf("cost got %d want %d", cost, want)
	}
}

func TestCalculateCost_LongContextTier(t *testing.T) {
	// A prompt that crosses the long-context threshold must bill EVERY token at
	// the higher tier (input+cached counted toward the threshold; output too).
	mp := &ModelPrice{
		ModelName:                            "m",
		InputRatePerMillion:                  2_000_000,
		CachedInputRatePerMillion:            500_000,
		OutputRatePerMillion:                 6_000_000,
		LongContextThreshold:                 200_000,
		LongContextInputRatePerMillion:       4_000_000,
		LongContextCachedInputRatePerMillion: 1_000_000,
		LongContextOutputRatePerMillion:      12_000_000,
		IsActive:                             true,
	}
	// 150k fresh + 100k cached = 250k prompt > 200k threshold → long-context.
	cost, err := mp.CalculateCost(150_000, 100_000, 10_000)
	if err != nil {
		t.Fatal(err)
	}
	want := int64(150_000*4_000_000+100_000*1_000_000+10_000*12_000_000) / 1_000_000
	if cost != want {
		t.Fatalf("long-context cost got %d want %d", cost, want)
	}
	// Just under the threshold stays on the standard tier.
	cost, err = mp.CalculateCost(150_000, 40_000, 10_000) // 190k < 200k
	if err != nil {
		t.Fatal(err)
	}
	want = int64(150_000*2_000_000+40_000*500_000+10_000*6_000_000) / 1_000_000
	if cost != want {
		t.Fatalf("standard-tier cost got %d want %d", cost, want)
	}
}

// The markup multiplies provider cost into what the user is charged, so a
// ratio at or below 1 sells every request at or below cost — silently, and
// worse the more the product is used. There is no valid configuration there.
func TestValidateMarkup(t *testing.T) {
	valid := [][2]int64{
		{5, 2},   // 2.5x, the seeded default
		{19, 10}, // 1.9x
		{3, 2},   // 1.5x
		{101, 100},
	}
	for _, v := range valid {
		if err := ValidateMarkup(v[0], v[1]); err != nil {
			t.Fatalf("%d/%d should be valid, got %v", v[0], v[1], err)
		}
	}

	invalid := [][2]int64{
		{1, 1},  // exactly cost — no margin
		{1, 2},  // 0.5x — sold at a loss
		{2, 5},  // 0.4x
		{0, 2},  // zero numerator: free
		{5, 0},  // zero denominator: divide by zero
		{-5, 2}, // negative
		{5, -2},
	}
	for _, v := range invalid {
		if err := ValidateMarkup(v[0], v[1]); err == nil {
			t.Fatalf("%d/%d must be rejected", v[0], v[1])
		}
	}
}

// Subscriptions bill on the same calendar day each month. A flat 30 days would
// drift and, in any 31-day month, fall due a day early — which for a
// clock-driven renewal means granting an allowance before its payment.
func TestAddOneMonth(t *testing.T) {
	d := func(y int, m time.Month, day int) time.Time {
		return time.Date(y, m, day, 9, 30, 0, 0, time.UTC)
	}
	cases := []struct{ from, want time.Time }{
		// Same day next month, the ordinary case.
		{d(2026, time.January, 15), d(2026, time.February, 15)},
		{d(2026, time.July, 20), d(2026, time.August, 20)},
		// Short target month clamps to its last day, never overflowing into
		// the month after (which is what AddDate would do).
		{d(2026, time.January, 31), d(2026, time.February, 28)},
		{d(2028, time.January, 31), d(2028, time.February, 29)}, // leap year
		{d(2026, time.March, 31), d(2026, time.April, 30)},
		// Year boundary.
		{d(2026, time.December, 31), d(2027, time.January, 31)},
		// A 31-day month must advance a full 31 days, not 30.
		{d(2026, time.May, 1), d(2026, time.June, 1)},
	}
	for _, c := range cases {
		if got := AddOneMonth(c.from); !got.Equal(c.want) {
			t.Errorf("AddOneMonth(%s) = %s, want %s",
				c.from.Format("2006-01-02"), got.Format("2006-01-02"), c.want.Format("2006-01-02"))
		}
	}

	// Time of day is preserved — the window resets at the hour it began.
	got := AddOneMonth(d(2026, time.January, 15))
	if got.Hour() != 9 || got.Minute() != 30 {
		t.Errorf("time of day lost: %s", got.Format(time.RFC3339))
	}
}

// ExhaustionError reports the most actionable blocker: opt-in credits first,
// then the appropriate allowance with its reset.
func TestExhaustionError_CreditsAndAllowance(t *testing.T) {
	now := time.Now().UTC()
	reset := now.Add(10 * 24 * time.Hour)

	// Credits available but not enabled → prompt to enable them.
	b := &UserBalance{Tier: TierPro, IsPaid: true, DepositBalanceMc: 50_000,
		AllowCreditUsage: false, PaidMonthlyResetAt: &reset}
	if err := b.ExhaustionError(10_000); !errorsIs(err, ErrCreditsNotEnabled) {
		t.Fatalf("want ErrCreditsNotEnabled, got %v", err)
	}
	// Paid, spent, no credits → monthly-allowance message with renewal date.
	b = &UserBalance{Tier: TierPro, IsPaid: true, PaidMonthlyResetAt: &reset}
	if err := b.ExhaustionError(10_000); !errorsIs(err, ErrPaidCreditsExhausted) {
		t.Fatalf("want ErrPaidCreditsExhausted, got %v", err)
	}
	// Free tier, monthly allowance spent → free-allowance message.
	b = &UserBalance{Tier: TierFree, FreeMonthlyResetAt: &now}
	if err := b.ExhaustionError(10_000); !errorsIs(err, ErrFreeCreditsExhausted) {
		t.Fatalf("want ErrFreeCreditsExhausted, got %v", err)
	}
}

func errorsIs(err, target error) bool { return err != nil && errors.Is(err, target) }
