---
name: dragent-plugins
description: "Inspect, install, update, switch off and remove Dragent plugins (a package of skills and MCP servers), check each step of a plugin working, and set up or troubleshoot MCP servers for Dragent and external agents. Use dragent-integrations for the connected services' items themselves."
---

# Plugins and MCP servers

A **plugin** is a package (a folder or git repository with
`dragent-plugin.json`) bringing **skills** (instructions agents read) and
**MCP servers** (tools). Use this to add or maintain one, or to work out why
its tools don't answer. Reading and changing Linear, GitHub, Slack… items is
`dragent-integrations`; a broken app is `dragent-doctor`.

## What is the person's, always

- **Running a program.** A plugin's servers are added **switched off**. Only
  the person turns one on (Settings → MCP), and approves its tools there. You
  never do either, and never edit Dragent's settings files to do it.
- **Secrets.** The person fills them in Settings → MCP. Never ask for a secret
  in chat, put one in a command, a file or a report.
- **Trust after an update.** An update that changes what a server runs
  switches it off and withdraws its approval: the person decides again.

## Look before installing

```sh
dragent plugins inspect ./acme-plugin --json
dragent plugins inspect https://github.com/acme/tickets-plugin --ref v1.2.0 --json
```

Nothing from the package runs. Read and report: its skills, each server's
exact program here (`runs`, whether it ships with the package, and `systems`:
a server with `runs_here: false` has no build for this computer and isn't
added), the settings
and secrets it needs, programs it requires (`requires`, found here or not),
and names already taken (`skill_names_taken`, `server_names_taken`). Install
only what the person asked for, from the source they named.

## Install and follow each step

```sh
dragent plugins install ./acme-plugin --json
dragent plugins status acme-tickets --json
```

`status` reports every step separately. Per account and server,
`next_step` is the first one still missing:

| next_step | Means | Who |
|---|---|---|
| `needs_settings` | A setting or secret is empty (`missing_settings`) | The person, Settings → MCP |
| `off` | Not switched on | The person |
| `not_approved` | On, but its tools aren't approved | The person |
| `not_connected` | Approved, but it doesn't connect (`error`) | Check `requires`, the error; tell the person |
| `ready` | Connected, tools listed (`tools`) | — |
| `unknown` | Dragent is closed: connection can't be known | Ask the person to open it |

Skills: `edited` files are the person's changes (kept on update and removal),
`missing` ones they deleted. Then prove a call works, with a tool that only
reads:

```sh
dragent plugins check acme-tickets --tool list_projects --json        # works: true/false
dragent plugins check acme-tickets --tool get_ticket --args '{"id":"T-1"}' --account work
```

## Accounts

```sh
dragent plugins accounts add acme-tickets work      # its own copy of every server
dragent plugins accounts remove acme-tickets work
```

Each account is its own set of server entries (`acme-tickets-tickets-work`),
so every tool call's name says which account it uses. Ask which account to
use for anything that writes; never pick one silently.

## Update, switch off, remove

```sh
dragent plugins update acme-tickets --json                 # from where it came
dragent plugins update acme-tickets --from ./acme-plugin-2 --json
dragent plugins disable acme-tickets                        # skills out, servers off
dragent plugins enable acme-tickets                         # skills back; servers stay off
dragent plugins remove acme-tickets --json
dragent plugins sync                                        # put back missing entries/skills
```

Report `switched_off_and_unapproved` after an update. Disabling keeps
credentials; removal deletes the plugin's server entries with their tokens and
approvals, and keeps skill files the person edited (`skills.kept`). `sync`
puts back missing server entries (switched off) and skill files, never over
the person's edits.

## Failures

| status / code | Meaning | Do |
|---|---|---|
| `invalid_input` / `invalid_package` | The manifest or package isn't sound (paths outside it, links, secrets with values…) | Nothing was installed: report the reason |
| `invalid_input` / `already_installed` | — | `plugins update` |
| `invalid_input` / `server_name_taken` | Settings → MCP already has a server by a name the plugin needs | Ask the person; never remove or rename theirs yourself |
| `invalid_target` / `not_installed` | — | `plugins list` |
| `unavailable` / `fetch_failed` | The repository or `--ref` can't be reached | Check the address with the person |
| `partial` / `servers_not_added`, `remove_incomplete` | A step failed; the answer says which | `plugins status`, fix, `plugins sync` or remove again |
| `unavailable` / `app_not_open` | `check` needs Dragent open | Ask the person to open it |

Skills refused as `someone else's` or a Dragent name stay as they were: say so.

## MCP servers without a plugin, and external agents

Servers can also be added by hand in Settings → MCP (the person's step; adding
one there approves it). For an external agent that should use Dragent's own
tools, configure it to run the Dragent launcher with `mcp` (stdio; no port or
URL): `~/.dragent/bin/dragent mcp`. A server that connects can still have its
calls refused by approvals or the global tools switch: report which.

## Report

Plugin id and version, where it came from, what it placed (skills, server
entries per account), each server's `next_step`, any `check` result, and what
the person still has to do (settings, switching on, approving).

Worked examples: `references/examples.md`.
