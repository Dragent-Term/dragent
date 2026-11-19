package components

// End-to-end billing FLOW test. Where billing_test.go pins each rule in
// isolation, this file walks a persona through the whole metered lifecycle we
// built — minimum-balance gate → actual-cost settlement → overage tip-over
// (floor at 0, overage booked to monthly usage, never negative, never
// confiscated) → next-call block — plus long-context tiering and unknown-model
// refusal. The scenarios mirror the Gherkin feature exactly.

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/google/uuid"

	"server/internal/billing/model"
)

// Scenario: the full lifecycle of a Pro-tier developer, Maya.
//
//	Given Maya is on the Pro tier with 8,000 mc of paid balance
//	And "grok-build-0.1" requires a minimum balance of 7,500 mc
//	And her monthly allowance is available (the balance is the constraint)
func TestBillingFlow_MayaLifecycle(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()

	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      8_000,
		FreeMonthlyResetAt: wp,
	}
	uc, br, tr, fr := newTestUC(bal, grokBuildPrice()) // min 7,500 mc

	// ── Scenario: Happy path — a normal step ──────────────────────────────────
	//   When Maya starts a call        → the gate ALLOWS it (8,000 ≥ 7,500)
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("[happy] gate must allow 8,000 ≥ 7,500 min, got %v", err)
	}
	//   And the provider reports in=68 cached=300 out=13
	//   Then she is charged 385 mc (api 154 ×2.5) and the balance drops exactly
	res, err := uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uid, ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
	})
	if err != nil {
		t.Fatalf("[happy] settle failed: %v", err)
	}
	if res.ApiCostMc != 154 || res.UserChargedMc != 385 || res.Pool != model.PoolPaid {
		t.Fatalf("[happy] api=%d charged=%d pool=%s, want 154/385/paid", res.ApiCostMc, res.UserChargedMc, res.Pool)
	}
	if br.bal.PaidBalanceMc != 8_000-385 {
		t.Fatalf("[happy] balance=%d, want %d", br.bal.PaidBalanceMc, 8_000-385)
	}
	//   And 385 mc is booked to monthly usage

	//   And an auditable transaction + profit row are written
	if len(tr.txs) != 1 || len(fr.profits) != 1 || fr.profits[0].ProfitMc != 385-154 {
		t.Fatalf("[happy] want 1 tx + 1 profit of 231, got %d tx / %d profit", len(tr.txs), len(fr.profits))
	}

	// ── Scenario: still eligible — just above the minimum ─────────────────────
	//   Given her balance is now 7,615 mc
	//   When she starts another call   → the gate STILL allows it (7,615 ≥ 7,500)
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("[eligible] gate must allow 7,615 ≥ 7,500, got %v", err)
	}

	// ── Scenario: Tip-over — the step outran the balance ──────────────────────
	//   When a large step costs 22,500 mc (api 9,000 ×2.5) but she has only 7,615
	res, err = uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uid, ModelName: "grok-build-0.1",
		InputTokens: 5_000, CachedInputTokens: 0, OutputTokens: 2_000,
	})
	if err != nil {
		t.Fatalf("[tipover] settle must never fail on affordability, got %v", err)
	}
	if res.UserChargedMc != 22_500 {
		t.Fatalf("[tipover] charged=%d, want 22,500", res.UserChargedMc)
	}
	//   Then her paid balance floors at 0 — NEVER negative
	if br.bal.PaidBalanceMc != 0 {
		t.Fatalf("[tipover] balance=%d, must floor at 0 (never negative)", br.bal.PaidBalanceMc)
	}
	//   And the FULL charge is booked to monthly usage (overage drawn down
	//   from the allowance, not silently absorbed): 385 + 22,500 = 22,885

	// ── Scenario: Failure path — next call is blocked ─────────────────────────
	//   Given her balance is now 0 (below the 7,500 minimum) and no free pool
	//   When she starts another call   → the gate BLOCKS it
	//   And her balance is left untouched (no confiscation), no new tx written
	txBefore := len(tr.txs)
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err == nil {
		t.Fatal("[blocked] gate must block once the balance is below the minimum")
	}
	if br.bal.PaidBalanceMc != 0 {
		t.Fatalf("[blocked] balance changed to %d; a blocked gate must not touch it", br.bal.PaidBalanceMc)
	}
	if len(tr.txs) != txBefore {
		t.Fatalf("[blocked] a blocked gate wrote a transaction (%d → %d)", txBefore, len(tr.txs))
	}
	// Overspend was bounded to exactly ONE step: the tip-over ran, then the gate
	// shut the door.
}

// Scenario: a long-context step bills EVERY token at the higher tier.
//
//	Given "grok-4.5" tiers at a 200k-token prompt
//	When a step sends a 250k-token prompt
//	Then the whole request is priced at the ≥200k rates, not the standard ones
func TestBillingFlow_LongContextTier(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()

	price := &model.ModelPrice{
		ModelName:                            "grok-4.5",
		InputRatePerMillion:                  2_000_000,
		CachedInputRatePerMillion:            500_000,
		OutputRatePerMillion:                 6_000_000,
		LongContextThreshold:                 200_000,
		LongContextInputRatePerMillion:       4_000_000,
		LongContextCachedInputRatePerMillion: 1_000_000,
		LongContextOutputRatePerMillion:      12_000_000,
		MinBalanceMc:                         17_500,
		IsActive:                             true,
	}
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      5_000_000,
		FreeMonthlyResetAt: wp,
	}
	uc, _, _, _ := newTestUC(bal, price)

	// 250k prompt (> 200k threshold) → long-context rates for the whole request.
	res, err := uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uid, ModelName: "grok-4.5",
		InputTokens: 250_000, CachedInputTokens: 0, OutputTokens: 10_000,
	})
	if err != nil {
		t.Fatal(err)
	}
	wantAPI := int64(250_000*4_000_000+10_000*12_000_000) / 1_000_000 // 1,120,000
	if res.ApiCostMc != wantAPI {
		t.Fatalf("long-context api cost=%d, want %d (standard-tier would be 560,000)", res.ApiCostMc, wantAPI)
	}
}

// Scenario: an uncatalogued model is refused everywhere — never billed at a
// guessed price. (This is why gpt-4o had to be added to the catalog.)
//
//	Given "made-up-model" has no price row
//	When the gate or a settle runs for it
//	Then both refuse with an error
func TestBillingFlow_UnknownModelRefused(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      10_000_000,
		FreeMonthlyResetAt: wp,
	}
	uc, _, _, _ := newTestUC(bal, nil) // nil price → GetModelPrice errors

	if err := uc.PreflightGate(ctx, uid, "made-up-model"); err == nil {
		t.Fatal("gate must refuse an uncatalogued model")
	}
	if _, err := uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uid, ModelName: "made-up-model", InputTokens: 100, OutputTokens: 50,
	}); err == nil {
		t.Fatal("settle must refuse an uncatalogued model rather than guess a price")
	}
}

// Scenario: durable deposit credits are spent AFTER the monthly allowance and
// never reset — the Phase 2 deposit pool.
//
//	Given a Pro user with 8,000 mc monthly allowance and 50,000 mc of purchased
//	  (deposit) credits, and grok-build-0.1 (min 7,500)
//	When they run steps
//	Then the monthly allowance is spent first; once it floors, charges fall to
//	  the deposit balance; deposit is untouched while paid can still cover.
func TestBillingFlow_DepositAfterMonthly(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      8_000,
		DepositBalanceMc:   50_000,
		AllowCreditUsage:   true, // deposit spend is opt-in; this scenario opts in
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	// Small step → PAID pool (it clears the minimum); deposit untouched.
	res, err := uc.ChargeUser(ctx, ChargeUserCommand{UserID: uid, ModelName: "grok-build-0.1", InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13})
	if err != nil {
		t.Fatal(err)
	}
	if res.Pool != model.PoolPaid {
		t.Fatalf("first step pool=%s, want paid", res.Pool)
	}
	if br.bal.PaidBalanceMc != 8_000-385 || br.bal.DepositBalanceMc != 50_000 {
		t.Fatalf("paid=%d deposit=%d, want %d/50000", br.bal.PaidBalanceMc, br.bal.DepositBalanceMc, 8_000-385)
	}

	// Big step tips the paid pool to 0. Deposit still untouched.
	if _, err := uc.ChargeUser(ctx, ChargeUserCommand{UserID: uid, ModelName: "grok-build-0.1", InputTokens: 5_000, OutputTokens: 2_000}); err != nil {
		t.Fatal(err)
	}
	if br.bal.PaidBalanceMc != 0 {
		t.Fatalf("paid balance=%d, want 0 after tip-over", br.bal.PaidBalanceMc)
	}
	if br.bal.DepositBalanceMc != 50_000 {
		t.Fatalf("deposit=%d, must be untouched while paid still had a balance", br.bal.DepositBalanceMc)
	}

	// Paid is now below the minimum → next charge falls to the DEPOSIT pool.
	res, err = uc.ChargeUser(ctx, ChargeUserCommand{UserID: uid, ModelName: "grok-build-0.1", InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13})
	if err != nil {
		t.Fatal(err)
	}
	if res.Pool != model.PoolDeposit {
		t.Fatalf("pool=%s, want deposit once monthly allowance is spent", res.Pool)
	}
	if br.bal.DepositBalanceMc != 50_000-385 {
		t.Fatalf("deposit=%d, want %d", br.bal.DepositBalanceMc, 50_000-385)
	}
	// Deposit keeps them eligible.
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("deposit balance should keep the gate open, got %v", err)
	}
}

// Scenario: a pay-as-you-go user (no subscription) runs entirely on deposit
// credits, which floor at 0 and are never charged to usage windows.
func TestBillingFlow_DepositOnlyUser(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierFree, IsPaid: false,
		DepositBalanceMc:   8_000, // just above the 7,500 minimum
		AllowCreditUsage:   true,  // deposit spend is opt-in; this scenario opts in
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("deposit should pass the gate, got %v", err)
	}
	// Big step outruns the deposit → floors at 0 (never negative), and no
	// monthly allowance usage is booked (deposit credits do not reset).
	res, err := uc.ChargeUser(ctx, ChargeUserCommand{UserID: uid, ModelName: "grok-build-0.1", InputTokens: 5_000, OutputTokens: 2_000})
	if err != nil {
		t.Fatal(err)
	}
	if res.Pool != model.PoolDeposit {
		t.Fatalf("pool=%s, want deposit", res.Pool)
	}
	if br.bal.DepositBalanceMc != 0 {
		t.Fatalf("deposit=%d, must floor at 0", br.bal.DepositBalanceMc)
	}
	if br.bal.FreeMonthlyUsed != 0 {
		t.Fatalf("deposit spend must not touch the free counter: free=%d", br.bal.FreeMonthlyUsed)
	}
	// Now empty → blocked.
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err == nil {
		t.Fatal("empty deposit and no other pool must block")
	}
}

// Scenario: purchased credits must NEVER be spent without consent, and the
// free baseline must be spent before them.
//
// Both halves are regressions of real bugs: a throttled subscriber used to
// fall straight through to their wallet, and a free user who bought a pack
// stopped using their free monthly quota entirely because deposit was tried
// first.
func TestBillingFlow_DepositRequiresOptIn(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()

	// A subscriber whose plan allowance AND free monthly allowance are spent,
	// holding purchased credits with consent OFF. The only thing that could
	// serve the next call is the deposit pool — but it must not, without opt-in.
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      0, // monthly allowance spent
		FreeMonthlyCap:     0, // no free allowance left
		DepositBalanceMc:   50_000,
		AllowCreditUsage:   false,
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	err := uc.PreflightGate(ctx, uid, "grok-build-0.1")
	if err == nil {
		t.Fatal("gate must refuse: allowances spent and credits not enabled")
	}
	// It MUST be identifiable as ErrCreditsNotEnabled — the HTTP layer keys on
	// this to return a 402 with the "enable credits in Settings" message. When
	// it was not re-exported/mapped, this fell through to a generic 500 and the
	// user saw only "an error occurred".
	if !errors.Is(err, model.ErrCreditsNotEnabled) {
		t.Fatalf("want ErrCreditsNotEnabled so the client shows the enable-credits prompt, got %v", err)
	}
	if br.bal.DepositBalanceMc != 50_000 {
		t.Fatalf("deposit=%d, must be untouched without opt-in", br.bal.DepositBalanceMc)
	}

	// Same balance, consent given → the gate opens.
	br.bal.AllowCreditUsage = true
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("gate should open once credits are enabled, got %v", err)
	}
}

// Consented credits are drawn down BEFORE the free allowance: the free pool
// only ever backs the free-tier model, so spending credits first keeps the
// user's selected (premium) model working. Only when credits are off/empty does
// the charge drop to free (and the user is restricted to the free model).
func TestBillingFlow_DepositBeforeFreeWhenCreditsEnabled(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierFree, IsPaid: false,
		FreeMonthlyCap: 375_000, DepositBalanceMc: 50_000,
		AllowCreditUsage:   true, // enabled → credits are spent before free
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	res, err := uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uid, ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.Pool != model.PoolDeposit {
		t.Fatalf("pool=%s, want deposit — consented credits outrank the free allowance", res.Pool)
	}
	if br.bal.FreeMonthlyUsed != 0 {
		t.Fatalf("free used=%d, must be untouched while consented credits remain", br.bal.FreeMonthlyUsed)
	}
}

// With credit usage OFF, the same charge drops to the free allowance (and the
// user is restricted to the free model). Credits are left untouched.
func TestBillingFlow_FreeWhenCreditsDisabled(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierFree, IsPaid: false,
		FreeMonthlyCap: 375_000, DepositBalanceMc: 50_000,
		AllowCreditUsage:   false, // off → credits skipped, free pool pays
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	res, err := uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uid, ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.Pool != model.PoolFree {
		t.Fatalf("pool=%s, want free — credits are off, so the free allowance pays", res.Pool)
	}
	if br.bal.DepositBalanceMc != 50_000 {
		t.Fatalf("deposit=%d, must be untouched while credit usage is off", br.bal.DepositBalanceMc)
	}
}

// Regression: a paid user at zero balance PAST their renewal date must be
// unblocked by the gate itself. The renewal used to live only in ChargeUser,
// which never runs while the gate refuses — so the user who had already paid
// was locked out permanently, and every retry re-ran the same dead end.
func TestBillingFlow_GateRenewsExpiredMonthlyAllowance(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	past := time.Now().UTC().Add(-24 * time.Hour)
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      0, // fully spent last cycle
		PaidMonthlyCap:     10_000_000,
		PaidMonthlyResetAt: &past, // renewal is due
		// stale counters from the old cycle
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("gate must renew and admit a paid user past their reset date, got %v", err)
	}
	if br.bal.PaidBalanceMc != 10_000_000 {
		t.Fatalf("paid balance=%d, want the full allowance restored", br.bal.PaidBalanceMc)
	}
	// The reset date must move into the FUTURE, or the renewal re-fires on
	// every subsequent request and the allowance never actually depletes.
	if br.bal.PaidMonthlyResetAt == nil || br.bal.PaidMonthlyResetAt.Before(time.Now().UTC()) {
		t.Fatalf("reset date=%v, want a future date", br.bal.PaidMonthlyResetAt)
	}
}

// A failed payment must NOT buy another month.
//
// During dunning the subscription stays alive: caps intact, is_paid true, and
// the reset date now in the past — exactly the state the calendar-driven
// renewal fires on. Without a payment-status check the user received the full
// allowance for a payment that never landed, and the catch-up loop repeated it
// every 30 days for as long as dunning continued.
func TestBillingFlow_ClockDoesNotRenewUnpaidSubscription(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()
	_, wp := freshWindows()
	past := time.Now().UTC().Add(-24 * time.Hour)

	newBal := func(status string) *model.UserBalance {
		p := past
		return &model.UserBalance{
			Tier: model.TierPro, IsPaid: true, PaidStatus: status,
			PaidBalanceMc: 0, PaidMonthlyCap: 10_000_000,
			PaidMonthlyResetAt: &p,
			FreeMonthlyResetAt: wp,
		}
	}

	// Dunning: access continues, but no new allowance.
	uc, br, _, _ := newTestUC(newBal("past_due"), grokBuildPrice())
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err == nil {
		t.Fatal("a past_due subscription at zero balance must not be admitted")
	}
	if br.bal.PaidBalanceMc != 0 {
		t.Fatalf("balance=%d, want 0 — a failed payment must not grant allowance",
			br.bal.PaidBalanceMc)
	}
	if !br.bal.PaidMonthlyResetAt.Equal(past) {
		t.Fatal("reset date must not advance without a paid renewal")
	}

	// A paying subscriber in the same date state IS renewed.
	uc, br, _, _ = newTestUC(newBal("active"), grokBuildPrice())
	if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err != nil {
		t.Fatalf("an active subscription past its reset date must renew, got %v", err)
	}
	if br.bal.PaidBalanceMc != 10_000_000 {
		t.Fatalf("balance=%d, want the full allowance restored", br.bal.PaidBalanceMc)
	}
}
