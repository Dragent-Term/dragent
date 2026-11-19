-- +goose Up
-- +goose StatementBegin
ALTER TABLE users ADD COLUMN IF NOT EXISTS avatar_url TEXT NOT NULL DEFAULT '';
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE users DROP COLUMN IF EXISTS avatar_url;
-- +goose StatementEnd
