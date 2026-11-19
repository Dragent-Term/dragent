import assert from "node:assert/strict";
import { test } from "node:test";
import { TerminalRuntimeStore } from "../src/terminal/runtime/TerminalRuntimeStore";

test("runtime store carries screen modes in every frame", () => {
  const store = new TerminalRuntimeStore();
  const initial = store.frameSince(0);
  assert.deepEqual(initial.screenModes, {
    alternateScreen: false,
    mouseReporting: false,
    sgrMouse: false,
    bracketedPaste: false,
  });
  store.setScreenModes({ alternateScreen: true, mouseReporting: true, sgrMouse: false, bracketedPaste: true });
  const framed = store.frameSince(0);
  assert.deepEqual(framed.screenModes, {
    alternateScreen: true,
    mouseReporting: true,
    sgrMouse: false,
    bracketedPaste: true,
  });
  // Reset clears modes for the next handle epoch.
  store.reset();
  assert.deepEqual(store.frameSince(0).screenModes, {
    alternateScreen: false,
    mouseReporting: false,
    sgrMouse: false,
    bracketedPaste: false,
  });
  store.dispose();
});
