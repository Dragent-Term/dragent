// Package config exposes admin-only endpoints to manage model prices and
// system constants at runtime, with strict typed validation and an audit log.
package config

import (
	"context"

	"github.com/google/uuid"
	billingModel "server/internal/billing/model"
)

// Repo is the persistence surface the config handler needs. Implemented by the
// Postgres billing repository (which owns model_prices and system_configs).
type Repo interface {
	IsAdmin(ctx context.Context, userID uuid.UUID) (bool, error)

	// Model pricing (upsert + read).
	UpdateModelPrice(ctx context.Context, mp *billingModel.ModelPrice) error
	GetModelPrice(ctx context.Context, modelName string) (*billingModel.ModelPrice, error)
	SetActive(ctx context.Context, modelName string, active bool) error

	// System constants (typed key/value).
	Get(ctx context.Context, key string) (string, error)
	Set(ctx context.Context, key, value string) error
	ListConfigs(ctx context.Context) ([]billingModel.SystemConfigRow, error)
	GetConfigMeta(ctx context.Context, key string) (*billingModel.SystemConfigRow, error)

	// Audit.
	RecordAudit(ctx context.Context, adminID uuid.UUID, entity, key string, oldVal, newVal []byte) error
}
