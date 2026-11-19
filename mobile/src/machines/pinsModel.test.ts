import assert from "node:assert/strict";
import { test } from "node:test";

import { MAX_PINS, parsePins, withPin } from "./pinsModel";

test("pins are read defensively", () => {
  assert.deepEqual(parsePins(null), {});
  assert.deepEqual(parsePins("not json"), {});
  assert.deepEqual(parsePins("[1,2]"), {});
  assert.deepEqual(parsePins(JSON.stringify({ d1: ["a", "a", "", 3, "b"], d2: "x", d3: [] })), { d1: ["a", "b"] });
});

test("pinning keeps the newest last; unpinning the last drops the desktop", () => {
  let pins = withPin({}, "d1", "a", true);
  pins = withPin(pins, "d1", "b", true);
  pins = withPin(pins, "d1", "a", true);
  assert.deepEqual(pins, { d1: ["b", "a"] });
  pins = withPin(pins, "d1", "b", false);
  pins = withPin(pins, "d1", "a", false);
  assert.deepEqual(pins, {});
});

test("a desktop keeps at most MAX_PINS pins", () => {
  let pins = {};
  for (let i = 0; i <= MAX_PINS; i++) pins = withPin(pins, "d1", `w${i}`, true);
  const kept = (pins as Record<string, string[]>).d1!;
  assert.equal(kept.length, MAX_PINS);
  assert.equal(kept[0], "w1", "the oldest pin went first");
});
