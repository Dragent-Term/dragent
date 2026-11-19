package relay

import (
	"context"
	"sort"
	"sync"
	"time"
)

// machineOnlineTTL bounds the machine lease: a desktop is online while the
// relay has seen a publish within the last 30 seconds (contract). Heartbeats
// refresh the lease; anything older reads as offline but keeps its last-known
// snapshot.
const machineOnlineTTL = 30 * time.Second

// maxMachinesPerAccount bounds how many device catalogs one account may
// publish. The catalog is a listing, not storage: beyond the bound the
// least-recently-seen device is evicted so a misbehaving host cannot grow
// the relay without bound.
const maxMachinesPerAccount = 32

// Store persists one validated catalog per device-scoped owner. Production
// may back it with Redis (mirroring server/internal/session/components);
// the zero-dependency MemoryStore covers tests and single-instance dev.
type Store interface {
	Put(ctx context.Context, owner string, catalog Catalog) error
	Get(ctx context.Context, owner string) (Catalog, bool, error)
}

// MachineLister lists one account's machines for discovery (account-scoped:
// callers pass only their own user id, never another account's). Root
// asserts this optionally alongside Store for machines.list.
type MachineLister interface {
	ListMachines(ctx context.Context, userID string) ([]MachineSummary, error)
}

// MachineGetter reads one device-scoped owner's machine summary. Root
// asserts this optionally alongside Store for the catalog subscription.
type MachineGetter interface {
	GetMachine(ctx context.Context, owner string) (MachineSummary, bool, error)
}

// MachineDiscovery is the combined optional machine surface MemoryStore
// implements. Root type-asserts Store to this (or to the single-method
// interfaces above) after ConfigureSocket.
type MachineDiscovery interface {
	MachineLister
	MachineGetter
}

// CatalogSnapshotGetter is the optional atomic snapshot surface: one call
// returns the catalog and its machine summary from the same publication
// under one lock and one clock reading, so a catalog stream can never pair
// a catalog from one publish with a last-seen from another. Root asserts
// this for the catalog subscription.
type CatalogSnapshotGetter interface {
	GetCatalogSnapshot(ctx context.Context, owner string) (Catalog, MachineSummary, bool, error)
}

// Compile-time proof that MemoryStore carries the discovery surface while
// Store itself stays Put/Get for the existing socket path.
var (
	_ Store                 = (*MemoryStore)(nil)
	_ MachineDiscovery      = (*MemoryStore)(nil)
	_ CatalogSnapshotGetter = (*MemoryStore)(nil)
)

// machineEntry is one device's discovery record: the last published machine
// metadata (possibly absent for legacy publishers) plus the relay-assigned
// last-seen time that drives the online flag.
type machineEntry struct {
	userID   string
	deviceID string
	info     MachineInfo
	hasInfo  bool
	lastSeen time.Time
}

// MemoryStore is an in-memory Store.
type MemoryStore struct {
	mu       sync.Mutex
	catalogs map[string]Catalog
	machines map[string]machineEntry
	now      func() time.Time
}

// NewMemoryStore returns an empty MemoryStore.
func NewMemoryStore() *MemoryStore {
	return &MemoryStore{
		catalogs: make(map[string]Catalog),
		machines: make(map[string]machineEntry),
	}
}

// SetClock injects the lease clock (tests). A nil clock restores wall time.
// It must be set before concurrent use; publishes and reads use the injected
// time for last-seen assignment and the online computation.
func (s *MemoryStore) SetClock(fn func() time.Time) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.now = fn
}

func (s *MemoryStore) currentTimeLocked() time.Time {
	if s.now != nil {
		return s.now()
	}
	return time.Now()
}

func (s *MemoryStore) Put(_ context.Context, owner string, catalog Catalog) error {
	userID, deviceID, valid := splitOwner(owner)
	if !valid {
		return errInvalidOwner
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.currentTimeLocked()
	s.catalogs[owner] = catalog.cloned()
	entry, ok := s.machines[owner]
	if !ok {
		entry = machineEntry{userID: userID, deviceID: deviceID}
	}
	if catalog.Machine != nil {
		entry.info = *catalog.Machine
		entry.hasInfo = true
	}
	entry.lastSeen = now
	s.machines[owner] = entry
	s.evictStaleOwnersLocked(entry.userID, owner)
	return nil
}

func (s *MemoryStore) Get(_ context.Context, owner string) (Catalog, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	catalog, ok := s.catalogs[owner]
	if !ok {
		return Catalog{}, false, nil
	}
	// Copy on read: the stored slices must never alias a caller's view, so a
	// reader holding (or mutating) the result cannot corrupt the catalog or
	// race a concurrent publish (which replaces the stored entry wholesale).
	return catalog.cloned(), true, nil
}

// ListMachines returns the calling account's machines only. Foreign devices
// never appear: entries are filtered by the user id embedded in the owner,
// not by device-name matching. Results sort by device id for a stable wire
// order and encode as [] (never null). Offline devices keep their last-known
// metadata with online=false.
func (s *MemoryStore) ListMachines(_ context.Context, userID string) ([]MachineSummary, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.currentTimeLocked()
	machines := []MachineSummary{}
	for owner, entry := range s.machines {
		if entry.userID == "" || entry.userID != userID {
			continue
		}
		deviceID := entry.deviceID
		if deviceID == "" {
			if _, device, ok := splitOwner(owner); ok {
				deviceID = device
			}
		}
		machines = append(machines, summarizeMachine(deviceID, entry.info, entry.hasInfo, entry.lastSeen, now))
	}
	sort.Slice(machines, func(i, j int) bool { return machines[i].DeviceID < machines[j].DeviceID })
	return machines, nil
}

// GetMachine returns one device-scoped owner's summary with the online flag
// computed from the injected lease clock. Unknown owners read as absent.
func (s *MemoryStore) GetMachine(_ context.Context, owner string) (MachineSummary, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.currentTimeLocked()
	entry, ok := s.machines[owner]
	if !ok {
		return MachineSummary{}, false, nil
	}
	deviceID := entry.deviceID
	if deviceID == "" {
		if _, device, valid := splitOwner(owner); valid {
			deviceID = device
		}
	}
	return summarizeMachine(deviceID, entry.info, entry.hasInfo, entry.lastSeen, now), true, nil
}

// GetCatalogSnapshot returns one owner's catalog and machine summary from
// the same publication: a single lock and a single clock reading cover both,
// so the pair can never mix a catalog from one publish with a last-seen from
// another. Unknown owners read as absent. Returned catalogs are deep copies
// (see Get); summaries are values.
func (s *MemoryStore) GetCatalogSnapshot(_ context.Context, owner string) (Catalog, MachineSummary, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.currentTimeLocked()
	catalog, ok := s.catalogs[owner]
	if !ok {
		return Catalog{}, MachineSummary{}, false, nil
	}
	entry, ok := s.machines[owner]
	if !ok {
		return Catalog{}, MachineSummary{}, false, nil
	}
	deviceID := entry.deviceID
	if deviceID == "" {
		if _, device, valid := splitOwner(owner); valid {
			deviceID = device
		}
	}
	return catalog.cloned(), summarizeMachine(deviceID, entry.info, entry.hasInfo, entry.lastSeen, now), true, nil
}

// evictStaleOwnersLocked keeps one account's device count bounded by
// dropping its least-recently-seen owners. The just-published owner is never
// evicted. Callers hold s.mu.
func (s *MemoryStore) evictStaleOwnersLocked(userID, keepOwner string) {
	if userID == "" {
		return
	}
	for {
		var oldest string
		var oldestSeen time.Time
		count := 0
		first := true
		for owner, entry := range s.machines {
			if entry.userID != userID {
				continue
			}
			count++
			if owner == keepOwner {
				continue
			}
			if first || entry.lastSeen.Before(oldestSeen) {
				oldest, oldestSeen, first = owner, entry.lastSeen, false
			}
		}
		if count <= maxMachinesPerAccount || oldest == "" {
			return
		}
		delete(s.machines, oldest)
		delete(s.catalogs, oldest)
	}
}

func summarizeMachine(deviceID string, info MachineInfo, hasInfo bool, lastSeen, now time.Time) MachineSummary {
	summary := MachineSummary{DeviceID: deviceID}
	if hasInfo {
		summary.Name = info.Name
		summary.Platform = info.Platform
		summary.Version = info.Version
	}
	if !lastSeen.IsZero() {
		summary.LastSeenAt = lastSeen.UnixMilli()
		summary.Online = now.Sub(lastSeen) <= machineOnlineTTL
	}
	return summary
}

// cloned deep-copies the catalog: slices, the machine metadata, and the
// workspace branch/project/id strings a caller could otherwise mutate
// through the returned pointers.
func (c Catalog) cloned() Catalog {
	out := Catalog{
		Projects:   append([]ProjectEntry{}, c.Projects...),
		Workspaces: make([]WorkspaceEntry, 0, len(c.Workspaces)),
		Tabs:       append([]TabEntry{}, c.Tabs...),
	}
	if c.Machine != nil {
		machine := *c.Machine
		out.Machine = &machine
	}
	for i := range out.Projects {
		if out.Projects[i].ID != nil {
			id := *out.Projects[i].ID
			out.Projects[i].ID = &id
		}
	}
	for _, w := range c.Workspaces {
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
		out.Workspaces = append(out.Workspaces, w)
	}
	for i := range out.Tabs {
		if out.Tabs[i].WorkspaceID != nil {
			id := *out.Tabs[i].WorkspaceID
			out.Tabs[i].WorkspaceID = &id
		}
		if out.Tabs[i].ProjectID != nil {
			id := *out.Tabs[i].ProjectID
			out.Tabs[i].ProjectID = &id
		}
		if out.Tabs[i].AgentKind != nil {
			kind := *out.Tabs[i].AgentKind
			out.Tabs[i].AgentKind = &kind
		}
		if out.Tabs[i].AgentState != nil {
			state := *out.Tabs[i].AgentState
			out.Tabs[i].AgentState = &state
		}
		if out.Tabs[i].Attention != nil {
			attention := *out.Tabs[i].Attention
			out.Tabs[i].Attention = &attention
		}
	}
	return out
}
