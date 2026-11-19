package tasks

import (
	"testing"

	"server/internal/agent/pipeline"
)

func ev(text string) pipeline.AgentEvent {
	return pipeline.AgentEvent{Kind: pipeline.KindTextDelta, Text: text}
}

// A fresh subscriber (fromSeq 0) gets no backlog and only live events — the
// unchanged behaviour for a first connection.
func TestSubscribe_FreshConnectionGetsLiveOnly(t *testing.T) {
	task := &Task{}
	task.broadcast(ev("before subscribe")) // seq 1, no subscribers yet

	backlog, ch, ok := task.Subscribe(0)
	if !ok {
		t.Fatal("fresh subscribe must be resumable")
	}
	// Fresh connect replays the full history so far (here, the one earlier
	// event) — that is the backlog, and it is what makes the first /stream
	// deliver everything the runner produced before the client attached.
	if len(backlog) != 1 || backlog[0].Ev.Text != "before subscribe" {
		t.Fatalf("backlog = %+v, want the one prior event", backlog)
	}

	task.broadcast(ev("after subscribe"))
	got := <-ch
	if got.Ev.Text != "after subscribe" || got.Seq != 2 {
		t.Fatalf("live event = %+v, want seq 2 'after subscribe'", got)
	}
}

// The core of reconnect: a client that dropped after seq N gets exactly the
// events after N replayed, in order, once — no gap, no duplication.
func TestSubscribe_ReplaysExactlyTheGap(t *testing.T) {
	task := &Task{}
	for i := 1; i <= 5; i++ {
		task.broadcast(ev("e"))
	}
	// Client saw up to seq 3, then dropped. Reconnect from 3.
	backlog, _, ok := task.Subscribe(3)
	if !ok {
		t.Fatal("resume from a held seq must succeed")
	}
	if len(backlog) != 2 {
		t.Fatalf("backlog len = %d, want 2 (seq 4 and 5)", len(backlog))
	}
	if backlog[0].Seq != 4 || backlog[1].Seq != 5 {
		t.Fatalf("backlog seqs = %d,%d, want 4,5", backlog[0].Seq, backlog[1].Seq)
	}
}

// No event may slip between the history snapshot and the live subscription:
// Subscribe holds the lock, and broadcast takes the same lock, so an event
// broadcast right after Subscribe lands on the channel, not in a lost gap.
func TestSubscribe_NoGapBetweenBacklogAndLive(t *testing.T) {
	task := &Task{}
	task.broadcast(ev("1"))
	task.broadcast(ev("2"))

	backlog, ch, _ := task.Subscribe(0)
	// backlog covers 1,2. The very next broadcast must arrive live as seq 3,
	// with nothing between.
	task.broadcast(ev("3"))

	if backlog[len(backlog)-1].Seq != 2 {
		t.Fatalf("last backlog seq = %d, want 2", backlog[len(backlog)-1].Seq)
	}
	live := <-ch
	if live.Seq != 3 {
		t.Fatalf("first live seq = %d, want 3 — an event was lost or duplicated", live.Seq)
	}
}

// A resume from before the retained window is refused, so the client restarts
// cleanly instead of the runner deadlocking on a tool_call it can never resend.
func TestSubscribe_UnrecoverableGapIsRefused(t *testing.T) {
	task := &Task{}
	// Overflow the ring so the oldest seqs are evicted.
	for i := 0; i < maxHistory+100; i++ {
		task.broadcast(ev("e"))
	}
	// Ask to resume from seq 1, long evicted.
	_, _, ok := task.Subscribe(1)
	if ok {
		t.Fatal("resume from an evicted seq must be refused, not silently gapped")
	}
	// But resuming from a seq still in the window works.
	recent := task.CurrentSeq() - 10
	if _, _, ok := task.Subscribe(recent); !ok {
		t.Fatal("resume from a retained seq must succeed")
	}
}

// The ring buffer is bounded — a long turn cannot grow history without limit.
func TestBroadcast_HistoryIsBounded(t *testing.T) {
	task := &Task{}
	for i := 0; i < maxHistory*2; i++ {
		task.broadcast(ev("e"))
	}
	if len(task.history) > maxHistory {
		t.Fatalf("history len = %d, exceeds cap %d", len(task.history), maxHistory)
	}
	// Seq keeps counting past the cap — it is a cursor, not an index.
	if task.CurrentSeq() != uint64(maxHistory*2) {
		t.Fatalf("seq = %d, want %d", task.CurrentSeq(), maxHistory*2)
	}
}
