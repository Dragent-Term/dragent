-- +goose Up
-- +goose StatementBegin

-- OpenAI GPT-5.6 price cut (announced 2026-07-30).
--
--   Luna : $1.00/$6.00  →  $0.20/$1.20   (input cut ~80%)
--   Terra: $2.50/$15.00 →  $2.00/$12.00  (input cut 20%)
--   Sol  : unchanged at $5.00/$30.00
--
-- Cache rates follow the same ratios the 5.6 family already used:
--   cache read  = 0.10× input  (the "90% cheaper" cached-input rate)
--   cache write = 1.25× input  (informational; CalculateCost does not bill it)
--
-- Rate unit: micro-cents per million tokens ($1.00/M = 1,000,000 mc).
--
-- ⚠️ SOURCING: openai.com returns 403 to automated fetches, so these figures come
-- from corroborating secondary sources (aggregator listing dated 2026-08-01, plus
-- CNBC/VentureBeat coverage of the 30 July cut) rather than the vendor page
-- directly. They agree with each other and with OpenRouter's Terra listing (which
-- shows $1/$6 under a "50% off" promotion, i.e. a $2/$12 base). Verify against
-- OpenAI's own pricing page before relying on this for margin calculations —
-- these rates bill real money.

UPDATE model_prices SET
    input_rate_per_million        =  200000,   -- $0.20/M
    cached_input_rate_per_million =   20000,   -- $0.02/M  (cache read)
    cache_write_rate_per_million  =  250000,   -- $0.25/M  (1.25× input)
    output_rate_per_million       = 1200000    -- $1.20/M
WHERE model_name = 'gpt-5.6-luna';

UPDATE model_prices SET
    input_rate_per_million        =  2000000,  -- $2.00/M
    cached_input_rate_per_million =   200000,  -- $0.20/M  (cache read)
    cache_write_rate_per_million  =  2500000,  -- $2.50/M  (1.25× input)
    output_rate_per_million       = 12000000   -- $12.00/M
WHERE model_name = 'gpt-5.6-terra';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Restore the pre-cut GPT-5.6 launch pricing (from 00019).
UPDATE model_prices SET
    input_rate_per_million        = 1000000,   -- $1.00/M
    cached_input_rate_per_million =  100000,   -- $0.10/M
    cache_write_rate_per_million  = 1250000,   -- $1.25/M
    output_rate_per_million       = 6000000    -- $6.00/M
WHERE model_name = 'gpt-5.6-luna';

UPDATE model_prices SET
    input_rate_per_million        =  2500000,  -- $2.50/M
    cached_input_rate_per_million =   250000,  -- $0.25/M
    cache_write_rate_per_million  =  3125000,  -- $3.125/M
    output_rate_per_million       = 15000000   -- $15.00/M
WHERE model_name = 'gpt-5.6-terra';

-- +goose StatementEnd
