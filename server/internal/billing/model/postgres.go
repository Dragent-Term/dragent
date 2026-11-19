package model

import (
	"context"
	"database/sql"
	"errors"
	"fmt"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

type dbConn interface {
	Exec(ctx context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

type PostgresBillingRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresBillingRepository(pool *pgxpool.Pool) *PostgresBillingRepository {
	return &PostgresBillingRepository{pool: pool}
}

func (r *PostgresBillingRepository) getDB(tx any) dbConn {
	if tx != nil {
		if pgTx, ok := tx.(pgx.Tx); ok {
			return pgTx
		}
	}
	return r.pool
}

// ── BalanceRepository ─────────────────────────────────────────────────────────

// balanceColumns and scanUserBalance are shared by every query that reads a
// user_balances row, so the column list and error mapping live in exactly
// one place regardless of which WHERE clause a caller needs.
const balanceColumns = `
	user_id, organization_id, tier, is_paid, paid_status,
	paid_balance_mc, paid_monthly_cap_mc, paid_monthly_reset_at,
	deposit_balance_mc,
	free_monthly_cap_mc, free_monthly_used_mc, free_monthly_reset_at,
	allow_credit_usage,
	updated_at
`

func scanUserBalance(row pgx.Row) (*UserBalance, error) {
	var bal UserBalance
	err := row.Scan(
		&bal.UserID, &bal.OrganizationID, &bal.Tier, &bal.IsPaid, &bal.PaidStatus,
		&bal.PaidBalanceMc, &bal.PaidMonthlyCap, &bal.PaidMonthlyResetAt,
		&bal.DepositBalanceMc,
		&bal.FreeMonthlyCap, &bal.FreeMonthlyUsed, &bal.FreeMonthlyResetAt,
		&bal.AllowCreditUsage,
		&bal.UpdatedAt,
	)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &bal, nil
}

// GetByUserID is the plain (non-locking) read — the account/usage/voice/
// byok screens. GetByUserIDTx is its FOR-UPDATE counterpart for the
// transactional charge/authorize read-modify-write path; both are scoped
// (user_id, organization_id) and share the same
// query shape below, differing only in the lock.
func (r *PostgresBillingRepository) GetByUserID(ctx context.Context, userID, organizationID uuid.UUID) (*UserBalance, error) {
	return r.getByUserID(ctx, nil, userID, organizationID)
}

func (r *PostgresBillingRepository) GetByUserIDTx(ctx context.Context, tx any, userID, organizationID uuid.UUID) (*UserBalance, error) {
	return r.getByUserID(ctx, tx, userID, organizationID)
}

func (r *PostgresBillingRepository) getByUserID(ctx context.Context, tx any, userID, organizationID uuid.UUID) (*UserBalance, error) {
	db := r.getDB(tx)
	query := `SELECT ` + balanceColumns + ` FROM user_balances WHERE user_id = $1 AND organization_id = $2`
	// Inside a transaction this read feeds a read-modify-write (charge,
	// preflight, capacity lock) — take a row lock so concurrent charges
	// SERIALIZE instead of silently losing each other's counter updates.
	// Plain reads (tx == nil) skip the lock.
	if tx != nil {
		query += " FOR UPDATE"
	}
	return scanUserBalance(db.QueryRow(ctx, query, userID, organizationID))
}

// UpdateTx writes back the (balance.UserID, balance.OrganizationID) row —
// the exact pair GetByUserIDTx's result identifies, so a caller never
// separately tracks which org it read from before writing back to it.
func (r *PostgresBillingRepository) UpdateTx(ctx context.Context, tx any, balance *UserBalance) error {
	db := r.getDB(tx)
	const query = `
		UPDATE user_balances
		SET tier                 = $1,
		    paid_balance_mc      = $2,
		    paid_monthly_reset_at= $3,
		    deposit_balance_mc   = $4,
		    free_monthly_used_mc = $5,
		    free_monthly_reset_at= $6,
		    updated_at           = $7
		WHERE user_id = $8 AND organization_id = $9
	`
	_, err := db.Exec(ctx, query,
		balance.Tier,
		balance.PaidBalanceMc,
		balance.PaidMonthlyResetAt,
		balance.DepositBalanceMc,
		balance.FreeMonthlyUsed,
		balance.FreeMonthlyResetAt,
		balance.UpdatedAt,
		balance.UserID,
		balance.OrganizationID,
	)
	return err
}

// InitializeTx creates a user's billing row, scoped to organizationID (the
// org they just landed in at signup — see identity's OrganizationInitializer,
// which must run first, or whichever org GetOrInitBalance resolved as
// active for a lazy first-use creation). (user_id, organization_id) is the
// real primary key — a user can hold one row per org they have a seat in.
func (r *PostgresBillingRepository) InitializeTx(ctx context.Context, tx any, userID, organizationID uuid.UUID) error {
	db := r.getDB(tx)
	const query = `
		INSERT INTO user_balances
			(user_id, organization_id, paid_balance_mc, paid_monthly_cap_mc, free_monthly_reset_at, updated_at)
		VALUES ($1, $2, 0, 0, NOW() + INTERVAL '1 month', NOW())
		ON CONFLICT (user_id, organization_id) DO NOTHING
	`
	_, err := db.Exec(ctx, query, userID, organizationID)
	return err
}

// ── BillingOperationRepository ───────────────────────────────────────────────

const billingOperationColumns = `
	id, operation_key, task_id, user_id, operation_type, model_name, round_number,
	pool, minimum_hold_mc, status, input_tokens, cached_input_tokens, output_tokens,
	flat_fee_mc, call_type, api_cost_mc, user_charged_mc, credit_transaction_id,
	last_error, created_at, updated_at, expires_at`

func scanBillingOperation(row pgx.Row) (*BillingOperation, error) {
	var op BillingOperation
	err := row.Scan(&op.ID, &op.OperationKey, &op.TaskID, &op.UserID,
		&op.OperationType, &op.ModelName, &op.RoundNumber, &op.Pool,
		&op.MinimumHoldMc, &op.Status, &op.InputTokens, &op.CachedInputTokens,
		&op.OutputTokens, &op.FlatFeeMc, &op.CallType, &op.ApiCostMc,
		&op.UserChargedMc, &op.CreditTransactionID, &op.LastError,
		&op.CreatedAt, &op.UpdatedAt, &op.ExpiresAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, sql.ErrNoRows
	}
	return &op, err
}

func (r *PostgresBillingRepository) GetOperationByKeyTx(ctx context.Context, tx any, key string) (*BillingOperation, error) {
	query := `SELECT ` + billingOperationColumns + ` FROM billing_operations WHERE operation_key=$1`
	if tx != nil {
		query += ` FOR UPDATE`
	}
	return scanBillingOperation(r.getDB(tx).QueryRow(ctx, query, key))
}

func (r *PostgresBillingRepository) GetOperationByIDTx(ctx context.Context, tx any, id uuid.UUID) (*BillingOperation, error) {
	query := `SELECT ` + billingOperationColumns + ` FROM billing_operations WHERE id=$1`
	if tx != nil {
		query += ` FOR UPDATE`
	}
	return scanBillingOperation(r.getDB(tx).QueryRow(ctx, query, id))
}

func (r *PostgresBillingRepository) CreateOperationTx(ctx context.Context, tx any, op *BillingOperation) error {
	_, err := r.getDB(tx).Exec(ctx, `INSERT INTO billing_operations
		(id, operation_key, task_id, user_id, operation_type, model_name, round_number,
		 pool, minimum_hold_mc, status, call_type, created_at, updated_at, expires_at)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$12,$13)`,
		op.ID, op.OperationKey, op.TaskID, op.UserID, op.OperationType,
		op.ModelName, op.RoundNumber, op.Pool, op.MinimumHoldMc, op.Status,
		op.CallType, op.CreatedAt, op.ExpiresAt)
	return err
}

func (r *PostgresBillingRepository) ActiveHoldsByPoolTx(ctx context.Context, tx any, userID uuid.UUID) (map[string]int64, error) {
	rows, err := r.getDB(tx).Query(ctx, `SELECT pool, COALESCE(SUM(minimum_hold_mc),0)
		FROM billing_operations WHERE user_id=$1
		AND (status IN ('usage_recorded','settlement_failed')
		     OR (status='authorized' AND expires_at > NOW())) GROUP BY pool`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	holds := make(map[string]int64)
	for rows.Next() {
		var pool string
		var amount int64
		if err := rows.Scan(&pool, &amount); err != nil {
			return nil, err
		}
		holds[pool] = amount
	}
	return holds, rows.Err()
}

func (r *PostgresBillingRepository) RecordOperationUsage(ctx context.Context, id uuid.UUID, input, cached, output, flat int64, callType string) error {
	if callType == "" {
		callType = CallTypeAgent
	}
	_, err := r.pool.Exec(ctx, `UPDATE billing_operations SET
		input_tokens=$2, cached_input_tokens=$3, output_tokens=$4, flat_fee_mc=$5,
		call_type=$6, status=CASE WHEN status='settled' THEN status ELSE 'usage_recorded' END,
		last_error='', updated_at=NOW() WHERE id=$1 AND status <> 'voided'`,
		id, input, cached, output, flat, callType)
	return err
}

func (r *PostgresBillingRepository) MarkOperationSettledTx(ctx context.Context, tx any, id, transactionID uuid.UUID, apiCost, charged int64, pool string) error {
	_, err := r.getDB(tx).Exec(ctx, `UPDATE billing_operations SET status='settled',
		credit_transaction_id=$2, api_cost_mc=$3, user_charged_mc=$4, pool=$5,
		last_error='', settled_at=NOW(), updated_at=NOW() WHERE id=$1`,
		id, transactionID, apiCost, charged, pool)
	return err
}

func (r *PostgresBillingRepository) MarkOperationSettlementFailed(ctx context.Context, id uuid.UUID, message string) error {
	_, err := r.pool.Exec(ctx, `UPDATE billing_operations SET status='settlement_failed',
		last_error=$2, updated_at=NOW() WHERE id=$1 AND status <> 'settled'`, id, message)
	return err
}

func (r *PostgresBillingRepository) VoidOperation(ctx context.Context, id uuid.UUID, reason string) error {
	_, err := r.pool.Exec(ctx, `UPDATE billing_operations SET status='voided',
		last_error=$2, updated_at=NOW() WHERE id=$1 AND status IN ('authorized','usage_recorded')`, id, reason)
	return err
}

func (r *PostgresBillingRepository) ListUnsettledOperations(ctx context.Context, limit int) ([]*BillingOperation, error) {
	if limit <= 0 || limit > 500 {
		limit = 100
	}
	rows, err := r.pool.Query(ctx, `SELECT `+billingOperationColumns+` FROM billing_operations
		WHERE status IN ('usage_recorded','settlement_failed') ORDER BY updated_at LIMIT $1`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var operations []*BillingOperation
	for rows.Next() {
		op, err := scanBillingOperation(rows)
		if err != nil {
			return nil, err
		}
		operations = append(operations, op)
	}
	return operations, rows.Err()
}

// ── PriceRepository ───────────────────────────────────────────────────────────

func (r *PostgresBillingRepository) GetModelPrice(ctx context.Context, modelName string) (*ModelPrice, error) {
	const query = `
		SELECT model_name,
		       input_rate_per_million, cache_write_rate_per_million,
		       cached_input_rate_per_million, output_rate_per_million,
		       long_context_threshold,
		       long_context_input_rate_per_million,
		       long_context_cached_input_rate_per_million,
		       long_context_output_rate_per_million,
		       context_window_tokens, min_balance_mc,
		       is_active, is_user_facing
		FROM model_prices ORDER BY LENGTH(model_name) DESC
	`
	rows, err := r.pool.Query(ctx, query)
	if err != nil {
		// Fail CLOSED: no silent fallback pricing. A charge or gate for this
		// call will refuse rather than bill at made-up rates.
		return nil, fmt.Errorf("model price lookup: %w", err)
	}
	defer rows.Close()

	var exact *ModelPrice
	var prefixMatch *ModelPrice

	for rows.Next() {
		p, scanErr := scanModelPrice(rows)
		if scanErr != nil {
			continue
		}
		if p.ModelName == modelName {
			exact = p
			break
		}
		if prefixMatch == nil &&
			len(modelName) >= len(p.ModelName) &&
			modelName[:len(p.ModelName)] == p.ModelName {
			prefixMatch = p
		}
	}

	if exact != nil {
		return exact, nil
	}
	// Longest-prefix match still serves dated variants (e.g. a
	// "claude-sonnet-4-6-20260101" request matches the "claude-sonnet-4-6"
	// row) — but generic catch-alls ("claude-", "gpt-") were removed from the
	// catalog (migration 00017), so this can no longer family-match arbitrary
	// unknown models.
	if prefixMatch != nil {
		return prefixMatch, nil
	}
	// Unknown model: REFUSE. The old behavior silently billed unknown models
	// at hardcoded $3/$15 rates — a typo'd model name overcharged mini-class
	// users up to ~30x with no log. BYOK custom models never reach here.
	return nil, fmt.Errorf("%w: %q", ErrUnknownModel, modelName)
}

func (r *PostgresBillingRepository) ListUserFacingModels(ctx context.Context) ([]*UserFacingModel, error) {
	const query = `
		SELECT model_name, COALESCE(context_window_tokens, 0), is_active
		FROM model_prices WHERE is_user_facing = TRUE
		ORDER BY model_name
	`
	rows, err := r.pool.Query(ctx, query)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var models []*UserFacingModel
	for rows.Next() {
		var m UserFacingModel
		if err := rows.Scan(&m.ModelName, &m.ContextWindowTokens, &m.IsActive); err != nil {
			return nil, err
		}
		models = append(models, &m)
	}
	return models, nil
}

func scanModelPrice(rows pgx.Rows) (*ModelPrice, error) {
	var p ModelPrice
	var (
		cacheWrite    *int64
		cachedInput   *int64
		lcThreshold   *int64
		lcInput       *int64
		lcCachedInput *int64
		lcOutput      *int64
		contextWindow *int64
		minBalance    *int64
	)
	err := rows.Scan(
		&p.ModelName,
		&p.InputRatePerMillion, &cacheWrite,
		&cachedInput, &p.OutputRatePerMillion,
		&lcThreshold, &lcInput, &lcCachedInput, &lcOutput,
		&contextWindow, &minBalance,
		&p.IsActive, &p.IsUserFacing,
	)
	if err != nil {
		return nil, err
	}
	if cacheWrite != nil {
		p.CacheWriteRatePerMillion = *cacheWrite
	}
	if cachedInput != nil {
		p.CachedInputRatePerMillion = *cachedInput
	}
	if lcThreshold != nil {
		p.LongContextThreshold = *lcThreshold
	}
	if lcInput != nil {
		p.LongContextInputRatePerMillion = *lcInput
	}
	if lcCachedInput != nil {
		p.LongContextCachedInputRatePerMillion = *lcCachedInput
	}
	if lcOutput != nil {
		p.LongContextOutputRatePerMillion = *lcOutput
	}
	if contextWindow != nil {
		p.ContextWindowTokens = *contextWindow
	}
	if minBalance != nil {
		p.MinBalanceMc = *minBalance
	}
	return &p, nil
}

func (r *PostgresBillingRepository) UpdateModelPrice(ctx context.Context, mp *ModelPrice) error {
	const query = `
		INSERT INTO model_prices (
			model_name,
			input_rate_per_million, cache_write_rate_per_million,
			cached_input_rate_per_million, output_rate_per_million,
			long_context_threshold,
			long_context_input_rate_per_million,
			long_context_cached_input_rate_per_million,
			long_context_output_rate_per_million,
			context_window_tokens, min_balance_mc,
			is_active, is_user_facing
		) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
		ON CONFLICT (model_name) DO UPDATE SET
			input_rate_per_million                     = EXCLUDED.input_rate_per_million,
			cache_write_rate_per_million               = EXCLUDED.cache_write_rate_per_million,
			cached_input_rate_per_million              = EXCLUDED.cached_input_rate_per_million,
			output_rate_per_million                    = EXCLUDED.output_rate_per_million,
			long_context_threshold                     = EXCLUDED.long_context_threshold,
			long_context_input_rate_per_million        = EXCLUDED.long_context_input_rate_per_million,
			long_context_cached_input_rate_per_million = EXCLUDED.long_context_cached_input_rate_per_million,
			long_context_output_rate_per_million       = EXCLUDED.long_context_output_rate_per_million,
			context_window_tokens                      = EXCLUDED.context_window_tokens,
			min_balance_mc                             = EXCLUDED.min_balance_mc,
			is_active                                  = EXCLUDED.is_active,
			is_user_facing                             = EXCLUDED.is_user_facing
	`
	nullIfZero := func(v int64) *int64 {
		if v == 0 {
			return nil
		}
		return &v
	}
	// min_balance_mc is NOT NULL — an unset (0) value means "use the global
	// floor", persisted as the column default rather than NULL.
	minBalance := mp.MinBalanceMc
	if minBalance <= 0 {
		minBalance = 5000
	}
	_, err := r.pool.Exec(ctx, query,
		mp.ModelName,
		mp.InputRatePerMillion, nullIfZero(mp.CacheWriteRatePerMillion),
		nullIfZero(mp.CachedInputRatePerMillion), mp.OutputRatePerMillion,
		nullIfZero(mp.LongContextThreshold),
		nullIfZero(mp.LongContextInputRatePerMillion),
		nullIfZero(mp.LongContextCachedInputRatePerMillion),
		nullIfZero(mp.LongContextOutputRatePerMillion),
		nullIfZero(mp.ContextWindowTokens), minBalance,
		mp.IsActive, mp.IsUserFacing,
	)
	return err
}

func (r *PostgresBillingRepository) SetActive(ctx context.Context, modelName string, active bool) error {
	const query = `UPDATE model_prices SET is_active = $1 WHERE model_name = $2`
	_, err := r.pool.Exec(ctx, query, active, modelName)
	return err
}

// ── TransactionRepository ─────────────────────────────────────────────────────

func (r *PostgresBillingRepository) CreateTx(ctx context.Context, tx any, t *CreditTransaction) (uuid.UUID, error) {
	db := r.getDB(tx)
	const query = `
		INSERT INTO credit_transactions
			(id, user_id, amount_mc, description, model_name, input_tokens, output_tokens, call_type, pool, created_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
	`
	_, err := db.Exec(ctx, query,
		t.ID, t.UserID, t.AmountMc, t.Description,
		nullableStr(t.ModelName), t.InputTokens, t.OutputTokens,
		nullableStr(t.CallType), t.Pool, t.CreatedAt,
	)
	if err != nil {
		return uuid.Nil, err
	}
	return t.ID, nil
}

func (r *PostgresBillingRepository) ListByUserID(ctx context.Context, userID uuid.UUID, limit int) ([]*CreditTransaction, error) {
	const query = `
		SELECT id, user_id, amount_mc, description, created_at
		FROM credit_transactions
		WHERE user_id = $1
		ORDER BY created_at DESC LIMIT $2
	`
	rows, err := r.pool.Query(ctx, query, userID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var txs []*CreditTransaction
	for rows.Next() {
		var t CreditTransaction
		if err := rows.Scan(&t.ID, &t.UserID, &t.AmountMc, &t.Description, &t.CreatedAt); err != nil {
			return nil, err
		}
		txs = append(txs, &t)
	}
	return txs, nil
}

// ── ProfitRepository ──────────────────────────────────────────────────────────

func (r *PostgresBillingRepository) CreateProfitTx(ctx context.Context, tx any, p *PlatformProfit) error {
	db := r.getDB(tx)
	const query = `
		INSERT INTO platform_profits
			(id, user_id, transaction_id, model_name, call_type, pool,
			 input_tokens, output_tokens, api_cost_mc, user_charged_mc, profit_mc, markup, created_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW())
	`
	_, err := db.Exec(ctx, query,
		p.ID, p.UserID, p.TransactionID, p.ModelName, p.CallType, p.Pool,
		p.InputTokens, p.OutputTokens, p.ApiCostMc, p.UserChargedMc, p.ProfitMc, p.Markup,
	)
	return err
}

// ── SystemConfigRepository ────────────────────────────────────────────────────

func (r *PostgresBillingRepository) Get(ctx context.Context, key string) (string, error) {
	const query = `SELECT config_value FROM system_configs WHERE config_key = $1`
	var val string
	err := r.pool.QueryRow(ctx, query, key).Scan(&val)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return "", sql.ErrNoRows
		}
		return "", err
	}
	return val, nil
}

func (r *PostgresBillingRepository) Set(ctx context.Context, key string, value string) error {
	const query = `
		INSERT INTO system_configs (config_key, config_value, updated_at)
		VALUES ($1, $2, NOW())
		ON CONFLICT (config_key) DO UPDATE
		SET config_value = EXCLUDED.config_value, updated_at = NOW()
	`
	_, err := r.pool.Exec(ctx, query, key, value)
	return err
}

// ── TxManager ─────────────────────────────────────────────────────────────────

type PostgresTxManager struct {
	pool *pgxpool.Pool
}

func NewPostgresTxManager(pool *pgxpool.Pool) *PostgresTxManager {
	return &PostgresTxManager{pool: pool}
}

func (m *PostgresTxManager) WithTx(ctx context.Context, fn func(tx any) error) error {
	tx, err := m.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	if err := fn(tx); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

// ── helpers ───────────────────────────────────────────────────────────────────

func nullableStr(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// ── Admin & config-management repo methods ────────────────────────────────────

func (r *PostgresBillingRepository) IsAdmin(ctx context.Context, userID uuid.UUID) (bool, error) {
	var isAdmin bool
	err := r.pool.QueryRow(ctx, `SELECT is_admin FROM users WHERE id = $1`, userID).Scan(&isAdmin)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return false, nil
		}
		return false, err
	}
	return isAdmin, nil
}

func (r *PostgresBillingRepository) ListConfigs(ctx context.Context) ([]SystemConfigRow, error) {
	rows, err := r.pool.Query(ctx,
		`SELECT config_key, config_value, value_type, description FROM system_configs ORDER BY config_key`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []SystemConfigRow
	for rows.Next() {
		var c SystemConfigRow
		if err := rows.Scan(&c.Key, &c.Value, &c.ValueType, &c.Description); err != nil {
			return nil, err
		}
		out = append(out, c)
	}
	return out, rows.Err()
}

func (r *PostgresBillingRepository) GetConfigMeta(ctx context.Context, key string) (*SystemConfigRow, error) {
	var c SystemConfigRow
	err := r.pool.QueryRow(ctx,
		`SELECT config_key, config_value, value_type, description FROM system_configs WHERE config_key = $1`, key).
		Scan(&c.Key, &c.Value, &c.ValueType, &c.Description)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &c, nil
}

func (r *PostgresBillingRepository) RecordAudit(ctx context.Context, adminID uuid.UUID, entity, key string, oldVal, newVal []byte) error {
	_, err := r.pool.Exec(ctx,
		`INSERT INTO config_audit_log (admin_id, entity, entity_key, old_value, new_value)
		 VALUES ($1, $2, $3, $4, $5)`,
		adminID, entity, key, oldVal, newVal)
	return err
}

// ── interface compliance guards ───────────────────────────────────────────────

var _ BalanceRepository = (*PostgresBillingRepository)(nil)
var _ PriceRepository = (*PostgresBillingRepository)(nil)
var _ TransactionRepository = (*PostgresBillingRepository)(nil)
var _ ProfitRepository = (*PostgresBillingRepository)(nil)
var _ SystemConfigRepository = (*PostgresBillingRepository)(nil)
var _ TxManager = (*PostgresTxManager)(nil)

// SetAllowCreditUsage updates only the opt-in column, leaving every balance
// and counter untouched. Initializes the row (scoped to organizationID) if
// the user has never been charged, so the preference can be set before any
// spend has happened.
func (r *PostgresBillingRepository) SetAllowCreditUsage(ctx context.Context, userID, organizationID uuid.UUID, allow bool) error {
	// free_monthly_reset_at is set here as well as in InitializeTx so a user who
	// sets this preference before their first charge does not end up with a
	// half-populated balance row.
	_, err := r.pool.Exec(ctx, `
		INSERT INTO user_balances (user_id, organization_id, paid_balance_mc, paid_monthly_cap_mc,
		                           allow_credit_usage, credit_usage_set_by_user,
		                           free_monthly_reset_at, updated_at)
		VALUES ($1, $2, 0, 0, $3, TRUE, NOW() + INTERVAL '1 month', NOW())
		ON CONFLICT (user_id, organization_id) DO UPDATE
		SET allow_credit_usage = EXCLUDED.allow_credit_usage,
		    -- The user chose. Purchases must never overwrite this again.
		    credit_usage_set_by_user = TRUE,
		    updated_at = NOW()
	`, userID, organizationID, allow)
	return err
}
