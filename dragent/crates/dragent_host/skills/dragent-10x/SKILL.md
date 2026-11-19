---
name: dragent-10x
description: "Audit how the user currently uses Dragent and set up a small number of relevant workflow improvements when requested. Use for workflow discovery and onboarding, not ordinary coding tasks."
---

# Improve the user's Dragent workflow

Use this skill when the user asks to get more out of Dragent, be onboarded,
or fix a recurring friction. When the request is really about something
specific, load that skill instead: what agents did (`dragent-standup`), work
across agents (`dragent-orchestrate`), schedules (`dragent-automate`),
service items (`dragent-integrations`), fresh-worktree setup
(`dragent-setup`), panes (`dragent-panes`), desktop apps
(`dragent-computer`), pages (`dragent-page`).

## Prerequisites and discovery

Needs the CLI and the current configuration, not history. Start here; all
three work signed out and while the app is closed:

```sh
dragent capabilities --json      # what this build supports (trust over memory)
dragent standup --setup --json   # current configuration: agents, automations,
                                 # services, skills, MCP servers, projects
dragent standup --help           # the recap's options, including --setup
```

Report only the configuration you read. Never infer historical adoption and
never invent time savings: a directory listing proves nothing about usage.
The setup report's schema and what each section does (and does not) tell
you: `references/setup-report.md`.

## Understand before recommending

Anchor on the friction the user describes, then inspect only the relevant
slice: the project in question (`standup --project PATH`), its workspaces
and agent states, its automations, and the connected services that could
serve it. Useful reads are `projects list`, `workspaces list`,
`agents running`, `automations list` and `integrations list` (all with
`--json`). When the evidence cannot establish the workflow, ask a focused
question (schedule? destination? who approves writes? budget for agents?)
instead of guessing. Do not recommend more agents or automation because the
app provides them.

## Recommend little, with trade-offs

Propose a small number of concrete improvements (one is often right), each
with the evidence, the expected benefit and the cost, without invented
percentages. Candidate patterns and their trade-offs:
`references/playbook.md`. Say what stays manual and what each change cannot
do, so the user can decline.

## Implement only what the user asks

Existing authorization to improve is not authorization to reconfigure: start
nothing, connect nothing, post nothing and spend nothing beyond what was
asked. Carry out the chosen change with its focused skill
(`dragent-panes`, `dragent-orchestrate`, `dragent-automate`,
`dragent-integrations`, `dragent-setup`), following that skill's own
verification. Preserve existing preferences and installed skills; do not
enable every feature or change approval settings as an optimization.

Verify the change in the real workflow, not just its settings flag: the
command, pane, automation definition or artifact the user can find again.
Show how to use it and how to undo it, what was tested, and what remains
manual. A useful result is one improvement the user understands and can
repeat. Common failures and recovery: `references/setup-report.md`.

## Report

Finish with what you inspected (commands and ids), the questions you asked
and their answers, the improvements proposed with trade-offs, the one(s)
implemented with verification output, how to undo each, and what you
deliberately left alone. This skill creates only what the user approved;
name each thing and its undo.
