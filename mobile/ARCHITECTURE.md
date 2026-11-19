# Mobile implementation boundaries

The mobile app uses original code and the existing Dragent backend contracts. The desktop checkout and imported changes are preserved while mobile development proceeds.

- `app/` contains route composition, navigation and native pair link entry
  points (`dragent://pair?c=…&g=…` → `/pair-confirm`).
- `src/components/` owns accessible, themed presentation primitives. Screens
  compose these primitives; they do not recreate buttons, fields or typography.
- `src/layout/` owns window-width and font-scale decisions. Device names and
  operating systems do not decide whether content fits.
- `src/theme/` derives semantic UI and terminal colors from the desktop palette
  catalog. The public palette choices are Light, Dark, Gruvbox Light and
  Gruvbox Dark. A deterministic generator keeps desktop palettes in sync. Runtime
  code consumes mobile data and never imports Rust or development scripts.
- `src/pairing/` owns phone pairing: link parsing, the HTTP pairing API, the
  SecureStore device credential store, the single-flight rotating-token
  controller, and the React provider. It replaces account sign-in; revoked or
  expired credentials fall back to the pair screen.
- `src/machines/` owns desktop discovery (`machines.list`) and the per-desktop
  socket source. The owner identity is the paired user id; pairing-scoped
  leases stay isolated across pairings, gateways and desktops.
- `src/transport/` owns the WebSocket state machine. Screens subscribe to its
  state, rather than implementing reconnect, authorization or parsing again.
  Machine data access uses `RelaySocketClient` and a single reusable
  `RelayRpcClient`. It authenticates before requests, injects the target
  desktop identity, rejects pending calls on disconnect, and resubscribes
  only after fresh admission. `clock.ts` owns the injectable clock shared by
  transport clients and tests.
- `src/storage/` adapts platform persistence behind one small interface. Tests
  inject memory storage; credentials never enter ordinary preference storage.
- `src/features/` composes domain hooks and shared screens into routes.
  Session composers own drafts; the transport owns the shared UTF-8 validation
  boundary. Sending input never appends an implicit Enter or retries a write.
- `src/preview/` owns original sample fixtures and an in-memory preview store.
  Development routes provide this store to desktop/workspace screens.

Extract a reusable unit when responsibilities or real consumers justify it.
Use explicit typed inputs and children/slots instead of many boolean modes.
Keep domain models separate from row rendering: different data can share the
same presentation without acquiring the same business rules.

All application surfaces, navigation, inputs, terminal content and feedback use
semantic theme colors. Controls support font scaling, screen readers, keyboard
insets and at least 44-point touch targets. Layout responds to the available window,
including split screen, rotation and tablets. Long content must wrap or scroll.

Tests focus on pairing (single-use race, expiry, rotation/reuse revocation,
revoke, rate limit, cross-user isolation), contracts, recovery, credential
isolation and responsive bounds. The companion lease lives for 24 hours from
server creation, independently of access-token expiry and socket lifetime.
Reconnect resumes the original lease; Redis reads never renew its TTL.
Session revocation is checked before RPC execution and on idle sockets. The
mobile credential repository serializes lease writes with existing credential
transactions, keeps expired records to prevent silent renewal after restart,
and isolates pairings, gateways and desktops. Explicit forget drains
registered connections with bounded revocation attempts before local closure;
route cleanup only detaches sockets.
Verification includes type checking, lint, platform bundles and browser layout
checks; a browser preview does not establish native-device behavior.

The gateway offers pairing plus a pairing-scoped machine data channel, not
persistent session sharing. Device enrollment is single-use pairing; actual
device push delivery still needs verification. Push leases are mounted
server-side; APNs/FCM use real provider HTTP transports and explicit native
opt-in enrollment. Credentials/provisioning and attention sources beyond the
read-only mirror still need integration and device checks. Do not present
placeholders or untested device delivery as functioning capabilities.

Whole-machine catalog/state publication and host-command execution are
pairing-scoped. The native desktop publisher is implemented
(UI-thread `note_rendered` collect plus background `PUT /v1/relay/catalog`
with `<=15s` heartbeat via `API_CLIENT`); the relay holds host-published
snapshots in process-local `MemoryStore`/`MemoryHostStore`. Catalog tabs carry
workspace/project membership and explicit `agent_kind`/`agent_state` (unknown
agents stay terminal, never inferred). Terminal and agent leaves attach to real
daemon terminals through the authenticated host broker. Their handles bind the
companion lease and physical phone connection; detach never closes a PTY.
Sequential foreground polling feeds the shared WASM runtime. Missing raw history
switches to display-only grid snapshots; it never seeds a parser from a screen
projection. Semantic agent conversation, browser forwarding and other machine
feature adapters remain unfinished. Host routing is process-local; distributed
catalog persistence and cross-instance routing remain separate work.
Preview screens stay sample data on separate `/preview` routes until further
contracts land.

## Visual direction

Use a focused workspace layout: sidebar on wider windows, bottom navigation on
phones, resource rows for desktops, direct functional copy and one primary
action per task. Typography and spacing establish hierarchy. Keep borders subtle,
corners controlled, and color semantic. Do not add decorative orbs, halos,
gradients, marketing slogans, oversized badges, or nested cards to fill space.

The shell owns safe-area insets. A viewport provider measures the actual main
content width after navigation, so responsive decisions use available space.
Screens compose shared fields, buttons, icon actions and text instead of styling
another version of the same control.

## Home overview

The home screen shows paired desktops: live `MachineSummary` entries from
pairing-scoped `machines.list` that open the live machine routes. Discovery
failure never falls back to sample data. The unpaired empty state is the
3-step pairing flow (open desktop Settings → Mobile, scan the code, you're
connected). There are no saved sessions, groups, metrics, resume or quick
actions.

## Local desktop preview

The development-only `/preview` route starts at a desktop picker. Its hierarchy
follows Dragent's desktop model: desktop → project → workspace → tab. Projects
own multiple workspaces; each workspace has its own branch, files, changes and
checklists. Workspaces own independently identified Agent, Terminal and Browser tabs.
Dragent's built-in agent is an Agent choice, alongside sample external agents.
Workspace and tab catalogs are explicit screens, and the open-tab strip contains
only tabs belonging to the selected workspace. Parent/child identifiers are
checked through the full chain before a screen obtains tab state or writers.
A visible Preview label identifies sample data. Reset restores fixtures and
invalidates writers previously issued for any tab.

Selecting a machine opens Project, Agent coordination, Automation, Code review
and Tasks areas in that order. There is no machine-wide Agents shortcut.
The selected area is a route query parameter, preserving keyboard focus when
switching areas and returning from detail routes. Coordination follows the
desktop Teams view's All/Active/Finished and Overview/Activity concepts;
automation follows Mine/Team/Inbox scopes with a Runs/Settings detail flow. Their typed samples and
machine selectors are separate from terminal fixtures. Every linked agent,
workspace and project is checked against its owning machine before access.
Local messages, configuration, sample runs, review progress and notes are bounded
immutable state in the existing preview store. Reset restores every feature.
The live Tasks area follows the desktop's Tasks: Open/Closed/All, assigned-to-me, search, status, priority, assignee, discussion and history.

Tab presentation is separate from fixture generation and state updates. The
companion owns its compact header and open-tab strip instead of inheriting the
global navigation dock. Its continuous terminal canvas fills the
remaining height. Each terminal and agent tab owns its own `TerminalState` and
tab-scoped `SessionWriters`, sharing the real session reduction, span rendering, scrolling
and input-validation boundaries. Canvas and card presentation are explicit
variants of this pipeline. The compact composer uses the same draft and action
logic as the full form. Browser tabs own bounded local-page history and reload
state; the browser preview renderer receives state and callbacks and performs no
network requests. Files and checklists use typed workspace-level views. The
machine Code review area uses a PR/MR inbox and shared request detail renderer
for Overview, Files, Commits, Checks and Activity. Narrow screens navigate to a
request; wide screens place inbox and detail side by side. Core request selectors
validate machine/project/workspace ownership and derive filters and readiness.
Provider-confirmed requirements are necessary for a Ready to merge state. Local
viewed-file state and drafts are keyed by request ID and exact head commit;
provider status never changes when a draft is saved. Sample creation is bounded
and does not create Git objects or provider requests. Workspace changes retain
separate progress and notes. Both use the same diff presentation component with
caller-owned state; no Git mutation is exposed.
Adding live desktop
support uses the pairing-authenticated desktop/project/workspace contract above.
Live terminal leaves
use the authenticated daemon bridge described above. Live Agent coordination
uses `coordination.list/get` over that same machine socket. Native reads reuse
the desktop daemon's team store, restricted to published sidebar projects and
the socket's captured pairing identity (user/organisation). The relay validates the current
lease, host generation and returned project membership. Results are bounded
(100 teams, 60 tasks, 100 recent messages, 512 KiB) and explicitly indicate
truncation. Task prompts, terminal identities and daemon credentials are omitted.
Over the end-to-end encrypted channel the channel itself is the lease, so
the desktop fills in the relay-era `session_id`/`expires_at` when absent.
The mobile list/detail share presentation with preview. While a team plans
or runs, the phone can message its lead (`coordination.send`), stop it
(`coordination.stop`), cancel a task (`coordination.cancel_task`), set
workers at once / auto-start / lead-answers-prompts / cost
(`coordination.configure`) and forget a finished team (`coordination.forget`);
the desktop checks each change against the same owner and sidebar-project
rules as reads, and answers with the team as it is now. A message sent while
the lead's question is open goes as its answer (`reply_to`), as on the
desktop. The detail carries the open question and the phone's tab ids for the
lead's and workers' terminals (mapped on the desktop from agent panes; no
terminal identities leave it). `coordination.create` starts a new team and
`coordination.options` lists the desktop's projects and agents.
`coordination.task_model` moves an unfinished task to a model from the
desktop's bundled list for its agent (or back to the agent's default);
`coordination.new_lead` and `coordination.clean_up` hand the desktop page's
"Start a new lead" and "Clean up" to the window showing the team's project,
which runs the same code (`teams::new_lead_plan`, `teams::cleanup`). The
detail says when each is on offer (`needs_new_lead`, `cleanup` by task and
worktree folder name, never full paths). Foreground polling uses one resource controller and masks retired
owners immediately.

Live Tasks are the organisation's tasks, reached through the paired
desktop's sign-in (`realtime/companion_tasks.rs`): `tasks.list/get/create/
update/delete/restore/comment/comment_edit/comment_delete/link/unlink/
members/options`, only in the channel's organisation, with exact fields,
bounded text and no raw server errors (workspace links never carry their
folder). Starts (`tasks.start`, `coordination.create`;
`realtime/companion_starts.rs`) run on a window's UI thread through the same
new-workspace prompt the desktop uses, in one of the account's sidebar
projects, and answer `{started, reason}`. `tasks.list` filters by status,
words, assignee (`me`, `none` or a member) and label; workspace links on this
desktop carry their folder so the phone can open that workspace. Connected
trackers (`realtime/companion_trackers.rs`): `tasks.trackers`,
`tasks.tracker_items/move/import/create/start`, run through the desktop's
own connections (keys never leave it) and the Tasks page's own functions;
starting an item goes through a window's prompt like a task. Several tasks
start one after another from the phone, each waiting while the desktop is
still opening the one before. Due dates are picked on a month calendar
(local noon of the day), on both the phone and the desktop. The phone polls them every
15 s with the same machine resource. Automation and code review still lack
live adapters; preview routes remain separate development samples.
