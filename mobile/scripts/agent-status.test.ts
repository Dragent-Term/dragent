import assert from "node:assert/strict";
import { test } from "node:test";
import { previewTabAgentState } from "../src/preview/agentStatus";
import { previewSurface } from "../src/preview/fixtures";
import { PreviewStore } from "../src/preview/PreviewStore";
import type { PreviewTabDetail } from "../src/preview/types";
import type { TerminalState } from "../src/terminal/types";

function terminal(overrides: Partial<TerminalState> = {}): TerminalState {
  return {
    blocks: [], agentBlocks: [], altScreen: null, surface: null,
    styledOutputSeen: false, rawOutputPending: false, historyTruncated: false,
    ...overrides,
  };
}

function agentTab(agentBlocks: TerminalState["agentBlocks"]): PreviewTabDetail {
  return {
    id: "tab", workspaceId: "w", projectId: "p", title: "Agent",
    kind: "agent", agent: "codex", terminal: terminal({ agentBlocks }),
  };
}

test("agent tabs read working while a query waits, done after a reply", () => {
  assert.equal(previewTabAgentState(agentTab([])), "idle");
  assert.equal(previewTabAgentState(agentTab([
    { id: 1, tsMs: 1, kind: "user_query", label: "", text: "hi", status: null, truncated: false },
  ])), "working");
  assert.equal(previewTabAgentState(agentTab([
    { id: 1, tsMs: 1, kind: "user_query", label: "", text: "hi", status: null, truncated: false },
    { id: 2, tsMs: 2, kind: "agent_message", label: "", text: "done", status: null, truncated: false },
  ])), "done");
});

test("a stop note reads interrupted", () => {
  assert.equal(previewTabAgentState(agentTab([
    { id: 1, tsMs: 1, kind: "system_note", label: "", text: "Agent stopped locally", status: null, truncated: false },
  ])), "interrupted");
});

test("opening a tab moves finished work from needs-review back to idle", () => {
  const done = agentTab([
    { id: 1, tsMs: 1, kind: "user_query", label: "", text: "hi", status: null, truncated: false },
    { id: 2, tsMs: 2, kind: "agent_message", label: "", text: "done", status: null, truncated: false },
  ]);
  assert.equal(previewTabAgentState(done, 0), "done");
  assert.equal(previewTabAgentState(done, 1), "done");
  assert.equal(previewTabAgentState(done, 2), "idle");
  const running = agentTab([
    { id: 1, tsMs: 1, kind: "user_query", label: "", text: "hi", status: null, truncated: false },
  ]);
  assert.equal(previewTabAgentState(running, 99), "working");
});

test("markTabSeen records the tab opened and clears on reset", () => {
  const store = new PreviewStore();
  const id = "preview-navigation-agent";
  assert.equal(store.markTabSeen("missing"), false);
  assert.ok(store.markTabSeen(id));
  assert.equal(store.markTabSeen(id), false);
  assert.ok(store.getSnapshot().tabSeen[id]! > 0);
  store.reset();
  assert.deepEqual(store.getSnapshot().tabSeen, {});
});

test("terminal tabs follow command_running and browsers stay idle", () => {
  const running: PreviewTabDetail = {
    id: "t", workspaceId: "w", projectId: "p", title: "Term",
    kind: "terminal",
    terminal: terminal({ surface: { ...previewSurface("/"), command_running: true } }),
  };
  assert.equal(previewTabAgentState(running), "working");
  assert.equal(previewTabAgentState({ ...running, terminal: terminal() }), "idle");
  assert.equal(previewTabAgentState(null), "idle");
});
