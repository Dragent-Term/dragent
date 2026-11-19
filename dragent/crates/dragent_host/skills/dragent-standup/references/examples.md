# Worked standup examples

All output below is real, from this build (paths shortened). Run each
command yourself before quoting it; never invent history the report does
not contain.

## 1. Ordinary recap: everything recent

```sh
dragent standup --json
```

```json
{
  "status": "ok",
  "workspaces": [
    {
      "name": "hey",
      "path": "/Users/jahsminemma/dev/Terminal agent",
      "branch": "feat/git-gaps-scaffold",
      "project": "/Users/jahsminemma/dev/Terminal agent",
      "evidence": "verified"
    }
  ],
  "agents": [
    {
      "id": "4088e74e",
      "agent": "claude",
      "state": "running",
      "state_evidence": "verified",
      "cwd": "/Users/jahsminemma/dev/Terminal agent",
      "excerpt": "",
      "evidence": "excerpt"
    }
  ],
  "teams": [],
  "automations": [],
  "linked": [
    {
      "id": "link-1",
      "workspace": "/Users/jahsminemma/dev/Terminal agent",
      "branch": "feat/git-gaps-scaffold",
      "agents": ["cce181d7", "4088e74e", "c1f3c2f7", "75fe358f"],
      "reason": "shared workspace"
    }
  ],
  "artifacts": [],
  "errors": []
}
```

Read it as: one workspace on `feat/git-gaps-scaffold` with four agent
sessions linked to it (`link-1`, reported once), no teams or automations,
nothing truncated, every source read (`"status": "ok"`). The recap then
checks the branch (`git status`, recent commits) before calling anything
done. Why the worktree check matters: an agent's `done` summary is
`claimed`; only the branch contents are `verified`.

## 2. Scoped recap: one project, one week

```sh
dragent standup --project /Users/jahsminemma/dev/Terminal agent --since 7d --json
```

Same shape, with `"project"` set, `"since_ms"` set, and older runs and
tasks dropped. Items without timestamps are kept. Scoping matters because
an unscoped recap mixes projects and buries the decision the user faces.

## 3. The host is unreachable: partial, not failure

With no host endpoint (a throwaway home reproduces this):

```sh
DRAGENT_HOME=$(mktemp -d) dragent --dragent-cli standup --json
```

```json
{
  "status": "partial",
  "workspaces": [],
  "agents": [],
  "teams": [],
  "automations": [],
  "linked": [],
  "artifacts": [],
  "errors": [
    {
      "source": "host",
      "status": "unavailable",
      "code": "app_not_open",
      "message": "Dragent's host isn't reachable, so agent aliveness and excerpts could not be checked; the records below are local only."
    }
  ]
}
```

Exit code is 0: the records present are usable. Report the recap as
incomplete (no excerpts, aliveness unchecked), do not retry in a loop, and
do not present local records as live state. The same shape with
`--text` renders sections with an `Incomplete sources` footer.

## 4. Bad input: fix and rerun

```sh
dragent standup --since bogus --json
```

```json
{
  "ok": false,
  "status": "invalid_input",
  "code": "usage",
  "message": "--since takes a duration like 30m, 24h, 7d, or an ISO date like 2026-10-25, not `bogus`",
  "next": "See `dragent standup --help`.",
  "retry_safe": true
}
```

`retry_safe: true` means nothing happened: correct the flag and run again.
Act on `status` and `code`, never on the wording of `message`.
