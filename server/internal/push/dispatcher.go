package push

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
)

// AttentionKind is the set of server events that used to wake a phone
// before E2EE. The values are kept because relay's event mirror still
// names them; the server no longer generates pushes from them. Pushes come
// only from desktop-sealed host-notify blobs (§8).
type AttentionKind string

const (
	// AttentionJoinApproved fires when the host approves a pending join.
	AttentionJoinApproved AttentionKind = "join_approved"
	// AttentionControlGranted fires when a viewer is granted control.
	AttentionControlGranted AttentionKind = "control_granted"
	// AttentionAgentCompleted fires when an agent run finishes.
	AttentionAgentCompleted AttentionKind = "agent_completed"
)

// NotifyDispatcher delivers desktop-sealed host-notify blobs to the phone's
// live registration (§8). The relay validates the envelope opaquely and
// passes it here; this package resolves the phone's registration and sends
// one fixed-size push. It never inspects, logs, stores or forwards content:
// the blob, the sending desktop and the notification kind stay inside the
// seal, readable only by the phone.
type NotifyDispatcher struct {
	Store  Store
	Sender Sender
	Now    func() time.Time
}

func (d *NotifyDispatcher) now() time.Time {
	if d.Now != nil {
		return d.Now()
	}
	return time.Now()
}

// HandleNotify sends one sealed host-notify blob to the phone's live
// registration. Its signature matches relay.NotifyIntakeFunc, so the
// integrator wires it with
// rpcHandler.SetE2ENotifyIntake(notify.HandleNotify). accountID routes to
// the phone and is never stored, logged or forwarded.
//
// It returns an error (wrapping ErrNotFound when there is simply nothing
// live to send to) unless the blob was handed to the sender. The relay
// already counted the intake in its aggregates; errors here only decide
// whether the desktop socket stays up, which it always does.
func (d *NotifyDispatcher) HandleNotify(ctx context.Context, accountID, phoneID, blob string) error {
	if !ValidSealedPushBlob(blob) {
		return errors.New("push: invalid sealed blob")
	}
	phone, err := uuid.Parse(phoneID)
	if err != nil || phone == uuid.Nil {
		return errors.New("push: invalid phone id")
	}
	account, err := uuid.Parse(accountID)
	if err != nil || account == uuid.Nil {
		return errors.New("push: invalid account id")
	}
	if d.Store == nil || d.Sender == nil {
		return errors.New("push: notify dispatcher is not wired")
	}
	reg, found, err := d.Store.Get(ctx, phone)
	if err != nil {
		return err
	}
	if !found {
		return fmt.Errorf("push: no registration for this phone: %w", ErrNotFound)
	}
	if reg.UserID != account {
		return errors.New("push: phone does not belong to this account")
	}
	if reg.Expired(d.now()) {
		return fmt.Errorf("push: registration has lapsed: %w", ErrNotFound)
	}
	return d.Sender.Send(ctx, PushMessage{
		DeviceID: reg.DeviceID,
		UserID:   reg.UserID,
		Platform: reg.Platform,
		Token:    reg.PushToken,
		Blob:     blob,
	})
}
