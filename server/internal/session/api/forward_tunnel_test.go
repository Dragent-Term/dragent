package api

import (
	"bytes"
	"context"
	"encoding/binary"
	"testing"
	"time"
)

func TestForwardTunnelFrameRoundTripPreservesRawBytes(t *testing.T) {
	want := []byte{0, 1, 127, 255}
	frame := encodeTunnelFrame(tunnelFrameData, 42, want)
	kind, streamID, got, err := decodeTunnelFrame(frame)
	if err != nil {
		t.Fatal(err)
	}
	if kind != tunnelFrameData || streamID != 42 || !bytes.Equal(got, want) {
		t.Fatalf("decoded frame = kind %d, stream %d, payload %v", kind, streamID, got)
	}
}

func TestForwardTunnelAcknowledgesColdModuleBurst(t *testing.T) {
	host := newForwardTunnelHost(nil)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	const streams = 256
	results := make(chan error, streams)
	for i := 0; i < streams; i++ {
		go func() {
			stream, err := host.open(ctx, forwardTunnelOpen{Port: 5173, Method: "GET", Path: "/src/module.ts"}, nil)
			if stream != nil {
				stream.close()
			}
			results <- err
		}()
	}
	opened := 0
	for opened < streams {
		select {
		case frame := <-host.writes:
			kind, streamID, _, err := decodeTunnelFrame(frame)
			if err != nil {
				t.Fatalf("burst frame %d: %v", opened, err)
			}
			if kind == tunnelFrameReset {
				continue
			}
			if kind != tunnelFrameOpen {
				t.Fatalf("burst frame %d has unexpected kind %d", opened, kind)
			}
			host.dispatch(tunnelFrameOpened, streamID, nil)
			opened++
		case <-ctx.Done():
			t.Fatalf("received only %d/%d stream opens", opened, streams)
		}
	}
	for i := 0; i < streams; i++ {
		if err := <-results; err != nil {
			t.Fatalf("burst stream %d did not open: %v", i, err)
		}
	}
}

func TestForwardTunnelRejectsMalformedPayloadLength(t *testing.T) {
	frame := encodeTunnelFrame(tunnelFrameData, 1, []byte("hello"))
	binary.BigEndian.PutUint32(frame[10:14], 99)
	if _, _, _, err := decodeTunnelFrame(frame); err == nil {
		t.Fatal("malformed payload length was accepted")
	}
}

func TestForwardTunnelOpenKeepsBodyOutsideJSON(t *testing.T) {
	body := []byte{0, 255, 3}
	payload, err := encodeTunnelOpen(forwardTunnelOpen{
		Port: 5173, Method: "POST", Path: "/upload", Headers: [][2]string{{"Content-Type", "application/octet-stream"}},
	}, body)
	if err != nil {
		t.Fatal(err)
	}
	metadataSize := int(binary.BigEndian.Uint32(payload[:4]))
	if metadataSize <= 0 || !bytes.Equal(payload[4+metadataSize:], body) {
		t.Fatalf("raw request body was not preserved: %v", payload)
	}
}
