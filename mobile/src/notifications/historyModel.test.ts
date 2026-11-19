import assert from "node:assert/strict";
import { test } from "node:test";
import { addEntry, MAX_ENTRIES, readEntries, unreadCount } from "./historyModel";

const alert = (id: string, at: number, title = "Claude needs you") => ({ id, title, body: "Allow npm install?", desktop: "mac", at });

test("alerts are kept newest first; the same one seen twice is one", () => {
  let list = addEntry([], alert("a", 1000));
  list = addEntry(list, alert("b", 5000, "Done"));
  assert.deepEqual(list.map((item) => item.id), ["b", "a"]);
  // Banner first, then the tap with the system id: the same alert, now read.
  list = addEntry(list, alert("sys-1", 1500), true);
  assert.equal(list.length, 2);
  assert.equal(list.find((item) => item.id === "a")?.read, true);
  assert.equal(unreadCount(list), 1);
  // Much later, the same words are a new alert.
  list = addEntry(list, alert("c", 10 * 60 * 1000));
  assert.equal(list.length, 3);
});

test("at most MAX_ENTRIES are kept, and only what reads back as entries", () => {
  let list = addEntry([], alert("0", 0, "t0"));
  for (let i = 1; i <= MAX_ENTRIES + 5; i++) list = addEntry(list, alert(String(i), i * 1_000_000, `t${i}`));
  assert.equal(list.length, MAX_ENTRIES);
  assert.equal(list[0]!.id, String(MAX_ENTRIES + 5));
  assert.deepEqual(readEntries(JSON.stringify([{ ...alert("x", 1), read: false }, { id: 3 }])).map((e) => e.id), ["x"]);
  assert.deepEqual(readEntries("not json"), []);
});
