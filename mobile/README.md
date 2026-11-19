# Dragent Mobile

An original Expo / React Native companion for Dragent. The phone never signs
in: the signed-in desktop shows a pairing QR in Settings → Mobile, the phone
scans it once (or pastes the code) and is paired. Transport is the relay;
same-Wi-Fi direct is phase 2.

## Implemented

- Phone pairing end to end: `/pair-scan` (camera plus "Paste code or link"
  fallback), `/pair-confirm` (phone name, Connect, explicit expired/used,
  rate-limited and offline states), and the `dragent://pair?c=…&g=…` deep
  link. The single-use code redeems for device credentials bound to the
  desktop's user and organisation.
- A device credential store plus controller replacing account sign-in:
  SecureStore-held device token, rotating token refresh (single-flight),
  reuse-of-rotated-token revocation, and revoked/expired credentials falling
  back to the pair screen. The existing relay socket and 24h companion
  session code resolve the device access token wherever they used the portal
  account token.
- A home overview of paired desktops with the live machine/project/
  workspace/tab hierarchy, and a pairing empty state (Open desktop Settings →
  Mobile, Scan the code, You're connected).
- Shared presentation components, responsive screen layouts, and navigation
  that switches between a desktop/tablet sidebar and phone bottom navigation.
- Four application themes: **Light, Dark, Gruvbox Light, Gruvbox Dark**.
  Gruvbox palettes come from the desktop source; neutral light/dark chrome
  uses one semantic token system. Navigation, forms, feedback and terminal
  surfaces share the active theme. Appearance selection survives restart.
- A single realtime client with admission deadlines, keepalive, reconnect,
  foreground/network recovery, stale-socket guards, and no replay of user
  input.
- Native styled command history and structured Agent conversation, with
  shared disclosures for thoughts/tools and read-only host requests. A local
  WebView reuses the desktop terminal WASM emulator for contiguous raw output
  and authoritative alternate-screen grids, without its own network
  connection.
- Edit sessions can run commands, explicitly send text and terminal keys, ask
  the agent questions, and request a stop. Unsent drafts survive reconnects;
  switching sessions clears them. Input is never queued or replayed.
- Ordered terminal reduction with coalesced UI snapshots. Outgoing text uses
  one UTF-8 byte-validation boundary shared by the transport and composers.
- A development-only companion preview following the desktop hierarchy:
  desktops contain projects, projects contain workspaces with their own
  branches, and workspaces contain Agent, Terminal and Browser tabs. Dragent
  is a built-in Agent option alongside the sample external agents. Each tab
  owns its state and a full content canvas. Each machine offers Project, Agent
  coordination, Automation, Code review and Tasks areas. Files and checklists
  belong to workspaces. Machine-wide Code review opens a PR/MR inbox with
  Overview, Files, Commits, Checks and Activity; workspace changes stay
  separate.

This is not yet the complete planned companion. Full terminal
selection/clipboard/device interaction, live tasks/files/git data, voice,
home-screen widgets and release automation remain planned. APNs/FCM
transports are implemented; provisioned-device delivery and durable
event-source integration still require validation. Explicit foreground Live
Activity tracking and local notification tests are implemented for native
development builds; Expo Go and web expose their unsupported capabilities.

There is no phone sign-in, no portal browser auth, no share-link flow, no
saved sessions and no user-facing connection settings. Settings keeps paired
desktops (rename/forget), notifications, theme and about.

## Relay integration status

The authenticated machine data channel is pairing-scoped. `RelaySocketClient`
connects to `/v1/relay/socket`, sends a freshly resolved device JWT in its
first frame, then uses the shared correlated RPC client and strict method
readers. Tokens never enter URLs. Disconnects fail pending calls immediately;
only subscriptions resume after authenticated admission. Calls and mutations
are never automatically replayed. `useRelaySocket` belongs at a machine
boundary, where child screens share its `client.rpc`; the live catalog rides
the `catalog` subscription (`subscribeMachineCatalog`) over that channel.

Companion sessions have a fixed 24-hour server deadline. The first
authenticated socket creates a lease; subsequent connections send its
`resume_session_id`. Neither reconnect nor access-token refresh extends that
deadline. Production stores leases in Redis; tests use a bounded memory
store. The phone persists the lease in device/gateway/desktop-scoped secure
storage (memory on web), refreshes credentials before access expiry, and
resumes after network changes. Expired or revoked leases stop automatic
recovery; **Start new session** is an explicit user action. Forgetting the
pairing attempts device revocation before clearing device credentials and
always closes local channels. Desktop processes are unaffected.

The Go API mounts the socket, HTTP RPC/state publish, catalog and
push-registry routes. Catalog/state stores are process-local in-memory
snapshots (`MemoryStore`/`MemoryHostStore` for single-instance dev and
tests; production persistent storage is separate work and is not claimed
here).
The native desktop publisher is implemented: the UI thread collects cheap
in-memory state only (`desktop_catalog.rs: note_rendered`, no network/disk)
and a background worker publishes via the shared `API_CLIENT`
(`desktop_publisher.rs`, `<=15s` heartbeat, latest-only sends, pairing-scoped
owner, explicit window-close/sign-out retirement). It publishes real machine
metadata plus projects, workspaces with branches, and tabs with
workspace/project membership and explicit `agent_kind`/`agent_state`. No
credentials, transcripts, file bodies or share secrets are published.
Live hooks are mounted on real routes: Home `MachinesSection` discovers
desktops via pairing-scoped `machines.list` (`useMachines`, foreground
refresh plus `15s` poll), and `app/machine/[deviceId]/_layout` owns one
targeted socket per desktop (`MachineProvider`) whose `rpc` plus `catalog`
subscription (`subscribeMachineCatalog`: seq 1 snapshot, then full
replacements) backs `MachineScreen` and the project (`project/[projectId]`),
workspace (`workspace/[workspaceId]`) and tab (`tab/[tabId]`) routes. Device
controls are disabled while offline; failures never substitute preview data.

Agent coordination also reads live desktop teams through `coordination.list`
and `coordination.get`. The existing desktop daemon remains the source of
truth: the phone shows bounded team summaries, task progress and recent
messages for published projects. These screens reuse preview presentation
while keeping live data and read-only behavior separate. Native reads have
one shared eight-second budget and verify the captured pairing identity
before and after daemon I/O. Automation, code-review and standalone Tasks
integration remain unfinished.
Terminal and agent tabs now attach to the existing desktop daemon terminal.
The desktop opens an authenticated outbound `/v1/relay/host` socket; the
relay brokers `tabs.attach`, `tabs.read`, `tabs.screen`, `tabs.input`,
`tabs.resize` and `tabs.detach`. Handles are bound to pairing, desktop,
companion lease and phone connection, with four attachments per lease. Host
reconnects invalidate old handles; input is never replayed. Mobile polls
sequentially while visible and connected, feeding the existing WASM runtime.
A byte gap or mobile history limit switches to authoritative screen
snapshots, without fabricated scrollback. Detach and session expiry release
access without killing desktop processes.
The shared composer offers exact input and explicit terminal keys; semantic
command execution and agent chat adapters remain separate work. Browser
forwarding and the coordination/automation/review/task adapters remain
unavailable on live routes. `tabs.createTerminal` and `worktree.create`
require a connected authenticated host executor and otherwise fail closed
(never synthesize a resource); provider searches likewise require an
owner-scoped host adapter, with no cloud CLI or provider-token fallback.
APNs and FCM senders now implement authenticated HTTP delivery with bounded
requests and sanitized provider errors. The phone has an explicit native
push opt-in; enrollment uses the verified device claim, foreground renewal
and token-rotation handling. Forgetting the pairing attempts unregister
before credential cleanup. Web and Expo Go report unsupported
capabilities. Provider credentials, provisioning and actual device delivery
still require deployment/device checks; local HTTP fixtures establish
transport behavior, not delivered notifications.

In a live desktop session, **Activity** opens explicit **Track activity** /
**Stop tracking**, notification permission and **Test notification** controls.
Tracking never starts on connection. Lock-screen payloads contain generic
status and local identity, not transcript text, commands or credentials.
Background/disconnected tracking becomes stale; automatic completion alerts
and remote Live Activity updates are unavailable. Standard push opt-in is
separate from these local activity controls. These controls need a native
development build and device verification, not just a successful JavaScript
export.

See [ARCHITECTURE.md](ARCHITECTURE.md) for ownership and implementation boundaries.

## Install and run

Use Node 22+ and the pnpm version pinned in `package.json`:

```sh
cd mobile
pnpm install
pnpm start
```

Other entry points:

```sh
pnpm ios
pnpm android
pnpm web
pnpm mock-server
```

The app opens unpaired. Pair it from your desktop: open Settings → Mobile,
then scan the QR with the phone (`/pair-scan`) or paste the code. There are
no portal credentials and no connection settings on the phone. The only
override is a development gateway for the mock:

```sh
EXPO_PUBLIC_GATEWAY_URL=http://127.0.0.1:6769
```

Development builds allow HTTP/WS for local testing; production requires
HTTPS/WSS.

## Local desktop preview

In a development build, open **Settings → Developer → Open preview** to
explore the sample desktops. With the web server running, open `/preview`
directly. Choose a desktop, choose a project, then open a workspace and one
of its tabs. Each workspace shows its own branch. Create a sample workspace
with a name and branch, or add more Agent, Terminal and Browser tabs within
an existing workspace. Agent choices include **Dragent (built-in), Codex and
OpenCode**. Multiple tabs of the same type retain independent output,
conversations or browser history.

Selecting a machine opens five areas in desktop product order: **Project,
Agent coordination, Automation, Code review, Tasks**. Coordination mirrors
the desktop's All/Active/Finished list and Overview/Activity detail:
lead, worker plan, dependencies, parallelism and messages. Automation offers
Mine, Team and Inbox scopes, with Runs/Settings detail, local enablement and
fixed sample run results. Team has an explicit empty state until teammate
data exists. Schedules are read-only and no desktop job is launched. Tasks
has an explicit planned empty state until the desktop task-management
contract exists; it is separate from workspace checklists.

A terminal fills the available height beneath the compact navigation and
input dock. Agent conversations have their own tab and composer. Browser
tabs show original local example pages with independent back/forward history
and reload controls; they do not load external sites or mirror a real
desktop browser. The machine's Code review area follows the desktop PR/MR
inbox: project, state, scope and search filters open Overview, Files,
Commits, Checks and Activity detail. Wide screens show the inbox and detail
together. Viewed files and review drafts belong to the request and its exact
head commit. Creating a request creates a local sample; approval and change
requests save local drafts. No provider review is published or merged.
Workspace tools open separate local changes with their own notes and
progress, reusing the diff presentation. Files and checklists belong to the
selected workspace.

This preview uses original sample data held in memory. Commands and agent
replies are simulated locally; files and diffs do not come from your disk.
Nothing is sent to a shell, model or desktop. Preview actions do not change
pairing credentials. **Reset preview** restores the samples; reloading
discards its changes. The preview routes are unavailable in production
builds.

## Local mock

`pnpm mock-server` listens on `http://127.0.0.1:6769` (HTTP pairing API) plus
`ws://127.0.0.1:6769/v1/relay/socket` (relay admission), bound to loopback.
Point the dev gateway override at it:

```sh
EXPO_PUBLIC_GATEWAY_URL=http://127.0.0.1:6769
```

Create a single-use code (desktop fixture bearer) and redeem it from
`/pair-scan` → paste, or drive the contract directly:

```sh
curl -X POST http://127.0.0.1:6769/v1/companion/pairings \
  -H 'Authorization: Bearer mock-desktop-token'
# -> {pairing_id, code, expires_at, gateway_url}
```

The mock implements the pairing contract: single-use 10-minute codes (max 5
live per user, oldest dropped), 410 on used/expired, per-IP and per-code
rate limiting (429), 30-day device tokens stored hashed with rotation on
every `/token` call, reuse-of-rotated-token revocation (401), device revoke
(closes relay sockets), cross-user isolation, and relay-auth admission with
24h leases plus a sample `machines.list` and `catalog` stream. No real
desktop, no real auth, no E2EE; crypto-random fixtures, never executes
shells.

## Credentials and backend contracts

Device credentials use SecureStore with device-only accessibility. The
opaque device token, phone identity, gateway and paired user live in secure
storage keyed by gateway; ordinary preferences contain only theme choice,
desktop display labels and the gateway pointer. The in-memory access token
is resolved fresh per dial; web builds keep credentials in memory, so
refreshing the page requires pairing again.

Pairing codes are single-use, 10-minute, 20-char base32, bound to user and
organisation server-side. Redeem and token rotation are the only
unauthenticated HTTP calls; everything else (devices list/revoke, relay
socket) presents the current device access token.

## Checks and generated contracts

```sh
pnpm typecheck
pnpm lint
pnpm test
pnpm themes:check
pnpm protocol:check
pnpm terminal:check
pnpm exec expo export --platform all
```

After changing canonical desktop palettes or the portal wire types,
regenerate:

```sh
pnpm themes:sync
pnpm protocol:sync
pnpm terminal:sync
```

Generators keep the runtime bundle inside `mobile/`; the app does not import
Rust, backend modules or reference repositories. Tests exercise pairing
(single-use race, expiry, rotation/reuse revocation, revoke, rate limit,
cross-user isolation), admission, recovery, rotation races, credential
isolation, desktop palette parity, terminal bounds and responsive
width/font-scale decisions. Native background behavior, camera access and
accessibility still need device verification before release.

For the actual Go socket ↔ shipping TypeScript client/readers check, with
mobile dependencies installed:

```sh
cd ../server
DRAGENT_TEST_MOBILE_INTEROP=1 go test ./internal/relay -run TestRelaySocketMobileInterop -count=1 -v
```

This uses a loopback test server and disposable fixture identity, performs
real socket admission, catalog/tab reads and ordered event delivery, and
confirms a missing host executor cannot report a successful mutation.
