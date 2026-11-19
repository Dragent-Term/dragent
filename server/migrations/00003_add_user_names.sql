-- +goose Up
-- +goose StatementBegin
ALTER TABLE users ADD COLUMN IF NOT EXISTS first_name VARCHAR(255) NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN IF NOT EXISTS last_name VARCHAR(255) NOT NULL DEFAULT '';
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE users DROP COLUMN IF EXISTS first_name;
ALTER TABLE users DROP COLUMN IF EXISTS last_name;
-- +goose StatementEnd
