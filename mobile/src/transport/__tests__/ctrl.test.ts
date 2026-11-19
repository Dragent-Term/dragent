// CTRL codec against the crate's vectors (docs/design/e2ee.md §4.4; pure TS, no cargo).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import { fromHex, toHex } from "../e2ee/bytes";
import { decodeCtrl, encodeCtrl, type Ctrl } from "../e2ee/ctrl";

interface Vector {
  name: string;
  content_hex: string;
  expect?: Record<string, unknown>;
  expect_error?: string;
}

const vectors = (JSON.parse(readFileSync(join(__dirname, "../../../../dragent/crates/dragent_e2ee/vectors/codec/ctrl.json"), "utf8")) as { vectors: Vector[] }).vectors;

function describe(c: Ctrl | null): Record<string, unknown> {
  if (c === null) return { ctrl: null };
  switch (c.kind) {
    case "hello": return { ctrl: "hello", resume: c.resume && { chan: toHex(c.resume.chan), last_sent_ctr: c.resume.lastSentCtr } };
    case "bye": return { ctrl: "bye", reason: c.reason };
    case "pair-confirm": return { ctrl: "pair-confirm", h_hex: toHex(c.h) };
    case "pair-done": return { ctrl: "pair-done", desktop_name: c.desktopName, catalog_version: c.catalogVersion };
    case "identity-rotate": return { ctrl: "identity-rotate", new_pub_hex: toHex(c.newPub), new_push_pub_hex: c.newPushPub && toHex(c.newPushPub) };
    case "input_lost": return { ctrl: "input_lost", handles: [...c.handles] };
    case "input_lost_ack": return { ctrl: "input_lost_ack" };
    case "control_taken": return { ctrl: "control_taken", handles: [...c.handles] };
  }
}

test("control_taken round-trips in the desktop's own bytes", () => {
  const bytes = encodeCtrl({ kind: "control_taken", handles: [3, 9] });
  assert.equal(new TextDecoder().decode(bytes), 'control_taken {"handles":[3,9]}');
  assert.deepEqual(decodeCtrl(bytes), { kind: "control_taken", handles: [3, 9] });
});

test("every accepted ctrl vector decodes to its expectation and re-encodes byte for byte", () => {
  let n = 0;
  for (const v of vectors.filter((x) => x.expect)) {
    const content = fromHex(v.content_hex)!;
    const decoded = decodeCtrl(content);
    assert.deepEqual(describe(decoded), v.expect, v.name);
    if (decoded) assert.equal(toHex(encodeCtrl(decoded)), v.content_hex, v.name);
    n++;
  }
  assert.ok(n >= 20);
});

test("every rejected ctrl vector is rejected", () => {
  let n = 0;
  for (const v of vectors.filter((x) => x.expect_error)) {
    assert.throws(() => decodeCtrl(fromHex(v.content_hex)!), (e: unknown) => e instanceof Error, v.name);
    n++;
  }
  assert.ok(n >= 40);
});
