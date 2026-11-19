# Worked feedback examples

All commands below were run against this build with a throwaway home
(`DRAGENT_HOME=$(mktemp -d)`, `--dragent-cli` first). `--draft` never sends.

## 1. Bug: pane spins forever

You already diagnosed with `dragent-doctor`: `dragent status --json` shows
the app running, `agents running` shows the stuck session, and the terminal
shows `watcher timeout after 10s`. Reuse that; do not ask for it again.

```sh
dragent feedback --draft --kind bug \
  --observed "Workspace pane shows a spinner forever" \
  --expected "File list appears within seconds" \
  --steps "1. Open workspace
2. Watch the pane" \
  --frequency always \
  --cause "Maybe the watcher misses the first event (unverified)" \
  --evidence "watcher timeout after 10s (redacted)"
```

Why `--draft` first: the person spots a wrong step or a secret before
anything leaves the machine. Review the printed draft with them.

Send once they approve (needs app open and signed in):

```sh
dragent feedback --kind bug --observed "Workspace pane shows a spinner forever" \
  --expected "File list appears within seconds" \
  --steps "1. Open workspace
2. Watch the pane" --frequency always \
  --cause "Maybe the watcher misses the first event (unverified)" \
  --evidence "watcher timeout after 10s (redacted)" --json
```

`{"sent": true, ...}` means the app accepted it. Anything else was not sent.

## 2. Feature request: dark-mode toggle

```sh
dragent feedback --draft --kind feature \
  --observed "No dark-mode toggle in Settings on this build" \
  --expected "A Settings toggle that switches the theme" \
  --body "Who it affects: night-shift reviewers. Workaround: OS dark mode, which misses the app chrome."
```

Why this shape: who cannot do what, who it affects, current workaround,
then the desired behavior. Implementation ideas stay secondary.

## 3. General feedback with a body file

```sh
printf 'Overall fast, but the first workspace open feels slow.\n' > /tmp/fb-body.txt
dragent feedback --draft --kind general --body-file /tmp/fb-body.txt
dragent feedback --kind general --body-file /tmp/fb-body.txt --json
```

Why a file: long text survives quoting intact. Missing files fail before
sending (`couldn't read ...`), so fix the path and retry.

## 4. One-line general note (old form still works)

```sh
dragent feedback --draft "Diff view is great, thank you"
```

Positional words feed Additional detail with kind `general`.
