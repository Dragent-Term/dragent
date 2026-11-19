import assert from "node:assert/strict";
import { test } from "node:test";
import { addLog, formatLog, MAX_LOG, readLog } from "./connectionLogModel";

test("events are kept newest first, and a repeat straight after itself is counted", () => {
  let log = addLog([], { level: "info", desktop: "d1", text: "Connecting…", at: 1000 });
  log = addLog(log, { level: "warn", desktop: "d1", text: "Connection lost; retrying (attempt 1)", at: 2000 });
  log = addLog(log, { level: "warn", desktop: "d1", text: "Connection lost; retrying (attempt 1)", at: 3000 });
  log = addLog(log, { level: "info", desktop: "d1", text: "Connected", at: 4000 });
  assert.deepEqual(log.map((e) => [e.text, e.count]), [["Connected", 1], ["Connection lost; retrying (attempt 1)", 2], ["Connecting…", 1]]);
  assert.equal(log[1]!.last, 3000);
  assert.equal(addLog(log, { level: "info", desktop: null, text: "   ", at: 1 }).length, 3, "blank events are dropped");
});

test("at most MAX_LOG lines, and only what reads back as entries", () => {
  let log = addLog([], { level: "info", desktop: null, text: "x0", at: 0 });
  for (let i = 1; i <= MAX_LOG + 10; i++) log = addLog(log, { level: "info", desktop: null, text: `x${i}`, at: i });
  assert.equal(log.length, MAX_LOG);
  assert.deepEqual(readLog(JSON.stringify([{ at: 1, last: 1, count: 1, level: "info", desktop: null, text: "ok" }, { at: "bad" }])).length, 1);
  assert.deepEqual(readLog("{"), []);
});

test("copied as plain text, oldest first, with desktop names", () => {
  let log = addLog([], { level: "info", desktop: "d1", text: "Connecting…", at: new Date(2026, 10, 16, 9, 0, 0).getTime() });
  log = addLog(log, { level: "error", desktop: null, text: "Push registration failed", at: new Date(2026, 10, 16, 9, 0, 5).getTime() });
  const text = formatLog(log, (id) => (id === "d1" ? "MacBook" : id));
  assert.equal(text, "2026-11-16 09:00:00 INFO  MacBook: Connecting…\n2026-11-16 09:00:05 ERROR App: Push registration failed");
});
