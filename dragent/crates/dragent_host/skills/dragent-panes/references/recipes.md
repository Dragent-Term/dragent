# Pane recipes

Each recipe starts from `dragent panes list` and uses ids from that answer. Replace
the commands with the ones this repository really uses (its README, `package.json`
scripts, `Makefile`, …).

## Dev server beside a browser

```sh
dragent panes list
dragent panes open --right --label "dev server" npm run dev
# → {"pane": 14, "focus": {"kept": true, …}, "workspace_tab": {…}}
dragent panes open --browser --down --target 14 --label "app" localhost:3000
```

Why `--target 14`: the browser goes under the server, not under you. The server
needs a moment: if the page shows a connection error, reload it through
`dragent-browser` once the server is listening rather than opening another browser.
Verify by reading the page (`dragent-browser`), not by the pane existing.

## Tests below the terminal

```sh
dragent panes open --down --label "tests" "cargo test 2>&1 | tee /tmp/dragent-tests.log"
```

Quote the whole command when it has pipes or redirects, so the pane's shell gets
it intact. Read `/tmp/dragent-tests.log` for the result; the pane closing or the
command finishing doesn't tell you whether tests passed.

## Watching workers

When a team or several agents run in other tabs of this workspace tab, show them
side by side without taking the keyboard:

```sh
dragent panes list                         # find the workers' tab ids (kind "terminal")
dragent panes move 21 --target 4 --right   # worker A beside you
dragent panes move 22 --target 21 --down   # worker B under worker A
dragent panes equalize
```

`move` shows an existing session; nothing restarts. Watching isn't steering:
messages to workers go through `dragent-orchestrate`, never keystrokes typed into
their panes.

## Rearranging what's there

```sh
dragent panes swap 21 --with 22            # exchange places
dragent panes resize 4 --share 0.6         # give your pane 60% of its split
dragent panes unsplit 22                   # back to its own tab, still running
```

Shares are a pane's part of the split directly containing it (0.1–0.9); its
neighbours keep their proportions. `equalize` evens out every split on screen.

## After a timeout

```sh
dragent panes list            # is a pane with your --label there?
dragent requests show REQUEST_ID
```

If the labelled pane exists, use it. Open again only when it doesn't. If the first
try had an `--idempotency-key`, repeating that key is refused (its outcome is
unknown): once you've confirmed there's no pane, open again with a new key.

## Closing what you opened

```sh
dragent panes list            # check "running" and that it's yours (your --label)
dragent panes close 14
```

If the answer is `needs_approval`, the person was asked to confirm because
something still runs there. Tell them what it is and wait.
