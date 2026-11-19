-- +goose Up
-- +goose StatementBegin

-- Claude Sonnet 5 — transition from introductory to standard pricing.
--
-- Anthropic introductory rate (through 2026-08-31): $2.00/$10.00 input/output.
-- Standard rate (from 2026-09-01):                  $3.00/$15.00 input/output.
--
-- Confirmed from https://platform.claude.com/docs/en/about-claude/pricing (July 2026).
--
-- Cache rates (standard):
--   5-minute cache write : $3.75/M  (1.25× input)
--   1-hour  cache write  : $6.00/M  (2×    input)
--   Cache read / refresh : $0.30/M  (0.10× input)
--
-- Rate unit: micro-cents per million tokens ($1.00/M = 1,000,000 mc).
--
-- NOTE: Do NOT run this migration before 2026-09-01. It applies the standard
-- rates unconditionally. Hold it back in your deploy pipeline until that date,
-- then run `goose up` (or let CI run it) to flip the pricing.

UPDATE model_prices SET
    input_rate_per_million        = 3000000,   -- $3.00/M
    cached_input_rate_per_million =  300000,   -- $0.30/M  (cache read / refresh)
    cache_write_rate_per_million  = 3750000,   -- $3.75/M  (5-min cache write)
    output_rate_per_million       = 15000000   -- $15.00/M
WHERE model_name = 'claude-sonnet-5';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Revert to introductory pricing.
UPDATE model_prices SET
    input_rate_per_million        = 2000000,   -- $2.00/M
    cached_input_rate_per_million =  200000,   -- $0.20/M
    cache_write_rate_per_million  = 2500000,   -- $2.50/M
    output_rate_per_million       = 10000000   -- $10.00/M
WHERE model_name = 'claude-sonnet-5';

-- +goose StatementEnd
