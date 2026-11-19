# Follow-ups, duplicates, failures and verification

## Private feedback has no thread to continue

This build exposes no feedback list, receipt id or follow-up address for
`dragent feedback`. Each send is independent. There is no CLI mechanism to
check for an existing matching report, append to one, or reopen one. Say so
plainly instead of inventing a lookup: "I cannot check for an existing
private report from here; each send starts a new one."

To avoid accidental duplicates: review with `--draft`, send once, and read
the result before retrying. After a `timeout`, run
`dragent requests show REQUEST_ID` first — the app may have sent it while
the CLI waited. Only resend with a new draft when the check says the first
attempt did not land.

## Public repository issues use the integrations channel

File a public issue only when the person explicitly asked for that
repository. Discover the channel first:

```sh
dragent integrations help
dragent integrations list --json
```

Search before filing, and prefer commenting on the existing thread when one
fits; file once when none does. The repository comes from the person or
`--project PATH` for GitHub/GitLab/Forgejo (default: current folder) — never
a guessed owner/repo. Review every line for public content first: no tokens,
no private paths, no customer data, no full logs. New items post under the
user's own connected account.

## Reading results (result contract)

JSON failures carry `status`, `code`, `message`, `next`, `request_id` and
`retry_safe`; the exit code matches the status. Act on `status`/`code`,
never the wording. Success carries `{"sent": true, ...}`; `--draft` carries
`{"sent": false, "draft": ...}` and never contacts the app.

| status / code | meaning | next step |
|---|---|---|
| `invalid_input` / `feedback_kind`, `feedback_empty` | flags or empty draft | fix the command; nothing happened, safe to retry |
| `needs_auth` / `signed_out` | nobody signed in | ask the person to open Dragent and sign in |
| `unavailable` / `app_not_open`, `request_not_taken` | app closed or wrong organisation | open the app (`dragent open PATH` only if needed), compare `dragent status --json` |
| `timeout` / `no_answer` | app took it, no answer in time | `dragent requests show REQUEST_ID` before resending |
| `failed` / `app_error` | server or app refused | read `message`, fix or report back |

## Keep the draft on failure

A send failure after the draft was built saves it under
`~/.dragent/feedback-drafts/feedback-<secs>-<uuid>.md` (mode `0600`) and
puts the path in `next`. That file is the state you own: verify it exists
with the printed path, quote the path in the report, and remove it only when
the person says the report landed elsewhere. `--draft` itself creates no
file. Never delete unrelated drafts.
