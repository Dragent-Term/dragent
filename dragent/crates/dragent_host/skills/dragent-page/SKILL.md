---
name: dragent-page
description: "Create or revise a self-contained HTML page, report or interactive explainer and preview it in Dragent’s browser. Publish only through an available authorized hosting tool; Dragent has no page-publish CLI."
---

# Build and preview a standalone page

## Connection and context

Use Dragent’s connected tools when they expose the needed operation; otherwise use
its CLI. In a Dragent terminal, `dragent` is on PATH. Else use
`~/.dragent/bin/dragent` on macOS/Linux or `%USERPROFILE%\.dragent\bin\dragent.cmd`
on Windows. Start with `dragent status` and `dragent help`. Browser, panes,
computer, teams, workspaces, automations and integrations also expose their own
help commands; other command groups are documented in the top-level help.
Commands operate in the current organisation and often the caller’s workspace.
Do not change either silently. If the app, sign-in or command is unavailable,
explain the missing prerequisite; do not fabricate results or retry indefinitely.

## Establish the deliverable

Identify the audience, question the page should answer, source evidence and whether
the user wants a local preview, a file or a published URL. Use the requested format:
this skill produces HTML, not a substitute for an explicitly requested document,
spreadsheet or slide deck. For an existing page inspect its source and preserve the
user’s content and style unless redesign is part of the task.

Create a useful, self-contained artifact using the repository’s existing tools when
appropriate. Use semantic sections, clear hierarchy, readable contrast, responsive
layout and keyboard-operable controls. Charts need units, legends and accessible
summaries. Cite evidence near the claims it supports; distinguish measured facts,
assumptions and illustrative data. Do not place credentials or hidden private data
in client-side JavaScript or assets that may later become public.

## Preview the real file

Write the page to a concrete path. Start a local server scoped to its output folder
using an available runtime, for example `python3 -m http.server 8765 --bind 127.0.0.1
--directory /absolute/output`. Use a free port rather than killing another process.
Open its URL with `dragent browser open http://127.0.0.1:8765` and inspect snapshot,
console and a screenshot. Test important controls, narrow layouts and links; a
successful server start does not verify the page. Keep track of the temporary
server and stop only that process when cleanup is appropriate.

## Hosting is a separate capability

This build has no `dragent page publish` command and no hosted-page comment system
exposed by this skill. A localhost address or local file path is not a shareable
hosted URL. If the user requested publication and an actual hosting integration or
CLI is configured, inspect its supported workflow, target and access settings,
finish the page first, and publish within the user’s authorization. Verify the
returned URL independently. Otherwise deliver the HTML file and local preview and
state that hosting is not available; never invent a deployment result or comments.

For changes requested in feedback, identify the page/version and update the source
before republishing. Finish with the file or verified URL, what was tested and any
external service the published page depends on.
