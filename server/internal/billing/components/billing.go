package components

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
	"server/internal/billing/model"
)

var (
	// Canonical credit/session sentinels live in the model package so the
	// balance methods and these use-cases return the SAME error values.
	ErrFreeCreditsExhausted = model.ErrFreeCreditsExhausted
	ErrPaidCreditsExhausted = model.ErrPaidCreditsExhausted
	// ErrCreditsNotEnabled: exhausted, but the user has purchased credits they
	// have not opted into. A USER-actionable state (turn on the toggle), so it
	// MUST map to a 402 with its message — not fall through to a generic 500.
	ErrCreditsNotEnabled = model.ErrCreditsNotEnabled
)

type ChargeUserCommand struct {
	UserID            uuid.UUID
	ModelName         string
	InputTokens       int64
	CachedInputTokens int64
	OutputTokens      int64
	CallType          string // 'chat' | 'agent'; defaults to 'agent'
	// FlatFeeMc is a provider fee in micro-dollars charged on top of token
	// cost (e.g. Anthropic's $10/1k web searches = 10,000 mc per search).
	// It joins the API cost BEFORE markup, like any other provider cost.
	FlatFeeMc int64
}

type ChargeUserResult struct {
	ApiCostMc      int64
	UserChargedMc  int64
	Pool           string
	NewPaidBalance int64
	TransactionID  uuid.UUID
}

type AuthorizeOperationCommand struct {
	OperationKey  string
	TaskID        *uuid.UUID
	UserID        uuid.UUID
	OperationType string
	ModelName     string
	RoundNumber   int
	CallType      string
}

// OrgResolver resolves a user's currently active organization — needed only
// for GetOrInitBalance's lazy-init path (a user whose balance row doesn't
// exist yet needs SOME organization_id to create it against, now that the
// row is keyed (user_id, organization_id)). Declared here as a
// one-method interface, not imported from the organization package, so
// billing does not depend on organization's domain shape (OrganizationWithRole
// — role/name/slug, none of which billing has any legitimate use for) — the
// concrete organization.PostgresRepository already satisfies it via its own
// ActiveOrganizationID accessor. Optional: nil falls back to uuid.Nil, which
// the NOT NULL constraint will reject once enforced — acceptable because
// every real request path resolves org context explicitly by the time that
// constraint lands (Phase 11.F.2+).
type OrgResolver interface {
	ActiveOrganizationID(ctx context.Context, userID uuid.UUID) (uuid.UUID, error)
}

type BillingUseCase struct {
	balanceRepo   model.BalanceRepository
	priceRepo     model.PriceRepository
	txRepo        model.TransactionRepository
	profitRepo    model.ProfitRepository
	configRepo    model.SystemConfigRepository
	txManager     model.TxManager
	operationRepo model.BillingOperationRepository
	orgResolver   OrgResolver
}

// SetOrgResolver wires active-org resolution in. A setter, not a constructor
// arg, so existing call sites and tests keep compiling — mirrors identity's
// SetBalanceInitializer/SetOrganizationInitializer pattern.
func (uc *BillingUseCase) SetOrgResolver(r OrgResolver) {
	uc.orgResolver = r
}

// ModelContextWindow returns the active catalog value used by the account API
// and model selector. Agent context management must not maintain a separate
// hard-coded copy of this value.
func (uc *BillingUseCase) ModelContextWindow(ctx context.Context, modelName string) (int64, error) {
	price, err := uc.priceRepo.GetModelPrice(ctx, modelName)
	if err != nil {
		return 0, err
	}
	return price.ContextWindowTokens, nil
}

func NewBillingUseCase(
	balanceRepo model.BalanceRepository,
	priceRepo model.PriceRepository,
	txRepo model.TransactionRepository,
	profitRepo model.ProfitRepository,
	configRepo model.SystemConfigRepository,
	txManager model.TxManager,
) *BillingUseCase {
	uc := &BillingUseCase{
		balanceRepo: balanceRepo,
		priceRepo:   priceRepo,
		txRepo:      txRepo,
		profitRepo:  profitRepo,
		configRepo:  configRepo,
		txManager:   txManager,
	}
	// Production uses one repository adapter for all billing tables. Keeping
	// this as a capability assertion preserves lightweight test adapters and
	// makes older callers fail closed only when they use durable operations.
	if repo, ok := balanceRepo.(model.BillingOperationRepository); ok {
		uc.operationRepo = repo
	}
	return uc
}

// AuthorizeOperation atomically reserves one model's existing minimum-balance
// requirement. It does not debit credits or alter pricing; active holds are
// subtracted only while selecting capacity for concurrent provider calls.
func (uc *BillingUseCase) AuthorizeOperation(ctx context.Context, cmd AuthorizeOperationCommand) (*model.BillingOperation, error) {
	if uc.operationRepo == nil {
		return nil, errors.New("durable billing operations are not configured")
	}
	if strings.TrimSpace(cmd.OperationKey) == "" {
		return nil, errors.New("billing operation key is required")
	}
	minBal, err := uc.minBalanceFor(ctx, cmd.ModelName)
	if err != nil {
		return nil, err
	}
	if _, _, err := uc.markup(ctx); err != nil {
		return nil, err
	}
	orgID, err := uc.ActiveOrganizationID(ctx, cmd.UserID)
	if err != nil {
		return nil, fmt.Errorf("resolve active organization: %w", err)
	}

	var authorized *model.BillingOperation
	err = uc.txManager.WithTx(ctx, func(tx any) error {
		existing, getErr := uc.operationRepo.GetOperationByKeyTx(ctx, tx, cmd.OperationKey)
		if getErr == nil {
			if existing.UserID != cmd.UserID || existing.ModelName != cmd.ModelName {
				return errors.New("billing operation key already belongs to another request")
			}
			authorized = existing
			return nil
		}
		if !errors.Is(getErr, sql.ErrNoRows) {
			return getErr
		}

		bal, err := uc.getOrInitBalanceTx(ctx, tx, cmd.UserID, orgID)
		if err != nil {
			return err
		}
		now := time.Now().UTC()
		advancedFree := advanceFreeMonthly(bal, now)
		advancedPaid := advanceMonthlyRenewal(bal, now)
		holds, err := uc.operationRepo.ActiveHoldsByPoolTx(ctx, tx, cmd.UserID)
		if err != nil {
			return fmt.Errorf("fetch active billing holds: %w", err)
		}

		available := *bal
		available.PaidBalanceMc = max64(0, available.PaidBalanceMc-holds[model.PoolPaid])
		available.DepositBalanceMc = max64(0, available.DepositBalanceMc-holds[model.PoolDeposit])
		available.FreeMonthlyUsed += holds[model.PoolFree]
		pool, ok := selectPool(&available, minBal)
		if !ok {
			if advancedFree || advancedPaid {
				bal.UpdatedAt = now
				if err := uc.balanceRepo.UpdateTx(ctx, tx, bal); err != nil {
					return err
				}
			}
			return bal.ExhaustionError(minBal)
		}
		callType := cmd.CallType
		if callType == "" {
			callType = model.CallTypeAgent
		}
		op := &model.BillingOperation{
			ID: uuid.New(), OperationKey: cmd.OperationKey, TaskID: cmd.TaskID,
			UserID: cmd.UserID, OperationType: cmd.OperationType,
			ModelName: cmd.ModelName, RoundNumber: cmd.RoundNumber, Pool: pool,
			MinimumHoldMc: minBal, Status: model.OperationAuthorized,
			CallType: callType, CreatedAt: now, UpdatedAt: now, ExpiresAt: now.Add(15 * time.Minute),
		}
		if op.OperationType == "" {
			op.OperationType = "agent_model"
		}
		if advancedFree || advancedPaid {
			bal.UpdatedAt = now
			if err := uc.balanceRepo.UpdateTx(ctx, tx, bal); err != nil {
				return err
			}
		}
		if err := uc.operationRepo.CreateOperationTx(ctx, tx, op); err != nil {
			return err
		}
		authorized = op
		return nil
	})
	return authorized, err
}

func max64(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}

// GetOrInitBalance fetches (or creates) a user's balance row, scoped to
// their currently active organization — a balance row is keyed (user_id,
// organization_id). Returns the balance after advancing any expired rolling windows.
func (uc *BillingUseCase) GetOrInitBalance(ctx context.Context, userID uuid.UUID) (*model.UserBalance, error) {
	orgID, err := uc.ActiveOrganizationID(ctx, userID)
	if err != nil {
		return nil, fmt.Errorf("resolve active organization: %w", err)
	}

	bal, err := uc.balanceRepo.GetByUserID(ctx, userID, orgID)
	if err != nil {
		if !errors.Is(err, sql.ErrNoRows) {
			return nil, fmt.Errorf("fetch balance: %w", err)
		}
		// First use — create the row; DB defaults give the user their free monthly pool.
		if initErr := uc.balanceRepo.InitializeTx(ctx, nil, userID, orgID); initErr != nil {
			return nil, fmt.Errorf("initialise wallet: %w", initErr)
		}
		return uc.balanceRepo.GetByUserID(ctx, userID, orgID)
	}

	return bal, nil
}

// getOrInitBalanceTx is GetOrInitBalance's transactional twin: same
// fetch-or-lazily-create logic, but against the row-locked (FOR UPDATE)
// read, for callers already inside a WithTx closure — AuthorizeOperation
// and SettleOperation below, which cannot use the plain GetOrInitBalance
// without dropping the row lock their read-modify-write correctness
// depends on. One implementation of "get or create," not two copies that
// could drift.
func (uc *BillingUseCase) getOrInitBalanceTx(ctx context.Context, tx any, userID, orgID uuid.UUID) (*model.UserBalance, error) {
	bal, err := uc.balanceRepo.GetByUserIDTx(ctx, tx, userID, orgID)
	if err != nil {
		if !errors.Is(err, sql.ErrNoRows) {
			return nil, fmt.Errorf("fetch balance in tx: %w", err)
		}
		if initErr := uc.balanceRepo.InitializeTx(ctx, tx, userID, orgID); initErr != nil {
			return nil, fmt.Errorf("initialise wallet: %w", initErr)
		}
		return uc.balanceRepo.GetByUserIDTx(ctx, tx, userID, orgID)
	}
	return bal, nil
}

// SetAllowCreditUsage flips the caller's deposit-credit opt-in, scoped to
// their currently active organization. A thin wrapper over the repository
// method so handler packages depend on this use case alone for every
// balance operation — read (GetOrInitBalance) and write alike — instead of
// some going through BillingUseCase and others reaching past it to the raw
// repository for one specific write.
func (uc *BillingUseCase) SetAllowCreditUsage(ctx context.Context, userID uuid.UUID, allow bool) error {
	orgID, err := uc.ActiveOrganizationID(ctx, userID)
	if err != nil {
		return fmt.Errorf("resolve active organization: %w", err)
	}
	return uc.balanceRepo.SetAllowCreditUsage(ctx, userID, orgID, allow)
}

// ActiveOrganizationID is the single place every caller — internal to this
// use case, or a handler package that just needs the id without a full
// GetOrInitBalance — resolves which org a user's balance operation applies
// to. Exported so read-only screens (usage/voice/byok/account) can depend
// on *BillingUseCase alone instead of each declaring their own duplicate
// org-resolution interface.
func (uc *BillingUseCase) ActiveOrganizationID(ctx context.Context, userID uuid.UUID) (uuid.UUID, error) {
	if uc.orgResolver == nil {
		return uuid.Nil, nil
	}
	return uc.orgResolver.ActiveOrganizationID(ctx, userID)
}

// ChargeUser settles ONE already-run step's ACTUAL cost. It never fails on
// affordability and never confiscates:
//  1. Calculates raw provider cost (api_cost) from the real provider-reported
//     token counts (input/cached/output) and DB-seeded rates, honouring the
//     long-context tier when the prompt crosses the model's threshold. Unknown
//     models are REFUSED (ErrUnknownModel) — never fallback-priced.
//  2. Applies the configured markup (system_configs markup_numerator/
//     markup_denominator; integers, e.g. 19/10 = 1.9×) → user_charged.
//  3. In a single DB transaction:
//     a. Advances expired monthly allowance windows.
//     b. Checks paid monthly renewal (no rollover — balance resets to cap).
//     c. Picks the settling pool (paid first, then free) consistently with the
//     pre-flight gate; on a moved balance, falls back to the tier pool.
//     d. Charges the actual cost: the paid balance floors at 0 (never negative)
//     and the FULL cost is booked into that pool's monthly usage, so a
//     single-step overshoot draws down the monthly allowance.
//     e. Records credit_transaction + platform_profits.
func (uc *BillingUseCase) ChargeUser(ctx context.Context, cmd ChargeUserCommand) (*ChargeUserResult, error) {
	charge, err := uc.prepareCharge(ctx, cmd)
	if err != nil {
		return nil, err
	}
	if charge.userChargedMc == 0 {
		return &ChargeUserResult{}, nil
	}
	var result *ChargeUserResult
	err = uc.txManager.WithTx(ctx, func(tx any) error {
		var settleErr error
		result, settleErr = uc.settlePreparedTx(ctx, tx, cmd, charge)
		return settleErr
	})
	if err != nil {
		return nil, err
	}
	uc.recordProfit(ctx, cmd, charge, result)
	return result, nil
}

type preparedCharge struct {
	apiCostMc, userChargedMc           int64
	minBalanceMc, markupNum, markupDen int64
	callType                           string
}

func (uc *BillingUseCase) prepareCharge(ctx context.Context, cmd ChargeUserCommand) (preparedCharge, error) {
	price, err := uc.priceRepo.GetModelPrice(ctx, cmd.ModelName)
	if err != nil {
		return preparedCharge{}, fmt.Errorf("price lookup: %w", err)
	}
	apiCostMc, err := price.CalculateCost(cmd.InputTokens, cmd.CachedInputTokens, cmd.OutputTokens)
	if err != nil {
		return preparedCharge{}, err
	}
	apiCostMc += cmd.FlatFeeMc
	num, den, err := uc.markup(ctx)
	if err != nil {
		return preparedCharge{}, err
	}
	userChargedMc := apiCostMc * num / den
	minBal := price.MinBalanceMc
	if minBal <= 0 {
		minBal = uc.MinRequiredCredits(ctx)
	}
	callType := cmd.CallType
	if callType == "" {
		callType = model.CallTypeAgent
	}
	return preparedCharge{apiCostMc: apiCostMc, userChargedMc: userChargedMc,
		minBalanceMc: minBal, markupNum: num, markupDen: den, callType: callType}, nil
}

func (uc *BillingUseCase) settlePreparedTx(ctx context.Context, tx any, cmd ChargeUserCommand, charge preparedCharge) (*ChargeUserResult, error) {
	orgID, err := uc.ActiveOrganizationID(ctx, cmd.UserID)
	if err != nil {
		return nil, fmt.Errorf("resolve active organization: %w", err)
	}
	bal, err := uc.getOrInitBalanceTx(ctx, tx, cmd.UserID, orgID)
	if err != nil {
		return nil, err
	}
	now := time.Now().UTC()
	advanceFreeMonthly(bal, now)
	advanceMonthlyRenewal(bal, now)
	pool := selectSettlePool(bal, charge.minBalanceMc)
	applySettlement(bal, pool, charge.userChargedMc)
	bal.UpdatedAt = now
	if err := uc.balanceRepo.UpdateTx(ctx, tx, bal); err != nil {
		return nil, fmt.Errorf("update balance: %w", err)
	}
	description := fmt.Sprintf("%s (in:%d cached:%d out:%d)", cmd.ModelName, cmd.InputTokens, cmd.CachedInputTokens, cmd.OutputTokens)
	if cmd.FlatFeeMc > 0 {
		description += fmt.Sprintf(" +flat:%dmc", cmd.FlatFeeMc)
	}
	txID, err := uc.txRepo.CreateTx(ctx, tx, &model.CreditTransaction{
		ID: uuid.New(), UserID: cmd.UserID, AmountMc: -charge.userChargedMc,
		Description: description, ModelName: cmd.ModelName, InputTokens: cmd.InputTokens,
		OutputTokens: cmd.OutputTokens, CallType: charge.callType, Pool: pool, CreatedAt: now,
	})
	if err != nil {
		return nil, fmt.Errorf("record transaction: %w", err)
	}
	return &ChargeUserResult{ApiCostMc: charge.apiCostMc, UserChargedMc: charge.userChargedMc,
		Pool: pool, NewPaidBalance: bal.PaidBalanceMc, TransactionID: txID}, nil
}

func (uc *BillingUseCase) recordProfit(ctx context.Context, cmd ChargeUserCommand, charge preparedCharge, result *ChargeUserResult) {
	settledTxID := result.TransactionID
	profit := &model.PlatformProfit{
		ID:            uuid.New(),
		UserID:        cmd.UserID,
		TransactionID: &settledTxID,
		ModelName:     cmd.ModelName,
		CallType:      charge.callType,
		Pool:          result.Pool,
		InputTokens:   cmd.InputTokens,
		OutputTokens:  cmd.OutputTokens,
		ApiCostMc:     charge.apiCostMc,
		UserChargedMc: charge.userChargedMc,
		ProfitMc:      charge.userChargedMc - charge.apiCostMc,
		Markup:        float64(charge.markupNum) / float64(charge.markupDen),
	}
	if err := uc.profitRepo.CreateProfitTx(ctx, nil, profit); err != nil {
		fmt.Printf(
			"[billing] WARN profit-tracking insert failed (charge ALREADY committed — user WAS billed) user=%s model=%s pool=%s: %v\n",
			cmd.UserID, cmd.ModelName, result.Pool, err,
		)
	}
}

// SettleOperation persists provider usage before applying the unchanged
// ChargeUser calculation, then closes the hold. If charging fails, the usage
// and failure remain durable for reconciliation instead of disappearing into a
// log line.
func (uc *BillingUseCase) SettleOperation(ctx context.Context, operationID uuid.UUID, cmd ChargeUserCommand) (*ChargeUserResult, error) {
	if uc.operationRepo == nil {
		return nil, errors.New("durable billing operations are not configured")
	}
	op, err := uc.operationRepo.GetOperationByIDTx(ctx, nil, operationID)
	if err != nil {
		return nil, err
	}
	if op.UserID != cmd.UserID || op.ModelName != cmd.ModelName {
		return nil, errors.New("billing operation does not match settlement")
	}
	if op.Status == model.OperationSettled {
		return &ChargeUserResult{ApiCostMc: op.ApiCostMc, UserChargedMc: op.UserChargedMc,
			Pool: op.Pool, TransactionID: valueOrNilUUID(op.CreditTransactionID)}, nil
	}
	if op.Status == model.OperationVoided {
		return nil, errors.New("billing operation was voided")
	}
	charge, err := uc.prepareCharge(ctx, cmd)
	if err != nil {
		_ = uc.operationRepo.MarkOperationSettlementFailed(ctx, operationID, err.Error())
		return nil, err
	}
	if err := uc.operationRepo.RecordOperationUsage(ctx, operationID, cmd.InputTokens,
		cmd.CachedInputTokens, cmd.OutputTokens, cmd.FlatFeeMc, cmd.CallType); err != nil {
		return nil, fmt.Errorf("record operation usage: %w", err)
	}
	if charge.userChargedMc == 0 {
		// A zero-cost provider call has no ledger row, but must release its hold.
		if err := uc.operationRepo.VoidOperation(ctx, operationID, "zero-cost call"); err != nil {
			return nil, err
		}
		return &ChargeUserResult{}, nil
	}
	var result *ChargeUserResult
	didSettle := false
	err = uc.txManager.WithTx(ctx, func(tx any) error {
		locked, err := uc.operationRepo.GetOperationByIDTx(ctx, tx, operationID)
		if err != nil {
			return err
		}
		if locked.Status == model.OperationSettled {
			result = &ChargeUserResult{ApiCostMc: locked.ApiCostMc, UserChargedMc: locked.UserChargedMc,
				Pool: locked.Pool, TransactionID: valueOrNilUUID(locked.CreditTransactionID)}
			return nil
		}
		if locked.Status == model.OperationVoided {
			return errors.New("billing operation was voided")
		}
		result, err = uc.settlePreparedTx(ctx, tx, cmd, charge)
		if err != nil {
			return err
		}
		if err := uc.operationRepo.MarkOperationSettledTx(ctx, tx, operationID, result.TransactionID,
			result.ApiCostMc, result.UserChargedMc, result.Pool); err != nil {
			return err
		}
		didSettle = true
		return nil
	})
	if err != nil {
		_ = uc.operationRepo.MarkOperationSettlementFailed(ctx, operationID, err.Error())
		return nil, err
	}
	if didSettle {
		uc.recordProfit(ctx, cmd, charge, result)
	}
	return result, nil
}

func valueOrNilUUID(id *uuid.UUID) uuid.UUID {
	if id == nil {
		return uuid.Nil
	}
	return *id
}

func (uc *BillingUseCase) VoidOperation(ctx context.Context, id uuid.UUID, reason string) error {
	if uc.operationRepo == nil {
		return errors.New("durable billing operations are not configured")
	}
	return uc.operationRepo.VoidOperation(ctx, id, reason)
}

// ReconcileUnsettledOperations retries usage that was durably recorded but
// could not be charged because of a transient database failure.
func (uc *BillingUseCase) ReconcileUnsettledOperations(ctx context.Context, limit int) (int, error) {
	if uc.operationRepo == nil {
		return 0, errors.New("durable billing operations are not configured")
	}
	operations, err := uc.operationRepo.ListUnsettledOperations(ctx, limit)
	if err != nil {
		return 0, err
	}
	settled := 0
	for _, op := range operations {
		_, err := uc.SettleOperation(ctx, op.ID, ChargeUserCommand{
			UserID: op.UserID, ModelName: op.ModelName, InputTokens: op.InputTokens,
			CachedInputTokens: op.CachedInputTokens, OutputTokens: op.OutputTokens,
			FlatFeeMc: op.FlatFeeMc, CallType: op.CallType,
		})
		if err == nil {
			settled++
		}
	}
	return settled, nil
}

// ── Minimum-balance pre-flight gating ─────────────────────────────────────────
//
// We do NOT estimate a call's cost to gate it. A length-based estimate is
// cache-blind (it prices the whole history at the full input rate) and floored,
// so it systematically over-blocks affordable calls — a user with real cached
// context would be refused for a call that actually costs a fraction of the
// guess. Instead the gate is a flat per-model minimum: hold at least
// min_balance_mc(model) in a usable pool and the call proceeds; the ACTUAL cost
// is settled afterward. Because every step settles immediately, any overshoot
// is bounded to a single step.

// minBalanceFor returns the minimum wallet balance (micro-cents) required to
// call modelName, from the model's DB row, falling back to the global
// min_required_credits_mc floor when a row leaves it unset. Unknown models are
// REFUSED (the price catalog is the source of truth; no guessed pricing).
func (uc *BillingUseCase) minBalanceFor(ctx context.Context, modelName string) (int64, error) {
	price, err := uc.priceRepo.GetModelPrice(ctx, modelName)
	if err != nil {
		return 0, fmt.Errorf("model %q is not available: %w", modelName, err)
	}
	if price.MinBalanceMc > 0 {
		return price.MinBalanceMc, nil
	}
	return uc.MinRequiredCredits(ctx), nil
}

// PreflightGate authorises ONE upcoming LLM call by checking the user holds at
// least the model's minimum balance in a usable pool (paid first, then free) —
// no cost estimate, no capacity lock, nothing confiscated. If no pool clears
// the minimum it returns the window-specific exhaustion error and leaves the
// balance untouched (any leftover stays spendable on a cheaper/smaller call).
//
// Call this before EVERY LLM call on the server-key path (task steps,
// self-heal, subtasks). BYOK calls that use the user's own key skip it.
func (uc *BillingUseCase) PreflightGate(ctx context.Context, userID uuid.UUID, modelName string) error {
	minBal, err := uc.minBalanceFor(ctx, modelName)
	if err != nil {
		return err
	}

	// Resolve the markup HERE, before the model runs, purely to validate it.
	// Settle() swallows charge errors and logs "step NOT billed", so a broken
	// markup config discovered at settlement time would hand out free compute
	// on every request. Failing at the gate turns that into a refused request
	// instead — the safe direction for a billing misconfiguration.
	if _, _, err := uc.markup(ctx); err != nil {
		return err
	}
	orgID, err := uc.ActiveOrganizationID(ctx, userID)
	if err != nil {
		return fmt.Errorf("resolve active organization: %w", err)
	}

	var exhaustErr error
	txErr := uc.txManager.WithTx(ctx, func(tx any) error {
		bal, err := uc.getOrInitBalanceTx(ctx, tx, userID, orgID)
		if err != nil {
			return err
		}
		now := time.Now().UTC()
		advancedW := advanceFreeMonthly(bal, now)
		advancedM := advanceMonthlyRenewal(bal, now)

		if _, ok := selectPool(bal, minBal); !ok {
			// Not enough in any pool to start. Persist any window rollover so the
			// exhaustion reason/reset time is accurate, then block — WITHOUT
			// touching the balance (no drain: the leftover is still usable for a
			// smaller/cheaper call once a window resets).
			exhaustErr = bal.ExhaustionError(minBal)
		}

		// Write back ONLY if a window actually rolled over — otherwise this is a
		// pure read, and skipping the write halves the row-write traffic per step
		// (pre-flight + settle → settle only).
		if advancedW || advancedM {
			bal.UpdatedAt = now
			return uc.balanceRepo.UpdateTx(ctx, tx, bal)
		}
		return nil
	})
	if txErr != nil {
		return txErr
	}
	return exhaustErr
}

// Settle charges an already-consumed step's ACTUAL cost via ChargeUser. It
// never confiscates and never fails on affordability — ChargeUser floors the
// paid balance at 0 and books any single-step overshoot into monthly usage. A
// returned error means an operational failure (unknown model,
// inactive model, DB down), in which case the step is logged as uncollected
// rather than destroying the user's balance. The NEXT pre-flight is what blocks.
func (uc *BillingUseCase) Settle(ctx context.Context, cmd ChargeUserCommand) {
	result, err := uc.ChargeUser(ctx, cmd)
	if err != nil {
		fmt.Printf("[billing] ERROR step charge failed user=%s model=%s in=%d cached=%d out=%d: %v — step NOT billed\n",
			cmd.UserID, cmd.ModelName, cmd.InputTokens, cmd.CachedInputTokens, cmd.OutputTokens, err)
		return
	}
	// Per-step settlement audit line: what THIS call cost the user, from which
	// pool, and what remains — visible in logs as spend happens.
	fmt.Printf(
		"[billing] step settled user=%s model=%s in=%d cached=%d out=%d api_cost=%dmc charged=%dmc pool=%s paid_balance=%dmc\n",
		cmd.UserID, cmd.ModelName,
		cmd.InputTokens, cmd.CachedInputTokens, cmd.OutputTokens,
		result.ApiCostMc, result.UserChargedMc, result.Pool, result.NewPaidBalance,
	)
}

// selectSettlePool chooses which pool settles an already-run step. It mirrors
// the pre-flight gate (paid first, then consented credits, then free — whichever
// holds the model minimum) so settlement charges the same pool that authorised
// the call. When the balance moved below the minimum since the gate (e.g. a
// concurrent task), the step still ran, so it falls back to the user's tier pool
// and the overage is booked there.
func selectSettlePool(bal *model.UserBalance, minBal int64) string {
	if pool, ok := selectPool(bal, minBal); ok {
		return pool
	}
	if bal.IsPaid {
		return model.PoolPaid
	}
	// Consent applies to settlement too, or a step that slipped past the gate
	// would charge credits the user never authorised. `>= cost` rather than
	// `> 0`: a single micro-cent of deposit must not absorb an arbitrarily
	// large overage.
	if bal.AllowCreditUsage && bal.DepositBalanceMc >= minBal {
		return model.PoolDeposit
	}
	return model.PoolFree
}

// applySettlement charges `cost` (the marked-up user charge) to a pool, never
// going negative. The paid monthly balance and the deposit balance floor at 0;
// the free pool tracks a used-counter against its monthly cap.
func applySettlement(bal *model.UserBalance, pool string, cost int64) {
	switch pool {
	case model.PoolPaid:
		if bal.PaidBalanceMc >= cost {
			bal.PaidBalanceMc -= cost
		} else {
			bal.PaidBalanceMc = 0
		}
	case model.PoolDeposit:
		if bal.DepositBalanceMc >= cost {
			bal.DepositBalanceMc -= cost
		} else {
			bal.DepositBalanceMc = 0
		}
	case model.PoolFree:
		bal.FreeMonthlyUsed += cost
	}
}

// MinRequiredCredits is the minimum micro-cents needed to start a task,
// configurable via system_configs (default 5000 ~= $0.005).
func (uc *BillingUseCase) MinRequiredCredits(ctx context.Context) int64 {
	return uc.configInt(ctx, "min_required_credits_mc", 5000)
}

// RestrictedToFreeModel reports whether the user must be forced onto the free
// model: they have no premium-capable funds (no paid allowance and no consented
// credits). This is the single source of truth for the free-model gate — the
// agent path uses it to substitute the model, and the account API surfaces it so
// the client can lock the model selector with a matching reason. It is NOT
// purely tier-based: a free-tier user who buys credits and switches them on can
// run premium models, and a paid user who exhausts their allowance with credits
// off is restricted, exactly like a free user.
func (uc *BillingUseCase) RestrictedToFreeModel(ctx context.Context, bal *model.UserBalance) bool {
	return !bal.CanUsePremiumModel(uc.MinRequiredCredits(ctx))
}

// configModel reads a string model-name config, falling back to def when the
// row is missing or blank. Every internal-task model name is resolved this way
// so a deprecated model can be swapped in the DB (system_configs) without a
// code change — see migration 00024 for the seeded keys.
func (uc *BillingUseCase) configModel(ctx context.Context, key, def string) string {
	v, err := uc.configRepo.Get(ctx, key)
	if err != nil || strings.TrimSpace(v) == "" {
		return def
	}
	return strings.TrimSpace(v)
}

// FreeTierModel is the model free-tier users are restricted to (config
// free_tier_model).
func (uc *BillingUseCase) FreeTierModel(ctx context.Context) string {
	return uc.configModel(ctx, "free_tier_model", "gpt-5.6-luna")
}

// DefaultAgentModel is the fallback agent model used when the client sends no
// model name (config default_agent_model).
func (uc *BillingUseCase) DefaultAgentModel(ctx context.Context) string {
	return uc.configModel(ctx, "default_agent_model", "gpt-5.6-luna")
}

// SelfHealModel is the model used to diagnose failed terminal commands (config
// selfheal_model).
func (uc *BillingUseCase) SelfHealModel(ctx context.Context) string {
	return uc.configModel(ctx, "selfheal_model", "gpt-5.4-mini")
}

// WebExtractModel is the Anthropic model used to extract main content from
// fetched web pages (config web_extract_model). It MUST remain an Anthropic
// model — the extraction endpoint calls the Anthropic API directly.
func (uc *BillingUseCase) WebExtractModel(ctx context.Context) string {
	return uc.configModel(ctx, "web_extract_model", "claude-haiku-4-5-20251001")
}

// WebSearchFeeMc returns the flat per-search provider fee in micro-dollars
// for a web-tool provider family, config-overridable per family
// (web_search_fee_mc_<family> in system_configs). Compiled defaults track the
// providers' published list prices:
//
//	exa        $7/1k  requests            =  7,000 mc
//	anthropic  $10/1k searches            = 10,000 mc
//	openai     $10/1k tool calls (mini)   = 10,000 mc
//	gemini     $35/1k grounded prompts    = 35,000 mc
//	xai        $25/1k Live Search sources = 25,000 mc
//
// Exa bills per REQUEST, not per result, and involves no model — so its fee is
// the entire cost of a search, where the provider families also bill the tokens
// their digest model consumes on top of the fee below.
func (uc *BillingUseCase) WebSearchFeeMc(ctx context.Context, family string) int64 {
	defaults := map[string]int64{
		"exa":       7_000, // $7 per 1,000 requests
		"anthropic": 10_000,
		"openai":    10_000,
		"gemini":    35_000,
		"xai":       25_000,
	}
	def, ok := defaults[family]
	if !ok {
		return 0
	}
	return uc.configInt(ctx, "web_search_fee_mc_"+family, def)
}

// markup returns the (numerator, denominator) for the api_cost → user_charged
// markup. The DATABASE IS THE ONLY SOURCE — there is deliberately no compiled
// fallback, so the rate that is charged is always the rate an operator can see
// and audit in system_configs. Both rows are seeded by migration 00015.
//
// A missing, unparseable, or non-positive value is an ERROR, not a default.
// That matters because the alternative is worse in both directions: charging a
// silently different rate than the one configured, or (if it defaulted to zero)
// charging nothing at all.
func (uc *BillingUseCase) markup(ctx context.Context) (int64, int64, error) {
	num, err := uc.configIntStrict(ctx, "markup_numerator")
	if err != nil {
		return 0, 0, err
	}
	den, err := uc.configIntStrict(ctx, "markup_denominator")
	if err != nil {
		return 0, 0, err
	}
	if err := model.ValidateMarkup(num, den); err != nil {
		return 0, 0, err
	}
	return num, den, nil
}

// configIntStrict reads an integer config with NO fallback: absent or
// unparseable is an error the caller must handle.
func (uc *BillingUseCase) configIntStrict(ctx context.Context, key string) (int64, error) {
	v, err := uc.configRepo.Get(ctx, key)
	if err != nil {
		return 0, fmt.Errorf("billing misconfigured: system_configs %q is missing: %w", key, err)
	}
	n, err := strconv.ParseInt(strings.TrimSpace(v), 10, 64)
	if err != nil {
		return 0, fmt.Errorf(
			"billing misconfigured: system_configs %q = %q is not an integer "+
				"(use integers, e.g. 1.9x markup = numerator 19 / denominator 10)", key, v)
	}
	return n, nil
}

// configInt reads an integer config value, returning def on any miss/parse
// error so a bad or absent row never breaks billing. A parse failure is
// LOGGED loudly: a config row that exists but doesn't parse means an operator
// changed a control (e.g. markup) and the system silently kept the old value
// — exactly how a "3.8" markup once stayed at the 2.5x default unnoticed.
func (uc *BillingUseCase) configInt(ctx context.Context, key string, def int64) int64 {
	v, err := uc.configRepo.Get(ctx, key)
	if err != nil {
		return def
	}
	n, err := strconv.ParseInt(v, 10, 64)
	if err != nil {
		fmt.Printf(
			"[billing] WARNING config %s=%q is not an integer — IGNORED, using default %d. Use integer values (e.g. markup 1.9x = numerator 19 / denominator 10).\n",
			key, v, def,
		)
		return def
	}
	return n
}

// advanceFreeMonthly resets the free monthly used-counter when the allowance's
// calendar month has elapsed. Mirrors advanceMonthlyRenewal (paid), but the
// free pool always renews — there is no payment status to gate it. Returns true
// when it modified the balance (callers can then skip a pointless row write on
// otherwise read-only paths).
func advanceFreeMonthly(bal *model.UserBalance, now time.Time) bool {
	if bal.FreeMonthlyResetAt == nil {
		// Establish the first window: one calendar month out. The current
		// used-counter is preserved (this is not a reset, just anchoring).
		next := model.AddOneMonth(now)
		bal.FreeMonthlyResetAt = &next
		return true
	}
	if now.Before(*bal.FreeMonthlyResetAt) {
		return false
	}
	// The month has elapsed: zero the counter and advance the reset date in
	// whole calendar months so a long gap doesn't leave it in the past.
	next := *bal.FreeMonthlyResetAt
	for !now.Before(next) {
		next = model.AddOneMonth(next)
	}
	bal.FreeMonthlyResetAt = &next
	bal.FreeMonthlyUsed = 0
	return true
}

// advanceMonthlyRenewal grants the next subscription allowance once the reset
// date has passed. Returns true when it fired.
//
// This MUST run in the pre-flight gate as well as in the charge. It used to
// live only in ChargeUser, which runs *after* the gate passes — so a paid user
// sitting at zero balance past their renewal date was blocked by the gate, and
// the renewal that would have unblocked them could never execute. Permanent
// lockout for someone who had already paid.
func advanceMonthlyRenewal(bal *model.UserBalance, now time.Time) bool {
	if bal.PaidMonthlyResetAt == nil || now.Before(*bal.PaidMonthlyResetAt) {
		return false
	}
	// The date says a new cycle is due; only the payment status says it was
	// PAID for. During dunning the subscription is still alive with its caps
	// intact and its reset date in the past — precisely this state — so
	// without this check a failed payment silently bought another month, and
	// the catch-up loop below repeated it every 30 days.
	if !bal.EligibleForClockRenewal() {
		return false
	}
	bal.PaidBalanceMc = bal.PaidMonthlyCap
	// Catch up in whole cycles so a long gap doesn't leave the reset date in
	// the past and re-fire on the next request. Calendar months, matching the
	// provider's own schedule — a flat 30 days drifts and comes due early in
	// any 31-day month.
	next := *bal.PaidMonthlyResetAt
	for !now.Before(next) {
		next = model.AddOneMonth(next)
	}
	bal.PaidMonthlyResetAt = &next
	return true
}

// selectPool chooses which pool covers a `cost` charge: the monthly paid
// allowance first, then the monthly free allowance, then purchased credits (only
// with the user's opt-in). Returns (pool, true) or ("", false) when nothing can
// serve it — the caller then derives the user-facing reason from the balance
// via ExhaustionError. Both the paid and free pools are monthly allowances,
// each usable at any pace.
func selectPool(bal *model.UserBalance, cost int64) (string, bool) {
	// Treat the paid allowance as eligible only while the subscription is
	// actually paid. Legacy/downgraded rows may contain a stale numeric balance;
	// consuming it would charge an invisible pool and leave the free usage meter
	// unchanged even though the user is on the free tier.
	if bal.IsPaid && bal.PaidBalanceMc >= cost {
		return model.PoolPaid, true
	}
	// Purchased credits BEFORE the free allowance, and only with explicit
	// consent. The free pool only ever backs the free-tier model, so spending
	// credits before it keeps the user's SELECTED (premium) model working after
	// their paid allowance runs out. Dropping to free would silently downgrade
	// them to the free model — exactly what paying for credits should avoid.
	if bal.AllowCreditUsage && bal.DepositBalanceMc >= cost {
		return model.PoolDeposit, true
	}
	// Free allowance last: reaching it means the user has no premium-capable
	// funds, so the call is (server-side) restricted to the free-tier model.
	if bal.FreeMonthlyCap-bal.FreeMonthlyUsed >= cost {
		return model.PoolFree, true
	}
	return "", false
}

// VerifyMarkupConfigured resolves and validates the markup exactly as a charge
// would. Call it at startup: the markup has no compiled default, so a missing
// or invalid row means the service cannot bill anyone. Discovering that at boot
// (loudly, before serving traffic) is far better than discovering it when the
// first user is refused — or, worse, at settlement time where the error is
// swallowed and the work is given away.
func (uc *BillingUseCase) VerifyMarkupConfigured(ctx context.Context) error {
	num, den, err := uc.markup(ctx)
	if err != nil {
		return err
	}
	fmt.Printf("[billing] markup configured: %d/%d (%.3gx) — from system_configs\n",
		num, den, float64(num)/float64(den))
	return nil
}
