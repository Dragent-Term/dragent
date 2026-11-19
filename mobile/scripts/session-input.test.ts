import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MAX_COMMAND_HISTORY,
  TERMINAL_KEYSTROKES,
  canRecallNewer,
  canRecallOlder,
  canSendToSession,
  commandToSend,
  draftReducer,
  initialDrafts,
  isUsableCommand,
  isUsableKeystrokeText,
  keystrokeBytes,
  keystrokesToSend,
  pushCommandHistory,
  queryToSend,
  prepareAgentDraft,
  sendBlockerReason,
  stepCommandHistory,
  tooLargeMessage,
} from "../src/features/shared/sessionInput";
import { MAX_WRITE_BYTES, prepareSessionText } from "../src/transport/frames";

test("terminal keys use canonical escape sequences", () => {
  const bytes: Record<string, string> = Object.fromEntries(
    TERMINAL_KEYSTROKES.map((key) => [key.id, key.bytes]),
  );
  assert.equal(bytes["escape"], "\u001b");
  assert.equal(bytes["tab"], "\t");
  assert.equal(bytes["enter"], "\r");
  assert.equal(bytes["up"], "\u001b[A");
  assert.equal(bytes["down"], "\u001b[B");
  assert.equal(bytes["left"], "\u001b[D");
  assert.equal(bytes["right"], "\u001b[C");
  assert.equal(bytes["backspace"], "\u007f");
  assert.equal(bytes["ctrlC"], "\u0003");
  assert.equal(bytes["ctrlD"], "\u0004");
  assert.equal(TERMINAL_KEYSTROKES.length, 10);
  assert.equal(new Set(TERMINAL_KEYSTROKES.map((key) => key.id)).size, 10);
  for (const key of TERMINAL_KEYSTROKES) {
    assert.ok(key.bytes.length > 0, `${key.id} sends nothing`);
    assert.ok(key.label.length > 0 && key.hint.length > 0, `${key.id} needs UI labels`);
  }
});

test("keystroke lookup resolves every id and nothing else", () => {
  for (const key of TERMINAL_KEYSTROKES) {
    assert.equal(keystrokeBytes(key.id), key.bytes);
  }
  assert.equal(keystrokeBytes("meta" as never), null);
});

test("sends never silently append Enter", () => {
  assert.equal(commandToSend("ls"), "ls");
  assert.equal(keystrokesToSend("ls"), "ls");
  // Only the explicit Enter key carries a carriage return.
  for (const key of TERMINAL_KEYSTROKES) {
    if (key.id === "enter") continue;
    assert.ok(!key.bytes.includes("\r") && !key.bytes.includes("\n"), `${key.id} must not submit`);
  }
  assert.equal(keystrokeBytes("enter"), "\r");
});

test("buffered text preserves Unicode and line breaks byte-for-byte", () => {
  const paste = "echo café 👩🏽‍💻 —–\nls $(pwd)\n";
  assert.equal(keystrokesToSend(paste), paste);
  assert.equal(commandToSend("  printf 'héllo'  "), "  printf 'héllo'  ");
  // Pastes are data, never an implicit submit.
  assert.ok(!keystrokesToSend(paste)!.endsWith("\r"));
});

test("empty drafts cannot send, but whitespace keystrokes stay meaningful", () => {
  assert.equal(commandToSend(""), null);
  assert.equal(commandToSend("   "), null);
  assert.equal(keystrokesToSend(""), null);
  assert.equal(keystrokesToSend(" "), " ");
  assert.equal(keystrokesToSend("\n"), "\n");
  assert.equal(queryToSend(""), null);
  assert.equal(queryToSend("   "), null);
  assert.equal(isUsableCommand("  ls "), true);
  assert.equal(isUsableKeystrokeText(" "), true);
});

test("agent questions use shared trimming while commands stay verbatim", () => {
  assert.equal(queryToSend("  what changed?  "), "what changed?");
  assert.equal(queryToSend("what changed?"), "what changed?");
  // Commands keep their exact bytes; only the transport validates them.
  assert.equal(commandToSend("  ls  "), "  ls  ");
});

test("agent usability and empty feedback use Go whitespace, including NEL and excluding BOM", () => {
  for (const value of ["", " \r\n\t", "\u0085", "\u0085\u00a0\u1680\u2000\u2028\u202f\u205f\u3000"]) {
    const draft = prepareAgentDraft(value);
    assert.equal(draft.ok, false);
    assert.equal(queryToSend(value), null);
    assert.ok(!draft.ok);
    assert.equal(draft.error, "empty");
    assert.match(draft.message, /Type a question/);
    assert.doesNotMatch(draft.message, /large|limit|bytes/i);
  }
  for (const value of ["\ufeff", "\u0085 question \u3000", "\u0085\ufeff\u0085"]) {
    const draft = prepareAgentDraft(value);
    const transport = prepareSessionText("agentQuery", value);
    assert.deepEqual(draft, transport);
    assert.ok(draft.ok);
    assert.equal(queryToSend(value), draft.text);
  }
});

test("agent byte-limit feedback is distinct from empty/invalid feedback and follows Go trimming", () => {
  const atLimit = "é".repeat(MAX_WRITE_BYTES / 2);
  const valid = prepareAgentDraft(`\u0085 ${atLimit}\u3000`);
  assert.ok(valid.ok);
  assert.equal(valid.byteLength, MAX_WRITE_BYTES);
  assert.equal(valid.text, atLimit);
  const oversized = prepareAgentDraft(atLimit + "é");
  assert.ok(!oversized.ok);
  assert.equal(oversized.error, "too-large");
  assert.equal(oversized.message, tooLargeMessage());
  const invalid = prepareAgentDraft(null);
  assert.ok(!invalid.ok);
  assert.equal(invalid.error, "invalid-text");
  assert.doesNotMatch(invalid.message, /large|limit|bytes/i);
});

test("oversized drafts fail through the shared byte boundary with clear feedback", () => {
  // 64 KiB is the shared transport cap: an ASCII payload just over it fails.
  const overAscii = "x".repeat(64 * 1024 + 1);
  assert.equal(commandToSend(overAscii), null);
  assert.equal(keystrokesToSend(overAscii), null);
  assert.equal(queryToSend(overAscii), null);
  // Multi-byte text fails on bytes, not characters: 40k é is 80k bytes.
  const overBytes = "é".repeat(40_000);
  assert.ok(overBytes.length < 64 * 1024);
  assert.equal(keystrokesToSend(overBytes), null);
  assert.equal(commandToSend(`ok ${overBytes}`), null);
  // At the boundary, payloads still pass through untouched.
  const atLimit = "x".repeat(64 * 1024);
  assert.equal(keystrokesToSend(atLimit), atLimit);
  assert.match(tooLargeMessage(), /64|limit/i);
});

test("draft reducer edits, clears on success, and resets on session switch", () => {
  const empty = initialDrafts("s1");
  assert.deepEqual(empty, { sessionId: "s1", command: "", keystrokes: "", query: "" });

  const typed = draftReducer(empty, { type: "edit", field: "command", value: "ls" });
  assert.equal(typed.command, "ls");
  assert.equal(typed.keystrokes, "");
  // No-op edits keep referential identity (no wasted renders).
  assert.equal(draftReducer(typed, { type: "edit", field: "command", value: "ls" }), typed);

  const sent = draftReducer(typed, { type: "sent", field: "command" });
  assert.equal(sent.command, "");
  assert.equal(draftReducer(sent, { type: "sent", field: "command" }), sent);

  const other = draftReducer(typed, { type: "edit", field: "query", value: "q" });
  assert.equal(draftReducer(other, { type: "sent", field: "query" }).command, "ls");

  const switched = draftReducer(other, { type: "session", sessionId: "s2" });
  assert.deepEqual(switched, initialDrafts("s2"));
  assert.equal(draftReducer(switched, { type: "session", sessionId: "s2" }), switched);
});

test("composer send flow retains drafts on failure and clears on success", () => {
  // Mirrors the composer actions: gate the payload, call the writer, and
  // dispatch `sent` only when the transport accepted the write.
  const sent: string[] = [];
  const flakyWriter = (ok: boolean) => (payload: string): boolean => {
    if (ok) sent.push(payload);
    return ok;
  };
  const attempt = (draft: string, write: (payload: string) => boolean) => {
    let state = draftReducer(initialDrafts("s1"), { type: "edit", field: "command", value: draft });
    const payload = commandToSend(state.command);
    if (payload === null) return { sent: false as const, draft: state.command };
    const accepted = write(payload);
    if (accepted) state = draftReducer(state, { type: "sent", field: "command" });
    return { sent: accepted, draft: state.command };
  };

  assert.deepEqual(attempt("ls -la", flakyWriter(false)), { sent: false, draft: "ls -la" });
  assert.deepEqual(sent, []);
  assert.deepEqual(attempt("ls -la", flakyWriter(true)), { sent: true, draft: "" });
  assert.deepEqual(sent, ["ls -la"]);
  // Empty drafts never reach the writer at all.
  assert.deepEqual(attempt("   ", flakyWriter(true)), { sent: false, draft: "   " });
  assert.deepEqual(sent, ["ls -la"]);
});

test("drafts survive role flaps and disconnects without a session switch", () => {
  // The composer stays mounted across role/status changes (only `session`
  // actions reset), so a failed send followed by recovery keeps its text.
  let state = draftReducer(initialDrafts("s1"), { type: "edit", field: "keystrokes", value: "partial" });
  const payload = keystrokesToSend(state.keystrokes);
  assert.equal(payload, "partial");
  // Transport rejects while disconnected: no `sent` action is dispatched.
  assert.equal(state.keystrokes, "partial");
  // Recovery sends the same retained text.
  state = draftReducer(state, { type: "sent", field: "keystrokes" });
  assert.equal(state.keystrokes, "");
});

test("writes require a live controller connection", () => {
  assert.equal(canSendToSession({ status: "connected", role: "controller" }), true);
  for (const gate of [
    { status: "connected", role: "viewer" },
    { status: "connected", role: null },
    { status: "connecting", role: "controller" },
    { status: "reconnecting", role: "controller" },
    { status: "idle", role: null },
    { status: "ended", role: "viewer" },
    { status: "error", role: "controller" },
  ]) {
    assert.equal(canSendToSession(gate), false, JSON.stringify(gate));
    assert.ok(typeof sendBlockerReason(gate) === "string" && sendBlockerReason(gate)!.length > 0);
  }
  assert.equal(sendBlockerReason({ status: "connected", role: "controller" }), null);
  assert.match(sendBlockerReason({ status: "connected", role: "viewer" })!, /Edit/);
  assert.match(sendBlockerReason({ status: "idle", role: null })!, /Connect/);
});

test("drafts survive role change, downgrade, and failed sends; only send/cancel clears", () => {
  // Type a command as controller, then flap role/status with no reducer action:
  // the composer stays mounted, so the draft must survive.
  let state = draftReducer(initialDrafts("s1"), { type: "edit", field: "command", value: "partial --flag" });
  const gates = [
    { status: "connected", role: "controller" },
    { status: "reconnecting", role: null },
    { status: "connected", role: "viewer" },
    { status: "idle", role: null },
    { status: "connected", role: "controller" },
  ] as const;
  for (const gate of gates) {
    // Gating only controls writability; it never dispatches a draft action.
    void canSendToSession({ status: gate.status, role: gate.role });
    assert.equal(state.command, "partial --flag", JSON.stringify(gate));
  }
  // A failed transport write dispatches nothing, so the draft is retained.
  const flakyWriter = (ok: boolean) => (_payload: string): boolean => ok;
  const payload = commandToSend(state.command);
  assert.equal(payload, "partial --flag");
  assert.equal(flakyWriter(false)(payload!), false);
  assert.equal(state.command, "partial --flag");
  // Other fields are independent: clearing one never touches the rest.
  state = draftReducer(state, { type: "edit", field: "keystrokes", value: "tail" });
  state = draftReducer(state, { type: "edit", field: "query", value: "why?" });
  const cancelled = draftReducer(state, { type: "cancel", field: "command" });
  assert.equal(cancelled.command, "");
  assert.equal(cancelled.keystrokes, "tail");
  assert.equal(cancelled.query, "why?");
  // Explicit cancel on an empty field is a no-op (referential identity).
  assert.equal(draftReducer(cancelled, { type: "cancel", field: "command" }), cancelled);
  // Successful send clears only its own field.
  const sent = draftReducer(state, { type: "sent", field: "query" });
  assert.equal(sent.query, "");
  assert.equal(sent.command, "partial --flag");
  assert.equal(sent.keystrokes, "tail");
  // Only a session switch clears everything, so text never leaks rooms.
  const switched = draftReducer(sent, { type: "session", sessionId: "s2" });
  assert.deepEqual(switched, initialDrafts("s2"));
});

test("command history keeps send order and rejects empty/oversized input", () => {
  const empty: string[] = [];
  assert.deepEqual(pushCommandHistory(empty, ""), empty);
  assert.deepEqual(pushCommandHistory(empty, "   "), empty);
  assert.deepEqual(pushCommandHistory(empty, "\n\t "), empty);
  const overAscii = "x".repeat(64 * 1024 + 1);
  assert.equal(commandToSend(overAscii), null);
  assert.deepEqual(pushCommandHistory(empty, overAscii), empty);
  const overBytes = "é".repeat(40_000);
  assert.equal(commandToSend(overBytes), null);
  assert.deepEqual(pushCommandHistory(empty, overBytes), empty);

  let history = pushCommandHistory(empty, "ls -la");
  assert.deepEqual(history, ["ls -la"]);
  history = pushCommandHistory(history, "  make test  ");
  assert.deepEqual(history, ["ls -la", "  make test  "]);
  // History stores the exact sent payload, verbatim (no trimming/Enter).
  assert.equal(history[1], "  make test  ");
  // Consecutive duplicates collapse without growing history.
  const deduped = pushCommandHistory(history, "  make test  ");
  assert.equal(deduped, history);
  assert.deepEqual(deduped, ["ls -la", "  make test  "]);
  // Non-consecutive repeats are kept (shell-style ordering).
  history = pushCommandHistory(history, "ls -la");
  assert.deepEqual(history, ["ls -la", "  make test  ", "ls -la"]);
  // Bounded: only the newest `limit` entries survive.
  const bounded = pushCommandHistory(["a", "b"], "c", 2);
  assert.deepEqual(bounded, ["b", "c"]);
  const many: string[] = [];
  let grown: string[] = many;
  for (let index = 0; index < MAX_COMMAND_HISTORY + 5; index += 1) {
    grown = pushCommandHistory(grown, `cmd-${index}`);
  }
  assert.equal(grown.length, MAX_COMMAND_HISTORY);
  assert.equal(grown[0], "cmd-5");
  assert.equal(grown[grown.length - 1], `cmd-${MAX_COMMAND_HISTORY + 4}`);
});

test("history recall walks newest-first, clamps at bounds, and restores the draft", () => {
  const history = ["ls", "make test", "git status"];
  const draft = "partial --flag";
  // Empty history always stays idle and keeps the draft.
  assert.deepEqual(stepCommandHistory([], null, "older", draft), { index: null, value: draft });
  assert.deepEqual(stepCommandHistory([], 0, "newer", draft), { index: null, value: draft });
  assert.equal(canRecallOlder([], null), false);
  assert.equal(canRecallNewer([], null), false);

  // Older from idle recalls newest-first.
  let step = stepCommandHistory(history, null, "older", draft);
  assert.deepEqual(step, { index: 2, value: "git status" });
  assert.equal(canRecallOlder(history, step.index), true);
  assert.equal(canRecallNewer(history, step.index), true);
  step = stepCommandHistory(history, step.index, "older", draft);
  assert.deepEqual(step, { index: 1, value: "make test" });
  step = stepCommandHistory(history, step.index, "older", draft);
  assert.deepEqual(step, { index: 0, value: "ls" });
  // Clamps at the oldest entry instead of underflowing.
  const clamped = stepCommandHistory(history, 0, "older", draft);
  assert.deepEqual(clamped, { index: 0, value: "ls" });
  assert.equal(canRecallOlder(history, 0), false);
  assert.equal(canRecallNewer(history, 0), true);

  // Newer walks forward, then exits browsing and restores the stashed draft.
  let forward = stepCommandHistory(history, 0, "newer", draft);
  assert.deepEqual(forward, { index: 1, value: "make test" });
  forward = stepCommandHistory(history, forward.index, "newer", draft);
  assert.deepEqual(forward, { index: 2, value: "git status" });
  forward = stepCommandHistory(history, forward.index, "newer", draft);
  assert.deepEqual(forward, { index: null, value: draft });
  // Newer from idle is a no-op (already at the draft).
  assert.deepEqual(stepCommandHistory(history, null, "newer", draft), { index: null, value: draft });
  assert.equal(canRecallOlder(history, null), true);
  assert.equal(canRecallNewer(history, null), false);

  // Stale cursors (history reset while browsing) restart from the draft.
  assert.deepEqual(stepCommandHistory(["only"], 5, "older", draft), { index: 0, value: "only" });
  assert.deepEqual(stepCommandHistory(history, 99, "newer", draft), { index: null, value: draft });
  assert.equal(canRecallOlder(["only"], 5), true);
  assert.equal(canRecallNewer(history, 99), false);
});

test("empty and oversized payloads never send, with distinct feedback per kind", () => {
  for (const value of ["", "   ", "\n\t "]) {
    assert.equal(commandToSend(value), null);
    assert.equal(keystrokesToSend(value), value === "" ? null : value);
    assert.equal(queryToSend(value), null);
    assert.deepEqual(pushCommandHistory([], value), []);
  }
  // Keystroke whitespace is meaningful, but empty is not.
  assert.equal(keystrokesToSend(""), null);
  assert.equal(keystrokesToSend(" "), " ");
  // Oversized input fails on bytes (not characters) for every kind.
  const overAscii = "x".repeat(64 * 1024 + 1);
  assert.equal(commandToSend(overAscii), null);
  assert.equal(keystrokesToSend(overAscii), null);
  assert.equal(queryToSend(overAscii), null);
  assert.deepEqual(pushCommandHistory([], overAscii), []);
  const overBytes = "é".repeat(40_000);
  assert.ok(overBytes.length < 64 * 1024);
  assert.equal(commandToSend(overBytes), null);
  assert.equal(keystrokesToSend(overBytes), null);
  assert.deepEqual(pushCommandHistory(["ok"], overBytes), ["ok"]);
  // At the boundary payloads still pass through untouched.
  const atLimit = "x".repeat(64 * 1024);
  assert.equal(commandToSend(atLimit), atLimit);
  assert.equal(keystrokesToSend(atLimit), atLimit);
  assert.match(tooLargeMessage(), /limit/i);
});
