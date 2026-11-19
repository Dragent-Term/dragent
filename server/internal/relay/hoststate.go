// Host-published mirror state for the R2 method table.
//
// The server never touches the host's disks or processes: the authenticated
// desktop publishes bounded snapshots of what its own modules already render
// (tabs in the shape of `dragent panes list`, worktrees in the shape of
// `dragent workspaces list`, file listings/contents read through the host's
// own file access), and the phone's RPC methods read that mirror. Owner
// scoping reuses ownerKey, so no method can publish or read another account's
// data.
package relay

import (
	"context"
	"net/http"
	"strings"
	"sync"
	"unicode/utf8"
)

// Publish caps: the mirror is a listing, not storage. Host snapshots that
// exceed these bounds are rejected (400) rather than truncated, so a buggy
// host surfaces instead of presenting a partial machine.
const (
	maxPublishedListings = 512
	maxListingEntries    = 2048
	maxStoredFiles       = 256
	// maxStoredFileBytes bounds one stored file's text. Reads serve a smaller
	// window with an explicit truncated flag (see files.go).
	maxStoredFileBytes = 256 * 1024
	// maxPublishBodyBytes bounds the whole host publish body.
	maxPublishBodyBytes = 2 * 1024 * 1024
)

// FileEntry is one entry of a published directory listing. Kind is exactly
// "file" or "dir"; dotfiles are included, symlinks are never followed —
// both are host-side duties (see dragent/app/src/remote_fs.rs) re-checked
// here at publish time.
type FileEntry struct {
	Name string `json:"name"`
	Kind string `json:"kind"`
}

// FileListing is the published snapshot of one directory.
type FileListing struct {
	Path    string      `json:"path"`
	Entries []FileEntry `json:"entries"`
}

// FileContent is one published file's text.
type FileContent struct {
	Path    string `json:"path"`
	Content string `json:"content"`
}

// HostState is one device's full R2 mirror: tab and worktree snapshots in
// the catalog shapes plus bounded file listings/contents.
type HostState struct {
	Tabs      []TabEntry       `json:"tabs"`
	Worktrees []WorkspaceEntry `json:"worktrees"`
	Listings  []FileListing    `json:"listings"`
	Contents  []FileContent    `json:"contents"`
}

// HostStore persists one HostState per device-scoped owner and remembers
// which accounts published under each device id (for the 403 foreign-device
// rule). Production may back it with Redis; the in-memory store covers tests
// and single-instance dev.
type HostStore interface {
	Publish(ctx context.Context, owner string, state HostState) error
	Get(ctx context.Context, owner string) (HostState, bool, error)
	// DeviceUsers lists the account ids that published under deviceID. Empty
	// (no error) means the device never published: reads report 404, never
	// an oracle for other accounts.
	DeviceUsers(ctx context.Context, deviceID string) ([]string, error)
}

// MemoryHostStore is an in-memory HostStore.
type MemoryHostStore struct {
	mu      sync.Mutex
	states  map[string]HostState
	devices map[string]map[string]bool
}

// NewMemoryHostStore returns an empty MemoryHostStore.
func NewMemoryHostStore() *MemoryHostStore {
	return &MemoryHostStore{
		states:  make(map[string]HostState),
		devices: make(map[string]map[string]bool),
	}
}

func splitOwner(owner string) (userID, deviceID string, ok bool) {
	parts := strings.Split(owner, "\x00")
	if len(parts) != 3 || parts[0] != "device" || parts[1] == "" || parts[2] == "" {
		return "", "", false
	}
	return parts[1], parts[2], true
}

func (s *MemoryHostStore) Publish(_ context.Context, owner string, state HostState) error {
	userID, deviceID, ok := splitOwner(owner)
	if !ok {
		return errInvalidOwner
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.states[owner] = state.cloned()
	set := s.devices[deviceID]
	if set == nil {
		set = make(map[string]bool)
		s.devices[deviceID] = set
	}
	set[userID] = true
	return nil
}

func (s *MemoryHostStore) Get(_ context.Context, owner string) (HostState, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	state, ok := s.states[owner]
	if !ok {
		return HostState{}, false, nil
	}
	// Copy on read: the stored slices must never alias a caller's view, so a
	// reader holding (or mutating) the result cannot corrupt the mirror or
	// race a concurrent publish (which replaces the stored entry wholesale).
	return state.cloned(), true, nil
}

// cloned deep-copies the mirror: slices, nested listing entries, and the
// workspace/tab reference strings (branch, project, ids, agent identity) a
// caller could otherwise mutate through the returned pointers.
func (s HostState) cloned() HostState {
	out := HostState{
		Tabs:      make([]TabEntry, 0, len(s.Tabs)),
		Worktrees: make([]WorkspaceEntry, 0, len(s.Worktrees)),
		Listings:  make([]FileListing, 0, len(s.Listings)),
		Contents:  append([]FileContent{}, s.Contents...),
	}
	for _, t := range s.Tabs {
		out.Tabs = append(out.Tabs, cloneTabEntry(t))
	}
	for _, wt := range s.Worktrees {
		out.Worktrees = append(out.Worktrees, cloneWorkspaceEntry(wt))
	}
	for _, l := range s.Listings {
		l.Entries = append([]FileEntry{}, l.Entries...)
		out.Listings = append(out.Listings, l)
	}
	return out
}

// cloneWorkspaceEntry deep-copies one workspace's reference pointers.
func cloneWorkspaceEntry(w WorkspaceEntry) WorkspaceEntry {
	if w.Branch != nil {
		branch := *w.Branch
		w.Branch = &branch
	}
	if w.Project != nil {
		project := *w.Project
		w.Project = &project
	}
	if w.ID != nil {
		id := *w.ID
		w.ID = &id
	}
	if w.ProjectID != nil {
		projectID := *w.ProjectID
		w.ProjectID = &projectID
	}
	return w
}

// cloneTabEntry deep-copies one tab's reference pointers.
func cloneTabEntry(t TabEntry) TabEntry {
	if t.WorkspaceID != nil {
		id := *t.WorkspaceID
		t.WorkspaceID = &id
	}
	if t.ProjectID != nil {
		id := *t.ProjectID
		t.ProjectID = &id
	}
	if t.AgentKind != nil {
		kind := *t.AgentKind
		t.AgentKind = &kind
	}
	if t.AgentState != nil {
		state := *t.AgentState
		t.AgentState = &state
	}
	if t.Attention != nil {
		attention := *t.Attention
		t.Attention = &attention
	}
	return t
}

func (s *MemoryHostStore) DeviceUsers(_ context.Context, deviceID string) ([]string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var users []string
	for u := range s.devices[strings.TrimSpace(deviceID)] {
		users = append(users, u)
	}
	return users, nil
}

// publishStateBody is what the authenticated desktop sends: its device plus
// the mirror sections it currently renders. Every section is optional on any
// one publish, but PUT semantics replace the whole mirror (like the catalog
// PUT): the host always publishes everything it knows.
type publishStateBody struct {
	DeviceID  string           `json:"device_id"`
	Tabs      []TabEntry       `json:"tabs"`
	Worktrees []WorkspaceEntry `json:"worktrees"`
	Listings  []FileListing    `json:"listings"`
	Contents  []FileContent    `json:"contents"`
}

// PublishState served the calling account's host-state mirror. E2EE
// cut-over (§10, F3): the desktop state mirror is removed — the desktop
// serves listings and files inside the KK channel. The route returns 410
// before the body is read; nothing is stored.
func (h *RPCHandler) PublishState(w http.ResponseWriter, r *http.Request) {
	if _, authed := callerUserID(r); !authed {
		writeErr(w, http.StatusUnauthorized, "login required for relay")
		return
	}
	goneRPC(w, nil)
}

// normalized trims whitespace and replaces nil slices with empty ones so the
// mirror always carries arrays.
func (s HostState) normalized() HostState {
	out := HostState{
		Tabs:      make([]TabEntry, 0, len(s.Tabs)),
		Worktrees: make([]WorkspaceEntry, 0, len(s.Worktrees)),
		Listings:  make([]FileListing, 0, len(s.Listings)),
		Contents:  make([]FileContent, 0, len(s.Contents)),
	}
	for _, t := range s.Tabs {
		t.Kind = strings.TrimSpace(t.Kind)
		t.Title = strings.TrimSpace(t.Title)
		if t.WorkspaceID != nil {
			id := strings.TrimSpace(*t.WorkspaceID)
			t.WorkspaceID = &id
		}
		if t.ProjectID != nil {
			id := strings.TrimSpace(*t.ProjectID)
			t.ProjectID = &id
		}
		if t.AgentKind != nil {
			kind := strings.TrimSpace(*t.AgentKind)
			t.AgentKind = &kind
		}
		if t.AgentState != nil {
			state := strings.TrimSpace(*t.AgentState)
			t.AgentState = &state
		}
		if t.Attention != nil {
			attention := strings.TrimSpace(*t.Attention)
			t.Attention = &attention
		}
		out.Tabs = append(out.Tabs, t)
	}
	for _, wt := range s.Worktrees {
		wt.Name = strings.TrimSpace(wt.Name)
		wt.Path = strings.TrimSpace(wt.Path)
		if wt.Branch != nil {
			branch := strings.TrimSpace(*wt.Branch)
			wt.Branch = &branch
		}
		if wt.Project != nil {
			project := strings.TrimSpace(*wt.Project)
			wt.Project = &project
		}
		if wt.ID != nil {
			id := strings.TrimSpace(*wt.ID)
			wt.ID = &id
		}
		if wt.ProjectID != nil {
			projectID := strings.TrimSpace(*wt.ProjectID)
			wt.ProjectID = &projectID
		}
		out.Worktrees = append(out.Worktrees, wt)
	}
	for _, l := range s.Listings {
		nl := FileListing{Path: cleanPubPath(l.Path), Entries: make([]FileEntry, 0, len(l.Entries))}
		for _, e := range l.Entries {
			e.Name = strings.TrimSpace(e.Name)
			e.Kind = strings.TrimSpace(e.Kind)
			nl.Entries = append(nl.Entries, e)
		}
		out.Listings = append(out.Listings, nl)
	}
	for _, c := range s.Contents {
		c.Path = cleanPubPath(c.Path)
		out.Contents = append(out.Contents, c)
	}
	return out
}

func (s HostState) validate() error {
	// Reuse the catalog bounds for the tab/worktree shapes: the R2 mirror
	// carries the same host entries (already whitespace-trimmed above).
	if err := (Catalog{Workspaces: s.Worktrees, Tabs: s.Tabs}).Validate(); err != nil {
		return err
	}
	if len(s.Listings) > maxPublishedListings {
		return errTooManyListings
	}
	seen := make(map[string]bool, len(s.Listings))
	for i := range s.Listings {
		if err := s.Listings[i].validate(); err != nil {
			return err
		}
		if seen[s.Listings[i].Path] {
			return errDuplicateListing
		}
		seen[s.Listings[i].Path] = true
	}
	if len(s.Contents) > maxStoredFiles {
		return errTooManyFiles
	}
	contentsSeen := make(map[string]bool, len(s.Contents))
	for i := range s.Contents {
		if err := s.Contents[i].validate(); err != nil {
			return err
		}
		if contentsSeen[s.Contents[i].Path] {
			return errDuplicateContent
		}
		contentsSeen[s.Contents[i].Path] = true
	}
	return nil
}

func (l FileListing) validate() error {
	if !validPubPath(l.Path) {
		return errInvalidPath
	}
	if len(l.Entries) > maxListingEntries {
		return errTooManyEntries
	}
	for i := range l.Entries {
		if err := l.Entries[i].validate(); err != nil {
			return err
		}
	}
	return nil
}

func (e FileEntry) validate() error {
	name := e.Name
	if name == "" || len(name) > maxNameLength {
		return errInvalidEntry
	}
	// Entries are bare names: no separators, no parent traversal. The host
	// must publish dotfiles verbatim and must never follow symlinks.
	if strings.ContainsAny(name, "/\\") || name == "." || name == ".." {
		return errInvalidEntry
	}
	switch e.Kind {
	case "file", "dir":
	default:
		return errInvalidEntry
	}
	return nil
}

func (c FileContent) validate() error {
	if !validPubPath(c.Path) {
		return errInvalidPath
	}
	if len(c.Content) > maxStoredFileBytes {
		return errFileTooLarge
	}
	// files.open is text only: reject binary at publish so reads never serve
	// undecodable bytes.
	if !utf8.ValidString(c.Content) {
		return errBinaryContent
	}
	return nil
}
