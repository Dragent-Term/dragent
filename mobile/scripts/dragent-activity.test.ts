import assert from "node:assert/strict";
import { test } from "node:test";
import {
  dragentActivityBlockKind, dragentActivityBlockLabel, dragentActivityBlocks,
} from "../src/terminal/dragentActivity";
import type { RelayAgentActivity } from "../src/transport/relayCatalog";

const entry = (sequence: number, kind: string, text: string): RelayAgentActivity =>
  Object.freeze({ sequence, kind, text });

test("pipeline kinds map onto the conversation row vocabulary", () => {
  assert.equal(dragentActivityBlockKind("thinking"), "thinking");
  assert.equal(dragentActivityBlockKind("tool"), "tool_step");
  assert.equal(dragentActivityBlockKind("narration"), "agent_message");
  assert.equal(dragentActivityBlockKind("finished"), "agent_message");
  assert.equal(dragentActivityBlockKind("coordination"), "unknown");
  assert.equal(dragentActivityBlockKind("  TOOL  "), "tool_step");

  assert.equal(dragentActivityBlockLabel("thinking"), "Thinking");
  assert.equal(dragentActivityBlockLabel("tool"), "Tool");
  assert.equal(dragentActivityBlockLabel("narration"), "Update");
  assert.equal(dragentActivityBlockLabel("finished"), "Finished");
  assert.equal(dragentActivityBlockLabel("coordination"), "coordination");
  assert.equal(dragentActivityBlockLabel("   "), "Activity");
});

test("feed rows keep wire order, sequences and text verbatim", () => {
  const blocks = dragentActivityBlocks([
    entry(1, "thinking", "Thinking…"),
    entry(2, "tool", "Dispatched: edit"),
    entry(3, "finished", "Attempt finished"),
  ]);
  assert.equal(blocks.length, 3);
  assert.deepEqual(blocks.map((block) => block.kind), ["thinking", "tool_step", "agent_message"]);
  assert.deepEqual(blocks.map((block) => block.label), ["Thinking", "Tool", "Finished"]);
  assert.deepEqual(blocks.map((block) => block.id), [1, 2, 3]);
  assert.equal(blocks[1]!.text, "Dispatched: edit");
  // Newest last: the live step trails the feed, like the desktop header.
  assert.equal(blocks.at(-1)!.text, "Attempt finished");
  for (const block of blocks) {
    assert.equal(block.status, null);
    assert.equal(block.truncated, false);
  }
  assert.deepEqual(dragentActivityBlocks([]), []);
});
