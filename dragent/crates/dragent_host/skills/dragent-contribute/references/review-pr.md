# Review, commit and PR preparation

## Keep the change reviewable

```sh
git diff --stat
git diff
git status --short
```

Why in this order: `--stat` shows scope creep first; full `diff` shows
content; `status` shows untracked files you forgot. Avoid repository-wide
formatting for a local edit. If `git status` shows files you did not mean
to touch — especially another session's work — leave them alone and say so
in the report. Never commit, push or open anything the person did not ask
for.

## Discover branch and remote, never hardcode

```sh
git branch --show-current
git remote -v
git log --oneline -5
```

Use the returned branch as the base context and the returned remote (often
`origin`, but verify) for any fetch, push or PR command. Do not guess
`main`, `master` or `origin`. For PR metadata, check `.github/` for a pull
request template first; this checkout has workflows only, no template, so
write the summary yourself.

## Commit, push and PR only on request

Commit on your current branch only when asked, with a clear message; do not
push, do not touch other branches. Example once authorized:

```sh
git status --short
git diff --stat
git commit -m "Short imperative summary"
```

Report exactly what happened: which files, whether anything was committed
(SHA), pushed (remote, branch) or opened (PR URL/number) versus left
uncommitted. Publication, merging and release actions follow the user's
scope; do not deploy a running app or change sign-in to obtain evidence.

## Final report contents

Concrete problem, resulting behavior, files changed, every check with its
result (e.g. `cargo test -p dragent --lib cli::account: 6 passed`),
preview outcome for visible changes, what remains untested, and the precise
commit/push/PR state. Name native platform checks and runtime testing as
separate evidence. State limitations and follow-ups, or none.
