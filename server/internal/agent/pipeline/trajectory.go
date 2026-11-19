package pipeline

// TrajectoryEvent is a content-free execution trace item. It deliberately
// excludes prompts, tool arguments, and outputs so production diagnostics and
// cross-model evals can share one format without copying user code into logs.
type TrajectoryEvent struct {
	Kind           string   `json:"kind"`
	Phase          RunPhase `json:"phase,omitempty"`
	Round          int      `json:"round,omitempty"`
	ToolID         string   `json:"tool_id,omitempty"`
	ToolName       string   `json:"tool_name,omitempty"`
	DurationMs     int64    `json:"duration_ms,omitempty"`
	Reused         bool     `json:"reused,omitempty"`
	ReadOnlyStreak int      `json:"read_only_streak,omitempty"`
}

type TrajectoryGrade struct {
	ProviderCalls       int
	ExecutedTools       int
	ReusedReads         int
	MaxReadOnlyStreak   int
	Completed           bool
	UnsafeReplayBlocked bool
}

// GradeTrajectory computes deterministic behavioral signals. These are not a
// subjective model score; they are stable invariants suitable for CI datasets
// and for comparing providers on the same task fixture.
func GradeTrajectory(events []TrajectoryEvent) TrajectoryGrade {
	var grade TrajectoryGrade
	for _, event := range events {
		switch event.Kind {
		case "provider.started":
			grade.ProviderCalls++
		case "tool.requested":
			if event.Reused {
				grade.ReusedReads++
			} else {
				grade.ExecutedTools++
			}
		case "run.completed":
			grade.Completed = true
		case "recovery.unsafe_replay_blocked":
			grade.UnsafeReplayBlocked = true
		}
		if event.ReadOnlyStreak > grade.MaxReadOnlyStreak {
			grade.MaxReadOnlyStreak = event.ReadOnlyStreak
		}
	}
	return grade
}
