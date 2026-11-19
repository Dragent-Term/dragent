# Dragent Workspace Wire Contract v1

This directory contains language-neutral JSON fixtures for the first agentic
workspace command and event protocol. Rust desktop code and Go cloud code both
decode, validate, and reproduce these fixtures in their test suites. Future web
and companion clients must do the same before they send workspace commands or
consume workspace events.

## Compatibility rules

- JSON field names and tagged payload names are `snake_case`.
- UUID values are lowercase hyphenated strings.
- Unix timestamps are milliseconds.
- A command is idempotent by `command_id`. Retrying it must not apply it twice.
- Every command carries a client-generated `work_item_id`, including work-item
  creation, so offline and cloud representations retain one stable identity.
- `create_work_item` is accepted only before that work item has canonical
  history. An idempotent retry returns the original `work_created` event;
  another create command for the same initialized work item is rejected.
- `transition_work` requires `expected_revision`; authority validates the
  current durable status and the domain lifecycle before updating the work row
  and appending one `work_status_changed` event.
- `transition_plan_task` is also revision guarded. Authority reconstructs the
  latest approved Plan and scheduler inputs, validates the requested lifecycle
  edge against the deterministic scheduler projection, then appends one
  task-scoped `plan_task_status_changed` event carrying the updated Plan.
- A scheduler-ready task is not runtime-ready until its Approved work contract
  has an assigned agent instance. In particular, `ready` does not imply that a
  model may run or tools may execute.
- `provision_agent_instance` is a separate revision-guarded command. Authority
  accepts it only for Ready-to-Execute work and a latest canonical Approved
  contract with no existing binding. The resulting single
  `agent_instance_provisioned` event contains both the Pending instance and its
  updated Approved contract, so replay cannot observe half of the binding.
- Generic `upsert_work_contract` commands cannot assign `agent_instance_id`.
  That authority belongs exclusively to provisioning. A Pending instance and
  Approved bound contract still cannot run.
- `activate_agent_instance` is a separate revision-guarded command. It requires
  the Pending instance's exact Approved contract binding, a resolved effective
  model route, explicit charging ownership, and a bounded execution lease. One
  atomic `agent_instance_activated` event moves the instance to Ready, its
  contract to Active, and records the Active lease; replay cannot observe only
  part of that runtime authorization.
- An `auto` model route is provisioning intent and cannot be activated. The
  execution authority must resolve it before activation. Local authority
  accepts only Local Compute or Bring Your Own Key charging. Platform Credits
  require a durable billing-operation authorization from cloud authority and
  fail closed locally. Activation itself invokes no model or tool and does not
  represent billing settlement.
- Principal shaping proposes an exact available model route per specialized
  Work Contract before approval. Plan tasks visibly inherit the route of their
  assigned contract; changing it returns the contract to a reviewable state.
  Stronger routes are reserved for architecture, ambiguous cross-system work,
  security, and review, while bounded implementation and extraction should use
  the least-cost suitable route. This recommendation grants no execution or
  billing authority by itself.
- Role definitions explicitly distinguish `worker` from `reviewer`; older
  payloads without `kind` remain workers. Reviewer contracts are always
  read-only. This semantic type drives reviewer attribution and cannot be
  inferred from a role's display name.
- Lease duration is authority policy, not client authority. The current local
  authority caps execution leases at 15 minutes. `renew_execution_lease`
  extends only an Active lease bound to a live agent, requires the charging
  owner, and must move expiry forward. The execution host requests renewal
  before expiry; rendering never renews a lease. An elapsed Running lease can
  never be renewed. A Ready pre-start reservation may be extended by its
  charging owner after downtime because no provider execution has begun.
- `start_agent_execution` is the only command that may move an activated agent
  and its assigned task into Running. Generic task transitions cannot fabricate
  execution. Authority verifies the Ready agent, Active unexpired lease,
  assigned contract, scheduler eligibility, and unique attempt identity, then
  records one `agent_execution_started` fact containing the Running agent,
  lease, execution attempt, and updated Plan. The same transaction moves the
  work item from Ready to Execute into Executing for the first launch.
- A committed execution-start event is authorization for an adapter to invoke
  the recorded effective model route; it is not evidence that a provider call
  has already happened. Adapters must never invoke a model or tool first and
  attempt to reconstruct this event afterward.
- Local writable attempts run from a deterministic isolated Git worktree, not
  the user's primary checkout. Dragent seeds a detached synthetic baseline from
  the approved filesystem state (including staged, unstaged, and untracked
  files), race-checks the source during preparation, and fails closed if no
  isolated root is available. The isolation identity and base revision must
  become canonical before authority may allow overlapping write scopes to run
  concurrently. Read-only attempts use the canonical shared-project isolation;
  writable attempts must use Git-worktree isolation. Authority allows
  overlapping write scopes only when every overlapping Running attempt has a
  distinct validated Git-worktree identity. Legacy or ambiguous isolation
  fails closed.
- `finish_agent_execution` terminalizes exactly one Running attempt with a
  non-empty attributable summary and requires the lease charging owner. A
  successful provider attempt moves the task
  and agent to Reviewing and keeps the lease Active for review feedback; it
  does not mark the task or work item Completed. A successful writable attempt
  must include a structured change summary tied to its exact isolation ID and
  immutable base revision. Paths and line counts remain attributable data,
  rather than a fabricated patch document. Failure moves both to Failed
  and releases the lease, cancellation cancels the task/agent/lease, and timeout
  fails the task/agent while expiring the lease.
- On restart, provider recovery must reject inactive or elapsed leases before
  inspecting or reattaching a local run. The execution host submits an elapsed
  Running attempt as a canonical Timed Out finish, after which ordinary
  bounded retry and escalation rules apply without requiring user input.
- `cancel_agent_execution` and `pause_agent_execution` are revision-guarded
  authority commands. The execution host stops provider work only after it
  observes the accepted terminal event. Cancellation is terminal. Pause moves
  the work item to Paused, returns its task and agent to Ready, retains the
  Active lease, and resume creates a new attempt rather than reviving the old
  provider invocation.
- `resolve_agent_execution_review` is the only command that closes a successful
  review. `accepted` closes a successful read-only result and moves its task to
  Verifying without pretending that files were integrated. `integrated` moves the task to Verifying, completes the
  agent and contract, releases the lease, and records the resolution on the
  attempt. `revision_requested` records the reviewed attempt, returns the task
  and agent to Ready under the active contract and lease, and requires a fresh
  numbered attempt; execution cannot jump directly from Reviewing to Running.
  `discarded` cancels the task and agent, revokes the contract, and cancels the
  lease. The desktop removes the old local review worktree only after the
  canonical event is accepted, so restart recovery never resurrects a resolved
  review.
- Successful workers post a routed `handoff`; successful explicitly typed
  reviewers post a routed `review_finding`. Reviewer tasks receive bounded
  canonical messages from their dependency chain as untrusted context. These
  messages cannot grant tools, permissions, write scope, or lifecycle changes.
- `prepare_agent_execution_retry` is the only automatic-retry re-arm. It
  accepts only the latest Failed or Timed Out attempt, only after deterministic
  exponential backoff, and only while fewer than three automatic attempts have
  run. It atomically returns the failed task and agent to Ready and creates a
  fresh bounded lease with the prior charging authorization. Paused,
  cancelled, succeeded, superseded, and exhausted attempts cannot auto-retry.
- When automatic attempts are exhausted, the scheduler raises one stable
  `failure` Attention item for the Principal Agent. It is a durable escalation
  point: the Principal can revise the plan or contract, resolve the blocker,
  or request user consent only when new authority is genuinely required.
- Generic task transitions cannot move a Running task out of Running. This
  preserves one atomic `agent_execution_finished` fact containing the terminal
  attempt and matching agent, lease, and Plan state. Accepted evidence, final
  completion, post-review lease release, and billing settlement remain
  deterministic authority decisions rather than provider-declared outcomes.
- Operational agent communication uses `post_message` with explicit routing.
  Handoffs, blockers, decision requests, review findings, and approvals name
  their recipients and task; an execution attempt may further scope the
  message. Authority verifies that sender, recipients, task, attempt, reply,
  and thread are canonical and related. A message communicates work or
  evidence but never grants capabilities, changes a contract, or performs a
  lifecycle transition. Unrouted discussion remains backward compatible for
  shaping conversation.
- `raise_attention` and `resolve_attention` are strict revision commands.
  Permission attention is bound to one Running execution attempt and one tool
  call, remains durable across restart, and may only be resolved by an assigned
  actor. Reusing a tool-call identity cannot create parallel open approvals.
  Resolution has a typed outcome; permission requests accept only `approved`
  or `rejected`, never an inference from free-form resolution text.
- `expected_revision`, when present, is an optimistic concurrency guard.
- Event `sequence` starts at 1 and is monotonic within one `work_item_id`.
- Task events carry the same non-nil `task_id` in both their envelope identity
  and semantic payload; consumers must reject a mismatch.
- `correlation_id` groups one user-visible operation and its resulting work.
- Event `causation_id` identifies the command that directly caused the event.
- Unknown `schema_version` values must be rejected until a compatible decoder
  or explicit upcaster exists.
- Clients must not derive semantic workspace state by parsing chat text.
- `complete_shaping_turn` atomically emits the Principal message, complete
  structured shaping proposal, and normalized shaping-record snapshot under
  one command outcome; consumers must never expose a proposal revision without
  its attributable message and records.
- Structured-record edits are strict revision commands. They preserve record
  identity, require `expected_revision`, append an immutable update event, and
  must carry provenance for the command actor.
- `upsert_work_brief` is also a strict revision command. Version 1 retains one
  stable brief identity, requires `updated_by` to identify the command actor,
  and emits one immutable `work_brief_updated` event. `created_by` remains the
  original author when a different actor produces a later version.
- Role definitions, work contracts, and proposed teams are typed canonical
  records. Draft and proposed contracts must not carry an agent-instance
  identity, so creating or revising a role cannot start execution implicitly.
- `upsert_work_contract` is a strict revision command. Contracts carry a
  positive version, preserve their original `created_by`, and identify the
  command actor through `updated_by`. Editing an approved contract returns it
  to Proposed so revised authority must be reviewed again.
- Approving a work contract changes governance state only. It does not assign an
  agent instance, choose a runtime model, schedule work, or grant authority
  beyond the explicitly reviewed contract.
- Authority must validate current canonical team readiness before accepting a
  transition to `ready_to_execute`; client-side button state is never sufficient.
  Every role referenced by the latest proposed team must resolve to the matching
  role definition and an Approved work contract. The latest typed Plan must be
  Approved, every task must be assigned to one of those contracts, and every
  contract must own at least one task. This transition still does not create
  runtime agents or begin execution.

The existing shared-terminal protocol has its own version and lifecycle. It is
not replaced or renumbered by this contract.

## Single schema source (resolves OPEN-001 for v1)

The Rust types in `dragent_session_protocol::workspace` are the single source
of truth. `schemars` derives the canonical JSON Schemas checked in under
`schema/` directly from them — no hand-written mirror:

- `workspace-command-envelope.v1.schema.json`
- `workspace-event-envelope.v1.schema.json`

Adding or changing a wire type follows this order, enforced by tests — never
by discipline alone:

1. Change the Rust types (`dragent_session_protocol`, `dragent_workspace_model`).
2. Regenerate: `REGENERATE_SCHEMAS=1 cargo test -p dragent_session_protocol --lib`
   (otherwise `checked_in_schemas_match_generated_types` fails).
3. Extend the fixtures in `fixtures/` when the change alters the wire shape.
4. Mirror the change in Go (`server/internal/workspace/protocol`): Go
   strict-decodes (`DisallowUnknownFields`) and round-trips every fixture, so
   dropped or unknown fields fail `go test ./internal/workspace/protocol/`.
   `TestCheckedInSchemasMatchSupportedPayloadTypes` additionally fails on any
   payload-type drift in either direction; genuinely deferred payloads live in
   `schemaKnownGaps` with a reason, never silently.
5. Rust `all_shared_fixtures_validate_against_canonical_schemas` proves every
   fixture conforms to the checked-in schemas.

Consequences: fixtures can never silently diverge from either implementation,
and neither implementation can add a wire-visible field or payload type
without updating the schema, the fixtures, and the other language.
