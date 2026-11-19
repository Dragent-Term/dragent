package api

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
)

const (
	forwardTunnelVersion   = byte(1)
	tunnelFrameOpen        = byte(1)
	tunnelFrameOpened      = byte(2)
	tunnelFrameData        = byte(3)
	tunnelFrameFin         = byte(4)
	tunnelFrameReset       = byte(5)
	tunnelFrameHeaderSize  = 14
	tunnelWriteQueueSize   = 1024
	tunnelEventQueueSize   = 64
	forwardTunnelReadLimit = 16 << 20
)

type forwardTunnelOpen struct {
	Port    uint16      `json:"port"`
	Method  string      `json:"method"`
	Path    string      `json:"path"`
	Headers [][2]string `json:"headers"`
}

func encodeTunnelFrame(kind byte, streamID uint64, payload []byte) []byte {
	frame := make([]byte, tunnelFrameHeaderSize+len(payload))
	frame[0] = forwardTunnelVersion
	frame[1] = kind
	binary.BigEndian.PutUint64(frame[2:10], streamID)
	binary.BigEndian.PutUint32(frame[10:14], uint32(len(payload)))
	copy(frame[tunnelFrameHeaderSize:], payload)
	return frame
}

func decodeTunnelFrame(frame []byte) (byte, uint64, []byte, error) {
	if len(frame) < tunnelFrameHeaderSize || frame[0] != forwardTunnelVersion {
		return 0, 0, nil, errors.New("invalid forward tunnel frame")
	}
	size := int(binary.BigEndian.Uint32(frame[10:14]))
	if size != len(frame)-tunnelFrameHeaderSize {
		return 0, 0, nil, errors.New("invalid forward tunnel payload length")
	}
	return frame[1], binary.BigEndian.Uint64(frame[2:10]), frame[tunnelFrameHeaderSize:], nil
}

func encodeTunnelOpen(open forwardTunnelOpen, body []byte) ([]byte, error) {
	metadata, err := json.Marshal(open)
	if err != nil {
		return nil, err
	}
	if len(metadata) > int(^uint32(0)) {
		return nil, errors.New("forward metadata is too large")
	}
	payload := make([]byte, 4+len(metadata)+len(body))
	binary.BigEndian.PutUint32(payload[:4], uint32(len(metadata)))
	copy(payload[4:], metadata)
	copy(payload[4+len(metadata):], body)
	return payload, nil
}

type forwardTunnelEvent struct {
	kind byte
	data []byte
}

type forwardTunnelStream struct {
	id     uint64
	host   *forwardTunnelHost
	events chan forwardTunnelEvent
	once   sync.Once
}

func (s *forwardTunnelStream) send(ctx context.Context, kind byte, data []byte) error {
	return s.host.send(ctx, encodeTunnelFrame(kind, s.id, data))
}

func (s *forwardTunnelStream) close() {
	s.once.Do(func() {
		s.host.removeStream(s.id)
		_ = s.send(context.Background(), tunnelFrameReset, nil)
	})
}

type forwardTunnelHost struct {
	conn      *websocket.Conn
	writes    chan []byte
	done      chan struct{}
	nextID    atomic.Uint64
	mu        sync.RWMutex
	streams   map[uint64]*forwardTunnelStream
	closeOnce sync.Once
}

func newForwardTunnelHost(conn *websocket.Conn) *forwardTunnelHost {
	return &forwardTunnelHost{
		conn: conn, writes: make(chan []byte, tunnelWriteQueueSize), done: make(chan struct{}),
		streams: make(map[uint64]*forwardTunnelStream),
	}
}

func (h *forwardTunnelHost) send(ctx context.Context, frame []byte) error {
	select {
	case h.writes <- frame:
		return nil
	case <-h.done:
		return errors.New("forward tunnel disconnected")
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (h *forwardTunnelHost) open(ctx context.Context, request forwardTunnelOpen, body []byte) (*forwardTunnelStream, error) {
	payload, err := encodeTunnelOpen(request, body)
	if err != nil {
		return nil, err
	}
	id := h.nextID.Add(1)
	stream := &forwardTunnelStream{id: id, host: h, events: make(chan forwardTunnelEvent, tunnelEventQueueSize)}
	h.mu.Lock()
	h.streams[id] = stream
	h.mu.Unlock()
	if err := h.send(ctx, encodeTunnelFrame(tunnelFrameOpen, id, payload)); err != nil {
		h.removeStream(id)
		return nil, err
	}
	select {
	case event := <-stream.events:
		if event.kind == tunnelFrameOpened {
			return stream, nil
		}
		h.removeStream(id)
		if len(event.data) != 0 {
			return nil, fmt.Errorf("loopback open failed: %s", event.data)
		}
		return nil, errors.New("loopback stream closed before opening")
	case <-h.done:
		h.removeStream(id)
		return nil, errors.New("forward tunnel disconnected")
	case <-ctx.Done():
		h.removeStream(id)
		return nil, ctx.Err()
	}
}

func (h *forwardTunnelHost) removeStream(id uint64) {
	h.mu.Lock()
	delete(h.streams, id)
	h.mu.Unlock()
}

func (h *forwardTunnelHost) dispatch(kind byte, id uint64, payload []byte) {
	h.mu.RLock()
	stream := h.streams[id]
	h.mu.RUnlock()
	if stream == nil {
		return
	}
	event := forwardTunnelEvent{kind: kind, data: append([]byte(nil), payload...)}
	select {
	case stream.events <- event:
	case <-h.done:
	}
}

func (h *forwardTunnelHost) close() {
	h.closeOnce.Do(func() {
		close(h.done)
		h.mu.Lock()
		streams := h.streams
		h.streams = make(map[uint64]*forwardTunnelStream)
		h.mu.Unlock()
		for _, stream := range streams {
			select {
			case stream.events <- forwardTunnelEvent{kind: tunnelFrameReset, data: []byte("forward tunnel disconnected")}:
			default:
			}
		}
	})
}

type forwardTunnelRegistry struct {
	mu    sync.RWMutex
	hosts map[string]*forwardTunnelHost
}

func newForwardTunnelRegistry() *forwardTunnelRegistry {
	return &forwardTunnelRegistry{hosts: make(map[string]*forwardTunnelHost)}
}

func (r *forwardTunnelRegistry) get(sessionID string) *forwardTunnelHost {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.hosts[sessionID]
}

func (r *forwardTunnelRegistry) wait(ctx context.Context, sessionID string) *forwardTunnelHost {
	if host := r.get(sessionID); host != nil {
		return host
	}
	ticker := time.NewTicker(25 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-ticker.C:
			if host := r.get(sessionID); host != nil {
				return host
			}
		case <-ctx.Done():
			return nil
		}
	}
}

func (r *forwardTunnelRegistry) register(sessionID string, host *forwardTunnelHost) {
	r.mu.Lock()
	previous := r.hosts[sessionID]
	r.hosts[sessionID] = host
	r.mu.Unlock()
	if previous != nil && previous != host {
		previous.close()
		_ = previous.conn.Close(websocket.StatusNormalClosure, "forward tunnel replaced")
	}
}

func (r *forwardTunnelRegistry) unregister(sessionID string, host *forwardTunnelHost) {
	r.mu.Lock()
	if r.hosts[sessionID] == host {
		delete(r.hosts, sessionID)
	}
	r.mu.Unlock()
	host.close()
}
