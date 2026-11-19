// Workspaces at work come first in a project, the rest in their own order.
import assert from "node:assert/strict";
import { test } from "node:test";
import { atWorkFirst, NO_NEEDS, type NeedCounts } from "../src/machines/attention";

const counts = (overrides: Partial<NeedCounts>): NeedCounts => ({ ...NO_NEEDS, ...overrides });

test("working and waiting-for-permission workspaces rise; the rest keep their order", () => {
  const list = [
    { name: "a", needs: NO_NEEDS },
    { name: "b", needs: counts({ review: 1 }) },
    { name: "c", needs: counts({ working: 2 }) },
    { name: "d", needs: counts({ failed: 1 }) },
    { name: "e", needs: counts({ needsInput: 1 }) },
  ];
  assert.deepEqual(atWorkFirst(list, (item) => item.needs).map((item) => item.name), ["c", "e", "a", "b", "d"]);
  // Nothing at work: unchanged.
  const idle = list.map((item) => ({ ...item, needs: NO_NEEDS }));
  assert.deepEqual(atWorkFirst(idle, (item) => item.needs).map((item) => item.name), ["a", "b", "c", "d", "e"]);
});
