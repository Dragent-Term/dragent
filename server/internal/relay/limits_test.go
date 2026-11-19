package relay

import (
	"testing"
	"time"
)

func TestChannelCapsEnforced(t *testing.T) {
	c := NewChannelCaps()
	var chans [][16]byte
	for i := 0; i < MaxE2EEChannelsPerLease; i++ {
		var id [16]byte
		id[0] = byte(i + 1)
		pair := "phone\x00desktop"
		// Spread across pairs so the per-pair cap never trips: use a fresh
		// pair per channel here; the per-pair cap is covered in router tests.
		_ = pair
		if !c.Reserve(id, "lease-1", string(rune('a'+i))+"\x00desktop") {
			t.Fatalf("reserve %d refused", i)
		}
		chans = append(chans, id)
	}
	var extra [16]byte
	extra[0] = 0xFF
	if c.Reserve(extra, "lease-1", "z\x00desktop") {
		t.Fatal("ninth channel per lease admitted")
	}
	// Duplicate chan refused even on another lease.
	if c.Reserve(chans[0], "lease-2", "other\x00desktop") {
		t.Fatal("duplicate chan admitted")
	}
	c.Release(chans[0])
	if !c.Reserve(extra, "lease-1", "z\x00desktop") {
		t.Fatal("release did not free capacity")
	}
}

func TestSizeBucketBounded(t *testing.T) {
	for _, n := range []int{0, 64, 65, 256, 1024, 4096, 16384, 65535, 1 << 20} {
		b := SizeBucket(n)
		switch b {
		case "le64", "le256", "le1k", "le4k", "le16k", "le64k":
		default:
			t.Fatalf("bucket %d = %q", n, b)
		}
	}
}

func TestRelayStatsAggregateOnly(t *testing.T) {
	s := NewRelayStats()
	s.Record("e2ee-binary", "ok", "le1k", "none")
	s.Record("e2ee-binary", "ok", "le1k", "none")
	s.Record("e2ee-hs1", "refused", "le64", "bad-target")
	snap := s.SnapshotAndReset()
	if snap["e2ee-binary|ok|le1k|none"] != 2 {
		t.Fatalf("counts %+v", snap)
	}
	if len(s.SnapshotAndReset()) != 0 {
		t.Fatal("reset did not clear")
	}
}

func TestThroughputLimiterWindow(t *testing.T) {
	now := time.Now()
	current := now
	clock := func() time.Time { return current }
	l := NewThroughputLimiter(2, 10, clock)
	if !l.AllowFrame("lease-1", 5) || !l.AllowFrame("lease-1", 5) {
		t.Fatal("budget refused early frames")
	}
	if l.AllowFrame("lease-1", 1) {
		t.Fatal("frame cap bypassed")
	}
	// Other leases are independent.
	if !l.AllowFrame("lease-2", 5) {
		t.Fatal("leases share a budget")
	}
	// Next second resets.
	current = current.Add(2 * time.Second)
	if !l.AllowFrame("lease-1", 5) {
		t.Fatal("window did not reset")
	}
}
