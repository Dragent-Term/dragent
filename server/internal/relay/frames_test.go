package relay

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/uuid"
)

// headerVectors mirrors E1b's header.json schema (gen.py documents it).
type headerVectorFile struct {
	Vectors []struct {
		Name     string `json:"name"`
		BytesHex string `json:"bytes_hex"`
		Expect   *struct {
			Kind    *string `json:"kind"`
			Step    *int    `json:"step"`
			Purpose *string `json:"purpose"`
			Chan    *string `json:"chan"`
			Peer    *string `json:"peer"`
			Ctr     *uint64 `json:"ctr"`
			BodyHex *string `json:"body_hex"`
			Err     *string `json:"error"`
		} `json:"expect"`
		Error *string `json:"error"`
	} `json:"vectors"`
}

func loadHeaderVectors(t *testing.T) headerVectorFile {
	t.Helper()
	// From server/internal/relay to the workspace root's E1b vectors.
	candidates := []string{
		"../../../dragent/crates/dragent_e2ee/vectors/codec/header.json",
		"../../dragent/crates/dragent_e2ee/vectors/codec/header.json",
	}
	var raw []byte
	var err error
	for _, c := range candidates {
		raw, err = os.ReadFile(filepath.Clean(c))
		if err == nil {
			break
		}
	}
	if raw == nil {
		t.Fatalf("header vectors not found: %v", err)
	}
	var f headerVectorFile
	if err := json.Unmarshal(raw, &f); err != nil {
		t.Fatal(err)
	}
	return f
}

func TestE2EEHeaderVectors(t *testing.T) {
	f := loadHeaderVectors(t)
	if len(f.Vectors) == 0 {
		t.Fatal("no header vectors")
	}
	for _, v := range f.Vectors {
		t.Run(v.Name, func(t *testing.T) {
			raw, err := hex.DecodeString(v.BytesHex)
			if err != nil {
				t.Fatal(err)
			}
			frame, derr := DecodeE2EEFrame(raw)
			errName := v.Error
			if errName == nil && v.Expect != nil {
				errName = v.Expect.Err
			}
			if errName != nil {
				if derr == nil {
					t.Fatalf("expected error %s, decoded %+v", *errName, frame)
				}
				want := map[string]error{
					"BadVersion":     ErrFrameBadVersion,
					"BadKind":        ErrFrameBadKind,
					"BadStep":        ErrFrameBadStep,
					"BadPurpose":     ErrFrameBadPurpose,
					"MissingPeer":    ErrFrameMissingPeer,
					"UnexpectedPeer": ErrFrameUnexpectedPeer,
					"NonZeroCtr":     ErrFrameNonZeroCtr,
					"BadCloseBody":   ErrFrameBadCloseBody,
					"BodyTooLong":    ErrFrameBodyTooLong,
					"Truncated":      ErrFrameTruncated,
				}[*errName]
				if want == nil {
					t.Fatalf("unknown vector error %q", *errName)
				}
				if !errors.Is(derr, want) {
					t.Fatalf("error = %v, want %v", derr, want)
				}
				return
			}
			if derr != nil {
				t.Fatalf("decode: %v", derr)
			}
			exp := v.Expect
			wantKind := map[string]byte{"hs": E2EEKindHS, "data": E2EEKindData, "close": E2EEKindClose}[*exp.Kind]
			if frame.Kind != wantKind {
				t.Fatalf("kind = %#02x, want %s", frame.Kind, *exp.Kind)
			}
			if int(frame.Step) != *exp.Step {
				t.Fatalf("step = %d, want %d", frame.Step, *exp.Step)
			}
			var wantPurpose byte
			if exp.Purpose != nil {
				wantPurpose = map[string]byte{"companion": E2EEPurposeCompanion, "pairing": E2EEPurposePairing}[*exp.Purpose]
			}
			if frame.Purpose != wantPurpose {
				t.Fatalf("purpose = %#02x, want %v", frame.Purpose, exp.Purpose)
			}
			wantChan, _ := hex.DecodeString(strings.ReplaceAll(*exp.Chan, "-", ""))
			for i := range frame.Chan {
				if frame.Chan[i] != wantChan[i] {
					t.Fatalf("chan = %x, want %s", frame.Chan, *exp.Chan)
				}
			}
			if exp.Peer == nil && frame.Peer != nil {
				t.Fatalf("peer = %x, want none", *frame.Peer)
			}
			if exp.Peer != nil {
				if frame.Peer == nil {
					t.Fatalf("peer missing, want %s", *exp.Peer)
				}
				id := uuid.MustParse(*exp.Peer)
				if *frame.Peer != id {
					t.Fatalf("peer = %v, want %v", *frame.Peer, id)
				}
			}
			if exp.Ctr != nil && frame.Ctr != *exp.Ctr {
				t.Fatalf("ctr = %d, want %d", frame.Ctr, *exp.Ctr)
			}
			if exp.BodyHex != nil && hex.EncodeToString(frame.Body) != *exp.BodyHex {
				t.Fatalf("body = %x, want %s", frame.Body, *exp.BodyHex)
			}
			// Byte-exact re-encoding: the relay forwards frames unmodified,
			// so decode→encode must round-trip.
			re, err := EncodeE2EEFrame(frame.Kind, frame.Step, frame.Purpose, frame.Chan, frame.Peer, frame.Ctr, frame.Body)
			if err != nil {
				t.Fatalf("re-encode: %v", err)
			}
			if hex.EncodeToString(re) != v.BytesHex {
				t.Fatal("re-encoding differs from vector bytes")
			}
		})
	}
}

func TestE2EEHeaderRejectsUnknownKindAndVersion(t *testing.T) {
	good, err := EncodeE2EEFrame(E2EEKindData, 0, 0, [16]byte{1}, nil, 7, []byte{0xAA})
	if err != nil {
		t.Fatal(err)
	}
	bad := append([]byte(nil), good...)
	bad[0] = 0x02
	if _, err := DecodeE2EEFrame(bad); !errors.Is(err, ErrFrameBadVersion) {
		t.Fatalf("version: %v", err)
	}
	bad = append([]byte(nil), good...)
	bad[1] = 0x09
	if _, err := DecodeE2EEFrame(bad); !errors.Is(err, ErrFrameBadKind) {
		t.Fatalf("kind: %v", err)
	}
	if _, err := DecodeE2EEFrame(good[:10]); !errors.Is(err, ErrFrameTruncated) {
		t.Fatalf("truncated: %v", err)
	}
}

func TestE2EECloseReasonKnown(t *testing.T) {
	if !CloseReasonKnown(E2EECloseProtocolError) || !CloseReasonKnown(E2EECloseUnknownPeer) {
		t.Fatal("known reasons rejected")
	}
	if CloseReasonKnown(0x09) {
		t.Fatal("unknown reason accepted")
	}
}
