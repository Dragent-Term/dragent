import assert from "node:assert/strict";
import { test } from "node:test";
import { resolveLayout } from "../src/layout/resolveLayout";

test("available width crosses breakpoints at 600 and 1024 logical pixels", () => {
  for (const [width, size, columns] of [
    [320, "compact", 1],
    [599, "compact", 1],
    [600, "medium", 1],
    [1023, "medium", 1],
    [1024, "expanded", 2],
  ] as const) {
    const layout = resolveLayout(width, 1);
    assert.equal(layout.size, size, String(width));
    assert.equal(layout.columns, columns, String(width));
  }
});

test("larger fonts collapse columns and preserve readable space", () => {
  assert.equal(resolveLayout(1200, 1).columns, 2);
  assert.equal(resolveLayout(1200, 1.25).size, "medium");
  assert.equal(resolveLayout(1200, 2).size, "medium");
  assert.equal(resolveLayout(1200, 2.01).size, "compact");
  assert.equal(resolveLayout(800, 1).size, "medium");
  assert.equal(resolveLayout(800, 1.5).size, "compact");
  assert.equal(resolveLayout(1024 * 1.5, 1.5).columns, 2);
  assert.equal(resolveLayout(600 * 1.5, 1.5).size, "medium");
  assert.equal(resolveLayout(600 * 1.5 - 1, 1.5).size, "compact");
});

test("resizing from a tablet window to split-screen recomputes layout", () => {
  const wide = resolveLayout(1440, 1);
  const split = resolveLayout(480, 1);
  assert.equal(wide.columns, 2);
  assert.equal(split.columns, 1);
  assert.equal(split.size, "compact");
  assert.ok(split.maxContentWidth + 2 * split.gutter <= 480);
  assert.deepEqual(resolveLayout(1440, 1), wide);
});

test("main-pane breakpoints use the width left after a sidebar", () => {
  const windowWidth = 1280;
  const sidebarWidth = 280;
  const mainWidth = windowWidth - sidebarWidth;
  assert.equal(resolveLayout(windowWidth, 1).columns, 2);
  const main = resolveLayout(mainWidth, 1);
  assert.equal(main.size, "medium");
  assert.equal(main.columns, 1);
  assert.ok(main.maxContentWidth + 2 * main.gutter <= mainWidth);
});

test("available pane width and large text jointly determine readable layout", () => {
  const mainWidth = 840 - 240;
  assert.equal(resolveLayout(mainWidth, 1).size, "medium");
  const largeText = resolveLayout(mainWidth, 1.25);
  assert.equal(largeText.size, "compact");
  assert.equal(largeText.columns, 1);
  assert.ok(largeText.maxContentWidth + 2 * largeText.gutter <= mainWidth);
  // A zero-width measured pane stays empty until layout reports a real width.
  assert.equal(resolveLayout(0, 1.25).maxContentWidth, 0);
});

test("content width and safe gutters fit even extreme window and font sizes", () => {
  for (const width of [0, 1, 24, 280, 320, 390, 600, 768, 1024, 1440, 2560, 8000]) {
    for (const fontScale of [0.5, 1, 1.3, 2, 3, 8]) {
      const layout = resolveLayout(width, fontScale);
      assert.ok(layout.gutter >= 0);
      assert.ok(layout.maxContentWidth >= 0);
      assert.ok(layout.maxContentWidth + 2 * layout.gutter <= width);
      if (layout.columns === 2) {
        assert.ok(width / Math.max(1, fontScale) >= 1024);
      }
    }
  }
  assert.equal(resolveLayout(2560, 1).maxContentWidth, 1200);
  assert.equal(resolveLayout(2560, 2).maxContentWidth, 2400);
});

test("smaller fonts do not create extra columns; invalid measurements stay bounded", () => {
  assert.deepEqual(resolveLayout(768, 0.7), resolveLayout(768, 1));
  assert.deepEqual(resolveLayout(390, Number.NaN), resolveLayout(390, 1));
  assert.deepEqual(resolveLayout(390, Number.POSITIVE_INFINITY), resolveLayout(390, 1));
  for (const width of [-10, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.deepEqual(resolveLayout(width, 1), { size: "compact", columns: 1, maxContentWidth: 0, gutter: 0 });
  }
});
