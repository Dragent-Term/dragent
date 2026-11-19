import { test } from "node:test";
import assert from "node:assert/strict";
import {
  clockLabel, layoutTranscript, runSummary, stepFromToolRow, type Entry, type Line, type Step, type ToolRow,
} from "../../features/chat/transcript";

const tool = (id: string, tool: string, text: string, extra: Partial<ToolRow> = {}): ToolRow => ({
  id, tool, text, error: false, running: false, path: null, added: null, removed: null, at: null, ...extra,
});
const step = (row: ToolRow, live = true): Entry<never> => ({ kind: "step", step: stepFromToolRow(row, live), at: row.at });
const you = (id: string, at: number | null = null): Entry<never> => ({ kind: "you", id, text: id, at });
const reply = (id: string, at: number | null = null): Entry<never> => ({ kind: "reply", id, text: id, at, streaming: false });
const shape = (lines: Line<never>[]) => lines.map((line) => {
  switch (line.kind) {
    case "run": return `run(${line.steps.length}${line.active ? "*" : ""})`;
    case "step": return `step:${line.step.id}`;
    case "reply": return `reply:${line.id}${line.lastReply ? "!" : ""}`;
    case "you": return `you:${line.id}`;
    case "changes": return `changes(${line.files.length})`;
    default: return line.kind;
  }
});

test("a turn: your message, its clock, steps gathered under one line, replies in full", () => {
  const lines = layoutTranscript([
    you("q"),
    reply("a1"),
    step(tool("1", "Read", "/x/a.ts", { path: "/x/a.ts" })),
    step(tool("2", "Bash", "npm test")),
    reply("a2"),
    step(tool("3", "Bash", "npm run lint")),
    reply("a3"),
  ], { working: false, waiting: false });
  assert.deepEqual(shape(lines), ["you:q", "clock", "reply:a1", "run(2)", "reply:a2", "step:3", "reply:a3!"]);
});

test("while working, the trailing run is active and no reply is final yet", () => {
  const lines = layoutTranscript([you("q"), step(tool("1", "Bash", "ls -la src")), step(tool("2", "Bash", "make", { running: true }))], { working: true, waiting: false });
  // The live clock sits at the bottom, under the newest line.
  assert.deepEqual(shape(lines), ["you:q", "run(2*)", "clock"]);
  const clock = lines[2] as Extract<Line<never>, { kind: "clock" }>;
  assert.equal(clock.running, true);
});

test("a settled turn that changed files gets a changes card after its last reply", () => {
  const lines = layoutTranscript([
    you("q"),
    step(tool("1", "Edit", "/x/a.ts", { path: "/x/a.ts", added: 3, removed: 1 })),
    step(tool("2", "Edit", "/x/a.ts", { path: "/x/a.ts", added: 2, removed: 0 })),
    step(tool("3", "Write", "/x/b.ts", { path: "/x/b.ts", added: 10, removed: 0 })),
    reply("done"),
  ], { working: false, waiting: false });
  assert.deepEqual(shape(lines), ["you:q", "clock", "run(3)", "reply:done!", "changes(2)"]);
  const card = lines[4] as Extract<Line<never>, { kind: "changes" }>;
  assert.deepEqual(card.files.map((file) => [file.name, file.added, file.removed]), [["a.ts", 5, 1], ["b.ts", 10, 0]]);
});

test("the clock says how long a turn worked", () => {
  const lines = layoutTranscript([you("q", 1_000), reply("a", 64_000)], { working: false, waiting: false });
  assert.equal(clockLabel(lines[1] as never, 0), "Done in 1m 3s");
  const live = layoutTranscript([you("q", 1_000)], { working: true, waiting: false });
  assert.equal(clockLabel(live[1] as never, 13_500), "Working · 12s");
  const waiting = layoutTranscript([you("q", 1_000), reply("a")], { working: true, waiting: true });
  assert.equal(clockLabel(waiting[2] as never, 5_000), "Waiting on you");
});

test("steps read as what they did", () => {
  const view = (s: Step) => [s.kind, s.verb, s.file, s.label, s.mono];
  assert.deepEqual(view(stepFromToolRow(tool("1", "Edit", "/x/src/auth.ts", { path: "/x/src/auth.ts" }), false)), ["edit", "Edited", "auth.ts", "", false]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "Edit", "/x/a.ts", { path: "/x/a.ts", running: true }), true)), ["edit", "Editing", "a.ts", "", false]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "Bash", "cargo test -p app"), false)), ["execute", null, null, "cargo test -p app", true]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "Bash", "rg -n useTheme src"), false)), ["search", "Searched", null, "useTheme", false]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "exec_command", "cat src/main.rs"), false)), ["read", "Read", "main.rs", "", false]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "Grep", "TODO"), false)), ["search", "Searched", null, "TODO", false]);
  // A step left "running" by an agent that stopped isn't shown as running.
  assert.equal(stepFromToolRow(tool("1", "Bash", "x", { running: true }), false).running, false);
});

test("a run's summary counts what it did", () => {
  const steps = [
    stepFromToolRow(tool("1", "Bash", "npm test"), false),
    stepFromToolRow(tool("2", "Bash", "npm run build"), false),
    stepFromToolRow(tool("3", "Edit", "/a", { path: "/a" }), false),
    stepFromToolRow(tool("4", "Read", "/b", { path: "/b" }), false),
  ];
  assert.equal(runSummary(steps), "2 commands · 1 edit · 1 read");
});

test("thoughts between calls don't count as steps", () => {
  const think: Step = { ...stepFromToolRow(tool("t", "x", "y"), false), kind: "think" };
  assert.equal(runSummary([think, stepFromToolRow(tool("1", "Bash", "ls -la x | wc"), false)]), "1 command");
  assert.equal(runSummary([think]), "Reasoning");
});

test("other agents' tools read the same way", () => {
  const view = (s: Step) => [s.kind, s.verb, s.file ?? s.label];
  assert.deepEqual(view(stepFromToolRow(tool("1", "run_shell_command", "npm test"), false)), ["execute", null, "npm test"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "write_file", "/x/new.ts", { path: "/x/new.ts" }), false)), ["edit", "Wrote", "new.ts"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "replace", "/x/a.ts", { path: "/x/a.ts" }), false)), ["edit", "Edited", "a.ts"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "str_replace_editor", "src/a.rs", { path: "src/a.rs" }), false)), ["edit", "Edited", "a.rs"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "search_file_content", "TODO"), false)), ["search", "Searched", "TODO"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "webfetch", "https://x.dev"), false)), ["fetch", "Fetched", "https://x.dev"]);
});

test("Cursor's tools read the same way", () => {
  const view = (s: Step) => [s.kind, s.verb, s.file ?? s.label];
  assert.deepEqual(view(stepFromToolRow(tool("1", "StrReplace", "src/h.tsx", { path: "src/h.tsx" }), false)), ["edit", "Edited", "h.tsx"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "ReadFile", "src/h.tsx", { path: "src/h.tsx" }), false)), ["read", "Read", "h.tsx"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "Delete", "old.ts", { path: "src/old.ts" }), false)), ["delete", "Deleted", "old.ts"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "SemanticSearch", "where is auth"), false)), ["search", "Searched", "where is auth"]);
  assert.deepEqual(view(stepFromToolRow(tool("1", "Shell", "npm test"), false)), ["execute", null, "npm test"]);
});

test("while working the clock follows the stream; once done it moves under the prompt", () => {
  const entries = [you("q", 1_000), reply("a1", 2_000), step(tool("1", "Bash", "npm test")), reply("a2", 9_000)];
  assert.deepEqual(shape(layoutTranscript(entries, { working: true, waiting: false })), ["you:q", "reply:a1", "step:1", "reply:a2", "clock"]);
  assert.deepEqual(shape(layoutTranscript(entries, { working: false, waiting: false })), ["you:q", "clock", "reply:a1", "step:1", "reply:a2!"]);
});
