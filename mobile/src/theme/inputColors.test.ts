import assert from "node:assert/strict";
import { test } from "node:test";
import { inputColorsFor, isGrey } from "./inputColors";

const THEMES = {
  dark: { accent: "#eeeeef", background: "#0e0e10" },
  light: { accent: "#18181b", background: "#ffffff" },
  gruvbox_dark: { accent: "#56c2b8", background: "#282828" },
  gruvbox_light: { accent: "#458588", background: "#fbf1c7" },
};

test("grey accents get Dragent's cyan; coloured accents keep their own", () => {
  assert.equal(isGrey("#eeeeef"), true);
  assert.equal(isGrey("#56c2b8"), false);
  assert.equal(inputColorsFor(THEMES.dark.accent, THEMES.dark.background, "ios").caret, "#3cbfeb");
  assert.equal(inputColorsFor(THEMES.light.accent, THEMES.light.background, "ios").caret, "#0a78ad");
  assert.equal(inputColorsFor(THEMES.gruvbox_dark.accent, THEMES.gruvbox_dark.background, "ios").caret, "#56c2b8");
  assert.equal(inputColorsFor(THEMES.gruvbox_light.accent, THEMES.gruvbox_light.background, "ios").caret, "#458588");
});

test("iOS gets the strong colour (it fades the highlight itself); Android a see-through one", () => {
  for (const { accent, background } of Object.values(THEMES)) {
    const ios = inputColorsFor(accent, background, "ios");
    const android = inputColorsFor(accent, background, "android");
    assert.equal(ios.selection, ios.caret);
    assert.match(android.selection, /^#[0-9a-f]{8}$/iu);
    assert.ok(android.selection.startsWith(android.caret));
  }
});
