// E2EE relay caps and aggregate-only telemetry (§2.3, §4.1, §7).
//
// The relay enforces, per lease and per (phone, desktop) pair:
//   - at most 8 channels per lease, 2 per pair (§4.1);
//   - per-channel frame rate and per-lease byte rate caps;
//   - the 65,535-byte body cap (frames.go) and the 128 KiB gateway read cap.
//
// Logging rule (§2.3): per-minute aggregate counters keyed by route
// template, status class, size bucket and error class only. Never request
// or response bodies, query strings, tokens, titles, names, fingerprints,
// push tokens, or user/device ids in per-request lines. There is no
// per-request access log. This file is the only place E2EE relay code may
// log, and it logs aggregates.
package relay

import (
	"log/slog"
	"sort"
	"sync"
	"time"
)

// Channel and rate caps (§4.1). Numbers are relay-local DoS bounds, not
// protocol limits: legitimate use (keystrokes, 64 KiB TERM records, seeds
// via the part protocol) stays far under them.
const (
	// MaxE2EEChannelsPerLease bounds channels on one companion lease.
	MaxE2EEChannelsPerLease = 8
	// MaxE2EEChannelsPerPair bounds channels per (phone, desktop) pair.
	// Rekey/reconnect holds at most 2 (old + new); a third evicts the oldest.
	MaxE2EEChannelsPerPair = 2
	// MaxE2EEFramesPerSecondPerChannel bounds one channel's frame rate.
	MaxE2EEFramesPerSecondPerChannel = 500
	// MaxE2EEBytesPerSecondPerLease bounds one lease's binary throughput.
	MaxE2EEBytesPerSecondPerLease = 2 * 1024 * 1024
	// MaxE2EENotifyBlobChars bounds one host-notify blob (§8: exactly 1,494
	// base64url chars for a 1,120-byte sealed payload). Intake accepts only
	// the exact sealed size so length leaks nothing.
	MaxE2EENotifyBlobChars = 1494
)

// SizeBucket buckets one frame body for aggregate counters (§7: bucketed
// sizes only, never exact lengths).
func SizeBucket(n int) string {
	switch {
	case n <= 64:
		return "le64"
	case n <= 256:
		return "le256"
	case n <= 1024:
		return "le1k"
	case n <= 4096:
		return "le4k"
	case n <= 16384:
		return "le16k"
	default:
		return "le64k"
	}
}

// RelayStats holds per-minute aggregate counters. The key joins route
// template, status class, size bucket and error class — never ids, tokens,
// or content. Use SnapshotAndReset once a minute from a single goroutine.
type RelayStats struct {
	mu     sync.Mutex
	counts map[string]uint64
}

// NewRelayStats returns an empty counter set.
func NewRelayStats() *RelayStats { return &RelayStats{counts: make(map[string]uint64)} }

// Record adds one event to the aggregate. All four labels are fixed
// vocabularies (route templates like "e2ee-binary", status classes like
// "ok"/"refused"/"error", size buckets from SizeBucket, error classes from
// the frame codec or "none").
func (s *RelayStats) Record(route, statusClass, sizeBucket, errorClass string) {
	if s == nil {
		return
	}
	k := route + "|" + statusClass + "|" + sizeBucket + "|" + errorClass
	s.mu.Lock()
	s.counts[k]++
	s.mu.Unlock()
}

// SnapshotAndReset returns the current counters and clears them.
func (s *RelayStats) SnapshotAndReset() map[string]uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make(map[string]uint64, len(s.counts))
	for k, v := range s.counts {
		out[k] = v
	}
	s.counts = make(map[string]uint64)
	return out
}

// LogAggregates emits one slog line per non-empty counter key. It carries no
// ids, tokens, or content — only the aggregate key and its count.
func (s *RelayStats) LogAggregates(log *slog.Logger) {
	if s == nil {
		return
	}
	if log == nil {
		log = slog.Default()
	}
	snap := s.SnapshotAndReset()
	for _, k := range sortedKeys(snap) {
		log.Info("e2ee relay aggregate", "counter", k, "count", snap[k])
	}
}

func sortedKeys(m map[string]uint64) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// ChannelCaps tracks chan counts per lease and per pair so the router can
// enforce MaxE2EEChannelsPerLease / MaxE2EEChannelsPerPair without parsing
// any content.
type ChannelCaps struct {
	mu       sync.Mutex
	byLease  map[string]int
	byPair   map[string]int
	chans    map[[16]byte]string // chan -> lease (uniqueness)
	chanPair map[[16]byte]string // chan -> pair key
}

// NewChannelCaps returns empty cap tracking.
func NewChannelCaps() *ChannelCaps {
	return &ChannelCaps{
		byLease:  make(map[string]int),
		byPair:   make(map[string]int),
		chans:    make(map[[16]byte]string),
		chanPair: make(map[[16]byte]string),
	}
}

// Reserve claims one chan for lease+pair. It fails when the chan id is
// already known or either cap is reached.
func (c *ChannelCaps) Reserve(chanID [16]byte, lease, pair string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, dup := c.chans[chanID]; dup {
		return false
	}
	if c.byLease[lease] >= MaxE2EEChannelsPerLease {
		return false
	}
	if c.byPair[pair] >= MaxE2EEChannelsPerPair {
		return false
	}
	c.chans[chanID] = lease
	c.chanPair[chanID] = pair
	c.byLease[lease]++
	c.byPair[pair]++
	return true
}

// Release frees one chan. Unknown chans are ignored.
func (c *ChannelCaps) Release(chanID [16]byte) {
	c.mu.Lock()
	defer c.mu.Unlock()
	lease, ok := c.chans[chanID]
	if !ok {
		return
	}
	pair := c.chanPair[chanID]
	delete(c.chans, chanID)
	delete(c.chanPair, chanID)
	if c.byLease[lease] > 0 {
		c.byLease[lease]--
		if c.byLease[lease] == 0 {
			delete(c.byLease, lease)
		}
	}
	if c.byPair[pair] > 0 {
		c.byPair[pair]--
		if c.byPair[pair] == 0 {
			delete(c.byPair, pair)
		}
	}
}

// Known reports whether chan is already registered (uniqueness check).
func (c *ChannelCaps) Known(chanID [16]byte) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	_, ok := c.chans[chanID]
	return ok
}

// LeaseCount and PairCount expose current use for tests.
func (c *ChannelCaps) LeaseCount(lease string) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.byLease[lease]
}

// PairCount exposes current pair use for tests.
func (c *ChannelCaps) PairCount(pair string) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.byPair[pair]
}

// ThroughputLimiter is a fixed-window per-lease byte/frame cap. Windows are
// one second; over-cap frames are refused (the channel is closed with
// relay-chan-closed) rather than queued, so the relay never reorders.
type ThroughputLimiter struct {
	mu        sync.Mutex
	frames    map[string]*window
	bytes     map[string]*window
	now       func() time.Time
	maxFrames int
	maxBytes  int
}

type window struct {
	start time.Time
	n     int
}

// NewThroughputLimiter returns a limiter with the package caps, or custom
// caps for tests (pass <=0 to keep the default).
func NewThroughputLimiter(maxFrames, maxBytes int, now func() time.Time) *ThroughputLimiter {
	if maxFrames <= 0 {
		// 8 channels × 500 frames/s (§4.1 cap × per-channel rate cap).
		maxFrames = 4000
	}
	if maxBytes <= 0 {
		maxBytes = MaxE2EEBytesPerSecondPerLease
	}
	if now == nil {
		now = time.Now
	}
	return &ThroughputLimiter{
		frames:    make(map[string]*window),
		bytes:     make(map[string]*window),
		now:       now,
		maxFrames: maxFrames,
		maxBytes:  maxBytes,
	}
}

// AllowFrame reports whether one frame of bodyLen bytes fits this second's
// budget for lease. Channel frame-rate uses a share of the lease budget.
func (l *ThroughputLimiter) AllowFrame(lease string, bodyLen int) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.now()
	w := l.frames[lease]
	if w == nil || now.Sub(w.start) >= time.Second {
		w = &window{start: now}
		l.frames[lease] = w
	}
	b := l.bytes[lease]
	if b == nil || now.Sub(b.start) >= time.Second {
		b = &window{start: now}
		l.bytes[lease] = b
	}
	if w.n+1 > l.maxFrames || b.n+bodyLen > l.maxBytes {
		return false
	}
	w.n++
	b.n += bodyLen
	return true
}
