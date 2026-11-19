// Package types is the server-side mirror of the client's
// `dragent_session_protocol` crate (crates/dragent_session_protocol in the
// Dragent repo). The JSON on the wire MUST match that crate exactly — same
// internally-tagged discriminators ("type" for messages, "kind" for terminal
// deltas), same snake_case values, same field names. If the two drift, clients
// silently fail to connect. Keep this file and the Rust crate in lockstep.
package types

import (
	"encoding/json"
	"strings"

	"github.com/google/uuid"
)

// Protocol version. Must equal PROTOCOL_VERSION in the Rust crate.
const ProtocolVersion = 2

// Message discriminator values (the "type" tag).
const (
	TypeInitialize              = "initialize"
	TypeReconnect               = "reconnect"
	TypeDelta                   = "delta"
	TypeSetParticipantRole      = "set_participant_role"
	TypeResolveCommand          = "resolve_command"
	TypeEndSession              = "end_session"
	TypeInitialized             = "initialized"
	TypeReconnected             = "reconnected"
	TypeParticipantsUpdated     = "participants_updated"
	TypeCommandRequested        = "command_requested"
	TypeTerminated              = "terminated"
	TypeJoin                    = "join"
	TypeRequestControl          = "request_control"
	TypeExecuteCommand          = "execute_command"
	TypeAgentQuery              = "agent_query"
	TypeAgentStop               = "agent_stop"
	TypeAgentQueryRequested     = "agent_query_requested"
	TypeAgentStopRequested      = "agent_stop_requested"
	TypeInputBytes              = "input_bytes"
	TypeInputReceived           = "input_received"
	TypeResize                  = "resize"
	TypeResizeRequested         = "resize_requested"
	TypeControl                 = "control"
	TypeControlReceived         = "control_received"
	TypePointer                 = "pointer"
	TypePointerMoved            = "pointer_moved"
	TypeAccessRequested         = "access_requested"
	TypeDenyAccessRequest       = "deny_access_request"
	TypeAccessDenied            = "access_denied"
	TypeRemoveParticipant       = "remove_participant"
	TypeJoined                  = "joined"
	TypeRoleChanged             = "role_changed"
	TypeRemoved                 = "removed"
	TypeEnded                   = "ended"
	TypePing                    = "ping"
	TypePong                    = "pong"
	TypeForwardCreate           = "forward_create"
	TypeForwardClose            = "forward_close"
	TypeForwardResponse         = "forward_response"
	TypeForwardCreated          = "forward_created"
	TypeForwardRequest          = "forward_request"
	TypeForwardClosed           = "forward_closed"
	TypeForwardError            = "forward_error"
	TypeForwardWebSocketOpen    = "forward_web_socket_open"
	TypeForwardWebSocketData    = "forward_web_socket_message"
	TypeForwardWebSocketClose   = "forward_web_socket_close"
	TypeForwardWebSocketOpened  = "forward_web_socket_opened"
	TypeForwardWebSocketMessage = "forward_web_socket_message"
	TypeForwardWebSocketClosed  = "forward_web_socket_closed"
)

// Role values (snake_case), matching the Rust `Role` enum.
const (
	RoleHost       = "host"
	RoleController = "controller"
	RoleViewer     = "viewer"
)

// AccessLevel values, matching the Rust `AccessLevel` enum.
const (
	AccessControl  = "control"
	AccessViewOnly = "view_only"
)

// Envelope peeks the discriminator on any inbound frame.
type Envelope struct {
	Type string `json:"type"`
}

// Participant mirrors the Rust `Participant` struct.
type Participant struct {
	ID          uuid.UUID `json:"id"`
	DisplayName string    `json:"display_name"`
	// AvatarURL is the profile photo when the participant has an account; empty
	// means clients should fall back to an initials avatar.
	AvatarURL string `json:"avatar_url,omitempty"`
	Role      string `json:"role"`
	Present   bool   `json:"present"`
}

// HostParticipantID is a stable sentinel id for the (single) host of a session,
// so viewers can render the host's presence in the participant list. The host's
// own participant list omits it (the host renders itself from its account).
var HostParticipantID = uuid.MustParse("00000000-0000-0000-0000-000000000001")

// HostParticipant is the synthetic presence entry for a session's host. When
// name/avatar are empty, viewers fall back to the literal "Host" label.
func HostParticipant(name, avatarURL string) Participant {
	if strings.TrimSpace(name) == "" {
		name = "Host"
	}
	return Participant{
		ID:          HostParticipantID,
		DisplayName: name,
		AvatarURL:   avatarURL,
		Role:        RoleHost,
		Present:     true,
	}
}

// ── Inbound: host → server ───────────────────────────────────────────────────

type HostInitialize struct {
	Type            string `json:"type"`
	ProtocolVersion int    `json:"protocol_version"`
	DeviceID        string `json:"device_id"`
	TabTitle        string `json:"tab_title"`
}

type HostReconnect struct {
	Type    string    `json:"type"`
	Session uuid.UUID `json:"session"`
	Token   string    `json:"token"`
}

// HostDelta relays an ordered terminal change. The server does not inspect the
// delta payload — it forwards it verbatim to viewers — so it stays raw.
type HostDelta struct {
	Type  string          `json:"type"`
	Delta json.RawMessage `json:"delta"`
}

type HostSetParticipantRole struct {
	Type        string    `json:"type"`
	Participant uuid.UUID `json:"participant"`
	Access      string    `json:"access"`
}

// HostDenyAccessRequest declines a participant's pending Edit request.
type HostDenyAccessRequest struct {
	Type        string    `json:"type"`
	Participant uuid.UUID `json:"participant"`
}

// HostRemoveParticipant kicks a participant from the session (revoke access).
type HostRemoveParticipant struct {
	Type        string    `json:"type"`
	Participant uuid.UUID `json:"participant"`
}

type HostResolveCommand struct {
	Type      string    `json:"type"`
	RequestID uuid.UUID `json:"request_id"`
	Approved  bool      `json:"approved"`
}

// HostEndSession ends the session. Reason (optional) is forwarded to viewers as
// their end message — e.g. "Session ended due to inactivity".
type HostEndSession struct {
	Type   string `json:"type"`
	Reason string `json:"reason,omitempty"`
}

type HostForwardCreate struct {
	Type          string `json:"type"`
	Port          uint16 `json:"port"`
	Label         string `json:"label"`
	TunnelVersion uint8  `json:"tunnel_version,omitempty"`
}

type HostForwardClose struct {
	Type      string `json:"type"`
	ForwardID string `json:"forward_id"`
}

type HostForwardResponse struct {
	Type      string      `json:"type"`
	RequestID string      `json:"request_id"`
	Status    uint16      `json:"status"`
	Headers   [][2]string `json:"headers"`
	Body      string      `json:"body_base64"`
}

// HostForwardWebSocket* carries one long-lived browser connection over the
// existing authenticated session socket. Data is raw TCP stream bytes (not
// decoded WebSocket messages) and is base64 because the control channel is
// text-framed.
type HostForwardWebSocketOpen struct {
	Type      string      `json:"type"`
	RequestID string      `json:"request_id"`
	Headers   [][2]string `json:"headers"`
}
type HostForwardWebSocketData struct {
	Type        string `json:"type"`
	RequestID   string `json:"request_id"`
	MessageType string `json:"message_type"` // retained for wire compatibility; always "data"
	Data        string `json:"data_base64"`
}
type HostForwardWebSocketClose struct {
	Type      string `json:"type"`
	RequestID string `json:"request_id"`
}

// ── Inbound: viewer → server ─────────────────────────────────────────────────

type ViewerJoin struct {
	Type            string              `json:"type"`
	ProtocolVersion int                 `json:"protocol_version"`
	Session         uuid.UUID           `json:"session"`
	Secret          string              `json:"secret"`
	DeviceID        string              `json:"device_id"`
	DisplayName     string              `json:"display_name"`
	Capabilities    *ViewerCapabilities `json:"capabilities,omitempty"`
}

type ViewerExecuteCommand struct {
	Type    string `json:"type"`
	Command string `json:"command"`
}

type ViewerAgentQuery struct {
	Type      string    `json:"type"`
	RequestID uuid.UUID `json:"request_id"`
	Query     string    `json:"query"`
}

type ViewerAgentStop struct {
	Type string `json:"type"`
}

// ViewerInputBytes carries raw input (typed keys + alt-screen keystrokes) from a
// controller. Forwarded to the host only if the sender holds Control.
type ViewerInputBytes struct {
	Type       string `json:"type"`
	DataBase64 string `json:"data_base64"`
}

// ViewerResize asks the host to resize its shared PTY to the controller's grid.
// Forwarded to the host only if the sender holds Control.
type ViewerResize struct {
	Type string `json:"type"`
	Rows uint16 `json:"rows"`
	Cols uint16 `json:"cols"`
}

// ViewerControl carries a controller's live input/scroll surface (what it's
// typing + where it scrolled) so the host desktop reflects it. `Surface` is kept
// OPAQUE (json.RawMessage) — the gateway forwards it verbatim and never parses
// its fields; only the Rust client interprets it. Forwarded only for Control.
type ViewerControl struct {
	Type    string          `json:"type"`
	Surface json.RawMessage `json:"surface"`
}

// PointerPosition is normalized to the viewer viewport. Pointer presence is
// deliberately separate from ViewerControl: view-only participants may be
// visible to the host without receiving permission to edit the terminal.
type PointerPosition struct {
	X float32 `json:"x"`
	Y float32 `json:"y"`
}

type ViewerPointer struct {
	Type     string          `json:"type"`
	Position PointerPosition `json:"position"`
}

type ViewerForwardRequest struct {
	Type      string      `json:"type"`
	ForwardID string      `json:"forward_id"`
	RequestID string      `json:"request_id"`
	Method    string      `json:"method"`
	Path      string      `json:"path"`
	Headers   [][2]string `json:"headers"`
	Body      string      `json:"body_base64"`
}

// ── Outbound: server → host ──────────────────────────────────────────────────

type ServerHostInitialized struct {
	Type           string    `json:"type"`
	Session        uuid.UUID `json:"session"`
	PairingCode    *string   `json:"pairing_code"`
	ReconnectToken string    `json:"reconnect_token"`
}

func Initialized(session uuid.UUID, pairingCode *string, reconnectToken string) ServerHostInitialized {
	return ServerHostInitialized{
		Type: TypeInitialized, Session: session, PairingCode: pairingCode, ReconnectToken: reconnectToken,
	}
}

type ServerParticipantsUpdated struct {
	Type         string        `json:"type"`
	Participants []Participant `json:"participants"`
}

func ParticipantsUpdated(p []Participant) ServerParticipantsUpdated {
	return ServerParticipantsUpdated{Type: TypeParticipantsUpdated, Participants: p}
}

type ServerCommandRequested struct {
	Type      string    `json:"type"`
	RequestID uuid.UUID `json:"request_id"`
	From      uuid.UUID `json:"from"`
	Command   string    `json:"command"`
	// AutoApproved: the request came from the host's own account, so the host
	// runs it immediately without prompting.
	AutoApproved bool `json:"auto_approved"`
}

func CommandRequested(requestID, from uuid.UUID, command string, autoApproved bool) ServerCommandRequested {
	return ServerCommandRequested{
		Type: TypeCommandRequested, RequestID: requestID, From: from,
		Command: command, AutoApproved: autoApproved,
	}
}

type ServerAgentQueryRequested struct {
	Type      string    `json:"type"`
	RequestID uuid.UUID `json:"request_id"`
	From      uuid.UUID `json:"from"`
	Query     string    `json:"query"`
}

func AgentQueryRequested(requestID, from uuid.UUID, query string) ServerAgentQueryRequested {
	return ServerAgentQueryRequested{
		Type: TypeAgentQueryRequested, RequestID: requestID, From: from, Query: query,
	}
}

type ServerAgentStopRequested struct {
	Type string    `json:"type"`
	From uuid.UUID `json:"from"`
}

func AgentStopRequested(from uuid.UUID) ServerAgentStopRequested {
	return ServerAgentStopRequested{Type: TypeAgentStopRequested, From: from}
}

// ServerInputReceived forwards a controller's raw input bytes to the host.
type ServerInputReceived struct {
	Type       string    `json:"type"`
	From       uuid.UUID `json:"from"`
	DataBase64 string    `json:"data_base64"`
}

func InputReceived(from uuid.UUID, dataBase64 string) ServerInputReceived {
	return ServerInputReceived{Type: TypeInputReceived, From: from, DataBase64: dataBase64}
}

// ServerResizeRequested forwards a controller's PTY resize request to the host.
type ServerResizeRequested struct {
	Type string    `json:"type"`
	From uuid.UUID `json:"from"`
	Rows uint16    `json:"rows"`
	Cols uint16    `json:"cols"`
}

func ResizeRequested(from uuid.UUID, rows, cols uint16) ServerResizeRequested {
	return ServerResizeRequested{Type: TypeResizeRequested, From: from, Rows: rows, Cols: cols}
}

// ServerControlReceived forwards a controller's input/scroll surface to the
// host, tagging it with the originating participant. `Surface` is the same
// opaque payload the viewer sent — the host client interprets it.
type ServerControlReceived struct {
	Type    string          `json:"type"`
	From    uuid.UUID       `json:"from"`
	Surface json.RawMessage `json:"surface"`
}

type ServerPointerMoved struct {
	Type     string          `json:"type"`
	From     uuid.UUID       `json:"from"`
	Position PointerPosition `json:"position"`
}

type ServerForwardCreated struct {
	Type      string `json:"type"`
	ForwardID string `json:"forward_id"`
	Port      uint16 `json:"port"`
	PublicURL string `json:"public_url"`
}

type ServerForwardRequest struct {
	Type      string      `json:"type"`
	ForwardID string      `json:"forward_id"`
	Port      uint16      `json:"port"`
	RequestID string      `json:"request_id"`
	Method    string      `json:"method"`
	Path      string      `json:"path"`
	Headers   [][2]string `json:"headers"`
	Body      string      `json:"body_base64"`
}

type ServerForwardClosed struct {
	Type      string `json:"type"`
	ForwardID string `json:"forward_id"`
}

func ControlReceived(from uuid.UUID, surface json.RawMessage) ServerControlReceived {
	return ServerControlReceived{Type: TypeControlReceived, From: from, Surface: surface}
}

func PointerMoved(from uuid.UUID, position PointerPosition) ServerPointerMoved {
	return ServerPointerMoved{Type: TypePointerMoved, From: from, Position: position}
}

// ServerAccessRequested tells the host a view-only participant wants Edit.
type ServerAccessRequested struct {
	Type        string    `json:"type"`
	Participant uuid.UUID `json:"participant"`
	DisplayName string    `json:"display_name"`
}

func AccessRequested(participant uuid.UUID, displayName string) ServerAccessRequested {
	return ServerAccessRequested{Type: TypeAccessRequested, Participant: participant, DisplayName: displayName}
}

type ServerTerminated struct {
	Type   string `json:"type"`
	Reason string `json:"reason"`
}

func Terminated(reason string) ServerTerminated {
	return ServerTerminated{Type: TypeTerminated, Reason: reason}
}

// ── Outbound: server → viewer ────────────────────────────────────────────────

type ServerViewerJoined struct {
	Type        string    `json:"type"`
	Participant uuid.UUID `json:"participant"`
	Role        string    `json:"role"`
	// IsHostAccount: this viewer authenticated as the HOST's own account (e.g. a
	// second device), so it may drive the host's terminal size. A different user
	// (teammate) may not — the client letterboxes the host's grid instead.
	IsHostAccount bool `json:"is_host_account"`
}

type ServerForwardResponse struct {
	Type      string      `json:"type"`
	RequestID string      `json:"request_id"`
	Status    uint16      `json:"status"`
	Headers   [][2]string `json:"headers"`
	Body      string      `json:"body_base64"`
}

type ServerForwardWebSocketOpen struct {
	Type      string      `json:"type"`
	ForwardID string      `json:"forward_id"`
	Port      uint16      `json:"port"`
	RequestID string      `json:"request_id"`
	Method    string      `json:"method"`
	Path      string      `json:"path"`
	Headers   [][2]string `json:"headers"`
	Body      string      `json:"body_base64"`
}
type ServerForwardWebSocketData struct {
	Type        string `json:"type"`
	RequestID   string `json:"request_id"`
	MessageType string `json:"message_type"` // retained for wire compatibility; always "data"
	Data        string `json:"data_base64"`
}
type ServerForwardWebSocketClose struct {
	Type      string `json:"type"`
	RequestID string `json:"request_id"`
}

type ServerForwardWebSocketEvent struct {
	Opened      bool
	Closed      bool
	RequestID   string
	MessageType string
	Data        string
}

type ServerForwardError struct {
	Type      string `json:"type"`
	RequestID string `json:"request_id"`
	Reason    string `json:"reason"`
}

func Joined(participant uuid.UUID, role string, isHostAccount bool) ServerViewerJoined {
	return ServerViewerJoined{Type: TypeJoined, Participant: participant, Role: role, IsHostAccount: isHostAccount}
}

// ServerDelta forwards a raw terminal delta to a viewer.
type ServerDelta struct {
	Type  string          `json:"type"`
	Delta json.RawMessage `json:"delta"`
}

func Delta(raw json.RawMessage) ServerDelta {
	return ServerDelta{Type: TypeDelta, Delta: raw}
}

type ServerRoleChanged struct {
	Type string `json:"type"`
	Role string `json:"role"`
}

func RoleChanged(role string) ServerRoleChanged {
	return ServerRoleChanged{Type: TypeRoleChanged, Role: role}
}

// AccessDenied tells a viewer their Edit request was declined by the host.
func AccessDenied() Simple { return Simple{Type: TypeAccessDenied} }

type ServerEnded struct {
	Type   string `json:"type"`
	Reason string `json:"reason"`
}

func Ended(reason string) ServerEnded   { return ServerEnded{Type: TypeEnded, Reason: reason} }
func Removed(reason string) ServerEnded { return ServerEnded{Type: TypeRemoved, Reason: reason} }

// ── Both directions ──────────────────────────────────────────────────────────

type Simple struct {
	Type string `json:"type"`
}

func Pong() Simple        { return Simple{Type: TypePong} }
func Reconnected() Simple { return Simple{Type: TypeReconnected} }
