-- +goose Up
-- +goose StatementBegin

-- Correct context_window_tokens to the values published in each provider's
-- official docs (verified July 2026). This field drives the client's
-- context-budget meter; it does NOT affect billing (pricing tiers are governed
-- by long_context_threshold, unset on these rows).
--
--   Anthropic (platform.claude.com/docs/.../models/overview):
--     claude-haiku-4-5   200K   (was unset)
--     claude-opus-4-8    1M     (was 200K — understated)
--     claude-sonnet-4-6  1M     (was 200K — understated)
--   OpenAI (developers.openai.com/api/docs/models):
--     gpt-5.6 sol/terra/luna  "1.05M" → 1,048,576 (was 400K — understated)
--
-- Already correct and unchanged: grok-4.5 (500K), grok-4.3 (1M),
-- grok-build-0.1 (256K), claude-fable-5 (1M), claude-sonnet-5 (1M),
-- gemini-2.5-flash (1M).
UPDATE model_prices SET context_window_tokens =  200000 WHERE model_name = 'claude-haiku-4-5';
UPDATE model_prices SET context_window_tokens = 1000000 WHERE model_name = 'claude-opus-4-8';
UPDATE model_prices SET context_window_tokens = 1000000 WHERE model_name = 'claude-sonnet-4-6';
UPDATE model_prices SET context_window_tokens = 1048576 WHERE model_name = 'gpt-5.6-sol';
UPDATE model_prices SET context_window_tokens = 1048576 WHERE model_name = 'gpt-5.6-terra';
UPDATE model_prices SET context_window_tokens = 1048576 WHERE model_name = 'gpt-5.6-luna';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
UPDATE model_prices SET context_window_tokens =    NULL WHERE model_name = 'claude-haiku-4-5';
UPDATE model_prices SET context_window_tokens =  200000 WHERE model_name = 'claude-opus-4-8';
UPDATE model_prices SET context_window_tokens =  200000 WHERE model_name = 'claude-sonnet-4-6';
UPDATE model_prices SET context_window_tokens =  400000 WHERE model_name = 'gpt-5.6-sol';
UPDATE model_prices SET context_window_tokens =  400000 WHERE model_name = 'gpt-5.6-terra';
UPDATE model_prices SET context_window_tokens =  400000 WHERE model_name = 'gpt-5.6-luna';
-- +goose StatementEnd
