# Feedback draft fields

`dragent feedback --help` lists every flag this build accepts. All fields
stay client-side in the draft text: the backend takes text only
(`Request::Feedback { text }`), so no flag invents a new API field.

## Fields

- `--kind KIND` — `bug`, `feature` or `general` (default `general`).
  Anything else is `invalid_input` / `feedback_kind`.
- `--observed TEXT` — measured observation: what you saw. Paste output,
  not interpretation.
- `--expected TEXT` — what should happen instead (bugs), or the desired
  behavior (features).
- `--steps TEXT` — minimal repeat sequence, one step per line.
- `--frequency TEXT` — free text, e.g. `always`, `sometimes`, `once`.
- `--cause TEXT` — suspected cause, always separated from observations and
  labelled unverified in the draft.
- `--evidence TEXT` — redacted pasted text only (log excerpt, error line).
  A local path written here is NOT uploaded; the draft says so explicitly.
  Never paste tokens, secret environment values or full terminal history.
- `--body TEXT` and `--body-file FILE` — additional free-form detail.
  `--body-file` reads the file as-is; missing files fail before sending.
  Plain positional words (`dragent feedback TEXT`) also feed this section,
  so the old one-line form keeps working.
- `--draft` — print exactly what would be sent, without sending. Text mode
  prints the raw draft; `--json` wraps it as `{"sent": false, "draft": ...}`.
  `--draft` works signed out and with the app closed.

Omit empty fields; the draft shows only sections you filled, plus always:

- `App version:` from the CLI build, `OS:` as `OS ARCH`
- footer noting account and organisation are added by the app on send

## Where private feedback goes and what is transmitted

Destination: the Dragent team (private) via `POST /api/v1/feedback`, sent
by the open app as the signed-in user. Transmitted: the draft text above,
plus account email, organisation name/id, app version and OS added by the
app. Nothing is published publicly. The CLI sends no attachments: no
screenshots, no log files, no diagnostics bundle. If the UI dialog offers
image or log attachment, that is the dialog only; do not describe the CLI
as uploading anything.

Redact before sending: the draft is what leaves the machine. Strip tokens,
passwords, private URLs and unrelated user text from `--evidence` and
`--body` first.
