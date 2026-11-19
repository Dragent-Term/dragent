import assert from "node:assert/strict";
import { test } from "node:test";
import { createSocketPool } from "../sharedSocketPool";

/** A pool whose timers run only when the test says so. */
function harness(lingerMs = 1000) {
  const timers = new Map<number, () => void>();
  let next = 1;
  const pool = createSocketPool<{ id: number }>({
    lingerMs,
    setTimer: (run) => {
      const id = next++;
      timers.set(id, run);
      return id;
    },
    clearTimer: (id) => timers.delete(id as number),
  });
  let built = 0;
  let closed = 0;
  const build = () => {
    built += 1;
    const id = built;
    return { client: { id }, teardown: () => { closed += 1; } };
  };
  const fire = () => {
    const pending = [...timers.values()];
    timers.clear();
    for (const run of pending) run();
  };
  return { pool, build, fire, counts: () => ({ built, closed, timers: timers.size }) };
}

test("screens showing the same desktop share one connection", () => {
  const { pool, build, counts } = harness();
  const home = pool.acquire("mac", undefined, build);
  const screen = pool.acquire("mac", undefined, build);
  assert.equal(home.client, screen.client);
  assert.deepEqual(counts(), { built: 1, closed: 0, timers: 0 });
  pool.acquire("other-mac", undefined, build);
  assert.equal(counts().built, 2);
});

test("moving between screens reuses the connection instead of reconnecting", () => {
  const { pool, build, fire, counts } = harness();
  const first = pool.acquire("mac", undefined, build);
  first.release(); // the old screen unmounts…
  const second = pool.acquire("mac", undefined, build); // …and the next one mounts
  assert.equal(second.client, first.client);
  fire();
  assert.deepEqual(counts(), { built: 1, closed: 0, timers: 0 });
});

test("the last one out closes it after the linger, and a later screen builds a new one", () => {
  const { pool, build, fire, counts } = harness();
  const only = pool.acquire("mac", undefined, build);
  only.release();
  assert.equal(counts().closed, 0, "still open while it lingers");
  assert.equal(pool.size(), 1);
  fire();
  assert.equal(counts().closed, 1);
  assert.equal(pool.size(), 0);
  const later = pool.acquire("mac", undefined, build);
  assert.notEqual(later.client, only.client);
  assert.equal(counts().built, 2);
});

test("releasing twice counts once", () => {
  const { pool, build, fire, counts } = harness();
  const a = pool.acquire("mac", undefined, build);
  pool.acquire("mac", undefined, build);
  a.release();
  a.release();
  fire();
  assert.equal(counts().closed, 0, "the other screen still holds it");
});

test("input-loss notices reach every screen that's still there", () => {
  const { pool } = harness();
  let notify: ((handles: readonly number[], message: string) => void) | null = null;
  const heard: string[] = [];
  const a = pool.acquire("mac", () => heard.push("a"), (inputLost) => {
    notify = inputLost;
    return { client: { id: 1 }, teardown: () => {} };
  });
  pool.acquire("mac", () => heard.push("b"), () => assert.fail("not rebuilt"));
  a.release();
  notify!([1], "lost");
  assert.deepEqual(heard, ["b"]);
});
