package relay

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"time"

	"github.com/coder/websocket"
	"server/internal/companionsession"
)

const (
	companionSessionEndedCode = websocket.StatusCode(4408)
	companionStoreTimeout     = 2 * time.Second
	// How long a socket stays up while its session or device can't be
	// confirmed because storage is slow or down. A blip used to end the
	// phone's connection on the next check, so phones kept reconnecting. A
	// revocation can't be recorded while storage is down either, so this
	// doesn't delay one.
	companionStorageGrace = 60 * time.Second
)

// SetCompanionSessions is called before serving. Tests use the bounded memory
// store; production supplies Redis. Nil cannot disable the fixed lease boundary.
// A store that also delivers cross-instance revocations (the Redis store)
// becomes this instance's revocation source: one subscription per process
// replaces per-socket polling (see companion_devices.go).
func (h *RPCHandler) SetCompanionSessions(store companionsession.Store) {
	if store == nil {
		store = companionsession.NewMemoryStore()
	}
	h.sessions = store
	if src, ok := store.(companionsession.RevocationSource); ok {
		h.SetRevocationSource(src)
	}
}

// Missing optional fields retain legacy discovery admission. Present null,
// non-string, or empty resume ids must never fall through to creating a lease.
func companionAuthField(raw json.RawMessage, allowEmpty bool) (string, error) {
	if len(raw) == 0 {
		return "", nil
	}
	var value *string
	if json.Unmarshal(raw, &value) != nil || value == nil || (!allowEmpty && *value == "") || len(*value) > 128 {
		return "", companionsession.ErrUnavailable
	}
	return *value, nil
}

func (h *RPCHandler) admitCompanionSession(ctx context.Context, identity SocketIdentity, deviceID, resumeID string) (companionsession.Lease, error) {
	authCtx, stopAuth := context.WithDeadline(ctx, identity.ExpiresAt)
	defer stopAuth()
	storeCtx, stop := context.WithTimeout(authCtx, companionStoreTimeout)
	defer stop()
	if resumeID != "" {
		return h.sessions.Resume(storeCtx, resumeID, identity.UserID, deviceID)
	}
	return h.sessions.Create(storeCtx, identity.UserID, deviceID)
}

func companionFailureCode(err error) websocket.StatusCode {
	if errors.Is(err, companionsession.ErrUnavailable) || errors.Is(err, companionsession.ErrInvalid) || errors.Is(err, companionsession.ErrLimit) {
		return companionSessionEndedCode
	}
	return websocket.StatusInternalError
}

// companionAdmissionReason: why a phone's session couldn't be admitted, for
// the close log (fixed strings only).
func companionAdmissionReason(err error) string {
	switch {
	case errors.Is(err, companionsession.ErrLimit):
		return "companion session limit"
	case errors.Is(err, companionsession.ErrUnavailable):
		return "companion session ended"
	case errors.Is(err, companionsession.ErrInvalid):
		return "companion session invalid"
	default:
		return "companion session storage unavailable"
	}
}

// companionSessionGone: the session really ended (revoked, expired, invalid
// or over the limit), as opposed to storage not answering.
func companionSessionGone(err error) bool {
	return companionFailureCode(err) == companionSessionEndedCode
}

func writeCompanionAdmissionError(ctx context.Context, conn *websocket.Conn, write func(context.Context, any) error, err error) {
	message := "The companion service is temporarily unavailable."
	if companionFailureCode(err) == companionSessionEndedCode {
		message = "This companion session has ended. Reconnect to start a new one."
	}
	if errors.Is(err, companionsession.ErrLimit) {
		message = "Too many companion sessions are active."
	}
	_ = write(ctx, map[string]any{"type": "relay-error", "v": 1, "message": message, "loginRequired": false, "sessionEnded": companionFailureCode(err) == companionSessionEndedCode})
	closePhoneSocket(conn, companionFailureCode(err), companionAdmissionReason(err))
}

func writeRawRelayReply(ctx context.Context, conn *websocket.Conn, response *replyBuffer) error {
	deadline, stop := context.WithTimeout(ctx, 5*time.Second)
	defer stop()
	return conn.Write(deadline, websocket.MessageText, response.body.Bytes())
}

func (h *RPCHandler) checkCompanionSession(ctx context.Context, lease companionsession.Lease) error {
	storeCtx, stop := context.WithTimeout(ctx, companionStoreTimeout)
	defer stop()
	_, err := h.sessions.Resume(storeCtx, lease.ID, lease.UserID, lease.DeviceID)
	return err
}

// The caller can revoke only the lease admitted on THIS socket. No session id
// or owner from params is accepted, and underlying desktop processes are untouched.
func (h *RPCHandler) revokeSocketCompanion(w http.ResponseWriter, ctx context.Context, frame rpcFrame, lease companionsession.Lease) bool {
	_, params, err := deviceParams(frame.Params)
	if err != nil || len(params) != 0 {
		replyErr(w, http.StatusBadRequest, frame.ID, "invalid session revoke request", false)
		return false
	}
	storeCtx, stop := context.WithTimeout(ctx, companionStoreTimeout)
	defer stop()
	if err := h.sessions.Revoke(storeCtx, lease.ID, lease.UserID, lease.DeviceID); err != nil {
		replyErr(w, http.StatusServiceUnavailable, frame.ID, "the companion session could not be ended", false)
		return false
	}
	replyOK(w, frame.ID, map[string]bool{"revoked": true})
	return true
}
