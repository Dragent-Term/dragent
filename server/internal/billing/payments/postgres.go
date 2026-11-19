package payments

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/binary"
	"errors"
	"fmt"
	"log"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// ErrEventInFlight means another delivery of the same webhook is mid-apply.
// The caller should stop: that delivery will finish the work.
var ErrEventInFlight = errors.New("payments: webhook event already being processed")

// ErrPlanNotFound is returned when a plan id/variant has no active row.
var ErrPlanNotFound = errors.New("plan not found")

type PostgresRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresRepository(pool *pgxpool.Pool) *PostgresRepository {
	return &PostgresRepository{pool: pool}
}

// personalOrganizationID resolves a user's own personal organization — their
// oldest membership, the exact same rule migration 00053's backfill used.
//
// Used only by ActivateSubscription/EndSubscription now — the individual
// subscription checkout path was retired (every subscription is an org
// subscription now, personal orgs included; see
// organization.BillingService.Subscribe), but these two stay wired to
// keep processing renewal/cancellation webhooks for whoever subscribed
// through the old individual path before it was retired. AddDepositCredits
// no longer uses this: credit-pack purchases now target the buyer's active
// org (captured at checkout time), not unconditionally their personal one —
// a deliberate reversal of this file's previous "strict separation" stance,
// made once dragent's org model was confirmed to match the reference implementation
// (a personal account is just a 1-member org, not a distinct billing
// concept), not an oversight.
func (r *PostgresRepository) personalOrganizationID(ctx context.Context, userID uuid.UUID) (uuid.UUID, error) {
	var orgID uuid.UUID
	err := r.pool.QueryRow(ctx, `
		SELECT organization_id FROM memberships
		WHERE user_id = $1 ORDER BY created_at ASC LIMIT 1
	`, userID).Scan(&orgID)
	if err != nil {
		return uuid.Nil, fmt.Errorf("resolve personal organization: %w", err)
	}
	return orgID, nil
}

const planColumns = `id, tier, billing_interval, monthly_allowance_mc, price_cents,
	currency, display_name, lemonsqueezy_variant_id, is_active`

func scanPlan(row pgx.Row) (*Plan, error) {
	var p Plan
	err := row.Scan(&p.ID, &p.Tier, &p.BillingInterval, &p.MonthlyAllowanceMc, &p.PriceCents,
		&p.Currency, &p.DisplayName, &p.LemonSqueezyVariantID, &p.IsActive)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, ErrPlanNotFound
		}
		return nil, err
	}
	return &p, nil
}

func (r *PostgresRepository) GetPlanByID(ctx context.Context, id string) (*Plan, error) {
	return scanPlan(r.pool.QueryRow(ctx,
		`SELECT `+planColumns+` FROM plans WHERE id = $1 AND is_active = TRUE`, id))
}

func (r *PostgresRepository) GetPlanByVariant(ctx context.Context, variantID string) (*Plan, error) {
	if variantID == "" {
		return nil, ErrPlanNotFound
	}
	return scanPlan(r.pool.QueryRow(ctx,
		`SELECT `+planColumns+` FROM plans WHERE lemonsqueezy_variant_id = $1 AND is_active = TRUE`, variantID))
}

func (r *PostgresRepository) ListActivePlans(ctx context.Context) ([]*Plan, error) {
	rows, err := r.pool.Query(ctx,
		`SELECT `+planColumns+` FROM plans WHERE is_active = TRUE ORDER BY price_cents`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var plans []*Plan
	for rows.Next() {
		p, err := scanPlan(rows)
		if err != nil {
			return nil, err
		}
		plans = append(plans, p)
	}
	return plans, rows.Err()
}

// ── Credit packs ──────────────────────────────────────────────────────────────

const packColumns = `id, price_cents, credits_mc, currency, display_name,
	lemonsqueezy_variant_id, is_active`

func scanPack(row pgx.Row) (*CreditPack, error) {
	var p CreditPack
	err := row.Scan(&p.ID, &p.PriceCents, &p.CreditsMc, &p.Currency, &p.DisplayName,
		&p.LemonSqueezyVariantID, &p.IsActive)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, ErrPlanNotFound
		}
		return nil, err
	}
	return &p, nil
}

func (r *PostgresRepository) GetCreditPackByID(ctx context.Context, id string) (*CreditPack, error) {
	return scanPack(r.pool.QueryRow(ctx,
		`SELECT `+packColumns+` FROM credit_packs WHERE id = $1 AND is_active = TRUE`, id))
}

func (r *PostgresRepository) GetCreditPackByVariant(ctx context.Context, variantID string) (*CreditPack, error) {
	if variantID == "" {
		return nil, ErrPlanNotFound
	}
	return scanPack(r.pool.QueryRow(ctx,
		`SELECT `+packColumns+` FROM credit_packs WHERE lemonsqueezy_variant_id = $1 AND is_active = TRUE`, variantID))
}

func (r *PostgresRepository) ListActivePacks(ctx context.Context) ([]*CreditPack, error) {
	rows, err := r.pool.Query(ctx,
		`SELECT `+packColumns+` FROM credit_packs WHERE is_active = TRUE ORDER BY price_cents`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var packs []*CreditPack
	for rows.Next() {
		p, err := scanPack(rows)
		if err != nil {
			return nil, err
		}
		packs = append(packs, p)
	}
	return packs, rows.Err()
}

func (r *PostgresRepository) AddDepositCredits(ctx context.Context, userID, orgID uuid.UUID, creditsMc int64, description string) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	// Buying a top-up enables credit spending — the opt-in exists to stop
	// surprise charges, and a deliberate purchase is not a surprise; otherwise
	// the user pays for credits that sit inert behind a toggle they have no
	// reason to know about, still blocked at the limit that prompted the buy.
	//
	// But ONLY for a user who never expressed a preference. Someone who
	// switched credits off and is now banking a pack for later has made an
	// explicit choice, and a purchase must not silently reverse it.
	if _, err := tx.Exec(ctx, `
		INSERT INTO user_balances (user_id, organization_id, paid_balance_mc, paid_monthly_cap_mc,
		                           deposit_balance_mc, allow_credit_usage, free_monthly_reset_at, updated_at)
		VALUES ($1, $2, 0, 0, $3, TRUE, NOW() + INTERVAL '1 month', NOW())
		ON CONFLICT (user_id, organization_id) DO UPDATE SET
			deposit_balance_mc = user_balances.deposit_balance_mc + EXCLUDED.deposit_balance_mc,
			allow_credit_usage = CASE
				WHEN user_balances.credit_usage_set_by_user THEN user_balances.allow_credit_usage
				ELSE TRUE
			END,
			updated_at         = NOW()
	`, userID, orgID, creditsMc); err != nil {
		return fmt.Errorf("credit deposit balance: %w", err)
	}

	// Auditable positive transaction in the deposit pool.
	if _, err := tx.Exec(ctx, `
		INSERT INTO credit_transactions
			(id, user_id, amount_mc, description, input_tokens, output_tokens, call_type, pool, created_at)
		VALUES ($1, $2, $3, $4, 0, 0, 'deposit', 'deposit', NOW())
	`, uuid.New(), userID, creditsMc, description); err != nil {
		return fmt.Errorf("record deposit transaction: %w", err)
	}

	return tx.Commit(ctx)
}

// DeductDepositCredits reverses a refunded top-up: it subtracts the pack's
// credits from the deposit balance, floored at 0 (some may already be spent —
// we never drive the balance negative), and records a negative transaction.
// Scoped to the specific (userID, orgID) row the original purchase credited
// — a prior version of this query had no organization_id in its WHERE
// clause at all, so once user_balances became one row per org (11.F.1) it
// would have deducted from EVERY org a user has a balance row in, not just
// the one the refunded pack was credited to. Fixed here while adding the
// org parameter this consolidation needed anyway.
func (r *PostgresRepository) DeductDepositCredits(ctx context.Context, userID, orgID uuid.UUID, creditsMc int64, description string) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Exec(ctx, `
		UPDATE user_balances
		SET deposit_balance_mc = GREATEST(0, deposit_balance_mc - $3),
		    updated_at         = NOW()
		WHERE user_id = $1 AND organization_id = $2
	`, userID, orgID, creditsMc); err != nil {
		return fmt.Errorf("debit deposit balance: %w", err)
	}

	// Auditable negative transaction in the deposit pool.
	if _, err := tx.Exec(ctx, `
		INSERT INTO credit_transactions
			(id, user_id, amount_mc, description, input_tokens, output_tokens, call_type, pool, created_at)
		VALUES ($1, $2, $3, $4, 0, 0, 'refund', 'deposit', NOW())
	`, uuid.New(), userID, -creditsMc, description); err != nil {
		return fmt.Errorf("record refund transaction: %w", err)
	}

	return tx.Commit(ctx)
}

// ── Webhook idempotency ───────────────────────────────────────────────────────

func (r *PostgresRepository) ClaimEvent(ctx context.Context, id, provider, eventName, externalID string, userID *uuid.UUID, payload []byte) (bool, error) {
	// DO UPDATE (not DO NOTHING) so an unfinished claim is re-taken and
	// RETURNING yields a row. The WHERE makes a completed event terminal:
	// the update matches nothing, no row returns, and the caller skips.
	// DO UPDATE also takes a row lock, serialising concurrent deliveries of
	// the same event.
	var claimed string
	err := r.pool.QueryRow(ctx,
		`INSERT INTO payment_events (id, provider, event_name, user_id, external_id, payload)
		 VALUES ($1, $2, $3, $4, $5, $6)
		 ON CONFLICT (id) DO UPDATE
		     SET received_at = NOW()
		     WHERE payment_events.processed_at IS NULL
		 RETURNING id`,
		id, provider, eventName, userID, externalID, payload).Scan(&claimed)
	if errors.Is(err, pgx.ErrNoRows) {
		return false, nil // already applied — genuine duplicate delivery
	}
	if err != nil {
		return false, err
	}
	return true, nil
}

func (r *PostgresRepository) MarkEventProcessed(ctx context.Context, id string) error {
	_, err := r.pool.Exec(ctx,
		`UPDATE payment_events SET processed_at = NOW() WHERE id = $1`, id)
	return err
}

func (r *PostgresRepository) ActivateSubscription(ctx context.Context, userID uuid.UUID, plan *Plan, subscriptionID, customerID, status string, renewsAt *time.Time) error {
	// The provider's own renews_at is authoritative. The fallback only applies
	// when a webhook omits it, and uses a calendar month to match how the
	// subscription actually bills.
	reset := addOneMonth(time.Now().UTC())
	if renewsAt != nil {
		reset = renewsAt.UTC()
	}

	orgID, err := r.personalOrganizationID(ctx, userID)
	if err != nil {
		return err
	}

	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	// Grant the monthly entitlement. Plans are sold as a monthly budget that is
	// usable at any pace during the billing cycle.
	//
	// CRITICAL: money and usage counters move ONLY when this event represents a
	// genuinely new entitlement. `subscription_updated` is not renewal-specific
	// — LemonSqueezy fires it for card updates, billing-address edits and
	// dunning transitions. Re-granting on every one of those let a user refill
	// a spent session cap and a spent monthly allowance simply by editing their
	// payment details, which defeats the session cap entirely. A failed payment
	// (status past_due) did the same.
	//
	// A new entitlement means one of:
	//   * first activation      — the row is not currently paid
	//   * a plan change         — the tier or allowance differs (upgrade/downgrade)
	//   * a new billing period  — renews_at has moved FORWARD past the stored reset
	//
	// Everything else refreshes status/tier/caps only and leaves the wallet and
	// the rolling windows untouched.
	if _, err := tx.Exec(ctx, `
		INSERT INTO user_balances (
			user_id, organization_id, tier, is_paid, paid_status,
			paid_balance_mc, paid_monthly_cap_mc, paid_monthly_reset_at,
			free_monthly_reset_at, updated_at
		) VALUES ($1, $6, $2, TRUE, $5, $3, $3, $4, NOW() + INTERVAL '1 month', NOW())
		ON CONFLICT (user_id, organization_id) DO UPDATE SET
			tier                  = EXCLUDED.tier,
			is_paid               = TRUE,
			-- Recorded even on a non-granting update: a past_due arriving here
			-- is exactly what must stop the clock from renewing.
			paid_status           = EXCLUDED.paid_status,
			paid_monthly_cap_mc   = EXCLUDED.paid_monthly_cap_mc,
			-- Money moves ONLY on a genuinely new entitlement (first activation,
			-- plan change, or renews_at moved forward). Everything else — card
			-- updates, dunning — refreshes status/tier only, so a subscriber
			-- cannot refill a spent monthly allowance by editing billing details.
			paid_balance_mc = CASE WHEN (
				    NOT user_balances.is_paid
				 OR user_balances.tier <> EXCLUDED.tier
				 OR user_balances.paid_monthly_cap_mc <> EXCLUDED.paid_monthly_cap_mc
				 OR user_balances.paid_monthly_reset_at IS NULL
				 OR EXCLUDED.paid_monthly_reset_at > user_balances.paid_monthly_reset_at
				) THEN EXCLUDED.paid_balance_mc
				  ELSE user_balances.paid_balance_mc END,
			paid_monthly_reset_at = CASE WHEN (
				    NOT user_balances.is_paid
				 OR user_balances.paid_monthly_reset_at IS NULL
				 OR EXCLUDED.paid_monthly_reset_at > user_balances.paid_monthly_reset_at
				) THEN EXCLUDED.paid_monthly_reset_at
				  ELSE user_balances.paid_monthly_reset_at END,
			updated_at = NOW()
	`, userID, plan.Tier, plan.MonthlyAllowanceMc, reset, status, orgID); err != nil {
		return fmt.Errorf("grant balance: %w", err)
	}

	if _, err := tx.Exec(ctx, `
		UPDATE users SET
			lemonsqueezy_customer_id     = COALESCE(NULLIF($2, ''), lemonsqueezy_customer_id),
			lemonsqueezy_subscription_id = $3,
			subscription_status          = $4,
			subscription_plan_id         = $5,
			subscription_renews_at       = $6,
			updated_at                   = NOW()
		WHERE id = $1
	`, userID, customerID, subscriptionID, status, plan.ID, renewsAt); err != nil {
		return fmt.Errorf("update user subscription: %w", err)
	}

	return tx.Commit(ctx)
}

// EndSubscription downgrades ONLY the caller's personal-org balance row —
// scoped by organization_id, not user_id alone. Without that scope this
// would now (user_balances is keyed (user_id, organization_id), one row per
// org a user holds a seat in) wipe every org-seat balance the user has
// too the moment their own unrelated personal subscription ends, which is
// exactly the cross-source clobbering per-organization billing rules out.
func (r *PostgresRepository) EndSubscription(ctx context.Context, userID uuid.UUID, status string) error {
	orgID, err := r.personalOrganizationID(ctx, userID)
	if err != nil {
		return err
	}

	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Exec(ctx, `
		UPDATE user_balances SET
			tier                  = 'free',
			is_paid               = FALSE,
			paid_status           = '',
			paid_balance_mc       = 0,
			paid_monthly_cap_mc   = 0,
			paid_monthly_reset_at = NULL,
			updated_at            = NOW()
		WHERE user_id = $1 AND organization_id = $2
	`, userID, orgID); err != nil {
		return fmt.Errorf("downgrade balance: %w", err)
	}

	if _, err := tx.Exec(ctx, `
		UPDATE users SET
			subscription_status    = $2,
			subscription_plan_id   = '',
			subscription_renews_at = NULL,
			updated_at             = NOW()
		WHERE id = $1
	`, userID, status); err != nil {
		return fmt.Errorf("update user subscription: %w", err)
	}

	return tx.Commit(ctx)
}

// GrantOrgSeat mirrors ActivateSubscription's grant-only-on-genuine-new-
// entitlement CASE WHEN, keyed the same way — ON CONFLICT (user_id,
// organization_id) — but targeting the (userID, orgID) row specifically.
// A member joining a different org is simply a different row (INSERT, not
// an update that has to detect a source change): every org's grant is
// already isolated by construction, nothing extra to guard here.
func (r *PostgresRepository) GrantOrgSeat(ctx context.Context, userID, orgID uuid.UUID, tier string, monthlyAllowanceMc int64, status string, renewsAt *time.Time) error {
	reset := addOneMonth(time.Now().UTC())
	if renewsAt != nil {
		reset = renewsAt.UTC()
	}
	_, err := r.pool.Exec(ctx, `
		INSERT INTO user_balances (
			user_id, organization_id, tier, is_paid, paid_status,
			paid_balance_mc, paid_monthly_cap_mc, paid_monthly_reset_at,
			free_monthly_reset_at, updated_at
		) VALUES ($1, $6, $2, TRUE, $5, $3, $3, $4, NOW() + INTERVAL '1 month', NOW())
		ON CONFLICT (user_id, organization_id) DO UPDATE SET
			tier                  = EXCLUDED.tier,
			is_paid               = TRUE,
			paid_status           = EXCLUDED.paid_status,
			paid_monthly_cap_mc   = EXCLUDED.paid_monthly_cap_mc,
			paid_balance_mc = CASE WHEN (
				    NOT user_balances.is_paid
				 OR user_balances.tier <> EXCLUDED.tier
				 OR user_balances.paid_monthly_cap_mc <> EXCLUDED.paid_monthly_cap_mc
				 OR user_balances.paid_monthly_reset_at IS NULL
				 OR EXCLUDED.paid_monthly_reset_at > user_balances.paid_monthly_reset_at
				) THEN EXCLUDED.paid_balance_mc
				  ELSE user_balances.paid_balance_mc END,
			paid_monthly_reset_at = CASE WHEN (
				    NOT user_balances.is_paid
				 OR user_balances.paid_monthly_reset_at IS NULL
				 OR EXCLUDED.paid_monthly_reset_at > user_balances.paid_monthly_reset_at
				) THEN EXCLUDED.paid_monthly_reset_at
				  ELSE user_balances.paid_monthly_reset_at END,
			updated_at = NOW()
	`, userID, tier, monthlyAllowanceMc, reset, status, orgID)
	if err != nil {
		return fmt.Errorf("grant org seat balance: %w", err)
	}
	return nil
}

// RevokeOrgSeat downgrades exactly the (user, orgID) row — organization_id
// is the row's own real key now (0 rows affected, not an error, if no such
// row exists), so there's no separate source-tracking field left to guard
// against a stale revoke touching the wrong org's row the way there was
// under the old one-row-per-user model.
//
// This is for the org's OWN subscription ending while membership continues
// (see BillingService.ApplyOrgEvent) — every current member keeps their row,
// just downgraded to free. A departing member is a different case, see
// DeleteOrgSeat.
func (r *PostgresRepository) RevokeOrgSeat(ctx context.Context, userID, orgID uuid.UUID) error {
	_, err := r.pool.Exec(ctx, `
		UPDATE user_balances SET
			tier                  = 'free',
			is_paid               = FALSE,
			paid_status           = '',
			paid_balance_mc       = 0,
			paid_monthly_cap_mc   = 0,
			paid_monthly_reset_at = NULL,
			updated_at            = NOW()
		WHERE user_id = $1 AND organization_id = $2
	`, userID, orgID)
	if err != nil {
		return fmt.Errorf("revoke org seat balance: %w", err)
	}
	return nil
}

// DeleteOrgSeat removes the (user, orgID) balance row entirely. Called when a
// member leaves or is removed from the org (isolation plan §4.1): unlike
// RevokeOrgSeat, a departing member has no remaining relationship to this
// org at all, so no balance state should exist for it — not even a
// downgraded free one. Unconditional on paid status: a Free-tier org's
// member can still have accrued a lazily-created row (free-quota tracking),
// and that row is just as much entitlement residue as a paid one once the
// membership itself ends.
func (r *PostgresRepository) DeleteOrgSeat(ctx context.Context, userID, orgID uuid.UUID) error {
	_, err := r.pool.Exec(ctx, `
		DELETE FROM user_balances WHERE user_id = $1 AND organization_id = $2
	`, userID, orgID)
	if err != nil {
		return fmt.Errorf("delete org seat balance: %w", err)
	}
	return nil
}

func (r *PostgresRepository) SubscriptionID(ctx context.Context, userID uuid.UUID) (string, error) {
	var id string
	err := r.pool.QueryRow(ctx,
		`SELECT lemonsqueezy_subscription_id FROM users WHERE id = $1`, userID).Scan(&id)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return "", nil
		}
		return "", err
	}
	return id, nil
}

func (r *PostgresRepository) RecordReceipt(ctx context.Context, rec Receipt) error {
	_, err := r.pool.Exec(ctx, `
		INSERT INTO receipts (id, user_id, organization_id, amount_cents, currency, provider, provider_reference, description, kind, created_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
	`, uuid.New(), rec.UserID, rec.OrganizationID, rec.AmountCents, rec.Currency, rec.Provider, rec.ProviderReference, rec.Description, string(rec.Kind))
	if err != nil {
		return fmt.Errorf("record receipt: %w", err)
	}
	return nil
}

func (r *PostgresRepository) UserIDBySubscriptionID(ctx context.Context, subscriptionID string) (uuid.UUID, error) {
	if subscriptionID == "" {
		// lemonsqueezy_subscription_id is NOT NULL DEFAULT '' — every user
		// without a subscription has this exact value, so an empty input
		// must never reach the query below or it would match one of them.
		return uuid.Nil, sql.ErrNoRows
	}
	var id uuid.UUID
	err := r.pool.QueryRow(ctx,
		`SELECT id FROM users WHERE lemonsqueezy_subscription_id = $1`, subscriptionID).Scan(&id)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return uuid.Nil, sql.ErrNoRows
		}
		return uuid.Nil, err
	}
	return id, nil
}

var _ Repository = (*PostgresRepository)(nil)

// WithEventLock serialises concurrent deliveries of one webhook event.
//
// Postgres advisory locks are SESSION scoped, so this pins a single pooled
// connection for the duration — that is the point: the lock must outlive the
// claim statement and cover the apply, which runs in its own transaction.
// `pg_try_advisory_lock` rather than the blocking form so a duplicate delivery
// returns immediately instead of holding a connection behind a slow apply.
func (r *PostgresRepository) WithEventLock(ctx context.Context, id string, fn func() error) error {
	conn, err := r.pool.Acquire(ctx)
	if err != nil {
		return fmt.Errorf("acquire conn for event lock: %w", err)
	}
	defer conn.Release()

	key := advisoryKey(id)
	var got bool
	if err := conn.QueryRow(ctx, `SELECT pg_try_advisory_lock($1)`, key).Scan(&got); err != nil {
		return fmt.Errorf("take event lock: %w", err)
	}
	if !got {
		return ErrEventInFlight
	}
	defer func() {
		// Best-effort release on a context that cannot already be cancelled,
		// or a lock leaks for the life of the pooled connection.
		rctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 5*time.Second)
		defer cancel()
		if _, uerr := conn.Exec(rctx, `SELECT pg_advisory_unlock($1)`, key); uerr != nil {
			log.Printf("[payments] WARNING could not release advisory lock for event %s: %v", id, uerr)
		}
	}()

	return fn()
}

// advisoryKey folds an event id into the int64 pg_advisory_lock takes.
// Collisions are harmless: two unrelated events sharing a key merely serialise
// against each other, which costs a little concurrency and no correctness.
func advisoryKey(id string) int64 {
	h := sha256.Sum256([]byte(id))
	return int64(binary.BigEndian.Uint64(h[:8]))
}

// addOneMonth advances by one CALENDAR month, clamping to the last day when the
// target month is shorter (Jan 31 → Feb 28/29). Mirrors billing/model's
// AddOneMonth; duplicated rather than imported to keep payments free of a
// dependency on the billing model package.
//
// Subscriptions bill on the same calendar day each month, so a flat 30 days
// drifts against the provider's schedule.
func addOneMonth(t time.Time) time.Time {
	y, m, d := t.Date()
	firstOfNext := time.Date(y, m, 1, t.Hour(), t.Minute(), t.Second(), t.Nanosecond(), t.Location()).
		AddDate(0, 1, 0)
	last := time.Date(firstOfNext.Year(), firstOfNext.Month()+1, 0, 0, 0, 0, 0, time.UTC).Day()
	if d > last {
		d = last
	}
	return time.Date(firstOfNext.Year(), firstOfNext.Month(), d,
		t.Hour(), t.Minute(), t.Second(), t.Nanosecond(), t.Location())
}
