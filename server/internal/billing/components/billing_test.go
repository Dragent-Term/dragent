package components

import (
	"context"
	"database/sql"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	"server/internal/billing/model"
)

// PreflightGate skips the row write when no window advanced — these tests pin
// the advance helpers' changed-reporting that decision depends on.

func TestAdvanceFreeMonthly_ReportsChange(t *testing.T) {
	now := time.Now().UTC()

	// Nil reset → initialised (one month out) → changed.
	b := &model.UserBalance{}
	if !advanceFreeMonthly(b, now) {
		t.Fatal("nil free reset must report changed")
	}

	// Fresh window (reset is a month out) → unchanged.
	if advanceFreeMonthly(b, now.Add(time.Hour)) {
		t.Fatal("mid-window advance must report unchanged")
	}

	// Expired window → counter reset → changed.
	old := now.Add(-2 * time.Hour)
	b.FreeMonthlyResetAt = &old
	b.FreeMonthlyUsed = 123
	if !advanceFreeMonthly(b, now) {
		t.Fatal("expired month must report changed")
	}
	if b.FreeMonthlyUsed != 0 {
		t.Fatalf("expired month must reset counter, got %d", b.FreeMonthlyUsed)
	}
}

// ── Test doubles for the min-balance gate + lock-free settle ──────────────────

type fakeBalRepo struct{ bal *model.UserBalance }

func (f *fakeBalRepo) GetByUserID(_ context.Context, _, _ uuid.UUID) (*model.UserBalance, error) {
	return f.bal, nil
}
func (f *fakeBalRepo) GetByUserIDTx(_ context.Context, _ any, _, _ uuid.UUID) (*model.UserBalance, error) {
	return f.bal, nil
}
func (f *fakeBalRepo) UpdateTx(_ context.Context, _ any, b *model.UserBalance) error {
	f.bal = b
	return nil
}
func (f *fakeBalRepo) InitializeTx(_ context.Context, _ any, _, _ uuid.UUID) error { return nil }

type fakePriceRepo struct{ price *model.ModelPrice }

func (f *fakePriceRepo) GetModelPrice(_ context.Context, _ string) (*model.ModelPrice, error) {
	if f.price == nil {
		return nil, model.ErrUnknownModel
	}
	return f.price, nil
}

func (f *fakeBalRepo) SetAllowCreditUsage(_ context.Context, _, _ uuid.UUID, allow bool) error {
	f.bal.AllowCreditUsage = allow
	return nil
}
func (f *fakePriceRepo) ListUserFacingModels(_ context.Context) ([]*model.UserFacingModel, error) {
	return nil, nil
}
func (f *fakePriceRepo) UpdateModelPrice(_ context.Context, _ *model.ModelPrice) error { return nil }
func (f *fakePriceRepo) SetActive(_ context.Context, _ string, _ bool) error           { return nil }

type fakeTxRepo struct{ txs []*model.CreditTransaction }

func (f *fakeTxRepo) CreateTx(_ context.Context, _ any, t *model.CreditTransaction) (uuid.UUID, error) {
	f.txs = append(f.txs, t)
	return t.ID, nil
}
func (f *fakeTxRepo) ListByUserID(_ context.Context, _ uuid.UUID, _ int) ([]*model.CreditTransaction, error) {
	return nil, nil
}

type fakeProfitRepo struct{ profits []*model.PlatformProfit }

func (f *fakeProfitRepo) CreateProfitTx(_ context.Context, _ any, p *model.PlatformProfit) error {
	f.profits = append(f.profits, p)
	return nil
}

type fakeTxMgr struct{}

func (fakeTxMgr) WithTx(ctx context.Context, fn func(tx any) error) error { return fn(nil) }

// fakeConfig mirrors the rows migration 00015 seeds. The markup keys MUST be
// present: there is no compiled fallback, so an empty config now correctly
// blocks every charge rather than silently billing a default rate.
type fakeConfig struct{}

func (fakeConfig) Get(_ context.Context, key string) (string, error) {
	switch key {
	case "markup_numerator":
		return "5", nil
	case "markup_denominator":
		return "2", nil
	}
	return "", sql.ErrNoRows
}
func (fakeConfig) Set(_ context.Context, _, _ string) error { return nil }

func TestFreeTierModelFallbackIsGPT56Luna(t *testing.T) {
	uc, _, _, _ := newTestUC(&model.UserBalance{}, grokBuildPrice())
	if got := uc.FreeTierModel(context.Background()); got != "gpt-5.6-luna" {
		t.Fatalf("FreeTierModel() = %q, want gpt-5.6-luna", got)
	}
}

func newTestUC(bal *model.UserBalance, price *model.ModelPrice) (*BillingUseCase, *fakeBalRepo, *fakeTxRepo, *fakeProfitRepo) {
	br := &fakeBalRepo{bal: bal}
	tr := &fakeTxRepo{}
	fr := &fakeProfitRepo{}
	uc := NewBillingUseCase(br, &fakePriceRepo{price: price}, tr, fr, fakeConfig{}, fakeTxMgr{})
	return uc, br, tr, fr
}

// grokBuildPrice mirrors the real grok-build-0.1 row (min 7500mc).
func grokBuildPrice() *model.ModelPrice {
	return &model.ModelPrice{
		ModelName:                 "grok-build-0.1",
		InputRatePerMillion:       1_000_000,
		CachedInputRatePerMillion: 200_000,
		OutputRatePerMillion:      2_000_000,
		MinBalanceMc:              7_500,
		IsActive:                  true,
	}
}

// freshWindows returns now and a free-monthly reset pointer one calendar month
// out — a currently-active window whose used-counter accumulates (does not
// reset) during the test.
func freshWindows() (now time.Time, wp *time.Time) {
	now = time.Now().UTC()
	n := model.AddOneMonth(now)
	return now, &n
}

// ── Pre-flight gate: min-balance, no estimate, no drain ───────────────────────

func TestPreflightGate_BlocksBelowMinimum_WithoutTouchingBalance(t *testing.T) {
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      3_000, // below the 7500 minimum
		FreeMonthlyResetAt: wp,
	}
	uc, br, tr, _ := newTestUC(bal, grokBuildPrice())

	err := uc.PreflightGate(context.Background(), uuid.New(), "grok-build-0.1")
	if err == nil {
		t.Fatal("expected exhaustion error when balance is below the minimum")
	}
	if br.bal.PaidBalanceMc != 3_000 {
		t.Fatalf("gate confiscated balance: got %d, want 3000 untouched", br.bal.PaidBalanceMc)
	}
	if len(tr.txs) != 0 {
		t.Fatalf("gate wrote %d transaction(s); a blocked gate must charge nothing", len(tr.txs))
	}
}

func TestPreflightGate_AllowsAtMinimum(t *testing.T) {
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      7_500, // exactly the minimum
		FreeMonthlyResetAt: wp,
	}
	uc, _, _, _ := newTestUC(bal, grokBuildPrice())
	if err := uc.PreflightGate(context.Background(), uuid.New(), "grok-build-0.1"); err != nil {
		t.Fatalf("balance at the minimum must pass, got %v", err)
	}
}

func TestPreflightGate_PreservesPaidDustWhenFreePoolCovers(t *testing.T) {
	// Paid dust below the minimum, but the free pool clears it: the gate passes
	// via free and the paid dust is preserved (the F1 anti-confiscation fix).
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:  3_000, // dust, below minimum
		FreeMonthlyCap: 100_000, FreeMonthlyResetAt: wp,
	}
	uc, br, tr, _ := newTestUC(bal, grokBuildPrice())
	if err := uc.PreflightGate(context.Background(), uuid.New(), "grok-build-0.1"); err != nil {
		t.Fatalf("free pool should cover the minimum, got %v", err)
	}
	if br.bal.PaidBalanceMc != 3_000 {
		t.Fatalf("paid dust was not preserved: got %d, want 3000", br.bal.PaidBalanceMc)
	}
	if len(tr.txs) != 0 {
		t.Fatalf("a passing gate must not write transactions, got %d", len(tr.txs))
	}
}

func TestPreflightGate_RefusesUnknownModel(t *testing.T) {
	_, wp := freshWindows()
	bal := &model.UserBalance{Tier: model.TierPro, IsPaid: true, PaidBalanceMc: 1_000_000, FreeMonthlyResetAt: wp}
	uc, _, _, _ := newTestUC(bal, nil) // nil price → GetModelPrice errors
	if err := uc.PreflightGate(context.Background(), uuid.New(), "made-up"); err == nil {
		t.Fatal("unknown model must be refused, not allowed to run free")
	}
}

// ── Settlement: actual cost, floor at 0, overage → monthly usage ──────────────

func TestChargeUser_NormalCase_DeductsExactAndBooksUsage(t *testing.T) {
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      100_000,
		FreeMonthlyResetAt: wp,
	}
	uc, br, tr, fr := newTestUC(bal, grokBuildPrice())

	// in 68, cached 300, out 13 → api 154mc → ×2.5 = 385mc charged.
	res, err := uc.ChargeUser(context.Background(), ChargeUserCommand{
		UserID: uuid.New(), ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.ApiCostMc != 154 || res.UserChargedMc != 385 {
		t.Fatalf("cost math: api=%d charged=%d, want api=154 charged=385", res.ApiCostMc, res.UserChargedMc)
	}
	if res.Pool != model.PoolPaid {
		t.Fatalf("pool=%s, want paid", res.Pool)
	}
	if br.bal.PaidBalanceMc != 100_000-385 {
		t.Fatalf("balance=%d, want %d", br.bal.PaidBalanceMc, 100_000-385)
	}

	if len(tr.txs) != 1 || len(fr.profits) != 1 {
		t.Fatalf("expected 1 tx + 1 profit, got %d/%d", len(tr.txs), len(fr.profits))
	}
	if fr.profits[0].ProfitMc != 385-154 {
		t.Fatalf("profit=%d, want %d", fr.profits[0].ProfitMc, 385-154)
	}
}

func TestChargeUser_TipOver_FloorsAtZeroAndBooksFullOverageToUsage(t *testing.T) {
	// The step outran the balance. The paid balance must floor at 0 (never
	// negative) and the FULL charge must land in monthly usage so the
	// overage draws down the allowance — nothing silently absorbed.
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      200, // less than the 385 charge
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	res, err := uc.ChargeUser(context.Background(), ChargeUserCommand{
		UserID: uuid.New(), ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
	})
	if err != nil {
		t.Fatalf("settle must never fail on affordability, got %v", err)
	}
	if br.bal.PaidBalanceMc != 0 {
		t.Fatalf("balance went to %d; must floor at 0, never negative", br.bal.PaidBalanceMc)
	}

	if res.NewPaidBalance != 0 {
		t.Fatalf("result balance=%d, want 0", res.NewPaidBalance)
	}
}

func TestChargeUser_OneStepOvershootBound_NextGateBlocks(t *testing.T) {
	// After a tip-over drains the balance to 0, the next pre-flight must block —
	// this is what bounds overspend to a single step.
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      200,
		FreeMonthlyResetAt: wp,
	}
	uc, _, _, _ := newTestUC(bal, grokBuildPrice())
	uid := uuid.New()
	if _, err := uc.ChargeUser(context.Background(), ChargeUserCommand{UserID: uid, ModelName: "grok-build-0.1", InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13}); err != nil {
		t.Fatal(err)
	}
	if err := uc.PreflightGate(context.Background(), uid, "grok-build-0.1"); err == nil {
		t.Fatal("after the balance hit 0, the next call must be blocked")
	}
}

// ── Pure settlement helpers ───────────────────────────────────────────────────

func TestApplySettlement_PaidNeverGoesNegative(t *testing.T) {
	bal := &model.UserBalance{PaidBalanceMc: 200}
	applySettlement(bal, model.PoolPaid, 500)
	if bal.PaidBalanceMc != 0 {
		t.Fatalf("paid balance=%d, want 0 (never negative)", bal.PaidBalanceMc)
	}

}

func TestApplySettlement_FreePoolBooksUsageOnly(t *testing.T) {
	bal := &model.UserBalance{PaidBalanceMc: 1_000}
	applySettlement(bal, model.PoolFree, 300)
	if bal.PaidBalanceMc != 1_000 {
		t.Fatalf("free-pool settle must not touch paid balance, got %d", bal.PaidBalanceMc)
	}
	if bal.FreeMonthlyUsed != 300 {
		t.Fatalf("free monthly usage=%d, want 300", bal.FreeMonthlyUsed)
	}
}

func TestSelectSettlePool(t *testing.T) {
	const min = 5_000
	// Paid clears the minimum → paid.
	paid := &model.UserBalance{IsPaid: true, PaidBalanceMc: 10_000}
	if p := selectSettlePool(paid, min); p != model.PoolPaid {
		t.Fatalf("paid-covers: got %s want paid", p)
	}
	// Paid below minimum, free clears → free (dust preserved).
	dust := &model.UserBalance{IsPaid: true, PaidBalanceMc: 3_000, FreeMonthlyCap: 10_000}
	if p := selectSettlePool(dust, min); p != model.PoolFree {
		t.Fatalf("free-covers: got %s want free", p)
	}
	// Neither clears, paid tier → paid (overage booked to paid).
	broke := &model.UserBalance{IsPaid: true, PaidBalanceMc: 100}
	if p := selectSettlePool(broke, min); p != model.PoolPaid {
		t.Fatalf("neither-covers paid tier: got %s want paid", p)
	}
	// Neither clears, free tier → free.
	freeUser := &model.UserBalance{IsPaid: false}
	if p := selectSettlePool(freeUser, min); p != model.PoolFree {
		t.Fatalf("neither-covers free tier: got %s want free", p)
	}
	// A downgraded/free account can contain stale paid balance from an older
	// schema or interrupted webhook. It must remain in the visible free pool.
	stalePaid := &model.UserBalance{
		IsPaid: false, PaidBalanceMc: 100_000, FreeMonthlyCap: 100_000,
	}
	if p := selectSettlePool(stalePaid, min); p != model.PoolFree {
		t.Fatalf("stale paid balance on free tier: got %s want free", p)
	}
}

func TestSelectPoolIgnoresIneligiblePaidBalance(t *testing.T) {
	const min = 5_000
	bal := &model.UserBalance{
		Tier: model.TierFree, IsPaid: false, PaidBalanceMc: 100_000,
		FreeMonthlyCap: 100_000,
	}
	pool, ok := selectPool(bal, min)
	if !ok || pool != model.PoolFree {
		t.Fatalf("selectPool=(%q,%v), want (%q,true)", pool, ok, model.PoolFree)
	}
}

func TestChargeUser_FlatFeeJoinsApiCostBeforeMarkup(t *testing.T) {
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      100_000,
		FreeMonthlyResetAt: wp,
	}
	uc, br, tr, _ := newTestUC(bal, grokBuildPrice())

	// Same tokens as the normal case (api 154mc) + a 10,000mc per-search web
	// fee → api 10,154mc → ×2.5 markup = 25,385mc charged.
	res, err := uc.ChargeUser(context.Background(), ChargeUserCommand{
		UserID: uuid.New(), ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
		FlatFeeMc: 10_000,
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.ApiCostMc != 10_154 || res.UserChargedMc != 25_385 {
		t.Fatalf("cost math: api=%d charged=%d, want api=10154 charged=25385", res.ApiCostMc, res.UserChargedMc)
	}
	if br.bal.PaidBalanceMc != 100_000-25_385 {
		t.Fatalf("balance=%d, want %d", br.bal.PaidBalanceMc, 100_000-25_385)
	}
	if len(tr.txs) != 1 || !strings.Contains(tr.txs[0].Description, "+flat:10000mc") {
		t.Fatalf("expected tx description to record the flat fee, got %+v", tr.txs)
	}
}

// A flat-fee-only settlement (e.g. a search whose token counts were zero or a
// provider that reports none) must still charge.
func TestChargeUser_FlatFeeOnly(t *testing.T) {
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      100_000,
		FreeMonthlyResetAt: wp,
	}
	uc, br, _, _ := newTestUC(bal, grokBuildPrice())

	res, err := uc.ChargeUser(context.Background(), ChargeUserCommand{
		UserID: uuid.New(), ModelName: "grok-build-0.1",
		FlatFeeMc: 10_000,
	})
	if err != nil {
		t.Fatal(err)
	}
	if res.ApiCostMc != 10_000 || res.UserChargedMc != 25_000 {
		t.Fatalf("cost math: api=%d charged=%d, want api=10000 charged=25000", res.ApiCostMc, res.UserChargedMc)
	}
	if br.bal.PaidBalanceMc != 100_000-25_000 {
		t.Fatalf("balance=%d, want %d", br.bal.PaidBalanceMc, 100_000-25_000)
	}
}

func TestWebSearchFeeMc_DefaultsPerFamily(t *testing.T) {
	uc, _, _, _ := newTestUC(&model.UserBalance{}, grokBuildPrice())
	cases := map[string]int64{
		"anthropic": 10_000,
		"openai":    10_000,
		"gemini":    35_000,
		"xai":       25_000,
		"":          0,
		"groq":      0,
	}
	for family, want := range cases {
		if got := uc.WebSearchFeeMc(context.Background(), family); got != want {
			t.Errorf("WebSearchFeeMc(%q) = %d, want %d", family, got, want)
		}
	}
}

// ── Markup comes only from the database ───────────────────────────────────────

// A config repo that returns whatever the test wants for the markup keys.
type markupConfig struct {
	num, den string
	missing  bool
}

func (c markupConfig) Get(_ context.Context, key string) (string, error) {
	if c.missing {
		return "", sql.ErrNoRows
	}
	switch key {
	case "markup_numerator":
		return c.num, nil
	case "markup_denominator":
		return c.den, nil
	}
	return "", sql.ErrNoRows
}
func (markupConfig) Set(_ context.Context, _, _ string) error { return nil }

func ucWithMarkup(cfg markupConfig) (*BillingUseCase, *fakeBalRepo) {
	_, wp := freshWindows()
	bal := &model.UserBalance{
		Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc:      10_000_000,
		FreeMonthlyResetAt: wp,
	}
	br := &fakeBalRepo{bal: bal}
	uc := NewBillingUseCase(br, &fakePriceRepo{price: grokBuildPrice()}, &fakeTxRepo{},
		&fakeProfitRepo{}, cfg, fakeTxMgr{})
	return uc, br
}

// The configured rate is the rate charged — no compiled default can override it.
func TestMarkup_ComesFromDatabase(t *testing.T) {
	ctx := context.Background()
	// 1.9x instead of the historical 2.5x.
	uc, _ := ucWithMarkup(markupConfig{num: "19", den: "10"})
	res, err := uc.ChargeUser(ctx, ChargeUserCommand{
		UserID: uuid.New(), ModelName: "grok-build-0.1",
		InputTokens: 68, CachedInputTokens: 300, OutputTokens: 13,
	})
	if err != nil {
		t.Fatal(err)
	}
	want := res.ApiCostMc * 19 / 10
	if res.UserChargedMc != want {
		t.Fatalf("charged=%d, want %d (api %d at 1.9x)", res.UserChargedMc, want, res.ApiCostMc)
	}
}

// A missing or broken markup must BLOCK at the gate. Settle() swallows charge
// errors, so if this were only caught at settlement time every request would
// run unbilled — a misconfiguration must refuse work, never give it away.
func TestMarkup_MisconfigurationFailsClosed(t *testing.T) {
	ctx := context.Background()
	uid := uuid.New()

	for name, cfg := range map[string]markupConfig{
		"missing rows":     {missing: true},
		"not an integer":   {num: "2.5", den: "1"},
		"zero denominator": {num: "5", den: "0"},
		"negative":         {num: "-5", den: "2"},
	} {
		uc, br := ucWithMarkup(cfg)
		before := br.bal.PaidBalanceMc
		if err := uc.PreflightGate(ctx, uid, "grok-build-0.1"); err == nil {
			t.Fatalf("%s: gate must refuse a misconfigured markup", name)
		}
		if _, err := uc.ChargeUser(ctx, ChargeUserCommand{
			UserID: uid, ModelName: "grok-build-0.1", InputTokens: 68, OutputTokens: 13,
		}); err == nil {
			t.Fatalf("%s: charge must fail rather than bill a guessed rate", name)
		}
		if br.bal.PaidBalanceMc != before {
			t.Fatalf("%s: balance moved on a failed charge", name)
		}
	}
}
