import assert from "node:assert/strict";
import { test } from "node:test";
import {
  agentDurationLabel,
  applyAgentUpdate,
  EMPTY_AGENT_CONVERSATION,
  parseAgentRow,
  pendingAgentRequest,
} from "../src/transport/agentConversation";
import { actOnAgent, sendAgentPrompt } from "../src/transport/relayRpcMethods";
import type { RelayRpcClient } from "../src/transport/relayRpcClient";
import { markdownBlocks, markdownInline } from "../src/features/agent/markdownText";

const REF = "3f2c1b9a-7d64-4c1e-9a55-0b1c2d3e4f50";

function apply(previous: Parameters<typeof applyAgentUpdate>[0], payload: unknown, snapshot = false) {
  const result = applyAgentUpdate(previous, payload, snapshot);
  if (!result.ok) throw new Error(`rejected: ${result.reason}`);
  return result.state;
}

test("the first message holds every row; later ones carry only what changed", () => {
  const first = apply(EMPTY_AGENT_CONVERSATION, {
    rev: 1, busy: true, clipped: false, ready: true, order: [1, 2],
    blocks: [
      { id: 1, ts_ms: 10, kind: "user_query", text: "fix the tests" },
      { id: 2, ts_ms: 11, kind: "agent_message", text: "Look", streaming: true },
    ],
  }, true);
  assert.equal(first.busy, true);
  assert.deepEqual(first.rows.map((row) => row.kind), ["user_query", "agent_message"]);
  const second = apply(first, {
    rev: 2, busy: false, clipped: false, ready: true, order: [1, 2, 3],
    blocks: [
      { id: 2, ts_ms: 11, kind: "agent_message", text: "Looking at it", streaming: false },
      { id: 3, ts_ms: 12, kind: "tool_step", name: "Read", icon: "", args: "a.ts", status: "done", duration_ms: 40 },
    ],
  });
  assert.equal(second.busy, false);
  assert.equal(second.rows.length, 3);
  // An unchanged row is the very same object, so its view doesn't redraw.
  assert.equal(second.rows[0], first.rows[0]);
  const message = second.rows[1]!;
  assert.ok(message.kind === "agent_message" && message.text === "Looking at it" && !message.streaming);
  // Rows the desktop dropped from the order go away here too.
  const third = apply(second, { rev: 3, busy: false, ready: true, order: [3], blocks: [] });
  assert.deepEqual(third.rows.map((row) => row.id), [3]);
});

test("a message naming a row this phone never got asks for everything again", () => {
  const held = apply(EMPTY_AGENT_CONVERSATION, {
    rev: 1, order: [1], blocks: [{ id: 1, kind: "system_note", text: "a" }],
  }, true);
  assert.deepEqual(applyAgentUpdate(held, { rev: 5, order: [1, 9], blocks: [] }, false), { ok: false, reason: "resync" });
  // A new subscription's first message replaces what was held.
  assert.deepEqual(applyAgentUpdate(held, { rev: 1, order: [1], blocks: [] }, true), { ok: false, reason: "resync" });
  for (const bad of [null, {}, { order: [1, 1], blocks: [] }, { order: "x", blocks: [] }, { order: [], blocks: [{ kind: "error" }] }]) {
    assert.deepEqual(applyAgentUpdate(held, bad, false), { ok: false, reason: "unreadable" });
  }
});

test("the stream opens not-ready until the desktop pane has projected", () => {
  const waiting = apply(EMPTY_AGENT_CONVERSATION, { rev: 0, busy: false, ready: false, order: [], blocks: [] }, true);
  assert.equal(waiting.ready, false);
  assert.equal(waiting.rows.length, 0);
});

test("cards carry what the phone answers with", () => {
  const approval = parseAgentRow({ id: 4, kind: "approval", command: "rm -rf build", reason: "clean", status: "pending", ref: REF });
  assert.ok(approval?.kind === "approval" && approval.ref === REF && approval.status === "pending");
  const question = parseAgentRow({
    id: 5, kind: "question", header: "Auth", question: "Which?", options: ["OAuth", "Keys"],
    descriptions: ["Browser sign-in", ""], picked: ["Keys"], multi: true, custom: false, index: 1, count: 3, status: "pending", ref: REF,
  });
  assert.ok(question?.kind === "question");
  assert.deepEqual(question.options, [{ label: "OAuth", description: "Browser sign-in" }, { label: "Keys", description: "" }]);
  assert.deepEqual([question.multi, question.custom, question.index, question.count, question.picked], [true, false, 1, 3, ["Keys"]]);
  // A reference that isn't an id is never sent back.
  const forged = parseAgentRow({ id: 6, kind: "plan_review", content: "1. do", status: "pending", ref: "../../x" });
  assert.ok(forged?.kind === "plan_review" && forged.ref === null);
  const state = apply(EMPTY_AGENT_CONVERSATION, {
    order: [4, 7], blocks: [
      { id: 4, kind: "approval", command: "x", reason: "", status: "approved", ref: REF },
      { id: 7, kind: "question", question: "?", options: ["a"], status: "pending", ref: REF },
    ],
  }, true);
  assert.equal(pendingAgentRequest(state)?.id, 7);
});

test("tool steps keep their diff, and unknown rows stay visible", () => {
  const step = parseAgentRow({
    id: 8, kind: "tool_step", name: "Edit", args: "src/a.ts", status: "weird", result: "ok",
    diff: { files: [{ path: "src/a.ts", added: 1, removed: 1, lines: [
      { kind: "remove", text: "old" }, { kind: "add", text: "new" }, { kind: "context", text: "same" },
    ] }] },
  });
  assert.ok(step?.kind === "tool_step");
  assert.equal(step.status, "done", "an unknown status never shows a spinner forever");
  assert.deepEqual(step.diff[0]!.lines.map((line) => line.kind), ["del", "add", "ctx"]);
  const later = parseAgentRow({ id: 9, kind: "holo_deck", text: "new thing" });
  assert.ok(later?.kind === "unknown" && later.label === "holo_deck" && later.text === "new thing");
  assert.equal(parseAgentRow({ kind: "agent_message", text: "no id" }), null);
  const turn = parseAgentRow({ id: 10, kind: "turn_end", duration_ms: 80_000, status_note: null });
  assert.ok(turn?.kind === "turn_end" && agentDurationLabel(turn.durationMs) === "1m 20s");
  assert.equal(agentDurationLabel(400), "1s");
  assert.equal(agentDurationLabel(null), null);
});

function fakeClient(calls: Array<{ method: string; params: unknown }>, reply: unknown): RelayRpcClient {
  return {
    request: async (method: string, params: unknown) => {
      calls.push({ method, params });
      return reply;
    },
  } as unknown as RelayRpcClient;
}

test("prompts and answers are checked before anything is sent", async () => {
  const calls: Array<{ method: string; params: unknown }> = [];
  await sendAgentPrompt(fakeClient(calls, { sent: true }), 31, "fix it");
  assert.deepEqual(calls, [{ method: "agent.send", params: { tab_id: 31, text: "fix it" } }]);
  calls.length = 0;
  const client = fakeClient(calls, { done: true });
  await actOnAgent(client, 31, "stop");
  await actOnAgent(client, 31, "answer", { ref: REF, option: "Keys" });
  assert.deepEqual(calls.map((call) => call.params), [
    { tab_id: 31, action: "stop" },
    { tab_id: 31, action: "answer", ref: REF, option: "Keys" },
  ]);
  calls.length = 0;
  await assert.rejects(sendAgentPrompt(client, 31, "   "));
  await assert.rejects(sendAgentPrompt(client, 31, "x".repeat(40_000)));
  await assert.rejects(actOnAgent(client, 31, "approve"));
  await assert.rejects(actOnAgent(client, 31, "answer", { ref: REF }));
  await assert.rejects(actOnAgent(client, 31, "answer_text", { ref: REF, text: " " }));
  assert.equal(calls.length, 0);
  // A reply that isn't the expected acknowledgement is an error, not success.
  await assert.rejects(sendAgentPrompt(fakeClient([], { ok: true }), 31, "x"));
});

test("the agent's Markdown reads as blocks and inline runs", () => {
  const blocks = markdownBlocks("# Plan\n\nDo **this** then `that`.\n\n- one\n  - nested\n1) first\n\n```ts\nconst a = 1;\n```\n> note\n---");
  assert.deepEqual(blocks.map((block) => block.kind), ["heading", "paragraph", "item", "item", "item", "code", "quote", "rule"]);
  const nested = blocks[3]!;
  assert.ok(nested.kind === "item" && nested.depth === 1 && nested.marker === "•");
  const numbered = blocks[4]!;
  assert.ok(numbered.kind === "item" && numbered.marker === "1.");
  const code = blocks[5]!;
  assert.ok(code.kind === "code" && code.language === "ts" && code.text === "const a = 1;");
  assert.deepEqual(markdownInline("a **b** `c` [d](https://e.dev) *f*"), [
    { kind: "text", text: "a " }, { kind: "bold", text: "b" }, { kind: "text", text: " " },
    { kind: "code", text: "c" }, { kind: "text", text: " " },
    { kind: "link", text: "d", href: "https://e.dev" }, { kind: "text", text: " " }, { kind: "italic", text: "f" },
  ]);
  // A fence still open while the answer streams is already code.
  const open = markdownBlocks("```\nhalf");
  assert.deepEqual(open, [{ kind: "code", language: "", text: "half" }]);
  // Only web links become links.
  assert.deepEqual(markdownInline("[x](javascript:alert(1))"), [{ kind: "text", text: "[x](javascript:alert(1))" }]);
});
