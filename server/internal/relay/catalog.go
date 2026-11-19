// Package relay is an explicitly authorized adapter that exposes a host's
// whole-machine catalog over the cloud relay: the projects open in Dragent,
// the sidebar workspaces with their git branches, and the workspace tabs with
// their kinds.
//
// The wire shapes mirror the host's existing local APIs so the desktop can
// publish verbatim what it already renders:
//
//   - `dragent projects list` → Catalog.Projects ({name, path})
//   - `dragent workspaces list` → Catalog.Workspaces ({name, path, branch, project})
//   - `dragent panes list` → Catalog.Tabs ({id, kind, title, ...})
//
// Tab kinds are exactly the host's pane kinds: terminal, browser, settings.
//
// Authorization follows the permission pattern in server/internal/session:
// every call requires an authenticated same-account caller. The catalog is
// keyed by a device-scoped owner ("device\x00<user>\x00<device>", the same
// construction as the session forward owner), so a caller can only ever
// address their own account's catalog — a different account's device name
// resolves to a different, empty owner. There is no unauthenticated surface:
// RegisterRoutes must be mounted under RequireAuth, and the handler itself
// still rejects requests without a user id.
//
// The lead integrates this package by mounting RegisterRoutes and having the
// authenticated desktop publish its catalog (PUT) for the mobile
// same-account controller to read (GET).
package relay

import (
	"context"
	"encoding/base64"
	"errors"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
)

// Tab kinds, mirroring the host's pane kinds in
// dragent/app/src/workspace/pane_tools.rs.
const (
	TabKindTerminal = "terminal"
	TabKindBrowser  = "browser"
	TabKindSettings = "settings"
	// TabKindAgent is an explicit agent identity on a pane. Unknown agent
	// detection must remain terminal: only an explicitly published agent
	// kind counts as agent.
	TabKindAgent = "agent"
)

// Agent states for an agent tab's lifecycle.
const (
	AgentStateIdle    = "idle"
	AgentStateWorking = "working"
	AgentStateReview  = "review"
)

// What an agent tab needs from the person right now, beside its state:
// it waits at a permission prompt or a question, or its turn failed.
// Publishers that predate it omit it; readers that predate it ignore it.
const (
	AttentionNeedsInput = "needs_input"
	AttentionFailed     = "failed"
)

// Numeric tab IDs must survive JSON decoding in mobile JavaScript exactly.
const maxSafeJSInteger = 9007199254740991

// Catalog bounds. A catalog is a small listing, not a data dump: these caps
// keep one malicious or buggy host from turning the relay into storage (the
// session gateway applies the same thinking to forwarded headers and bodies).
const (
	maxProjects   = 128
	maxWorkspaces = 256
	maxTabs       = 1024

	maxNameLength   = 256
	maxPathLength   = 1024
	maxTitleLength  = 1024
	maxBranchLength = 256

	// maxCatalogIDLength bounds optional relationship IDs. Stable
	// repository / worktree paths are allowed as IDs (they may contain
	// slashes), so the bound matches paths rather than the RPC envelope's
	// correlation-id bound (rpc.go keeps its own maxIDLength separate).
	maxCatalogIDLength = 1024
	// maxMachineFieldLength bounds machine metadata display fields.
	maxMachineFieldLength = 256
	// maxAgentKindLength bounds the explicit agent identity string.
	maxAgentKindLength = 128
)

// MachineInfo is desktop metadata published with the catalog (no claimed
// identity: the owner key remains the only identity). Version is optional.
type MachineInfo struct {
	Name     string `json:"name"`
	Platform string `json:"platform"`
	Version  string `json:"version,omitempty"`
}

// MachineSummary is the discovery wire shape for one device: its published
// metadata plus the relay-assigned last-seen time (Unix ms) and the
// relay-computed online flag (last seen within machineOnlineTTL, see
// store.go). Version is omitted when empty.
type MachineSummary struct {
	DeviceID   string `json:"device_id"`
	Name       string `json:"name"`
	Platform   string `json:"platform"`
	Version    string `json:"version,omitempty"`
	LastSeenAt int64  `json:"last_seen_at"`
	Online     bool   `json:"online"`
}

// ProjectEntry is one folder open in the host's Dragent sidebar
// (`dragent projects list`: {name, path}). ID is optional: when supplied it
// is the stable identity other entries reference; when absent the entry is
// legacy and no tab placement may be guessed from it.
type ProjectEntry struct {
	ID   *string `json:"id,omitempty"`
	Name string  `json:"name"`
	Path string  `json:"path"`
}

// WorkspaceEntry is one sidebar workspace (`dragent workspaces list`).
// Branch is nil when the workspace has no git branch (e.g. a --local
// checkout); Project is the legacy session-folder project path and carries
// no membership semantics. ID and ProjectID are the optional stable
// identities: when ProjectID is supplied it must reference a published
// project ID, otherwise the workspace is legacy.
type WorkspaceEntry struct {
	ID        *string `json:"id,omitempty"`
	Name      string  `json:"name"`
	Path      string  `json:"path"`
	Branch    *string `json:"branch,omitempty"`
	Project   *string `json:"project,omitempty"`
	ProjectID *string `json:"project_id,omitempty"`
}

// TabEntry is one workspace tab (`dragent panes list` panes: id, kind,
// title, active/visible). Kind is one of terminal, browser, settings, agent.
// WorkspaceID/ProjectID are optional placement references into the published
// project/workspace IDs; when absent the tab is legacy and its placement
// must not be guessed. AgentKind/AgentState further describe an agent tab
// only and must be absent otherwise.
type TabEntry struct {
	ID          uint64  `json:"id"`
	Kind        string  `json:"kind"`
	Title       string  `json:"title,omitempty"`
	Active      bool    `json:"active,omitempty"`
	Visible     bool    `json:"visible,omitempty"`
	WorkspaceID *string `json:"workspace_id,omitempty"`
	ProjectID   *string `json:"project_id,omitempty"`
	AgentKind   *string `json:"agent_kind,omitempty"`
	AgentState  *string `json:"agent_state,omitempty"`
	Attention   *string `json:"attention,omitempty"`
}

// Catalog is the host's whole-machine listing a same-account controller may
// read. Empty (not nil) slices encode as [] so mobile decoders see arrays.
// Machine is optional metadata; legacy publishers omit it and remain valid.
type Catalog struct {
	Machine    *MachineInfo     `json:"machine,omitempty"`
	Projects   []ProjectEntry   `json:"projects"`
	Workspaces []WorkspaceEntry `json:"workspaces"`
	Tabs       []TabEntry       `json:"tabs"`
}

// ownerKey scopes a catalog to one account's device. It is the same
// construction as the session forward owner
// (server/internal/session/api.forwardOwnerID): user id first, so different
// accounts never share an owner even for the same device name.
func ownerKey(userID, deviceID string) (string, bool) {
	deviceID = strings.TrimSpace(deviceID)
	if userID == "" || deviceID == "" || len(deviceID) > 128 {
		return "", false
	}
	return "device\x00" + userID + "\x00" + deviceID, true
}

// Validate checks a host-published catalog before it is stored. Unknown
// entries are rejected rather than silently dropped so a host/gateway skew
// surfaces instead of presenting a partial machine. Legacy catalogs without
// relationship IDs are accepted; enriched catalogs with IDs must reference
// only published IDs and must agree where both sides declare membership —
// the relay never guesses placement by array order.
func (c Catalog) Validate() error {
	if c.Machine != nil {
		if err := c.Machine.validate(); err != nil {
			return err
		}
	}
	if len(c.Projects) > maxProjects {
		return errors.New("too many projects")
	}
	for i := range c.Projects {
		if err := c.Projects[i].validate(); err != nil {
			return err
		}
	}
	if len(c.Workspaces) > maxWorkspaces {
		return errors.New("too many workspaces")
	}
	for i := range c.Workspaces {
		if err := c.Workspaces[i].validate(); err != nil {
			return err
		}
	}
	if len(c.Tabs) > maxTabs {
		return errors.New("too many tabs")
	}
	for i := range c.Tabs {
		if err := c.Tabs[i].validate(); err != nil {
			return err
		}
	}
	if err := c.validateRelationships(); err != nil {
		return err
	}
	return nil
}

// validateRelationships checks the optional ID graph. Absent IDs are legacy
// and impose nothing; present IDs must be unique per section, must resolve
// to a published entry, and a tab naming both a workspace and a project
// must agree with that workspace's declared project.
func (c Catalog) validateRelationships() error {
	projectIDs := make(map[string]bool, len(c.Projects))
	for i := range c.Projects {
		if c.Projects[i].ID == nil {
			continue
		}
		id := strings.TrimSpace(*c.Projects[i].ID)
		if projectIDs[id] {
			return errors.New("duplicate project id")
		}
		projectIDs[id] = true
	}
	workspaceIDs := make(map[string]bool, len(c.Workspaces))
	workspaceProject := make(map[string]*string, len(c.Workspaces))
	for i := range c.Workspaces {
		if c.Workspaces[i].ID == nil {
			continue
		}
		id := strings.TrimSpace(*c.Workspaces[i].ID)
		if workspaceIDs[id] {
			return errors.New("duplicate workspace id")
		}
		workspaceIDs[id] = true
		if c.Workspaces[i].ProjectID != nil {
			project := strings.TrimSpace(*c.Workspaces[i].ProjectID)
			workspaceProject[id] = &project
		}
	}
	for i := range c.Workspaces {
		if c.Workspaces[i].ProjectID == nil {
			continue
		}
		id := strings.TrimSpace(*c.Workspaces[i].ProjectID)
		if !projectIDs[id] {
			return errors.New("workspace references an unknown project")
		}
	}
	for i := range c.Tabs {
		var tabWorkspace *string
		if c.Tabs[i].WorkspaceID != nil {
			id := strings.TrimSpace(*c.Tabs[i].WorkspaceID)
			if !workspaceIDs[id] {
				return errors.New("tab references an unknown workspace")
			}
			tabWorkspace = &id
		}
		var tabProject *string
		if c.Tabs[i].ProjectID != nil {
			id := strings.TrimSpace(*c.Tabs[i].ProjectID)
			if !projectIDs[id] {
				return errors.New("tab references an unknown project")
			}
			tabProject = &id
		}
		if tabWorkspace != nil && tabProject != nil {
			if declared, ok := workspaceProject[*tabWorkspace]; ok && declared != nil && *declared != *tabProject {
				return errors.New("tab project does not match its workspace project")
			}
		}
	}
	return nil
}

// Normalized returns the catalog with surrounding whitespace trimmed and nil
// slices replaced by empty ones (so the wire shape always carries arrays).
func (c Catalog) Normalized() Catalog {
	out := Catalog{
		Projects:   make([]ProjectEntry, 0, len(c.Projects)),
		Workspaces: make([]WorkspaceEntry, 0, len(c.Workspaces)),
		Tabs:       make([]TabEntry, 0, len(c.Tabs)),
	}
	if c.Machine != nil {
		machine := MachineInfo{
			Name:     strings.TrimSpace(c.Machine.Name),
			Platform: strings.TrimSpace(c.Machine.Platform),
			Version:  strings.TrimSpace(c.Machine.Version),
		}
		out.Machine = &machine
	}
	for _, p := range c.Projects {
		p.Name = strings.TrimSpace(p.Name)
		p.Path = strings.TrimSpace(p.Path)
		if p.ID != nil {
			id := strings.TrimSpace(*p.ID)
			p.ID = &id
		}
		out.Projects = append(out.Projects, p)
	}
	for _, w := range c.Workspaces {
		w.Name = strings.TrimSpace(w.Name)
		w.Path = strings.TrimSpace(w.Path)
		if w.Branch != nil {
			branch := strings.TrimSpace(*w.Branch)
			w.Branch = &branch
		}
		if w.Project != nil {
			project := strings.TrimSpace(*w.Project)
			w.Project = &project
		}
		if w.ID != nil {
			id := strings.TrimSpace(*w.ID)
			w.ID = &id
		}
		if w.ProjectID != nil {
			projectID := strings.TrimSpace(*w.ProjectID)
			w.ProjectID = &projectID
		}
		out.Workspaces = append(out.Workspaces, w)
	}
	for _, t := range c.Tabs {
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
	return out
}

// validateOptionalID checks an optional relationship ID. Nil means legacy
// (absent) and is valid; a present ID must be non-blank after trimming,
// bounded, and free of NUL separators. Slashes are allowed: stable
// repository/worktree paths are valid IDs.
func validateOptionalID(v *string) error {
	if v == nil {
		return nil
	}
	trimmed := strings.TrimSpace(*v)
	if trimmed == "" {
		return errors.New("id must not be empty")
	}
	if len(trimmed) > maxCatalogIDLength {
		return errors.New("id is too long")
	}
	if strings.ContainsRune(trimmed, '\x00') {
		return errors.New("id is invalid")
	}
	return nil
}

func (m MachineInfo) validate() error {
	if strings.TrimSpace(m.Name) == "" {
		return errors.New("machine name is required")
	}
	if len(m.Name) > maxMachineFieldLength {
		return errors.New("machine name is too long")
	}
	if strings.TrimSpace(m.Platform) == "" {
		return errors.New("machine platform is required")
	}
	if len(m.Platform) > maxMachineFieldLength {
		return errors.New("machine platform is too long")
	}
	if strings.TrimSpace(m.Version) != "" && len(strings.TrimSpace(m.Version)) > maxMachineFieldLength {
		return errors.New("machine version is too long")
	}
	if strings.ContainsRune(m.Name, '\x00') || strings.ContainsRune(m.Platform, '\x00') || strings.ContainsRune(m.Version, '\x00') {
		return errors.New("machine metadata is invalid")
	}
	return nil
}

func (p ProjectEntry) validate() error {
	if err := validateOptionalID(p.ID); err != nil {
		return err
	}
	if strings.TrimSpace(p.Name) == "" {
		return errors.New("project name is required")
	}
	if len(p.Name) > maxNameLength {
		return errors.New("project name is too long")
	}
	if strings.TrimSpace(p.Path) == "" {
		return errors.New("project path is required")
	}
	if len(p.Path) > maxPathLength {
		return errors.New("project path is too long")
	}
	return nil
}

func (w WorkspaceEntry) validate() error {
	if err := validateOptionalID(w.ID); err != nil {
		return err
	}
	if err := validateOptionalID(w.ProjectID); err != nil {
		return err
	}
	if strings.TrimSpace(w.Name) == "" {
		return errors.New("workspace name is required")
	}
	if len(w.Name) > maxNameLength {
		return errors.New("workspace name is too long")
	}
	if strings.TrimSpace(w.Path) == "" {
		return errors.New("workspace path is required")
	}
	if len(w.Path) > maxPathLength {
		return errors.New("workspace path is too long")
	}
	if w.Branch != nil && len(*w.Branch) > maxBranchLength {
		return errors.New("workspace branch is too long")
	}
	if w.Project != nil {
		if strings.TrimSpace(*w.Project) == "" {
			return errors.New("workspace project must not be empty")
		}
		if len(*w.Project) > maxPathLength {
			return errors.New("workspace project is too long")
		}
	}
	return nil
}

func (t TabEntry) validate() error {
	if t.ID > maxSafeJSInteger {
		return errors.New("tab id exceeds the supported range")
	}
	switch strings.TrimSpace(t.Kind) {
	case TabKindTerminal, TabKindBrowser, TabKindSettings, TabKindAgent:
	default:
		return errors.New("tab kind must be terminal, browser, settings, or agent")
	}
	if len(t.Title) > maxTitleLength {
		return errors.New("tab title is too long")
	}
	if err := validateOptionalID(t.WorkspaceID); err != nil {
		return errors.New("tab workspace id is invalid")
	}
	if err := validateOptionalID(t.ProjectID); err != nil {
		return errors.New("tab project id is invalid")
	}
	// Agent identity is explicit: agent fields ride only on agent tabs.
	// Anything else keeps the terminal default (unknown agents stay
	// terminal rather than being guessed).
	if strings.TrimSpace(t.Kind) != TabKindAgent {
		if t.AgentKind != nil || t.AgentState != nil || t.Attention != nil {
			return errors.New("agent fields require an agent tab")
		}
		return nil
	}
	if t.AgentKind != nil {
		kind := strings.TrimSpace(*t.AgentKind)
		if kind == "" {
			return errors.New("agent kind must not be empty")
		}
		if len(kind) > maxAgentKindLength {
			return errors.New("agent kind is too long")
		}
		if strings.ContainsRune(kind, '\x00') {
			return errors.New("agent kind is invalid")
		}
	}
	if t.AgentState != nil {
		switch strings.TrimSpace(*t.AgentState) {
		case AgentStateIdle, AgentStateWorking, AgentStateReview:
		default:
			return errors.New("agent state must be idle, working, or review")
		}
	}
	if t.Attention != nil {
		switch strings.TrimSpace(*t.Attention) {
		case AttentionNeedsInput, AttentionFailed:
		default:
			return errors.New("attention must be needs_input or failed")
		}
	}
	return nil
}

// ---------------------------------------------------------------------------
// Sealed catalog blob (§6.5): the only catalog shape the server stores after
// the cut-over. The desktop encrypts the catalog JSON once with a random K
// (XChaCha20-Poly1305, AD = "dragent catalog v1" || account || desktop ||
// version) and wraps K || version for each paired phone with Noise X.
// The relay is a blind router: it sees routing only — desktop id,
// recipient phone ids, blob version, padded size, publish times — and never
// any catalog field (no machine name, projects, paths, branches, tab
// titles, or agent states).
//
// Blob, padded to 4 KiB buckets:
//
//	{"v":1,"desktop":"<id>","version":N,"recipients":[{"phone":"<id>","wrap":"<b64>"}],
//	 "nonce":"<b64>","body":"<b64>"}
//
// The phone checks the Noise X sender is the pinned desktop key, the
// versions agree, and version > high_water (§6.5). Same version with a
// different body is rejected and flagged.
const (
	// sealedCatalogVersion is the only blob version the relay accepts.
	sealedCatalogVersion = 1
	// maxSealedVersion is the u64 cap shared with the desktop (§6.5:
	// version ≤ 2^53 − 1, never reused).
	maxSealedVersion = 9007199254740991
	// maxSealedRecipients caps the phones one blob may wrap for.
	maxSealedRecipients = 32
	// sealedNonceBytes is the XChaCha20-Poly1305 nonce length.
	sealedNonceBytes = 24
	// sealedBodyBucket is the padding quantum: decoded bodies are a
	// multiple of 4 KiB.
	sealedBodyBucket = 4096
	// maxSealedBodyBytes caps one decoded sealed body (256 buckets = 1 MiB).
	// The live catalog travels inside the KK channel; at rest it is a small
	// listing, never a dump.
	maxSealedBodyBytes = 256 * sealedBodyBucket
	// maxSealedWrapBytes caps one decoded key wrap (Noise X of a 40-byte
	// K || version is ~136 bytes; the cap leaves headroom, not content).
	maxSealedWrapBytes = 1024
)

// SealedRecipient is one phone the blob is wrapped for: routing ids only.
// The wrap is opaque ciphertext to the relay.
type SealedRecipient struct {
	Phone string `json:"phone"`
	Wrap  string `json:"wrap"`
}

// SealedCatalog is the opaque at-rest blob. Every field is routing or
// ciphertext; none carries content.
type SealedCatalog struct {
	V          int               `json:"v"`
	Desktop    string            `json:"desktop"`
	Version    uint64            `json:"version"`
	Recipients []SealedRecipient `json:"recipients"`
	Nonce      string            `json:"nonce"`
	Body       string            `json:"body"`
}

// decodeSealedB64 accepts standard or URL base64, padded or raw: desktops
// emit base64url, and the relay must not reject a valid wrap on alphabet.
func decodeSealedB64(s string) ([]byte, error) {
	for _, enc := range []*base64.Encoding{
		base64.StdEncoding, base64.URLEncoding,
		base64.RawStdEncoding, base64.RawURLEncoding,
	} {
		if b, err := enc.DecodeString(strings.TrimSpace(s)); err == nil {
			return b, nil
		}
	}
	return nil, errors.New("invalid base64")
}

// Validate checks routing only: shape, ids, version, and opaque sizes. It
// never decrypts and never sees content.
//
// A blob sealed for nobody (0 recipients) is valid: a new desktop with no
// paired phones yet must still prove it published, and the body stays
// opaque. The cap is 0..maxSealedRecipients.
func (b SealedCatalog) Validate() error {
	if b.V != sealedCatalogVersion {
		return errors.New("unsupported sealed catalog version")
	}
	if !validSealedDeviceID(b.Desktop) {
		return errors.New("invalid desktop")
	}
	if b.Version == 0 || b.Version > maxSealedVersion {
		return errors.New("invalid catalog version")
	}
	if len(b.Recipients) > maxSealedRecipients {
		return errors.New("invalid recipients")
	}
	seen := make(map[string]bool, len(b.Recipients))
	for i := range b.Recipients {
		phone := strings.TrimSpace(b.Recipients[i].Phone)
		if !validSealedDeviceID(phone) {
			return errors.New("invalid recipient")
		}
		if seen[phone] {
			return errors.New("duplicate recipient")
		}
		seen[phone] = true
		wrap, err := decodeSealedB64(b.Recipients[i].Wrap)
		if err != nil || len(wrap) == 0 || len(wrap) > maxSealedWrapBytes {
			return errors.New("invalid wrap")
		}
	}
	nonce, err := decodeSealedB64(b.Nonce)
	if err != nil || len(nonce) != sealedNonceBytes {
		return errors.New("invalid nonce")
	}
	body, err := decodeSealedB64(b.Body)
	if err != nil || len(body) == 0 || len(body) > maxSealedBodyBytes || len(body)%sealedBodyBucket != 0 {
		return errors.New("invalid body")
	}
	return nil
}

// Normalized trims routing whitespace; ciphertext is byte-exact.
func (b SealedCatalog) Normalized() SealedCatalog {
	out := SealedCatalog{V: b.V, Desktop: strings.TrimSpace(b.Desktop), Version: b.Version}
	out.Recipients = make([]SealedRecipient, 0, len(b.Recipients))
	for _, r := range b.Recipients {
		out.Recipients = append(out.Recipients, SealedRecipient{
			Phone: strings.TrimSpace(r.Phone),
			Wrap:  strings.TrimSpace(r.Wrap),
		})
	}
	out.Nonce = strings.TrimSpace(b.Nonce)
	out.Body = strings.TrimSpace(b.Body)
	return out
}

// cloned deep-copies the recipient slice.
func (b SealedCatalog) cloned() SealedCatalog {
	out := b.Normalized()
	return out
}

func validSealedDeviceID(id string) bool {
	if len(id) != 36 {
		return false
	}
	parsed, err := uuid.Parse(strings.TrimSpace(id))
	return err == nil && parsed.String() == strings.TrimSpace(id)
}

// SealedMachineSummary is the routing-only discovery row (§2.2): desktop
// id, online, last seen, blob version, padded size. No names, no platforms,
// no content.
type SealedMachineSummary struct {
	DeviceID    string `json:"device_id"`
	Online      bool   `json:"online"`
	LastSeenAt  int64  `json:"last_seen_at"`
	BlobVersion uint64 `json:"blob_version"`
	BlobSize    int    `json:"blob_size"`
}

// SealedStore persists one sealed blob per device-scoped owner. It is the
// only catalog store written after the cut-over; the plaintext Store stays
// for E2a socket/host admission compatibility and is never fed content.
type SealedStore interface {
	PutSealed(ctx context.Context, owner string, blob SealedCatalog) error
	GetSealed(ctx context.Context, owner string) (SealedCatalog, bool, error)
	ListSealed(ctx context.Context, userID string) ([]SealedMachineSummary, error)
}

// MemorySealedStore is an in-memory SealedStore.
type MemorySealedStore struct {
	mu      sync.Mutex
	blobs   map[string]sealedEntry
	now     func() time.Time
	maxDevs int
}

type sealedEntry struct {
	userID   string
	deviceID string
	blob     SealedCatalog
	bodySize int
	lastSeen time.Time
}

// NewMemorySealedStore returns an empty MemorySealedStore.
func NewMemorySealedStore() *MemorySealedStore {
	return &MemorySealedStore{blobs: make(map[string]sealedEntry)}
}

// SetClock injects the lease clock (tests).
func (s *MemorySealedStore) SetClock(fn func() time.Time) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.now = fn
}

func (s *MemorySealedStore) currentTimeLocked() time.Time {
	if s.now != nil {
		return s.now()
	}
	return time.Now()
}

func (s *MemorySealedStore) PutSealed(ctx context.Context, owner string, blob SealedCatalog) error {
	userID, deviceID, ok := splitOwner(owner)
	if !ok {
		return errInvalidOwner
	}
	blob = blob.Normalized()
	if err := blob.Validate(); err != nil {
		return err
	}
	if !strings.EqualFold(blob.Desktop, deviceID) {
		return errors.New("blob desktop does not match device")
	}
	body, _ := decodeSealedB64(blob.Body)
	if err := ctx.Err(); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.currentTimeLocked()
	s.blobs[owner] = sealedEntry{
		userID: userID, deviceID: deviceID,
		blob: blob, bodySize: len(body), lastSeen: now,
	}
	s.evictLocked(userID, owner)
	return nil
}

func (s *MemorySealedStore) GetSealed(_ context.Context, owner string) (SealedCatalog, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	e, ok := s.blobs[owner]
	if !ok {
		return SealedCatalog{}, false, nil
	}
	return e.blob.cloned(), true, nil
}

func (s *MemorySealedStore) ListSealed(_ context.Context, userID string) ([]SealedMachineSummary, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.currentTimeLocked()
	out := []SealedMachineSummary{}
	for _, e := range s.blobs {
		if e.userID != userID {
			continue
		}
		out = append(out, SealedMachineSummary{
			DeviceID:    e.deviceID,
			Online:      now.Sub(e.lastSeen) <= machineOnlineTTL,
			LastSeenAt:  e.lastSeen.UnixMilli(),
			BlobVersion: e.blob.Version,
			BlobSize:    e.bodySize,
		})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].DeviceID < out[j].DeviceID })
	return out, nil
}

func (s *MemorySealedStore) evictLocked(userID, keepOwner string) {
	limit := s.maxDevs
	if limit <= 0 {
		limit = maxMachinesPerAccount
	}
	for {
		var oldest string
		var oldestSeen time.Time
		count, first := 0, true
		for owner, e := range s.blobs {
			if e.userID != userID {
				continue
			}
			count++
			if owner == keepOwner {
				continue
			}
			if first || e.lastSeen.Before(oldestSeen) {
				oldest, oldestSeen, first = owner, e.lastSeen, false
			}
		}
		if count <= limit || oldest == "" {
			return
		}
		delete(s.blobs, oldest)
	}
}

var _ SealedStore = (*MemorySealedStore)(nil)
