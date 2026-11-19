import assert from "node:assert/strict";
import { test } from "node:test";
import type { TerminalDelta, WireAgentBlock, WireLine } from "../src/transport";
import { createTerminalState, reduceTerminalDelta, TERMINAL_LIMITS as limits } from "../src/terminal/state";
import { agentCollapseKey, agentDotBadgeTone, agentDotState, agentRole, defaultAgentExpanded, isAgentCollapsible, isAtTerminalBottom, resolveAgentExpanded, spanPresentation, terminalItems } from "../src/terminal/presentation";
import type { TerminalLine, TerminalState } from "../src/terminal/types";
import { ScrollFollowing } from "../src/components/keyboard";

const line = (text: string): WireLine => ({ spans: [{ text }] });
const content = (row: TerminalLine): string => row.spans.map((span) => span.text).join("");
const rowText = (state: TerminalState): string[] => state.blocks[0]!.lines.map(content);

function apply(state: TerminalState, ...deltas: TerminalDelta[]): TerminalState {
  return deltas.reduce(reduceTerminalDelta, state);
}

function malformed(state: TerminalState, delta: unknown): TerminalState {
  return reduceTerminalDelta(state, delta as TerminalDelta);
}

test("row-range replacement preserves the tail and attach replay does not duplicate or reopen blocks", () => {
  const started: TerminalDelta = { kind: "block_started", block_id: "build", command: "build", cwd: "/project", ts_ms: 12 };
  const initialRows: TerminalDelta = { kind: "block_lines", block_id: "build", base_line: 0, lines: [line("a"), line("b"), line("c")] };
  const completed: TerminalDelta = { kind: "block_completed", block_id: "build", exit_code: 0, duration_ms: 34, failed: false };
  const snapshot = apply(createTerminalState(), started, initialRows, completed);
  const patched = reduceTerminalDelta(snapshot, { kind: "block_lines", block_id: "build", base_line: 1, lines: [line("updated")] });
  assert.deepEqual(rowText(patched), ["a", "updated", "c"]);
  assert.deepEqual(rowText(snapshot), ["a", "b", "c"], "the prior snapshot is immutable");
  assert.deepEqual(apply(snapshot, started, initialRows, completed), snapshot);
  assert.equal(reduceTerminalDelta(snapshot, started).blocks[0]!.status, "completed");
  assert.equal(snapshot.blocks.length, 1);
});

test("lines and completion can precede metadata during attach without losing state", () => {
  const state = apply(createTerminalState(),
    { kind: "block_lines", block_id: "late", base_line: 7, lines: [line("saved output")] },
    { kind: "block_completed", block_id: "late", exit_code: 2, failed: true },
    { kind: "block_started", block_id: "late", command: "check", ts_ms: 50 },
  );
  const block = state.blocks[0]!;
  assert.equal(block.command, "check");
  assert.equal(block.status, "completed");
  assert.equal(block.exitCode, 2);
  assert.equal(block.failed, true);
  assert.equal(block.lines[0]!.index, 7);
});

test("large absolute offsets do not allocate gaps and out-of-order rows still replace by index", () => {
  const state = apply(createTerminalState(),
    { kind: "block_lines", block_id: "sparse", base_line: 1_000_000_000, lines: [line("far")] },
    { kind: "block_lines", block_id: "sparse", base_line: 0, lines: [line("near")] },
    { kind: "block_lines", block_id: "sparse", base_line: 1_000_000_000, lines: [line("far updated")] },
  );
  assert.deepEqual(state.blocks[0]!.lines.map((row) => row.index), [0, 1_000_000_000]);
  assert.deepEqual(rowText(state), ["near", "far updated"]);
  assert.equal(state.blocks[0]!.lines.length, 2);
});

test("per-block retention rejects replayed rows below discarded history", () => {
  const rows = Array.from({ length: limits.linesPerBlock + 20 }, (_, index) => line(String(index)));
  let state = reduceTerminalDelta(createTerminalState(), { kind: "block_lines", block_id: "long", base_line: 0, lines: rows });
  assert.equal(state.blocks[0]!.lines.length, limits.linesPerBlock);
  assert.equal(state.blocks[0]!.lines[0]!.index, 20);
  // A following row also makes the retained floor explicit.
  state = reduceTerminalDelta(state, { kind: "block_lines", block_id: "long", base_line: rows.length, lines: [line("latest")] });
  assert.equal(state.blocks[0]!.discardedBeforeLine, 21);
  state = reduceTerminalDelta(state, { kind: "block_lines", block_id: "long", base_line: 0, lines: [line("old replay")] });
  assert.equal(state.blocks[0]!.lines.length, limits.linesPerBlock);
  assert.equal(state.blocks[0]!.lines[0]!.index, 21);
  assert.equal(content(state.blocks[0]!.lines.at(-1)!), "latest");
  assert.equal(state.historyTruncated, true);
});

test("blocks, aggregate rows and aggregate text stay bounded", () => {
  let state = createTerminalState();
  for (let index = 0; index <= limits.blocks; index++) {
    state = reduceTerminalDelta(state, { kind: "block_started", block_id: String(index), command: "run" });
  }
  assert.equal(state.blocks.length, limits.blocks);
  assert.equal(state.blocks[0]!.id, "1");
  assert.equal(state.historyTruncated, true);

  state = createTerminalState();
  for (let index = 0; index < 6; index++) {
    state = reduceTerminalDelta(state, {
      kind: "block_lines", block_id: String(index), base_line: 0,
      lines: Array.from({ length: limits.linesPerBlock }, () => line("row")),
    });
  }
  assert.equal(state.blocks.reduce((sum, block) => sum + block.lines.length, 0), limits.totalLines);
  assert.equal(state.blocks.at(-1)!.lines.length, limits.linesPerBlock);

  const huge = line("x".repeat(limits.lineCharacters + 100));
  state = reduceTerminalDelta(state, {
    kind: "block_lines", block_id: "huge", base_line: 0,
    lines: Array.from({ length: limits.linesPerBlock }, () => huge),
  });
  const characters = state.blocks.flatMap((block) => block.lines).reduce((sum, row) => sum + content(row).length, 0);
  assert.ok(characters <= limits.totalCharacters);
  assert.ok(state.blocks.at(-1)!.lines.every((row) => content(row).length <= limits.lineCharacters));
  assert.equal(state.blocks.at(-1)!.lines.at(-1)!.truncated, true);
});

test("raw chunks never duplicate styled output and raw-only sessions expose the limitation", () => {
  const raw: TerminalDelta = { kind: "output_chunk", block_id: "x", seq: 1, data_base64: "aGVsbG8=" };
  const pending = reduceTerminalDelta(createTerminalState(), raw);
  assert.equal(pending.rawOutputPending, true);
  assert.deepEqual(terminalItems(pending), [], "raw bytes are not counterfeit styled rows");
  assert.strictEqual(reduceTerminalDelta(pending, raw), pending);
  const styled = reduceTerminalDelta(pending, { kind: "block_lines", block_id: "x", base_line: 0, lines: [line("hello")] });
  assert.equal(styled.rawOutputPending, false);
  assert.equal(styled.styledOutputSeen, true);
  assert.strictEqual(reduceTerminalDelta(styled, raw), styled);
  assert.deepEqual(rowText(styled), ["hello"]);
});

test("surface snapshots replace presentation state and bound malformed optional fields", () => {
  const input = {
    kind: "surface_state", state: {
      command_input: "abc", command_cursor: 100, command_running: true,
      cwd: "/work", activity: "Building", attachments: ["a", null, 7],
      ai_input: {}, ai_cursor: -1, ai_ghost: 99,
    },
  };
  let state = malformed(createTerminalState(), input);
  assert.equal(state.surface!.command_cursor, 3);
  assert.equal(state.surface!.ai_input, "");
  assert.equal(state.surface!.ai_cursor, 0);
  assert.equal(state.surface!.ai_ghost, null);
  assert.deepEqual(state.surface!.attachments, ["a"]);
  state = malformed(state, { kind: "surface_state", state: {} });
  assert.equal(state.surface!.command_input, "");
  assert.equal(state.surface!.cwd, null);
  assert.equal(state.surface!.command_running, false);
});

test("malformed optional data cannot erase valid rows or retain arbitrary payloads", () => {
  const initial = reduceTerminalDelta(createTerminalState(), { kind: "block_lines", block_id: "x", base_line: 0, lines: [line("keep")] });
  for (const bad of [
    null, {}, { kind: "unknown" },
    { kind: "block_lines", block_id: "x", base_line: -1, lines: [] },
    { kind: "block_lines", block_id: "x", base_line: Number.MAX_SAFE_INTEGER, lines: [line("overflow")] },
    { kind: "block_lines", block_id: "x", base_line: 0, lines: null },
    { kind: "agent_blocks", blocks: null },
    { kind: "alt_screen", rows: -1, cols: 2, lines: [] },
    { kind: "surface_state", state: [] },
    { kind: "output_chunk", block_id: "x", seq: -1, data_base64: "invalid" },
  ]) assert.strictEqual(malformed(initial, bad), initial);
  const state = malformed(initial, {
    kind: "block_lines", block_id: "x", base_line: 0,
    lines: [{ spans: null }, { spans: [null, { text: "safe", fg: [999, 0, 0, 255], bg: "bad", flags: "bad" }, { text: {} }] }],
  });
  assert.deepEqual(rowText(state), ["keep", "safe"]);
  assert.deepEqual(state.blocks[0]!.lines[1]!.spans[0], { text: "safe", fg: null, bg: null, flags: 0 });
});

test("stored styled rows are detached from mutable transport frame arrays", () => {
  const frame: TerminalDelta = {
    kind: "block_lines", block_id: "color", base_line: 0,
    lines: [{ spans: [{ text: "original", fg: [3, 9, 27, 255] }] }],
  };
  const state = reduceTerminalDelta(createTerminalState(), frame);
  frame.lines![0]!.spans[0]!.text = "mutated";
  frame.lines![0]!.spans[0]!.fg![0] = 255;
  assert.equal(content(state.blocks[0]!.lines[0]!), "original");
  assert.deepEqual(state.blocks[0]!.lines[0]!.spans[0]!.fg, [3, 9, 27, 255]);
});

test("agent snapshots replace by id and project requests as inert text", () => {
  const first: WireAgentBlock[] = [
    { id: 1, ts_ms: 4, kind: "user_query", text: "Question", author: { display_name: "Ada" } },
    { id: 2, ts_ms: 5, kind: "approval", command: "deploy", reason: "Approval needed", status: "pending" },
    { id: 3, ts_ms: 6, kind: "turn_group", children: [
      { id: 4, ts_ms: 6, kind: "tool_step", name: "Inspect", icon: "tool", args: "file", result: "result", status: "done" },
    ] },
  ];
  let state = reduceTerminalDelta(createTerminalState(), { kind: "agent_blocks", blocks: first });
  assert.equal(state.agentBlocks[0]!.label, "User · Ada");
  assert.equal(state.agentBlocks[1]!.label, "Approval request · read only");
  assert.equal(state.agentBlocks[1]!.text, "deploy\nApproval needed");
  assert.equal(state.agentBlocks[2]!.text, "Inspect\nfile\nresult");
  assert.equal("children" in state.agentBlocks[2]!, false);
  state = reduceTerminalDelta(state, { kind: "agent_blocks", blocks: [
    { id: 1, ts_ms: 4, kind: "agent_message", text: "first" },
    { id: 1, ts_ms: 4, kind: "agent_message", text: "latest" },
  ] });
  assert.equal(state.agentBlocks.length, 1);
  assert.equal(state.agentBlocks[0]!.text, "latest");
  assert.deepEqual(reduceTerminalDelta(state, { kind: "agent_blocks", blocks: [] }).agentBlocks, []);
});

test("agent nesting, payload text and snapshot counts stay bounded", () => {
  const group: Record<string, unknown> = { id: 1, ts_ms: 0, kind: "turn_group", children: [] };
  group.children = [group];
  assert.equal(malformed(createTerminalState(), { kind: "agent_blocks", blocks: [group] }).agentBlocks[0]!.truncated, true);
  const state = malformed(createTerminalState(), {
    kind: "agent_blocks", blocks: Array.from({ length: limits.agentBlocks + 5 }, (_, id) => ({
      id, kind: "agent_message", ts_ms: id, text: "x".repeat(limits.agentCharacters + 100),
      unrecognized: { arbitrary: "data" },
    })),
  });
  assert.equal(state.agentBlocks.length, limits.agentBlocks);
  assert.ok(state.agentBlocks.every((block) => block.text.length <= limits.agentCharacters));
  assert.equal(state.historyTruncated, true);
  assert.equal("unrecognized" in state.agentBlocks[0]!, false);
});

test("alternate-screen snapshots replace the grid and ending preserves command history", () => {
  const original = reduceTerminalDelta(createTerminalState(), { kind: "block_lines", block_id: "shell", base_line: 0, lines: [line("history")] });
  let state = reduceTerminalDelta(original, {
    kind: "alt_screen", rows: 2, cols: 80, cursor_row: 99, cursor_col: 99, cursor_visible: true,
    lines: [line("top"), line("bottom")],
  });
  assert.equal(state.altScreen!.cursorRow, 1);
  assert.equal(state.altScreen!.cursorColumn, 79);
  state = reduceTerminalDelta(state, { kind: "alt_screen", rows: 1, cols: 80, lines: [line("replacement")] });
  assert.deepEqual(state.altScreen!.lines.map(content), ["replacement"]);
  assert.equal(state.altScreen!.cursorVisible, false);
  state = reduceTerminalDelta(state, { kind: "alt_screen_ended" });
  assert.equal(state.altScreen, null);
  assert.deepEqual(rowText(state), ["history"]);
  state = reduceTerminalDelta(state, {
    kind: "alt_screen", rows: limits.altRows + 50, cols: 80,
    lines: Array.from({ length: limits.altRows + 50 }, () => line("row")),
  });
  assert.equal(state.altScreen!.lines.length, limits.altRows);
  assert.equal(state.altScreen!.truncated, true);
});

test("timeline interleaves source timestamps and keeps command/agent identities distinct", () => {
  const state = apply(createTerminalState(),
    { kind: "block_started", block_id: "1", command: "build", ts_ms: 20 },
    { kind: "agent_blocks", blocks: [{ id: 1, ts_ms: 10, kind: "agent_message", text: "before" }] },
    { kind: "alt_screen", rows: 1, cols: 1, lines: [line("a")] },
  );
  assert.deepEqual(terminalItems(state).map((item) => item.key), ["agent:1", "command:1", "alt-screen"]);
});

test("source RGBA and SGR attributes are honored with current-theme defaults", () => {
  const defaults = { foreground: "selected-theme-foreground", background: "selected-theme-background" };
  const styled = spanPresentation({
    text: "styled", fg: [4, 8, 12, 128], bg: null,
    flags: (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 8),
  }, defaults);
  assert.equal(styled.style.color, `rgba(4, 8, 12, ${128 / 255})`);
  assert.equal(styled.style.backgroundColor, defaults.background);
  assert.equal(styled.style.fontWeight, "700");
  assert.equal(styled.style.fontStyle, "italic");
  assert.equal(styled.style.textDecorationLine, "underline line-through");
  assert.equal(styled.style.opacity, 0.65);
  const inverted = spanPresentation({ text: "inverse", fg: null, bg: null, flags: 1 << 6 }, defaults);
  assert.equal(inverted.style.color, defaults.background);
  assert.equal(inverted.style.backgroundColor, defaults.foreground);
  assert.equal(spanPresentation({ text: "secret", fg: null, bg: null, flags: 1 << 7 }, defaults).text, "      ");
});

test("bottom detection follows short output and pauses when reading earlier rows", () => {
  assert.equal(isAtTerminalBottom(0, 600, 300), true);
  assert.equal(isAtTerminalBottom(400, 600, 1000), true);
  assert.equal(isAtTerminalBottom(399.5, 600, 1000), true);
  assert.equal(isAtTerminalBottom(390, 600, 1000), false);
  assert.equal(isAtTerminalBottom(300, 600, 1000), false);
  assert.equal(isAtTerminalBottom(0, 300, 1000), false);
});

test("wheel/keyboard scrollback pauses before output grows and resumes only at the bottom", () => {
  const follow = new ScrollFollowing();
  assert.equal(follow.canFollow, true);
  // User wheels upward or presses PageUp before a browser scroll event fires.
  follow.towardHistory(400);
  assert.equal(follow.canFollow, false);
  // Appending output/layout shifts never force this reader to the tail.
  follow.scrolled(false);
  follow.scrolled(false);
  assert.equal(follow.canFollow, false);
  // Wheel/keyboard scrolling back to the physical bottom enables following.
  follow.scrolled(true);
  assert.equal(follow.canFollow, true);
  // An automatic event while output grows must not disable active follow.
  follow.scrolled(false);
  assert.equal(follow.canFollow, true);
});

test("native drags and pointer/scrollbar interaction preserve anchoring until release", () => {
  const follow = new ScrollFollowing();
  follow.beginInteraction();
  assert.equal(follow.canFollow, false);
  assert.equal(follow.isInteracting, true);
  follow.scrolled(false);
  follow.endInteraction(false);
  assert.equal(follow.canFollow, false);
  assert.equal(follow.isInteracting, false);
  follow.beginInteraction();
  follow.scrolled(true);
  assert.equal(follow.canFollow, false, "Never follow during a drag, even at the bottom");
  follow.endInteraction(true);
  assert.equal(follow.canFollow, true);
});

test("switching output modes starts fresh follow state instead of inheriting paused scrollback", () => {
  const terminal = new ScrollFollowing();
  terminal.towardHistory(200);
  terminal.beginInteraction();
  const agent = new ScrollFollowing();
  assert.equal(agent.canFollow, true);
  assert.equal(agent.isInteracting, false);
  assert.equal(terminal.canFollow, false);
  // Scrolling a short, non-scrollable new view cannot accidentally pause it.
  agent.towardHistory(0);
  assert.equal(agent.canFollow, true);
});

test("content filtering separates terminal output and agent conversation without duplicating scroll logic", () => {
  const state = apply(createTerminalState(),
    { kind: "block_started", block_id: "1", command: "build", ts_ms: 20 },
    { kind: "agent_blocks", blocks: [{ id: 1, ts_ms: 10, kind: "agent_message", text: "before" }] },
    { kind: "alt_screen", rows: 1, cols: 1, lines: [line("a")] },
  );
  assert.deepEqual(terminalItems(state, "all").map((item) => item.key), ["agent:1", "command:1", "alt-screen"]);
  assert.deepEqual(terminalItems(state).map((item) => item.key), ["agent:1", "command:1", "alt-screen"], "default stays all");
  assert.deepEqual(terminalItems(state, "terminal").map((item) => item.key), ["command:1", "alt-screen"]);
  assert.deepEqual(terminalItems(state, "agent").map((item) => item.key), ["agent:1"]);
  const terminalOnly = terminalItems(state, "terminal");
  assert.ok(terminalOnly.every((item) => item.kind !== "agent"));
  const agentOnly = terminalItems(state, "agent");
  assert.ok(agentOnly.every((item) => item.kind === "agent"));
});

test("agent role mapping keeps message kinds distinguishable", () => {
  assert.equal(agentRole("user_query"), "user");
  assert.equal(agentRole("agent_message"), "agent");
  assert.equal(agentRole("error"), "agent");
  assert.equal(agentRole("system_note"), "system");
  assert.equal(agentRole("thinking"), "thought");
  assert.equal(agentRole("tool_step"), "tool");
  assert.equal(agentRole("action_result"), "result");
  assert.equal(agentRole("approval"), "request");
  assert.equal(agentRole("question"), "request");
  assert.equal(agentRole("plan_review"), "request");
  assert.equal(agentRole("queued"), "status");
  assert.equal(agentRole("loading"), "status");
  assert.equal(agentRole("turn_group"), "group");
  assert.equal(agentRole("activity_group"), "group");
  assert.equal(agentRole("unknown"), "unknown");
});

test("only verbose thinking/tool/group rows collapse", () => {
  assert.equal(isAgentCollapsible("thinking"), true);
  assert.equal(isAgentCollapsible("tool_step"), true);
  assert.equal(isAgentCollapsible("turn_group"), true);
  assert.equal(isAgentCollapsible("activity_group"), true);
  assert.equal(isAgentCollapsible("agent_message"), false);
  assert.equal(isAgentCollapsible("user_query"), false);
  assert.equal(isAgentCollapsible("system_note"), false);
  assert.equal(isAgentCollapsible("approval"), false);
  assert.equal(isAgentCollapsible("question"), false);
  assert.equal(isAgentCollapsible("plan_review"), false);
  assert.equal(isAgentCollapsible("action_result"), false);
  assert.equal(isAgentCollapsible("loading"), false);
  assert.equal(isAgentCollapsible("unknown"), false);
});

test("verbose thinking/tool/group rows start collapsed with stable keys", () => {
  assert.equal(defaultAgentExpanded("thinking"), false);
  assert.equal(defaultAgentExpanded("tool_step"), false);
  assert.equal(defaultAgentExpanded("turn_group"), false);
  assert.equal(defaultAgentExpanded("activity_group"), false);
  assert.equal(defaultAgentExpanded("agent_message"), true);
  assert.equal(defaultAgentExpanded("user_query"), true);
  assert.equal(defaultAgentExpanded("system_note"), true);
  assert.equal(defaultAgentExpanded("approval"), true);
  assert.equal(defaultAgentExpanded("loading"), true);
  assert.equal(defaultAgentExpanded("unknown"), true);

  assert.equal(agentCollapseKey(7), "agent:7");
  assert.equal(resolveAgentExpanded("thinking", 7, {}), false, "collapsed by default");
  assert.equal(resolveAgentExpanded("thinking", 7, { "agent:7": true }), true, "explicit expand survives re-renders");
  assert.equal(resolveAgentExpanded("thinking", 7, { "agent:7": false }), false);
  assert.equal(resolveAgentExpanded("agent_message", 1, {}), true);
  assert.equal(resolveAgentExpanded("agent_message", 1, { "agent:1": false }), false, "explicit collapse survives re-renders");
  assert.equal(resolveAgentExpanded("turn_group", 3, {}), false, "groups start collapsed too");

  const state = apply(createTerminalState(), {
    kind: "agent_blocks", blocks: [{ id: 7, ts_ms: 1, kind: "thinking", content: "hmm" }],
  });
  assert.deepEqual(terminalItems(state).map((item) => item.key), ["agent:7"]);
  assert.equal(agentCollapseKey(7), terminalItems(state)[0]!.key, "collapse key matches the rendered row key");
});

test("agent dot states surface stop/retry/interrupted in shared vocabulary", () => {
  const dot = (kind: Parameters<typeof agentDotState>[0]["kind"], status: string | null, text = "") =>
    agentDotState({ kind, status, text });
  assert.equal(dot("error", null, "boom"), "failed");
  assert.equal(dot("tool_step", "error", "out"), "failed");
  assert.equal(dot("agent_message", "Failed", "x"), "failed");
  assert.equal(dot("tool_step", "running", "out"), "working");
  assert.equal(dot("agent_message", "Streaming", "partial"), "working");
  assert.equal(dot("loading", null, "busy"), "working");
  assert.equal(dot("tool_step", "done", "out"), "done");
  assert.equal(dot("agent_message", "Done", "x"), "done");
  assert.equal(dot("approval", "approved", "cmd"), "done");
  assert.equal(dot("approval", "pending", "cmd"), "blocked");
  assert.equal(dot("question", "waiting for input", "q"), "blocked");
  assert.equal(dot("plan_review", null, "review"), "blocked");
  assert.equal(dot("queued", null, "q"), "waiting");
  assert.equal(dot("agent_message", "retrying", "x"), "waiting");
  assert.equal(dot("agent_message", "waiting", "x"), "waiting");
  assert.equal(dot("agent_message", "stopped", "x"), "interrupted");
  assert.equal(dot("agent_message", "cancelled", "x"), "interrupted");
  assert.equal(dot("system_note", null, "Run stopped by the user"), "interrupted");
  assert.equal(dot("user_query", null, "hello"), "working");
  assert.equal(dot("agent_message", null, "hello"), "idle");
  assert.equal(dot("system_note", null, "note"), "idle");
  assert.equal(dot("thinking", null, "hmm"), "idle");
  assert.equal(dot("unknown", null, "x"), "idle");
});

test("dot badge tones stay theme-driven with a muted stop state", () => {
  assert.equal(agentDotBadgeTone("working"), "accent");
  assert.equal(agentDotBadgeTone("done"), "success");
  assert.equal(agentDotBadgeTone("blocked"), "warning");
  assert.equal(agentDotBadgeTone("waiting"), "warning");
  assert.equal(agentDotBadgeTone("failed"), "danger");
  assert.equal(agentDotBadgeTone("interrupted"), "neutral");
  assert.equal(agentDotBadgeTone("idle"), "neutral");
});
