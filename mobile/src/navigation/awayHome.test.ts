import assert from "node:assert/strict";
import { test } from "node:test";
import { AWAY_HOME_MS, shouldGoHome, staysPut } from "./awayHomeRules";

const now = 10_000_000;
test("five minutes away goes Home; less comes back where you were", () => {
  assert.equal(shouldGoHome(AWAY_HOME_MS, "/machine/abc/tab/1", 0, now), true);
  assert.equal(shouldGoHome(AWAY_HOME_MS + 60_000, "/machine/abc", 0, now), true);
  assert.equal(shouldGoHome(AWAY_HOME_MS - 1, "/machine/abc/tab/1", 0, now), false);
});

test("a notification tap decides where to go", () => {
  assert.equal(shouldGoHome(AWAY_HOME_MS * 2, "/machine/abc", now - 500, now), false);
  assert.equal(shouldGoHome(AWAY_HOME_MS * 2, "/machine/abc", now - 60_000, now), true);
});

test("Home, Settings and pairing stay put", () => {
  for (const path of ["/", "/settings", "/pair-scan", "/pair-confirm"]) {
    assert.equal(staysPut(path), true, path);
    assert.equal(shouldGoHome(AWAY_HOME_MS * 2, path, 0, now), false, path);
  }
});
