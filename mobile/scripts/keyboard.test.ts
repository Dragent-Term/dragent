import assert from "node:assert/strict";
import { test } from "node:test";
import { radioKeyIndex, scrollKeyDirection } from "../src/components/keyboard";

test("radio activation keys resolve the focused option", () => {
  for (const key of [" ", "Spacebar", "Enter"]) {
    assert.equal(radioKeyIndex(key, 1, 3), 1);
  }
});

test("radio arrow navigation wraps and Home/End select the boundaries", () => {
  for (const key of ["ArrowRight", "ArrowDown"]) {
    assert.equal(radioKeyIndex(key, 0, 3), 1);
    assert.equal(radioKeyIndex(key, 2, 3), 0);
  }
  for (const key of ["ArrowLeft", "ArrowUp"]) {
    assert.equal(radioKeyIndex(key, 0, 3), 2);
    assert.equal(radioKeyIndex(key, 2, 3), 1);
  }
  assert.equal(radioKeyIndex("Home", 2, 3), 0);
  assert.equal(radioKeyIndex("End", 0, 3), 2);
  assert.equal(radioKeyIndex("ArrowLeft", 0, 1), 0);
});

test("radio navigation leaves Tab and unrelated keys alone and rejects missing focus", () => {
  for (const key of ["Tab", "Escape", "PageDown", "a"]) assert.equal(radioKeyIndex(key, 1, 3), null);
  assert.equal(radioKeyIndex("ArrowRight", -1, 3), null);
  assert.equal(radioKeyIndex("Enter", 3, 3), null);
  assert.equal(radioKeyIndex("Home", 0, 0), null);
});

test("output scroll keys distinguish history from downward navigation without capturing input keys", () => {
  for (const key of ["ArrowUp", "PageUp", "Home"]) assert.equal(scrollKeyDirection(key), -1);
  for (const key of ["ArrowDown", "PageDown", "End"]) assert.equal(scrollKeyDirection(key), 1);
  assert.equal(scrollKeyDirection(" "), 1);
  assert.equal(scrollKeyDirection(" ", true), -1);
  for (const key of ["ArrowLeft", "ArrowRight", "Enter", "Tab", "a"]) assert.equal(scrollKeyDirection(key), null);
});
