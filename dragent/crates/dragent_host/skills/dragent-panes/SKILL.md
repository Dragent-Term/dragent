---
name: dragent-panes
description: "Arrange the tabs and split panes of the Dragent workspace you're running in: list them, open terminals or browser panes beside a pane, move, swap, resize or unsplit panes, focus or close one. Use for the visible layout only — not for starting agents or teams (dragent-orchestrate) or reading web pages (dragent-browser)."
---

# Workspace panes

Use this when the user wants something *shown* in Dragent: a dev server beside
the terminal, tests below it, a browser next to the code, panes rearranged or
closed. Starting agents to do work is `dragent-orchestrate`; reading or clicking
a page is `dragent-browser`; reporting on agents is `dragent-standup`.

## Words, and what each action does to them

- **Workspace tab** — one of the window's top tabs.
- **Tab** — a terminal, browser or settings view in a workspace tab, listed in its
  sidebar. A *split* shows several tabs side by side; a split exists only while
  it's on screen.
- **Pane** — a tab as it appears on screen. Pane ids are tab ids.
- **Terminal session** — the shell behind a terminal tab. **Agent session** — an
  agent running in one.

| Action | The tab | Its session |
|---|---|---|
| `unsplit` | becomes its own sidebar tab | keeps running |
| `move` / `swap` / `resize` | changes place or size | keeps running |
| `close` | is removed | **ends** (the person confirms if something runs) |

## Prerequisites

The Dragent app must be open, and you should run inside a Dragent terminal: commands
act relative to *your* pane, marked `"you"`. From elsewhere, name panes explicitly
(`--target ID`). Check what this build supports with `dragent panes help` (or the
`panes_*` / `pane_*` tools in `dragent capabilities --json`).

## Workflow

1. **Look first.** `dragent panes list` returns every workspace tab with its panes
   (`id`, `kind`, `title`, `visible`, `in_split`, `active`, `you`, `running`) and
   its `layout` tree (splits with `arrangement` and each child's `share`). Ids
   change between sessions: always take them from a fresh list.
2. **Change the layout** with one command per step:

   ```sh
   dragent panes open --right --label "dev server" npm run dev
   dragent panes open --browser --down --label "app" localhost:3000
   dragent panes open --label "notes"                 # a new sidebar tab, not shown
   dragent panes open --label "tests" cargo test      # a new terminal on your right
   dragent panes move 12 --target 4 --down            # show tab 12's session below pane 4
   dragent panes swap 12 --with 4
   dragent panes resize 12 --share 0.3
   dragent panes equalize
   dragent panes unsplit 12                           # back to its own tab, still running
   dragent panes focus 12
   dragent panes close 12
   ```

   A terminal command opens a **new pane on your right** by default, preserving
   your agent's terminal and session. Use `--tab` only when a separate sidebar
   tab is wanted. Never run a shell command by sending it into an agent's REPL
   or replacing the terminal where you are working.

   Split commands act beside a pane that is **on screen** (`"visible": true`):
   yours by default, or `--target ID`. The keyboard stays where the person has it
   unless you add `--focus` — use that only when the user asked to be taken there.
3. **Read the answer.** Every change returns JSON: `pane` (the new or affected id),
   `focus` (`kept` and where the keyboard is) and `workspace_tab` (the updated
   panes and layout). Failures follow the shared result contract (`status`, `code`,
   `next`).
4. **Verify the outcome, not just the layout.** A terminal opening doesn't mean its
   command worked: check the server responds (`dragent-browser`), or have the
   command write a log you can read (`cargo test 2>&1 | tee /tmp/dragent-tests.log`).
   There's no command for reading a plain terminal's screen.

## Failures and recovery

| status / code | Meaning | Do |
|---|---|---|
| `invalid_target` / `no_such_pane` | The id is gone or never existed | List again; use a current id |
| `invalid_target` / `pane_not_shown` | The target isn't on screen | Pick a visible pane, or `focus` it first if the user agrees |
| `invalid_target` / `no_caller_pane` | You aren't in a Dragent terminal | Pass `--target ID` from `list` |
| `invalid_target` / `not_in_split` | unsplit/swap/resize on a pane outside the split | List and check `in_split` |
| `invalid_input` / `own_pane` | You tried to close your own pane | Don't; end your session instead |
| `needs_approval` / `close_confirmation` | The person was asked to confirm a close | Wait; never answer it or retry to get round it |
| `unavailable` / `app_not_open` | Dragent isn't open | Tell the user; don't start it just to arrange panes |
| `timeout` / `no_answer` | The app didn't answer in time | **Don't open again.** `dragent panes list` and look for your `--label`; `dragent requests show ID` |

Open panes with a `--label` so a timeout never makes you open a duplicate, and
use `--idempotency-key` for an open you might retry.

## What you create, and cleaning up

You create tabs and sessions. Close the ones you opened for this task when they're
no longer needed, after checking what runs there. Never close, move away or
unsplit panes you didn't create unless the user asked, never close the pane you
run in, and never send keystrokes to get past a close confirmation.

## Report

Say which panes you opened or changed (ids and labels), where they are, whether the
keyboard moved, what you verified about their content, and anything left running.

More layouts (dev server + browser, tests below, watching workers, rearranging):
`references/recipes.md`.
