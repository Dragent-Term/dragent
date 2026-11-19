# `dragent doctor` report reference

JSON by default for agents (`--json`, automatic when an agent or CI runs the
command); `--text` renders the same report for a person, `--quiet` prints
only failing check ids, one per line. The top-level shape:

```json
{
  "status": "partial",
  "generated_at_ms": 1793481081824,
  "cli_version": "0.2.3",
  "project": null,
  "checks": [],
  "failing": ["app"]
}
```

## Sections

- `checks`: one entry per check: `id` and `layer` (the same name:
  `launcher`, `app`, `signin`, `host`, `agents`, `mcp`, `skills`, `desktop`,
  `browser`, `automations`, `hangs`), `status` (`ok`, `warn`, `fail`,
  `skipped`), stable `code` (e.g. `app_not_open`, `signed_out`,
  `account_binding_broken`), `summary`, bounded redacted `evidence`,
  and `next` (the immediate step for that check).
- `failing`: ids of checks with `status: fail`. Any entry makes `status`
  `"partial"`; `warn` and `skipped` alone keep `"ok"`.
- `project`: the `--project` scope, if given.
- `--check ID[,ID]`: run only some checks. Unknown ids fail with `usage`
  and list the valid ones; nothing runs (`retry_safe: true`).

## Options

- `--check ID[,ID]`: only these checks, e.g. `--check app,signin`.
- `--project PATH`: scope workspace-related checks (sign-in context,
  automations) to this project.
- `--json` / `--text` / `--quiet`: machine-readable (default for agents) /
  human-readable / failing ids only.
- `help` / `--help` / `-h`: the checks and options.

## Worked examples

All output below is real, from this build. Run each command yourself before
quoting it; never invent results the report does not contain.

### 1. The app is closed: partial, with the failing layers named

With a throwaway home (no app, no data):

```sh
DRAGENT_HOME=$(mktemp -d) dragent --dragent-cli doctor --check app --json
```

```json
{
  "checks": [
    {
      "code": "app_not_open",
      "evidence": {
        "cli_version": "0.2.3",
        "running": false
      },
      "id": "app",
      "layer": "app",
      "next": "`dragent open PATH` starts it, if the failing path needs the app; rerun `dragent doctor --check app`.",
      "status": "fail",
      "summary": "Dragent isn't open, so no request could be answered."
    }
  ],
  "cli_version": "0.2.3",
  "failing": ["app"],
  "generated_at_ms": 1793481093325,
  "project": null,
  "status": "partial"
}
```

Exit code is 0: the report stands on its own. Every check that needs the app
fails the same way (`browser`, `desktop`, `host`, `signin`); checks that read
local records (`agents`, `mcp`, `skills`, `automations`, `hangs`) still report.
Why one check per layer matters: `app_not_open` in six places is one cause,
not six problems — fix the app layer and rerun.

### 2. A healthy layer: freeze reports

```sh
dragent --dragent-cli doctor --check hangs --json
```

```json
{
  "checks": [
    {
      "code": "no_hang_reports",
      "evidence": {
        "count": 0
      },
      "id": "hangs",
      "layer": "hangs",
      "next": "Nothing to do.",
      "status": "ok",
      "summary": "No freeze reports exist."
    }
  ],
  "cli_version": "0.2.3",
  "failing": [],
  "generated_at_ms": 1793481093339,
  "project": null,
  "status": "ok"
}
```

A `hang_reported` result names only the newest report's path (shortened) and
timestamp: read the file yourself before citing it, and attach it to feedback
(`dragent feedback --draft` first) only when the freeze repeats.

### 3. Bad input: fix and rerun

```sh
dragent doctor --check frobnicate --json
```

```json
{
  "code": "usage",
  "message": "unknown check `frobnicate`",
  "next": "Checks: launcher, app, signin, host, agents, mcp, skills, desktop, browser, automations, hangs",
  "ok": false,
  "retry_safe": true,
  "status": "invalid_input"
}
```

`retry_safe: true` means nothing happened: correct the check id and run
again. Act on `status` and `code`, never on the wording of `message`.

## Status values

`ok` means every check passed or was skipped (a skip carries its reason, e.g.
desktop control on an unsupported platform). `partial` means at least one
check failed and `failing` says which; everything else in the report still
stands. There are no other statuses on success. A command failure of `doctor`
itself (bad flags) follows the result contract instead: `ok: false` with
`status`, `code`, `message`, `next`, `retry_safe` and a matching exit code.
