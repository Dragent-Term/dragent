-- +goose Up
CREATE TABLE billing_operations (
    id UUID PRIMARY KEY,
    operation_key TEXT NOT NULL UNIQUE,
    task_id UUID,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    operation_type TEXT NOT NULL,
    model_name TEXT NOT NULL,
    round_number INTEGER NOT NULL DEFAULT 0,
    pool TEXT NOT NULL CHECK (pool IN ('paid', 'free', 'deposit')),
    minimum_hold_mc BIGINT NOT NULL CHECK (minimum_hold_mc > 0),
    status TEXT NOT NULL CHECK (status IN (
        'authorized', 'usage_recorded', 'settled', 'voided', 'settlement_failed'
    )),
    input_tokens BIGINT NOT NULL DEFAULT 0,
    cached_input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    flat_fee_mc BIGINT NOT NULL DEFAULT 0,
    call_type TEXT NOT NULL DEFAULT 'agent',
    api_cost_mc BIGINT NOT NULL DEFAULT 0,
    user_charged_mc BIGINT NOT NULL DEFAULT 0,
    credit_transaction_id UUID REFERENCES credit_transactions(id),
    last_error TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    settled_at TIMESTAMPTZ
);

CREATE INDEX billing_operations_active_holds_idx
    ON billing_operations (user_id, pool)
    WHERE status IN ('authorized', 'usage_recorded', 'settlement_failed');
CREATE INDEX billing_operations_unsettled_idx
    ON billing_operations (updated_at)
    WHERE status IN ('usage_recorded', 'settlement_failed');

-- +goose Down
DROP TABLE billing_operations;
