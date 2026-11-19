# Worked examples

References (`e12`), ids and addresses below are illustrations: always use the
ones your own `list` and `snapshot` return.

## Testing a local dev server

```sh
dragent browser open localhost:5173
# Waiting for localhost:5173: nothing is answering there yet.
```

Nothing listens yet: start the server (its README or `package.json` says how)
in a pane of its own (`dragent-panes`), then the page opens by itself:

```sh
dragent browser wait --for "Sign in" --idle --timeout 30
dragent browser snapshot
dragent browser console --errors     # a clean start has none
```

Verify what the person asked about by what the page shows, not by the server
having started.

## A multi-step form

```sh
dragent browser snapshot
#   [e21] input[email] "Work email" (required)
#   [e22] combobox "Team size" options: 1-10 | 11-50 | 51+
#   [e23] input[checkbox] "I agree to the terms" (unchecked)
#   [e24] button "Continue" (disabled)
dragent browser type e21 "ada@example.com"
dragent browser type e22 "11-50"
dragent browser click e23
dragent browser click e24
dragent browser wait --url /onboarding/step-2 --timeout 15
dragent browser snapshot             # new page, new references
```

`Continue` was disabled until the terms were ticked: clicking it first would
have failed as `element_disabled`.

## Validation errors

```sh
dragent browser type e21 "ada@"
# Typed into e21. The field reports: Please enter a part following '@'.
dragent browser click e24
dragent browser snapshot
# Alerts and status messages:
#   Enter a valid work email
# Elements (…):
#   [e31] input[email] "Work email" value="ada@" (required) (invalid: Please enter …)
```

Report the message the page shows (the alert) and the field's own message;
then fix the input and check the alert is gone:
`dragent browser wait --gone "Enter a valid work email"`.

## Searching or filtering a page that changes

```sh
dragent browser type e8 "invoice"
dragent browser wait --idle --timeout 10      # the search request finished, nothing re-rendering
dragent browser snapshot
```

Results that arrive in pieces (infinite scroll): `scroll bottom`, then `wait
--idle`, then snapshot again. To wait for a specific result:
`dragent browser wait --selector "li.result" --for "Invoice #1042"`.

## Diagnosing a console error

```sh
dragent browser console --clear      # start clean
dragent browser click e12            # the step that breaks
dragent browser wait --idle
dragent browser console --errors
# [error] TypeError: Cannot read properties of undefined (reading 'id') at Cart.tsx:88
# [network] POST http://localhost:3000/api/cart → 500 Internal Server Error
```

The failed request comes first in time: look at the server's log for the 500,
then at `Cart.tsx:88` for the code that assumed a response. Quote both lines in
your report.

## A stale element

```sh
dragent browser click e14
# {"status":"invalid_target","code":"stale_ref","message":"e14 isn't on the page any more …"}
dragent browser snapshot             # the list re-rendered: find the item again
dragent browser click e57
```

Never reuse a reference from an older snapshot; references don't repeat, so an
old one can only fail, never click something else.

## A submission that timed out

```sh
dragent browser click e9             # "Place order"
# {"status":"timeout","code":"no_answer","request_id":"7f3c…","next":"… requests show 7f3c …"}
dragent requests show 7f3c…          # did the app answer after all?
dragent browser status               # where is the page now?
dragent browser snapshot             # an order confirmation, or the form still?
```

Only when the page clearly shows nothing happened (the form, unchanged, no
confirmation) click again; otherwise you'd order twice.

## A dialog

```sh
dragent browser click e30            # "Delete project"
# Clicked e30. A confirm dialog appeared: “Delete this project?” (dismissed).
```

It was dismissed because nobody set an answer. Deleting is the person's call:
ask them first; with their go-ahead:

```sh
dragent browser dialog accept
dragent browser click e30
dragent browser wait --gone "My project"
```
