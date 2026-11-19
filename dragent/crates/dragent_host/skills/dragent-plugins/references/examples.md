# Worked examples

Ids, server names and tools below are illustrations: take yours from
`inspect`, `status` and the server's own tool list.

## A package for a manifest you write

```json
{
  "schema": 1,
  "id": "acme-tickets",
  "name": "Acme Tickets",
  "version": "1.0.0",
  "description": "Search and update Acme tickets.",
  "skills": ["skills/acme-tickets"],
  "servers": {
    "tickets": {
      "transport": "stdio",
      "command": "node",
      "args": ["${PLUGIN_DIR}/server/index.js"],
      "env": {
        "ACME_TOKEN": { "secret": true, "description": "An Acme API token" },
        "ACME_SITE": { "default": "acme.example" }
      }
    }
  },
  "requires": { "commands": ["node"] }
}
```

A package that ships a build per system names each one; `args` carry over
unless a system gives its own:

```json
"tickets": {
  "transport": "stdio",
  "args": ["--stdio"],
  "platforms": {
    "macos": { "command": "./bin/macos/tickets-server" },
    "linux": { "command": "./bin/linux/tickets-server" },
    "windows": { "command": "./bin/windows/tickets-server.exe" }
  }
}
```

Prefer a program every system has (`node`, `python3`, `uvx`) with the script in
the package: one `command` then works on macOS, Windows and Linux.

Rules `inspect` enforces: `id` lowercase with hyphens (not starting
`dragent`); every path inside the package; a stdio `command` is a program on
PATH or `./path` in the package, arguments in `args`; an http server is
`https://` (or `http://localhost`) with no `env` (it signs in, or takes a
token, in Settings → MCP); secrets never carry a value; no links in
the package; at most 2,000 files / 50 MB. Unknown fields are refused.

## Installing one the person asked for

```sh
dragent plugins inspect https://github.com/acme/tickets-plugin --ref v1.0.0 --json
#   servers: [{ "name": "tickets", "runs": "node <plugin folder>/server/index.js",
#               "secrets": ["ACME_TOKEN"], "settings": ["ACME_SITE"] }]
#   requires: [["node", true]]
dragent plugins install https://github.com/acme/tickets-plugin --ref v1.0.0 --json
dragent plugins status acme-tickets --json
#   accounts[0].servers[0].next_step: "needs_settings"  (missing_settings: ["ACME_TOKEN"])
```

Tell the person: "Acme Tickets is installed. In Settings → MCP, fill in
ACME_TOKEN for acme-tickets-tickets, switch it on, and approve its tools." Then
check again; once `next_step` is `ready`:

```sh
dragent plugins check acme-tickets --tool list_projects --json
# { "works": true, "output": "…" }
```

## Two accounts

```sh
dragent plugins accounts add acme-tickets work
# server_entries: ["acme-tickets-tickets-work"]  (off: the person sets its token and switches it on)
```

The default account's tools are `mcp__acme_tickets_tickets__…`, the work
account's `mcp__acme_tickets_tickets_work__…`. Before a write, ask which one.

## An update that changes what runs

```sh
dragent plugins update acme-tickets --json
# "to": "2.0.0", "switched_off_and_unapproved": ["acme-tickets-tickets"]
```

The server's program changed, so it's off and unapproved again. Report it and
let the person decide; their token stays.

## The person edited the plugin's skill

```sh
dragent plugins status acme-tickets --json
#   skills: [{ "name": "acme-tickets", "edited": ["references/fields.md"] }]
dragent plugins update acme-tickets --json
#   skills.kept: ["acme-tickets/references/fields.md"]
```

Their edit stays through updates and removal; mention that their copy won't
get the plugin's changes to that file.

## Removing one, and a step that fails

```sh
dragent plugins remove acme-tickets --json
# { "status": "partial", "code": "remove_incomplete",
#   "message": "acme-tickets is partly removed: server entries not removed …" }
dragent plugins status acme-tickets --json     # what's left
dragent plugins remove acme-tickets --json     # again, once Dragent is reachable
```
