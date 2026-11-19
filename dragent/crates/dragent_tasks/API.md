# Tasks API

The organisation's tasks live on the Dragent server. Every route below is
under `/api/v1/organizations/{orgID}` and admits only members of that
organisation (the same `member` gate the automations registry uses). JSON
field names and values are exactly the serde shapes in `src/lib.rs`; times
are Unix milliseconds. Errors use the server's usual `{"error": "…"}` body
with a 4xx/5xx status.

## Numbering

Each organisation has a key prefix and a counter: tasks are `DRA-1`,
`DRA-2`, … The prefix is made once, from the organisation's name (its first
three letters A–Z, uppercase; `TSK` when there are none), and never changes
in this version. Numbers are given out atomically and never reused, even
after a deletion. A task can be addressed by its id or its key everywhere a
`{task}` appears (case-insensitive key).

## Routes

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/tasks` | — | `ListResponse` |
| POST | `/tasks` | `NewTask` | `201` + `Task` |
| GET | `/tasks/{task}` | — | `TaskDetail` |
| PATCH | `/tasks/{task}` | `TaskPatch` | `Task` |
| DELETE | `/tasks/{task}` | — | `204` |
| POST | `/tasks/{task}/restore` | — | `Task` |
| POST | `/tasks/{task}/comments` | `NewComment` | `201` + `Comment` |
| PATCH | `/tasks/{task}/comments/{commentID}` | `{"body": "…"}` | `Comment` |
| DELETE | `/tasks/{task}/comments/{commentID}` | — | `204` |
| POST | `/tasks/{task}/links` | `NewLink` | `201` (new) or `200` (updated) + `TaskLink` |
| PATCH | `/tasks/{task}/links/{linkID}` | `{"title"?, "url"?, "state"?}` | `TaskLink` |
| DELETE | `/tasks/{task}/links/{linkID}` | — | `204` |
| GET | `/task-labels` | — | `{"labels": ["bug", …]}` (every label in use, sorted) |

Assignee pickers use the existing `GET /members`.

## Listing

`GET /tasks` takes the `ListQuery` fields as query parameters:

- `status`: `open` (default: backlog, todo, in progress, in review),
  `closed` (done, canceled), `all`, or status keys joined by commas.
- `assignee`: `me` (the caller), `none`, or a member's user id.
- `label`: tasks carrying that label. `project`: a `ProjectRef.key`.
- `q`: case-insensitive words, all of which appear in the key, title or
  description.
- `updated_after_ms`: only tasks changed after that time, including any
  status; `deleted_ids` then lists tasks deleted after it (so a client can
  refresh incrementally).
- `limit` (default 200, at most 500) and `cursor` (opaque, from
  `next_cursor`; absent on the last page).

Order: most recently updated first. Deleted tasks never appear in `tasks`.
`prefix` is always the organisation's key prefix.

## Rules

- **Validation.** Title: 1–500 characters after trimming. Description: at
  most 100 000 characters. Labels: at most 20, each 1–40 characters, trimmed,
  stored lowercase, duplicates dropped. Comment body: 1–20 000 characters.
  Link title 1–300, reference 1–1000, url http(s) only. `assignee_id` must be
  a member of the organisation. Unknown fields are ignored.
- **Status times.** `started_at_ms` is set the first time a task goes in
  progress (or in review) and kept after. `completed_at_ms` is set when it
  goes done or canceled and cleared when it reopens.
- **History.** Creating, every changed field of a PATCH (one entry per
  field, only when the value really changes; `description` carries no
  values), deleting, restoring, and adding or removing a link each add an
  `Activity` entry with the caller as actor. `by_agent: true` in a body
  makes the actor kind `agent` (still the caller's account); otherwise
  `person`. `updated_at_ms` moves on every change, comments included.
- **Comments.** Only the author edits or deletes their comment; an
  organisation admin may delete any. `comment_count` counts live comments.
- **Links.** A link is unique per task by `kind` + `reference`: posting the
  same pair again updates its title, url and state (`200`) instead of
  adding one, and adds no history entry unless it was new.
- **Deleting.** Soft: the task leaves every list and detail answers `404`
  until restored. The creator or an admin may delete; anyone may restore
  within 30 days. Numbers stay taken.
- **Concurrency.** A PATCH changes only the fields it carries; the last
  write wins per field.

## Desktop side

The desktop reaches these routes through its signed-in API client and keeps
the last list per organisation on disk, so the Tasks page opens instantly
and reads while offline (changes need the server). The page refreshes with
`updated_after_ms` every 30 seconds while it's open, and after each change it
makes.
