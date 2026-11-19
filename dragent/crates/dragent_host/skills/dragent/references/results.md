# Reading Dragent results

Every `dragent` command, every tool from `dragent mcp` and Dragent's own agent tools
report failures the same way. Act on `status` and `code`, never on the wording of
`message`. `dragent capabilities --json` has the authoritative list under
`results`.

## A failure

In JSON mode (an agent or CI runs the command, or `--json`), a failure is printed on
stdout as one object, and the exit code tells the status apart without parsing:

```json
{
  "ok": false,
  "status": "timeout",
  "code": "no_answer",
  "message": "Dragent didn't answer in time",
  "next": "It may still finish: `dragent requests show 9b2…` says whether it did …",
  "request_id": "9b2…",
  "retry_safe": false
}
```

Through MCP the same object is the error's text and its `structuredContent`.

| status | exit | What it means for you |
|---|---|---|
| `invalid_input` | 2 | Fix the command or arguments; nothing happened. |
| `needs_auth` | 3 | The person must sign in or connect an account; don't work around it. |
| `unavailable` | 4 | Dragent or a service isn't reachable; nothing happened. |
| `invalid_target` | 5 | The id is wrong or stale: list again and pick a current one. |
| `needs_approval` | 6 | Waiting on the person. Never approve for them. |
| `timeout` | 7 | It **may have happened**. Inspect before doing it again. |
| `cancelled` | 8 | The person or app stopped it; don't redo it unasked. |
| `unsupported` | 9 | This build or platform can't; say so. |
| `missing_dependency` | 10 | A tool or OS permission is missing; tell the person which. |
| `failed` | 1 | Anything else; read `message`. |

`retry_safe: true` means nothing happened, so the same command can run again once
the cause is fixed. When it's `false`, find out what happened first.

## A success

A successful command prints its own JSON. A `status` field of `accepted` (started,
follow it with the returned id), `partial` (some of it done; the result says what
is left) or `unverified` (delivered, outcome not confirmed) qualifies it; no
`status` means it completed.

## After a timeout

```sh
dragent requests show 9b2…   # state: queued, in_progress, done or unknown
```

`done` includes the app's answer, success or failure. `unknown` means it was never
made, its answer was already collected, or more than a day has passed.

## Commands with outside effects

For anything that would be wrong to do twice — posting a comment, creating an item,
a workspace or a team, sending a message — pass a key you make up once per
intended action:

```sh
dragent integrations comment linear ENG-42 --body "Fixed in #812" --idempotency-key eng42-fix-note
```

Running it again with the same key and arguments replays the first result instead
of posting twice. If the first run's outcome is unknown, it refuses with
`timeout` / `outcome_unknown`: check whether it happened, and use a new key only
if it didn't.
