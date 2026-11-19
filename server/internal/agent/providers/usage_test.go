package providers

import "testing"

func TestFinalizeUsage_ZeroGuardEstimates(t *testing.T) {
	// Provider reported nothing but the turn produced output → must estimate, not bill zero.
	r := &StepResult{Content: "hello world this is some output text"}
	out := finalizeUsage("test", "test-model", r, 1234)
	if out.InputTokens != 1234 {
		t.Fatalf("expected input estimate 1234, got %d", out.InputTokens)
	}
	if out.OutputTokens == 0 {
		t.Fatal("expected non-zero output estimate")
	}
}

func TestFinalizeUsage_ClampsGarbage(t *testing.T) {
	r := &StepResult{Content: "x", InputTokens: -5, OutputTokens: maxSaneTokens + 1000}
	out := finalizeUsage("test", "m", r, 10)
	if out.InputTokens < 0 {
		t.Fatal("negative not clamped")
	}
	if out.OutputTokens > maxSaneTokens {
		t.Fatal("absurd not clamped")
	}
}

func TestFinalizeUsage_KeepsRealCounts(t *testing.T) {
	r := &StepResult{Content: "hi", InputTokens: 100, CachedInputTokens: 20, OutputTokens: 50}
	out := finalizeUsage("test", "m", r, 999)
	if out.InputTokens != 100 || out.OutputTokens != 50 || out.CachedInputTokens != 20 {
		t.Fatalf("real counts altered: %+v", out)
	}
}
