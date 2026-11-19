// Binary relay framing (§4.1): the only bytes the server parses on the sockets.
//
// Wire layout (network byte order), 44-byte header + body (≤ 65,535):
//
//	offset size field
//	0      1    version = 0x01
//	1      1    kind: 0x01 HS, 0x02 DATA, 0x03 CLOSE
//	2      1    step: HS → 1..3; else 0
//	3      1    purpose: 0x01 companion (KK), 0x02 pairing (IKpsk1); else 0
//	4      16   chan: channel id (128-bit random, initiator-chosen)
//	20     16   peer: HS step 1 only; else zero
//	36     8    ctr: DATA only, u64; else 0
//	44     ...  body
//
// This mirrors dragent_e2ee/src/codec/header.rs exactly (same checks, same
// error names) so the Go relay (E2a) and the Rust codec (E1b) agree. Anything
// stateful (HS1 target, chan uniqueness, channel/byte caps) is the router's
// (router.go) and limiter's (limits.go), not the codec's.
package relay

import (
	"encoding/binary"
	"errors"
	"fmt"
)

// Wire constants (§4.1, §4.5).
const (
	// E2EEFrameVersion is the only accepted version byte.
	E2EEFrameVersion = 0x01
	// E2EEHeaderLen is the fixed header size.
	E2EEHeaderLen = 44
	// E2EEMaxBodyLen bounds one frame body (§4.1: at most 65,535 bytes).
	E2EEMaxBodyLen = 65535
	// E2EEMaxFrameLen is the largest whole frame (44 + 65,535 = 65,579).
	E2EEMaxFrameLen = E2EEHeaderLen + E2EEMaxBodyLen
	// E2EEGatewayReadLimit is the gateway read cap after the cut-over
	// (§4.5: drops from 8 MiB to 128 KiB). Binary frames never approach it,
	// but the socket read limit must stay bounded.
	E2EEGatewayReadLimit = 128 * 1024
)

// Frame kinds (§4.1).
const (
	E2EEKindHS    = 0x01
	E2EEKindData  = 0x02
	E2EEKindClose = 0x03
)

// Purposes (§4.1): live companion channel (KK) and pairing (IKpsk1).
const (
	E2EEPurposeCompanion = 0x01
	E2EEPurposePairing   = 0x02
)

// CLOSE reason bytes (unauthenticated hints; authenticated reasons travel
// as CTRL bye). Unknown bytes are preserved on decode so a new reason never
// breaks parsing; use CloseReasonKnown to enforce the known set.
const (
	E2EECloseProtocolError = 0x01
	E2EECloseUnknownPeer   = 0x02
)

// CloseReasonKnown reports whether a CLOSE reason byte is one this design defines.
func CloseReasonKnown(b byte) bool { return b == E2EECloseProtocolError || b == E2EECloseUnknownPeer }

// E2EE header validation errors. Names match the Rust HeaderError variants
// so E1b vectors map one-to-one.
var (
	ErrFrameTruncated      = errors.New("frame truncated")
	ErrFrameBadVersion     = errors.New("bad version")
	ErrFrameBadKind        = errors.New("bad kind")
	ErrFrameBadStep        = errors.New("bad step")
	ErrFrameBadPurpose     = errors.New("bad purpose")
	ErrFrameMissingPeer    = errors.New("HS step 1 without a peer id")
	ErrFrameUnexpectedPeer = errors.New("peer id where zero is required")
	ErrFrameNonZeroCtr     = errors.New("nonzero counter outside DATA")
	ErrFrameBadCloseBody   = errors.New("CLOSE body must be exactly one reason byte")
	ErrFrameBodyTooLong    = errors.New("body too long")
)

// E2EEFrame is one decoded §4.1 frame. Body aliases the input for DATA/HS;
// CLOSE bodies are the single reason byte. Peer is nil when the wire field
// is zero; Purpose is 0 when the wire byte is 0.
type E2EEFrame struct {
	Kind    byte
	Step    byte
	Purpose byte
	Chan    [16]byte
	Peer    *[16]byte
	Ctr     uint64
	Body    []byte
}

// IsHS / IsData / IsClose helpers for routing.
func (f *E2EEFrame) IsHS() bool    { return f.Kind == E2EEKindHS }
func (f *E2EEFrame) IsData() bool  { return f.Kind == E2EEKindData }
func (f *E2EEFrame) IsClose() bool { return f.Kind == E2EEKindClose }

// HasPeer reports whether the peer field is present (non-zero).
func (f *E2EEFrame) HasPeer() bool { return f.Peer != nil }

// DecodeE2EEFrame decodes one frame: header plus body. raw must hold exactly
// one frame's bytes (44 + body); trailing bytes belong to the next frame and
// must be split by the caller.
func DecodeE2EEFrame(raw []byte) (*E2EEFrame, error) {
	if len(raw) < E2EEHeaderLen {
		return nil, fmt.Errorf("%w: %d bytes", ErrFrameTruncated, len(raw))
	}
	bodyLen := len(raw) - E2EEHeaderLen
	if bodyLen > E2EEMaxBodyLen {
		return nil, fmt.Errorf("%w: %d bytes", ErrFrameBodyTooLong, bodyLen)
	}
	return decodeE2EEHeader(raw[:E2EEHeaderLen], raw[E2EEHeaderLen:], bodyLen)
}

// DecodeE2EEHeader decodes when the body arrives separately (the relay read
// path): header is exactly the 44 header bytes; bodyLen is checked against
// the 65,535-byte cap without needing the body in hand. body may be nil.
func DecodeE2EEHeader(header []byte, body []byte, bodyLen int) (*E2EEFrame, error) {
	if len(header) != E2EEHeaderLen {
		return nil, fmt.Errorf("%w: %d bytes", ErrFrameTruncated, len(header))
	}
	if bodyLen < 0 || bodyLen > E2EEMaxBodyLen {
		return nil, fmt.Errorf("%w: %d bytes", ErrFrameBodyTooLong, bodyLen)
	}
	if body != nil && len(body) != bodyLen {
		return nil, fmt.Errorf("%w: header says %d, got %d", ErrFrameTruncated, bodyLen, len(body))
	}
	full := make([]byte, 0, E2EEHeaderLen+bodyLen)
	full = append(full, header...)
	full = append(full, body...)
	return decodeE2EEHeader(header, full[E2EEHeaderLen:], bodyLen)
}

func decodeE2EEHeader(header, body []byte, bodyLen int) (*E2EEFrame, error) {
	if header[0] != E2EEFrameVersion {
		return nil, fmt.Errorf("%w: %#04x", ErrFrameBadVersion, header[0])
	}
	kind := header[1]
	if kind != E2EEKindHS && kind != E2EEKindData && kind != E2EEKindClose {
		return nil, fmt.Errorf("%w: %#04x", ErrFrameBadKind, kind)
	}
	step := header[2]
	purpose := header[3]
	var chanID [16]byte
	copy(chanID[:], header[4:20])
	var peerRaw [16]byte
	copy(peerRaw[:], header[20:36])
	ctr := binary.BigEndian.Uint64(header[36:44])
	var zero16 [16]byte
	var peer *[16]byte
	if peerRaw != zero16 {
		p := peerRaw
		peer = &p
	}
	switch kind {
	case E2EEKindHS:
		if step < 1 || step > 3 {
			return nil, fmt.Errorf("%w: %d", ErrFrameBadStep, step)
		}
		if purpose != E2EEPurposeCompanion && purpose != E2EEPurposePairing {
			return nil, fmt.Errorf("%w: %#04x", ErrFrameBadPurpose, purpose)
		}
		if ctr != 0 {
			return nil, fmt.Errorf("%w: %d", ErrFrameNonZeroCtr, ctr)
		}
		if step == 1 && peer == nil {
			return nil, ErrFrameMissingPeer
		}
		if step != 1 && peer != nil {
			return nil, ErrFrameUnexpectedPeer
		}
	case E2EEKindData:
		if step != 0 {
			return nil, fmt.Errorf("%w: %d", ErrFrameBadStep, step)
		}
		if purpose != 0 {
			return nil, fmt.Errorf("%w: %#04x", ErrFrameBadPurpose, purpose)
		}
		if peer != nil {
			return nil, ErrFrameUnexpectedPeer
		}
	case E2EEKindClose:
		if step != 0 {
			return nil, fmt.Errorf("%w: %d", ErrFrameBadStep, step)
		}
		if purpose != 0 {
			return nil, fmt.Errorf("%w: %#04x", ErrFrameBadPurpose, purpose)
		}
		if peer != nil {
			return nil, ErrFrameUnexpectedPeer
		}
		if ctr != 0 {
			return nil, fmt.Errorf("%w: %d", ErrFrameNonZeroCtr, ctr)
		}
		if bodyLen != 1 {
			return nil, fmt.Errorf("%w: %d bytes", ErrFrameBadCloseBody, bodyLen)
		}
	}
	return &E2EEFrame{Kind: kind, Step: step, Purpose: purpose, Chan: chanID, Peer: peer, Ctr: ctr, Body: body}, nil
}

// EncodeE2EEFrame builds one wire frame (44-byte header + body), applying the
// same length-and-enum check the relay enforces on receipt.
func EncodeE2EEFrame(kind, step, purpose byte, chanID [16]byte, peer *[16]byte, ctr uint64, body []byte) ([]byte, error) {
	if len(body) > E2EEMaxBodyLen {
		return nil, fmt.Errorf("%w: %d bytes", ErrFrameBodyTooLong, len(body))
	}
	out := make([]byte, E2EEHeaderLen+len(body))
	out[0] = E2EEFrameVersion
	out[1] = kind
	out[2] = step
	out[3] = purpose
	copy(out[4:20], chanID[:])
	if peer != nil {
		copy(out[20:36], peer[:])
	}
	binary.BigEndian.PutUint64(out[36:44], ctr)
	copy(out[44:], body)
	if _, err := DecodeE2EEFrame(out); err != nil {
		return nil, err
	}
	return out, nil
}

// Content-free text frame types that remain after the cut-over (§4.1):
// relay-auth, relay-ready, relay-error, relay-ping/pong, relay-chan-closed,
// companion.session.revoke, and the host-notify envelope (§8).
const (
	TextRelayAuth       = "relay-auth"
	TextRelayReady      = "relay-ready"
	TextRelayError      = "relay-error"
	TextRelayPing       = "relay-ping"
	TextRelayPong       = "relay-pong"
	TextRelayChanClosed = "relay-chan-closed"
	TextHostNotify      = "host-notify"
)

// RelayChanClosedFrame builds the backpressure text payload
// `relay-chan-closed {chan, reason}`. chanID is lowercase hex (no dashes);
// reason is the §4.1 hint that also rode the CLOSE body.
func RelayChanClosedFrame(chanHex, reason string) map[string]any {
	return map[string]any{"type": TextRelayChanClosed, "v": 1, "chan": chanHex, "reason": reason}
}
