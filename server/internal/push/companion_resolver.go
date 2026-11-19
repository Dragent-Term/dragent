package push

import (
	"context"

	"github.com/google/uuid"

	"server/internal/companionpairing"
)

// companionDeviceStore is the narrow slice of the companion pairing store
// push registration checks. *companionpairing.MemoryStore and
// *companionpairing.RedisStore both satisfy it; tests stub it.
type companionDeviceStore interface {
	// IsDeviceValid reports whether the device is known, owned by userID,
	// unrevoked and unexpired.
	IsDeviceValid(ctx context.Context, userID, deviceID string) (bool, error)
	// DeviceE2E reports the cut-over flag for admission: known reports
	// whether the device exists for this user; e2e reports whether it was
	// redeemed after the cut-over.
	DeviceE2E(ctx context.Context, userID, deviceID string) (known bool, e2e bool, err error)
}

// CompanionDeviceResolver answers whether a companion phone may register
// for push: the device must exist for that user in the companion pairing
// store, be an E2E device, and be neither revoked nor expired (FIX1, §8).
// Companion phones live only in the pairing store (Redis in production),
// never in the Postgres devices table, so the Postgres resolver cannot see
// them. Unknown, foreign, revoked, expired and pre-cut-over (e2e:false)
// devices all report false with a nil error, so the handler answers them
// identically (404, no oracle). Storage errors propagate for a 500.
type CompanionDeviceResolver struct {
	Store companionDeviceStore
}

// NewCompanionDeviceResolver wires the resolver to the companion pairing
// store (Memory in tests, Redis in production).
func NewCompanionDeviceResolver(store companionpairing.Store) *CompanionDeviceResolver {
	return &CompanionDeviceResolver{Store: store}
}

func (r *CompanionDeviceResolver) Exists(ctx context.Context, userID, deviceID uuid.UUID) (bool, error) {
	if r == nil || r.Store == nil {
		return false, nil
	}
	user, device := userID.String(), deviceID.String()
	valid, err := r.Store.IsDeviceValid(ctx, user, device)
	if err != nil || !valid {
		return false, err
	}
	known, e2e, err := r.Store.DeviceE2E(ctx, user, device)
	if err != nil || !known || !e2e {
		return false, err
	}
	return true, nil
}

var _ DeviceResolver = (*CompanionDeviceResolver)(nil)
