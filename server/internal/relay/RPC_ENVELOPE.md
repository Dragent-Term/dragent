# Relay RPC envelope (R0 spec — all relay workers build against this)

Everything the phone reads through the relay — tabs, workspaces, files,
provider work items, notifications — travels as correlated RPC on the relay
socket, never as fire-and-forget frames and never direct to providers. The
phone holds no provider tokens; the host holds all CLIs and credentials.

## Mounted transport and current integration

The mobile whole-machine client uses `/v1/relay/socket`, separate from public
shared-session `/realtime` permissions. On a new socket it sends
`{"type":"relay-auth","v":1,"access_token":"<fresh account JWT>"}` within
10 seconds. The server verifies the account token and returns
`{"type":"relay-ready","v":1}` before accepting RPC. Invalid authentication
returns a generic `relay-error` with `loginRequired:true` and closes 4401.
Expiry also closes 4401. Credentials/query strings are rejected in this endpoint's
URL. Browser and native clients use the same first-frame admission.

All request/subscription params include `device_id` for the target desktop;
the mobile transport injects it. Request IDs are nonzero UUID strings. Socket
attention stream handles use the literal `runtime.events`: seq 1 carries
`{events:[...]}` as a snapshot, later payloads are individual attention events.
Unsubscribe/resubscribe stops just that stream; it must not cancel an in-flight
write on the shared connection. `relay-ping`/`relay-pong` v1 frames provide the
application heartbeat.

HTTP `/v1/relay/rpc` and `/v1/relay/host/state` remain authenticated adapters;
the older SSE adapter uses its own server handles. Mobile uses the socket
contract above. `catalog.read` shares the catalog store used by authenticated
PUT/GET `/v1/relay/catalog`.

This contract is implemented and covered by a real Go-to-TypeScript socket
test. Production still has no desktop publisher/command/provider adapter, and
catalog/state/events are process-local. Mutations return unavailable without
host confirmation; the server does not create a mirror entry as a substitute
for a desktop operation. Provider execution must select the authorized host
owner and never run an ambient cloud CLI. Preview screens remain sample data
until machine discovery, hierarchy relationships, and their UI bindings land.

## Wire frames (JSON, `v: 1`)

Request (mobile → relay):

```json
{ "type": "rpc-request", "v": 1, "id": "<client-unique>", "method": "<method>", "params": { } }
```

Reply (relay → mobile, always answers the `id`):

```json
{ "type": "rpc-reply", "v": 1, "id": "<same>", "ok": true, "result": { } }
{ "type": "rpc-reply", "v": 1, "id": "<same>", "ok": false, "error": { "message": "<user-safe>", "loginRequired": false } }
```

Streams (subscribe → snapshot first, then ordered events):

```json
{ "type": "rpc-subscribe", "v": 1, "id": "<client-unique>", "stream": "<name>", "params": { } }
{ "type": "rpc-unsubscribe", "v": 1, "id": "<client-unique>", "stream": "<handle>" }
{ "type": "rpc-stream", "v": 1, "stream": "<handle>", "seq": 7, "payload": { } }
```

## Client rules (mobile)

- Request ids are unique per connection (reuse the existing request-id helper
  and validity check). Default deadline 15s; on timeout surface "request timed
  out" locally. Never blindly retry a timed-out request (idempotency unknown).
- The server never replays requests across reconnects. The client resets all
  stream state on a new connection epoch and re-subscribes; the first message
  on a re-subscribed stream is always a fresh snapshot.
- Stream `seq` is strictly increasing per stream. On a gap, drop the stream
  back to snapshot (re-subscribe); never render a partial sequence.
- Unsubscribe-before-first-snapshot is legal: hold it, deliver the snapshot,
  then confirm the unsubscribe (older hosts only register the stream when the
  first snapshot emits).
- Unknown methods, streams, and malformed payloads are dropped (log + ignore).
  Readers are strict hand-written validators in the style of
  `mobile/src/transport/relayCatalog.ts` (`parseRelayCatalog` returns null on
  any bad shape). Error `message` strings are user-safe; never put tokens,
  URLs with credentials, or internal errors in them.

## Server rules

- Every method and stream checks same-account controller authorization first
  (existing `RequireAuth` + user/device-claim patterns). Anonymous → reply
  `ok: false` with 401 semantics and `loginRequired: true`. Foreign device →
  403 `loginRequired: false`.
- No method may publish another account's data. Owner scoping follows the
  existing catalog pattern (owner key embeds the caller id).
- Pre-replay history is never delivered: after a reconnect the server sends a
  fresh snapshot before any new events (matches the terminal epoch-reset rule).
- Payload caps: params JSON ≤ 64KB; list results paginated (`limit` default
  50, max 200); file content truncated with an explicit `truncated: true` flag.

## Method table (R2)

| Method | Params | Result |
|---|---|---|
| `catalog.read` | `{}` | `{ projects, workspaces, tabs }`, existing catalog shapes |
| `tabs.list` | `{}` | `{ tabs: [{ id, kind, title, active, visible }] }` (`kind`: `terminal`\|`browser`\|`settings`) |
| `tabs.createTerminal` | `{}` | `{ id }` of the new terminal tab |
| `worktree.list` | `{}` | `{ worktrees: [{ name, path, branch, project }] }` |
| `worktree.create` | `{ path, name?, branch? }` | host-confirmed created worktree entry |
| `files.list` | `{ path }` | `{ entries: [{ name, kind }] }`, dotfiles included, symlinks not followed |
| `files.search` | `{ query }` | `{ entries: [...] }`, same shape, bounded |
| `files.open` | `{ path }` | `{ content, truncated }`, text only, capped |
| `runtime.events.subscribe` | `{}` | stream of attention events (mirror of the push dispatcher's server-side decisions) |

## Provider table (R3)

Served from the host's own CLIs and credentials. The phone sends only
search parameters; tokens never cross the relay.

| Method | Params | Result |
|---|---|---|
| `github.listWorkItems` | `{ query?, repo? }` | `{ items: [{ id, number, title, url, state, repo }] }` |
| `gitlab.listWorkItems` | `{ query?, project? }` | same item shape |

Linear is explicitly out of scope (no desktop integration exists).

## Mobile client table (R4)

Typed methods over `RealtimeSessionClient` following this envelope, hand
readers in the `relayCatalog.ts` style, fake-socket unit tests covering:
timeout, unknown-method drop, malformed-payload drop, seq-gap resync,
unsubscribe-before-snapshot, and 401/403 mapping. No UI binding (lead wires
screens in R5).
## Companion lifetime and terminal host bridge

The account socket first sends `relay-auth` with its access token, optional
`device_id`, and optional `resume_session_id`. Every admission verifies the JWT.
`relay-ready` includes `session_id`, `session_expires_at` and `access_expires_at`
(Unix milliseconds). The companion deadline is fixed at creation plus 24 hours;
resuming never extends it. Access-token and companion deadlines are independent.
Expiry/revocation closes 4408 and ends automatic recovery; expired JWT closes
4401 and may recover with fresh credentials and the same companion ID.
`relay-error.sessionEnded: true` ends recovery before the close frame arrives.
`companion.session.revoke` revokes only the current socket's companion session.

Desktop hosts connect outbound to `/v1/relay/host` and authenticate in the first
`relay-host-auth` frame using the account JWT, `device_id` and
`expected_user_id`. The verified account must match the assertion and own the
published catalog. The broker allows at most 32 connected hosts per account.
Correlated `host-request`/`host-reply` frames carry only allowlisted terminal
and read-only coordination operations. Replacing a host connection invalidates old attachments and pending
calls; mutations are never replayed to the successor connection.

Phone methods `tabs.attach`, `tabs.read`, `tabs.screen`, `tabs.input`,
`tabs.resize` and `tabs.detach` require the current companion socket. Opaque
handles bind account, desktop, lease and physical phone connection, with four
attachments per lease. Attach uses the catalog's tab ID and selects the existing
daemon PTY. Detach, socket loss and lease expiry release access without closing
the desktop terminal or taking over a public share.

Read responses contain `base`, `total`, base64 `data`, `alive` and optional
`exit_code`. The next offset is `base + decoded byte length`, not `total`.
Reads are capped at 64 KiB. A gap switches mobile to `tabs.screen`, whose
`{complete,total,delta}` response carries the existing `alt_screen` wire grid.
An incomplete snapshot has `delta: null` and requires recovery; a display grid
is never used as a parser checkpoint followed by partial VT bytes. Input is
capped at 16 KiB UTF-8 and never receives an implicit Enter or automatic replay.
An interrupted input acknowledgement leaves delivery unknown.

Host routing and catalog snapshots currently remain process-local. Distributed
host routing and durable catalog storage require a separate adapter.

### Live Agent coordination

`coordination.list` takes optional `project_id`, resolved through the selected
machine's catalog to the native sidebar path. It returns `{items,truncated}`;
each item contains `id,title,project_path,status,task_count,done_count,
running_count,blocked_count,needs_user,updated_at`. `needs_user` is boolean and
timestamps are Unix milliseconds.

`coordination.get` takes `team_id` and returns `{team,truncated}`. The team has
`id,title,project_path,status,goal,tasks,messages`. Tasks expose only
`id,title,status,agent,model,depends_on,result_summary`; messages expose
`id,from,label,kind,text,created_at` with string IDs. Team status is
`planning|running|done|stopped`; task and message states follow the daemon enums.

Both methods require the current companion socket and live owned desktop.
The broker injects `session_id` and `expires_at`; clients cannot select them.
Native dispatch checks the socket's captured account and organisation before
and after I/O, and lists each allowlisted project explicitly. No unscoped team
listing or mutation is exposed. Responses are projected to allowed fields and
bounded to 100 teams, 60 tasks, 100 recent messages and 512 KiB. Omitted entries
or shortened text set `truncated`. Project membership is rechecked after I/O.
