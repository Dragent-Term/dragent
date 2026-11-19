-- +goose Up
-- +goose StatementBegin

-- The free-tier 5-hour session cap was weekly/6 (62,500 mc = $0.0625), which a
-- SINGLE mid-size LLM call exceeds — one API call consumed the whole session.
-- Raise the FREE-TIER session cap to HALF the weekly cap so a session allows
-- real work while still rate-limiting burst spend. Paid tiers (pro/ultra/mega)
-- are intentionally untouched: their paid_session_cap stays weekly/6, and
-- their free-pool fallback caps stay as-is.

-- Existing free-tier users: derive from their actual weekly cap.
UPDATE user_balances
    SET free_session_cap_mc = free_weekly_cap_mc / 2
    WHERE tier = 'free';

-- New users start on the free tier: default matches (375000 / 2).
ALTER TABLE user_balances
    ALTER COLUMN free_session_cap_mc SET DEFAULT 187500;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

UPDATE user_balances
    SET free_session_cap_mc = free_weekly_cap_mc / 6
    WHERE tier = 'free';

ALTER TABLE user_balances
    ALTER COLUMN free_session_cap_mc SET DEFAULT 62500;

-- +goose StatementEnd
