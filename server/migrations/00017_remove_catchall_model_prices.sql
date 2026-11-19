-- +goose Up
-- +goose StatementBegin

-- Remove generic catch-all price rows ("claude-", "gemini-", "gpt-",
-- "llama", "mixtral", ...). Combined with GetModelPrice's longest-prefix
-- matching they silently priced ANY model in a family — so an unknown or
-- typo'd model name billed at some arbitrary family rate instead of being
-- refused. The catalog now contains only real, known models; unknown models
-- are rejected at pre-flight (ErrUnknownModel). BYOK custom models never
-- consult this table and are unaffected.
--
-- Dated-variant matching still works: "claude-sonnet-4-6-20260101" resolves
-- to the concrete "claude-sonnet-4-6" row via longest-prefix.

DELETE FROM model_prices
WHERE model_name LIKE '%-'                              -- trailing-dash catch-alls
   OR model_name IN ('llama', 'mixtral', 'qwen', 'gemma'); -- bare family aliases

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Irreversible data cleanup: the catch-all rows are deliberately not
-- restored. Re-seed specific models via the admin model-price endpoint or a
-- new migration if ever needed.
SELECT 1;

-- +goose StatementEnd
