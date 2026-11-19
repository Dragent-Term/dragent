import assert from "node:assert/strict";
import { test } from "node:test";
import { displayMachineName, isUnpairedMachineEntry } from "../src/machines/machineCatalog";

test("nameless discovery entries are unpaired desktops", () => {
  assert.equal(isUnpairedMachineEntry({ name: "" }), true);
  assert.equal(isUnpairedMachineEntry({ name: "   " }), true);
  assert.equal(isUnpairedMachineEntry({ name: "Studio" }), false);
});

test("paired names come from the pin store; the relay name is only fallback", () => {
  assert.equal(displayMachineName({ name: "" }, "Studio"), "Studio");
  assert.equal(displayMachineName({ name: "old" }, "Studio"), "Studio");
  assert.equal(displayMachineName({ name: "old" }, null), "old");
  assert.equal(displayMachineName({ name: "old" }, "  "), "old");
  assert.equal(displayMachineName({ name: "" }, null), "");
});
