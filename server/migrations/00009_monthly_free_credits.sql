-- +goose Up
-- Track when free-tier monthly credits were last granted.
-- NULL on existing rows means "never explicitly tracked" — the next task creation
-- will back-fill the column and grant fresh credits for the current month.
ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS free_credits_reset_at TIMESTAMPTZ;

-- +goose Down
ALTER TABLE user_balances
    DROP COLUMN IF EXISTS free_credits_reset_at;
