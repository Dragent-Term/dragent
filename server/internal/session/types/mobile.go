package types

// ViewerCapabilities negotiates additive v2 messages. Older Rust/web clients
// must not receive new discriminators they did not explicitly opt into.
type ViewerCapabilities struct {
	ReplayBoundary bool `json:"replay_boundary,omitempty"`
}

const TypeReplayComplete = "replay_complete"

// ReplayComplete marks the end of this gateway's cached initial room history.
// It does not delimit the desktop's later resynchronization, grant catalog
// access, or make first-seen completed work a new live completion event.
func ReplayComplete() Simple { return Simple{Type: TypeReplayComplete} }
