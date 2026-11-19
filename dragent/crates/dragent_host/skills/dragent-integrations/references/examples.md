# Worked examples

Services, ids and addresses below are illustrations: use what your own
`describe`, `search` and `show` return. Nothing here posts anything unless the
person asked for it.

## Search, then read the full item

```sh
dragent integrations describe sentry --json      # accounts, identifiers, scope.config.organization
dragent integrations search sentry "checkout" --json
```

```json
{"items": [
  {"id": "BACKEND-3K", "title": "TimeoutError in /checkout", "detail": "backend · unresolved", "url": "https://sentry.example/organizations/acme/issues/4412/"},
  {"id": "BACKEND-2Z", "title": "TimeoutError in /checkout/confirm", "detail": "backend · resolved", "url": "https://sentry.example/organizations/acme/issues/4290/"}
 ],
 "count": 2, "truncated": false, "service": "sentry", "account": {"selector": "team", "label": "acme"}}
```

```sh
dragent integrations show sentry BACKEND-3K --json
```

Quote the item's status, the latest stack frame and its `url`; the preview in
`search` is not enough to act on.

## Choosing between similar results

Two hits look right. Read both, compare: state (unresolved or resolved),
last-seen date, route, and whether the comments mention a fix. Say it plainly:
"BACKEND-3K is unresolved and last seen today; BACKEND-2Z was resolved in
March, so I took BACKEND-3K." If they are equally plausible, ask the person
instead of picking.

## An authorized update

The person said: "comment on ENG-142 that the fix is in PR 88".

```sh
dragent integrations show linear ENG-142 --json     # confirm it is the right issue
dragent integrations comment linear ENG-142 --body-file /absolute/comment.md --json
dragent integrations show linear ENG-142 --json     # the comment is in the text
```

Report the issue `url`, the account named in the answer, and that you read it
back. Do not move the status unless asked.

## Following pagination

`search` is one page of 40 with no cursor. When `truncated` is true:

```sh
dragent integrations search linear "checkout label:bug" --json
dragent integrations search linear "checkout label:bug updated:>2026-10-01" --json
```

For a service's own cursor, use `api` with the paging style from `describe`:

```sh
dragent integrations api gitlab /projects/PROJECT_ID/issues --query state=opened --query per_page=100 --query page=1 --json
dragent integrations api gitlab /projects/PROJECT_ID/issues --query state=opened --query per_page=100 --query page=2 --json
```

Stop on an empty array; say how many pages you read. When a service gives the
next cursor in a header that `api` does not return (Sentry), narrow instead
and say the list may be incomplete.

## Recovering from an ambiguous write timeout

```json
{"ok": false, "status": "timeout", "code": "write_outcome_unknown",
 "message": "Couldn't reach Linear: ... timed out", "retry_safe": false}
```

The comment may exist. Do not post it again.

```sh
dragent integrations show linear ENG-142 --json     # is your text already in the comments?
```

If the text is there, you are done: report it. If not, post it once more. For
`create`, search the exact title first. For `status`, the read shows the
current state. When the app answered late, `dragent requests show ID` gives
the outcome of the original request.

## Expired key versus missing permission

```json
{"status": "needs_auth", "code": "credentials_expired",
 "next": "Ask the person to reconnect this account ..."}
```
Ask the person to reconnect it: that is the fix.

```json
{"status": "needs_auth", "code": "insufficient_permission",
 "next": "Reconnecting the same account will not help ..."}
```

The key works; the account cannot see this project. Check `describe` for
another account (`--account team`), or ask the owner of the project for
access. Reconnecting would fail the same way. If nothing else is available,
say what you could not read and continue with local evidence.
