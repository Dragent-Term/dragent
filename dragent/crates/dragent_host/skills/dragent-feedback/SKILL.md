---
name: dragent-feedback
description: "Draft and submit a user-requested Dragent bug report, feature request or product feedback, with reproduction steps and redacted supporting evidence."
---

# Report a Dragent issue or suggestion

Use this skill only when the person asked to report something to the Dragent
team, or asked to file it as a public repository issue. Investigating a bug
is `dragent-doctor`; scheduled watches are `dragent-automate`; reading or
changing items in Linear, GitHub, Sentry or Slack is `dragent-integrations`.
A debugging session is not consent to send anything.

## Prerequisites

Private feedback needs the Dragent app open and the person signed in: the
CLI never signs in itself, it asks the app to send as the signed-in user.
`--draft` needs neither; use it to show the exact text without sending.
Commands act in the current organisation; do not switch it silently. If the
app, sign-in or command is unavailable, explain what is missing and stop;
do not fabricate a sent result.

## Discover what this build supports

```sh
dragent capabilities --json   # feedback family, result contract, skills
dragent feedback --help       # every flag this build accepts
dragent status --json         # is the app running, which organisation
```

Trust this output over memory. A flag that is not listed does not exist.

## Build the draft from what you already know

Reuse the prior investigation: trigger, observed vs expected, steps,
frequency, platform/build and affected workspace. Do not ask the person to
repeat what the terminal already shows. Keep measured observations separate
from suspected causes; mark guesses as unverified. Redact tokens, secret
environment values and unrelated private text. A local screenshot or log
path stays a local path: the CLI uploads no files, it sends pasted text
only. Full field guide: `references/draft-fields.md`.

```sh
dragent feedback --draft --kind bug --observed "pane spins forever" \
  --expected "file list appears" --steps "1. Open workspace" \
  --frequency always --cause "maybe watcher misses first event" \
  --evidence "watcher timeout after 10s (redacted)"
```

Short worked reports for a bug, a feature request and general feedback,
plus body-file use: `references/examples.md`. Skipping `--draft` review is
the common mistake: the person cannot correct what they never saw.

## Follow-ups are new drafts unless a thread exists

This build exposes no feedback list, receipt id or thread to continue: each
private send is independent. Say so when asked about duplicates; do not
claim a duplicate check the CLI cannot do. For an explicitly requested
public repository issue, search first with `dragent integrations` and
continue the existing thread with a comment when one fits; otherwise file
once. Public issues need the intended repository and a public-content
review first (no secrets, no private paths). Details: `references/followups.md`.

## Send, then read the result

```sh
dragent feedback --kind bug --observed "..." --expected "..." --json
```

Private feedback goes to the Dragent team via `POST /api/v1/feedback` from
the open app as the signed-in user: draft text plus account, organisation,
app version and OS. Nothing is published publicly. Read the JSON result as
data per `status` and `code` (`references/results.md` in `dragent`), never
the wording: `sent: true` means accepted; `needs_auth`, `unavailable` or
`timeout` mean it was not confirmed sent.

## Failures keep the draft

A failed send saves the draft under `~/.dragent/feedback-drafts/` and names
the path in `next`. Keep that file; do not retry blindly. After a `timeout`,
run `dragent requests show REQUEST_ID` before resending. After
`needs_auth`, ask the person to sign in, then resend the same draft. After
`unavailable` (`app_not_open`, `request_not_taken`), open the app first.
Common cases: `references/followups.md`.

## Verify and report

Verify by reading the result object and, for a public issue, the returned
URL or id. Confirm the draft file exists when a failure claimed one. State
creates only a local draft file on failure; remove only files you own.
Report: kind, destination (private team vs public repo URL), what was sent
in one paragraph, the result status/code, the draft path on failure, and
what was left untested or unsent.
