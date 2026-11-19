package context

import "testing"

func TestDatabaseContextWindowOverridesStaticFallback(t *testing.T) {
	limits := WithContextWindow("gpt-5.6-luna", 1_048_576)
	if limits.MaxInputTokens != 1_048_576 {
		t.Fatalf("max input=%d, want database value", limits.MaxInputTokens)
	}
	if limits.CompactionThreshold() <= 800_000 {
		t.Fatalf("compaction threshold=%d still reflects stale fallback", limits.CompactionThreshold())
	}
}

func TestInvalidDatabaseWindowKeepsConservativeFallback(t *testing.T) {
	want := For("unknown-model")
	got := WithContextWindow("unknown-model", 0)
	if got != want {
		t.Fatalf("invalid database value changed limits: got=%+v want=%+v", got, want)
	}
}
