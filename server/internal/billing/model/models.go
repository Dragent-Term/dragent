package model

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
)

var (
	ErrInsufficientCredits = errors.New("insufficient wallet balance credits")
	ErrNegativeAmount      = errors.New("amount must be positive")
	ErrInactiveModel       = errors.New("the selected model is currently inactive")
	// ErrUnknownModel: the requested model has no price row. Server-key calls
	// are REFUSED for unknown models (billing correctness over availability);
	// BYOK custom models never consult the catalog, so they are unaffected.
	ErrUnknownModel         = errors.New("model is not in the price catalog")
	ErrFreeCreditsExhausted = errors.New("monthly free credits exhausted")
	ErrPaidCreditsExhausted = errors.New("paid monthly credits exhausted")
	// ErrCreditsNotEnabled: the user has purchased credits but has not opted
	// into spending them. Distinct from exhaustion — nothing is used up, the
	// user just has to say yes.
	ErrCreditsNotEnabled = errors.New("credit usage not enabled")
)

const (
	// Markup is NOT defined here. The api_cost → user_charged multiplier lives
	// solely in system_configs (markup_numerator / markup_denominator, seeded
	// by migration 00015) so the charged rate is always the rate an operator
	// can read and audit in the database. A compiled default would silently
	// win whenever the config was missing, which is exactly the case where you
	// most want to know something is wrong.

	TierFree  = "free"
	TierPro   = "pro"
	TierUltra = "ultra"
	TierMega  = "mega"

	PoolPaid    = "paid"
	PoolFree    = "free"
	PoolDeposit = "deposit"
	PoolRefund  = "refund"

	CallTypeChat  = "chat"
	CallTypeAgent = "agent"
)

// UserBalance mirrors user_balances after migrations 00011-00013.
type UserBalance struct {
	UserID uuid.UUID
	// OrganizationID is the org this balance row belongs to — every row is
	// scoped (user_id, organization_id), never user_id alone.
	OrganizationID uuid.UUID
	Tier           string // 'free' | 'pro' | 'ultra' | 'mega'
	IsPaid         bool   // true when tier != 'free'
	// PaidStatus is the provider's subscription status from the last webhook.
	// IsPaid stays true during dunning (access continues), so this is the only
	// signal distinguishing "paying" from "payment failed".
	PaidStatus string

	// Paid pool — the whole monthly allowance, usable at any pace. No rolling
	// sub-window: charges decrement this monthly balance directly, and
	// it renews (no rollover) on PaidMonthlyResetAt.
	PaidBalanceMc      int64
	PaidMonthlyCap     int64
	PaidMonthlyResetAt *time.Time

	// Deposit pool — durable purchased credits (top-ups). Never reset by the
	// monthly renewal. Spent only as a LAST resort, after both the monthly
	// allowance and the free baseline, and only when the user has opted in.
	DepositBalanceMc int64

	// AllowCreditUsage gates the deposit pool. False means "never spend my
	// purchased credits" — the user is blocked with the real reason instead.
	// Defaults to false so credits are never spent without an explicit choice.
	AllowCreditUsage bool

	// Free pool — a MONTHLY allowance for ALL users, usable at any pace; the
	// fallback once the paid balance is spent. It has no shorter sub-window:
	// it mirrors the paid pool's shape (one allowance, any pace) and renews on
	// FreeMonthlyResetAt (a used-counter against FreeMonthlyCap).
	FreeMonthlyCap  int64
	FreeMonthlyUsed int64

	// When the free monthly allowance next resets (used-counter → 0). Mirrors
	// PaidMonthlyResetAt.
	FreeMonthlyResetAt *time.Time

	UpdatedAt time.Time
}

// HasAvailableCredits returns true when at least one pool can cover min
// micro-cents. Pure disjunction of the same predicates selectPool branches on,
// so the boolean is order-independent and cannot disagree with the charge.
func (ub *UserBalance) HasAvailableCredits(min int64) bool {
	// A numeric paid balance is not sufficient by itself. Old rows can retain a
	// stale allowance after a subscription is cancelled or downgraded; allowing
	// that value to select the paid pool makes free-tier spend invisible because
	// the account UI intentionally hides the paid meter for non-paying users.
	if ub.IsPaid && ub.PaidBalanceMc >= min {
		return true
	}
	// Purchased credits, only with consent.
	if ub.AllowCreditUsage && ub.DepositBalanceMc >= min {
		return true
	}
	if ub.FreeMonthlyCap-ub.FreeMonthlyUsed >= min {
		return true
	}
	return false
}

// CanUsePremiumModel reports whether the user has funds in a PREMIUM-capable
// pool — the paid monthly allowance, or consented purchased credits — to run a
// non-free model. The free allowance does NOT count: it only ever backs the
// free-tier model. When this is false the user is restricted to the free model
// (and the client locks the model selector). Turning credit usage off, or
// running credits dry, drops a paid-exhausted user into this state.
func (ub *UserBalance) CanUsePremiumModel(min int64) bool {
	if ub.IsPaid && ub.PaidBalanceMc >= min {
		return true
	}
	if ub.AllowCreditUsage && ub.DepositBalanceMc >= min {
		return true
	}
	return false
}

// NextFreeResetTime returns when the free monthly allowance next refreshes.
func (ub *UserBalance) NextFreeResetTime() time.Time {
	if ub.FreeMonthlyResetAt == nil {
		return AddOneMonth(time.Now().UTC())
	}
	return ub.FreeMonthlyResetAt.UTC()
}

// ExhaustionError names why a charge of `min` micro-cents was refused, with a
// reset time where one applies. It reports whichever blocker the user can act on
// most easily — credits sitting behind the opt-in first (one toggle away), then
// the tier-appropriate allowance reset. Spend order is paid → credits → free.
func (ub *UserBalance) ExhaustionError(min int64) error {
	const layout = "January 2, 2006 15:04 UTC"

	// Purchased credits sitting behind the opt-in are the most actionable
	// blocker — one toggle away — so report that first.
	if !ub.AllowCreditUsage && ub.DepositBalanceMc >= min {
		return fmt.Errorf("%w: you have purchased credits available. Enable "+
			"\"Use my credits\" in Settings to continue past this limit",
			ErrCreditsNotEnabled)
	}

	if ub.Tier == TierFree {
		return fmt.Errorf("%w: your free monthly allowance resets on %s",
			ErrFreeCreditsExhausted, ub.NextFreeResetTime().Format(layout))
	}
	// Paid user, monthly allowance spent (and no consented credits).
	reset := "your next billing date"
	if ub.PaidMonthlyResetAt != nil {
		reset = ub.PaidMonthlyResetAt.UTC().Format(layout)
	}
	return fmt.Errorf("%w: your monthly allowance is used up. It renews on %s — "+
		"or enable \"Use my credits\" in Settings to continue now",
		ErrPaidCreditsExhausted, reset)
}

// ModelPrice mirrors model_prices after migration 00010.
type ModelPrice struct {
	ModelName                            string
	InputRatePerMillion                  int64
	CacheWriteRatePerMillion             int64 // 0 = no cache-write surcharge
	CachedInputRatePerMillion            int64 // 0 = no caching discount
	OutputRatePerMillion                 int64
	LongContextThreshold                 int64
	LongContextInputRatePerMillion       int64
	LongContextCachedInputRatePerMillion int64
	LongContextOutputRatePerMillion      int64
	ContextWindowTokens                  int64
	// MinBalanceMc is the minimum wallet balance (micro-cents) a user must hold
	// to start a call with this model. It is the ONLY pre-call affordability
	// gate — we do not estimate a call's cost. 0 falls back to the global
	// min_required_credits_mc floor.
	MinBalanceMc int64
	IsActive     bool
	IsUserFacing bool
}

// CalculateCost computes the provider's raw API cost in micro-cents (no markup).
func (mp *ModelPrice) CalculateCost(inputTokens, cachedInputTokens, outputTokens int64) (int64, error) {
	if !mp.IsActive {
		return 0, ErrInactiveModel
	}

	totalInput := inputTokens + cachedInputTokens
	useLongContext := mp.LongContextThreshold > 0 && totalInput > mp.LongContextThreshold

	var inputRate, cachedRate, outputRate int64
	if useLongContext {
		inputRate = mp.LongContextInputRatePerMillion
		cachedRate = mp.LongContextCachedInputRatePerMillion
		outputRate = mp.LongContextOutputRatePerMillion
	} else {
		inputRate = mp.InputRatePerMillion
		cachedRate = mp.CachedInputRatePerMillion
		outputRate = mp.OutputRatePerMillion
	}

	if cachedRate == 0 {
		cachedRate = inputRate
	}

	cost := (inputTokens*inputRate + cachedInputTokens*cachedRate + outputTokens*outputRate) / 1_000_000
	return cost, nil
}

// CreditTransaction mirrors credit_transactions after migration 00011.
type CreditTransaction struct {
	ID     uuid.UUID
	UserID uuid.UUID
	// AmountMc is micro-cents ($1 = 1,000,000 mc), negative for consumption.
	// A displayed "credit" is 1 cent = 10,000 mc — do not confuse the two.
	AmountMc     int64
	Description  string
	ModelName    string
	InputTokens  int64
	OutputTokens int64
	CallType     string // 'chat' | 'agent' | 'deposit' | 'refund'
	Pool         string // 'paid' | 'free' | 'deposit' | 'refund'
	CreatedAt    time.Time
}

// PlatformProfit mirrors platform_profits after migration 00011.
type PlatformProfit struct {
	ID            uuid.UUID
	UserID        uuid.UUID
	TransactionID *uuid.UUID
	ModelName     string
	CallType      string
	Pool          string
	InputTokens   int64
	OutputTokens  int64
	ApiCostMc     int64
	UserChargedMc int64
	ProfitMc      int64
	Markup        float64
	CreatedAt     time.Time
}

const (
	OperationAuthorized       = "authorized"
	OperationUsageRecorded    = "usage_recorded"
	OperationSettled          = "settled"
	OperationVoided           = "voided"
	OperationSettlementFailed = "settlement_failed"
)

// BillingOperation is the durable authorization for one platform-funded
// provider call. MinimumHoldMc reserves capacity only; the existing pricing
// and ChargeUser settlement math remains the source of the actual charge.
type BillingOperation struct {
	ID                  uuid.UUID
	OperationKey        string
	TaskID              *uuid.UUID
	UserID              uuid.UUID
	OperationType       string
	ModelName           string
	RoundNumber         int
	Pool                string
	MinimumHoldMc       int64
	Status              string
	InputTokens         int64
	CachedInputTokens   int64
	OutputTokens        int64
	FlatFeeMc           int64
	CallType            string
	ApiCostMc           int64
	UserChargedMc       int64
	CreditTransactionID *uuid.UUID
	LastError           string
	CreatedAt           time.Time
	UpdatedAt           time.Time
	ExpiresAt           time.Time
}

// UserFacingModel is a model with is_user_facing=TRUE, returned by the models endpoint.
type UserFacingModel struct {
	ModelName           string
	ContextWindowTokens int64
	IsActive            bool
}

type SystemConfig struct {
	Key         string
	Value       string
	Description string
	UpdatedAt   time.Time
}

type BalanceRepository interface {
	GetByUserID(ctx context.Context, userID, organizationID uuid.UUID) (*UserBalance, error)
	// GetByUserIDTx locks the row (FOR UPDATE) for the read-modify-write
	// charge/authorize path — balance.OrganizationID on the result is what
	// UpdateTx below writes back to, so a caller never has to separately
	// track which org it read.
	GetByUserIDTx(ctx context.Context, tx any, userID, organizationID uuid.UUID) (*UserBalance, error)
	UpdateTx(ctx context.Context, tx any, balance *UserBalance) error
	InitializeTx(ctx context.Context, tx any, userID, organizationID uuid.UUID) error
	// SetAllowCreditUsage flips the opt-in only. Deliberately NOT a full
	// UpdateTx: writing the whole balance from a preferences request would
	// race a concurrent charge and clobber its usage counters.
	SetAllowCreditUsage(ctx context.Context, userID, organizationID uuid.UUID, allow bool) error
}

type PriceRepository interface {
	GetModelPrice(ctx context.Context, modelName string) (*ModelPrice, error)
	ListUserFacingModels(ctx context.Context) ([]*UserFacingModel, error)
	UpdateModelPrice(ctx context.Context, modelPrice *ModelPrice) error
	SetActive(ctx context.Context, modelName string, active bool) error
}

type TransactionRepository interface {
	CreateTx(ctx context.Context, tx any, transaction *CreditTransaction) (uuid.UUID, error)
	ListByUserID(ctx context.Context, userID uuid.UUID, limit int) ([]*CreditTransaction, error)
}

type ProfitRepository interface {
	CreateProfitTx(ctx context.Context, tx any, profit *PlatformProfit) error
}

// BillingOperationRepository makes provider authorization and settlement
// idempotent across retries and process restarts.
type BillingOperationRepository interface {
	GetOperationByKeyTx(ctx context.Context, tx any, operationKey string) (*BillingOperation, error)
	GetOperationByIDTx(ctx context.Context, tx any, id uuid.UUID) (*BillingOperation, error)
	CreateOperationTx(ctx context.Context, tx any, operation *BillingOperation) error
	ActiveHoldsByPoolTx(ctx context.Context, tx any, userID uuid.UUID) (map[string]int64, error)
	RecordOperationUsage(ctx context.Context, id uuid.UUID, inputTokens, cachedInputTokens, outputTokens, flatFeeMc int64, callType string) error
	MarkOperationSettledTx(ctx context.Context, tx any, id, transactionID uuid.UUID, apiCostMc, userChargedMc int64, pool string) error
	MarkOperationSettlementFailed(ctx context.Context, id uuid.UUID, message string) error
	VoidOperation(ctx context.Context, id uuid.UUID, reason string) error
	ListUnsettledOperations(ctx context.Context, limit int) ([]*BillingOperation, error)
}

type SystemConfigRepository interface {
	Get(ctx context.Context, key string) (string, error)
	Set(ctx context.Context, key string, value string) error
}

type TxManager interface {
	WithTx(ctx context.Context, fn func(tx any) error) error
}

// SystemConfigRow is one typed configuration entry (system_configs row).
type SystemConfigRow struct {
	Key         string `json:"key"`
	Value       string `json:"value"`
	ValueType   string `json:"value_type"`
	Description string `json:"description"`
}

// ValidateMarkup enforces the two invariants a markup must satisfy.
//
// Positive, obviously. But also STRICTLY GREATER THAN 1: the multiplier turns
// provider cost into what the user is charged, so a ratio of 1 sells at cost
// and anything below it sells at a loss on every single call — silently, and
// faster the more the product is used. There is no legitimate configuration
// below 1, so it is rejected rather than trusted.
//
// Lives in the model package because it is a domain invariant, and so both
// the charge path and the admin config endpoint can enforce it — a bad value is
// then rejected at write time instead of at the next charge.
func ValidateMarkup(num, den int64) error {
	if num <= 0 || den <= 0 {
		return fmt.Errorf(
			"markup must be positive, got %d/%d — fix system_configs", num, den)
	}
	if num <= den {
		return fmt.Errorf(
			"markup must be greater than 1, got %d/%d (=%.3gx) — at or below 1 "+
				"every request is sold at or below provider cost", num, den,
			float64(num)/float64(den))
	}
	return nil
}

// EligibleForClockRenewal reports whether a calendar-driven monthly renewal may
// grant this balance a fresh allowance.
//
// The clock exists so a lost or delayed renewal webhook cannot strand someone
// who genuinely paid. It must NOT hand out an allowance to someone whose
// payment failed: 'past_due' keeps access during dunning but is not payment.
func (ub *UserBalance) EligibleForClockRenewal() bool {
	if !ub.IsPaid {
		return false
	}
	switch ub.PaidStatus {
	case "active", "on_trial":
		return true
	case "":
		// Pre-migration rows, and any row whose status we have never seen.
		// Allowing these preserves the resilience the clock is for; the
		// backfill in migration 00034 makes this rare and transitional.
		return true
	default:
		return false
	}
}

// AddOneMonth advances by one CALENDAR month, clamping to the last day when the
// target month is shorter. Jan 31 → Feb 28 (or Feb 29 in a leap year) → Mar 31.
//
// Subscriptions bill on the same calendar day each month, so a flat 30 days is
// the wrong unit: it drifts against the provider's schedule and, in any 31-day
// month, comes due a day early — which for a clock-driven renewal means
// granting an allowance before the payment it corresponds to.
//
// Go's t.AddDate(0, 1, 0) is NOT usable here: it normalises overflow, turning
// Jan 31 into Mar 2/3 rather than clamping to the end of February, which would
// skip a billing month entirely.
func AddOneMonth(t time.Time) time.Time {
	y, m, d := t.Date()
	firstOfNext := time.Date(y, m, 1, t.Hour(), t.Minute(), t.Second(), t.Nanosecond(), t.Location()).
		AddDate(0, 1, 0)
	if last := daysInMonth(firstOfNext.Year(), firstOfNext.Month()); d > last {
		d = last
	}
	return time.Date(firstOfNext.Year(), firstOfNext.Month(), d,
		t.Hour(), t.Minute(), t.Second(), t.Nanosecond(), t.Location())
}

// daysInMonth: day 0 of the following month is the last day of this one.
func daysInMonth(y int, m time.Month) int {
	return time.Date(y, m+1, 0, 0, 0, 0, 0, time.UTC).Day()
}
