# Worked examples

Ids, branches and answers below are shortened; always use the ones your
commands return.

## Side-by-side work, verified and integrated

The person: "Add email invites: API with Codex, the page with Claude, in
parallel." The repository has their uncommitted work on `main`.

```sh
dragent team create "Email invites" --agents codex,claude --project /abs/app
dragent tasks add "Invite API" --agent codex --owns server/invites/ \
  --prompt "Add POST /invites in server/invites/: validate the email, store a token that expires in 7 days. cargo test -p server must pass. Done: endpoint + tests committed."
dragent tasks add "Invite page" --agent claude --owns web/src/invite/ \
  --prompt "Build web/src/invite/InvitePage.tsx calling POST /invites {email}. npm test must pass."
dragent tasks add "QA invites" --qa --integrated --agent claude \
  --prompt "Start the app (README). Invite, accept, expired link, invalid email, double invite."
dragent team show --text        # show the person; wait for go-ahead
dragent tasks start              # t1 and t2 start from a snapshot of /abs/app
dragent tasks wait --timeout 100
```

`t1` reports done. Verify before taking it:

```sh
dragent tasks diff t1 --json
# {"files":[{"path":"server/invites/mod.rs","status":"added"},
#           {"path":"Cargo.lock","status":"modified"}],
#  "outside_area":["Cargo.lock"], "stage":"implemented", …}
dragent tasks show t1 --json     # "workspace": "/abs/wt/invite-api-t1-…"
(cd /abs/wt/invite-api-t1-… && cargo test -p server)
```

`Cargo.lock` outside its area is expected (a new dependency); a change to
another task's area would be a reason to ask the worker why. Then:

```sh
dragent tasks integrate t1 --check "cargo test -p server: pass"
# Integrated into /abs/app, uncommitted:
# t1: 2 files
(cd /abs/app && git status --short && cargo test -p server)
```

The same for `t2`. Once both are integrated, `t3` (QA) starts on its own in
a snapshot of `/abs/app` with everything in it. When it approves, report to
the person: what is uncommitted in their checkout per task, how each was
checked, and QA's flows.

## A conflict

```sh
dragent tasks integrate t2
# {"status":"failed","code":"integration_conflict",
#  "message":"t2's changes don't apply to /abs/app as it is now: web/src/App.tsx; nothing of t2 was changed. …",
#  "next":"Send it back to be brought up to date with the checkout …"}
```

The person edited `web/src/App.tsx` after `t2` started. Nothing of `t2` was
applied. Either ask the person how to combine the two, or send it back:

```sh
dragent tasks reopen t2 --feedback "web/src/App.tsx changed in the person's checkout since you started (they added a router). Rebase your change onto that version: the current file is attached below. …"
```

With several tasks in one call, the ones before the conflict stay integrated
and the answer's status is `partial`: integrate the stopped task alone once
it's fixed.

## QA of the integrated work finds problems

```sh
dragent tasks wait
# - t3 found problems in the integrated work: Inviting the same email twice
#   returns 500. Send the work responsible back (…), integrate the fix, and
#   t3 tests again on its own.
dragent tasks reopen t1 --feedback "QA: a second invite to the same email returns 500. Return 409 with a message; add a test."
# … t1 done again → verify → integrate again (only its new commits apply):
dragent tasks integrate t1 --check "cargo test -p server: pass"
```

`t3` waited for "an integrated fix"; once `t1` is integrated again it starts
on its own.

## A worker waits at a permission prompt

```sh
dragent tasks wait
# - The worker on t2 (Invite page) is waiting at a permission prompt …
dragent tasks read t2 --lines 20
#   △ Permission required
#     ← Access external directory /tmp/invite-check
#   Allow once   Allow always   Reject
```

It's a scratch folder for the task's own check: allow it once.

```sh
dragent tasks keys t2 enter --json
# {"answered_prompt": true, "screen_after": "… esc interrupt …", …}
```

`screen_after` shows the worker carrying on, so it's done. Had it shown a
confirmation ("Always allow … Confirm / Cancel"), answer that step too. A
prompt to run `git push` or to read `~/.aws` is the person's: leave it and
`dragent msg send user "t2 wants to push to origin: allow it?" --kind
needs_user`. With answering turned off for the team, `tasks keys` answers
`needs_approval` / `person_answers_prompts`: tell the person instead.

## Work that builds on several tasks

`t4 "Wire page to API" --after t1,t2` waits for "t1 to be integrated, t2 to
be integrated", then starts from the person's checkout, where both already
are. Work that builds on a single task starts from that task's branch instead,
so integrate the earlier task first: `dragent tasks integrate t1 t4` applies
them in plan order.

## Taking over

```sh
dragent team list --json         # the team whose lead stopped
dragent team show --text --team team-1a2b3c4d
dragent msg inbox --team team-1a2b3c4d
```

Tell the person which tasks are done, which are integrated, which are
running or blocked, and what's waiting for an answer; then carry on with
`tasks wait`.
