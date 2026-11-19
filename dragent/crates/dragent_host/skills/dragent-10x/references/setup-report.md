# `dragent standup --setup` report reference

Current configuration only: what is installed and connected right now. It
says nothing about how anything was used before, and it must never be read
as adoption history or savings. Real output (trimmed):

```json
{
  "status": "ok",
  "mode": "setup",
  "installed_agents": [
    {"key": "claude", "name": "Claude Code", "evidence": "verified"},
    {"key": "codex", "name": "Codex", "evidence": "verified"}
  ],
  "automations": [
    {
      "id": "9b2…",
      "name": "Weekday checks",
      "enabled": true,
      "schedule": "0 9 * * 1-5",
      "agent": "dragent",
      "evidence": "verified"
    }
  ],
  "services": [
    {"service": "github", "connected": true, "evidence": "verified"},
    {"service": "linear", "connected": false, "evidence": "verified"}
  ],
  "skills": {
    "bundled": ["dragent-standup", "dragent-10x"],
    "shared_folders_installed": true,
    "desktop_guidance_installed": true,
    "evidence": "verified"
  },
  "mcp_servers": [
    {"name": "Expo", "transport": "SSE", "enabled": true, "evidence": "verified"}
  ],
  "projects": [
    {"name": "myapp", "path": "/Users/jahsminemma/dev/workspace-ui/myapp", "evidence": "verified"}
  ],
  "errors": []
}
```

## What each section tells you

- `installed_agents`: agent keys this computer can start (`agents list`
  agrees). Presence is not usage: do not claim the user relies on any of
  them.
- `automations`: configured schedules with their agent and enabled flag.
  A definition is not a result: read `automations runs ID --limit N
  --json` before judging whether one earns its keep.
- `services`: connected or not, with the account where connected. A
  connected service is an option, not a recommendation to post there.
- `skills`: bundled skill names plus whether shared folders and desktop
  guidance are installed. Installed is not adopted.
- `mcp_servers`: saved servers with transport and enabled flag. Saved is
  not used.
- `projects`: folders open in Dragent. Open is not active.
- `errors`: per-source failures; any entry makes `status` `"partial"`.
  Read the rest anyway.

## Failures and bounded recovery

| Failure | Meaning | Next step |
|---|---|---|
| `partial` with a source in `errors` | That slice could not be read | Work from the slices present; say which is missing |
| `needs_auth` / `signed_out` on a follow-up | Writes need sign-in or a connected account | Ask the person; keep auditing read-only |
| A feature missing from `capabilities` | This build does not have it | Say so; do not approximate it with something else |

Never retry indefinitely. Never present a guess as a measurement.
